use boxxy_core_widgets::autocomplete::{CompletionItem, CompletionProvider};

pub struct AgentCompletionProvider;

impl CompletionProvider for AgentCompletionProvider {
    fn trigger(&self) -> String {
        "@".to_string()
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        let query_lower = query.to_lowercase();
        let mut items = Vec::new();

        let runtime = boxxy_ai_core::utils::runtime();
        let agents = runtime.block_on(async {
            boxxy_claw::registry::workspace::global_workspace()
                .await
                .get_all_agents()
                .await
        });

        for agent in agents {
            if agent.name.to_lowercase().contains(&query_lower) {
                items.push(CompletionItem {
                    display_name: agent.name.clone(),
                    replacement_text: format!("@{}", agent.name),
                    icon_name: Some("boxxyclaw".to_string()),
                    secondary_text: Some(agent.status),
                });
            }
        }

        items
    }
}

pub struct CommandCompletionProvider;

impl CompletionProvider for CommandCompletionProvider {
    fn trigger(&self) -> String {
        "/".to_string()
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        let query_lower = query.to_lowercase();

        if "resume".contains(&query_lower) {
            items.push(CompletionItem {
                display_name: "resume".to_string(),
                replacement_text: "/resume".to_string(),
                icon_name: Some("boxxy-chat-symbolic".to_string()),
                secondary_text: Some("Resume a past session".to_string()),
            });
        }

        items
    }
}

pub struct ResumeCompletionProvider;

impl CompletionProvider for ResumeCompletionProvider {
    fn trigger(&self) -> String {
        "/resume".to_string()
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        // Query starts after "/resume". If user types "/resume ", query is " ".
        // We trim it to handle both cases.
        let query_lower = query.trim().to_lowercase();
        let mut items = Vec::new();

        let runtime = boxxy_ai_core::utils::runtime();
        let sessions = runtime.block_on(async {
            if let Ok(db) = boxxy_db::Db::new().await {
                let store = boxxy_db::store::Store::new(db.pool());
                store
                    .get_recent_active_sessions(10)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        });

        for session in sessions {
            let title = session
                .title
                .unwrap_or_else(|| "Untitled Session".to_string());
            let agent_name = session.agent_name.unwrap_or_else(|| "Unknown".to_string());
            let cwd = session.last_cwd.unwrap_or_else(|| "/".to_string());
            let msg_count = session.message_count;

            // Format age (very basic implementation)
            let age = if let Some(updated_at) = session.updated_at {
                // Since SQLite returns a string for updated_at, and we didn't parse it to chrono yet
                // we'll just show the raw timestamp or a placeholder if it looks too complex to parse here
                // without adding more dependencies to this crate.
                // Let's just use the raw string for now or skip it if too long.
                updated_at.split(' ').next().unwrap_or("").to_string()
            } else {
                "unknown".to_string()
            };

            if query_lower.is_empty()
                || title.to_lowercase().contains(&query_lower)
                || agent_name.to_lowercase().contains(&query_lower)
            {
                items.push(CompletionItem {
                    display_name: format!("{title} [{msg_count} msgs]"),
                    replacement_text: format!("/resume {}", session.id),
                    icon_name: Some("boxxy-chat-symbolic".to_string()),
                    secondary_text: Some(format!("{agent_name} • {age} • {cwd}")),
                });
            }
        }

        items
    }
}

