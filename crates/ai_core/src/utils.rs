use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Returns a reference to the global multi-threaded Tokio runtime.
/// This runtime is used for background tasks (I/O, CPU-heavy work)
/// to keep them off the GTK UI thread.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

/// Returns true if the application is running inside a Flatpak sandbox.
pub fn is_flatpak() -> bool {
    ashpd::is_sandboxed()
}
