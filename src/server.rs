use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tower_http::cors::{Any, CorsLayer};

use crate::access_log::AccessLogger;
use crate::config::TargetSpec;

#[derive(Clone)]
struct AppState {
    repo_dir: Arc<PathBuf>,
    #[allow(dead_code)]
    access_logger: Arc<AccessLogger>,
    targets: Arc<Vec<TargetSpec>>,
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/fonts/{name}", get(serve_font))
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

const HM_SANS_FONT: &[u8] = include_bytes!("fonts/hm-sans-subset.woff2");
const FIRA_CODE_FONT: &[u8] = include_bytes!("fonts/fira-code-subset.woff2");

async fn serve_font(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let (body, content_type) = match name.as_str() {
        "hm-sans-subset.woff2" => (HM_SANS_FONT, "font/woff2"),
        "fira-code-subset.woff2" => (FIRA_CODE_FONT, "font/woff2"),
        _ => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    Response::builder()
        .status(200)
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=31536000")
        .body(Body::from(body))
        .unwrap()
}

async fn serve_index(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let html = crate::index_page::render(&state.targets, &state.repo_dir, host);
    Response::builder()
        .status(200)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}

fn make_state(repo_dir: PathBuf, targets: Vec<TargetSpec>) -> AppState {
    let access_logger = Arc::new(
        AccessLogger::open(&repo_dir.join(".access_log.db"))
            .unwrap_or_else(|e| panic!("无法打开 access_log.db: {e}")),
    );
    AppState {
        repo_dir: Arc::new(repo_dir),
        access_logger,
        targets: Arc::new(targets),
    }
}

pub async fn start_server(
    host: &str,
    port: u16,
    repository_dir: PathBuf,
    targets: Vec<TargetSpec>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !repository_dir.exists() {
        return Err(
            format!("仓库目录不存在: {}", repository_dir.display()).into()
        );
    }
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("PIP 镜像服务器启动\n  地址: http://{host}:{port}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, build_router(make_state(repository_dir, targets)))
        .await?;
    Ok(())
}

/// Determine the file path to serve: either index.html (for dirs) or the file
/// itself.
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

fn file_body(
    file: tokio::fs::File,
) -> impl futures::Stream<Item = Result<axum::body::Bytes, std::io::Error>> {
    futures::stream::try_unfold(file, |mut file| async move {
        let mut buf = vec![0u8; 65536];
        match tokio::io::AsyncReadExt::read(&mut file, &mut buf).await {
            Ok(0) => Ok(None),
            Ok(n) => {
                buf.truncate(n);
                Ok(Some((axum::body::Bytes::from(buf), file)))
            }
            Err(e) => Err(e),
        }
    })
}

fn serve_stream_response(
    file: tokio::fs::File,
    content_type: &'static str,
) -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", content_type)
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from_stream(file_body(file)))
        .unwrap()
}

pub fn content_type_for(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "json" => "application/vnd.pypi.simple.v1+json",
        "whl" => "application/octet-stream",
        _ => "application/vnd.pypi.simple.v1+html",
    }
}

fn wants_json(req: &axum::extract::Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .contains("application/vnd.pypi.simple.v1+json")
}

fn wants_vendor_html(req: &axum::extract::Request) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .contains("application/vnd.pypi.simple.v1+html")
}

fn simple_content_type(
    req: &axum::extract::Request,
    path: &Path,
) -> &'static str {
    if wants_json(req) {
        return "application/vnd.pypi.simple.v1+json";
    }
    if wants_vendor_html(req) {
        return content_type_for(path);
    }
    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        return "application/vnd.pypi.simple.v1+json";
    }
    "text/html"
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
    let content_type = simple_content_type(&req, &serve_path);
    match tokio::fs::File::open(&serve_path).await {
        Ok(file) => serve_stream_response(file, content_type),
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

fn header_str(
    req: &axum::extract::Request,
    name: header::HeaderName,
) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

async fn serve_python_builds_file(
    State(state): State<AppState>,
    axum::extract::Path(tail): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> Response {
    let path = state.repo_dir.join("python-builds").join(&tail);
    let (status, resp) = match tokio::fs::File::open(&path).await {
        Ok(file) => {
            let mime = if tail.ends_with(".json") {
                "application/json"
            } else {
                "application/octet-stream"
            };
            let resp = Response::builder()
                .status(200)
                .header("Content-Type", mime)
                .body(Body::from_stream(file_body(file)))
                .unwrap();
            (200_u16, resp)
        }
        Err(_) => {
            let resp = (StatusCode::NOT_FOUND, "Not Found").into_response();
            (404_u16, resp)
        }
    };
    state
        .access_logger
        .log(
            &crate::access_log::AccessRecord::builder()
                .timestamp(chrono::Utc::now().to_rfc3339())
                .client_ip(
                    header_str(
                        &req,
                        "X-Forwarded-For".parse().unwrap_or(header::FORWARDED),
                    )
                    .unwrap_or_else(|| "unknown".to_string()),
                )
                .method("GET")
                .path(format!("/python-builds/{}", tail))
                .status_code(status)
                .user_agent(header_str(&req, header::USER_AGENT))
                .referer(header_str(&req, header::REFERER))
                .build(),
        )
        .ok();
    resp
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
