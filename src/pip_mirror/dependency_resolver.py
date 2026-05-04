"""依赖解析器：递归解析包依赖树.

策略:
1. 从顶层包开始，逐层递归解析依赖
2. 每层解析该层所有包的 requires_dist
3. 合并同一依赖包的所有版本约束
4. 用约束过滤后下载满足条件的版本
5. 递归直到没有新包或达到最大深度
"""

from __future__ import annotations

import logging
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from typing import Any

import requests
from packaging.requirements import Requirement
from packaging.specifiers import SpecifierSet
from packaging.version import parse as parse_version

from ._session import make_session
from .filters import normalize_package_name

logger = logging.getLogger("pip-mirror")


@dataclass(frozen=True)
class DepConstraint:
    """依赖约束."""

    name: str
    specifier: str


@dataclass(frozen=True)
class ResolvedDep:
    """resolver 输出:某个依赖包过滤后的版本 + 合并约束 + 全集.

    `merged_spec` 透传给 downloader,backfill 阶段过滤候选老版本时使用;
    `all_versions` 提供给上层(如诊断/调试),与 versions 一致都来自 PyPI.
    """

    versions: list[str]
    merged_spec: str
    all_versions: list[str]


def extract_extras(package_ref: str) -> tuple[str, set[str]]:
    """从包引用中提取包名和 extras."""
    if "[" not in package_ref:
        return package_ref, set()

    name_part = package_ref.split("[")[0]
    extras_part = package_ref.split("[")[1].rstrip("]")
    extras = {e.strip() for e in extras_part.split(",") if e.strip()}
    return name_part, extras


def _get_version_info(
    session: requests.Session, package: str, version: str, pypi_url: str,
) -> dict[str, Any] | None:
    """获取特定版本的 info（包含 requires_dist）."""
    normalized = normalize_package_name(package)
    url = f"{pypi_url.rstrip('/')}/pypi/{normalized}/{version}/json"
    try:
        resp = session.get(url, timeout=30)
        resp.raise_for_status()
        return resp.json()
    except requests.RequestException:
        return None


def _parse_requires_dist(
    requires_dist: list[str] | None,
    extras: set[str] | None = None,
    python_version: str | None = None,
) -> list[DepConstraint]:
    """解析 requires_dist，返回依赖约束列表.

    Args:
        requires_dist: PyPI JSON API 的 requires_dist 列表.
        extras: 当前包激活的 extra 集合.
        python_version: 目标 Python 版本(如 "3.12")，用于过滤 python_version marker.
                       None 表示不过滤，保留所有约束(旧行为).
    """
    if not requires_dist:
        return []

    deps: list[DepConstraint] = []
    extras = extras or set()

    for req_str in requires_dist:
        try:
            req = Requirement(req_str)
        except Exception:
            continue

        marker = req.marker
        marker_str = str(marker) if marker else ""
        marker_lower = marker_str.lower()

        # 跳过 extra 不匹配的依赖
        if "extra" in marker_lower:
            if not extras:
                continue
            has_match = False
            for extra in extras:
                if f'"{extra}"' in marker_str or f"'{extra}'" in marker_str:
                    has_match = True
                    break
            if not has_match:
                continue

        # 按环境 marker 过滤: 只保留匹配当前运行环境/目标 Python 版本的约束.
        # mirror 在 Linux 上构建,sys_platform=="win32" 的约束会被跳过;
        # 这是设计取舍:跨平台版本约束矛盾极少见,避免 AND 合并后无解.
        if marker is not None:
            env: dict[str, str] = {}
            if python_version is not None and "python_version" in marker_str:
                env["python_version"] = python_version
                env["python_full_version"] = python_version + ".0"
            if "sys_platform" in marker_str:
                env["sys_platform"] = sys.platform
            if "platform_machine" in marker_str:
                import platform
                env["platform_machine"] = platform.machine()
            if env:
                try:
                    if not marker.evaluate(env):
                        continue
                except Exception:
                    # evaluate 失败时保守保留
                    pass

        specifier = str(req.specifier) if req.specifier else ""
        deps.append(DepConstraint(name=req.name, specifier=specifier))

    return deps


def _get_all_versions(
    session: requests.Session,
    package: str,
    pypi_url: str,
    allow_prerelease: bool = False,
) -> list[str]:
    """获取包的所有版本号列表（降序）.

    默认过滤掉预发行版（rc / alpha / beta / dev）;
    将 allow_prerelease 设为 True 可保留全部版本.

    若过滤后为空但原列表非空（包仅有预发行版）, 回退保留全部并打 WARNING 日志,
    不静默丢弃.
    """
    normalized = normalize_package_name(package)
    url = f"{pypi_url.rstrip('/')}/pypi/{normalized}/json"
    try:
        resp = session.get(url, timeout=30)
        resp.raise_for_status()
        data = resp.json()
        versions = list(data.get("releases", {}).keys())
        try:
            versions.sort(key=parse_version, reverse=True)
        except Exception:
            versions.sort(reverse=True)
        if not allow_prerelease and versions:
            kept: list[str] = []
            for v in versions:
                try:
                    if parse_version(v).is_prerelease:
                        continue
                except Exception:
                    pass
                kept.append(v)
            dropped = len(versions) - len(kept)
            if not kept:
                logger.warning(
                    f"  ! {package} 仅有预发行版 ({len(versions)} 个), "
                    "回退保留全部版本",
                )
            else:
                if dropped:
                    logger.debug(f"  {package}: 过滤掉 {dropped} 个预发行版")
                versions = kept
        if not versions:
            logger.warning(f"PyPI 返回空 releases: {package} ({url})")
        return versions
    except requests.RequestException as e:
        logger.warning(f"获取 {package} 版本列表失败 ({url}): {e}")
        return []


def _filter_versions(versions: list[str], specifier: str) -> list[str]:
    """用版本约束过滤版本列表.

    精确版本约束 (==x.y.z) 按 OR 关系处理：满足任意一个即可。
    范围约束按 AND 关系处理：必须同时满足所有范围约束。
    """
    if not specifier:
        return versions

    # 分离精确版本约束和范围约束
    exact_versions: set[str] = set()
    range_parts: list[str] = []

    for part in specifier.split(","):
        part = part.strip()
        if part.startswith("=="):
            exact_versions.add(part[2:].strip())
        elif part:
            range_parts.append(part)

    # 如果有精确版本约束，直接匹配这些版本（OR 关系）
    if exact_versions:
        result = []
        for v in versions:
            if v in exact_versions:
                result.append(v)
        return result

    # 否则用范围约束过滤（AND 关系）
    if range_parts:
        try:
            spec = SpecifierSet(",".join(range_parts))
            result = []
            for v in versions:
                try:
                    if parse_version(v) in spec:
                        result.append(v)
                except Exception:
                    continue
            return result
        except Exception:
            return versions

    return versions


def _resolve_one_layer(
    packages: list[str],
    package_versions: dict[str, list[str]],
    pkg_extras: dict[str, set[str]],
    pypi_url: str,
    workers: int,
    session: requests.Session,
    python_version: str | None = None,
) -> dict[str, list[str]]:
    """解析一层的依赖约束.

    Returns:
        {dep_name: [specifier1, specifier2, ...]}
    """
    all_constraints: dict[str, list[str]] = {}

    tasks = []
    for pkg_name in packages:
        extras = pkg_extras.get(pkg_name, set())
        versions = package_versions.get(pkg_name, [])
        for version in versions:
            tasks.append((pkg_name, version, extras))

    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {
            executor.submit(_get_version_info, session, pkg, ver, pypi_url): (pkg, ver, extras)
            for pkg, ver, extras in tasks
        }

        for future in as_completed(futures):
            pkg, ver, extras = futures[future]
            try:
                info = future.result()
                if not info:
                    continue
                requires_dist = info.get("info", {}).get("requires_dist")
                deps = _parse_requires_dist(requires_dist, extras, python_version)
                for dep in deps:
                    all_constraints.setdefault(dep.name, []).append(dep.specifier)
            except Exception as e:
                logger.warning(f"解析 {pkg}=={ver} 依赖失败: {e}")

    return all_constraints


# mirror 需要服务的 Python 版本范围(对应 pyproject.toml requires-python ">=3.8")
_TARGET_PYTHON_VERSIONS = ["3.8", "3.9", "3.10", "3.11", "3.12", "3.13", "3.14"]


def _resolve_single_tree(
    top_package: str,
    top_versions: dict[str, list[str]],
    pkg_extras: dict[str, set[str]],
    pypi_url: str,
    workers: int,
    max_depth: int,
    allow_prerelease: bool,
    python_version: str | None = None,
) -> dict[str, ResolvedDep]:
    """解析单个顶层包的完整依赖树,约束只在树内累积,不跨树污染."""
    processed: set[str] = {top_package}
    tree_pkg_extras = dict(pkg_extras)
    accumulated_constraints: dict[str, list[str]] = {}

    current_layer_packages = [top_package]
    current_layer_versions = {top_package: top_versions.get(top_package, [])}

    with make_session() as session:
        for depth in range(1, max_depth + 1):
            if not current_layer_packages:
                break

            py_tag = f"(py{python_version}) " if python_version else ""
            logger.info(
                f"  [{top_package}] {py_tag}第 {depth} 层: {len(current_layer_packages)} 个包"
            )

            constraints = _resolve_one_layer(
                current_layer_packages,
                current_layer_versions,
                tree_pkg_extras,
                pypi_url,
                workers,
                session,
                python_version,
            )

            if not constraints:
                break

            for dep_name, specs in constraints.items():
                accumulated_constraints.setdefault(dep_name, []).extend(specs)

            new_packages = []
            for dep_name in constraints:
                normalized = normalize_package_name(dep_name)
                if normalized not in processed:
                    processed.add(normalized)
                    new_packages.append(dep_name)

            if not new_packages:
                logger.info(f"  [{top_package}] {py_tag}第 {depth} 层无新包")
                break

            current_layer_packages = []
            current_layer_versions = {}

            with ThreadPoolExecutor(max_workers=workers) as executor:
                futures = {
                    executor.submit(
                        _get_all_versions, session, pkg, pypi_url, allow_prerelease,
                    ): pkg
                    for pkg in new_packages
                }

                for future in as_completed(futures):
                    pkg = futures[future]
                    try:
                        versions = future.result()
                        if versions:
                            current_layer_packages.append(pkg)
                            current_layer_versions[pkg] = versions[:5]
                    except Exception:
                        pass

            logger.info(
                f"  [{top_package}] {py_tag}第 {depth} 层发现 {len(current_layer_packages)} 个新包"
            )

    if not accumulated_constraints:
        return {}

    # 过滤版本
    result: dict[str, ResolvedDep] = {}
    missing: list[str] = []

    with make_session() as session:
        with ThreadPoolExecutor(max_workers=workers) as executor:
            futures = {
                executor.submit(
                    _get_all_versions, session, dep_name, pypi_url, allow_prerelease,
                ): dep_name
                for dep_name in accumulated_constraints
            }

            for future in as_completed(futures):
                dep_name = futures[future]
                try:
                    all_versions = future.result()
                    if not all_versions:
                        missing.append(dep_name)
                        continue

                    merged_spec = ",".join(
                        s.strip() for s in accumulated_constraints[dep_name] if s.strip()
                    )
                    filtered = _filter_versions(all_versions, merged_spec)

                    limit = 10 if "<" in merged_spec else 3
                    to_keep = filtered[:limit]

                    if not to_keep:
                        logger.warning(
                            f"  ! [{top_package}] {dep_name}: "
                            f"spec='{merged_spec or '(empty)'}', "
                            f"all={len(all_versions)}, filtered={len(filtered)}, "
                            f"limit={limit}, kept={len(to_keep)}"
                        )

                    if to_keep:
                        result[dep_name] = ResolvedDep(
                            versions=to_keep,
                            merged_spec=merged_spec,
                            all_versions=all_versions,
                        )
                    else:
                        missing.append(dep_name)

                except Exception as e:
                    logger.warning(f"获取 {dep_name} 版本失败: {e}")
                    missing.append(dep_name)

    return result


def resolve_dependencies(
    top_packages: list[str],
    top_versions: dict[str, list[str]],
    pypi_url: str,
    workers: int = 8,
    max_depth: int = 5,
    allow_prerelease: bool = False,
) -> dict[str, ResolvedDep]:
    """对每个顶层包 × 每个 Python 版本分别解析,最后合并去重.

    不同 Python 版本的路径独立解析,避免 python_version marker 被错误 AND 合并;
    多棵树之间不交叉污染约束;同一依赖在不同树/版本里版本不同时取并集.
    """
    logger.info("解析依赖...")

    all_results: dict[str, ResolvedDep] = {}

    for pkg_ref in top_packages:
        name, extras = extract_extras(pkg_ref)
        tree_extras: dict[str, set[str]] = {}
        if extras:
            tree_extras[name] = extras

        for py_ver in _TARGET_PYTHON_VERSIONS:
            tree_result = _resolve_single_tree(
                top_package=name,
                top_versions=top_versions,
                pkg_extras=tree_extras,
                pypi_url=pypi_url,
                workers=workers,
                max_depth=max_depth,
                allow_prerelease=allow_prerelease,
                python_version=py_ver,
            )

            for dep_name, dep in tree_result.items():
                if dep_name in all_results:
                    existing = all_results[dep_name]
                    seen = set(existing.versions)
                    merged = list(existing.versions)
                    for v in dep.versions:
                        if v not in seen:
                            seen.add(v)
                            merged.append(v)
                    try:
                        merged.sort(key=parse_version, reverse=True)
                    except Exception:
                        merged.sort(reverse=True)
                    unique: list[str] = []
                    seen2: set[str] = set()
                    for v in merged:
                        if v not in seen2:
                            seen2.add(v)
                            unique.append(v)
                    all_results[dep_name] = ResolvedDep(
                        versions=unique,
                        merged_spec="",
                        all_versions=dep.all_versions,
                    )
                else:
                    all_results[dep_name] = dep

    total_versions = sum(len(r.versions) for r in all_results.values())
    logger.info(f"  依赖解析完成: {len(all_results)} 个包, {total_versions} 个版本")

    return all_results
