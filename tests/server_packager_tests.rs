use std::path::Path;
use std::sync::Mutex;

use pip_mirror::logging;
use pip_mirror::packager::IncrementalPackage;
use pip_mirror::server;

// 避免并行测试同时修改 PIP_MIRROR_TAR_COMPRESSION 环境变量
static TAR_COMPRESSION_LOCK: Mutex<()> = Mutex::new(());

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
        explicit_url: false,
        filename: "pkg-1.0.whl".to_string(),
        url: "https://x.com/pkg.whl".to_string(),
        sha256: Some("a".repeat(64)),
        size: Some(100),
        package_name: "pkg".to_string(),
        version: "1.0".to_string(),
        yanked: None,
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

fn make_test_file_info() -> pip_mirror::downloader::FileInfo {
    pip_mirror::downloader::FileInfo {
        explicit_url: false,
        filename: "pkg-1.0.whl".to_string(),
        url: "https://x.com/pkg.whl".to_string(),
        sha256: Some("a".repeat(64)),
        size: Some(100),
        package_name: "pkg".to_string(),
        version: "1.0".to_string(),
        yanked: None,
    }
}

#[test]
fn test_create_incremental_package_default_compression() {
    let _guard = TAR_COMPRESSION_LOCK.lock().unwrap();
    // SAFETY: serialized by TAR_COMPRESSION_LOCK and only used in this test binary
    unsafe { std::env::remove_var("PIP_MIRROR_TAR_COMPRESSION") };

    let fi = make_test_file_info();
    let tmp = tempfile::tempdir().unwrap().path().to_path_buf();
    let spec = IncrementalPackage {
        simple_files: &[fi],
        python_builds_files: &[],
        python_builds_index: None,
        repository_dir: &tmp,
        output_dir: &tmp.join("out"),
    };
    std::fs::create_dir_all(&tmp).unwrap();
    // 正常路径下应返回 Ok(Some(_))
    let result = pip_mirror::packager::create_incremental_package(&spec);
    assert!(result.is_ok(), "create_incremental_package 应返回 Ok");
    let path = result.unwrap().expect("应产生增量包");
    assert!(
        path.to_string_lossy().ends_with(".tar.zst"),
        "增量包后缀应为 .tar.zst: {path:?}"
    );
}

#[test]
fn test_create_incremental_package_none_compression() {
    let _guard = TAR_COMPRESSION_LOCK.lock().unwrap();
    // SAFETY: serialized by TAR_COMPRESSION_LOCK and only used in this test binary
    unsafe { std::env::set_var("PIP_MIRROR_TAR_COMPRESSION", "none") };

    let fi = make_test_file_info();
    let tmp = tempfile::tempdir().unwrap().path().to_path_buf();
    let spec = IncrementalPackage {
        simple_files: &[fi],
        python_builds_files: &[],
        python_builds_index: None,
        repository_dir: &tmp,
        output_dir: &tmp.join("out"),
    };
    std::fs::create_dir_all(&tmp).unwrap();
    let result = pip_mirror::packager::create_incremental_package(&spec);
    // SAFETY: serialized by TAR_COMPRESSION_LOCK and only used in this test binary
    unsafe { std::env::remove_var("PIP_MIRROR_TAR_COMPRESSION") };
    assert!(result.is_ok(), "create_incremental_package 应返回 Ok");
    let path = result.unwrap().expect("应产生增量包");
    assert!(
        path.to_string_lossy().ends_with(".tar"),
        "无压缩时增量包后缀应为 .tar: {path:?}"
    );
}
