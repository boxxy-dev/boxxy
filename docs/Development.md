Boxxy is largely developed with agents. The easiest way to start developing Boxxy is to clone the repository and ask your agent to read the AGENTS.md to get familiar with the project. 

---

## Committing

Because Boxxy is early in development, we care to speed things up as much as possible. So internally we use a private "dev" branch that we work in a bunch of things. When a task is ready, we squeeze all commits to a single BIG, and push in "main". It is still easy to understand the commit changes with an Agent though.

---

## Contributing

While your code contributions are more than welcome, due to the super rapid development in downstream (at least for now), they would be almost always impossible to get merged as it is. In general we prefer "agentic" style PRs; Promtps with codes examples


---

## Flatpak Building

We use Rust 1.94, and for development just use cargo

```
cargo build -p boxxy-agent -p boxxy-terminal
```

However that doesn't guarantee that the Flatpak will work identically, especially if `boxxy-agent` is involved. 

### To build and test a Flatpak


Generate sources

```
python3 flatpak/flatpak-cargo-generator.py Cargo.lock -o flatpak/cargo-sources.json
```

Build repo and export bundle

```
flatpak-builder --repo=repo --force-clean build-dir flatpak/dev.boxxy.BoxxyTerminal.yml
flatpak build-bundle repo boxxy.flatpak dev.boxxy.BoxxyTerminal
```

Or Build & Install in single command

```
flatpak-builder --user --install --force-clean build-dir flatpak/dev.boxxy.BoxxyTerminal.yml
```
