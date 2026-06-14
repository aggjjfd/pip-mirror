use dashmap::DashMap;
use pip_mirror::downloader;

#[test]
fn test_select_latest_versions_basic() {
    let files = vec![
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-2.0.0-py3-none-any.whl".to_string(),
            url: "https://example.com/pkg-2.0.0.whl".to_string(),
            sha256: Some("a".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "2.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0.0-py3-none-any.whl".to_string(),
            url: "https://example.com/pkg-1.0.0.whl".to_string(),
            sha256: Some("b".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-0.9.0-py3-none-any.whl".to_string(),
            url: "https://example.com/pkg-0.9.0.whl".to_string(),
            sha256: Some("c".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "0.9.0".to_string(),
            yanked: None,
        },
    ];
    let selected = downloader::select_latest_versions(&files, 2, false);
    let versions: Vec<_> =
        selected.iter().map(|f| f.version.as_str()).collect();
    assert_eq!(versions.len(), 2);
    assert!(versions.contains(&"2.0.0"));
    assert!(versions.contains(&"1.0.0"));
}

#[test]
fn test_select_latest_versions_max_zero_returns_all() {
    let files = vec![
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0.0-py3-none-any.whl".to_string(),
            url: "https://example.com/pkg-1.0.0.whl".to_string(),
            sha256: Some("a".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-0.9.0-py3-none-any.whl".to_string(),
            url: "https://example.com/pkg-0.9.0.whl".to_string(),
            sha256: Some("b".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "0.9.0".to_string(),
            yanked: None,
        },
    ];
    let selected = downloader::select_latest_versions(&files, 0, false);
    assert_eq!(selected.len(), 2);
}

#[test]
fn test_collect_version_files_filters_platform() {
    let files = vec![
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0-py3-none-manylinux1_x86_64.whl".to_string(),
            url: "https://example.com/linux.whl".to_string(),
            sha256: Some("a".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0-py3-none-macosx_10_9_x86_64.whl".to_string(),
            url: "https://example.com/macos.whl".to_string(),
            sha256: Some("b".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0.tar.gz".to_string(),
            url: "https://example.com/sdist.tar.gz".to_string(),
            sha256: Some("c".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
    ];
    let result = downloader::collect_version_files(&files);
    // linux wheel accepted, macos rejected, sdist skipped (same version has whl)
    assert_eq!(result.len(), 1);
    assert!(result.iter().any(|f| f.filename.contains("manylinux")));
}

#[test]
fn test_collect_version_files_skips_sdist_when_same_version_has_wheel() {
    let files = vec![
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0-cp312-cp312-win_amd64.whl".to_string(),
            url: "https://example.com/win.whl".to_string(),
            sha256: Some("a".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0.tar.gz".to_string(),
            url: "https://example.com/sdist.tar.gz".to_string(),
            sha256: Some("b".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
    ];
    let result = downloader::collect_version_files(&files);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].filename, "pkg-1.0-cp312-cp312-win_amd64.whl");
}

#[test]
fn test_version_has_target() {
    let files = vec![
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0-py3-none-manylinux1_x86_64.whl".to_string(),
            url: "https://example.com/linux.whl".to_string(),
            sha256: Some("a".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
        downloader::FileInfo {
            explicit_url: false,
            filename: "pkg-1.0-py3-none-win_amd64.whl".to_string(),
            url: "https://example.com/win.whl".to_string(),
            sha256: Some("b".repeat(64)),
            size: Some(100),
            package_name: "pkg".to_string(),
            version: "1.0.0".to_string(),
            yanked: None,
        },
    ];
    assert!(downloader::version_has_target(&files, "linux_x86_64"));
    assert!(downloader::version_has_target(&files, "win_amd64"));
    assert!(!downloader::version_has_target(&files, "win32"));
}

#[test]
fn test_backfill_one_target_finds_old_target() {
    let older_versions = vec!["0.9.0".to_string(), "0.8.0".to_string()];
    let all_grouped: DashMap<String, Vec<downloader::FileInfo>> = {
        let m = DashMap::new();
        m.insert(
            "0.9.0".to_string(),
            vec![
                downloader::FileInfo {
                    explicit_url: false,
                    filename: "p-0.9.0-cp312-cp312-linux_x86_64.whl"
                        .to_string(),
                    url: "https://example.com/linux.whl".to_string(),
                    sha256: Some("a".repeat(64)),
                    size: Some(100),
                    package_name: "p".to_string(),
                    version: "0.9.0".to_string(),
                    yanked: None,
                },
                downloader::FileInfo {
                    explicit_url: false,
                    filename: "p-0.9.0-cp312-cp312-win_amd64.whl".to_string(),
                    url: "https://example.com/win64.whl".to_string(),
                    sha256: Some("b".repeat(64)),
                    size: Some(100),
                    package_name: "p".to_string(),
                    version: "0.9.0".to_string(),
                    yanked: None,
                },
                downloader::FileInfo {
                    explicit_url: false,
                    filename: "p-0.9.0-cp312-cp312-win32.whl".to_string(),
                    url: "https://example.com/win32.whl".to_string(),
                    sha256: Some("c".repeat(64)),
                    size: Some(100),
                    package_name: "p".to_string(),
                    version: "0.9.0".to_string(),
                    yanked: None,
                },
            ],
        );
        m
    };
    let result =
        downloader::backfill_one_target("win32", &older_versions, &all_grouped);
    assert!(result.is_some());
    let (files, is_pre) = result.unwrap();
    assert!(files.iter().any(|f| f.filename.contains("win32")));
    assert!(!is_pre);
}

#[test]
fn test_backfill_one_target_no_history() {
    let older_versions = vec!["0.9.0".to_string(), "0.5.0".to_string()];
    let all_grouped: DashMap<String, Vec<downloader::FileInfo>> = {
        let m = DashMap::new();
        m.insert(
            "0.9.0".to_string(),
            vec![downloader::FileInfo {
                explicit_url: false,
                filename: "p-0.9.0-cp312-cp312-linux_x86_64.whl".to_string(),
                url: "https://example.com/linux.whl".to_string(),
                sha256: Some("a".repeat(64)),
                size: Some(100),
                package_name: "p".to_string(),
                version: "0.9.0".to_string(),
                yanked: None,
            }],
        );
        m
    };
    let result =
        downloader::backfill_one_target("win32", &older_versions, &all_grouped);
    assert!(result.is_none());
}

#[test]
fn test_backfill_one_target_respects_order() {
    let older_versions = vec!["0.9.0".to_string(), "0.8.0".to_string()];
    let all_grouped: DashMap<String, Vec<downloader::FileInfo>> = {
        let m = DashMap::new();
        m.insert(
            "0.9.0".to_string(),
            vec![downloader::FileInfo {
                explicit_url: false,
                filename: "p-0.9.0-cp312-cp312-linux_x86_64.whl".to_string(),
                url: "https://example.com/v0.9.whl".to_string(),
                sha256: Some("a".repeat(64)),
                size: Some(100),
                package_name: "p".to_string(),
                version: "0.9.0".to_string(),
                yanked: None,
            }],
        );
        m.insert(
            "0.8.0".to_string(),
            vec![downloader::FileInfo {
                explicit_url: false,
                filename: "p-0.8.0-cp312-cp312-win32.whl".to_string(),
                url: "https://example.com/v0.8.whl".to_string(),
                sha256: Some("b".repeat(64)),
                size: Some(100),
                package_name: "p".to_string(),
                version: "0.8.0".to_string(),
                yanked: None,
            }],
        );
        m
    };
    let result =
        downloader::backfill_one_target("win32", &older_versions, &all_grouped);
    // should return 0.9.0's files (first match) not 0.8.0
    let (files, _) = result.unwrap();
    assert_eq!(files[0].version, "0.8.0");
}

#[tokio::test]
async fn test_download_file_copies_local_wheel() {
    let tmp = tempfile::tempdir().unwrap();

    let source = tmp.path().join("source").join("mypkg-1.0-py3-none-any.whl");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, b"wheel bytes").unwrap();

    let dest = tmp
        .path()
        .join("dest")
        .join("simple")
        .join("mypkg")
        .join("mypkg-1.0-py3-none-any.whl");

    let fi = downloader::FileInfo {
        explicit_url: true,
        filename: "mypkg-1.0-py3-none-any.whl".to_string(),
        url: format!("file://{}", source.to_str().unwrap()),
        sha256: None,
        size: None,
        package_name: "mypkg".to_string(),
        version: "1.0".to_string(),
        yanked: None,
    };

    let client = reqwest::Client::new();
    let (ok, msg) = downloader::download_file(&client, &fi, &dest).await;
    assert!(ok, "copy failed: {msg}");
    assert!(dest.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), b"wheel bytes");
}

#[tokio::test]
async fn test_download_file_local_hash_mismatch_fails() {
    let tmp = tempfile::tempdir().unwrap();

    let source = tmp.path().join("mypkg-1.0-py3-none-any.whl");
    std::fs::write(&source, b"wheel bytes").unwrap();

    let dest = tmp.path().join("dest.whl");

    let fi = downloader::FileInfo {
        explicit_url: true,
        filename: "mypkg-1.0-py3-none-any.whl".to_string(),
        url: format!("file://{}", source.to_str().unwrap()),
        sha256: Some("0".repeat(64)),
        size: None,
        package_name: "mypkg".to_string(),
        version: "1.0".to_string(),
        yanked: None,
    };

    let client = reqwest::Client::new();
    let (ok, msg) = downloader::download_file(&client, &fi, &dest).await;
    assert!(!ok);
    assert!(msg.contains("hash") || msg.contains("校验"));
    assert!(!dest.exists());
}

#[tokio::test]
async fn test_download_file_local_missing_source_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("nonexistent-1.0-py3-none-any.whl");
    let dest = tmp.path().join("dest.whl");

    let fi = downloader::FileInfo {
        explicit_url: true,
        filename: "nonexistent-1.0-py3-none-any.whl".to_string(),
        url: format!("file://{}", source.to_str().unwrap()),
        sha256: None,
        size: None,
        package_name: "nonexistent".to_string(),
        version: "1.0".to_string(),
        yanked: None,
    };

    let client = reqwest::Client::new();
    let (ok, msg) = downloader::download_file(&client, &fi, &dest).await;
    assert!(!ok);
    assert!(msg.contains("读取") || msg.contains("失败"));
    assert!(!dest.exists());
}

#[tokio::test]
async fn test_explicit_url_wheel_bypasses_platform_filter() {
    let tmp = tempfile::tempdir().unwrap();

    // macosx wheel is normally rejected by is_accepted_wheel.
    let source = tmp.path().join("mypkg-1.0-py3-none-macosx_10_9_x86_64.whl");
    std::fs::write(&source, b"mac wheel bytes").unwrap();

    let dest = tmp
        .path()
        .join("simple")
        .join("mypkg")
        .join("mypkg-1.0-py3-none-macosx_10_9_x86_64.whl");

    let fi = downloader::FileInfo {
        explicit_url: true,
        filename: "mypkg-1.0-py3-none-macosx_10_9_x86_64.whl".to_string(),
        url: format!("file://{}", source.to_str().unwrap()),
        sha256: None,
        size: None,
        package_name: "mypkg".to_string(),
        version: "1.0".to_string(),
        yanked: None,
    };

    let client = reqwest::Client::new();
    let result = downloader::download_pkg_files_with_prefetched(
        &client,
        tmp.path(),
        &[fi],
        &downloader::PrefetchedFiles::new(),
        false,
        1,
        None,
    )
    .await;

    assert_eq!(result.downloaded.len(), 1);
    assert!(dest.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), b"mac wheel bytes");
}
