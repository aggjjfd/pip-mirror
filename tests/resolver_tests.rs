use std::collections::HashSet;

use dashmap::DashMap;
use pep440_rs::Version;

use pip_mirror::resolver::pubgrub;

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
fn test_bare_name() {
    assert_eq!(pubgrub::bare_name("requests"), "requests");
    assert_eq!(pubgrub::bare_name("markitdown[pptx]"), "markitdown");
}

#[test]
fn test_parse_python_requires_simple() {
    let deps = pubgrub::parse_python_requires(&[
        "requests >=2.0".to_string(),
        "numpy >=1.20.0,<2.0".to_string(),
    ]);
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].0, "requests");
    assert_eq!(deps[0].1, ">=2.0");
}

#[test]
fn test_parse_python_requires_skips_extras() {
    let deps = pubgrub::parse_python_requires(&[
        "dep; extra == \"dev\"".to_string(),
        "requests".to_string(),
    ]);
    assert_eq!(deps.len(), 1); // "dep" skipped because "extra =="
}

#[test]
fn test_version_window_empty() {
    let solutions: Vec<pubgrub::Solution> = vec![];
    let av: DashMap<String, Vec<Version>> = DashMap::new();
    let result = pubgrub::compute_version_windows(&solutions, &av, 3);
    assert!(result.is_empty());
}

#[test]
fn test_version_window_basic() {
    let v200: Version = "2.0.0".parse().unwrap();
    let v150: Version = "1.5.0".parse().unwrap();
    let v100: Version = "1.0.0".parse().unwrap();
    let v050: Version = "0.5.0".parse().unwrap();

    let av: DashMap<String, Vec<Version>> = {
        let m = DashMap::new();
        m.insert(
            "dep".to_string(),
            vec![v200.clone(), v150.clone(), v100.clone(), v050.clone()],
        );
        m
    };

    let sol1: pubgrub::Solution = {
        let m = DashMap::new();
        m.insert("dep".to_string(), v200.clone());
        m
    };
    let sol2: pubgrub::Solution = {
        let m = DashMap::new();
        m.insert("dep".to_string(), v100.clone());
        m
    };

    let result = pubgrub::compute_version_windows(&[sol1, sol2], &av, 3);
    let versions = result.get("dep").unwrap();
    assert!(versions.contains(&v200));
    assert!(versions.contains(&v100));
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
fn test_version_window_overlap() {
    let v200: Version = "2.0.0".parse().unwrap();
    let v150: Version = "1.5.0".parse().unwrap();
    let v100: Version = "1.0.0".parse().unwrap();
    let v050: Version = "0.5.0".parse().unwrap();

    let v200c = v200.clone();
    let v150c = v150.clone();
    let v100c = v100.clone();
    let av: DashMap<String, Vec<Version>> = {
        let m = DashMap::new();
        m.insert("dep".to_string(), vec![v200c, v150c, v100c, v050]);
        m
    };

    let sol1: pubgrub::Solution = {
        let m = DashMap::new();
        m.insert("dep".to_string(), v150.clone());
        m
    };
    let sol2: pubgrub::Solution = {
        let m = DashMap::new();
        m.insert("dep".to_string(), v150.clone());
        m
    };

    let result = pubgrub::compute_version_windows(&[sol1, sol2], &av, 3);
    let versions = result.get("dep").unwrap();
    // both solutions at v150, window = [v200, v150, v100], no duplicates
    assert_eq!(versions.len(), 3);
    assert!(versions.contains(&v200));
    assert!(versions.contains(&v150));
    assert!(versions.contains(&v100));
}

#[test]
fn test_version_window_not_in_all() {
    let v999: Version = "9.9.9".parse().unwrap();
    let v200: Version = "2.0.0".parse().unwrap();
    let v100: Version = "1.0.0".parse().unwrap();

    let av: DashMap<String, Vec<Version>> = {
        let m = DashMap::new();
        m.insert("dep".to_string(), vec![v200, v100]);
        m
    };

    let sol: pubgrub::Solution = {
        let m = DashMap::new();
        m.insert("dep".to_string(), v999.clone());
        m
    };

    let result = pubgrub::compute_version_windows(&[sol], &av, 3);
    let versions = result.get("dep").unwrap();
    // version not in all_versions → kept as-is
    assert_eq!(versions.len(), 1);
    assert!(versions.contains(&v999));
}
