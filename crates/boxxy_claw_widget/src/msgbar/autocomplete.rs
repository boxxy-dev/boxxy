use boxxy_core_widgets::autocomplete::{CompletionItem, CompletionProvider};

pub struct AgentCompletionProvider;

impl CompletionProvider for AgentCompletionProvider {
    fn trigger(&self) -> String {
        "@".to_string()
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        let query_lower = query.to_lowercase();
        let mut items = Vec::new();

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();

        for info in registry.iter() {
            if info.config.name.to_lowercase().contains(&query_lower)
                || info
                    .config
                    .display_name
                    .to_lowercase()
                    .contains(&query_lower)
            {
                let badge_color = Some(info.config.color.clone());

                let is_taken = claims
                    .iter()
                    .any(|claim| claim.character_id == info.config.id);

                let (secondary_text, icon_name) = if is_taken {
                    ("In Use".to_string(), "boxxy-chat-symbolic".to_string())
                } else {
                    ("Available".to_string(), "boxxy-chat-symbolic".to_string())
                };

                let avatar_path = boxxy_claw_protocol::character_loader::get_characters_dir()
                    .ok()
                    .map(|d| d.join(&info.config.name).join("AVATAR.png"))
                    .filter(|p| p.exists())
                    .map(|p| p.to_string_lossy().into_owned());

                items.push(CompletionItem {
                    display_name: info.config.display_name.clone(),
                    replacement_text: format!("@{} ", info.config.name),
                    icon_name: Some(icon_name),
                    icon_path: avatar_path,
                    secondary_text: Some(secondary_text),
                    badge_text: None,
                    badge_color,
                    disabled: false,
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
                display_name: "Resume past session".to_string(),
                replacement_text: "/resume".to_string(),
                icon_name: None,
                icon_path: None,
                secondary_text: None,
                badge_text: None,
                badge_color: None,
                disabled: false,
            });
        }

        items
    }
}

fn generate_color(name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();

    let r = (hash & 0xFF) as u8 % 150 + 50;
    let g = ((hash >> 8) & 0xFF) as u8 % 150 + 50;
    let b = ((hash >> 16) & 0xFF) as u8 % 150 + 50;

    format!("rgb({}, {}, {})", r, g, b)
}

fn get_relative_age(updated_at: &str) -> String {
    use chrono::{NaiveDateTime, Utc};
    if let Ok(ndt) = NaiveDateTime::parse_from_str(updated_at, "%Y-%m-%d %H:%M:%S") {
        let now = Utc::now().naive_utc();
        let duration = now.signed_duration_since(ndt);

        if duration.num_days() > 0 {
            format!("{}d", duration.num_days())
        } else if duration.num_hours() > 0 {
            format!("{}h", duration.num_hours())
        } else if duration.num_minutes() > 0 {
            format!("{}m", duration.num_minutes())
        } else {
            "now".to_string()
        }
    } else {
        "unknown".to_string()
    }
}

pub struct ResumeCompletionProvider;

impl CompletionProvider for ResumeCompletionProvider {
    fn trigger(&self) -> String {
        "/resume ".to_string()
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        // Query starts after "/resume". If user types "/resume ", query is " ".
        // We trim it to handle both cases.
        let query_lower = query.trim().to_lowercase();
        let mut items = Vec::new();

        let runtime = boxxy_sys_utils::runtime();
        let sessions = runtime.block_on(async {
            if let Ok(db) = boxxy_db::Db::new().await {
                let store = boxxy_db::store::Store::new(db.pool());
                let res = store
                    .get_recent_active_sessions(10)
                    .await
                    .unwrap_or_default();
                log::debug!(
                    "ResumeCompletionProvider fetched {} sessions from DB.",
                    res.len()
                );
                for s in &res {
                    log::debug!("  - Session: {} (Title: {:?})", s.id, s.title);
                }
                res
            } else {
                Vec::new()
            }
        });

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

        for session in sessions {
            let mut title = session
                .title
                .unwrap_or_else(|| "Untitled Session".to_string());
            if title.trim().is_empty() {
                title = "Untitled Session".to_string();
            }
            // character_name is the registry key; agent_name is the display name
            let char_name = session.agent_name.clone().unwrap_or_default();
            let msg_count = session.message_count;

            let age = if let Some(updated_at) = session.updated_at {
                get_relative_age(&updated_at)
            } else {
                "unknown".to_string()
            };

            if query_lower.is_empty()
                || title.to_lowercase().contains(&query_lower)
                || char_name.to_lowercase().contains(&query_lower)
            {
                // Look up the character in the registry for official color + display name.
                // If not found, fallback to the first character in the registry as requested.
                let char_info = registry
                    .iter()
                    .find(|c| c.config.name == char_name || c.config.display_name == char_name)
                    .or_else(|| registry.first());

                let badge_color = char_info
                    .map(|c| c.config.color.clone())
                    .unwrap_or_else(|| generate_color(&char_name));

                let badge_text = char_info
                    .map(|c| c.config.display_name.clone())
                    .unwrap_or_else(|| char_name.clone());

                let mut is_disabled = false;
                let mut display_age = age;

                if let Some(info) = char_info {
                    let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
                    let is_taken = claims
                        .iter()
                        .any(|claim| claim.character_id == info.config.id);
                    if is_taken {
                        is_disabled = true;
                        display_age = "In Use".to_string();
                    }
                }

                let icon_name = if session.pinned {
                    "boxxy-view-pin-symbolic".to_string()
                } else {
                    "boxxy-boxxyclaw-symbolic".to_string()
                };

                // Prefer the character's avatar PNG over the generic themed icon.
                let avatar_path = char_info
                    .and_then(|info| {
                        boxxy_claw_protocol::character_loader::get_characters_dir()
                            .ok()
                            .map(|d| d.join(&info.config.name).join("AVATAR.png"))
                    })
                    .filter(|p| p.exists())
                    .map(|p| p.to_string_lossy().into_owned());

                items.push(CompletionItem {
                    display_name: format!("{title} [{msg_count} msgs]"),
                    replacement_text: format!("/resume {}", session.id),
                    icon_name: Some(icon_name),
                    icon_path: avatar_path,
                    secondary_text: Some(display_age),
                    badge_text: Some(badge_text),
                    badge_color: Some(badge_color),
                    disabled: is_disabled,
                });
            }
        }

        items
    }
}
