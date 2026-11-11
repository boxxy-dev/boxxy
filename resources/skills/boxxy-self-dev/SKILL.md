---
name: Boxxy Self-Dev & Bug Reporter
description: Investigate Boxxy bugs in source, file high-signal GitHub bug reports, or install GTK4/Rust dependencies, run isolated --dev builds, and open pull requests directly from Boxxy.
triggers:
  - boxxy bug
  - report bug
  - bug report
  - boxxy issue
  - fix boxxy
  - boxxy source
  - boxxy pr
  - contribute to boxxy
  - self-dev
pinned: false
---
# Boxxy Self-Dev & Bug Reporter Instructions

When the user spots a bug in Boxxy, wants to investigate how a Boxxy feature works internally, or wants to fix an issue and open a Pull Request on `https://github.com/boxxy-dev/boxxy` directly from Boxxy:

## 1. Choose the Right Track First
Ask or infer whether the user wants:
1. **Track A — Fast Source-Backed Bug Report (No compiler needed):** Shallow-clone Boxxy's source (~2s), pinpoint the exact root cause in Rust/UI files, collect sanitized system info, and draft/file a maintainer-ready GitHub issue.
2. **Track B — Fix, Build, Verify in `--dev` Mode, & Open PR:** Check/install Rust + GTK4 build dependencies, clone into a workspace folder, implement + test the fix, launch a side-by-side `boxxy-dev` window to verify live without touching Stable Boxxy, and open a PR.

---

## 2. Shallow-Cloning Boxxy's Source Code
Never clone full Git history unless needed. First check if the current working directory is already a `boxxy` checkout (`Cargo.toml` containing `boxxy-window` and `boxxy-agent`). Otherwise:
- **For Track A (read-only investigation):** Shallow-clone into `~/.cache/boxxy-terminal/self-dev/boxxy-src` (or refresh if already present):
  ```bash
  mkdir -p ~/.cache/boxxy-terminal/self-dev
  git clone --depth 1 https://github.com/boxxy-dev/boxxy.git ~/.cache/boxxy-terminal/self-dev/boxxy-src
  ```
  *(If already cloned: `git -C ~/.cache/boxxy-terminal/self-dev/boxxy-src fetch --depth 1 origin main && git -C ~/.cache/boxxy-terminal/self-dev/boxxy-src reset --hard origin/main`)*
- **For Track B (editing & PRs):** Ask the user where to place the checkout (e.g. `~/Projects/boxxy`), shallow-clone with `--depth 50` so branching and rebasing are fast, and create a dedicated branch (`git switch -c fix/<short-slug>`):
  ```bash
  git clone --depth 50 https://github.com/boxxy-dev/boxxy.git ~/Projects/boxxy
  ```
- **Always read `AGENTS.md` first:** Read the root `AGENTS.md` and the target crate's `crates/<crate>/AGENTS.md` via `file_read` before diagnosing or editing code.

---

## 3. Track A — Submitting a Maintainer-Grade Bug Report
1. **Locate the Root Cause:** Use `grep_search`, `glob_search`, and `file_read` in the shallow clone to trace the exact crate, file, and line numbers involved.
2. **Gather Safe Diagnostics:** Collect OS/distro, desktop session (`$XDG_CURRENT_DESKTOP`, Wayland vs X11), Boxxy version (`boxxy-agent status` or `boxxy-terminal --version`), and reproduction steps.
3. **Strict Privacy & Redaction Guard (CRITICAL):**
   - **NEVER** include API keys, `settings.json`, tokens, personal usernames (replace `/home/<user>` with `~`), private file paths, or unrelated terminal scrollback in any issue body.
4. **Review Before Publishing:** Present the full Markdown issue (`## Summary`, `## Steps to Reproduce`, `## Root Cause Analysis` with file/line references, `## Environment`) to the user first.
   - Only after explicit user approval: if GitHub CLI is authenticated (`gh auth status`), publish with `gh issue create --repo boxxy-dev/boxxy --title "..." --body-file ...`. Otherwise copy the Markdown to clipboard (`set_clipboard`) and give the user `https://github.com/boxxy-dev/boxxy/issues/new`.

---

## 4. Track B — Installing Linux GTK4 & Rust Build Dependencies
Before compiling Boxxy, check what is already installed (`rustc --version`, `pkg-config --modversion gtk4 libadwaita-1 gtksourceview-5 gstreamer-1.0`). Boxxy requires **Rust 1.98+** (Edition 2024), **GTK 4.22+**, **Libadwaita 1.9+**, **GtkSourceView 5**, **GStreamer 1.0**, `pkg-config`, and a C toolchain.

If dependencies are missing, detect the user's Linux distribution (`/etc/os-release`) and propose the matching command for user approval:

- **Fedora / RHEL / Ultramarine:**
  ```bash
  sudo dnf install gcc gcc-c++ make pkgconf-pkg-config git curl \
    gtk4-devel libadwaita-devel gtksourceview5-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel gdk-pixbuf2-devel \
    glib2-devel sqlite-devel
  ```
- **Ubuntu / Debian / Pop!_OS / Linux Mint:**
  ```bash
  sudo apt update && sudo apt install build-essential pkg-config git curl \
    libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    libgdk-pixbuf-2.0-dev libglib2.0-dev libglib2.0-dev-bin libsqlite3-dev
  ```
- **Arch Linux / CachyOS / EndeavourOS / Manjaro:**
  ```bash
  sudo pacman -S --needed base-devel pkgconf git curl \
    gtk4 libadwaita gtksourceview5 gstreamer gst-plugins-base \
    gdk-pixbuf2 glib2 sqlite
  ```
- **openSUSE Tumbleweed:**
  ```bash
  sudo zypper install gcc gcc-c++ make pkg-config git curl \
    gtk4-devel libadwaita-devel gtksourceview5-devel \
    gstreamer-devel gstreamer-plugins-base-devel gdk-pixbuf-devel \
    glib2-devel sqlite3-devel
  ```
- **Nix / NixOS:** Run `nix develop` inside the cloned repository (`flake.nix` provides the complete Rust + GTK4 + GStreamer shell automatically).
- **Rust Toolchain (`rustup` — Rust 1.98+):**
  If `cargo` is missing or `rustc` is older than `1.98` (distro `rust` packages are often outdated—prefer `rustup`):
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  . "$HOME/.cargo/env"
  rustup update stable
  ```
- **GitHub CLI (optional, for fork & PR workflow):** Install `gh` (`dnf install gh` / `apt install gh` / `pacman -S github-cli`) and run `gh auth login` interactively if the user wants to open PRs from the terminal.

---

## 5. Implementing, Testing, & Running Side-by-Side (`--dev` Mode)
1. **Follow Boxxy Coding Standards (`AGENTS.md`):**
   - Use strictly synthetic, generic fixtures in tests (`tempfile::tempdir()`, synthetic strings)—**never** hardcode local `/home/...` paths or user session data.
   - Cover all `boxxy-db` changes with `Db::new_in_memory()` unit tests.
   - Avoid `!important` in CSS (use higher selector specificity) and avoid raw `&` in `.ui` XML labels (use `and`).
   - Update crate-level `AGENTS.md` files if architectural behavior changes.
2. **Verify with Targeted Cargo Checks First:**
   ```bash
   cargo test -p <modified-crate>
   cargo clippy -p <modified-crate> --no-deps --all-targets
   ```
3. **Launch Isolated `boxxy-dev` Alongside Stable Boxxy:**
    NEVER run plain `cargo run -p boxxy-app` or `cargo run -p boxxy-agent` without `--dev` (plain runs collide with or stop the user's running Stable Boxxy!). Always use the workspace `--dev` aliases, ideally in two companion split panes (`spawn_agent(location="vertical")`):
   - **With user's seeded settings (`~/.config/boxxy-terminal-dev/`, separate SQLite DB):**
     ```bash
     # Pane 1 (isolated dev daemon + optional context logs):
     BOXXY_DEBUG_CONTEXT=1 cargo dev-agent

     # Pane 2 (isolated dev window — shows warning `dev` tag in the headerbar):
     cargo dev-app
     ```
     *(Use `cargo dev-sync` to refresh settings from Stable, or `--instance <slug>` when testing multiple branches in parallel.)*
   - **With clean factory defaults (fresh ephemeral config):**
     ```bash
     cargo dev-ephemeral
     ```

---

## 6. Opening a Pull Request on `boxxy-dev/boxxy`
1. **Never commit, push, or publish without explicit user approval.**
2. Show the user `git status`, `git diff --stat`, a clear summary of the fix, and the passing `cargo test` output.
3. Once the user explicitly approves opening a PR:
   - Stage only relevant files and write a clear commit message on the `fix/<slug>` branch.
   - Fork and push via `gh repo fork boxxy-dev/boxxy --remote` (if no fork remote exists yet), push the branch to the user's fork, and run `gh pr create --repo boxxy-dev/boxxy` linking any related issue (`Fixes #<number>`).
