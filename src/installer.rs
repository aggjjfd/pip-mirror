use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
};

const UV_INSTALLER_SH: &str =
    include_str!("../assets/installers/uv-installer.sh");
const UV_INSTALLER_PS1: &str =
    include_str!("../assets/installers/uv-installer.ps1");

const UV_LINUX: &[u8] = include_bytes!(concat!(
    "../assets/uv-releases/",
    env!("UV_EMBED_VERSION"),
    "/uv-x86_64-unknown-linux-gnu.tar.gz"
));
const UV_LINUX_SHA256: &[u8] = include_bytes!(concat!(
    "../assets/uv-releases/",
    env!("UV_EMBED_VERSION"),
    "/uv-x86_64-unknown-linux-gnu.tar.gz.sha256"
));
const UV_WINDOWS: &[u8] = include_bytes!(concat!(
    "../assets/uv-releases/",
    env!("UV_EMBED_VERSION"),
    "/uv-x86_64-pc-windows-msvc.zip"
));
const UV_WINDOWS_SHA256: &[u8] = include_bytes!(concat!(
    "../assets/uv-releases/",
    env!("UV_EMBED_VERSION"),
    "/uv-x86_64-pc-windows-msvc.zip.sha256"
));

fn serve_range(
    body: &[u8],
    ct: &'static str,
    range: &str,
    total: u64,
    cors: bool,
) -> Response {
    let Some((s, e)) = crate::server::parse_range(range, total) else {
        return Response::builder()
            .status(416)
            .header("Content-Range", format!("bytes */{}", total))
            .body(Body::empty())
            .unwrap();
    };
    let sliced = body[s as usize..=e as usize].to_vec();
    let mut b = Response::builder()
        .status(206)
        .header("Content-Type", ct)
        .header("Content-Length", sliced.len())
        .header("Content-Range", format!("bytes {}-{}/{}", s, e, total));
    if cors {
        b = b.header("Access-Control-Allow-Origin", "*");
    }
    b.body(Body::from(sliced)).unwrap()
}

fn serve_bytes(
    body: &[u8],
    ct: &'static str,
    range: Option<&str>,
    cors: bool,
) -> Response {
    let total = body.len() as u64;
    if let Some(r) = range {
        return serve_range(body, ct, r, total, cors);
    }
    let mut b = Response::builder()
        .status(200)
        .header("Content-Type", ct)
        .header("Content-Length", total);
    if cors {
        b = b.header("Access-Control-Allow-Origin", "*");
    }
    b.body(Body::from(body.to_vec())).unwrap()
}

fn patch_installer(content: &str, url: &str, version: &str) -> String {
    let old_urls_sh = format!(
        "ARTIFACT_DOWNLOAD_URLS=\"https://releases.astral.sh/github/uv/releases/download/{v} https://github.com/astral-sh/uv/releases/download/{v}\"",
        v = version
    );
    let old_urls_ps1 = format!(
        "$ArtifactDownloadUrls = @(\"https://releases.astral.sh/github/uv/releases/download/{v}\", \"https://github.com/astral-sh/uv/releases/download/{v}\")",
        v = version
    );
    let old_url = format!(
        "https://releases.astral.sh/github/uv/releases/download/{}",
        version
    );

    content
        .replace(&old_urls_sh, &format!("ARTIFACT_DOWNLOAD_URLS=\"{}\"", url))
        .replace(
            &old_urls_ps1,
            &format!("$ArtifactDownloadUrls = @(\"{}\")", url),
        )
        .replace(&old_url, url)
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
    let version = env!("UV_EMBED_VERSION");
    let url = format!("http://{}/uv-releases/{}", host, version);
    let (patched, content_type) = match name.as_str() {
        "uv-installer.sh" => (
            patch_installer(UV_INSTALLER_SH, &url, version),
            "text/plain; charset=utf-8",
        ),
        "uv-installer.ps1" => (
            patch_installer(UV_INSTALLER_PS1, &url, version),
            "text/plain; charset=utf-8",
        ),
        _ => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    let body = patched.into_bytes();
    let range = req.headers().get("range").and_then(|v| v.to_str().ok());
    serve_bytes(&body, content_type, range, false)
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
    req: axum::extract::Request,
) -> Response {
    let data: &'static [u8] = match tail.as_str() {
        concat!(
            env!("UV_EMBED_VERSION"),
            "/uv-x86_64-unknown-linux-gnu.tar.gz"
        ) => UV_LINUX,
        concat!(
            env!("UV_EMBED_VERSION"),
            "/uv-x86_64-unknown-linux-gnu.tar.gz.sha256"
        ) => UV_LINUX_SHA256,
        concat!(env!("UV_EMBED_VERSION"), "/uv-x86_64-pc-windows-msvc.zip") => {
            UV_WINDOWS
        }
        concat!(
            env!("UV_EMBED_VERSION"),
            "/uv-x86_64-pc-windows-msvc.zip.sha256"
        ) => UV_WINDOWS_SHA256,
        _ => return (StatusCode::NOT_FOUND, "Not Found").into_response(),
    };
    let mime = mime_for_release(&tail);
    let range = req.headers().get("range").and_then(|v| v.to_str().ok());
    serve_bytes(data, mime, range, true)
}
