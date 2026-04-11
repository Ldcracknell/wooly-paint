//! Check GitHub Releases and apply self-updates (Linux x86_64 tarball, Windows x86_64 zip).

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;
use tar::Archive;

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

fn repo_slug() -> &'static str {
    option_env!("WOOLYPAINT_GITHUB_REPO").unwrap_or("Ldcracknell/wooly-paint")
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Latest release info when a newer semver is published than this build.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: Version,
    pub tag_name: String,
    pub release_page_url: String,
    pub download_url: String,
    pub asset_name: String,
}

pub fn packaged_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION must be semver")
}

fn parse_release_version(tag: &str) -> Result<Version> {
    let s = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(s).with_context(|| format!("invalid release tag {tag:?}"))
}

fn pick_asset(assets: &[GhAsset]) -> Option<&GhAsset> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        assets.iter().find(|a| {
            a.name.ends_with(".tar.gz") && a.name.contains("linux-arch") && a.name.contains("x86_64")
        })
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        assets.iter().find(|a| {
            a.name.ends_with(".zip") && a.name.contains("windows") && a.name.contains("x86_64")
        })
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        let _ = assets;
        None
    }
}

/// `GET /repos/.../releases/latest`. Returns `Ok(None)` when already up to date or no matching asset.
pub fn check_for_update() -> Result<Option<UpdateInfo>> {
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        return Ok(None);
    }

    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        repo_slug()
    );
    let release: GhRelease = ureq::get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", USER_AGENT)
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .context("GitHub API request failed")?
        .into_json()
        .context("parse GitHub release JSON")?;

    let remote_ver = parse_release_version(&release.tag_name)?;
    let current = packaged_version();
    if remote_ver <= current {
        return Ok(None);
    }

    let Some(asset) = pick_asset(&release.assets) else {
        return Ok(None);
    };

    Ok(Some(UpdateInfo {
        version: remote_ver,
        tag_name: release.tag_name.clone(),
        release_page_url: release.html_url,
        download_url: asset.browser_download_url.clone(),
        asset_name: asset.name.clone(),
    }))
}

fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/octet-stream")
        .call()
        .context("download release asset")?
        .into_reader()
        .read_to_end(&mut buf)
        .context("read release asset")?;
    Ok(buf)
}

fn extract_linux_binary(tgz: &[u8], dest: &Path) -> Result<()> {
    let dec = GzDecoder::new(Cursor::new(tgz));
    let mut archive = Archive::new(dec);
    for entry in archive.entries().context("read tarball")? {
        let mut entry = entry.context("tar entry")?;
        let path = entry.path().context("tar path")?;
        if path.file_name().and_then(|n| n.to_str()) != Some("wooly-paint") {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let _ = std::fs::remove_file(dest);
        entry.unpack(dest).context("unpack wooly-paint binary")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(dest, mode).context("chmod +x")?;
        }
        return Ok(());
    }
    bail!("no wooly-paint binary found in release tarball");
}

fn extract_windows_exe(zip_bytes: &[u8], dest: &Path) -> Result<()> {
    let reader = Cursor::new(zip_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).context("open release zip")?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("zip entry")?;
        let name = file.name();
        if !name.ends_with("wooly-paint.exe") || name.ends_with('/') {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let _ = std::fs::remove_file(dest);
        let mut out = std::fs::File::create(dest).context("create temp exe")?;
        std::io::copy(&mut file, &mut out).context("write exe")?;
        return Ok(());
    }
    bail!("no wooly-paint.exe found in release zip");
}

#[cfg(unix)]
fn replace_running_binary(new_binary: &Path, current_exe: &Path) -> Result<()> {
    let parent = current_exe
        .parent()
        .context("current executable has no parent directory")?;
    let staged = parent.join(".wooly-paint.update");
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(new_binary, &staged).context("stage new binary")?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .context("chmod staged binary")?;
    let backup = parent.join(".wooly-paint.prev");
    let _ = std::fs::remove_file(&backup);
    if current_exe.exists() {
        std::fs::rename(current_exe, &backup).context("backup current binary")?;
    }
    std::fs::rename(&staged, current_exe).context("activate new binary")?;
    Ok(())
}

#[cfg(windows)]
fn spawn_windows_updater(new_exe: &Path, current_exe: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let new_s = new_exe.to_string_lossy();
    let cur_s = current_exe.to_string_lossy();
    let script = format!(
        r#"@echo off
ping 127.0.0.1 -n 3 >nul
copy /Y "{new_s}" "{cur_s}"
start "" "{cur_s}"
del "%~f0"
"#
    );
    let bat = std::env::temp_dir().join("wooly-paint-self-update.bat");
    std::fs::write(&bat, script).context("write updater script")?;
    Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()))
        .arg("/C")
        .arg(&bat)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("spawn updater")?;
    Ok(())
}

/// Download the selected release asset, extract the app binary, swap it in, and re-launch.
/// On Windows the process exits after spawning a helper batch file; on Linux the new binary is started here.
pub fn download_and_apply(info: &UpdateInfo) -> Result<()> {
    let bytes = download_bytes(&info.download_url)?;
    let current = std::env::current_exe().context("current_exe")?;
    let tmp_dir = std::env::temp_dir().join("wooly-paint-update");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).context("temp dir")?;

    #[cfg(target_os = "linux")]
    {
        let extracted = tmp_dir.join("wooly-paint");
        extract_linux_binary(&bytes, &extracted)?;
        replace_running_binary(&extracted, &current)?;
        Command::new(&current)
            .args(std::env::args().skip(1))
            .spawn()
            .context("restart")?;
        std::process::exit(0);
    }

    #[cfg(target_os = "windows")]
    {
        let extracted = tmp_dir.join("wooly-paint.exe");
        extract_windows_exe(&bytes, &extracted)?;
        spawn_windows_updater(&extracted, &current)?;
        std::process::exit(0);
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (bytes, current, tmp_dir);
        bail!("self-update is only supported on Linux and Windows x86_64");
    }
}
