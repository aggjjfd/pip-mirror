use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::info;

use crate::downloader::FileInfo;

pub struct IncrementalPackage<'a> {
    pub simple_files: &'a [FileInfo],
    pub python_builds_files: &'a [PathBuf],
    pub python_builds_index: Option<&'a Path>,
    pub repository_dir: &'a Path,
    pub output_dir: &'a Path,
}

/// Create an incremental tar.gz package containing only new/changed files.
pub fn create_incremental_package(spec: &IncrementalPackage<'_>) -> Option<PathBuf> {
    let store_db = spec.repository_dir.join(".store.db");
    let files_to_pack = spec.simple_files.len() + spec.python_builds_files.len();

    if files_to_pack == 0 && spec.python_builds_index.is_none() {
        info!("no changes: 本次没有新文件下载, 未产生增量包");
        return None;
    }

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let archive_name = format!("incremental_{timestamp}.tar.gz");
    let archive_path = spec.output_dir.join(&archive_name);

    std::fs::create_dir_all(spec.output_dir).ok();

    let file = std::fs::File::create(&archive_path).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);

    for fi in spec.simple_files {
        let src = spec
            .repository_dir
            .join("simple")
            .join(&fi.package_name)
            .join(&fi.filename);
        if src.exists() {
            let member_name = format!("simple/{}/{}", fi.package_name, fi.filename);
            tar.append_path_with_name(&src, &member_name).ok();
        }
    }

    for pb in spec.python_builds_files {
        if pb.exists() {
            let member_name = format!(
                "python-builds/{}",
                pb.file_name().unwrap().to_string_lossy()
            );
            tar.append_path_with_name(pb, &member_name).ok();
        }
    }

    if spec.python_builds_index.is_some_and(|p| p.exists()) {
        tar.append_path_with_name(
            spec.python_builds_index.unwrap(),
            "python-builds/index.json",
        )
        .ok();
    }

    // Add .store.db
    if store_db.exists() {
        tar.append_path_with_name(&store_db, ".store.db").ok();
    }

    drop(tar);

    info!("增量包已创建: {}", archive_path.display());
    Some(archive_path)
}

/// Write sha256 checksum file in sha256sum format.
pub fn write_sha256(archive: &Path) -> std::io::Result<PathBuf> {
    let mut file = std::fs::File::open(archive)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let digest = format!("{:x}", hasher.finalize());
    let sha_path = archive.with_extension("sha256");
    let name = archive.file_name().unwrap().to_string_lossy();
    std::fs::write(&sha_path, format!("{digest}  {name}\n"))?;
    Ok(sha_path)
}
