use std::collections::HashMap;

use pep440_rs::Version;

use super::super::solve::SolveResult;
use super::super::solve_cache::{SolveCacheKey, SolveResultCache};
use crate::resolver::types::TargetEnv;

#[test]
fn test_solve_cache_key_hash_and_eq() {
    let target = TargetEnv::all_resolution_targets()[0].clone();
    let key1 = SolveCacheKey {
        package: "demo".to_string(),
        version: Version::new([1, 0, 0]),
        target: target.clone(),
        extras: vec!["feature".to_string()],
    };
    let key2 = SolveCacheKey {
        package: "demo".to_string(),
        version: Version::new([1, 0, 0]),
        target,
        extras: vec!["feature".to_string()],
    };
    assert_eq!(key1, key2);

    let cache = SolveResultCache::new();
    cache.insert(
        key1,
        SolveResult {
            solved_versions: HashMap::new(),
            active_extras: HashMap::new(),
        },
    );
    assert!(cache.contains_key(&key2));
}
