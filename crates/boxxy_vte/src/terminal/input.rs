use gtk4::gdk::{Key, ModifierType};

/// Translates a GTK key event into an ANSI byte sequence for the PTY
pub fn translate_key(key: Key, modifiers: ModifierType, is_app_cursor: bool) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();

    let ctrl = modifiers.contains(ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(ModifierType::ALT_MASK);
    let shift = modifiers.contains(ModifierType::SHIFT_MASK);

    // Standard modifier code: 1=none, 2=shift, 3=alt, 4=shift+alt, 5=ctrl, 6=shift+ctrl, 7=alt+ctrl, 8=shift+alt+ctrl
    let modifier_code = {
        let mut code = 1;
        if shift {
            code += 1;
        }
        if alt {
            code += 2;
        }
        if ctrl {
            code += 4;
        }
        code
    };

    let has_modifier = modifier_code > 1;

    // Prepend Esc for Alt if it's not a special CSI sequence
    let prepend_esc = alt;

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
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[3;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[3~");
            }
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
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}A", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOA");
            } else {
                bytes.extend_from_slice(b"\x1b[A");
            }
            return Some(bytes);
        }
        Key::Down => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}B", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOB");
            } else {
                bytes.extend_from_slice(b"\x1b[B");
            }
            return Some(bytes);
        }
        Key::Right => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}C", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOC");
            } else {
                bytes.extend_from_slice(b"\x1b[C");
            }
            return Some(bytes);
        }
        Key::Left => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}D", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOD");
            } else {
                bytes.extend_from_slice(b"\x1b[D");
            }
            return Some(bytes);
        }
        Key::Home => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}H", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOH");
            } else {
                bytes.extend_from_slice(b"\x1b[H");
            }
            return Some(bytes);
        }
        Key::End => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}F", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOF");
            } else {
                bytes.extend_from_slice(b"\x1b[F");
            }
            return Some(bytes);
        }
        Key::Page_Up => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[5;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[5~");
            }
            return Some(bytes);
        }
        Key::Page_Down => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[6;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[6~");
            }
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
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(c as u8 - b'A' + 1);
                return Some(bytes);
            }
        } else {
            if prepend_esc {
                bytes.push(0x1b);
            }
            let mut b = [0; 4];
            let s = ch.encode_utf8(&mut b);
            bytes.extend_from_slice(s.as_bytes());
            return Some(bytes);
        }
    }

    if bytes.is_empty() { None } else { Some(bytes) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::gdk::ModifierType;

    #[test]
    fn test_translate_basic_keys() {
        assert_eq!(translate_key(Key::Return, ModifierType::empty(), false), Some(vec![b'\r']));
        assert_eq!(translate_key(Key::BackSpace, ModifierType::empty(), false), Some(vec![0x7f]));
        assert_eq!(translate_key(Key::Tab, ModifierType::empty(), false), Some(vec![b'\t']));
        assert_eq!(translate_key(Key::Escape, ModifierType::empty(), false), Some(vec![0x1b]));
    }

    #[test]
    fn test_translate_arrows_no_mod() {
        assert_eq!(translate_key(Key::Up, ModifierType::empty(), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(translate_key(Key::Down, ModifierType::empty(), false), Some(b"\x1b[B".to_vec()));
        assert_eq!(translate_key(Key::Right, ModifierType::empty(), false), Some(b"\x1b[C".to_vec()));
        assert_eq!(translate_key(Key::Left, ModifierType::empty(), false), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn test_translate_arrows_app_cursor() {
        assert_eq!(translate_key(Key::Up, ModifierType::empty(), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(translate_key(Key::Down, ModifierType::empty(), true), Some(b"\x1bOB".to_vec()));
        assert_eq!(translate_key(Key::Right, ModifierType::empty(), true), Some(b"\x1bOC".to_vec()));
        assert_eq!(translate_key(Key::Left, ModifierType::empty(), true), Some(b"\x1bOD".to_vec()));
    }

    #[test]
    fn test_translate_ctrl_chars() {
        assert_eq!(translate_key(Key::a, ModifierType::CONTROL_MASK, false), Some(vec![1]));
        assert_eq!(translate_key(Key::c, ModifierType::CONTROL_MASK, false), Some(vec![3]));
        assert_eq!(translate_key(Key::z, ModifierType::CONTROL_MASK, false), Some(vec![26]));
    }

    #[test]
    fn test_translate_arrows_with_modifiers() {
        // Ctrl + Right -> \x1b[1;5C
        assert_eq!(translate_key(Key::Right, ModifierType::CONTROL_MASK, false), Some(b"\x1b[1;5C".to_vec()));
        // Alt + Left -> \x1b[1;3D
        assert_eq!(translate_key(Key::Left, ModifierType::ALT_MASK, false), Some(b"\x1b[1;3D".to_vec()));
        // Ctrl + Alt + Up -> \x1b[1;7A
        assert_eq!(translate_key(Key::Up, ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, false), Some(b"\x1b[1;7A".to_vec()));
    }

    #[test]
    fn test_translate_home_end_with_modifiers() {
        assert_eq!(translate_key(Key::Home, ModifierType::CONTROL_MASK, false), Some(b"\x1b[1;5H".to_vec()));
        assert_eq!(translate_key(Key::End, ModifierType::ALT_MASK, false), Some(b"\x1b[1;3F".to_vec()));
    }

    #[test]
    fn test_translate_delete_page_with_modifiers() {
        assert_eq!(translate_key(Key::Delete, ModifierType::CONTROL_MASK, false), Some(b"\x1b[3;5~".to_vec()));
        assert_eq!(translate_key(Key::Page_Up, ModifierType::SHIFT_MASK, false), Some(b"\x1b[5;2~".to_vec()));
    }

    #[test]
    fn test_translate_ctrl_alt_chars() {
        assert_eq!(translate_key(Key::c, ModifierType::CONTROL_MASK | ModifierType::ALT_MASK, false), Some(vec![0x1b, 3]));
    }
}
