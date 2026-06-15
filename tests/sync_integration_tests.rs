use std::io::Write;

use pip_mirror::config::{Config, PackageSpec, PackageUrlSpec, UvEmbedConfig};
use pip_mirror::downloader::{
    Downloadable, DownloadableItem, ExplicitWheel, FileInfo,
};
use pip_mirror::http::HttpClient;
use pip_mirror::redact::redact_url_for_display;
use pip_mirror::resolver::resolve::ResolveError;
use pip_mirror::sync::SyncError;
use pip_mirror::sync::phases::PlanPhase;
use pip_mirror::sync::url_wheel::*;

fn test_client() -> HttpClient {
    HttpClient::builder().build().unwrap()
}

fn raw_test_client() -> HttpClient {
    HttpClient::builder().build().unwrap()
}

fn build_test_wheel(metadata: &str, filename_hint: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let dist_info = format!("{filename_hint}.dist-info/METADATA");
        zip.start_file_from_path(&dist_info, options).unwrap();
        zip.write_all(metadata.as_bytes()).unwrap();
        zip.start_file_from_path("testpkg/__init__.py", options)
            .unwrap();
        zip.write_all(b"# empty").unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn minimal_config() -> Config {
    Config {
        packages: vec![],
        repository_dir: std::path::PathBuf::from("./packages"),
        incremental_dir: std::path::PathBuf::from("./incremental"),
        pypi_url: "https://pypi.org".to_string(),
        pypi_urls: vec![],
        index_url: "https://mirrors.ustc.edu.cn/pypi/simple".to_string(),
        include_source: false,
        resolve_workers: 1,
        metadata_workers: 1,
        download_workers: 1,
        top_versions_per_package: 1,
        adjacent_versions_per_side: 0,
        allow_prerelease: false,
        linux_max_glibc: "2.39".to_string(),
        server_port: 8080,
        server_host: "127.0.0.1".to_string(),
        targets: vec![],
        uv_embed: UvEmbedConfig::default(),
    }
}

#[tokio::test]
async fn test_create_sync_plan_with_url_wheel_no_deps() {
    let config = minimal_config();
    let client = test_client();
    let pkgs = vec![PackageSpec::Url(PackageUrlSpec {
        url: "https://example.com/mypkg-1.0-py3-none-any.whl".to_string(),
        sha256: Some("abc".to_string()),
    })];

    let plan = PlanPhase::run(&config, &client, &pkgs, true, None)
        .await
        .expect("plan should succeed");

    assert_eq!(plan.planned_files.len(), 1);
    let fi = &plan.planned_files[0];
    assert_eq!(fi.package_name(), "mypkg");
    assert_eq!(fi.version(), "1.0");
    assert_eq!(fi.filename(), "mypkg-1.0-py3-none-any.whl");
    assert_eq!(
        fi.source_url(),
        "https://example.com/mypkg-1.0-py3-none-any.whl"
    );
    assert_eq!(fi.sha256(), Some("abc"));
    assert!(fi.is_explicit_url());
}

#[tokio::test]
async fn test_create_sync_plan_url_wheel_invalid_extension() {
    let config = minimal_config();
    let client = test_client();
    let pkgs = vec![PackageSpec::Url(PackageUrlSpec {
        url: "https://example.com/mypkg.tar.gz".to_string(),
        sha256: None,
    })];

    let err = PlanPhase::run(&config, &client, &pkgs, true, None)
        .await
        .expect_err("should fail");
    assert!(
        matches!(err, SyncError::Resolve(ResolveError::Config(_))),
        "expected Config error, got {err}"
    );
}

#[tokio::test]
async fn test_create_sync_plan_dedupes_duplicate_url_wheels() {
    let config = minimal_config();
    let client = test_client();
    let url = "https://example.com/mypkg-1.0-py3-none-any.whl";
    let pkgs = vec![
        PackageSpec::Url(PackageUrlSpec {
            url: url.to_string(),
            sha256: None,
        }),
        PackageSpec::Url(PackageUrlSpec {
            url: url.to_string(),
            sha256: None,
        }),
    ];

    let plan = PlanPhase::run(&config, &client, &pkgs, true, None)
        .await
        .expect("plan should succeed");

    assert_eq!(plan.planned_files.len(), 1);
}

#[tokio::test]
async fn test_create_sync_plan_url_wheel_no_deps_false() {
    let metadata = r#"Metadata-Version: 2.1
Name: mypkg
Version: 1.0
"#;
    let wheel = build_test_wheel(metadata, "mypkg-1.0");
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("mypkg-1.0-py3-none-any.whl");
    std::fs::write(&path, &wheel).unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let config = minimal_config();
    let client = test_client();
    let pkgs = vec![PackageSpec::Url(PackageUrlSpec { url, sha256: None })];

    let plan = PlanPhase::run(&config, &client, &pkgs, false, None)
        .await
        .expect("plan should succeed");

    assert_eq!(plan.planned_files.len(), 1);
    assert!(plan.planned_files[0].is_explicit_url());
}

#[test]
fn test_split_package_specs_separates_names_and_urls() {
    let pkgs = vec![
        PackageSpec::Name("requests".to_string()),
        PackageSpec::Url(PackageUrlSpec {
            url: "https://example.com/foo-1.0-py3-none-any.whl".to_string(),
            sha256: None,
        }),
    ];
    let (names, urls) = split_package_specs(&pkgs);
    assert_eq!(names, vec!["requests".to_string()]);
    assert_eq!(urls.len(), 1);
    assert_eq!(urls[0].url, "https://example.com/foo-1.0-py3-none-any.whl");
}

#[tokio::test]
async fn test_collect_url_wheel_deps_file_url_extracts_names() {
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
Requires-Dist: requests >=2.0
Requires-Dist: click >=7.0
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0");
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("testpkg-1.0-py3-none-any.whl");
    std::fs::write(&path, &wheel).unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let (names, prefetched) =
        collect_url_wheel_deps(&client, &specs).await.unwrap();

    assert!(names.contains(&"requests".to_string()));
    assert!(names.contains(&"click".to_string()));
    // file:// wheels should not be prefetched into memory.
    assert!(prefetched.is_empty());
}

#[tokio::test]
async fn test_create_sync_plan_resolves_url_wheel_dependencies() {
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
Requires-Dist: typing-extensions
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0");
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("testpkg-1.0-py3-none-any.whl");
    std::fs::write(&path, &wheel).unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let pkgs = vec![PackageSpec::Url(PackageUrlSpec { url, sha256: None })];

    let config = minimal_config();
    let client = test_client();
    let plan = PlanPhase::run(&config, &client, &pkgs, false, None)
        .await
        .expect("plan should succeed");

    // The URL wheel itself must be present.
    let url_file = plan
        .planned_files
        .iter()
        .find(|f| f.package_name() == "testpkg");
    assert!(url_file.is_some(), "URL wheel should be in planned files");
    assert!(url_file.unwrap().is_explicit_url());

    // Its dependency must have been resolved via PyPI.
    assert!(
        plan.solved_versions.contains_key("typing-extensions"),
        "typing-extensions should be resolved as a URL wheel dependency"
    );
}

fn wheel_serve_app(bytes: std::sync::Arc<Vec<u8>>) -> axum::Router {
    axum::Router::new().route(
        "/remotepkg-2.0-py3-none-any.whl",
        axum::routing::get(move || async move {
            axum::response::Response::builder()
                .header("Content-Type", "application/zip")
                .body(axum::body::Body::from(bytes.to_vec()))
                .unwrap()
        }),
    )
}

fn wheel_serve_app_with_status(status: reqwest::StatusCode) -> axum::Router {
    axum::Router::new().route(
        "/remotepkg-2.0-py3-none-any.whl",
        axum::routing::get(move || async move { (status, "not found") }),
    )
}

fn build_byte_chunks(
    bytes: &std::sync::Arc<Vec<u8>>,
    chunk_size: usize,
) -> Vec<Result<axum::body::Bytes, std::convert::Infallible>> {
    let total = bytes.len();
    let mut chunks = Vec::new();
    for start in (0..total).step_by(chunk_size) {
        let end = (start + chunk_size).min(total);
        chunks.push(Ok(axum::body::Bytes::copy_from_slice(&bytes[start..end])));
    }
    chunks
}

fn wheel_serve_app_chunked(bytes: std::sync::Arc<Vec<u8>>) -> axum::Router {
    use axum::body::Body;
    use axum::response::Response;
    use futures::stream;

    let chunks = build_byte_chunks(&bytes, 1024);
    let stream = stream::iter(chunks);

    axum::Router::new().route(
        "/remotepkg-2.0-py3-none-any.whl",
        axum::routing::get(|| async move {
            Response::builder()
                .header("Content-Type", "application/zip")
                .header("Transfer-Encoding", "chunked")
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    )
}

async fn start_chunked_wheel_server(bytes: Vec<u8>) -> u16 {
    let wheel_bytes = std::sync::Arc::new(bytes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let _ = tx.send(());
        axum::serve(listener, wheel_serve_app_chunked(wheel_bytes))
            .await
            .unwrap();
    });

    let _ = rx.await;
    port
}

async fn start_wheel_server(bytes: Vec<u8>) -> u16 {
    let wheel_bytes = std::sync::Arc::new(bytes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let _ = tx.send(());
        axum::serve(listener, wheel_serve_app(wheel_bytes))
            .await
            .unwrap();
    });

    // Wait for the server to signal readiness.
    let _ = rx.await;
    port
}

async fn start_failing_wheel_server(status: reqwest::StatusCode) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let _ = tx.send(());
        axum::serve(listener, wheel_serve_app_with_status(status))
            .await
            .unwrap();
    });

    let _ = rx.await;
    port
}

#[tokio::test]
async fn test_collect_url_wheel_deps_prefetches_remote_wheels() {
    let metadata = r#"Metadata-Version: 2.1
Name: remotepkg
Version: 2.0
Requires-Dist: urllib3
"#;
    let wheel = build_test_wheel(metadata, "remotepkg-2.0");
    let port = start_wheel_server(wheel.clone()).await;

    let url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let (names, prefetched) =
        collect_url_wheel_deps(&client, &specs).await.unwrap();

    assert_eq!(names, vec!["urllib3"]);
    let key = (
        "remotepkg".to_string(),
        "remotepkg-2.0-py3-none-any.whl".to_string(),
    );
    assert!(prefetched.contains_key(&key));
    assert_eq!(prefetched[&key], wheel);
}

#[tokio::test]
async fn test_collect_url_wheel_deps_http_404_fails() {
    let port = start_failing_wheel_server(reqwest::StatusCode::NOT_FOUND).await;
    let url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(msg.contains("404"), "error should mention 404: {msg}");
}

#[tokio::test]
async fn test_collect_url_wheel_deps_bad_zip_fails() {
    let bad_bytes = b"not a zip file".to_vec();
    let port = start_wheel_server(bad_bytes).await;
    let url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.to_ascii_lowercase().contains("zip"),
        "error should mention zip: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_hash_mismatch_fails() {
    let metadata = r#"Metadata-Version: 2.1
Name: remotepkg
Version: 2.0
"#;
    let wheel = build_test_wheel(metadata, "remotepkg-2.0");
    let port = start_wheel_server(wheel).await;

    let url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    let specs = vec![PackageUrlSpec {
        url,
        sha256: Some(
            "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        ),
    }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("sha256") || msg.contains("hash"),
        "error should mention hash: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_dedupes_duplicate_specs() {
    // If two URL specs share the same package/filename, only the first
    // should be fetched; the second must be skipped so that the prefetched
    // bytes match the FileInfo kept by dedupe_planned_files.
    let metadata = r#"Metadata-Version: 2.1
Name: remotepkg
Version: 2.0
"#;
    let wheel = build_test_wheel(metadata, "remotepkg-2.0");
    let port = start_wheel_server(wheel.clone()).await;

    let first_url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    // The second URL points to an unreachable port; if it were processed,
    // collect_url_wheel_deps would fail.
    let second_url =
        "http://127.0.0.1:1/remotepkg-2.0-py3-none-any.whl".to_string();
    let specs = vec![
        PackageUrlSpec {
            url: first_url,
            sha256: None,
        },
        PackageUrlSpec {
            url: second_url,
            sha256: None,
        },
    ];

    let client = raw_test_client();
    let (names, prefetched) =
        collect_url_wheel_deps(&client, &specs).await.unwrap();

    assert!(names.is_empty());
    let key = (
        "remotepkg".to_string(),
        "remotepkg-2.0-py3-none-any.whl".to_string(),
    );
    assert_eq!(prefetched.get(&key), Some(&wheel));
}

async fn accept_and_respond(
    listener: tokio::net::TcpListener,
    response: Vec<u8>,
) {
    use tokio::io::AsyncWriteExt;

    let Ok((mut socket, _)) = listener.accept().await else {
        return;
    };
    let _ = socket.write_all(&response).await;
}

async fn start_raw_http_server(response: Vec<u8>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let _ = tx.send(());
        accept_and_respond(listener, response).await;
    });

    let _ = rx.await;
    port
}

#[tokio::test]
async fn test_collect_url_wheel_deps_content_length_too_large_fails() {
    let oversized_length = MAX_REMOTE_WHEEL_BYTES + 1;
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/zip\r\n\
         Content-Length: {oversized_length}\r\n\
         \r\n"
    )
    .into_bytes();
    let port = start_raw_http_server(response).await;

    let url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("过大") || msg.contains("超过"),
        "error should indicate size limit: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_chunked_body_too_large_fails() {
    // A chunked response with no Content-Length and a body that exceeds
    // the limit must still be rejected.
    let oversized = vec![b'x'; (MAX_REMOTE_WHEEL_BYTES + 1024) as usize];
    let port = start_chunked_wheel_server(oversized).await;

    let url =
        format!("http://127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl", port);
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("过大") || msg.contains("超过"),
        "error should indicate size limit: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_missing_local_file_fails() {
    let url = "file:///tmp/nonexistent_pkg-1.0-py3-none-any.whl".to_string();
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("读取本地文件失败") || msg.contains("METADATA"),
        "error should indicate file/read failure: {msg}"
    );
}

fn assert_no_credentials(msg: &str) {
    assert!(!msg.contains("user:pass"), "leaked credentials: {msg}");
    assert!(!msg.contains("token=secret"), "leaked token: {msg}");
}

#[tokio::test]
async fn test_resolve_file_url_parse_error_does_not_leak_credentials() {
    let url =
        "file://user:pass@example.com:badport/foo.whl?token=secret".to_string();
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    assert_no_credentials(&format!("{err}"));
}

#[tokio::test]
async fn test_resolve_file_url_to_file_path_error_does_not_leak_credentials() {
    // file:// with a non-empty host fails to_file_path and must redact.
    let url =
        "file://user:pass@remotehost/tmp/foo.whl?token=secret".to_string();
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    assert_no_credentials(&format!("{err}"));
}

#[tokio::test]
async fn test_read_requires_dist_from_file_url_error_does_not_leak_credentials()
{
    let url =
        "file:///tmp/nonexistent_cred_test-1.0-py3-none-any.whl?token=secret"
            .to_string();
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    assert_no_credentials(&format!("{err}"));
}

#[tokio::test]
async fn test_parse_wheel_url_failure_does_not_leak_credentials() {
    let url =
        "https://user:pass@example.com/mypkg.tar.gz?token=secret".to_string();
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    assert_no_credentials(&format!("{err}"));
}

#[tokio::test]
async fn test_collect_url_wheel_deps_http_404_does_not_leak_credentials() {
    let port = start_failing_wheel_server(reqwest::StatusCode::NOT_FOUND).await;
    let url = format!(
        "http://user:pass@127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl?token=secret",
        port
    );
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(msg.contains("404"), "error should mention 404: {msg}");
    assert_no_credentials(&msg);
}

#[tokio::test]
async fn test_collect_url_wheel_deps_bad_zip_does_not_leak_credentials() {
    let bad_bytes = b"not a zip file".to_vec();
    let port = start_wheel_server(bad_bytes).await;
    let url = format!(
        "http://user:pass@127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl?token=secret",
        port
    );
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.to_ascii_lowercase().contains("zip"),
        "error should mention zip: {msg}"
    );
    assert_no_credentials(&msg);
}

#[tokio::test]
async fn test_collect_url_wheel_deps_hash_mismatch_does_not_leak_credentials() {
    let metadata = r#"Metadata-Version: 2.1
Name: remotepkg
Version: 2.0
"#;
    let wheel = build_test_wheel(metadata, "remotepkg-2.0");
    let port = start_wheel_server(wheel).await;

    let url = format!(
        "http://user:pass@127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl?token=secret",
        port
    );
    let specs = vec![PackageUrlSpec {
        url,
        sha256: Some(
            "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        ),
    }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("sha256") || msg.contains("hash"),
        "error should mention hash: {msg}"
    );
    assert_no_credentials(&msg);
}

#[tokio::test]
async fn test_collect_url_wheel_deps_content_length_does_not_leak_credentials()
{
    let oversized_length = MAX_REMOTE_WHEEL_BYTES + 1;
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/zip\r\n\
         Content-Length: {oversized_length}\r\n\
         \r\n"
    )
    .into_bytes();
    let port = start_raw_http_server(response).await;

    let url = format!(
        "http://user:pass@127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl?token=secret",
        port
    );
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("过大") || msg.contains("超过"),
        "error should indicate size limit: {msg}"
    );
    assert_no_credentials(&msg);
}

#[tokio::test]
async fn test_collect_url_wheel_deps_chunked_too_large_does_not_leak_credentials()
 {
    let oversized = vec![b'x'; (MAX_REMOTE_WHEEL_BYTES + 1024) as usize];
    let port = start_chunked_wheel_server(oversized).await;

    let url = format!(
        "http://user:pass@127.0.0.1:{}/remotepkg-2.0-py3-none-any.whl?token=secret",
        port
    );
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("过大") || msg.contains("超过"),
        "error should indicate size limit: {msg}"
    );
    assert_no_credentials(&msg);
}

#[test]
fn test_redact_url_for_display_strips_secrets() {
    let url = "https://user:pass@example.com/foo.whl?token=secret#frag";
    let safe = redact_url_for_display(url);
    assert!(!safe.contains("secret"));
    assert!(!safe.contains("token"));
    assert!(!safe.contains("user:pass"));
    assert!(!safe.contains("#frag"));
    assert!(safe.contains("example.com/foo.whl"));
}

#[tokio::test]
async fn test_download_wheel_bytes_reqwest_error_does_not_leak_credentials() {
    let client = HttpClient::builder().with_timeout(5).build().unwrap();
    let url =
        "http://user:pass@127.0.0.1:1/secret-1.0-py3-none-any.whl?token=abc";

    let err = download_wheel_bytes(&client, url, None)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(!msg.contains("token=abc"), "error leaked token: {msg}");
}

async fn write_partial_response(
    listener: tokio::net::TcpListener,
    response_prefix: Vec<u8>,
    ready: tokio::sync::oneshot::Sender<()>,
) {
    use tokio::io::AsyncWriteExt;
    let _ = ready.send(());
    if let Ok((mut socket, _)) = listener.accept().await {
        let _ = socket.write_all(&response_prefix).await;
    }
    // Intentionally drop the socket mid-body to force a stream error.
}

async fn start_partial_http_server(response_prefix: Vec<u8>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(write_partial_response(listener, response_prefix, tx));

    let _ = rx.await;
    port
}

#[tokio::test]
async fn test_download_wheel_bytes_stream_error_does_not_leak_credentials() {
    let response = b"HTTP/1.1 200 OK\r\n\
        Content-Type: application/zip\r\n\
        Content-Length: 100000\r\n\r\n\
        partial"
        .to_vec();
    let port = start_partial_http_server(response).await;

    let client = HttpClient::builder().with_timeout(5).build().unwrap();
    let url = format!(
        "http://user:pass@127.0.0.1:{}/secret-1.0-py3-none-any.whl?token=abc",
        port
    );

    let err = download_wheel_bytes(&client, &url, None)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(!msg.contains("token=abc"), "error leaked token: {msg}");
}

fn remote_file(url: &str) -> DownloadableItem {
    DownloadableItem::Remote(FileInfo {
        filename: "pkg-1.0-py3-none-any.whl".to_string(),
        url: url.to_string(),
        sha256: None,
        size: None,
        package_name: "pkg".to_string(),
        version: "1.0".to_string(),
        yanked: None,
    })
}

fn explicit_wheel(url: &str) -> DownloadableItem {
    DownloadableItem::Explicit(ExplicitWheel {
        filename: "pkg-1.0-py3-none-any.whl".to_string(),
        url: url.to_string(),
        sha256: None,
        package_name: "pkg".to_string(),
        version: "1.0".to_string(),
    })
}

#[test]
fn test_dedupe_planned_files_keeps_first_non_explicit() {
    let mut files = vec![
        remote_file("https://pypi.example.com/pkg-1.0-py3-none-any.whl"),
        remote_file("https://mirror.example.com/pkg-1.0-py3-none-any.whl"),
    ];
    dedupe_planned_files(&mut files);
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].source_url(),
        "https://pypi.example.com/pkg-1.0-py3-none-any.whl"
    );
}

#[test]
fn test_dedupe_planned_files_explicit_url_overrides_non_explicit() {
    let mut files = vec![
        remote_file("https://pypi.example.com/pkg-1.0-py3-none-any.whl"),
        explicit_wheel("https://explicit.example.com/pkg-1.0-py3-none-any.whl"),
    ];
    dedupe_planned_files(&mut files);
    assert_eq!(files.len(), 1);
    assert!(files[0].is_explicit_url());
    assert_eq!(
        files[0].source_url(),
        "https://explicit.example.com/pkg-1.0-py3-none-any.whl"
    );
}

#[test]
fn test_dedupe_planned_files_keeps_existing_explicit_url() {
    let mut files = vec![
        explicit_wheel("https://first.example.com/pkg-1.0-py3-none-any.whl"),
        remote_file("https://pypi.example.com/pkg-1.0-py3-none-any.whl"),
    ];
    dedupe_planned_files(&mut files);
    assert_eq!(files.len(), 1);
    assert!(files[0].is_explicit_url());
    assert_eq!(
        files[0].source_url(),
        "https://first.example.com/pkg-1.0-py3-none-any.whl"
    );
}

#[test]
fn test_dedupe_planned_files_keeps_first_explicit_url_when_both_explicit() {
    let mut files = vec![
        explicit_wheel("https://first.example.com/pkg-1.0-py3-none-any.whl"),
        explicit_wheel("https://second.example.com/pkg-1.0-py3-none-any.whl"),
    ];
    dedupe_planned_files(&mut files);
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].source_url(),
        "https://first.example.com/pkg-1.0-py3-none-any.whl"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_file_url_hash_mismatch_fails() {
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0");
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("testpkg-1.0-py3-none-any.whl");
    std::fs::write(&path, &wheel).unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let specs = vec![PackageUrlSpec {
        url,
        sha256: Some(
            "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        ),
    }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("sha256") || msg.contains("hash"),
        "error should mention hash: {msg}"
    );
    assert!(
        !msg.contains("00000000"),
        "error should not leak hash value: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_empty_file_fails() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("empty-1.0-py3-none-any.whl");
    std::fs::write(&path, b"").unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.to_ascii_lowercase().contains("zip")
            || msg.to_ascii_lowercase().contains("metadata"),
        "error should mention zip or metadata: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_truncated_zip_fails() {
    // A valid local file header but missing central directory.
    let mut truncated = vec![
        0x50, 0x4B, 0x03, 0x04, // local file header signature
        0x0A, 0x00, // version needed
        0x00, 0x00, // general purpose bit flag
        0x00, 0x00, // compression method (stored)
        0x00, 0x00, // file last modification time
        0x00, 0x00, // file last modification date
        0x00, 0x00, 0x00, 0x00, // CRC-32
        0x05, 0x00, 0x00, 0x00, // compressed size
        0x05, 0x00, 0x00, 0x00, // uncompressed size
        0x08, 0x00, // file name length
        0x00, 0x00, // extra field length
    ];
    truncated.extend_from_slice(b"META\x00"); // 8-byte filename + 5-byte data
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("truncated-1.0-py3-none-any.whl");
    std::fs::write(&path, &truncated).unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        msg.to_ascii_lowercase().contains("zip")
            || msg.to_ascii_lowercase().contains("metadata"),
        "error should mention zip or metadata: {msg}"
    );
}

#[tokio::test]
async fn test_collect_url_wheel_deps_file_url_oversized_fails() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let path = tmp_dir.path().join("huge-1.0-py3-none-any.whl");
    // Write a file slightly larger than the limit.
    let huge = vec![b'x'; (MAX_REMOTE_WHEEL_BYTES + 1) as usize];
    std::fs::write(&path, &huge).unwrap();

    let url = format!("file://{}", path.canonicalize().unwrap().display());
    let specs = vec![PackageUrlSpec { url, sha256: None }];

    let client = raw_test_client();
    let err = collect_url_wheel_deps(&client, &specs)
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(msg.contains("过大"), "error should mention size: {msg}");
}
