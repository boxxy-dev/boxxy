use std::time::{Duration, Instant};

/// Cooldown after a transient MCP connection failure before re-probing.
pub(super) const TRANSIENT_COOLDOWN: Duration = Duration::from_secs(60);
/// Cooldown after a permanent configuration or authorization failure before re-probing
/// (also immediately reset whenever the user edits the server in Preferences).
pub(super) const PERMANENT_CONFIG_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub(super) struct CircuitBreakerState {
    pub tripped_at: Instant,
    pub cooldown: Duration,
    pub last_error: String,
}

impl CircuitBreakerState {
    /// If the circuit breaker is still within its cooldown window, returns the open error.
    #[must_use]
    pub(super) fn open_error(&self, server_name: &str) -> Option<CircuitBreakerOpenError> {
        let elapsed = self.tripped_at.elapsed();
        if elapsed < self.cooldown {
            let remaining_secs = self.cooldown.saturating_sub(elapsed).as_secs().max(1);
            Some(CircuitBreakerOpenError {
                server_name: server_name.to_string(),
                remaining_secs,
                last_error: self.last_error.clone(),
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "MCP server '{server_name}' is in circuit-breaker cooldown ({remaining_secs}s remaining): {last_error}"
)]
pub struct CircuitBreakerOpenError {
    pub server_name: String,
    pub remaining_secs: u64,
    pub last_error: String,
}
