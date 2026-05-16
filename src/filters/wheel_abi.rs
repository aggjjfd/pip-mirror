use crate::resolver::types::TargetEnv;

pub fn parse_wheel_tags(
    filename: &str,
) -> Option<(Vec<&str>, Vec<&str>, Vec<&str>)> {
    if !filename.ends_with(".whl") {
        return None;
    }
    let stem = &filename[..filename.len() - 4];
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return None;
    }
    let py_tags = parts[parts.len() - 3].split('.').collect();
    let abi_tags = parts[parts.len() - 2].split('.').collect();
    let platform_tags = parts[parts.len() - 1].split('.').collect();
    Some((py_tags, abi_tags, platform_tags))
}

pub fn parse_wheel_platform(filename: &str) -> Option<Vec<&str>> {
    parse_wheel_tags(filename).map(|(_, _, platform_tags)| platform_tags)
}

fn parse_minor_from_tag(tag: &str, prefix: &str) -> Option<u32> {
    let raw = tag.strip_prefix(prefix)?;
    let digits: String =
        raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 2 {
        return None;
    }
    let major = digits[0..1].parse::<u32>().ok()?;
    let minor = digits[1..].parse::<u32>().ok()?;
    (major == 3).then_some(minor)
}

fn target_python_minor(target: &TargetEnv) -> Option<u32> {
    let mut parts = target.python_version().split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    (major == 3).then_some(minor)
}

fn py_tag_minor(py_tag: &str) -> Option<u32> {
    parse_minor_from_tag(py_tag, "cp")
        .or_else(|| parse_minor_from_tag(py_tag, "py"))
}

fn py_tag_matches_target(py_tag: &str, target_minor: u32) -> bool {
    if py_tag == "py3" {
        return true;
    }
    py_tag_minor(py_tag).is_some_and(|minor| minor == target_minor)
}

fn abi3_pair_matches(py_tag: &str, target_minor: u32) -> bool {
    if py_tag == "py3" {
        return true;
    }
    py_tag_minor(py_tag).is_some_and(|minor| target_minor >= minor)
}

fn tag_pair_matches_target(
    py_tag: &str,
    abi_tag: &str,
    target_minor: u32,
) -> bool {
    if abi_tag.ends_with('t') {
        return false;
    }
    if abi_tag == "abi3" {
        return abi3_pair_matches(py_tag, target_minor);
    }
    if abi_tag == "none" {
        return py_tag_matches_target(py_tag, target_minor);
    }
    parse_minor_from_tag(abi_tag, "cp").is_some_and(|minor| {
        minor == target_minor && py_tag_matches_target(py_tag, target_minor)
    })
}

pub fn python_abi_matches_target(filename: &str, target: &TargetEnv) -> bool {
    let Some((py_tags, abi_tags, _)) = parse_wheel_tags(filename) else {
        return false;
    };
    let Some(target_minor) = target_python_minor(target) else {
        return false;
    };
    py_tags.iter().any(|py_tag| {
        abi_tags.iter().any(|abi_tag| {
            tag_pair_matches_target(py_tag, abi_tag, target_minor)
        })
    })
}
