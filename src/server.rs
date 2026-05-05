use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tower_http::cors::{Any, CorsLayer};

use crate::access_log::AccessLogger;

#[derive(Clone)]
struct AppState {
    repo_dir: Arc<PathBuf>,
    #[allow(dead_code)]
    access_logger: Arc<AccessLogger>,
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/simple/{*tail}", get(serve_simple))
        .route("/python-builds/index.json", get(serve_python_builds_index))
        .route("/python-builds/{*tail}", get(serve_python_builds_file))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET])
                .allow_headers([header::CONTENT_TYPE, header::ACCEPT]),
        )
        .with_state(state)
}

fn make_state(repo_dir: PathBuf) -> AppState {
    let access_logger = Arc::new(
        AccessLogger::open(&repo_dir.join(".access_log.db"))
            .unwrap_or_else(|e| panic!("无法打开 access_log.db: {e}")),
    );
    AppState {
        repo_dir: Arc::new(repo_dir),
        access_logger,
    }
}

pub async fn start_server(
    host: &str,
    port: u16,
    repository_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if !repository_dir.exists() {
        return Err(
            format!("仓库目录不存在: {}", repository_dir.display()).into()
        );
    }
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("PIP 镜像服务器启动\n  地址: http://{host}:{port}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, build_router(make_state(repository_dir))).await?;
    Ok(())
}

/// Determine the file path to serve: either index.html (for dirs) or the file itself.
pub fn resolve_serve_path(base: &Path, tail: &str) -> (PathBuf, PathBuf) {
    let path = if tail.is_empty() {
        base.to_path_buf()
    } else {
        base.join(tail)
    };
    let json_path = if path.is_dir() {
        path.join("index.json")
    } else {
        path.with_file_name(format!(
            "{}/index.json",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    };
    let serve_path = if path.is_dir() {
        path.join("index.html")
    } else {
        path
    };
    (json_path, serve_path)
}

fn try_serve_json(body: Vec<u8>) -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", "application/vnd.pypi.simple.v1+json")
        .body(axum::body::Body::from(body))
        .unwrap()
}

pub fn content_type_for(path: &Path) -> &'static str {
    if path.extension().is_some_and(|e| e == "json") {
        "application/vnd.pypi.simple.v1+json"
    } else {
        "application/vnd.pypi.simple.v1+html"
    }
}

fn serve_file_response(body: Vec<u8>, path: &Path) -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", content_type_for(path))
        .header("Access-Control-Allow-Origin", "*")
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn wants_json(req: &axum::extract::Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .contains("application/vnd.pypi.simple.v1+json")
}

async fn serve_simple(
    State(state): State<AppState>,
    axum::extract::Path(tail): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> Response {
    let (json_path, serve_path) =
        resolve_serve_path(&state.repo_dir.join("simple"), &tail);
    if wants_json(&req)
        && json_path.exists()
        && let Ok(body) = tokio::fs::read(&json_path).await
    {
        return try_serve_json(body);
    }
    if !serve_path.exists() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    match tokio::fs::read(&serve_path).await {
        Ok(body) => serve_file_response(body, &serve_path),
        Err(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Read error").into_response()
        }
    }
}

pub fn rewrite_relative_urls(data: &mut serde_json::Value, base: &str) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    for entry in obj.values_mut() {
        let Some(url) = entry.get("url").and_then(|u| u.as_str()) else {
            continue;
        };
        if url.starts_with('/') {
            entry["url"] = serde_json::Value::String(format!("{base}{url}"));
        }
    }
}

async fn serve_python_builds_file(
    State(state): State<AppState>,
    axum::extract::Path(tail): axum::extract::Path<String>,
) -> Response {
    let path = state.repo_dir.join("python-builds").join(&tail);
    match tokio::fs::read(&path).await {
        Ok(body) => {
            let mime = if tail.ends_with(".json") {
                "application/json"
            } else {
                "application/octet-stream"
            };
            Response::builder()
                .status(200)
                .header("Content-Type", mime)
                .body(axum::body::Body::from(body))
                .unwrap()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

async fn serve_python_builds_index(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let path = state.repo_dir.join("python-builds").join("index.json");
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    let mut data = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed: {e}"))
                .into_response();
        }
    };
    rewrite_relative_urls(&mut data, &format!("http://{host}"));
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string_pretty(&data).unwrap(),
        ))
        .unwrap()
}
