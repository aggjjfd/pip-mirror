use std::collections::HashSet;
use std::str::FromStr;

use pep440_rs::VersionSpecifier;
use pep508_rs::{Requirement, VerbatimUrl};

use super::types::TargetEnv;

/// A dependency parsed from a `requires_dist` line, after marker evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDependency {
    pub package_name: String,
    pub extras: HashSet<String>,
    pub version_spec: String,
}

#[derive(Debug, Clone)]
pub enum MarkerError {
    UnsupportedDirectUrl(String),
    UnsupportedMarkerKey(String),
    ParseError(String),
    MarkerEnvError(String),
}

impl std::fmt::Display for MarkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarkerError::UnsupportedDirectUrl(s) => {
                write!(f, "direct URL/path requirement not supported: {}", s)
            }
            MarkerError::UnsupportedMarkerKey(s) => {
                write!(f, "unsupported marker key in requirement: {}", s)
            }
            MarkerError::ParseError(s) => {
                write!(f, "failed to parse requirement: {}", s)
            }
            MarkerError::MarkerEnvError(s) => {
                write!(f, "failed to build marker environment: {}", s)
            }
        }
    }
}

impl std::error::Error for MarkerError {}

/// Check whether a marker expression uses any unsupported keys.
fn marker_uses_unsupported_keys(marker: &pep508_rs::MarkerTree) -> bool {
    if let Some(s) = marker.try_to_string() {
        let lower = s.to_lowercase();
        lower.contains("platform_release") || lower.contains("platform_version")
    } else {
        false
    }
}

/// Convert a pep508_rs version specifier to our string format.
fn stringify_version_spec(spec: &VersionSpecifier) -> String {
    spec.to_string()
}

/// Extract version spec string from a requirement.
fn extract_version_spec<T: pep508_rs::Pep508Url>(
    req: &Requirement<T>,
) -> String {
    match &req.version_or_url {
        Some(pep508_rs::VersionOrUrl::VersionSpecifier(vs)) => vs
            .iter()
            .map(stringify_version_spec)
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

fn build_parsed_dependency<T: pep508_rs::Pep508Url>(
    req: Requirement<T>,
) -> ParsedDependency {
    let version_spec = extract_version_spec(&req);
    ParsedDependency {
        package_name: normalize_name(req.name.as_ref()),
        extras: req.extras.into_iter().map(|e| e.to_string()).collect(),
        version_spec,
    }
}

fn evaluate_marker(
    marker: &pep508_rs::MarkerTree,
    target_env: &TargetEnv,
    active_extras: &HashSet<String>,
) -> Result<bool, MarkerError> {
    if marker_uses_unsupported_keys(marker) {
        return Err(MarkerError::UnsupportedMarkerKey(
            marker.try_to_string().unwrap_or_default(),
        ));
    }
    let marker_env = target_env
        .to_marker_env()
        .map_err(|e| MarkerError::MarkerEnvError(e.to_string()))?;
    let extra_set: Vec<pep508_rs::ExtraName> = active_extras
        .iter()
        .filter_map(|e| e.parse().ok())
        .collect();
    Ok(marker.evaluate(&marker_env, &extra_set))
}

/// Parse a single `requires_dist` line and evaluate it against the target
/// environment and active extras.
///
/// Returns `Ok(None)` if the marker evaluates to false for this target.
/// Returns `Ok(Some(ParsedDependency))` if the dependency applies.
/// Returns `Err` for unsupported constructs (direct URL, unsupported marker
/// keys).
pub fn parse_dependency_line(
    line: &str,
    active_extras: &HashSet<String>,
    target_env: &TargetEnv,
) -> Result<Option<ParsedDependency>, MarkerError> {
    let req = Requirement::<VerbatimUrl>::from_str(line)
        .map_err(|e| MarkerError::ParseError(e.to_string()))?;

    // Reject direct URL/path requirements.
    if let Some(pep508_rs::VersionOrUrl::Url(_)) = &req.version_or_url {
        return Err(MarkerError::UnsupportedDirectUrl(line.to_string()));
    }

    // No marker: always applies.
    if req.marker.is_true() {
        return Ok(Some(build_parsed_dependency(req)));
    }

    if !evaluate_marker(&req.marker, target_env, active_extras)? {
        return Ok(None);
    }

    Ok(Some(build_parsed_dependency(req)))
}

/// Parse a full `requires_dist` list against the target environment and active
/// extras, returning only dependencies that apply.
pub fn parse_requires_dist(
    lines: &[String],
    active_extras: &HashSet<String>,
    target_env: &TargetEnv,
) -> Result<Vec<ParsedDependency>, MarkerError> {
    let mut deps = Vec::new();
    for line in lines {
        if let Some(dep) =
            parse_dependency_line(line, active_extras, target_env)?
        {
            deps.push(dep);
        }
    }
    Ok(deps)
}

/// Normalize a package name to lowercase with hyphens (PEP 503 style).
fn normalize_name(name: &str) -> String {
    let bare = name.split_once('[').map_or(name, |(n, _)| n);
    crate::filters::normalize_package_name(bare)
}
