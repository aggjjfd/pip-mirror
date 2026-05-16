use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::server::AppState;

fn patch_installer(content: &str, url: &str) -> String {
    content
        .replace(
            "ARTIFACT_DOWNLOAD_URLS=\"https://releases.astral.sh/github/uv/releases/download/0.11.14 https://github.com/astral-sh/uv/releases/download/0.11.14\"",
            &format!("ARTIFACT_DOWNLOAD_URLS=\"{}\"", url),
        )
        .replace(
            "$ArtifactDownloadUrls = @(\"https://releases.astral.sh/github/uv/releases/download/0.11.14\", \"https://github.com/astral-sh/uv/releases/download/0.11.14\")",
            &format!("$ArtifactDownloadUrls = @(\"{}\")", url),
        )
        .replace(
            "https://releases.astral.sh/github/uv/releases/download/0.11.14",
            url,
        )
}

pub async fn serve_installer(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let path = state.repo_dir.join("installers").join(&name);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    let url = format!("http://{}/uv-releases/0.11.14", host);
    let patched = patch_installer(&content, &url);
    Response::builder()
        .status(200)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(Body::from(patched))
        .unwrap()
}

fn mime_for_release(tail: &str) -> &'static str {
    if tail.ends_with(".sha256") {
        "text/plain; charset=utf-8"
    } else if tail.ends_with(".tar.gz") {
        "application/gzip"
    } else if tail.ends_with(".zip") {
        "application/zip"
    } else {
        "application/octet-stream"
    }
}

pub async fn serve_uv_release(
    State(state): State<AppState>,
    axum::extract::Path(tail): axum::extract::Path<String>,
) -> Response {
    let path = state.repo_dir.join("uv-releases").join(&tail);
    if !path.exists() {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    let mime = mime_for_release(&tail);
    match tokio::fs::File::open(&path).await {
        Ok(file) => Response::builder()
            .status(200)
            .header("Content-Type", mime)
            .header("Access-Control-Allow-Origin", "*")
            .body(Body::from_stream(crate::server::file_body(file)))
            .unwrap(),
        Err(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Read error").into_response()
        }
    }
}
