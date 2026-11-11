use boxxy_core_toolbox::utils::resolve_path;
use boxxy_preferences::config::{normalize_vibe_dir_path, validate_vibe_trusted_dir};
use std::path::Path;

const BLOCKED_FILENAMES: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".zlogin",
    ".profile",
    "config.fish",
    ".gitconfig",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "authorized_keys",
    "known_hosts",
    "id_rsa",
    "id_ed25519",
    "id_ed25519_sk",
    "id_ecdsa",
    "id_ecdsa_sk",
    "id_dsa",
];

/// Returns `true` if `cwd` is inside at least one valid entry in `trusted_dirs`.
#[must_use]
pub fn is_cwd_vibe_trusted(cwd: &str, trusted_dirs: &[String]) -> bool {
    let Some(cwd_norm) = normalize_vibe_dir_path(cwd) else {
        return false;
    };
    if validate_vibe_trusted_dir(&cwd_norm.to_string_lossy()).is_err() {
        return false;
    }
    trusted_dirs.iter().any(|trusted| {
        validate_vibe_trusted_dir(trusted)
            .is_ok_and(|valid_root| cwd_norm.starts_with(Path::new(&valid_root)))
    })
}

fn has_sensitive_components(path: &Path) -> bool {
    let components: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => Some(os.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();

    for (i, comp) in components.iter().enumerate() {
        if matches!(
            comp.as_str(),
            ".ssh" | ".gnupg" | ".aws" | ".kube" | ".docker"
        ) {
            return true;
        }
        if comp == ".git"
            && let Some(next) = components.get(i + 1)
            && matches!(next.as_str(), "hooks" | "config" | "info")
        {
            return true;
        }
    }

    if let Some(file_name) = components.last() {
        if file_name == ".env" {
            return true;
        }
        if file_name.starts_with(".env.")
            && !matches!(
                file_name.as_str(),
                ".env.example" | ".env.sample" | ".env.template" | ".env.dist"
            )
        {
            return true;
        }
        if BLOCKED_FILENAMES.contains(&file_name.as_str()) {
            return true;
        }
    }

    false
}

/// Returns `true` if writing or modifying `target_raw` (resolved against `cwd`)
/// is safe to auto-approve in Vibe Mode.
#[must_use]
pub fn is_safe_vibe_path(target_raw: &str, cwd: &str, trusted_dirs: &[String]) -> bool {
    let trimmed = target_raw.trim();
    if trimmed.is_empty()
        || trimmed.contains(['*', '?', '[', '\0', '\n', '\r'])
        || !is_cwd_vibe_trusted(cwd, trusted_dirs)
    {
        return false;
    }

    let resolved_str = resolve_path(cwd, trimmed);
    let Some(resolved) = normalize_vibe_dir_path(&resolved_str) else {
        return false;
    };

    // Must remain inside at least one valid trusted directory.
    let inside_trusted = trusted_dirs.iter().any(|trusted| {
        validate_vibe_trusted_dir(trusted)
            .is_ok_and(|valid_root| resolved.starts_with(Path::new(&valid_root)))
    });
    if !inside_trusted {
        return false;
    }

    // Reject Boxxy's own configuration tree (both lexical and canonical).
    let boxxy_cfg = boxxy_sys_utils::get_config_dir();
    let canon_boxxy_cfg = boxxy_cfg
        .canonicalize()
        .unwrap_or_else(|_| boxxy_cfg.clone());
    if resolved.starts_with(&boxxy_cfg) || resolved.starts_with(&canon_boxxy_cfg) {
        return false;
    }

    if has_sensitive_components(&resolved) {
        return false;
    }

    // Inspect every existing/symlinked ancestor from leaf upward.
    // Using `symlink_metadata()` catches dangling symlinks (`link -> /etc/cron.d/new`),
    // where `Path::exists()` would return `false` and skip validation.
    for ancestor in resolved.ancestors() {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if ancestor.symlink_metadata().is_ok() {
            let Ok(canon) = ancestor.canonicalize() else {
                // Dangling or unreadable symlink cannot be verified safe.
                return false;
            };
            if canon.starts_with(&boxxy_cfg) || canon.starts_with(&canon_boxxy_cfg) {
                return false;
            }
            let canon_inside = trusted_dirs.iter().any(|trusted| {
                let Ok(valid_root) = validate_vibe_trusted_dir(trusted) else {
                    return false;
                };
                let root_path = Path::new(&valid_root);
                let canon_root = root_path
                    .canonicalize()
                    .unwrap_or_else(|_| root_path.to_path_buf());
                canon.starts_with(&canon_root)
            });
            if !canon_inside {
                return false;
            }
            let full_canon = if let Ok(rel_tail) = resolved.strip_prefix(ancestor) {
                canon.join(rel_tail)
            } else {
                canon
            };
            if has_sensitive_components(&full_canon) {
                return false;
            }
            break;
        }
    }

    true
}
