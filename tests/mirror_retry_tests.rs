use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router, body::Body, extract::State, http::StatusCode, routing::get,
};
use pip_mirror::downloader::client::MirrorRetryMiddleware;
use serde_json::{Value, json};

fn mirror_client(
    mirrors: Vec<String>,
) -> reqwest_middleware::ClientWithMiddleware {
    let origins = mirrors
        .into_iter()
        .map(|s| reqwest::Url::parse(&s).unwrap())
        .collect();
    reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(MirrorRetryMiddleware::new(origins))
        .build()
}

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
        (StatusCode::OK, Body::from("ok"))
    }
}

#[tokio::test]
async fn test_mirror_retries_once_on_server_error() {
    let state = FlakyState {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/pypi/demo/json", get(flaky_handler))
        .with_state(state.clone());
    let (port, handle) = start_server(app).await;

    let client = mirror_client(vec![format!("http://127.0.0.1:{port}")]);
    let resp = client
        .get(format!("http://127.0.0.1:{port}/pypi/demo/json"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
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
    StatusCode::NOT_FOUND
}

async fn ok_handler() -> Json<Value> {
    Json(json!({ "name": "demo" }))
}

#[tokio::test]
async fn test_mirror_failover_to_second_mirror() {
    let down_state = DownState {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let down_app = Router::new()
        .route("/pypi/demo/json", get(down_handler))
        .with_state(down_state.clone());
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
    assert_eq!(down_state.hits.load(Ordering::SeqCst), 2);

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
