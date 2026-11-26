# Boxxy

**A self-improving Linux terminal powered by AI characters.**

![Boxxy](https://i.imgur.com/Z0hVqaZ.png)

Boxxy is a full terminal emulator with an agentic AI layer — **BoxxyClaw** — built directly into it. Unlike chat-window add-ons, BoxxyClaw has eyes on your shell, reads your terminal buffer, remembers your preferences, and can autonomously fix broken dependencies, manage system configs, and run your scripts. Just press `Ctrl+/` and you're in.

---

## Install

**Native** *(Requires GTK 4.24+)*
```bash
curl -sSf https://raw.githubusercontent.com/boxxy-dev/boxxy/main/scripts/install.sh | sh
```

**Flatpak**
```bash
curl -O https://boxxy-dev.github.io/boxxy-flatpak-remote/boxxy.gpg && \
  flatpak remote-add --user --if-not-exists --gpg-import=boxxy.gpg \
  boxxy https://boxxy-dev.github.io/boxxy-flatpak-remote/repo && \
  flatpak install --user boxxy dev.boxxy.BoxxyTerminal
```

**NixOS** *(Nix Flake)*
```bash
nix profile install github:boxxy-dev/boxxy
```
<sub>Try without installing: <code>nix run github:boxxy-dev/boxxy</code> · Flake input: <code>github:boxxy-dev/boxxy</code></sub>

<sub>Desktop panel companions: <a href="https://github.com/boxxy-dev/gnome-shell-boxxy-companion">GNOME Shell Extension ↗</a> · <a href="https://github.com/boxxy-dev/cosmic-boxxy-companion">COSMIC Desktop Applet ↗</a></sub>

---

## Architecture

Boxxy splits a GPU-rendered GTK4/Libadwaita frontend from a host-privileged D-Bus daemon so running shells, character sessions, and reminders survive window closes and Flatpak sandboxing.

| Crate | Role |
| :--- | :--- |
| `boxxy-app` / `boxxy-window` | GTK4 & Libadwaita UI, tabs, splits, frosted glass, and custom theme engine |
| `boxxy-vte` | Lock-free pure-Rust terminal engine with GSK rendering, Kittygraphics, and OSC 7/8/133 |
| `boxxy-agent` | Host D-Bus daemon managing persistent PTY ring buffers, character claims, and Dream Cycles |
| `boxxy-claw` | Per-pane actor engine for characters, FTS5 memory recall, skills, swarm tools, and approvals |
| `boxxy-ai-core` / `boxxy-mcp` | Unified multi-provider LLM & OAuth layer (`rig`) plus lazy-boot Stdio/HTTP MCP client |

---

## Documentation

Full docs at **[boxxy.dev](https://boxxy.dev)**

- [Getting Started](https://boxxy.dev/getting-started)
- [How It Works](https://boxxy.dev/how-it-works)
- [Supported Providers](https://boxxy.dev/providers)
- [Characters](https://boxxy.dev/characters)
- [Skills](https://boxxy.dev/skills)
- [MCP Support](https://boxxy.dev/mcp)
- [Boxxy Theme API](https://boxxy.dev/theme-api)
- [Development](https://boxxy.dev/development)

---

## License

MIT
