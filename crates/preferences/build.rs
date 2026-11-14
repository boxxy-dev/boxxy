use std::path::{Path, PathBuf};
use std::process::Command;

fn resolve_git_dir(dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    if dot_git.is_file()
        && let Ok(contents) = std::fs::read_to_string(dot_git)
        && let Some(gitdir) = contents.trim().strip_prefix("gitdir:")
    {
        let raw = Path::new(gitdir.trim());
        let resolved = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            dot_git.parent()?.join(raw)
        };
        if resolved.is_dir() {
            return Some(resolved);
        }
    }
    None
}

fn main() {
    // If the environment provides a hash (e.g. from CI or Nix), use it. Otherwise, query git.
    let output = std::env::var("GIT_HASH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string())
        });

    println!("cargo:rustc-env=GIT_HASH={output}");
    println!("cargo:rerun-if-env-changed=GIT_HASH");

    // Only emit rerun-if-changed for paths that actually exist on disk so tarball/Nix
    // builds and packed-ref repos do not trigger unconditional Cargo rebuilds.
    if let Some(git_dir) = resolve_git_dir(Path::new("../../.git")) {
        let head_path = git_dir.join("HEAD");
        if head_path.exists() {
            println!("cargo:rerun-if-changed={}", head_path.display());
            if let Ok(ref_text) = std::fs::read_to_string(&head_path)
                && let Some(stripped) = ref_text.strip_prefix("ref:")
            {
                let ref_file = git_dir.join(stripped.trim());
                if ref_file.exists() {
                    println!("cargo:rerun-if-changed={}", ref_file.display());
                }
            }
        }
        let packed_refs = git_dir.join("packed-refs");
        if packed_refs.exists() {
            println!("cargo:rerun-if-changed={}", packed_refs.display());
        }
    }
}
