use boxxy_db::Db;
use boxxy_db::store::Store;
use directories::ProjectDirs;
use log::debug;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn sync_memories_to_markdown(db: Arc<Mutex<Option<Db>>>) -> anyhow::Result<()> {
    let db_guard = db.lock().await;
    let Some(db) = db_guard.as_ref() else {
        return Ok(());
    };

    let store = Store::new(db.pool());
    let memories = store.get_all_memories().await?;

    if let Some(dirs) = ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        let config_dir = dirs.config_dir();
        let memory_md_path = config_dir.join("boxxyclaw").join("MEMORY.md");

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&memory_md_path)?;

        writeln!(file, "# 🧠 Boxxy-Claw Long-term Memories")?;
        writeln!(
            file,
            "This file is a mirror of your agent's brain. You can edit it manually."
        )?;
        writeln!(
            file,
            "Each bullet point MUST be in the format: `- [project_path] key: content`"
        )?;
        writeln!(file, "Use `[global]` if the memory applies everywhere.")?;
        writeln!(
            file,
            "Add the 📌 emoji anywhere in the line to permanently pin a memory to the context."
        )?;
        writeln!(file)?;

        let mut unverified = Vec::new();
        let mut verified = Vec::new();

        for mem in &memories {
            if mem.verified.unwrap_or(false) {
                verified.push(mem);
            } else {
                unverified.push(mem);
            }
        }

        if !unverified.is_empty() {
            writeln!(file, "## ⏳ Pending Verification")?;
            writeln!(
                file,
                "The agent implicitly extracted these facts. Move them to Active Memories below to verify, or delete them."
            )?;
            for mem in unverified {
                let pin = if mem.pinned.unwrap_or(false) {
                    " 📌"
                } else {
                    ""
                };
                let path = if mem.project_path.is_empty() {
                    "global"
                } else {
                    &mem.project_path
                };
                writeln!(file, "- [{}] {}:{}{}", path, mem.key, pin, mem.content)?;
            }
            writeln!(file)?;
        }

        writeln!(file, "## 🟢 Active Memories")?;
        for mem in verified {
            let pin = if mem.pinned.unwrap_or(false) {
                " 📌"
            } else {
                ""
            };
            let path = if mem.project_path.is_empty() {
                "global"
            } else {
                &mem.project_path
            };
            writeln!(file, "- [{}] {}:{} {}", path, mem.key, pin, mem.content)?;
        }

        debug!("Mirrored {} memories to MEMORY.md", memories.len());
    }

    Ok(())
}

/// Scans MEMORY.md and updates the database if keys are found.
/// This allows the user to 'inject' knowledge by just editing the file.
pub async fn sync_markdown_to_db(db: Arc<Mutex<Option<Db>>>) -> anyhow::Result<()> {
    if let Some(dirs) = ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        let config_dir = dirs.config_dir();
        let memory_md_path = config_dir.join("boxxyclaw").join("MEMORY.md");

        if !memory_md_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&memory_md_path)?;
        let db_guard = db.lock().await;
        let Some(db) = db_guard.as_ref() else {
            return Ok(());
        };
        let store = Store::new(db.pool());

        let mut is_pending_section = false;

        for line in content.lines() {
            let line = line.trim();
            if line == "## ⏳ Pending Verification" {
                is_pending_section = true;
                continue;
            } else if line == "## 🟢 Active Memories" {
                is_pending_section = false;
                continue;
            }

            if line.starts_with("- ") {
                let mut line_content = &line[2..];
                let mut project_path = "global".to_string();

                if line_content.starts_with('[')
                    && let Some(end_idx) = line_content.find(']') {
                        project_path = line_content[1..end_idx].trim().to_string();
                        line_content = line_content[end_idx + 1..].trim();
                    }

                let parts: Vec<&str> = line_content.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let key = parts[0].trim();
                    let mut val = parts[1].trim().to_string();

                    let mut pinned = false;
                    if val.contains("📌") {
                        pinned = true;
                        val = val.replace("📌", "").trim().to_string();
                    }

                    if !key.is_empty() && !val.is_empty() {
                        // Any memory manually placed/left in Active Memories is verified
                        let verified = !is_pending_section;
                        let path_opt = if project_path == "global" {
                            None
                        } else {
                            Some(project_path.as_str())
                        };
                        let _ = store
                            .add_memory(key, path_opt, &val, Some("manual_sync"), verified, pinned)
                            .await;
                    }
                }
            }
        }
    }
    Ok(())
}
