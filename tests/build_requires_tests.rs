use std::time::Duration;

use pip_mirror::resolver::build_requires::{
    download_sdist, probe_build_requires_from_version_json,
};

#[tokio::test]
async fn test_download_sdist_error_does_not_leak_credentials() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let url = "http://user:pass@127.0.0.1:1/pkg-1.0.tar.gz?token=secret";
    let err = download_sdist(&client, url).await.expect_err("should fail");
    assert!(
        !err.contains("user:pass"),
        "error leaked credentials: {err}"
    );
    assert!(!err.contains("token=secret"), "error leaked token: {err}");
}

#[tokio::test]
async fn test_probe_build_requires_sdist_url_error_does_not_leak_credentials() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let version_json = serde_json::json!({
        "urls": [{
            "packagetype": "sdist",
            "filename": "pkg-1.0.tar.gz",
            "url": "http://user:pass@127.0.0.1:1/pkg-1.0.tar.gz?token=secret",
        }]
    });
    let err = probe_build_requires_from_version_json(&client, &version_json)
        .await
        .expect_err("should fail");
    assert!(
        !err.contains("user:pass"),
        "error leaked credentials: {err}"
    );
    assert!(!err.contains("token=secret"), "error leaked token: {err}");
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
async fn test_download_sdist_bytes_error_does_not_leak_credentials() {
    let response = b"HTTP/1.1 200 OK\r\n\
        Content-Type: application/gzip\r\n\
        Content-Length: 100000\r\n\r\n\
        partial"
        .to_vec();
    let port = start_partial_http_server(response).await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let url = format!(
        "http://user:pass@127.0.0.1:{}/pkg-1.0.tar.gz?token=secret",
        port
    );
    let err = download_sdist(&client, &url)
        .await
        .expect_err("should fail");
    assert!(
        !err.contains("user:pass"),
        "error leaked credentials: {err}"
    );
    assert!(!err.contains("token=secret"), "error leaked token: {err}");
}
