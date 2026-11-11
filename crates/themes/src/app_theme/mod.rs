pub mod cascade;
pub mod colors;
pub mod community;
pub mod oklch;

pub use cascade::{active_mention_colors, apply_app_theme};
pub use colors::{
    ACCENT_PRESETS, AppThemeColors, AppThemeDefinition, builtin_app_themes,
    generate_accent_override_css,
};
pub use community::{
    discover_community_themes, discover_community_themes_in, ensure_app_themes_dir,
    get_app_themes_dir, parse_community_theme_toml, resolve_app_theme,
};
pub use oklch::{
    Oklch, contrast_fg_for_hex, hex_to_oklch, is_hex_dark, oklch_to_hex,
    readable_color_on_surface, relative_luminance_hex, sanitize_hex_color,
};

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;
    use gtk4 as gtk;
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_sanitize_hex_color_valid_and_invalid() {
        assert_eq!(sanitize_hex_color("#00ACC1"), Some("#00acc1".to_string()));
        assert_eq!(sanitize_hex_color(" #ff7eb6 "), Some("#ff7eb6".to_string()));
        // Opaque 8-digit hex normalizes cleanly to 6-digit #rrggbb
        assert_eq!(
            sanitize_hex_color("#112233ff"),
            Some("#112233".to_string())
        );

        // Non-opaque 8-digit hex is rejected so surfaces can never turn invisible
        assert_eq!(sanitize_hex_color("#11223300"), None);
        assert_eq!(sanitize_hex_color("#11223380"), None);

        // Reject CSS injection attempts or malformed hex strings
        assert_eq!(sanitize_hex_color("red; } * { padding: 999px"), None);
        assert_eq!(sanitize_hex_color("#12345"), None);
        assert_eq!(sanitize_hex_color("#12345g"), None);
        assert_eq!(sanitize_hex_color("00acc1"), None);
    }

    #[test]
    fn test_wcag_contrast_fg_selection() {
        // Dark fills receive crisp white foreground
        assert_eq!(contrast_fg_for_hex("#1d1d20"), "#ffffff");
        assert_eq!(contrast_fg_for_hex("#1d4ed8"), "#ffffff");
        assert_eq!(contrast_fg_for_hex("#e63946"), "#ffffff");

        // Vibrant mid/bright fills receive high-contrast dark ink
        assert_eq!(contrast_fg_for_hex("#00acc1"), "#111418");
        assert_eq!(contrast_fg_for_hex("#7ec868"), "#111418");
        assert_eq!(contrast_fg_for_hex("#ffe066"), "#111418");
        assert_eq!(contrast_fg_for_hex("#ff7eb6"), "#111418");
    }

    #[test]
    fn test_oklch_roundtrip() {
        let orig = "#7ec868";
        let lch = hex_to_oklch(orig).expect("valid hex");
        let roundtripped = oklch_to_hex(lch);
        assert_eq!(roundtripped, orig);
    }

    #[test]
    fn test_readable_color_on_surface_clamps_light_and_dark() {
        // Pale yellow on a white card must darken to remain readable as standalone text
        let clamped_light = readable_color_on_surface("#ffe066", false);
        let l_light = hex_to_oklch(&clamped_light).unwrap().l;
        assert!(l_light <= 0.53, "expected L <= 0.53 on light card, got {l_light}");

        // Deep cobalt blue on a dark card must lighten to remain readable as standalone text
        let clamped_dark = readable_color_on_surface("#1d4ed8", true);
        let l_dark = hex_to_oklch(&clamped_dark).unwrap().l;
        assert!(l_dark >= 0.73, "expected L >= 0.73 on dark card, got {l_dark}");
    }

    #[test]
    fn test_parse_minimal_community_theme_derives_defaults() {
        let toml_src = r##"
[meta]
name = "Synthetic Pastel"
author = "tester"
description = "Minimal synthetic test theme"
mode = "light"
tags = ["pastel", "test"]

[colors]
window_bg = "#fff5f8"
window_fg = "#1e1b24"
accent = "#ff7eb6"
secondary = "#7cc6fe"
highlight = "#ffe066"
"##;

        let parsed = parse_community_theme_toml("synthetic-pastel", toml_src)
            .expect("minimal community theme should parse");
        assert_eq!(parsed.id, "synthetic-pastel");
        assert_eq!(parsed.name, "Synthetic Pastel");
        assert_eq!(parsed.author, "tester");
        assert!(!parsed.is_dark);
        assert!(!parsed.is_builtin);
        assert_eq!(parsed.colors.window_bg, "#fff5f8");
        assert_eq!(parsed.colors.sidebar_bg, "#fff5f8");
        assert_eq!(parsed.colors.card_bg, "#fff5f8");
        assert_eq!(parsed.colors.popover_bg, "#fff5f8");
        assert_eq!(parsed.colors.accent, "#ff7eb6");
        assert_eq!(parsed.colors.secondary, "#7cc6fe");
        assert_eq!(parsed.colors.highlight, "#ffe066");
        assert_eq!(parsed.colors.ink_border, "#111418");
    }

    #[test]
    fn test_parse_explicit_secondary_and_highlight_tokens() {
        let toml_src = r##"
[meta]
name = "Explicit Triad"
author = "tester"
mode = "dark"

[colors]
window_bg = "#1d1d20"
window_fg = "#f2f4f2"
accent = "#7ec868"
secondary = "#91baff"
secondary_fg = "#101828"
highlight = "#f5b971"
highlight_fg = "#20160a"
"##;

        let parsed = parse_community_theme_toml("explicit-triad", toml_src)
            .expect("explicit triad theme should parse");
        assert_eq!(parsed.colors.secondary, "#91baff");
        assert_eq!(parsed.colors.secondary_fg, "#101828");
        assert_eq!(parsed.colors.highlight, "#f5b971");
        assert_eq!(parsed.colors.highlight_fg, "#20160a");
        assert_eq!(
            parsed.colors.preview_dots(),
            ["#1d1d20", "#7ec868", "#91baff", "#f5b971"]
        );

        let css = parsed.colors.to_gtk_css(true);
        assert!(css.contains("@define-color boxxy_secondary_bg_color #91baff;"));
        assert!(css.contains("@define-color boxxy_secondary_fg_color #101828;"));
        assert!(css.contains("@define-color boxxy_highlight_bg_color #f5b971;"));
        assert!(css.contains("@define-color boxxy_highlight_fg_color #20160a;"));
        assert!(css.contains("@define-color boxxy_search_match_bg alpha(@boxxy_highlight_bg_color, 0.32);"));
    }

    #[test]
    fn test_parse_community_theme_rejects_css_injection() {
        let malicious = r##"
[meta]
name = "Bad Theme"

[colors]
window_bg = "#ffffff; @import url('https://example.com/evil.css');"
window_fg = "#000000"
accent = "#00acc1"
"##;
        assert!(parse_community_theme_toml("bad-theme", malicious).is_none());
    }

    #[test]
    fn test_discover_community_themes_in_tempdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid_toml = r##"
[meta]
name = "Sunset Glow"
author = "synth"
mode = "dark"

[colors]
window_bg = "#1a1423"
window_fg = "#f8f0fb"
accent = "#f77f00"
secondary = "#7aa2f7"
highlight = "#fcbf49"
"##;
        fs::write(dir.path().join("sunset-glow.toml"), valid_toml).unwrap();
        fs::write(dir.path().join("ignored.txt"), "not a toml file").unwrap();

        let themes = discover_community_themes_in(dir.path());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].id, "sunset-glow");
        assert_eq!(themes[0].name, "Sunset Glow");
        assert!(themes[0].is_dark);
    }

    #[test]
    fn test_accent_override_css_generation() {
        let css = generate_accent_override_css("#00acc1").expect("valid hex");
        assert!(css.contains("@define-color accent_color #00acc1;"));
        assert!(css.contains("@define-color accent_bg_color #00acc1;"));
        assert!(css.contains("@define-color accent_fg_color #111418;"));
        // Accent override must NOT overwrite secondary or highlight tokens
        assert!(!css.contains("boxxy_secondary"));
        assert!(!css.contains("boxxy_highlight"));
    }

    #[test]
    fn test_boxxy_skeleton_imports_all_exist_and_registered() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let themes_dir = repo_root.join("resources/themes");
        let skeleton_path = themes_dir.join("boxxy_skeleton.css");
        let skeleton_src =
            fs::read_to_string(&skeleton_path).expect("resources/themes/boxxy_skeleton.css exists");

        let mut imported_paths = Vec::new();
        for line in skeleton_src.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("@import \"")
                && let Some(rel_path) = rest.strip_suffix("\";")
            {
                imported_paths.push(rel_path.to_string());
            }
        }

        assert!(
            !imported_paths.is_empty(),
            "boxxy_skeleton.css must declare @import modules"
        );

        let gresource_xml = fs::read_to_string(repo_root.join("resources/resources.gresource.xml"))
            .expect("resources/resources.gresource.xml exists");

        for rel in &imported_paths {
            let module_path = themes_dir.join(rel);
            let content = fs::read_to_string(&module_path).unwrap_or_else(|_| {
                panic!("imported CSS module must exist: {}", module_path.display())
            });
            assert!(
                !content.trim().is_empty(),
                "imported CSS module must not be empty: {}",
                module_path.display()
            );
            let expected_alias = format!("alias=\"themes/{rel}\"");
            assert!(
                gresource_xml.contains(&expected_alias),
                "resources.gresource.xml must register imported module: {expected_alias}"
            );
        }
    }

    #[test]
    fn test_gtk_css_provider_parses_style_skeleton_and_tokens_without_errors() {
        if gtk::init().is_err() {
            return;
        }

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let style_path = repo_root.join("resources/style.css");
        let skeleton_path = repo_root.join("resources/themes/boxxy_skeleton.css");

        let errors: std::rc::Rc<RefCell<Vec<String>>> = std::rc::Rc::new(RefCell::new(Vec::new()));

        for css_path in [&style_path, &skeleton_path] {
            let provider = gtk::CssProvider::new();
            let err_sink = errors.clone();
            provider.connect_parsing_error(move |_prov, section, err| {
                let file_name = section
                    .file()
                    .and_then(|f| f.path())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<inline>".to_string());
                let line = section.start_location().lines() + 1;
                err_sink
                    .borrow_mut()
                    .push(format!("{file_name}:{line}: {err}"));
            });
            provider.load_from_path(css_path);
        }

        // Also validate generated Layer 3 token blocks for built-in and seeded themes
        let mut all_themes = builtin_app_themes();
        all_themes.push(
            parse_community_theme_toml("cotton-candy", community::SEEDED_COTTON_CANDY)
                .expect("seeded cotton-candy.toml must be valid"),
        );
        all_themes.push(
            parse_community_theme_toml("bauhaus", community::SEEDED_BAUHAUS)
                .expect("seeded bauhaus.toml must be valid"),
        );

        for theme in all_themes {
            let provider = gtk::CssProvider::new();
            let err_sink = errors.clone();
            let id = theme.id.clone();
            provider.connect_parsing_error(move |_prov, section, err| {
                let line = section.start_location().lines() + 1;
                err_sink
                    .borrow_mut()
                    .push(format!("tokens({id}):{line}: {err}"));
            });
            provider.load_from_string(&theme.colors.to_gtk_css(theme.is_dark));
        }

        let collected = errors.borrow();
        assert!(
            collected.is_empty(),
            "GTK CSS parser reported errors:\n{}",
            collected.join("\n")
        );
    }
}
