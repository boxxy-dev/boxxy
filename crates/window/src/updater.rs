use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use self_update::check_interval::UpdateCheckGuard;
use self_update::http::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use self_update::{ArchiveKind, Checksum, Compression, Download, Extract, MoveAll};
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};

const REPO_OWNER: &str = "boxxy-dev";
const REPO_NAME: &str = "boxxy";
const GITHUB_API_BASE: &str = "https://api.github.com";
const USER_AGENT_VALUE: &str = concat!("boxxy-terminal/", env!("CARGO_PKG_VERSION"));
const CHECK_THROTTLE_INTERVAL: Duration = Duration::from_secs(60 * 60); // 1 hour
const MAX_UPDATE_ARCHIVE_BYTES: u64 = 250 * 1024 * 1024; // 250 MiB safety cap

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    url: String,
    browser_download_url: Option<String>,
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    created_at: String,
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}

/// Handles background update checking, verification, extraction, and atomic multi-binary installation.
pub struct Updater;

impl Updater {
    /// Checks for a new nightly build on GitHub.
    ///
    /// Uses `UpdateCheckGuard` to throttle background checks to once per hour (preventing
    /// exhaustion of GitHub's 60 req/hr unauthenticated IP quota), and queries the dedicated
    /// `/releases/tags/nightly` endpoint directly (`self_update 1.x`'s `ReleaseList::fetch`
    /// intentionally skips non-semver rolling tags like `nightly`).
    ///
    /// Returns `(version, date, download_url, checksum_source)` if an update is available.
    /// `checksum_source` may be either a `sha256:<hex>` digest published directly by GitHub's
    /// release API or the download URL of the companion `.sha256` sums asset.
    pub async fn check_for_update() -> Result<Option<(String, String, String, Option<String>)>> {
        if !boxxy_sys_utils::can_self_update() {
            return Ok(None);
        }

        let app_dir = Self::get_app_dir()?;
        if !app_dir.exists() {
            let _ = fs::create_dir_all(&app_dir);
        }

        let check_guard = UpdateCheckGuard::new(
            app_dir.join(".last_update_check"),
            CHECK_THROTTLE_INTERVAL,
        );
        match check_guard.should_check() {
            Ok(false) => {
                log::debug!("Update check throttled by UpdateCheckGuard; skipping.");
                return Ok(None);
            }
            Ok(true) => {}
            Err(e) => {
                log::debug!("UpdateCheckGuard read warning ({e}); proceeding with check.");
            }
        }

        log::debug!("Checking GitHub for nightly updates...");

        let release = Self::fetch_nightly_release(GITHUB_API_BASE).await?;
        let _ = check_guard.record_check();

        let Some(nightly) = release else {
            return Ok(None);
        };

        let release_date = nightly
            .published_at
            .as_deref()
            .unwrap_or(&nightly.created_at);

        // Check if we have already installed this specific build date.
        let last_update_path = app_dir.join(".last_update");
        if last_update_path.exists()
            && let Ok(last_id) = fs::read_to_string(&last_update_path)
            && last_id.trim() == release_date.trim()
        {
            return Ok(None);
        }

        let target_arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            "aarch64"
        };

        Ok(Self::select_nightly_asset(&nightly, target_arch))
    }

    fn select_nightly_asset(
        nightly: &GithubRelease,
        target_arch: &str,
    ) -> Option<(String, String, String, Option<String>)> {
        let asset = nightly
            .assets
            .iter()
            .find(|a| a.name.contains(target_arch) && a.name.ends_with(".tar.gz"))?;

        let checksum_name = format!("{}.sha256", asset.name);
        let checksum_asset = nightly.assets.iter().find(|a| a.name == checksum_name);

        // Prefer the companion `.sha256` URL if present, falling back to GitHub's per-asset
        // `digest` field (`sha256:<hex>`).
        let checksum_source = checksum_asset
            .map(|a| {
                a.browser_download_url
                    .as_deref()
                    .unwrap_or(&a.url)
                    .to_string()
            })
            .or_else(|| asset.digest.clone());

        let download_url = asset
            .browser_download_url
            .as_deref()
            .unwrap_or(&asset.url)
            .to_string();

        let release_date = nightly
            .published_at
            .as_deref()
            .unwrap_or(&nightly.created_at)
            .to_string();

        Some((
            nightly.tag_name.clone(),
            release_date,
            download_url,
            checksum_source,
        ))
    }

    async fn fetch_nightly_release(api_base: &str) -> Result<Option<GithubRelease>> {
        let url = format!(
            "{}/repos/{}/{}/releases/tags/nightly",
            api_base.trim_end_matches('/'),
            REPO_OWNER,
            REPO_NAME
        );

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        let mut req = client
            .get(&url)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(ACCEPT, "application/vnd.github+json");

        if let Some(token) = Self::github_token_from_env() {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }

        let resp = req.send().await?;
        let status = resp.status();

        if !status.is_success() {
            let err = self_update::Error::http_status_error_with_headers(
                status.as_u16(),
                &url,
                resp.headers(),
            );
            match err {
                self_update::Error::NotFound { .. } => {
                    log::debug!("No nightly release found at {url}");
                    return Ok(None);
                }
                err @ self_update::Error::RateLimited { .. } => {
                    log::warn!(
                        "GitHub API rate limit reached while checking for updates (retry delay: {:?})",
                        err.rate_limit_delay()
                    );
                    return Ok(None);
                }
                other => return Err(other.into()),
            }
        }

        let release: GithubRelease = resp
            .json()
            .await
            .context("Failed to deserialize GitHub nightly release payload")?;
        Ok(Some(release))
    }

    /// Reads `GH_TOKEN` then `GITHUB_TOKEN` (matching `gh` CLI and `self_update 1.0` precedence).
    fn github_token_from_env() -> Option<String> {
        for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
            if let Ok(val) = std::env::var(var) {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    pub async fn download_update(
        url: String,
        date: String,
        checksum_source: Option<String>,
    ) -> Result<PathBuf> {
        if !boxxy_sys_utils::can_self_update() {
            anyhow::bail!("Self-update is disabled in this build.");
        }

        log::info!("Starting update download from: {url}");
        let pending_dir = Self::get_app_dir()?.join("updates").join("pending");
        if pending_dir.exists() {
            fs::remove_dir_all(&pending_dir)?;
        }
        fs::create_dir_all(&pending_dir)?;

        let asset_file_name = url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("update.tar.gz")
            .to_string();
        let tmp_tarball = pending_dir.join(&asset_file_name);

        {
            let mut dest = fs::File::create(&tmp_tarball)?;
            let mut download = Download::from_url(&url);
            download
                .request_header(USER_AGENT, USER_AGENT_VALUE)
                .request_header(ACCEPT, "application/octet-stream")
                .timeout(Duration::from_secs(120))
                .max_download_size(MAX_UPDATE_ARCHIVE_BYTES);

            if let Some(token) = Self::github_token_from_env() {
                download.request_header(AUTHORIZATION, format!("Bearer {token}"));
            }

            download
                .download_to_async(&mut dest)
                .await
                .context("Failed to download update archive")?;
        }

        log::info!("Download complete, verifying checksum...");

        if let Some(source) = checksum_source {
            Self::resolve_and_verify_checksum(&source, &asset_file_name, &tmp_tarball).await?;
        } else {
            log::warn!("No checksum source provided; skipping verification.");
        }

        log::info!("Extracting tarball via self_update::Extract...");
        let extract_dir = pending_dir.clone();
        let tarball_for_extract = tmp_tarball.clone();
        tokio::task::spawn_blocking(move || {
            Extract::from_source(&tarball_for_extract)
                .archive(ArchiveKind::Tar(Some(Compression::Gz)))
                .extract_into(&extract_dir)
        })
        .await
        .context("Failed to join archive extraction task")?
        .context("Failed to extract update archive")?;

        // Remove the downloaded archive after successful unpack so only the extracted tree remains.
        let _ = fs::remove_file(&tmp_tarball);

        log::info!("Extraction complete to: {}", pending_dir.display());

        // Persist the release date so apply_update_and_restart can write
        // .last_update without a second network round-trip.
        fs::write(pending_dir.join(".update_date"), &date)?;

        Ok(pending_dir)
    }

    async fn resolve_and_verify_checksum(
        source: &str,
        asset_file_name: &str,
        archive_path: &Path,
    ) -> Result<()> {
        let trimmed = source.trim();
        let checksum_opt = if trimmed.starts_with("sha256:") || trimmed.starts_with("sha512:") {
            Some(
                Checksum::parse_digest(trimmed)
                    .context("Invalid release asset digest from GitHub")?,
            )
        } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            match Self::fetch_checksum_text(trimmed).await {
                Ok(sums_text) if !sums_text.trim().is_empty() => Some(
                    Checksum::from_sums_file(&sums_text, asset_file_name)
                        .context("Failed to parse .sha256 sums file")?,
                ),
                Ok(_) => {
                    log::warn!("Checksum asset returned an empty body; skipping verification.");
                    None
                }
                Err(e) => {
                    log::warn!("Checksum asset unavailable ({e}); skipping verification.");
                    None
                }
            }
        } else if !trimmed.is_empty() {
            Some(
                Checksum::from_sums_file(trimmed, asset_file_name)
                    .context("Failed to parse inline checksum")?,
            )
        } else {
            None
        };

        if let Some(checksum) = checksum_opt {
            let path = archive_path.to_path_buf();
            tokio::task::spawn_blocking(move || Self::verify_file_checksum(&path, &checksum))
                .await
                .context("Checksum verification task panicked")??;
            log::info!("Checksum verified OK.");
        }

        Ok(())
    }

    pub(crate) fn verify_file_checksum(path: &Path, checksum: &Checksum) -> Result<()> {
        match checksum {
            Checksum::Sha256(expected) => {
                let actual = Self::hash_file::<Sha256>(path)?;
                if !actual.eq_ignore_ascii_case(expected.trim()) {
                    return Err(self_update::Error::checksum_mismatch(
                        expected.trim().to_lowercase(),
                        actual,
                    )
                    .into());
                }
            }
            Checksum::Sha512(expected) => {
                let actual = Self::hash_file::<Sha512>(path)?;
                if !actual.eq_ignore_ascii_case(expected.trim()) {
                    return Err(self_update::Error::checksum_mismatch(
                        expected.trim().to_lowercase(),
                        actual,
                    )
                    .into());
                }
            }
            _ => anyhow::bail!("Unsupported checksum algorithm"),
        }
        Ok(())
    }

    /// Performs the transactional "Atomic Swap" via `self_update::MoveAll` and restarts the app.
    pub fn apply_update_and_restart() -> Result<()> {
        if !boxxy_sys_utils::can_self_update() {
            anyhow::bail!("Self-update is disabled in this build.");
        }

        log::info!("Applying update and restarting...");
        let app_dir = Self::get_app_dir()?;
        let bin_dir = Self::apply_staged_update(&app_dir)?;

        log::info!("Stopping old agent before restart...");
        let _ = Command::new(bin_dir.join("boxxy-agent"))
            .arg("stop")
            .output();

        // crates/window/src/agent_deployer.rs automatically checks for and
        // spawns the new agent when the new `boxxy-terminal` process starts.
        log::info!("Restarting app...");
        let _ = Command::new(bin_dir.join("boxxy-terminal"))
            .arg("--new-window")
            .spawn();

        std::process::exit(0);
    }

    /// Transactionally swaps `boxxy-terminal` and `boxxy-agent` from `app_dir/updates/pending`
    /// into `app_dir/bin` using `self_update::MoveAll`, rolling back both if either move fails.
    pub(crate) fn apply_staged_update(app_dir: &Path) -> Result<PathBuf> {
        let bin_dir = app_dir.join("bin");
        let updates_dir = app_dir.join("updates");
        let pending_dir = updates_dir.join("pending");

        if !bin_dir.exists() {
            fs::create_dir_all(&bin_dir)?;
        }

        let nightly_date = fs::read_to_string(pending_dir.join(".update_date")).unwrap_or_default();

        // Locate the extracted top-level directory (e.g. `boxxy-terminal-nightly-linux-x86_64`).
        let mut inner_folder = None;
        for entry in fs::read_dir(&pending_dir)? {
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

        let new_terminal = pending_bin.join("boxxy-terminal");
        let new_agent = pending_bin.join("boxxy-agent");

        for bin in [&new_terminal, &new_agent] {
            if !bin.exists() {
                anyhow::bail!("Staged binary missing from update archive: {}", bin.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(bin)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(bin, perms)?;
            }
        }

        log::info!(
            "Transactionally swapping binaries from {} to {}",
            pending_bin.display(),
            bin_dir.display()
        );

        // Create the stash directory inside `app_dir` so all renames remain on the same filesystem.
        let stash_dir = tempfile::TempDir::new_in(app_dir)
            .context("Failed to create transactional stash directory")?;

        MoveAll::from_temp(stash_dir.path())
            .add(&new_terminal, bin_dir.join("boxxy-terminal"))
            .add(&new_agent, bin_dir.join("boxxy-agent"))
            .commit()
            .context("Transactional binary swap failed (rolled back to previous version)")?;

        // Best-effort sync of updated desktop entry and icon inside app_dir/share
        let pending_share = inner_folder.join("share");
        if pending_share.exists() {
            let _ = Self::sync_share_assets(&pending_share, &app_dir.join("share"));
        }

        if !nightly_date.is_empty() {
            fs::write(app_dir.join(".last_update"), nightly_date)?;
        }

        let _ = fs::remove_dir_all(&pending_dir);

        Ok(bin_dir)
    }

    fn sync_share_assets(src_share: &Path, dst_share: &Path) -> Result<()> {
        let rel_paths = [
            "applications/dev.boxxy.BoxxyTerminal.desktop",
            "icons/hicolor/scalable/apps/dev.boxxy.BoxxyTerminal.png",
        ];
        for rel in rel_paths {
            let src = src_share.join(rel);
            if src.exists() {
                let dst = dst_share.join(rel);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&src, &dst)?;
            }
        }
        Ok(())
    }

    async fn fetch_checksum_text(url: &str) -> Result<String> {
        let mut buf = Vec::new();
        let mut download = Download::from_url(url);
        download
            .request_header(USER_AGENT, USER_AGENT_VALUE)
            .request_header(ACCEPT, "application/octet-stream")
            .timeout(Duration::from_secs(15))
            .max_download_size(64 * 1024);

        if let Some(token) = Self::github_token_from_env() {
            download.request_header(AUTHORIZATION, format!("Bearer {token}"));
        }

        download.download_to_async(&mut buf).await?;
        Ok(String::from_utf8(buf)?)
    }

    fn hash_file<D: Digest>(path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)?;
        let mut hasher = D::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let hash = hasher.finalize();
        let mut hex_str = String::with_capacity(hash.len() * 2);
        for b in hash {
            use std::fmt::Write;
            let _ = write!(&mut hex_str, "{b:02x}");
        }
        Ok(hex_str)
    }

    fn get_app_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".local").join("boxxy-terminal"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_verification_accepts_sums_file_and_release_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("boxxy-terminal-nightly-linux-x86_64.tar.gz");
        fs::write(&archive_path, b"synthetic-archive-payload").unwrap();

        let expected_sha256 = Updater::hash_file::<Sha256>(&archive_path).unwrap();

        // 1. Verify via coreutils `.sha256` sums format (`<hex>  <filename>`)
        let sums_content = format!(
            "{expected_sha256}  boxxy-terminal-nightly-linux-x86_64.tar.gz\n"
        );
        let parsed_from_sums = Checksum::from_sums_file(
            &sums_content,
            "boxxy-terminal-nightly-linux-x86_64.tar.gz",
        )
        .unwrap();
        Updater::verify_file_checksum(&archive_path, &parsed_from_sums).unwrap();

        // 2. Verify via GitHub's `sha256:<hex>` per-asset digest format
        let parsed_from_digest =
            Checksum::parse_digest(&format!("sha256:{expected_sha256}")).unwrap();
        Updater::verify_file_checksum(&archive_path, &parsed_from_digest).unwrap();

        // 3. Reject mismatched digest with Error::ChecksumMismatch
        let wrong = Checksum::parse_digest(&format!("sha256:{}", "00".repeat(32))).unwrap();
        let err = Updater::verify_file_checksum(&archive_path, &wrong).unwrap_err();
        let su_err = err.downcast_ref::<self_update::Error>().expect("self_update::Error");
        assert!(matches!(su_err, self_update::Error::ChecksumMismatch { .. }));
    }

    #[test]
    fn extract_and_apply_staged_update_swaps_both_binaries_transactionally() {
        let tmp = tempfile::tempdir().unwrap();
        let app_dir = tmp.path();
        let bin_dir = app_dir.join("bin");
        let pending_dir = app_dir.join("updates").join("pending");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&pending_dir).unwrap();

        // Seed existing "old" binaries
        fs::write(bin_dir.join("boxxy-terminal"), b"old-terminal-bin").unwrap();
        fs::write(bin_dir.join("boxxy-agent"), b"old-agent-bin").unwrap();

        // Build a synthetic `.tar.gz` archive in memory/tempdir with `./` prefix to exercise
        // self_update 1.3.0's archive extraction.
        let tarball_path = pending_dir.join("update.tar.gz");
        {
            let tar_gz = fs::File::create(&tarball_path).unwrap();
            let enc = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
            let mut tar_builder = tar::Builder::new(enc);

            let prefix = "boxxy-terminal-nightly-linux-x86_64/bin";
            for (name, content) in [
                ("boxxy-terminal", b"new-terminal-bin".as_slice()),
                ("boxxy-agent", b"new-agent-bin".as_slice()),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar_builder
                    .append_data(&mut header, format!("{prefix}/{name}"), content)
                    .unwrap();
            }
            tar_builder.into_inner().unwrap().finish().unwrap();
        }

        Extract::from_source(&tarball_path)
            .archive(ArchiveKind::Tar(Some(Compression::Gz)))
            .extract_into(&pending_dir)
            .unwrap();
        fs::remove_file(&tarball_path).unwrap();
        fs::write(pending_dir.join(".update_date"), "2026-10-15T00:00:00Z").unwrap();

        Updater::apply_staged_update(app_dir).unwrap();

        assert_eq!(
            fs::read(bin_dir.join("boxxy-terminal")).unwrap(),
            b"new-terminal-bin"
        );
        assert_eq!(
            fs::read(bin_dir.join("boxxy-agent")).unwrap(),
            b"new-agent-bin"
        );
        assert_eq!(
            fs::read_to_string(app_dir.join(".last_update")).unwrap(),
            "2026-10-15T00:00:00Z"
        );
        assert!(!pending_dir.exists(), "pending dir should be cleaned up");
    }

    #[test]
    fn select_nightly_asset_falls_back_to_github_asset_digest() {
        let release = GithubRelease {
            tag_name: "nightly".to_string(),
            created_at: "2026-10-15T01:00:00Z".to_string(),
            published_at: Some("2026-10-15T02:00:00Z".to_string()),
            assets: vec![GithubAsset {
                name: "boxxy-terminal-nightly-linux-x86_64.tar.gz".to_string(),
                url: "https://api.github.com/repos/boxxy-dev/boxxy/releases/assets/1".to_string(),
                browser_download_url: Some(
                    "https://example.com/boxxy-terminal-nightly-linux-x86_64.tar.gz".to_string(),
                ),
                digest: Some(
                    "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                        .to_string(),
                ),
            }],
        };

        let (version, date, url, checksum) =
            Updater::select_nightly_asset(&release, "x86_64").unwrap();
        assert_eq!(version, "nightly");
        assert_eq!(date, "2026-10-15T02:00:00Z");
        assert_eq!(
            url,
            "https://example.com/boxxy-terminal-nightly-linux-x86_64.tar.gz"
        );
        assert_eq!(
            checksum.as_deref(),
            Some("sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }
}
