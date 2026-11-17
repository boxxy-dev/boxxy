//! Sandbox, installation channel, and self-update capability detection.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static HAS_FLATPAK_INFO_FILE: OnceLock<bool> = OnceLock::new();
static CANONICAL_EXE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

const SYSTEM_MANAGED_PREFIXES: &[&str] = &[
    "/nix/store",
    "/usr",
    "/opt",
    "/snap",
    "/bin",
    "/sbin",
    "/app",
];

/// Returns `true` if the application is running inside a Flatpak sandbox.
///
/// Deliberately checks `/.flatpak-info` and `FLATPAK_ID` directly rather than
/// `ashpd::is_sandboxed()`, which also returns `true` when `GTK_USE_PORTAL=1`
/// is set on native desktops or inside Snap packages (where `flatpak-spawn`
/// does not exist).
#[must_use]
pub fn is_flatpak() -> bool {
    let has_info = *HAS_FLATPAK_INFO_FILE.get_or_init(|| Path::new("/.flatpak-info").exists());
    has_info || std::env::var_os("FLATPAK_ID").is_some()
}

fn is_system_managed_exe_path(path: &Path) -> bool {
    SYSTEM_MANAGED_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

fn canonical_current_exe() -> Option<&'static Path> {
    CANONICAL_EXE_PATH
        .get_or_init(|| {
            std::env::current_exe()
                .ok()
                .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        })
        .as_deref()
}

/// Classifies the installation channel into one of the three telemetry-safe package buckets:
/// - `"flatpak"` — Flatpak sandboxed build
/// - `"distro"` — Nix, Arch AUR, Fedora/RPM, Debian/APT, Gentoo, or any system-packaged build
/// - `"native"` — Official standalone install (`~/.local/boxxy-terminal`) or local cargo build
#[must_use]
pub fn classify_package_type(
    flatpak: bool,
    self_update_compiled_in: bool,
    exe_path: Option<&Path>,
) -> &'static str {
    if flatpak {
        return "flatpak";
    }
    if !self_update_compiled_in {
        return "distro";
    }
    if exe_path.is_some_and(is_system_managed_exe_path) {
        return "distro";
    }
    "native"
}

/// Detects the active runtime installation channel (`"flatpak"`, `"distro"`, or `"native"`).
#[must_use]
pub fn detect_package_type() -> &'static str {
    let self_update_compiled_in = cfg!(all(
        feature = "self-update",
        not(feature = "disable-self-update")
    ));
    classify_package_type(
        is_flatpak(),
        self_update_compiled_in,
        canonical_current_exe(),
    )
}

#[must_use]
const fn can_self_update_for(
    self_update_compiled_in: bool,
    dev_instance: bool,
    package_type: &str,
) -> bool {
    self_update_compiled_in && !dev_instance && matches!(package_type.as_bytes(), b"native")
}

/// Returns `true` if the internal self-updater is allowed to run.
///
/// Disabled in Flatpak, in system-managed package paths (`/usr`, `/nix/store`, etc.),
/// in isolated dev instances, or when compiled with `disable-self-update`.
#[must_use]
pub fn can_self_update() -> bool {
    let self_update_compiled_in = cfg!(all(
        feature = "self-update",
        not(feature = "disable-self-update")
    ));
    if !self_update_compiled_in {
        return false;
    }
    can_self_update_for(
        self_update_compiled_in,
        crate::is_dev_instance(),
        detect_package_type(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::EnvStateGuard;
    use crate::{CliDevOverrides, configure_dev_options};
    use serial_test::serial;

    #[test]
    fn test_classify_package_type() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let standalone_bin = tmp.path().join("boxxy-terminal/bin/boxxy-terminal");

        assert_eq!(
            classify_package_type(true, true, Some(&standalone_bin)),
            "flatpak"
        );
        assert_eq!(
            classify_package_type(false, false, Some(&standalone_bin)),
            "distro"
        );
        assert_eq!(
            classify_package_type(
                false,
                true,
                Some(Path::new(
                    "/nix/store/00000000000000000000000000000000-boxxy/bin/boxxy-terminal"
                ))
            ),
            "distro"
        );
        assert_eq!(
            classify_package_type(false, true, Some(Path::new("/usr/bin/boxxy-terminal"))),
            "distro"
        );
        assert_eq!(
            classify_package_type(false, true, Some(Path::new("/app/bin/boxxy-terminal"))),
            "distro"
        );
        // Component-aware matching must not misclassify sibling directory names like `/usr_local_custom`.
        assert_eq!(
            classify_package_type(
                false,
                true,
                Some(Path::new("/usr_custom/boxxy-terminal/bin/boxxy-terminal"))
            ),
            "native"
        );
        assert_eq!(
            classify_package_type(false, true, Some(&standalone_bin)),
            "native"
        );
    }

    #[test]
    fn test_can_self_update_for_matrix() {
        assert!(can_self_update_for(true, false, "native"));
        assert!(!can_self_update_for(false, false, "native"));
        assert!(!can_self_update_for(true, true, "native"));
        assert!(!can_self_update_for(true, false, "flatpak"));
        assert!(!can_self_update_for(true, false, "distro"));
    }

    #[test]
    #[serial(config_dir)]
    fn test_gtk_use_portal_does_not_trigger_flatpak_detection() {
        let mut guard = EnvStateGuard::new(None);
        guard.remove_extra_var("FLATPAK_ID");
        guard.set_extra_var("GTK_USE_PORTAL", "1");

        if !Path::new("/.flatpak-info").exists() {
            assert!(
                !is_flatpak(),
                "GTK_USE_PORTAL=1 on a native host must never report is_flatpak() == true"
            );
        }

        guard.set_extra_var("FLATPAK_ID", "dev.boxxy.BoxxyTerminal");
        assert!(is_flatpak());
    }

    #[test]
    #[serial(config_dir)]
    fn test_can_self_update_logic() {
        let _guard = EnvStateGuard::new(None);
        let pkg_type = detect_package_type();
        let can_update = can_self_update();

        if cfg!(feature = "disable-self-update") || pkg_type != "native" {
            assert!(
                !can_update,
                "Self-update must be disabled for non-native or disable-self-update builds"
            );
        } else {
            assert!(
                can_update,
                "Self-update should be enabled for standalone native builds"
            );
        }

        // Dev instances must always disable self-update
        configure_dev_options(CliDevOverrides {
            dev: true,
            ..Default::default()
        });
        assert!(
            !can_self_update(),
            "Self-update must be disabled in dev mode"
        );
    }
}
