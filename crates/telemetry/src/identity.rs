use arc_swap::ArcSwap;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

static SESSION_ID: LazyLock<ArcSwap<String>> =
    LazyLock::new(|| ArcSwap::from_pointee(Uuid::new_v4().to_string()));

#[cfg(test)]
pub(crate) static TEST_TELEMETRY_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
const SYNTHETIC_TEST_INSTALL_ID: &str = "00000000-0000-4000-8000-000000000001";

/// Fast lock-free check of whether telemetry is currently enabled in user preferences.
#[inline]
#[must_use]
pub(crate) fn is_telemetry_enabled() -> bool {
    #[cfg(test)]
    {
        match TEST_TELEMETRY_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
            1 => return true,
            2 => return false,
            _ => {}
        }
    }
    if boxxy_sys_utils::is_dev_instance()
        && boxxy_sys_utils::env::telemetry_endpoint().is_none()
    {
        return false;
    }
    boxxy_preferences::Settings::load().enable_telemetry
}

/// Validates and canonicalizes a UUID string (lowercase hyphenated).
#[inline]
#[must_use]
pub(crate) fn parse_canonical_uuid(raw: &str) -> Option<String> {
    Uuid::parse_str(raw.trim()).ok().map(|u| u.to_string())
}

/// Reads the current `install_id` lock-free from `Settings` and validates that it is a
/// well-formed UUID so malformed or missing IDs are never written into `telemetry_journal`.
#[must_use]
pub(crate) fn valid_install_id() -> Option<String> {
    if let Some(raw) = boxxy_preferences::Settings::load().install_id.as_deref()
        && let Some(valid) = parse_canonical_uuid(raw)
    {
        return Some(valid);
    }

    #[cfg(test)]
    if TEST_TELEMETRY_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) == 1 {
        return Some(SYNTHETIC_TEST_INSTALL_ID.to_string());
    }

    None
}

/// Returns the active telemetry `session_id` (always a valid canonical `UUIDv4`).
#[must_use]
pub fn session_id() -> String {
    (**SESSION_ID.load()).clone()
}

/// Synchronizes the active telemetry `session_id` with `boxxy-agent`.
/// Non-UUID inputs are ignored so `SESSION_ID` can never be corrupted.
pub fn set_session_id(id: &str) {
    if let Some(canonical) = parse_canonical_uuid(id) {
        SESSION_ID.store(Arc::new(canonical));
    }
}

/// Rotates to a fresh `session_id` (called by `boxxy-agent` when a fresh UI session attaches)
/// and returns the new `UUIDv4` string.
#[must_use]
pub fn rotate_session_id() -> String {
    let fresh = Uuid::new_v4().to_string();
    SESSION_ID.store(Arc::new(fresh.clone()));
    fresh
}
