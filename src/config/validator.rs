use std::collections::HashMap;
use std::fmt;

use url::Url;

use super::{Config, PackageSpec, PackageUrlSpec};
use crate::filters::parse_package_ref;
use crate::redact::redact_url_for_display;

/// 配置校验阶段的统一错误类型。
pub enum ConfigError {
    /// 镜像地址无效。
    InvalidMirror { url: String, reason: String },
    /// 显式 whl URL 无效。
    InvalidPackageUrl { url: String, reason: String },
    /// 包名看起来像 URL，应使用 `url = "..."` 表格形式。
    UrlMistakenForName(String),
    /// 包引用中的版本约束格式无效。
    InvalidVersionSpec {
        package: String,
        raw: String,
        reason: String,
    },
    /// 同一包名在 packages 中出现多次且带有版本约束。
    DuplicateVersionSpec { package: String },
}

impl fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidMirror { url, reason } => {
                debug_mirror(f, url, reason)
            }
            ConfigError::InvalidPackageUrl { url, reason } => {
                debug_package_url(f, url, reason)
            }
            ConfigError::UrlMistakenForName(name) => {
                debug_url_mistaken_for_name(f, name)
            }
            ConfigError::InvalidVersionSpec {
                package,
                raw,
                reason,
            } => debug_invalid_version_spec(f, package, raw, reason),
            ConfigError::DuplicateVersionSpec { package } => {
                debug_duplicate_version_spec(f, package)
            }
        }
    }
}

fn debug_mirror(
    f: &mut fmt::Formatter<'_>,
    url: &str,
    reason: &str,
) -> fmt::Result {
    f.debug_struct("InvalidMirror")
        .field("url", &redact_url_for_display(url))
        .field("reason", &reason)
        .finish()
}

fn debug_package_url(
    f: &mut fmt::Formatter<'_>,
    url: &str,
    reason: &str,
) -> fmt::Result {
    f.debug_struct("InvalidPackageUrl")
        .field("url", &redact_url_for_display(url))
        .field("reason", &reason)
        .finish()
}

fn debug_url_mistaken_for_name(
    f: &mut fmt::Formatter<'_>,
    name: &str,
) -> fmt::Result {
    f.debug_tuple("UrlMistakenForName")
        .field(&redact_url_for_display(name))
        .finish()
}

fn debug_invalid_version_spec(
    f: &mut fmt::Formatter<'_>,
    package: &str,
    raw: &str,
    reason: &str,
) -> fmt::Result {
    f.debug_struct("InvalidVersionSpec")
        .field("package", &package)
        .field("raw", &raw)
        .field("reason", &reason)
        .finish()
}

fn debug_duplicate_version_spec(
    f: &mut fmt::Formatter<'_>,
    package: &str,
) -> fmt::Result {
    f.debug_struct("DuplicateVersionSpec")
        .field("package", &package)
        .finish()
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidMirror { url, reason } => {
                display_mirror(f, url, reason)
            }
            ConfigError::InvalidPackageUrl { url, reason } => {
                display_package_url(f, url, reason)
            }
            ConfigError::UrlMistakenForName(name) => {
                display_url_mistaken_for_name(f, name)
            }
            ConfigError::InvalidVersionSpec {
                package,
                raw,
                reason,
            } => display_invalid_version_spec(f, package, raw, reason),
            ConfigError::DuplicateVersionSpec { package } => {
                display_duplicate_version_spec(f, package)
            }
        }
    }
}

fn display_mirror(
    f: &mut fmt::Formatter<'_>,
    url: &str,
    reason: &str,
) -> fmt::Result {
    let safe = redact_url_for_display(url);
    write!(f, "镜像地址无效 ({safe}): {reason}")
}

fn display_package_url(
    f: &mut fmt::Formatter<'_>,
    url: &str,
    reason: &str,
) -> fmt::Result {
    let safe = redact_url_for_display(url);
    write!(f, "URL whl 无效 ({safe}): {reason}")
}

fn display_url_mistaken_for_name(
    f: &mut fmt::Formatter<'_>,
    name: &str,
) -> fmt::Result {
    let safe = redact_url_for_display(name);
    write!(
        f,
        "包名 `{safe}` 看起来像 URL。如需指定 whl URL，请使用 `{{ url = \"{safe}\" }}` 表格式。"
    )
}

fn display_invalid_version_spec(
    f: &mut fmt::Formatter<'_>,
    package: &str,
    raw: &str,
    reason: &str,
) -> fmt::Result {
    let safe = redact_url_for_display(raw);
    if package.is_empty() {
        return write!(f, "包引用 `{safe}` 的版本约束无效: {reason}");
    }
    write!(f, "包 `{package}` 的版本约束 `{safe}` 无效: {reason}")
}

fn display_duplicate_version_spec(
    f: &mut fmt::Formatter<'_>,
    package: &str,
) -> fmt::Result {
    write!(f, "包 `{package}` 在 packages 中重复出现且带有版本约束")
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

        Self::validate_package_refs(&config.packages)?;

        for spec in config.packages.iter().filter_map(|s| match s {
            PackageSpec::Url(u) => Some(u),
            _ => None,
        }) {
            Self::validate_package_url(spec)?;
        }

        Ok(())
    }

    fn validate_package_refs(
        packages: &[PackageSpec],
    ) -> Result<(), ConfigError> {
        let mut seen: HashMap<String, Option<String>> = HashMap::new();
        for spec in packages.iter().filter_map(|s| s.as_name()) {
            let (name, version_spec) = parse_and_validate_package_ref(spec)?;
            check_duplicate_package_ref(&mut seen, name, version_spec)?;
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

fn parse_and_validate_package_ref(
    spec: &str,
) -> Result<(String, Option<String>), ConfigError> {
    let parsed = match parse_package_ref(spec) {
        Ok(p) => p,
        Err(reason) => {
            return Err(ConfigError::InvalidVersionSpec {
                package: String::new(),
                raw: spec.to_string(),
                reason,
            });
        }
    };

    if let Some(version_spec) = &parsed.version_spec
        && let Err(reason) =
            crate::resolver::pubgrub::validate_version_spec(version_spec)
    {
        return Err(ConfigError::InvalidVersionSpec {
            package: parsed.name.clone(),
            raw: spec.to_string(),
            reason,
        });
    }

    Ok((parsed.name, parsed.version_spec))
}

fn check_duplicate_package_ref(
    seen: &mut HashMap<String, Option<String>>,
    name: String,
    version_spec: Option<String>,
) -> Result<(), ConfigError> {
    if let Some(existing) = seen.get(&name)
        && (existing.is_some() || version_spec.is_some())
    {
        return Err(ConfigError::DuplicateVersionSpec { package: name });
    }
    seen.insert(name, version_spec);
    Ok(())
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
