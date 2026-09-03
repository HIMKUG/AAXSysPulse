//! Card layout computation for different display modes.
//!
//! Handles the layout calculations for the overlay card including
//! regular mode, minimal mode, and various vertical layouts.

/// Vertical layout for minimal mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VertLayout {
    /// Single row: all metrics on one line.
    SingleRow,
    /// Single column: each metric on its own line.
    SingleCol,
    /// Split column: two columns within minimal height.
    SplitCol,
}

/// Zoom level multiplier (percentage / 100).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    Zoom75,
    Zoom100,
    Zoom125,
    Zoom150,
    Zoom200,
}

impl Zoom {
    fn as_factor(&self) -> f64 {
        match self {
            Zoom::Zoom75 => 0.75,
            Zoom::Zoom100 => 1.0,
            Zoom::Zoom125 => 1.25,
            Zoom::Zoom150 => 1.5,
            Zoom::Zoom200 => 2.0,
        }
    }
}

/// Card shape style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Rounded corners.
    Round,
    /// Sharp corners.
    Square,
    /// Content-only (no background/border).
    Content,
}

/// Logical dimensions at 96 DPI.
const CARD_W_L: i32 = 268;
const CARD_H_L: i32 = 184;
const MIN_H_L: i32 = 36;
const MIN_W_L: i32 = 320;
const MINV_W_L: i32 = 128;
const MINV_H_L: i32 = 96;
const MINV2_W_L: i32 = 88;
const MINV2_H_L: i32 = 162;
const PAD_X: i32 = 16;
const _RADIUS: i32 = 14;
const _MARGIN: i32 = 24;

/// Row layout within a card.
#[derive(Clone, Debug)]
pub struct RowLayout {
    /// Label rectangle: (x, y, width, height).
    pub label_rect: (i32, i32, i32, i32),
    /// Value rectangle: (x, y, width, height).
    pub value_rect: (i32, i32, i32, i32),
    /// Progress bar track rectangle: (x, y, width, height).
    pub track_rect: (i32, i32, i32, i32),
    /// Fill width percentage (0..=100) for the progress bar.
    pub fill_width: i32,
}

/// Complete card layout.
#[derive(Clone, Debug)]
pub struct CardLayout {
    /// Total card width in pixels.
    pub width: i32,
    /// Total card height in pixels.
    pub height: i32,
    /// Row layouts for each metric row.
    pub rows: Vec<RowLayout>,
}

/// Compute card layout based on display mode and DPI.
///
/// # Arguments
///
/// * `minimal` - Whether to use minimal (compact) mode.
/// * `vert` - Vertical layout variant (only meaningful when minimal=true).
/// * `zoom` - Zoom level multiplier.
/// * `dpi` - Current DPI (used to compute scale factor relative to 96).
/// * `shape` - Card shape style.
///
/// # Returns
///
/// A [`CardLayout`] with all dimensions and row positions calculated.
pub fn compute(
    minimal: bool,
    vert: VertLayout,
    zoom: Zoom,
    dpi: u32,
    _shape: Shape,
) -> CardLayout {
    let scale = dpi as f64 / 96.0;
    let zoom_factor = zoom.as_factor();
    let f = |logical: i32| -> i32 {
        (logical as f64 * scale * zoom_factor + 0.5) as i32
    };

    let (card_w, card_h) = if minimal {
        match vert {
            VertLayout::SingleCol => (f(MINV_W_L), f(MINV_H_L)),
            VertLayout::SplitCol => (f(MINV2_W_L), f(MINV2_H_L)),
            VertLayout::SingleRow => (f(MIN_W_L), f(MIN_H_L)),
        }
    } else {
        (f(CARD_W_L), f(CARD_H_L))
    };

    let pad_x = f(PAD_X);
    let lh = f(18);
    let bh = f(6);
    let row_h = f(17);

    let mut rows = Vec::with_capacity(4);

    if minimal {
        // Minimal mode: simplified layout without progress bars.
        let ty = f(9);
        let rh = f(16);

        match vert {
            VertLayout::SingleRow => {
                // Single row: no rows needed (handled in drawing code).
            }
            VertLayout::SingleCol => {
                // CPU, Memory, GPU, Network each on its own line.
                let _line_h = f(12);
                rows.push(RowLayout {
                    label_rect: (pad_x, ty, card_w - pad_x * 2, rh),
                    value_rect: (pad_x, ty, card_w - pad_x * 2, rh),
                    track_rect: (0, 0, 0, 0),
                    fill_width: 0,
                });
            }
            VertLayout::SplitCol => {
                // Two columns layout.
                rows.push(RowLayout {
                    label_rect: (pad_x, f(8), card_w - pad_x * 2, rh),
                    value_rect: (pad_x, f(24), card_w - pad_x * 2, rh),
                    track_rect: (0, 0, 0, 0),
                    fill_width: 0,
                });
            }
        }
    } else {
        // Regular mode: full layout with progress bars.
        let bar_w = card_w - pad_x * 2;

        // CPU row.
        let y = f(12) + lh + f(9);
        rows.push(RowLayout {
            label_rect: (pad_x, y, bar_w / 2, row_h),
            value_rect: (card_w - pad_x, y, bar_w / 2, row_h),
            track_rect: (pad_x, y + row_h + f(3), bar_w, bh),
            fill_width: 0, // Set by caller based on metric value.
        });

        // Memory row.
        let y = y + row_h + f(3) + bh + f(9);
        rows.push(RowLayout {
            label_rect: (pad_x, y, bar_w / 2, row_h),
            value_rect: (card_w - pad_x, y, bar_w / 2, row_h),
            track_rect: (pad_x, y + row_h + f(3), bar_w, bh),
            fill_width: 0,
        });

        // GPU row.
        let y = y + row_h + f(3) + bh + f(9);
        rows.push(RowLayout {
            label_rect: (pad_x, y, bar_w / 2, row_h),
            value_rect: (card_w - pad_x, y, bar_w / 2, row_h),
            track_rect: (pad_x, y + row_h + f(3), bar_w, bh),
            fill_width: 0,
        });

        // Network row (no progress bar, but keeps the structure).
        let y = y + row_h + f(3) + bh + f(9);
        rows.push(RowLayout {
            label_rect: (pad_x, y, bar_w / 2, row_h),
            value_rect: (card_w - pad_x, y, bar_w / 2, row_h),
            track_rect: (0, 0, 0, 0),
            fill_width: 0,
        });
    }

    CardLayout {
        width: card_w,
        height: card_h,
        rows,
    }
}

/// Calculate fill width percentage for a progress bar.
pub fn fill_pct(value: f32) -> i32 {
    value.clamp(0.0, 100.0) as i32
}

/// Scale a logical coordinate to screen coordinates based on DPI and zoom.
pub fn scale(logical: i32, dpi: u32, zoom: Zoom) -> i32 {
    let scale = dpi as f64 / 96.0;
    let zoom_factor = zoom.as_factor();
    (logical as f64 * scale * zoom_factor + 0.5) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zoom_factors() {
        assert_eq!(Zoom::Zoom75.as_factor(), 0.75);
        assert_eq!(Zoom::Zoom100.as_factor(), 1.0);
        assert_eq!(Zoom::Zoom125.as_factor(), 1.25);
        assert_eq!(Zoom::Zoom150.as_factor(), 1.5);
        assert_eq!(Zoom::Zoom200.as_factor(), 2.0);
    }

    #[test]
    fn test_card_dimensions_normal() {
        let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, Shape::Round);
        // At 96 DPI with 1.0 zoom, dimensions should be logical values.
        assert_eq!(layout.width, CARD_W_L);
        assert_eq!(layout.height, CARD_H_L);
    }

    #[test]
    fn test_card_dimensions_zoomed() {
        let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom200, 96, Shape::Round);
        assert_eq!(layout.width, CARD_W_L * 2);
        assert_eq!(layout.height, CARD_H_L * 2);
    }

    #[test]
    fn test_card_dimensions_minimal_single_row() {
        let layout = compute(true, VertLayout::SingleRow, Zoom::Zoom100, 96, Shape::Round);
        assert_eq!(layout.width, MIN_W_L);
        assert_eq!(layout.height, MIN_H_L);
    }

    #[test]
    fn test_card_dimensions_minimal_single_col() {
        let layout = compute(true, VertLayout::SingleCol, Zoom::Zoom100, 96, Shape::Round);
        assert_eq!(layout.width, MINV_W_L);
        assert_eq!(layout.height, MINV_H_L);
    }

    #[test]
    fn test_card_dimensions_minimal_split_col() {
        let layout = compute(true, VertLayout::SplitCol, Zoom::Zoom100, 96, Shape::Round);
        assert_eq!(layout.width, MINV2_W_L);
        assert_eq!(layout.height, MINV2_H_L);
    }

    #[test]
    fn test_card_dimensions_dpi_scaling() {
        // 192 DPI (2x scaling) should double the dimensions.
        let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 192, Shape::Round);
        assert_eq!(layout.width, CARD_W_L * 2);
        assert_eq!(layout.height, CARD_H_L * 2);
    }

    #[test]
    fn test_card_dimensions_dpi_and_zoom_combined() {
        // 192 DPI with 1.5x zoom = 3x total scaling.
        let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom150, 192, Shape::Round);
        // 268 * 2 * 1.5 = 804
        assert_eq!(layout.width, CARD_W_L * 3);
        assert_eq!(layout.height, CARD_H_L * 3);
    }

    #[test]
    fn test_fill_pct() {
        assert_eq!(fill_pct(50.0), 50);
        assert_eq!(fill_pct(0.0), 0);
        assert_eq!(fill_pct(100.0), 100);
        // Clamping
        assert_eq!(fill_pct(-10.0), 0);
        assert_eq!(fill_pct(150.0), 100);
    }

    #[test]
    fn test_scale() {
        assert_eq!(scale(100, 96, Zoom::Zoom100), 100);
        // 192 DPI doubles
        assert_eq!(scale(100, 192, Zoom::Zoom100), 200);
        // 2x zoom doubles
        assert_eq!(scale(100, 96, Zoom::Zoom200), 200);
    }

    #[test]
    fn test_rows_populated_regular_mode() {
        let layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, Shape::Round);
        // Regular mode has 4 rows (CPU, Memory, GPU, Network).
        assert_eq!(layout.rows.len(), 4);
    }

    #[test]
    fn test_rows_populated_minimal_mode() {
        let layout = compute(true, VertLayout::SingleCol, Zoom::Zoom100, 96, Shape::Round);
        // Single column minimal has 1 "row" for layout purposes.
        assert!(!layout.rows.is_empty());
    }

    #[test]
    fn test_shape_rounding() {
        // Shape doesn't affect layout computation, only rendering.
        let round_layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, Shape::Round);
        let square_layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, Shape::Square);
        let content_layout = compute(false, VertLayout::SingleRow, Zoom::Zoom100, 96, Shape::Content);

        assert_eq!(round_layout.width, square_layout.width);
        assert_eq!(round_layout.width, content_layout.width);
    }
}
