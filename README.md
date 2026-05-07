# Boxxy

**A self-improving Linux terminal powered by AI characters.**

![Boxxy](https://i.imgur.com/7NtOXBp.png)

Boxxy is a full terminal emulator with an agentic AI layer — **BoxxyClaw** — built directly into it. Unlike chat-window add-ons, BoxxyClaw has eyes on your shell, reads your terminal buffer, remembers your preferences, and can autonomously fix broken dependencies, manage system configs, and run your scripts. Just press `Ctrl+/` and you're in.

---

## Install
Boxxy is currently in Preview. 

**Native Nightly** (Requires GTK 4.22, libAdwaita 1.,)
```bash
curl -sSf https://raw.githubusercontent.com/boxxy-dev/boxxy/main/scripts/install.sh | sh
```

**Flatpak Nightly**
```bash
curl -O https://boxxy-dev.github.io/boxxy-flatpak-remote/boxxy.gpg && \
  flatpak remote-add --user --if-not-exists --gpg-import=boxxy.gpg \
  boxxy https://boxxy-dev.github.io/boxxy-flatpak-remote/repo && \
  flatpak install --user boxxy dev.boxxy.BoxxyTerminal
```

---

## Architecture

| Crate | Role |
| :--- | :--- |
| `boxxy-app` | GTK4/Adwaita UI |
| `boxxy-agent` | Host-level daemon — PTY management, shell tools, character claims, D-Bus IPC |
| `boxxy-vte` | Headless terminal engine written in pure Rust |
| `boxxy-claw` | Agentic intelligence layer — characters, memory, skills, toolbox |

---

## Documentation

Full docs at **[boxxy.dev](https://boxxy.dev)**

- [Getting Started](https://boxxy.dev/getting-started)
- [How It Works](https://boxxy.dev/how-it-works)
- [Characters](https://boxxy.dev/characters)
- [Skills](https://boxxy.dev/skills)
- [MCP Support](https://boxxy.dev/mcp)
- [Development](https://boxxy.dev/development)

---

## License

MIT
