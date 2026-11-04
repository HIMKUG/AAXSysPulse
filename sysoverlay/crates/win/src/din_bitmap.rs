//! 性能（位图）引擎的三档字重：用 DWrite 把 DIN 三版栅格化成 8×16 位图表，
//! 注入 `ui::font`（[`aaxsyspulse_ui::font::set_weight_glyph`]）。
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
    DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FACE_TYPE_CFF, DWRITE_FONT_FACE_TYPE_TRUETYPE,
    DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN, DWRITE_MATRIX, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_RENDERING_MODE_NATURAL, DWRITE_TEXTURE_TYPE,
};
use aaxsyspulse_ui::font::{FONT_WIDTH, GLYPH_VISIBLE_ROWS};

/// 点阵格：引用 ui 常量（审查 537：ui 侧一改即自动跟随，不再手写 8.0）。
/// 字形墨迹只放进顶部 GLYPH_VISIBLE_ROWS 行，布局按该行数预留。
const CELL_W: f32 = FONT_WIDTH as f32;
const CELL_H: f32 = GLYPH_VISIBLE_ROWS as f32;

/// Grayscale 纹理类型（windows-0.58 未导出命名常量，值固定为 2；
/// 审查 539：原先两处各写一份硬编码 + 注释，收敛为一份）。
const DWRITE_TEXTURE_TYPE_GRAYSCALE: DWRITE_TEXTURE_TYPE = DWRITE_TEXTURE_TYPE(2);

/// 按文件魔数判定 DWRITE_FONT_FACE_TYPE（审查 133：硬编码 CFF 会在
/// TrueType 字体上静默回退内置点阵）。支持 OTF(CFF/OTTO) 与 TTF 两类。
fn font_face_type_for(path: &std::path::Path) -> Option<DWRITE_FONT_FACE_TYPE> {
    use std::io::Read;
    let mut head = [0u8; 4];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    match head {
        [b'O', b'T', b'T', b'O'] => Some(DWRITE_FONT_FACE_TYPE_CFF),
        [0x00, 0x01, 0x00, 0x00] | [b't', b'r', b'u', b'e'] => {
            Some(DWRITE_FONT_FACE_TYPE_TRUETYPE)
        }
        _ => None,
    }
}

fn factory() -> Option<&'static IDWriteFactory> {
    static F: OnceLock<Option<IDWriteFactory>> = OnceLock::new();
    F.get_or_init(|| unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok() })
        .as_ref()
}

/// 加载指定字重的 DIN 字体（用 DWrite 私有字体集，内存态、不污染系统字体）。
fn face_for(weight: u8) -> Option<IDWriteFontFace> {
    let dwrite = factory()?;
    let path = crate::render_d2d::find_din_font_path(weight)?;
    let face_type = font_face_type_for(&path)?;
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
                face_type,
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

    let run = |face: &IDWriteFontFace,
               transform: Option<&DWRITE_MATRIX>|
     -> Option<(IDWriteGlyphRunAnalysis, RECT)> {
        // 审查 538：origin_y 恒 0.0，参数删除。
        let mut gr = DWRITE_GLYPH_RUN {
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
            // 审查 132：clone 进 glyph run 的 COM 引用手动保管后从不释放
            // （3 档 × 95 字符 × 2 趟 = 570 次泄漏）—— 分析对象创建后立即
            // 配对 drop（含错误路径）。
            let analysis = match dwrite.CreateGlyphRunAnalysis(
                &gr,
                1.0,
                transform.map(|m| m as *const DWRITE_MATRIX),
                DWRITE_RENDERING_MODE_NATURAL,
                DWRITE_MEASURING_MODE_NATURAL,
                0.0,
                0.0,
            ) {
                Ok(a) => a,
                Err(_) => {
                    ManuallyDrop::drop(&mut gr.fontFace);
                    return None;
                }
            };
            ManuallyDrop::drop(&mut gr.fontFace);
            let bounds = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_TYPE_GRAYSCALE).ok()?;
            Some((analysis, bounds))
        }
    };

    // 趟 1：identity，仅取墨迹包围盒尺寸（位置无关）。
    let (_a1, b1) = run(face, None)?;
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
    let (a2, b2) = run(face, Some(&mat))?;
    let bw = (b2.right - b2.left).max(0) as usize;
    let bh = (b2.bottom - b2.top).max(0) as usize;
    if bw == 0 || bh == 0 {
        return None;
    }
    let mut alpha = vec![0u8; bw * bh];
    unsafe {
        a2.CreateAlphaTexture(DWRITE_TEXTURE_TYPE_GRAYSCALE, &b2, &mut alpha).ok()?;
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
    // 审查 131：改用 thread_local 守护 —— 进程级 OnceLock 只注入一次，而
    // 字形表是 thread_local 的：渲染线程 respawn 后新线程的表回到内置点阵，
    // DIN 三档注入永久丢失。thread_local 标记让每个新渲染线程重新注入一次。
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|d| d.get()) {
        return;
    }
    DONE.with(|d| d.set(true));
    {
        // 逐字符写入常驻表；未注入的槽位保持内置点阵（get_char_bitmap 原样回退）。
        // 写入与读取同线程（UI 渲染线程），thread_local 纯数组写，无锁、无发布。
        for weight in 0..3u8 {
            let Some(face) = face_for(weight) else {
                crate::probe::log(&format!("din_bitmap: DIN weight {} not loaded — skip", weight));
                continue;
            };
            let mut ok_chars = 0usize;
            for cp in 0x20u32..=0x7E {
                if let Some(t) = rasterize_char(&face, cp) {
                    // 审查 540：set_weight_glyph 现在返回是否生效，按真实成功数记录
                    // —— DIN 存在但栅格化全失败时日志不再谎报"注入成功"。
                    if aaxsyspulse_ui::font::set_weight_glyph(weight, (cp - 0x20) as usize, t) {
                        ok_chars += 1;
                    }
                }
            }
            crate::probe::log(&format!(
                "din_bitmap: weight {} injected {ok_chars}/95 chars (DIN {})",
                weight,
                crate::render_d2d::din_file_name(weight)
            ));
        }
        // 全部装完无需发布：thread_local 表对本线程即刻可见（get_char_bitmap 同线程读）。
    }
}