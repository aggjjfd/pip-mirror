use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
};

const UV_INSTALLER_SH: &str =
    include_str!("../packages/installers/uv-installer.sh");
const UV_INSTALLER_PS1: &str =
    include_str!("../packages/installers/uv-installer.ps1");

const UV_LINUX: &[u8] = include_bytes!(
    "../packages/uv-releases/0.11.14/uv-x86_64-unknown-linux-gnu.tar.gz"
);
const UV_LINUX_SHA256: &[u8] = include_bytes!(
    "../packages/uv-releases/0.11.14/uv-x86_64-unknown-linux-gnu.tar.gz.sha256"
);
const UV_WINDOWS: &[u8] = include_bytes!(
    "../packages/uv-releases/0.11.14/uv-x86_64-pc-windows-msvc.zip"
);
const UV_WINDOWS_SHA256: &[u8] = include_bytes!(
    "../packages/uv-releases/0.11.14/uv-x86_64-pc-windows-msvc.zip.sha256"
);

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
    axum::extract::Path(name): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let url = format!("http://{}/uv-releases/0.11.14", host);
    let (patched, content_type) = match name.as_str() {
        "uv-installer.sh" => (
            patch_installer(UV_INSTALLER_SH, &url),
            "text/plain; charset=utf-8",
        ),
        "uv-installer.ps1" => (
            patch_installer(UV_INSTALLER_PS1, &url),
            "text/plain; charset=utf-8",
        ),
        _ => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    Response::builder()
        .status(200)
        .header("Content-Type", content_type)
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
    axum::extract::Path(tail): axum::extract::Path<String>,
) -> Response {
    let data: &'static [u8] = match tail.as_str() {
        "0.11.14/uv-x86_64-unknown-linux-gnu.tar.gz" => UV_LINUX,
        "0.11.14/uv-x86_64-unknown-linux-gnu.tar.gz.sha256" => UV_LINUX_SHA256,
        "0.11.14/uv-x86_64-pc-windows-msvc.zip" => UV_WINDOWS,
        "0.11.14/uv-x86_64-pc-windows-msvc.zip.sha256" => UV_WINDOWS_SHA256,
        _ => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    let mime = mime_for_release(&tail);
    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from(data))
        .unwrap()
}
