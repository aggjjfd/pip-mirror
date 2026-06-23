use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use pip_mirror::downloader::{
    BatchDownloader, DownloadPolicy, DownloadableItem, FileInfo,
    PrefetchedFiles,
};
use pip_mirror::http::HttpClient;
use pip_mirror::resolver::eligibility::{SolveContext, version_matches_target};
use pip_mirror::resolver::metadata::MetadataCache;
use pip_mirror::resolver::plan::{PlanParams, build_dependency_plan};
use pip_mirror::resolver::types::TargetEnv;
use serde_json::{Value, json};
use tempfile::TempDir;

const LINUX_MAX_GLIBC: &str = "2.39";

#[derive(Clone, Default)]
struct PeakCounter {
    current: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl PeakCounter {
    fn enter(&self) -> PeakGuard {
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        raise_peak(&self.peak, current);
        PeakGuard {
            current: self.current.clone(),
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

struct PeakGuard {
    current: Arc<AtomicUsize>,
}

impl Drop for PeakGuard {
    fn drop(&mut self) {
        self.current.fetch_sub(1, Ordering::SeqCst);
    }
}

fn raise_peak(peak: &AtomicUsize, current: usize) {
    loop {
        let observed = peak.load(Ordering::SeqCst);
        if current <= observed {
            return;
        }
        if peak
            .compare_exchange(
                observed,
                current,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            return;
        }
    }
}

#[derive(Clone)]
struct MetadataFixtureState {
    counter: PeakCounter,
    delay: Duration,
    package_json: Arc<HashMap<String, Value>>,
    version_json: Arc<HashMap<(String, String), Value>>,
}

#[derive(Clone)]
struct DownloadFixtureState {
    counter: PeakCounter,
    delay: Duration,
    files: Arc<HashMap<String, Vec<u8>>>,
}

async fn metadata_package_handler(
    State(state): State<MetadataFixtureState>,
    Path(package): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let _guard = state.counter.enter();
    tokio::time::sleep(state.delay).await;
    state
        .package_json
        .get(&package)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn metadata_version_handler(
    State(state): State<MetadataFixtureState>,
    Path((package, version)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
    let _guard = state.counter.enter();
    tokio::time::sleep(state.delay).await;
    state
        .version_json
        .get(&(package, version))
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn download_handler(
    State(state): State<DownloadFixtureState>,
    Path(filename): Path<String>,
) -> Result<Vec<u8>, StatusCode> {
    let _guard = state.counter.enter();
    tokio::time::sleep(state.delay).await;
    state
        .files
        .get(&filename)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)
}

async fn spawn_metadata_server(state: MetadataFixtureState) -> String {
    let app = Router::new()
        .route("/pypi/{package}/json", get(metadata_package_handler))
        .route(
            "/pypi/{package}/{version}/json",
            get(metadata_version_handler),
        )
        .with_state(state);
    spawn_server(app).await
}

async fn spawn_download_server(state: DownloadFixtureState) -> String {
    let app = Router::new()
        .route("/files/{filename}", get(download_handler))
        .with_state(state);
    spawn_server(app).await
}

async fn spawn_server(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn metadata_package_response(package: &str) -> Value {
    json!({
        "releases": {
            "1.0.0": [metadata_release_file(package, "1.0.0")]
        }
    })
}

fn metadata_release_file(package: &str, version: &str) -> Value {
    json!({
        "filename": format!("{package}-{version}-py3-none-any.whl"),
        "url": format!("https://example.com/{package}/{version}.whl"),
        "digests": { "sha256": "abc" },
        "size": 1,
    })
}

fn metadata_version_response() -> Value {
    json!({
        "info": {
            "requires_dist": [],
            "requires_python": ">=3.8",
        }
    })
}

fn download_file(filename: &str, base_url: &str) -> DownloadableItem {
    DownloadableItem::Remote(
        FileInfo::builder()
            .filename(filename.to_string())
            .url(format!("{base_url}/files/{filename}"))
            .package_name("demo".to_string())
            .version("1.0.0".to_string())
            .size(Some(8))
            .build(),
    )
}

fn linux_target() -> TargetEnv {
    TargetEnv::test_env("linux", "x86_64", "3.12")
}

fn test_client() -> HttpClient {
    HttpClient::builder().build().unwrap()
}

fn metadata_cache(base_url: &str) -> MetadataCache {
    MetadataCache::new(test_client(), base_url.to_string(), 8)
}

#[tokio::test]
async fn test_build_dependency_plan_fetches_metadata_concurrently() {
    let counter = PeakCounter::default();
    let state = MetadataFixtureState {
        counter: counter.clone(),
        delay: Duration::from_millis(40),
        package_json: Arc::new(HashMap::from([
            ("demo-a".to_string(), metadata_package_response("demo-a")),
            ("demo-b".to_string(), metadata_package_response("demo-b")),
        ])),
        version_json: Arc::new(HashMap::from([
            (
                ("demo-a".to_string(), "1.0.0".to_string()),
                metadata_version_response(),
            ),
            (
                ("demo-b".to_string(), "1.0.0".to_string()),
                metadata_version_response(),
            ),
        ])),
    };
    let base_url = spawn_metadata_server(state).await;
    let base_urls = vec![base_url];
    let top_packages = vec!["demo-a".to_string(), "demo-b".to_string()];
    let params = PlanParams {
        top_packages: &top_packages,
        pypi_urls: &base_urls,
        top_versions_per_package: 1,
        adjacent_versions_per_side: 0,
        allow_prerelease: false,
        include_source: false,
        linux_max_glibc: LINUX_MAX_GLIBC,
        resolve_workers: 4,
        metadata_workers: 8,
        targets: pip_mirror::resolver::types::TargetEnv::all_resolution_targets(
        ),
        version_specs: &std::collections::HashMap::new(),
    };

    let plan = build_dependency_plan(&params, &test_client(), None)
        .await
        .unwrap();
    assert!(!plan.planned_files.is_empty());
    assert!(counter.peak() > 1, "metadata peak concurrency must be > 1");
}

#[tokio::test]
async fn test_download_pkg_files_runs_concurrently() {
    let counter = PeakCounter::default();
    let filenames = [
        "demo-1.0.0-py3-none-any.whl",
        "demo-1.0.1-py3-none-any.whl",
        "demo-1.0.2-py3-none-any.whl",
    ];
    let state = DownloadFixtureState {
        counter: counter.clone(),
        delay: Duration::from_millis(40),
        files: Arc::new(HashMap::from([
            (filenames[0].to_string(), b"wheel-a".to_vec()),
            (filenames[1].to_string(), b"wheel-b".to_vec()),
            (filenames[2].to_string(), b"wheel-c".to_vec()),
        ])),
    };
    let base_url = spawn_download_server(state).await;
    let repo = TempDir::new().unwrap();
    let files = filenames
        .iter()
        .map(|filename| download_file(filename, &base_url))
        .collect::<Vec<_>>();

    let client = HttpClient::builder().build().unwrap();
    let policy = DownloadPolicy {
        include_source: false,
        workers: 4,
    };
    let downloader =
        BatchDownloader::new(client, repo.path(), None, policy, None);
    let result = downloader.download(&files, &PrefetchedFiles::new()).await;

    assert!(result.failed.is_empty());
    assert_eq!(result.downloaded.len(), filenames.len());
    assert!(counter.peak() > 1, "download peak concurrency must be > 1");
}

#[tokio::test]
async fn test_version_matches_target_normalizes_legacy_requires_python() {
    let state = MetadataFixtureState {
        counter: PeakCounter::default(),
        delay: Duration::from_millis(1),
        package_json: Arc::new(HashMap::from([(
            "demo".to_string(),
            metadata_package_response("demo"),
        )])),
        version_json: Arc::new(HashMap::from([(
            ("demo".to_string(), "1.0.0".to_string()),
            json!({ "info": { "requires_dist": [], "requires_python": ">=3.6," } }),
        )])),
    };
    let base_url = spawn_metadata_server(state).await;
    let cache = metadata_cache(&base_url);
    let target = linux_target();
    let ctx = SolveContext {
        cache: &cache,
        target: &target,
        allow_prerelease: false,
        include_source: false,
        linux_max_glibc: LINUX_MAX_GLIBC,
        metadata_workers: 8,
        parsed_deps_cache: None,
    };

    let matches =
        version_matches_target(&ctx, "demo", &"1.0.0".parse().unwrap())
            .await
            .unwrap();
    assert!(matches);
}
