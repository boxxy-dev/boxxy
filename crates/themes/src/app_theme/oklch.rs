/// Perceptual OKLab / OKLCH color math, WCAG relative luminance, contrast foreground
/// selection, and surface-readability lightness clamping.
///
/// Implemented in pure Rust with zero external dependencies.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    /// Perceptual lightness in `[0.0, 1.0]`.
    pub l: f32,
    /// Perceptual chroma (`0.0` = neutral grey, `~0.32` = max sRGB saturation).
    pub c: f32,
    /// Hue angle in degrees `[0.0, 360.0)`.
    pub h_deg: f32,
}

/// Validates a hex color string, returning canonical lowercase 6-digit `#rrggbb`.
///
/// - Accepts `#RRGGBB` directly.
/// - Accepts opaque 8-digit `#RRGGBBFF` (commonly copied from design tools) and normalizes to `#rrggbb`.
/// - Rejects translucent/transparent `#RRGGBBAA` (`AA != FF`) so solid surface/accent tokens
///   can never make windows or text invisible, and rejects any CSS injection characters.
#[must_use]
pub fn sanitize_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{}", hex.to_ascii_lowercase())),
        8 if hex[6..8].eq_ignore_ascii_case("ff") => {
            Some(format!("#{}", hex[0..6].to_ascii_lowercase()))
        }
        _ => None,
    }
}

/// Parses a `#RRGGBB` string into `(r, g, b)` bytes.
#[must_use]
pub fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let clean = hex.trim().strip_prefix('#')?;
    if clean.len() < 6 {
        return None;
    }
    let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
    let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
    let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
    Some((r, g, b))
}

#[inline]
fn srgb_to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb_u8(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

/// Computes ITU-R BT.709 / WCAG 2.x relative luminance `Y` in `[0.0, 1.0]` from linearized sRGB.
#[must_use]
pub fn relative_luminance_hex(hex: &str) -> Option<f32> {
    let (r, g, b) = parse_hex_rgb(hex)?;
    let rl = srgb_to_linear(r);
    let gl = srgb_to_linear(g);
    let bl = srgb_to_linear(b);
    Some(0.2126 * rl + 0.7152 * gl + 0.0722 * bl)
}

/// Classifies whether a `#RRGGBB` color is perceptually dark.
///
/// Uses linearized WCAG relative luminance with a `0.22` threshold (near the mathematical
/// contrast midpoint `Y ≈ 0.193` between `#111418` dark ink and `#ffffff` white, nudged slightly
/// upward so deep saturated reds/crimsons like `#e63946` receive crisp white foregrounds while
/// vibrant mid-tones like `#00acc1` cyan, `#ff7eb6` pink, and `#7ec868` green pair with `#111418` dark ink).
#[must_use]
pub fn is_hex_dark(hex: &str) -> bool {
    relative_luminance_hex(hex).is_none_or(|y| y < 0.22)
}

/// Computes a high-contrast foreground color (`#111418` or `#ffffff`) for a `#RRGGBB` fill.
#[must_use]
pub fn contrast_fg_for_hex(hex: &str) -> String {
    if is_hex_dark(hex) {
        "#ffffff".to_string()
    } else {
        "#111418".to_string()
    }
}

/// Converts a `#RRGGBB` hex string into perceptual `Oklch`.
#[must_use]
pub fn hex_to_oklch(hex: &str) -> Option<Oklch> {
    let (r8, g8, b8) = parse_hex_rgb(hex)?;
    let r = srgb_to_linear(r8);
    let g = srgb_to_linear(g8);
    let b = srgb_to_linear(b8);

    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    let ok_l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let ok_a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let ok_b = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;

    let c = ok_a.hypot(ok_b);
    let h_deg = if c < 1e-4 {
        0.0
    } else {
        ok_b.atan2(ok_a).to_degrees().rem_euclid(360.0)
    };

    Some(Oklch {
        l: ok_l.clamp(0.0, 1.0),
        c: c.max(0.0),
        h_deg,
    })
}

#[inline]
fn oklch_to_linear_srgb(l: f32, c: f32, h_rad: f32) -> (f32, f32, f32) {
    let ok_a = c * h_rad.cos();
    let ok_b = c * h_rad.sin();

    let l_ = l + 0.396_337_78 * ok_a + 0.215_803_76 * ok_b;
    let m_ = l - 0.105_561_35 * ok_a - 0.063_854_17 * ok_b;
    let s_ = l - 0.089_484_18 * ok_a - 1.291_485_5 * ok_b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    (r, g, b)
}

#[inline]
fn in_srgb_gamut(r: f32, g: f32, b: f32) -> bool {
    const EPS: f32 = 1e-4;
    (-EPS..=1.0 + EPS).contains(&r)
        && (-EPS..=1.0 + EPS).contains(&g)
        && (-EPS..=1.0 + EPS).contains(&b)
}

/// Converts an `Oklch` color to `#rrggbb`, binary-searching chroma if needed so the
/// target lightness `L` and hue `H` map into sRGB without per-channel hue distortion.
#[must_use]
pub fn oklch_to_hex(color: Oklch) -> String {
    let l = color.l.clamp(0.0, 1.0);
    let h_rad = color.h_deg.rem_euclid(360.0).to_radians();
    let target_c = color.c.max(0.0);

    let (mut r, mut g, mut b) = oklch_to_linear_srgb(l, target_c, h_rad);
    if !in_srgb_gamut(r, g, b) {
        let mut low = 0.0_f32;
        let mut high = target_c;
        for _ in 0..14 {
            let mid = 0.5 * (low + high);
            let (mr, mg, mb) = oklch_to_linear_srgb(l, mid, h_rad);
            if in_srgb_gamut(mr, mg, mb) {
                low = mid;
            } else {
                high = mid;
            }
        }
        (r, g, b) = oklch_to_linear_srgb(l, low, h_rad);
    }

    format!(
        "#{:02x}{:02x}{:02x}",
        linear_to_srgb_u8(r),
        linear_to_srgb_u8(g),
        linear_to_srgb_u8(b)
    )
}

/// Adjusts a color's OKLCH lightness `L` (preserving hue) so it remains readable when used as
/// standalone text, icon tint, or subtle chip label directly on top of a dark or light card surface.
///
/// For example, a sunny pastel yellow (`#ffe066`, `L ≈ 0.90`) stays `#ffe066` as a solid badge fill
/// (`@boxxy_highlight_bg_color`), while clamping down to `L <= 0.52` for `@boxxy_highlight_color`
/// when rendered as text on a crisp `#ffffff` light card.
#[must_use]
pub fn readable_color_on_surface(hex: &str, is_surface_dark: bool) -> String {
    let Some(mut oklch) = hex_to_oklch(hex) else {
        return hex.to_string();
    };

    if is_surface_dark {
        if oklch.l < 0.74 {
            oklch.l = 0.74;
        }
    } else if oklch.l > 0.52 {
        oklch.l = 0.52;
    }

    oklch_to_hex(oklch)
}
