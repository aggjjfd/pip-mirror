use std::fmt;

use crate::redact::redact_url_for_display;

/// 统一 HTTP 错误类型，避免在日志/错误信息中泄露凭证。
#[derive(Debug)]
pub enum HttpError {
    /// 请求超时。
    Timeout,
    /// 连接失败。
    Connect,
    /// HTTP 非成功响应。
    Http { status: u16, url: String },
    /// JSON 解析失败。
    Json { url: String, source: String },
    /// SHA256 校验失败。
    Sha256Mismatch { url: String },
    /// 所有镜像均不可用。
    AllMirrorsFailed { urls: Vec<String> },
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Timeout => write!(f, "请求超时"),
            HttpError::Connect => write!(f, "连接失败"),
            HttpError::Http { status, url } => {
                write_http_error(f, *status, url)
            }
            HttpError::Json { url, source } => write_json_error(f, url, source),
            HttpError::Sha256Mismatch { url } => write_sha256_error(f, url),
            HttpError::AllMirrorsFailed { urls } => {
                write_all_mirrors_failed(f, urls)
            }
        }
    }
}

fn write_http_error(
    f: &mut fmt::Formatter<'_>,
    status: u16,
    url: &str,
) -> fmt::Result {
    write!(f, "HTTP {}: {}", status, redact_url_for_display(url))
}

fn write_json_error(
    f: &mut fmt::Formatter<'_>,
    url: &str,
    source: &str,
) -> fmt::Result {
    write!(
        f,
        "JSON 解析失败 ({}): {}",
        redact_url_for_display(url),
        source
    )
}

fn write_sha256_error(f: &mut fmt::Formatter<'_>, url: &str) -> fmt::Result {
    write!(f, "sha256 校验失败: {}", redact_url_for_display(url))
}

fn write_all_mirrors_failed(
    f: &mut fmt::Formatter<'_>,
    urls: &[String],
) -> fmt::Result {
    let safe: Vec<_> = urls.iter().map(|u| redact_url_for_display(u)).collect();
    write!(f, "所有镜像均不可用: {}", safe.join(", "))
}

impl std::error::Error for HttpError {}
