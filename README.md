
Boxxy is the ONLY app you'll ever need ..but, yeah, it does look like a Linux Terminal :p

![Boxxy](https://i.imgur.com/NlKIIpP.png)

## Getting Started

Read the [Getting Started](docs/GettingStarted.md) and check news on [Releases](https://github.com/miifrommera/boxxy/releases)

---

## Features
Boxxy is currently in early preview, but it does have most of the things you expect from a terminal:

- Tabs on the headerbar
- Notifications
- Split panes with softswap
- Preview images and videos (via GTK popover)
- AI Chat
- Integrated Self Improving Claw 🦀
- Search
- Support images with Kitty Graphics Protocol
- Command Palette
- Themes
- And some more! 

---

## Installation

```bash
#Nightly builds; It supports automatic updates from within the app
curl -sSf https://raw.githubusercontent.com/miifrommera/boxxy/main/scripts/install.sh | sh
```

Flathub submission [in progress..](https://github.com/flathub/flathub/pull/8098)

---

## Not Yet Another Terminal Emulator
While Boxxy is more than capable of running your Linux commands, that's not her primary goal; Boxxy is specifically designed to integrate `boxxy-claw`, a super fast [OpenClaw](https://github.com/openclaw/openclaw) agent, similar to [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) with a very tight integration with the Linux terminal and your Linux system.

Also Boxxy is **made to be fun!** There is a `Characters` feature planned with community plugins that will be able to change the AI personality, and AI voice (with voice cloning).

Boxxy has 4 major components:

- `boxxy-app`: The UI
- `boxxy-agent`: The privileged agent that runs outside the sandbox. It is responsible for bypassing Flatpak limitations, managing your PTY and host processes, and securely piping that data back to the UI.
- `boxxy-vte`: A headless, modern VTE written in pure Rust
- `boxxy-claw`: The agentic part of Boxxy; The original reason Boxxy was created :p

---

## Development

See [Development](docs/Development.md)

---

## Common Issues
Boxxy has very little use in the wild yet, so it won't **really** be a surprise to discover stupid bugs. But most particularly you will face 4 kinds:

- **APP:** Typical UI bugs; Also clear (or backup) the current configuration, as Boxxy  while in Preview doesn't automatically handle settings migrations for newer versions. 

- **VTE:** Some feature in VTE doesn't work correctly, some CLI app is broken? Please, if possible, compare with [Ghostty](https://github.com/ghostty-org/ghostty) which has very solid mechanics

- **SESSION:** Boxxy might fail to read environment variables; 100% that's an issue on Flatpak builds and in `boxxy-agent`

- **CLAW:** If you use Ollama models, if possible, please re-test a behavior at least with Gemini Flash Lite with "Low" thinking level; 

---

## Roadmap
 - **Bookmarks.** Save scripts and script variables for quick execution  
 - **Boxxy Scripts.** Let Boxxy write and run Lua scripts
 - **Voice.** Speech to Text, Text to Speech 
 - **Characters.** Add characters with voice cloning
 - **Boxxy Marketplace.** A repo that hosts community skills and characters
 - **Cloud Backend and Sync.** That's only a maybe

 --- 

## License
MIT
