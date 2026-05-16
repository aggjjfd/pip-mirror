use std::path::Path;
use std::time::SystemTime;

use axum::{
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::server::{AppState, build_access_record, file_body};

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

pub async fn serve_python_builds_file(
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
    let record =
        build_access_record(&req, &format!("/python-builds/{}", tail), status);
    state.access_logger.log(&record).ok();
    resp
}
