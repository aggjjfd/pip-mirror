#![allow(clippy::excessive_nesting)]

use std::fs;
use std::net::TcpListener;
use std::time::Duration;

use tempfile::tempdir;
use tokio::task::LocalSet;

#[tokio::test]
async fn test_serve_generates_index_without_store_db() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let pkg_dir = repo.join("simple").join("demo");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join("demo-1.0-py3-none-any.whl"), b"").unwrap();

    // 模拟内网场景：只有包文件，没有数据库
    assert!(!repo.join(".store.db").exists());

    // 找一个临时端口
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let repo_for_server = repo.clone();

    LocalSet::new()
        .run_until(async move {
            let server_handle = tokio::task::spawn_local(async move {
                pip_mirror::server::start_server(
                    "127.0.0.1",
                    port,
                    repo_for_server,
                    vec![],
                )
                .await
            });

            let client = reqwest::Client::new();
            let base = format!("http://127.0.0.1:{port}");

            // 等待服务启动
            let mut ready = false;
            for _ in 0..50 {
                if client
                    .get(format!("{base}/simple/index.json"))
                    .send()
                    .await
                    .is_ok()
                {
                    ready = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            assert!(ready, "server did not become ready");

            // 顶层 /simple/index.json 应包含 demo
            let resp = client
                .get(format!("{base}/simple/index.json"))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            let names: Vec<&str> = body["projects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["name"].as_str().unwrap())
                .collect();
            assert!(names.contains(&"demo"));

            // 包级 /simple/demo/index.json 应包含 whl 文件
            let resp = client
                .get(format!("{base}/simple/demo/index.json"))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(body["name"], "demo");
            let files: Vec<&str> = body["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["filename"].as_str().unwrap())
                .collect();
            assert!(files.contains(&"demo-1.0-py3-none-any.whl"));

            // 包级 HTML 也应生成
            let resp = client
                .get(format!("{base}/simple/demo/index.html"))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let html = resp.text().await.unwrap();
            assert!(html.contains("demo-1.0-py3-none-any.whl"));

            server_handle.abort();
            let _ = server_handle.await;
        })
        .await;
}
