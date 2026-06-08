use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

const DEFAULT_INCLUDE_SOURCE: bool = false;
const DEFAULT_RESOLVE_WORKERS: usize = 8;
const DEFAULT_METADATA_WORKERS: usize = 32;
const DEFAULT_DOWNLOAD_WORKERS: usize = 8;
const DEFAULT_TOP_VERSIONS_PER_PACKAGE: usize = 5;
const DEFAULT_ADJACENT_VERSIONS_PER_SIDE: usize = 2;
const DEFAULT_LINUX_MAX_GLIBC: &str = "2.39";
const DEFAULT_SERVER_PORT: u16 = 8080;
const DEFAULT_SERVER_HOST: &str = "127.0.0.1";

/// 用户配置的一个解析目标。
/// python: Python 版本号，如 "3.10" 或 "3.10.0"
/// os:     操作系统，支持 "linux" / "windows"
/// arch:   架构，支持 "x64" / "x86"
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSpec {
    pub python: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PackageSpec {
    Name(String),
    Url(PackageUrlSpec),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageUrlSpec {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

impl PackageSpec {
    pub fn as_url(&self) -> Option<&str> {
        match self {
            PackageSpec::Url(u) => Some(&u.url),
            PackageSpec::Name(_) => None,
        }
    }

    pub fn as_name(&self) -> Option<&str> {
        match self {
            PackageSpec::Name(n) => Some(n),
            PackageSpec::Url(_) => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub packages: Vec<PackageSpec>,
    #[serde(default = "default_repository_dir")]
    pub repository_dir: PathBuf,
    #[serde(default = "default_incremental_dir")]
    pub incremental_dir: PathBuf,
    #[serde(default = "default_pypi_url")]
    pub pypi_url: String,
    #[serde(default = "default_index_url")]
    pub index_url: String,
    #[serde(default = "default_include_source")]
    pub include_source: bool,
    #[serde(default = "default_resolve_workers")]
    pub resolve_workers: usize,
    #[serde(default = "default_metadata_workers")]
    pub metadata_workers: usize,
    #[serde(default = "default_download_workers")]
    pub download_workers: usize,
    #[serde(default = "default_top_versions_per_package")]
    pub top_versions_per_package: usize,
    #[serde(default = "default_adjacent_versions_per_side")]
    pub adjacent_versions_per_side: usize,
    #[serde(default)]
    pub allow_prerelease: bool,
    #[serde(default = "default_linux_max_glibc")]
    pub linux_max_glibc: String,
    #[serde(default = "default_server_port")]
    pub server_port: u16,
    #[serde(default = "default_server_host")]
    pub server_host: String,
    #[serde(default = "default_targets")]
    pub targets: Vec<TargetSpec>,
}

fn default_repository_dir() -> PathBuf {
    PathBuf::from("./packages")
}
fn default_incremental_dir() -> PathBuf {
    PathBuf::from("./incremental")
}
fn default_pypi_url() -> String {
    "https://pypi.org".into()
}
fn default_index_url() -> String {
    "https://mirrors.ustc.edu.cn/pypi/simple".into()
}
fn default_include_source() -> bool {
    DEFAULT_INCLUDE_SOURCE
}
fn default_resolve_workers() -> usize {
    DEFAULT_RESOLVE_WORKERS
}
fn default_metadata_workers() -> usize {
    DEFAULT_METADATA_WORKERS
}
fn default_download_workers() -> usize {
    DEFAULT_DOWNLOAD_WORKERS
}
fn default_top_versions_per_package() -> usize {
    DEFAULT_TOP_VERSIONS_PER_PACKAGE
}
fn default_adjacent_versions_per_side() -> usize {
    DEFAULT_ADJACENT_VERSIONS_PER_SIDE
}
fn default_linux_max_glibc() -> String {
    DEFAULT_LINUX_MAX_GLIBC.into()
}
fn default_server_port() -> u16 {
    DEFAULT_SERVER_PORT
}
fn default_server_host() -> String {
    DEFAULT_SERVER_HOST.into()
}

fn default_targets() -> Vec<TargetSpec> {
    vec![
        // Python 3.8
        TargetSpec {
            python: "3.8".into(),
            os: "linux".into(),
            arch: "x64".into(),
        },
        TargetSpec {
            python: "3.8".into(),
            os: "windows".into(),
            arch: "x86".into(),
        },
        TargetSpec {
            python: "3.8".into(),
            os: "windows".into(),
            arch: "x64".into(),
        },
        // Python 3.9
        TargetSpec {
            python: "3.9".into(),
            os: "linux".into(),
            arch: "x64".into(),
        },
        TargetSpec {
            python: "3.9".into(),
            os: "windows".into(),
            arch: "x64".into(),
        },
        // Python 3.10
        TargetSpec {
            python: "3.10".into(),
            os: "linux".into(),
            arch: "x64".into(),
        },
        TargetSpec {
            python: "3.10".into(),
            os: "windows".into(),
            arch: "x64".into(),
        },
        // Python 3.11
        TargetSpec {
            python: "3.11".into(),
            os: "linux".into(),
            arch: "x64".into(),
        },
        TargetSpec {
            python: "3.11".into(),
            os: "windows".into(),
            arch: "x64".into(),
        },
        // Python 3.12
        TargetSpec {
            python: "3.12".into(),
            os: "linux".into(),
            arch: "x64".into(),
        },
        TargetSpec {
            python: "3.12".into(),
            os: "windows".into(),
            arch: "x64".into(),
        },
    ]
}

fn load_explicit(p: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(p)?;
    Ok(toml::from_str(&content)?)
}

fn try_env() -> Option<Config> {
    let pkgs = std::env::var("PIP_MIRROR_PACKAGES").ok()?;
    Some(Config {
        packages: pkgs
            .split(',')
            .map(|s| PackageSpec::Name(s.trim().to_string()))
            .collect(),
        ..Config::default()
    })
}

fn try_default_toml() -> Result<Option<Config>, Box<dyn std::error::Error>> {
    let p = Path::new("pip-mirror.toml");
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(load_explicit(p)?))
}

impl Config {
    pub fn load(
        path: Option<&Path>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let cfg = match path {
            Some(p) if !p.exists() => {
                return Err(format!("配置文件不存在: {}", p.display()).into());
            }
            Some(p) => load_explicit(p)?,
            None => match try_env() {
                Some(cfg) => cfg,
                None => try_default_toml()?.unwrap_or_default(),
            },
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_no_url_names()?;
        self.validate_url_specs()
    }

    fn validate_no_url_names(&self) -> Result<(), String> {
        let looks_like_url = |s: &str| {
            let lower = s.trim().to_ascii_lowercase();
            lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("file://")
        };
        if let Some(name) = self
            .packages
            .iter()
            .filter_map(|s| s.as_name())
            .find(|s| looks_like_url(s))
        {
            let safe = crate::filters::redact_url_for_display(name);
            return Err(format!(
                "包名 `{safe}` 看起来像 URL。如需指定 whl URL，请使用 `{{ url = \"{safe}\" }}` 表格式。"
            ));
        }
        Ok(())
    }

    fn validate_url_specs(&self) -> Result<(), String> {
        if let Some(err) = self
            .packages
            .iter()
            .filter_map(|spec| match spec {
                PackageSpec::Url(u) => validate_url_spec(u),
                _ => None,
            })
            .next()
        {
            return Err(err);
        }
        Ok(())
    }
}

fn validate_url_spec(u: &PackageUrlSpec) -> Option<String> {
    let lower = u.url.to_ascii_lowercase();
    let safe = crate::filters::redact_url_for_display(&u.url);
    if !lower.starts_with("http://")
        && !lower.starts_with("https://")
        && !lower.starts_with("file://")
    {
        return Some(format!("URL whl 只支持 http/https/file 协议: {safe}"));
    }
    let parsed = match Url::parse(&u.url) {
        Ok(p) => p,
        Err(_) => return Some(format!("无法解析 URL: {safe}")),
    };
    if !parsed.path().to_ascii_lowercase().ends_with(".whl") {
        return Some(format!("URL whl 必须以 .whl 结尾: {safe}"));
    }
    None
}

impl Default for Config {
    fn default() -> Self {
        Config {
            packages: vec![],
            repository_dir: default_repository_dir(),
            incremental_dir: default_incremental_dir(),
            pypi_url: default_pypi_url(),
            index_url: default_index_url(),
            include_source: default_include_source(),
            resolve_workers: default_resolve_workers(),
            metadata_workers: default_metadata_workers(),
            download_workers: default_download_workers(),
            top_versions_per_package: default_top_versions_per_package(),
            adjacent_versions_per_side: default_adjacent_versions_per_side(),
            allow_prerelease: false,
            linux_max_glibc: default_linux_max_glibc(),
            server_port: default_server_port(),
            server_host: default_server_host(),
            targets: default_targets(),
        }
    }
}
