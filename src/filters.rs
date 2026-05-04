use std::collections::HashSet;
use std::sync::LazyLock;

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
    "aarch64",
    "arm64",
    "armv",
    "arm_",
    "armhf",
    "armel",
    "arm32",
    "musllinux",
    "macosx",
    "s390x",
    "ppc64le",
    "ppc64",
    "riscv64",
    "wasm32",
];

/// Map a wheel platform tag → set of target platform names.
pub fn platform_to_target(tag: &str) -> HashSet<&'static str> {
    if tag == "any" {
        return ["win32", "win_amd64", "linux_x86_64"].into();
    }
    let mut covered = HashSet::new();
    for sub in tag.split('.') {
        match sub {
            "win32" => {
                covered.insert("win32");
            }
            "win_amd64" => {
                covered.insert("win_amd64");
            }
            s if s.contains("x86_64") || s.contains("_x86_64") => {
                covered.insert("linux_x86_64");
            }
            _ => {}
        }
    }
    covered
}

fn parse_wheel_platform(filename: &str) -> Option<Vec<&str>> {
    let stem = &filename[..filename.len() - 4];
    let parts: Vec<&str> = stem.split('-').collect();
    (parts.len() >= 5).then(|| parts[parts.len() - 1].split('.').collect())
}

fn sub_tag_rejected(sub: &str) -> bool {
    REJECTED_SUBSTRINGS.iter().any(|r| sub.contains(r))
}

fn sub_tag_accepted(sub: &str) -> bool {
    ACCEPTED_PLATFORMS.contains(sub) || (sub.contains("manylinux") && sub.contains("x86_64"))
}

pub fn is_accepted_wheel(filename: &str) -> bool {
    let Some(sub_tags) = parse_wheel_platform(filename) else {
        return false;
    };
    if sub_tags.iter().any(|s| sub_tag_rejected(s)) {
        return false;
    }
    sub_tags.iter().any(|s| sub_tag_accepted(s))
}

pub fn is_pure_python_wheel(filename: &str) -> bool {
    let Some(sub_tags) = parse_wheel_platform(filename) else {
        return false;
    };
    sub_tags.contains(&"any")
}

pub fn is_source_distribution(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".zip")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.xz")
}

pub fn normalize_package_name(name: &str) -> String {
    name.to_lowercase().replace(['_', '.'], "-")
}
