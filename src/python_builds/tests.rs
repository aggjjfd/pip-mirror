use super::*;

#[test]
fn test_python_build_entry_builder() {
    let e = PythonBuildEntry::builder()
        .key("k".to_string())
        .url("u".to_string())
        .filename("f".to_string())
        .sha256(Some("s".to_string()))
        .raw(serde_json::json!({"url":"u","sha256":"s"}))
        .build();
    assert_eq!(e.key, "k");
    assert_eq!(e.sha256, Some("s".to_string()));

    let url = "https://e/p%2B3.tar.gz";
    let p = PythonBuildEntry::builder()
        .key("e".to_string())
        .url(url.to_string())
        .filename(
            url.rfind('/')
                .map(|p| url[p + 1..].replace("%2B", "+").to_string())
                .unwrap_or_default(),
        )
        .raw(serde_json::json!({"url":url}))
        .build();
    assert_eq!(p.filename, "p+3.tar.gz");

    let x = PythonBuildEntry::builder()
        .key("x".to_string())
        .url("".to_string())
        .filename("".to_string())
        .raw(serde_json::json!({}))
        .build();
    assert_eq!(x.url, "");
    assert_eq!(x.sha256, None);
}
