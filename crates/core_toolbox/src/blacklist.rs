//! Host filesystem path blacklist enforcement for workspace tools.
//!
//! Aligns `boxxy-core-toolbox` search tools with `boxxy-agent`'s `BLACKLIST.md` policy:
//! merges custom entries with built-in protected paths (never wiping built-ins), caches
//! parsed rules by `(config_path, mtime, size)`, enforces path-segment boundaries, and
//! resolves symlinks / `..` traversals before allowing access.

use parking_lot::Mutex;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

const BUILTIN_BLACKLIST: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/root/.ssh",
    ".ssh/id_rsa",
    ".ssh/id_ed25519",
    ".ssh/id_ed25519_sk",
    ".ssh/id_ecdsa",
    ".ssh/id_ecdsa_sk",
    ".ssh/id_dsa",
    ".ssh/authorized_keys",
    ".gnupg/private-keys-v1.d",
    ".aws/credentials",
    ".codex/auth.json",
];

type BlacklistCacheEntry = (PathBuf, Option<(SystemTime, u64)>, Arc<Vec<String>>);

static BLACKLIST_CACHE: LazyLock<Mutex<Option<BlacklistCacheEntry>>> =
    LazyLock::new(|| Mutex::new(None));

/// Merges user-supplied `BLACKLIST.md` lines on top of [`BUILTIN_BLACKLIST`].
#[must_use]
pub fn parse_blacklist_with_defaults(custom_content: Option<&str>) -> Vec<String> {
    let mut rules: Vec<String> = BUILTIN_BLACKLIST.iter().map(|s| (*s).to_string()).collect();
    if let Some(content) = custom_content {
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && !rules.iter().any(|existing| existing == trimmed)
            {
                rules.push(trimmed.to_string());
            }
        }
    }
    rules
}

/// Loads the merged path blacklist from `<config_dir>/boxxyclaw/BLACKLIST.md` with mtime caching.
#[must_use]
pub fn load_blacklist() -> Arc<Vec<String>> {
    let config_dir = boxxy_sys_utils::get_config_dir();
    let blacklist_path = config_dir.join("boxxyclaw").join("BLACKLIST.md");
    let stamp = std::fs::metadata(&blacklist_path)
        .ok()
        .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()));

    {
        let guard = BLACKLIST_CACHE.lock();
        if let Some((cached_path, cached_stamp, rules)) = guard.as_ref()
            && cached_path == &blacklist_path
            && cached_stamp == &stamp
        {
            return rules.clone();
        }
    }

    let custom = stamp
        .is_some()
        .then(|| std::fs::read_to_string(&blacklist_path).ok())
        .flatten();
    let mut parsed = parse_blacklist_with_defaults(custom.as_deref());
    let auth_dir_rule = config_dir.join("auth").to_string_lossy().into_owned();
    if !parsed.iter().any(|r| r == &auth_dir_rule) {
        parsed.push(auth_dir_rule);
    }
    let rules = Arc::new(parsed);
    *BLACKLIST_CACHE.lock() = Some((blacklist_path, stamp, rules.clone()));
    rules
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            c => normalized.push(c.as_os_str()),
        }
    }
    normalized
}

fn resolve_deepest_canonical(path: &Path) -> Option<PathBuf> {
    if let Ok(c) = std::fs::canonicalize(path) {
        return Some(c);
    }

    let has_parent_dir = path.components().any(|c| matches!(c, Component::ParentDir));
    if !has_parent_dir {
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            if let Ok(canon_anc) = std::fs::canonicalize(ancestor) {
                if let Ok(rel_tail) = path.strip_prefix(ancestor) {
                    return Some(canon_anc.join(rel_tail));
                }
                break;
            }
        }
    }

    let mut acc = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                acc.pop();
                if !acc.as_os_str().is_empty()
                    && let Ok(canon) = std::fs::canonicalize(&acc)
                {
                    acc = canon;
                }
            }
            c => {
                acc.push(c.as_os_str());
                if let Ok(canon) = std::fs::canonicalize(&acc) {
                    acc = canon;
                }
            }
        }
    }
    (!acc.as_os_str().is_empty()).then_some(acc)
}

/// Checks whether `candidate_path` matches `rule` on path segment boundaries.
#[must_use]
pub fn path_matches_blacklist_entry(candidate_path: &str, rule: &str) -> bool {
    let rule = rule.trim().trim_end_matches('/');
    if rule.is_empty() {
        return false;
    }
    let cand = candidate_path.trim_end_matches('/');
    if rule.starts_with('/') {
        cand == rule
            || cand
                .strip_prefix(rule)
                .is_some_and(|rest| rest.starts_with('/'))
    } else {
        if cand == rule {
            return true;
        }
        if cand
            .strip_prefix(rule)
            .is_some_and(|rest| rest.starts_with('/'))
        {
            return true;
        }
        if let Some(prefix) = cand.strip_suffix(rule)
            && prefix.ends_with('/')
        {
            return true;
        }
        // Allocation-free segment check for `/<rule>/` inside `cand`
        let bytes = cand.as_bytes();
        let r_bytes = rule.as_bytes();
        let needed = r_bytes.len() + 2;
        if bytes.len() >= needed {
            for idx in 0..=(bytes.len() - needed) {
                if bytes[idx] == b'/'
                    && bytes[idx + 1 + r_bytes.len()] == b'/'
                    && &bytes[idx + 1..idx + 1 + r_bytes.len()] == r_bytes
                {
                    return true;
                }
            }
        }
        false
    }
}

/// Returns `true` if `path` (raw, lexically normalized, or symlink-resolved) matches any rule.
#[must_use]
pub fn is_path_blacklisted(path: &Path, rules: &[String]) -> bool {
    let raw = path.to_string_lossy();
    let lexical = normalize_lexical_path(path);
    let lexical_str = lexical.to_string_lossy();
    let anchored = if path.is_relative() {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    } else {
        path.to_path_buf()
    };
    let canonical = resolve_deepest_canonical(&anchored);
    let canon_str = canonical.as_deref().map(Path::to_string_lossy);

    rules.iter().any(|rule| {
        path_matches_blacklist_entry(&raw, rule)
            || path_matches_blacklist_entry(&lexical_str, rule)
            || canon_str
                .as_deref()
                .is_some_and(|c| path_matches_blacklist_entry(c, rule))
    })
}

/// Fast lexical-only segment check for non-symlink entries beneath an already-verified canonical root.
#[must_use]
pub(crate) fn is_entry_lexically_blacklisted(path: &Path, rules: &[String]) -> bool {
    let raw = path.to_string_lossy();
    rules
        .iter()
        .any(|rule| path_matches_blacklist_entry(&raw, rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_blacklist_merges_without_erasing_builtins() {
        let merged = parse_blacklist_with_defaults(Some("# comment\n.env\nsecrets/prod.toml\n"));
        assert!(merged.iter().any(|r| r == "/etc/shadow"));
        assert!(merged.iter().any(|r| r == "/etc/sudoers"));
        assert!(merged.iter().any(|r| r == ".ssh/authorized_keys"));
        assert!(merged.iter().any(|r| r == ".aws/credentials"));
        assert!(merged.iter().any(|r| r == ".codex/auth.json"));
        assert!(merged.iter().any(|r| r == ".env"));
        assert!(merged.iter().any(|r| r == "secrets/prod.toml"));
    }

    #[test]
    fn segment_boundary_blocks_exact_segments_without_substring_false_positives() {
        assert!(path_matches_blacklist_entry("/workspace/.env", ".env"));
        assert!(path_matches_blacklist_entry("/workspace/.env/keys", ".env"));
        assert!(!path_matches_blacklist_entry(
            "/workspace/src/environment.rs",
            ".env"
        ));
        assert!(path_matches_blacklist_entry(
            "/root/.ssh/id_ed25519",
            "/root/.ssh"
        ));
        assert!(!path_matches_blacklist_entry(
            "/root/.ssh_public/readme.txt",
            "/root/.ssh"
        ));
    }

    #[test]
    fn canonical_resolution_catches_parent_traversal_and_symlinks() {
        let rules = parse_blacklist_with_defaults(Some("vault_secret"));
        assert!(is_path_blacklisted(
            Path::new("/workspace/sub/../../etc/shadow"),
            &rules
        ));

        let tmp = tempfile::tempdir().unwrap();
        let secret_dir = tmp.path().join("vault_secret");
        std::fs::create_dir_all(&secret_dir).unwrap();
        let link_path = tmp.path().join("innocent_alias");
        std::os::unix::fs::symlink(&secret_dir, &link_path).unwrap();

        assert!(is_path_blacklisted(&link_path, &rules));

        // Active config_dir/auth (where ChatGPT OAuth tokens live) is blacklisted without
        // blocking unrelated `auth/` directories inside a user's workspace.
        let loaded = load_blacklist();
        let oauth_record = boxxy_sys_utils::get_config_dir()
            .join("auth")
            .join("chatgpt.json");
        assert!(is_path_blacklisted(&oauth_record, &loaded));
        assert!(!is_path_blacklisted(
            Path::new("/workspace/project/src/auth/login.rs"),
            &loaded
        ));
    }
}
