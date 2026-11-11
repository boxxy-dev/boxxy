use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// Maximum bytes read when scanning a skill's YAML frontmatter header for UI display.
const MAX_FRONTMATTER_SCAN_BYTES: u64 = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

/// Lightweight metadata summary for a skill on disk, read without loading the markdown body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSummary {
    /// Directory name (for `<slug>/SKILL.md`) or file stem (for `<slug>.md`).
    /// Used as the primary identifier in `Settings.disabled_skills` so disabled skills
    /// can be skipped before opening or reading the markdown file.
    pub slug: String,
    /// Display name from frontmatter (falls back to `slug`).
    pub name: String,
    /// Short description from frontmatter (falls back to `"Custom skill"`).
    pub description: String,
}

/// Returns the base directory for skills: `~/.config/boxxy-terminal/boxxyclaw/skills/`
#[must_use]
pub fn get_skills_dir() -> PathBuf {
    boxxy_sys_utils::get_config_dir()
        .join("boxxyclaw")
        .join("skills")
}

/// Returns `true` if a skill is marked disabled in `disabled_skills` (matching either its
/// on-disk `slug` or its frontmatter `name`).
#[must_use]
pub fn is_skill_disabled(disabled_skills: &[String], slug: &str, name: &str) -> bool {
    disabled_skills
        .iter()
        .any(|d| d == slug || (!name.is_empty() && d == name))
}

/// Resolves a directory entry inside `skills_dir` to `(slug, markdown_file_path)`
/// without reading the file contents.
#[must_use]
pub fn resolve_skill_entry(path: &Path) -> Option<(String, PathBuf)> {
    if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
        let slug = path.file_stem()?.to_str()?.trim().to_string();
        if slug.is_empty() {
            return None;
        }
        Some((slug, path.to_path_buf()))
    } else if path.is_dir() {
        let slug = path.file_name()?.to_str()?.trim().to_string();
        if slug.is_empty() {
            return None;
        }
        let skill_md = path.join("SKILL.md");
        if skill_md.is_file() {
            Some((slug, skill_md))
        } else {
            None
        }
    } else {
        None
    }
}

/// Parses frontmatter from the start of a markdown document, returning `(frontmatter, body)`
/// if a valid YAML frontmatter block is present.
#[must_use]
pub fn parse_frontmatter_block(content: &str) -> Option<(SkillFrontmatter, &str)> {
    if !(content.starts_with("---\n") || content.starts_with("---\r\n")) {
        return None;
    }
    let after_open = if content.starts_with("---\r\n") { 5 } else { 4 };
    let rest = &content[after_open..];
    let end_idx = rest.find("\n---")?;
    let frontmatter_str = &rest[..end_idx];
    let after_close = &rest[end_idx + 4..];
    let body = after_close
        .strip_prefix("\r\n")
        .or_else(|| after_close.strip_prefix('\n'))
        .unwrap_or(after_close);

    #[allow(deprecated)]
    let parsed = serde_yml::from_str::<SkillFrontmatter>(frontmatter_str).ok()?;
    Some((parsed, body))
}

/// Parses a full skill document into `(SkillFrontmatter, body_markdown)`, falling back
/// to `slug` and `"Custom skill"` when frontmatter is absent.
#[must_use]
pub fn parse_skill_document(slug: &str, content: &str) -> (SkillFrontmatter, String) {
    if let Some((mut fm, body)) = parse_frontmatter_block(content) {
        if fm.name.trim().is_empty() {
            fm.name = slug.to_string();
        }
        if fm.description.trim().is_empty() {
            fm.description = "Custom skill".to_string();
        }
        (fm, body.trim_start().to_string())
    } else {
        let fallback_name = if slug.trim().is_empty() {
            "legacy-skill".to_string()
        } else {
            slug.to_string()
        };
        (
            SkillFrontmatter {
                name: fallback_name,
                description: "Custom skill".to_string(),
                triggers: Vec::new(),
                pinned: false,
            },
            content.to_string(),
        )
    }
}

/// Reads only the frontmatter header (capped at 4 KB, stopping at the closing `---`)
/// so listing skills in Preferences never loads full markdown bodies into memory.
pub fn read_skill_summary(slug: &str, md_path: &Path) -> Option<SkillSummary> {
    let file = File::open(md_path).ok()?;
    let mut reader = BufReader::new(file.take(MAX_FRONTMATTER_SCAN_BYTES));

    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;

    if first_line.trim_end_matches(['\r', '\n']) == "---" {
        let mut yaml_buf = String::new();
        let mut found_close = false;
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line).ok()?;
            if bytes == 0 {
                break;
            }
            if line.trim_end_matches(['\r', '\n']) == "---" {
                found_close = true;
                break;
            }
            yaml_buf.push_str(&line);
        }

        if found_close {
            #[allow(deprecated)]
            if let Ok(fm) = serde_yml::from_str::<SkillFrontmatter>(&yaml_buf) {
                let name = if fm.name.trim().is_empty() {
                    slug.to_string()
                } else {
                    fm.name.trim().to_string()
                };
                let description = if fm.description.trim().is_empty() {
                    "Custom skill".to_string()
                } else {
                    fm.description.trim().to_string()
                };
                return Some(SkillSummary {
                    slug: slug.to_string(),
                    name,
                    description,
                });
            }
        }
    }

    Some(SkillSummary {
        slug: slug.to_string(),
        name: slug.to_string(),
        description: "Custom skill".to_string(),
    })
}

/// Discovers all skills in the active config's `skills` directory using lightweight header reads.
#[must_use]
pub fn discover_skill_summaries() -> Vec<SkillSummary> {
    discover_skill_summaries_in(&get_skills_dir())
}

/// Discovers all skills in `skills_dir` using lightweight header reads, sorted by name.
#[must_use]
pub fn discover_skill_summaries_in(skills_dir: &Path) -> Vec<SkillSummary> {
    let mut summaries = Vec::new();
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return summaries;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some((slug, md_path)) = resolve_skill_entry(&path)
            && let Some(summary) = read_skill_summary(&slug, &md_path)
        {
            summaries.push(summary);
        }
    }

    summaries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    summaries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_summaries_reads_folder_and_file_skills_without_loading_body() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path();

        let rust_dir = skills_dir.join("rust-helper");
        std::fs::create_dir_all(&rust_dir).unwrap();
        std::fs::write(
            rust_dir.join("SKILL.md"),
            "---\nname: Rust Helper\ndescription: Ownership and async tips\ntriggers:\n  - cargo\n---\n# Huge Body\nLots of instructions...",
        )
        .unwrap();

        std::fs::write(
            skills_dir.join("alpha-note.md"),
            "# Plain Markdown Skill\nNo YAML frontmatter here.",
        )
        .unwrap();

        let summaries = discover_skill_summaries_in(skills_dir);
        assert_eq!(summaries.len(), 2);

        assert_eq!(summaries[0].slug, "alpha-note");
        assert_eq!(summaries[0].name, "alpha-note");
        assert_eq!(summaries[0].description, "Custom skill");

        assert_eq!(summaries[1].slug, "rust-helper");
        assert_eq!(summaries[1].name, "Rust Helper");
        assert_eq!(summaries[1].description, "Ownership and async tips");
    }

    #[test]
    fn is_skill_disabled_matches_slug_or_name() {
        let disabled = vec!["rust-helper".to_string(), "Docker Admin".to_string()];
        assert!(is_skill_disabled(&disabled, "rust-helper", "Rust Helper"));
        assert!(is_skill_disabled(
            &disabled,
            "docker-folder",
            "Docker Admin"
        ));
        assert!(!is_skill_disabled(&disabled, "git-flow", "Git Flow"));
    }
}
