use std::collections::HashSet;

use pep508_rs::{MarkerTree, VerbatimUrl};

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

struct RequirementParts {
    package_name: String,
    extras: HashSet<String>,
    version_spec: String,
    marker: MarkerTree,
}

fn parse_requirement_parts(
    line: &str,
) -> Result<RequirementParts, MarkerError> {
    let (requirement, marker) = split_requirement_and_marker(line);
    let requirement = requirement.trim();
    if requirement.is_empty() {
        return Err(MarkerError::ParseError(line.to_string()));
    }
    if requirement.contains('@') {
        return Err(MarkerError::UnsupportedDirectUrl(line.to_string()));
    }

    let (name, remainder) = split_name_and_rest(requirement);
    if name.is_empty() {
        return Err(MarkerError::ParseError(line.to_string()));
    }
    let (extras, version_spec) = parse_extras_and_spec(remainder, line)?;

    Ok(RequirementParts {
        package_name: normalize_name(name),
        extras,
        version_spec: normalize_version_spec(version_spec)?,
        marker: parse_marker(marker)?,
    })
}

fn split_requirement_and_marker(line: &str) -> (&str, Option<&str>) {
    match line.split_once(';') {
        Some((requirement, marker)) => (requirement, Some(marker)),
        None => (line, None),
    }
}

fn split_name_and_rest(requirement: &str) -> (&str, &str) {
    let name_end = requirement
        .char_indices()
        .find(|(_, ch)| is_name_delimiter(*ch))
        .map_or(requirement.len(), |(index, _)| index);
    (&requirement[..name_end], &requirement[name_end..])
}

fn is_name_delimiter(ch: char) -> bool {
    ch.is_ascii_whitespace()
        || matches!(ch, '[' | '(' | '<' | '>' | '=' | '!' | '~' | '@')
}

fn parse_extras_and_spec<'a>(
    remainder: &'a str,
    line: &str,
) -> Result<(HashSet<String>, &'a str), MarkerError> {
    let remainder = remainder.trim_start();
    if !remainder.starts_with('[') {
        return Ok((HashSet::new(), remainder));
    }
    let Some(end) = remainder.find(']') else {
        return Err(MarkerError::ParseError(line.to_string()));
    };
    let extras = remainder[1..end]
        .split(',')
        .map(str::trim)
        .filter(|extra| !extra.is_empty())
        .map(str::to_string)
        .collect();
    Ok((extras, remainder[end + 1..].trim_start()))
}

fn normalize_version_spec(spec: &str) -> Result<String, MarkerError> {
    let spec = strip_wrapping_parens(spec)?;
    let spec = super::normalize_legacy_wildcards(spec.trim());
    Ok(spec
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", "))
}

fn strip_wrapping_parens(spec: &str) -> Result<&str, MarkerError> {
    let trimmed = spec.trim();
    if !trimmed.starts_with('(') {
        return Ok(trimmed);
    }
    let Some(stripped) =
        trimmed.strip_prefix('(').and_then(|s| s.strip_suffix(')'))
    else {
        return Err(MarkerError::ParseError(trimmed.to_string()));
    };
    Ok(stripped.trim())
}

fn parse_marker(marker: Option<&str>) -> Result<MarkerTree, MarkerError> {
    let Some(marker) =
        marker.map(str::trim).filter(|marker| !marker.is_empty())
    else {
        return Ok(MarkerTree::TRUE);
    };
    MarkerTree::parse_str::<VerbatimUrl>(marker)
        .map_err(|error| MarkerError::ParseError(error.to_string()))
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
    let requirement = parse_requirement_parts(line)?;

    // No marker: always applies.
    if requirement.marker.is_true() {
        return Ok(Some(ParsedDependency {
            package_name: requirement.package_name,
            extras: requirement.extras,
            version_spec: requirement.version_spec,
        }));
    }

    if !evaluate_marker(&requirement.marker, target_env, active_extras)? {
        return Ok(None);
    }

    Ok(Some(ParsedDependency {
        package_name: requirement.package_name,
        extras: requirement.extras,
        version_spec: requirement.version_spec,
    }))
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
