use std::collections::HashSet;
use std::process::Command;

use pep440_rs::Version;
use pip_mirror::resolver::eligibility::{SolveContext, version_matches_target};
use pip_mirror::resolver::metadata::MetadataCache;
use pip_mirror::resolver::plan::{PlanParams, build_dependency_plan};
use pip_mirror::resolver::pubgrub::{bare_name, collect_pkg_extras};
use pip_mirror::resolver::solve::solve_one_target;
use pip_mirror::resolver::types::TargetEnv;

const PYPI_URL: &str = "https://pypi.org";
const LINUX_MAX_GLIBC: &str = "2.39";
const TEST_RESOLVE_WORKERS: usize = 4;
const TEST_METADATA_WORKERS: usize = 8;

fn py312_linux_target() -> TargetEnv {
    TargetEnv::test_env("linux", "x86_64", "3.12")
}

fn uv_platform(target: &TargetEnv) -> &'static str {
    match (target.sys_platform(), target.platform_machine()) {
        ("linux", "x86_64") => "x86_64-manylinux_2_39",
        ("win32", "x86") => "i686-pc-windows-msvc",
        ("win32", "AMD64") => "x86_64-pc-windows-msvc",
        other => panic!("unsupported uv target mapping: {other:?}"),
    }
}

fn uv_resolve_exact(requirement: &str, target: &TargetEnv) -> HashSet<String> {
    let target_dir = std::env::temp_dir()
        .join(format!("pip-mirror-uv-dry-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target_dir);
    std::fs::create_dir_all(&target_dir).unwrap();
    let output = Command::new("uv")
        .args([
            "pip",
            "install",
            "--dry-run",
            "--target",
            target_dir.to_str().unwrap(),
            "--only-binary",
            ":all:",
            "--prerelease",
            "disallow",
            "--python-version",
            target.python_version(),
            "--python-platform",
            uv_platform(target),
            requirement,
        ])
        .output()
        .expect("uv must be installed");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("uv resolve failed for {requirement}:\n{stderr}");
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix('+').map(str::trim))
        .filter_map(|line| {
            line.split_once("==").map(|(name, _)| bare_name(name))
        })
        .collect()
}

async fn solve_exact_target(
    package_ref: &str,
    target: &TargetEnv,
) -> (Version, HashSet<String>) {
    let client = reqwest::Client::new();
    let cache =
        MetadataCache::new(client, PYPI_URL.to_string(), TEST_METADATA_WORKERS);
    let package = bare_name(package_ref);
    let extras = collect_pkg_extras(&[package_ref.to_string()])
        .remove(&package)
        .unwrap_or_default();
    let ctx = SolveContext {
        cache: &cache,
        target,
        allow_prerelease: false,
        include_source: false,
        linux_max_glibc: LINUX_MAX_GLIBC,
        metadata_workers: TEST_METADATA_WORKERS,
        parsed_deps_cache: None,
    };
    let root_version =
        select_first_installable_version(&cache, &ctx, &package).await;
    let result = solve_one_target(&ctx, &package, &root_version, &extras)
        .await
        .expect("solver should succeed");

    let packages = result
        .solved_versions
        .keys()
        .filter(|name| *name != &package)
        .cloned()
        .collect();
    (root_version, packages)
}

async fn select_first_installable_version(
    cache: &MetadataCache,
    ctx: &SolveContext<'_>,
    package: &str,
) -> Version {
    for version in cache.get_all_versions(package).await.unwrap() {
        if version.any_prerelease() {
            continue;
        }
        if version_matches_target(ctx, package, &version)
            .await
            .unwrap()
        {
            return version;
        }
    }
    panic!("no stable installable version found for {package}");
}

#[tokio::test]
#[ignore = "e2e network test: runs ~2.5 min against PyPI"]
async fn test_solve_one_target_matches_uv_for_requests_linux_py312() {
    pip_mirror::logging::init(false);
    let target = py312_linux_target();
    let (root_version, our_packages) =
        solve_exact_target("requests", &target).await;
    let requirement = format!("requests=={root_version}");
    let uv_packages = uv_resolve_exact(&requirement, &target);
    let uv_deps: HashSet<_> = uv_packages
        .into_iter()
        .filter(|name| name != "requests")
        .collect();

    assert_eq!(our_packages, uv_deps);
}

#[tokio::test]
#[ignore = "e2e network test: runs ~2.5 min against PyPI"]
async fn test_build_dependency_plan_e2e_smoke() {
    pip_mirror::logging::init(false);
    let client = reqwest::Client::new();
    let e2e_packages = [
        "openai",
        "gradio",
        "markitdown[pptx,docx,xls,xlsx,pdf]",
        "rapidocr-onnxruntime",
        "pyside6",
        "playwright",
    ];

    for package in e2e_packages {
        let params = PlanParams {
            top_packages: &[package.to_string()],
            pypi_url: PYPI_URL,
            top_versions_per_package: 1,
            adjacent_versions_per_side: 0,
            allow_prerelease: false,
            include_source: false,
            linux_max_glibc: LINUX_MAX_GLIBC,
            resolve_workers: TEST_RESOLVE_WORKERS,
            metadata_workers: TEST_METADATA_WORKERS,
            targets:
                pip_mirror::resolver::types::TargetEnv::all_resolution_targets(),
        };
        let plan = build_dependency_plan(&params, &client, None)
            .await
            .expect("build plan should succeed");
        assert!(
            !plan.planned_files.is_empty(),
            "planned files should not be empty for {package}"
        );
    }
}
