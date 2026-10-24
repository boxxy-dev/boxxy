use crate::engine::term::TermMode;
use gtk4::gdk::{Key, ModifierType};

/// Translates a GTK key event into an ANSI byte sequence for the PTY using
/// legacy xterm/VT100 encoding rules (with optional `APP_CURSOR` mode).
pub fn translate_key(key: Key, modifiers: ModifierType, is_app_cursor: bool) -> Option<Vec<u8>> {
    let mut mode = TermMode::NONE;
    if is_app_cursor {
        mode.insert(TermMode::APP_CURSOR);
    }
    translate_key_with_mode(key, modifiers, mode)
}

/// Translates a GTK key event into an ANSI byte sequence for the PTY,
/// respecting both legacy modes (`APP_CURSOR`) and the Kitty Keyboard Protocol
/// (`DISAMBIGUATE_ESC_CODES`, `REPORT_ALL_KEYS_AS_ESC`).
pub fn translate_key_with_mode(
    key: Key,
    modifiers: ModifierType,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();

    let ctrl = modifiers.contains(ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(ModifierType::ALT_MASK);
    let shift = modifiers.contains(ModifierType::SHIFT_MASK);
    let is_app_cursor = mode.contains(TermMode::APP_CURSOR);
    let kitty_disambiguate = mode
        .intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC);
    let kitty_all_esc = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);

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
            if (kitty_disambiguate && (has_modifier || kitty_all_esc)) || shift || ctrl {
                if has_modifier {
                    bytes.extend_from_slice(format!("\x1b[13;{}u", modifier_code).as_bytes());
                } else {
                    bytes.extend_from_slice(b"\x1b[13u");
                }
            } else {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(b'\r');
            }
            return Some(bytes);
        }
        Key::BackSpace => {
            if kitty_disambiguate && (has_modifier || kitty_all_esc) {
                if has_modifier {
                    bytes.extend_from_slice(format!("\x1b[127;{}u", modifier_code).as_bytes());
                } else {
                    bytes.extend_from_slice(b"\x1b[127u");
                }
            } else {
                if alt {
                    bytes.push(0x1b);
                }
                if ctrl {
                    bytes.push(0x08); // BS (^H) for Ctrl+Backspace
                } else {
                    bytes.push(0x7f); // DEL
                }
            }
            return Some(bytes);
        }
        Key::Insert | Key::KP_Insert => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[2;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[2~");
            }
            return Some(bytes);
        }
        Key::Delete | Key::KP_Delete => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[3;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[3~");
            }
            return Some(bytes);
        }
        Key::Tab | Key::KP_Tab | Key::ISO_Left_Tab => {
            // On GTK4/XKB, Shift+Tab produces Key::ISO_Left_Tab (often with SHIFT_MASK consumed).
            let tab_shift = shift || key == Key::ISO_Left_Tab;
            let tab_mod_code = 1
                + if tab_shift { 1 } else { 0 }
                + if alt { 2 } else { 0 }
                + if ctrl { 4 } else { 0 };
            if kitty_disambiguate && (tab_mod_code > 1 || kitty_all_esc) {
                if tab_mod_code > 1 {
                    bytes.extend_from_slice(format!("\x1b[9;{}u", tab_mod_code).as_bytes());
                } else {
                    bytes.extend_from_slice(b"\x1b[9u");
                }
            } else if tab_shift {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.extend_from_slice(b"\x1b[Z");
            } else {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(b'\t');
            }
            return Some(bytes);
        }
        Key::Escape => {
            if kitty_disambiguate {
                if has_modifier {
                    bytes.extend_from_slice(format!("\x1b[27;{}u", modifier_code).as_bytes());
                } else {
                    bytes.extend_from_slice(b"\x1b[27u");
                }
            } else {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(0x1b);
            }
            return Some(bytes);
        }
        Key::Up | Key::KP_Up => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}A", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOA");
            } else {
                bytes.extend_from_slice(b"\x1b[A");
            }
            return Some(bytes);
        }
        Key::Down | Key::KP_Down => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}B", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOB");
            } else {
                bytes.extend_from_slice(b"\x1b[B");
            }
            return Some(bytes);
        }
        Key::Right | Key::KP_Right => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}C", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOC");
            } else {
                bytes.extend_from_slice(b"\x1b[C");
            }
            return Some(bytes);
        }
        Key::Left | Key::KP_Left => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}D", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOD");
            } else {
                bytes.extend_from_slice(b"\x1b[D");
            }
            return Some(bytes);
        }
        Key::Home | Key::KP_Home => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}H", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOH");
            } else {
                bytes.extend_from_slice(b"\x1b[H");
            }
            return Some(bytes);
        }
        Key::End | Key::KP_End => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}F", modifier_code).as_bytes());
            } else if is_app_cursor {
                bytes.extend_from_slice(b"\x1bOF");
            } else {
                bytes.extend_from_slice(b"\x1b[F");
            }
            return Some(bytes);
        }
        Key::Page_Up | Key::KP_Page_Up => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[5;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[5~");
            }
            return Some(bytes);
        }
        Key::Page_Down | Key::KP_Page_Down => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[6;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[6~");
            }
            return Some(bytes);
        }
        Key::KP_Begin => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}E", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[E");
            }
            return Some(bytes);
        }
        Key::F1 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}P", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1bOP");
            }
            return Some(bytes);
        }
        Key::F2 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}Q", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1bOQ");
            }
            return Some(bytes);
        }
        Key::F3 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}R", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1bOR");
            }
            return Some(bytes);
        }
        Key::F4 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[1;{}S", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1bOS");
            }
            return Some(bytes);
        }
        Key::F5 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[15;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[15~");
            }
            return Some(bytes);
        }
        Key::F6 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[17;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[17~");
            }
            return Some(bytes);
        }
        Key::F7 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[18;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[18~");
            }
            return Some(bytes);
        }
        Key::F8 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[19;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[19~");
            }
            return Some(bytes);
        }
        Key::F9 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[20;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[20~");
            }
            return Some(bytes);
        }
        Key::F10 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[21;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[21~");
            }
            return Some(bytes);
        }
        Key::F11 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[23;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[23~");
            }
            return Some(bytes);
        }
        Key::F12 => {
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[24;{}~", modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(b"\x1b[24~");
            }
            return Some(bytes);
        }
        _ => {}
    }

    // Handle printable characters and Control / Alt combinations
    if let Some(ch) = key.to_unicode() {
        // Under Kitty Keyboard Protocol, disambiguate modified keys (Ctrl, Alt, or all keys in REPORT_ALL_KEYS_AS_ESC)
        // using the lowercase/unshifted Unicode codepoint.
        if (kitty_disambiguate && (ctrl || alt)) || kitty_all_esc {
            let base_cp = ch.to_lowercase().next().unwrap_or(ch) as u32;
            if has_modifier {
                bytes.extend_from_slice(format!("\x1b[{};{}u", base_cp, modifier_code).as_bytes());
            } else {
                bytes.extend_from_slice(format!("\x1b[{}u", base_cp).as_bytes());
            }
            return Some(bytes);
        }

        if ctrl {
            let ctrl_byte = match ch {
                'a'..='z' | 'A'..='Z' => Some(ch.to_ascii_uppercase() as u8 - b'A' + 1),
                ' ' | '@' | '2' => Some(0x00),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '~' | '6' => Some(0x1e),
                '_' | '/' | '7' => Some(0x1f),
                '?' | '8' => Some(0x7f),
                _ => None,
            };
            if let Some(b) = ctrl_byte {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(b);
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
        assert_eq!(
            translate_key(Key::Return, ModifierType::empty(), false),
            Some(vec![b'\r'])
        );
        assert_eq!(
            translate_key(Key::BackSpace, ModifierType::empty(), false),
            Some(vec![0x7f])
        );
        assert_eq!(
            translate_key(Key::Tab, ModifierType::empty(), false),
            Some(vec![b'\t'])
        );
        assert_eq!(
            translate_key(Key::Escape, ModifierType::empty(), false),
            Some(vec![0x1b])
        );
    }

    #[test]
    fn test_translate_arrows_no_mod() {
        assert_eq!(
            translate_key(Key::Up, ModifierType::empty(), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            translate_key(Key::Down, ModifierType::empty(), false),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            translate_key(Key::Right, ModifierType::empty(), false),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            translate_key(Key::Left, ModifierType::empty(), false),
            Some(b"\x1b[D".to_vec())
        );
    }

    #[test]
    fn test_translate_arrows_app_cursor() {
        assert_eq!(
            translate_key(Key::Up, ModifierType::empty(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            translate_key(Key::Down, ModifierType::empty(), true),
            Some(b"\x1bOB".to_vec())
        );
        assert_eq!(
            translate_key(Key::Right, ModifierType::empty(), true),
            Some(b"\x1bOC".to_vec())
        );
        assert_eq!(
            translate_key(Key::Left, ModifierType::empty(), true),
            Some(b"\x1bOD".to_vec())
        );
    }

    #[test]
    fn test_translate_ctrl_chars() {
        assert_eq!(
            translate_key(Key::a, ModifierType::CONTROL_MASK, false),
            Some(vec![1])
        );
        assert_eq!(
            translate_key(Key::c, ModifierType::CONTROL_MASK, false),
            Some(vec![3])
        );
        assert_eq!(
            translate_key(Key::z, ModifierType::CONTROL_MASK, false),
            Some(vec![26])
        );
    }

    #[test]
    fn test_translate_arrows_with_modifiers() {
        // Ctrl + Right -> \x1b[1;5C
        assert_eq!(
            translate_key(Key::Right, ModifierType::CONTROL_MASK, false),
            Some(b"\x1b[1;5C".to_vec())
        );
        // Alt + Left -> \x1b[1;3D
        assert_eq!(
            translate_key(Key::Left, ModifierType::ALT_MASK, false),
            Some(b"\x1b[1;3D".to_vec())
        );
        // Ctrl + Alt + Up -> \x1b[1;7A
        assert_eq!(
            translate_key(
                Key::Up,
                ModifierType::CONTROL_MASK | ModifierType::ALT_MASK,
                false
            ),
            Some(b"\x1b[1;7A".to_vec())
        );
    }

    #[test]
    fn test_translate_home_end_with_modifiers() {
        assert_eq!(
            translate_key(Key::Home, ModifierType::CONTROL_MASK, false),
            Some(b"\x1b[1;5H".to_vec())
        );
        assert_eq!(
            translate_key(Key::End, ModifierType::ALT_MASK, false),
            Some(b"\x1b[1;3F".to_vec())
        );
    }

    #[test]
    fn test_translate_delete_page_with_modifiers() {
        assert_eq!(
            translate_key(Key::Delete, ModifierType::CONTROL_MASK, false),
            Some(b"\x1b[3;5~".to_vec())
        );
        assert_eq!(
            translate_key(Key::Page_Up, ModifierType::SHIFT_MASK, false),
            Some(b"\x1b[5;2~".to_vec())
        );
    }

    #[test]
    fn test_translate_ctrl_alt_chars() {
        assert_eq!(
            translate_key(
                Key::c,
                ModifierType::CONTROL_MASK | ModifierType::ALT_MASK,
                false
            ),
            Some(vec![0x1b, 3])
        );
    }

    #[test]
    fn test_translate_kp_keys() {
        assert_eq!(
            translate_key(Key::KP_Up, ModifierType::empty(), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            translate_key(Key::KP_Home, ModifierType::empty(), false),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            translate_key(Key::Insert, ModifierType::empty(), false),
            Some(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            translate_key(Key::KP_Insert, ModifierType::empty(), false),
            Some(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            translate_key(Key::KP_Begin, ModifierType::empty(), false),
            Some(b"\x1b[E".to_vec())
        );
        assert_eq!(
            translate_key(Key::KP_Tab, ModifierType::empty(), false),
            Some(vec![b'\t'])
        );
        assert_eq!(
            translate_key(Key::KP_Delete, ModifierType::empty(), false),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn test_translate_shift_tab_and_modified_c0_keys() {
        // Shift+Tab via ISO_Left_Tab (XKB consumed shift) or Key::Tab + SHIFT_MASK
        assert_eq!(
            translate_key(Key::ISO_Left_Tab, ModifierType::empty(), false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            translate_key(Key::ISO_Left_Tab, ModifierType::SHIFT_MASK, false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            translate_key(Key::Tab, ModifierType::SHIFT_MASK, false),
            Some(b"\x1b[Z".to_vec())
        );
        // Alt+Backspace -> ESC DEL, Ctrl+Backspace -> BS (0x08)
        assert_eq!(
            translate_key(Key::BackSpace, ModifierType::ALT_MASK, false),
            Some(vec![0x1b, 0x7f])
        );
        assert_eq!(
            translate_key(Key::BackSpace, ModifierType::CONTROL_MASK, false),
            Some(vec![0x08])
        );
        // Alt+Return -> ESC CR, Shift+Return -> CSI 13;2u
        assert_eq!(
            translate_key(Key::Return, ModifierType::ALT_MASK, false),
            Some(vec![0x1b, b'\r'])
        );
        assert_eq!(
            translate_key(Key::Return, ModifierType::SHIFT_MASK, false),
            Some(b"\x1b[13;2u".to_vec())
        );
    }

    #[test]
    fn test_translate_non_alpha_ctrl_and_modified_fkeys() {
        assert_eq!(
            translate_key(Key::space, ModifierType::CONTROL_MASK, false),
            Some(vec![0x00])
        );
        assert_eq!(
            translate_key(Key::slash, ModifierType::CONTROL_MASK, false),
            Some(vec![0x1f])
        );
        assert_eq!(
            translate_key(Key::underscore, ModifierType::CONTROL_MASK, false),
            Some(vec![0x1f])
        );
        assert_eq!(
            translate_key(Key::bracketright, ModifierType::CONTROL_MASK, false),
            Some(vec![0x1d])
        );
        assert_eq!(
            translate_key(Key::F1, ModifierType::SHIFT_MASK, false),
            Some(b"\x1b[1;2P".to_vec())
        );
        assert_eq!(
            translate_key(Key::F5, ModifierType::CONTROL_MASK, false),
            Some(b"\x1b[15;5~".to_vec())
        );
    }

    #[test]
    fn test_kitty_keyboard_protocol_disambiguate() {
        let mode = TermMode::DISAMBIGUATE_ESC_CODES;
        // Unmodified Enter/Tab/Backspace stay legacy under DISAMBIGUATE_ESC_CODES
        assert_eq!(
            translate_key_with_mode(Key::Return, ModifierType::empty(), mode),
            Some(vec![b'\r'])
        );
        assert_eq!(
            translate_key_with_mode(Key::Tab, ModifierType::empty(), mode),
            Some(vec![b'\t'])
        );
        assert_eq!(
            translate_key_with_mode(Key::BackSpace, ModifierType::empty(), mode),
            Some(vec![0x7f])
        );
        // Escape is always disambiguated as CSI 27 u
        assert_eq!(
            translate_key_with_mode(Key::Escape, ModifierType::empty(), mode),
            Some(b"\x1b[27u".to_vec())
        );
        // Shift+Return -> CSI 13;2u (multiline input in OpenCode / modern TUIs)
        assert_eq!(
            translate_key_with_mode(Key::Return, ModifierType::SHIFT_MASK, mode),
            Some(b"\x1b[13;2u".to_vec())
        );
        // Shift+Tab (ISO_Left_Tab) -> CSI 9;2u
        assert_eq!(
            translate_key_with_mode(Key::ISO_Left_Tab, ModifierType::SHIFT_MASK, mode),
            Some(b"\x1b[9;2u".to_vec())
        );
        // Ctrl+I -> CSI 105;5u (distinct from Tab)
        assert_eq!(
            translate_key_with_mode(Key::i, ModifierType::CONTROL_MASK, mode),
            Some(b"\x1b[105;5u".to_vec())
        );
        // Ctrl+Shift+A -> uses lowercase codepoint 97 with modifier 6
        assert_eq!(
            translate_key_with_mode(
                Key::A,
                ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK,
                mode
            ),
            Some(b"\x1b[97;6u".to_vec())
        );
    }
}
