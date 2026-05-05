use anyhow::{Context, Result};
#[allow(unused_imports)]
use flate2::read::GzDecoder;
use reqwest::header::{HeaderValue, USER_AGENT};
use self_update::Download;
use self_update::backends::github::ReleaseList;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
#[allow(unused_imports)]
use tar::Archive;

const REPO_OWNER: &str = "boxxy-dev";
const REPO_NAME: &str = "boxxy";

/// Handles the background update checking and downloading.
pub struct Updater;

impl Updater {
    /// Checks for a new nightly build on GitHub.
    /// Returns `(version, date, download_url, checksum_url)` if an update is available.
    pub async fn check_for_update() -> Result<Option<(String, String, String, Option<String>)>> {
        if !boxxy_sys_utils::can_self_update() {
            return Ok(None);
        }

        log::debug!("Checking GitHub for nightly updates...");

        let releases = tokio::task::spawn_blocking(|| {
            ReleaseList::configure()
                .repo_owner(REPO_OWNER)
                .repo_name(REPO_NAME)
                .build()?
                .fetch()
        })
        .await
        .context("Failed to spawn blocking task for release check")??;

        if releases.is_empty() {
            return Ok(None);
        }

        // For nightly, we check the latest release with tag "nightly"
        let nightly = releases
            .iter()
            .find(|r| r.version == "nightly")
            .context("Could not find nightly release on GitHub")?;

        // Check if we've already installed this specific release.
        let last_update_path = Self::get_app_dir()?.join(".last_update");
        if last_update_path.exists() {
            let last_id = fs::read_to_string(&last_update_path)?;
            if last_id.trim() == nightly.date {
                return Ok(None);
            }
        }

        // Find the correct asset for this architecture.
        let target_arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            "aarch64"
        };
        let asset = nightly
            .assets
            .iter()
            .find(|a| a.name.contains(target_arch) && a.name.ends_with(".tar.gz"));

        if let Some(asset) = asset {
            // Try to find a corresponding .sha256 asset
            let checksum_name = format!("{}.sha256", asset.name);
            let checksum_asset = nightly.assets.iter().find(|a| a.name == checksum_name);

            return Ok(Some((
                nightly.version.clone(),
                nightly.date.clone(),
                asset.download_url.clone(),
                checksum_asset.map(|a| a.download_url.clone()),
            )));
        }

        Ok(None)
    }

    pub async fn download_update(
        url: String,
        date: String,
        checksum_url: Option<String>,
    ) -> Result<PathBuf> {
        if !boxxy_sys_utils::can_self_update() {
            anyhow::bail!("Self-update is disabled in this build.");
        }

        log::info!("Starting update download from: {}", url);
        let pending_dir = Self::get_app_dir()?.join("updates").join("pending");
        if pending_dir.exists() {
            fs::remove_dir_all(&pending_dir)?;
        }
        fs::create_dir_all(&pending_dir)?;

        let tmp_tarball = pending_dir.join("update.tar.gz");
        let url_clone = url.clone();
        let tmp_tarball_clone = tmp_tarball.clone();

        tokio::task::spawn_blocking(move || {
            let mut dest = fs::File::create(&tmp_tarball_clone)?;
            Download::from_url(&url_clone)
                .set_header(USER_AGENT, HeaderValue::from_static("boxxy-terminal"))
                .set_header(
                    reqwest::header::ACCEPT,
                    HeaderValue::from_static("application/octet-stream"),
                )
                .download_to(&mut dest)
        })
        .await
        .context("Failed to download update")??;

        log::info!("Download complete, verifying checksum...");

        // Attempt to fetch and verify the SHA-256 checksum published alongside
        // the release asset.
        if let Some(checksum_url) = checksum_url {
            match Self::fetch_checksum(&checksum_url).await {
                Ok(expected) if !expected.is_empty() => {
                    let path_for_hash = tmp_tarball.clone();
                    let actual =
                        tokio::task::spawn_blocking(move || Self::sha256_file(&path_for_hash))
                            .await
                            .context("Checksum computation task failed")??;
                    if actual != expected {
                        return Err(anyhow::anyhow!(
                            "Checksum mismatch: expected {expected}, got {actual}"
                        ));
                    }
                    log::info!("Checksum verified OK.");
                }
                Ok(_) => {
                    log::warn!("Checksum asset returned an empty hash; skipping verification.")
                }
                Err(e) => log::warn!("Checksum asset unavailable ({e}); skipping verification."),
            }
        } else {
            log::warn!("No checksum URL provided; skipping verification.");
        }

        log::info!("Extracting tarball...");
        let extract_path = pending_dir.clone();
        let tmp_tarball_extract = tmp_tarball.clone();
        tokio::task::spawn_blocking(move || {
            let tar_gz = fs::File::open(&tmp_tarball_extract)?;
            let tar = GzDecoder::new(tar_gz);
            let mut archive = Archive::new(tar);
            archive.unpack(&extract_path)
        })
        .await
        .context("Failed to extract update")??;

        log::info!("Extraction complete to: {:?}", pending_dir);

        // Persist the release date so apply_update_and_restart can write
        // .last_update without a second network round-trip.
        fs::write(pending_dir.join(".update_date"), &date)?;

        Ok(pending_dir)
    }

    /// Performs the "Atomic Swap" and restarts the application.
    /// This should be called when the user clicks "Restart to Update".
    pub fn apply_update_and_restart() -> Result<()> {
        if !boxxy_sys_utils::can_self_update() {
            anyhow::bail!("Self-update is disabled in this build.");
        }

        log::info!("Applying update and restarting...");
        let app_dir = Self::get_app_dir()?;
        let bin_dir = app_dir.join("bin");
        let pending_dir = app_dir.join("updates").join("pending");

        // Ensure bin directory exists (in case of fresh install via updater).
        if !bin_dir.exists() {
            fs::create_dir_all(&bin_dir)?;
        }

        // Read the release date stored during download — no network call needed.
        // Falls back to an empty string for installs predating this mechanism.
        let nightly_date = fs::read_to_string(pending_dir.join(".update_date")).unwrap_or_default();

        // The archive has a top-level folder like "boxxy-terminal-nightly-linux-x86_64".
        let entries = fs::read_dir(&pending_dir)?;
        let mut inner_folder = None;
        for entry in entries {
            if let Ok(entry) = entry
                && entry.file_type()?.is_dir()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("boxxy-terminal")
            {
                inner_folder = Some(entry.path());
                break;
            }
        }

        let inner_folder = inner_folder.context("Could not find extracted content folder")?;
        let pending_bin = inner_folder.join("bin");

        log::info!("Swapping binaries from {:?} to {:?}", pending_bin, bin_dir);

        Self::swap_binary(
            &bin_dir.join("boxxy-terminal"),
            &pending_bin.join("boxxy-terminal"),
        )?;

        Self::swap_binary(
            &bin_dir.join("boxxy-agent"),
            &pending_bin.join("boxxy-agent"),
        )?;

        // Update .last_update file with the build date.
        fs::write(app_dir.join(".last_update"), nightly_date)?;

        log::info!("Stopping old agent before restart...");
        let _ = Command::new(bin_dir.join("boxxy-agent"))
            .arg("stop")
            .output();

        // We do NOT need to manually start the new agent here.
        // crates/window/src/agent_deployer.rs automatically checks for and
        // spawns the agent when the new `boxxy-terminal` process starts.

        log::info!("Restarting app...");
        let _ = Command::new(bin_dir.join("boxxy-terminal"))
            .arg("--new-window")
            .spawn();

        std::process::exit(0);
    }

    fn swap_binary(current: &Path, new: &Path) -> Result<()> {
        if !new.exists() {
            return Ok(());
        }

        let old_backup = current.with_extension("old");
        if current.exists() {
            let _ = std::fs::remove_file(&old_backup);
            std::fs::rename(current, &old_backup)?;
        }

        // Atomically move the new binary into place.
        std::fs::rename(new, current)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(current)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(current, perms)?;
        }

        Ok(())
    }

    async fn fetch_checksum(url: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let text = client
            .get(url)
            .header(USER_AGENT, HeaderValue::from_static("boxxy-terminal"))
            .header(
                reqwest::header::ACCEPT,
                HeaderValue::from_static("application/octet-stream"),
            )
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        // .sha256 file format: "hexhash  filename\n" — we only need the hash.
        Ok(text.split_whitespace().next().unwrap_or("").to_string())
    }

    fn sha256_file(path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let hash = hasher.finalize();
        let mut hex_str = String::with_capacity(64);
        for b in hash {
            use std::fmt::Write;
            write!(&mut hex_str, "{:02x}", b).unwrap();
        }
        Ok(hex_str)
    }

    fn get_app_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".local").join("boxxy-terminal"))
    }
}
