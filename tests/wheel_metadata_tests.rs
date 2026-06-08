use std::io::Write;

use pip_mirror::wheel_metadata::{
    MAX_METADATA_BYTES, MAX_ZIP_ENTRIES, extract_package_names,
    extract_requires_dist_from_bytes, extract_requires_dist_from_wheel,
    safe_requires_dist_summary,
};

fn build_test_wheel(metadata: &str, dist_info_name: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let metadata_path = format!("{dist_info_name}/METADATA");
        zip.start_file_from_path(&metadata_path, options).unwrap();
        zip.write_all(metadata.as_bytes()).unwrap();
        zip.start_file_from_path("testpkg/__init__.py", options)
            .unwrap();
        zip.write_all(b"# empty").unwrap();
        zip.finish().unwrap();
    }
    buf
}

#[test]
fn test_extract_package_names_skips_direct_url() {
    let dist = vec!["pkg @ https://example.com/foo.whl".to_string()];
    let names = extract_package_names(&dist);
    assert!(names.is_empty());
}

#[test]
fn test_extract_package_names_from_wheel() {
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
Requires-Dist: requests >=2.0
Requires-Dist: click >=7.0
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0.dist-info");
    let dist = extract_requires_dist_from_bytes(
        &wheel,
        "testpkg-1.0.dist-info/METADATA",
    )
    .unwrap();
    let names = extract_package_names(&dist);
    assert_eq!(names, vec!["requests", "click"]);
}

#[test]
fn test_extract_package_names_warns_on_invalid() {
    let dist = vec!["@@@".to_string(), "requests".to_string()];
    let names = extract_package_names(&dist);
    assert_eq!(names, vec!["requests"]);
}

#[test]
fn test_extract_requires_dist_from_bytes() {
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
Requires-Dist: requests >=2.0
Requires-Dist: click >=7.0
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0.dist-info");
    let dist = extract_requires_dist_from_bytes(
        &wheel,
        "testpkg-1.0.dist-info/METADATA",
    )
    .unwrap();
    assert_eq!(dist.len(), 2);
    assert!(dist.iter().any(|d| d.contains("requests")));
    assert!(dist.iter().any(|d| d.contains("click")));
}

#[test]
fn test_extract_requires_dist_from_wheel_file() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let wheel_path = tmp_dir.path().join("testpkg-1.0-py3-none-any.whl");
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
Requires-Dist: requests >=2.0
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0.dist-info");
    std::fs::write(&wheel_path, &wheel).unwrap();
    let dist = extract_requires_dist_from_wheel(
        &wheel_path,
        "testpkg-1.0.dist-info/METADATA",
    )
    .unwrap();
    assert_eq!(dist.len(), 1);
    assert!(dist[0].contains("requests"));
}

#[test]
fn test_extract_requires_dist_missing_metadata() {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file_from_path("testpkg/__init__.py", options)
            .unwrap();
        zip.write_all(b"# empty").unwrap();
        zip.finish().unwrap();
    }
    let err = extract_requires_dist_from_bytes(
        &buf,
        "testpkg-1.0.dist-info/METADATA",
    )
    .unwrap_err();
    assert!(err.contains("未找到"));
}

#[test]
fn test_extract_requires_dist_rejects_wrong_metadata_entry() {
    let metadata = r#"Metadata-Version: 2.1
Name: testpkg
Version: 1.0
Requires-Dist: requests >=2.0
"#;
    let wheel = build_test_wheel(metadata, "testpkg-1.0.dist-info");
    let err =
        extract_requires_dist_from_bytes(&wheel, "wrong.dist-info/METADATA")
            .unwrap_err();
    assert!(err.contains("未找到"));
}

#[test]
fn test_parse_requires_dist_basic() {
    let dist = vec!["requests >=2.0".to_string(), "click".to_string()];
    let names = extract_package_names(&dist);
    assert_eq!(names, vec!["requests", "click"]);
}

#[test]
fn test_parse_requires_dist_case_insensitive() {
    let dist = vec!["Requests >=2.0".to_string(), "Click".to_string()];
    let names = extract_package_names(&dist);
    assert_eq!(names, vec!["requests", "click"]);
}

#[test]
fn test_parse_requires_dist_continuation() {
    let line =
        "requests >=2.0 ; python_version >= '3.8' and extra == 'security'"
            .to_string();
    let names = extract_package_names(&[line]);
    assert_eq!(names, vec!["requests"]);
}

#[test]
fn test_extract_package_names_normalizes() {
    let dist = vec!["Some_Package.Name".to_string()];
    let names = extract_package_names(&dist);
    assert_eq!(names, vec!["some-package-name"]);
}

#[test]
fn test_safe_requires_dist_summary_redacts_embedded_https() {
    let line = "pkg@https://user:pass@example.com/foo.whl?token=secret";
    let safe = safe_requires_dist_summary(line);
    assert!(!safe.contains("user:pass"), "safe: {safe}");
    assert!(!safe.contains("token=secret"), "safe: {safe}");
    assert!(safe.contains("pkg@"), "safe: {safe}");
    assert!(safe.contains("example.com/foo.whl"), "safe: {safe}");
}

#[test]
fn test_safe_requires_dist_summary_redacts_non_http_schemes() {
    let line = "pkg @ file:///secret/path?token=abc";
    let safe = safe_requires_dist_summary(line);
    assert!(!safe.contains("token=abc"), "safe: {safe}");
    assert!(safe.contains("file:///secret/path"), "safe: {safe}");
}

#[test]
fn test_safe_requires_dist_summary_redacts_git_plus_https() {
    let line = "pkg @ git+https://TOKEN@github.com/org/repo.git";
    let safe = safe_requires_dist_summary(line);
    assert!(!safe.contains("TOKEN"), "safe: {safe}");
    assert!(safe.contains("git+https://"), "safe: {safe}");
    assert!(safe.contains("github.com/org/repo.git"), "safe: {safe}");
}

#[test]
fn test_safe_requires_dist_summary_redacts_malformed_url() {
    let line =
        "pkg @ https://user:pass@example.com:badport/foo.whl?token=secret";
    let safe = safe_requires_dist_summary(line);
    assert!(!safe.contains("user:pass"), "safe: {safe}");
    assert!(!safe.contains("token=secret"), "safe: {safe}");
}

#[test]
fn test_safe_requires_dist_summary_redacts_no_scheme_url() {
    let line = "pkg @ user:pass@example.com/foo.whl?token=secret";
    let safe = safe_requires_dist_summary(line);
    assert!(!safe.contains("user:pass"), "safe: {safe}");
    assert!(!safe.contains("token=secret"), "safe: {safe}");
}

#[test]
fn test_safe_requires_dist_summary_redacts_relative_path_url() {
    let line = "pkg @ /path/to/foo.whl?token=secret#frag";
    let safe = safe_requires_dist_summary(line);
    assert!(!safe.contains("token=secret"), "safe: {safe}");
    assert!(!safe.contains("frag"), "safe: {safe}");
    assert!(safe.contains("/path/to/foo.whl"), "safe: {safe}");
}

fn write_oversized_zip(buf: &mut Vec<u8>) {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(buf));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for index in 0..=MAX_ZIP_ENTRIES {
        let path = format!("dummy{index}.txt");
        zip.start_file_from_path(&path, options).unwrap();
        zip.write_all(b"x").unwrap();
    }
    zip.finish().unwrap();
}

fn write_oversized_metadata_zip(buf: &mut Vec<u8>) {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(buf));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    let huge_metadata = "x".repeat(MAX_METADATA_BYTES as usize + 1);
    zip.start_file_from_path("testpkg-1.0.dist-info/METADATA", options)
        .unwrap();
    zip.write_all(huge_metadata.as_bytes()).unwrap();
    zip.start_file_from_path("testpkg/__init__.py", options)
        .unwrap();
    zip.write_all(b"# empty").unwrap();
    zip.finish().unwrap();
}

#[test]
fn test_extract_requires_dist_honours_metadata_size_limit() {
    let mut buf = Vec::new();
    write_oversized_metadata_zip(&mut buf);
    let err = extract_requires_dist_from_bytes(
        &buf,
        "testpkg-1.0.dist-info/METADATA",
    )
    .unwrap_err();
    assert!(err.contains("METADATA 文件过大"));
}

#[test]
fn test_extract_requires_dist_honours_zip_entry_limit() {
    let mut buf = Vec::new();
    write_oversized_zip(&mut buf);
    let err = extract_requires_dist_from_bytes(
        &buf,
        "testpkg-1.0.dist-info/METADATA",
    )
    .unwrap_err();
    assert!(err.contains("zip 条目过多"));
}

#[test]
fn test_safe_requires_dist_summary_redacts_direct_url_dependency() {
    let line = "pkg @ https://user:pass@example.com/foo.whl?token=secret";
    let safe = safe_requires_dist_summary(line);
    assert!(
        !safe.contains("user:pass"),
        "safe summary leaked credentials: {safe}"
    );
    assert!(
        !safe.contains("token=secret"),
        "safe summary leaked token: {safe}"
    );
    assert!(
        safe.contains("example.com/foo.whl"),
        "safe summary should keep URL for context: {safe}"
    );
}
