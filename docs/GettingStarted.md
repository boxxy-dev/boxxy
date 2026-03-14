
Boxxy is an agentic terminal, but you can also use it without the AI part... then again, where's the fun in that? 😅

---

## Shell Integration

Boxxy relies on your Shell and not injecting features to modify it. It is strongly recommended to use a modern Shell in Rust like [Fish](https://github.com/fish-shell/fish-shell). Also, to enable Hyperlinks (OSC 8), make sure that your CLI tools like `ls` or `eza` use `--hyperlink=auto`

```bash
# Zsh Integration (~/.zshrc)
function ?() {
    printf "\033]777;BoxxyClaw;%s\033\\" "$*"
}
alias '??'='?'
```

```bash
# Fish Integration (~/.config/fish/config.fish)
function ?
    printf "\033]777;BoxxyClaw;%s\033\\" "$argv"
end
function ??
    ? $argv
end
```

```bash
# Bash Integration (~/.bashrc)
function ?() {
    printf "\033]777;BoxxyClaw;%s\033\\" "$*"
}
alias ??='?'
```
You can now type `? help me to debug my audio not working` to message `boxxy-claw`

---

## Model Selection

Boxxy currently supports [Gemini](https://aistudio.google.com/) and local Ollama connections, but more providers will come. However Boxxy for now is strongly tested and optimized for the Gemini family. From Preferences -> APIs, add your connection strings. Then, press `Ctrl+Shift+P` to open the Command Palette. Type "models" and in the Model Selection dialog select the model you want to use for both Claw, AI Chat and Memories.

Memories is an extra model that can match prompt queries to SQL FTS; Use a fast model like Gemini Flash Lite with "Minimal" thinking level.  

---

## Enable Claw Mode

By default all Boxxy windows start with Claw Mode Off. Enable Claw from the Claw icon on headerbar. Now, all Tabs and Split panes will use ClawAgents. Every pane starts its own agent. So, from pane (UUID1) you can ask the agent what pane (UUID2) agent does, and let them debate (that's a planned feature basically!)

---

## Proactive Vs Lazy Modes

Lazy Mode is the default, and it will only create a response if you ask for it (5s cooldown), while Proactive will immediately create a response. To put it simply, Proactive will consume your model resources much faster 🥲 

---

## Distraction-Free Mode

Boxxy's goal is to provide a non-distracting experience directly in terminal view; We are not there  just yet 😪 You can disable Boxxy Popovers and only use BoxxyClaw in sidebar; That also provides a more verbose experience, so, sometimes you may want to use both!

---

## Set Skills

From Preferences open the config folder and navigate to "boxxyclaw" and "skills". Here, you can use standard [Agentic Skills](https://agentskills.io/home), but a Boxxy Skills marketplace is also planned! In any case, don't forget to add your System Specs in the "linux-system" default skill

---

## Memories

Boxxy is a self-improving system via Memories. You can tell BoxxyClaw "? my favorite editor is micro" and Boxxy will remember that for the next time; While the initial functionality is here, Boxxy is still a Preview; There isn't a migration strategy yet, that means your memories might get wiped in Boxxy updates. However, the Long Term memory will survive! You can manually edit this file in `.config/boxxy-terminal/boxxyclaw/MEMORY.md`

## Others

You may need to raise your inotify limits. Check your current values:

```
cat /proc/sys/fs/inotify/max_user_instances
cat /proc/sys/fs/inotify/max_user_watches
```

For example, Fedora's defaults are too low; Raise both:

```
echo fs.inotify.max_user_instances=65536 | sudo tee -a /etc/sysctl.conf && sudo sysctl -p
echo fs.inotify.max_user_watches=524288 | sudo tee -a /etc/sysctl.conf && sudo sysctl -p
```
