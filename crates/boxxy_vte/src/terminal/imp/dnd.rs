use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

impl super::TerminalWidget {
    pub(super) fn setup_drag_and_drop(&self) {
        let obj = self.obj();
        // Use COPY only so file managers and editors never treat a drop into the
        // terminal as a destructive move on the drag source.
        let drop_target = gtk4::DropTarget::new(glib::Type::INVALID, gtk4::gdk::DragAction::COPY);
        drop_target.set_types(&[
            gtk4::gdk::FileList::static_type(),
            gtk4::gio::File::static_type(),
            glib::Type::STRING,
        ]);

        drop_target.connect_drop(glib::clone!(
            #[weak]
            obj,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Some(payload) = extract_drop_payload(value) else {
                    return false;
                };
                obj.grab_focus();
                obj.imp().paste_text(&payload);
                true
            }
        ));

        obj.add_controller(drop_target);
    }
}

fn extract_drop_payload(value: &glib::Value) -> Option<String> {
    if let Ok(file_list) = value.get::<gtk4::gdk::FileList>() {
        format_gio_files(&file_list.files())
    } else if let Ok(file) = value.get::<gtk4::gio::File>() {
        format_gio_files(std::slice::from_ref(&file))
    } else if let Ok(text) = value.get::<String>() {
        format_dropped_string(&text)
    } else {
        None
    }
}

fn format_gio_files(files: &[gtk4::gio::File]) -> Option<String> {
    let mut parts = Vec::with_capacity(files.len());
    for file in files {
        if let Some(path) = file.path() {
            parts.push(shell_quote_str(&path.to_string_lossy()));
        } else {
            let uri = file.uri();
            if !uri.is_empty() {
                parts.push(shell_quote_str(uri.as_str()));
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn format_dropped_string(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    // Some X11 / portal drag sources deliver dropped files as `text/uri-list`
    // or newline-delimited `file://` URIs over `GString`. Convert those to
    // local paths so the shell receives usable filesystem paths rather than URIs.
    let uri_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    if !uri_lines.is_empty() && uri_lines.iter().all(|line| line.starts_with("file://")) {
        let files: Vec<gtk4::gio::File> = uri_lines
            .into_iter()
            .map(gtk4::gio::File::for_uri)
            .collect();
        return format_gio_files(&files);
    }

    Some(text.to_string())
}

fn is_shell_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '/' | '.' | '_' | '-' | '+' | '@' | ':' | ',' | '%')
        || (!c.is_ascii() && c.is_alphanumeric())
}

fn shell_quote_str(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if !s.starts_with('-') && s.chars().all(is_shell_safe_char) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::{format_dropped_string, format_gio_files, shell_quote_str};
    use gtk4::gio;

    #[test]
    fn clean_paths_remain_unquoted() {
        assert_eq!(
            shell_quote_str("/tmp/workspace/assets/banner_v2.png"),
            "/tmp/workspace/assets/banner_v2.png"
        );
        assert_eq!(
            shell_quote_str("/opt/app-1.0/bin/run+debug@2x.sh"),
            "/opt/app-1.0/bin/run+debug@2x.sh"
        );
    }

    #[test]
    fn paths_with_spaces_or_metacharacters_are_single_quoted() {
        assert_eq!(
            shell_quote_str("/tmp/My Assets/photo (1).png"),
            "'/tmp/My Assets/photo (1).png'"
        );
        assert_eq!(
            shell_quote_str("/tmp/it's a file.txt"),
            "'/tmp/it'\\''s a file.txt'"
        );
        assert_eq!(
            shell_quote_str("/tmp/$HOME;echo_test"),
            "'/tmp/$HOME;echo_test'"
        );
        assert_eq!(shell_quote_str("-flag-file.txt"), "'-flag-file.txt'");
        assert_eq!(shell_quote_str(""), "''");
    }

    #[test]
    fn multiple_gio_files_join_with_spaces() {
        let f1 = gio::File::for_path("/tmp/project/alpha.png");
        let f2 = gio::File::for_path("/tmp/project/beta gamma.png");
        let formatted = format_gio_files(&[f1, f2]);
        assert_eq!(
            formatted.as_deref(),
            Some("/tmp/project/alpha.png '/tmp/project/beta gamma.png'")
        );
    }

    #[test]
    fn file_uri_list_string_converts_to_local_paths() {
        let raw = "# comment\r\nfile:///tmp/project/image.png\r\nfile:///tmp/My%20Docs/notes.md\r\n";
        assert_eq!(
            format_dropped_string(raw).as_deref(),
            Some("/tmp/project/image.png '/tmp/My Docs/notes.md'")
        );
    }

    #[test]
    fn non_file_strings_pass_through_unchanged() {
        assert_eq!(
            format_dropped_string("https://example.com/docs?a=1&b=2").as_deref(),
            Some("https://example.com/docs?a=1&b=2")
        );
        assert_eq!(
            format_dropped_string("echo hello").as_deref(),
            Some("echo hello")
        );
        assert_eq!(format_dropped_string(""), None);
    }
}
