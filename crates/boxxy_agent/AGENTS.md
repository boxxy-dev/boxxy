# Boxxy Agent

## Overview

The `boxxy-agent` is a privileged helper process designed to bridge the gap between the sandboxed Boxxy UI (Flatpak) and the host system. It is modeled after the `ptyxis-agent` architecture.

When running inside a Flatpak, Boxxy cannot directly create PTYs on the host or manage host processes effectively. The agent runs on the host (via `flatpak-spawn --host` or the `org.freedesktop.Flatpak` D-Bus portal) and communicates with the UI over a Unix socket using D-Bus (zbus).

## Why we need it

1.  **PTY Management:** Creating PTYs on the host's `/dev/pts` ensures that they are visible and manageable by host processes, which is not possible from within a sandboxed PTS namespace.
2.  **Job Control:** By creating the session and controlling terminal on the host, we avoid the `tcsetpgrp` / "no job control" issues common with simple `flatpak-spawn --host` wrappers.
3.  **CWD Tracking:** The agent can reliably track the current working directory of child processes by reading `/proc/{pid}/cwd` on the host, even when the UI is sandboxed.
4.  **Foreground Process Tracking:** The agent monitors which process is currently in the foreground of a PTY (e.g., `sudo`, `ssh`, `vim`) by polling the Terminal Process Group ID (`tpgid`). It emits the `foreground_process_changed` D-Bus signal in real-time whenever the interactive application changes, enabling UI/AI modality awareness. To ensure zero overhead when AI is disabled, this tracking loop is strictly opt-in per-PID and can be dynamically paused/resumed by the UI via `set_foreground_tracking()`.
5.  **Sandbox Escape:** It uses the `TIOCGPTPEER` ioctl to safely pass PTY slave file descriptors back to the sandboxed UI over a Unix socket.

## Architecture

-   **Transport:** A private, anonymous Unix domain socket pair (`socketpair()`). One end is retained by the UI, while the remote end is explicitly mapped to **File Descriptor 3** via `dup2` before spawning.
-   **FD Forwarding:** To pass the socket through the Flatpak boundary securely, the UI uses `flatpak-spawn --forward-fd=3`. The agent binary is executed with the argument `--socket-fd=3`, allowing it to reconstruct the D-Bus connection natively without touching the filesystem.
-   **Protocol:** D-Bus (via `zbus`) for structured RPC and PTY file descriptor passing. This P2P D-Bus connection is established directly over the inherited socket, entirely bypassing the public session bus broker.
-   **Lifecycle:** The UI process spawns the agent on the host using the `--watch-bus` flag. The agent continually polls the socket and immediately shuts down if the connection is dropped or the UI process terminates.
-   **Path Discovery:** When running in Flatpak, the UI discovers the host-side path of the agent by reading `app-path` from `/.flatpak-info` and targeting `libexec/boxxy-agent`.

## Implementation Details & Caveats

1.  **The `O_CLOEXEC` Pitfall:** By default, Rust sets the `O_CLOEXEC` (Close-on-Exec) flag on newly created file descriptors like our Unix sockets. When we map the socket to FD 3 using `dup2(fd, 3)`, the flag is typically cleared. However, if the original `fd` happened to already be exactly 3, `dup2` is a no-op and leaves `O_CLOEXEC` intact! We must explicitly clear `O_CLOEXEC` on FD 3 using `fcntl` before calling `flatpak-spawn`; otherwise, the socket is silently closed by the OS upon execution, resulting in an immediate "Broken Pipe".
2.  **Flatpak Environment Handling:** When running inside a Flatpak, `std::env::vars()` in the UI returns the sandboxed environment (e.g., `HOME=/app`, a mangled `PATH`). The UI therefore sends an **empty** `env` vector to the agent in Flatpak mode, rather than leaking sandbox paths to the host shell. The agent detects the empty vector and seeds a baseline host environment from the real user's `/etc/passwd` entry — setting `HOME`, `USER`, `LOGNAME`, and `SHELL` to the correct host values via `User::from_uid(getuid())`. The agent then applies any explicit overrides from `SpawnOptions.env` on top (e.g. `TERM`, `COLORTERM`). Combined with the `login_shell` setting (which appends `--login` to the shell argv), the shell sources its own startup files and builds the full user environment from there. In native (non-Flatpak) builds, `std::env::vars()` already contains the correct session environment, so the UI forwards it verbatim and no passwd lookup is needed.
3.  **CWD Tracking:** Native CWD tracking using `/proc/{pid}/cwd` works well when the agent and shell run locally. However, when the terminal is wrapped in Flatpak and `flatpak-spawn`, relying purely on host PID tracking can be fragile. We use standard OSC 7 escape sequences emitted by modern shells to accurately track the current working directory from the terminal stream itself, falling back to `/proc` polling only when necessary.

## Sandbox Fallback

To ensure the application remains functional across different host distributions (which may have older `glibc` versions incompatible with the Flatpak-built agent), the `AgentManager` implements a fallback mechanism:

1.  **Host Attempt:** Try spawning the agent on the host via `flatpak-spawn --host`.
2.  **Validation:** Attempt to establish a D-Bus P2P connection over the passed socket.
3.  **Fallback:** If the host execution fails (e.g., due to `glibc` mismatch) or the connection times out, the manager spawns a local instance of the agent *inside* the Flatpak sandbox.
4.  **Degraded Mode:** While in fallback mode, features requiring host-privileged access (like native PTYs and host process monitoring) will operate in a limited capacity within the sandbox environment.

## TODO / Future Improvements

-   **Agent Portability:** Investigate static linking (e.g., `musl`) for the `boxxy-agent` binary. This would eliminate `glibc` version mismatch issues when the Flatpak-compiled agent executes on an older host distribution.
-   **Resource Cleanup:** Implement stricter timeout rules and garbage collection for orphaned PTY master FDs if a catastrophic communication failure occurs between the UI and the host agent.
-   **Container Integration:** Similar to Ptyxis, explore extending the agent to interrogate Podman, Toolbox, or Distrobox containers, allowing the terminal to seamlessly spawn shells inside user containers natively.

> **Note:** A more reliable long-term solution is required to ensure the agent can run on the host regardless of `glibc` differences. Potential avenues include static linking of the agent or using a more portable IPC mechanism that doesn't rely on executing a complex Rust binary on the host directly (e.g., a minimal C wrapper or portal-native spawning).

## Responsibilities

-   Allocating PTY master/slave pairs on the host.
-   Spawning shells and other processes on the host.
-   Enforcing a security blacklist for file operations (Claw engine).
-   Forwarding signals (like `SIGWINCH`) to host processes.
-   Monitoring process exit statuses and foreground changes (modality) and emitting D-Bus signals (`exited`, `foreground_process_changed`).
-   Reporting CWD and foreground process information to the UI via `/proc`.

## Public Traits & Structs

### `BoxxyAgent` (struct)
Implements the `dev.boxxy.BoxxyTerminal.Agent` D-Bus interface. Hosted via `zbus`.
Handles incoming method calls like `create_pty`, `spawn`, `get_cwd`, and `signal_process_group`.

### `SpawnOptions` (struct)
Data payload sent by the UI when spawning a process on the host.
- `cwd`: The initial working directory.
- `argv`: The command and arguments.
- `env`: Environment variables to forward from the sandbox to the host.
