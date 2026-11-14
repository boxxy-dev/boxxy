use std::fmt::Write as _;

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
            || skill
                .frontmatter
                .triggers
                .iter()
                .any(|trigger| !trigger.is_empty() && query_lower.contains(&trigger.to_lowercase()));

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

    if !pane_id.is_empty() {
        let active_names: Vec<String> = active_skills
            .iter()
            .map(|s| s.frontmatter.name.clone())
            .collect();
        crate::registry::workspace::global_workspace()
            .await
            .update_pane_skills(pane_id.to_string(), active_names)
            .await;
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
