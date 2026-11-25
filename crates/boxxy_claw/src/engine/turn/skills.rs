use std::fmt::Write as _;

/// Checks whether `haystack_lower` contains `trigger_lower` on clean word boundaries
/// (allowing simple trailing `-s` / `-es` plurals on `>= 3`-char triggers while preventing
/// substring false positives such as `"oci"` matching `"social"` or `"tar"` matching `"start"`).
#[must_use]
fn contains_trigger_phrase(haystack_lower: &str, trigger_lower: &str) -> bool {
    let needle = trigger_lower.trim();
    if needle.is_empty() {
        return false;
    }

    for (start, _) in haystack_lower.match_indices(needle) {
        let before_ok = start == 0
            || haystack_lower[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
        if !before_ok {
            continue;
        }

        let end = start + needle.len();
        if end >= haystack_lower.len() {
            return true;
        }

        let rest = &haystack_lower[end..];
        let Some(next_ch) = rest.chars().next() else {
            return true;
        };
        if !next_ch.is_alphanumeric() {
            return true;
        }

        if needle.len() >= 3 {
            if next_ch == 's'
                && rest['s'.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                return true;
            }
            if rest.starts_with("es")
                && rest["es".len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric())
            {
                return true;
            }
        }
    }

    false
}

pub async fn build_skills_context(prompt: &str, pane_id: &str) -> (String, String, Vec<String>) {
    let mut active_skills_text = String::new();
    let mut available_skills_text = String::new();
    let mut explicitly_triggered = Vec::new();
    let query_lower = prompt.to_lowercase();

    let registry = crate::registry::skills::global_registry().await;

    // 1. Semantic Search: Get only the TOP 1 most relevant skill to avoid context bloat
    let mut active_skills = registry.search_relevant_skills(prompt, 1).await;

    // 2. Pinned & Keyword Fallback: Include pinned skills or skills explicitly triggered by keywords
    let all_skills = registry.get_skills().await;
    for skill in &all_skills {
        let triggered = skill.frontmatter.pinned
            || skill.frontmatter.triggers.iter().any(|trigger| {
                !trigger.trim().is_empty()
                    && contains_trigger_phrase(&query_lower, &trigger.to_lowercase())
            });

        if triggered {
            explicitly_triggered.push(skill.frontmatter.name.clone());
        }

        if active_skills
            .iter()
            .any(|s| s.frontmatter.name == skill.frontmatter.name)
        {
            continue;
        }

        if triggered {
            active_skills.push(skill.clone());
        }
    }

    if !pane_id.is_empty() && !active_skills.is_empty() {
        let ws = crate::registry::workspace::global_workspace().await;
        for skill in &active_skills {
            ws.add_pane_skill(pane_id, skill.frontmatter.name.clone())
                .await;
        }
    }

    if !active_skills.is_empty() {
        active_skills_text.push_str("\n--- ACTIVE SKILLS ---\n");
        active_skills_text.push_str("These skills are currently loaded and ready to use:\n");
        for skill in &active_skills {
            let _ = writeln!(
                active_skills_text,
                "- {}: {}",
                skill.frontmatter.name, skill.frontmatter.description
            );
            active_skills_text.push_str(&skill.content);
            active_skills_text.push('\n');
        }
    }

    // List ALL other available skills so the agent knows what it can activate
    let available_skills: Vec<_> = all_skills
        .into_iter()
        .filter(|s| {
            !active_skills
                .iter()
                .any(|active| active.frontmatter.name == s.frontmatter.name)
        })
        .collect();

    if !available_skills.is_empty() {
        available_skills_text.push_str("\n--- AVAILABLE SKILLS (TOOLBOX - Compact) ---\n");
        available_skills_text.push_str(
            "Use `activate_skill(name)` if you need the full instructions for any of these:\n",
        );
        for skill in available_skills {
            let _ = writeln!(
                available_skills_text,
                "- {}: {}",
                skill.frontmatter.name, skill.frontmatter.description
            );
        }
    }

    (
        active_skills_text,
        available_skills_text,
        explicitly_triggered,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_phrase_respects_word_boundaries_and_plurals() {
        assert!(contains_trigger_phrase("inspect oci-manifest layers", "oci"));
        assert!(contains_trigger_phrase("pack reproducible tarballs", "tarball"));
        assert!(contains_trigger_phrase("fix css patches now", "patch"));
        assert!(contains_trigger_phrase("please report bugs upstream", "report bug"));

        // Substring collisions inside unrelated words must never trigger skills
        assert!(!contains_trigger_phrase("check social velocity metrics", "oci"));
        assert!(!contains_trigger_phrase("start the target server", "tar"));
        assert!(!contains_trigger_phrase("verify digital signature", "git"));
    }
}
