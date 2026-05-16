pub(crate) mod build_requires;
pub(crate) mod discovery;
pub mod eligibility;
pub mod error;
pub mod markers;
pub mod metadata;
pub mod metadata_types;
pub mod plan;
pub mod pubgrub;
pub mod resolve;
pub mod solve;
pub mod types;

// PyPI 上仍有少量历史元数据写成 `>=7.*` 这类非法通配比较。
// 在保留原文的前提下，把比较段规范化成 pep440/pep508 可接受的形式。
pub(crate) fn normalize_legacy_wildcards(requirement: &str) -> String {
    let mut normalized = String::with_capacity(requirement.len());
    let mut index = 0;

    while index < requirement.len() {
        if let Some((operator, width)) =
            invalid_wildcard_operator(&requirement[index..])
        {
            normalized.push_str(operator);
            index += width;

            let space_end = skip_ascii_whitespace(requirement, index);
            normalized.push_str(&requirement[index..space_end]);
            index = space_end;

            let token_end = find_version_token_end(requirement, index);
            let token = &requirement[index..token_end];
            normalized.push_str(token.strip_suffix(".*").unwrap_or(token));
            index = token_end;
            continue;
        }

        let ch = requirement[index..]
            .chars()
            .next()
            .expect("index must point at a valid character boundary");
        normalized.push(ch);
        index += ch.len_utf8();
    }

    normalized
}

fn invalid_wildcard_operator(input: &str) -> Option<(&'static str, usize)> {
    [(">=", 2), ("<=", 2), ("~=", 2), (">", 1), ("<", 1)]
        .into_iter()
        .find(|(operator, _)| {
            input
                .strip_prefix(operator)
                .is_some_and(|rest| rest.contains(".*"))
        })
}

fn skip_ascii_whitespace(input: &str, start: usize) -> usize {
    start
        + input[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_whitespace())
            .map(char::len_utf8)
            .sum::<usize>()
}

fn find_version_token_end(input: &str, start: usize) -> usize {
    input[start..]
        .char_indices()
        .find(|(_, ch)| ch.is_ascii_whitespace() || *ch == ',' || *ch == ')')
        .map_or(input.len(), |(offset, _)| start + offset)
}
