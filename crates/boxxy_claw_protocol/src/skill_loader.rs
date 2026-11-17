//! Lightweight YAML frontmatter scanner and skill discovery shared between `boxxy-preferences` and `boxxy-claw`.

use crate::str_utils::cmp_case_insensitive;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{DirEntry, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

/// Maximum bytes read when scanning a skill's YAML frontmatter header for UI display.
const MAX_FRONTMATTER_SCAN_BYTES: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    #[serde(default)]
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

/// Returns the base directory for skills: `~/.config/boxxy-terminal/boxxyclaw/skills/`.
#[must_use]
pub fn get_skills_dir() -> PathBuf {
    boxxy_sys_utils::get_config_dir()
        .join("boxxyclaw")
        .join("skills")
}

/// Returns `true` if a skill is marked disabled in `disabled_skills` (matching either its
/// on-disk `slug` or its frontmatter `name`, ignoring surrounding whitespace).
#[must_use]
pub fn is_skill_disabled(disabled_skills: &[String], slug: &str, name: &str) -> bool {
    let trimmed_slug = slug.trim();
    let trimmed_name = name.trim();
    disabled_skills.iter().any(|d| {
        let dt = d.trim();
        dt == trimmed_slug || (!trimmed_name.is_empty() && dt == trimmed_name)
    })
}

fn resolve_skill_with_kind(
    path: &Path,
    is_file: bool,
    is_dir: bool,
) -> Option<(String, PathBuf, bool)> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.starts_with('.') {
        return None;
    }

    if is_file
        && path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        let slug = path.file_stem()?.to_str()?.trim().to_string();
        if slug.is_empty() {
            return None;
        }
        Some((slug, path.to_path_buf(), false))
    } else if is_dir {
        let slug = file_name.trim().to_string();
        if slug.is_empty() {
            return None;
        }
        let skill_md = path.join("SKILL.md");
        skill_md.is_file().then_some((slug, skill_md, true))
    } else {
        None
    }
}

/// Resolves a `DirEntry` using `d_type` (`entry.file_type()`) on Linux filesystems so
/// regular files and directories require zero extra `stat()` syscalls.
fn resolve_skill_dir_entry(entry: &DirEntry) -> Option<(String, PathBuf, bool)> {
    let path = entry.path();
    let (is_file, is_dir) = match entry.file_type() {
        Ok(ft) if ft.is_symlink() => (path.is_file(), path.is_dir()),
        Ok(ft) => (ft.is_file(), ft.is_dir()),
        Err(_) => (path.is_file(), path.is_dir()),
    };
    resolve_skill_with_kind(&path, is_file, is_dir)
}

/// Resolves a directory entry inside `skills_dir` to `(slug, markdown_file_path)`
/// without reading the file contents. Hidden entries starting with `.` are ignored.
#[must_use]
pub fn resolve_skill_entry(path: &Path) -> Option<(String, PathBuf)> {
    let meta = std::fs::metadata(path).ok()?;
    resolve_skill_with_kind(path, meta.is_file(), meta.is_dir()).map(|(slug, md, _)| (slug, md))
}

/// Splits a markdown document into `(yaml_frontmatter_slice, remaining_body_slice)` when a
/// valid opening `---` line and matching closing `---` line exist.
fn split_frontmatter(raw_content: &str) -> Option<(&str, &str)> {
    let content = raw_content.strip_prefix('\u{feff}').unwrap_or(raw_content);
    let first_nl = content.find('\n')?;
    let first_line = content[..first_nl].trim_end_matches(['\r', ' ', '\t']);
    if first_line != "---" {
        return None;
    }

    let fm_start = first_nl + 1;
    let mut line_start = fm_start;
    while line_start <= content.len() {
        let (line_end, next_start) = content[line_start..]
            .find('\n')
            .map_or((content.len(), content.len() + 1), |rel| {
                (line_start + rel, line_start + rel + 1)
            });

        let line = content[line_start..line_end].trim_end_matches(['\r', ' ', '\t']);
        if line == "---" {
            let yaml_str = &content[fm_start..line_start];
            let body = if line_end < content.len() {
                &content[line_end + 1..]
            } else {
                ""
            };
            return Some((yaml_str, body));
        }

        line_start = next_start;
    }

    None
}

/// Parses frontmatter from the start of a markdown document, returning `(frontmatter, body)`
/// if a valid YAML frontmatter block is present.
#[must_use]
pub fn parse_frontmatter_block(content: &str) -> Option<(SkillFrontmatter, &str)> {
    let (frontmatter_str, body) = split_frontmatter(content)?;
    if frontmatter_str.trim().is_empty() {
        return Some((SkillFrontmatter::default(), body));
    }

    let parsed = serde_yml::from_str::<SkillFrontmatter>(frontmatter_str).ok()?;
    Some((parsed, body))
}

fn normalize_name_and_description(slug: &str, name: &str, description: &str) -> (String, String) {
    let fallback_slug = if slug.trim().is_empty() {
        "legacy-skill"
    } else {
        slug.trim()
    };
    let resolved_name = {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            fallback_slug.to_string()
        } else {
            trimmed.to_string()
        }
    };
    let resolved_desc = {
        let trimmed = description.trim();
        if trimmed.is_empty() {
            "Custom skill".to_string()
        } else {
            trimmed.to_string()
        }
    };
    (resolved_name, resolved_desc)
}

/// Parses a full skill document into `(SkillFrontmatter, body_markdown)`, falling back
/// to `slug` and `"Custom skill"` when frontmatter is absent or fields are blank.
///
/// If a `--- ... ---` block is present with invalid YAML syntax, logs a warning and still
/// strips the broken frontmatter header so malformed metadata is not injected into prompts.
#[must_use]
pub fn parse_skill_document(slug: &str, content: &str) -> (SkillFrontmatter, String) {
    if let Some((frontmatter_str, body)) = split_frontmatter(content) {
        let fm_opt = if frontmatter_str.trim().is_empty() {
            Some(SkillFrontmatter::default())
        } else {
            match serde_yml::from_str::<SkillFrontmatter>(frontmatter_str) {
                Ok(parsed) => Some(parsed),
                Err(err) => {
                    log::warn!("Failed to parse YAML frontmatter for skill '{slug}': {err}");
                    None
                }
            }
        };

        let mut fm = fm_opt.unwrap_or_default();
        let (name, description) = normalize_name_and_description(slug, &fm.name, &fm.description);
        fm.name = name;
        fm.description = description;
        return (fm, body.trim_start().to_string());
    }

    let (name, description) = normalize_name_and_description(slug, "", "");
    let stripped = content.strip_prefix('\u{feff}').unwrap_or(content);
    (
        SkillFrontmatter {
            name,
            description,
            triggers: Vec::new(),
            pinned: false,
        },
        stripped.to_string(),
    )
}

/// Reads at most `buf.len()` bytes from `file`, retrying on `ErrorKind::Interrupted`.
fn read_bounded_prefix(mut file: File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Reads only the frontmatter header (capped at 4 KiB on the stack) so listing skills
/// in Preferences never loads full markdown bodies into heap memory.
#[must_use]
pub fn read_skill_summary(slug: &str, md_path: &Path) -> Option<SkillSummary> {
    let file = File::open(md_path).ok()?;
    let mut buf = [0u8; MAX_FRONTMATTER_SCAN_BYTES];
    let n = read_bounded_prefix(file, &mut buf).ok()?;
    let slice = &buf[..n];

    // If the 4 KiB boundary cut through the middle of a multi-byte UTF-8 codepoint in the
    // markdown body, slice cleanly up to `valid_up_to()` instead of dropping the skill.
    let head_str = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(err) => std::str::from_utf8(&slice[..err.valid_up_to()]).ok()?,
    };

    if let Some((fm, _)) = parse_frontmatter_block(head_str) {
        let (name, description) = normalize_name_and_description(slug, &fm.name, &fm.description);
        return Some(SkillSummary {
            slug: slug.to_string(),
            name,
            description,
        });
    }

    let (name, description) = normalize_name_and_description(slug, "", "");
    Some(SkillSummary {
        slug: slug.to_string(),
        name,
        description,
    })
}

/// Discovers all skills in the active config's `skills` directory using lightweight header reads.
#[must_use]
pub fn discover_skill_summaries() -> Vec<SkillSummary> {
    discover_skill_summaries_in(&get_skills_dir())
}

/// Discovers all skills in `skills_dir` using lightweight header reads.
///
/// Deduplicates colliding slugs (preferring `<slug>/SKILL.md` over legacy `<slug>.md`)
/// and sorts case-insensitively by display name without per-comparison allocations.
#[must_use]
pub fn discover_skill_summaries_in(skills_dir: &Path) -> Vec<SkillSummary> {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return Vec::new();
    };

    // Track `(is_dir_layout, summary)` by slug so `<slug>/SKILL.md` wins deterministically
    // if both `<slug>/SKILL.md` and `<slug>.md` exist on disk.
    let mut by_slug: HashMap<String, (bool, SkillSummary)> = HashMap::new();

    for entry in entries.flatten() {
        if let Some((slug, md_path, is_dir)) = resolve_skill_dir_entry(&entry) {
            if let Some((existing_is_dir, _)) = by_slug.get(&slug)
                && (*existing_is_dir || !is_dir)
            {
                continue;
            }
            if let Some(summary) = read_skill_summary(&slug, &md_path) {
                by_slug.insert(slug, (is_dir, summary));
            }
        }
    }

    let mut summaries: Vec<SkillSummary> = by_slug.into_values().map(|(_, s)| s).collect();
    summaries
        .sort_by(|a, b| cmp_case_insensitive(&a.name, &b.name).then_with(|| a.slug.cmp(&b.slug)));
    summaries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_summaries_reads_folder_and_file_skills_and_deduplicates_slugs() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path();

        let rust_dir = skills_dir.join("rust-helper");
        std::fs::create_dir_all(&rust_dir).unwrap();
        std::fs::write(
            rust_dir.join("SKILL.md"),
            "---\nname: Rust Helper\ndescription: Ownership and async tips\ntriggers:\n  - cargo\n---\n# Huge Body\nLots of instructions...",
        )
        .unwrap();

        // Colliding legacy flat file `<slug>.md` loses deterministically to `<slug>/SKILL.md`
        std::fs::write(
            skills_dir.join("rust-helper.md"),
            "---\nname: Legacy Flat Rust\ndescription: Old\n---\n",
        )
        .unwrap();

        std::fs::write(
            skills_dir.join("alpha-note.MD"),
            "# Plain Markdown Skill\nNo YAML frontmatter here.",
        )
        .unwrap();

        // Hidden files/folders must be ignored
        std::fs::write(
            skills_dir.join(".hidden-swap.md"),
            "---\nname: Hidden\n---\n",
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
    fn frontmatter_parser_handles_bom_empty_malformed_and_embedded_triple_dashes() {
        // 1. UTF-8 BOM + embedded `---flag` line inside YAML description must NOT terminate early
        let doc = "\u{feff}---\nname: \"  CLI Flag Guide  \"\ndescription: |\n  Explains flags like:\n  ---verbose\n---\n# Actual Body\n";
        let (fm, body) = parse_skill_document("cli-guide", doc);
        assert_eq!(fm.name, "CLI Flag Guide");
        assert!(fm.description.contains("---verbose"));
        assert_eq!(body, "# Actual Body\n");

        // 2. Empty frontmatter `---\n---\n` strips delimiters cleanly and falls back to slug
        let (empty_fm, empty_body) = parse_skill_document("my-slug", "---\n---\n# Heading");
        assert_eq!(empty_fm.name, "my-slug");
        assert_eq!(empty_fm.description, "Custom skill");
        assert_eq!(empty_body, "# Heading");

        // 3. Malformed YAML inside `--- ... ---` strips the broken header instead of leaking YAML into body
        let malformed = "---\nname: Broken\ndescription: unquoted: colon: error\n---\n# Clean Body";
        let (bad_fm, bad_body) = parse_skill_document("broken-slug", malformed);
        assert_eq!(bad_fm.name, "broken-slug");
        assert_eq!(bad_fm.description, "Custom skill");
        assert_eq!(bad_body, "# Clean Body");

        // 4. Truncated mid-UTF8 codepoint at byte 4096 in `read_skill_summary` still succeeds
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unicode-boundary.md");
        let mut payload = String::from("---\nunclosed_frontmatter: |\n  ");
        while payload.len() < MAX_FRONTMATTER_SCAN_BYTES - 1 {
            payload.push('a');
        }
        payload.push('•');
        std::fs::write(&path, payload).unwrap();

        let summary = read_skill_summary("unicode-boundary", &path).expect("must not drop skill");
        assert_eq!(summary.slug, "unicode-boundary");
        assert_eq!(summary.name, "unicode-boundary");
    }

    #[test]
    fn is_skill_disabled_matches_slug_or_trimmed_name() {
        let disabled = vec!["  rust-helper  ".to_string(), "Docker Admin".to_string()];
        assert!(is_skill_disabled(&disabled, "rust-helper", "Rust Helper"));
        assert!(is_skill_disabled(
            &disabled,
            "docker-folder",
            "  Docker Admin  "
        ));
        assert!(!is_skill_disabled(&disabled, "git-flow", "Git Flow"));
    }
}
