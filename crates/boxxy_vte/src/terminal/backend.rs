use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::sync::Arc;

use crate::engine::event::{Event, EventListener, OnResize, WindowSize};
use crate::engine::event_loop::{EventLoop, Msg, Notifier};
use crate::engine::grid::Dimensions;
use crate::engine::term::{Config, RenderState, Term};
use crate::engine::tty::{self, ChildEvent, EventedPty, EventedReadWrite, Options};
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
        loop {
            let n = unsafe {
                libc::read(
                    self.master_fd.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as usize);
        }
    }
}

impl Write for ProxyAgentPty {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        loop {
            let n = unsafe {
                libc::write(
                    self.master_fd.as_raw_fd(),
                    buf.as_ptr() as *const libc::c_void,
                    buf.len(),
                )
            };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as usize);
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AsRawFd for ProxyAgentPty {
    fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        self.master_fd.as_raw_fd()
    }
}

impl EventedReadWrite for ProxyAgentPty {
    type Reader = Self;
    type Writer = Self;

    fn reader(&mut self) -> &mut Self::Reader {
        self
    }
    fn writer(&mut self) -> &mut Self::Writer {
        self
    }
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
        let proxy = GtkEventProxy {
            sender,
            pending_wakeups: pending_wakeups.clone(),
        };
        let config = Config::default();
        let term = Term::new(config, &dim_size, proxy.clone());

        // Build the event loop and extract the channel half (Notifier) we need
        // to send input to the PTY without holding a reference to the loop.
        let (event_loop, render_state) = EventLoop::new(term, proxy, pty, false, false).unwrap();

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
        Self::from_fd_with_replay(sender, master_fd, Vec::new())
    }

    pub fn from_fd_with_replay(
        sender: async_channel::Sender<Event>,
        master_fd: OwnedFd,
        replay_bytes: Vec<u8>,
    ) -> Self {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let (cols, lines) = unsafe {
            if libc::ioctl(master_fd.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) == 0
                && ws.ws_col > 0
                && ws.ws_row > 0
            {
                (ws.ws_col as usize, ws.ws_row as usize)
            } else {
                (80, 24)
            }
        };
        let dim_size = MySize { cols, lines };

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

        let proxy = GtkEventProxy {
            sender,
            pending_wakeups: pending_wakeups.clone(),
        };
        let config = Config::default();
        let term = Term::new(config, &dim_size, proxy.clone());

        let pty = ProxyAgentPty { master_fd };

        let (event_loop, render_state) = EventLoop::new(term, proxy, pty, false, false).unwrap();

        let notifier = Notifier(event_loop.channel());
        // Enqueue replay bytes BEFORE spawning the event loop task so they are
        // guaranteed to be parsed into the VTE state machine prior to any live
        // bytes read from `master_fd`.
        if !replay_bytes.is_empty() {
            let _ = notifier.0.send(Msg::ReplayOutput(replay_bytes));
        }

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
        if bytes.is_empty() {
            return;
        }
        let _ = self.notifier.0.send(Msg::Input(bytes.into()));
    }

    /// Feed buffered output captured while detached directly into the VTE parser.
    pub fn replay_output(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.notifier.0.send(Msg::ReplayOutput(bytes));
    }

    pub fn clear_pending_wakeups(&self) {
        self.pending_wakeups
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Resize the terminal grid and notify the PTY (triggers `SIGWINCH` in the
    /// child process).  No-ops when the dimensions have not changed.
    pub fn resize(
        &self,
        columns: usize,
        lines: usize,
        cell_width: f64,
        cell_height: f64,
        pixel_width: i32,
        pixel_height: i32,
    ) {
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

    pub fn scroll_to_prompt(&self, direction: crate::engine::index::Direction) {
        let _ = self.notifier.0.send(Msg::ScrollToPrompt(direction));
    }

    pub fn copy_last_command_output(&self, clipboard_type: crate::engine::term::ClipboardType) {
        let _ = self
            .notifier
            .0
            .send(Msg::CopyLastCommandOutput(clipboard_type));
    }

    pub fn set_selection(&self, sel: Option<crate::engine::selection::Selection>) {
        let _ = self.notifier.0.send(Msg::UpdateSelection(sel));
    }

    pub fn update_selection(
        &self,
        point: crate::engine::index::Point,
        side: crate::engine::index::Side,
    ) {
        let _ = self.notifier.0.send(Msg::UpdateSelectionExt(point, side));
    }

    pub fn update_selection_viewport(
        &self,
        row: usize,
        col: crate::engine::index::Column,
        side: crate::engine::index::Side,
    ) {
        let _ = self
            .notifier
            .0
            .send(Msg::UpdateSelectionViewport(row, col, side));
    }

    pub fn scroll_and_update_selection(
        &self,
        scroll: crate::engine::grid::Scroll,
        row: usize,
        col: crate::engine::index::Column,
        side: crate::engine::index::Side,
    ) {
        let _ = self
            .notifier
            .0
            .send(Msg::ScrollAndSelect(scroll, row, col, side));
    }

    pub fn select_all(&self) {
        let _ = self.notifier.0.send(Msg::SelectAll);
    }

    pub fn adjust_selection(&self, adjust: crate::engine::selection::SelectionAdjust) {
        let _ = self.notifier.0.send(Msg::AdjustSelection(adjust));
    }

    pub fn clear_selection(&self) {
        let _ = self.notifier.0.send(Msg::ClearSelection);
    }

    pub fn clear_scrollback(&self) {
        let _ = self.notifier.0.send(Msg::ClearScrollback);
    }

    pub fn reset_state(&self) {
        let _ = self.notifier.0.send(Msg::ResetState);
    }

    pub fn copy_selection(&self, clipboard_type: crate::engine::term::ClipboardType) {
        let _ = self.notifier.0.send(Msg::CopySelection(clipboard_type));
    }

    pub fn search(
        &self,
        query: String,
        direction: crate::engine::index::Direction,
        case_insensitive: bool,
    ) {
        let _ = self
            .notifier
            .0
            .send(Msg::Search(query, direction, case_insensitive));
    }

    pub async fn get_text_snapshot(&self, max_lines: usize, offset_lines: usize) -> Option<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .notifier
            .0
            .send(Msg::GetTextSnapshot(max_lines, offset_lines, tx))
            .is_ok()
        {
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

    /// Write text to the PTY, wrapping in bracketed-paste sequences when the
    /// running application has enabled that mode (e.g. fish, Fresh, vim).
    pub fn paste(&self, text: String) {
        let bracketed = self
            .render_state
            .load()
            .mode
            .contains(crate::engine::term::TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            let mut buf = Vec::with_capacity(text.len() + 12);
            buf.extend_from_slice(b"\x1b[200~");
            buf.extend_from_slice(text.as_bytes());
            buf.extend_from_slice(b"\x1b[201~");
            buf
        } else {
            text.into_bytes()
        };
        self.write_to_pty(bytes);
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        // Explicitly signal the background PTY event loop to exit and close
        // the message channel so its `master_fd` and `GtkEventProxy` are
        // dropped immediately—even when `pty_persistence` keeps the child
        // shell running in the daemon.
        let _ = self.notifier.0.send(Msg::Shutdown);
        self.notifier.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::FromRawFd;

    #[tokio::test]
    async fn dropping_terminal_backend_shuts_down_event_loop_and_closes_event_channel() {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        // Keep write_fd open so `read_fd` never hits EOF on its own.
        let _write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

        let (tx, rx) = async_channel::unbounded::<Event>();
        let backend = TerminalBackend::from_fd(tx, read_fd);

        // Dropping the backend must send `Msg::Shutdown`, terminating the
        // `EventLoop` task and dropping `GtkEventProxy`, which closes `rx`.
        drop(backend);

        let recv_result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Ok(ev) = rx.recv().await {
                if matches!(ev, Event::Exit) {
                    break;
                }
            }
            rx.is_closed()
        })
        .await;

        assert_eq!(recv_result, Ok(true));
    }

    #[tokio::test]
    async fn coalesced_pty_output_flushes_trailing_snapshot_and_immediate_selection_bypasses_throttle()
     {
        use crate::engine::index::{Column, Line, Point, Side};
        use crate::engine::selection::{Selection, SelectionType};

        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

        let (tx, rx) = async_channel::unbounded::<Event>();
        let backend = TerminalBackend::from_fd(tx, read_fd);

        // 1. First write publishes immediately (0ms initial latency).
        unsafe {
            libc::write(write_fd.as_raw_fd(), b"First\r\n".as_ptr() as *const _, 7);
        }
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("first wakeup timeout");
        backend.clear_pending_wakeups();
        assert_eq!(
            backend
                .render_state
                .load()
                .cell(Point::new(Line(0), Column(0)))
                .c,
            'F'
        );

        // 2. Burst of rapid writes within the 8ms throttle window is coalesced,
        // and the trailing flush timer guarantees the final text is published.
        for _ in 0..10 {
            unsafe {
                libc::write(write_fd.as_raw_fd(), b"X".as_ptr() as *const _, 1);
            }
        }
        unsafe {
            libc::write(write_fd.as_raw_fd(), b"Z".as_ptr() as *const _, 1);
        }

        // Wait slightly longer than MIN_SNAPSHOT_INTERVAL (8ms) for trailing flush.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        while rx.try_recv().is_ok() {}
        backend.clear_pending_wakeups();
        assert_eq!(
            backend
                .render_state
                .load()
                .cell(Point::new(Line(1), Column(10)))
                .c,
            'Z'
        );

        // 3. Interactive selection (`RenderPriority::Immediate`) updates immediately
        // without waiting for the throttle window.
        backend.set_selection(Some(Selection::new(
            SelectionType::Semantic,
            Point::new(Line(0), Column(0)),
            Side::Left,
        )));

        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("selection wakeup timeout");
        assert_eq!(
            backend.render_state.load().selection_text().as_deref(),
            Some("First")
        );
    }

    #[tokio::test]
    async fn scroll_and_select_captures_full_offscreen_history_selection() {
        use crate::engine::grid::Scroll;
        use crate::engine::index::{Column, Line, Point, Side};
        use crate::engine::selection::{Selection, SelectionType};

        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

        let (tx, rx) = async_channel::unbounded::<Event>();
        let backend = TerminalBackend::from_fd(tx, read_fd);

        // Emit 40 lines into a 24-line terminal so lines 0..16 scroll into history.
        let mut payload = String::new();
        for i in 0..40 {
            payload.push_str(&format!("line-{i:02}\r\n"));
        }
        unsafe {
            libc::write(
                write_fd.as_raw_fd(),
                payload.as_ptr() as *const _,
                payload.len(),
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        while rx.try_recv().is_ok() {}
        backend.clear_pending_wakeups();

        let state = backend.render_state.load();
        assert!(
            state.total_lines > state.screen_lines,
            "expected scrollback history"
        );

        // Start selection at the bottom visible line (line-39, above the final trailing newline).
        let bottom_content_line = Line(state.screen_lines as i32 - 2);
        backend.set_selection(Some(Selection::new(
            SelectionType::Simple,
            Point::new(bottom_content_line, Column(7)),
            Side::Right,
        )));
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("initial selection wakeup");
        backend.clear_pending_wakeups();

        // Simulate edge auto-scrolling / wheel-scrolling all the way to the top of history
        // while keeping the drag cursor at the top-left of the viewport (row 0, col 0).
        let history_lines = (state.total_lines - state.screen_lines) as i32;
        backend.scroll_and_update_selection(
            Scroll::Delta(history_lines),
            0,
            Column(0),
            Side::Left,
        );

        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("scroll_and_select wakeup");
        backend.clear_pending_wakeups();

        let full_text = backend
            .render_state
            .load()
            .selection_text()
            .expect("expected non-empty selection text");

        // Every single line from line-00 (in history) through line-39 (now off-screen below
        // the scrolled viewport) must be present in `selection_text()`.
        let copied_lines: Vec<&str> = full_text.lines().collect();
        assert_eq!(copied_lines.len(), 40);
        assert_eq!(copied_lines.first().copied(), Some("line-00"));
        assert_eq!(copied_lines.last().copied(), Some("line-39"));
    }

    #[tokio::test]
    async fn empty_rows_selection_does_not_produce_newline_only_clipboard_text() {
        use crate::engine::index::{Column, Line, Point, Side};
        use crate::engine::selection::{Selection, SelectionType};

        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let _write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

        let (tx, rx) = async_channel::unbounded::<Event>();
        let backend = TerminalBackend::from_fd(tx, read_fd);

        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(Line(5), Column(0)),
            Side::Left,
        );
        sel.update(Point::new(Line(8), Column(10)), Side::Right);
        backend.set_selection(Some(sel));

        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("selection wakeup");

        assert!(
            backend.render_state.load().selection_text().is_none(),
            "selecting across blank rows must not yield a newline-only clipboard string"
        );
    }
}
