//! 应用图标（AAX SysPulse）。
//!
//! 一个**多尺寸 ICO**（16 / 64 / 128，PNG 压缩）在编译期内嵌进 exe，
//! 运行时解析 ICO 目录表取出最合适的一帧，经 `CreateIconFromResourceEx`
//! 解码为 HICON。无外部资源文件、无运行时磁盘依赖 —— exe 自包含。
//!
//! 同一个文件同时被 `crates/app/build.rs` 编成 Windows 资源，
//! 因此资源管理器里看到的 exe 图标与本模块取出的图标一致。

use std::sync::OnceLock;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, HICON, LR_DEFAULTCOLOR,
};

/// 多尺寸应用图标：16×16 / 64×64 / 128×128（均为 PNG 压缩帧）。
pub const ICO_MAIN: &[u8] = include_bytes!("../assets/app_icon.ico");

/// ICONDIR 固定 6 字节；其后每个 ICONDIRENTRY 固定 16 字节。
const ICO_HEADER_LEN: usize = 6;
const ICO_DIRENTRY_LEN: usize = 16;

/// 从 ICO 字节中挑出与目标尺寸**最接近**的一帧，返回该帧的原始图像数据
/// （PNG 流，或旧式 BMP 的 BITMAPINFOHEADER 块）。
///
/// ICO 用 0 表示 256，需还原。越界/畸形数据一律跳过，绝不 panic。
pub fn pick_frame(data: &'static [u8], want: u32) -> Option<&'static [u8]> {
    if data.len() < ICO_HEADER_LEN {
        return None;
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut best: Option<(u32, usize, usize)> = None; // (距离, 偏移, 长度)
    for i in 0..count {
        let o = ICO_HEADER_LEN.checked_add(i.checked_mul(ICO_DIRENTRY_LEN)?)?;
        if o + ICO_DIRENTRY_LEN > data.len() {
            break;
        }
        // 宽/高字节为 0 时表示 256
        let w = if data[o] == 0 { 256u32 } else { data[o] as u32 };
        let h = if data[o + 1] == 0 { 256u32 } else { data[o + 1] as u32 };
        let len = u32::from_le_bytes(data[o + 8..o + 12].try_into().ok()?) as usize;
        let off = u32::from_le_bytes(data[o + 12..o + 16].try_into().ok()?) as usize;
        if off.checked_add(len)? > data.len() {
            continue;
        }
        let dist = w.abs_diff(want) + h.abs_diff(want);
        if best.map(|(d, _, _)| dist < d).unwrap_or(true) {
            best = Some((dist, off, len));
        }
    }
    let (_, off, len) = best?;
    Some(&data[off..off + len])
}

/// 由 ICO 字节创建 HICON：自动选最接近 `size` 的一帧，并**使用该帧的原始
/// 尺寸**（cxDesired/cyDesired 传 0），避免缩放导致的模糊。
///
/// 失败返回 `HICON::default()`（空句柄）—— 调用方按"没图标"处理即可，
/// 不会中断启动流程。
pub fn hicon(ico: &'static [u8], size: u32) -> HICON {
    match pick_frame(ico, size) {
        Some(bits) => unsafe {
            CreateIconFromResourceEx(bits, true, 0x0003_0000, 0, 0, LR_DEFAULTCOLOR)
                .unwrap_or_default()
        },
        None => HICON::default(),
    }
}

// ── 缓存：HICON 非 Send/Sync，句柄值以 isize 存在 static ──────────────

static CACHE_TRAY: OnceLock<isize> = OnceLock::new();
static CACHE_LARGE: OnceLock<isize> = OnceLock::new();
static CACHE_SMALL: OnceLock<isize> = OnceLock::new();

fn cached(cell: &OnceLock<isize>, size: u32) -> HICON {
    let v = *cell.get_or_init(|| hicon(ICO_MAIN, size).0 as isize);
    HICON(v as *mut _)
}

/// 托盘图标（取 16×16 帧）。
pub fn tray_icon() -> HICON {
    cached(&CACHE_TRAY, 16)
}

/// 窗口大图标（取 128×128 帧），用于 WNDCLASSEXW.hIcon / WM_SETICON(ICON_BIG)。
pub fn large_icon() -> HICON {
    cached(&CACHE_LARGE, 128)
}

/// 窗口小图标（取 16×16 帧），用于 WNDCLASSEXW.hIconSm / WM_SETICON(ICON_SMALL)。
pub fn small_icon() -> HICON {
    cached(&CACHE_SMALL, 16)
}

// ── D2D 位图（关于页展示图标） ──────────────────────────────────

/// 关于页图标原始像素：64×64 **预乘 BGRA**（与 `app_icon.ico` 同源，
/// 由 ICO 的 64×64 帧离线解码后预乘得到）。
const ABOUT_RGBA: &[u8] = include_bytes!("../assets/app_icon_64bgra.bin");
/// 关于页图标像素边长。
const ABOUT_SIDE: u32 = 64;

/// 在指定 D2D 渲染目标上创建关于页图标位图。
///
/// 直接用 `CreateBitmap` 上传**预乘 BGRA** 像素，**不经 WIC/COM 解码**：
/// 行为完全确定，且显式声明 `PREMULTIPLIED`，杜绝 alpha 解释不一致。
///
/// > 历史坑：此前走 WIC 解码，一旦失败就回退成 `fill_round(t.label)` 纯色
/// > 圆角块 —— 深色主题下 label 为白，用户看到的就是「纯白图片」。
///
/// 失败返回 `None`：调用方回退到**弱化的**占位描边，不能再画纯色块。
pub fn d2d_bitmap(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
) -> Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap> {
    use windows::Win32::Graphics::Direct2D::Common::{
        D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
    };
    use windows::Win32::Graphics::Direct2D::D2D1_BITMAP_PROPERTIES;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

    if ABOUT_RGBA.len() < (ABOUT_SIDE * ABOUT_SIDE * 4) as usize {
        return None;
    }
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let size = D2D_SIZE_U {
        width: ABOUT_SIDE,
        height: ABOUT_SIDE,
    };
    unsafe {
        rt.CreateBitmap(
            size,
            Some(ABOUT_RGBA.as_ptr() as *const core::ffi::c_void),
            ABOUT_SIDE * 4,
            &props,
        )
        .ok()
    }
}
