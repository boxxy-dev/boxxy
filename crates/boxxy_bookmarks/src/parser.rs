use regex::Regex;
use std::sync::LazyLock;

/// Compiled once per process rather than on every keystroke or card bind.
static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\{([^}]+)\}\}\}").expect("valid placeholder regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkTemplate {
    pub raw_script: String,
    pub placeholders: Vec<String>,
}

impl BookmarkTemplate {
    #[must_use]
    pub fn parse(script: &str) -> Self {
        let mut placeholders = Vec::new();

        for cap in PLACEHOLDER_RE.captures_iter(script) {
            let name = cap[1].trim().to_string();
            if !name.is_empty() && !placeholders.contains(&name) {
                placeholders.push(name);
            }
        }

        Self {
            raw_script: script.to_string(),
            placeholders,
        }
    }

    #[must_use]
    pub fn render(&self, values_str: &str) -> String {
        let values: Vec<String> = values_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();

        let mut rendered = self.raw_script.clone();

        for (i, name) in self.placeholders.iter().enumerate() {
            if let Some(val) = values.get(i) {
                let pattern = format!("{{{{{{{name}}}}}}}");
                rendered = rendered.replace(&pattern, val);
            }
        }

        rendered
    }

    #[must_use]
    pub fn get_default_input(&self) -> String {
        self.placeholders.join(", ")
    }

    #[must_use]
    pub fn has_placeholders(&self) -> bool {
        !self.placeholders.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unique_placeholders_in_order() {
        let script = "#!/bin/bash\nssh {{{ user }}}@{{{host}}} -p {{{port}}} # {{{user}}}";
        let tpl = BookmarkTemplate::parse(script);
        assert_eq!(tpl.placeholders, vec!["user", "host", "port"]);
        assert!(tpl.has_placeholders());
        assert_eq!(tpl.get_default_input(), "user, host, port");
    }

    #[test]
    fn renders_placeholders_from_comma_separated_values() {
        let script = "scp {{{src}}} {{{user}}}@{{{host}}}:{{{dest}}}";
        let tpl = BookmarkTemplate::parse(script);
        let rendered = tpl.render("archive.tar.gz, root, example.com, /tmp");
        assert_eq!(rendered, "scp archive.tar.gz root@example.com:/tmp");
    }
}
