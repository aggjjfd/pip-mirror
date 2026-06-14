pub mod access_log;
pub mod config;
pub mod downloader;
pub mod filters;
pub mod index_page;
pub mod indexer;
pub mod installer;
pub mod logging;
pub mod packager;
pub mod progress;
pub mod python_builds;
pub mod resolver;
pub mod server;
pub mod store;
pub mod sync;
pub mod wheel_metadata;
pub mod wheel_url;

/// 将字节切片格式化为小写十六进制字符串。
pub fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
