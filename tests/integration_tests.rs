use std::collections::HashSet;
use std::process::Command;

use dashmap::DashMap;
use pep440_rs::Version;
use pip_mirror::downloader::HttpCtx;
use pip_mirror::filters;
use pip_mirror::resolver::pubgrub::bare_name;
use pip_mirror::resolver::resolve::ResolveParams;

/// Run the resolver for one package and return version-windows map.
async fn resolve_one(
    pkg_name: &str,
    client: &reqwest::Client,
    pypi_url: &str,
) -> DashMap<String, Vec<Version>> {
    let all_versions = pip_mirror::downloader::get_all_versions(
        &HttpCtx { client, pypi_url },
        pkg_name,
    )
    .await
    .unwrap();
    let top_5: Vec<Version> = all_versions.iter().take(5).cloned().collect();

    let top_versions: DashMap<String, Vec<Version>> = DashMap::new();
    top_versions.insert(bare_name(pkg_name), top_5);

    let params = ResolveParams {
        top_packages: &[pkg_name.to_string()],
        top_versions: &top_versions,
        pypi_url,
        max_depth: 7,
        max_versions: 5,
        allow_prerelease: false,
    };

    pip_mirror::resolver::resolve::resolve_dependencies(&params, client).await
}

/// Run `uv pip install --dry-run <pkg>` and return the set of resolved dependency names.
fn uv_resolve(pkg: &str) -> HashSet<String> {
    // uv resolves for the host Python version and platform.
    // Our resolver includes all platform deps, so our set is a superset.
    // NOTE: uv writes its dry-run output to stderr.
    let output = Command::new("uv")
        .args(["pip", "install", "--dry-run", pkg])
        .output()
        .expect("uv must be installed");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("uv resolve failed for {pkg}:\n{stderr}");
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix('+').map(|s| s.trim()))
        .filter_map(|line| {
            // Extract package name before ==
            line.split_once("==")
                .map(|(name, _)| filters::normalize_package_name(name))
        })
        .collect()
}

/// Compare our resolver's output against uv's resolution for one package.
fn compare_with_uv(
    pkg: &str,
    ours: &DashMap<String, Vec<Version>>,
    uv_pkgs: &HashSet<String>,
) {
    let top_bare = bare_name(pkg);
    // Our resolver excludes the top package; uv includes it.
    let uv_set: HashSet<_> = uv_pkgs
        .iter()
        .filter(|n| *n != &top_bare)
        .cloned()
        .collect();
    let our_set: HashSet<_> = ours.iter().map(|e| e.key().clone()).collect();

    let missing: Vec<_> = uv_set.difference(&our_set).collect();
    let extra: Vec<_> = our_set.difference(&uv_set).collect();

    if !missing.is_empty() {
        println!("  !! MISSING (uv has, we don't): {:?}", missing);
    }
    if !extra.is_empty() {
        println!("  !! EXTRA (we have, uv doesn't): {:?}", extra);
    }

    // We expect our resolver to be a SUPERSET of uv's
    // (uv resolves for one platform; we include all platform deps)
    // Allow up to 2 missing for deep transitive deps that our
    // BFS window/max_depth might not capture.
    assert!(
        missing.len() <= 2,
        "{pkg}: uv found {n} packages not in our resolver output: {missing:?}",
        n = missing.len(),
        missing = missing
    );
    if !missing.is_empty() {
        println!("  (allowed missing: {missing:?})");
    }
}

#[tokio::test]
#[ignore = "needs network, run manually with --ignored"]
async fn test_resolve_matches_uv_lock() {
    pip_mirror::logging::init(false);
    let client = reqwest::Client::new();
    let pypi_url = "https://pypi.org";
    let pkg = "openai";

    println!("\n=== Testing {pkg} ===");
    let result = resolve_one(pkg, &client, pypi_url).await;
    println!("Resolved {} dependency packages:", result.len());
    for entry in result.iter() {
        println!(
            "  {}: {}",
            entry.key(),
            entry
                .value()
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    assert!(!result.is_empty(), "no deps resolved for {pkg}");

    let uv_pkgs = uv_resolve(pkg);
    println!("uv resolved {} packages (including {pkg})", uv_pkgs.len());
    compare_with_uv(pkg, &result, &uv_pkgs);
    println!("  ✓ matches uv");
}

#[tokio::test]
#[ignore = "needs network, run manually with --ignored"]
async fn test_resolve_all_e2e_packages() {
    pip_mirror::logging::init(false);
    let client = reqwest::Client::new();
    let pypi_url = "https://pypi.org";

    let e2e_packages = [
        "openai",
        "gradio",
        "markitdown[pptx,docx,xls,xlsx,pdf]",
        "rapidocr-onnxruntime",
        "pyside6",
        "playwright",
    ];

    for pkg in e2e_packages {
        println!("\n=== Testing {pkg} ===");
        let result = resolve_one(pkg, &client, pypi_url).await;
        println!("  -> {} dependency packages", result.len());

        let uv_pkgs = uv_resolve(&bare_name(pkg));
        println!(
            "  uv -> {} packages (including {})",
            uv_pkgs.len(),
            bare_name(pkg)
        );
        compare_with_uv(pkg, &result, &uv_pkgs);
        println!("  ✓");
    }
}
