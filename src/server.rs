use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

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

mod python_builds;
pub use python_builds::rewrite_relative_urls;

#[derive(Clone)]
pub struct AppState {
    pub repo_dir: Arc<PathBuf>,
    #[allow(dead_code)]
    pub access_logger: Arc<AccessLogger>,
    pub targets: Arc<Vec<TargetSpec>>,
    pub python_builds_cache:
        Arc<tokio::sync::RwLock<HashMap<String, (String, SystemTime)>>>,
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/fonts/{name}", get(serve_font))
        .route("/installers/{name}", get(crate::installer::serve_installer))
        .route(
            "/uv-releases/{*tail}",
            get(crate::installer::serve_uv_release),
        )
        .route("/simple/{*tail}", get(serve_simple))
        .route(
            "/python-builds/index.json",
            get(python_builds::serve_python_builds_index),
        )
        .route(
            "/python-builds/{*tail}",
            get(python_builds::serve_python_builds_file),
        )
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
        python_builds_cache: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
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
    crate::indexer::generate_index(&repository_dir);
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

fn parse_range_bounds(rest: &str, total: u64) -> Option<(u64, u64)> {
    let (start_str, end_str) = rest.split_once('-')?;
    let start = start_str.parse::<u64>().ok()?;
    let end = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        end_str.parse::<u64>().ok()?
    };
    if start >= total || start > end {
        return None;
    }
    Some((start, end))
}

pub(crate) fn parse_range(range: &str, total: u64) -> Option<(u64, u64)> {
    let rest = range.strip_prefix("bytes=")?;
    if rest.contains(',') {
        return None;
    }
    parse_range_bounds(rest, total)
}

pub(crate) fn file_body_range(
    file: tokio::fs::File,
    remaining: u64,
) -> impl futures::Stream<Item = Result<axum::body::Bytes, std::io::Error>> {
    futures::stream::try_unfold(
        (file, remaining),
        |(mut file, remaining)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            let chunk = 65536u64.min(remaining);
            let mut buf = vec![0u8; chunk as usize];
            let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf).await?;
            if n == 0 {
                return Ok(None);
            }
            buf.truncate(n);
            let new_remaining = remaining - n as u64;
            Ok(Some((axum::body::Bytes::from(buf), (file, new_remaining))))
        },
    )
}

pub(crate) async fn build_range_response(
    mut file: tokio::fs::File,
    content_type: &'static str,
    start: u64,
    end: u64,
    file_size: u64,
) -> Response {
    if tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(start))
        .await
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Seek error")
            .into_response();
    }
    let length = end - start + 1;
    Response::builder()
        .status(206)
        .header("Content-Type", content_type)
        .header("Access-Control-Allow-Origin", "*")
        .header("Content-Length", length)
        .header(
            "Content-Range",
            format!("bytes {}-{}/{}", start, end, file_size),
        )
        .body(Body::from_stream(file_body_range(file, length)))
        .unwrap()
}

pub(crate) fn range_not_satisfiable(file_size: u64) -> Response {
    Response::builder()
        .status(416)
        .header("Content-Range", format!("bytes */{}", file_size))
        .body(Body::empty())
        .unwrap()
}

async fn serve_file_with_range(
    file: tokio::fs::File,
    content_type: &'static str,
    range_header: Option<&str>,
) -> Response {
    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Read error")
                .into_response();
        }
    };
    let file_size = metadata.len();

    if let Some(range) = range_header {
        if let Some((start, end)) = parse_range(range, file_size) {
            return build_range_response(
                file,
                content_type,
                start,
                end,
                file_size,
            )
            .await;
        }
        return range_not_satisfiable(file_size);
    }

    Response::builder()
        .status(200)
        .header("Content-Type", content_type)
        .header("Access-Control-Allow-Origin", "*")
        .header("Content-Length", file_size)
        .body(Body::from_stream(file_body_range(file, file_size)))
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
    let range = req.headers().get("range").and_then(|v| v.to_str().ok());
    match tokio::fs::File::open(&serve_path).await {
        Ok(file) => serve_file_with_range(file, content_type, range).await,
        Err(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Read error").into_response()
        }
    }
}

pub(crate) fn header_str(
    req: &axum::extract::Request,
    name: header::HeaderName,
) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

pub(crate) fn build_access_record(
    req: &axum::extract::Request,
    path: &str,
    status: u16,
) -> crate::access_log::AccessRecord {
    crate::access_log::AccessRecord::builder()
        .timestamp(chrono::Utc::now().to_rfc3339())
        .client_ip(
            header_str(
                req,
                "X-Forwarded-For".parse().unwrap_or(header::FORWARDED),
            )
            .unwrap_or_else(|| "unknown".to_string()),
        )
        .method("GET")
        .path(path.to_string())
        .status_code(status)
        .user_agent(header_str(req, header::USER_AGENT))
        .referer(header_str(req, header::REFERER))
        .build()
}
