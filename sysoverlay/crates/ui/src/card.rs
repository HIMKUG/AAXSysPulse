//! Card drawing functions for the overlay UI.
//!
//! This module implements the card rendering logic that mirrors the C `drawCard` function.
//! It draws the overlay card with background, metrics, and progress bars.

use crate::offscreen::SoftwareSurface;
use crate::render::{FontSpec, Rect, RenderTarget, Rgba};
use sysoverlay_core::config::Config;
use sysoverlay_core::layout::{CardLayout, Shape};
use sysoverlay_core::snapshot::Metric;
use sysoverlay_core::snapshot::{MemoryStat, NetStat, Snapshot};

/// Accent colors based on value percentage thresholds.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Background color.
    pub bg: Rgba,
    /// Border color.
    pub border: Rgba,
    /// Progress bar track color.
    pub track: Rgba,
    /// Title text color.
    pub title: Rgba,
    /// Label text color.
    pub name: Rgba,
    /// Value text color.
    pub value: Rgba,
    /// Subtitle text color.
    pub sub: Rgba,
    /// OK status color (0-59%).
    pub ok: Rgba,
    /// Warning status color (60-84%).
    pub warn: Rgba,
    /// Hot status color (85-100%).
    pub hot: Rgba,
    /// Idle status color (<=1%).
    pub idle: Rgba,
}

impl Palette {
    /// Derives a complete palette from background and foreground colors.
    ///
    /// Mirrors the C version's color derivation logic:
    /// - title = fg * 0.76
    /// - name = fg * 0.65
    /// - sub = fg * 0.45
    /// - track = mix(bg, fg, 0.10)
    /// - border = mix(bg, fg, 0.13)
    pub fn derive(bg: (u8, u8, u8), fg: (u8, u8, u8)) -> Self {
        let bg = Rgba::opaque(bg.0, bg.1, bg.2);
        let fg = Rgba::opaque(fg.0, fg.1, fg.2);

        Self {
            bg,
            border: mix_colors(bg, fg, 0.13),
            track: mix_colors(bg, fg, 0.10),
            title: scale_color(fg, 0.76),
            name: scale_color(fg, 0.65),
            value: fg,
            sub: scale_color(fg, 0.45),
            ok: Rgba::opaque(74, 222, 128),
            warn: Rgba::opaque(251, 191, 36),
            hot: Rgba::opaque(248, 113, 113),
            idle: Rgba::opaque(63, 69, 80),
        }
    }
}

/// Scales a color by the given factor (multiplies each component).
fn scale_color(c: Rgba, factor: f32) -> Rgba {
    Rgba::opaque(
        (c.r as f32 * factor) as u8,
        (c.g as f32 * factor) as u8,
        (c.b as f32 * factor) as u8,
    )
}

/// Blends two colors together: mix = c1 * (1 - t) + c2 * t
fn mix_colors(c1: Rgba, c2: Rgba, t: f32) -> Rgba {
    let t1 = 1.0 - t;
    Rgba::opaque(
        (c1.r as f32 * t1 + c2.r as f32 * t) as u8,
        (c1.g as f32 * t1 + c2.g as f32 * t) as u8,
        (c1.b as f32 * t1 + c2.b as f32 * t) as u8,
    )
}

/// Returns the accent color for a given percentage value.
pub fn accent_for(pct: f32, pal: &Palette) -> Rgba {
    if pct >= 85.0 {
        pal.hot
    } else if pct >= 60.0 {
        pal.warn
    } else if pct <= 1.0 {
        pal.idle
    } else {
        pal.ok
    }
}

/// Formats a network rate string based on the value.
///
/// Returns (formatted_string, is_short_form)
fn fmt_rate(rx_bps: u64, tx_bps: u64, unit: &sysoverlay_core::config::NetUnit) -> (String, bool) {
    // Convert to MB/s for formatting
    let rx_mbps = rx_bps as f64 / 1_000_000.0;
    let tx_mbps = tx_bps as f64 / 1_000_000.0;

    match unit {
        sysoverlay_core::config::NetUnit::KB => {
            let rx_k = rx_mbps * 1024.0;
            let tx_k = tx_mbps * 1024.0;
            (format!("{:.0}K", rx_k), true)
        }
        sysoverlay_core::config::NetUnit::GB => {
            let rx_g = rx_mbps / 1024.0;
            let tx_g = tx_mbps / 1024.0;
            (format!("{:.2}G", rx_g), true)
        }
        sysoverlay_core::config::NetUnit::MB => {
            if rx_mbps >= 100.0 {
                (format!("{:.0}M", rx_mbps), true)
            } else {
                (format!("{:.2}M", rx_mbps), true)
            }
        }
    }
}

/// Formats network rate for the network row.
fn fmt_network_rate(rx_bps: u64, tx_bps: u64, unit: &sysoverlay_core::config::NetUnit) -> String {
    let (rx_str, _) = fmt_rate(rx_bps, tx_bps, unit);
    let (tx_str, _) = fmt_rate(tx_bps, rx_bps, unit); // Reuse but swap
    format!("{} / {}", rx_str, tx_str)
}

/// Draws the card background with optional rounded corners.
fn draw_background(surface: &mut SoftwareSurface, cfg: &Config, layout: &CardLayout, pal: &Palette) {
    match cfg.shape {
        sysoverlay_core::config::Shape::Round => {
            let radius = 14; // RADIUS constant
            surface.fill_round_rect(
                Rect::new(layout.width, layout.height),
                radius,
                pal.bg,
            );
        }
        sysoverlay_core::config::Shape::Square => {
            surface.fill_rect(Rect::new(layout.width, layout.height), pal.bg);
        }
        sysoverlay_core::config::Shape::Content => {
            // Content mode: no background, canvas must be cleared by caller
            surface.clear();
        }
    }
}

/// Draws the host name and uptime at the top of the card.
fn draw_header(
    surface: &mut SoftwareSurface,
    layout: &CardLayout,
    snap: &Snapshot,
    cfg: &Config,
    pal: &Palette,
) {
    let pad_x = 16i32;
    let lh = 18i32;
    let y = 12;

    // Host name (placeholder - actual hostname would come from system)
    let host = "PC";
    surface.draw_text(
        host,
        Rect::with_pos(pad_x, y, layout.width - pad_x * 2, lh),
        pal.title,
        FontSpec::ui(),
    );

    // Uptime (if valid)
    if let Metric::Value(_) = snap.cpu {
        let uptime_str = format!("{} sec", snap.timestamp_ms / 1000);
        let (w, _) = surface.measure_text(&uptime_str, FontSpec::ui());
        let x = layout.width - pad_x - w;
        surface.draw_text(
            &uptime_str,
            Rect::with_pos(x, y + 2, w, lh),
            pal.sub,
            FontSpec::ui(),
        );
    }
}

/// Draws a metric row with label, value, and progress bar.
fn draw_metric_row(
    surface: &mut SoftwareSurface,
    label: &str,
    value_str: &str,
    pct: f32,
    x: i32,
    y: i32,
    bar_w: i32,
    row_h: i32,
    bar_h: i32,
    pal: &Palette,
) {
    // Label
    surface.draw_text(
        label,
        Rect::with_pos(x, y, bar_w / 2, row_h),
        pal.name,
        FontSpec::ui(),
    );

    // Value
    let (value_w, _) = surface.measure_text(value_str, FontSpec::mono());
    let value_x = x + bar_w - value_w;
    surface.draw_text(
        value_str,
        Rect::with_pos(value_x, y, value_w, row_h),
        pal.value,
        FontSpec::mono(),
    );

    // Progress bar track
    let track_y = y + row_h + 3;
    surface.fill_round_rect(
        Rect::with_pos(x, track_y, bar_w, bar_h),
        bar_h / 2,
        pal.track,
    );

    // Progress bar fill
    let fill_w = ((bar_w as f32) * (pct / 100.0)).round() as i32;
    if fill_w > 0 {
        surface.fill_round_rect(
            Rect::with_pos(x, track_y, fill_w, bar_h),
            bar_h / 2,
            accent_for(pct, pal),
        );
    }
}

/// Draws the network row (no progress bar, shows rate instead).
fn draw_network_row(
    surface: &mut SoftwareSurface,
    net: &NetStat,
    x: i32,
    y: i32,
    bar_w: i32,
    row_h: i32,
    cfg: &Config,
    pal: &Palette,
) {
    // Label
    surface.draw_text(
        "Network",
        Rect::with_pos(x, y, bar_w / 2, row_h),
        pal.name,
        FontSpec::ui(),
    );

    // Value (rate)
    let rate_str = fmt_network_rate(net.rx_bps, net.tx_bps, &cfg.net_unit);
    let (value_w, _) = surface.measure_text(&rate_str, FontSpec::mono());
    let value_x = x + bar_w - value_w;
    surface.draw_text(
        &rate_str,
        Rect::with_pos(value_x, y, value_w, row_h),
        pal.value,
        FontSpec::mono(),
    );
}

/// Draws the complete overlay card.
pub fn draw_card(
    surface: &mut SoftwareSurface,
    snap: &Snapshot,
    cfg: &Config,
    layout: &CardLayout,
    hover_alpha: f32,
) {
    // Derive palette from config colors
    let pal = Palette::derive((cfg.bg.r, cfg.bg.g, cfg.bg.b), (cfg.fg.r, cfg.fg.g, cfg.fg.b));

    // Draw background
    draw_background(surface, cfg, layout, &pal);

    // Constants matching C code
    let pad_x = 16i32;
    let lh = 18i32;
    let row_h = 17i32;
    let bar_h = 6i32;
    let bar_w = layout.width - pad_x * 2;

    // Draw header (host name and uptime)
    draw_header(surface, layout, snap, cfg, &pal);

    let mut y = 12 + lh + 9; // Start after header

    // Check if we have valid metrics
    let valid = matches!(snap.cpu, Metric::Value(_));

    if !valid {
        // Draw "Initializing..." message
        surface.draw_text(
            "Initializing...",
            Rect::with_pos(pad_x, y, bar_w, row_h),
            pal.name,
            FontSpec::ui(),
        );
        return;
    }

    // CPU row
    if let Metric::Value(cpu) = snap.cpu {
        let value_str = format!("{:.0}%", cpu);
        draw_metric_row(
            surface,
            "CPU",
            &value_str,
            cpu,
            pad_x,
            y,
            bar_w,
            row_h,
            bar_h,
            &pal,
        );
    }
    y += row_h + 3 + bar_h + 9;

    // Memory row (special format: "X% · Y.Z/W.G")
    let mem = &snap.memory;
    let value_str = format!(
        "{:.0}%",
        mem.load_pct
    );
    surface.draw_text(
        "Memory",
        Rect::with_pos(pad_x, y, bar_w / 2, row_h),
        pal.name,
        FontSpec::ui(),
    );
    let (value_w, _) = surface.measure_text(&value_str, FontSpec::mono());
    let value_x = pad_x + bar_w - value_w;
    surface.draw_text(
        &value_str,
        Rect::with_pos(value_x, y, value_w, row_h),
        pal.value,
        FontSpec::mono(),
    );
    y += row_h + 3;

    // Memory progress bar
    surface.fill_round_rect(
        Rect::with_pos(pad_x, y, bar_w, bar_h),
        bar_h / 2,
        pal.track,
    );
    let fill_w = ((bar_w as f32) * (mem.load_pct / 100.0)).round() as i32;
    if fill_w > 0 {
        surface.fill_round_rect(
            Rect::with_pos(pad_x, y, fill_w, bar_h),
            bar_h / 2,
            accent_for(mem.load_pct, &pal),
        );
    }
    y += bar_h + 9;

    // GPU row
    if let Metric::Value(gpu) = snap.gpu {
        let value_str = format!("{:.0}%", gpu);
        draw_metric_row(
            surface,
            "GPU",
            &value_str,
            gpu,
            pad_x,
            y,
            bar_w,
            row_h,
            bar_h,
            &pal,
        );
    }
    y += row_h + 3 + bar_h + 9;

    // Network row
    draw_network_row(surface, &snap.net, pad_x, y, bar_w, row_h, cfg, &pal);

    // Apply hover alpha (multiply all pixel alphas by hover_alpha)
    if hover_alpha < 1.0 {
        apply_hover_alpha(surface, hover_alpha);
    }
}

/// Applies hover alpha effect by scaling all pixel alphas.
fn apply_hover_alpha(surface: &mut SoftwareSurface, alpha: f32) {
    for chunk in surface.pixels.chunks_exact_mut(4) {
        let old_a = chunk[3] as f32;
        let new_a = (old_a * alpha).min(255.0) as u8;
        chunk[3] = new_a;
    }
}

/// Creates a demo snapshot for testing.
///
/// This hardcoded snapshot is used to generate reference PNG images
/// for visual comparison with the C implementation.
pub fn demo_snapshot() -> Snapshot {
    Snapshot {
        seq: 1,
        timestamp_ms: 1000,
        cpu: Metric::Value(67.5),
        memory: MemoryStat {
            load_pct: 45.0,
            used_gb: 8.52,
            total_gb: 16.0,
        },
        gpu: Metric::Value(89.2),
        net: NetStat {
            rx_bps: 1_234_567,
            tx_bps: 234_567,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_palette_derive() {
        let pal = Palette::derive((20, 22, 27), (238, 241, 244));
        assert_eq!(pal.bg.r, 20);
        assert_eq!(pal.value.r, 238);
    }

    #[test]
    fn test_accent_for() {
        let pal = Palette::derive((20, 22, 27), (238, 241, 244));
        assert_eq!(accent_for(90.0, &pal).r, 248); // hot
        assert_eq!(accent_for(70.0, &pal).r, 251); // warn
        assert_eq!(accent_for(50.0, &pal).r, 74);  // ok
        assert_eq!(accent_for(0.5, &pal).r, 63);   // idle
    }

    #[test]
    fn test_demo_snapshot() {
        let snap = demo_snapshot();
        assert!(matches!(snap.cpu, Metric::Value(67.5)));
    }
}
