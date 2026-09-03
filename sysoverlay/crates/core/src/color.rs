//! Color module for SysOverlay.
//!
//! Provides RGB color handling, palette derivation, and color mixing.

use crate::config::Rgb;

/// Palette containing all derived colors for rendering.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Card background color.
    pub bg: Rgb,
    /// Primary text/numeric color.
    pub fg: Rgb,
    /// Label text color (e.g., "CPU").
    pub title: Rgb,
    /// Secondary text color (e.g., units).
    pub secondary: Rgb,
    /// Border color.
    pub border: Rgb,
    /// Progress bar track color.
    pub track: Rgb,
    /// Progress bar fill color (normal state).
    pub fill: Rgb,
    /// Normal/OK state color (RGB(74, 222, 128)).
    pub ok: Rgb,
    /// Warning state color (RGB(251, 191, 36)).
    pub warn: Rgb,
    /// High load/hot state color (RGB(248, 113, 113)).
    pub hot: Rgb,
    /// Idle state color (RGB(63, 69, 80)).
    pub idle: Rgb,
}

/// Returns the accent color based on the given percentage value.
///
/// - `pct >= 85` -> hot (high load)
/// - `pct >= 60` -> warn (warning)
/// - `pct <= 1` -> idle (essentially idle)
/// - otherwise -> ok (normal)
pub fn accent_for(pct: f32, palette: &Palette) -> Rgb {
    if pct >= 85.0 {
        palette.hot
    } else if pct >= 60.0 {
        palette.warn
    } else if pct <= 1.0 {
        palette.idle
    } else {
        palette.ok
    }
}

/// Derives a complete color palette from the given background and foreground colors.
pub fn derive(bg: Rgb, fg: Rgb) -> Palette {
    let title = mix(fg, Rgb { r: 255, g: 255, b: 255 }, 0.15);
    let secondary = mix(fg, bg, 0.4);
    let border = mix(bg, fg, 0.15);
    let track = mix(bg, Rgb { r: 255, g: 255, b: 255 }, 0.08);
    let fill = fg;

    let ok = Rgb { r: 74, g: 222, b: 128 };
    let warn = Rgb { r: 251, g: 191, b: 36 };
    let hot = Rgb { r: 248, g: 113, b: 113 };
    let idle = Rgb { r: 63, g: 69, b: 80 };

    Palette {
        bg,
        fg,
        title,
        secondary,
        border,
        track,
        fill,
        ok,
        warn,
        hot,
        idle,
    }
}

/// Mixes two colors in perceptually uniform space.
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let a_lin = gamma_to_linear(a);
    let b_lin = gamma_to_linear(b);

    let r = (a_lin.r as f32 + (b_lin.r as f32 - a_lin.r as f32) * t).round() as u8;
    let g = (a_lin.g as f32 + (b_lin.g as f32 - a_lin.g as f32) * t).round() as u8;
    let b = (a_lin.b as f32 + (b_lin.b as f32 - a_lin.b as f32) * t).round() as u8;

    linear_to_gamma(Rgb { r, g, b })
}

fn gamma_to_linear(c: Rgb) -> Rgb {
    Rgb {
        r: gamma_to_linear_channel(c.r),
        g: gamma_to_linear_channel(c.g),
        b: gamma_to_linear_channel(c.b),
    }
}

fn gamma_to_linear_channel(v: u8) -> u8 {
    let v = v as f32 / 255.0;
    let lin = if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    };
    (lin * 255.0).round() as u8
}

fn linear_to_gamma(c: Rgb) -> Rgb {
    Rgb {
        r: linear_to_gamma_channel(c.r),
        g: linear_to_gamma_channel(c.g),
        b: linear_to_gamma_channel(c.b),
    }
}

fn linear_to_gamma_channel(v: u8) -> u8 {
    let v = v as f32 / 255.0;
    let gamma = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (gamma.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mix_colors() {
        let mixed = mix(Rgb { r: 255, g: 0, b: 0 }, Rgb { r: 0, g: 0, b: 255 }, 0.5);
        assert!(mixed.r > 100 && mixed.r < 150);
        assert!(mixed.g < 20);
        assert!(mixed.b > 100 && mixed.b < 150);
    }

    #[test]
    fn test_mix_at_boundaries() {
        let a = Rgb { r: 100, g: 100, b: 100 };
        let b = Rgb { r: 200, g: 200, b: 200 };
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
    }

    #[test]
    fn test_derive_palette() {
        let bg = Rgb { r: 20, g: 22, b: 27 };
        let fg = Rgb { r: 238, g: 241, b: 244 };
        let palette = derive(bg, fg);

        assert_eq!(palette.bg, bg);
        assert_eq!(palette.fg, fg);
        assert_eq!(palette.ok, Rgb { r: 74, g: 222, b: 128 });
        assert_eq!(palette.warn, Rgb { r: 251, g: 191, b: 36 });
        assert_eq!(palette.hot, Rgb { r: 248, g: 113, b: 113 });
        assert_eq!(palette.idle, Rgb { r: 63, g: 69, b: 80 });
    }

    #[test]
    fn test_accent_for() {
        let palette = derive(
            Rgb { r: 20, g: 22, b: 27 },
            Rgb { r: 238, g: 241, b: 244 },
        );

        assert_eq!(accent_for(85.0, &palette), palette.hot);
        assert_eq!(accent_for(100.0, &palette), palette.hot);
        assert_eq!(accent_for(60.0, &palette), palette.warn);
        assert_eq!(accent_for(84.9, &palette), palette.warn);
        assert_eq!(accent_for(0.0, &palette), palette.idle);
        assert_eq!(accent_for(1.0, &palette), palette.idle);
        assert_eq!(accent_for(50.0, &palette), palette.ok);
        assert_eq!(accent_for(2.0, &palette), palette.ok);
        assert_eq!(accent_for(59.9, &palette), palette.ok);
    }

    #[test]
    fn test_gamma_roundtrip() {
        let original = Rgb { r: 128, g: 64, b: 192 };
        let linear = gamma_to_linear(original);
        let back = linear_to_gamma(linear);
        assert!((original.r as i16 - back.r as i16).abs() <= 1);
        assert!((original.g as i16 - back.g as i16).abs() <= 1);
        assert!((original.b as i16 - back.b as i16).abs() <= 1);
    }
}
