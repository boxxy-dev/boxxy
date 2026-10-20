//! The main event loop which performs I/O on the pseudoterminal.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::ansi;
use crate::engine::event::{self, Event, EventListener, WindowSize};
use crate::engine::grid::Scroll;
use crate::engine::index::{Column, Direction, Point, Side};
use crate::engine::selection::{Selection, SelectionType};
use crate::engine::term::ClipboardType;
use crate::engine::term::{RenderState, Term};
use crate::engine::tty;
use crate::engine::tty::{EventedPty, EventedReadWrite};
use arc_swap::ArcSwap;
use log::error;
use tokio::io::unix::AsyncFd;

/// Max bytes to read from the PTY before forced terminal synchronization.
pub(crate) const READ_BUFFER_SIZE: usize = 0x10_0000;

/// Max bytes to read from the PTY while the terminal is locked.
const MAX_LOCKED_READ: usize = u16::MAX as usize;

/// Minimum interval between coalesced (parser-driven) snapshots (~120 Hz).
/// Interactive UI events (scrolling, selection, search, resize) bypass this
/// throttle via `RenderPriority::Immediate`.
pub(crate) const MIN_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(8);

/// Priority level for publishing a new `RenderState` snapshot to the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPriority {
    /// Immediate publication for user interactions (selection, scroll, search, resize, exit).
    Immediate,
    /// Coalesced publication for high-throughput PTY parser output.
    Coalesced,
}

/// Messages that may be sent to the `EventLoop`.
#[derive(Debug)]
pub enum Msg {
    /// Data that should be written to the PTY.
    Input(Cow<'static, [u8]>),

    /// Buffered PTY output captured by the daemon while detached, replayed
    /// directly into the VTE state machine on reattach.
    ReplayOutput(Vec<u8>),

    /// Indicates that the `EventLoop` should shut down.
    Shutdown,

    /// Instruction to resize the PTY.
    Resize(WindowSize, bool),

    // RCU UI Actions
    Scroll(Scroll),
    UpdateSelection(Option<Selection>),
    UpdateSelectionExt(Point, Side),
    UpdateSelectionViewport(usize, Column, Side),
    ScrollAndSelect(Scroll, usize, Column, Side),
    ClearSelection,
    CopySelection(ClipboardType),
    Search(String, Direction, bool),
    GetTextSnapshot(usize, usize, tokio::sync::oneshot::Sender<String>),
}

/// The main event loop.
///
/// Handles all the PTY I/O and runs the PTY parser which updates terminal
/// state.
pub struct EventLoop<T: tty::EventedPty, U: EventListener> {
    pty: T,
    rx: async_channel::Receiver<Msg>,
    tx: Option<async_channel::Sender<Msg>>,
    terminal: Term<U>,
    render_state: Arc<ArcSwap<RenderState>>,
    spare_render_state: Option<Arc<RenderState>>,
    next_generation: u64,
    event_proxy: U,
    drain_on_exit: bool,
    ref_test: bool,
}

pub type EventLoopResult<T, U> = io::Result<(EventLoop<T, U>, Arc<ArcSwap<RenderState>>)>;

impl<T, U> EventLoop<T, U>
where
    T: EventedReadWrite + EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Send + 'static,
{
    /// Create a new event loop.
    pub fn new(
        mut terminal: Term<U>,
        event_proxy: U,
        pty: T,
        drain_on_exit: bool,
        ref_test: bool,
    ) -> EventLoopResult<T, U> {
        let (tx, rx) = async_channel::unbounded();
        let mut initial_state = RenderState::default();
        initial_state.update_from(&mut terminal, 1);
        let render_state = Arc::new(ArcSwap::from_pointee(initial_state));
        Ok((
            EventLoop {
                pty,
                tx: Some(tx),
                rx,
                terminal,
                render_state: render_state.clone(),
                spare_render_state: None,
                next_generation: 2,
                event_proxy,
                drain_on_exit,
                ref_test,
            },
            render_state,
        ))
    }

    pub fn channel(&self) -> EventLoopSender {
        EventLoopSender {
            sender: self
                .tx
                .as_ref()
                .expect("channel() called after spawn()")
                .clone(),
        }
    }

    /// Spawn the event loop as a background Tokio task.
    pub fn spawn(mut self) -> tokio::task::JoinHandle<(Self, State)> {
        // Drop internal sender clone so `self.rx.recv()` observes channel
        // closure as soon as all external `EventLoopSender`s are dropped.
        self.tx = None;

        tokio::spawn(async move {
            let mut state = State::default();
            let mut buf = vec![0u8; READ_BUFFER_SIZE];

            let pty_fd = self.pty.reader().as_raw_fd();
            // SAFETY: `self.pty` owns `pty_fd` and `child_event_fd`, and both
            // `AsyncFd` registrations are explicitly dropped before `self` is returned.
            let async_pty_reader = unsafe {
                AsyncFd::register(pty_fd).expect("Failed to register AsyncFd for PTY reader")
            };

            let async_signals = self.pty.child_event_fd().map(|fd| unsafe {
                AsyncFd::register(fd).expect("Failed to register AsyncFd for signals")
            });

            let mut pipe = if self.ref_test {
                Some(File::create("./boxxy.recording").expect("create boxxy recording"))
            } else {
                None
            };

            let mut resize_pending: Option<(WindowSize, bool)> = None;
            let mut last_snapshot_at: Option<tokio::time::Instant> = None;
            let mut pending_coalesced_render = false;

            'event_loop: loop {
                let sync_timeout = state.parser.sync_timeout().sync_timeout();
                let needs_write = state.needs_write();
                if needs_write {
                    log::trace!(
                        "EventLoop: loop start, needs_write=true, write_list_len={}, writing={}",
                        state.write_list.len(),
                        state.writing.is_some()
                    );
                }

                let sleep_fut = async {
                    if let Some(timeout) = sync_timeout {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(timeout)).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                };

                // The timeout for resize coalescing
                let resize_sleep_fut = async {
                    if resize_pending.is_some() {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                };

                // Trailing flush timer for coalesced PTY parser updates
                let coalesce_sleep_fut = async {
                    if pending_coalesced_render {
                        if let Some(last) = last_snapshot_at {
                            tokio::time::sleep_until(last + MIN_SNAPSHOT_INTERVAL).await;
                        }
                    } else {
                        std::future::pending::<()>().await;
                    }
                };

                let needs_write = state.needs_write();

                let write_fut = async {
                    if needs_write {
                        // The PTY reader and writer use the same underlying FD in our implementation.
                        // Return the readiness guard so we only clear edge-triggered EPOLLOUT
                        // readiness if `pty_write` actually observes `WouldBlock`.
                        async_pty_reader.writable().await
                    } else {
                        std::future::pending().await
                    }
                };

                tokio::select! {
                    biased; // Prioritize UI messages to keep it snappy

                    msg = self.rx.recv() => {
                        let mut needs_render = false;
                        match msg {
                            Ok(Msg::Input(input)) => {
                                log::trace!("EventLoop: received Msg::Input, len={}", input.len());
                                state.write_list.push_back(input);
                            },
                            Ok(Msg::ReplayOutput(bytes)) => {
                                state.parser.advance(&mut self.terminal, &bytes);
                                needs_render = true;
                            },
                            Ok(Msg::Resize(window_size, resize_grid)) => {
                                // Coalesce resize
                                resize_pending = Some((window_size, resize_grid));
                            },
                            Ok(Msg::Scroll(scroll)) => {
                                if self.terminal.mode().contains(crate::engine::term::TermMode::ALT_SCREEN) {
                                    let is_app_cursor = self.terminal.mode().contains(crate::engine::term::TermMode::APP_CURSOR);
                                    let seq_up = if is_app_cursor { b"\x1bOA" } else { b"\x1b[A" };
                                    let seq_down = if is_app_cursor { b"\x1bOB" } else { b"\x1b[B" };

                                    let lines = match scroll {
                                        Scroll::Delta(d) => d,
                                        Scroll::PageUp => 5,
                                        Scroll::PageDown => -5,
                                        Scroll::Top => 50,
                                        Scroll::Bottom => -50,
                                    };

                                    if lines > 0 {
                                        for _ in 0..lines {
                                            state.write_list.push_back(Cow::Borrowed(seq_up));
                                        }
                                    } else if lines < 0 {
                                        for _ in 0..-lines {
                                            state.write_list.push_back(Cow::Borrowed(seq_down));
                                        }
                                    }
                                } else {
                                    self.terminal.scroll_display(scroll);
                                    needs_render = true;
                                }
                            },
                            Ok(Msg::UpdateSelection(sel)) => {
                                self.terminal.selection = sel;
                                needs_render = true;
                            },
                            Ok(Msg::UpdateSelectionExt(point, side)) => {
                                if let Some(ref mut selection) = self.terminal.selection {
                                    selection.update(point, side);
                                    needs_render = true;
                                }
                            },
                            Ok(Msg::UpdateSelectionViewport(row, col, side)) => {
                                needs_render = self.update_selection_at_viewport(row, col, side);
                            },
                            Ok(Msg::ScrollAndSelect(scroll, row, col, side)) => {
                                if !self.terminal.mode().contains(crate::engine::term::TermMode::ALT_SCREEN) {
                                    self.terminal.scroll_display(scroll);
                                    self.update_selection_at_viewport(row, col, side);
                                    needs_render = true;
                                }
                            },
                            Ok(Msg::ClearSelection) => {
                                self.terminal.selection = None;
                                needs_render = true;
                            },
                            Ok(Msg::CopySelection(clipboard_type)) => {
                                if let Some(text) = self
                                    .terminal
                                    .selection_to_string()
                                    .filter(|s| !s.trim_matches(['\n', '\r']).is_empty())
                                {
                                    self.event_proxy.send_event(Event::ClipboardStore(clipboard_type, text));
                                } else {
                                    log::debug!("EventLoop: Copy requested but selection is empty");
                                }
                            },
                            Ok(Msg::Search(query, direction, case_insensitive)) => {
                                if let Ok(mut searcher) = crate::engine::term::search::RegexSearch::new(&query, case_insensitive) {
                                    let mut origin = self.terminal.grid().cursor.point;
                                    if let Some(ref sel) = self.terminal.selection
                                        && let Some(range) = sel.to_range(&self.terminal) {
                                            origin = if direction == Direction::Right {
                                                range.end.add(&self.terminal, crate::engine::index::Boundary::Grid, 1)
                                            } else {
                                                range.start.sub(&self.terminal, crate::engine::index::Boundary::Grid, 1)
                                            };
                                        }
                                    let mut match_opt = self.terminal.search_next(&mut searcher, origin, direction, Side::Left, None);
                                    if match_opt.is_none() {
                                        use crate::engine::grid::Dimensions;
                                        let wrap_origin = if direction == Direction::Right {
                                            Point::new(self.terminal.topmost_line(), Column(0))
                                        } else {
                                            Point::new(self.terminal.bottommost_line(), self.terminal.last_column())
                                        };
                                        match_opt = self.terminal.search_next(&mut searcher, wrap_origin, direction, Side::Left, None);
                                    }

                                    if let Some(m) = match_opt {
                                        self.terminal.selection = Some(Selection::new(SelectionType::Simple, *m.start(), Side::Left));
                                        if let Some(ref mut sel) = self.terminal.selection {
                                            sel.update(*m.end(), Side::Right);
                                        }
                                        self.terminal.scroll_to_point(*m.start());
                                        needs_render = true;
                                    }
                                }
                            },
                            Ok(Msg::GetTextSnapshot(max_lines, offset_lines, sender)) => {
                                use crate::engine::grid::Dimensions;
                                use crate::engine::index::{Line, Column};

                                let bottom = self.terminal.bottommost_line();
                                let topmost = self.terminal.topmost_line().0;

                                let mut end_line_idx = bottom.0 - offset_lines as i32;
                                if end_line_idx < topmost {
                                    end_line_idx = topmost;
                                }
                                let mut start_line_idx = end_line_idx - max_lines as i32 + 1;

                                if start_line_idx < topmost {
                                    start_line_idx = topmost;
                                }

                                let start_point = Point::new(Line(start_line_idx), Column(0));
                                let end_point = Point::new(Line(end_line_idx), self.terminal.last_column());

                                let text = self.terminal.semantic_bounds_to_string(start_point, end_point);
                                let _ = sender.send(text);
                            },                            Ok(Msg::Shutdown) | Err(_) => {
                                core::hint::cold_path();
                                break 'event_loop;
                            }
                        }

                        if needs_render {
                            self.publish_render_state(
                                RenderPriority::Immediate,
                                &mut last_snapshot_at,
                                &mut pending_coalesced_render,
                            );
                        }
                    }

                    _ = resize_sleep_fut => {
                        if let Some((window_size, resize_grid)) = resize_pending.take() {
                            self.pty.on_resize(window_size);
                            if resize_grid {
                                self.terminal.resize(window_size);
                                self.publish_render_state(
                                    RenderPriority::Immediate,
                                    &mut last_snapshot_at,
                                    &mut pending_coalesced_render,
                                );
                            }
                        }
                    }

                    write_guard = write_fut => {
                        let Ok(mut guard) = write_guard else {
                            break 'event_loop;
                        };
                        // Write to PTY; only clear edge-triggered writable readiness
                        // if the kernel PTY buffer filled (`WouldBlock`).
                        match self.pty_write(&mut state) {
                            Ok(true) => guard.clear_ready(),
                            Ok(false) => {}
                            Err(err) => {
                                error!("Error writing to PTY in event loop: {err}");
                                break 'event_loop;
                            }
                        }
                    }

                    read_guard = async_pty_reader.readable() => {
                        let Ok(mut guard) = read_guard else {
                            break 'event_loop;
                        };
                        match self.pty_read(&mut state, &mut buf, pipe.as_mut()) {
                            Ok((false, _, _)) => {
                                // EOF Reached
                                guard.clear_ready();
                                self.event_proxy.send_event(Event::Exit);
                                self.terminal.exit();
                                self.publish_render_state(
                                    RenderPriority::Immediate,
                                    &mut last_snapshot_at,
                                    &mut pending_coalesced_render,
                                );
                                break 'event_loop;
                            }
                            Ok((true, needs_coalesced_render, would_block)) => {
                                if needs_coalesced_render {
                                    self.publish_render_state(
                                        RenderPriority::Coalesced,
                                        &mut last_snapshot_at,
                                        &mut pending_coalesced_render,
                                    );
                                }
                                // Only clear edge-triggered readable readiness if we actually
                                // drained until WouldBlock; if we paused at MAX_LOCKED_READ to
                                // yield to UI messages, keep readiness armed for the next turn.
                                if would_block {
                                    guard.clear_ready();
                                }
                            }
                            Err(err) => {
                                #[cfg(target_os = "linux")]
                                if err.raw_os_error() == Some(libc::EIO) {
                                    // PTY closed (child exited)
                                    guard.clear_ready();
                                    self.event_proxy.send_event(Event::Exit);
                                    self.terminal.exit();
                                    self.publish_render_state(
                                        RenderPriority::Immediate,
                                        &mut last_snapshot_at,
                                        &mut pending_coalesced_render,
                                    );
                                    break 'event_loop;
                                }
                                error!("Error reading from PTY in event loop: {err}");
                                break 'event_loop;
                            }
                        }
                    }

                    _ = coalesce_sleep_fut => {
                        if pending_coalesced_render {
                            self.publish_render_state(
                                RenderPriority::Immediate,
                                &mut last_snapshot_at,
                                &mut pending_coalesced_render,
                            );
                        }
                    }

                    sig_guard = async {
                        match &async_signals {
                            Some(signals) => signals.readable().await.unwrap(),
                            None => std::future::pending().await,
                        }
                    } => {
                        let mut guard = sig_guard;
                        if let Some(tty::ChildEvent::Exited(status)) = self.pty.next_child_event() {
                            if let Some(status) = status {
                                self.event_proxy.send_event(Event::ChildExit(status));
                            }
                            if self.drain_on_exit {
                                let _ = self.pty_read(&mut state, &mut buf, pipe.as_mut());
                            }
                            self.terminal.exit();
                            self.publish_render_state(
                                RenderPriority::Immediate,
                                &mut last_snapshot_at,
                                &mut pending_coalesced_render,
                            );
                            break 'event_loop;
                        }
                        guard.clear_ready();
                    }

                    _ = sleep_fut => {
                        // Sync timeout reached
                        state.parser.stop_sync(&mut self.terminal);
                        self.publish_render_state(
                            RenderPriority::Immediate,
                            &mut last_snapshot_at,
                            &mut pending_coalesced_render,
                        );
                    }
                }
            }

            drop(async_pty_reader);
            drop(async_signals);
            (self, state)
        })
    }

    #[inline]
    fn update_selection_at_viewport(&mut self, row: usize, col: Column, side: Side) -> bool {
        use crate::engine::grid::Dimensions;
        let display_offset = self.terminal.grid().display_offset() as i32;
        let clamped_row = row.min(self.terminal.screen_lines().saturating_sub(1)) as i32;
        let clamped_col = Column(col.0.min(self.terminal.columns().saturating_sub(1)));
        if let Some(ref mut selection) = self.terminal.selection {
            let point = Point::new(
                crate::engine::index::Line(clamped_row - display_offset),
                clamped_col,
            );
            selection.update(point, side);
            true
        } else {
            false
        }
    }

    #[inline]
    fn swap_updated_render_state(&mut self) {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);

        let new_arc = if let Some(mut spare) = self.spare_render_state.take()
            && let Some(reusable) = Arc::get_mut(&mut spare)
        {
            reusable.update_from(&mut self.terminal, generation);
            spare
        } else {
            let mut fresh = RenderState::default();
            fresh.update_from(&mut self.terminal, generation);
            Arc::new(fresh)
        };

        let previous = self.render_state.swap(new_arc);
        self.spare_render_state = Some(previous);
        self.event_proxy.send_event(Event::Wakeup);
    }

    #[inline]
    fn publish_render_state(
        &mut self,
        priority: RenderPriority,
        last_snapshot_at: &mut Option<tokio::time::Instant>,
        pending_coalesced_render: &mut bool,
    ) {
        let now = tokio::time::Instant::now();
        match priority {
            RenderPriority::Immediate => {
                *pending_coalesced_render = false;
                *last_snapshot_at = Some(now);
                self.swap_updated_render_state();
            }
            RenderPriority::Coalesced => {
                let elapsed_ok = last_snapshot_at
                    .map(|last| now.saturating_duration_since(last) >= MIN_SNAPSHOT_INTERVAL)
                    .unwrap_or(true);
                if elapsed_ok {
                    *pending_coalesced_render = false;
                    *last_snapshot_at = Some(now);
                    self.swap_updated_render_state();
                } else {
                    *pending_coalesced_render = true;
                }
            }
        }
    }

    #[inline]
    fn pty_read<X>(
        &mut self,
        state: &mut State,
        buf: &mut [u8],
        mut writer: Option<&mut X>,
    ) -> io::Result<(bool, bool, bool)>
    where
        X: Write,
    {
        let mut unprocessed = 0;
        let mut processed = 0;
        let mut eof = false;
        let mut would_block = false;

        loop {
            // Read from the PTY.
            match self.pty.reader().read(&mut buf[unprocessed..]) {
                Ok(0) if unprocessed == 0 => {
                    eof = true;
                    break;
                }
                Ok(got) => unprocessed += got,
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => {
                        if unprocessed == 0 {
                            continue;
                        }
                    }
                    ErrorKind::WouldBlock => {
                        would_block = true;
                        if unprocessed == 0 {
                            break;
                        }
                    }
                    _ => return Err(err),
                },
            }

            if let Some(writer) = &mut writer {
                writer.write_all(&buf[..unprocessed]).unwrap();
            }

            state
                .parser
                .advance(&mut self.terminal, &buf[..unprocessed]);

            processed += unprocessed;
            unprocessed = 0;

            if would_block || processed >= MAX_LOCKED_READ {
                break;
            }
        }

        let needs_coalesced_render = state.parser.sync_bytes_count() < processed && processed > 0;
        Ok((!eof, needs_coalesced_render, would_block))
    }

    /// Writes queued input buffers to the PTY.
    /// Returns `Ok(true)` if the write loop stopped due to `ErrorKind::WouldBlock`.
    #[inline]
    fn pty_write(&mut self, state: &mut State) -> io::Result<bool> {
        state.ensure_next();

        'write_many: while let Some(mut current) = state.take_current() {
            if current.finished() {
                state.goto_next();
                continue 'write_many;
            }

            log::trace!(
                "EventLoop: pty_write current chunk len={}",
                current.remaining_bytes().len()
            );
            'write_one: loop {
                match self.pty.writer().write(current.remaining_bytes()) {
                    Ok(0) => {
                        state.set_current(Some(current));
                        return Ok(true);
                    }
                    Ok(n) => {
                        log::trace!(
                            "EventLoop: wrote {} bytes to PTY: {:?}",
                            n,
                            &current.remaining_bytes()[..n]
                        );
                        current.advance(n);
                        if current.finished() {
                            state.goto_next();
                            break 'write_one;
                        }
                    }
                    Err(err) => match err.kind() {
                        ErrorKind::Interrupted => continue 'write_one,
                        ErrorKind::WouldBlock => {
                            state.set_current(Some(current));
                            return Ok(true);
                        }
                        _ => {
                            state.set_current(Some(current));
                            return Err(err);
                        }
                    },
                }
            }
        }

        Ok(false)
    }
}

/// Helper type which tracks how much of a buffer has been written.
struct Writing {
    source: Cow<'static, [u8]>,
    written: usize,
}

pub struct Notifier(pub EventLoopSender);

impl event::Notify for Notifier {
    fn notify<B>(&self, bytes: B) -> std::io::Result<()>
    where
        B: Into<Cow<'static, [u8]>>,
    {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Ok(());
        }

        self.0
            .send(Msg::Input(bytes))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.to_string()))
    }
}

impl event::OnResize for Notifier {
    fn on_resize(&mut self, window_size: WindowSize) {
        let _ = self.0.send(Msg::Resize(window_size, false));
    }
}

#[derive(Debug)]
pub enum EventLoopSendError {
    /// Error sending a message to the event loop.
    Send(async_channel::SendError<Msg>),
}

impl Display for EventLoopSendError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            EventLoopSendError::Send(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for EventLoopSendError {}

#[derive(Clone)]
pub struct EventLoopSender {
    sender: async_channel::Sender<Msg>,
}

impl EventLoopSender {
    pub fn send(&self, msg: Msg) -> Result<(), EventLoopSendError> {
        self.sender.try_send(msg).map_err(|e| match e {
            async_channel::TrySendError::Full(m) | async_channel::TrySendError::Closed(m) => {
                EventLoopSendError::Send(async_channel::SendError(m))
            }
        })
    }

    pub fn close(&self) {
        self.sender.close();
    }
}

/// All of the mutable state needed to run the event loop.
#[derive(Default)]
pub struct State {
    write_list: VecDeque<Cow<'static, [u8]>>,
    writing: Option<Writing>,
    parser: ansi::Processor,
}

impl State {
    #[inline]
    fn ensure_next(&mut self) {
        if self.writing.is_none() {
            self.goto_next();
        }
    }

    #[inline]
    fn goto_next(&mut self) {
        self.writing = self.write_list.pop_front().map(Writing::new);
    }

    #[inline]
    fn take_current(&mut self) -> Option<Writing> {
        self.writing.take()
    }

    #[inline]
    fn needs_write(&self) -> bool {
        self.writing.is_some() || !self.write_list.is_empty()
    }

    #[inline]
    fn set_current(&mut self, new: Option<Writing>) {
        self.writing = new;
    }
}

impl Writing {
    #[inline]
    fn new(c: Cow<'static, [u8]>) -> Writing {
        Writing {
            source: c,
            written: 0,
        }
    }

    #[inline]
    fn advance(&mut self, n: usize) {
        self.written += n;
    }

    #[inline]
    fn remaining_bytes(&self) -> &[u8] {
        &self.source[self.written..]
    }

    #[inline]
    fn finished(&self) -> bool {
        self.written >= self.source.len()
    }
}
