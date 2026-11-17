//! Structured `env_logger` setup with quiet third-party defaults, `RUST_LOG` override
//! support, and `--debug-context` / `BOXXY_DEBUG_CONTEXT` promotion.

use log::LevelFilter;

const NOISY_MODULES: &[(&str, LevelFilter)] = &[
    ("zbus", LevelFilter::Warn),
    ("tracing", LevelFilter::Warn),
    ("sqlx", LevelFilter::Warn),
    ("rig", LevelFilter::Warn),
    ("rig_core", LevelFilter::Warn),
    ("rig_core::driver", LevelFilter::Error),
    ("rmcp", LevelFilter::Warn),
    ("h2", LevelFilter::Warn),
    ("hyper", LevelFilter::Warn),
    ("reqwest", LevelFilter::Warn),
    ("rustls", LevelFilter::Warn),
];

/// Initializes the global process logger.
///
/// Must be called *after* [`boxxy_sys_utils::configure_dev_options`] so `--debug-context`
/// is reflected when promoting the `model-context` target.
pub(crate) fn init() {
    let rust_log = std::env::var("RUST_LOG").ok();
    let debug_context = boxxy_sys_utils::env::debug_context();
    let logger = build_logger(rust_log.as_deref(), debug_context);
    let max_level = logger.filter();

    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max_level);
    }
}

fn build_logger(rust_log: Option<&str>, debug_context: bool) -> env_logger::Logger {
    let mut builder = env_logger::Builder::new();
    builder.filter_level(LevelFilter::Info);

    // Apply noisy-dependency defaults *before* parsing `RUST_LOG` so explicit developer
    // overrides such as `RUST_LOG=zbus=debug` or `RUST_LOG=sqlx=trace` take precedence.
    for &(module, level) in NOISY_MODULES {
        builder.filter_module(module, level);
    }

    if let Some(spec) = rust_log.map(str::trim).filter(|s| !s.is_empty()) {
        builder.parse_filters(spec);
    }

    // Guarantee `--debug-context` / `BOXXY_DEBUG_CONTEXT=1` surfaces `model-context`
    // payloads even when the user runs with a stricter global filter like `RUST_LOG=warn`.
    if debug_context {
        builder.filter_module("model-context", LevelFilter::Info);
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::{Level, Log, Metadata};

    fn allows(logger: &env_logger::Logger, target: &str, level: Level) -> bool {
        let meta = Metadata::builder().target(target).level(level).build();
        logger.enabled(&meta)
    }

    #[test]
    fn default_filters_silence_noisy_crates_while_keeping_app_info() {
        let logger = build_logger(None, false);

        assert!(allows(&logger, "boxxy_app", Level::Info));
        assert!(!allows(&logger, "boxxy_app", Level::Debug));
        assert!(!allows(&logger, "zbus::connection", Level::Info));
        assert!(allows(&logger, "zbus::connection", Level::Warn));
        assert!(!allows(&logger, "rig_core::driver", Level::Warn));
        assert!(allows(&logger, "rig_core::driver", Level::Error));
    }

    #[test]
    fn rust_log_can_override_noisy_module_filters() {
        let logger = build_logger(Some("debug,zbus=debug,rig_core::driver=info"), false);

        assert!(allows(&logger, "boxxy_window", Level::Debug));
        assert!(allows(&logger, "zbus::connection", Level::Debug));
        assert!(allows(&logger, "rig_core::driver", Level::Info));
        // Unmentioned noisy crates stay quiet at Warn even under global `debug`.
        assert!(!allows(&logger, "h2::codec", Level::Debug));
        assert!(allows(&logger, "h2::codec", Level::Warn));
    }

    #[test]
    fn debug_context_promotes_model_context_target_under_strict_rust_log() {
        let logger = build_logger(Some("error"), true);

        assert!(!allows(&logger, "boxxy_window", Level::Warn));
        assert!(allows(&logger, "model-context", Level::Info));
    }
}
