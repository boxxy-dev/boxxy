//! PTY session registry: tracks viewer count, persistence flag, and
//! (when detached) a reader task that drains PTY output into a 4 MB
//! ring buffer so a UI can reattach later.
//!
//! Detach is an explicit IPC call; the UI doesn't hand an FD over —
//! the daemon captures its own `dup()` of the master at spawn time
//! and keeps it idle until `detach()` activates it. The ring buffer
//! only fills while the session is detached. If the UI dies without
//! calling `detach()`, the shell blocks once the kernel PTY buffer
//! fills (~64 KB) and won't recover without external `kill`.

use std::collections::{HashMap, VecDeque};
use std::os::unix::io::{AsRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Instant;

use tokio::io::unix::AsyncFd;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// 4 MB per detached session. Roughly one heavy compilation log;
/// callers don't need more scrollback than this since the UI keeps
/// its own terminal scrollback once reattached.
pub const RING_BUFFER_CAPACITY: usize = 4 * 1024 * 1024;

/// Maximum idle time before the sweeper SIGTERMs a detached, persistent
/// session. The timer resets whenever the shell produces output.
pub const DETACHED_TTL: std::time::Duration = std::time::Duration::from_secs(4 * 60 * 60);

/// Sweeper tick interval. Coarse on purpose — the TTL is hours.
pub const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Retention window for exited process records so late-subscribing or
/// reconnecting UI panes can still retrieve a process's exit code.
pub const EXITED_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Read chunk. PTY output is typically bursty (shell prompt, command
/// output); 16 KB keeps syscall count low without wasting memory.
const READ_CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Exited { exit_code: i32, exited_at: Instant },
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// Byte-oriented FIFO that drops the oldest bytes when full. Backed by a
/// `VecDeque<u8>` because our bursts are dominated by short writes; the
/// cost of the wraparound copy on snapshot is irrelevant at 4 MB.
pub struct RingBuffer {
    cap: usize,
    data: VecDeque<u8>,
    evicted_any: bool,
    in_alt_screen: bool,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            data: VecDeque::with_capacity(cap),
            evicted_any: false,
            in_alt_screen: false,
        }
    }

    fn update_alt_screen_state(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i + 5 <= bytes.len() {
            if bytes[i] == 0x1b && bytes[i + 1] == b'[' && bytes[i + 2] == b'?' {
                let tail = &bytes[i + 3..];
                if tail.starts_with(b"1049h")
                    || tail.starts_with(b"1047h")
                    || tail.starts_with(b"47h")
                {
                    self.in_alt_screen = true;
                } else if tail.starts_with(b"1049l")
                    || tail.starts_with(b"1047l")
                    || tail.starts_with(b"47l")
                {
                    self.in_alt_screen = false;
                }
            }
            i += 1;
        }
    }

    /// Appends `bytes`, evicting from the front if it would exceed capacity.
    /// When eviction occurs, aligns the leading edge to a valid UTF-8 boundary
    /// (and to the next newline for buffers >= 64 bytes) so replaying into VTE
    /// never starts mid-codepoint or mid-CSI escape sequence.
    pub fn push(&mut self, bytes: &[u8]) {
        self.update_alt_screen_state(bytes);
        if bytes.len() >= self.cap {
            // Fast path: the new write alone is bigger than the buffer.
            // Keep only the trailing `cap` bytes.
            self.data.clear();
            let start = bytes.len() - self.cap;
            self.data.extend(&bytes[start..]);
            self.evicted_any = true;
            self.align_front_boundary();
            return;
        }
        let overflow = (self.data.len() + bytes.len()).saturating_sub(self.cap);
        if overflow > 0 {
            self.data.drain(..overflow);
            self.evicted_any = true;
        }
        self.data.extend(bytes);
        if overflow > 0 {
            self.align_front_boundary();
        }
    }

    fn align_front_boundary(&mut self) {
        // 1. Always strip leading UTF-8 continuation bytes (`0b10xx_xxxx`).
        let mut utf8_skip = 0;
        for &b in self.data.iter().take(4) {
            if (b & 0b1100_0000) == 0b1000_0000 {
                utf8_skip += 1;
            } else {
                break;
            }
        }
        if utf8_skip > 0 && utf8_skip < self.data.len() {
            self.data.drain(..utf8_skip);
        }

        // 2. For real terminal buffers (cap >= 64), if there is a newline within the
        //    first 256 bytes (and at least half the buffer remains), advance past it
        //    so replay starts on a clean line rather than mid-escape sequence.
        if self.cap >= 64 {
            let max_scan = 256.min(self.data.len() / 2);
            if let Some(nl_pos) = self.data.iter().take(max_scan).position(|&b| b == b'\n') {
                self.data.drain(..=nl_pos);
            }
        }
    }

    /// Returns a contiguous copy, oldest byte first.
    /// For real terminal buffers (`cap >= 64`) that have evicted leading output,
    /// prepends `\x1b[0m` so prior SGR attributes don't bleed into the tail.
    pub fn snapshot(&self) -> Vec<u8> {
        let (a, b) = self.data.as_slices();
        let prefix: &[u8] = if self.evicted_any && self.cap >= 64 && !self.data.is_empty() {
            if self.in_alt_screen {
                b"\x1b[?1049h\x1b[0m"
            } else {
                b"\x1b[0m"
            }
        } else {
            b""
        };
        let mut out = Vec::with_capacity(prefix.len() + a.len() + b.len());
        out.extend_from_slice(prefix);
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        out
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }
}

// ---------------------------------------------------------------------------
// Session record
// ---------------------------------------------------------------------------

pub struct PtySession {
    pub pid: u32,
    /// The pane_id / layout_tag exposed in `list_detached_sessions()` for split grouping.
    pub pane_id: String,
    /// The active terminal pane UUID (`holder_id` in `CharacterRegistry` / `WorkspaceRegistry`).
    pub original_pane_id: String,
    /// Unique D-Bus name (`:1.xxx`) of the UI client currently attached to this PTY.
    /// Used by `client_tracker` to automatically detach (or SIGTERM) on UI exit/crash.
    pub owner_bus_name: String,
    /// Initial working directory supplied at spawn time (fallback if `/proc/<pid>/cwd` fails).
    pub initial_cwd: String,
    pub state: ProcessState,
    pub persistence_enabled: bool,
    pub viewer_count: u32,
    pub last_activity: Instant,
    /// Present only while detached. Filled by `reader_task`.
    pub ring_buffer: Option<RingBuffer>,
    /// Present only while detached. `JoinHandle` aborted on reattach.
    reader_task: Option<JoinHandle<()>>,
    /// The daemon's own dup of the PTY master, captured at spawn time.
    /// Sits idle while the UI is attached (no polling → no races on PTY
    /// output), and is activated into a reader on `detach()`. `reattach()`
    /// hands the UI a fresh `dup()` and keeps this one — so subsequent
    /// detach/reattach cycles still work.
    master_fd: Option<OwnedFd>,
}

fn extract_base_pane_id(pane_id: &str) -> String {
    pane_id.split("::").next().unwrap_or(pane_id).to_string()
}

impl PtySession {
    fn new(
        pid: u32,
        pane_id: String,
        owner_bus_name: String,
        initial_cwd: String,
        persistence_enabled: bool,
        master_fd: Option<OwnedFd>,
    ) -> Self {
        let original_pane_id = extract_base_pane_id(&pane_id);
        Self {
            pid,
            pane_id,
            original_pane_id,
            owner_bus_name,
            initial_cwd,
            state: ProcessState::Running,
            persistence_enabled,
            viewer_count: 1, // the spawning UI is already attached
            last_activity: Instant::now(),
            ring_buffer: None,
            reader_task: None,
            master_fd,
        }
    }

    /// True if the session is currently detached (no UI reading the FD)
    /// AND persistence was requested — i.e. eligible to appear in
    /// `list_detached_sessions()` and to be reattached later.
    pub fn is_detached(&self) -> bool {
        matches!(self.state, ProcessState::Running)
            && self.viewer_count == 0
            && self.persistence_enabled
            && self.reader_task.is_some()
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Central daemon-side registry of live PTY sessions. One entry per shell
/// spawned via `PtySubsystem::spawn`. Cloned into `AgentState` so every
/// D-Bus handler and the zombie sweeper can reach it without threading
/// a reference through each call.
#[derive(Clone)]
pub struct PtyRegistry {
    inner: Arc<RwLock<HashMap<u32, PtySession>>>,
    changed_tx: tokio::sync::broadcast::Sender<()>,
}

impl Default for PtyRegistry {
    fn default() -> Self {
        let (changed_tx, _) = tokio::sync::broadcast::channel(32);
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            changed_tx,
        }
    }
}

pub async fn has_pending_claw_work_for_pane(pane_id: &str) -> bool {
    if pane_id.is_empty() {
        return false;
    }
    let workspace = boxxy_claw::registry::workspace::global_workspace().await;
    if workspace.get_pane_pending_task_count(pane_id).await > 0 {
        return true;
    }
    let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
    claims
        .iter()
        .any(|c| c.holder_id == pane_id && c.owner_bus_name.is_empty())
}

impl PtyRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.changed_tx.subscribe()
    }

    pub fn notify_changed(&self) {
        let _ = self.changed_tx.send(());
    }

    /// Inserts a fresh session record. Called from `spawn()` after the
    /// child successfully starts. `master_fd` is the daemon's dup of the
    /// PTY master and is kept idle until `detach()` starts reading it.
    pub async fn register(&self, pid: u32, pane_id: String, master_fd: Option<OwnedFd>) {
        self.register_with_owner(pid, pane_id, String::new(), String::new(), false, master_fd)
            .await;
    }

    pub async fn register_with_owner(
        &self,
        pid: u32,
        pane_id: String,
        owner_bus_name: String,
        initial_cwd: String,
        persistence_enabled: bool,
        master_fd: Option<OwnedFd>,
    ) {
        let now = Instant::now();
        let mut map = self.inner.write().await;
        map.retain(|_, s| match s.state {
            ProcessState::Running => true,
            ProcessState::Exited { exited_at, .. } => {
                now.saturating_duration_since(exited_at) <= EXITED_TTL
            }
        });
        map.insert(
            pid,
            PtySession::new(
                pid,
                pane_id,
                owner_bus_name,
                initial_cwd,
                persistence_enabled,
                master_fd,
            ),
        );
    }

    /// Transitions a session to `ProcessState::Exited`, immediately releasing
    /// its master FD and ring buffer while retaining the exit code for
    /// `EXITED_TTL` so late-subscribing or reconnecting panes can query it.
    pub async fn mark_exited(&self, pid: u32, exit_code: i32) {
        let now = Instant::now();
        let reader_handle = {
            let mut map = self.inner.write().await;
            map.retain(|_, s| match s.state {
                ProcessState::Running => true,
                ProcessState::Exited { exited_at, .. } => {
                    now.saturating_duration_since(exited_at) <= EXITED_TTL
                }
            });
            if let Some(s) = map.get_mut(&pid) {
                s.state = ProcessState::Exited {
                    exit_code,
                    exited_at: now,
                };
                s.master_fd = None;
                s.ring_buffer = None;
                s.reader_task.take()
            } else {
                None
            }
        };
        if let Some(handle) = reader_handle {
            handle.abort();
        }
        self.notify_changed();
    }

    /// Returns `Some(exit_code)` if the process is known to have exited within
    /// the retention window, or `None` if it is still running / unknown.
    pub async fn get_exit_status(&self, pid: u32) -> Option<i32> {
        let map = self.inner.read().await;
        match map.get(&pid)?.state {
            ProcessState::Exited { exit_code, .. } => Some(exit_code),
            ProcessState::Running => None,
        }
    }

    /// Called when explicitly removing a session record from the registry.
    /// Aborts any live reader so its FD drops cleanly.
    pub async fn remove(&self, pid: u32) {
        let session = { self.inner.write().await.remove(&pid) };
        if let Some(mut s) = session {
            if let Some(handle) = s.reader_task.take() {
                handle.abort();
            }
            // `master_fd` drops here, closing the daemon's copy.
            self.notify_changed();
        }
    }

    pub async fn set_persistence(&self, pid: u32, enabled: bool) {
        if let Some(s) = self.inner.write().await.get_mut(&pid) {
            s.persistence_enabled = enabled;
        }
    }

    /// Updates `persistence_enabled` on all currently attached, running PTY sessions
    /// so toggling `Persistent Shells` in Preferences takes effect immediately in the
    /// daemon even if the UI later exits or crashes before per-pane drop futures run.
    pub async fn sync_default_persistence(&self, enabled: bool) {
        let mut map = self.inner.write().await;
        for s in map.values_mut() {
            if matches!(s.state, ProcessState::Running) && s.viewer_count > 0 {
                s.persistence_enabled = enabled;
            }
        }
    }

    pub async fn set_owner(&self, pid: u32, owner_bus_name: String) {
        if let Some(s) = self.inner.write().await.get_mut(&pid) {
            s.owner_bus_name = owner_bus_name;
        }
    }

    pub async fn set_pane_id(&self, pid: u32, pane_id: String) {
        if let Some(s) = self.inner.write().await.get_mut(&pid) {
            if !pane_id.contains("::") {
                s.original_pane_id = pane_id.clone();
            }
            s.pane_id = pane_id;
        }
    }

    pub async fn set_original_pane_id(&self, pid: u32, original_pane_id: String) {
        if let Some(s) = self.inner.write().await.get_mut(&pid) {
            s.original_pane_id = original_pane_id;
        }
    }

    pub async fn get_original_pane_id(&self, pid: u32) -> Option<String> {
        self.inner
            .read()
            .await
            .get(&pid)
            .map(|s| s.original_pane_id.clone())
    }

    pub async fn find_pid_by_pane_id(&self, pane_id: &str) -> Option<(u32, bool)> {
        let map = self.inner.read().await;
        for s in map.values() {
            if matches!(s.state, ProcessState::Running)
                && (s.original_pane_id == pane_id || s.pane_id == pane_id)
            {
                return Some((s.pid, s.is_detached()));
            }
        }
        None
    }

    /// Called by `client_tracker` when a UI process disconnects or crashes.
    /// Automatically detaches (or SIGTERMs if persistence is off) every still-attached
    /// PTY session owned by `owner_bus_name`.
    pub async fn release_owner(&self, owner_bus_name: &str) -> Vec<(u32, DetachOutcome)> {
        if owner_bus_name.is_empty() {
            return Vec::new();
        }
        let pids: Vec<u32> = {
            let mut map = self.inner.write().await;
            let mut found = Vec::new();
            for s in map.values_mut() {
                if matches!(s.state, ProcessState::Running)
                    && s.viewer_count > 0
                    && s.owner_bus_name == owner_bus_name
                {
                    s.owner_bus_name.clear();
                    found.push(s.pid);
                }
            }
            found
        };

        let mut outcomes = Vec::with_capacity(pids.len());
        for pid in pids {
            let outcome = self.detach(pid).await;
            outcomes.push((pid, outcome));
        }
        outcomes
    }

    /// Increments the viewer count. If the session was detached,
    /// stops the reader task, snapshots the ring buffer, and hands
    /// the UI a **fresh `dup()`** of the master FD — the daemon keeps
    /// its own copy so a later detach/reattach cycle still works.
    /// Returns `None` if `pid` is unknown or the session isn't detached.
    pub async fn reattach(&self, pid: u32) -> Option<(Vec<u8>, OwnedFd)> {
        let mut map = self.inner.write().await;
        let s = map.get_mut(&pid)?;
        if !matches!(s.state, ProcessState::Running) {
            return None;
        }

        s.viewer_count = s.viewer_count.saturating_add(1);
        s.last_activity = Instant::now();

        // If no reader task ran, this session was never actively
        // detached. Caller still has its own FD; nothing to return.
        let Some(handle) = s.reader_task.take() else {
            return None;
        };
        handle.abort();
        let buffer = s
            .ring_buffer
            .take()
            .map(|b| b.snapshot())
            .unwrap_or_default();

        // dup the stored FD and hand the dup to the UI. Our original
        // stays parked in the session, idle, ready for the next detach.
        let stored = s.master_fd.as_ref()?;
        let dup_raw = unsafe { libc::dup(stored.as_raw_fd()) };
        if dup_raw < 0 {
            log::warn!("reattach(pid={}): dup failed", pid);
            return None;
        }
        let fresh = unsafe { OwnedFd::from_raw_fd(dup_raw) };
        drop(map);
        self.notify_changed();
        Some((buffer, fresh))
    }

    /// Decrements the viewer count. When it reaches zero:
    ///   - with persistence disabled → SIGTERM the process group.
    ///   - with persistence enabled → activate the stored master FD and
    ///     spawn a reader task that drains output into the ring buffer.
    ///
    /// Returns `DetachOutcome::Terminated` or `DetachOutcome::Detached`
    /// so callers (and telemetry) can observe the decision.
    pub async fn detach(&self, pid: u32) -> DetachOutcome {
        self.detach_with_snapshot(pid, &[]).await
    }

    pub async fn detach_with_snapshot(&self, pid: u32, initial_snapshot: &[u8]) -> DetachOutcome {
        let orig_pane_id = {
            let map = self.inner.read().await;
            map.get(&pid)
                .map(|s| s.original_pane_id.clone())
                .unwrap_or_default()
        };
        let has_active_work = super::shell_has_active_process(pid)
            || has_pending_claw_work_for_pane(&orig_pane_id).await;
        let (start_reader, should_kill) = {
            let mut map = self.inner.write().await;
            let Some(s) = map.get_mut(&pid) else {
                return DetachOutcome::Unknown;
            };
            if !matches!(s.state, ProcessState::Running) {
                return DetachOutcome::Unknown;
            }

            s.viewer_count = s.viewer_count.saturating_sub(1);
            if s.viewer_count > 0 {
                return DetachOutcome::StillViewed;
            }

            if !s.persistence_enabled || !has_active_work {
                // Either persistence is disabled, or the shell is sitting idle
                // at a prompt with no running child processes — terminate it
                // cleanly instead of keeping an empty shell alive in the daemon.
                s.persistence_enabled = false;
                (false, true)
            } else if s.master_fd.is_some() {
                let mut rb = RingBuffer::new(RING_BUFFER_CAPACITY);
                if !initial_snapshot.is_empty() {
                    rb.push(initial_snapshot);
                }
                s.ring_buffer = Some(rb);
                s.last_activity = Instant::now();
                (true, false)
            } else {
                // No FD on record — session was registered without one
                // (e.g. a test, or a future code path). Nothing to drain.
                log::warn!(
                    "detach(pid={}): persistence_enabled but session has no FD — shell will block when kernel PTY buffer fills",
                    pid
                );
                return DetachOutcome::DetachedUnbuffered;
            }
        };

        // Spawn the reader outside the lock so its own writes to the map
        // don't deadlock.
        if start_reader {
            let registry = self.clone();
            let handle = tokio::spawn(async move {
                run_reader(registry, pid).await;
            });
            if let Some(s) = self.inner.write().await.get_mut(&pid) {
                s.reader_task = Some(handle);
            }
            self.notify_changed();
            return DetachOutcome::Detached;
        }

        if should_kill {
            // SIGTERM the process group; child-waiter will remove() us on
            // SIGCHLD. The UI's FD copy is closed independently when the
            // pane widget is dropped.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            return DetachOutcome::Terminated;
        }

        DetachOutcome::Unknown
    }

    /// Snapshots all currently-detached sessions for a future
    /// "Detached" UI view. Returns `(pid, pane_id, idle_secs)`.
    /// Foreground-process lookup is cheap enough to do here, but we
    /// keep the IPC stable by stuffing it in a separate call — see
    /// `PtySubsystem::list_detached_sessions`.
    pub async fn list_detached(&self) -> Vec<(u32, String, u64)> {
        let now = Instant::now();
        self.inner
            .read()
            .await
            .values()
            .filter(|s| s.is_detached())
            .map(|s| {
                (
                    s.pid,
                    s.pane_id.clone(),
                    now.saturating_duration_since(s.last_activity).as_secs(),
                )
            })
            .collect()
    }

    /// Returns `(pid, pane_id, original_pane_id, idle_secs, initial_cwd)` for all detached sessions.
    pub async fn list_detached_with_cwd(&self) -> Vec<(u32, String, String, u64, String)> {
        let now = Instant::now();
        self.inner
            .read()
            .await
            .values()
            .filter(|s| s.is_detached())
            .map(|s| {
                (
                    s.pid,
                    s.pane_id.clone(),
                    s.original_pane_id.clone(),
                    now.saturating_duration_since(s.last_activity).as_secs(),
                    s.initial_cwd.clone(),
                )
            })
            .collect()
    }

    /// Periodic cleanup: SIGTERMs detached sessions idle longer than
    /// `DETACHED_TTL` and prunes exited session records older than `EXITED_TTL`.
    pub async fn sweep_zombies(&self) {
        let now = Instant::now();
        let to_kill: Vec<u32> = {
            let mut map = self.inner.write().await;
            map.retain(|_, s| match s.state {
                ProcessState::Running => true,
                ProcessState::Exited { exited_at, .. } => {
                    now.saturating_duration_since(exited_at) <= EXITED_TTL
                }
            });
            map.values()
                .filter(|s| s.is_detached() && now.duration_since(s.last_activity) > DETACHED_TTL)
                .map(|s| s.pid)
                .collect()
        };

        for pid in to_kill {
            log::info!("pty-registry: SIGTERM zombie detached session pid={}", pid);
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            // mark_exited() is driven by the child-waiter when SIGCHLD fires.
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    /// pid was not in the registry.
    Unknown,
    /// A viewer detached but others are still attached.
    StillViewed,
    /// Last viewer, persistence disabled → SIGTERM sent.
    Terminated,
    /// Last viewer, persistence enabled, FD captured → reader task
    /// started, output now flowing into the ring buffer.
    Detached,
    /// Persistence enabled but caller didn't hand over an FD; see the
    /// warning in `detach()`. Session stays alive but unbuffered.
    DetachedUnbuffered,
}

// ---------------------------------------------------------------------------
// Reader task
// ---------------------------------------------------------------------------

/// Drains the master FD into the session's ring buffer until the shell
/// closes it (EOF), the task is aborted (reattach or remove), or the
/// session disappears from the registry.
async fn run_reader(registry: PtyRegistry, pid: u32) {
    // Take a raw-fd view of the master without consuming the OwnedFd
    // stored in the session. AsyncFd needs to own its backing file, so
    // we dup the fd, wrap the dup in AsyncFd, and read through that.
    // The session's OwnedFd stays put for reattach().
    let raw = {
        let map = registry.inner.read().await;
        match map.get(&pid).and_then(|s| s.master_fd.as_ref()) {
            Some(fd) => unsafe { libc::dup(fd.as_raw_fd()) },
            None => return,
        }
    };
    if raw < 0 {
        log::warn!("run_reader(pid={}): dup failed", pid);
        return;
    }

    // Put our dup in non-blocking mode so AsyncFd works correctly.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let owned = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: `owned` is an `OwnedFd` whose descriptor stays open and unchanged
    // for the entire lifetime of `async_fd`.
    let async_fd = match unsafe { AsyncFd::register(owned) } {
        Ok(f) => f,
        Err(e) => {
            log::warn!("run_reader(pid={}): AsyncFd::register failed: {}", pid, e);
            return;
        }
    };

    let mut buf = [0u8; READ_CHUNK];
    loop {
        let mut ready = match async_fd.readable().await {
            Ok(g) => g,
            Err(_) => break,
        };

        let read_res = ready.try_io(|inner| {
            let fd = inner.get_ref().as_raw_fd();
            loop {
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                if n < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(err);
                }
                return Ok(n as usize);
            }
        });

        match read_res {
            Ok(Ok(0)) => break, // EOF: shell closed the PTY.
            Ok(Ok(n)) => {
                let mut map = registry.inner.write().await;
                let Some(s) = map.get_mut(&pid) else { break };
                if let Some(rb) = s.ring_buffer.as_mut() {
                    rb.push(&buf[..n]);
                }
                s.last_activity = Instant::now();
            }
            Ok(Err(_e)) => {
                // Real I/O error (PTY gone, etc.). Bail — next reattach
                // will just see an empty buffer and fail to get an FD.
                break;
            }
            Err(_would_block) => continue,
        }
    }

    // Our dup drops with AsyncFd. The session's original FD stays live
    // until reattach() or remove() claims it.
}

// ---------------------------------------------------------------------------
// Standard imports needed above
// ---------------------------------------------------------------------------

use std::os::unix::io::FromRawFd;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest() {
        let mut rb = RingBuffer::new(4);
        rb.push(b"abc");
        assert_eq!(rb.snapshot(), b"abc");
        rb.push(b"de");
        // Over capacity: 'a' should have been dropped.
        assert_eq!(rb.snapshot(), b"bcde");
    }

    #[test]
    fn ring_buffer_write_bigger_than_capacity() {
        let mut rb = RingBuffer::new(4);
        rb.push(b"abcdefgh");
        // Only the trailing `cap` bytes are retained.
        assert_eq!(rb.snapshot(), b"efgh");
    }

    #[test]
    fn ring_buffer_empty_snapshot() {
        let rb = RingBuffer::new(16);
        assert!(rb.snapshot().is_empty());
    }

    #[tokio::test]
    async fn registry_register_and_remove() {
        let reg = PtyRegistry::new();
        reg.register(42, "pane-a".into(), None).await;
        assert_eq!(reg.list_detached().await.len(), 0);
        reg.remove(42).await;
    }

    #[tokio::test]
    async fn registry_detach_without_persistence_is_terminated_outcome() {
        let reg = PtyRegistry::new();
        reg.register(99999, "pane-x".into(), None).await;
        // Persistence off → should attempt SIGTERM. libc::kill(-99999,
        // SIGTERM) is a no-op on ESRCH and doesn't panic. The outcome
        // is what we assert.
        let out = reg.detach(99999).await;
        assert_eq!(out, DetachOutcome::Terminated);
    }

    #[tokio::test]
    async fn registry_detach_persistence_without_fd_logs_unbuffered() {
        let reg = PtyRegistry::new();
        reg.register(77777, "pane-y".into(), None).await;
        reg.set_persistence(77777, true).await;
        let out = reg.detach(77777).await;
        assert_eq!(out, DetachOutcome::DetachedUnbuffered);
    }

    // ---- End-to-end persistence test with a real PTY ----------------
    //
    // Opens a PTY pair directly (no child process), hands the master
    // side to the registry as if we had detached, writes bytes into
    // the slave side (simulating shell output), then reattaches and
    // verifies the bytes flowed through the reader task into the ring
    // buffer. Exercises the full detach/reattach path in under half a
    // second without spawning a real shell.
    #[tokio::test]
    async fn persistence_captures_slave_output_into_ring_buffer() {
        use nix::fcntl::OFlag;
        use nix::pty::{grantpt, posix_openpt, ptsname, unlockpt};
        use std::os::fd::{FromRawFd, IntoRawFd};
        use std::time::Duration;

        // 1. Open a PTY master/slave pair. O_NONBLOCK on master so the
        //    reader's AsyncFd works without stalling.
        let master = posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_NONBLOCK)
            .expect("posix_openpt");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let slave_path = unsafe { ptsname(&master).expect("ptsname") };
        let slave_fd = unsafe {
            libc::open(
                std::ffi::CString::new(slave_path).unwrap().as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY,
            )
        };
        assert!(slave_fd >= 0, "open slave pty failed");

        let master_owned = unsafe { OwnedFd::from_raw_fd(master.into_raw_fd()) };

        // 2. Register *with the FD already stored*, mirroring what
        //    `PtySubsystem::spawn` does for a real shell.
        let reg = PtyRegistry::new();
        let fake_pid = 31337u32;
        reg.register(fake_pid, "pane-test".into(), Some(master_owned))
            .await;
        reg.set_persistence(fake_pid, true).await;

        // 3. Detach — activates the reader over the stored FD. No FD
        //    is handed in; the registry already owns one.
        let outcome = reg.detach(fake_pid).await;
        assert_eq!(outcome, DetachOutcome::Detached);

        // 4. Write into the slave (as a shell would) and give the
        //    reader task a tick to drain into the ring buffer. The PTY
        //    line discipline translates `\n` → `\r\n` on output, so
        //    `marker` below is what we expect to see, not `payload`.
        let payload = b"captured-while-detached\n";
        let marker: &[u8] = b"captured-while-detached";
        let written = unsafe { libc::write(slave_fd, payload.as_ptr() as *const _, payload.len()) };
        assert_eq!(written, payload.len() as isize);

        // Tokio needs a real wait here; the reader is a separate task.
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            if reg
                .inner
                .read()
                .await
                .get(&fake_pid)
                .and_then(|s| s.ring_buffer.as_ref().map(|b| b.len()))
                .unwrap_or(0)
                >= marker.len()
            {
                break;
            }
        }

        // 5. Reattach: should return the buffered bytes plus the FD.
        let (bytes, _returned_fd) = reg
            .reattach(fake_pid)
            .await
            .expect("reattach should return buffer + fd");
        assert!(
            bytes.windows(marker.len()).any(|w| w == marker),
            "ring buffer did not contain marker; got {:?}",
            String::from_utf8_lossy(&bytes)
        );

        // Cleanup
        unsafe { libc::close(slave_fd) };
    }

    #[tokio::test]
    async fn mark_exited_retains_exit_code_and_blocks_detach() {
        let reg = PtyRegistry::new();
        reg.register(555, "pane-exit".into(), None).await;
        assert_eq!(reg.get_exit_status(555).await, None);

        reg.mark_exited(555, 127).await;
        assert_eq!(reg.get_exit_status(555).await, Some(127));
        assert_eq!(reg.detach(555).await, DetachOutcome::Unknown);

        reg.remove(555).await;
        assert_eq!(reg.get_exit_status(555).await, None);
    }

    #[test]
    fn ring_buffer_eviction_never_splits_multibyte_utf8() {
        // "🦀" is 4 bytes: [0xF0, 0x9F, 0xA6, 0x80]
        let mut rb = RingBuffer::new(6);
        rb.push("🦀ab".as_bytes());
        // Pushing 2 more bytes ("cd") causes 2 bytes of "🦀" to be evicted,
        // leaving 2 orphaned UTF-8 continuation bytes at the front unless aligned.
        rb.push(b"cd");
        let snap = rb.snapshot();
        assert!(
            std::str::from_utf8(&snap).is_ok(),
            "snapshot must be valid UTF-8, got {snap:?}"
        );
        assert_eq!(&snap, b"abcd");
    }

    #[tokio::test]
    async fn release_owner_auto_detaches_on_client_disconnect() {
        use nix::fcntl::OFlag;
        use nix::pty::{grantpt, posix_openpt, unlockpt};
        use std::os::fd::{FromRawFd, IntoRawFd};

        let master = posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_NONBLOCK)
            .expect("posix_openpt");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let master_owned = unsafe { OwnedFd::from_raw_fd(master.into_raw_fd()) };

        let reg = PtyRegistry::new();
        reg.register_with_owner(
            4242,
            "pane-crash-test".into(),
            ":1.99".into(),
            "/tmp".into(),
            true,
            Some(master_owned),
        )
        .await;

        let outcomes = reg.release_owner(":1.99").await;
        assert_eq!(outcomes, vec![(4242, DetachOutcome::Detached)]);
        assert_eq!(reg.list_detached().await.len(), 1);

        reg.remove(4242).await;
    }
}
