use crate::config::{Config, PackageSpec};
use crate::downloader::{DownloadableItem, ExplicitWheel};
use crate::http::HttpClient;
use crate::progress::ProgressHandle;
use crate::resolver::plan::{
    DependencyPlan, PlanParams, build_dependency_plan,
};
use crate::resolver::resolve::ResolveError;
use crate::sync::pipeline::SyncError;
use crate::sync::plan;

pub struct PlanPhase;

impl PlanPhase {
    pub async fn run(
        config: &Config,
        client: &HttpClient,
        pkgs: &[PackageSpec],
        no_deps: bool,
        progress: Option<ProgressHandle>,
    ) -> Result<DependencyPlan, SyncError> {
        let (mut name_pkgs, url_pkgs) =
            crate::sync::url_wheel::split_package_specs(pkgs);

        let url_prefetched =
            crate::sync::url_wheel_download::maybe_collect_url_wheel_deps(
                client,
                &url_pkgs,
                no_deps,
                &mut name_pkgs,
            )
            .await?;

        let mut plan =
            build_plan(config, client, &name_pkgs, no_deps, progress).await?;
        plan.prefetched_files.extend(url_prefetched);

        for spec in &url_pkgs {
            add_url_wheel_to_plan(&mut plan, spec)?;
        }

        crate::sync::url_wheel::dedupe_planned_files(&mut plan.planned_files);
        crate::sync::url_wheel::dedupe_solved_versions(
            &mut plan.solved_versions,
        );

        Ok(plan)
    }
}

async fn build_plan(
    config: &Config,
    client: &HttpClient,
    name_pkgs: &[String],
    no_deps: bool,
    progress: Option<ProgressHandle>,
) -> Result<DependencyPlan, ResolveError> {
    if no_deps {
        return plan::build_top_only_plan(config, client, name_pkgs).await;
    }
    let params = PlanParams {
        top_packages: name_pkgs,
        pypi_urls: &config.effective_mirrors(),
        top_versions_per_package: config.top_versions_per_package,
        adjacent_versions_per_side: config.adjacent_versions_per_side,
        allow_prerelease: config.allow_prerelease,
        include_source: config.include_source,
        linux_max_glibc: &config.linux_max_glibc,
        resolve_workers: config.resolve_workers,
        metadata_workers: config.metadata_workers,
        targets: crate::resolver::types::TargetEnv::from_specs(&config.targets),
    };
    build_dependency_plan(&params, client, progress).await
}

fn add_url_wheel_to_plan(
    plan: &mut DependencyPlan,
    spec: &crate::config::PackageUrlSpec,
) -> Result<(), ResolveError> {
    let parsed =
        crate::wheel_url::parse_wheel_url(&spec.url, spec.sha256.clone())
            .map_err(|e| {
                ResolveError::Config(format!(
                    "URL whl 解析失败 ({}): {e}",
                    crate::filters::redact_url_for_display(&spec.url)
                ))
            })?;
    let wheel = ExplicitWheel {
        filename: parsed.filename,
        url: parsed.url,
        sha256: parsed.sha256,
        package_name: parsed.package_name.clone(),
        version: parsed.version.clone(),
    };
    plan.planned_files.push(DownloadableItem::Explicit(wheel));
    let version =
        parsed.version.parse::<pep440_rs::Version>().map_err(|_| {
            ResolveError::Config(format!(
                "无法解析 whl 版本: {}",
                parsed.version
            ))
        })?;
    plan.solved_versions
        .entry(parsed.package_name)
        .or_default()
        .push(version);
    Ok(())
}
