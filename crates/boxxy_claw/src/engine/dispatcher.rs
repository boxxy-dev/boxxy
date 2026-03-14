pub fn extract_command_and_clean(text: &str) -> (Option<String>, String) {
    let mut blocks = Vec::new();
    let mut current_block = String::new();
    let mut in_block = false;
    let mut lang = String::new();
    
    let mut clean_text = String::new();

    for line in text.lines() {
        if let Some(stripped) = line.strip_prefix("```") {
            if in_block {
                blocks.push((lang.clone(), current_block.trim().to_string()));
                in_block = false;
                current_block.clear();
                lang.clear();
            } else {
                in_block = true;
                lang = stripped.trim().to_string();
            }
        } else if in_block {
            current_block.push_str(line);
            current_block.push('\n');
        } else {
            clean_text.push_str(line);
            clean_text.push('\n');
        }
    }

    // Prefer bash/sh blocks. Do not extract other languages like python or rust.
    let target = blocks.iter()
        .find(|(l, _)| {
            let lang = l.to_lowercase();
            lang == "bash" || lang == "sh" || lang == "zsh" || lang == "fish" || lang.is_empty()
        });

    let final_cmd = if let Some((lang, cmd)) = target {
        // If it's an empty language block, but it looks like a script (has import, fn, etc), don't treat as bash
        if lang.is_empty() && (cmd.contains("import ") || cmd.contains("def ") || cmd.contains("fn ") || cmd.contains("use ")) {
            None
        } else {
            let sanitized: Vec<&str> = cmd.lines()
                .filter(|l| !l.trim().is_empty())
                .filter(|l| !l.trim().starts_with('#'))
                .collect();
                
            let mut final_cmd = String::new();
            for (i, line) in sanitized.iter().enumerate() {
                let trimmed = line.trim();
                if let Some(stripped) = trimmed.strip_suffix('\\') {
                    // Remove trailing backslash for line continuation
                    final_cmd.push_str(stripped.trim_end());
                    final_cmd.push(' ');
                } else {
                    final_cmd.push_str(trimmed);
                    if i < sanitized.len() - 1 {
                        if trimmed.ends_with("&&") || trimmed.ends_with("||") || trimmed.ends_with(';') {
                            final_cmd.push(' ');
                        } else {
                            final_cmd.push_str(" && ");
                        }
                    }
                }
            }
            
            if final_cmd.is_empty() {
                None
            } else {
                Some(final_cmd)
            }
        }
    } else {
        None
    };

    (final_cmd, clean_text.trim().to_string())
}
