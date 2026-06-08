use std::path::Path;

use sha2::Digest;
use url::Url;

use crate::downloader::FileInfo;

async fn read_source_bytes(url: &str) -> Result<Vec<u8>, String> {
    let parsed =
        Url::parse(url).map_err(|e| format!("无效的 file URL: {e}"))?;
    let path = parsed
        .to_file_path()
        .map_err(|_| "无法将 file URL 转换为文件路径".to_string())?;
    tokio::fs::read(&path)
        .await
        .map_err(|e| format!("读取本地文件失败 {}: {e}", path.display()))
}

fn verify_hash(bytes: &[u8], expected: &str) -> bool {
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    let actual = format!("{:x}", h.finalize());
    actual.eq_ignore_ascii_case(expected)
}

pub async fn copy_local_wheel(
    url: &str,
    fi: &FileInfo,
    dest: &Path,
) -> (bool, String) {
    let bytes = match read_source_bytes(url).await {
        Ok(b) => b,
        Err(msg) => return (false, msg),
    };
    if fi
        .sha256
        .as_ref()
        .is_some_and(|exp| !verify_hash(&bytes, exp))
    {
        return (false, "hash 校验失败".into());
    }
    super::write_atomic(dest, &bytes).await
}
