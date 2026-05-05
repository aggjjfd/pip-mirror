use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::info;

use flate2::Compression;

use crate::downloader::FileInfo;

pub struct IncrementalPackage<'a> {
    pub simple_files: &'a [FileInfo],
    pub python_builds_files: &'a [PathBuf],
    pub python_builds_index: Option<&'a Path>,
    pub repository_dir: &'a Path,
    pub output_dir: &'a Path,
}

fn add_simple_files(
    tar: &mut tar::Builder<impl std::io::Write>,
    spec: &IncrementalPackage<'_>,
) {
    for fi in spec.simple_files {
        let src = spec
            .repository_dir
            .join("simple")
            .join(&fi.package_name)
            .join(&fi.filename);
        if src.exists() {
            tar.append_path_with_name(
                &src,
                format!("simple/{}/{}", fi.package_name, fi.filename),
            )
            .ok();
        }
    }
}

fn add_python_builds(
    tar: &mut tar::Builder<impl std::io::Write>,
    spec: &IncrementalPackage<'_>,
) {
    for pb in spec.python_builds_files {
        if pb.exists() {
            tar.append_path_with_name(
                pb,
                format!(
                    "python-builds/{}",
                    pb.file_name().unwrap().to_string_lossy()
                ),
            )
            .ok();
        }
    }
    if spec.python_builds_index.is_some_and(|p| p.exists()) {
        tar.append_path_with_name(
            spec.python_builds_index.unwrap(),
            "python-builds/index.json",
        )
        .ok();
    }
}

pub fn no_changes(spec: &IncrementalPackage<'_>) -> bool {
    spec.simple_files.is_empty()
        && spec.python_builds_files.is_empty()
        && spec.python_builds_index.is_none()
}

pub fn create_incremental_package(
    spec: &IncrementalPackage<'_>,
) -> Option<PathBuf> {
    if no_changes(spec) {
        info!("no changes: 本次没有新文件下载, 未产生增量包");
        return None;
    }
    std::fs::create_dir_all(spec.output_dir).ok();
    let archive_path = spec.output_dir.join(format!(
        "incremental_{}.tar.gz",
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
    ));
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        std::fs::File::create(&archive_path).unwrap(),
        flate2::Compression::default(),
    ));
    add_simple_files(&mut tar, spec);
    add_python_builds(&mut tar, spec);
    let store_db = spec.repository_dir.join(".store.db");
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
    // mirror.tar.gz → mirror.sha256 (not mirror.tar.sha256)
    let stem = archive.file_stem().unwrap().to_str().unwrap();
    let base = stem.rsplit_once('.').map_or(stem, |x| x.0);
    let sha_path = archive.with_file_name(format!("{base}.sha256"));
    let name = archive.file_name().unwrap().to_string_lossy();
    std::fs::write(&sha_path, format!("{digest}  {name}\n"))?;
    Ok(sha_path)
}

pub fn tar_compression() -> Compression {
    match std::env::var("PIP_MIRROR_TAR_COMPRESSION").as_deref() {
        Ok("none") => Compression::none(),
        _ => Compression::best(),
    }
}

pub fn pack_mirror_archive(
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let archive = std::env::current_dir()?.join("mirror.tar.gz");
    crate::downloader::pack_full_mirror(repo, &archive, tar_compression())?;
    let sha = write_sha256(&archive)?;
    let mb = std::fs::metadata(&archive)?.len() as f64 / 1024.0 / 1024.0;
    tracing::info!("mirror.tar.gz : {} ({mb:.2} MB)", archive.display());
    tracing::info!("mirror.sha256 : {}", sha.display());
    Ok(())
}
