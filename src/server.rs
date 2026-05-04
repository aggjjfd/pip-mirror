use std::net::SocketAddr;
use std::path::PathBuf;
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
    access_logger: Arc<AccessLogger>,
}

pub async fn start_server(
    host: &str,
    port: u16,
    repository_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if !repository_dir.exists() {
        return Err(format!("仓库目录不存在: {}", repository_dir.display()).into());
    }

    let access_logger = Arc::new(
        AccessLogger::open(&repository_dir.join(".access_log.db"))
            .unwrap_or_else(|e| panic!("无法打开 access_log.db: {e}")),
    );

    let state = AppState {
        repo_dir: Arc::new(repository_dir),
        access_logger,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET])
        .allow_headers([header::CONTENT_TYPE, header::ACCEPT]);

    let app = Router::new()
        .route("/simple/{*tail}", get(serve_simple))
        .route("/python-builds/index.json", get(serve_python_builds_index))
        .layer(cors)
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("PIP 镜像服务器启动");
    tracing::info!("  地址: http://{host}:{port}");
    tracing::info!("  pip 使用: pip install --index-url http://{host}:{port}/simple <package>");
    tracing::info!("  Python 解释器: UV_PYTHON_DOWNLOADS_JSON_URL=http://{host}:{port}/python-builds/index.json");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_simple(
    State(state): State<AppState>,
    axum::extract::Path(tail): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> Response {
    let simple_base = state.repo_dir.join("simple");
    let path = if tail.is_empty() {
        simple_base.clone()
    } else {
        simple_base.join(&tail)
    };

    // Content negotiation: JSON vs HTML
    let accept = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Try index.json first for PEP 691 clients
    let json_path = if path.is_dir() {
        path.join("index.json")
    } else {
        path.with_file_name(format!(
            "{}/index.json",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    };

    if accept.contains("application/vnd.pypi.simple.v1+json") {
        if json_path.exists() {
            match tokio::fs::read(&json_path).await {
                Ok(body) => return Response::builder()
                    .status(200)
                    .header("Content-Type", "application/vnd.pypi.simple.v1+json")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
                Err(_) => {}
            }
        }
    }

    // Default: serve HTML index or file
    let serve_path = if path.is_dir() {
        path.join("index.html")
    } else {
        path
    };

    if !serve_path.exists() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    match tokio::fs::read(&serve_path).await {
        Ok(body) => {
            let content_type = if serve_path.extension().map_or(false, |e| e == "json") {
                "application/vnd.pypi.simple.v1+json"
            } else {
                "application/vnd.pypi.simple.v1+html"
            };
            Response::builder()
                .status(200)
                .header("Content-Type", content_type)
                .header("Access-Control-Allow-Origin", "*")
                .body(axum::body::Body::from(body))
                .unwrap()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Read error").into_response(),
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
        Err(_) => return (StatusCode::NOT_FOUND, "python-builds index not found").into_response(),
    };

    let base = format!("http://{host}");
    let data: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to parse index.json: {e}"),
            )
                .into_response()
        }
    };

    // Rewrite relative URLs to absolute
    let mut data = data;
    if let Some(obj) = data.as_object_mut() {
        for entry in obj.values_mut() {
            if let Some(url) = entry.get("url").and_then(|u| u.as_str()) {
                if url.starts_with('/') {
                    entry["url"] = serde_json::Value::String(format!("{base}{url}"));
                }
            }
        }
    }

    let body = serde_json::to_string_pretty(&data).unwrap();
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
}
