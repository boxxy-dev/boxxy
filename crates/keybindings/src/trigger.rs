use gtk4::gdk;
use gtk4::glib::translate::IntoGlib;
use std::fmt::{self, Write as _};

/// Standard modifier mask considered significant for shortcut matching and persistence.
pub const ACCEL_MOD_MASK: gdk::ModifierType = gdk::ModifierType::CONTROL_MASK
    .union(gdk::ModifierType::SHIFT_MASK)
    .union(gdk::ModifierType::ALT_MASK)
    .union(gdk::ModifierType::SUPER_MASK);

/// A normalized single key combination, optionally marked `performable:`.
///
/// Keyvals are normalized to lowercase (matching `GtkKeyvalTrigger` internals) so
/// `"Ctrl+Shift+V"`, `"<Control><Shift>v"`, and `"<Ctrl><Shift>V"` compare equal.
/// Parsed via `gdk::Key::from_name` rather than `gtk::accelerator_parse` so it works
/// headlessly in `boxxy-cli`, unit tests, and background `spawn_blocking` tasks
/// without requiring `gtk::init()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub keyval: gdk::Key,
    pub modifiers: gdk::ModifierType,
    pub performable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerParseError {
    pub input: String,
    pub reason: String,
}

impl fmt::Display for TriggerParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid shortcut '{}': {}", self.input, self.reason)
    }
}

impl std::error::Error for TriggerParseError {}

impl Chord {
    /// Constructs a canonical `Chord` from a GDK keyval and modifier set.
    #[must_use]
    pub fn new(keyval: gdk::Key, modifiers: gdk::ModifierType, performable: bool) -> Self {
        let mut clean_mods = modifiers & ACCEL_MOD_MASK;
        let normalized_key = if keyval == gdk::Key::ISO_Left_Tab {
            clean_mods.insert(gdk::ModifierType::SHIFT_MASK);
            gdk::Key::Tab
        } else {
            keyval.to_lower()
        };

        Self {
            keyval: normalized_key,
            modifiers: clean_mods,
            performable,
        }
    }

    /// Identity key ignoring `performable:` for collision and override checks.
    #[must_use]
    pub const fn signature(self) -> (gdk::Key, gdk::ModifierType) {
        (self.keyval, self.modifiers)
    }

    /// Returns `true` if this chord is a bare `F1`..=`F12` press with no modifiers.
    #[must_use]
    pub const fn is_bare_function_key(self) -> bool {
        self.modifiers.is_empty()
            && matches!(
                self.keyval,
                gdk::Key::F1
                    | gdk::Key::F2
                    | gdk::Key::F3
                    | gdk::Key::F4
                    | gdk::Key::F5
                    | gdk::Key::F6
                    | gdk::Key::F7
                    | gdk::Key::F8
                    | gdk::Key::F9
                    | gdk::Key::F10
                    | gdk::Key::F11
                    | gdk::Key::F12
            )
    }

    /// Parses either human syntax (`"Ctrl+Shift+V"`, `"Ctrl+/"`, `"performable:Ctrl+C"`)
    /// or GTK accelerator syntax (`"<Ctrl><Shift>v"`, `"<Control>slash"`).
    ///
    /// # Errors
    /// Returns `TriggerParseError` if `raw` is empty, lacks a non-modifier key, or names
    /// an unrecognized modifier or keysym.
    pub fn parse(raw: &str) -> Result<Self, TriggerParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(TriggerParseError {
                input: raw.to_string(),
                reason: "shortcut cannot be empty".to_string(),
            });
        }

        let (performable, body) = strip_performable_prefix(trimmed);
        if body.is_empty() {
            return Err(TriggerParseError {
                input: raw.to_string(),
                reason: "missing key after 'performable:'".to_string(),
            });
        }

        let (mods, key_token) = if body.starts_with('<') {
            parse_gtk_angle_syntax(body)
        } else {
            parse_human_plus_syntax(body)
        }
        .map_err(|reason| TriggerParseError {
            input: raw.to_string(),
            reason,
        })?;

        let Some(keyval) = gdk::Key::from_name(&key_token) else {
            return Err(TriggerParseError {
                input: raw.to_string(),
                reason: format!("unrecognized key '{key_token}'"),
            });
        };

        if keyval == gdk::Key::VoidSymbol || is_standalone_modifier(keyval) {
            return Err(TriggerParseError {
                input: raw.to_string(),
                reason: "shortcut must include a non-modifier key".to_string(),
            });
        }

        Ok(Self::new(keyval, mods, performable))
    }

    /// Formats as a GTK accelerator string suitable for `adw::ShortcutLabel` and `GtkShortcutTrigger`
    /// without requiring `gtk::init()`.
    #[must_use]
    pub fn to_gtk_accel(self) -> String {
        let mut out = String::with_capacity(28);
        if self.modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            out.push_str("<Control>");
        }
        if self.modifiers.contains(gdk::ModifierType::ALT_MASK) {
            out.push_str("<Alt>");
        }
        if self.modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            out.push_str("<Shift>");
        }
        if self.modifiers.contains(gdk::ModifierType::SUPER_MASK) {
            out.push_str("<Super>");
        }
        if let Some(name) = self.keyval.name() {
            out.push_str(&name);
        } else {
            let _ = write!(out, "0x{:x}", self.keyval.into_glib());
        }
        out
    }

    /// Formats in clean human-readable syntax (`"Ctrl+Shift+V"`, `"performable:Ctrl+C"`, `"Ctrl+/"`)
    /// for `keybindings.toml` and CLI output.
    #[must_use]
    pub fn to_human_string(self) -> String {
        let mut out = String::with_capacity(24);
        if self.performable {
            out.push_str("performable:");
        }
        if self.modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            out.push_str("Ctrl+");
        }
        if self.modifiers.contains(gdk::ModifierType::ALT_MASK) {
            out.push_str("Alt+");
        }
        if self.modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
            out.push_str("Shift+");
        }
        if self.modifiers.contains(gdk::ModifierType::SUPER_MASK) {
            out.push_str("Super+");
        }
        out.push_str(&human_key_name(self.keyval));
        out
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_human_string())
    }
}

#[must_use]
pub fn is_unbind_keyword(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "none" | "disabled" | "unbind" | "unbound" | "never"
    )
}

#[must_use]
pub const fn is_standalone_modifier(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
    )
}

fn strip_performable_prefix(s: &str) -> (bool, &str) {
    const PREFIX: &str = "performable:";
    if s.len() >= PREFIX.len() && s[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        (true, s[PREFIX.len()..].trim())
    } else {
        (false, s)
    }
}

/// Parses `<Control><Shift>v` / `<Ctrl>slash` / `<Ctrl>/` into `(ModifierType, keysym_name)`.
fn parse_gtk_angle_syntax(mut rest: &str) -> Result<(gdk::ModifierType, String), String> {
    let mut mods = gdk::ModifierType::empty();
    while rest.starts_with('<') {
        let Some(end) = rest.find('>') else {
            return Err("unclosed '<' in modifier".to_string());
        };
        let mod_name = rest[1..end].trim();
        let Some(flag) = map_modifier_token(mod_name) else {
            return Err(format!("unknown modifier '<{mod_name}>'"));
        };
        mods.insert(flag);
        rest = rest[end + 1..].trim();
    }

    let key_token =
        map_key_token(rest).ok_or_else(|| "missing key after modifier".to_string())?;
    Ok((mods, key_token))
}

/// Parses `"Ctrl+Shift+V"` / `"Ctrl++"` / `"Ctrl+-"` into `(ModifierType, keysym_name)`.
fn parse_human_plus_syntax(input: &str) -> Result<(gdk::ModifierType, String), String> {
    let mut mods = gdk::ModifierType::empty();
    let mut rest = input.trim();

    // Peel recognized `Modifier+` prefixes from left to right while preserving trailing `+` keys
    // (so `"Ctrl++"` peels `"Ctrl+"` and leaves `rest == "+"`).
    while let Some((head, tail)) = rest.split_once('+') {
        if tail.is_empty() {
            break;
        }
        let Some(flag) = map_modifier_token(head.trim()) else {
            return Err(format!("unknown modifier '{}'", head.trim()));
        };
        mods.insert(flag);
        rest = tail.trim();
    }

    let key_token =
        map_key_token(rest).ok_or_else(|| format!("empty or invalid key token '{rest}'"))?;
    Ok((mods, key_token))
}

fn map_modifier_token(token: &str) -> Option<gdk::ModifierType> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "primary" | "ctl" => Some(gdk::ModifierType::CONTROL_MASK),
        "shift" | "shft" => Some(gdk::ModifierType::SHIFT_MASK),
        "alt" | "opt" | "option" | "mod1" => Some(gdk::ModifierType::ALT_MASK),
        "super" | "meta" | "win" | "cmd" | "command" | "mod4" => Some(gdk::ModifierType::SUPER_MASK),
        _ => None,
    }
}

fn map_key_token(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }

    let mapped = match t {
        "+" => "plus",
        "-" => "minus",
        "=" => "equal",
        "_" => "underscore",
        "/" => "slash",
        "?" => "question",
        "," => "comma",
        "." => "period",
        ";" => "semicolon",
        ":" => "colon",
        "'" => "apostrophe",
        "\"" => "quotedbl",
        "[" => "bracketleft",
        "]" => "bracketright",
        "{" => "braceleft",
        "}" => "braceright",
        "|" => "bar",
        "\\" => "backslash",
        "`" => "grave",
        "~" => "asciitilde",
        "!" => "exclam",
        "@" => "at",
        "#" => "numbersign",
        "$" => "dollar",
        "%" => "percent",
        "^" => "asciicircum",
        "&" => "ampersand",
        "*" => "asterisk",
        "(" => "parenleft",
        ")" => "parenright",
        other => {
            if other.len() == 1 {
                let ch = other.as_bytes()[0] as char;
                if ch.is_ascii_alphabetic() {
                    return Some(ch.to_ascii_lowercase().to_string());
                }
                if ch.is_ascii_digit() {
                    return Some(other.to_string());
                }
            }
            let lower = other.to_ascii_lowercase();
            match lower.as_str() {
                "plus" => "plus",
                "minus" | "hyphen" => "minus",
                "equal" | "equals" => "equal",
                "underscore" => "underscore",
                "slash" => "slash",
                "question" => "question",
                "comma" => "comma",
                "period" | "dot" => "period",
                "semicolon" => "semicolon",
                "colon" => "colon",
                "apostrophe" | "quote" => "apostrophe",
                "grave" | "backtick" | "backquote" => "grave",
                "asciitilde" | "tilde" => "asciitilde",
                "bracketleft" => "bracketleft",
                "bracketright" => "bracketright",
                "backslash" => "backslash",
                "bar" | "pipe" => "bar",
                "enter" | "return" => "Return",
                "esc" | "escape" => "Escape",
                "backspace" | "bs" => "BackSpace",
                "del" | "delete" => "Delete",
                "ins" | "insert" => "Insert",
                "tab" => "Tab",
                "space" => "space",
                "up" | "arrowup" => "Up",
                "down" | "arrowdown" => "Down",
                "left" | "arrowleft" => "Left",
                "right" | "arrowright" => "Right",
                "home" => "Home",
                "end" => "End",
                "pageup" | "pgup" | "page_up" => "Page_Up",
                "pagedown" | "pgdn" | "pgdown" | "page_down" => "Page_Down",
                _ if lower.starts_with('f')
                    && lower[1..].parse::<u8>().is_ok_and(|n| (1..=35).contains(&n)) =>
                {
                    return Some(other.to_ascii_uppercase());
                }
                _ => return Some(other.to_string()),
            }
        }
    };
    Some(mapped.to_string())
}

fn human_key_name(key: gdk::Key) -> String {
    match key {
        gdk::Key::plus => "+".to_string(),
        gdk::Key::minus => "-".to_string(),
        gdk::Key::equal => "=".to_string(),
        gdk::Key::underscore => "_".to_string(),
        gdk::Key::slash => "/".to_string(),
        gdk::Key::question => "?".to_string(),
        gdk::Key::comma => ",".to_string(),
        gdk::Key::period => ".".to_string(),
        gdk::Key::semicolon => ";".to_string(),
        gdk::Key::colon => ":".to_string(),
        gdk::Key::apostrophe => "'".to_string(),
        gdk::Key::grave => "`".to_string(),
        gdk::Key::asciitilde => "~".to_string(),
        gdk::Key::bracketleft => "[".to_string(),
        gdk::Key::bracketright => "]".to_string(),
        gdk::Key::backslash => "\\".to_string(),
        gdk::Key::bar => "|".to_string(),
        gdk::Key::Return => "Enter".to_string(),
        gdk::Key::Escape => "Esc".to_string(),
        gdk::Key::BackSpace => "Backspace".to_string(),
        gdk::Key::Page_Up => "PageUp".to_string(),
        gdk::Key::Page_Down => "PageDown".to_string(),
        gdk::Key::space => "Space".to_string(),
        other => {
            if let Some(ch) = other.to_unicode()
                && ch.is_ascii_alphanumeric()
            {
                return ch.to_ascii_uppercase().to_string();
            }
            other
                .name()
                .map_or_else(|| format!("0x{:x}", other.into_glib()), |n| n.to_string())
        }
    }
}
