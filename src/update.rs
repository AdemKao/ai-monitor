use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const API_URL: &str = "https://api.github.com/repos/AdemKao/ai-monitor/releases/latest";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug)]
pub struct UpdateInfo {
    pub current: Version,
    pub latest: Version,
    pub latest_tag: String,
}

pub fn check() -> Result<UpdateInfo> {
    let current = Version::parse(env!("CARGO_PKG_VERSION")).context("invalid local version")?;
    let release = fetch_latest_release()?;
    let latest = release_version(&release)?;
    Ok(UpdateInfo {
        current,
        latest,
        latest_tag: release.tag_name,
    })
}

pub fn install_latest() -> Result<UpdateInfo> {
    let release = fetch_latest_release()?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).context("invalid local version")?;
    let latest = release_version(&release)?;
    let target = target_triple()?;
    let archive_name = archive_name(target);
    let checksum_name = format!("{archive_name}.sha256");
    let archive = find_asset(&release, &archive_name)?;
    let checksum = find_asset(&release, &checksum_name)?;
    let client = http_client()?;
    let temporary = TempDir::new().context("could not create update workspace")?;
    let archive_path = temporary.path().join(&archive.name);
    let archive_bytes = download(&client, &archive.browser_download_url)?;
    verify_checksum(
        &archive_bytes,
        &download(&client, &checksum.browser_download_url)?,
        &archive.name,
    )?;
    fs::write(&archive_path, archive_bytes).context("could not save update archive")?;
    let extracted = extract_archive(&archive_path, temporary.path(), target)?;
    self_replace::self_replace(&extracted).context("could not replace current binary")?;
    Ok(UpdateInfo {
        current,
        latest,
        latest_tag: release.tag_name,
    })
}

fn fetch_latest_release() -> Result<Release> {
    let client = http_client()?;
    client
        .get(API_URL)
        .send()
        .context("could not query GitHub releases")?
        .error_for_status()
        .context("GitHub release lookup failed")?
        .json()
        .context("GitHub release response was invalid")
}

fn http_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("ai-monitor/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("could not initialize update client")
}

fn release_version(release: &Release) -> Result<Version> {
    Version::parse(release.tag_name.trim_start_matches('v'))
        .with_context(|| format!("GitHub release tag is invalid: {}", release.tag_name))
}

fn find_asset<'a>(release: &'a Release, name: &str) -> Result<&'a Asset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .with_context(|| format!("GitHub release is missing asset: {name}"))
}

fn download(client: &Client, url: &str) -> Result<Vec<u8>> {
    client
        .get(url)
        .send()
        .context("update asset download failed")?
        .error_for_status()
        .context("update asset returned an error")?
        .bytes()
        .map(|bytes| bytes.to_vec())
        .context("could not read update asset")
}

fn verify_checksum(archive: &[u8], checksum: &[u8], name: &str) -> Result<()> {
    let checksum_text = String::from_utf8_lossy(checksum);
    let expected = checksum_text
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let filename = fields.next().unwrap_or_default().trim_start_matches('*');
            (filename.is_empty() || filename == name).then_some(hash)
        })
        .ok_or_else(|| anyhow::anyhow!("checksum file did not contain {name}"))?;
    let actual = format!("{:x}", Sha256::digest(archive));
    if actual != expected {
        bail!("checksum verification failed for {name}");
    }
    Ok(())
}

fn target_triple() -> Result<&'static str> {
    if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        bail!("automatic update is not supported on this platform")
    }
}

fn archive_name(target: &str) -> String {
    let extension = if target.contains("windows") {
        "zip"
    } else {
        "tar.xz"
    };
    format!("ai-monitor-{target}.{extension}")
}

fn extract_archive(archive: &Path, destination: &Path, target: &str) -> Result<PathBuf> {
    let root = destination.join(format!("ai-monitor-{target}"));
    if target.contains("windows") {
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "param($archive, $destination); Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force",
                &archive.to_string_lossy(),
                &destination.to_string_lossy(),
            ])
            .status()
            .context("could not start PowerShell to extract update")?;
        if !status.success() {
            bail!("PowerShell could not extract update archive");
        }
        return binary_path(&root, true);
    }

    let status = Command::new("tar")
        .args(["-xJf"])
        .arg(archive)
        .args(["-C"])
        .arg(destination)
        .status()
        .context("could not start tar to extract update")?;
    if !status.success() {
        bail!("tar could not extract update archive");
    }
    binary_path(&root, false)
}

fn binary_path(root: &Path, windows: bool) -> Result<PathBuf> {
    let path = root.join(if windows {
        "ai-monitor.exe"
    } else {
        "ai-monitor"
    });
    if !path.is_file() {
        bail!("update archive did not contain the expected binary");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_release_checksum() {
        let data = b"ai-monitor";
        let digest = format!("{:x}", Sha256::digest(data));
        let checksum = format!("{digest} *ai-monitor.tar.xz\n");
        verify_checksum(data, checksum.as_bytes(), "ai-monitor.tar.xz").unwrap();
    }

    #[test]
    fn rejects_wrong_checksum() {
        let error = verify_checksum(b"actual", b"00 *ai-monitor.tar.xz\n", "ai-monitor.tar.xz")
            .unwrap_err();
        assert!(error.to_string().contains("checksum verification failed"));
    }
}
