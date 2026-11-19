//! Zero-polling Wayland compositor socket hangup guard.
//!
//! If the Wayland compositor disconnects the client (e.g. `WL: error in client communication`)
//! at the exact moment GTK's main thread is blocked inside an infinite-timeout Vulkan WSI
//! explicit-sync call (`drm_syncobj_timeline_wait_ioctl`), `GLib`'s main-thread Wayland `GSource`
//! never runs to observe the broken pipe. Without an out-of-band hangup observer, the UI
//! process remains alive as a windowless zombie still owning `dev.boxxy.BoxxyTerminal` on D-Bus,
//! causing all subsequent desktop launcher invocations to time out after 25 seconds.
//!
//! This module resolves the Wayland display FD once on startup, duplicates it (`F_DUPFD_CLOEXEC`),
//! and blocks a single minimal-stack thread in `poll(fd, events = 0, timeout = -1)`. Because
//! `events = 0` masks `POLLIN` and `POLLOUT` inside the kernel, the thread incurs zero wakeups
//! during normal Wayland traffic and wakes strictly once if the compositor closes the socket
//! (`POLLHUP | POLLERR`).

use gtk4::prelude::*;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static GUARD_INSTALLED: OnceLock<()> = OnceLock::new();

/// Marks the process as undergoing clean `GApplication` shutdown so a concurrent
/// compositor teardown never triggers forced exit.
pub(crate) fn mark_shutting_down() {
    SHUTTING_DOWN.store(true, Ordering::Release);
}

/// Installs the one-time Wayland socket hangup guard if the default `gdk::Display`
/// is a `GdkWaylandDisplay`. Safe no-op on X11 or headless test environments.
pub(crate) fn install_for_default_display() {
    #[cfg(target_os = "linux")]
    {
        if GUARD_INSTALLED.get().is_some() {
            return;
        }
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };
        if display.type_().name() != "GdkWaylandDisplay" {
            return;
        }
        let Some(dup_fd) = dup_wayland_display_fd(&display) else {
            return;
        };
        let _ = GUARD_INSTALLED.set(());

        let _ = std::thread::Builder::new()
            .name("boxxy-wl-guard".to_string())
            .stack_size(64 * 1024)
            .spawn(move || run_hangup_watcher(&dup_fd));
    }
}

#[cfg(target_os = "linux")]
fn dup_wayland_display_fd(display: &gtk4::gdk::Display) -> Option<OwnedFd> {
    use gtk4::glib::translate::ToGlibPtr;
    use std::ffi::c_void;

    type GetWlDisplayFn = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type WlDisplayGetFdFn = unsafe extern "C" fn(*mut c_void) -> libc::c_int;

    // Resolve dynamically from already-loaded `libgtk-4` / `libwayland-client` so we
    // require zero extra pkg-config link flags on X11-only or minimal chroot builds.
    let raw_fd = unsafe {
        let get_wl_display_sym = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"gdk_wayland_display_get_wl_display".as_ptr(),
        );
        let get_fd_sym = libc::dlsym(libc::RTLD_DEFAULT, c"wl_display_get_fd".as_ptr());
        if get_wl_display_sym.is_null() || get_fd_sym.is_null() {
            return None;
        }

        let get_wl_display: GetWlDisplayFn = std::mem::transmute(get_wl_display_sym);
        let wl_display_get_fd: WlDisplayGetFdFn = std::mem::transmute(get_fd_sym);

        let gdk_ptr: *mut gtk4::gdk::ffi::GdkDisplay = display.to_glib_none().0;
        if gdk_ptr.is_null() {
            return None;
        }
        let wl_display = get_wl_display(gdk_ptr.cast::<c_void>());
        if wl_display.is_null() {
            return None;
        }
        wl_display_get_fd(wl_display)
    };

    if raw_fd < 0 {
        return None;
    }

    // Duplicate with `F_DUPFD_CLOEXEC` so normal `GdkDisplay` teardown closing the
    // original FD cannot race with FD reuse while this watcher exits.
    let borrowed = unsafe { BorrowedFd::borrow_raw(raw_fd) };
    borrowed.try_clone_to_owned().ok()
}

#[cfg(target_os = "linux")]
fn run_hangup_watcher(wayland_fd: &OwnedFd) {
    use std::os::fd::AsFd;
    use std::sync::Arc;

    if !wait_for_socket_hangup(wayland_fd.as_fd(), -1) {
        return;
    }
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return;
    }

    // Probe whether the GTK main loop is responsive enough to handle the Wayland
    // disconnect itself (`Lost connection to Wayland compositor`).
    let main_loop_alive = Arc::new(AtomicBool::new(false));
    let alive_probe = Arc::clone(&main_loop_alive);
    gtk4::glib::idle_add_once(move || {
        alive_probe.store(true, Ordering::Release);
    });

    // Give normal GTK teardown up to 2s if the main thread is blocked, or 5s total
    // if the main loop ticked once yet still failed to terminate on a dead Wayland socket.
    let start = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            return;
        }
        let elapsed = start.elapsed();
        let ticked = main_loop_alive.load(Ordering::Acquire);
        if (!ticked && elapsed >= Duration::from_secs(2)) || elapsed >= Duration::from_secs(5) {
            log::error!(
                "Wayland compositor socket closed (main_loop_ticked={ticked}); exiting so D-Bus single-instance registration is released."
            );
            boxxy_bookmarks::manager::BookmarksManager::clean_runs_dir();
            std::process::exit(1);
        }
    }
}

/// Blocks in kernel `poll(2)` with `events = 0` (`POLLHUP | POLLERR` only) up to
/// `timeout_ms` (`-1` = infinite). Returns `true` when the peer closes or errors the socket.
#[cfg(any(target_os = "linux", test))]
fn wait_for_socket_hangup(fd: BorrowedFd<'_>, timeout_ms: libc::c_int) -> bool {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: 0,
        revents: 0,
    };

    loop {
        let rc = unsafe { libc::poll(std::ptr::addr_of_mut!(pfd), 1, timeout_ms) };
        match rc.cmp(&0) {
            std::cmp::Ordering::Greater => {
                if (pfd.revents & libc::POLLNVAL) != 0 {
                    return false;
                }
                if (pfd.revents & (libc::POLLHUP | libc::POLLERR)) != 0 {
                    return true;
                }
            }
            std::cmp::Ordering::Equal => return false,
            std::cmp::Ordering::Less => {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn poll_zero_events_ignores_inbound_traffic_and_detects_peer_hangup() {
        let (client, mut server) = UnixStream::pair().expect("socketpair must succeed");
        let dup_client = client
            .as_fd()
            .try_clone_to_owned()
            .expect("dup must succeed");

        // Regular inbound Wayland traffic must NOT wake `events = 0`.
        server
            .write_all(b"synthetic-wayland-frame")
            .expect("write must succeed");
        assert!(
            !wait_for_socket_hangup(dup_client.as_fd(), 15),
            "inbound socket payload must not trigger hangup with events=0"
        );

        // Closing the compositor side must immediately surface POLLHUP even while
        // both `client` and `dup_client` FDs remain open locally.
        drop(server);
        assert!(
            wait_for_socket_hangup(dup_client.as_fd(), 200),
            "peer close must wake hangup watcher immediately"
        );
    }
}
