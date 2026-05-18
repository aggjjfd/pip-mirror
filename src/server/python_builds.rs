use std::path::Path;
use std::time::SystemTime;

use axum::{
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::server::{
    AppState, build_access_record, build_range_response, file_body_range,
    parse_range, range_not_satisfiable,
};

pub fn json_response(body: &str) -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
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

async fn build_python_builds_body(
    state: &AppState,
    host: &str,
) -> Result<String, Response> {
    let path = state.repo_dir.join("python-builds").join("index.json");
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => {
            return Err((StatusCode::NOT_FOUND, "not found").into_response());
        }
    };
    let mut data = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) => v,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed: {e}"),
            )
                .into_response());
        }
    };
    rewrite_relative_urls(&mut data, &format!("http://{host}"));
    Ok(serde_json::to_string_pretty(&data).unwrap())
}

fn cache_still_valid(entry: &(String, SystemTime), repo_dir: &Path) -> bool {
    let path = repo_dir.join("python-builds").join("index.json");
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return true;
    };
    mtime <= entry.1
}

fn try_cached_response(
    cache: &std::collections::HashMap<String, (String, SystemTime)>,
    host: &str,
    repo_dir: &Path,
) -> Option<Response> {
    let entry = cache.get(host)?;
    if cache_still_valid(entry, repo_dir) {
        Some(json_response(&entry.0))
    } else {
        None
    }
}

pub async fn serve_python_builds_index(
    State(state): State<AppState>,
    req: axum::extract::Request,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let path = state.repo_dir.join("python-builds").join("index.json");

    {
        let cache = state.python_builds_cache.read().await;
        if let Some(resp) = try_cached_response(&cache, host, &state.repo_dir) {
            return resp;
        }
    }

    let mut cache = state.python_builds_cache.write().await;
    if let Some(resp) = try_cached_response(&cache, host, &state.repo_dir) {
        return resp;
    }

    match build_python_builds_body(&state, host).await {
        Ok(body) => {
            let mtime = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or_else(SystemTime::now);
            cache.insert(host.to_string(), (body.clone(), mtime));
            json_response(&body)
        }
        Err(resp) => resp,
    }
}

async fn file_size(file: &tokio::fs::File) -> Result<u64, Response> {
    match file.metadata().await {
        Ok(m) => Ok(m.len()),
        Err(_) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Read error")
                .into_response())
        }
    }
}

fn full_response(
    file: tokio::fs::File,
    mime: &'static str,
    size: u64,
) -> Response {
    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        .header("Content-Length", size)
        .body(Body::from_stream(file_body_range(file, size)))
        .unwrap()
}

async fn serve_opened_file(
    file: tokio::fs::File,
    mime: &'static str,
    range: Option<&str>,
) -> Response {
    let size = match file_size(&file).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match range {
        Some(r) => match parse_range(r, size) {
            Some((s, e)) => build_range_response(file, mime, s, e, size).await,
            None => range_not_satisfiable(size),
        },
        None => full_response(file, mime, size),
    }
}

pub async fn serve_python_builds_file(
    State(state): State<AppState>,
    axum::extract::Path(tail): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> Response {
    let path = state.repo_dir.join("python-builds").join(&tail);
    let mime = if tail.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    };
    let range = req.headers().get("range").and_then(|v| v.to_str().ok());
    let (status, resp) = match tokio::fs::File::open(&path).await {
        Ok(file) => {
            let resp = serve_opened_file(file, mime, range).await;
            (resp.status().as_u16(), resp)
        }
        Err(_) => {
            let resp = (StatusCode::NOT_FOUND, "Not Found").into_response();
            (404_u16, resp)
        }
    };
    let record =
        build_access_record(&req, &format!("/python-builds/{}", tail), status);
    state.access_logger.log(&record).ok();
    resp
}
