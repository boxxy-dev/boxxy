Boxxy is the ONLY app you'll ever need ..but, yeah, it does look like a Linux Terminal :p

![Boxxy](https://i.imgur.com/NlKIIpP.png)

## Features
Boxxy is currently in early preview, but it does have most of the things you expect from a terminal:

- Tabs on the headerbar
- Notifications
- Split panes with softswap
- Preview images and videos (via GTK popover)
- AI Chat, currently with Gemini and Ollama providers, but more will be added
- Search
- Support images with Kitty Graphics Protocol
- Command Palette
- Boxxy Apps bridging Lua scripts to GTK4 widgets
- Themes
- More to come.. Much much more!!

## Installation
There is a temporary Flatpak remote for Boxxy while Flathub submission is in progress
```bash
flatpak remote-add --user --no-gpg-verify boxxy https://miifrommera.github.io/boxxy-flatpak-remote/repo
flatpak install --user boxxy play.mii.Boxxy
```
Boxxy version in About, should match Github [Releases](https://github.com/miifrommera/boxxy/releases) latest version. Requires GNOME 50 Sdk, currently GNOME Nightly

## Not Yet Another Terminal Emulator
While Boxxy is more than capable of running your Linux commands, that's not her primary goal; Boxxy is specifically designed to integrate `boxxy-claw`, a super fast [OpenClaw](https://github.com/openclaw/openclaw) agent, similar to [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) with a very tight integration with the Linux terminal and your Linux system.

Also Boxxy is **made to be fun!** There is a `Characters` feature planned with community plugins that will be able to change the AI personality, and AI voice (with voice cloning).

## Common Issues
Boxxy has very little use in the wild yet, so it won't **really** be a surprise to discover stupid bugs. But most particularly you will face two kinds: either some weird rendering, which is caused by `boxxy-vte`, or issues with your session, which will be caused by `boxxy-agent` trying to access userspace outside the sandbox and update the Flatpak client.

Before you open an issue, please, if possible, compare with [Ghostty](https://github.com/ghostty-org/ghostty) which has very solid mechanics.

## License
Boxxy will be closed source until version 0.1.0, which is the version where all the stuff will be figured out without frequent re-writes. Originally that was planned for before the GNOME 50 release, but I don't see it happening. However, it won't be much longer after, and Boxxy will 100% be an open source project!

## Boxxy's 4 Components
In Boxxy everything is a separate crate and they communicate with public structs and traits. However, there are 4 special components in the workspace:

- `boxxy-app`: The UI
- `boxxy-agent`: The privileged agent that runs outside the sandbox. It is responsible for bypassing Flatpak limitations, managing your PTY and host processes, and securely piping that data back to the UI.
- `boxxy-vte`: A headless, modern VTE written in pure Rust that GNOME and GTK apps can use instead of traditional `vte4-rs`.
- `boxxy-claw`: The agentic part of Boxxy, currently developed in another repo, and I don't think it will be in 0.1.0.

## Technology
Boxxy is built with some seriously cool tech under the hood:
- **Rust 2024:** Because safety and speed are awesome!
- **GTK4 & Libadwaita:** For a fully native, crisp, and modern UI.
- **Tokio & async-channel:** A heavy-duty multi-threaded async engine keeps the UI buttery smooth even when the terminal is sweating.
- **Zero C Dependencies & Pure-Rust Engine:** Yep, you read that right. `boxxy-vte` completely ditches traditional C libraries. It features a custom, lock-free, async-first ANSI state machine.
- **Blazing Fast Rendering:** We render the terminal grid directly via GTK4's native GSK scene graph for top-tier performance! 
- **Modern Terminal Smarts:** Native OSC 8 hyperlinks, debounced asynchronous media previews, and native OSC 7 parsing for instantaneous, event-driven CWD tracking across the sandbox boundary.
- **Flatpak Hole-Punching:** We use D-Bus and `socketpair()` magic to let `boxxy-agent` securely talk to your host system without breaking the Flatpak sandbox rules.
