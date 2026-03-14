use zbus::interface;
use zbus::fdo;
use tokio::process::Command;
use std::process::Stdio;

#[derive(Default)]
pub struct AgentClaw;

#[interface(name = "play.mii.Boxxy.AgentClaw")]
impl AgentClaw {
    /// Execute a non-interactive shell command on the host.
    async fn exec_shell(&self, command: String) -> fdo::Result<(i32, String, String)> {
        let child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| fdo::Error::Failed(format!("Failed to spawn bash: {}", e)))?;
            
        let output = child.wait_with_output().await
            .map_err(|e| fdo::Error::Failed(format!("Failed to wait for output: {}", e)))?;
            
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        
        Ok((exit_code, stdout, stderr))
    }
    
    /// Read a file from the host system.
    async fn read_file(&self, path: String) -> fdo::Result<String> {
        let blacklisted = load_blacklist();
        
        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(fdo::Error::Failed("Path is blacklisted for security reasons".to_string()));
            }
        }
        
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(content),
            Err(e) => Err(fdo::Error::Failed(format!("Failed to read file: {}", e))),
        }
    }

    /// Write a file to the host system.
    async fn write_file(&self, path: String, content: String) -> fdo::Result<()> {
        let blacklisted = load_blacklist();
        
        for black_path in blacklisted {
            if path.contains(&black_path) {
                return Err(fdo::Error::Failed("Path is blacklisted for security reasons".to_string()));
            }
        }
        
        match tokio::fs::write(&path, content).await {
            Ok(_) => Ok(()),
            Err(e) => Err(fdo::Error::Failed(format!("Failed to write file: {}", e))),
        }
    }
}

/// Load blacklist from config file or return defaults.
fn load_blacklist() -> Vec<String> {
    let mut blacklisted = vec![
        "/etc/shadow".to_string(),
        "/etc/gshadow".to_string(),
        "/root/.ssh".to_string(),
        ".ssh/id_rsa".to_string(),
        ".ssh/id_ed25519".to_string(),
    ];

    if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        let blacklist_path = dirs.config_dir().join("boxxyclaw").join("BLACKLIST.md");
        if let Ok(content) = std::fs::read_to_string(&blacklist_path) {
            // Read from file, replacing defaults
            let mut new_blacklist = Vec::new();
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    new_blacklist.push(trimmed.to_string());
                }
            }
            if !new_blacklist.is_empty() {
                blacklisted = new_blacklist;
            }
        }
    }
    blacklisted
}
