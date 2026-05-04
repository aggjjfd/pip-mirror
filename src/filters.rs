use std::collections::HashSet;
use std::sync::LazyLock;

// ── Accepted platforms ──

static ACCEPTED_PLATFORMS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "win32",
        "win_amd64",
        "manylinux1_x86_64",
        "manylinux2010_x86_64",
        "manylinux2014_x86_64",
        "manylinux_2_5_x86_64",
        "manylinux_2_12_x86_64",
        "manylinux_2_17_x86_64",
        "manylinux_2_24_x86_64",
        "manylinux_2_28_x86_64",
        "manylinux_2_31_x86_64",
        "manylinux_2_34_x86_64",
        "manylinux_2_35_x86_64",
        "manylinux_2_39_x86_64",
        "linux_x86_64",
        "any",
    ])
});

static REJECTED_SUBSTRINGS: &[&str] = &[
    "aarch64", "arm64", "armv", "arm_", "armhf", "armel", "arm32",
    "musllinux",
    "macosx",
    "s390x", "ppc64le", "ppc64", "riscv64", "wasm32",
];

/// Map a wheel platform tag → set of target platform names.
pub fn platform_to_target(tag: &str) -> HashSet<&'static str> {
    if tag == "any" {
        return ["win32", "win_amd64", "linux_x86_64"].into();
    }
    let mut covered = HashSet::new();
    for sub in tag.split('.') {
        match sub {
            "win32" => { covered.insert("win32"); }
            "win_amd64" => { covered.insert("win_amd64"); }
            s if s.contains("x86_64") || s.contains("_x86_64") => { covered.insert("linux_x86_64"); }
            _ => {}
        }
    }
    covered
}

pub fn is_accepted_wheel(filename: &str) -> bool {
    if !filename.ends_with(".whl") {
        return false;
    }
    let stem = &filename[..filename.len() - 4];
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return false;
    }

    let platform_tag = parts[parts.len() - 1];
    let sub_tags: Vec<&str> = platform_tag.split('.').collect();

    // reject if any sub-tag matches a rejected pattern
    for sub in &sub_tags {
        for rejected in REJECTED_SUBSTRINGS {
            if sub.contains(rejected) {
                return false;
            }
        }
    }

    // accept if any sub-tag is in the whitelist OR is a manylinux+x86_64 fallback
    for sub in &sub_tags {
        if ACCEPTED_PLATFORMS.contains(sub) {
            return true;
        }
        if sub.contains("manylinux") && sub.contains("x86_64") {
            return true;
        }
    }

    false
}

pub fn is_pure_python_wheel(filename: &str) -> bool {
    if !filename.ends_with(".whl") {
        return false;
    }
    let stem = &filename[..filename.len() - 4];
    let parts: Vec<&str> = stem.split('-').collect();
    parts.len() >= 5 && parts[parts.len() - 1] == "any"
}

pub fn is_source_distribution(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".zip")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.xz")
}

pub fn normalize_package_name(name: &str) -> String {
    name.to_lowercase().replace('_', "-").replace('.', "-")
}
