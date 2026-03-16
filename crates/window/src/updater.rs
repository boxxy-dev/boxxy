use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use self_update::backends::github::ReleaseList;
use self_update::Download;
use tar::Archive;
use flate2::read::GzDecoder;
use reqwest::header::{USER_AGENT, HeaderValue};

const REPO_OWNER: &str = "miifrommera";
const REPO_NAME: &str = "boxxy";

/// Handles the background update checking and downloading.
pub struct Updater;

impl Updater {
    /// Checks for a new nightly build on GitHub.
    /// Returns the release ID and download URL if an update is available.
    pub async fn check_for_update() -> Result<Option<(String, String)>> {
        if boxxy_ai_core::utils::is_flatpak() {
            return Ok(None);
        }

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
        let nightly = releases.iter()
            .find(|r| r.version == "nightly")
            .context("Could not find nightly release on GitHub")?;

        // Check if we've already installed this specific release.
        // We use the "date" as a heuristic for nightly releases.
        let last_update_path = Self::get_app_dir()?.join(".last_update");
        if last_update_path.exists() {
            let last_id = fs::read_to_string(&last_update_path)?;
            if last_id.trim() == nightly.date {
                return Ok(None);
            }
        }

        // Find the correct asset for this architecture
        let target_arch = if cfg!(target_arch = "x86_64") { "x86_64" } else { "aarch64" };
        let asset = nightly.assets.iter()
            .find(|a| a.name.contains(target_arch) && a.name.ends_with(".tar.gz"));

        if let Some(asset) = asset {
            return Ok(Some((nightly.version.clone(), asset.download_url.clone())));
        }

        Ok(None)
    }

    pub async fn download_update(url: String) -> Result<PathBuf> {
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
                .set_header(reqwest::header::ACCEPT, HeaderValue::from_static("application/octet-stream"))
                .download_to(&mut dest)
        })
        .await
        .context("Failed to download update")??;

        log::info!("Download complete, extracting tarball...");

        // Extract the tarball
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

        Ok(pending_dir)
    }

    /// Performs the "Atomic Swap" and restarts the application.
    /// This should be called when the user clicks "Restart to Update".
    pub fn apply_update_and_restart() -> Result<()> {
        log::info!("Applying update and restarting...");
        let app_dir = Self::get_app_dir()?;
        let bin_dir = app_dir.join("bin");
        let pending_dir = app_dir.join("updates").join("pending");
        
        // Ensure bin directory exists (in case of fresh install via updater)
        if !bin_dir.exists() {
            fs::create_dir_all(&bin_dir)?;
        }

        // Find the release info to get the date for the .last_update file
        // This is a bit redundant but ensures we don't prompt again immediately
        let releases = ReleaseList::configure()
            .repo_owner(REPO_OWNER)
            .repo_name(REPO_NAME)
            .build()?
            .fetch()?;
        
        let nightly_date = releases.iter()
            .find(|r| r.version == "nightly")
            .map(|r| r.date.clone())
            .unwrap_or_default();

        // The archive structure has a top-level folder like "boxxy-terminal-nightly-linux-x86_64"
        let entries = fs::read_dir(&pending_dir)?;
        let mut inner_folder = None;
        for entry in entries {
            if let Ok(entry) = entry {
                if entry.file_type()?.is_dir() && entry.file_name().to_string_lossy().starts_with("boxxy-terminal") {
                    inner_folder = Some(entry.path());
                    break;
                }
            }
        }
        
        let inner_folder = inner_folder.context("Could not find extracted content folder")?;
        let pending_bin = inner_folder.join("bin");

        log::info!("Swapping binaries from {:?} to {:?}", pending_bin, bin_dir);

        // Swap boxxy-terminal
        Self::swap_binary(&bin_dir.join("boxxy-terminal"), &pending_bin.join("boxxy-terminal"))?;
        
        // Swap boxxy-agent
        Self::swap_binary(&bin_dir.join("boxxy-agent"), &pending_bin.join("boxxy-agent"))?;

        // Update .last_update file with the build date
        fs::write(app_dir.join(".last_update"), nightly_date)?;

        log::info!("Restarting app...");
        let _ = Command::new(bin_dir.join("boxxy-terminal"))
            .arg("--new-window")
            .spawn();

        std::process::exit(0);
    }

    fn swap_binary(current: &Path, new: &Path) -> Result<()> {
        if !new.exists() { return Ok(()); }
        
        let old_backup = current.with_extension("old");
        if current.exists() {
            let _ = fs::remove_file(&old_backup);
            fs::rename(current, &old_backup)?;
        }
        fs::copy(new, current)?;
        
        // Ensure it's executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(current)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(current, perms)?;
        }

        Ok(())
    }

    fn get_app_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".local").join("boxxy-terminal"))
    }
}
