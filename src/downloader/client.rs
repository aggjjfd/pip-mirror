use std::time::Duration;

use async_trait::async_trait;
use http::Extensions;
use reqwest::{Request, Response, Url};
use reqwest_middleware::{
    Error as MiddlewareError, Middleware, Next, Result as MiddlewareResult,
};
use tracing::warn;

#[derive(Debug)]
struct MirrorError(String);

impl std::fmt::Display for MirrorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MirrorError {}

fn mirror_error(msg: impl Into<String>) -> MirrorError {
    MirrorError(msg.into())
}

pub struct MirrorRetryMiddleware {
    mirrors: Vec<Url>,
}

impl MirrorRetryMiddleware {
    pub fn new(mirrors: Vec<Url>) -> Self {
        Self { mirrors }
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
        self.mirrors
            .iter()
            .any(|mirror| mirror.origin() == url.origin())
    }

    async fn try_mirrors(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        let mut last_error: Option<MiddlewareError> = None;

        for mirror in &self.mirrors {
            let result =
                try_mirror(mirror, &req, extensions, next.clone()).await;
            match handle_mirror_result(mirror, result, &mut last_error) {
                Some(resp) => return Ok(resp),
                None => continue,
            }
        }

        Err(last_error.unwrap_or_else(|| {
            MiddlewareError::middleware(mirror_error("所有镜像均不可用"))
        }))
    }
}

fn handle_mirror_result(
    mirror: &Url,
    result: MiddlewareResult<Response>,
    last_error: &mut Option<MiddlewareError>,
) -> Option<Response> {
    match result {
        Ok(resp) => Some(resp),
        Err(err) => {
            warn!("镜像 {} 不可用", mirror);
            *last_error = Some(err);
            None
        }
    }
}

enum AttemptOutcome {
    Return(Response),
    Retry,
    Fail(MiddlewareError),
}

async fn try_mirror(
    mirror: &Url,
    req: &Request,
    extensions: &mut Extensions,
    next: Next<'_>,
) -> MiddlewareResult<Response> {
    let attempt_url = build_attempt_url(mirror, req.url())?;

    for attempt in 0..3 {
        let attempt_req = clone_request(req, attempt_url.clone())?;
        let result = next.clone().run(attempt_req, extensions).await;

        match evaluate_attempt(mirror, attempt, result).await {
            AttemptOutcome::Return(resp) => return Ok(resp),
            AttemptOutcome::Retry => continue,
            AttemptOutcome::Fail(err) => return Err(err),
        }
    }

    Err(MiddlewareError::middleware(mirror_error("镜像重试耗尽")))
}

fn build_attempt_url(mirror: &Url, original: &Url) -> MiddlewareResult<Url> {
    let path_and_query = build_path_and_query(original);
    mirror.join(&path_and_query).map_err(|e| {
        MiddlewareError::middleware(mirror_error(format!(
            "镜像 URL 拼接失败: {e}"
        )))
    })
}

async fn evaluate_attempt(
    mirror: &Url,
    attempt: usize,
    result: MiddlewareResult<Response>,
) -> AttemptOutcome {
    match result {
        Ok(resp) => evaluate_response(mirror, attempt, resp).await,
        Err(err) => evaluate_error(mirror, attempt, err),
    }
}

async fn evaluate_response(
    mirror: &Url,
    attempt: usize,
    resp: Response,
) -> AttemptOutcome {
    if resp.status().is_success() {
        return AttemptOutcome::Return(resp);
    }
    if attempt == 0 {
        warn!("镜像 {} 返回 {}，准备重试", mirror, resp.status());
        sleep_before_retry().await;
        return AttemptOutcome::Retry;
    }
    AttemptOutcome::Fail(MiddlewareError::middleware(mirror_error(format!(
        "镜像 {} 返回 {}",
        mirror,
        resp.status()
    ))))
}

fn evaluate_error(
    mirror: &Url,
    attempt: usize,
    err: MiddlewareError,
) -> AttemptOutcome {
    if should_retry(&err) && attempt == 0 {
        warn!("镜像 {} 请求失败: {err}，准备重试", mirror);
        AttemptOutcome::Retry
    } else {
        AttemptOutcome::Fail(err)
    }
}

async fn sleep_before_retry() {
    tokio::time::sleep(Duration::from_millis(100)).await;
}

fn clone_request(req: &Request, url: Url) -> MiddlewareResult<Request> {
    let mut attempt_req = req.try_clone().ok_or_else(|| {
        MiddlewareError::middleware(mirror_error("无法克隆请求"))
    })?;
    *attempt_req.url_mut() = url;
    Ok(attempt_req)
}

fn build_path_and_query(url: &Url) -> String {
    let mut s = url.path().to_string();
    if let Some(q) = url.query() {
        s.push('?');
        s.push_str(q);
    }
    s
}

fn should_retry(err: &MiddlewareError) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_path_and_query() {
        let url = Url::parse("https://pypi.org/pypi/pytz/json?a=1").unwrap();
        assert_eq!(build_path_and_query(&url), "/pypi/pytz/json?a=1");
    }

    #[test]
    fn test_build_path_without_query() {
        let url = Url::parse("https://pypi.org/pypi/pytz/json").unwrap();
        assert_eq!(build_path_and_query(&url), "/pypi/pytz/json");
    }
}
