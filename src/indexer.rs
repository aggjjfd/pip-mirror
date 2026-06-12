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

fn collect_files(path: &std::path::Path) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|f| f.path().is_file())
        .map(|f| f.file_name().to_string_lossy().to_string())
        .filter(|n| {
            !n.starts_with('.')
                && !n.ends_with(".tmp")
                && !n.ends_with(".metadata")
        })
        .collect();
    files.sort();
    files
}

struct IndexPkg<'a> {
    path: &'a std::path::Path,
    name: &'a str,
    hashes: &'a DashMap<String, String>,
    meta: &'a DashMap<String, String>,
    yanked: &'a DashMap<String, String>,
}

fn index_pkg(ctx: &IndexPkg<'_>) {
    let files = collect_files(ctx.path);
    let pkg_ctx = PkgCtx {
        package_name: ctx.name,
        files: &files,
        hashes: ctx.hashes,
        metadata_hashes: ctx.meta,
        yanked: ctx.yanked,
    };
    let _ = std::fs::write(
        ctx.path.join("index.html"),
        generate_package_html(&pkg_ctx),
    );
    let _ = std::fs::write(
        ctx.path.join("index.json"),
        generate_package_json(&pkg_ctx),
    );
}

pub fn generate_index(repository_dir: &Path) {
    let simple_dir = repository_dir.join("simple");
    if !simple_dir.exists() {
        info!("仓库目录为空，跳过索引生成");
        return;
    }
    info!("生成 PEP 503 / PEP 691 索引...");
    let db_path = repository_dir.join(".store.db");
    let (hashes, meta, yanked) =
        if let Ok(store) = DownloadStore::open(&db_path) {
            (
                store.get_all_hashes(),
                store.get_all_metadata_hashes(),
                store.get_all_yanked(),
            )
        } else {
            info!(".store.db 不存在或无法打开，使用空 hash/yanked 生成索引");
            (DashMap::new(), DashMap::new(), DashMap::new())
        };
    let mut names: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&simple_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let n = path.file_name().unwrap().to_string_lossy().to_string();
        names.push(n.clone());
        index_pkg(&IndexPkg {
            path: &path,
            name: &n,
            hashes: &hashes,
            meta: &meta,
            yanked: &yanked,
        });
    }
    names.sort();
    let _ = std::fs::write(
        simple_dir.join("index.html"),
        generate_index_html(&names),
    );
    let _ = std::fs::write(
        simple_dir.join("index.json"),
        generate_index_json(&names),
    );
    info!("索引生成完成: {} 个包", names.len());
}

fn generate_index_html(package_names: &[String]) -> String {
    let links: Vec<String> = package_names
        .iter()
        .map(|name| format!(r#"    <a href="{name}/">{name}</a><br/>"#))
        .collect();
    INDEX_HTML_TEMPLATE.replace("{links}", &links.join("\n"))
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
    yanked: &'a DashMap<String, String>,
}

fn build_link_attrs(
    f: &str,
    hashes: &DashMap<String, String>,
    meta: &DashMap<String, String>,
    yanked: &DashMap<String, String>,
) -> String {
    let mut attrs = format!(r#"href="{f}""#);
    if let Some(sha) = hashes.get(f) {
        attrs.push_str(&format!(r#" data-sha256="{}""#, sha.value()));
    }
    if f.ends_with(".whl")
        && let Some(m) = meta.get(f)
    {
        attrs.push_str(&format!(
            r#" data-core-metadata="sha256={}" data-dist-info-metadata="sha256={}""#,
            m.value(),
            m.value()
        ));
    }
    if let Some(y) = yanked.get(f) {
        attrs.push_str(&format!(r#" data-yanked="{}""#, y.value()));
    }
    attrs
}

fn generate_package_html(ctx: &PkgCtx<'_>) -> String {
    let links: Vec<String> = ctx
        .files
        .iter()
        .map(|f| {
            let attrs = build_link_attrs(
                f,
                ctx.hashes,
                ctx.metadata_hashes,
                ctx.yanked,
            );
            format!(r#"    <a {attrs}>{f}</a><br/>"#)
        })
        .collect();
    PACKAGE_HTML_TEMPLATE
        .replace("{package_name}", ctx.package_name)
        .replace("{links}", &links.join("\n"))
}

fn generate_package_json(ctx: &PkgCtx<'_>) -> String {
    let file_entries: Vec<serde_json::Value> = ctx
        .files
        .iter()
        .map(|f| {
            let mut entry =
                serde_json::json!({"filename": f, "url": f, "hashes": {}});
            if let Some(sha) = ctx.hashes.get(f) {
                entry["hashes"]["sha256"] =
                    serde_json::Value::String(sha.value().clone());
            }
            if f.ends_with(".whl")
                && let Some(meta) = ctx.metadata_hashes.get(f)
            {
                entry["dist-info-metadata"] =
                    serde_json::json!({"sha256": meta.value()});
            }
            if let Some(y) = ctx.yanked.get(f) {
                entry["yanked"] = serde_json::Value::String(y.value().clone());
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
