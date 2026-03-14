use std::collections::HashMap;
use std::sync::Arc;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as Base64;
use log::{error, info};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    #[default]
    TransmitAndDisplay,
    DisplayExisting,
    TransmitOnly,
    Query,
    ControlPlacement,
    Delete,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    #[default]
    Rgb = 24,
    Rgba = 32,
    Png = 100,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Transmission {
    #[default]
    Direct,
    File,
    TempFile,
    SharedMemory,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    #[default]
    None,
    Zlib,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CursorMovement {
    #[default]
    Move,
    DoNotMove,
}

#[derive(Debug, Default, Clone)]
pub struct Command {
    pub action: Action,
    pub format: Format,
    pub transmission: Transmission,
    pub compression: Compression,
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub columns: Option<u32>,
    pub rows: Option<u32>,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub z: Option<i32>,
    pub offset: Option<u32>,
    pub size: Option<u32>,
    pub delete_mode: Option<u8>,
    pub cursor_movement: CursorMovement,
    pub quiet: u8,
    pub more: bool,
    // Add more fields as needed
}

use crate::engine::index::Point;

#[derive(Clone)]
pub struct Placement {
    pub image_id: u32,
    pub placement_id: u32,
    pub point: Point,
    pub width: Option<u32>, // in cells
    pub height: Option<u32>, // in cells
    pub x_offset: u32, // in pixels
    pub y_offset: u32, // in pixels
    pub z_index: i32,
    pub visible_width: u32, // in pixels
    pub visible_height: u32, // in pixels
}

#[derive(Clone)]
pub enum KittyImageData {
    Dynamic(image::DynamicImage),
    RawRgb { width: u32, height: u32, data: Arc<[u8]> },
    RawRgba { width: u32, height: u32, data: Arc<[u8]> },
}

impl KittyImageData {
    pub fn width(&self) -> u32 {
        match self {
            Self::Dynamic(img) => img.width(),
            Self::RawRgb { width, .. } => *width,
            Self::RawRgba { width, .. } => *width,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Self::Dynamic(img) => img.height(),
            Self::RawRgb { height, .. } => *height,
            Self::RawRgba { height, .. } => *height,
        }
    }
}

pub struct KittyImage {
    pub id: u32,
    pub data: KittyImageData,
    pub texture: Option<gtk4::gdk::Texture>,
}

pub struct KittyGraphics {
    pub images: HashMap<u32, Arc<KittyImage>>,
    pub placements: Vec<Placement>,
    pub pending_data: Vec<u8>,
    pub pending_command: Option<Command>,
}

pub type KittyResponse = (Option<String>, Option<(Option<u32>, Option<u32>, u32, u32)>);

impl Default for KittyGraphics {
    fn default() -> Self {
        Self::new()
    }
}

impl KittyGraphics {
    pub fn rotate_placements(&mut self, range: &std::ops::Range<crate::engine::index::Line>, delta: i32) {
        for placement in &mut self.placements {
            let old_line = placement.point.line.0;
            let range_top = range.start.0;
            let range_bottom = range.end.0;

            if (old_line >= range_top || range_top == 0) && old_line < range_bottom {
                placement.point.line -= delta;
            }
            if old_line != placement.point.line.0 {
                log::trace!("KittyGraphics: Rotated placement {} from line {} to {}", placement.image_id, old_line, placement.point.line.0);
            }
        }
    }

    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            placements: Vec::new(),
            pending_data: Vec::new(),
            pending_command: None,
        }
    }

    pub fn handle_command(&mut self, data: &[u8], cursor_point: Point) -> KittyResponse {
        // Remove 'G' prefix
        let data = if data.starts_with(b"G") {
            &data[1..]
        } else {
            data
        };
        
        let mut parts = data.splitn(2, |&b| b == b';');
        let control = parts.next().unwrap_or(&[]);
        let payload = parts.next().unwrap_or(&[]);
        
        info!("KittyGraphics: handle_command raw control len={}, payload len={}", control.len(), payload.len());
        if !control.is_empty() {
            info!("KittyGraphics: control sequence: {}", String::from_utf8_lossy(control));
        }

        let mut cmd = Command::default();
        let mut data_only = false;

        for param in control.split(|&b| b == b',') {
            if param.is_empty() { continue; }
            let mut kv = param.splitn(2, |&b| b == b'=');
            let key = kv.next().unwrap_or(&[]);
            let value = kv.next().unwrap_or(&[]);

            match key {
                b"a" => cmd.action = match value {
                    b"T" => Action::TransmitAndDisplay,
                    b"p" => Action::DisplayExisting,
                    b"t" => Action::TransmitOnly,
                    b"q" => Action::Query,
                    b"c" => Action::ControlPlacement,
                    b"d" => Action::Delete,
                    _ => Action::TransmitAndDisplay,
                },
                b"f" => cmd.format = match value {
                    b"24" => Format::Rgb,
                    b"32" => Format::Rgba,
                    b"100" => Format::Png,
                    _ => Format::Rgb,
                },
                b"t" => cmd.transmission = match value {
                    b"f" => Transmission::File,
                    b"t" => Transmission::TempFile,
                    b"s" => Transmission::SharedMemory,
                    _ => Transmission::Direct,
                },
                b"o" => cmd.compression = match value {
                    b"z" => Compression::Zlib,
                    _ => Compression::None,
                },
                b"i" => cmd.image_id = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"I" => cmd.image_number = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"p" => cmd.placement_id = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"s" => cmd.width = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"v" => cmd.height = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"c" => cmd.columns = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"r" => cmd.rows = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"x" => cmd.x = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"y" => cmd.y = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"z" => cmd.z = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"O" => cmd.offset = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"S" => cmd.size = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()),
                b"d" => cmd.delete_mode = value.first().copied(),
                b"q" => cmd.quiet = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok()).unwrap_or(0),
                b"m" => cmd.more = value == b"1",
                _ => {}
            }
        }

        // If it's a chunk of data for a previous command
        if cmd.image_id.is_none() && cmd.image_number.is_none() && self.pending_command.is_some() {
            data_only = true;
        }

        if data_only {
            if let Some(ref mut pending) = self.pending_command {
                // We MUST update the `more` flag, otherwise we'll never know when the last chunk arrives!
                pending.more = cmd.more;
            }
        } else {
            self.pending_command = Some(cmd.clone());
        }

        if !payload.is_empty() {
            self.pending_data.extend_from_slice(payload);
        }
        
        info!("KittyGraphics: chunk parsed, action={:?}, id={:?}, more={}, quiet={}, pending_len={}", 
               cmd.action, cmd.image_id, cmd.more, cmd.quiet, self.pending_data.len());

        if self.pending_command.as_ref().is_some_and(|c| !c.more) {
            info!("KittyGraphics: more=0, processing pending command...");
            let (response, cursor_movement) = self.process_pending(cursor_point);
            info!("KittyGraphics: returning response: {:?}", response);
            return (response, cursor_movement);
        }
        (None, None)
    }

    fn process_pending(&mut self, cursor_point: Point) -> KittyResponse {
        let cmd = match self.pending_command.take() {
            Some(c) => c,
            None => return (None, None),
        };

        let mut raw_data = Vec::new();
        if !self.pending_data.is_empty() {
            // Kitty protocol may send base64 data with whitespace (like newlines) or missing padding.
            // We need to sanitize it before decoding.
            let mut sanitized: Vec<u8> = self.pending_data.iter().copied().filter(|&b| !b.is_ascii_whitespace()).collect();
            
            // Add padding if missing
            while !sanitized.len().is_multiple_of(4) {
                sanitized.push(b'=');
            }
            
            match Base64.decode(&sanitized) {
                Ok(d) => raw_data = d,
                Err(e) => {
                    error!("KittyGraphics: Failed to decode base64: {} (sanitized len: {})", e, sanitized.len());
                    self.pending_data.clear();
                    
                    if cmd.quiet != 2 {
                        let err_msg = "ERROR:base64 decode error";
                        if let Some(id) = cmd.image_id {
                            return (Some(format!("\x1b_Gi={};{}\x1b\\", id, err_msg)), None);
                        } else if let Some(num) = cmd.image_number {
                            return (Some(format!("\x1b_GI={};{}\x1b\\", num, err_msg)), None);
                        }
                    }
                    return (None, None);
                }
            };
        }
        self.pending_data.clear();

        info!("KittyGraphics: Processing image: action={:?}, id={:?}, num={:?}, len={}", cmd.action, cmd.image_id, cmd.image_number, raw_data.len());

        // Assign a generated ID for anonymous images (id 0 or not specified)
        let mut image_id = cmd.image_id.unwrap_or(0);
        if image_id == 0 {
            static NEXT_ANON_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(2_000_000_000);
            image_id = NEXT_ANON_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        let mut error_msg = None;

        let raw_data = match cmd.transmission {
            Transmission::Direct => raw_data,
            Transmission::File | Transmission::TempFile => {
                let path = String::from_utf8_lossy(&raw_data).to_string();
                let data = match std::fs::read(&path) {
                    Ok(d) => d,
                    Err(e) => {
                        error!("KittyGraphics: Failed to read file {}: {}", path, e);
                        error_msg = Some(format!("ERROR:failed to read file: {}", e));
                        Vec::new()
                    }
                };
                if cmd.transmission == Transmission::TempFile {
                    let _ = std::fs::remove_file(&path);
                }
                data
            }
            Transmission::SharedMemory => {
                let name = String::from_utf8_lossy(&raw_data).to_string();
                let path = if name.starts_with('/') {
                    format!("/dev/shm{}", name)
                } else {
                    format!("/dev/shm/{}", name)
                };
                
                let data = match std::fs::read(&path) {
                    Ok(mut d) => {
                        if let Some(offset) = cmd.offset {
                            let offset = offset as usize;
                            if offset < d.len() {
                                d.drain(0..offset);
                            } else {
                                d.clear();
                            }
                        }
                        if let Some(size) = cmd.size {
                            let size = size as usize;
                            if size < d.len() {
                                d.truncate(size);
                            }
                        }
                        d
                    }
                    Err(e) => {
                        error!("KittyGraphics: Failed to read shm {}: {}", path, e);
                        error_msg = Some(format!("ERROR:failed to read shm: {}", e));
                        Vec::new()
                    }
                };
                
                // Unlink shared memory
                let _ = std::fs::remove_file(&path);
                data
            }
        };

        let raw_data = if cmd.compression == Compression::Zlib && error_msg.is_none() {
            match miniz_oxide::inflate::decompress_to_vec_zlib(&raw_data) {
                Ok(data) => data,
                Err(e) => {
                    error!("KittyGraphics: Failed to decompress zlib data: {:?}", e);
                    error_msg = Some("ERROR:zlib decompression failed".to_string());
                    Vec::new() // Will be handled by the next check
                }
            }
        } else {
            raw_data
        };

        let data_len = raw_data.len();

        if cmd.action == Action::Delete {
            let delete_mode = cmd.delete_mode.unwrap_or(b'a');
            info!("KittyGraphics: Deleting images, mode={}", delete_mode as char);

            match delete_mode {
                b'a' | b'A' => {
                    self.placements.clear();
                    if delete_mode == b'A' {
                        self.images.clear();
                    }
                }
                b'i' | b'I' => {
                    if let Some(id) = cmd.image_id {
                        self.placements.retain(|p| p.image_id != id);
                        if delete_mode == b'I' {
                            self.images.remove(&id);
                        }
                    }
                }
                b'p' | b'P' => {
                    if let Some(pid) = cmd.placement_id {
                        self.placements.retain(|p| p.placement_id != pid);
                    }
                }
                b'c' | b'C' => {
                    // Simple check: delete placements that start at current cursor
                    self.placements.retain(|p| p.point != cursor_point);
                }
                _ => {
                    self.placements.clear();
                }
            }
            
            let response = if cmd.quiet != 1 && cmd.quiet != 2 {
                Some("\x1b_G;OK\x1b\\".to_string())
            } else {
                None
            };
            return (response, None);
        }

        if matches!(cmd.action, Action::TransmitAndDisplay | Action::TransmitOnly | Action::Query) && error_msg.is_none() {
            let img_data = if cmd.format == Format::Png {
                match image::load_from_memory(&raw_data) {
                    Ok(img) => Some(KittyImageData::Dynamic(img)),
                    Err(e) => {
                        error!("KittyGraphics: Failed to load PNG image: {}", e);
                        error_msg = Some(format!("ERROR:{}", e));
                        None
                    }
                }
            } else {
                // RGB or RGBA
                let width = cmd.width.unwrap_or(0);
                let height = cmd.height.unwrap_or(0);
                if width == 0 || height == 0 {
                    error_msg = Some("ERROR:width or height is zero for raw pixels".to_string());
                    None
                } else {
                    let expected_len = (width * height * (if cmd.format == Format::Rgb { 3 } else { 4 })) as usize;
                    if data_len < expected_len {
                        error_msg = Some(format!("ERROR:data too short, expected {}, got {}", expected_len, data_len));
                        None
                    } else {
                        let arc_data: Arc<[u8]> = Arc::from(raw_data.into_boxed_slice());
                        match cmd.format {
                            Format::Rgb => Some(KittyImageData::RawRgb { width, height, data: arc_data }),
                            Format::Rgba => Some(KittyImageData::RawRgba { width, height, data: arc_data }),
                            _ => unreachable!(),
                        }
                    }
                }
            };

            if let Some(img_data) = img_data {
                if cmd.action == Action::Query {
                    info!("KittyGraphics: Query successful for image data (len={})", data_len);
                } else {
                    let kitty_img = Arc::new(KittyImage {
                        id: image_id,
                        data: img_data,
                        texture: None,
                    });
                    self.images.insert(image_id, kitty_img);
                    info!("KittyGraphics: Loaded image {}", image_id);
                }
            }
        }

        if matches!(cmd.action, Action::TransmitAndDisplay | Action::DisplayExisting) && error_msg.is_none() {
            if let Some(img) = self.images.get(&image_id) {
                // Create a placement at current cursor position
                let placement = Placement {
                    image_id,
                    placement_id: cmd.placement_id.unwrap_or(0),
                    point: cursor_point,
                    width: cmd.columns,
                    height: cmd.rows,
                    x_offset: 0,
                    y_offset: 0,
                    z_index: cmd.z.unwrap_or(0),
                    visible_width: img.data.width(),
                    visible_height: img.data.height(),
                };
                self.placements.push(placement);
                info!("KittyGraphics: Created placement for image {} at {:?}", image_id, cursor_point);
            } else {
                error_msg = Some("ERROR:image not found".to_string());
            }
        }

        // Return a response if required
        let response = if let Some(err) = error_msg {
            if cmd.quiet != 2 && cmd.quiet != 1 { // quiet=1 suppresses OK, quiet=2 suppresses errors
                if let Some(id) = cmd.image_id {
                    Some(format!("\x1b_Gi={};{}\x1b\\", id, err))
                } else {
                    cmd.image_number.map(|num| format!("\x1b_GI={};{}\x1b\\", num, err))
                }
            } else {
                None
            }
        } else {
            if cmd.quiet == 1 {
                None
            } else if let Some(id) = cmd.image_id {
                if let Some(pid) = cmd.placement_id {
                    Some(format!("\x1b_Gi={},p={};OK\x1b\\", id, pid))
                } else {
                    Some(format!("\x1b_Gi={};OK\x1b\\", id))
                }
            } else {
                cmd.image_number.map(|num| format!("\x1b_Gi={},I={};OK\x1b\\", image_id, num))
            }
        };

        let mut cursor_offset = None;
        if cmd.cursor_movement == CursorMovement::Move && matches!(cmd.action, Action::TransmitAndDisplay | Action::DisplayExisting)
            && let Some(img) = self.images.get(&image_id) {
                // Return either explicitly requested cell dimensions, or the image's pixel dimensions to let Term calculate cells
                cursor_offset = Some((
                    cmd.columns,
                    cmd.rows,
                    img.data.width(),
                    img.data.height()
                ));
        }

        (response, cursor_offset)
    }
}
