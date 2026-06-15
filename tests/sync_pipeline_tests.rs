use std::path::PathBuf;

use pip_mirror::config::{Config, PackageSpec, PackageUrlSpec, UvEmbedConfig};
use pip_mirror::http::HttpClient;
use pip_mirror::sync::SyncPipeline;

fn test_config(repo: PathBuf) -> Config {
    Config {
        packages: vec![],
        repository_dir: repo,
        incremental_dir: PathBuf::from("./incremental"),
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
async fn test_pipeline_dry_run_empty_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path().to_path_buf());
    let client = HttpClient::builder().build().unwrap();
    let pkgs: Vec<PackageSpec> = vec![];

    let outcome = SyncPipeline::new(&config, client, &pkgs)
        .no_deps(false)
        .dry_run(true)
        .download_python_builds(false)
        .run(None)
        .await
        .expect("dry run should succeed");

    assert!(outcome.downloaded.is_empty());
    assert!(outcome.skipped.is_empty());
    assert!(outcome.failed.is_empty());
}

#[tokio::test]
async fn test_pipeline_dry_run_url_wheel_no_deps() {
    let tmp = tempfile::tempdir().unwrap();
    let config = test_config(tmp.path().to_path_buf());
    let pkgs = vec![PackageSpec::Url(PackageUrlSpec {
        url: "https://example.com/mypkg-1.0-py3-none-any.whl".to_string(),
        sha256: None,
    })];

    let client = HttpClient::builder().build().unwrap();
    let outcome = SyncPipeline::new(&config, client, &pkgs)
        .no_deps(true)
        .dry_run(true)
        .download_python_builds(false)
        .run(None)
        .await
        .expect("dry run should succeed");

    assert!(outcome.downloaded.is_empty());
    assert!(outcome.skipped.is_empty());
    assert!(outcome.failed.is_empty());
}
