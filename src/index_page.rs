use std::path::Path;

use crate::config::TargetSpec;

const INDEX_HTML_TMPL: &str = include_str!("index.html");

fn list_packages(repo_dir: &Path) -> Vec<String> {
    let simple_dir = repo_dir.join("simple");
    if !simple_dir.exists() {
        return vec![];
    }
    let mut pkgs = Vec::new();
    let entries = match std::fs::read_dir(&simple_dir) {
        Ok(e) => e,
        Err(_) => return pkgs,
    };
    for entry in entries.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !name.starts_with('.') {
            pkgs.push(name);
        }
    }
    pkgs.sort();
    pkgs
}

fn extract_version(name: &str, pkg: &str) -> Option<String> {
    let stem = name
        .strip_suffix(".whl")
        .or_else(|| name.strip_suffix(".tar.gz"))
        .or_else(|| name.strip_suffix(".zip"))
        .unwrap_or(name);
    let rest = stem.strip_prefix(pkg)?.trim_start_matches('-');
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

fn package_versions(repo_dir: &Path, pkg: &str) -> String {
    let pkg_dir = repo_dir.join("simple").join(pkg);
    let mut versions = Vec::new();
    let entries = match std::fs::read_dir(&pkg_dir) {
        Ok(e) => e,
        Err(_) => return String::new(),
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(v) = extract_version(&name, pkg) else {
            continue;
        };
        if !versions.contains(&v) {
            versions.push(v);
        }
    }
    versions.sort();
    versions.join(", ")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn build_targets_html(targets: &[TargetSpec]) -> String {
    if targets.is_empty() {
        return "<p class=\"note\">未配置特定目标环境，默认覆盖全部内置组合。</p>"
            .to_string();
    }
    let mut rows = String::new();
    for t in targets {
        rows.push_str(&format!(
            "<tr><td>Python {}</td><td>{}</td><td>{}</td></tr>\n",
            t.python, t.os, t.arch
        ));
    }
    format!(
        "<table class=\"target-table\">\n\
         <thead><tr><th>Python 版本</th><th>操作系统</th><th>架构</th></tr></thead>\n\
         <tbody>\n{}</tbody>\n</table>",
        rows
    )
}

fn build_pkg_items(repo_dir: &Path, pkgs: &[String]) -> String {
    pkgs.iter()
        .map(|n| {
            let versions = package_versions(repo_dir, n);
            if versions.is_empty() {
                format!("<span class=\"pkg-item\">{}</span>", n)
            } else {
                format!(
                    "<span class=\"pkg-item\" data-versions=\"{}\">{}</span>",
                    html_escape(&versions),
                    n
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render(targets: &[TargetSpec], repo_dir: &Path, host: &str) -> String {
    let targets_html = build_targets_html(targets);
    let pkgs = list_packages(repo_dir);
    let pkgs_json =
        serde_json::to_string(&pkgs).unwrap_or_else(|_| "[]".to_string());
    let pkg_items = build_pkg_items(repo_dir, &pkgs);
    let index_url = format!("http://{}/simple", host);
    let python_url = format!("http://{}/python-builds/index.json", host);
    INDEX_HTML_TMPL
        .replace("<!--TARGETS_PLACEHOLDER-->", &targets_html)
        .replace("data-pkgs='[]'", &format!("data-pkgs='{}'", pkgs_json))
        .replace("<!--PACKAGES_PLACEHOLDER-->", &pkg_items)
        .replace("<!--PKG_COUNT_PLACEHOLDER-->", &pkgs.len().to_string())
        .replace("<!--INDEX_URL-->", &index_url)
        .replace("<!--HOST_ONLY-->", host)
        .replace("<!--PYTHON_URL-->", &python_url)
}
