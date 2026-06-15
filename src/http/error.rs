use std::fmt;

use crate::redact::redact_url_for_display;

/// 统一 HTTP 错误类型，避免在日志/错误信息中泄露凭证。
#[derive(Clone)]
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
    /// 重试次数已耗尽（包括 max_attempts 为 0 的退化情况）。
    RetryExhausted { url: String },
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
            HttpError::RetryExhausted { url } => {
                write!(f, "重试次数耗尽: {}", redact_url_for_display(url))
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

impl fmt::Debug for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Timeout => f.debug_struct("Timeout").finish(),
            HttpError::Connect => f.debug_struct("Connect").finish(),
            HttpError::Http { status, url } => debug_http(f, *status, url),
            HttpError::Json { url, source } => debug_json(f, url, source),
            HttpError::Sha256Mismatch { url } => debug_sha256(f, url),
            HttpError::AllMirrorsFailed { urls } => {
                debug_all_mirrors_failed(f, urls)
            }
            HttpError::RetryExhausted { url } => debug_retry_exhausted(f, url),
        }
    }
}

fn debug_http(
    f: &mut fmt::Formatter<'_>,
    status: u16,
    url: &str,
) -> fmt::Result {
    f.debug_struct("Http")
        .field("status", &status)
        .field("url", &redact_url_for_display(url))
        .finish()
}

fn debug_json(
    f: &mut fmt::Formatter<'_>,
    url: &str,
    source: &str,
) -> fmt::Result {
    f.debug_struct("Json")
        .field("url", &redact_url_for_display(url))
        .field("source", &source)
        .finish()
}

fn debug_sha256(f: &mut fmt::Formatter<'_>, url: &str) -> fmt::Result {
    f.debug_struct("Sha256Mismatch")
        .field("url", &redact_url_for_display(url))
        .finish()
}

fn debug_all_mirrors_failed(
    f: &mut fmt::Formatter<'_>,
    urls: &[String],
) -> fmt::Result {
    let safe: Vec<_> = urls.iter().map(|u| redact_url_for_display(u)).collect();
    f.debug_struct("AllMirrorsFailed")
        .field("urls", &safe)
        .finish()
}

fn debug_retry_exhausted(f: &mut fmt::Formatter<'_>, url: &str) -> fmt::Result {
    f.debug_struct("RetryExhausted")
        .field("url", &redact_url_for_display(url))
        .finish()
}

impl std::error::Error for HttpError {}
