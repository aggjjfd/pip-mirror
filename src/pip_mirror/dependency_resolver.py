"""依赖解析器：按 (Python 版本 × 平台) Target 解析 + 版本窗口下载.

策略:
1. 对每个 target = (py_ver, platform) 分别用 SAT 求解一个可行版本组合
2. 不同 target 的约束互不干扰,避免 AND 合并导致的矛盾
3. 每个 target 的解中,每个依赖只保留一个版本
4. 最终对所有 target 的解取版本窗口并集作为下载清单
"""

from __future__ import annotations

import logging
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from typing import Any

import pycosat
import requests
from packaging.requirements import Requirement
from packaging.specifiers import SpecifierSet
from packaging.version import parse as parse_version

from ._session import make_session
from .filters import normalize_package_name

logger = logging.getLogger("pip-mirror")


@dataclass(frozen=True)
class TargetEnv:
    """目标运行环境 (Python 版本 × 平台)."""

    python_version: str       # e.g. "3.12"
    python_full_version: str  # e.g. "3.12.0"
    sys_platform: str         # e.g. "linux", "win32"
    platform_machine: str     # e.g. "x86_64", "AMD64"


@dataclass(frozen=True)
class DepConstraint:
    """依赖约束."""

    name: str
    specifier: str


# 21 个 target: 7 Python 版本 × 3 平台
_TARGET_ENVS: list[TargetEnv] = [
    TargetEnv("3.8",  "3.8.0",  "linux",   "x86_64"),
    TargetEnv("3.9",  "3.9.0",  "linux",   "x86_64"),
    TargetEnv("3.10", "3.10.0", "linux",   "x86_64"),
    TargetEnv("3.11", "3.11.0", "linux",   "x86_64"),
    TargetEnv("3.12", "3.12.0", "linux",   "x86_64"),
    TargetEnv("3.13", "3.13.0", "linux",   "x86_64"),
    TargetEnv("3.14", "3.14.0", "linux",   "x86_64"),
    TargetEnv("3.8",  "3.8.0",  "win32",   "x86"),
    TargetEnv("3.9",  "3.9.0",  "win32",   "x86"),
    TargetEnv("3.10", "3.10.0", "win32",   "x86"),
    TargetEnv("3.11", "3.11.0", "win32",   "x86"),
    TargetEnv("3.12", "3.12.0", "win32",   "x86"),
    TargetEnv("3.13", "3.13.0", "win32",   "x86"),
    TargetEnv("3.14", "3.14.0", "win32",   "x86"),
    TargetEnv("3.8",  "3.8.0",  "win32",   "AMD64"),
    TargetEnv("3.9",  "3.9.0",  "win32",   "AMD64"),
    TargetEnv("3.10", "3.10.0", "win32",   "AMD64"),
    TargetEnv("3.11", "3.11.0", "win32",   "AMD64"),
    TargetEnv("3.12", "3.12.0", "win32",   "AMD64"),
    TargetEnv("3.13", "3.13.0", "win32",   "AMD64"),
    TargetEnv("3.14", "3.14.0", "win32",   "AMD64"),
]


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
    target: TargetEnv | None = None,
) -> list[DepConstraint]:
    """解析 requires_dist，返回依赖约束列表.

    Args:
        requires_dist: PyPI JSON API 的 requires_dist 列表.
        extras: 当前包激活的 extra 集合.
        target: 目标运行环境,用于过滤 python_version / sys_platform / platform_machine marker.
                None 表示不过滤,保留所有约束(旧行为).
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

        # 按环境 marker 过滤: 只保留匹配目标运行环境的约束.
        if marker is not None:
            env: dict[str, str] = {}
            if target is not None:
                if "python_version" in marker_str:
                    env["python_version"] = target.python_version
                    env["python_full_version"] = target.python_full_version
                if "sys_platform" in marker_str:
                    env["sys_platform"] = target.sys_platform
                if "platform_machine" in marker_str:
                    env["platform_machine"] = target.platform_machine
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
        if part.startswith("==") and "*" not in part:
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


def _resolve_one_target_sat(
    top_packages: list[str],
    top_versions: dict[str, list[str]],
    pkg_extras: dict[str, set[str]],
    target: TargetEnv,
    pypi_url: str,
    session: requests.Session,
    max_depth: int = 5,
    allow_prerelease: bool = False,
) -> dict[str, str]:
    """对单个 target 用 SAT 求解一个可行解.

    Returns:
        {package_name: chosen_version}
    """
    # 缓存
    version_info_cache: dict[tuple[str, str], dict[str, Any] | None] = {}
    all_versions_cache: dict[str, list[str]] = {}

    # ---------- 1. BFS 收集所有相关包 ----------
    packages_to_resolve: set[str] = set()
    queue: list[tuple[str, int]] = []

    for pkg_ref in top_packages:
        name, _ = extract_extras(pkg_ref)
        if name not in packages_to_resolve:
            packages_to_resolve.add(name)
            queue.append((name, 0))

    while queue:
        pkg_name, depth = queue.pop(0)
        if depth >= max_depth:
            continue

        if pkg_name not in all_versions_cache:
            all_versions_cache[pkg_name] = _get_all_versions(
                session, pkg_name, pypi_url, allow_prerelease,
            )

        versions = all_versions_cache[pkg_name]
        if not versions:
            continue

        # 用哪些版本探索依赖树
        if pkg_name in top_versions:
            explore_versions = top_versions[pkg_name]
        else:
            explore_versions = versions[:3]

        for ver in explore_versions:
            key = (pkg_name, ver)
            if key not in version_info_cache:
                version_info_cache[key] = _get_version_info(
                    session, pkg_name, ver, pypi_url,
                )
            info = version_info_cache[key]
            if not info:
                continue

            requires_dist = info.get("info", {}).get("requires_dist")
            extras = pkg_extras.get(pkg_name, set())
            deps = _parse_requires_dist(requires_dist, extras, target)

            for dep in deps:
                dep_norm = normalize_package_name(dep.name)
                if dep_norm not in packages_to_resolve:
                    packages_to_resolve.add(dep_norm)
                    queue.append((dep.name, depth + 1))

    # ---------- 2. 为所有相关包获取全部版本 ----------
    for pkg in list(packages_to_resolve):
        if pkg not in all_versions_cache:
            all_versions_cache[pkg] = _get_all_versions(
                session, pkg, pypi_url, allow_prerelease,
            )

    # 过滤掉没有版本的包
    packages_to_resolve = {
        pkg for pkg in packages_to_resolve
        if all_versions_cache.get(pkg)
    }

    if not packages_to_resolve:
        return {}

    # ---------- 3. 确定每个包参与 SAT 的版本 ----------
    # 顶层包用 caller 指定的全部版本;
    # 依赖包限制为最新 10 个版本,避免变量爆炸.
    pkg_sat_versions: dict[str, list[str]] = {}
    for pkg in packages_to_resolve:
        all_vers = all_versions_cache[pkg]
        if pkg in top_versions:
            pkg_sat_versions[pkg] = top_versions[pkg]
        else:
            pkg_sat_versions[pkg] = all_vers[:10]

    # ---------- 4. 变量映射 ----------
    var_id: dict[tuple[str, str], int] = {}
    id_to_pkg_ver: dict[int, tuple[str, str]] = {}
    next_id = 1

    for pkg in packages_to_resolve:
        for ver in pkg_sat_versions[pkg]:
            var_id[(pkg, ver)] = next_id
            id_to_pkg_ver[next_id] = (pkg, ver)
            next_id += 1

    # ---------- 5. 编码 CNF ----------
    clauses: list[list[int]] = []

    # 每个包恰好一个版本
    for pkg in packages_to_resolve:
        versions = pkg_sat_versions[pkg]
        # 至少一个
        clauses.append([var_id[(pkg, v)] for v in versions])
        # 最多一个（两两互斥）
        n = len(versions)
        for i in range(n):
            for j in range(i + 1, n):
                clauses.append([-var_id[(pkg, versions[i])], -var_id[(pkg, versions[j])]])

    # requires_dist 蕴含约束
    for pkg in packages_to_resolve:
        for ver in pkg_sat_versions[pkg]:
            key = (pkg, ver)
            info = version_info_cache.get(key)
            if info is None:
                info = _get_version_info(session, pkg, ver, pypi_url)
                version_info_cache[key] = info
            if not info:
                continue

            requires_dist = info.get("info", {}).get("requires_dist")
            extras = pkg_extras.get(pkg, set())
            deps = _parse_requires_dist(requires_dist, extras, target)

            for dep in deps:
                dep_norm = normalize_package_name(dep.name)
                if dep_norm not in packages_to_resolve:
                    continue

                dep_versions = pkg_sat_versions.get(dep_norm, [])
                spec = SpecifierSet(dep.specifier) if dep.specifier else SpecifierSet("")
                valid_versions = []
                for dv in dep_versions:
                    try:
                        if parse_version(dv) in spec:
                            valid_versions.append(dv)
                    except Exception:
                        pass

                if valid_versions:
                    # x_{pkg,ver} -> OR(x_{dep,dv} for dv in valid_versions)
                    clause = [-var_id[(pkg, ver)]]
                    for dv in valid_versions:
                        clause.append(var_id[(dep_norm, dv)])
                    clauses.append(clause)

    # ---------- 6. SAT 求解 ----------
    # 顶层包版本固定(硬子句)
    fixed_clauses = list(clauses)
    for pkg_ref in top_packages:
        name, _ = extract_extras(pkg_ref)
        if name in top_versions and top_versions[name]:
            top_ver = top_versions[name][0]
            if (name, top_ver) in var_id:
                fixed_clauses.append([var_id[(name, top_ver)]])

    # 解码辅助
    def _decode(solution: list[int]) -> dict[str, str]:
        chosen: dict[str, str] = {}
        for lit in solution:
            if lit > 0:
                pkg, ver = id_to_pkg_ver[lit]
                chosen[pkg] = ver
        return chosen

    # 先找任意可行解
    sol = pycosat.solve(fixed_clauses)
    if sol == "UNSAT":
        logger.warning(
            f"  target {target.python_version}/{target.sys_platform} "
            f"({target.platform_machine}) UNSAT",
        )
        return {}

    chosen = _decode(sol)

    # iterative strengthening: 尝试把每个包升到更高版本
    improved = True
    while improved:
        improved = False
        for pkg in packages_to_resolve:
            if pkg not in chosen:
                continue
            current_ver = chosen[pkg]
            versions = pkg_sat_versions[pkg]
            try:
                current_idx = versions.index(current_ver)
            except ValueError:
                continue

            # 尝试更高版本
            for better_ver in versions[:current_idx]:
                # 强制选 better_ver: 排除所有比它旧的版本
                worse_versions = versions[versions.index(better_ver) + 1:]
                temp_clauses = fixed_clauses + [
                    [-var_id[(pkg, v)]] for v in worse_versions
                ]
                new_sol = pycosat.solve(temp_clauses)
                if new_sol != "UNSAT":
                    chosen = _decode(new_sol)
                    improved = True
                    break
            if improved:
                break

    return chosen


def _compute_version_windows(
    target_solutions: list[dict[str, str]],
    all_versions: dict[str, list[str]],
    max_versions: int,
) -> dict[str, list[str]]:
    """把多个 target 的解合并成每个包的下载版本列表.

    对每个包:
    1. 收集所有 target 中该包的 solution versions(去重).
    2. 对每个 solution version,在 all_versions[pkg] 中找到它的索引.
    3. 取 [idx - N//2, idx + N//2] 窗口内的版本(含自身).
    4. 所有窗口取并集、去重、保持降序.
    """
    if not target_solutions:
        return {}

    half = max_versions // 2
    result: dict[str, set[str]] = {}

    for sol in target_solutions:
        for pkg, ver in sol.items():
            pkg_versions = all_versions.get(pkg, [])
            if not pkg_versions:
                continue
            if ver not in pkg_versions:
                # solution version 不在可用版本列表中(理论上不应发生)
                result.setdefault(pkg, set()).add(ver)
                continue

            idx = pkg_versions.index(ver)
            start = max(0, idx - half)
            end = min(len(pkg_versions), idx + half + 1)
            window = pkg_versions[start:end]
            result.setdefault(pkg, set()).update(window)

    # 保持降序,去重
    final: dict[str, list[str]] = {}
    for pkg, ver_set in result.items():
        pkg_versions = all_versions.get(pkg, [])
        ordered = [v for v in pkg_versions if v in ver_set]
        if not ordered:
            ordered = sorted(ver_set, key=parse_version, reverse=True)
        final[pkg] = ordered

    return final


def resolve_dependencies(
    top_packages: list[str],
    top_versions: dict[str, list[str]],
    pypi_url: str,
    workers: int = 8,
    max_depth: int = 5,
    max_versions: int = 5,
    allow_prerelease: bool = False,
) -> dict[str, list[str]]:
    """对每个 target 分别解析一个可行解,合并版本窗口后返回下载清单.

    Returns:
        {package_name: [version1, version2, ...]} 降序
    """
    logger.info("解析依赖...")

    # 提取 extras
    pkg_extras: dict[str, set[str]] = {}
    for pkg_ref in top_packages:
        name, extras = extract_extras(pkg_ref)
        if extras:
            pkg_extras[name] = extras

    target_solutions: list[dict[str, str]] = []

    with make_session() as session:
        # 并发对 21 个 target 调用 SAT 求解
        with ThreadPoolExecutor(max_workers=workers) as executor:
            futures = {
                executor.submit(
                    _resolve_one_target_sat,
                    top_packages,
                    top_versions,
                    pkg_extras,
                    target,
                    pypi_url,
                    session,
                    max_depth,
                    allow_prerelease,
                ): target
                for target in _TARGET_ENVS
            }

            for future in as_completed(futures):
                target = futures[future]
                try:
                    sol = future.result()
                    if sol:
                        target_solutions.append(sol)
                        logger.debug(
                            f"  target {target.python_version}/{target.sys_platform} "
                            f"解: {len(sol)} 个包",
                        )
                except Exception as e:
                    logger.warning(
                        f"  target {target.python_version}/{target.sys_platform} "
                        f"解析失败: {e}",
                    )

    if not target_solutions:
        logger.warning("所有 target 均无有效解")
        return {}

    # 收集所有出现在任一解中的包
    all_pkgs: set[str] = set()
    for sol in target_solutions:
        all_pkgs.update(sol.keys())

    # 去重顶层包(它们由 caller 单独处理)
    top_names = {extract_extras(p)[0] for p in top_packages}
    dep_pkgs = all_pkgs - top_names

    if not dep_pkgs:
        logger.info("  无依赖需要下载")
        return {}

    # 获取所有依赖包的全部版本
    all_versions: dict[str, list[str]] = {}
    with make_session() as session:
        with ThreadPoolExecutor(max_workers=workers) as executor:
            futures = {
                executor.submit(
                    _get_all_versions, session, pkg, pypi_url, allow_prerelease,
                ): pkg
                for pkg in dep_pkgs
            }
            for future in as_completed(futures):
                pkg = futures[future]
                try:
                    versions = future.result()
                    if versions:
                        all_versions[pkg] = versions
                except Exception as e:
                    logger.warning(f"获取 {pkg} 版本列表失败: {e}")

    # 计算版本窗口
    dep_versions = _compute_version_windows(target_solutions, all_versions, max_versions)

    # 截断保护: 单个包版本数过多时截断
    for pkg in list(dep_versions.keys()):
        versions = dep_versions[pkg]
        if len(versions) > 20:
            logger.warning(
                f"  ! {pkg}: 窗口合并后版本数过多 ({len(versions)}), "
                f"截断到最新 20 个",
            )
            dep_versions[pkg] = versions[:20]

    total_versions = sum(len(v) for v in dep_versions.values())
    logger.info(
        f"  依赖解析完成: {len(dep_versions)} 个包, {total_versions} 个版本 "
        f"(来自 {len(target_solutions)} 个 target 的解)",
    )

    return dep_versions
