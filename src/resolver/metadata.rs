use crate::downloader::HttpCtx;

pub async fn get_all_versions(
    http: &HttpCtx<'_>,
    package: &str,
    allow_prerelease: bool,
) -> Result<Vec<pep440_rs::Version>, reqwest::Error> {
    let bare = package.split_once('[').map_or(package, |(n, _)| n);
    let normalized = crate::filters::normalize_package_name(bare);
    let url = format!(
        "{}/pypi/{}/json",
        http.pypi_url.trim_end_matches('/'),
        normalized
    );
    let resp: serde_json::Value =
        http.client.get(&url).send().await?.json().await?;
    let mut versions: Vec<pep440_rs::Version> = resp
        .get("releases")
        .and_then(|r| r.as_object())
        .map(|obj| {
            obj.keys()
                .filter_map(|v| v.parse::<pep440_rs::Version>().ok())
                .filter(|v| allow_prerelease || !v.any_prerelease())
                .collect()
        })
        .unwrap_or_default();
    versions.sort_by(|a, b| b.cmp(a));
    Ok(versions)
}

pub async fn get_requires_dist(
    http: &HttpCtx<'_>,
    package: &str,
    version: &str,
) -> Result<Option<Vec<String>>, reqwest::Error> {
    let bare = package.split_once('[').map_or(package, |(n, _)| n);
    let normalized = crate::filters::normalize_package_name(bare);
    let url = format!(
        "{}/pypi/{}/{}/json",
        http.pypi_url.trim_end_matches('/'),
        normalized,
        version
    );
    let resp: serde_json::Value =
        http.client.get(&url).send().await?.json().await?;
    Ok(resp
        .get("info")
        .and_then(|i| i.get("requires_dist"))
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        }))
}
