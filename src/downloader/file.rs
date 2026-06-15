use std::path::{Path, PathBuf};

use type_state_builder::TypeStateBuilder;

/// Trait for any file that can be downloaded into the mirror repository.
pub trait Downloadable: Send + Sync + std::fmt::Debug {
    fn filename(&self) -> &str;
    fn package_name(&self) -> &str;
    fn version(&self) -> &str;
    fn sha256(&self) -> Option<&str>;
    fn size(&self) -> Option<u64>;
    fn source_url(&self) -> &str;
    fn yanked(&self) -> Option<&str>;
    fn is_explicit_url(&self) -> bool;
    fn dest_path(&self, repo: &Path) -> PathBuf;
}

/// A file discovered via PyPI JSON API.
#[derive(Debug, Clone, TypeStateBuilder)]
#[builder(impl_into)]
pub struct RemoteFile {
    #[builder(required)]
    pub filename: String,
    #[builder(required)]
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub yanked: Option<String>,
    #[builder(required)]
    pub package_name: String,
    #[builder(required)]
    pub version: String,
}

impl Downloadable for RemoteFile {
    fn filename(&self) -> &str {
        &self.filename
    }

    fn package_name(&self) -> &str {
        &self.package_name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    fn size(&self) -> Option<u64> {
        self.size
    }

    fn source_url(&self) -> &str {
        &self.url
    }

    fn yanked(&self) -> Option<&str> {
        self.yanked.as_deref()
    }

    fn is_explicit_url(&self) -> bool {
        false
    }

    fn dest_path(&self, repo: &Path) -> PathBuf {
        repo.join("simple")
            .join(&self.package_name)
            .join(&self.filename)
    }
}

/// A wheel explicitly provided by the user via URL.
#[derive(Debug, Clone)]
pub struct ExplicitWheel {
    pub filename: String,
    pub url: String,
    pub sha256: Option<String>,
    pub package_name: String,
    pub version: String,
}

impl Downloadable for ExplicitWheel {
    fn filename(&self) -> &str {
        &self.filename
    }

    fn package_name(&self) -> &str {
        &self.package_name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    fn size(&self) -> Option<u64> {
        None
    }

    fn source_url(&self) -> &str {
        &self.url
    }

    fn yanked(&self) -> Option<&str> {
        None
    }

    fn is_explicit_url(&self) -> bool {
        true
    }

    fn dest_path(&self, repo: &Path) -> PathBuf {
        repo.join("simple")
            .join(&self.package_name)
            .join(&self.filename)
    }
}

/// Either a PyPI-discovered file or a user-provided explicit wheel.
#[derive(Debug, Clone)]
pub enum DownloadableItem {
    Remote(RemoteFile),
    Explicit(ExplicitWheel),
}

impl DownloadableItem {
    pub fn as_remote(&self) -> Option<&RemoteFile> {
        match self {
            DownloadableItem::Remote(r) => Some(r),
            DownloadableItem::Explicit(_) => None,
        }
    }

    pub fn as_explicit(&self) -> Option<&ExplicitWheel> {
        match self {
            DownloadableItem::Remote(_) => None,
            DownloadableItem::Explicit(e) => Some(e),
        }
    }
}

impl Downloadable for DownloadableItem {
    fn filename(&self) -> &str {
        match self {
            DownloadableItem::Remote(r) => r.filename(),
            DownloadableItem::Explicit(e) => e.filename(),
        }
    }

    fn package_name(&self) -> &str {
        match self {
            DownloadableItem::Remote(r) => r.package_name(),
            DownloadableItem::Explicit(e) => e.package_name(),
        }
    }

    fn version(&self) -> &str {
        match self {
            DownloadableItem::Remote(r) => r.version(),
            DownloadableItem::Explicit(e) => e.version(),
        }
    }

    fn sha256(&self) -> Option<&str> {
        match self {
            DownloadableItem::Remote(r) => r.sha256(),
            DownloadableItem::Explicit(e) => e.sha256(),
        }
    }

    fn size(&self) -> Option<u64> {
        match self {
            DownloadableItem::Remote(r) => r.size(),
            DownloadableItem::Explicit(e) => e.size(),
        }
    }

    fn source_url(&self) -> &str {
        match self {
            DownloadableItem::Remote(r) => r.source_url(),
            DownloadableItem::Explicit(e) => e.source_url(),
        }
    }

    fn yanked(&self) -> Option<&str> {
        match self {
            DownloadableItem::Remote(r) => r.yanked(),
            DownloadableItem::Explicit(e) => e.yanked(),
        }
    }

    fn is_explicit_url(&self) -> bool {
        match self {
            DownloadableItem::Remote(_) => false,
            DownloadableItem::Explicit(_) => true,
        }
    }

    fn dest_path(&self, repo: &Path) -> PathBuf {
        match self {
            DownloadableItem::Remote(r) => r.dest_path(repo),
            DownloadableItem::Explicit(e) => e.dest_path(repo),
        }
    }
}
