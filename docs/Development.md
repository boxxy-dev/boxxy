Boxxy is largely developed with agents. The easiest way to start developing Boxxy is to clone the repository and ask your agent to read the AGENTS.md to get familiar with the project. 

While we do our best to review the generated code, we certainly miss some spots. It would be awesome if you could find those and have them fixed!

---

## Committing

Because Boxxy is early in development, we care to speed things up as much as possible. So internally we use a private "dev" branch that we work in a bunch of things. When the task is ready, we squeeze all commits to a single BIG, and push in "main", and most usually that will be actual releases. It is still easy to understand the commit changes with an Agent though.

---

## Contributing

While your code contributions are more than welcome, due to the super rapid development in downstream (at least for now), they would be almost always impossible to get merged as it is. So, we can only add a reference to a PR in our commits

However, your software engineering values more! So if you have a software design suggestion, we are LISTENING!

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
flatpak-builder --repo=repo --force-clean build-dir flatpak/play.mii.Boxxy.yml
flatpak build-bundle repo boxxy.flatpak play.mii.Boxxy
```

Or Build & Install in single command

```
flatpak-builder --user --install --force-clean build-dir flatpak/play.mii.Boxxy.yml
```
