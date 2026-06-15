use reqwest::StatusCode;

/// 重试策略。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 单个镜像最大尝试次数。
    pub max_attempts: usize,
    /// 重试间隔（毫秒）。
    pub backoff_ms: u64,
    /// 遇到哪些 HTTP 状态码时重试。
    pub retry_on_status: fn(StatusCode) -> bool,
    /// 遇到哪些 reqwest 错误时重试。
    pub retry_on_error: fn(&reqwest::Error) -> bool,
    /// 遇到哪些响应体解析错误时重试。
    pub retry_on_body_error: fn(&serde_json::Error) -> bool,
}

fn default_retry_on_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT
}

fn default_retry_on_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

fn default_retry_on_body_error(_err: &serde_json::Error) -> bool {
    true
}

impl RetryPolicy {
    /// 默认镜像同步策略：3 次尝试、100ms 退避，遇到 5xx/408/超时/连接/请求错误时重试。
    pub fn mirror_default() -> Self {
        Self {
            max_attempts: 3,
            backoff_ms: 100,
            retry_on_status: default_retry_on_status,
            retry_on_error: default_retry_on_error,
            retry_on_body_error: default_retry_on_body_error,
        }
    }
}
