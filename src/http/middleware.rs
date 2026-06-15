use std::time::Duration;

use async_trait::async_trait;
use http::Extensions;
use reqwest::{Request, Response, Url};
use reqwest_middleware::{
    Error as MiddlewareError, Middleware, Next, Result as MiddlewareResult,
};
use tracing::warn;

use crate::filters::redact_url_for_display;
use crate::http::policy::RetryPolicy;

#[derive(Debug)]
pub struct MirrorRetryMiddleware {
    mirrors: Vec<Url>,
    policy: RetryPolicy,
}

impl MirrorRetryMiddleware {
    pub fn new(mirrors: Vec<Url>, policy: RetryPolicy) -> Self {
        Self { mirrors, policy }
    }
}

#[async_trait]
impl Middleware for MirrorRetryMiddleware {
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        if self.mirrors.is_empty() || !self.is_mirror_request(req.url()) {
            return next.run(req, extensions).await;
        }

        self.try_mirrors(req, extensions, next).await
    }
}

impl MirrorRetryMiddleware {
    fn is_mirror_request(&self, url: &Url) -> bool {
        self.mirrors.iter().any(|m| m.origin() == url.origin())
    }

    async fn try_mirrors(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        let mut last_error: Option<MiddlewareError> = None;
        let mut attempted_urls: Vec<String> = Vec::new();
        let mut last_status: Option<u16> = None;

        for mirror in &self.mirrors {
            match self
                .try_one_mirror(mirror, &req, extensions, next.clone())
                .await
            {
                Ok(resp) => return Ok(resp),
                Err(failure) => handle_mirror_failure(
                    failure,
                    mirror,
                    &mut attempted_urls,
                    &mut last_error,
                    &mut last_status,
                ),
            }
        }

        if attempted_urls.is_empty() {
            return Err(last_error.unwrap_or_else(make_no_mirror_error));
        }

        Err(MiddlewareError::middleware(
            MirrorRetryError::AllMirrorsFailed {
                urls: attempted_urls,
                last_status,
            },
        ))
    }

    async fn try_one_mirror(
        &self,
        mirror: &Url,
        req: &Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response, OneMirrorFailure> {
        let attempt_url =
            build_attempt_url(mirror, req.url()).map_err(|err| {
                OneMirrorFailure {
                    attempted_url: String::new(),
                    err,
                }
            })?;
        let attempt_req = clone_request_with_url(req, attempt_url.clone())
            .map_err(|err| OneMirrorFailure {
                attempted_url: attempt_url.to_string(),
                err,
            })?;
        try_mirror(mirror, &attempt_req, extensions, next, &self.policy)
            .await
            .map_err(|err| OneMirrorFailure {
                attempted_url: attempt_url.to_string(),
                err,
            })
    }
}

struct OneMirrorFailure {
    attempted_url: String,
    err: MiddlewareError,
}

fn record_mirror_failure(
    failure: OneMirrorFailure,
    mirror: &Url,
    attempted_urls: &mut Vec<String>,
    last_error: &mut Option<MiddlewareError>,
) {
    warn!("镜像 {} 不可用: {}", mirror, failure.err);
    attempted_urls.push(failure.attempted_url);
    *last_error = Some(failure.err);
}

fn update_last_status(err: &MiddlewareError, last_status: &mut Option<u16>) {
    if let MiddlewareError::Middleware(e) = err {
        *last_status = e
            .downcast_ref::<MirrorError>()
            .and_then(|m| m.1)
            .or(*last_status);
    }
}

fn handle_mirror_failure(
    failure: OneMirrorFailure,
    mirror: &Url,
    attempted_urls: &mut Vec<String>,
    last_error: &mut Option<MiddlewareError>,
    last_status: &mut Option<u16>,
) {
    update_last_status(&failure.err, last_status);
    record_mirror_failure(failure, mirror, attempted_urls, last_error);
}

fn make_no_mirror_error() -> MiddlewareError {
    MiddlewareError::middleware(MirrorError("没有可用镜像".to_string(), None))
}

#[derive(Debug)]
struct MirrorError(String, Option<u16>);

impl std::fmt::Display for MirrorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MirrorError {}

#[derive(Debug)]
pub enum MirrorRetryError {
    AllMirrorsFailed {
        urls: Vec<String>,
        /// 最后一个镜像返回的最终 HTTP 状态码（若存在）。
        last_status: Option<u16>,
    },
}

impl std::fmt::Display for MirrorRetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MirrorRetryError::AllMirrorsFailed { urls, .. } => {
                let safe: Vec<_> =
                    urls.iter().map(|u| redact_url_for_display(u)).collect();
                write!(f, "所有镜像均不可用: {}", safe.join(", "))
            }
        }
    }
}

impl std::error::Error for MirrorRetryError {}

enum AttemptOutcome {
    Return(Response),
    Retry,
    Fail(MiddlewareError),
}

async fn try_mirror(
    mirror: &Url,
    attempt_req: &Request,
    extensions: &mut Extensions,
    next: Next<'_>,
    policy: &RetryPolicy,
) -> MiddlewareResult<Response> {
    for attempt in 0..policy.max_attempts {
        let attempt_req = attempt_req.try_clone().ok_or_else(|| {
            MiddlewareError::middleware(MirrorError(
                "无法克隆请求".to_string(),
                None,
            ))
        })?;
        let result = next.clone().run(attempt_req, extensions).await;

        match evaluate_attempt(mirror, attempt, result, policy).await {
            AttemptOutcome::Return(resp) => return Ok(resp),
            AttemptOutcome::Retry => continue,
            AttemptOutcome::Fail(err) => return Err(err),
        }
    }

    Err(MiddlewareError::middleware(MirrorError(
        "镜像重试耗尽".to_string(),
        None,
    )))
}

fn build_attempt_url(mirror: &Url, original: &Url) -> MiddlewareResult<Url> {
    let path_and_query = build_path_and_query(original);
    mirror.join(&path_and_query).map_err(|e| {
        MiddlewareError::middleware(MirrorError(
            format!("镜像 URL 拼接失败: {e}"),
            None,
        ))
    })
}

fn clone_request_with_url(
    req: &Request,
    url: Url,
) -> MiddlewareResult<Request> {
    let mut attempt_req = req.try_clone().ok_or_else(|| {
        MiddlewareError::middleware(MirrorError(
            "无法克隆请求".to_string(),
            None,
        ))
    })?;
    *attempt_req.url_mut() = url;
    Ok(attempt_req)
}

async fn evaluate_attempt(
    mirror: &Url,
    attempt: usize,
    result: MiddlewareResult<Response>,
    policy: &RetryPolicy,
) -> AttemptOutcome {
    match result {
        Ok(resp) => evaluate_response(mirror, attempt, resp, policy).await,
        Err(err) => evaluate_error(mirror, attempt, err, policy).await,
    }
}

async fn evaluate_response(
    mirror: &Url,
    attempt: usize,
    resp: Response,
    policy: &RetryPolicy,
) -> AttemptOutcome {
    if resp.status().is_success() {
        return AttemptOutcome::Return(resp);
    }
    if should_retry_status(resp.status(), attempt, policy) {
        warn!("镜像 {} 返回 {}，准备重试", mirror, resp.status());
        sleep_before_retry(policy.backoff_ms).await;
        return AttemptOutcome::Retry;
    }
    AttemptOutcome::Fail(MiddlewareError::middleware(MirrorError(
        format!("镜像 {} 返回 {}", mirror, resp.status()),
        Some(resp.status().as_u16()),
    )))
}

fn should_retry_status(
    status: reqwest::StatusCode,
    attempt: usize,
    policy: &RetryPolicy,
) -> bool {
    (policy.retry_on_status)(status) && attempt + 1 < policy.max_attempts
}

async fn evaluate_error(
    mirror: &Url,
    attempt: usize,
    err: MiddlewareError,
    policy: &RetryPolicy,
) -> AttemptOutcome {
    if should_retry_error(&err, attempt, policy) {
        warn!("镜像 {} 请求失败: {err}，准备重试", mirror);
        sleep_before_retry(policy.backoff_ms).await;
        return AttemptOutcome::Retry;
    }
    AttemptOutcome::Fail(err)
}

fn should_retry_error(
    err: &MiddlewareError,
    attempt: usize,
    policy: &RetryPolicy,
) -> bool {
    let retryable = match err {
        MiddlewareError::Reqwest(e) => (policy.retry_on_error)(e),
        MiddlewareError::Middleware(_) => false,
    };
    retryable && attempt + 1 < policy.max_attempts
}

async fn sleep_before_retry(backoff_ms: u64) {
    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
}

fn build_path_and_query(url: &Url) -> String {
    let mut s = url.path().to_string();
    if let Some(q) = url.query() {
        s.push('?');
        s.push_str(q);
    }
    s
}

#[cfg(test)]
mod tests;
