use dashmap::DashMap;
use pep440_rs::Version;
use type_state_builder::TypeStateBuilder;

use crate::downloader::{FileInfo, PrefetchedFiles};
use crate::http::HttpClient;
use crate::resolver::metadata::MetadataCache;
use crate::resolver::plan::{DependencyPlan, select_top_versions};
use crate::resolver::pubgrub::bare_name;
use crate::resolver::resolve::ResolveError;
use crate::resolver::types::TargetEnv;

#[derive(TypeStateBuilder)]
#[builder(impl_into)]
struct FileSelector<'a> {
    #[builder(required)]
    cache: &'a MetadataCache,
    #[builder(required)]
    targets: &'a [TargetEnv],
    #[builder(required)]
    include_source: bool,
    #[builder(required)]
    linux_max_glibc: &'a str,
}

impl<'a> FileSelector<'a> {
    async fn select(
        &self,
        package: &str,
        version: &Version,
    ) -> Result<Vec<FileInfo>, ResolveError> {
        let files = self.cache.get_version_files(package, version).await?;
        Ok(crate::filters::select_files_for_version(
            &files,
            self.targets,
            self.include_source,
            self.linux_max_glibc,
        ))
    }
}

pub async fn build_top_only_plan(
    config: &crate::config::Config,
    client: &HttpClient,
    pkgs: &[String],
) -> Result<DependencyPlan, ResolveError> {
    let base_url = config
        .effective_mirrors()
        .into_iter()
        .next()
        .unwrap_or_else(|| "https://pypi.org".to_string());
    let cache =
        MetadataCache::new(client.clone(), base_url, config.metadata_workers);
    let targets =
        crate::resolver::types::TargetEnv::from_specs(&config.targets);
    let selector = FileSelector::builder()
        .cache(&cache)
        .targets(&*targets)
        .include_source(config.include_source)
        .linux_max_glibc(&*config.linux_max_glibc)
        .build();

    let mut planned_files = Vec::new();
    let solved_versions: DashMap<String, Vec<Version>> = DashMap::new();

    for pkg in pkgs {
        let package = bare_name(pkg);
        let selected_versions = select_top_versions(
            cache.get_all_versions(&package).await?,
            config.top_versions_per_package,
            config.allow_prerelease,
        );

        solved_versions.insert(package.clone(), selected_versions.clone());
        for version in selected_versions {
            let selected = selector.select(&package, &version).await?;
            planned_files.extend(selected);
        }
    }

    let mut seen = std::collections::HashSet::new();
    planned_files.retain(|fi| seen.insert(fi.filename.clone()));

    Ok(DependencyPlan {
        planned_files,
        prefetched_files: PrefetchedFiles::new(),
        solved_versions,
    })
}
