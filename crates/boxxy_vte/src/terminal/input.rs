use gtk4::gdk::{Key, ModifierType};

/// Translates a GTK key event into an ANSI byte sequence for the PTY
pub fn translate_key(key: Key, modifiers: ModifierType) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();

    let ctrl = modifiers.contains(ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(ModifierType::ALT_MASK);

    if alt {
        bytes.push(b'\x1b');
    }

    // Handle special keys
    match key {
        Key::Return | Key::KP_Enter => {
            bytes.push(b'\r');
            return Some(bytes);
        }
        Key::BackSpace => {
            bytes.push(0x7f); // DEL
            return Some(bytes);
        }
        Key::Delete => {
            bytes.extend_from_slice(b"\x1b[3~");
            return Some(bytes);
        }
        Key::Tab => {
            bytes.push(b'\t');
            return Some(bytes);
        }
        Key::Escape => {
            bytes.push(0x1b);
            return Some(bytes);
        }
        Key::Up => {
            bytes.extend_from_slice(b"\x1b[A");
            return Some(bytes);
        }
        Key::Down => {
            bytes.extend_from_slice(b"\x1b[B");
            return Some(bytes);
        }
        Key::Right => {
            bytes.extend_from_slice(b"\x1b[C");
            return Some(bytes);
        }
        Key::Left => {
            bytes.extend_from_slice(b"\x1b[D");
            return Some(bytes);
        }
        Key::Home => {
            bytes.extend_from_slice(b"\x1b[H");
            return Some(bytes);
        }
        Key::End => {
            bytes.extend_from_slice(b"\x1b[F");
            return Some(bytes);
        }
        Key::Page_Up => {
            bytes.extend_from_slice(b"\x1b[5~");
            return Some(bytes);
        }
        Key::Page_Down => {
            bytes.extend_from_slice(b"\x1b[6~");
            return Some(bytes);
        }
        Key::F1 => {
            bytes.extend_from_slice(b"\x1bOP");
            return Some(bytes);
        }
        Key::F2 => {
            bytes.extend_from_slice(b"\x1bOQ");
            return Some(bytes);
        }
        Key::F3 => {
            bytes.extend_from_slice(b"\x1bOR");
            return Some(bytes);
        }
        Key::F4 => {
            bytes.extend_from_slice(b"\x1bOS");
            return Some(bytes);
        }
        Key::F5 => {
            bytes.extend_from_slice(b"\x1b[15~");
            return Some(bytes);
        }
        Key::F6 => {
            bytes.extend_from_slice(b"\x1b[17~");
            return Some(bytes);
        }
        Key::F7 => {
            bytes.extend_from_slice(b"\x1b[18~");
            return Some(bytes);
        }
        Key::F8 => {
            bytes.extend_from_slice(b"\x1b[19~");
            return Some(bytes);
        }
        Key::F9 => {
            bytes.extend_from_slice(b"\x1b[20~");
            return Some(bytes);
        }
        Key::F10 => {
            bytes.extend_from_slice(b"\x1b[21~");
            return Some(bytes);
        }
        Key::F11 => {
            bytes.extend_from_slice(b"\x1b[23~");
            return Some(bytes);
        }
        Key::F12 => {
            bytes.extend_from_slice(b"\x1b[24~");
            return Some(bytes);
        }
        _ => {}
    }

    // Handle printable characters and Control combinations
    if let Some(ch) = key.to_unicode() {
        if ctrl {
            // Control combinations: A-Z map to 1-26
            let c = ch.to_ascii_uppercase();
            if c.is_ascii_uppercase() {
                bytes.push(c as u8 - b'A' + 1);
                return Some(bytes);
            }
            // Add other ctrl mappings (like Ctrl+Space, etc.) if needed
        } else {
            let mut b = [0; 4];
            let s = ch.encode_utf8(&mut b);
            bytes.extend_from_slice(s.as_bytes());
            return Some(bytes);
        }
    }

    if bytes.is_empty() {
        None
    } else {
        Some(bytes)
    }
}
