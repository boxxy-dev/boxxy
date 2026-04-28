pub async fn build_skills_context(prompt: &str, _pane_id: &str) -> (String, String) {
    let mut active_skills_text = String::new();
    let mut available_skills_text = String::new();
    let query_lower = prompt.to_lowercase();

    let registry = crate::registry::skills::global_registry().await;

    // 1. Semantic Search: Get only the TOP 1 most relevant skill to avoid context bloat
    let mut active_skills = registry.search_relevant_skills(prompt, 1).await;

    // 2. Keyword Fallback: Only add skills that were EXPLICITLY triggered by keywords in the query
    let all_skills = registry.get_skills().await;
    for skill in &all_skills {
        // Avoid duplicates
        if active_skills
            .iter()
            .any(|s| s.frontmatter.name == skill.frontmatter.name)
        {
            continue;
        }

        let mut should_load = false;
        // We NO LONGER load trigger-less skills by default.
        // They must be triggered or activated via tool.
        for trigger in &skill.frontmatter.triggers {
            if !trigger.is_empty() && query_lower.contains(&trigger.to_lowercase()) {
                should_load = true;
                break;
            }
        }

        if should_load {
            active_skills.push(skill.clone());
        }
    }

    if !active_skills.is_empty() {
        active_skills_text.push_str("\n--- ACTIVE SKILLS ---\n");
        active_skills_text.push_str("These skills are currently loaded and ready to use:\n");
        for skill in &active_skills {
            active_skills_text.push_str(&format!(
                "- {}: {}\n",
                skill.frontmatter.name, skill.frontmatter.description
            ));
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
            available_skills_text.push_str(&format!(
                "- {}: {}\n",
                skill.frontmatter.name, skill.frontmatter.description
            ));
        }
    }

    (active_skills_text, available_skills_text)
}
