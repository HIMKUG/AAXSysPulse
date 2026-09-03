//! Configuration module for SysOverlay.
//!
//! Handles parsing, serializing, and validation of user configuration.

use std::str::FromStr;

/// RGB color representation.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Creates RGB from a Windows COLORREF value (0x00BBGGRR).
    pub fn from_colorref(cr: u32) -> Self {
        Self {
            r: (cr & 0xFF) as u8,
            g: ((cr >> 8) & 0xFF) as u8,
            b: ((cr >> 16) & 0xFF) as u8,
        }
    }

    /// Converts to Windows COLORREF value (0x00BBGGRR).
    pub fn to_colorref(&self) -> u32 {
        (self.r as u32) | ((self.g as u32) << 8) | ((self.b as u32) << 16)
    }
}

impl Default for Rgb {
    fn default() -> Self {
        Self { r: 20, g: 22, b: 27 }
    }
}

impl Rgb {
    /// Parses a hex color string in the "RRGGBB" format.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }

    /// Converts to hex string.
    pub fn to_hex(&self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Luminance value.
    pub fn luminance(&self) -> f32 {
        let r_lin = Self::gamma_to_linear(self.r as f32 / 255.0);
        let g_lin = Self::gamma_to_linear(self.g as f32 / 255.0);
        let b_lin = Self::gamma_to_linear(self.b as f32 / 255.0);
        0.2126 * r_lin + 0.7152 * g_lin + 0.0722 * b_lin
    }

    fn gamma_to_linear(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Zoom {
    #[default]
    Zoom75,
    Zoom100,
    Zoom125,
    Zoom150,
    Zoom200,
}

impl Zoom {
    pub fn scale_factor(&self) -> f32 {
        match self {
            Zoom::Zoom75 => 0.75,
            Zoom::Zoom100 => 1.0,
            Zoom::Zoom125 => 1.25,
            Zoom::Zoom150 => 1.5,
            Zoom::Zoom200 => 2.0,
        }
    }

    fn from_int(v: u32) -> Self {
        match v {
            75 => Zoom::Zoom75,
            125 => Zoom::Zoom125,
            150 => Zoom::Zoom150,
            200 => Zoom::Zoom200,
            _ => Zoom::Zoom100,
        }
    }

    fn to_int(&self) -> u32 {
        match self {
            Zoom::Zoom75 => 75,
            Zoom::Zoom100 => 100,
            Zoom::Zoom125 => 125,
            Zoom::Zoom150 => 150,
            Zoom::Zoom200 => 200,
        }
    }
}

impl FromStr for Zoom {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<u32>() {
            Ok(v) => Ok(Zoom::from_int(v)),
            Err(_) => Ok(Zoom::Zoom100), // Fallback for invalid input
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum NetUnit {
    #[default]
    KB,
    MB,
    GB,
}

impl NetUnit {
    fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "KB" => NetUnit::KB,
            "GB" => NetUnit::GB,
            _ => NetUnit::MB,
        }
    }

    fn to_str(&self) -> &'static str {
        match self {
            NetUnit::KB => "KB",
            NetUnit::MB => "MB",
            NetUnit::GB => "GB",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Shape {
    #[default]
    Round,
    Square,
    Content,
}

impl Shape {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "square" => Shape::Square,
            "content" => Shape::Content,
            _ => Shape::Round,
        }
    }

    fn to_str(&self) -> &'static str {
        match self {
            Shape::Round => "round",
            Shape::Square => "square",
            Shape::Content => "content",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum VertLayout {
    #[default]
    SingleRow,
    SingleCol,
    SplitCol,
}

impl VertLayout {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "singlecol" | "single_col" => VertLayout::SingleCol,
            "splitcol" | "split_col" => VertLayout::SplitCol,
            _ => VertLayout::SingleRow,
        }
    }

    fn to_str(&self) -> &'static str {
        match self {
            VertLayout::SingleRow => "singlerow",
            VertLayout::SingleCol => "singlecol",
            VertLayout::SplitCol => "splitcol",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub minimal: bool,
    pub zoom: Zoom,
    pub hover: bool,
    pub net_unit: NetUnit,
    pub shape: Shape,
    pub vert: VertLayout,
    pub bg: Rgb,
    pub fg: Rgb,
    pub opacity: u8,
    pub interval_ms: u32,
    pub topmost: bool,
    pub window_pos: Option<(i32, i32)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            minimal: false,
            zoom: Zoom::Zoom100,
            hover: false,
            net_unit: NetUnit::MB,
            shape: Shape::Round,
            vert: VertLayout::SingleRow,
            bg: Rgb { r: 20, g: 22, b: 27 },
            fg: Rgb { r: 238, g: 241, b: 244 },
            opacity: 90,
            interval_ms: 1000,
            topmost: true,
            window_pos: None,
        }
    }
}

impl Config {
    /// Parse a boolean from string - accepts "1", "0", "true", "false"
    fn parse_bool(s: &str) -> bool {
        match s.trim().to_lowercase().as_str() {
            "1" | "true" => true,
            "0" | "false" => false,
            _ => false,
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut cfg = Config::default();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_lowercase();
                let value = value.trim();

                match key.as_str() {
                    "minimal" => {
                        cfg.minimal = Self::parse_bool(value);
                    }
                    "zoom" => {
                        if let Ok(z) = value.parse::<Zoom>() {
                            cfg.zoom = z;
                        }
                    }
                    "hover" => {
                        cfg.hover = Self::parse_bool(value);
                    }
                    "netunit" => {
                        cfg.net_unit = NetUnit::from_str(value);
                    }
                    "shape" => {
                        cfg.shape = Shape::from_str(value);
                    }
                    "vert" => {
                        cfg.vert = VertLayout::from_str(value);
                    }
                    "bg" => {
                        if let Some(c) = Rgb::from_hex(value) {
                            cfg.bg = c;
                        }
                    }
                    "fg" => {
                        if let Some(c) = Rgb::from_hex(value) {
                            cfg.fg = c;
                        }
                    }
                    "opacity" => {
                        if let Ok(v) = value.parse::<u8>() {
                            cfg.opacity = v.clamp(1, 100);
                        }
                    }
                    "interval" => {
                        if let Ok(v) = value.parse::<u32>() {
                            cfg.interval_ms = v.clamp(1000, 5000);
                        }
                    }
                    "topmost" => {
                        cfg.topmost = Self::parse_bool(value);
                    }
                    _ => {}
                }
            }
        }

        cfg
    }

    pub fn serialize(&self) -> String {
        let mut s = String::new();
        if let Some((x, y)) = self.window_pos {
            s.push_str(&format!("pos {} {}\n", x, y));
        }
        s.push_str(&format!("topmost {}\n", self.topmost as u32));
        s.push_str(&format!("opacity {}\n", self.opacity));
        s.push_str(&format!("interval {}\n", self.interval_ms));
        s.push_str(&format!("minimal {}\n", self.minimal as u32));
        s.push_str(&format!("zoom {}\n", self.zoom.to_int()));
        s.push_str(&format!("hover {}\n", self.hover as u32));
        s.push_str(&format!("netunit {}\n", self.net_unit.to_str()));
        s.push_str(&format!("shape {}\n", self.shape.to_str()));
        s.push_str(&format!("vert {}\n", self.vert.to_str()));
        s.push_str(&format!("bg {:06X}\n", self.bg.to_colorref()));
        s.push_str(&format!("fg {:06X}\n", self.fg.to_colorref()));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgb_default() {
        let c = Rgb::default();
        assert_eq!(c.r, 20);
        assert_eq!(c.g, 22);
        assert_eq!(c.b, 27);
    }

    #[test]
    fn test_rgb_from_hex() {
        let c = Rgb::from_hex("FF8040").unwrap();
        assert_eq!(c.r, 0xFF);
        assert_eq!(c.g, 0x80);
        assert_eq!(c.b, 0x40);
    }

    #[test]
    fn test_rgb_from_hex_with_hash() {
        let c = Rgb::from_hex("#ABCDEF").unwrap();
        assert_eq!(c.r, 0xAB);
        assert_eq!(c.g, 0xCD);
        assert_eq!(c.b, 0xEF);
    }

    #[test]
    fn test_rgb_from_hex_invalid() {
        assert!(Rgb::from_hex("invalid").is_none());
        assert!(Rgb::from_hex("12345").is_none());
    }

    #[test]
    fn test_rgb_colorref_roundtrip() {
        let original = 0x123456u32;
        let c = Rgb::from_colorref(original);
        assert_eq!(c.to_colorref(), original);
    }

    #[test]
    fn test_config_default() {
        let cfg = Config::default();
        assert!(!cfg.minimal);
        assert_eq!(cfg.zoom, Zoom::Zoom100);
        assert_eq!(cfg.opacity, 90);
        assert!(cfg.topmost);
    }

    #[test]
    fn test_config_parse() {
        let text = "minimal=1\nzoom=150\nhover=true\nnetunit=GB\nshape=square\nvert=splitcol\nbg=1A2B3C\nfg=F0F1F2\nopacity=80\ninterval=2000\ntopmost=0\n";
        let cfg = Config::parse(text);
        assert!(cfg.minimal);
        assert_eq!(cfg.zoom, Zoom::Zoom150);
        assert!(cfg.hover);
        assert_eq!(cfg.net_unit, NetUnit::GB);
        assert_eq!(cfg.shape, Shape::Square);
        assert_eq!(cfg.vert, VertLayout::SplitCol);
        assert_eq!(cfg.opacity, 80);
        assert_eq!(cfg.interval_ms, 2000);
        assert!(!cfg.topmost);
    }

    #[test]
    fn test_config_serialize_roundtrip() {
        let cfg = Config::default();
        let serialized = cfg.serialize();
        let parsed = Config::parse(&serialized);
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn test_zoom_parsing() {
        assert_eq!("75".parse::<Zoom>().unwrap(), Zoom::Zoom75);
        assert_eq!("100".parse::<Zoom>().unwrap(), Zoom::Zoom100);
        assert_eq!("125".parse::<Zoom>().unwrap(), Zoom::Zoom125);
        assert_eq!("invalid".parse::<Zoom>().unwrap(), Zoom::Zoom100);
    }
}
