//! Unified multi-provider image generation (`generate_image_with_provider`) built on `rig-core` (`0.47`).
//!
//! Bridges character-assigned [`ModelProvider`] credentials onto native image generation endpoints
//! (`Gemini`, `OpenAI`, `xAI`, `MiniMax`, and `OpenRouter` multimodal chat completions) while
//! handling per-provider dimension, `response_format`, and reference-image wire quirks.

use crate::credentials::AiCredentials;
use base64::Engine as _;
use boxxy_model_selection::{ModelProvider, catalog_get_spec, resolve_any_catalog_spec};
use rig::catalog::ModelSpec;
use rig::completion::{Cost, Usage};
use rig::image_generation::ImageGenerationRequestBuilder;
use rig::message::{DocumentSourceKind, Image, ImageMediaType};
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::Duration;

const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_OPENROUTER_IMAGE_MODEL: &str = "google/gemini-3.1-flash-image";

static OPENROUTER_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Portable aspect ratio presets mapped to each provider's native wire dimensions or `aspect_ratio` strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageAspectRatio {
    #[default]
    Square,
    Landscape,
    Portrait,
    Standard,
    Tall,
}

impl ImageAspectRatio {
    /// Parses a user- or LLM-supplied aspect ratio string leniently, defaulting to [`Self::Square`].
    #[must_use]
    pub fn from_str_lenient(raw: Option<&str>) -> Self {
        let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Square;
        };
        match trimmed.to_ascii_lowercase().as_str() {
            "16:9" | "16x9" | "21:9" | "landscape" | "wide" | "widescreen" => Self::Landscape,
            "9:16" | "9x16" | "portrait" | "vertical" => Self::Portrait,
            "4:3" | "4x3" | "3:2" | "3x2" | "standard" => Self::Standard,
            "3:4" | "3x4" | "2:3" | "2x3" | "tall" => Self::Tall,
            _ => Self::Square,
        }
    }

    /// Canonical `"W:H"` ratio string accepted by Gemini, xAI, and `OpenRouter`.
    #[must_use]
    pub const fn as_ratio_str(self) -> &'static str {
        match self {
            Self::Square => "1:1",
            Self::Landscape => "16:9",
            Self::Portrait => "9:16",
            Self::Standard => "4:3",
            Self::Tall => "3:4",
        }
    }

    /// Integer ratio sides used by `rig`'s Gemini image request encoder.
    #[must_use]
    pub const fn gemini_dimensions(self) -> (u32, u32) {
        match self {
            Self::Square => (1, 1),
            Self::Landscape => (16, 9),
            Self::Portrait => (9, 16),
            Self::Standard => (4, 3),
            Self::Tall => (3, 4),
        }
    }

    /// Valid pixel dimensions for `OpenAI` `gpt-image-2` (`256x256` default in `rig` is rejected by `gpt-image-2`).
    #[must_use]
    pub const fn openai_dimensions(self) -> (u32, u32) {
        match self {
            Self::Square => (1024, 1024),
            Self::Landscape | Self::Standard => (1536, 1024),
            Self::Portrait | Self::Tall => (1024, 1536),
        }
    }

    /// Valid pixel dimensions for `MiniMax` `image-01` (`512..=2048` in multiples of `8`).
    #[must_use]
    pub const fn minimax_dimensions(self) -> (u32, u32) {
        match self {
            Self::Square => (1024, 1024),
            Self::Landscape => (1280, 720),
            Self::Portrait => (720, 1280),
            Self::Standard => (1152, 864),
            Self::Tall => (864, 1152),
        }
    }
}

/// Input parameters for a single image generation or image-to-image editing call.
#[derive(Debug, Clone)]
pub struct ImageGenParams {
    pub prompt: String,
    pub aspect_ratio: ImageAspectRatio,
    /// Optional `(base64_data, media_type)` reference images for image-to-image / editing.
    pub reference_images: Vec<(String, ImageMediaType)>,
}

/// Decoded image bytes and provider metadata returned by [`generate_image_with_provider`].
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub media_type: ImageMediaType,
    pub model_used: String,
    pub usage: Option<Usage>,
}

const fn media_type_mime_str(media_type: &ImageMediaType) -> &'static str {
    match media_type {
        ImageMediaType::JPEG => "image/jpeg",
        ImageMediaType::PNG => "image/png",
        ImageMediaType::GIF => "image/gif",
        ImageMediaType::WEBP => "image/webp",
        ImageMediaType::HEIC => "image/heic",
        ImageMediaType::HEIF => "image/heif",
        ImageMediaType::SVG => "image/svg+xml",
    }
}

const fn has_reported_usage(u: &Usage) -> bool {
    u.input_tokens.is_some()
        || u.output_tokens.is_some()
        || u.total_tokens.is_some()
        || u.cost.is_some()
}

fn enrich_image_usage(mut usage: Usage, spec: Option<&ModelSpec>) -> Option<Usage> {
    if !has_reported_usage(&usage) {
        return None;
    }

    if usage
        .cost
        .is_some_and(|c| !c.total.is_finite() || c.total < 0.0)
    {
        usage.cost = None;
    }

    if usage.cost.is_none()
        && let Some(pricing) = spec.and_then(|s| s.pricing.as_ref())
        && let Some(cost) = pricing
            .cost(&usage)
            .filter(|c| c.total.is_finite() && c.total >= 0.0)
    {
        usage.cost = Some(cost);
    }

    Some(usage)
}

fn finalize_generated_image(
    bytes: Vec<u8>,
    model_used: String,
    usage: Usage,
    spec: Option<&ModelSpec>,
) -> Result<GeneratedImage, String> {
    if bytes.is_empty() {
        return Err("Image generation response contained empty image bytes.".to_owned());
    }
    let media_type = ImageMediaType::sniff(&bytes).unwrap_or(ImageMediaType::PNG);
    Ok(GeneratedImage {
        bytes,
        media_type,
        model_used,
        usage: enrich_image_usage(usage, spec),
    })
}

/// Selects the `OpenRouter` model slug to use for image generation.
///
/// Uses `configured_slug` directly when its catalog entry declares `output.image == true`
/// or matches a dedicated image model family; otherwise falls back to `google/gemini-3.1-flash-image`
/// under the same `OpenRouter` API key.
#[must_use]
pub fn select_openrouter_image_model(configured_slug: &str) -> &str {
    let trimmed = configured_slug.trim();
    if trimmed.is_empty() {
        return DEFAULT_OPENROUTER_IMAGE_MODEL;
    }

    if resolve_any_catalog_spec(trimmed)
        .and_then(|spec| spec.details.output)
        .is_some_and(|out| out.image)
    {
        return trimmed;
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("-image")
        || lower.contains("/flux")
        || lower.contains("imagen")
        || lower.contains("seedream")
        || lower.contains("recraft")
    {
        return trimmed;
    }

    DEFAULT_OPENROUTER_IMAGE_MODEL
}

fn decode_data_url_or_base64(raw: &str) -> Result<Vec<u8>, String> {
    let trimmed = raw.trim();
    let payload = if let Some(rest) = trimmed.strip_prefix("data:") {
        let Some((_, b64)) = rest.split_once(";base64,") else {
            return Err("OpenRouter returned a non-base64 data URI for the generated image.".to_owned());
        };
        b64.trim()
    } else {
        trimmed
    };

    // Strip any embedded whitespace/newlines some gateways insert into base64 payloads.
    if payload.bytes().any(|b| b.is_ascii_whitespace()) {
        let compact: String = payload.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(compact)
            .map_err(|e| format!("Failed to decode base64 image payload: {e}"))
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|e| format!("Failed to decode base64 image payload: {e}"))
    }
}

fn extract_openrouter_image_data(reply: &Value) -> Option<&str> {
    let msg = reply
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("message"))?;

    if let Some(images) = msg.get("images").and_then(Value::as_array) {
        for item in images {
            if let Some(url) = item
                .get("image_url")
                .and_then(|u| u.get("url"))
                .and_then(Value::as_str)
                .or_else(|| item.get("url").and_then(Value::as_str))
                .filter(|s| !s.trim().is_empty())
            {
                return Some(url);
            }
        }
    }

    if let Some(parts) = msg.get("content").and_then(Value::as_array) {
        for part in parts {
            if let Some(url) = part
                .get("image_url")
                .and_then(|u| u.get("url"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                return Some(url);
            }
        }
    }

    None
}

fn parse_openrouter_usage(reply: &Value) -> Usage {
    let Some(u) = reply.get("usage") else {
        return Usage::default();
    };
    let input_tokens = u
        .get("prompt_tokens")
        .or_else(|| u.get("input_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = u
        .get("completion_tokens")
        .or_else(|| u.get("output_tokens"))
        .and_then(Value::as_u64);
    let total_tokens = u.get("total_tokens").and_then(Value::as_u64);
    let cost = u
        .get("cost")
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c >= 0.0)
        .map(|total| {
            let mut c = Cost::default();
            c.total = total;
            c
        });

    let mut usage = Usage::default();
    usage.input_tokens = input_tokens;
    usage.output_tokens = output_tokens;
    usage.total_tokens = total_tokens;
    usage.cost = cost;
    usage
}

async fn generate_openrouter_image(
    configured_slug: &str,
    api_key: &str,
    params: ImageGenParams,
    base_url_override: Option<&str>,
) -> Result<GeneratedImage, String> {
    let selected_model = select_openrouter_image_model(configured_slug).to_owned();
    let base_url = base_url_override
        .unwrap_or(DEFAULT_OPENROUTER_BASE_URL)
        .trim()
        .trim_end_matches('/');
    let endpoint = format!("{base_url}/chat/completions");

    let content = if params.reference_images.is_empty() {
        Value::String(params.prompt)
    } else {
        let mut parts = Vec::with_capacity(1 + params.reference_images.len());
        parts.push(json!({
            "type": "text",
            "text": params.prompt,
        }));
        for (b64, media_type) in &params.reference_images {
            let mime = media_type_mime_str(media_type);
            parts.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{mime};base64,{b64}"),
                }
            }));
        }
        Value::Array(parts)
    };

    let body = json!({
        "model": selected_model,
        "messages": [
            {
                "role": "user",
                "content": content,
            }
        ],
        "modalities": ["image", "text"],
        "image_config": {
            "aspect_ratio": params.aspect_ratio.as_ratio_str(),
        }
    });

    let response = OPENROUTER_HTTP_CLIENT
        .post(&endpoint)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OpenRouter image request failed: {e}"))?;

    let status = response.status();
    let raw_text = response
        .text()
        .await
        .map_err(|e| format!("Failed to read OpenRouter response body: {e}"))?;

    if !status.is_success() {
        let snippet: String = raw_text.chars().take(400).collect();
        return Err(format!(
            "OpenRouter image generation failed (HTTP {status}): {snippet}"
        ));
    }

    let reply: Value = serde_json::from_str(&raw_text)
        .map_err(|e| format!("OpenRouter returned invalid JSON: {e}"))?;

    if let Some(err_msg) = reply
        .get("error")
        .and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str()))
        .filter(|s| !s.trim().is_empty())
    {
        return Err(format!("OpenRouter image generation error: {err_msg}"));
    }

    let data_url = extract_openrouter_image_data(&reply).ok_or_else(|| {
        "OpenRouter response did not include any generated image in `choices[0].message.images`."
            .to_owned()
    })?;

    let bytes = decode_data_url_or_base64(data_url)?;
    let model_used = reply
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or(selected_model, str::to_owned);
    let usage = parse_openrouter_usage(&reply);
    let spec = resolve_any_catalog_spec(&model_used);

    finalize_generated_image(bytes, model_used, usage, spec)
}

/// Generates an image using `provider` and its configured credentials in `creds`.
///
/// # Errors
///
/// Returns a human-readable error if `provider` does not support image generation, if its API key
/// is missing, if reference images are supplied to a text-to-image-only endpoint, or if the
/// upstream provider call fails.
pub async fn generate_image_with_provider(
    provider: &ModelProvider,
    creds: &AiCredentials,
    params: ImageGenParams,
) -> Result<GeneratedImage, String> {
    generate_image_with_base_url(provider, creds, params, None).await
}

/// Internal/testable entry point allowing mock local HTTP base URL overrides while exercising
/// the exact production `rig-core` and `OpenRouter` wire encoders.
#[doc(hidden)]
#[allow(clippy::too_many_lines)]
pub async fn generate_image_with_base_url(
    provider: &ModelProvider,
    creds: &AiCredentials,
    params: ImageGenParams,
    base_url_override: Option<&str>,
) -> Result<GeneratedImage, String> {
    let trimmed_prompt = params.prompt.trim();
    if trimmed_prompt.is_empty() {
        return Err("Image prompt must not be empty.".to_owned());
    }

    if !params.reference_images.is_empty()
        && matches!(
            provider,
            ModelProvider::OpenAi(_, _) | ModelProvider::Xai(_, _) | ModelProvider::MiniMax(_, _)
        )
    {
        return Err(format!(
            "{}'s image generation endpoint supports text-to-image only and does not accept reference images. \
             Use a character assigned to Gemini or OpenRouter for reference-image editing.",
            provider.provider_name()
        ));
    }

    match provider {
        ModelProvider::Gemini(m, _) => {
            let key = creds.resolve_api_key("Gemini");
            if key.is_empty() {
                return Err(
                    "Gemini API key is missing. Configure it in Preferences -> APIs.".to_owned(),
                );
            }
            let configured_id = m.api_name().trim();
            let target_model = if configured_id.contains("-image") {
                configured_id
            } else {
                rig::providers::gemini::GEMINI_3_1_FLASH_IMAGE
            };

            let mut cfg = rig::providers::gemini::GeminiConfig::new(key.as_ref());
            if let Some(base) = base_url_override {
                cfg = cfg.with_base_url(base);
            }
            let model = cfg.client().image_generation(target_model);

            let (w, h) = params.aspect_ratio.gemini_dimensions();
            let mut builder = ImageGenerationRequestBuilder::new(params.prompt)
                .width(w)
                .height(h)
                .additional_params(
                    rig::providers::gemini::image_generation::ImageOptions::new()
                        .aspect_ratio(params.aspect_ratio.as_ratio_str())
                        .params(),
                );
            for (b64, mime) in params.reference_images {
                builder = builder.image(Image {
                    data: DocumentSourceKind::Base64(b64),
                    media_type: Some(mime),
                    detail: None,
                    native: None,
                });
            }

            let resp = model
                .call(builder.build())
                .await
                .map_err(|e| format!("Gemini image generation failed: {e}"))?;
            let model_used = resp.model.unwrap_or_else(|| target_model.to_owned());
            let spec = catalog_get_spec("gcp.gemini", &model_used)
                .or_else(|| catalog_get_spec("gcp.gemini", target_model));
            finalize_generated_image(resp.image, model_used, resp.usage, spec)
        }
        ModelProvider::OpenAi(m, _) => {
            let key = creds.resolve_api_key("OpenAI");
            if key.is_empty() {
                return Err(
                    "OpenAI API key is missing. Configure it in Preferences -> APIs.".to_owned(),
                );
            }
            let configured_id = m.api_name().trim();
            let target_model = if configured_id.contains("-image") {
                configured_id
            } else {
                rig::providers::openai::GPT_IMAGE_2
            };

            let mut cfg = rig::providers::openai::OpenAIConfig::new(key.as_ref());
            if let Some(base) = base_url_override {
                cfg = cfg.with_base_url(base);
            }
            let model = cfg.client().image_generation(target_model);

            // Never use rig's 256x256 default (rejected by gpt-image-2) and never pass `response_format`.
            let (w, h) = params.aspect_ratio.openai_dimensions();
            let req = ImageGenerationRequestBuilder::new(params.prompt)
                .width(w)
                .height(h)
                .build();

            let resp = model
                .call(req)
                .await
                .map_err(|e| format!("OpenAI image generation failed: {e}"))?;
            let model_used = resp.model.unwrap_or_else(|| target_model.to_owned());
            let spec = catalog_get_spec("openai", &model_used)
                .or_else(|| catalog_get_spec("openai", target_model));
            finalize_generated_image(resp.image, model_used, resp.usage, spec)
        }
        ModelProvider::Xai(_, _) => {
            let key = creds.resolve_api_key("xAI");
            if key.is_empty() {
                return Err(
                    "xAI API key is missing. Configure it in Preferences -> APIs.".to_owned(),
                );
            }
            let target_model = rig::providers::xai::GROK_IMAGINE_IMAGE;

            let mut cfg = rig::providers::openai::OpenAIConfig::with_key(
                &rig::providers::xai::DIALECT,
                key.as_ref(),
            );
            if let Some(base) = base_url_override {
                cfg = cfg.with_base_url(base);
            }
            let model = cfg.client().image_generation(target_model);

            let req = ImageGenerationRequestBuilder::new(params.prompt)
                .additional_params(json!({
                    "aspect_ratio": params.aspect_ratio.as_ratio_str(),
                }))
                .build();

            let resp = model
                .call(req)
                .await
                .map_err(|e| format!("xAI image generation failed: {e}"))?;
            let model_used = resp.model.unwrap_or_else(|| target_model.to_owned());
            let spec = catalog_get_spec("xai", &model_used)
                .or_else(|| catalog_get_spec("xai", target_model));
            finalize_generated_image(resp.image, model_used, resp.usage, spec)
        }
        ModelProvider::MiniMax(_, _) => {
            let key = creds.resolve_api_key("MiniMax");
            if key.is_empty() {
                return Err(
                    "MiniMax API key is missing. Configure it in Preferences -> APIs.".to_owned(),
                );
            }
            let target_model = rig::providers::minimax::image_generation::IMAGE_01;

            let mut cfg = rig::providers::openai::OpenAIConfig::with_key(
                &rig::providers::openai::wire::MINIMAX,
                key.as_ref(),
            );
            if let Some(base) = base_url_override {
                cfg = cfg.with_base_url(base);
            }
            let model = cfg.client().image_generation(target_model);

            let (w, h) = params.aspect_ratio.minimax_dimensions();
            let req = ImageGenerationRequestBuilder::new(params.prompt)
                .width(w)
                .height(h)
                .build();

            let resp = model
                .call(req)
                .await
                .map_err(|e| format!("MiniMax image generation failed: {e}"))?;
            let model_used = resp.model.unwrap_or_else(|| target_model.to_owned());
            let spec = catalog_get_spec("minimax", &model_used)
                .or_else(|| catalog_get_spec("minimax", target_model));
            finalize_generated_image(resp.image, model_used, resp.usage, spec)
        }
        ModelProvider::OpenRouter(slug) => {
            let key = creds.resolve_api_key("OpenRouter");
            if key.is_empty() {
                return Err(
                    "OpenRouter API key is missing. Configure it in Preferences -> APIs."
                        .to_owned(),
                );
            }
            generate_openrouter_image(slug, key.as_ref(), params, base_url_override).await
        }
        _ => Err(format!(
            "Provider '{}' does not support native image generation.",
            provider.provider_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::{
        AnthropicModel, ChatGptModel, GeminiModel, MiniMaxModel, OpenAiModel, XaiModel,
    };
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn synthetic_png_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([32, 128, 224]));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)
            .expect("encode synthetic 1x1 PNG");
        buf.into_inner()
    }

    async fn spawn_mock_json_server(
        response_json: Value,
    ) -> (String, tokio::sync::oneshot::Receiver<(String, Value)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let resp_body = serde_json::to_vec(&response_json).unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::with_capacity(4096);
            let mut chunk = [0_u8; 2048];
            let header_end = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break 0;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
            };

            let headers_str = String::from_utf8_lossy(&raw[..header_end]).into_owned();
            let request_line = headers_str.lines().next().unwrap_or("").to_owned();
            let content_len = headers_str
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);

            while raw.len() < header_end + content_len {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }

            let body_slice = &raw[header_end..(header_end + content_len).min(raw.len())];
            let parsed_body: Value = serde_json::from_slice(body_slice).unwrap_or(Value::Null);
            let _ = tx.send((request_line, parsed_body));

            let http_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                resp_body.len()
            );
            let _ = socket.write_all(http_resp.as_bytes()).await;
            let _ = socket.write_all(&resp_body).await;
            let _ = socket.shutdown().await;
        });

        (format!("http://{addr}"), rx)
    }

    #[test]
    fn aspect_ratio_parsing_and_provider_dimensions() {
        assert_eq!(ImageAspectRatio::from_str_lenient(None), ImageAspectRatio::Square);
        assert_eq!(
            ImageAspectRatio::from_str_lenient(Some("16:9")),
            ImageAspectRatio::Landscape
        );
        assert_eq!(
            ImageAspectRatio::from_str_lenient(Some("portrait")),
            ImageAspectRatio::Portrait
        );
        assert_eq!(
            ImageAspectRatio::from_str_lenient(Some("4:3")),
            ImageAspectRatio::Standard
        );
        assert_eq!(
            ImageAspectRatio::from_str_lenient(Some("3:4")),
            ImageAspectRatio::Tall
        );

        assert_eq!(ImageAspectRatio::Square.openai_dimensions(), (1024, 1024));
        assert_eq!(
            ImageAspectRatio::Landscape.openai_dimensions(),
            (1536, 1024)
        );
        assert_eq!(ImageAspectRatio::Portrait.openai_dimensions(), (1024, 1536));

        for ratio in [
            ImageAspectRatio::Square,
            ImageAspectRatio::Landscape,
            ImageAspectRatio::Portrait,
            ImageAspectRatio::Standard,
            ImageAspectRatio::Tall,
        ] {
            let (w, h) = ratio.minimax_dimensions();
            assert!((512..=2048).contains(&w) && w.is_multiple_of(8));
            assert!((512..=2048).contains(&h) && h.is_multiple_of(8));
        }
    }

    #[test]
    fn openrouter_model_selector_preserves_image_models_and_defaults_chat_models() {
        assert_eq!(
            select_openrouter_image_model("anthropic/claude-sonnet-5-5"),
            "google/gemini-3.1-flash-image"
        );
        assert_eq!(
            select_openrouter_image_model("google/gemini-3.1-flash-image"),
            "google/gemini-3.1-flash-image"
        );
        assert_eq!(
            select_openrouter_image_model("black-forest-labs/flux.2-pro"),
            "black-forest-labs/flux.2-pro"
        );
    }

    #[tokio::test]
    async fn openai_wire_sends_valid_dimensions_omits_response_format_and_rejects_ref_images() {
        let png = synthetic_png_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        let mut keys = HashMap::new();
        keys.insert("OpenAI".to_owned(), "sk-synthetic-openai".to_owned());
        let creds = AiCredentials::new(keys, String::new());
        let provider = ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None);

        // Reference images must be rejected cleanly before network I/O
        let ref_err = generate_image_with_base_url(
            &provider,
            &creds,
            ImageGenParams {
                prompt: "A geometric blue cube".to_owned(),
                aspect_ratio: ImageAspectRatio::Square,
                reference_images: vec![(b64.clone(), ImageMediaType::PNG)],
            },
            Some("http://127.0.0.1:9"),
        )
        .await
        .unwrap_err();
        assert!(ref_err.contains("text-to-image only"));

        let (base_url, rx) = spawn_mock_json_server(json!({
            "created": 1_700_000_000,
            "data": [{ "b64_json": b64 }]
        }))
        .await;

        let out = generate_image_with_base_url(
            &provider,
            &creds,
            ImageGenParams {
                prompt: "A geometric blue cube".to_owned(),
                aspect_ratio: ImageAspectRatio::Landscape,
                reference_images: Vec::new(),
            },
            Some(&format!("{base_url}/v1")),
        )
        .await
        .expect("OpenAI mock image generation should succeed");

        let (req_line, req_body) = rx.await.unwrap();
        assert!(req_line.starts_with("POST /v1/images/generations "));
        assert_eq!(req_body["model"], "gpt-image-2");
        assert_eq!(req_body["size"], "1536x1024");
        assert!(
            req_body.get("response_format").is_none(),
            "response_format must not be sent on OpenAI wire: {req_body}"
        );
        assert_eq!(out.bytes, png);
        assert_eq!(out.media_type, ImageMediaType::PNG);
        assert_eq!(out.model_used, "gpt-image-2");
    }

    #[tokio::test]
    async fn gemini_wire_sends_aspect_ratio_inline_reference_image_and_extracts_usage() {
        let png = synthetic_png_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        let mut keys = HashMap::new();
        keys.insert("Gemini".to_owned(), "gem-synthetic-key".to_owned());
        let creds = AiCredentials::new(keys, String::new());
        let provider = ModelProvider::Gemini(GeminiModel::Flash3_8, None);

        let (base_url, rx) = spawn_mock_json_server(json!({
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
                "promptTokenCount": 120,
                "candidatesTokenCount": 1290,
                "totalTokenCount": 1410
            }
        }))
        .await;

        let out = generate_image_with_base_url(
            &provider,
            &creds,
            ImageGenParams {
                prompt: "Restyle this cube in isometric pixel art".to_owned(),
                aspect_ratio: ImageAspectRatio::Landscape,
                reference_images: vec![(b64, ImageMediaType::PNG)],
            },
            Some(&base_url),
        )
        .await
        .expect("Gemini mock image generation should succeed");

        let (req_line, req_body) = rx.await.unwrap();
        assert!(
            req_line.starts_with("POST /v1beta/models/gemini-3.1-flash-image:generateContent ")
        );
        assert_eq!(
            req_body["generationConfig"]["responseModalities"],
            json!(["IMAGE"])
        );
        assert_eq!(
            req_body["generationConfig"]["imageConfig"]["aspectRatio"],
            "16:9"
        );
        let parts = req_body["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");

        assert_eq!(out.bytes, png);
        assert_eq!(out.media_type, ImageMediaType::PNG);
        assert_eq!(out.model_used, "gemini-3.1-flash-image");
        let usage = out.usage.expect("Gemini usageMetadata should populate Usage");
        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(1290));
        assert_eq!(usage.total_tokens, Some(1410));
        assert!(usage.cost.is_some_and(|c| c.total > 0.0));
    }

    #[tokio::test]
    async fn openrouter_wire_posts_multimodal_chat_and_decodes_data_url() {
        let png = synthetic_png_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let data_url = format!("data:image/png;base64,{b64}");

        let mut keys = HashMap::new();
        keys.insert("OpenRouter".to_owned(), "or-synthetic-key".to_owned());
        let creds = AiCredentials::new(keys, String::new());
        let provider = ModelProvider::OpenRouter("anthropic/claude-sonnet-5-5".to_owned());

        let (base_url, rx) = spawn_mock_json_server(json!({
            "model": "google/gemini-3.1-flash-image",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Generated image.",
                    "images": [{
                        "type": "image_url",
                        "image_url": { "url": data_url }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 40,
                "completion_tokens": 800,
                "total_tokens": 840,
                "cost": 0.0025
            }
        }))
        .await;

        let out = generate_image_with_base_url(
            &provider,
            &creds,
            ImageGenParams {
                prompt: "A geometric blue cube".to_owned(),
                aspect_ratio: ImageAspectRatio::Tall,
                reference_images: Vec::new(),
            },
            Some(&format!("{base_url}/api/v1")),
        )
        .await
        .expect("OpenRouter mock image generation should succeed");

        let (req_line, req_body) = rx.await.unwrap();
        assert!(req_line.starts_with("POST /api/v1/chat/completions "));
        assert_eq!(req_body["model"], "google/gemini-3.1-flash-image");
        assert_eq!(req_body["modalities"], json!(["image", "text"]));
        assert_eq!(req_body["image_config"]["aspect_ratio"], "3:4");

        assert_eq!(out.bytes, png);
        assert_eq!(out.media_type, ImageMediaType::PNG);
        assert_eq!(out.model_used, "google/gemini-3.1-flash-image");
        let usage = out.usage.unwrap();
        assert_eq!(usage.total_tokens, Some(840));
        assert!((usage.cost.unwrap().total - 0.0025).abs() < 1e-9);
    }

    #[tokio::test]
    async fn xai_and_minimax_wires_encode_expected_fields_and_reject_unsupported_providers() {
        let png = synthetic_png_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        let mut keys = HashMap::new();
        keys.insert("xAI".to_owned(), "xai-synthetic-key".to_owned());
        keys.insert("MiniMax".to_owned(), "mm-synthetic-key".to_owned());
        let creds = AiCredentials::new(keys, String::new());

        // xAI wire check
        let (xai_url, xai_rx) = spawn_mock_json_server(json!({
            "data": [{ "b64_json": b64 }]
        }))
        .await;
        let xai_out = generate_image_with_base_url(
            &ModelProvider::Xai(XaiModel::Grok4_7, None),
            &creds,
            ImageGenParams {
                prompt: "A geometric blue cube".to_owned(),
                aspect_ratio: ImageAspectRatio::Portrait,
                reference_images: Vec::new(),
            },
            Some(&xai_url),
        )
        .await
        .unwrap();
        let (xai_line, xai_body) = xai_rx.await.unwrap();
        assert!(xai_line.starts_with("POST /v1/images/generations "));
        assert_eq!(xai_body["model"], "grok-imagine-image");
        assert_eq!(xai_body["aspect_ratio"], "9:16");
        assert_eq!(xai_out.bytes, png);

        // MiniMax wire check
        let (mm_url, mm_rx) = spawn_mock_json_server(json!({
            "id": "trace-1",
            "data": { "image_base64": [b64] }
        }))
        .await;
        let mm_out = generate_image_with_base_url(
            &ModelProvider::MiniMax(MiniMaxModel::MiniMaxM3, None),
            &creds,
            ImageGenParams {
                prompt: "A geometric blue cube".to_owned(),
                aspect_ratio: ImageAspectRatio::Landscape,
                reference_images: Vec::new(),
            },
            Some(&format!("{mm_url}/v1")),
        )
        .await
        .unwrap();
        let (mm_line, mm_body) = mm_rx.await.unwrap();
        assert!(mm_line.starts_with("POST /v1/image_generation "));
        assert_eq!(mm_body["model"], "image-01");
        assert_eq!(mm_body["width"], 1280);
        assert_eq!(mm_body["height"], 720);
        assert_eq!(mm_out.bytes, png);

        // Unsupported text-only providers return clean error
        for unsupported in [
            ModelProvider::Anthropic(AnthropicModel::ClaudeSonnet5, None),
            ModelProvider::ChatGpt(ChatGptModel::new("gpt-5.4"), None),
        ] {
            let err = generate_image_with_provider(
                &unsupported,
                &creds,
                ImageGenParams {
                    prompt: "A geometric blue cube".to_owned(),
                    aspect_ratio: ImageAspectRatio::Square,
                    reference_images: Vec::new(),
                },
            )
            .await
            .unwrap_err();
            assert!(err.contains("does not support native image generation"));
        }
    }
}
