use crate::engine::term::TermMode;
use gtk4::gdk::{Key, ModifierType};

/// Key event transition type for the Kitty Keyboard Protocol (`REPORT_EVENT_TYPES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyEventType {
    Press = 1,
    Repeat = 2,
    Release = 3,
}

/// Translates a GTK key press event into an ANSI byte sequence for the PTY using
/// legacy xterm/VT100 encoding rules (with optional `APP_CURSOR` mode).
pub fn translate_key(key: Key, modifiers: ModifierType, is_app_cursor: bool) -> Option<Vec<u8>> {
    let mut mode = TermMode::NONE;
    if is_app_cursor {
        mode.insert(TermMode::APP_CURSOR);
    }
    translate_key_with_mode(key, modifiers, mode)
}

/// Translates a GTK key press event into an ANSI byte sequence for the PTY,
/// respecting legacy modes (`APP_CURSOR`), `modifyOtherKeys`, and the Kitty Keyboard Protocol.
pub fn translate_key_with_mode(
    key: Key,
    modifiers: ModifierType,
    mode: TermMode,
) -> Option<Vec<u8>> {
    translate_key_event(key, modifiers, mode, KeyEventType::Press)
}

/// Translates a GTK key event (`Press`, `Repeat`, or `Release`) into an ANSI byte
/// sequence for the PTY according to active `TermMode` flags.
pub fn translate_key_event(
    key: Key,
    modifiers: ModifierType,
    mode: TermMode,
    event_type: KeyEventType,
) -> Option<Vec<u8>> {
    let report_events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    if event_type == KeyEventType::Release && !report_events {
        return None;
    }

    let mut bytes = Vec::new();

    let ctrl = modifiers.contains(ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(ModifierType::ALT_MASK);
    let shift = modifiers.contains(ModifierType::SHIFT_MASK);
    let super_mod = modifiers.intersects(ModifierType::SUPER_MASK | ModifierType::META_MASK);
    let is_app_cursor = mode.contains(TermMode::APP_CURSOR);
    let kitty_disambiguate = mode.intersects(
        TermMode::DISAMBIGUATE_ESC_CODES
            | TermMode::REPORT_ALL_KEYS_AS_ESC
            | TermMode::MODIFY_OTHER_KEYS_2,
    );
    let kitty_all_esc = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let report_alt_keys = mode.contains(TermMode::REPORT_ALTERNATE_KEYS);
    let report_assoc_text = mode.contains(TermMode::REPORT_ASSOCIATED_TEXT);
    let needs_event_suffix = report_events && event_type != KeyEventType::Press;

    // Standard modifier code: 1=none, +1=shift, +2=alt, +4=ctrl, +8=super/meta
    let modifier_code = {
        let mut code = 1u8;
        if shift {
            code += 1;
        }
        if alt {
            code += 2;
        }
        if ctrl {
            code += 4;
        }
        if super_mod {
            code += 8;
        }
        code
    };

    let has_modifier = modifier_code > 1;
    let format_mod_param = |mod_code: u8| -> String {
        if needs_event_suffix {
            format!("{}:{}", mod_code, event_type as u8)
        } else if mod_code > 1 {
            format!("{}", mod_code)
        } else {
            String::new()
        }
    };
    let mod_param = format_mod_param(modifier_code);

    let encode_csi_u = |out: &mut Vec<u8>, key_code: u32, mod_str: &str| {
        if mod_str.is_empty() {
            out.extend_from_slice(format!("\x1b[{key_code}u").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{key_code};{mod_str}u").as_bytes());
        }
    };

    let encode_csi_tilde = |out: &mut Vec<u8>, num: u8, mod_str: &str| {
        if mod_str.is_empty() {
            out.extend_from_slice(format!("\x1b[{num}~").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[{num};{mod_str}~").as_bytes());
        }
    };

    let encode_csi_letter =
        |out: &mut Vec<u8>, letter: char, app_mode_ss3: bool, mod_str: &str| {
            if !mod_str.is_empty() {
                out.extend_from_slice(format!("\x1b[1;{mod_str}{letter}").as_bytes());
            } else if app_mode_ss3 {
                out.extend_from_slice(format!("\x1bO{letter}").as_bytes());
            } else {
                out.extend_from_slice(format!("\x1b[{letter}").as_bytes());
            }
        };

    // Handle special keys
    match key {
        Key::Return | Key::KP_Enter => {
            if (kitty_disambiguate && (has_modifier || kitty_all_esc))
                || needs_event_suffix
                || shift
                || ctrl
                || super_mod
            {
                encode_csi_u(&mut bytes, 13, &mod_param);
            } else {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(b'\r');
            }
            return Some(bytes);
        }
        Key::BackSpace => {
            if (kitty_disambiguate && (has_modifier || kitty_all_esc))
                || needs_event_suffix
                || super_mod
            {
                encode_csi_u(&mut bytes, 127, &mod_param);
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
            encode_csi_tilde(&mut bytes, 2, &mod_param);
            return Some(bytes);
        }
        Key::Delete | Key::KP_Delete => {
            encode_csi_tilde(&mut bytes, 3, &mod_param);
            return Some(bytes);
        }
        Key::Tab | Key::KP_Tab | Key::ISO_Left_Tab => {
            // On GTK4/XKB, Shift+Tab produces Key::ISO_Left_Tab (often with SHIFT_MASK consumed).
            let tab_shift = shift || key == Key::ISO_Left_Tab;
            let tab_mod_code = 1
                + if tab_shift { 1 } else { 0 }
                + if alt { 2 } else { 0 }
                + if ctrl { 4 } else { 0 }
                + if super_mod { 8 } else { 0 };
            let tab_mod_param = format_mod_param(tab_mod_code);
            if (kitty_disambiguate && (tab_mod_code > 1 || kitty_all_esc))
                || needs_event_suffix
                || super_mod
            {
                encode_csi_u(&mut bytes, 9, &tab_mod_param);
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
            if kitty_disambiguate || needs_event_suffix || super_mod {
                encode_csi_u(&mut bytes, 27, &mod_param);
            } else {
                if alt {
                    bytes.push(0x1b);
                }
                bytes.push(0x1b);
            }
            return Some(bytes);
        }
        Key::Up | Key::KP_Up => {
            encode_csi_letter(&mut bytes, 'A', is_app_cursor, &mod_param);
            return Some(bytes);
        }
        Key::Down | Key::KP_Down => {
            encode_csi_letter(&mut bytes, 'B', is_app_cursor, &mod_param);
            return Some(bytes);
        }
        Key::Right | Key::KP_Right => {
            encode_csi_letter(&mut bytes, 'C', is_app_cursor, &mod_param);
            return Some(bytes);
        }
        Key::Left | Key::KP_Left => {
            encode_csi_letter(&mut bytes, 'D', is_app_cursor, &mod_param);
            return Some(bytes);
        }
        Key::Home | Key::KP_Home => {
            encode_csi_letter(&mut bytes, 'H', is_app_cursor, &mod_param);
            return Some(bytes);
        }
        Key::End | Key::KP_End => {
            encode_csi_letter(&mut bytes, 'F', is_app_cursor, &mod_param);
            return Some(bytes);
        }
        Key::Page_Up | Key::KP_Page_Up => {
            encode_csi_tilde(&mut bytes, 5, &mod_param);
            return Some(bytes);
        }
        Key::Page_Down | Key::KP_Page_Down => {
            encode_csi_tilde(&mut bytes, 6, &mod_param);
            return Some(bytes);
        }
        Key::KP_Begin => {
            encode_csi_letter(&mut bytes, 'E', false, &mod_param);
            return Some(bytes);
        }
        Key::F1 => {
            encode_csi_letter(&mut bytes, 'P', true, &mod_param);
            return Some(bytes);
        }
        Key::F2 => {
            encode_csi_letter(&mut bytes, 'Q', true, &mod_param);
            return Some(bytes);
        }
        Key::F3 => {
            encode_csi_letter(&mut bytes, 'R', true, &mod_param);
            return Some(bytes);
        }
        Key::F4 => {
            encode_csi_letter(&mut bytes, 'S', true, &mod_param);
            return Some(bytes);
        }
        Key::F5 => {
            encode_csi_tilde(&mut bytes, 15, &mod_param);
            return Some(bytes);
        }
        Key::F6 => {
            encode_csi_tilde(&mut bytes, 17, &mod_param);
            return Some(bytes);
        }
        Key::F7 => {
            encode_csi_tilde(&mut bytes, 18, &mod_param);
            return Some(bytes);
        }
        Key::F8 => {
            encode_csi_tilde(&mut bytes, 19, &mod_param);
            return Some(bytes);
        }
        Key::F9 => {
            encode_csi_tilde(&mut bytes, 20, &mod_param);
            return Some(bytes);
        }
        Key::F10 => {
            encode_csi_tilde(&mut bytes, 21, &mod_param);
            return Some(bytes);
        }
        Key::F11 => {
            encode_csi_tilde(&mut bytes, 23, &mod_param);
            return Some(bytes);
        }
        Key::F12 => {
            encode_csi_tilde(&mut bytes, 24, &mod_param);
            return Some(bytes);
        }
        // Standalone modifier keys are only reported when REPORT_ALL_KEYS_AS_ESC (8) is active
        Key::Shift_L | Key::Shift_R if kitty_all_esc => {
            let code = if key == Key::Shift_L { 57441 } else { 57447 };
            let m = if event_type == KeyEventType::Release {
                modifier_code.saturating_sub(if shift { 1 } else { 0 })
            } else if !shift {
                modifier_code + 1
            } else {
                modifier_code
            };
            encode_csi_u(&mut bytes, code, &format_mod_param(m.max(1)));
            return Some(bytes);
        }
        Key::Control_L | Key::Control_R if kitty_all_esc => {
            let code = if key == Key::Control_L { 57442 } else { 57448 };
            let m = if event_type == KeyEventType::Release {
                modifier_code.saturating_sub(if ctrl { 4 } else { 0 })
            } else if !ctrl {
                modifier_code + 4
            } else {
                modifier_code
            };
            encode_csi_u(&mut bytes, code, &format_mod_param(m.max(1)));
            return Some(bytes);
        }
        Key::Alt_L | Key::Alt_R if kitty_all_esc => {
            let code = if key == Key::Alt_L { 57443 } else { 57449 };
            let m = if event_type == KeyEventType::Release {
                modifier_code.saturating_sub(if alt { 2 } else { 0 })
            } else if !alt {
                modifier_code + 2
            } else {
                modifier_code
            };
            encode_csi_u(&mut bytes, code, &format_mod_param(m.max(1)));
            return Some(bytes);
        }
        Key::Super_L | Key::Super_R | Key::Meta_L | Key::Meta_R if kitty_all_esc => {
            let code = if matches!(key, Key::Super_L | Key::Meta_L) {
                57444
            } else {
                57450
            };
            let m = if event_type == KeyEventType::Release {
                modifier_code.saturating_sub(if super_mod { 8 } else { 0 })
            } else if !super_mod {
                modifier_code + 8
            } else {
                modifier_code
            };
            encode_csi_u(&mut bytes, code, &format_mod_param(m.max(1)));
            return Some(bytes);
        }
        _ => {}
    }

    // Handle printable characters and Control / Alt / Super combinations
    if let Some(ch) = key.to_unicode() {
        let use_csi_u = (kitty_disambiguate && (ctrl || alt || super_mod))
            || (mode.contains(TermMode::MODIFY_OTHER_KEYS_1) && (ctrl || alt || super_mod))
            || kitty_all_esc
            || needs_event_suffix;

        if use_csi_u {
            let base_char = ch.to_lowercase().next().unwrap_or(ch);
            let base_cp = base_char as u32;
            let shifted_char = if shift && ch == base_char && ch.is_ascii_lowercase() {
                ch.to_ascii_uppercase()
            } else {
                ch
            };
            let shifted_cp = shifted_char as u32;

            let key_field = if report_alt_keys && shift && shifted_cp != base_cp {
                format!("{base_cp}:{shifted_cp}")
            } else {
                format!("{base_cp}")
            };

            let include_assoc_text = report_assoc_text
                && event_type != KeyEventType::Release
                && !ctrl
                && !alt
                && !super_mod;

            if include_assoc_text {
                bytes.extend_from_slice(
                    format!("\x1b[{key_field};{mod_param};{shifted_cp}u").as_bytes(),
                );
            } else if !mod_param.is_empty() {
                bytes.extend_from_slice(format!("\x1b[{key_field};{mod_param}u").as_bytes());
            } else {
                bytes.extend_from_slice(format!("\x1b[{key_field}u").as_bytes());
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
            if alt {
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

    #[test]
    fn test_kitty_event_types_alternate_keys_and_associated_text() {
        // 1. Key release is ignored when REPORT_EVENT_TYPES is not active
        assert_eq!(
            translate_key_event(
                Key::a,
                ModifierType::empty(),
                TermMode::DISAMBIGUATE_ESC_CODES,
                KeyEventType::Release
            ),
            None
        );

        // 2. Key release under DISAMBIGUATE_ESC_CODES | REPORT_EVENT_TYPES
        let ev_mode = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(
            translate_key_event(Key::a, ModifierType::empty(), ev_mode, KeyEventType::Release),
            Some(b"\x1b[97;1:3u".to_vec())
        );
        assert_eq!(
            translate_key_event(
                Key::a,
                ModifierType::CONTROL_MASK,
                ev_mode,
                KeyEventType::Release
            ),
            Some(b"\x1b[97;5:3u".to_vec())
        );
        assert_eq!(
            translate_key_event(
                Key::Up,
                ModifierType::empty(),
                ev_mode,
                KeyEventType::Release
            ),
            Some(b"\x1b[1;1:3A".to_vec())
        );
        assert_eq!(
            translate_key_event(
                Key::Page_Up,
                ModifierType::empty(),
                ev_mode,
                KeyEventType::Repeat
            ),
            Some(b"\x1b[5;1:2~".to_vec())
        );

        // 3. REPORT_ALTERNATE_KEYS (4) encodes base:shifted codepoints
        let alt_mode = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALTERNATE_KEYS;
        assert_eq!(
            translate_key_with_mode(
                Key::A,
                ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK,
                alt_mode
            ),
            Some(b"\x1b[97:65;6u".to_vec())
        );

        // 4. REPORT_ALL_KEYS_AS_ESC (8) | REPORT_ALTERNATE_KEYS (4) | REPORT_ASSOCIATED_TEXT (16)
        let full_mode = TermMode::REPORT_ALL_KEYS_AS_ESC
            | TermMode::REPORT_ALTERNATE_KEYS
            | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(
            translate_key_with_mode(Key::a, ModifierType::empty(), full_mode),
            Some(b"\x1b[97;;97u".to_vec())
        );
        assert_eq!(
            translate_key_with_mode(Key::A, ModifierType::SHIFT_MASK, full_mode),
            Some(b"\x1b[97:65;2;65u".to_vec())
        );

        // 5. Super modifier bit (+8 -> modifier 9) and standalone modifier keys under REPORT_ALL_KEYS_AS_ESC
        assert_eq!(
            translate_key_with_mode(Key::k, ModifierType::SUPER_MASK, TermMode::DISAMBIGUATE_ESC_CODES),
            Some(b"\x1b[107;9u".to_vec())
        );
        assert_eq!(
            translate_key_with_mode(
                Key::Shift_L,
                ModifierType::empty(),
                TermMode::REPORT_ALL_KEYS_AS_ESC
            ),
            Some(b"\x1b[57441;2u".to_vec())
        );
    }
}
