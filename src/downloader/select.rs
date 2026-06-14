use std::collections::{BTreeMap, HashSet};

use super::FileInfo;
use crate::filters::{
    is_accepted_wheel, is_source_distribution, sdist_fallback_allowed,
};

fn filter_stable_versions(files: &[FileInfo]) -> Vec<FileInfo> {
    let stable: Vec<_> = files
        .iter()
        .filter(|f| {
            f.version
                .parse::<pep440_rs::Version>()
                .map(|v| !v.any_prerelease())
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    if !stable.is_empty() {
        return stable;
    }
    let pkg = files
        .first()
        .map(|f| f.package_name.as_str())
        .unwrap_or("?");
    let n = files
        .iter()
        .map(|f| &f.version)
        .collect::<HashSet<_>>()
        .len();
    tracing::warn!("  ! {pkg} 仅有预发行版 ({n} 个版本), 回退保留全部");
    files.to_vec()
}

fn group_by_version(
    files: Vec<FileInfo>,
) -> BTreeMap<pep440_rs::Version, Vec<FileInfo>> {
    let mut by_ver: BTreeMap<pep440_rs::Version, Vec<FileInfo>> =
        BTreeMap::new();
    for fi in files {
        if let Ok(v) = fi.version.parse::<pep440_rs::Version>() {
            by_ver.entry(v).or_default().push(fi);
        }
    }
    by_ver
}

/// Select the latest `max_versions` versions from a file list.
/// When `allow_prerelease` is false, prerelease versions are dropped.
/// If that leaves nothing, fall back to the original list with a warning.
pub fn select_latest_versions(
    files: &[FileInfo],
    max_versions: usize,
    allow_prerelease: bool,
) -> Vec<FileInfo> {
    if max_versions == 0 {
        return files.to_vec();
    }
    let candidates = if allow_prerelease {
        files.to_vec()
    } else {
        filter_stable_versions(files)
    };
    let by_ver = group_by_version(candidates);
    by_ver
        .into_iter()
        .rev()
        .take(max_versions)
        .flat_map(|(_, f)| f)
        .collect()
}

fn collect_wheels(files: &[FileInfo]) -> (Vec<FileInfo>, HashSet<String>) {
    let mut whl_versions = HashSet::new();
    let mut result = Vec::with_capacity(files.len());
    for fi in files {
        if fi.filename.ends_with(".whl") && is_accepted_wheel(&fi.filename) {
            whl_versions.insert(fi.version.clone());
            result.push(fi.clone());
        }
    }
    (result, whl_versions)
}

fn collect_sdists(
    files: &[FileInfo],
    whl_versions: &HashSet<String>,
) -> Vec<FileInfo> {
    let mut result = Vec::new();
    for fi in files {
        let is_sdist = is_source_distribution(&fi.filename);
        let no_wheel = !whl_versions.contains(&fi.version);
        if is_sdist && no_wheel {
            result.push(fi.clone());
        }
    }
    result
}

pub fn collect_version_files(files: &[FileInfo]) -> Vec<FileInfo> {
    let (mut result, whl_versions) = collect_wheels(files);
    if sdist_fallback_allowed(files, true) {
        result.extend(collect_sdists(files, &whl_versions));
    }
    result
}
