use pep440_rs::Version;

use super::markers::MarkerError;
use super::metadata_types::MetadataError;

#[derive(Debug, Clone)]
pub enum ResolveError {
    Metadata(MetadataError),
    Marker(MarkerError),
    InvalidRequiresPython {
        package: String,
        version: Version,
        spec: String,
        detail: String,
    },
    NoSolution {
        package: String,
        version: Version,
        target: String,
        detail: String,
    },
    NoMatchingVersion {
        package: String,
        spec: String,
    },
    Config(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Metadata(err) => {
                write!(f, "元数据获取失败: {err}")
            }
            ResolveError::Marker(err) => {
                write!(f, "依赖 marker 解析失败: {err}")
            }
            ResolveError::InvalidRequiresPython {
                package,
                version,
                spec,
                detail,
            } => write!(
                f,
                "无法解析 {package}@{version} 的 requires_python={spec}: {detail}"
            ),
            ResolveError::NoSolution {
                package,
                version,
                target,
                detail,
            } => write!(
                f,
                "无法为 {package}@{version} 在 {target} 上求得依赖解: {detail}"
            ),
            ResolveError::NoMatchingVersion { package, spec } => {
                write!(
                    f,
                    "包 {package} 在 PyPI 上找不到匹配版本约束 {spec} 的版本"
                )
            }
            ResolveError::Config(msg) => write!(f, "配置错误: {msg}"),
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<MetadataError> for ResolveError {
    fn from(value: MetadataError) -> Self {
        Self::Metadata(value)
    }
}

impl From<MarkerError> for ResolveError {
    fn from(value: MarkerError) -> Self {
        Self::Marker(value)
    }
}
