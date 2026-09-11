//! Simple 8x16 bitmap font rendering.
//!
//! This module provides a minimal bitmap font for ASCII text rendering.
//! Non-ASCII characters are rendered as a replacement character (■).
//!
//! This is a placeholder implementation for the offscreen rasterizer.
//! Phase 3 will integrate real GDI font rendering.

use std::cell::Cell;

/// 8x16 pixel bitmap for each ASCII character (16 bytes per char).
/// Each bit represents a pixel: 1 = filled, 0 = transparent.
/// Bits are organized left-to-right, top-to-bottom within each byte.
const FONT_BITMAP: [[u8; 16]; 96] = [
    // Space ' '
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '!'
    [0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00],
    // '"'
    [0x6C,0x6C,0x6C,0x6C,0x6C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '#'
    [0x6C,0x6C,0xFE,0x6C,0xFE,0x6C,0x6C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '$'
    [0x30,0x30,0xFC,0x30,0x30,0xFC,0x30,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '%'
    [0x00,0xC6,0xCC,0x18,0x30,0x66,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '&'
    [0x38,0x6C,0x38,0xFE,0xC6,0xC6,0xFE,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '''
    [0x30,0x30,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '('
    [0x18,0x30,0x30,0x30,0x30,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // ')'
    [0x30,0x18,0x18,0x18,0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '*'
    [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '+'
    [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // ','
    [0x00,0x00,0x00,0x00,0x00,0x30,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '-'
    [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '.'
    [0x00,0x00,0x00,0x00,0x00,0x00,0x30,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '/'
    [0x00,0x02,0x06,0x0C,0x18,0x30,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '0'
    [0x7C,0xC6,0xCE,0xD6,0xE6,0xC6,0x7C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '1'
    [0x30,0x70,0x30,0x30,0x30,0x30,0xFC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '2'
    [0x78,0xCC,0x0C,0x38,0x60,0xCC,0xFC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '3'
    [0x78,0xCC,0x0C,0x38,0x0C,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '4'
    [0x1C,0x3C,0x6C,0xCC,0xFE,0x0C,0x0C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '5'
    [0xFC,0xC0,0xF8,0x0C,0x0C,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '6'
    [0x38,0x60,0xC0,0xF8,0xCC,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '7'
    [0xFC,0xCC,0x0C,0x18,0x30,0x30,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '8'
    [0x78,0xCC,0xCC,0x78,0xCC,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '9'
    [0x78,0xCC,0xCC,0x7C,0x0C,0x18,0x70,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // ':'
    [0x00,0x00,0x30,0x30,0x00,0x30,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // ';'
    [0x00,0x00,0x30,0x30,0x00,0x30,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '<'
    [0x18,0x30,0x60,0xC0,0x60,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '='
    [0x00,0x00,0x7E,0x00,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '>'
    [0xC0,0x60,0x30,0x18,0x30,0x60,0xC0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '?'
    [0x78,0xCC,0x0C,0x18,0x30,0x00,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '@'
    [0x7C,0xC6,0xDE,0xDE,0xDE,0xC0,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'A'
    [0x30,0x78,0xCC,0xCC,0xFC,0xCC,0xCC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'B'
    [0xFC,0x66,0x66,0x7C,0x66,0x66,0xFC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'C'
    [0x3C,0x66,0xC0,0xC0,0xC0,0x66,0x3C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'D'
    [0xF8,0x6C,0x66,0x66,0x66,0x6C,0xF8,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'E'
    [0xFE,0x62,0x68,0x78,0x68,0x62,0xFE,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'F'
    [0xFE,0x62,0x68,0x78,0x68,0x60,0xF0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'G'
    [0x3C,0x66,0xC0,0xC0,0xCE,0x66,0x3E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'H'
    [0xCC,0xCC,0xCC,0xFC,0xCC,0xCC,0xCC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'I'
    [0x78,0x30,0x30,0x30,0x30,0x30,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'J'
    [0x1E,0x0C,0x0C,0x0C,0xCC,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'K'
    [0xE6,0x66,0x6C,0x78,0x6C,0x66,0xE6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'L'
    [0xF0,0x60,0x60,0x60,0x62,0x66,0xFE,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'M'
    [0xC6,0xEE,0xFE,0xD6,0xC6,0xC6,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'N'
    [0xC6,0xE6,0xF6,0xDE,0xCE,0xC6,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'O'
    [0x38,0x6C,0xC6,0xC6,0xC6,0x6C,0x38,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'P'
    [0xFC,0x66,0x66,0x7C,0x60,0x60,0xF0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'Q'
    [0x78,0xCC,0xCC,0xCC,0xDC,0x78,0x1C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'R'
    [0xFC,0x66,0x66,0x7C,0x6C,0x66,0xE6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'S'
    [0x78,0xCC,0xE0,0x78,0x0C,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'T'
    [0xFC,0xB4,0x30,0x30,0x30,0x30,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'U'
    [0xCC,0xCC,0xCC,0xCC,0xCC,0xCC,0xFC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'V'
    [0xCC,0xCC,0xCC,0xCC,0xCC,0x78,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'W'
    [0xC6,0xC6,0xC6,0xD6,0xD6,0xFE,0x6C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'X'
    [0xC6,0xC6,0x6C,0x38,0x6C,0xC6,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'Y'
    [0xCC,0xCC,0xCC,0x78,0x30,0x30,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'Z'
    [0xFE,0xC6,0x8C,0x18,0x32,0x66,0xFE,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '['
    [0x78,0x60,0x60,0x60,0x60,0x60,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '\'
    [0x00,0xC0,0x60,0x30,0x18,0x0C,0x06,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // ']'
    [0x1E,0x06,0x06,0x06,0x06,0x06,0x1E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '^'
    [0x18,0x3C,0x66,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '_'
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '`'
    [0x30,0x30,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'a'
    [0x00,0x00,0x78,0x0C,0x7C,0xCC,0x76,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'b'
    [0xE0,0x60,0x7C,0x66,0x66,0x66,0xFC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'c'
    [0x00,0x00,0x78,0xCC,0xC0,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'd'
    [0x1C,0x0C,0x3C,0x6C,0xCC,0xCC,0x76,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'e'
    [0x00,0x00,0x78,0xCC,0xFC,0xC0,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'f'
    [0x38,0x60,0xFC,0x60,0x60,0x60,0xF0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'g'
    [0x00,0x00,0x76,0xCC,0xCC,0x7C,0x0C,0xF8,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'h'
    [0xE0,0x60,0x6C,0x76,0x66,0x66,0xE6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'i'
    [0x30,0x00,0x70,0x30,0x30,0x30,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'j'
    [0x0C,0x00,0x1C,0x0C,0x0C,0xCC,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'k'
    [0xE0,0x60,0x66,0x6C,0x78,0x6C,0xE6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'l'
    [0x78,0x30,0x30,0x30,0x30,0x30,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'm'
    [0x00,0x00,0xCC,0xFE,0xD6,0xC6,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'n'
    [0x00,0x00,0xF8,0xCC,0xCC,0xCC,0xCC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'o'
    [0x00,0x00,0x78,0xCC,0xCC,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'p'
    [0x00,0x00,0xDC,0x66,0x66,0x7C,0x60,0xF0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'q'
    [0x00,0x00,0x76,0xCC,0xCC,0x7C,0x0C,0x1E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'r'
    [0x00,0x00,0xEC,0x6E,0x60,0x60,0xF0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 's'
    [0x00,0x00,0x78,0xCC,0x60,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 't'
    [0x00,0x00,0xCC,0xCC,0xCC,0xCC,0x78,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'u'
    [0x00,0x00,0xCC,0xCC,0xCC,0xCC,0x76,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'v'
    [0x00,0x00,0xCC,0xCC,0xCC,0x78,0x30,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'w'
    [0x00,0x00,0xC6,0xC6,0xD6,0xD6,0x6C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'x'
    [0x00,0x00,0xC6,0x6C,0x38,0x6C,0xC6,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'y'
    [0x00,0x00,0xC6,0xC6,0xC6,0x7E,0x0C,0xF8,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // 'z'
    [0x00,0x00,0xFC,0xC6,0x8C,0x18,0xFC,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '{'
    [0x1C,0x30,0x30,0xE0,0x30,0x30,0x1C,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '|'
    [0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '}'
    [0xE0,0x30,0x30,0x1C,0x30,0x30,0xE0,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // '~'
    [0x00,0x00,0x70,0x18,0x30,0x60,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    // DEL (0x7F)
    [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
];

/// Replacement character (■) bitmap for non-ASCII characters.
const REPLACEMENT_CHAR: [u8; 16] = [
    0x7E,0x81,0x81,0x81,0x81,0x81,0x81,0x7E,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
];

/// 自定义字形（位图字体只覆盖 ASCII，这里补充网速箭头）。
/// ↑ U+2191（上行）：
const GLYPH_UP: [u8; 16] = [
    0x18,0x3C,0x7E,0xFF,0x18,0x18,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
];
/// ↓ U+2193（下行）：
const GLYPH_DOWN: [u8; 16] = [
    0x00,0x18,0x18,0x18,0x18,0xFF,0x7E,0x3C,0x18,0x00,0x00,0x00,0x00,0x00,0x00,0x00,
];

/// Width of the bitmap font in pixels.
pub const FONT_WIDTH: i32 = 8;

/// Height of the bitmap font in pixels.
pub const FONT_HEIGHT: i32 = 16;

/// Character spacing (horizontal gap between characters).
pub const FONT_SPACING: i32 = 1;

/// 字形实际可见行数。
///
/// 位图字体把 7~8 行高的字形塞进 16 行单元（单元下半部为空白），
/// 因此行高/行距必须以可见行数为准。`render_char_scaled_f` 从 `y` 起逐行绘制，
/// 可见墨迹落在 row 0..=`GLYPH_VISIBLE_ROWS`-1，单元余下行全空。若按
/// `FONT_HEIGHT`(16) 预留行高，纵向会多出近一半死区——正是"间距为 0 时竖版
/// 仍有较大间隙"的根因。
pub const GLYPH_VISIBLE_ROWS: i32 = 8;
pub const GLYPH_VISIBLE_ROWS_F: f32 = GLYPH_VISIBLE_ROWS as f32;

/// 按字重覆盖的 8×16 字形表（win 层用 DWrite 栅格化 DIN 三版注入）。
///
/// 三张常驻 `[u8;16] × 96`（细/中/粗），`thread_local`：注入（`set_weight_glyph`，
/// 由 win 层 `din_bitmap::ensure_loaded` 在首帧前调用）与读取（`get_char_bitmap`）
/// 都发生在**同一个 UI 渲染线程**，因此零锁、无需原子、无 `unsafe`。
/// 未注入的槽位保持内置 [`FONT_BITMAP`]（原样），任何失败都不影响渲染。
///
/// # 锁无关 / 性能
///
/// 旧版用全局 `Mutex<HashMap<(weight,idx), [u8;16]>>`，每画一个字符都要加锁查表。
/// 本版在渲染热路径上是**纯数组读**：thread_local 直接下标，无锁、无哈希查找。
/// 三张字重表初始化为内置点阵（未注入槽位回退 [`FONT_BITMAP`]）；
/// DIN 三档栅格化会覆盖对应槽位。
const fn weight_tables_init() -> [[[u8; 16]; 96]; 3] {
    let mut t = [[[0u8; 16]; 96]; 3];
    let mut w = 0;
    while w < 3 {
        let mut i = 0;
        while i < 96 {
            t[w][i] = FONT_BITMAP[i];
            i += 1;
        }
        w += 1;
    }
    t
}
thread_local! {
    static FONT_WEIGHT_TABLES: RefCell<[[[u8; 16]; 96]; 3]> = RefCell::new(weight_tables_init());
}

// 当前渲染生效的字重（0 细 / 1 中 / 2 粗）。默认粗 = 历史原样观感；
// 由 win 层在性能模式整帧渲染前调用一次，正常工作流零读取开销（thread_local）。
thread_local! {
    static ACTIVE_WEIGHT: Cell<u8> = const { Cell::new(2) };
}

/// 设置当前帧生效的字重（win 层每帧调用一次）。
///
/// 只写 thread_local，不锁全局；渲染期由 `get_char_bitmap` 零锁读取。
pub fn set_active_weight(weight: u8) {
    ACTIVE_WEIGHT.with(|c| c.set(weight));
}

/// 安装某字重的 ASCII 字符 8×16 字形（win 层 DWrite 栅格化 DIN 三版后调用）。
///
/// 直接写进对应字重的常驻表，无锁、无 `unsafe`（与读取同线程）。
pub fn set_weight_glyph(weight: u8, ascii_idx: usize, table: [u8; 16]) {
    if weight < 3 && ascii_idx < 96 {
        FONT_WEIGHT_TABLES.with(|t| t.borrow_mut()[weight as usize][ascii_idx] = table);
    }
}

/// 当前字重下该字符的 8×16 点阵（16B 拷贝；无 `unsafe`、零锁）。
///
/// 直接读三张常驻表（细/中/粗，未注入即内置点阵），箭头/替代字回退各自静态表。
#[inline]
pub fn get_char_bitmap(c: char) -> [u8; 16] {
    match c {
        '\u{2191}' => GLYPH_UP,   // ↑
        '\u{2193}' => GLYPH_DOWN, // ↓
        _ if (c as u32) >= 0x20 && (c as u32) <= 0x7E => {
            let idx = (c as usize) - 0x20;
            let w = ACTIVE_WEIGHT.with(|x| x.get()) as usize;
            FONT_WEIGHT_TABLES.with(|t| t.borrow()[if w < 3 { w } else { 2 }][idx])
        }
        _ => REPLACEMENT_CHAR,
    }
}

/// Renders a character into a BGRA (pre-multiplied) pixel buffer.
///
/// # 为什么不再 `unsafe`
///
/// 旧签名是 `unsafe fn render_char(buf: *mut u8, buf_stride: i32, ...)`，
/// 把边界检查的责任推给调用方 —— 但 `ui` crate 的硬约束是**零 `unsafe`**
/// （`unsafe` 只允许出现在 `win` 平台层）。
/// 改成吃 `&mut [u8]` + 表面尺寸后，越界由切片自身保证，
/// 调用方（`SoftwareSurface`）也跟着变成全安全代码。
///
/// # 顺带修掉的一个 bug
///
/// 旧版有一行 `let dst_a = *pixel_ptr;` 并注释 "alpha at offset 3" ——
/// 实际读到的是 offset 0（B 通道）。幸而该变量未被使用，
/// 这里直接改为正确的 over 合成：`out = src + dst * (1 - src_a)`。
///
/// # Arguments
///
/// * `pixels` - 目标 BGRA 预乘缓冲。
/// * `surface_w` / `surface_h` - 目标表面尺寸（像素）。
/// * `c` - 要绘制的字符（非 ASCII 走替代字形 ■）。
/// * `x` / `y` - 左上角位置；越界部分自动裁剪（不 panic）。
/// * `fg_color` - 前景色（预乘 alpha）。
pub fn render_char(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    c: char,
    x: i32,
    y: i32,
    fg_color: crate::render::Rgba,
) {
    if surface_w <= 0 || surface_h <= 0 || fg_color.a == 0 {
        return;
    }

    let bitmap = get_char_bitmap(c);
    let factor = 1.0 - fg_color.alpha_float();

    for row in 0..FONT_HEIGHT {
        let py = y + row;
        if py < 0 || py >= surface_h {
            continue;
        }
        let bits = bitmap[row as usize];
        if bits == 0 {
            continue;
        }

        for col in 0..FONT_WIDTH {
            if bits & (1 << (7 - col)) == 0 {
                continue;
            }
            let px = x + col;
            if px < 0 || px >= surface_w {
                continue;
            }

            let idx = ((py * surface_w + px) * 4) as usize;
            if idx + 3 >= pixels.len() {
                continue;
            }

            let dst_b = pixels[idx] as f32;
            let dst_g = pixels[idx + 1] as f32;
            let dst_r = pixels[idx + 2] as f32;
            let dst_a = pixels[idx + 3] as f32;

            // 预乘 alpha 的 over 合成：out = src + dst * (1 - src_a)
            let out_r = fg_color.r as f32 + dst_r * factor;
            let out_g = fg_color.g as f32 + dst_g * factor;
            let out_b = fg_color.b as f32 + dst_b * factor;
            let out_a = (fg_color.a as f32 + dst_a * factor).min(255.0);

            pixels[idx] = out_b as u8;     // B
            pixels[idx + 1] = out_g as u8; // G
            pixels[idx + 2] = out_r as u8; // R
            pixels[idx + 3] = out_a as u8; // A
        }
    }
}

/// 位图字体的字符串宽度（像素）。
///
/// 与 [`render_char`] 的排布保持一致：`n * (W + SPACING) - SPACING`。
pub fn measure(s: &str) -> i32 {
    let n = s.chars().count() as i32;
    (n * (FONT_WIDTH + FONT_SPACING) - FONT_SPACING).max(0)
}

/// 整数倍缩放下的字符串宽度（像素）。
///
/// 每个字形像素放大为 `scale × scale` 块，字距同步放大：
/// `(n * (W + SPACING) - SPACING) * scale`。
/// 横向条的对齐稳定性依赖本函数：槽位宽 = 模板串宽 × scale，
/// 与实际内容无关，数值位数变化不会位移后续元素。
pub fn measure_scaled(s: &str, scale: u32) -> i32 {
    measure(s) * scale.max(1) as i32
}

/// 渲染一个字符到 BGRA 预乘缓冲，字形像素放大为 `scale × scale` 实心块。
///
/// 逻辑与 [`render_char`] 一致（预乘 over 合成），仅增加了块复制。
/// 越界部分自动裁剪，不 panic。
pub fn render_char_scaled(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    c: char,
    x: i32,
    y: i32,
    scale: u32,
    fg_color: crate::render::Rgba,
) {
    if surface_w <= 0 || surface_h <= 0 || fg_color.a == 0 {
        return;
    }
    // P3 快路径：**不透明**前景（正文/阴影/描边用的都是不透明色）时，
    // over 合成退化为「直接写 4 字节」——省掉读-改-写和全部 f32 运算。
    // 语义与慢路径严格等价（src_a=1 → out = src，out_a = 255），见单测。
    if fg_color.a == 255 {
        render_char_scaled_opaque(
            pixels,
            surface_w,
            surface_h,
            c,
            x,
            y,
            scale.max(1) as i32,
            fg_color,
        );
    } else {
        render_char_scaled_blend(pixels, surface_w, surface_h, c, x, y, scale, fg_color);
    }
}

/// 不透明快路径：直接写 BGRA，裁剪在块级别外提（不再逐像素判越界）。
fn render_char_scaled_opaque(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    c: char,
    x: i32,
    y: i32,
    scale: i32,
    fg_color: crate::render::Rgba,
) {
    let bitmap = get_char_bitmap(c);
    let (b, g, r) = (fg_color.b, fg_color.g, fg_color.r);

    for row in 0..FONT_HEIGHT {
        let bits = bitmap[row as usize];
        if bits == 0 {
            continue;
        }
        let py0 = y + row * scale;
        if py0 + scale <= 0 || py0 >= surface_h {
            continue; // 整块在画布外
        }
        let dy0 = if py0 < 0 { -py0 } else { 0 };
        let dy1 = (surface_h - py0).min(scale);

        for col in 0..FONT_WIDTH {
            if bits & (1 << (7 - col)) == 0 {
                continue;
            }
            let px0 = x + col * scale;
            if px0 + scale <= 0 || px0 >= surface_w {
                continue;
            }
            let dx0 = if px0 < 0 { -px0 } else { 0 };
            let dx1 = (surface_w - px0).min(scale);

            for dy in dy0..dy1 {
                let row_off = ((py0 + dy) * surface_w) as usize * 4;
                for dx in dx0..dx1 {
                    let idx = row_off + ((px0 + dx) as usize) * 4;
                    if idx + 3 >= pixels.len() {
                        continue;
                    }
                    pixels[idx] = b;
                    pixels[idx + 1] = g;
                    pixels[idx + 2] = r;
                    pixels[idx + 3] = 255;
                }
            }
        }
    }
}

/// 半透明路径：保留原始 over 合成（预乘 alpha），逐像素混合。
///
/// 我们的 UI 目前不用半透明字，但它是快路径的**语义基准**，
/// 单测逐像素比对两条路径，防止快路径日后被改歪。
fn render_char_scaled_blend(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    c: char,
    x: i32,
    y: i32,
    scale: u32,
    fg_color: crate::render::Rgba,
) {
    let scale = scale.max(1) as i32;
    if surface_w <= 0 || surface_h <= 0 || fg_color.a == 0 {
        return;
    }

    let bitmap = get_char_bitmap(c);
    let factor = 1.0 - fg_color.alpha_float();

    for row in 0..FONT_HEIGHT {
        let py0 = y + row * scale;
        let bits = bitmap[row as usize];
        if bits == 0 {
            continue;
        }
        for col in 0..FONT_WIDTH {
            if bits & (1 << (7 - col)) == 0 {
                continue;
            }
            let px0 = x + col * scale;
            // scale × scale 实心块
            for dy in 0..scale {
                let py = py0 + dy;
                if py < 0 || py >= surface_h {
                    continue;
                }
                for dx in 0..scale {
                    let px = px0 + dx;
                    if px < 0 || px >= surface_w {
                        continue;
                    }
                    let idx = ((py * surface_w + px) * 4) as usize;
                    if idx + 3 >= pixels.len() {
                        continue;
                    }
                    let dst_r = pixels[idx + 2] as f32;
                    let dst_g = pixels[idx + 1] as f32;
                    let dst_b = pixels[idx] as f32;
                    let dst_a = pixels[idx + 3] as f32;
                    let out_r = fg_color.r as f32 + dst_r * factor;
                    let out_g = fg_color.g as f32 + dst_g * factor;
                    let out_b = fg_color.b as f32 + dst_b * factor;
                    let out_a = (fg_color.a as f32 + dst_a * factor).min(255.0);
                    pixels[idx] = out_b as u8;
                    pixels[idx + 1] = out_g as u8;
                    pixels[idx + 2] = out_r as u8;
                    pixels[idx + 3] = out_a as u8;
                }
            }
        }
    }
}

/// 按缩放渲染一整串字符（字符间距同步放大）。
pub fn render_str_scaled(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    s: &str,
    x: i32,
    y: i32,
    scale: u32,
    fg_color: crate::render::Rgba,
) {
    let mut cursor = x;
    let step = (FONT_WIDTH + FONT_SPACING) * scale.max(1) as i32;
    for c in s.chars() {
        render_char_scaled(pixels, surface_w, surface_h, c, cursor, y, scale, fg_color);
        cursor += step;
    }
}

// ── v4 浮点字号：0.5 步进（8.0..=40.0 px） ─────────────────

/// 把配置字号（px，基准字形高 16px）换算为渲染缩放比。
///
/// 位图字形 8×16 是 "1.0 倍" 的基准：字号 16px ↔ scale 1.0。
/// 字号允许 1~999，无上下限。
pub fn scale_of_font(font_size: f32) -> f32 {
    (font_size.max(1.0) / FONT_HEIGHT as f32).max(0.5)
}

/// 浮点缩放下的字符串宽度（像素，向上取整）。
///
/// 与 [`render_str_scaled_f`] 的排布一致（最近邻放大，字符间距同比例）。
pub fn measure_scaled_f(s: &str, scale: f32) -> i32 {
    let n = s.chars().count() as f32;
    let w = (n * (FONT_WIDTH as f32 + FONT_SPACING as f32) - FONT_SPACING as f32) * scale;
    (w.ceil() as i32).max(0)
}

/// 浮点缩放：把一个字形像素放大为 `scale × scale` 的最近邻块。
///
/// 与整数版 [`render_char_scaled`] 的区别仅在块边界用浮点计算：
/// 字形源像素 (row, col) 覆盖目标区间
/// `[x + col*scale, x + (col+1)*scale) × [y + row*scale, y + (row+1)*scale)`，
/// 块内全部像素取同一个源像素（最近邻）。越界自动裁剪，不 panic。
pub fn render_char_scaled_f(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    c: char,
    x: i32,
    y: i32,
    scale: f32,
    fg_color: crate::render::Rgba,
) {
    if surface_w <= 0 || surface_h <= 0 || fg_color.a == 0 || scale <= 0.0 {
        return;
    }
    // B1/B3/B4：不透明字形走「预渲染字形图集」快路径——
    // 字形在 (char, scale, fg) 首次出现时只栅格化一次，之后每帧只做带裁剪的整块
    // blit，不再逐像素解位图 / 做 f32 运算。不透明（a==255）时掩码 blit 与 over
    // 合成逐像素等价（只覆盖不透明像素，透明像素跳过，不破坏目标已存在内容）。
    if fg_color.a == 255 {
        let tile = get_glyph_tile(c, scale, fg_color);
        blit_tile_masked(pixels, surface_w, surface_h, &tile, x, y);
    } else {
        render_char_scaled_f_blend(pixels, surface_w, surface_h, c, x, y, scale, fg_color);
    }
}

/// 浮点缩放的逐像素 over 合成路径——作为不透明图集快路径的**语义基准**。
///
/// 覆盖窗口的字形集合很小且稳定，UI 正文/阴影/描边用的都是不透明色，
/// 所以真正进这条冷路径的只有半透明字（罕见）。它与图集快路径在 a<255 时
/// 严格等价，是防快路径日后被改歪的护栏。
fn render_char_scaled_f_blend(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    c: char,
    x: i32,
    y: i32,
    scale: f32,
    fg_color: crate::render::Rgba,
) {
    if surface_w <= 0 || surface_h <= 0 || fg_color.a == 0 || scale <= 0.0 {
        return;
    }

    let bitmap = get_char_bitmap(c);
    let factor = 1.0 - fg_color.alpha_float();

    for row in 0..FONT_HEIGHT as usize {
        let bits = bitmap[row];
        if bits == 0 {
            continue;
        }
        let y0 = y as f32 + row as f32 * scale;
        let y1 = y0 + scale;
        let py0 = y0.floor() as i32;
        let py1 = (y1.ceil() as i32).max(py0 + 1);
        if py1 <= 0 || py0 >= surface_h {
            continue; // 整行在画布外
        }
        let dy0 = if py0 < 0 { -py0 } else { 0 };
        let dy1 = (surface_h - py0).min(py1 - py0);

        for col in 0..FONT_WIDTH as usize {
            if bits & (1 << (7 - col)) == 0 {
                continue;
            }
            let x0 = x as f32 + col as f32 * scale;
            let x1 = x0 + scale;
            let px0 = x0.floor() as i32;
            let px1 = (x1.ceil() as i32).max(px0 + 1);
            if px1 <= 0 || px0 >= surface_w {
                continue;
            }
            let dx0 = if px0 < 0 { -px0 } else { 0 };
            let dx1 = (surface_w - px0).min(px1 - px0);

            for dy in dy0..dy1 {
                let row_off = ((py0 + dy) * surface_w) as usize * 4;
                for dx in dx0..dx1 {
                    let idx = row_off + ((px0 + dx) as usize) * 4;
                    if idx + 3 >= pixels.len() {
                        continue;
                    }
                    // 半透明：预乘 over 合成
                    let dst_r = pixels[idx + 2] as f32;
                    let dst_g = pixels[idx + 1] as f32;
                    let dst_b = pixels[idx] as f32;
                    let dst_a = pixels[idx + 3] as f32;
                    pixels[idx] = (fg_color.b as f32 + dst_b * factor) as u8;
                    pixels[idx + 1] = (fg_color.g as f32 + dst_g * factor) as u8;
                    pixels[idx + 2] = (fg_color.r as f32 + dst_r * factor) as u8;
                    pixels[idx + 3] = (fg_color.a as f32 + dst_a * factor).min(255.0) as u8;
                }
            }
        }
    }
}

/// 浮点缩放渲染一整串字符（字符间距同比例放大）。
pub fn render_str_scaled_f(
    pixels: &mut [u8],
    surface_w: i32,
    surface_h: i32,
    s: &str,
    x: i32,
    y: i32,
    scale: f32,
    fg_color: crate::render::Rgba,
) {
    let step = (FONT_WIDTH as f32 + FONT_SPACING as f32) * scale;
    let mut cursor = x as f32;
    for c in s.chars() {
        render_char_scaled_f(pixels, surface_w, surface_h, c, cursor as i32, y, scale, fg_color);
        cursor += step;
    }
}

// ── 字形图集（B1/B3/B4） ──────────────────────────────────
//
// 把每个 (char, scale, fg) 的栅格化结果缓存进线程局部图集，后续同字形
// 只做整块 blit。覆盖窗口的字形集合很小且稳定（数字/字母/单位符号），
// 命中率接近 100%，每帧省掉全量逐像素栅格化。
// 阴影/描边各自用 `shade` 不透明色，因此同一套图集天然覆盖它们（B2）。

use std::cell::RefCell;
use std::rc::Rc;

/// 图集瓦片：一个字形放大后的 BGRA 预乘像素块（透明背景）。
struct GlyphTile {
    w: i32,
    h: i32,
    px: Vec<u8>,
}

/// 图集键：字符 + 缩放（精确 f32 bits）+ 字重 + 预乘前景色。
/// 字重进键：不同字重的字形点阵不同，共键会串色。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TileKey {
    ch: u32,
    scale_bits: u32,
    w: u8,
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

/// 图集容量：常见字形集合（~40 字符 × 1–2 档字号 × 2–4 色）约 120 条，取 256 留余量。
/// 固定数组 + 开放寻址，零堆分配、零 HashMap 运行时（exe 体积敏感）。
/// 极端配置写满时覆盖探测起点槽——只回退为「重建该字形」，不影响正确性。
const ATLAS_CAP: usize = 256;

struct AtlasSlot {
    key: TileKey,
    tile: Rc<GlyphTile>,
}

thread_local! {
    static GLYPH_ATLAS: RefCell<[Option<AtlasSlot>; ATLAS_CAP]> =
        RefCell::new(std::array::from_fn(|_| None));
}

/// 图集键散列（FNV-1a，无分配）。
fn atlas_hash(key: &TileKey) -> usize {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let rgba = (key.b as u64)
        | ((key.g as u64) << 8)
        | ((key.r as u64) << 16)
        | ((key.a as u64) << 24);
    for x in [key.ch as u64, key.scale_bits as u64, key.w as u64, rgba] {
        h ^= x;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    (h >> 8) as usize
}

/// 取（建或复用）一个字形瓦片。
fn get_glyph_tile(c: char, scale: f32, fg: crate::render::Rgba) -> Rc<GlyphTile> {
    let w = ACTIVE_WEIGHT.with(|x| x.get());
    let key = TileKey {
        ch: c as u32,
        scale_bits: scale.to_bits(),
        w,
        b: fg.b,
        g: fg.g,
        r: fg.r,
        a: fg.a,
    };
    let h = atlas_hash(&key);
    GLYPH_ATLAS.with(|m| {
        let mut slots = m.borrow_mut();
        for probe in 0..ATLAS_CAP {
            let idx = (h.wrapping_add(probe)) % ATLAS_CAP;
            if let Some(s) = &slots[idx] {
                if s.key == key {
                    return Rc::clone(&s.tile);
                }
                continue; // 占位冲突，继续探查
            }
            // 空槽：构建并驻留
            let tw = (FONT_WIDTH as f32 * scale).ceil() as i32;
            let th = (FONT_HEIGHT as f32 * scale).ceil() as i32;
            let mut px = vec![0u8; (tw * th * 4).max(0) as usize];
            if tw > 0 && th > 0 {
                // 在透明缓冲里用通用栅格器画一次字形（over 合成 over 0 == 直接预乘）
                render_char_scaled_f_blend(&mut px, tw, th, c, 0, 0, scale, fg);
            }
            let tile = Rc::new(GlyphTile { w: tw, h: th, px });
            slots[idx] = Some(AtlasSlot {
                key,
                tile: Rc::clone(&tile),
            });
            return tile;
        }
        // 全满（不应发生）：覆盖探测起点，重建该字形
        let tw = (FONT_WIDTH as f32 * scale).ceil() as i32;
        let th = (FONT_HEIGHT as f32 * scale).ceil() as i32;
        let mut px = vec![0u8; (tw * th * 4).max(0) as usize];
        if tw > 0 && th > 0 {
            render_char_scaled_f_blend(&mut px, tw, th, c, 0, 0, scale, fg);
        }
        let tile = Rc::new(GlyphTile { w: tw, h: th, px });
        slots[h % ATLAS_CAP] = Some(AtlasSlot {
            key,
            tile: Rc::clone(&tile),
        });
        tile
    })
}

/// 带裁剪的掩码 blit：只写瓦片中不透明（a!=0）的像素，透明像素跳过
/// （不破坏目标已存在的像素，等价于 over 合成）。行级整块拷贝 + 4 字节块写入（B3/B4）。
#[inline]
fn blit_tile_masked(dst: &mut [u8], sw: i32, sh: i32, tile: &GlyphTile, dx: i32, dy: i32) {
    let tw = tile.w;
    let th = tile.h;
    if tw <= 0 || th <= 0 {
        return;
    }
    let y0 = dy.max(0);
    let y1 = (dy + th).min(sh);
    if y1 <= y0 {
        return;
    }
    let x0 = dx.max(0);
    let x1 = (dx + tw).min(sw);
    if x1 <= x0 {
        return;
    }
    let tx0 = (x0 - dx) as usize;
    let tx1 = (x1 - dx) as usize;
    for yy in y0..y1 {
        let ty = (yy - dy) as usize;
        let s_row = (ty * tw as usize + tx0) * 4;
        let d_row = ((yy * sw + x0) as usize) * 4;
        for tx in tx0..tx1 {
            let off = (tx - tx0) * 4;
            let si = s_row + off;
            if tile.px[si + 3] == 0 {
                continue; // 透明：跳过（掩码）
            }
            let di = d_row + off;
            // B4：4 字节整块写（编译器通常落为单条 u32 store）
            dst[di..di + 4].copy_from_slice(&tile.px[si..si + 4]);
        }
    }
}

#[cfg(test)]
mod perf_path_tests {
    use super::*;
    use crate::render::Rgba;

    const W: i32 = 64;
    const H: i32 = 24;

    fn render_with(pixels: &mut [u8], c: char, scale: u32, color: Rgba, x: i32, y: i32) {
        render_char_scaled(pixels, W, H, c, x, y, scale, color);
    }

    /// 快路径（不透明直写）必须与 over 合成慢路径**逐像素一致**。
    ///
    /// 这条测试是 P3 优化的护栏：若哪天快路径被改歪（忘了预乘、
    /// 写错通道顺序、裁剪边界差一像素），这里会立刻红。
    #[test]
    fn opaque_fast_path_matches_blend_path() {
        let cases: [(char, u32, i32, i32); 12] = [
            ('A', 1, 0, 0),
            ('A', 2, 3, 2),
            ('g', 3, 5, 1),
            ('%', 4, 1, 0),
            ('0', 2, 7, 3),
            (' ', 2, 0, 0),
            ('\u{2191}', 3, 2, 2),
            ('\u{2193}', 4, 0, 1),
            // 越界：左/上越界、右/下越界、完全越界
            ('W', 3, -12, -6),
            ('W', 3, 50, 18),
            ('M', 4, -20, -20),
            ('M', 2, 70, 30),
        ];

        for (c, scale, x, y) in cases {
            let color = Rgba::opaque(200, 100, 50);
            let mut fast = vec![0u8; (W * H * 4) as usize];
            let mut slow = vec![0u8; (W * H * 4) as usize];

            render_with(&mut fast, c, scale, color, x, y);
            // 慢路径绕过 a==255 的快分支，直接调合成实现
            render_char_scaled_blend(&mut slow, W, H, c, x, y, scale, color);

            assert_eq!(
                fast, slow,
                "快/慢路径不一致: char={:?} scale={} at({},{})",
                c, scale, x, y
            );
        }
    }

    /// 快路径必须真的画出了东西（防止写成空实现而测试仍“通过”）。
    #[test]
    fn opaque_fast_path_actually_draws() {
        let mut px = vec![0u8; (W * H * 4) as usize];
        render_with(&mut px, 'A', 2, Rgba::opaque(10, 20, 30), 2, 2);
        let non_zero = px.iter().filter(|&&v| v != 0).count();
        assert!(non_zero > 0, "快路径什么都没画");
        // 不透明绘制：alpha 通道必须全 255（被覆盖处）
        let opaque_px = px.chunks_exact(4).filter(|c| c[3] == 255).count();
        assert!(opaque_px > 0, "不透明像素的 alpha 必须为 255");
    }

    /// 半透明前景仍走合成路径，结果与底色有关（快路径不得介入）。
    #[test]
    fn translucent_uses_blend_path() {
        let mut px = vec![0u8; (W * H * 4) as usize];
        let semi = Rgba::new(255, 255, 255, 128);
        render_with(&mut px, 'A', 1, semi, 0, 0);
        // 半透明下 A 通道应为 128 量级，而不是快路径的 255
        let has_semi = px.chunks_exact(4).any(|c| c[3] != 0 && c[3] != 255);
        assert!(has_semi, "半透明应走合成路径");
    }
}
