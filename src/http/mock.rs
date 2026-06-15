//! HTTP mock 传输层，用于测试注入预置响应或错误。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use http::Extensions;
use reqwest::{Request, Response};
use reqwest_middleware::{
    Error as MiddlewareError, Middleware, Next, Result as MiddlewareResult,
};
use serde_json::Value;

use crate::http::error::HttpError;

/// 按 URL 匹配返回预置响应或错误的 mock 中间件。
#[derive(Clone)]
pub struct MockMiddleware {
    responses: Arc<HashMap<String, MockResponse>>,
}

#[derive(Clone)]
enum MockResponse {
    Success { status: u16, body: Vec<u8> },
    Error(HttpError),
}

impl MockMiddleware {
    /// 创建一个新的 mock middleware 构建器。
    pub fn builder() -> MockMiddlewareBuilder<NoUrl> {
        MockMiddlewareBuilder::new()
    }
}

/// 尚未指定 URL 的 builder 状态。
pub struct NoUrl;

/// 已指定 URL 的 builder 状态。
pub struct WithUrl(String);

/// MockMiddleware 构建器（type-state，编译期保证先调用 `when`）。
pub struct MockMiddlewareBuilder<State> {
    responses: HashMap<String, MockResponse>,
    state: State,
}

impl MockMiddlewareBuilder<NoUrl> {
    /// 创建新构建器。
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
            state: NoUrl,
        }
    }

    /// 指定接下来响应匹配的 URL。
    pub fn when(
        mut self,
        url: impl Into<String>,
    ) -> MockMiddlewareBuilder<WithUrl> {
        let url = url.into();
        // 用占位条目记录当前正在配置的 URL；后续 then_* 方法会替换它。
        self.responses.insert(
            url.clone(),
            MockResponse::Success {
                status: 0,
                body: Vec::new(),
            },
        );
        MockMiddlewareBuilder {
            responses: self.responses,
            state: WithUrl(url),
        }
    }

    /// 构建 mock middleware。
    pub fn build(self) -> MockMiddleware {
        MockMiddleware {
            responses: Arc::new(self.responses),
        }
    }
}

impl MockMiddlewareBuilder<WithUrl> {
    fn replace_current(
        self,
        response: MockResponse,
    ) -> MockMiddlewareBuilder<NoUrl> {
        let url = self.state.0;
        let mut responses = self.responses;
        responses.insert(url, response);
        MockMiddlewareBuilder {
            responses,
            state: NoUrl,
        }
    }

    /// 返回指定状态码与 body。
    pub fn then_status(
        self,
        status: u16,
        body: Vec<u8>,
    ) -> MockMiddlewareBuilder<NoUrl> {
        self.replace_current(MockResponse::Success { status, body })
    }

    /// 返回指定状态码与 JSON body。
    pub fn then_json(
        self,
        status: u16,
        value: Value,
    ) -> MockMiddlewareBuilder<NoUrl> {
        let body = serde_json::to_vec(&value).expect("JSON 序列化失败");
        self.then_status(status, body)
    }

    /// 返回指定 HTTP 错误（包装为 MiddlewareError）。
    pub fn then_error(self, err: HttpError) -> MockMiddlewareBuilder<NoUrl> {
        self.replace_current(MockResponse::Error(err))
    }
}

impl Default for MockMiddlewareBuilder<NoUrl> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Middleware for MockMiddleware {
    async fn handle(
        &self,
        req: Request,
        _extensions: &mut Extensions,
        _next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        let url = req.url().to_string();
        match self.responses.get(&url) {
            Some(MockResponse::Success { status, body }) => {
                build_response(*status, body.clone())
            }
            Some(MockResponse::Error(err)) => Err(MiddlewareError::Middleware(
                anyhow::Error::new(err.clone()),
            )),
            None => Err(MiddlewareError::Middleware(anyhow::Error::new(
                HttpError::Http {
                    status: 404,
                    url: url.clone(),
                },
            ))),
        }
    }
}

fn build_response(status: u16, body: Vec<u8>) -> MiddlewareResult<Response> {
    let response = http::Response::builder()
        .status(status)
        .body(reqwest::Body::from(body))
        .map_err(|e| MiddlewareError::Middleware(anyhow::Error::new(e)))?;
    Ok(Response::from(response))
}

#[cfg(test)]
mod tests {
    use reqwest_middleware::ClientBuilder as MiddlewareClientBuilder;

    use super::*;
    use crate::http::{HttpClient, HttpError, RetryPolicy};

    fn mock_client(mock: MockMiddleware) -> HttpClient {
        let inner = reqwest::Client::new();
        let client = MiddlewareClientBuilder::new(inner).with(mock).build();
        HttpClient::from_client_with_policy(
            client,
            RetryPolicy::mirror_default(),
        )
    }

    #[tokio::test]
    async fn test_mock_json_response() {
        let mock = MockMiddleware::builder()
            .when("https://pypi.org/pypi/demo/json")
            .then_json(200, serde_json::json!({"name": "demo"}))
            .build();
        let client = mock_client(mock);

        let value = client
            .get_json("https://pypi.org/pypi/demo/json")
            .await
            .unwrap();
        assert_eq!(value["name"], "demo");
    }

    #[tokio::test]
    async fn test_mock_error_response() {
        let url = "https://pypi.org/pypi/bad/json";
        let mock = MockMiddleware::builder()
            .when(url)
            .then_error(HttpError::Http {
                status: 500,
                url: url.into(),
            })
            .build();
        let client = mock_client(mock);

        let err = client.get_json(url).await.unwrap_err();
        assert!(
            matches!(
                err,
                HttpError::Http { status: 500, url: ref err_url } if err_url == url
            ),
            "expected HttpError::Http(500, {url}), got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_mock_bytes_response() {
        let url = "https://pypi.org/packages/demo/demo-1.0.whl";
        let wheel_bytes = b"wheel data".to_vec();
        let mock = MockMiddleware::builder()
            .when(url)
            .then_status(200, wheel_bytes.clone())
            .build();
        let client = mock_client(mock);

        let bytes = client.get_bytes(url).await.unwrap();
        assert_eq!(bytes, wheel_bytes);
    }

    #[tokio::test]
    async fn test_mock_unmatched_url_returns_404() {
        let mock = MockMiddleware::builder()
            .when("https://pypi.org/pypi/demo/json")
            .then_json(200, serde_json::json!({"name": "demo"}))
            .build();
        let client = mock_client(mock);

        let err = client
            .get_json("https://pypi.org/pypi/other/json")
            .await
            .unwrap_err();
        assert!(
            matches!(err, HttpError::Http { status: 404, .. }),
            "expected 404 for unmatched URL, got {err:?}"
        );
    }
}
