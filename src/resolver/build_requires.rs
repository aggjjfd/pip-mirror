use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

use futures::{StreamExt, stream};
use pep440_rs::Version;

use crate::downloader::DownloadableItem;
use crate::http::HttpClient;

use super::error::ResolveError;
use super::metadata::MetadataCache;
use super::metadata_types::MetadataError;

const PYPROJECT_FILE_NAME: &str = "pyproject.toml";
const SDIST_KIND: &str = "sdist";

#[derive(Debug, Clone)]
pub struct PrefetchedSdist {
    pub filename: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BuildRequiresProbe {
    #[allow(dead_code)]
    pub requirements: Vec<String>,
    pub prefetched_sdist: Option<PrefetchedSdist>,
}

struct SdistCandidate {
    filename: String,
    url: String,
}

pub async fn probe_build_requires_from_version_json(
    client: &HttpClient,
    version_json: &serde_json::Value,
) -> Result<BuildRequiresProbe, String> {
    let Some(candidate) = find_sdist_candidate(version_json) else {
        return Ok(empty_probe());
    };
    let bytes = download_sdist(client, &candidate.url).await?;
    let prefetched = PrefetchedSdist {
        filename: candidate.filename.clone(),
        bytes,
    };
    let Some(pyproject) =
        extract_pyproject_from_sdist(&prefetched.bytes, &candidate.filename)?
    else {
        return Ok(BuildRequiresProbe {
            requirements: Vec::new(),
            prefetched_sdist: Some(prefetched),
        });
    };
    let requirements = parse_build_requires_from_pyproject(&pyproject)?;
    Ok(BuildRequiresProbe {
        requirements,
        prefetched_sdist: Some(prefetched),
    })
}

fn empty_probe() -> BuildRequiresProbe {
    BuildRequiresProbe {
        requirements: Vec::new(),
        prefetched_sdist: None,
    }
}

fn find_sdist_candidate(
    version_json: &serde_json::Value,
) -> Option<SdistCandidate> {
    let urls = version_json.get("urls")?.as_array()?;
    for entry in urls {
        let kind = entry.get("packagetype").and_then(|v| v.as_str());
        if kind != Some(SDIST_KIND) {
            continue;
        }
        let filename = entry.get("filename").and_then(|v| v.as_str())?;
        let url = entry.get("url").and_then(|v| v.as_str())?;
        return Some(SdistCandidate {
            filename: filename.to_string(),
            url: url.to_string(),
        });
    }
    None
}

pub async fn download_sdist(
    client: &HttpClient,
    url: &str,
) -> Result<Vec<u8>, String> {
    client
        .get_bytes(url)
        .await
        .map_err(|e| format!("下载源码包失败: {e}"))
}

fn extract_pyproject_from_sdist(
    bytes: &[u8],
    filename: &str,
) -> Result<Option<String>, String> {
    if filename.ends_with(".zip") {
        return extract_pyproject_from_zip(bytes);
    }
    if filename.ends_with(".tar.gz")
        || filename.ends_with(".tgz")
        || filename.ends_with(".tar")
    {
        return extract_pyproject_from_tar(bytes, filename);
    }
    Err(format!("不支持的源码包格式: {filename}"))
}

fn extract_pyproject_from_zip(bytes: &[u8]) -> Result<Option<String>, String> {
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| format!("zip 解包失败: {e}"))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| format!("zip 读取条目失败: {e}"))?;
        if !is_pyproject_entry(file.name()) {
            continue;
        }
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| format!("读取 pyproject.toml 失败: {e}"))?;
        return Ok(Some(content));
    }
    Ok(None)
}

fn extract_pyproject_from_tar(
    bytes: &[u8],
    filename: &str,
) -> Result<Option<String>, String> {
    let cursor = Cursor::new(bytes);
    if filename.ends_with(".tar") {
        return extract_pyproject_from_tar_archive(tar::Archive::new(cursor));
    }
    let decoder = flate2::read::GzDecoder::new(cursor);
    extract_pyproject_from_tar_archive(tar::Archive::new(decoder))
}

fn extract_pyproject_from_tar_archive<R>(
    mut archive: tar::Archive<R>,
) -> Result<Option<String>, String>
where
    R: Read,
{
    let entries = archive
        .entries()
        .map_err(|e| format!("tar 读取条目失败: {e}"))?;
    for entry in entries {
        let mut file = entry.map_err(|e| format!("tar 条目读取失败: {e}"))?;
        let path = file.path().map_err(|e| format!("tar 路径读取失败: {e}"))?;
        if !is_pyproject_entry(
            path.as_ref().as_os_str().to_string_lossy().as_ref(),
        ) {
            continue;
        }
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| format!("读取 pyproject.toml 失败: {e}"))?;
        return Ok(Some(content));
    }
    Ok(None)
}

fn is_pyproject_entry(path: &str) -> bool {
    path.rsplit(['/', '\\']).next() == Some(PYPROJECT_FILE_NAME)
}

fn parse_build_requires_from_pyproject(
    pyproject_text: &str,
) -> Result<Vec<String>, String> {
    let value: toml::Value = pyproject_text
        .parse()
        .map_err(|e| format!("解析 pyproject.toml 失败: {e}"))?;
    let array = extract_build_system_requires(&value)?;
    collect_string_items(&array)
}

fn extract_build_system_requires(
    value: &toml::Value,
) -> Result<Vec<toml::Value>, String> {
    let Some(build_system) = value.get("build-system") else {
        return Ok(Vec::new());
    };
    let build_system = build_system
        .as_table()
        .ok_or_else(|| "build-system 不是表".to_string())?;
    let requires = build_system
        .get("requires")
        .ok_or_else(|| "build-system.requires 缺失".to_string())?;
    requires
        .as_array()
        .map(|a| a.to_vec())
        .ok_or_else(|| "build-system.requires 不是数组".to_string())
}

fn collect_string_items(items: &[toml::Value]) -> Result<Vec<String>, String> {
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let line = item.as_str().ok_or_else(|| {
            "build-system.requires 存在非字符串项".to_string()
        })?;
        result.push(line.to_string());
    }
    Ok(result)
}

#[derive(Clone)]
pub(crate) struct PlannedSdist {
    pub package: String,
    pub version: Version,
}

async fn prefetch_one_sdist(
    cache: &MetadataCache,
    job: &PlannedSdist,
) -> Result<Option<((String, String), Vec<u8>)>, ResolveError> {
    let package = job.package.clone();
    let result = handle_planned_sdist(cache, job).await?;
    Ok(result.map(|p| ((package, p.filename), p.bytes)))
}

pub(crate) async fn collect_prefetched_sdists(
    cache: &MetadataCache,
    planned_files: &[DownloadableItem],
    include_source: bool,
    metadata_workers: usize,
) -> Result<HashMap<(String, String), Vec<u8>>, ResolveError> {
    if !include_source {
        return Ok(HashMap::new());
    }
    let jobs = collect_planned_sdist_jobs(planned_files);
    if jobs.is_empty() {
        return Ok(HashMap::new());
    }
    let results = stream::iter(jobs)
        .map(|job| async move { prefetch_one_sdist(cache, &job).await })
        .buffer_unordered(metadata_workers)
        .collect::<Vec<_>>()
        .await;
    let mut prefetched = HashMap::new();
    for (key, bytes) in results
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
    {
        prefetched.insert(key, bytes);
    }
    Ok(prefetched)
}

fn collect_planned_sdist_jobs(
    planned_files: &[DownloadableItem],
) -> Vec<PlannedSdist> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for file in planned_files {
        let Some(remote) = file.as_remote() else {
            continue;
        };
        if !crate::filters::is_source_distribution(&remote.filename) {
            continue;
        }
        let Ok(version) = remote.version.parse::<Version>() else {
            tracing::warn!(
                "  ! 无法解析 sdist 版本，跳过 build requires 解析: {}@{}",
                remote.package_name,
                remote.version
            );
            continue;
        };
        if !seen.insert((remote.package_name.clone(), version.clone())) {
            continue;
        }
        jobs.push(PlannedSdist {
            package: remote.package_name.clone(),
            version,
        });
    }
    jobs
}

async fn handle_planned_sdist(
    cache: &MetadataCache,
    job: &PlannedSdist,
) -> Result<Option<PrefetchedSdist>, ResolveError> {
    match cache
        .get_build_requires_probe(&job.package, &job.version)
        .await
    {
        Ok(probe) => Ok(probe.prefetched_sdist.clone()),
        Err(MetadataError::SdistBuildRequires { detail, .. }) => {
            tracing::warn!(
                "  ! {}@{} 解析源码包编译依赖失败，已跳过: {}",
                job.package,
                job.version,
                detail
            );
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}
