//! 性能（位图）引擎的三档字重：用 DWrite 把 DIN 三版栅格化成 8×16 位图表，
//! 注入 `ui::font`（[`sysoverlay_ui::font::set_weight_glyph`]）。
//!
//! # 为什么本模块存在
//!
//! 性能引擎的软渲染器是纯软件位图字体（`ui` crate 零 Win32），字形源是内置
//! 8×16 点阵。历史上"细/中/粗"只对阴影做 alpha 调制，主字形三档完全一样。
//! 这里用 DWrite（平台层才能访问）把 DIN 三版字重分别栅格化到 8×16 点阵，
//! 替换软渲染器的字形源 → 三档是**真实 DIN 字形粗细差异**，而软件渲染/图集/
//! 布局/脏区路径原样不变，零性能回归。
//!
//! # 内存与性能
//!
//! - 进程启动栅格化一次（≤ 3 档 × 95 字符），静默缓存，后续零成本。
//! - 每份点阵仅 16B，总量 ≈ 4.6KB，可忽略。
//! - 任何一步失败（无 DIN 文件 / DWrite 不可用 / 字符缺字形）→ 跳过该字重的
//!   注入，软渲染器回退内置点阵，**绝不影响启动与渲染**。

use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use windows::core::{Interface as _, PCWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFactory5, IDWriteFontFace, IDWriteFontFile,
    IDWriteGlyphRunAnalysis, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_FACE_TYPE_CFF, DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN, DWRITE_MATRIX,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_RENDERING_MODE_NATURAL, DWRITE_TEXTURE_TYPE,
};

/// 点阵格（与 `ui::font::FONT_WIDTH/GLYPH_VISIBLE_ROWS` 对齐：
/// 字形墨迹只放进顶部 8 行，布局按 8 行预留，避免纵向下压到下一指标）。
const CELL_W: f32 = 8.0;
const CELL_H: f32 = 8.0;

fn factory() -> Option<&'static IDWriteFactory> {
    static F: OnceLock<Option<IDWriteFactory>> = OnceLock::new();
    F.get_or_init(|| unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok() })
        .as_ref()
}

/// 加载指定字重的 DIN 字体（用 DWrite 私有字体集，内存态、不污染系统字体）。
fn face_for(weight: u8) -> Option<IDWriteFontFace> {
    let dwrite = factory()?;
    let path = crate::render_d2d::find_din_font_path(weight)?;
    let factory5: IDWriteFactory5 = dwrite.cast().ok()?;
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let file: IDWriteFontFile =
        unsafe { factory5.CreateFontFileReference(PCWSTR(wide.as_ptr()), None) }.ok()?;
    unsafe {
        factory5
            .CreateFontFace(
                DWRITE_FONT_FACE_TYPE_CFF,
                std::slice::from_ref(&Some(file)),
                0,
                Default::default(),
            )
            .ok()
    }
}

/// 把单个字符栅格化成 8×16 点阵（`[u8;16]`，行字节高 7−col 位）。
///
/// 两趟分析：先无变换取墨迹包围盒 → 求等比缩放到 8×8 的 scale → 带平移/缩放变换
/// 再分析 → 读灰度 alpha，阈值化位图。字符无墨迹（空格）或缺字形返回 None。
fn rasterize_char(face: &IDWriteFontFace, c: u32) -> Option<[u8; 16]> {
    let dwrite = factory()?;
    let mut glyph = [0u16; 1];
    unsafe { face.GetGlyphIndices(&c, 1, glyph.as_mut_ptr()) }.ok()?;
    if glyph[0] == 0 {
        return None; // 字形缺失（含 DIN 无此字符）→ 回退内置点阵
    }

    let run = |face: &IDWriteFontFace, transform: Option<&DWRITE_MATRIX>,
               origin_y: f32| -> Option<(IDWriteGlyphRunAnalysis, RECT)> {
        let gr = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(face.clone())),
            fontEmSize: 16.0,
            glyphCount: 1,
            glyphIndices: glyph.as_ptr(),
            glyphAdvances: std::ptr::null(),
            glyphOffsets: std::ptr::null::<DWRITE_GLYPH_OFFSET>(),
            isSideways: false.into(),
            bidiLevel: 0,
        };
        unsafe {
            let analysis = dwrite
                .CreateGlyphRunAnalysis(
                    &gr,
                    1.0,
                    transform.map(|m| m as *const DWRITE_MATRIX),
                    DWRITE_RENDERING_MODE_NATURAL,
                    DWRITE_MEASURING_MODE_NATURAL,
                    0.0,
                    origin_y,
                )
                .ok()?;
            // windows-0.58 未导出命名常量，Grayscale 纹理类型值为 2。
            let gs = DWRITE_TEXTURE_TYPE(2);
            let bounds = analysis.GetAlphaTextureBounds(gs).ok()?;
            Some((analysis, bounds))
        }
    };

    // 趟 1：identity，仅取墨迹包围盒尺寸（位置无关）。
    let (_a1, b1) = run(face, None, 0.0)?;
    let w1 = (b1.right - b1.left).max(0) as f32;
    let h1 = (b1.bottom - b1.top).max(0) as f32;
    if w1 < 0.5 || h1 < 0.5 {
        return None; // 空格/空白字符：无墨迹
    }

    // 等比缩放，使墨迹放进 8×8（保留 DIN 宽高比），水平居中、顶部对齐。
    let scale = (CELL_W / w1).min(CELL_H / h1);
    let dx = -b1.left as f32 * scale + (CELL_W - w1 * scale) / 2.0;
    let dy = -b1.top as f32 * scale;
    let mat = DWRITE_MATRIX {
        m11: scale,
        m12: 0.0,
        m21: 0.0,
        m22: scale,
        dx,
        dy,
    };

    // 趟 2：带缩放/平移变换 → 墨迹落在 (0..8, 0..8)。
    let (a2, b2) = run(face, Some(&mat), 0.0)?;
    let bw = (b2.right - b2.left).max(0) as usize;
    let bh = (b2.bottom - b2.top).max(0) as usize;
    if bw == 0 || bh == 0 {
        return None;
    }
    let mut alpha = vec![0u8; bw * bh];
    unsafe {
        a2.CreateAlphaTexture(
            DWRITE_TEXTURE_TYPE(2), // Grayscale 纹理类型（windows-0.58 未导出命名常量）
            &b2,
            &mut alpha,
        )
        .ok()?;
    }

    let mut table = [0u8; 16];
    for ay in 0..bh {
        for ax in 0..bw {
            let val = alpha[ay * bw + ax];
            if val < 128 {
                continue;
            }
            let ox = b2.left + ax as i32;
            let oy = b2.top + ay as i32;
            if ox < 0 || ox >= CELL_W as i32 || oy < 0 || oy >= CELL_H as i32 {
                continue;
            }
            table[oy as usize] |= 1 << (7 - ox);
        }
    }
    Some(table)
}

/// 进程级一次性：栅格化三档 DIN 并注入 `ui::font` 字重字形表。
///
/// 幂等、失败静默；性能引擎首帧渲染前调用。**先全量栅格化到局部表，装完后
/// 一次性原子发布**——避免逐字符发布让渲染线程在注入中途读到半成品。
/// 若 DIN 不可用则什么都不注入，软渲染器继续用内置点阵（各字重同形，
/// 回到历史行为，但不崩溃）。
pub fn ensure_loaded() {
    static DONE: OnceLock<()> = OnceLock::new();
    let _ = DONE.get_or_init(|| {
        // 逐字符写入常驻表；未注入的槽位保持内置点阵（get_char_bitmap 原样回退）。
        // 写入与读取同线程（UI 渲染线程），thread_local 纯数组写，无锁、无发布。
        for weight in 0..3u8 {
            let Some(face) = face_for(weight) else {
                crate::probe::log(&format!("din_bitmap: DIN weight {} not loaded — skip", weight));
                continue;
            };
            for cp in 0x20u32..=0x7E {
                if let Some(t) = rasterize_char(&face, cp) {
                    sysoverlay_ui::font::set_weight_glyph(weight, (cp - 0x20) as usize, t);
                }
            }
            crate::probe::log(&format!(
                "din_bitmap: weight {} injected (DIN {})",
                weight,
                crate::render_d2d::din_file_name(weight)
            ));
        }
        // 全部装完无需发布：thread_local 表对本线程即刻可见（get_char_bitmap 同线程读）。
    });
}