//! 颜色与调色板派生（纯逻辑，零依赖）。
//!
//! # 为什么调色板在 core 而不在 ui
//!
//! 颜色派生是**纯函数**：给定 (bg, fg) 就能确定整块配色，
//! 强调色阈值（多少 % 变黄/变红）也是纯业务规则。
//! 放在 core 意味着这些规则能离线单测，且 `ui` 只负责把 `Rgb` 转成
//! 自己内部的预乘 `Rgba`，不再复制一份派生逻辑。

/// RGB 颜色（非预乘）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    /// Red component (0-255).
    pub r: u8,
    /// Green component (0-255).
    pub g: u8,
    /// Blue component (0-255).
    pub b: u8,
}

impl Rgb {
    /// Creates a new RGB color from components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Creates RGB from a Windows COLORREF value (0x00BBGGRR).
    ///
    /// 这是配置文件中 `bg`/`fg` 的编码方式（C 版 `%06lx` + `RGB(a&255, …)`）。
    /// **低字节是 R**，与常见的 RRGGBB 相反 —— 早期 bug 就是把两者弄混导致 R/B 互换。
    pub const fn from_colorref(cr: u32) -> Self {
        Self {
            r: (cr & 0xFF) as u8,
            g: ((cr >> 8) & 0xFF) as u8,
            b: ((cr >> 16) & 0xFF) as u8,
        }
    }

    /// Converts to Windows COLORREF value (0x00BBGGRR).
    pub const fn to_colorref(self) -> u32 {
        (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16)
    }

    /// Parses a hex color string in the "RRGGBB" format.
    ///
    /// 注意：配置文件用的是 COLORREF（见 [`Rgb::from_colorref`]），
    /// 本函数仅用于 UI/调试等 RRGGBB 场景，不参与配置解析。
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }

    /// Converts to hex bytes (RRGGBB)。返回定长数组而非 `String`，避免分配。
    pub fn to_hex(self) -> [u8; 6] {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        [
            HEX[(self.r >> 4) as usize],
            HEX[(self.r & 0xF) as usize],
            HEX[(self.g >> 4) as usize],
            HEX[(self.g & 0xF) as usize],
            HEX[(self.b >> 4) as usize],
            HEX[(self.b & 0xF) as usize],
        ]
    }

    /// Relative luminance (WCAG), 0.0 = black, 1.0 = white.
    pub fn luminance(self) -> f32 {
        let r = Self::gamma_to_linear(self.r as f32 / 255.0);
        let g = Self::gamma_to_linear(self.g as f32 / 255.0);
        let b = Self::gamma_to_linear(self.b as f32 / 255.0);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn gamma_to_linear(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    /// 按系数缩放（乘以每个分量）。用于从 fg 派生 title/name/sub。
    pub fn scale(self, factor: f32) -> Self {
        Self {
            r: (self.r as f32 * factor).clamp(0.0, 255.0) as u8,
            g: (self.g as f32 * factor).clamp(0.0, 255.0) as u8,
            b: (self.b as f32 * factor).clamp(0.0, 255.0) as u8,
        }
    }

    /// 两色混合：mix = self * (1 - t) + other * t
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let u = 1.0 - t;
        Self {
            r: (self.r as f32 * u + other.r as f32 * t).clamp(0.0, 255.0) as u8,
            g: (self.g as f32 * u + other.g as f32 * t).clamp(0.0, 255.0) as u8,
            b: (self.b as f32 * u + other.b as f32 * t).clamp(0.0, 255.0) as u8,
        }
    }

    /// 降饱和（向灰度靠拢）。`t=0` 原色，`t=1` 完全灰。
    ///
    /// 用于 `Metric::Stale` 的「陈旧标灰」—— 值还显示，但要一眼看出不是新鲜数据。
    pub fn desaturate(self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        // 感知加权灰度（与 luminance 同权重）
        let gray = (0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32)
            .clamp(0.0, 255.0);
        Self {
            r: (self.r as f32 * (1.0 - t) + gray * t).clamp(0.0, 255.0) as u8,
            g: (self.g as f32 * (1.0 - t) + gray * t).clamp(0.0, 255.0) as u8,
            b: (self.b as f32 * (1.0 - t) + gray * t).clamp(0.0, 255.0) as u8,
        }
    }
}

impl Default for Rgb {
    fn default() -> Self {
        Self { r: 20, g: 22, b: 27 }
    }
}

/// 强调色档位（负载分级的**纯规则**，与具体颜色解耦）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    /// <= 1%：几乎空闲，用最暗的中性色。
    Idle,
    /// 1% ~ 59%
    Ok,
    /// 60% ~ 84%
    Warn,
    /// >= 85%
    Hot,
}

impl Accent {
    /// 由百分比判定档位。阈值与 C 版一致。
    pub fn for_pct(pct: f32) -> Self {
        if pct >= 85.0 {
            Accent::Hot
        } else if pct >= 60.0 {
            Accent::Warn
        } else if pct <= 1.0 {
            Accent::Idle
        } else {
            Accent::Ok
        }
    }
}

/// 完整调色板。由 (bg, fg) 两色派生，规则镜像 C 版。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// 背景
    pub bg: Rgb,
    /// 边框
    pub border: Rgb,
    /// 进度条底槽
    pub track: Rgb,
    /// 标题（主机名）
    pub title: Rgb,
    /// 指标名
    pub name: Rgb,
    /// 指标值
    pub value: Rgb,
    /// 次要信息（运行时长）
    pub sub: Rgb,
    /// < 60% 的强调色
    pub ok: Rgb,
    /// 60% ~ 84%
    pub warn: Rgb,
    /// >= 85%
    pub hot: Rgb,
    /// <= 1%
    pub idle: Rgb,
}

impl Palette {
    /// 由背景/前景两色派生整块配色。
    ///
    /// 镜像 C 版推导规则：
    /// - `title = fg * 0.76`
    /// - `name  = fg * 0.65`
    /// - `sub   = fg * 0.45`
    /// - `track = mix(bg, fg, 0.10)`
    /// - `border= mix(bg, fg, 0.13)`
    pub fn derive(bg: Rgb, fg: Rgb) -> Self {
        Self {
            bg,
            border: bg.mix(fg, 0.13),
            track: bg.mix(fg, 0.10),
            title: fg.scale(0.76),
            name: fg.scale(0.65),
            value: fg,
            sub: fg.scale(0.45),
            ok: Rgb::new(74, 222, 128),
            warn: Rgb::new(251, 191, 36),
            hot: Rgb::new(248, 113, 113),
            idle: Rgb::new(63, 69, 80),
        }
    }

    /// 取某个强调档位对应的具体颜色。
    pub fn accent(&self, a: Accent) -> Rgb {
        match a {
            Accent::Idle => self.idle,
            Accent::Ok => self.ok,
            Accent::Warn => self.warn,
            Accent::Hot => self.hot,
        }
    }

    /// 一步到位：由百分比取强调色。
    pub fn accent_for(&self, pct: f32) -> Rgb {
        self.accent(Accent::for_pct(pct))
    }

    /// 陈旧的强调色：**降饱和**而非换色，保留原有的红/黄/绿语义层次。
    ///
    /// v2 Q16：陈旧用「降饱和灰」标注，与估算值的 `~` 前缀区分开。
    pub fn stale_accent_for(&self, pct: f32) -> Rgb {
        const STALE_DESATURATION: f32 = 0.55;
        self.accent_for(pct).desaturate(STALE_DESATURATION)
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::derive(Rgb::default(), Rgb { r: 238, g: 241, b: 244 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_low_byte_is_red() {
        // 这是 R/B 互换 bug 的直接回归
        let c = Rgb::from_colorref(0x1B1614);
        assert_eq!(c.r, 0x14);
        assert_eq!(c.g, 0x16);
        assert_eq!(c.b, 0x1B);
    }

    #[test]
    fn colorref_roundtrip() {
        for cr in [0x000000u32, 0x123456, 0xFFFFFF, 0x1B1614] {
            assert_eq!(Rgb::from_colorref(cr).to_colorref(), cr);
        }
    }

    #[test]
    fn hex_roundtrip() {
        let c = Rgb::new(0xAB, 0xCD, 0xEF);
        let hex = c.to_hex();
        assert_eq!(&hex, b"ABCDEF");
        assert_eq!(Rgb::from_hex("ABCDEF"), Some(c));
        assert_eq!(Rgb::from_hex("#ABCDEF"), Some(c));
    }

    #[test]
    fn hex_rejects_malformed() {
        assert_eq!(Rgb::from_hex("invalid"), None);
        assert_eq!(Rgb::from_hex("12345"), None);
        assert_eq!(Rgb::from_hex(""), None);
    }

    #[test]
    fn luminance_bounds() {
        assert_eq!(Rgb::new(0, 0, 0).luminance(), 0.0);
        assert!((Rgb::new(255, 255, 255).luminance() - 1.0).abs() < 1e-5);
        // 绿色比蓝色亮（权重 0.7152 vs 0.0722）
        assert!(Rgb::new(0, 255, 0).luminance() > Rgb::new(0, 0, 255).luminance());
    }

    #[test]
    fn scale_matches_c_derivation() {
        let fg = Rgb::new(238, 241, 244);
        assert_eq!(fg.scale(0.76), Rgb::new(180, 183, 185));
        assert_eq!(fg.scale(0.65), Rgb::new(154, 156, 158));
        assert_eq!(fg.scale(0.45), Rgb::new(107, 108, 109));
    }

    #[test]
    fn mix_endpoints() {
        let a = Rgb::new(0, 0, 0);
        let b = Rgb::new(255, 255, 255);
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
        assert_eq!(a.mix(b, 0.5), Rgb::new(127, 127, 127));
    }

    #[test]
    fn desaturate_is_monotonic() {
        let c = Rgb::new(248, 113, 113); // hot 红
        assert_eq!(c.desaturate(0.0), c);
        let half = c.desaturate(0.5);
        // 降饱和 → R 变小、B 变大（向灰度收敛）
        assert!(half.r < c.r);
        assert!(half.b > c.b);
        let full = c.desaturate(1.0);
        assert_eq!(full.r, full.g, "完全去色后三通道相等");
        assert_eq!(full.g, full.b);
    }

    #[test]
    fn accent_thresholds_match_c() {
        assert_eq!(Accent::for_pct(0.0), Accent::Idle);
        assert_eq!(Accent::for_pct(1.0), Accent::Idle);
        assert_eq!(Accent::for_pct(1.1), Accent::Ok);
        assert_eq!(Accent::for_pct(59.9), Accent::Ok);
        assert_eq!(Accent::for_pct(60.0), Accent::Warn);
        assert_eq!(Accent::for_pct(84.9), Accent::Warn);
        assert_eq!(Accent::for_pct(85.0), Accent::Hot);
        assert_eq!(Accent::for_pct(100.0), Accent::Hot);
        // 越界值不崩
        assert_eq!(Accent::for_pct(-5.0), Accent::Idle);
        assert_eq!(Accent::for_pct(500.0), Accent::Hot);
    }

    #[test]
    fn palette_derive_matches_c() {
        let pal = Palette::default();
        assert_eq!(pal.bg, Rgb::new(20, 22, 27));
        assert_eq!(pal.value, Rgb::new(238, 241, 244));
        assert_eq!(pal.ok, Rgb::new(74, 222, 128));
        assert_eq!(pal.warn, Rgb::new(251, 191, 36));
        assert_eq!(pal.hot, Rgb::new(248, 113, 113));
    }

    #[test]
    fn stale_accent_keeps_hue_family() {
        let pal = Palette::default();
        let hot = pal.accent_for(90.0);
        let stale_hot = pal.stale_accent_for(90.0);
        // 仍偏红（R 最大），只是变灰
        assert!(stale_hot.r > stale_hot.g);
        assert!(stale_hot.r > stale_hot.b);
        assert!(stale_hot.r < hot.r, "必须比新鲜值更暗/更灰");
    }
}
