use std::sync::Arc;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, OwnedFd};

use crate::engine::event::{Event, EventListener, WindowSize, OnResize};
use crate::engine::event_loop::{EventLoop, Notifier, Msg};
use crate::engine::term::{Config, Term, RenderState};
use crate::engine::tty::{self, Options, EventedPty, EventedReadWrite, ChildEvent};
use crate::engine::grid::Dimensions;
use arc_swap::ArcSwap;

use std::sync::atomic::{AtomicUsize, Ordering};

/// Proxies events from the PTY event loop back to GTK.
#[derive(Clone)]
pub struct GtkEventProxy {
    pub sender: async_channel::Sender<Event>,
    pub pending_wakeups: Arc<AtomicUsize>,
}

impl EventListener for GtkEventProxy {
    fn send_event(&self, event: Event) {
        if let Event::Wakeup = event {
            // Drop Wakeup if one is already pending
            if self.pending_wakeups.fetch_add(1, Ordering::SeqCst) > 0 {
                return;
            }
        }
        let _ = self.sender.try_send(event);
    }
}

pub struct ProxyAgentPty {
    pub master_fd: OwnedFd,
}

impl Read for ProxyAgentPty {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = unsafe {
            libc::read(self.master_fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len())
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                return Err(err);
            }
            return Err(err);
        }
        Ok(n as usize)
    }
}

impl Write for ProxyAgentPty {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = unsafe {
            libc::write(self.master_fd.as_raw_fd(), buf.as_ptr() as *const libc::c_void, buf.len())
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                return Err(err);
            }
            return Err(err);
        }
        Ok(n as usize)
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl AsRawFd for ProxyAgentPty {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.master_fd.as_raw_fd()
    }
}

impl EventedReadWrite for ProxyAgentPty {
    type Reader = Self;
    type Writer = Self;

    fn reader(&mut self) -> &mut Self::Reader { self }
    fn writer(&mut self) -> &mut Self::Writer { self }
}

impl OnResize for ProxyAgentPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        let ws = libc::winsize {
            ws_row: window_size.num_lines,
            ws_col: window_size.num_cols,
            ws_xpixel: window_size.pixel_width,
            ws_ypixel: window_size.pixel_height,
        };
        unsafe {
            libc::ioctl(self.master_fd.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }
    }
}

impl EventedPty for ProxyAgentPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        None
    }

    fn child_event_fd(&self) -> Option<std::os::unix::io::RawFd> {
        None
    }
}

pub struct TerminalBackend {
    pub render_state: Arc<ArcSwap<RenderState>>,
    pub notifier: Notifier,
    pub pending_wakeups: Arc<AtomicUsize>,
    /// PID of the direct child process (the shell on native, host-spawn on
    /// Flatpak).  Used to read `/proc/{pid}/cwd` for CWD tracking.
    pub child_pid: Option<u32>,
}

// ─── Internal Dimensions helper ───────────────────────────────────────────────

struct MySize {
    cols: usize,
    lines: usize,
}

impl Dimensions for MySize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

fn set_nonblocking(fd: std::os::unix::io::RawFd) {
    use libc::{F_GETFL, F_SETFL, O_NONBLOCK, fcntl};
    let flags = unsafe { fcntl(fd, F_GETFL) };
    if flags < 0 {
        return;
    }
    unsafe {
        fcntl(fd, F_SETFL, flags | O_NONBLOCK);
    }
}

// ─── TerminalBackend ─────────────────────────────────────────────────────────

impl TerminalBackend {
    pub fn new(sender: async_channel::Sender<Event>, pty_options: Options) -> Self {
        let win_size = WindowSize {
            num_cols: 80,
            num_lines: 24,
            cell_width: 10,
            cell_height: 20,
            pixel_width: 800,
            pixel_height: 480,
        };

        let dim_size = MySize {
            cols: 80,
            lines: 24,
        };

        // Create the PTY — we must capture the child PID *before* moving `pty`
        // into the event loop (which consumes ownership).
        let pty = tty::new(&pty_options, win_size, 0).expect("Failed to create PTY");
        let child_pid = Some(pty.child().id());

        // Ensure non-blocking
        set_nonblocking(pty.file().as_raw_fd());

        let pending_wakeups = Arc::new(AtomicUsize::new(0));

        // Build the terminal state machine.
        let proxy = GtkEventProxy { sender, pending_wakeups: pending_wakeups.clone() };
        let config = Config::default();
        let term = Term::new(config, &dim_size, proxy.clone());

        // Build the event loop and extract the channel half (Notifier) we need
        // to send input to the PTY without holding a reference to the loop.
        let (event_loop, render_state) = EventLoop::new(
            term,
            proxy,
            pty,
            false,
            false,
        ).unwrap();

        let notifier = Notifier(event_loop.channel());

        event_loop.spawn();

        Self {
            render_state,
            notifier,
            pending_wakeups,
            child_pid,
        }
    }

    pub fn from_fd(sender: async_channel::Sender<Event>, master_fd: OwnedFd) -> Self {
        let dim_size = MySize {
            cols: 80,
            lines: 24,
        };

        // Ensure non-blocking
        set_nonblocking(master_fd.as_raw_fd());

        let pending_wakeups = Arc::new(AtomicUsize::new(0));

        let _win_size = WindowSize {
            num_cols: 80,
            num_lines: 24,
            cell_width: 10,
            cell_height: 20,
            pixel_width: 800,
            pixel_height: 480,
        };

        let proxy = GtkEventProxy { sender, pending_wakeups: pending_wakeups.clone() };
        let config = Config::default();
        let term = Term::new(config, &dim_size, proxy.clone());

        let pty = ProxyAgentPty { master_fd };

        let (event_loop, render_state) = EventLoop::new(
            term,
            proxy,
            pty,
            false,
            false,
        ).unwrap();

        let notifier = Notifier(event_loop.channel());

        event_loop.spawn();

        Self {
            render_state,
            notifier,
            pending_wakeups,
            child_pid: None,
        }
    }

    /// Write raw bytes into the PTY (keyboard input, pasted text, etc.).
    pub fn write_to_pty(&self, bytes: Vec<u8>) {
        let _ = self.notifier.0.send(Msg::Input(bytes.into()));
    }

    pub fn clear_pending_wakeups(&self) {
        self.pending_wakeups.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Resize the terminal grid and notify the PTY (triggers `SIGWINCH` in the
    /// child process).  No-ops when the dimensions have not changed.
    pub fn resize(&self, columns: usize, lines: usize, cell_width: f64, cell_height: f64, pixel_width: i32, pixel_height: i32) {
        let win_size = WindowSize {
            num_cols: columns as u16,
            num_lines: lines as u16,
            cell_width: cell_width.ceil() as u16,
            cell_height: cell_height.ceil() as u16,
            pixel_width: pixel_width as u16,
            pixel_height: pixel_height as u16,
        };
        // We always send resize_grid = true; the event loop will deduplicate or handle it properly.
        let _ = self.notifier.0.send(Msg::Resize(win_size, true));
    }

    /// Return the current working directory of the child process by reading
    /// `/proc/{pid}/cwd`.
    ///
    /// This works correctly on Linux for native (non-Flatpak) terminals where
    /// the child PID is the shell itself.  On Flatpak builds the child is
    /// `host-spawn`, which does not follow `cd` commands issued inside the
    /// host shell, so callers should skip CWD tracking in that case.
    ///
    /// Returns `None` if the PID is unavailable, the process has already
    /// exited, or `/proc` is not mounted.
    pub fn cwd(&self) -> Option<String> {
        let pid = self.child_pid?;
        let link = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        link.into_os_string().into_string().ok()
    }

    pub fn scroll_display(&self, scroll: crate::engine::grid::Scroll) {
        let _ = self.notifier.0.send(Msg::Scroll(scroll));
    }

    pub fn set_selection(&self, sel: Option<crate::engine::selection::Selection>) {
        let _ = self.notifier.0.send(Msg::UpdateSelection(sel));
    }

    pub fn update_selection(&self, point: crate::engine::index::Point, side: crate::engine::index::Side) {
        let _ = self.notifier.0.send(Msg::UpdateSelectionExt(point, side));
    }

    pub fn clear_selection(&self) {
        let _ = self.notifier.0.send(Msg::ClearSelection);
    }

    pub fn copy_selection(&self, clipboard_type: crate::engine::term::ClipboardType) {
        let _ = self.notifier.0.send(Msg::CopySelection(clipboard_type));
    }

    pub fn search(&self, query: String, direction: crate::engine::index::Direction, case_insensitive: bool) {
        let _ = self.notifier.0.send(Msg::Search(query, direction, case_insensitive));
    }

    pub async fn get_text_snapshot(&self, max_lines: usize) -> Option<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.notifier.0.send(Msg::GetTextSnapshot(max_lines, tx)).is_ok() {
            rx.await.ok()
        } else {
            None
        }
    }

    pub fn has_selection(&self) -> bool {
        self.render_state.load().selection_range.is_some()
    }

    pub fn is_alt_screen(&self) -> bool {
        self.render_state.load().is_alt_screen
    }
}
