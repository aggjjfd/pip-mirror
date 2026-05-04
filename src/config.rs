use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub packages: Vec<String>,

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

    #[serde(default = "default_workers")]
    pub workers: usize,

    #[serde(default = "default_max_versions")]
    pub max_versions: usize,

    #[serde(default)]
    pub allow_prerelease: bool,

    #[serde(default = "default_backfill_scan_limit")]
    pub backfill_scan_limit: usize,

    #[serde(default = "default_server_port")]
    pub server_port: u16,

    #[serde(default = "default_server_host")]
    pub server_host: String,
}

// ── defaults ──

fn default_repository_dir() -> PathBuf { PathBuf::from("./packages") }
fn default_incremental_dir() -> PathBuf { PathBuf::from("./incremental") }
fn default_pypi_url() -> String { "https://pypi.org".into() }
fn default_index_url() -> String { "https://mirrors.ustc.edu.cn/pypi/simple".into() }
fn default_include_source() -> bool { true }
fn default_workers() -> usize { 4 }
fn default_max_versions() -> usize { 5 }
fn default_backfill_scan_limit() -> usize { 50 }
fn default_server_port() -> u16 { 8080 }
fn default_server_host() -> String { "0.0.0.0".into() }

// ── loading ──

impl Config {
    /// Load from an explicit TOML path, or fall back to `pyproject.toml` → env.
    pub fn load(path: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(p) = path {
            if !p.exists() {
                return Err(format!("配置文件不存在: {}", p.display()).into());
            }
            let content = std::fs::read_to_string(p)?;
            return Ok(toml::from_str(&content)?);
        }

        // Try pyproject.toml [tool.pip-mirror]
        let pyproject = Path::new("pyproject.toml");
        if pyproject.exists() {
            let content = std::fs::read_to_string(pyproject)?;
            let root: toml::Value = toml::from_str(&content)?;
            if let Some(tool_config) = root.get("tool").and_then(|t| t.get("pip-mirror")) {
                return Ok(toml::Value::try_into(tool_config.clone())?);
            }
        }

        // Try env PIP_MIRROR_PACKAGES
        if let Ok(pkgs) = std::env::var("PIP_MIRROR_PACKAGES") {
            return Ok(Config {
                packages: pkgs.split(',').map(|s| s.trim().to_string()).collect(),
                ..Config::default()
            });
        }

        Ok(Config::default())
    }
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
            workers: default_workers(),
            max_versions: default_max_versions(),
            allow_prerelease: false,
            backfill_scan_limit: default_backfill_scan_limit(),
            server_port: default_server_port(),
            server_host: default_server_host(),
        }
    }
}
