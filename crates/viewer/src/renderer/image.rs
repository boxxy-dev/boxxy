use crate::parser::blocks::ContentBlock;
use crate::renderer::BlockRenderer;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::io::Cursor;
use std::sync::LazyLock;
use std::time::Duration;

const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 4096;

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) BoxxyTerminal/0.1")
        .build()
        .unwrap_or_default()
});

pub struct DecodedImage {
    pub width: i32,
    pub height: i32,
    pub rgba_bytes: Vec<u8>,
}

/// Decodes raw image bytes into an RGBA8 pixel buffer off the GTK main thread,
/// enforcing `MAX_IMAGE_BYTES` and `MAX_IMAGE_DIMENSION` guards.
pub(crate) fn decode_image_bytes(bytes: Vec<u8>) -> Result<DecodedImage, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!("Image exceeds 10 MB limit ({} bytes)", bytes.len()));
    }

    let reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| format!("Failed to guess image format: {e}"))?;

    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| format!("Failed to read image dimensions: {e}"))?;

    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(format!(
            "Image dimensions {width}x{height} out of allowed bounds (max {MAX_IMAGE_DIMENSION}px)"
        ));
    }

    let dyn_img = image::load_from_memory(&bytes)
        .map_err(|e| format!("Failed to decode image bytes: {e}"))?;
    let rgba = dyn_img.to_rgba8();

    Ok(DecodedImage {
        width: width as i32,
        height: height as i32,
        rgba_bytes: rgba.into_raw(),
    })
}

async fn fetch_and_decode_image(url: String) -> Option<DecodedImage> {
    let bytes = if url.starts_with("http://") || url.starts_with("https://") {
        let mut resp = match HTTP_CLIENT.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                log::error!("Network error fetching image {}: {}", url, e);
                return None;
            }
        };

        if !resp.status().is_success() {
            log::error!("Failed to fetch image {}: HTTP {}", url, resp.status());
            return None;
        }

        if let Some(len) = resp.content_length()
            && len > MAX_IMAGE_BYTES as u64
        {
            log::error!("Image {} is too large ({} bytes)", url, len);
            return None;
        }

        let mut acc = Vec::new();
        while let Ok(Some(chunk)) = resp.chunk().await {
            if acc.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
                log::error!("Image {} exceeded 10 MB during streaming; aborting", url);
                return None;
            }
            acc.extend_from_slice(&chunk);
        }
        acc
    } else {
        let path = url.strip_prefix("file://").unwrap_or(&url).to_string();
        if let Ok(meta) = tokio::fs::metadata(&path).await
            && meta.len() > MAX_IMAGE_BYTES as u64
        {
            log::error!("Local image {} is too large ({} bytes)", path, meta.len());
            return None;
        }
        match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                log::error!("Failed to read local image {}: {}", path, e);
                return None;
            }
        }
    };

    match tokio::task::spawn_blocking(move || decode_image_bytes(bytes)).await {
        Ok(Ok(decoded)) => Some(decoded),
        Ok(Err(e)) => {
            log::warn!("Image decode rejected for {}: {}", url, e);
            None
        }
        Err(e) => {
            log::error!("Image decode task panicked for {}: {}", url, e);
            None
        }
    }
}

pub struct ImageRenderer;

impl BlockRenderer for ImageRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Image { .. })
    }

    fn render(
        &self,
        block: &ContentBlock,
        _registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        if let ContentBlock::Image { url, title, alt } = block {
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_margin_bottom(12);

            let picture = gtk::Picture::new();
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_halign(gtk::Align::Center);
            picture.add_css_class("card");
            // Keep hidden while loading so an empty 0x0 `.card` border doesn't
            // render as a tiny white pill in the middle of the row.
            picture.set_visible(false);

            let overlay = gtk::Overlay::new();
            let spinner = gtk::Spinner::new();
            spinner.set_size_request(24, 24);
            spinner.set_halign(gtk::Align::Center);
            spinner.set_valign(gtk::Align::Center);
            spinner.set_margin_top(16);
            spinner.set_margin_bottom(16);
            spinner.start();

            overlay.set_child(Some(&picture));
            overlay.add_overlay(&spinner);
            overlay.set_measure_overlay(&spinner, true);

            vbox.append(&overlay);

            if !title.is_empty() {
                picture.set_tooltip_text(Some(title));
            } else if !alt.is_empty() {
                picture.set_tooltip_text(Some(alt));
            }

            let url_clone = url.clone();
            let picture_clone = picture.clone();
            let spinner_clone = spinner.clone();
            let overlay_clone = overlay.clone();
            let alt_clone = alt.clone();

            gtk::glib::spawn_future_local(async move {
                let url_for_fetch = url_clone.clone();
                let decoded_opt = boxxy_sys_utils::runtime()
                    .spawn(async move { fetch_and_decode_image(url_for_fetch).await })
                    .await
                    .ok()
                    .flatten();

                spinner_clone.stop();
                overlay_clone.remove_overlay(&spinner_clone);

                if let Some(decoded) = decoded_opt {
                    // Constrain display dimensions (max 480x320) while preserving
                    // aspect ratio, and set an explicit size request so virtualized
                    // `gtk::ListView` rows allocate the full image height.
                    let (max_w, max_h) = (480.0_f64, 320.0_f64);
                    let (w, h) = (decoded.width as f64, decoded.height as f64);
                    let scale = (max_w / w).min(max_h / h).min(1.0);
                    let disp_w = (w * scale).round().max(1.0) as i32;
                    let disp_h = (h * scale).round().max(1.0) as i32;

                    let glib_bytes = gtk::glib::Bytes::from_owned(decoded.rgba_bytes);
                    let texture = gtk::gdk::MemoryTexture::new(
                        decoded.width,
                        decoded.height,
                        gtk::gdk::MemoryFormat::R8g8b8a8,
                        &glib_bytes,
                        (decoded.width * 4) as usize,
                    );
                    picture_clone.set_size_request(disp_w, disp_h);
                    picture_clone.set_paintable(Some(&texture));
                    picture_clone.set_visible(true);
                } else {
                    fallback_to_link(&overlay_clone, &picture_clone, &url_clone, &alt_clone);
                }
            });

            vbox.upcast()
        } else {
            unreachable!()
        }
    }
}

fn fallback_to_link(overlay: &gtk::Overlay, picture: &gtk::Picture, url: &str, alt: &str) {
    picture.set_visible(false);
    let link = gtk::LinkButton::with_label(url, if alt.is_empty() { url } else { alt });
    link.set_halign(gtk::Align::Start);
    overlay.set_child(Some(&link));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_image_bytes_rejects_oversized_payload_and_decodes_valid_png() {
        let oversized = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert!(decode_image_bytes(oversized).is_err());

        let mut png_bytes = Vec::new();
        let img = image::RgbaImage::from_pixel(2, 3, image::Rgba([10, 20, 30, 255]));
        img.write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();

        let decoded = decode_image_bytes(png_bytes).expect("valid 2x3 PNG should decode");
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 3);
        assert_eq!(decoded.rgba_bytes.len(), 2 * 3 * 4);
    }
}
