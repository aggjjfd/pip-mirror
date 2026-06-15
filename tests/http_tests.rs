use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    Json, Router, body::Body, extract::State, http::StatusCode, routing::get,
};
use pip_mirror::http::{HttpClient, HttpError};
use serde_json::{Value, json};

async fn start_server(app: Router) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, handle)
}

#[derive(Clone)]
struct FlakyState {
    hits: Arc<AtomicUsize>,
}

async fn flaky_handler(State(state): State<FlakyState>) -> (StatusCode, Body) {
    let n = state.hits.fetch_add(1, Ordering::SeqCst);
    if n == 0 {
        (StatusCode::INTERNAL_SERVER_ERROR, Body::from("boom"))
    } else {
        (StatusCode::OK, Body::from(r#"{"name":"demo"}"#))
    }
}

#[tokio::test]
async fn test_single_mirror_500_then_retry_success() {
    let state = FlakyState {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/pypi/demo/json", get(flaky_handler))
        .with_state(state.clone());
    let (port, handle) = start_server(app).await;

    let client = HttpClient::builder()
        .with_mirrors(vec![format!("http://127.0.0.1:{port}")])
        .unwrap()
        .build()
        .unwrap();

    let value = client
        .get_json(&format!("http://127.0.0.1:{port}/pypi/demo/json"))
        .await
        .unwrap();
    assert_eq!(value["name"], "demo");
    assert_eq!(state.hits.load(Ordering::SeqCst), 2);

    handle.abort();
    let _ = handle.await;
}

#[derive(Clone)]
struct DownState {
    hits: Arc<AtomicUsize>,
}

async fn down_handler(State(state): State<DownState>) -> StatusCode {
    state.hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn ok_handler() -> Json<Value> {
    Json(json!({ "name": "demo" }))
}

#[tokio::test]
async fn test_multi_mirror_failover() {
    let down_state = DownState {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let down_app = Router::new()
        .route("/pypi/demo/json", get(down_handler))
        .with_state(down_state.clone());
    let (down_port, down_handle) = start_server(down_app).await;

    let ok_app = Router::new().route("/pypi/demo/json", get(ok_handler));
    let (ok_port, ok_handle) = start_server(ok_app).await;

    let client = HttpClient::builder()
        .with_mirrors(vec![
            format!("http://127.0.0.1:{down_port}"),
            format!("http://127.0.0.1:{ok_port}"),
        ])
        .unwrap()
        .build()
        .unwrap();

    let value = client
        .get_json(&format!("http://127.0.0.1:{down_port}/pypi/demo/json"))
        .await
        .unwrap();
    assert_eq!(value["name"], "demo");
    assert_eq!(down_state.hits.load(Ordering::SeqCst), 3);

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
async fn test_non_mirror_origin_url_not_rewritten() {
    let capture_state = CaptureState::default();
    let capture_app = Router::new()
        .route("/explicit.whl", get(capture_handler))
        .with_state(capture_state.clone());
    let (capture_port, capture_handle) = start_server(capture_app).await;

    // 配置一个镜像，但请求发往另一个主机；不应被重写。
    let client = HttpClient::builder()
        .with_mirrors(vec!["https://example-mirror.example.com".to_string()])
        .unwrap()
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{capture_port}/explicit.whl");
    let bytes = client.get_bytes(&url).await.unwrap();
    assert!(!bytes.is_empty());

    let paths = capture_state.paths.lock().await;
    assert_eq!(paths.as_slice(), ["/explicit.whl"]);

    capture_handle.abort();
    let _ = capture_handle.await;
}

#[derive(Clone)]
struct FlakyJsonState {
    hits: Arc<AtomicUsize>,
}

async fn flaky_json_handler(
    State(state): State<FlakyJsonState>,
) -> (StatusCode, Body) {
    let n = state.hits.fetch_add(1, Ordering::SeqCst);
    if n < 2 {
        (StatusCode::OK, Body::from("not json"))
    } else {
        (StatusCode::OK, Body::from(r#"{"name":"demo"}"#))
    }
}

#[tokio::test]
async fn test_json_decode_retry_success() {
    let state = FlakyJsonState {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/flaky.json", get(flaky_json_handler))
        .with_state(state.clone());
    let (port, handle) = start_server(app).await;

    let client = HttpClient::builder().build().unwrap();
    let url = format!("http://127.0.0.1:{port}/flaky.json");
    let value = client.get_json(&url).await.unwrap();

    assert_eq!(value["name"], "demo");
    assert!(state.hits.load(Ordering::SeqCst) > 1);

    handle.abort();
    let _ = handle.await;
}

async fn invalid_json_handler() -> (StatusCode, Body) {
    (StatusCode::OK, Body::from("not json"))
}

#[tokio::test]
async fn test_json_decode_error_wrapped_as_http_error_json() {
    let app = Router::new().route("/bad.json", get(invalid_json_handler));
    let (port, handle) = start_server(app).await;

    let client = HttpClient::builder().build().unwrap();
    let url = format!("http://127.0.0.1:{port}/bad.json");
    let err = client.get_json(&url).await.unwrap_err();

    match err {
        HttpError::Json { url: err_url, .. } => {
            assert_eq!(err_url, url);
        }
        other => panic!("expected HttpError::Json, got {other:?}"),
    }

    handle.abort();
    let _ = handle.await;
}
