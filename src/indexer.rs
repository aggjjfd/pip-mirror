use std::path::Path;

use dashmap::DashMap;
use tracing::info;

use crate::store::DownloadStore;

static INDEX_HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Simple Index</title>
</head>
<body>
{links}
</body>
</html>
"#;

static PACKAGE_HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Links for {package_name}</title>
</head>
<body>
<h1>Links for {package_name}</h1>
{links}
</body>
</html>
"#;

pub fn generate_index(repository_dir: &Path) {
    let simple_dir = repository_dir.join("simple");
    if !simple_dir.exists() {
        info!("仓库目录为空，跳过索引生成");
        return;
    }

    info!("生成 PEP 503 / PEP 691 索引...");

    let store = DownloadStore::open(&repository_dir.join(".store.db"))
        .unwrap_or_else(|_| panic!("无法打开 .store.db"));
    let hashes = store.get_all_hashes();
    let metadata_hashes = store.get_all_metadata_hashes();

    let mut package_names: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&simple_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let pkg_name = path.file_name().unwrap().to_string_lossy().to_string();
            package_names.push(pkg_name.clone());

            let mut files: Vec<String> = Vec::new();
            if let Ok(pkg_entries) = std::fs::read_dir(&path) {
                for f in pkg_entries.flatten() {
                    let fname = f.file_name().to_string_lossy().to_string();
                    if fname.starts_with('.')
                        || fname.ends_with(".tmp")
                        || fname.ends_with(".metadata")
                    {
                        continue;
                    }
                    if f.path().is_file() {
                        files.push(fname);
                    }
                }
            }
            files.sort();

            let html = generate_package_html(&pkg_name, &files, &hashes, &metadata_hashes);
            let _ = std::fs::write(path.join("index.html"), html);

            let json = generate_package_json(&pkg_name, &files, &hashes, &metadata_hashes);
            let _ = std::fs::write(path.join("index.json"), json);
        }
    }

    package_names.sort();
    let root_html = generate_index_html(&package_names);
    let _ = std::fs::write(simple_dir.join("index.html"), root_html);

    let root_json = generate_index_json(&package_names);
    let _ = std::fs::write(simple_dir.join("index.json"), root_json);

    info!("索引生成完成: {} 个包", package_names.len());
}

fn generate_index_html(package_names: &[String]) -> String {
    let mut links = String::new();
    for name in package_names {
        links.push_str(&format!(r#"    <a href="{name}/">{name}</a><br/>{}"#, "\n"));
    }
    INDEX_HTML_TEMPLATE.replace("{links}", &links)
}

fn generate_index_json(package_names: &[String]) -> String {
    let projects: Vec<serde_json::Value> = package_names
        .iter()
        .map(|n| serde_json::json!({"name": n}))
        .collect();
    let data = serde_json::json!({
        "meta": {"api-version": "1.0"},
        "projects": projects,
    });
    serde_json::to_string_pretty(&data).unwrap()
}

fn generate_package_html(
    package_name: &str,
    files: &[String],
    hashes: &DashMap<String, String>,
    metadata_hashes: &DashMap<String, String>,
) -> String {
    let mut links = String::new();
    for f in files {
        let mut attrs = format!(r#"href="{f}""#);
        if let Some(sha) = hashes.get(f) {
            attrs.push_str(&format!(r#" data-sha256="{}""#, sha.value()));
        }
        if f.ends_with(".whl") {
            if let Some(meta) = metadata_hashes.get(f) {
                attrs.push_str(&format!(
                    r#" data-core-metadata="sha256={}" data-dist-info-metadata="sha256={}""#,
                    meta.value(),
                    meta.value()
                ));
            }
        }
        links.push_str(&format!(r#"    <a {attrs}>{f}</a><br/>{}"#, "\n"));
    }
    PACKAGE_HTML_TEMPLATE
        .replace("{package_name}", package_name)
        .replace("{links}", &links)
}

fn generate_package_json(
    package_name: &str,
    files: &[String],
    hashes: &DashMap<String, String>,
    metadata_hashes: &DashMap<String, String>,
) -> String {
    let file_entries: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            let mut entry = serde_json::json!({
                "filename": f,
                "url": f,
                "hashes": {},
            });
            if let Some(sha) = hashes.get(f) {
                entry["hashes"]["sha256"] = serde_json::Value::String(sha.value().clone());
            }
            if f.ends_with(".whl") {
                if let Some(meta) = metadata_hashes.get(f) {
                    entry["dist-info-metadata"] =
                        serde_json::json!({"sha256": meta.value()});
                }
            }
            entry
        })
        .collect();

    let data = serde_json::json!({
        "meta": {"api-version": "1.0"},
        "name": package_name,
        "files": file_entries,
    });
    serde_json::to_string_pretty(&data).unwrap()
}
