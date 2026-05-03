"""依赖解析器：递归解析包依赖树.

策略:
1. 从顶层包开始，逐层递归解析依赖
2. 每层解析该层所有包的 requires_dist
3. 合并同一依赖包的所有版本约束
4. 用约束过滤后下载满足条件的版本
5. 递归直到没有新包或达到最大深度
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from typing import Any

import requests
from packaging.requirements import Requirement
from packaging.specifiers import SpecifierSet
from packaging.version import parse as parse_version

from .filters import normalize_package_name


@dataclass(frozen=True)
class DepConstraint:
    """依赖约束."""

    name: str
    specifier: str


def _extract_extras(package_ref: str) -> tuple[str, set[str]]:
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
    requires_dist: list[str] | None, extras: set[str] | None = None,
) -> list[DepConstraint]:
    """解析 requires_dist，返回依赖约束列表."""
    if not requires_dist:
        return []

    deps: list[DepConstraint] = []
    extras = extras or set()

    for req_str in requires_dist:
        try:
            req = Requirement(req_str)
        except Exception:
            continue

        marker_str = str(req.marker) if req.marker else ""
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

        # 忽略环境 marker，尽量多包含依赖
        specifier = str(req.specifier) if req.specifier else ""
        deps.append(DepConstraint(name=req.name, specifier=specifier))

    return deps


def _get_all_versions(
    session: requests.Session, package: str, pypi_url: str,
) -> list[str]:
    """获取包的所有版本号列表（降序）."""
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
        return versions
    except requests.RequestException:
        return []


def _filter_versions(versions: list[str], specifier: str) -> list[str]:
    """用版本约束过滤版本列表."""
    if not specifier:
        return versions

    try:
        spec = SpecifierSet(specifier)
    except Exception:
        return versions

    result = []
    for v in versions:
        try:
            if parse_version(v) in spec:
                result.append(v)
        except Exception:
            continue
    return result


def _merge_specifiers(specifiers: list[str]) -> str:
    """合并多个版本约束."""
    parts = [s.strip() for s in specifiers if s.strip()]
    return ",".join(parts)


def _resolve_one_layer(
    packages: list[str],
    package_versions: dict[str, list[str]],
    pkg_extras: dict[str, set[str]],
    pypi_url: str,
    workers: int,
    session: requests.Session,
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
                deps = _parse_requires_dist(requires_dist, extras)
                for dep in deps:
                    all_constraints.setdefault(dep.name, []).append(dep.specifier)
            except Exception as e:
                print(f"  [WARN] 解析 {pkg}=={ver} 依赖失败: {e}")

    return all_constraints


def resolve_dependencies(
    top_packages: list[str],
    top_versions: dict[str, list[str]],
    pypi_url: str,
    workers: int = 8,
    max_depth: int = 5,
) -> dict[str, list[str]]:
    """递归解析依赖树，返回需要下载的依赖包版本.

    Args:
        top_packages: 顶层包列表
        top_versions: {包名: [版本]} 已下载的版本
        pypi_url: PyPI JSON API URL
        workers: 并发数
        max_depth: 最大递归深度

    Returns:
        {包名: [版本号列表]} 所有需要下载的依赖
    """
    print("解析依赖...")

    # 提取顶层包 extras
    pkg_extras: dict[str, set[str]] = {}
    for pkg_ref in top_packages:
        name, extras = _extract_extras(pkg_ref)
        if extras:
            pkg_extras.setdefault(name, set()).update(extras)

    # 已处理的包（避免重复处理和循环依赖）
    processed: set[str] = set()
    for pkg_ref in top_packages:
        processed.add(_extract_extras(pkg_ref)[0])

    # 累积所有约束
    accumulated_constraints: dict[str, list[str]] = {}

    # 当前层需要解析的包
    current_layer_packages = list(processed)
    current_layer_versions = dict(top_versions)

    with requests.Session() as session:
        for depth in range(1, max_depth + 1):
            if not current_layer_packages:
                break

            print(f"  第 {depth} 层依赖: {len(current_layer_packages)} 个包")

            constraints = _resolve_one_layer(
                current_layer_packages,
                current_layer_versions,
                pkg_extras,
                pypi_url,
                workers,
                session,
            )

            if not constraints:
                break

            # 合并到累积约束
            for dep_name, specs in constraints.items():
                accumulated_constraints.setdefault(dep_name, []).extend(specs)

            # 找出新发现的依赖包（未处理过的）
            new_packages = []
            for dep_name in constraints:
                normalized = normalize_package_name(dep_name)
                if normalized not in processed:
                    processed.add(normalized)
                    new_packages.append(dep_name)

            if not new_packages:
                print(f"  第 {depth} 层无新包，结束递归")
                break

            print(f"  [DEBUG] 第 {depth} 层新包: {new_packages}")

            # 获取新包的所有版本，用于下一轮解析
            current_layer_packages = []
            current_layer_versions = {}

            with ThreadPoolExecutor(max_workers=workers) as executor:
                futures = {
                    executor.submit(_get_all_versions, session, pkg, pypi_url): pkg
                    for pkg in new_packages
                }

                for future in as_completed(futures):
                    pkg = futures[future]
                    try:
                        versions = future.result()
                        if versions:
                            current_layer_packages.append(pkg)
                            current_layer_versions[pkg] = versions[:5]  # 只取最新5个用于解析
                    except Exception:
                        pass

            print(f"  第 {depth} 层发现 {len(current_layer_packages)} 个新包")

    if not accumulated_constraints:
        print("  未找到依赖")
        return {}

    print(f"  [DEBUG] 所有约束包名: {sorted(accumulated_constraints.keys())}")
    print(f"  总共 {len(accumulated_constraints)} 个依赖包，开始过滤版本...")

    # 用约束过滤版本，确定最终需要下载的版本
    result: dict[str, list[str]] = {}

    with requests.Session() as session:
        with ThreadPoolExecutor(max_workers=workers) as executor:
            futures = {
                executor.submit(_get_all_versions, session, dep_name, pypi_url): dep_name
                for dep_name in accumulated_constraints
            }

            for future in as_completed(futures):
                dep_name = futures[future]
                try:
                    all_versions = future.result()
                    if not all_versions:
                        continue

                    merged_spec = _merge_specifiers(accumulated_constraints[dep_name])
                    filtered = _filter_versions(all_versions, merged_spec)

                    if not merged_spec:
                        to_keep = filtered[:3]
                    elif "<" in merged_spec or "<=" in merged_spec:
                        to_keep = filtered[:10]
                    else:
                        to_keep = filtered[:3]

                    if to_keep:
                        result[dep_name] = to_keep

                except Exception as e:
                    print(f"  [WARN] 获取 {dep_name} 版本失败: {e}")

    total_versions = sum(len(v) for v in result.values())
    print(f"  依赖解析完成: {len(result)} 个包, {total_versions} 个版本")
    if result:
        print(f"  [DEBUG] 结果包名: {sorted(result.keys())}")

    return result
