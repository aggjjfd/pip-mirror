use std::collections::HashSet;

use pep440_rs::Version;
use pip_mirror::resolver::pubgrub;
use pip_mirror::resolver::types::TargetEnv;

fn no_extras() -> HashSet<String> {
    HashSet::new()
}

fn linux_target() -> TargetEnv {
    TargetEnv::test_env("linux", "x86_64", "3.12")
}

// ── pubgrub helpers ──

#[test]
fn test_extract_extras_no_brackets() {
    let (name, extras) = pubgrub::extract_extras("requests");
    assert_eq!(name, "requests");
    assert!(extras.is_empty());
}

#[test]
fn test_extract_extras_single() {
    let (name, extras) = pubgrub::extract_extras("markitdown[pptx]");
    assert_eq!(name, "markitdown");
    assert_eq!(extras, HashSet::from(["pptx".to_string()]));
}

#[test]
fn test_extract_extras_multi() {
    let (name, extras) = pubgrub::extract_extras("markitdown[pptx,docx,xls]");
    assert_eq!(name, "markitdown");
    assert_eq!(
        extras,
        HashSet::from([
            "pptx".to_string(),
            "docx".to_string(),
            "xls".to_string()
        ])
    );
}

#[test]
fn test_extract_extras_strips_whitespace() {
    let (name, extras) = pubgrub::extract_extras("pkg[a, b , c]");
    assert_eq!(name, "pkg");
    assert_eq!(
        extras,
        HashSet::from(["a".to_string(), "b".to_string(), "c".to_string()])
    );
}

#[test]
fn test_bare_name() {
    assert_eq!(pubgrub::bare_name("requests"), "requests");
    assert_eq!(pubgrub::bare_name("markitdown[pptx]"), "markitdown");
}

#[test]
fn test_compatible_range_single_segment_has_upper_bound() {
    let v1: Version = "1.0.0".parse().unwrap();
    let v2: Version = "2.0.0".parse().unwrap();
    let range = pubgrub::compatible_range(v1.clone());
    assert!(range.contains(&v1));
    assert!(!range.contains(&v2));
}

#[test]
fn test_compatible_range_two_segment() {
    let v15: Version = "1.5".parse().unwrap();
    let v16: Version = "1.6.0".parse().unwrap();
    let v20: Version = "2.0.0".parse().unwrap();
    let v14: Version = "1.4.0".parse().unwrap();
    let range = pubgrub::compatible_range(v15.clone());
    assert!(range.contains(&v15));
    assert!(range.contains(&v16));
    assert!(!range.contains(&v14));
    assert!(!range.contains(&v20));
}

#[test]
fn test_compatible_range_three_segment() {
    let v123: Version = "1.2.3".parse().unwrap();
    let v130: Version = "1.3.0".parse().unwrap();
    let v124: Version = "1.2.4".parse().unwrap();
    let v121: Version = "1.2.1".parse().unwrap();
    let v200: Version = "2.0.0".parse().unwrap();
    let range = pubgrub::compatible_range(v123.clone());
    assert!(range.contains(&v123));
    assert!(range.contains(&v124));
    assert!(!range.contains(&v121));
    assert!(!range.contains(&v130));
    assert!(!range.contains(&v200));
}

// ── markers ──

#[test]
fn test_parse_dependency_line_simple() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep =
        parse_dependency_line("requests>=2.0", &no_extras(), &linux_target())
            .unwrap()
            .unwrap();
    assert_eq!(dep.package_name, "requests");
    assert_eq!(dep.version_spec, ">=2.0");
}

#[test]
fn test_parse_dependency_line_no_marker() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line("numpy", &no_extras(), &linux_target())
        .unwrap()
        .unwrap();
    assert_eq!(dep.package_name, "numpy");
    assert!(dep.version_spec.is_empty());
}

#[test]
fn test_parse_dependency_line_platform_linux_kept() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line(
        "inotify>=0.2; sys_platform == 'linux'",
        &no_extras(),
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_some());
    assert_eq!(dep.unwrap().package_name, "inotify");
}

#[test]
fn test_parse_dependency_line_platform_win32_skipped_on_linux() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line(
        "pypiwin32; sys_platform == 'win32'",
        &no_extras(),
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_none());
}

#[test]
fn test_parse_dependency_line_machine_x86_64_kept() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line(
        "cryptography; platform_machine == 'x86_64'",
        &no_extras(),
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_some());
}

#[test]
fn test_parse_dependency_line_machine_arm64_skipped() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line(
        "pyobjc; platform_machine == 'arm64'",
        &no_extras(),
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_none());
}

#[test]
fn test_parse_dependency_line_python_version_match() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line(
        "numpy>=1.26; python_version >= '3.12'",
        &no_extras(),
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_some());
    assert_eq!(dep.unwrap().package_name, "numpy");
}

#[test]
fn test_parse_dependency_line_normalizes_legacy_wildcard_specifier() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep =
        parse_dependency_line("click (>=7.*)", &no_extras(), &linux_target())
            .unwrap()
            .unwrap();
    assert_eq!(dep.package_name, "click");
    assert_eq!(dep.version_spec, ">=7");
}

#[test]
fn test_parse_dependency_line_python_version_no_match() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let dep = parse_dependency_line(
        "numpy>=1.24; python_version < '3.12'",
        &no_extras(),
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_none());
}

#[test]
fn test_parse_dependency_line_extra_match() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let extras = HashSet::from(["pptx".to_string()]);
    let dep = parse_dependency_line(
        "python-pptx>=0.6.21; extra == 'pptx'",
        &extras,
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_some());
    assert_eq!(dep.unwrap().package_name, "python-pptx");
}

#[test]
fn test_parse_dependency_line_extra_no_match() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let extras = HashSet::from(["pdf".to_string()]);
    let dep = parse_dependency_line(
        "python-pptx>=0.6.21; extra == 'pptx'",
        &extras,
        &linux_target(),
    )
    .unwrap();
    assert!(dep.is_none());
}

#[test]
fn test_parse_dependency_line_direct_url_rejected() {
    use pip_mirror::resolver::markers::parse_dependency_line;
    let result = parse_dependency_line(
        "requests @ https://example.com/requests.whl",
        &no_extras(),
        &linux_target(),
    );
    assert!(result.is_err());
}

#[test]
fn test_parse_requires_dist_multiple_lines() {
    use pip_mirror::resolver::markers::parse_requires_dist;
    let lines = vec![
        "requests>=2.0".to_string(),
        "numpy>=1.20.0,<2.0".to_string(),
        "pywin32; sys_platform == 'win32'".to_string(),
    ];
    let deps =
        parse_requires_dist(&lines, &no_extras(), &linux_target()).unwrap();
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].package_name, "requests");
    assert_eq!(deps[1].package_name, "numpy");
}

#[test]
fn test_parse_requires_dist_direct_url_is_error() {
    use pip_mirror::resolver::markers::parse_requires_dist;

    let lines = vec![
        "requests>=2.0".to_string(),
        "demo @ https://example.com/demo.whl".to_string(),
    ];
    let result = parse_requires_dist(&lines, &no_extras(), &linux_target());
    assert!(result.is_err());
}

// ── types / target env ──

#[test]
fn test_target_env_display() {
    let t = linux_target();
    assert_eq!(t.to_string(), "py3.12/linux/x86_64");
}

#[test]
fn test_target_to_marker_env_ok() {
    let t = linux_target();
    assert!(t.to_marker_env().is_ok());
}

// ── spec_to_range ──

#[test]
fn test_spec_to_range_empty_is_full() {
    let range = pubgrub::spec_to_range("");
    let v: Version = "1.0.0".parse().unwrap();
    assert!(range.contains(&v));
}

#[test]
fn test_spec_to_range_gte() {
    let range = pubgrub::spec_to_range(">=1.0.0");
    let v1: Version = "1.0.0".parse().unwrap();
    let v2: Version = "0.9.0".parse().unwrap();
    assert!(range.contains(&v1));
    assert!(!range.contains(&v2));
}

#[test]
fn test_spec_to_range_comma() {
    let range = pubgrub::spec_to_range(">=1.0.0,<2.0.0");
    let v1: Version = "1.5.0".parse().unwrap();
    let v2: Version = "2.0.0".parse().unwrap();
    let v0: Version = "0.5.0".parse().unwrap();
    assert!(range.contains(&v1));
    assert!(!range.contains(&v2));
    assert!(!range.contains(&v0));
}
