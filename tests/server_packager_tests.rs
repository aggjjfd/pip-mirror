use std::path::Path;

use pip_mirror::logging;
use pip_mirror::packager::IncrementalPackage;
use pip_mirror::server;

#[test]
fn test_logging_init_no_panic() {
    logging::init(false);
}

#[test]
fn test_resolve_serve_path_file() {
    let base = Path::new("/repo/simple");
    let (json_path, serve_path) =
        server::resolve_serve_path(base, "some-pkg/index.html");
    assert_eq!(serve_path, base.join("some-pkg/index.html"));
    assert_eq!(
        json_path,
        base.join("some-pkg/index.html.json")
            .with_file_name("index.html/index.json")
    );
}

#[test]
fn test_resolve_serve_path_directory() {
    let tmp = std::env::temp_dir().join("pip-mirror-test-dir");
    std::fs::create_dir_all(&tmp).unwrap();
    let (json_path, serve_path) = server::resolve_serve_path(&tmp, "");
    // when path is dir: serve_path = path/index.html
    assert_eq!(serve_path, tmp.join("index.html"));
    assert_eq!(json_path, tmp.join("index.json"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn test_content_type_json() {
    let p = Path::new("foo/index.json");
    assert_eq!(
        server::content_type_for(p),
        "application/vnd.pypi.simple.v1+json"
    );
}

#[test]
fn test_content_type_html_default() {
    let p = Path::new("foo/index.html");
    assert_eq!(
        server::content_type_for(p),
        "application/vnd.pypi.simple.v1+html"
    );
}

#[test]
fn test_content_type_wheel_returns_octet_stream() {
    let p = Path::new("foo/pkg-1.0-py3-none-any.whl");
    assert_eq!(server::content_type_for(p), "application/octet-stream");
}

#[test]
fn test_rewrite_relative_urls() {
    let mut data = serde_json::json!({
        "cp312": {"url": "/python-builds/cp312.tar.gz"},
        "cp311": {"url": "/python-builds/cp311.tar.gz"},
    });
    server::rewrite_relative_urls(&mut data, "http://localhost:8080");
    assert_eq!(
        data["cp312"]["url"],
        "http://localhost:8080/python-builds/cp312.tar.gz"
    );
    assert_eq!(
        data["cp311"]["url"],
        "http://localhost:8080/python-builds/cp311.tar.gz"
    );
}

#[test]
fn test_rewrite_relative_urls_skips_absolute() {
    let mut data = serde_json::json!({
        "ext": {"url": "https://ext.example.com/file.tar.gz"},
    });
    server::rewrite_relative_urls(&mut data, "http://localhost:8080");
    assert_eq!(data["ext"]["url"], "https://ext.example.com/file.tar.gz");
}

#[test]
fn test_no_changes_empty() {
    let spec = IncrementalPackage {
        simple_files: &[],
        python_builds_files: &[],
        python_builds_index: None,
        repository_dir: Path::new("/repo"),
        output_dir: Path::new("/out"),
    };
    assert!(pip_mirror::packager::no_changes(&spec));
}

#[test]
fn test_no_changes_has_simple_files() {
    let fi = pip_mirror::downloader::FileInfo {
        filename: "pkg-1.0.whl".to_string(),
        url: "https://x.com/pkg.whl".to_string(),
        sha256: Some("a".repeat(64)),
        size: Some(100),
        package_name: "pkg".to_string(),
        version: "1.0".to_string(),
    };
    let spec = IncrementalPackage {
        simple_files: &[fi],
        python_builds_files: &[],
        python_builds_index: None,
        repository_dir: Path::new("/repo"),
        output_dir: Path::new("/out"),
    };
    assert!(!pip_mirror::packager::no_changes(&spec));
}
