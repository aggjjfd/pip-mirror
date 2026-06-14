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
use dashmap::DashMap;
use futures::future::join_all;
use pep440_rs::Version;
use pip_mirror::downloader::FileInfo;
use pip_mirror::filters::version_is_installable_for_target;
use pip_mirror::resolver::eligibility::{ParsedDepsCacheKey, SolveContext};
use pip_mirror::resolver::markers::parse_requires_dist;
use pip_mirror::resolver::metadata::MetadataCache;
use pip_mirror::resolver::plan::{PlanParams, build_dependency_plan};
use pip_mirror::resolver::solve::solve_one_target;
use pip_mirror::resolver::types::TargetEnv;
use serde_json::{Value, json};

fn linux_target() -> TargetEnv {
    TargetEnv::test_env("linux", "x86_64", "3.12")
}

fn win_target() -> TargetEnv {
    TargetEnv::test_env("win32", "AMD64", "3.12")
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

    let py311 = TargetEnv::test_env("linux", "x86_64", "3.11");
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
        explicit_url: false,
        filename: name.to_string(),
        url: "https://example.com/file".to_string(),
        sha256: None,
        size: None,
        package_name: "demo".to_string(),
        version: "1.0.0".to_string(),
        yanked: None,
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

    let win32 = TargetEnv::test_env("win32", "x86", "3.12");
    assert!(!version_is_installable_for_target(
        &files, &win32, false, "2.39",
    ));
}

#[test]
fn test_installability_accepts_sdist_fallback_when_enabled() {
    let files = vec![file("demo-1.0.0-py3-none-manylinux_2_40_x86_64.whl")];
    assert!(!version_is_installable_for_target(
        &files,
        &linux_target(),
        false,
        "2.39",
    ));
    let with_non_pure_sdist = vec![
        file("demo-1.0.0-py3-none-manylinux_2_40_x86_64.whl"),
        file("demo-1.0.0.tar.gz"),
    ];
    assert!(version_is_installable_for_target(
        &with_non_pure_sdist,
        &linux_target(),
        true,
        "2.39",
    ));
    let with_wheel_evidence_sdist = vec![
        file("demo-1.0.0-py2-none-any.whl"),
        file("demo-1.0.0.tar.gz"),
    ];
    assert!(version_is_installable_for_target(
        &with_wheel_evidence_sdist,
        &linux_target(),
        true,
        "2.39",
    ));
}

#[derive(Clone)]
struct FixtureState {
    package_hits: Arc<AtomicUsize>,
    version_hits: Arc<AtomicUsize>,
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
    state.version_hits.fetch_add(1, Ordering::SeqCst);
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
    let version_hits = Arc::new(AtomicUsize::new(0));
    let state = FixtureState {
        package_hits: package_hits.clone(),
        version_hits: version_hits.clone(),
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
        version_hits: Arc::new(AtomicUsize::new(0)),
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
        metadata_workers: 8,
        parsed_deps_cache: None,
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

#[tokio::test]
async fn test_parsed_deps_cache_filled_during_solve() {
    let state = FixtureState {
        package_hits: Arc::new(AtomicUsize::new(0)),
        version_hits: Arc::new(AtomicUsize::new(0)),
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
    let parsed_deps_cache: DashMap<
        ParsedDepsCacheKey,
        Vec<pip_mirror::resolver::markers::ParsedDependency>,
    > = DashMap::new();
    let ctx = SolveContext {
        cache: &cache,
        target: &target,
        allow_prerelease: false,
        include_source: false,
        linux_max_glibc: "2.39",
        metadata_workers: 8,
        parsed_deps_cache: Some(&parsed_deps_cache),
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
    // 验证 ParsedDepsCache 在求解过程中被填充
    assert!(!parsed_deps_cache.is_empty());

    // 用相同的缓存再次求解，验证结果一致
    let result2 = solve_one_target(
        &ctx,
        "demo-root",
        &"1.0.0".parse().unwrap(),
        &HashSet::new(),
    )
    .await
    .unwrap();
    assert_eq!(result.solved_versions, result2.solved_versions);
}

#[tokio::test]
async fn test_prefilter_skips_incompatible_versions() {
    let version_hits = Arc::new(AtomicUsize::new(0));
    let state = FixtureState {
        package_hits: Arc::new(AtomicUsize::new(0)),
        version_hits: version_hits.clone(),
        package_json: Arc::new(HashMap::from([
            (
                "demo-macos-only".to_string(),
                json!({
                    "releases": {
                        "1.0.0": [{
                            "filename": "demo-macos-only-1.0.0-py3-none-macosx_11_0_arm64.whl",
                            "url": "https://example.com/demo-macos-only-1.0.0.whl",
                            "digests": { "sha256": "abc" },
                            "size": 1,
                        }]
                    }
                }),
            ),
            (
                "demo-universal".to_string(),
                package_response("demo-universal", &["1.0.0"]),
            ),
        ])),
        version_json: Arc::new(HashMap::from([(
            ("demo-universal".to_string(), "1.0.0".to_string()),
            version_response(&[]),
        )])),
    };
    let base_url = spawn_fixture_server(state).await;

    let top_packages =
        vec!["demo-macos-only".to_string(), "demo-universal".to_string()];
    let params = PlanParams {
        top_packages: &top_packages,
        pypi_url: &base_url,
        top_versions_per_package: 1,
        adjacent_versions_per_side: 0,
        allow_prerelease: false,
        include_source: false,
        linux_max_glibc: "2.39",
        resolve_workers: 2,
        metadata_workers: 4,
        targets: vec![linux_target()],
    };

    let plan = build_dependency_plan(&params, &reqwest::Client::new(), None)
        .await
        .unwrap();

    // demo-universal 应该有 planned files
    assert!(!plan.planned_files.is_empty());

    // demo-macos-only 在 linux target 下不可安装，不应产生 planned files
    let macos_files: Vec<_> = plan
        .planned_files
        .iter()
        .filter(|f| f.package_name == "demo-macos-only")
        .collect();
    assert!(
        macos_files.is_empty(),
        "demo-macos-only 不应有 planned files"
    );

    // version json 仅被 demo-universal 请求（demo-macos-only 被预过滤跳过）
    assert_eq!(
        version_hits.load(Ordering::SeqCst),
        1,
        "version json 请求数应为 1（仅 demo-universal）"
    );
}

#[tokio::test]
async fn test_metadata_cache_fetch_json_error_does_not_leak_credentials() {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let cache = MetadataCache::new(
        client,
        "http://user:pass@127.0.0.1:1".to_string(),
        1,
    );
    let err = cache
        .get_all_versions("demo")
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(
        !msg.contains("token"),
        "error may contain sensitive query: {msg}"
    );
}

async fn start_raw_http_server(response: Vec<u8>) -> u16 {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let _ = tx.send(());
        if let Ok((mut socket, _)) = listener.accept().await {
            let _ = socket.write_all(&response).await;
        }
    });

    let _ = rx.await;
    port
}

#[tokio::test]
async fn test_metadata_cache_json_error_does_not_leak_credentials() {
    use std::time::Duration;

    let body = b"hello";
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    let mut full_response = response;
    full_response.extend_from_slice(body);
    let port = start_raw_http_server(full_response).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let cache = MetadataCache::new(
        client,
        format!("http://user:pass@127.0.0.1:{port}"),
        1,
    );
    let err = cache
        .get_all_versions("demo")
        .await
        .expect_err("should fail");
    let msg = format!("{err}");
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(
        !msg.contains("token"),
        "error may contain sensitive query: {msg}"
    );
}
