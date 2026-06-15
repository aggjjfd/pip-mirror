use std::time::Duration;

use reqwest::Response;
use reqwest_middleware::{
    ClientBuilder as MiddlewareClientBuilder, ClientWithMiddleware,
    Error as MiddlewareError,
};
use serde_json::Value;
use url::Url;

use crate::http::error::HttpError;
use crate::http::middleware::{MirrorRetryError, MirrorRetryMiddleware};
use crate::http::policy::RetryPolicy;

/// 构建 HTTP 客户端时发生的错误。
#[derive(Debug)]
pub enum HttpClientError {
    /// 镜像地址解析失败。
    InvalidMirror(String),
    /// 底层 reqwest 客户端构建失败。
    Build(reqwest::Error),
}

impl std::fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpClientError::InvalidMirror(s) => {
                write!(f, "镜像地址无效: {s}")
            }
            HttpClientError::Build(e) => write!(f, "构建 HTTP 客户端失败: {e}"),
        }
    }
}

impl std::error::Error for HttpClientError {}

/// 构建 [`HttpClient`] 的配置器。
pub struct HttpClientBuilder {
    mirrors: Vec<String>,
    timeout_secs: u64,
    policy: RetryPolicy,
}

impl HttpClientBuilder {
    /// 创建一个新的构建器。
    pub fn new() -> Self {
        Self {
            mirrors: Vec::new(),
            timeout_secs: 300,
            policy: RetryPolicy::mirror_default(),
        }
    }

    /// 配置镜像列表；会校验每个 URL 是否可解析。
    pub fn with_mirrors(
        mut self,
        mirrors: Vec<String>,
    ) -> Result<Self, HttpClientError> {
        for url in &mirrors {
            validate_mirror_url(url)?;
        }
        self.mirrors = mirrors;
        Ok(self)
    }

    /// 配置请求超时时间（秒）。
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// 配置重试策略。
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// 构建 [`HttpClient`]。
    pub fn build(self) -> Result<HttpClient, HttpClientError> {
        let origins = parse_mirror_origins(&self.mirrors)?;

        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(HttpClientError::Build)?;

        let client = MiddlewareClientBuilder::new(inner)
            .with(MirrorRetryMiddleware::new(origins, self.policy.clone()))
            .build();

        Ok(HttpClient {
            client,
            policy: self.policy,
        })
    }
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_mirror_url(url: &str) -> Result<(), HttpClientError> {
    Url::parse(url)
        .map(|_| ())
        .map_err(|e| HttpClientError::InvalidMirror(format!("{url}: {e}")))
}

fn parse_mirror_origins(
    mirrors: &[String],
) -> Result<Vec<Url>, HttpClientError> {
    mirrors
        .iter()
        .map(|s| Url::parse(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HttpClientError::InvalidMirror(e.to_string()))
}

/// 统一 HTTP 客户端，内置镜像重试中间件。
#[derive(Clone)]
pub struct HttpClient {
    client: ClientWithMiddleware,
    policy: RetryPolicy,
}

impl HttpClient {
    /// 创建一个新的构建器。
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::new()
    }

    /// 访问底层带中间件的 reqwest 客户端。
    ///
    /// 用于需要直接操作响应流或保持现有接口（如 [`MetadataCache`]）的场景。
    pub(crate) fn inner(&self) -> &ClientWithMiddleware {
        &self.client
    }

    /// 发送 GET 请求并解析为 JSON。
    ///
    /// 若响应体 JSON 解析失败且策略允许，会按 `RetryPolicy` 进行重试。
    pub async fn get_json(&self, url: &str) -> Result<Value, HttpError> {
        for attempt in 0..self.policy.max_attempts {
            match self.try_get_json_once(url, attempt).await? {
                std::ops::ControlFlow::Continue(()) => continue,
                std::ops::ControlFlow::Break(value) => return Ok(value),
            }
        }

        // max_attempts 为 0 时循环体不会执行，安全兜底。
        Err(HttpError::Http {
            status: 0,
            url: url.to_string(),
        })
    }

    async fn try_get_json_once(
        &self,
        url: &str,
        attempt: usize,
    ) -> Result<std::ops::ControlFlow<Value, ()>, HttpError> {
        let resp = self.send_get(url).await?;
        check_success_status(url, resp.status())?;
        let bytes =
            resp.bytes().await.map_err(|e| map_reqwest_error(url, e))?;

        let parsed = serde_json::from_slice::<Value>(&bytes);
        if should_retry_json_error(&parsed, attempt, &self.policy) {
            tokio::time::sleep(Duration::from_millis(self.policy.backoff_ms))
                .await;
            return Ok(std::ops::ControlFlow::Continue(()));
        }

        parsed.map(std::ops::ControlFlow::Break).map_err(|err| {
            HttpError::Json {
                url: url.to_string(),
                source: err.to_string(),
            }
        })
    }

    /// 发送 GET 请求并读取完整响应体为字节。
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        let resp = self.send_get(url).await?;
        check_success_status(url, resp.status())?;
        let bytes =
            resp.bytes().await.map_err(|e| map_reqwest_error(url, e))?;
        Ok(bytes.to_vec())
    }

    /// 发送 GET 请求并读取完整响应体为文本。
    pub async fn get_text(&self, url: &str) -> Result<String, HttpError> {
        let resp = self.send_get(url).await?;
        check_success_status(url, resp.status())?;
        resp.text().await.map_err(|e| map_reqwest_error(url, e))
    }

    async fn send_get(&self, url: &str) -> Result<Response, HttpError> {
        self.client
            .get(url)
            .send()
            .await
            .map_err(|e| map_middleware_error(url, e))
    }
}

fn should_retry_json_error(
    parsed: &Result<Value, serde_json::Error>,
    attempt: usize,
    policy: &RetryPolicy,
) -> bool {
    match parsed {
        Ok(_) => false,
        Err(err) => {
            attempt + 1 < policy.max_attempts
                && (policy.retry_on_body_error)(err)
        }
    }
}

fn check_success_status(
    url: &str,
    status: reqwest::StatusCode,
) -> Result<(), HttpError> {
    if status.is_success() {
        Ok(())
    } else {
        Err(HttpError::Http {
            status: status.as_u16(),
            url: url.to_string(),
        })
    }
}

fn map_middleware_error(url: &str, err: MiddlewareError) -> HttpError {
    match err {
        MiddlewareError::Reqwest(err) => map_reqwest_error(url, err),
        MiddlewareError::Middleware(err) => map_mirror_retry_error(url, &err),
    }
}

fn map_mirror_retry_error(url: &str, err: &anyhow::Error) -> HttpError {
    if let Some(MirrorRetryError::AllMirrorsFailed { urls, last_status }) =
        err.downcast_ref()
    {
        if let Some(status) = last_status {
            return HttpError::Http {
                status: *status,
                url: urls.last().cloned().unwrap_or_else(|| url.to_string()),
            };
        }
        return HttpError::AllMirrorsFailed { urls: urls.clone() };
    }

    HttpError::Http {
        status: 0,
        url: url.to_string(),
    }
}

fn map_reqwest_error(url: &str, err: reqwest::Error) -> HttpError {
    if err.is_timeout() {
        HttpError::Timeout
    } else if err.is_connect() {
        HttpError::Connect
    } else if let Some(status) = err.status() {
        HttpError::Http {
            status: status.as_u16(),
            url: url.to_string(),
        }
    } else {
        HttpError::Http {
            status: 0,
            url: url.to_string(),
        }
    }
}
