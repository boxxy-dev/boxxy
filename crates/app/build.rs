use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Resource entry discovered under `../../resources`.
struct ResourceEntry {
    /// Forward-slash path relative to `resources/` (used as both `GResource` `alias` and source path).
    rel_path: String,
    /// Whether `glib-compile-resources` should deflate-compress this payload.
    /// Text/XML assets benefit substantially; already-compressed PNGs and seekable WAV audio
    /// streams stay uncompressed so GIO serves zero-copy `GMemoryInputStream` slices from `.rodata`.
    compressed: bool,
    /// Emit an explicit `alias="..."` attribute (kept on all nested entries for clarity and
    /// compatibility with `crates/themes` `GResource` manifest assertions).
    with_alias: bool,
}

fn main() {
    let resources_dir = Path::new("../../resources");
    println!("cargo:rerun-if-changed=build.rs");

    let mut entries = Vec::with_capacity(128);

    // 1. Core stylesheets & modular Boxxy theme CSS files.
    println!("cargo:rerun-if-changed=../../resources/style.css");
    entries.push(ResourceEntry {
        rel_path: "style.css".to_string(),
        compressed: true,
        with_alias: false,
    });

    println!("cargo:rerun-if-changed=../../resources/themes/boxxy_skeleton.css");
    entries.push(ResourceEntry {
        rel_path: "themes/boxxy_skeleton.css".to_string(),
        compressed: true,
        with_alias: true,
    });

    collect_dir_files(resources_dir, "themes/boxxy", false, &["css"], &mut entries);

    // 2. Icons (`.svg`, `.gpa` compressed; `.png` uncompressed to avoid double DEFLATE).
    collect_dir_files(
        resources_dir,
        "icons",
        false,
        &["png", "svg", "gpa"],
        &mut entries,
    );

    // 3. System & memory prompts (`.md`).
    collect_dir_files(resources_dir, "prompts", false, &["md"], &mut entries);

    // 4. Declarative GTK Builder UI definitions (`ui/**/*.ui`).
    collect_dir_files(resources_dir, "ui", true, &["ui"], &mut entries);

    // 5. Notification audio cues (`.wav` kept uncompressed for seekable zero-copy GStreamer playback).
    collect_dir_files(resources_dir, "sounds", false, &["wav"], &mut entries);

    let xml = render_gresource_xml(&entries);

    // Best-effort sync of the checked-in manifest in dev checkouts without failing builds
    // when the source tree is mounted read-only (e.g. Nix `buildRustPackage` or distro chroots).
    let repo_xml_path = resources_dir.join("resources.gresource.xml");
    if std::fs::read_to_string(&repo_xml_path).ok().as_deref() != Some(xml.as_str()) {
        let _ = std::fs::write(&repo_xml_path, &xml);
    }

    // Always compile from `$OUT_DIR/resources.gresource.xml` so builds succeed on read-only trees.
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let out_xml_path = out_dir.join("resources.gresource.xml");
    if std::fs::read_to_string(&out_xml_path).ok().as_deref() != Some(xml.as_str()) {
        std::fs::write(&out_xml_path, &xml)
            .expect("failed to write $OUT_DIR/resources.gresource.xml");
    }

    glib_build_tools::compile_resources(
        &[resources_dir],
        out_xml_path.to_str().expect("OUT_DIR must be valid UTF-8"),
        "compiled.gresource",
    );
}

fn should_compress_extension(ext_lower: &str) -> bool {
    !matches!(ext_lower, "png" | "jpg" | "jpeg" | "webp" | "wav")
}

fn collect_dir_files(
    resources_dir: &Path,
    subdir: &str,
    recursive: bool,
    allowed_exts: &[&str],
    out: &mut Vec<ResourceEntry>,
) {
    let root = resources_dir.join(subdir);
    let mut discovered = Vec::new();
    walk_dir(
        resources_dir,
        &root,
        recursive,
        allowed_exts,
        &mut discovered,
    );
    discovered.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    for entry in &discovered {
        println!("cargo:rerun-if-changed=../../resources/{}", entry.rel_path);
    }
    out.extend(discovered);
}

fn walk_dir(
    resources_dir: &Path,
    dir: &Path,
    recursive: bool,
    allowed_exts: &[&str],
    out: &mut Vec<ResourceEntry>,
) {
    println!("cargo:rerun-if-changed={}", dir.display());

    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read resource directory '{}': {e}", dir.display()));

    let mut subdirs = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            if recursive {
                subdirs.push(path);
            }
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let ext_lower = ext.to_ascii_lowercase();
        if !allowed_exts.iter().any(|allowed| *allowed == ext_lower) {
            continue;
        }
        let rel = path
            .strip_prefix(resources_dir)
            .expect("resource file must lie under resources_dir");
        let rel_path = rel
            .components()
            .map(|c| c.as_os_str().to_str().expect("UTF-8 resource path"))
            .collect::<Vec<_>>()
            .join("/");

        out.push(ResourceEntry {
            rel_path,
            compressed: should_compress_extension(&ext_lower),
            with_alias: true,
        });
    }

    subdirs.sort();
    for sub in subdirs {
        walk_dir(resources_dir, &sub, recursive, allowed_exts, out);
    }
}

fn render_gresource_xml(entries: &[ResourceEntry]) -> String {
    let mut xml = String::with_capacity(entries.len() * 96 + 128);
    xml.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<gresources>\n\
  <gresource prefix=\"/dev/boxxy/BoxxyTerminal\">\n",
    );

    for entry in entries {
        xml.push_str("    <file");
        if entry.compressed {
            xml.push_str(" compressed=\"true\"");
        }
        if entry.with_alias {
            let _ = write!(xml, " alias=\"{}\"", entry.rel_path);
        }
        let _ = writeln!(xml, ">{}</file>", entry.rel_path);
    }

    xml.push_str("  </gresource>\n</gresources>\n");
    xml
}
