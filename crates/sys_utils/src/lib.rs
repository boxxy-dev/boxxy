#![allow(clippy::redundant_pub_crate)]

//! Minimal, leaf-level utility crate for system environment detection,
//! isolated dev/ephemeral configuration routing, and global async runtime management.
//!
//! Contains no AI, GTK, or workspace-internal dependencies. Provides the *mechanism*
//! for system checks and ambient data fetching; all *policy* (such as privacy toggles)
//! lives in callers.

mod config_dir;
pub mod dev_mode;
pub mod env;
mod platform;
mod remote;
mod runtime;

#[cfg(test)]
pub(crate) mod test_support;

pub use config_dir::{get_config_dir, get_stable_config_dir};
pub use dev_mode::{
    CliDevOverrides, DEVEL_AGENT_BUS_NAME, DEVEL_GTK_APP_ID, STABLE_AGENT_BUS_NAME,
    STABLE_GTK_APP_ID, agent_bus_name, configure_dev_options, dev_header_tag, gtk_app_id,
    instance_id, is_dev_instance, is_ephemeral_mode, prepare_dev_environment,
    sanitize_instance_slug, seed_config_tree_from, should_sync_settings,
};
pub use platform::{can_self_update, classify_package_type, detect_package_type, is_flatpak};
pub use remote::{
    LocationContext, fetch_exchange_rates, fetch_location_context, get_exchange_rate,
    get_location_context,
};
pub use runtime::{current_worker_index, runtime};
