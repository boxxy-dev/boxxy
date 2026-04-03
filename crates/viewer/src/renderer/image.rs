use crate::parser::blocks::ContentBlock;
use crate::renderer::BlockRenderer;
use gtk4 as gtk;
use gtk4::prelude::*;

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
            picture.set_halign(gtk::Align::Center);
            // Optionally, give it a subtle border or radius
            picture.add_css_class("card");
            
            // A spinner or placeholder while loading
            let overlay = gtk::Overlay::new();
            let spinner = gtk::Spinner::new();
            spinner.set_halign(gtk::Align::Center);
            spinner.set_valign(gtk::Align::Center);
            spinner.set_margin_top(16);
            spinner.set_margin_bottom(16);
            spinner.start();
            
            overlay.set_child(Some(&picture));
            overlay.add_overlay(&spinner);
            
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
                let bytes_opt = if url_clone.starts_with("http://") || url_clone.starts_with("https://") {
                    let (tx, rx) = async_channel::bounded::<Vec<u8>>(1);
                    let url_req = url_clone.clone();
                    
                    std::thread::spawn(move || {
                        if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                            rt.block_on(async {
                                let client = reqwest::Client::builder()
                                    .timeout(std::time::Duration::from_secs(10))
                                    .user_agent("Mozilla/5.0 (X11; Linux x86_64) BoxxyTerminal/0.1")
                                    .build()
                                    .unwrap_or_default();

                                match client.get(&url_req).send().await {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        if !status.is_success() {
                                            log::error!("Failed to fetch image {}: HTTP {}", url_req, status);
                                            return;
                                        }

                                        let len = resp.content_length().unwrap_or(0);
                                        if len > 10 * 1024 * 1024 {
                                            log::error!("Image {} is too large ({} bytes)", url_req, len);
                                            return;
                                        }

                                        if let Ok(bytes) = resp.bytes().await {
                                            let _ = tx.send(bytes.to_vec()).await;
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Network error fetching image {}: {}", url_req, e);
                                    }
                                }
                            });
                        }
                    });
                    
                    rx.recv().await.ok()
                } else {
                    let path = url_clone.strip_prefix("file://").unwrap_or(&url_clone);
                    let path = path.to_string();
                    let (tx, rx) = async_channel::bounded::<Vec<u8>>(1);
                    
                    std::thread::spawn(move || {
                        if let Ok(bytes) = std::fs::read(&path) {
                            let _ = tx.send_blocking(bytes);
                        }
                    });
                    
                    rx.recv().await.ok()
                };

                spinner_clone.stop();
                overlay_clone.remove_overlay(&spinner_clone);

                if let Some(bytes) = bytes_opt {
                    let glib_bytes = gtk::glib::Bytes::from(&bytes);
                    if let Ok(texture) = gtk::gdk::Texture::from_bytes(&glib_bytes) {
                        picture_clone.set_paintable(Some(&texture));
                    } else {
                        // Failed to parse image. Fallback to link.
                        fallback_to_link(&overlay_clone, &picture_clone, &url_clone, &alt_clone);
                    }
                } else {
                    // Failed to fetch. Fallback to link.
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
