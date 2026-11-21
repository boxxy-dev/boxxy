//! Character-scoped `generate_image` tool backed by the active character's [`ModelProvider`].
//!
//! Writes generated image bytes atomically under `<config_dir>/boxxyclaw/generated/` so `boxxy-viewer`
//! renders them inline in the Claw overlay and across `/resume` without storing base64 blobs in
//! `SessionState.history`. When `output_path` is explicitly requested by the user, enforces
//! `BLACKLIST.md` and `ClawApprovalHandler::propose_file_write` (`DiffKind::Binary`).

use crate::engine::session::SessionState;
use crate::engine::tools::ClawApprovalHandler;
use base64::Engine as _;
use boxxy_ai_core::{AiCredentials, ImageAspectRatio, ImageGenParams, generate_image_with_base_url};
use boxxy_claw_protocol::FileDiff;
use boxxy_core_toolbox::ApprovalHandler as _;
use boxxy_core_toolbox::blacklist::{is_path_blacklisted, load_blacklist};
use boxxy_core_toolbox::utils::resolve_path;
use boxxy_model_selection::ModelProvider;
use rig::message::ImageMediaType;
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const MAX_REF_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SNAPSHOT_TOTAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_PREV_TEXT_BACKUP_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTION_CHARS: usize = 64;
const MAX_TRACE_PROMPT_CHARS: usize = 120;

#[derive(Debug, Deserialize)]
pub struct GenerateImageArgs {
    pub prompt: String,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub reference_image_path: Option<String>,
    #[serde(default)]
    pub use_attached_image: Option<bool>,
    #[serde(default)]
    pub output_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GenerateImageOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub markdown: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GenerateImageOutput {
    fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            path: String::new(),
            prompt: String::new(),
            markdown: String::new(),
            model: String::new(),
            message: String::new(),
            error: Some(error.into()),
        }
    }
}

pub struct GenerateImageTool {
    pub provider: ModelProvider,
    pub creds: AiCredentials,
    pub current_dir: String,
    pub state: Arc<Mutex<SessionState>>,
    pub approval: Arc<ClawApprovalHandler>,
    pub base_url_override: Option<String>,
    pub generated_dir_override: Option<PathBuf>,
}

/// Formats a CommonMark-safe `![caption](path)` image tag for `boxxy-viewer`.
#[must_use]
pub fn format_markdown_image(caption: &str, path: &str) -> String {
    let clean_caption: String = caption
        .chars()
        .map(|c| match c {
            '[' | ']' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let effective_caption = if clean_caption.is_empty() {
        "Generated image"
    } else {
        clean_caption.as_str()
    };

    if path.contains([' ', '(', ')', '<', '>']) {
        format!("![{effective_caption}](<{path}>)")
    } else {
        format!("![{effective_caption}]({path})")
    }
}

fn build_short_caption(prompt: &str) -> String {
    let collapsed: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_CAPTION_CHARS {
        collapsed
    } else {
        let mut s: String = collapsed.chars().take(MAX_CAPTION_CHARS).collect();
        s.push('…');
        s
    }
}

fn build_filename_slug(prompt: &str) -> String {
    let mut words = Vec::with_capacity(5);
    let mut current = String::new();
    for ch in prompt.chars() {
        if ch.is_ascii_alphanumeric() {
            if current.len() < 16 {
                current.push(ch.to_ascii_lowercase());
            }
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
            if words.len() >= 4 {
                break;
            }
        }
    }
    if !current.is_empty() && words.len() < 4 {
        words.push(current);
    }
    if words.is_empty() {
        "image".to_string()
    } else {
        words.join("_")
    }
}

fn sniff_b64_media_type(b64: &str) -> ImageMediaType {
    let trimmed = b64.trim();
    let take = trimmed.len().min(16) & !3;
    let mut buf = [0_u8; 12];
    if take >= 4
        && let Ok(n) = base64::engine::general_purpose::STANDARD
            .decode_slice(&trimmed.as_bytes()[..take], &mut buf)
        && let Some(media) = ImageMediaType::sniff(&buf[..n])
    {
        return media;
    }
    ImageMediaType::PNG
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
        .unwrap_or("generated.img");
    let nonce = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let tmp_path = target.with_file_name(format!(".{file_name}.{nonce}.tmp"));
    tokio::fs::write(&tmp_path, bytes).await?;
    if let Err(rename_err) = tokio::fs::rename(&tmp_path, target).await {
        // Fallback if target is on a filesystem that rejects atomic rename
        let write_res = tokio::fs::write(target, bytes).await;
        let _ = tokio::fs::remove_file(&tmp_path).await;
        write_res.or(Err(rename_err))?;
    }
    Ok(())
}

impl GenerateImageTool {
    fn effective_generated_dir(&self) -> PathBuf {
        self.generated_dir_override.clone().unwrap_or_else(|| {
            boxxy_sys_utils::get_config_dir()
                .join("boxxyclaw")
                .join("generated")
        })
    }

    async fn load_attached_turn_images(&self) -> Vec<(String, ImageMediaType)> {
        let attached = self.state.lock().await.current_turn_user_images.clone();
        attached
            .into_iter()
            .filter(|b| !b.trim().is_empty())
            .map(|b64| {
                let media = sniff_b64_media_type(&b64);
                (b64, media)
            })
            .collect()
    }

    async fn resolve_reference_images(
        &self,
        args: &GenerateImageArgs,
        base_dir: &str,
        blacklist_rules: &[String],
    ) -> Result<Vec<(String, ImageMediaType)>, String> {
        if let Some(raw_ref) = args
            .reference_image_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if raw_ref.eq_ignore_ascii_case("attached")
                || raw_ref.eq_ignore_ascii_case("clipboard")
            {
                let attached = self.load_attached_turn_images().await;
                if attached.is_empty() {
                    return Err(
                        "No attached image was found in the current user message.".to_string()
                    );
                }
                return Ok(attached);
            }

            let resolved_ref = resolve_path(base_dir, raw_ref);
            let ref_path = Path::new(&resolved_ref);
            if is_path_blacklisted(ref_path, blacklist_rules) {
                return Err(format!(
                    "Access denied: reference image path `{resolved_ref}` is protected by BLACKLIST.md."
                ));
            }

            let meta = tokio::fs::metadata(ref_path)
                .await
                .map_err(|e| format!("Cannot read reference image `{resolved_ref}`: {e}"))?;
            if !meta.is_file() {
                return Err(format!("Reference image `{resolved_ref}` is not a regular file."));
            }
            if meta.len() > MAX_REF_IMAGE_BYTES {
                return Err(format!(
                    "Reference image `{resolved_ref}` exceeds the 20 MiB size limit ({} bytes).",
                    meta.len()
                ));
            }

            let bytes = tokio::fs::read(ref_path)
                .await
                .map_err(|e| format!("Failed to read reference image `{resolved_ref}`: {e}"))?;
            let Some(media_type) = ImageMediaType::sniff(&bytes) else {
                return Err(format!(
                    "Reference file `{resolved_ref}` is not a supported image format (PNG, JPEG, WebP, or GIF)."
                ));
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(vec![(b64, media_type)]);
        }

        if args.use_attached_image == Some(true) {
            let attached = self.load_attached_turn_images().await;
            if attached.is_empty() {
                return Err(
                    "`use_attached_image` was set to true, but no image was attached in the current message."
                        .to_string(),
                );
            }
            return Ok(attached);
        }

        Ok(Vec::new())
    }
}

impl Tool for GenerateImageTool {
    const NAME: &'static str = "generate_image";

    type Error = std::io::Error;
    type Args = GenerateImageArgs;
    type Output = GenerateImageOutput;

    fn description(&self) -> String {
        let ref_note = if matches!(
            self.provider,
            ModelProvider::Gemini(_, _) | ModelProvider::OpenRouter(_)
        ) {
            " Supports editing/restyling existing images via `reference_image_path` or `use_attached_image: true`."
        } else {
            " Supports text-to-image generation."
        };
        format!(
            "Generate an image using your configured {} provider and save it locally so it renders inline in the Claw overlay.{ref_note} \
             Always include the returned `markdown` field (`![caption](/abs/path)`) in your final reply so the user sees the rendered image card. \
             Leave `output_path` unset unless the user explicitly asked to save the image to a specific workspace file path.",
            self.provider.provider_name()
        )
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Detailed visual description of the image to generate (subject, style, lighting, composition, colors, and any text to render)."
                },
                "aspect_ratio": {
                    "type": "string",
                    "enum": ["1:1", "16:9", "9:16", "4:3", "3:4"],
                    "description": "Desired aspect ratio. Defaults to '1:1' (square). Use '16:9' for landscapes/wallpapers/banners and '9:16' for vertical/mobile."
                },
                "reference_image_path": {
                    "type": "string",
                    "description": "Optional path to an existing local image file (.png, .jpg, .webp, .gif) to edit, restyle, or use as visual reference (supported on Gemini and OpenRouter)."
                },
                "use_attached_image": {
                    "type": "boolean",
                    "description": "Set to true when editing or restyling an image that the user pasted or attached directly in the current message (supported on Gemini and OpenRouter)."
                },
                "output_path": {
                    "type": "string",
                    "description": "ONLY set this when the user explicitly asked to save the image to a specific workspace/filesystem path (e.g. './assets/logo.png'). Omit for normal overlay image generation."
                }
            },
            "required": ["prompt"]
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

        let trimmed_prompt = args.prompt.trim();
        if trimmed_prompt.is_empty() {
            let out = GenerateImageOutput::failure("`prompt` must not be empty.");
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

        // Pre-validate `output_path` against BLACKLIST.md before spending image API credits.
        let resolved_output_path = match args
            .output_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw_target) => {
                let target = resolve_path(&base_dir, raw_target);
                if is_path_blacklisted(Path::new(&target), &blacklist) {
                    let out = GenerateImageOutput::failure(format!(
                        "Access denied: output path `{target}` is protected by BLACKLIST.md."
                    ));
                    self.approval.finish_tool(Self::NAME, &out).await;
                    return Ok(out);
                }
                Some(target)
            }
            None => None,
        };

        let reference_images =
            match self.resolve_reference_images(&args, &base_dir, &blacklist).await {
                Ok(imgs) => imgs,
                Err(err) => {
                    let out = GenerateImageOutput::failure(err);
                    self.approval.finish_tool(Self::NAME, &out).await;
                    return Ok(out);
                }
            };

        let aspect_ratio = ImageAspectRatio::from_str_lenient(args.aspect_ratio.as_deref());
        let start = std::time::Instant::now();
        let generated = match generate_image_with_base_url(
            &self.provider,
            &self.creds,
            ImageGenParams {
                prompt: trimmed_prompt.to_string(),
                aspect_ratio,
                reference_images,
            },
            self.base_url_override.as_deref(),
        )
        .await
        {
            Ok(img) => img,
            Err(err) => {
                let out = GenerateImageOutput::failure(err);
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }
        };
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Record telemetry and usage immediately since the provider call succeeded.
        let (in_tok, out_tok, total_tok, cost_usd) =
            generated.usage.as_ref().map_or((0, 0, 0, 0.0), |u| {
                let input = u.input_tokens.unwrap_or(0);
                let output = u.output_tokens.unwrap_or(0);
                let total = u.total_tokens.unwrap_or_else(|| input.saturating_add(output));
                let cost = u
                    .cost
                    .filter(|c| c.total.is_finite() && c.total > 0.0)
                    .map_or(0.0, |c| c.total);
                (input, output, total, cost)
            });

        boxxy_telemetry::track_ai_turn(
            self.provider.provider_name(),
            &generated.model_used,
            "claw_image",
            duration_ms,
            in_tok,
            out_tok,
        )
        .await;

        if total_tok > 0 || cost_usd > 0.0 {
            let mut state_lock = self.state.lock().await;
            state_lock.total_tokens = state_lock.total_tokens.saturating_add(total_tok);
            state_lock.total_cost_usd += cost_usd;
        }

        // 1. Always persist generated bytes atomically under `<config_dir>/boxxyclaw/generated/`
        let gen_dir = self.effective_generated_dir();
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let slug = build_filename_slug(trimmed_prompt);
        let short_uuid = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let ext = generated.media_type.extension();
        let generated_path = gen_dir.join(format!("{timestamp}_{slug}_{short_uuid}.{ext}"));

        if let Err(e) = write_bytes_atomic(&generated_path, &generated.bytes).await {
            let out = GenerateImageOutput::failure(format!(
                "Failed to save generated image to `{}`: {e}",
                generated_path.display()
            ));
            self.approval.finish_tool(Self::NAME, &out).await;
            return Ok(out);
        }

        // 2. Optional explicit workspace `output_path` (approval-gated via `propose_file_write`)
        let final_path = if let Some(target_path) = resolved_output_path {
            let diff = FileDiff::binary_with_previews(
                &target_path,
                vec![(
                    build_short_caption(trimmed_prompt),
                    generated_path.to_string_lossy().into_owned(),
                )],
            );
            let preview_note = format!(
                "[Binary image: {} bytes ({ext}, model: {})]",
                generated.bytes.len(),
                generated.model_used
            );
            self.approval.set_thinking(false).await;
            let outcome = self
                .approval
                .propose_file_write(target_path.clone(), preview_note, diff)
                .await;
            self.approval.set_thinking(true).await;

            if !outcome.is_approved() {
                let _ = tokio::fs::remove_file(&generated_path).await;
                let out = GenerateImageOutput::failure(outcome.rejection_message());
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }

            let target_fs_path = Path::new(&target_path);
            let existed_before = tokio::fs::metadata(target_fs_path).await.is_ok();
            let prev_utf8 = if existed_before {
                tokio::fs::read(target_fs_path)
                    .await
                    .ok()
                    .filter(|b| b.len() <= MAX_PREV_TEXT_BACKUP_BYTES)
                    .and_then(|b| String::from_utf8(b).ok())
            } else {
                None
            };

            if let Err(e) = write_bytes_atomic(target_fs_path, &generated.bytes).await {
                let out = GenerateImageOutput::failure(format!(
                    "Failed to write image to `{target_path}`: {e}"
                ));
                self.approval.finish_tool(Self::NAME, &out).await;
                return Ok(out);
            }

            // Record `/undo` snapshot for newly created files (`None` -> delete on `/undo`)
            // or small overwritten UTF-8 files (`Some(prev)` -> restore on `/undo`).
            if !existed_before || prev_utf8.is_some() {
                let mut state_lock = self.state.lock().await;
                if !state_lock
                    .current_turn_snapshots
                    .iter()
                    .any(|(p, _)| p == &target_path)
                {
                    let used: usize = state_lock
                        .current_turn_snapshots
                        .iter()
                        .map(|(_, c)| c.as_ref().map_or(0, String::len))
                        .sum();
                    let add = prev_utf8.as_ref().map_or(0, String::len);
                    if used.saturating_add(add) <= MAX_SNAPSHOT_TOTAL_BYTES {
                        state_lock
                            .current_turn_snapshots
                            .push((target_path.clone(), prev_utf8));
                    }
                }
            }

            target_path
        } else {
            generated_path.to_string_lossy().into_owned()
        };

        let short_caption = build_short_caption(trimmed_prompt);
        let trace_prompt: String = if trimmed_prompt.chars().count() <= MAX_TRACE_PROMPT_CHARS {
            trimmed_prompt.to_string()
        } else {
            let mut s: String = trimmed_prompt.chars().take(MAX_TRACE_PROMPT_CHARS).collect();
            s.push('…');
            s
        };
        let markdown = format_markdown_image(&short_caption, &final_path);

        self.state
            .lock()
            .await
            .generated_images_this_turn
            .push((short_caption, final_path.clone()));

        let out = GenerateImageOutput {
            success: true,
            path: final_path,
            prompt: trace_prompt,
            markdown,
            model: generated.model_used,
            message: "Image generated and saved. Include the exact `markdown` field in your response so the image displays inline in the overlay."
                .to_string(),
            error: None,
        };

        self.approval.finish_tool(Self::NAME, &out).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ClawMessage;
    use crate::engine::session::ClawSession;
    use boxxy_claw_protocol::ClawEngineEvent;
    use boxxy_model_selection::GeminiModel;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn synthetic_png_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([64, 160, 240]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    async fn spawn_mock_gemini_image_server(png: &[u8]) -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let resp_body = serde_json::to_vec(&json!({
            "modelVersion": "gemini-3.1-flash-image",
            "candidates": [{
                "content": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": b64
                        }
                    }]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 50,
                "candidatesTokenCount": 1000,
                "totalTokenCount": 1050
            }
        }))
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0_u8; 4096];
                let _ = socket.read(&mut buf).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    resp_body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&resp_body).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    fn make_test_tool(
        base_url: Option<String>,
        gen_dir: PathBuf,
        cwd: &Path,
    ) -> (
        GenerateImageTool,
        Arc<Mutex<SessionState>>,
        async_channel::Receiver<ClawEngineEvent>,
        async_channel::Sender<ClawMessage>,
    ) {
        let (session, tx_self, rx_ui) = ClawSession::new("pane-test".to_string());
        let state = session.state.clone();
        {
            let mut lock = state.try_lock().unwrap();
            lock.current_dir = cwd.to_string_lossy().into_owned();
            lock.agent_name = "Levi Kujo".to_string();
            lock.character_id = "id-levi".to_string();
        }
        let approval = Arc::new(ClawApprovalHandler {
            tx_ui: session.tx_ui.clone(),
            state: state.clone(),
            db: session.db.clone(),
            session_id: session.session_id,
            pane_id: "pane-test".to_string(),
        });
        let mut keys = HashMap::new();
        keys.insert("Gemini".to_string(), "gem-synthetic-key".to_string());
        let tool = GenerateImageTool {
            provider: ModelProvider::Gemini(GeminiModel::Flash3_8, None),
            creds: AiCredentials::new(keys, String::new()),
            current_dir: cwd.to_string_lossy().into_owned(),
            state: state.clone(),
            approval,
            base_url_override: base_url,
            generated_dir_override: Some(gen_dir),
        };
        (tool, state, rx_ui, tx_self)
    }

    #[tokio::test]
    async fn generates_image_into_config_generated_dir_and_records_turn_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let gen_dir = tmp.path().join("boxxyclaw").join("generated");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let png = synthetic_png_bytes();
        let mock_url = spawn_mock_gemini_image_server(&png).await;
        let (tool, state, _rx_ui, _tx_self) =
            make_test_tool(Some(mock_url), gen_dir.clone(), &workspace_dir);

        let mut ctx = ToolContext::default();
        let out = tool
            .call(
                &mut ctx,
                GenerateImageArgs {
                    prompt: "A geometric blue cube".to_string(),
                    aspect_ratio: Some("16:9".to_string()),
                    reference_image_path: None,
                    use_attached_image: None,
                    output_path: None,
                },
            )
            .await
            .unwrap();

        assert!(out.success, "expected success, got: {out:?}");
        assert!(Path::new(&out.path).starts_with(&gen_dir));
        assert!(
            Path::new(&out.path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        );
        assert_eq!(std::fs::read(&out.path).unwrap(), png);
        assert_eq!(out.markdown, format!("![A geometric blue cube]({})", out.path));

        let (gen_count, recorded_path, total_tokens, total_cost) = {
            let lock = state.lock().await;
            (
                lock.generated_images_this_turn.len(),
                lock.generated_images_this_turn[0].1.clone(),
                lock.total_tokens,
                lock.total_cost_usd,
            )
        };
        assert_eq!(gen_count, 1);
        assert_eq!(recorded_path, out.path);
        assert_eq!(total_tokens, 1050);
        assert!(total_cost > 0.0);
    }

    #[tokio::test]
    async fn enforces_blacklist_and_approval_handler_for_explicit_output_path() {
        let tmp = tempfile::tempdir().unwrap();
        let gen_dir = tmp.path().join("boxxyclaw").join("generated");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        // 1. Blacklisted output_path must fail fast before touching network
        let (tool_bl, _state_bl, _rx_bl, _tx_bl) = make_test_tool(
            Some("http://127.0.0.1:9".to_string()),
            gen_dir.clone(),
            &workspace_dir,
        );
        let mut ctx = ToolContext::default();
        let bl_out = tool_bl
            .call(
                &mut ctx,
                GenerateImageArgs {
                    prompt: "A geometric blue cube".to_string(),
                    aspect_ratio: None,
                    reference_image_path: None,
                    use_attached_image: None,
                    output_path: Some("/etc/shadow".to_string()),
                },
            )
            .await
            .unwrap();
        assert!(!bl_out.success);
        assert!(bl_out.error.as_deref().unwrap().contains("BLACKLIST.md"));

        // 2. Explicit output_path prompts via ProposeFileWrite (Binary diff) and records /undo snapshot on approval
        let png = synthetic_png_bytes();
        let mock_url = spawn_mock_gemini_image_server(&png).await;
        let (tool_ok, state_ok, rx_ui, _tx_ok) =
            make_test_tool(Some(mock_url), gen_dir.clone(), &workspace_dir);

        let state_approver = state_ok.clone();
        tokio::spawn(async move {
            while let Ok(ev) = rx_ui.recv().await {
                if let ClawEngineEvent::ProposeFileWrite { diff, .. } = ev {
                    assert_eq!(
                        diff.as_ref().map(|d| d.kind),
                        Some(boxxy_claw_protocol::DiffKind::Binary)
                    );
                    let reply_opt = state_approver.lock().await.pending_file_reply.take();
                    if let Some(reply_tx) = reply_opt {
                        let _ = reply_tx.send(boxxy_core_toolbox::ApprovalOutcome::Approved);
                    }
                    break;
                }
            }
        });

        let target_rel = "./assets/icon.png";
        let expected_target = workspace_dir.join("assets").join("icon.png");
        let ok_out = tool_ok
            .call(
                &mut ctx,
                GenerateImageArgs {
                    prompt: "A geometric blue cube".to_string(),
                    aspect_ratio: Some("1:1".to_string()),
                    reference_image_path: None,
                    use_attached_image: None,
                    output_path: Some(target_rel.to_string()),
                },
            )
            .await
            .unwrap();

        assert!(ok_out.success, "expected approved output_path write: {ok_out:?}");
        assert_eq!(Path::new(&ok_out.path), expected_target);
        assert_eq!(std::fs::read(&expected_target).unwrap(), png);

        let snapshots = state_ok.lock().await.current_turn_snapshots.clone();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(Path::new(&snapshots[0].0), expected_target);
        assert!(
            snapshots[0].1.is_none(),
            "newly created binary file should record None so /undo removes it"
        );
    }

    #[tokio::test]
    async fn rejection_on_output_path_cleans_up_and_returns_explicit_reject_token() {
        let tmp = tempfile::tempdir().unwrap();
        let gen_dir = tmp.path().join("boxxyclaw").join("generated");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let png = synthetic_png_bytes();
        let mock_url = spawn_mock_gemini_image_server(&png).await;
        let (tool, state, rx_ui, _tx) =
            make_test_tool(Some(mock_url), gen_dir.clone(), &workspace_dir);

        let state_rejector = state.clone();
        tokio::spawn(async move {
            while let Ok(ev) = rx_ui.recv().await {
                if matches!(ev, ClawEngineEvent::ProposeFileWrite { .. }) {
                    let reply_opt = state_rejector.lock().await.pending_file_reply.take();
                    if let Some(reply_tx) = reply_opt {
                        let _ = reply_tx.send(boxxy_core_toolbox::ApprovalOutcome::Rejected);
                    }
                    break;
                }
            }
        });

        let mut ctx = ToolContext::default();
        let out = tool
            .call(
                &mut ctx,
                GenerateImageArgs {
                    prompt: "A geometric blue cube".to_string(),
                    aspect_ratio: None,
                    reference_image_path: None,
                    use_attached_image: None,
                    output_path: Some("rejected.png".to_string()),
                },
            )
            .await
            .unwrap();

        assert!(!out.success);
        assert!(
            out.error
                .as_deref()
                .unwrap_or_default()
                .contains("[USER_EXPLICIT_REJECT]")
        );
        assert!(!workspace_dir.join("rejected.png").exists());
        assert_eq!(state.lock().await.generated_images_this_turn.len(), 0);
    }

    #[tokio::test]
    async fn supports_local_and_attached_reference_images_and_blocks_blacklisted_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let gen_dir = tmp.path().join("boxxyclaw").join("generated");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let png = synthetic_png_bytes();
        let ref_file = workspace_dir.join("source.png");
        std::fs::write(&ref_file, &png).unwrap();

        let mut ctx = ToolContext::default();

        // 1. Blacklisted reference_image_path must be denied before network I/O
        let (tool_bl, _s_bl, _rx_bl, _tx_bl) = make_test_tool(
            Some("http://127.0.0.1:9".to_string()),
            gen_dir.clone(),
            &workspace_dir,
        );
        let bl_res = tool_bl
            .call(
                &mut ctx,
                GenerateImageArgs {
                    prompt: "Restyle image".to_string(),
                    aspect_ratio: None,
                    reference_image_path: Some("~/.ssh/id_ed25519".to_string()),
                    use_attached_image: None,
                    output_path: None,
                },
            )
            .await
            .unwrap();
        assert!(!bl_res.success);
        assert!(bl_res.error.as_deref().unwrap().contains("BLACKLIST.md"));

        // 2. Valid local reference_image_path + attached image support
        let mock_url = spawn_mock_gemini_image_server(&png).await;
        let (tool_ref, state_ref, _rx_ref, _tx_ref) =
            make_test_tool(Some(mock_url), gen_dir.clone(), &workspace_dir);
        state_ref
            .lock()
            .await
            .current_turn_user_images
            .push(base64::engine::general_purpose::STANDARD.encode(&png));

        let ref_res = tool_ref
            .call(
                &mut ctx,
                GenerateImageArgs {
                    prompt: "Restyle attached cube".to_string(),
                    aspect_ratio: Some("1:1".to_string()),
                    reference_image_path: None,
                    use_attached_image: Some(true),
                    output_path: None,
                },
            )
            .await
            .unwrap();
        assert!(ref_res.success, "expected attached image edit to succeed: {ref_res:?}");
    }
}
