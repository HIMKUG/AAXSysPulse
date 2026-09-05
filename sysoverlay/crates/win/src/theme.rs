//! Apple 风格主题：明暗两套配色，跟随系统。
//!
//! 颜色全部以 COLORREF（0x00BBGGRR）存储，供 GDI 直接使用。
//! 色板取自 iOS/macOS 系统色（HIG semantic colors）：
//! - 强调色 `#007AFF` / `#0A84FF`（systemBlue）
//! - 开关绿 `#34C759` / `#30D158`（systemGreen）
//! - 分层：window < card < 控件，明暗各自独立标注
//!
//! 主题检测：注册表 `HKCU\...\Themes\Personalize\AppsUseLightTheme`
//! （0 = 深色）。读不到（旧系统/精简系统）默认浅色，安全兜底。

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_DWORD};

impl Theme {
    /// 读取系统当前主题（跟随系统；读取失败回落浅色）。
    pub fn system() -> Theme {
        if apps_use_light_theme() {
            LIGHT
        } else {
            DARK
        }
    }
}

/// COLORREF 组装（0x00BBGGRR）。
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 语义色板。所有字段都是 COLORREF。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    /// 窗口底色
    pub window_bg: u32,
    /// 分组卡片底色
    pub card: u32,
    /// 卡片描边（极淡，只为界定边界）
    pub card_stroke: u32,
    /// 主文字
    pub label: u32,
    /// 次级文字（分节标题 / 说明）
    pub secondary: u32,
    /// 强调色（主按钮）
    pub accent: u32,
    /// 按钮悬停（accent 加深）
    pub accent_hover: u32,
    /// 开关打开色
    pub toggle_on: u32,
    /// 开关关闭底色
    pub toggle_off: u32,
    /// 开关关闭描边
    pub toggle_off_stroke: u32,
    /// 分段控件底
    pub segmented_bg: u32,
    /// 分段控件选中片
    pub seg_selected: u32,
    /// 分段控件选中片描边
    pub seg_selected_stroke: u32,
    /// 开关滑块
    pub knob: u32,
    /// 滑块描边
    pub knob_stroke: u32,
    /// 步进器 / 次级按钮底
    pub button_bg: u32,
    /// 次级按钮悬停
    pub button_bg_hover: u32,
}

/// iOS systemBlue / systemGreen / systemGray 分层（浅色）。
pub const LIGHT: Theme = Theme {
    dark: false,
    window_bg: rgb(236, 236, 240),   // grouped background
    card: rgb(255, 255, 255),
    card_stroke: rgb(222, 222, 227), // separator，极淡
    label: rgb(29, 29, 31),          // #1D1D1F
    secondary: rgb(110, 110, 115),   // #6E6E73
    accent: rgb(0, 122, 255),        // systemBlue
    accent_hover: rgb(0, 98, 204),
    toggle_on: rgb(52, 199, 89),     // systemGreen
    toggle_off: rgb(233, 233, 236),
    toggle_off_stroke: rgb(216, 216, 220),
    segmented_bg: rgb(235, 235, 238),
    seg_selected: rgb(255, 255, 255),
    seg_selected_stroke: rgb(214, 214, 219),
    knob: rgb(255, 255, 255),
    knob_stroke: rgb(214, 214, 219),
    button_bg: rgb(235, 235, 238),
    button_bg_hover: rgb(224, 224, 228),
};

/// 深色（macOS System Settings dark 风格的分层）。
pub const DARK: Theme = Theme {
    dark: true,
    window_bg: rgb(36, 36, 38),
    card: rgb(54, 54, 58),
    card_stroke: rgb(70, 70, 74),
    label: rgb(245, 245, 247),
    secondary: rgb(152, 152, 157),
    accent: rgb(10, 132, 255),
    accent_hover: rgb(64, 156, 255),
    toggle_on: rgb(48, 209, 88),
    toggle_off: rgb(80, 80, 85),
    toggle_off_stroke: rgb(96, 96, 101),
    segmented_bg: rgb(68, 68, 73),
    seg_selected: rgb(99, 99, 102), // iOS dark selected segment
    seg_selected_stroke: rgb(99, 99, 102),
    knob: rgb(255, 255, 255),
    knob_stroke: rgb(255, 255, 255),
    button_bg: rgb(68, 68, 73),
    button_bg_hover: rgb(84, 84, 89),
};

/// 注册表 `AppsUseLightTheme`：1/缺失 = 浅色，0 = 深色。
fn apps_use_light_theme() -> bool {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;

    const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
    const VALUE: &str = "AppsUseLightTheme";

    unsafe {
        let key_w: Vec<u16> = KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let val_w: Vec<u16> = VALUE.encode_utf16().chain(std::iter::once(0)).collect();
        let mut data: u32 = 1;
        let mut size = 4u32;
        let ret = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            PCWSTR(val_w.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        );
        if ret != ERROR_SUCCESS {
            return true; // 读不到 → 浅色兜底
        }
        data != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgb_is_colorref_layout() {
        // COLORREF = 0x00BBGGRR：r 在低字节
        assert_eq!(rgb(0x12, 0x34, 0x56), 0x563412);
    }

    #[test]
    fn test_light_dark_distinct_layers() {
        assert_ne!(LIGHT.window_bg, DARK.window_bg);
        assert_ne!(LIGHT.card, DARK.card);
        assert_eq!(LIGHT.accent, rgb(0, 122, 255), "systemBlue 浅色");
        assert_eq!(DARK.accent, rgb(10, 132, 255), "systemBlue 深色");
        assert!(!LIGHT.dark && DARK.dark);
    }
}
