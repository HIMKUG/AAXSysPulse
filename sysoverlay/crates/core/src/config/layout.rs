//! `layout_h.cfg` / `layout_v.cfg` —— 横版 / 竖版各自的**布局专属**配置。
//!
//! 横竖由「加载哪个文件」决定：`config_io::load_config` 依据 `system.cfg` 的
//! `layout` 锚点读取对应文件，并据此派生 `cfg.render_style`。
//!
//! 每个文件包含：
//! - `[window]`：窗口几何（位置 `x/y`、尺寸 `w/h`、锁定 `locked`）。
//!   ⚠️ 窗口位置（`x/y`）为**全局语义**，横竖共享同一份坐标——
//!   切换布局时不会跳位置（除非用户手动调），两个布局文件都写同一份坐标。
//!   位置/尺寸为 `-1` / `0` 表示「未定位 / 默认自然尺寸」。
//! - `[metrics]`：逐监控项 4 分量文字偏移 `offset.{id} = lx ly vx vy`
//!   （像素，可负；分量 0=名称X 1=名称Y 2=数值X 3=数值Y）。
//! - `order`：监控项显示顺序（与 `[metrics]` 共同描述当前布局专属的渲染排列）。
//!
//! 偏移和顺序都是**布局相关**的：横版与竖版下 4 个分量的语义/数值不同，
//! 故分别保存在各自的 layout 文件里。内存中[`Config`] 只持有**当前激活布局**
//! 的几何与偏移和顺序。

use crate::config::ini::{parse_u32, Ini, Writer};
use crate::config::{Config, OFFSET_MAX, OFFSET_MIN};
use crate::types::RenderStyle;

pub const LAYOUT_H_FILE: &str = "layout_h.cfg";
pub const LAYOUT_V_FILE: &str = "layout_v.cfg";

/// 返回某渲染风格对应的 layout 文件名。
pub fn file_for(style: RenderStyle) -> &'static str {
    match style {
        RenderStyle::Vertical => LAYOUT_V_FILE,
        _ => LAYOUT_H_FILE,
    }
}

pub fn serialize(_style: RenderStyle, cfg: &Config) -> String {
    let mut w = Writer::new();
    w.section("meta");
    w.kv("version", crate::config::CONFIG_VERSION);
    w.section("window");
    match cfg.window_pos {
        Some((x, y)) => {
            w.kv("x", x);
            w.kv("y", y);
        }
        None => {
            w.kv("x", -1);
            w.kv("y", -1);
        }
    }
    match cfg.window_size {
        Some((width, height)) => {
            w.kv("w", width);
            w.kv("h", height);
        }
        None => {
            w.kv("w", 0);
            w.kv("h", 0);
        }
    }
    w.kv("locked", cfg.size_locked as u8);

    w.section("metrics");
    w.comment("逐监控项 4 分量文字偏移 lx ly vx vy（像素，可负）");
    for id in 1..=7u32 {
        let idx = (id - 1) as usize;
        let off = cfg.metric_offsets[idx];
        w.kv(
            &format!("offset.{}", id),
            format!("{} {} {} {}", off[0], off[1], off[2], off[3]),
        );
    }

    // 监控项显示顺序（1-based ID 列表，与 [metrics] 同属布局专属）
    w.section("layout");
    let order_ids: Vec<String> = cfg.metric_order.iter().map(|&i| (i + 1).to_string()).collect();
    w.kv("order", order_ids.join(" "));

    w.into_string()
}

pub fn parse(_style: RenderStyle, cfg: &mut Config, text: &str) {
    let ini = Ini::parse(text);

    // [meta] version（用于跨文件一致性校验）
    if let Some(v) = ini.get("meta", "version") {
        cfg.version = parse_u32(v, cfg.version);
    }

    // [window] 几何已移至 system.rs，layout 文件的 [window] 段不再解析。
    // 此处只解析 [metrics] 偏移和 [layout] order。

    if let Some(sec) = ini.section("metrics") {
        for id in 1..=7u32 {
            let idx = (id - 1) as usize;
            let key = format!("offset.{}", id);
            if let Some(v) = sec.get(&key) {
                let mut arr = [0i32; 4];
                let mut got = 0;
                for slot in arr.iter_mut() {
                    if let Some(tok) = v.split_whitespace().nth(got) {
                        if let Ok(n) = tok.parse::<i32>() {
                            *slot = n.clamp(OFFSET_MIN, OFFSET_MAX);
                            got += 1;
                        }
                    }
                }
                if got == 4 {
                    cfg.metric_offsets[idx] = arr;
                }
            }
        }
    }

    // ── order：监控项显示顺序（1-based ID → 0-based 下标）──
    // 解析失败则保持 cfg 原有顺序（来自 default 或 monitor.cfg 旧值）
    if let Some(sec) = ini.section("layout") {
        if let Some(v) = sec.get("order") {
            let mut order = [0u8; 9];
            let mut count = 0;
            for tok in v.split_whitespace() {
                if let Ok(id) = tok.parse::<u32>() {
                    // v4: 支持 1..=9（1..=7 为原有监控项，8=DiskRead, 9=DiskWrite）
                    if (1..=9).contains(&id) && count < 9 {
                        order[count] = (id - 1) as u8;
                        count += 1;
                    }
                }
            }
            if count > 0 {
                // 填充不足 9 项的剩余槽：用**重复值**（build_rows 去重 guard 会跳过）。
                // 不可填 `i`（i=7/8 即 DiskRead/DiskWrite）——那会让旧配置解析后
                // 自动开启磁盘监控，违背「默认关闭」。
                for i in count..9 {
                    order[i] = order[0];
                }
                cfg.metric_order = order;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_roundtrip_geometry() {
        // window_pos/size 属于 system.cfg，layout 文件只保存 metric_offsets。
        // serialize 仍写 [window] 段（兼容性），但 parse 不读它。
        let mut cfg = Config::default();
        cfg.metric_offsets[2] = [5, -3, 7, 9];

        let text = serialize(RenderStyle::Strip, &cfg);
        assert!(text.contains("[window]"), "layout 文件写 [window] 段（兼容性占位）");
        assert!(text.contains("offset.3 = 5 -3 7 9"), "offset 序列化格式正确");

        let mut c2 = Config::default();
        c2.window_pos = Some((100, 200)); // 模拟 system.cfg 已加载的场景
        parse(RenderStyle::Strip, &mut c2, &text);
        assert_eq!(c2.metric_offsets[2], [5, -3, 7, 9], "offset 往返一致");
        assert_eq!(c2.window_pos, Some((100, 200)), "window_pos 来自 system.cfg，不被 layout parse 覆盖");
    }

    #[test]
    fn layout_order_roundtrips_through_layout_file() {
        // order 属于 layout 文件，不在 monitor 文件里
        let mut cfg = Config::default();
        cfg.metric_order = [8, 7, 6, 5, 4, 3, 2, 1, 0]; // 倒序，DiskWrite/DiskRead 在前
        let text = serialize(RenderStyle::Strip, &cfg);
        assert!(text.contains("order = 9 8 7 6 5 4 3 2 1"), "order 应为 1-based ID 列表");

        let mut c2 = Config::default();
        c2.metric_order = [0, 1, 2, 3, 4, 5, 6, 7, 8]; // 先改成别的
        parse(RenderStyle::Strip, &mut c2, &text);
        assert_eq!(c2.metric_order, [8, 7, 6, 5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn layout_negative_one_means_unpositioned() {
        // layout parse 不读写 window_pos（由 system.cfg 负责），所以 parse 后 window_pos 保持原值。
        let cfg = Config::default();
        let text = serialize(RenderStyle::Vertical, &cfg);
        let mut c2 = Config::default();
        c2.window_pos = Some((10, 10));
        parse(RenderStyle::Vertical, &mut c2, &text);
        assert_eq!(c2.window_pos, Some((10, 10)), "layout parse 不改变 window_pos");
    }
}
