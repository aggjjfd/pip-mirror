use std::fmt;

use crate::redact::redact_url_for_display;

/// 解析阶段产出的文件描述，供 resolver 与 filters 共用。
///
/// 字段与 `downloader::RemoteFile` 对齐；在 sync pipeline 边界处转换为
/// `RemoteFile` / `DownloadableItem`。
#[derive(Clone)]
pub struct ResolvedFile {
    pub filename: String,
    pub url: String,
    pub package_name: String,
    pub version: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub yanked: Option<String>,
}

impl fmt::Debug for ResolvedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedFile")
            .field("filename", &self.filename)
            .field("url", &redact_url_for_display(&self.url))
            .field("package_name", &self.package_name)
            .field("version", &self.version)
            .field("sha256", &self.sha256)
            .field("size", &self.size)
            .field("yanked", &self.yanked)
            .finish()
    }
}
