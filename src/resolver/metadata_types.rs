use std::collections::HashMap;

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
    SdistBuildRequires {
        package: String,
        version: String,
        detail: String,
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
            MetadataError::SdistBuildRequires {
                package,
                version,
                detail,
            } => write!(
                f,
                "解析 {}@{} 的源码包编译依赖失败: {}",
                package, version, detail
            ),
        }
    }
}

impl std::error::Error for MetadataError {}

pub(crate) fn parse_file_info(
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

pub(crate) fn collect_files_by_version(
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
