pub mod editor;
pub mod manager;
pub mod parser;
pub mod sidebar;
pub mod tab;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Supported script languages for bookmarks, used to select icons, syntax
/// highlighting, and file extensions consistently across the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLanguage {
    Bash,
    Python,
}

impl ScriptLanguage {
    /// Detects the script language from a bookmark's name/filename and script body.
    /// Also tolerates common comma typos like `script,py`.
    #[must_use]
    pub fn detect(name_or_filename: &str, script: &str) -> Self {
        let lower_name = name_or_filename.trim().to_ascii_lowercase();
        let trimmed_script = script.trim_start();

        if lower_name.ends_with(".py")
            || lower_name.ends_with(",py")
            || trimmed_script.starts_with("#!/usr/bin/env python")
            || trimmed_script.starts_with("#!/usr/bin/python")
            || trimmed_script.starts_with("```python")
        {
            Self::Python
        } else {
            Self::Bash
        }
    }

    #[must_use]
    pub const fn icon_name(self) -> &'static str {
        match self {
            Self::Bash => "console",
            Self::Python => "python",
        }
    }

    #[must_use]
    pub const fn sourceview_lang_id(self) -> &'static str {
        match self {
            Self::Bash => "sh",
            Self::Python => "python",
        }
    }

    #[must_use]
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::Bash => "Bash (.sh)",
            Self::Python => "Python (.py)",
        }
    }

    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::Python => "Python",
        }
    }

    #[must_use]
    pub const fn default_extension(self) -> &'static str {
        match self {
            Self::Bash => ".sh",
            Self::Python => ".py",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub id: Uuid,
    pub name: String,
    pub filename: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script: String,
}

impl Bookmark {
    #[must_use]
    pub fn new(name: String, script: String, filename: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            filename,
            script,
        }
    }

    #[must_use]
    pub fn language(&self, script: &str) -> ScriptLanguage {
        if !self.filename.is_empty() {
            ScriptLanguage::detect(&self.filename, script)
        } else {
            ScriptLanguage::detect(&self.name, script)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_python_by_extension_or_shebang() {
        assert_eq!(
            ScriptLanguage::detect("deploy.py", "print('hi')"),
            ScriptLanguage::Python
        );
        assert_eq!(
            ScriptLanguage::detect("test,py", "print('hi')"),
            ScriptLanguage::Python
        );
        assert_eq!(
            ScriptLanguage::detect("runner", "#!/usr/bin/env python3\nimport sys"),
            ScriptLanguage::Python
        );
        assert_eq!(
            ScriptLanguage::detect("backup.sh", "#!/bin/bash\necho ok"),
            ScriptLanguage::Bash
        );
    }
}
