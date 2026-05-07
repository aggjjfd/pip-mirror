use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use pep440_rs::Version;

use crate::downloader::FileInfo;

/// Package-level index: all versions and their files.
#[derive(Debug, Clone)]
pub struct PackageIndex {
    pub versions: Vec<Version>,
    pub files_by_version: HashMap<Version, Vec<FileInfo>>,
}

/// Version-level metadata: requires_dist and requires_python.
#[derive(Debug, Clone)]
pub struct VersionMetadata {
    pub requires_dist: Vec<String>,
    pub requires_python: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MetadataError {
    Http {
        package: String,
        version: Option<String>,
        status: u16,
    },
    Json {
        package: String,
        version: Option<String>,
        msg: String,
    },
    MissingField {
        package: String,
        field: String,
    },
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataError::Http {
                package,
                version,
                status,
            } => write!(
                f,
                "HTTP {} for {}{}",
                status,
                package,
                version
                    .as_ref()
                    .map(|v| format!("@{}", v))
                    .unwrap_or_default()
            ),
            MetadataError::Json {
                package,
                version,
                msg,
            } => write!(
                f,
                "JSON error for {}{}: {}",
                package,
                version
                    .as_ref()
                    .map(|v| format!("@{}", v))
                    .unwrap_or_default(),
                msg
            ),
            MetadataError::MissingField { package, field } => {
                write!(
                    f,
                    "Missing field '{}' in response for {}",
                    field, package
                )
            }
        }
    }
}

impl std::error::Error for MetadataError {}

fn parse_file_info(
    f: &serde_json::Value,
    pkg: &str,
    version_str: &str,
) -> Option<FileInfo> {
    Some(FileInfo {
        filename: f["filename"].as_str()?.to_string(),
        url: f["url"].as_str()?.to_string(),
        sha256: f
            .get("digests")
            .and_then(|d| d.get("sha256"))
            .and_then(|s| s.as_str())
            .map(String::from),
        size: f["size"].as_u64(),
        package_name: pkg.to_string(),
        version: version_str.to_string(),
    })
}

fn collect_files_by_version(
    releases: &serde_json::Map<String, serde_json::Value>,
    pkg: &str,
) -> HashMap<Version, Vec<FileInfo>> {
    let mut result = HashMap::new();
    for (version_str, file_list) in releases {
        if let Ok(ver) = version_str.parse::<Version>() {
            let files: Vec<FileInfo> = file_list
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|f| parse_file_info(f, pkg, version_str))
                .collect();
            result.insert(ver, files);
        }
    }
    result
}

type InFlight<T> =
    Arc<tokio::sync::Mutex<Option<Result<Arc<T>, MetadataError>>>>;

/// Shared cache for package and version metadata, with in-flight deduplication.
pub struct MetadataCache {
    client: reqwest::Client,
    pypi_url: String,
    sem: tokio::sync::Semaphore,
    package_index: DashMap<String, InFlight<PackageIndex>>,
    version_metadata: DashMap<(String, Version), InFlight<VersionMetadata>>,
}

impl MetadataCache {
    pub fn new(
        client: reqwest::Client,
        pypi_url: String,
        metadata_workers: usize,
    ) -> Self {
        Self {
            client,
            pypi_url,
            sem: tokio::sync::Semaphore::new(metadata_workers),
            package_index: DashMap::new(),
            version_metadata: DashMap::new(),
        }
    }

    /// Return all versions for a package (newest first).
    pub async fn get_all_versions(
        &self,
        pkg: &str,
    ) -> Result<Vec<Version>, MetadataError> {
        let index = self.get_package_index(pkg).await?;
        Ok(index.versions.clone())
    }

    /// Return files for a specific package@version.
    pub async fn get_version_files(
        &self,
        pkg: &str,
        ver: &Version,
    ) -> Result<Vec<FileInfo>, MetadataError> {
        let index = self.get_package_index(pkg).await?;
        Ok(index.files_by_version.get(ver).cloned().unwrap_or_default())
    }

    /// Return requires_dist for a specific package@version.
    pub async fn get_requires_dist(
        &self,
        pkg: &str,
        ver: &Version,
    ) -> Result<Vec<String>, MetadataError> {
        let meta = self.get_version_metadata(pkg, ver).await?;
        Ok(meta.requires_dist.clone())
    }

    /// Return requires_python for a specific package@version.
    pub async fn get_requires_python(
        &self,
        pkg: &str,
        ver: &Version,
    ) -> Result<Option<String>, MetadataError> {
        let meta = self.get_version_metadata(pkg, ver).await?;
        Ok(meta.requires_python.clone())
    }

    /// Return full PackageIndex for a package.
    #[allow(clippy::excessive_nesting)]
    pub async fn get_package_index(
        &self,
        pkg: &str,
    ) -> Result<Arc<PackageIndex>, MetadataError> {
        let normalized = crate::filters::normalize_package_name(pkg);
        let shared = self
            .package_index
            .entry(normalized.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
            .clone();

        {
            let guard = shared.lock().await;
            if let Some(ref result) = *guard {
                return result.clone();
            }
        }

        let mut guard = shared.lock().await;
        if let Some(ref result) = *guard {
            return result.clone();
        }

        let result = self.fetch_package_index(&normalized).await;
        let arc_result = result.map(Arc::new);
        *guard = Some(arc_result.clone());
        arc_result
    }

    /// Return VersionMetadata for a specific package@version.
    #[allow(clippy::excessive_nesting)]
    pub async fn get_version_metadata(
        &self,
        pkg: &str,
        ver: &Version,
    ) -> Result<Arc<VersionMetadata>, MetadataError> {
        let normalized = crate::filters::normalize_package_name(pkg);
        let key = (normalized.clone(), ver.clone());
        let shared = self
            .version_metadata
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
            .clone();

        {
            let guard = shared.lock().await;
            if let Some(ref result) = *guard {
                return result.clone();
            }
        }

        let mut guard = shared.lock().await;
        if let Some(ref result) = *guard {
            return result.clone();
        }

        let result = self.fetch_version_metadata(&normalized, ver).await;
        let arc_result = result.map(Arc::new);
        *guard = Some(arc_result.clone());
        arc_result
    }

    async fn fetch_package_index(
        &self,
        pkg: &str,
    ) -> Result<PackageIndex, MetadataError> {
        let _permit = self.sem.acquire().await.expect("semaphore not closed");
        let url = format!(
            "{}/pypi/{}/json",
            self.pypi_url.trim_end_matches('/'),
            pkg
        );
        let resp = self.client.get(&url).send().await.map_err(|_e| {
            MetadataError::Http {
                package: pkg.to_string(),
                version: None,
                status: 0,
            }
        })?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(MetadataError::Http {
                package: pkg.to_string(),
                version: None,
                status,
            });
        }

        let json: serde_json::Value =
            resp.json().await.map_err(|e| MetadataError::Json {
                package: pkg.to_string(),
                version: None,
                msg: e.to_string(),
            })?;

        let releases = json
            .get("releases")
            .and_then(|r| r.as_object())
            .ok_or_else(|| MetadataError::MissingField {
                package: pkg.to_string(),
                field: "releases".to_string(),
            })?;

        let mut versions: Vec<Version> = releases
            .keys()
            .filter_map(|v| v.parse::<Version>().ok())
            .collect();
        versions.sort_by(|a, b| b.cmp(a));

        let files_by_version = collect_files_by_version(releases, pkg);

        Ok(PackageIndex {
            versions,
            files_by_version,
        })
    }

    async fn fetch_version_metadata(
        &self,
        pkg: &str,
        ver: &Version,
    ) -> Result<VersionMetadata, MetadataError> {
        let _permit = self.sem.acquire().await.expect("semaphore not closed");
        let ver_str = ver.to_string();
        let url = format!(
            "{}/pypi/{}/{}/json",
            self.pypi_url.trim_end_matches('/'),
            pkg,
            ver_str
        );
        let resp = self.client.get(&url).send().await.map_err(|_e| {
            MetadataError::Http {
                package: pkg.to_string(),
                version: Some(ver_str.clone()),
                status: 0,
            }
        })?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(MetadataError::Http {
                package: pkg.to_string(),
                version: Some(ver_str.clone()),
                status,
            });
        }

        let json: serde_json::Value =
            resp.json().await.map_err(|e| MetadataError::Json {
                package: pkg.to_string(),
                version: Some(ver_str.clone()),
                msg: e.to_string(),
            })?;

        let info =
            json.get("info")
                .ok_or_else(|| MetadataError::MissingField {
                    package: pkg.to_string(),
                    field: "info".to_string(),
                })?;

        let requires_dist = info
            .get("requires_dist")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let requires_python = info
            .get("requires_python")
            .and_then(|r| r.as_str())
            .map(String::from);

        Ok(VersionMetadata {
            requires_dist,
            requires_python,
        })
    }
}
