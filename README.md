Boxxy is the ONLY app you'll ever need ..but, yeah, it does look like a Linux Terminal :p

![Boxxy](https://i.imgur.com/NlKIIpP.png)

Check [releases](https://github.com/miifrommera/boxxy/releases) for changelogs

---

## Features
Boxxy is currently in early preview, but it does have most of the things you expect from a terminal:

- Tabs on the headerbar
- Notifications
- Split panes with softswap
- Preview images and videos (via GTK popover)
- AI Chat
- Integrated Claw 🦞
- Search
- Support images with Kitty Graphics Protocol
- Command Palette
- Themes
- And some more! 

---

## Installation
There is a temporary Flatpak remote for Boxxy while Flathub submission is in progress
```bash
flatpak remote-add --user --no-gpg-verify boxxy https://miifrommera.github.io/boxxy-flatpak-remote/repo
flatpak install --user boxxy play.mii.Boxxy
```
Boxxy version in About, should match Github [Releases](https://github.com/miifrommera/boxxy/releases) latest version. Requires GNOME 50 Sdk, currently GNOME Nightly

---

## Not Yet Another Terminal Emulator
While Boxxy is more than capable of running your Linux commands, that's not her primary goal; Boxxy is specifically designed to integrate `boxxy-claw`, a super fast [OpenClaw](https://github.com/openclaw/openclaw) agent, similar to [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) with a very tight integration with the Linux terminal and your Linux system.

Also Boxxy is **made to be fun!** There is a `Characters` feature planned with community plugins that will be able to change the AI personality, and AI voice (with voice cloning).

---

## 🦀 Boxxy Claw

`boxxy-claw` is the agentic part of Boxxy, currently in early testing. It has full access to your system, because we don't really want to limit a terminal use to `[workspaces]`, but it is highly verbose (for the testing phase) so if something bad happens, remember: YOU DID THAT TO YOUR SELVES :p

It supports [Agentic Skills](https://agentskills.io/) but Boxxy is still a Terminal after all; So, if you want to manage your email probably you'll be need a different tool

### Setup

1. In Settings and API add your model credentials; Boxxy currently supports Gemini and Ollama providers  
2. Open Command Palette with `Control+Shift+p`, search for models, and set the model you want to use
3. From "Settings -> Advanced" open "Configuration" and edit `boxxyclaw/skills/linux-system/SKILL.md` to reflect your current system; That's not some special skill, it's just a default skill
4. Shell Integration

```bash
#Zsh Integration (~/.zshrc)
function ?() {
    printf "\033]777;BoxxyClaw;%s\033\\" "$*"
}
alias '??'='?'
```

```bash
#Fish Integration  (~/.config/fish/config.fish)
function ?
    printf "\033]777;BoxxyClaw;%s\033\\" "$argv"
end
function ??
    ? $argv
end
```

```bash
#Bash Integration (~/.bashrc)
function ?() {
    printf "\033]777;BoxxyClaw;%s\033\\" "$*"
}
alias ??='?'
```
You can now type "? " to message `boxxy-claw`

5. By default Claw Mode is off; There is a Start/Stop button in the bottom of the sidebar  

**THAT WAS IT!** Please open issues and suggestions! 

---

## Common Issues
Boxxy has very little use in the wild yet, so it won't **really** be a surprise to discover stupid bugs. But most particularly you will face two kinds: either some weird rendering, which is caused by `boxxy-vte`, or issues with your session, which will be caused by `boxxy-agent` trying to access userspace outside the sandbox and update the Flatpak client.

### Troubleshooting
Before you open an issue, please, if possible, compare with [Ghostty](https://github.com/ghostty-org/ghostty) which has very solid mechanics. Also clear the current configuration, as Boxxy doesn't automatically handle settings migrations for newer versions.

---

## License
Boxxy will be closed source until version 0.1.0, which is the version where all the stuff will be figured out without frequent re-writes. Originally that was planned for before the GNOME 50 release, but I don't see it happening. However, it won't be much longer after, and Boxxy will 100% be an open source project

---

## Boxxy's 4 Components
In Boxxy everything is a separate crate and they communicate with public structs and traits. However, there are 4 special components in the workspace:

- `boxxy-app`: The UI
- `boxxy-agent`: The privileged agent that runs outside the sandbox. It is responsible for bypassing Flatpak limitations, managing your PTY and host processes, and securely piping that data back to the UI.
- `boxxy-vte`: A headless, modern VTE written in pure Rust that GNOME and GTK apps can use instead of traditional `vte4-rs`.
- `boxxy-claw`: The agentic part of Boxxy; The original reason Boxxy was created :p

---

## Technology
Boxxy is built with some seriously cool tech under the hood:
- **Rust 2024:** Because safety and speed are awesome!
- **GTK4 & Libadwaita:** For a fully native, crisp, and modern UI.
- **Tokio & async-channel:** A heavy-duty multi-threaded async engine keeps the UI buttery smooth even when the terminal is sweating.
- **Zero C Dependencies & Pure-Rust Engine:** Yep, you read that right. `boxxy-vte` completely ditches traditional C libraries. It features a custom, lock-free, async-first ANSI state machine.
- **Blazing Fast Rendering:** We render the terminal grid directly via GTK4's native GSK scene graph for top-tier performance! 
- **Modern Terminal Smarts:** Native OSC 8 hyperlinks, debounced asynchronous media previews, and native OSC 7 parsing for instantaneous, event-driven CWD tracking across the sandbox boundary.
- **Flatpak Hole-Punching:** We use D-Bus and `socketpair()` magic to let `boxxy-agent` securely talk to your host system without breaking the Flatpak sandbox rules.
