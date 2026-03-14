use std::env;
use std::fs;
use std::path::PathBuf;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct PaletteColors {
    primary: Option<PrimaryColors>,
    cursor: Option<CursorColors>,
    normal: Option<ColorSet>,
    bright: Option<ColorSet>,
    selection: Option<SelectionColors>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct PrimaryColors {
    background: Option<String>,
    foreground: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct CursorColors {
    cursor: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct ColorSet {
    black: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    blue: Option<String>,
    magenta: Option<String>,
    cyan: Option<String>,
    white: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct SelectionColors {
    background: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct TomlTheme {
    colors: Option<PaletteColors>,
}

fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = PathBuf::from(out_dir).join("generated_themes.rs");

    let resources_dir = PathBuf::from("../../resources");
    let palettes_dir = resources_dir.join("palettes");

    println!("cargo:rerun-if-changed=../../resources/palettes");

    let mut palette_files: Vec<_> = fs::read_dir(&palettes_dir)
        .expect("resources/palettes not found")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "toml")
                .unwrap_or(false)
        })
        .collect();

    palette_files.sort_by_key(|e| e.file_name());

    let mut themes_code = String::new();
    themes_code.push_str(
        "pub static THEMES: &[ParsedPaletteStatic] = &[\n",
    );

    for entry in palette_files {
        let path = entry.path();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(&path).unwrap();

        if let Some(parsed) = parse_toml_theme(&stem, &content) {
            themes_code.push_str("    ParsedPaletteStatic {\n");
            themes_code.push_str(&format!("        name: {:?},\n", parsed.name));
            themes_code.push_str(&format!("        id: {:?},\n", parsed.id));
            themes_code.push_str(&format!(
                "        light: {},\n",
                variant_to_code(&parsed.light, &parsed.id, &parsed.name)
            ));
            themes_code.push_str(&format!(
                "        dark: {},\n",
                variant_to_code(&parsed.dark, &parsed.id, &parsed.name)
            ));
            themes_code.push_str("    },\n");
        } else {
            eprintln!("Warning: Failed to parse theme {}", stem);
        }
    }

    themes_code.push_str("];\n");

    fs::write(&dest_path, themes_code).unwrap();
}

struct PaletteVariant {
    background: String,
    foreground: String,
    cursor: String,
    colors: [String; 16],
}

struct ParsedPalette {
    name: String,
    id: String,
    light: PaletteVariant,
    dark: PaletteVariant,
}

fn parse_toml_theme(stem: &str, content: &str) -> Option<ParsedPalette> {
    let theme: TomlTheme = toml::from_str(content).ok()?;
    let colors = theme.colors?;
    
    let primary = colors.primary?;
    let bg = primary.background.unwrap_or_else(|| "#000000".to_string());
    let fg = primary.foreground.unwrap_or_else(|| "#ffffff".to_string());
    
    let cursor = colors.cursor.and_then(|c| c.cursor).unwrap_or_else(|| fg.clone());
    
    let mut palette_colors: [String; 16] = std::array::from_fn(|_| "#000000".to_string());
    
    if let Some(normal) = colors.normal {
        palette_colors[0] = normal.black.unwrap_or_else(|| "#000000".to_string());
        palette_colors[1] = normal.red.unwrap_or_else(|| "#000000".to_string());
        palette_colors[2] = normal.green.unwrap_or_else(|| "#000000".to_string());
        palette_colors[3] = normal.yellow.unwrap_or_else(|| "#000000".to_string());
        palette_colors[4] = normal.blue.unwrap_or_else(|| "#000000".to_string());
        palette_colors[5] = normal.magenta.unwrap_or_else(|| "#000000".to_string());
        palette_colors[6] = normal.cyan.unwrap_or_else(|| "#000000".to_string());
        palette_colors[7] = normal.white.unwrap_or_else(|| "#000000".to_string());
    }
    
    if let Some(bright) = colors.bright {
        palette_colors[8] = bright.black.unwrap_or_else(|| "#000000".to_string());
        palette_colors[9] = bright.red.unwrap_or_else(|| "#000000".to_string());
        palette_colors[10] = bright.green.unwrap_or_else(|| "#000000".to_string());
        palette_colors[11] = bright.yellow.unwrap_or_else(|| "#000000".to_string());
        palette_colors[12] = bright.blue.unwrap_or_else(|| "#000000".to_string());
        palette_colors[13] = bright.magenta.unwrap_or_else(|| "#000000".to_string());
        palette_colors[14] = bright.cyan.unwrap_or_else(|| "#000000".to_string());
        palette_colors[15] = bright.white.unwrap_or_else(|| "#000000".to_string());
    }

    let variant = PaletteVariant {
        background: bg.clone(),
        foreground: fg.clone(),
        cursor: cursor.clone(),
        colors: palette_colors.clone(),
    };
    
    // We duplicate dark to light if we don't have separate definitions in the TOML
    // A more advanced parsing could try to generate a light theme from a dark theme, but we duplicate for now.
    Some(ParsedPalette {
        name: stem.to_string(),
        id: stem.to_string(),
        light: PaletteVariant {
            background: bg.clone(),
            foreground: fg.clone(),
            cursor: cursor.clone(),
            colors: palette_colors.clone(),
        },
        dark: variant,
    })
}

fn variant_to_code(v: &PaletteVariant, id: &str, name: &str) -> String {
    let mut code = String::new();
    code.push_str(
        "PaletteVariantStatic {
",
    );
    code.push_str(&format!(
        "            background: {:?},
",
        v.background
    ));
    code.push_str(&format!(
        "            foreground: {:?},
",
        v.foreground
    ));
    code.push_str(&format!(
        "            cursor: {:?},
",
        v.cursor
    ));

    code.push_str(
        "            colors: [
",
    );
    for color in &v.colors {
        code.push_str(&format!(
            "                {:?},
",
            color
        ));
    }
    code.push_str(
        "            ],
",
    );

    code.push_str(&format!(
        "            gtk_css: {:?},
",
        generate_gtk_css(v)
    ));
    code.push_str(&format!(
        "            sourceview_xml: {:?},
",
        generate_sourceview_xml(v, id, name)
    ));

    code.push_str("        }");
    code
}

fn generate_gtk_css(v: &PaletteVariant) -> String {
    let bg = &v.background;
    let fg = &v.foreground;
    let surface_owned = if is_dark(bg) {
        lighten(bg, 0.08)
    } else {
        darken(bg, 0.08)
    };
    let surface = &surface_owned;

    format!(
        r#"
.terminal-header {{
    background-color: {bg};
    background-image: none;
    color: {fg};
    border-color: transparent;
    box-shadow: none;
}}

.terminal-header tabbar {{
    background-color: transparent;
}}

.terminal-header tabbar tab {{
    background-color: {bg};
    background-image: none;
    color: {fg};
    border-color: transparent;
    box-shadow: none;
}}

.terminal-header tabbar tab:selected {{
    background-color: {surface};
    background-image: none;
    color: {fg};
    font-weight: bold;
}}
"#
    )
}

fn generate_sourceview_xml(v: &PaletteVariant, id: &str, name: &str) -> String {
    let bg = &v.background;
    let fg = &v.foreground;
    let surface_owned = if is_dark(bg) {
        lighten(bg, 0.08)
    } else {
        darken(bg, 0.08)
    };
    let surface = &surface_owned;
    let comment = &v.colors[8];
    let red = &v.colors[1];
    let green = &v.colors[2];
    let yellow = &v.colors[3];
    let blue = &v.colors[4];
    let purple = &v.colors[5];
    let cyan = &v.colors[6];

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<style-scheme id="{id}" _name="{name}" version="1.0">
  <description>Generated from {name} terminal palette</description>

  <color name="bg"      value="{bg}"/>
  <color name="fg"      value="{fg}"/>
  <color name="surface" value="{surface}"/>
  <color name="comment" value="{comment}"/>
  <color name="red"     value="{red}"/>
  <color name="green"   value="{green}"/>
  <color name="yellow"  value="{yellow}"/>
  <color name="blue"    value="{blue}"/>
  <color name="purple"  value="{purple}"/>
  <color name="cyan"    value="{cyan}"/>

  <style name="text"          background="bg"      foreground="fg"/>
  <style name="selection"     background="surface"/>
  <style name="cursor"        foreground="fg"/>
  <style name="current-line"  background="surface"/>
  <style name="line-numbers"  foreground="comment" background="bg"/>
  <style name="draw-spaces"   foreground="comment"/>

  <style name="def:comment"              foreground="comment" italic="true"/>
  <style name="def:shebang"              foreground="comment" bold="true"/>
  <style name="def:doc-comment-element"  foreground="comment" italic="true"/>

  <style name="def:string"           foreground="green"/>
  <style name="def:number"           foreground="yellow"/>
  <style name="def:floating-point"   foreground="yellow"/>
  <style name="def:boolean"          foreground="purple"/>
  <style name="def:constant"         foreground="purple"/>
  <style name="def:special-constant" foreground="purple" italic="true"/>
  <style name="def:special-char"     foreground="red"/>

  <style name="def:identifier"  foreground="fg"/>
  <style name="def:function"    foreground="green"/>
  <style name="def:keyword"     foreground="purple"/>
  <style name="def:statement"   foreground="purple"/>
  <style name="def:builtin"     foreground="cyan"   italic="true"/>
  <style name="def:type"        foreground="cyan"   italic="true"/>
  <style name="def:preprocessor" foreground="red"/>

  <style name="def:error"   foreground="red"    underline="true"/>
  <style name="def:warning" foreground="yellow" underline="true"/>
  <style name="def:note"    foreground="cyan"   underline="true"/>
</style-scheme>
"#
    )
}

fn darken(hex: &str, factor: f32) -> String {
    transform_lightness(hex, |l| (l * (1.0 - factor)).max(0.0))
}

fn lighten(hex: &str, factor: f32) -> String {
    transform_lightness(hex, |l| (l + (1.0 - l) * factor).min(1.0))
}

fn is_dark(hex: &str) -> bool {
    if let Some((r, g, b)) = hex_to_rgb(hex) {
        let (_, _, l) = rgb_to_hsl(r, g, b);
        l < 0.5
    } else {
        true
    }
}

fn transform_lightness(hex: &str, f: impl Fn(f32) -> f32) -> String {
    let Some((r, g, b)) = hex_to_rgb(hex) else {
        return hex.to_string();
    };
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let (nr, ng, nb) = hsl_to_rgb(h, s, f(l));
    format!("#{:02x}{:02x}{:02x}", nr, ng, nb)
}

fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s < f32::EPSILON {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    (
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}
