//! Snapshot tests for UI rendering.

use sysoverlay_core::config::Config;
use sysoverlay_core::layout::CardLayout;
use sysoverlay_core::layout::{compute, LayoutInput, RowsShown};
use sysoverlay_core::types::{Shape, VertLayout, Zoom};
use sysoverlay_ui::{
    decor::{EcoDecoration, QualityDecoration},
    palette::Style,
    raster::BitmapTextRasterizer,
    DrawCtx,
};

fn make_layout(minimal: bool, vert: VertLayout, zoom: Zoom, shape: Shape) -> CardLayout {
    compute(&LayoutInput {
        minimal,
        vert,
        zoom,
        dpi: 96,
        shape,
        rows: RowsShown::all(),
    })
}

fn make_ctx(mode: sysoverlay_core::types::RenderMode) -> DrawCtx<'static> {
    let decor: &dyn sysoverlay_ui::decor::Decoration = match mode {
        sysoverlay_core::types::RenderMode::Eco => &EcoDecoration,
        sysoverlay_core::types::RenderMode::Quality => &QualityDecoration,
    };
    DrawCtx {
        style: Style::default(),
        text: &BitmapTextRasterizer,
        decor,
        hover_alpha: 1.0,
    }
}

/// Test that draw_card renders without panicking.
#[test]
fn test_draw_card_basic() {
    let snap = sysoverlay_ui::demo_snapshot();
    let cfg = Config::default();
    let layout = make_layout(false, VertLayout::SingleRow, Zoom::Zoom100, Shape::Round);
    let mut surface = sysoverlay_ui::SoftwareSurface::new(layout.width, layout.height);
    sysoverlay_ui::draw_card(&mut surface, &snap, &cfg, &layout, &make_ctx(sysoverlay_core::types::RenderMode::Quality));

    let non_zero = surface.pixels.iter().filter(|&&p| p != 0).count();
    assert!(non_zero > 0, "Surface should have some non-zero pixels after drawing");
}

/// Test rendering with different shapes.
#[test]
fn test_draw_card_shapes() {
    let snap = sysoverlay_ui::demo_snapshot();
    let cfg = Config::default();

    for shape in [Shape::Round, Shape::Square, Shape::Content] {
        let layout = make_layout(false, VertLayout::SingleRow, Zoom::Zoom100, shape);
        let mut surface = sysoverlay_ui::SoftwareSurface::new(layout.width, layout.height);
        sysoverlay_ui::draw_card(&mut surface, &snap, &cfg, &layout, &make_ctx(sysoverlay_core::types::RenderMode::Quality));
    }
}

/// Test rendering with different zoom levels.
#[test]
fn test_draw_card_zoom() {
    let snap = sysoverlay_ui::demo_snapshot();
    let cfg = Config::default();

    for zoom in [Zoom::Zoom75, Zoom::Zoom100, Zoom::Zoom125, Zoom::Zoom150, Zoom::Zoom200] {
        let layout = make_layout(false, VertLayout::SingleRow, zoom, Shape::Round);
        let mut surface = sysoverlay_ui::SoftwareSurface::new(layout.width, layout.height);
        sysoverlay_ui::draw_card(&mut surface, &snap, &cfg, &layout, &make_ctx(sysoverlay_core::types::RenderMode::Quality));
    }
}

/// Test rendering with different network units.
#[test]
fn test_draw_card_net_units() {
    let snap = sysoverlay_ui::demo_snapshot();

    for unit in [
        sysoverlay_core::types::NetUnit::KB,
        sysoverlay_core::types::NetUnit::MB,
        sysoverlay_core::types::NetUnit::GB,
    ] {
        let mut cfg = Config::default();
        cfg.net_unit = unit;
        let layout = make_layout(false, VertLayout::SingleRow, Zoom::Zoom100, Shape::Round);
        let mut surface = sysoverlay_ui::SoftwareSurface::new(layout.width, layout.height);
        sysoverlay_ui::draw_card(&mut surface, &snap, &cfg, &layout, &make_ctx(sysoverlay_core::types::RenderMode::Quality));
    }
}

/// Test hover alpha effect.
#[test]
fn test_draw_card_hover_alpha() {
    let snap = sysoverlay_ui::demo_snapshot();
    let cfg = Config::default();
    let layout = make_layout(false, VertLayout::SingleRow, Zoom::Zoom100, Shape::Round);

    for alpha in [0.0, 0.5, 1.0] {
        let mut ctx = make_ctx(sysoverlay_core::types::RenderMode::Quality);
        ctx.hover_alpha = alpha;
        let mut surface = sysoverlay_ui::SoftwareSurface::new(layout.width, layout.height);
        sysoverlay_ui::draw_card(&mut surface, &snap, &cfg, &layout, &ctx);
    }
}

/// Test minimal mode rendering.
#[test]
fn test_draw_card_minimal() {
    let snap = sysoverlay_ui::demo_snapshot();
    let cfg = Config::default();

    for vert in [VertLayout::SingleRow, VertLayout::SingleCol, VertLayout::SplitCol] {
        let layout = make_layout(true, vert, Zoom::Zoom100, Shape::Round);
        let mut surface = sysoverlay_ui::SoftwareSurface::new(layout.width, layout.height);
        sysoverlay_ui::draw_card(&mut surface, &snap, &cfg, &layout, &make_ctx(sysoverlay_core::types::RenderMode::Quality));
    }
}
