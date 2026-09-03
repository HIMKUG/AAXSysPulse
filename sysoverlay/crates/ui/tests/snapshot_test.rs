//! Snapshot tests for UI rendering.
//!
//! These tests generate PNG images for visual comparison with the C implementation.
//! Run with `cargo test -p sysoverlay-ui` to generate test outputs.

use sysoverlay_core::config::{Config, NetUnit, Shape as ConfigShape};
use sysoverlay_core::layout::{compute, Shape as LayoutShape, VertLayout, Zoom};
use sysoverlay_ui::{draw_card, demo_snapshot, SoftwareSurface};

/// Test that draw_card renders without panicking.
#[test]
fn test_draw_card_basic() {
    let snap = demo_snapshot();
    let cfg = Config::default();
    let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, LayoutShape::Round);

    let mut surface = SoftwareSurface::new(layout.width, layout.height);
    draw_card(&mut surface, &snap, &cfg, &layout, 1.0);

    // Verify surface has some non-zero pixels
    let non_zero = surface.pixels.iter().filter(|&&p| p != 0).count();
    assert!(non_zero > 0, "Surface should have some non-zero pixels after drawing");
}

/// Test that generates a reference PNG for visual comparison.
#[test]
fn generate_reference_image() {
    let snap = demo_snapshot();
    let cfg = Config::default();
    let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, LayoutShape::Round);

    let mut surface = SoftwareSurface::new(layout.width, layout.height);
    draw_card(&mut surface, &snap, &cfg, &layout, 1.0);

    let png = surface.to_png();

    // Create test_output directory if needed
    let output_dir = std::path::Path::new("test_output");
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir).unwrap();
    }

    std::fs::write("test_output/test_card.png", png).unwrap();
    println!("Generated test_output/test_card.png ({} bytes)", std::fs::read("test_output/test_card.png").unwrap().len());
}

/// Test rendering with different shapes.
#[test]
fn test_draw_card_shapes() {
    let snap = demo_snapshot();
    let cfg = Config::default();
    let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, LayoutShape::Round);

    for shape in [ConfigShape::Round, ConfigShape::Square, ConfigShape::Content] {
        let mut cfg = cfg;
        cfg.shape = shape;
        let mut surface = SoftwareSurface::new(layout.width, layout.height);
        draw_card(&mut surface, &snap, &cfg, &layout, 1.0);
    }
}

/// Test rendering with different zoom levels.
#[test]
fn test_draw_card_zoom() {
    let snap = demo_snapshot();
    let cfg = Config::default();

    for zoom in [Zoom::Zoom75, Zoom::Zoom100, Zoom::Zoom125, Zoom::Zoom150, Zoom::Zoom200] {
        let layout = compute(false, VertLayout::SingleRow, zoom, 96, LayoutShape::Round);
        let mut surface = SoftwareSurface::new(layout.width, layout.height);
        draw_card(&mut surface, &snap, &cfg, &layout, 1.0);
    }
}

/// Test rendering with different network units.
#[test]
fn test_draw_card_net_units() {
    let snap = demo_snapshot();

    for unit in [NetUnit::KB, NetUnit::MB, NetUnit::GB] {
        let mut cfg = Config::default();
        cfg.net_unit = unit;
        let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, LayoutShape::Round);
        let mut surface = SoftwareSurface::new(layout.width, layout.height);
        draw_card(&mut surface, &snap, &cfg, &layout, 1.0);
    }
}

/// Test hover alpha effect.
#[test]
fn test_draw_card_hover_alpha() {
    let snap = demo_snapshot();
    let cfg = Config::default();
    let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, LayoutShape::Round);

    for alpha in [0.0, 0.5, 1.0] {
        let mut surface = SoftwareSurface::new(layout.width, layout.height);
        draw_card(&mut surface, &snap, &cfg, &layout, alpha);
    }
}

/// Test minimal mode rendering.
#[test]
fn test_draw_card_minimal() {
    let snap = demo_snapshot();
    let cfg = Config::default();

    for vert in [VertLayout::SingleRow, VertLayout::SingleCol, VertLayout::SplitCol] {
        let mut cfg = cfg;
        cfg.minimal = true;
        let layout = compute(true, vert, Zoom::Zoom100, 96, LayoutShape::Round);
        let mut surface = SoftwareSurface::new(layout.width, layout.height);
        draw_card(&mut surface, &snap, &cfg, &layout, 1.0);
    }
}
