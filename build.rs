use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn curl_download(
    url: &str,
    dest: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--retry",
            "3",
            "--location",
            "--silent",
            "--show-error",
            "--fail",
            "-o",
            dest,
            url,
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("curl exited with {:?}", status.code()).into())
    }
}

fn ureq_download(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let mut body = Vec::new();
    agent
        .get(url)
        .call()?
        .body_mut()
        .as_reader()
        .read_to_end(&mut body)?;
    Ok(body)
}

fn write_bytes(dest: &str, body: &[u8]) {
    if let Err(e) = fs::write(dest, body) {
        panic!("failed to write {}: {}", dest, e);
    }
}

fn fetch(url: &str, dest: &str) {
    if Path::new(dest).exists() {
        return;
    }
    println!("cargo:warning=Downloading {} ...", url);

    // Prefer curl (more reliable on slow networks)
    if curl_download(url, dest).is_ok() {
        return;
    }

    // Fallback to ureq
    for attempt in 1..=3 {
        match ureq_download(url) {
            Ok(body) => {
                write_bytes(dest, &body);
                return;
            }
            Err(e) => {
                eprintln!(
                    "ureq error for {} (attempt {}): {}",
                    url, attempt, e
                );
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    panic!("failed to download {} after all retries", url);
}

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");

    let cargo_toml = fs::read_to_string("Cargo.toml").expect("read Cargo.toml");
    let doc: toml::Table = cargo_toml.parse().expect("parse Cargo.toml");

    let version = doc["package"]["metadata"]["uv-embed"]["version"]
        .as_str()
        .expect("Cargo.toml [package.metadata.uv-embed].version is required");

    println!("cargo:rustc-env=UV_EMBED_VERSION={}", version);

    let base = format!(
        "https://github.com/astral-sh/uv/releases/download/{}",
        version
    );

    let release_dir = format!("assets/uv-releases/{}", version);
    fs::create_dir_all(&release_dir).unwrap();

    let release_files = [
        "uv-x86_64-unknown-linux-gnu.tar.gz",
        "uv-x86_64-unknown-linux-gnu.tar.gz.sha256",
        "uv-x86_64-pc-windows-msvc.zip",
        "uv-x86_64-pc-windows-msvc.zip.sha256",
    ];

    for file in &release_files {
        let dest = format!("{}/{}", release_dir, file);
        let url = format!("{}/{}", base, file);
        fetch(&url, &dest);
    }

    let installer_dir = "assets/installers";
    fs::create_dir_all(installer_dir).unwrap();

    for file in &["uv-installer.sh", "uv-installer.ps1"] {
        let dest = format!("{}/{}", installer_dir, file);
        let url = format!("{}/{}", base, file);
        fetch(&url, &dest);
    }
}
