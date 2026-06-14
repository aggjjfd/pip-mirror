use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::info;

use crate::downloader::FileInfo;

/// 增量/全量镜像包的压缩方式。
#[derive(Clone, Copy, Debug)]
pub enum TarCompression {
    /// 不压缩，输出纯 `.tar`。
    None,
    /// 使用 zstd 压缩，输出 `.tar.zst`。
    Zstd { level: i32 },
}

impl Default for TarCompression {
    fn default() -> Self {
        TarCompression::Zstd { level: 10 }
    }
}

/// 根据压缩方式选择 `.tar` 或 `.tar.zst` 后缀。
fn archive_extension(compression: TarCompression) -> &'static str {
    match compression {
        TarCompression::None => ".tar",
        TarCompression::Zstd { .. } => ".tar.zst",
    }
}

/// 包装普通文件或 zstd 编码器，统一作为 tar 写入目标。
enum CompressedWriter<W: Write> {
    Plain(W),
    Zstd(zstd::stream::write::Encoder<'static, W>),
}

impl<W: Write> Write for CompressedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            CompressedWriter::Plain(w) => w.write(buf),
            CompressedWriter::Zstd(e) => e.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            CompressedWriter::Plain(w) => w.flush(),
            CompressedWriter::Zstd(e) => e.flush(),
        }
    }
}

fn finish_compressed_writer<W: Write>(
    writer: CompressedWriter<W>,
) -> io::Result<()> {
    match writer {
        CompressedWriter::Plain(_) => Ok(()),
        CompressedWriter::Zstd(enc) => {
            enc.finish()?;
            Ok(())
        }
    }
}

fn create_compressed_writer<W: Write>(
    writer: W,
    compression: TarCompression,
) -> io::Result<CompressedWriter<W>> {
    match compression {
        TarCompression::None => Ok(CompressedWriter::Plain(writer)),
        TarCompression::Zstd { level } => {
            let mut encoder = zstd::stream::write::Encoder::new(writer, level)?;
            encoder.set_parameter(
                zstd::zstd_safe::CParameter::EnableLongDistanceMatching(true),
            )?;
            encoder.multithread(0)?;
            Ok(CompressedWriter::Zstd(encoder))
        }
    }
}

fn is_excluded(name: &std::ffi::OsStr, exclude: &[&str]) -> bool {
    exclude.contains(&name.to_string_lossy().as_ref())
}

fn append_entry(
    tar: &mut tar::Builder<impl std::io::Write>,
    path: &Path,
    dest: &str,
    exclude: &[&str],
) -> std::io::Result<()> {
    if is_excluded(path.file_name().unwrap_or_default(), exclude) {
        return Ok(());
    }
    if !path.is_dir() {
        tar.append_path_with_name(path, dest)?;
        return Ok(());
    }
    tar.append_dir(dest, path)?;
    append_dir_contents(tar, path, dest, exclude)
}

fn append_dir_contents(
    tar: &mut tar::Builder<impl std::io::Write>,
    path: &Path,
    dest: &str,
    exclude: &[&str],
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_excluded(&name, exclude) {
            continue;
        }
        let child_dest = format!("{}/{}", dest, name.to_string_lossy());
        append_entry(tar, &entry.path(), &child_dest, exclude)?;
    }
    Ok(())
}

pub fn pack_full_mirror(
    repo: &Path,
    output: &Path,
    compression: TarCompression,
) -> std::io::Result<()> {
    let archive = std::fs::File::create(output)?;
    let writer = create_compressed_writer(archive, compression)?;
    let mut tar = tar::Builder::new(writer);
    tar.follow_symlinks(false);
    let dest = repo.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    tar.append_dir(dest, repo)?;
    append_dir_contents(&mut tar, repo, dest, &[".access_log.db"])?;
    let writer = tar.into_inner()?;
    finish_compressed_writer(writer)?;
    Ok(())
}

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
) -> std::io::Result<()> {
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
            )?;
        }
    }
    Ok(())
}

fn add_python_builds(
    tar: &mut tar::Builder<impl std::io::Write>,
    spec: &IncrementalPackage<'_>,
) -> std::io::Result<()> {
    for pb in spec.python_builds_files {
        if pb.exists() {
            tar.append_path_with_name(
                pb,
                format!(
                    "python-builds/{}",
                    pb.file_name().unwrap().to_string_lossy()
                ),
            )?;
        }
    }
    if let Some(index) = spec.python_builds_index
        && index.exists()
    {
        tar.append_path_with_name(index, "python-builds/index.json")?;
    }
    Ok(())
}

pub fn no_changes(spec: &IncrementalPackage<'_>) -> bool {
    spec.simple_files.is_empty()
        && spec.python_builds_files.is_empty()
        && spec.python_builds_index.is_none()
}

fn build_incremental_archive(
    spec: &IncrementalPackage<'_>,
    archive_path: &Path,
    compression: TarCompression,
) -> std::io::Result<()> {
    let archive_file = std::fs::File::create(archive_path)?;
    let writer = create_compressed_writer(archive_file, compression)?;
    let mut tar = tar::Builder::new(writer);
    add_simple_files(&mut tar, spec)?;
    add_python_builds(&mut tar, spec)?;
    let store_db = spec.repository_dir.join(".store.db");
    if store_db.exists() {
        tar.append_path_with_name(&store_db, ".store.db")?;
    }
    let writer = tar.into_inner()?;
    finish_compressed_writer(writer)
}

pub fn create_incremental_package(
    spec: &IncrementalPackage<'_>,
) -> Result<Option<PathBuf>, std::io::Error> {
    if no_changes(spec) {
        info!("no changes: 本次没有新文件下载, 未产生增量包");
        return Ok(None);
    }
    let compression = tar_compression();
    std::fs::create_dir_all(spec.output_dir)?;
    let archive_path = spec.output_dir.join(format!(
        "incremental_{}{}",
        chrono::Utc::now().format("%Y%m%d_%H%M%S"),
        archive_extension(compression)
    ));
    build_incremental_archive(spec, &archive_path, compression)?;
    info!("增量包已创建: {}", archive_path.display());
    Ok(Some(archive_path))
}

/// Write sha256 checksum file in sha256sum format.
pub fn write_sha256(archive: &Path) -> std::io::Result<PathBuf> {
    let mut file = std::fs::File::open(archive)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let digest = format!("{:x}", hasher.finalize());
    // mirror.tar.zst → mirror.sha256 (not mirror.tar.sha256)
    let stem = archive.file_stem().unwrap().to_str().unwrap();
    let base = stem.rsplit_once('.').map_or(stem, |x| x.0);
    let sha_path = archive.with_file_name(format!("{base}.sha256"));
    let name = archive.file_name().unwrap().to_string_lossy();
    std::fs::write(&sha_path, format!("{digest}  {name}\n"))?;
    Ok(sha_path)
}

pub fn tar_compression() -> TarCompression {
    match std::env::var("PIP_MIRROR_TAR_COMPRESSION").as_deref() {
        Ok("none") => TarCompression::None,
        _ => TarCompression::default(),
    }
}

pub fn pack_mirror_archive(
    repo: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let compression = tar_compression();
    let archive = std::env::current_dir()?
        .join(format!("mirror{}", archive_extension(compression)));
    pack_full_mirror(repo, &archive, compression)?;
    let sha = write_sha256(&archive)?;
    let mb = std::fs::metadata(&archive)?.len() as f64 / 1024.0 / 1024.0;
    let name = archive.file_name().unwrap().to_string_lossy();
    tracing::info!("{name} : {} ({mb:.2} MB)", archive.display());
    tracing::info!("mirror.sha256 : {}", sha.display());
    Ok(())
}

pub fn build_incremental_package(
    repo: &Path,
    downloaded: &[crate::downloader::FileInfo],
    output_dir: &Path,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let inc = IncrementalPackage {
        simple_files: downloaded,
        python_builds_files: &[],
        python_builds_index: None,
        repository_dir: repo,
        output_dir,
    };
    Ok(create_incremental_package(&inc)?)
}

pub async fn build_incremental_package_async(
    repo: &Path,
    downloaded: &[crate::downloader::FileInfo],
    output_dir: &Path,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let repo = repo.to_path_buf();
    let output_dir = output_dir.to_path_buf();
    let downloaded = downloaded.to_vec();
    Ok(tokio::task::spawn_blocking(move || {
        build_incremental_package(&repo, &downloaded, &output_dir)
            .map_err(|e| format!("{e}"))
    })
    .await
    .map_err(|e| format!("打包线程错误: {e}"))??)
}

/// 根据扩展名打开 `.tar`、`.tar.zst` 或旧版 `.tar.gz` 归档，返回可读流。
pub fn open_archive_reader(
    path: &Path,
) -> Result<Box<dyn Read>, Box<dyn std::error::Error>> {
    let f = std::fs::File::open(path)?;
    let path_str = path.to_string_lossy();
    if path_str.ends_with(".tar.zst") {
        Ok(Box::new(zstd::stream::read::Decoder::new(f)?))
    } else if path_str.ends_with(".tar") {
        Ok(Box::new(f))
    } else if path_str.ends_with(".tar.gz") {
        Ok(Box::new(flate2::read::GzDecoder::new(f)))
    } else {
        let suffix = path_str.rfind('.').map_or_else(
            || path_str.to_string(),
            |i| path_str[i..].to_string(),
        );
        Err(format!("不支持的增量包格式: {}", suffix).into())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::open_archive_reader;

    fn append_test_file(tar: &mut tar::Builder<impl Write>, name: &str) {
        let content = format!("hello {name}");
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, content.as_bytes()).unwrap();
    }

    fn assert_archive_contains(path: &std::path::Path, expected: &str) {
        let reader = open_archive_reader(path).unwrap();
        let mut tar = tar::Archive::new(reader);
        let names: Vec<String> = tar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.contains(&expected.to_string()),
            "expected {expected} in {names:?}"
        );
    }

    #[test]
    fn test_open_archive_reader_plain_tar() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.tar");
        let file = std::fs::File::create(&path).unwrap();
        let mut tar = tar::Builder::new(file);
        append_test_file(&mut tar, "plain.txt");
        tar.into_inner().unwrap();
        assert_archive_contains(&path, "plain.txt");
    }

    #[test]
    fn test_open_archive_reader_tar_gz() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let gz =
            flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        append_test_file(&mut tar, "gz.txt");
        tar.into_inner().unwrap().finish().unwrap();
        assert_archive_contains(&path, "gz.txt");
    }

    #[test]
    fn test_open_archive_reader_tar_zst() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.tar.zst");
        let file = std::fs::File::create(&path).unwrap();
        let mut zst = zstd::stream::write::Encoder::new(file, 3).unwrap();
        {
            let mut tar = tar::Builder::new(&mut zst);
            append_test_file(&mut tar, "zst.txt");
        }
        zst.finish().unwrap();
        assert_archive_contains(&path, "zst.txt");
    }
}
