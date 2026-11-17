//! Shared RAII environment and CLI-override state guard for `#[serial(config_dir)]` unit tests.

use crate::{CliDevOverrides, configure_dev_options, dev_mode, env};
use serial_test::is_locked_serially;
use std::ffi::OsString;

pub(crate) struct EnvStateGuard {
    previous_env: Vec<(&'static str, Option<OsString>)>,
    extra_env: Vec<(&'static str, Option<OsString>)>,
    previous_cli: CliDevOverrides,
}

impl EnvStateGuard {
    pub(crate) fn new(config_dir_value: Option<&str>) -> Self {
        assert!(
            is_locked_serially(Some("config_dir")),
            "Tests mutating process env or CLI overrides must be annotated with #[serial(config_dir)]"
        );
        let previous_env = env::ENV_SPECS
            .iter()
            .filter(|s| s.name.starts_with("BOXXY_"))
            .map(|s| (s.name, std::env::var_os(s.name)))
            .collect::<Vec<_>>();
        for (k, _) in &previous_env {
            // SAFETY: Protected by `#[serial(config_dir)]` across all env-mutating tests in this crate.
            unsafe { std::env::remove_var(k) };
        }
        if let Some(v) = config_dir_value {
            // SAFETY: Protected by `#[serial(config_dir)]`.
            unsafe { std::env::set_var("BOXXY_CONFIG_DIR", v) };
        }
        let previous_cli = dev_mode::snapshot_cli_overrides();
        configure_dev_options(CliDevOverrides::default());
        Self {
            previous_env,
            extra_env: Vec::new(),
            previous_cli,
        }
    }

    pub(crate) fn set_extra_var(&mut self, key: &'static str, value: &str) {
        self.record_extra_once(key);
        // SAFETY: Protected by `#[serial(config_dir)]`.
        unsafe { std::env::set_var(key, value) };
    }

    pub(crate) fn remove_extra_var(&mut self, key: &'static str) {
        self.record_extra_once(key);
        // SAFETY: Protected by `#[serial(config_dir)]`.
        unsafe { std::env::remove_var(key) };
    }

    fn record_extra_once(&mut self, key: &'static str) {
        if !self.previous_env.iter().any(|(k, _)| *k == key)
            && !self.extra_env.iter().any(|(k, _)| *k == key)
        {
            self.extra_env.push((key, std::env::var_os(key)));
        }
    }
}

impl Drop for EnvStateGuard {
    fn drop(&mut self) {
        for (k, prev) in self.extra_env.iter().rev().chain(self.previous_env.iter()) {
            match prev {
                // SAFETY: Protected by `#[serial(config_dir)]`.
                Some(val) => unsafe { std::env::set_var(k, val) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        configure_dev_options(self.previous_cli.clone());
    }
}
