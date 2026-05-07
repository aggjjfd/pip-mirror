use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use futures::future::join_all;
use pep440_rs::Version;
use pip_mirror::downloader::FileInfo;
use pip_mirror::filters::version_is_installable_for_target;
use pip_mirror::resolver::eligibility::SolveContext;
use pip_mirror::resolver::markers::parse_requires_dist;
use pip_mirror::resolver::metadata::MetadataCache;
use pip_mirror::resolver::solve::solve_one_target;
use pip_mirror::resolver::types::TargetEnv;
use serde_json::{Value, json};

fn linux_target() -> TargetEnv {
    TargetEnv {
        python_version: "3.12".to_string(),
        python_full_version: "3.12.0".to_string(),
        sys_platform: "linux".to_string(),
        platform_machine: "x86_64".to_string(),
        platform_system: "Linux".to_string(),
        os_name: "posix".to_string(),
        implementation_name: "cpython".to_string(),
        platform_python_implementation: "CPython".to_string(),
        implementation_version: "3.12.0".to_string(),
    }
}

fn win_target() -> TargetEnv {
    TargetEnv {
        python_version: "3.12".to_string(),
        python_full_version: "3.12.0".to_string(),
        sys_platform: "win32".to_string(),
        platform_machine: "AMD64".to_string(),
        platform_system: "Windows".to_string(),
        os_name: "nt".to_string(),
        implementation_name: "cpython".to_string(),
        platform_python_implementation: "CPython".to_string(),
        implementation_version: "3.12.0".to_string(),
    }
}

#[test]
fn test_markers_platform_filtering() {
    let lines = vec![
        "linux-dep; sys_platform == 'linux'".to_string(),
        "win-dep; sys_platform == 'win32'".to_string(),
        "universal-dep".to_string(),
    ];

    let linux_deps =
        parse_requires_dist(&lines, &HashSet::new(), &linux_target()).unwrap();
    assert_eq!(linux_deps.len(), 2);
    let names: HashSet<_> =
        linux_deps.iter().map(|d| d.package_name.clone()).collect();
    assert!(names.contains("linux-dep"));
    assert!(names.contains("universal-dep"));

    let win_deps =
        parse_requires_dist(&lines, &HashSet::new(), &win_target()).unwrap();
    assert_eq!(win_deps.len(), 2);
    let names: HashSet<_> =
        win_deps.iter().map(|d| d.package_name.clone()).collect();
    assert!(names.contains("win-dep"));
    assert!(names.contains("universal-dep"));
}

#[test]
fn test_markers_python_version_fork() {
    let lines = vec![
        "numpy>=1.24; python_version < '3.12'".to_string(),
        "numpy>=1.26; python_version >= '3.12'".to_string(),
    ];

    let py311 = TargetEnv {
        python_version: "3.11".to_string(),
        python_full_version: "3.11.0".to_string(),
        ..linux_target()
    };
    let deps = parse_requires_dist(&lines, &HashSet::new(), &py311).unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].version_spec, ">=1.24");

    let py312 = linux_target();
    let deps = parse_requires_dist(&lines, &HashSet::new(), &py312).unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].version_spec, ">=1.26");
}

fn file(name: &str) -> FileInfo {
    FileInfo {
        filename: name.to_string(),
        url: "https://example.com/file".to_string(),
        sha256: None,
        size: None,
        package_name: "demo".to_string(),
        version: "1.0.0".to_string(),
    }
}

#[test]
fn test_installability_respects_target_platform() {
    let files = vec![
        file("demo-1.0.0-py3-none-manylinux_2_28_x86_64.whl"),
        file("demo-1.0.0-py3-none-win_amd64.whl"),
    ];
    assert!(version_is_installable_for_target(
        &files,
        &linux_target(),
        false,
        "2.39",
    ));
    assert!(version_is_installable_for_target(
        &files,
        &win_target(),
        false,
        "2.39",
    ));

    let win32 = TargetEnv {
        platform_machine: "x86".to_string(),
        ..win_target()
    };
    assert!(!version_is_installable_for_target(
        &files, &win32, false, "2.39",
    ));
}

#[test]
fn test_installability_rejects_newer_glibc_without_sdist() {
    let files = vec![file("demo-1.0.0-py3-none-manylinux_2_40_x86_64.whl")];
    assert!(!version_is_installable_for_target(
        &files,
        &linux_target(),
        false,
        "2.39",
    ));
    let with_sdist = vec![
        file("demo-1.0.0-py3-none-manylinux_2_40_x86_64.whl"),
        file("demo-1.0.0.tar.gz"),
    ];
    assert!(version_is_installable_for_target(
        &with_sdist,
        &linux_target(),
        true,
        "2.39",
    ));
}

#[derive(Clone)]
struct FixtureState {
    package_hits: Arc<AtomicUsize>,
    package_json: Arc<HashMap<String, Value>>,
    version_json: Arc<HashMap<(String, String), Value>>,
}

async fn package_handler(
    State(state): State<FixtureState>,
    Path(package): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.package_hits.fetch_add(1, Ordering::SeqCst);
    state
        .package_json
        .get(&package)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn version_handler(
    State(state): State<FixtureState>,
    Path((package, version)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
    state
        .version_json
        .get(&(package, version))
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn spawn_fixture_server(state: FixtureState) -> String {
    let app = Router::new()
        .route("/pypi/{package}/json", get(package_handler))
        .route("/pypi/{package}/{version}/json", get(version_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn release_file(package: &str, version: &str) -> Value {
    json!({
        "filename": format!("{package}-{version}-py3-none-any.whl"),
        "url": format!("https://example.com/{package}/{version}.whl"),
        "digests": { "sha256": "abc" },
        "size": 1,
    })
}

fn package_response(package: &str, versions: &[&str]) -> Value {
    let releases = versions
        .iter()
        .map(|version| {
            (
                (*version).to_string(),
                Value::Array(vec![release_file(package, version)]),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    json!({ "releases": releases })
}

fn version_response(requires_dist: &[&str]) -> Value {
    json!({
        "info": {
            "requires_dist": requires_dist,
            "requires_python": ">=3.8",
        }
    })
}

fn fixture_cache(base_url: &str) -> MetadataCache {
    MetadataCache::new(reqwest::Client::new(), base_url.to_string(), 8)
}

#[tokio::test]
async fn test_metadata_cache_dedupes_inflight_package_requests() {
    let package_hits = Arc::new(AtomicUsize::new(0));
    let state = FixtureState {
        package_hits: package_hits.clone(),
        package_json: Arc::new(HashMap::from([(
            "demo".to_string(),
            package_response("demo", &["1.0.0"]),
        )])),
        version_json: Arc::new(HashMap::new()),
    };
    let base_url = spawn_fixture_server(state).await;
    let cache = fixture_cache(&base_url);

    let results =
        join_all((0..8).map(|_| cache.get_all_versions("demo"))).await;
    for result in results {
        let versions = result.unwrap();
        assert_eq!(versions, vec!["1.0.0".parse::<Version>().unwrap()]);
    }
    assert_eq!(package_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_solve_one_target_reaches_fixpoint_after_extra_propagation() {
    let state = FixtureState {
        package_hits: Arc::new(AtomicUsize::new(0)),
        package_json: Arc::new(HashMap::from([
            (
                "demo-root".to_string(),
                package_response("demo-root", &["1.0.0"]),
            ),
            (
                "demo-mid".to_string(),
                package_response("demo-mid", &["1.0.0"]),
            ),
            (
                "demo-leaf".to_string(),
                package_response("demo-leaf", &["1.0.0"]),
            ),
        ])),
        version_json: Arc::new(HashMap::from([
            (
                ("demo-root".to_string(), "1.0.0".to_string()),
                version_response(&["demo-mid[feature]>=1.0"]),
            ),
            (
                ("demo-mid".to_string(), "1.0.0".to_string()),
                version_response(&["demo-leaf>=1.0; extra == 'feature'"]),
            ),
            (
                ("demo-leaf".to_string(), "1.0.0".to_string()),
                version_response(&[]),
            ),
        ])),
    };
    let base_url = spawn_fixture_server(state).await;
    let cache = fixture_cache(&base_url);
    let target = linux_target();
    let ctx = SolveContext {
        cache: &cache,
        target: &target,
        allow_prerelease: false,
        include_source: false,
        linux_max_glibc: "2.39",
    };

    let result = solve_one_target(
        &ctx,
        "demo-root",
        &"1.0.0".parse().unwrap(),
        &HashSet::new(),
    )
    .await
    .unwrap();

    assert!(result.solved_versions.contains_key("demo-root"));
    assert!(result.solved_versions.contains_key("demo-mid"));
    assert!(result.solved_versions.contains_key("demo-leaf"));
    assert!(
        result
            .active_extras
            .get("demo-mid")
            .is_some_and(|extras| extras.contains("feature"))
    );
}
