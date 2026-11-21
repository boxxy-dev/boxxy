//! Skill-gated `prepare_character_assets` tool for the **Boxxy Character Creator** workflow.
//!
//! Uses `boxxy-assets` (`process_portrait_focused` and `process_character_avatar_focused`) on
//! `tokio::task::spawn_blocking` to crop and resize generated or user-supplied images into
//! exact Characters v2 `768×432` (`16:9`) lossless `portrait.webp` and `256×256` (`1:1`)
//! `AVATAR.png` files with zero external CLI (`imagemagick`/`ffmpeg`/`python`) dependencies.

use crate::engine::session::SessionState;
use crate::engine::tools::ClawApprovalHandler;
use crate::engine::tools::image::format_markdown_image;
use base64::Engine as _;
use boxxy_assets::image::{
    MAX_INPUT_BYTES, process_character_avatar_focused, process_portrait_focused,
};
use boxxy_claw_protocol::FileDiff;
use boxxy_claw_protocol::characters::{INCOGNITO_CHARACTER_ID, INCOGNITO_CHARACTER_SLUG};
use boxxy_core_toolbox::ApprovalHandler as _;
use boxxy_core_toolbox::blacklist::{is_path_blacklisted, load_blacklist};
use boxxy_core_toolbox::utils::resolve_path;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const MAX_SLUG_LEN: usize = 64;
const MAX_SNAPSHOT_TOTAL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct PrepareCharacterAssetsArgs {
    pub slug: String,
    #[serde(default)]
    pub portrait_source_path: Option<String>,
    #[serde(default)]
    pub portrait_focus_x: Option<f32>,
    #[serde(default)]
    pub portrait_focus_y: Option<f32>,
    #[serde(default)]
    pub portrait_zoom: Option<f32>,
    #[serde(default)]
    pub avatar_source_path: Option<String>,
    #[serde(default)]
    pub avatar_focus_x: Option<f32>,
    #[serde(default)]
    pub avatar_focus_y: Option<f32>,
    #[serde(default)]
    pub avatar_zoom: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct PrepareCharacterAssetsOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub slug: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub character_dir: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub manifest_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub character_toml_path: String,
    pub existing_character: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portrait_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portrait_markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_markdown: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PrepareCharacterAssetsOutput {
    fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            slug: String::new(),
            character_dir: String::new(),
            manifest_path: String::new(),
            character_toml_path: String::new(),
            existing_character: false,
            portrait_path: None,
            avatar_path: None,
            portrait_markdown: None,
            avatar_markdown: None,
            message: String::new(),
            error: Some(error.into()),
        }
    }
}

pub struct PrepareCharacterAssetsTool {
    pub current_dir: String,
    pub state: Arc<Mutex<SessionState>>,
    pub approval: Arc<ClawApprovalHandler>,
    pub characters_dir_override: Option<PathBuf>,
}

/// Validates and normalizes a character folder slug to lowercase kebab-case (`[a-z0-9-]`).
///
/// # Errors
/// Returns a descriptive message if `raw` is empty, exceeds 64 chars, contains path/invalid
/// characters, or matches the reserved `/incognito` slug.
pub fn validate_character_slug(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("`slug` must not be empty.".to_string());
    }
    if trimmed.len() > MAX_SLUG_LEN {
        return Err(format!(
            "`slug` exceeds maximum length of {MAX_SLUG_LEN} characters."
        ));
    }
    if trimmed.contains(['/', '\\', '.', '\0']) {
        return Err(format!(
            "Invalid `slug` '{trimmed}': must be a lowercase kebab-case name (e.g. 'sora-dev') without path separators or dots."
        ));
    }

    let normalized = trimmed.to_ascii_lowercase();
    if normalized == INCOGNITO_CHARACTER_SLUG || normalized == INCOGNITO_CHARACTER_ID {
        return Err(format!(
            "`{normalized}` is reserved for Boxxy's `/incognito` mode; choose a different slug."
        ));
    }

    let bytes = normalized.as_bytes();
    let starts_and_ends_alnum = bytes
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric);
    let chars_valid = normalized
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !starts_and_ends_alnum || !chars_valid || normalized.contains("--") {
        return Err(format!(
            "Invalid `slug` '{trimmed}': use lowercase ASCII letters, digits, and single hyphens (e.g. 'sora-dev')."
        ));
    }

    Ok(normalized)
}

async fn write_bytes_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("character_asset");
    let nonce = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let tmp_path = target.with_file_name(format!(".{file_name}.{nonce}.tmp"));
    tokio::fs::write(&tmp_path, bytes).await?;
    if let Err(rename_err) = tokio::fs::rename(&tmp_path, target).await {
        let write_res = tokio::fs::write(target, bytes).await;
        let _ = tokio::fs::remove_file(&tmp_path).await;
        write_res.or(Err(rename_err))?;
    }
    Ok(())
}

enum ResolvedSource {
    Attached,
    File(String),
}

impl PrepareCharacterAssetsTool {
    fn effective_characters_dir(&self) -> Result<PathBuf, String> {
        if let Some(ref custom) = self.characters_dir_override {
            return Ok(custom.clone());
        }
        boxxy_claw_protocol::character_loader::get_characters_dir()
            .map_err(|e| format!("Failed to resolve Boxxy characters directory: {e}"))
    }

    fn classify_source(
        raw: &str,
        base_dir: &str,
        blacklist_rules: &[String],
    ) -> Result<ResolvedSource, String> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("attached") || trimmed.eq_ignore_ascii_case("clipboard") {
            return Ok(ResolvedSource::Attached);
        }
        let resolved = resolve_path(base_dir, trimmed);
        if is_path_blacklisted(Path::new(&resolved), blacklist_rules) {
            return Err(format!(
                "Access denied: image source `{resolved}` is protected by BLACKLIST.md."
            ));
        }
        Ok(ResolvedSource::File(resolved))
    }

    async fn load_source_bytes(&self, src: &ResolvedSource) -> Result<Arc<[u8]>, String> {
        match src {
            ResolvedSource::Attached => {
                let first_b64 = self
                    .state
                    .lock()
                    .await
                    .current_turn_user_images
                    .iter()
                    .find(|b| !b.trim().is_empty())
                    .cloned()
                    .ok_or_else(|| {
                        "No attached image was found in the current user message.".to_string()
                    })?;
                let compact: String = first_b64
                    .chars()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect();
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(compact)
                    .map_err(|e| format!("Failed to decode attached image: {e}"))?;
                if decoded.len() > MAX_INPUT_BYTES {
                    return Err(format!(
                        "Attached image exceeds the 20 MiB limit ({} bytes).",
                        decoded.len()
                    ));
                }
                Ok(Arc::from(decoded))
            }
            ResolvedSource::File(resolved) => {
                let path = Path::new(resolved);
                let meta = tokio::fs::metadata(path)
                    .await
                    .map_err(|e| format!("Cannot read image `{resolved}`: {e}"))?;
                if !meta.is_file() {
                    return Err(format!("Image source `{resolved}` is not a regular file."));
                }
                if meta.len() > MAX_INPUT_BYTES as u64 {
                    return Err(format!(
                        "Image `{resolved}` exceeds the 20 MiB limit ({} bytes).",
                        meta.len()
                    ));
                }
                let bytes = tokio::fs::read(path)
                    .await
                    .map_err(|e| format!("Failed to read image `{resolved}`: {e}"))?;
                Ok(Arc::from(bytes))
            }
        }
    }
}

impl Tool for PrepareCharacterAssetsTool {
    const NAME: &'static str = "prepare_character_assets";

    type Error = std::io::Error;
    type Args = PrepareCharacterAssetsArgs;
    type Output = PrepareCharacterAssetsOutput;

    fn description(&self) -> String {
        "Resolve the active Boxxy character package directory (`<config_dir>/boxxyclaw/characters/<slug>`) \
         and optionally crop & resize source artwork into canonical Characters v2 assets: \
         `portrait.webp` (768x432 16:9 lossless WebP) and `AVATAR.png` (256x256 1:1 PNG). \
         Pass the same widescreen illustration to both `portrait_source_path` and `avatar_source_path` \
         (with `avatar_focus_y` ~0.28 and `avatar_zoom` ~1.5) to frame a consistent close-up headshot."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "slug": {
                    "type": "string",
                    "description": "Lowercase kebab-case folder slug for the character (e.g. 'sora-dev'). Never use 'inco-gnito'."
                },
                "portrait_source_path": {
                    "type": "string",
                    "description": "Optional path to a local image (e.g. returned by `generate_image` or `@file.png`), or 'attached' for a message-attached image, to crop/resize to 768x432 (16:9) `portrait.webp`."
                },
                "portrait_focus_x": {
                    "type": "number",
                    "description": "Horizontal focal center (0.0 = left, 0.5 = center, 1.0 = right) for the 16:9 portrait crop. Defaults to 0.5."
                },
                "portrait_focus_y": {
                    "type": "number",
                    "description": "Vertical focal center (0.0 = top, 0.5 = center, 1.0 = bottom) for the 16:9 portrait crop. Defaults to 0.5."
                },
                "portrait_zoom": {
                    "type": "number",
                    "description": "Zoom factor >= 1.0 for the 16:9 portrait crop (1.0 = largest 16:9 fit). Defaults to 1.0."
                },
                "avatar_source_path": {
                    "type": "string",
                    "description": "Optional path to a local image (can be the same path as `portrait_source_path`, a separate 1:1 image, or 'attached') to crop/resize to 256x256 (1:1) `AVATAR.png`."
                },
                "avatar_focus_x": {
                    "type": "number",
                    "description": "Horizontal focal center (0.0..=1.0) for the 1:1 avatar crop. Defaults to 0.5."
                },
                "avatar_focus_y": {
                    "type": "number",
                    "description": "Vertical focal center (0.0 = top, 0.5 = center, 1.0 = bottom) for the 1:1 avatar crop. Use ~0.25-0.35 when framing a face out of a waist-up portrait."
                },
                "avatar_zoom": {
                    "type": "number",
                    "description": "Zoom factor >= 1.0 for the 1:1 avatar crop (1.0 = full square fit; ~1.4-1.8 zooms in tightly on the character's head & shoulders)."
                }
            },
            "required": ["slug"]
        })
    }

    fn annotations(&self) -> rig::completion::ToolAnnotations {
        rig::completion::ToolAnnotations::default().with_read_only(false)
    }

    #[allow(clippy::too_many_lines)]
    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        self.approval.begin_tool(Self::NAME).await;

        let slug = match validate_character_slug(&args.slug) {
            Ok(s) => s,
            Err(err) => {
                let out = PrepareCharacterAssetsOutput::failure(err);
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let chars_root = match self.effective_characters_dir() {
            Ok(dir) => dir,
            Err(err) => {
                let out = PrepareCharacterAssetsOutput::failure(err);
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let char_dir = chars_root.join(&slug);
        let existing_character = tokio::fs::metadata(char_dir.join("CHARACTER.toml"))
            .await
            .is_ok_and(|m| m.is_file());
        let char_dir_str = char_dir.to_string_lossy().into_owned();
        let manifest_path = char_dir.join("manifest.toml").to_string_lossy().into_owned();
        let character_toml_path = char_dir
            .join("CHARACTER.toml")
            .to_string_lossy()
            .into_owned();

        let raw_portrait_src = args
            .portrait_source_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let raw_avatar_src = args
            .avatar_source_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        // Path-discovery fast path when no source images are supplied.
        if raw_portrait_src.is_none() && raw_avatar_src.is_none() {
            let dir_hint = if existing_character {
                "Existing character directory found."
            } else {
                "New character directory (no existing manifest.toml or CHARACTER.toml; write both directly with `file_write` without calling `list_directory`)."
            };
            let out = PrepareCharacterAssetsOutput {
                success: true,
                slug,
                character_dir: char_dir_str,
                manifest_path,
                character_toml_path,
                existing_character,
                portrait_path: None,
                avatar_path: None,
                portrait_markdown: None,
                avatar_markdown: None,
                message: format!("Resolved character package paths. {dir_hint}"),
                error: None,
            };
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        let base_dir = self
            .approval
            .current_dir()
            .await
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.current_dir.clone());
        let blacklist = load_blacklist();

        let portrait_src = match raw_portrait_src {
            Some(raw) => match Self::classify_source(raw, &base_dir, &blacklist) {
                Ok(s) => Some(s),
                Err(err) => {
                    let out = PrepareCharacterAssetsOutput::failure(err);
                    self.approval.finish_tool(Self::NAME, &out).await;
                    return Ok(out);
                }
            },
            None => None,
        };

        let avatar_src = match raw_avatar_src {
            Some(raw) => match Self::classify_source(raw, &base_dir, &blacklist) {
                Ok(s) => Some(s),
                Err(err) => {
                    let out = PrepareCharacterAssetsOutput::failure(err);
                    self.approval.finish_tool(Self::NAME, &out).await;
                    return Ok(out);
                }
            },
            None => None,
        };

        let same_source = matches!(
            (&portrait_src, &avatar_src),
            (Some(ResolvedSource::Attached), Some(ResolvedSource::Attached))
        ) || matches!(
            (&portrait_src, &avatar_src),
            (Some(ResolvedSource::File(a)), Some(ResolvedSource::File(b))) if a == b
        );

        let portrait_input: Option<Arc<[u8]>> = match portrait_src.as_ref() {
            Some(src) => match self.load_source_bytes(src).await {
                Ok(b) => Some(b),
                Err(err) => {
                    let out = PrepareCharacterAssetsOutput::failure(err);
                    self.approval.finish_tool(Self::NAME, &out).await;
                    return Ok(out);
                }
            },
            None => None,
        };

        let avatar_input: Option<Arc<[u8]>> = if same_source {
            portrait_input.clone()
        } else {
            match avatar_src.as_ref() {
                Some(src) => match self.load_source_bytes(src).await {
                    Ok(b) => Some(b),
                    Err(err) => {
                        let out = PrepareCharacterAssetsOutput::failure(err);
                        self.approval.finish_tool(Self::NAME, &out).await;
                        return Ok(out);
                    }
                },
                None => None,
            }
        };

        let p_fx = args.portrait_focus_x.unwrap_or(0.5);
        let p_fy = args.portrait_focus_y.unwrap_or(0.5);
        let p_zoom = args.portrait_zoom.unwrap_or(1.0);
        let a_fx = args.avatar_focus_x.unwrap_or(0.5);
        let a_fy = args.avatar_focus_y.unwrap_or(0.5);
        let a_zoom = args.avatar_zoom.unwrap_or(1.0);

        type EncodedCharacterPair = Result<(Option<Vec<u8>>, Option<Vec<u8>>), String>;
        let encoded = tokio::task::spawn_blocking(move || -> EncodedCharacterPair {
            let webp = match portrait_input {
                Some(raw) => Some(
                    process_portrait_focused(&raw, p_fx, p_fy, p_zoom)
                        .map_err(|e| format!("Failed to process 16:9 portrait: {e}"))?
                        .webp_bytes,
                ),
                None => None,
            };
            let png = match avatar_input {
                Some(raw) => Some(
                    process_character_avatar_focused(&raw, a_fx, a_fy, a_zoom)
                        .map_err(|e| format!("Failed to process 1:1 avatar: {e}"))?
                        .png_bytes,
                ),
                None => None,
            };
            Ok((webp, png))
        })
        .await
        .map_err(|e| std::io::Error::other(format!("Asset worker panicked: {e}")))?;

        let (portrait_bytes_opt, avatar_bytes_opt) = match encoded {
            Ok(pair) => pair,
            Err(err) => {
                let out = PrepareCharacterAssetsOutput::failure(err);
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
        };

        let portrait_fs_path = char_dir.join("portrait.webp");
        let avatar_fs_path = char_dir.join("AVATAR.png");
        let primary_target = if portrait_bytes_opt.is_some() {
            portrait_fs_path.to_string_lossy().into_owned()
        } else {
            avatar_fs_path.to_string_lossy().into_owned()
        };

        let mut summary_parts = Vec::with_capacity(2);
        if let Some(ref wb) = portrait_bytes_opt {
            summary_parts.push(format!("portrait.webp (768x432 WebP, {} bytes)", wb.len()));
        }
        if let Some(ref pb) = avatar_bytes_opt {
            summary_parts.push(format!("AVATAR.png (256x256 PNG, {} bytes)", pb.len()));
        }
        let preview_note = format!(
            "[Characters v2 artwork for '{slug}': {}]",
            summary_parts.join(" + ")
        );

        // Stage cropped preview files under `<boxxyclaw>/generated/` so the user can visually
        // inspect BOTH the 16:9 portrait and 1:1 avatar right inside the `Approve & Write` card
        // (and steer with a new prompt if they want changes before writing to `characters/<slug>/`).
        let gen_dir = chars_root
            .parent()
            .unwrap_or(&chars_root)
            .join("generated");
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let short_uuid = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let mut staged_paths: Vec<PathBuf> = Vec::with_capacity(2);
        let mut preview_images: Vec<(String, String)> = Vec::with_capacity(2);

        if let Some(ref wb) = portrait_bytes_opt {
            let staged = gen_dir.join(format!("{timestamp}_{slug}_portrait_{short_uuid}.webp"));
            if write_bytes_atomic(&staged, wb).await.is_ok() {
                preview_images.push((
                    "portrait.webp (768×432 · 16:9)".to_string(),
                    staged.to_string_lossy().into_owned(),
                ));
                staged_paths.push(staged);
            }
        }
        if let Some(ref pb) = avatar_bytes_opt {
            let staged = gen_dir.join(format!("{timestamp}_{slug}_avatar_{short_uuid}.png"));
            if write_bytes_atomic(&staged, pb).await.is_ok() {
                preview_images.push((
                    "AVATAR.png (256×256 · 1:1)".to_string(),
                    staged.to_string_lossy().into_owned(),
                ));
                staged_paths.push(staged);
            }
        }

        let diff = FileDiff::binary_with_previews(&primary_target, preview_images);
        self.approval.set_thinking(false).await;
        let outcome = self
            .approval
            .propose_file_write(primary_target, preview_note, diff)
            .await;
        self.approval.set_thinking(true).await;

        if !outcome.is_approved() {
            for staged in staged_paths {
                let _ = tokio::fs::remove_file(staged).await;
            }
            let out = PrepareCharacterAssetsOutput::failure(outcome.rejection_message());
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        let mut written_targets: Vec<(String, bool)> = Vec::with_capacity(2);

        if let Some(ref webp_bytes) = portrait_bytes_opt {
            let existed = tokio::fs::metadata(&portrait_fs_path).await.is_ok();
            if let Err(e) = write_bytes_atomic(&portrait_fs_path, webp_bytes).await {
                let out = PrepareCharacterAssetsOutput::failure(format!(
                    "Failed to write `{}`: {e}",
                    portrait_fs_path.display()
                ));
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
            written_targets.push((portrait_fs_path.to_string_lossy().into_owned(), existed));
        }

        if let Some(ref png_bytes) = avatar_bytes_opt {
            let existed = tokio::fs::metadata(&avatar_fs_path).await.is_ok();
            if let Err(e) = write_bytes_atomic(&avatar_fs_path, png_bytes).await {
                let out = PrepareCharacterAssetsOutput::failure(format!(
                    "Failed to write `{}`: {e}",
                    avatar_fs_path.display()
                ));
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
            written_targets.push((avatar_fs_path.to_string_lossy().into_owned(), existed));
        }

        let portrait_out_str = portrait_bytes_opt
            .as_ref()
            .map(|_| portrait_fs_path.to_string_lossy().into_owned());
        let avatar_out_str = avatar_bytes_opt
            .as_ref()
            .map(|_| avatar_fs_path.to_string_lossy().into_owned());
        let portrait_md = portrait_out_str
            .as_deref()
            .map(|p| format_markdown_image(&format!("{slug} portrait (16:9)"), p));
        let avatar_md = avatar_out_str
            .as_deref()
            .map(|p| format_markdown_image(&format!("{slug} avatar (1:1)"), p));

        {
            let mut state_lock = self.state.lock().await;

            // Record `/undo` deletion snapshots for newly created binary files.
            for (target_str, existed_before) in &written_targets {
                if !*existed_before
                    && !state_lock
                        .current_turn_snapshots
                        .iter()
                        .any(|(p, _)| p == target_str)
                {
                    let used: usize = state_lock
                        .current_turn_snapshots
                        .iter()
                        .map(|(_, c)| c.as_ref().map_or(0, String::len))
                        .sum();
                    if used <= MAX_SNAPSHOT_TOTAL_BYTES {
                        state_lock
                            .current_turn_snapshots
                            .push((target_str.clone(), None));
                    }
                }
            }

            // Replace consumed raw `generate_image` paths with the cropped character assets so
            // `append_missing_generated_images` shows the final cropped artwork without duplicates.
            for src in [&portrait_src, &avatar_src].into_iter().flatten() {
                if let ResolvedSource::File(ref consumed_path) = *src {
                    state_lock
                        .generated_images_this_turn
                        .retain(|(_, p)| p != consumed_path);
                }
            }
            if let Some(ref p) = portrait_out_str {
                state_lock
                    .generated_images_this_turn
                    .push((format!("{slug} portrait (16:9)"), p.clone()));
            }
            if let Some(ref a) = avatar_out_str {
                state_lock
                    .generated_images_this_turn
                    .push((format!("{slug} avatar (1:1)"), a.clone()));
            }
            drop(state_lock);
        }

        let next_step_note = if existing_character {
            "Existing character directory detected."
        } else {
            "Brand-new character directory (no existing manifest.toml or CHARACTER.toml — write both directly with `file_write` without calling `list_directory`)."
        };
        let out = PrepareCharacterAssetsOutput {
            success: true,
            slug,
            character_dir: char_dir_str,
            manifest_path,
            character_toml_path,
            existing_character,
            portrait_path: portrait_out_str,
            avatar_path: avatar_out_str,
            portrait_markdown: portrait_md,
            avatar_markdown: avatar_md,
            message: format!(
                "Character artwork cropped and saved (`portrait.webp` 768x432, `AVATAR.png` 256x256). {next_step_note} Include ONLY `portrait_markdown` and `avatar_markdown` in your final reply (do NOT repeat the raw `generate_image` markdown)."
            ),
            error: None,
        };
        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::session::ClawSession;
    use boxxy_assets::image::{AVATAR_SIZE, PORTRAIT_HEIGHT, PORTRAIT_WIDTH, decode_character_asset_rgba};
    use boxxy_claw_protocol::ClawEngineEvent;

    fn synthetic_widescreen_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |_x, y| {
            if y < h / 2 {
                image::Rgb([220, 60, 80])
            } else {
                image::Rgb([40, 120, 220])
            }
        });
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn make_test_tool(
        chars_dir: PathBuf,
        cwd: &Path,
    ) -> (
        PrepareCharacterAssetsTool,
        Arc<Mutex<SessionState>>,
        async_channel::Receiver<ClawEngineEvent>,
    ) {
        let (session, _tx_self, rx_ui) = ClawSession::new("pane-char-test".to_string());
        let state = session.state.clone();
        {
            let mut lock = state.try_lock().unwrap();
            lock.current_dir = cwd.to_string_lossy().into_owned();
            lock.agent_name = "Niko Una".to_string();
            lock.character_id = "id-niko".to_string();
        }
        let approval = Arc::new(ClawApprovalHandler {
            tx_ui: session.tx_ui.clone(),
            state: state.clone(),
            db: session.db.clone(),
            session_id: session.session_id,
            pane_id: "pane-char-test".to_string(),
        });
        let tool = PrepareCharacterAssetsTool {
            current_dir: cwd.to_string_lossy().into_owned(),
            state: state.clone(),
            approval,
            characters_dir_override: Some(chars_dir),
        };
        (tool, state, rx_ui)
    }

    #[test]
    fn slug_validator_enforces_kebab_case_and_blocks_reserved_or_traversal_names() {
        assert_eq!(validate_character_slug("  Sora-Dev  ").unwrap(), "sora-dev");
        assert_eq!(validate_character_slug("unit01").unwrap(), "unit01");

        assert!(validate_character_slug("").is_err());
        assert!(validate_character_slug("inco-gnito").is_err());
        assert!(validate_character_slug("__incognito__").is_err());
        assert!(validate_character_slug("../escape").is_err());
        assert!(validate_character_slug("sub/char").is_err());
        assert!(validate_character_slug("-leading").is_err());
        assert!(validate_character_slug("trailing-").is_err());
        assert!(validate_character_slug("double--dash").is_err());
        assert!(validate_character_slug("has space").is_err());
    }

    #[tokio::test]
    async fn crops_single_master_image_into_exact_768x432_webp_and_256x256_png_and_replaces_turn_preview()
     {
        let tmp = tempfile::tempdir().unwrap();
        let chars_dir = tmp.path().join("boxxyclaw").join("characters");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let master_png = synthetic_widescreen_png(1536, 1024);
        let master_path = workspace_dir.join("master_render.png");
        std::fs::write(&master_path, &master_png).unwrap();
        let master_str = master_path.to_string_lossy().into_owned();

        let (tool, state, rx_ui) = make_test_tool(chars_dir.clone(), &workspace_dir);
        state
            .lock()
            .await
            .generated_images_this_turn
            .push(("Raw render".to_string(), master_str.clone()));

        let state_approver = state.clone();
        tokio::spawn(async move {
            while let Ok(ev) = rx_ui.recv().await {
                if let ClawEngineEvent::ProposeFileWrite { diff, .. } = ev {
                    let d = diff.expect("expected binary diff with staged previews");
                    assert_eq!(d.preview_images.len(), 2);
                    for (_, preview_path) in &d.preview_images {
                        assert!(
                            Path::new(preview_path).is_file(),
                            "staged preview must exist on disk before approval: {preview_path}"
                        );
                    }
                    let reply_opt = state_approver.lock().await.pending_file_reply.take();
                    if let Some(tx) = reply_opt {
                        let _ = tx.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                    break;
                }
            }
        });

        let mut ctx = ToolContext::default();
        let out = tool
            .call(
                &mut ctx,
                PrepareCharacterAssetsArgs {
                    slug: "sora-dev".to_string(),
                    portrait_source_path: Some(master_str.clone()),
                    portrait_focus_x: Some(0.5),
                    portrait_focus_y: Some(0.5),
                    portrait_zoom: Some(1.0),
                    avatar_source_path: Some(master_str.clone()),
                    avatar_focus_x: Some(0.5),
                    avatar_focus_y: Some(0.22),
                    avatar_zoom: Some(1.8),
                },
            )
            .await
            .unwrap();

        assert!(out.success, "expected success, got: {out:?}");
        let portrait_path = PathBuf::from(out.portrait_path.as_deref().unwrap());
        let avatar_path = PathBuf::from(out.avatar_path.as_deref().unwrap());
        assert_eq!(portrait_path, chars_dir.join("sora-dev").join("portrait.webp"));
        assert_eq!(avatar_path, chars_dir.join("sora-dev").join("AVATAR.png"));

        let decoded_portrait =
            decode_character_asset_rgba(&std::fs::read(&portrait_path).unwrap()).unwrap();
        assert_eq!(decoded_portrait.width, PORTRAIT_WIDTH);
        assert_eq!(decoded_portrait.height, PORTRAIT_HEIGHT);

        let decoded_avatar =
            decode_character_asset_rgba(&std::fs::read(&avatar_path).unwrap()).unwrap();
        assert_eq!(decoded_avatar.width, AVATAR_SIZE);
        assert_eq!(decoded_avatar.height, AVATAR_SIZE);
        // Upper half of synthetic source is red (`220, 60, 80`), verifying `avatar_focus_y = 0.22`
        assert!(decoded_avatar.rgba[0] > 180);

        let lock = state.lock().await;
        assert_eq!(lock.current_turn_snapshots.len(), 2);
        assert!(
            !lock
                .generated_images_this_turn
                .iter()
                .any(|(_, p)| p == &master_str),
            "consumed raw master image should be replaced by cropped character assets"
        );
        assert_eq!(lock.generated_images_this_turn.len(), 2);
    }

    #[tokio::test]
    async fn supports_attached_image_path_discovery_and_blocks_blacklisted_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let chars_dir = tmp.path().join("boxxyclaw").join("characters");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let (tool, state, rx_ui) = make_test_tool(chars_dir.clone(), &workspace_dir);
        let mut ctx = ToolContext::default();

        // 1. Path discovery fast-path without source images requires zero approvals
        let discovery = tool
            .call(
                &mut ctx,
                PrepareCharacterAssetsArgs {
                    slug: "plain-agent".to_string(),
                    portrait_source_path: None,
                    portrait_focus_x: None,
                    portrait_focus_y: None,
                    portrait_zoom: None,
                    avatar_source_path: None,
                    avatar_focus_x: None,
                    avatar_focus_y: None,
                    avatar_zoom: None,
                },
            )
            .await
            .unwrap();
        assert!(discovery.success);
        assert_eq!(
            Path::new(&discovery.character_dir),
            chars_dir.join("plain-agent")
        );

        // 2. Blacklisted source path fails fast before user prompt
        let bl = tool
            .call(
                &mut ctx,
                PrepareCharacterAssetsArgs {
                    slug: "plain-agent".to_string(),
                    portrait_source_path: Some("~/.ssh/id_ed25519".to_string()),
                    portrait_focus_x: None,
                    portrait_focus_y: None,
                    portrait_zoom: None,
                    avatar_source_path: None,
                    avatar_focus_x: None,
                    avatar_focus_y: None,
                    avatar_zoom: None,
                },
            )
            .await
            .unwrap();
        assert!(!bl.success);
        assert!(bl.error.as_deref().unwrap().contains("BLACKLIST.md"));

        // 3. Attached base64 image source crops cleanly on approval
        let attached_png = synthetic_widescreen_png(800, 600);
        state
            .lock()
            .await
            .current_turn_user_images
            .push(base64::engine::general_purpose::STANDARD.encode(&attached_png));

        let state_approver = state.clone();
        tokio::spawn(async move {
            while let Ok(ev) = rx_ui.recv().await {
                if matches!(ev, ClawEngineEvent::ProposeFileWrite { .. }) {
                    let reply_opt = state_approver.lock().await.pending_file_reply.take();
                    if let Some(tx) = reply_opt {
                        let _ = tx.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                    break;
                }
            }
        });

        let attached_out = tool
            .call(
                &mut ctx,
                PrepareCharacterAssetsArgs {
                    slug: "pasted-companion".to_string(),
                    portrait_source_path: Some("attached".to_string()),
                    portrait_focus_x: None,
                    portrait_focus_y: None,
                    portrait_zoom: None,
                    avatar_source_path: Some("attached".to_string()),
                    avatar_focus_x: None,
                    avatar_focus_y: None,
                    avatar_zoom: None,
                },
            )
            .await
            .unwrap();
        assert!(attached_out.success, "expected attached crop: {attached_out:?}");
        assert!(chars_dir.join("pasted-companion").join("portrait.webp").is_file());
        assert!(chars_dir.join("pasted-companion").join("AVATAR.png").is_file());
    }
}
