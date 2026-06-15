use url::Url;

/// 去除 URL 中的 query、fragment 与用户信息，用于安全的日志/错误输出。
///
/// 若字符串无法解析为 URL，则返回占位符而不是回显原始输入，从而避免包含
/// 凭证的畸形 URL 在错误信息中泄露。
pub fn redact_url_for_display(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.set_username("").ok();
            parsed.set_password(None).ok();
            if parsed.host_str().is_none()
                && parsed.scheme() != "file"
                && url.contains('@')
            {
                return "<无法解析的 URL>".to_string();
            }
            parsed.to_string()
        }
        Err(_) => "<无法解析的 URL>".to_string(),
    }
}
