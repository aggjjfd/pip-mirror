use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router, body::Body, extract::State, http::StatusCode, routing::get,
};
use reqwest_middleware::{
    ClientBuilder as MiddlewareClientBuilder, ClientWithMiddleware,
};
use serde_json::{Value, json};

use super::*;
use crate::http::RetryPolicy;

type Hits = Arc<AtomicUsize>;

fn mirror_client(mirrors: Vec<String>) -> ClientWithMiddleware {
    let origins = mirrors
        .into_iter()
        .map(|s| reqwest::Url::parse(&s).unwrap())
        .collect();
    MiddlewareClientBuilder::new(reqwest::Client::new())
        .with(MirrorRetryMiddleware::new(
            origins,
            RetryPolicy::mirror_default(),
        ))
        .build()
}

async fn start_server(app: Router) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle =
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, handle)
}

async fn flaky_handler(State(hits): State<Hits>) -> (StatusCode, Body) {
    if hits.fetch_add(1, Ordering::SeqCst) == 0 {
        (StatusCode::INTERNAL_SERVER_ERROR, Body::from("boom"))
    } else {
        (StatusCode::OK, Body::from("ok"))
    }
}

#[tokio::test]
async fn test_mirror_retries_once_on_server_error() {
    let hits: Hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/pypi/demo/json", get(flaky_handler))
        .with_state(hits.clone());
    let (port, handle) = start_server(app).await;

    let client = mirror_client(vec![format!("http://127.0.0.1:{port}")]);
    let resp = client
        .get(format!("http://127.0.0.1:{port}/pypi/demo/json"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    handle.abort();
    let _ = handle.await;
}

async fn down_handler(State(hits): State<Hits>) -> StatusCode {
    hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NOT_FOUND
}

async fn ok_handler() -> Json<Value> {
    Json(json!({ "name": "demo" }))
}

#[tokio::test]
async fn test_mirror_failover_to_second_mirror() {
    let down_hits: Hits = Arc::new(AtomicUsize::new(0));
    let down_app = Router::new()
        .route("/pypi/demo/json", get(down_handler))
        .with_state(down_hits.clone());
    let (down_port, down_handle) = start_server(down_app).await;

    let ok_app = Router::new().route("/pypi/demo/json", get(ok_handler));
    let (ok_port, ok_handle) = start_server(ok_app).await;

    let client = mirror_client(vec![
        format!("http://127.0.0.1:{down_port}"),
        format!("http://127.0.0.1:{ok_port}"),
    ]);

    let resp = client
        .get(format!("http://127.0.0.1:{down_port}/pypi/demo/json"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    // 默认策略不会重试 404（仅重试 5xx/408），因此 down 镜像只被命中一次。
    assert_eq!(down_hits.load(Ordering::SeqCst), 1);

    down_handle.abort();
    ok_handle.abort();
    let _ = down_handle.await;
    let _ = ok_handle.await;
}

#[derive(Clone, Default)]
struct CaptureState {
    paths: Arc<tokio::sync::Mutex<Vec<String>>>,
}

async fn capture_handler(
    State(state): State<CaptureState>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> Json<Value> {
    state.paths.lock().await.push(uri.to_string());
    Json(json!({ "ok": true }))
}

#[tokio::test]
async fn test_explicit_url_not_rewritten_to_mirror() {
    let capture_state = CaptureState::default();
    let capture_app = Router::new()
        .route("/explicit.whl", get(capture_handler))
        .with_state(capture_state.clone());
    let (capture_port, capture_handle) = start_server(capture_app).await;

    // 配置一个镜像，但请求发往另一个主机；不应被重写。
    let client =
        mirror_client(vec!["https://example-mirror.example.com".to_string()]);
    let url = format!("http://127.0.0.1:{capture_port}/explicit.whl");
    let resp = client.get(&url).send().await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let paths = capture_state.paths.lock().await;
    assert_eq!(paths.as_slice(), ["/explicit.whl"]);

    capture_handle.abort();
    let _ = capture_handle.await;
}

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
