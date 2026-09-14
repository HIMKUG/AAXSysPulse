//! 悬浮窗高画质渲染路径：整帧 Direct2D + DirectWrite。
//!
//! # 设计
//!
//! 悬浮窗是透明分层窗口（无背景卡片，只有文字 + 阴影/描边），其绘制在
//! `ui::Renderer::paint` 默认实现里**完全绕过** `RenderTarget` trait、直接戳
//! `SoftwareSurface.pixels`（像素块位图字体）。因此高画质模式不能复用那套绘制，
//! 必须**并行**实现：复用引擎无关的 `layout`（纯几何/文字 cell），再用本模块的
//! `D2DSurface`（实现 `ui::RenderTarget`，DWrite 画字 + D2D 填充）逐个 cell 重画。
//!
//! 上屏复用 `gdi::present` 的 DIB 缓存：把 `ID2D1DCRenderTarget` 绑到 DIB 的
//! 内存 DC（`BindDC`），D2D 直接写进 DIB 的预乘 BGRA 像素，再 `UpdateLayeredWindow`
//! 上传——与性能模式的通道完全一致，只是"画进 DIB"的来源不同。
//!
//! **性能模式零行为变化**：本模块不被性能模式调用，像素块路径原样保留，帧哈希一致。
//!
//! # Safety（本文件 unsafe 汇总）
//!
//! - `D2D1_FACTORY_TYPE_SINGLE_THREADED` 工厂 ⇒ 本模块的 D2D 对象只在渲染线程
//!   创建与使用，`D2DSurface` 不跨线程传递；DWrite 虽用 SHARED 工厂，其对象
//!   同样只在渲染线程触碰；
//! - COM 对象生命周期由 `D2DSurface` 持有、Drop 统一释放，无裸指针逃逸；
//! - `BindDC` 的 HDC 来自 gdi.rs 的 DIB 缓存，由同一渲染线程在同一帧内使用，
//!   不存在跨线程 DC 共享。

use std::collections::HashMap;
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFactory5, IDWriteFontCollection,
    IDWriteFontCollection1, IDWriteFontFile, IDWriteFontSet, IDWriteFontSetBuilder1,
    IDWriteStringList, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_PROPERTY_ID_FAMILY_NAME, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;

use aaxsyspulse_core::config::{Config, RenderEngine};
use aaxsyspulse_ui::Rgba;
use aaxsyspulse_ui::{Rect, RenderTarget};

// ── 进程级工厂（与设置页的 Gfx 独立，悬浮窗独占） ──────────

fn d2d_factory() -> Option<&'static ID2D1Factory> {
    static F: OnceLock<Option<ID2D1Factory>> = OnceLock::new();
    F.get_or_init(|| unsafe {
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()
    })
    .as_ref()
}

/// D2D RenderTarget 缓存：避免每帧调用 `CreateDCRenderTarget`（昂贵的 GPU 上下文创建）。
///
/// 仅在尺寸变化或设备丢失时重建。
struct RtCache {
    rt: Option<ID2D1DCRenderTarget>,
    w: i32,
    h: i32,
    mem_dc: HDC,
    /// RT 世代号：每次重建（尺寸变化 / 设备丢失）自增。
    /// 画刷缓存以它为键的一部分——画刷属于某个 RT，跨 RT 复用会画到旧 DC 上。
    gen: u64,
}

thread_local! {
    static RT_CACHE: RefCell<RtCache> = const { RefCell::new(RtCache {
        rt: None,
        w: 0,
        h: 0,
        mem_dc: HDC(std::ptr::null_mut()),
        gen: 0,
    }) };
}

thread_local! {
    static RT_DEVICE_LOST: RefCell<bool> = const { RefCell::new(false) };
}

// 文字格式缓存（进程级/线程级，**跨帧保留**）。
//
// 只依赖 DWrite 全局工厂 + `din_font` 的进程级私有字体集，与 RT 无关，
// 因此可以跨帧、跨 RT 复用。键 =（字号量化, 字重）。
//
// ⚠️ 2026-09-14：此前该缓存挂在 `D2DSurface` 上，而 `D2DSurface::bind` 每帧
// 新建实例 → 缓存每帧从空开始 → **每帧重复 `CreateTextFormat`**（字体集查找 +
// font face 解析，非廉价）。改为 thread_local 后只创建一次。
thread_local! {
    static FORMAT_CACHE: RefCell<HashMap<(u32, u32), IDWriteTextFormat>> =
        RefCell::new(HashMap::new());
}

// 画刷缓存（跨帧保留，**随 RT 世代失效**）。
struct BrushCache {
    gen: u64,
    map: HashMap<(u8, u8, u8, u8), ID2D1SolidColorBrush>,
}

thread_local! {
    static BRUSH_CACHE: RefCell<BrushCache> = RefCell::new(BrushCache {
        gen: u64::MAX,
        map: HashMap::new(),
    });
}

/// 当前 RT 世代号（未创建过 RT 时为 0）。
fn rt_gen() -> u64 {
    RT_CACHE.with(|c| c.borrow().gen)
}

/// 取（或创建）画刷。RT 世代变了（重建）即整体丢弃重建，避免把画刷留在旧 DC 上。
fn cached_brush(rt: &ID2D1DCRenderTarget, c: Rgba) -> Option<ID2D1SolidColorBrush> {
    let gen = rt_gen();
    let key = (c.r, c.g, c.b, c.a);
    BRUSH_CACHE.with(|bc| {
        let mut bc = bc.borrow_mut();
        if bc.gen != gen {
            bc.map.clear();
            bc.gen = gen;
        }
        if let Some(b) = bc.map.get(&key) {
            return Some(b.clone());
        }
        let b: ID2D1SolidColorBrush = unsafe { rt.CreateSolidColorBrush(&straight(c), None).ok()? };
        bc.map.insert(key, b.clone());
        Some(b)
    })
}

/// 取（或创建）文字格式。只依赖全局 DWrite 工厂与进程级字体集，跨帧安全。
fn cached_text_format(px: f32, weight: u8) -> Option<IDWriteTextFormat> {
    let key = ((px * 4.0).round() as u32, weight as u32);
    FORMAT_CACHE.with(|fc| {
        let mut fc = fc.borrow_mut();
        if let Some(f) = fc.get(&key) {
            return Some(f.clone());
        }
        let dwrite = dwrite_factory()?;
        // 高画质模式字体：DIN 三版（私有字体集，进程级加载一次，按字重取档）；
        // 文件缺失/加载失败 → 回退系统 Segoe UI（按其真实字重档位）。
        let f: IDWriteTextFormat = match din_font(weight) {
            Some(cf) => unsafe {
                let col: IDWriteFontCollection = cf.collection.cast().ok()?;
                dwrite
                    .CreateTextFormat(
                        PCWSTR(cf.family.as_ptr()),
                        Some(&col),
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        px,
                        w!(""),
                    )
                    .ok()?
            },
            None => unsafe {
                dwrite
                    .CreateTextFormat(
                        w!("Segoe UI"),
                        None,
                        dwrite_weight(weight),
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        px,
                        w!(""),
                    )
                    .ok()?
            },
        };
        unsafe {
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        fc.insert(key, f.clone());
        Some(f)
    })
}

// ── 帧内细分段计时（诊断用，PERF=1 才有值） ────────────────
//
// seg_draw（D2D 绘制 1274µs，帧成本最大单项）此前只有总数、无内部分解，
// 导致"引擎固有成本、无单点肥肉"的结论缺证据。这里在 cell 级循环里自累加
// 四类候选成本，帧末一条 `note_section_us` 落盘（避免上万条样本撑爆缓冲）。

const ACC_FMT: usize = 0;
const ACC_BRUSH: usize = 1;
const ACC_LAYOUT: usize = 2;
const ACC_TEXT: usize = 3;

thread_local! {
    static SEG_ACC: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
}

/// 累加一段耗时到 `SEG_ACC[idx]`（`start == 0` 时零成本返回）。
fn acc_add(idx: usize, start: u64) {
    if start == 0 {
        return;
    }
    let us = crate::perf::elapsed_since(start);
    SEG_ACC.with(|c| {
        let mut a = c.get();
        a[idx] += us;
        c.set(a);
    });
}

/// 帧末（gdi::render_d2d 读完 seg_draw 之后）调用：把累加值落进 perf 段样本并清零。
/// 全部为 0（PERF 关闭）时不做任何事。
pub fn flush_seg_acc() {
    SEG_ACC.with(|c| {
        let a = c.get();
        if a == [0; 4] {
            return;
        }
        c.set([0; 4]);
        crate::perf::note_section_us("seg_d2d_fmt", a[ACC_FMT]);
        crate::perf::note_section_us("seg_d2d_brush", a[ACC_BRUSH]);
        crate::perf::note_section_us("seg_d2d_layout", a[ACC_LAYOUT]);
        crate::perf::note_section_us("seg_d2d_text", a[ACC_TEXT]);
    });
}

/// 将文本编码为 UTF16，返回 Vec。
fn encode_text(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// 启动期预热：把 DWrite **首次**字体解析的一次性成本从"首帧"挪到"渲染线程
/// 刚起来时"，与主线程的窗口/托盘/自启初始化**并行**。
///
/// 背景（2026-09-14 实测，perf 段 `seg_d2d_layout`）：冷启动首帧的
/// `CreateTextLayout` 单次占用 **317ms**（整帧 377ms），DIN 私有字体集在第一次
/// 真正排版时才被 DWrite 解析。加上预热后，这段成本被提前到窗口还不可见时，
/// 且与主线程启动工作重叠 —— 首帧因此变快，窗口内容更早出现。
///
/// ⚠️ 只预热**当前配置用到的字重与字号**：每个字重是一个独立 .otf，
/// 逐一预热会把启动成本乘上字重数；字重/字号变更（设置操作）时各自首帧
/// 付一次即可接受。
///
/// 线程亲和：本函数只在渲染线程调用（`render_thread::start` 的线程体开头），
/// 创建的 `IDWriteTextFormat` 落进本线程的 [`FORMAT_CACHE`]，与后续渲染同线程。
pub fn warmup(cfg: &Config) {
    if cfg.render_engine != RenderEngine::D2d {
        return; // 性能模式不碰 D2D/DWrite 字体集，不预热
    }
    let t = crate::perf::begin();
    // 1) 私有字体集（DIN 三档之一）+ 字号 format：建立字体集与字号对象
    let _ = din_font(cfg.font_weight);
    let Some(dwrite) = dwrite_factory() else { return };
    let mut sizes = vec![cfg.font_size];
    for s in cfg.metric_font_sizes.iter().flatten() {
        sizes.push(*s);
    }
    // 2) 真正触发字体解析的是第一次 CreateTextLayout——逐个字号做一次即弃。
    //    ⚠️ 文本必须覆盖**实际会渲染的字符集**：2026-09-14 实测，只喂 ASCII
    //    （"0123456789:%"）时预热仅 1.2ms，首帧 CreateTextLayout 仍要 140ms——
    //    因为真实文本里有 DIN 字体不含的字符，首次触发 DWrite 的**回退字体
    //    查找**（建立系统字体集索引，很贵）。这里把 ASCII 可打印字符 + 数字
    //    标点 + 常用中文标签全覆盖，一次性把回退链建好。
    let text = encode_text(
        " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`\
         abcdefghijklmnopqrstuvwxyz{|}~处理器内存显卡网络功耗时钟温度频率使用率\
         综合占用显存延迟上传下载当前最高最低平均",
    );
    let n_sizes = sizes.len();
    for px in sizes {
        let Some(fmt) = cached_text_format(px, cfg.font_weight) else {
            continue;
        };
        let _ = unsafe { dwrite.CreateTextLayout(&text, &fmt, 1e6, 1e6) };
    }
    crate::probe::log(&format!(
        "d2d warmup done us={} weight={} sizes={}",
        crate::perf::elapsed_since(t),
        cfg.font_weight,
        n_sizes
    ));
}

/// [`warmup`] 的便捷入口：读渲染线程的配置镜像并预热；
/// 镜像尚未发布（watchdog respawn 路径）时静默跳过。
pub fn warmup_if_cfg() {
    if let Some(cfg) = crate::render_thread::cfg_snapshot() {
        warmup(&cfg);
    }
}

/// 获取或创建 D2D DC RenderTarget。尺寸变化或设备丢失时重建。
fn get_or_create_rt(mem_dc: HDC, w: i32, h: i32) -> Option<ID2D1DCRenderTarget> {
    let device_lost = RT_DEVICE_LOST.with(|flag| *flag.borrow());

    let mut cache = RT_CACHE.with(|c| {
        let b = c.borrow();
        RtCache {
            rt: b.rt.clone(),
            w: b.w,
            h: b.h,
            mem_dc: b.mem_dc,
            gen: b.gen,
        }
    });

    let size_changed = cache.w != w || cache.h != h;
    let needs_create = cache.rt.is_none() || device_lost || size_changed;

    if !needs_create {
        return cache.rt.clone();
    }

    if size_changed {
        cache.rt = None;
    }

    let d2d = d2d_factory()?;
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let new_rt: ID2D1DCRenderTarget = unsafe { d2d.CreateDCRenderTarget(&props).ok()? };
    let sub = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    unsafe {
        new_rt.BindDC(mem_dc, &sub).ok()?;
        new_rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    }
    cache.rt = Some(new_rt);
    cache.w = w;
    cache.h = h;
    cache.mem_dc = mem_dc;
    // 世代自增：通知画刷缓存整体失效（见 RtCache::gen 注释）。
    cache.gen = cache.gen.wrapping_add(1);
    RT_DEVICE_LOST.with(|flag| *flag.borrow_mut() = false);
    crate::probe::log(&format!("d2d rt cache: created/rebuilt {}x{}", w, h));
    let result = cache.rt.clone();
    RT_CACHE.with(|c| *c.borrow_mut() = cache);
    result
}

/// 标记设备丢失：下次 `get_or_create_rt` 会重建 RT。
pub fn mark_device_lost() {
    RT_DEVICE_LOST.with(|flag| *flag.borrow_mut() = true);
}

fn dwrite_factory() -> Option<&'static IDWriteFactory> {
    static F: OnceLock<Option<IDWriteFactory>> = OnceLock::new();
    F.get_or_init(|| unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok() }).as_ref()
}

// ── 自定义字体（DIN Black，高画质模式专用） ─────────────────

/// 自定义字体：family 名（NUL 结尾 UTF-16）+ 私有字体集。
///
/// 进程级只加载一次（`custom_font`）：`D2DSurface` 每帧重建，
/// 字体文件解析/字体集构建绝不能跟着每帧做。
struct CustomFont {
    family: Vec<u16>,
    collection: IDWriteFontCollection1,
}

/// 字重 → DIN 字体文件名（三档对应三版 OTF）。
///
/// 0=细(DIN-Medium) / 1=中(DIN-Bold) / 2=粗(DIN-Black)。
pub fn din_file_name(weight: u8) -> &'static str {
    match weight {
        0 => "DIN-Medium.otf",
        1 => "DIN-Bold.otf",
        _ => "DIN-Black.otf",
    }
}

/// 定位指定字重的 DIN.otf：用户 Downloads 目录优先（开发机字体来源），
/// 再 exe 目录逐级向上找（适配构建树/便携布局），最后回退到工程根。
pub fn find_din_font_path(weight: u8) -> Option<std::path::PathBuf> {
    let fname = din_file_name(weight);
    let user = std::path::Path::new(r"C:\Users\VOS_User\Downloads\en_Din_jb51\en_Din").join(fname);
    if user.is_file() {
        return Some(user);
    }
    let mut dir = std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf());
    for _ in 0..6 {
        if let Some(d) = &dir {
            let cand = d.join(fname);
            if cand.is_file() {
                return Some(cand);
            }
        }
        dir = dir.and_then(|d| d.parent().map(|p| p.to_path_buf()));
    }
    let literal = std::path::Path::new(r"D:\code\sys-overlay-c-master").join(fname);
    if literal.is_file() {
        return Some(literal);
    }
    None
}

/// 从字体文件构建私有字体集，并查询其 family 名（不写死 "DIN Black"，
/// 以字体文件内的真实命名为准）。`weight` ∈ 0..3 决定加载哪一版 DIN。
fn load_custom_font(weight: u8) -> Option<CustomFont> {
    let path = find_din_font_path(weight)?;
    let dwrite = dwrite_factory()?;
    unsafe {
        // Factory5（Win10 1809+）：CreateFontSetBuilder 返回 Builder1，
        // 其 AddFontFile 才能直接吃字体文件（基础 Builder 只收 FaceReference）。
        let factory5: IDWriteFactory5 = dwrite.cast().ok()?;
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let file: IDWriteFontFile =
            factory5.CreateFontFileReference(PCWSTR(wide.as_ptr()), None).ok()?;
        let builder: IDWriteFontSetBuilder1 = factory5.CreateFontSetBuilder().ok()?;
        builder.AddFontFile(&file).ok()?;
        let set: IDWriteFontSet = builder.CreateFontSet().ok()?;
        let names: IDWriteStringList =
            set.GetPropertyValues(DWRITE_FONT_PROPERTY_ID_FAMILY_NAME).ok()?;
        if names.GetCount() == 0 {
            return None;
        }
        let len = names.GetStringLength(0).ok()? as usize;
        let mut buf = vec![0u16; len + 1];
        names.GetString(0, &mut buf).ok()?;
        let collection: IDWriteFontCollection1 =
            factory5.CreateFontCollectionFromFontSet(&set).ok()?;
        crate::probe::log(&format!(
            "d2d custom font loaded: {}",
            path.display()
        ));
        Some(CustomFont { family: buf, collection })
    }
}

/// 进程级三档 DIN 字体（加载失败 → None → 回退系统字体 Segoe UI）。
///
/// 每档独立 `OnceLock`，只栅格化/解析一次，`D2DSurface` 每帧复用，不做重复加载。
fn din_font(weight: u8) -> Option<&'static CustomFont> {
    static FONTS: [OnceLock<Option<CustomFont>>; 3] =
        [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let i = (weight as usize).min(2);
    FONTS[i].get_or_init(|| load_custom_font(i as u8)).as_ref()
}

/// 预乘 Rgba → D2D 直色（D2D 在预乘 alpha 目标上内部再预乘一次）。
fn straight(c: Rgba) -> D2D1_COLOR_F {
    let a = c.a as f32 / 255.0;
    if a <= 0.0 {
        return D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    }
    let inv = 1.0 / a;
    D2D1_COLOR_F {
        r: (c.r as f32 * inv) / 255.0,
        g: (c.g as f32 * inv) / 255.0,
        b: (c.b as f32 * inv) / 255.0,
        a,
    }
}

/// 把像素块位图字体的基准 16px 缩放比映射成 DWrite 字号（DIP=px，RT 恒等 DPI）。
fn d2d_font_px(scale: f32) -> f32 {
    (16.0 * scale).max(1.0)
}

/// 字重(0/1/2) → DirectWrite 数值字重（回退字体 Segoe UI 拿不到 DIN 三档时用）。
///
/// 语义对齐 DIN 三版：0=细(DIN-Medium)≈500, 1=中(DIN-Bold)≈700, 2=粗(DIN-Black)≈900。
fn dwrite_weight(weight: u8) -> DWRITE_FONT_WEIGHT {
    DWRITE_FONT_WEIGHT(match weight {
        0 => 500,
        1 => 700,
        _ => 900,
    })
}

/// 绑到 DIB 内存 DC 的 D2D 离屏渲染目标；实现 `ui::RenderTarget` 供悬浮窗复用。
pub struct D2DSurface {
    rt: ID2D1DCRenderTarget,
    /// 本帧已绘制的**最右文本右缘**（DWrite 实测，含宽度）。布局按位图字体度量
    /// 计算 cell 几何，DIN 真字形通常更宽 → 实测右缘超出窗口即被右缘裁掉。
    /// gdi::render_d2d 在 end_draw 后读取它做自愈扩窗（一次到位，无需估算余量）。
    pub max_text_right: f32,
}

impl D2DSurface {
    /// 创建并绑到 `mem_dc`（DIB 的内存 DC），尺寸 `w×h`。
    /// 失败返回 None（调用方回退到性能路径或跳过本帧）。
    ///
    /// **RT 缓存优化**：`CreateDCRenderTarget` 是昂贵的 GPU 上下文创建操作，
    /// 这里改为使用 `get_or_create_rt` 缓存——尺寸不变时复用已创建的 RT，
    /// 仅在尺寸变化或设备丢失时重建。
    pub fn bind(mem_dc: HDC, w: i32, h: i32) -> Option<Self> {
        let rt = get_or_create_rt(mem_dc, w, h)?;
        Some(Self {
            rt,
            max_text_right: 0.0,
        })
    }

    fn brush(&mut self, c: Rgba) -> Option<ID2D1SolidColorBrush> {
        let t = crate::perf::begin();
        let b = cached_brush(&self.rt, c);
        acc_add(ACC_BRUSH, t);
        b
    }

    /// 在 (x,y) 处用 DWrite 画一段文字（含可选阴影/描边，镜像 draw_row_text 语义）。
    ///
    /// `right_align`：该 cell 是右侧数值（右对齐）。布局的 cell 几何由**位图等宽
    /// 度量**推算，而 DIN 真字形是比例字体、宽度与之不等 → 直接按 `x` 绘制会让
    /// 数值右缘逐行漂移（同一列对不齐，网速串尤甚）。置 true 时用 DWrite 实测宽度
    /// 修正绘制原点，使 ink 右缘恒等于布局锚点 `x + 位图度量宽`。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_glyphs(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        scale: f32,
        weight: u8,
        color: Rgba,
        shade: Rgba,
        shadow: bool,
        outline: bool,
        right_align: bool,
    ) {
        if text.is_empty() || (color.a == 0 && shade.a == 0) {
            return;
        }
        let px = d2d_font_px(scale);
        let t_fmt = crate::perf::begin();
        let fmt = cached_text_format(px, weight);
        acc_add(ACC_FMT, t_fmt);
        let Some(fmt) = fmt else { return };
        // 画刷先解析一次（每个 cell 最多两种颜色：主色 + 阴影/描边色），
        // 不在描边 4 次循环里重复查缓存。
        let brush_main = match self.brush(color) {
            Some(b) => b,
            None => return,
        };
        let brush_shade = if outline || shadow { self.brush(shade) } else { None };

        let t_lay = crate::perf::begin();
        let text_buf = encode_text(text);
        let lay = dwrite_factory()
            .and_then(|d| unsafe { d.CreateTextLayout(&text_buf, &fmt, 1e6, 1e6).ok() })
            .map(|l| {
                let mut tm = DWRITE_TEXT_METRICS::default();
                unsafe {
                    let _ = l.GetMetrics(&mut tm);
                }
                (l, tm.widthIncludingTrailingWhitespace)
            });
        acc_add(ACC_LAYOUT, t_lay);
        let Some((layout, actual_w)) = lay else { return };
        // 右对齐修正：布局锚点 = x + 位图度量宽（与 cell_sig 用同一度量函数）。
        let base_x = if right_align {
            x as f32 + aaxsyspulse_ui::font::measure_scaled_f(text, scale) as f32 - actual_w
        } else {
            x as f32
        };
        let draw = |surface: &mut Self, ox: i32, oy: i32, b: &ID2D1SolidColorBrush| {
            // 自愈扩窗依据：DWrite 实测该行文本右缘（DIN 真字形宽度，非位图度量）。
            let right = base_x + ox as f32 + actual_w;
            if right > surface.max_text_right {
                surface.max_text_right = right;
            }
            let t = crate::perf::begin();
            unsafe {
                surface.rt.DrawTextLayout(
                    D2D_POINT_2F { x: base_x + ox as f32, y: (y + oy) as f32 },
                    &layout,
                    b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            acc_add(ACC_TEXT, t);
        };
        if outline {
            if let Some(b) = &brush_shade {
                draw(self, -1, 0, b);
                draw(self, 1, 0, b);
                draw(self, 0, -1, b);
                draw(self, 0, 1, b);
            }
        } else if shadow {
            if let Some(b) = &brush_shade {
                draw(self, 1, 1, b);
            }
        }
        draw(self, 0, 0, &brush_main);
    }

    /// 开始一帧绘制（**必须**在 clear/任何绘制前调用）。
    ///
    /// ⚠️ 2026-09-09 修复：此前从未调用 BeginDraw 就直接画 + EndDraw，
    /// EndDraw 每帧返回 WRONG_STATE → 高画质模式全部丢帧（probe.log
    /// 22 次 "end_draw FAILED"），叠加引擎切换后的尺寸清零 → 窗口直接消失。
    pub fn begin_draw(&self) {
        unsafe {
            self.rt.BeginDraw();
        }
    }

    /// 结束绘制（配对 BeginDraw）。设备丢失返回 Err，调用方本帧跳过 + 下帧重建。
    pub fn end_draw(&self) -> bool {
        match unsafe { self.rt.EndDraw(None, None) } {
            Ok(_) => true,
            Err(e) => {
                crate::probe::log(&format!("d2d end_draw error: {}", e.code().0));
                mark_device_lost();
                false
            }
        }
    }
}

impl RenderTarget for D2DSurface {
    fn clear(&mut self) {
        unsafe {
            // 背景必须是 alpha == 0（不是 1/255 之类的近零值）：分层窗口的命中
            // 测试由系统按像素 alpha 判定，只有 alpha 恰为 0 的像素才把鼠标事件
            // 放给下方窗口；alpha=1 的像素会被本窗口吞掉点击，而 WM_NCHITTEST
            // 返回 HTTRANSPARENT 只对同线程窗口有效，跨进程救不回来（点击被吃掉）。
            // 文字/描边/阴影(alpha>0) 保持命中 → 可拖拽、可悬停。
            // 与 GDI 软件光栅底色保持一致（SoftwareSurface::clear 同为全 0）。
            self.rt
                .Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        }
    }

    fn scale_alpha(&mut self, _factor: f32) {
        // 全局透明度由 present 的 SourceConstantAlpha 处理，本路径无需逐像素缩放。
    }

    fn fill_round_rect(&mut self, r: Rect, radius: i32, color: Rgba) {
        if r.w <= 0 || r.h <= 0 || color.a == 0 {
            return;
        }
        let b = match self.brush(color) {
            Some(b) => b,
            None => return,
        };
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: r.x as f32,
                top: r.y as f32,
                right: (r.x + r.w) as f32,
                bottom: (r.y + r.h) as f32,
            },
            radiusX: radius as f32,
            radiusY: radius as f32,
        };
        unsafe { self.rt.FillRoundedRectangle(&rr, &b) };
    }

    fn fill_rect(&mut self, r: Rect, color: Rgba) {
        if r.w <= 0 || r.h <= 0 || color.a == 0 {
            return;
        }
        let b = match self.brush(color) {
            Some(b) => b,
            None => return,
        };
        unsafe {
            self.rt.FillRectangle(
                &D2D_RECT_F {
                    left: r.x as f32,
                    top: r.y as f32,
                    right: (r.x + r.w) as f32,
                    bottom: (r.y + r.h) as f32,
                },
                &b,
            );
        }
    }

    fn draw_text(&mut self, s: &str, at: Rect, color: Rgba, _font: aaxsyspulse_ui::FontSpec) {
        // 调用方使用 draw_glyphs（含阴影/描边）；单色直绘兜底（默认粗、左对齐）。
        self.draw_glyphs(s, at.x, at.y, 1.0, 2, color, Rgba::transparent(), false, false, false);
    }

    fn measure_text(&self, s: &str, _font: aaxsyspulse_ui::FontSpec) -> (i32, i32) {
        // 一次性 format（不走缓存：&self；此方法在 cell 绘制路径不被调用，仅为 trait 完整性）。
        let fallback = (s.len() as i32 * 8, 16);
        let Some(dwrite) = dwrite_factory() else { return fallback };
        let fmt: IDWriteTextFormat = match unsafe {
            dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                d2d_font_px(1.0),
                w!(""),
            )
        } {
            Ok(f) => f,
            Err(_) => return fallback,
        };
        let text_buf = encode_text(s);
        let layout = match unsafe { dwrite.CreateTextLayout(&text_buf, &fmt, 1e6, 1e6) } {
            Ok(l) => l,
            Err(_) => return fallback,
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        if unsafe { layout.GetMetrics(&mut m).is_err() } {
            return fallback;
        }
        (m.width.ceil() as i32, m.height.ceil() as i32)
    }

    fn blit_coverage(
        &mut self,
        _at: Rect,
        _coverage: &[u8],
        _cov_stride: usize,
        _cov_w: i32,
        _cov_h: i32,
        _color: Rgba,
    ) {
        // D2D 路径用 DWrite 直绘，不走位图字体覆盖率蒙版。
    }
}

/// 把一帧 cell 用 D2D 画进已绑定的 surface（整帧重绘；脏区只影响上屏子矩形）。
///
/// `weight` ∈ 0..3（0 细 / 1 中 / 2 粗），决定整帧统一使用的 DIN 字重档。
pub fn paint_cells(
    surface: &mut D2DSurface,
    cells: &[aaxsyspulse_ui::renderer::CellSig],
    weight: u8,
) {
    for c in cells {
        surface.draw_glyphs(
            c.text.as_str(),
            c.x,
            c.y,
            c.scale,
            weight,
            c.color,
            c.shade,
            c.shadow,
            c.outline,
            c.right_align,
        );
    }
}

/// 引擎切换时强制整窗重绘：把帧哈希与性能模式区分开（同内容不同像素不能共享跳过）。
pub const ENGINE_HASH_XOR: u64 = 0x9E37_79B9_7F4A_7C15;
