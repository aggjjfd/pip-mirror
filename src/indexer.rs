use std::path::Path;

use dashmap::DashMap;
use tracing::info;

use crate::store::DownloadStore;

static INDEX_HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>Simple Index</title></head>
<body>{links}</body></html>"#;

static PACKAGE_HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>Links for {package_name}</title></head>
<body><h1>Links for {package_name}</h1>{links}</body></html>"#;

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
    let Ok(entries) = std::fs::read_dir(&simple_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let pkg_name = path.file_name().unwrap().to_string_lossy().to_string();
        package_names.push(pkg_name.clone());

        let mut files: Vec<String> = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|f| f.path().is_file())
            .map(|f| f.file_name().to_string_lossy().to_string())
            .filter(|n| !n.starts_with('.') && !n.ends_with(".tmp") && !n.ends_with(".metadata"))
            .collect();
        files.sort();

        let ctx = PkgCtx {
            package_name: &pkg_name,
            files: &files,
            hashes: &hashes,
            metadata_hashes: &metadata_hashes,
        };
        let _ = std::fs::write(path.join("index.html"), generate_package_html(&ctx));
        let _ = std::fs::write(path.join("index.json"), generate_package_json(&ctx));
    }

    package_names.sort();
    let _ = std::fs::write(
        simple_dir.join("index.html"),
        generate_index_html(&package_names),
    );
    let _ = std::fs::write(
        simple_dir.join("index.json"),
        generate_index_json(&package_names),
    );

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
    serde_json::to_string_pretty(&serde_json::json!({
        "meta": {"api-version": "1.0"},
        "projects": projects,
    }))
    .unwrap()
}

struct PkgCtx<'a> {
    package_name: &'a str,
    files: &'a [String],
    hashes: &'a DashMap<String, String>,
    metadata_hashes: &'a DashMap<String, String>,
}

fn generate_package_html(ctx: &PkgCtx<'_>) -> String {
    let mut links = String::new();
    for f in ctx.files {
        let mut attrs = format!(r#"href="{f}""#);
        if let Some(sha) = ctx.hashes.get(f) {
            attrs.push_str(&format!(r#" data-sha256="{}""#, sha.value()));
        }
        if let Some(meta) = f
            .ends_with(".whl")
            .then(|| ctx.metadata_hashes.get(f))
            .flatten()
        {
            attrs.push_str(&format!(
                r#" data-core-metadata="sha256={}" data-dist-info-metadata="sha256={}""#,
                meta.value(),
                meta.value()
            ));
        }
        links.push_str(&format!(r#"    <a {attrs}>{f}</a><br/>{}"#, "\n"));
    }
    PACKAGE_HTML_TEMPLATE
        .replace("{package_name}", ctx.package_name)
        .replace("{links}", &links)
}

fn generate_package_json(ctx: &PkgCtx<'_>) -> String {
    let file_entries: Vec<serde_json::Value> = ctx
        .files
        .iter()
        .map(|f| {
            let mut entry = serde_json::json!({"filename": f, "url": f, "hashes": {}});
            if let Some(sha) = ctx.hashes.get(f) {
                entry["hashes"]["sha256"] = serde_json::Value::String(sha.value().clone());
            }
            if f.ends_with(".whl") && ctx.metadata_hashes.get(f).is_some() {
                entry["dist-info-metadata"] =
                    serde_json::json!({"sha256": ctx.metadata_hashes.get(f).unwrap().value()});
            }
            entry
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "meta": {"api-version": "1.0"},
        "name": ctx.package_name,
        "files": file_entries,
    }))
    .unwrap()
}
