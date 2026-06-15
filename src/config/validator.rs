use std::fmt;

use url::Url;

use super::{Config, PackageSpec, PackageUrlSpec};
use crate::redact::redact_url_for_display;

/// 配置校验阶段的统一错误类型。
pub enum ConfigError {
    /// 镜像地址无效。
    InvalidMirror { url: String, reason: String },
    /// 显式 whl URL 无效。
    InvalidPackageUrl { url: String, reason: String },
    /// 包名看起来像 URL，应使用 `url = "..."` 表格形式。
    UrlMistakenForName(String),
}

impl fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidMirror { url, reason } => f
                .debug_struct("InvalidMirror")
                .field("url", &redact_url_for_display(url))
                .field("reason", reason)
                .finish(),
            ConfigError::InvalidPackageUrl { url, reason } => f
                .debug_struct("InvalidPackageUrl")
                .field("url", &redact_url_for_display(url))
                .field("reason", reason)
                .finish(),
            ConfigError::UrlMistakenForName(name) => f
                .debug_tuple("UrlMistakenForName")
                .field(&redact_url_for_display(name))
                .finish(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidMirror { url, reason } => {
                let safe = redact_url_for_display(url);
                write!(f, "镜像地址无效 ({safe}): {reason}")
            }
            ConfigError::InvalidPackageUrl { url, reason } => {
                let safe = redact_url_for_display(url);
                write!(f, "URL whl 无效 ({safe}): {reason}")
            }
            ConfigError::UrlMistakenForName(name) => {
                let safe = redact_url_for_display(name);
                write!(
                    f,
                    "包名 `{safe}` 看起来像 URL。如需指定 whl URL，请使用 `{{ url = \"{safe}\" }}` 表格式。"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// 集中管理配置加载阶段的校验逻辑。
///
/// 所有需要在使用前确认的 URL / whl 规则都在这里统一检查，避免在运行时
/// 各处重复做防御性判断。
pub struct ConfigValidator;

impl ConfigValidator {
    /// 校验完整配置。
    pub fn validate(config: &Config) -> Result<(), ConfigError> {
        if let Some(name) = config
            .packages
            .iter()
            .filter_map(|s| s.as_name())
            .find(|s| looks_like_url(s))
        {
            return Err(ConfigError::UrlMistakenForName(name.to_string()));
        }

        Self::validate_mirrors(&config.effective_mirrors())?;

        for spec in config.packages.iter().filter_map(|s| match s {
            PackageSpec::Url(u) => Some(u),
            _ => None,
        }) {
            Self::validate_package_url(spec)?;
        }

        Ok(())
    }

    /// 校验镜像地址列表，只接受 `http` / `https`。
    pub fn validate_mirrors(urls: &[String]) -> Result<(), ConfigError> {
        for url in urls {
            validate_mirror(url)?;
        }
        Ok(())
    }

    /// 校验显式 whl URL，只接受 `http` / `https` / `file`，且路径必须以 `.whl` 结尾。
    pub fn validate_package_url(
        spec: &PackageUrlSpec,
    ) -> Result<(), ConfigError> {
        let url = &spec.url;
        let lower = url.to_ascii_lowercase();
        if !lower.starts_with("http://")
            && !lower.starts_with("https://")
            && !lower.starts_with("file://")
        {
            return Err(ConfigError::InvalidPackageUrl {
                url: url.clone(),
                reason: "只支持 http/https/file 协议".to_string(),
            });
        }

        let parsed =
            Url::parse(url).map_err(|e| ConfigError::InvalidPackageUrl {
                url: url.clone(),
                reason: format!("无法解析 URL: {e}"),
            })?;

        if !parsed.path().to_ascii_lowercase().ends_with(".whl") {
            return Err(ConfigError::InvalidPackageUrl {
                url: url.clone(),
                reason: "URL 必须以 .whl 结尾".to_string(),
            });
        }

        Ok(())
    }

    /// 判断一个字符串是否看起来像 URL（以 `http://` / `https://` / `file://` 开头，
    /// 大小写不敏感）。
    pub fn looks_like_url(name: &str) -> bool {
        looks_like_url(name)
    }
}

fn validate_mirror(url: &str) -> Result<(), ConfigError> {
    let parsed = Url::parse(url).map_err(|e| ConfigError::InvalidMirror {
        url: url.to_string(),
        reason: format!("解析失败: {e}"),
    })?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ConfigError::InvalidMirror {
            url: url.to_string(),
            reason: "只支持 http/https 协议".to_string(),
        });
    }
    Ok(())
}

fn looks_like_url(s: &str) -> bool {
    let lower = s.trim().to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
}
