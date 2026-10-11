//! `renderer` 的单测（批次 6·⑩ P2-13：从 `renderer.rs` 原样搬出，未改一条断言）。
//!
//! 生产代码与测试分文件存放，是为了让 `renderer.rs` 只剩渲染逻辑；
//! 这里靠 `use super::*` 看见父模块的私有项，语义与原内联模块完全一致。
use super::*;

fn demo_cfg() -> Config {
    // 新默认值里 CPU/内存/GPU 显示是关的（用户拍板）；
    // 渲染测试关心的是"行的内容与几何"，与默认开关策略无关，
    // 这里显式打开以保持既有断言语义。
    let mut c = Config::default();
    c.show_cpu = true;
    c.show_mem = true;
    c.show_gpu = true;
    c
}

fn demo_snap() -> Snapshot {
    let mut s = Snapshot::placeholder(0);
    s.cpu = SnapMetric::Value(47.0);
    s.gpu = SnapMetric::Value(23.5);
    s.memory.load_pct = 58.0;
    s.memory.used_gb = 8.5;
    s.net.rx_bps = 1_200_000;
    s.net.tx_bps = 340_000;
    s
}

#[test]
fn renderers_render_pixels() {
    let snap = demo_snap();
    for renderer in [
        &StripRenderer as &dyn Renderer,
        &VerticalRenderer as &dyn Renderer,
    ] {
        let (w, h) = renderer.natural_size(&demo_cfg());
        let mut s = SoftwareSurface::new(w.max(100), h.max(20));
        renderer.draw(&mut s, &snap, &demo_cfg());
        let lit = s.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 0, "{} 渲染器必须画出像素（lit={}）", renderer.name(), lit);
    }
}

#[test]
fn alpha_and_values_change_hash() {
    let snap = demo_snap();
    let cfg = demo_cfg();
    let h1 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 90);
    let h2 = StripRenderer.frame_hash(&snap, &cfg, 300, 36, 91);
    assert_ne!(h1, h2, "alpha 变化必须改变指纹");

    let mut cfg2 = cfg;
    cfg2.net_unit = aaxsyspulse_core::types::NetUnit::GB;
    let h3 = StripRenderer.frame_hash(&snap, &cfg2, 300, 36, 90);
    assert_ne!(h1, h3, "单位切换必须改变指纹");

    let mut cfg3 = cfg;
    cfg3.font_size = 20.0;
    let h4 = StripRenderer.frame_hash(&snap, &cfg3, 300, 36, 90);
    assert_ne!(h1, h4, "字号变化必须改变指纹");

    let mut cfg4 = cfg;
    cfg4.metric_colors[metric_index(MetricEnum::Cpu)].label =
        Some(Rgb::new(255, 0, 0));
    let h5 = StripRenderer.frame_hash(&snap, &cfg4, 300, 36, 90);
    assert_ne!(h1, h5, "逐项配色变化必须改变指纹");
}

#[test]
fn metric_offsets_shift_cells_and_grow_natural_size() {
    let snap = demo_snap();
    let cfg = demo_cfg();
    // h=0 → 垂直居中被 PAD_Y 下限吞掉，基线对两版一致，便于逐值断言。
    let (nw, nh, cells) = StripRenderer.layout(&snap, &cfg, 0, 0);

    // CPU 的数值右移 5、下移 3；名称不动。
    let mut cfg2 = cfg;
    cfg2.metric_offsets[metric_index(MetricEnum::Cpu)] = [0, 0, 5, 3];
    let (nw2, nh2, cells2) = StripRenderer.layout(&snap, &cfg2, 0, 0);

    assert_eq!(cells2.len(), cells.len(), "cell 数量不随偏移变化");
    assert_eq!(
        (cells2[0].x, cells2[0].y),
        (cells[0].x, cells[0].y),
        "名称 cell 不受数值偏移影响"
    );
    assert_eq!(cells2[1].x, cells[1].x + 5, "数值 X 偏移 +5");
    assert_eq!(cells2[1].y, cells[1].y + 3, "数值 Y 偏移 +3");
    assert!(nw2 > nw, "正值 X 偏移必须扩宽自然宽度 {nw} → {nw2}");
    assert!(nh2 > nh, "正值 Y 偏移必须增高自然高度 {nh} → {nh2}");

    // 指纹随之改变（否则改偏移不触发重绘）。
    assert_ne!(
        StripRenderer.frame_hash(&snap, &cfg, 0, 0, 90),
        StripRenderer.frame_hash(&snap, &cfg2, 0, 0, 90),
        "偏移变化必须改变门面指纹"
    );

    // 竖版同样生效：数值右移 5。
    let (_, _, vcells) = VerticalRenderer.layout(&snap, &cfg, 0, 0);
    let (_, _, vcells2) = VerticalRenderer.layout(&snap, &cfg2, 0, 0);
    assert_eq!(vcells2[0].x, vcells[0].x, "竖版名称不动");
    assert_eq!(vcells2[1].x, vcells[1].x + 5, "竖版数值 X 偏移 +5");
    assert_eq!(vcells2[1].y, vcells[1].y + 3, "竖版数值 Y 偏移 +3");

    // 负偏移（向左上）也必须扩宽/增高，避免被窗口边缘裁掉。
    let mut cfg3 = cfg;
    cfg3.metric_offsets[metric_index(MetricEnum::Cpu)] = [-6, -4, 0, 0];
    let (nw3, nh3, _) = StripRenderer.layout(&snap, &cfg3, 0, 0);
    assert!(nw3 > nw, "负 X 偏移也要扩宽 {nw} → {nw3}");
    assert!(nh3 > nh, "负 Y 偏移也要增高 {nh} → {nh3}");
}

#[test]
fn renderer_names_are_stable() {
    assert_eq!(StripRenderer.name(), "strip");
    assert_eq!(VerticalRenderer.name(), "vertical");
}

#[test]
fn vertical_is_taller_than_strip() {
    let cfg = demo_cfg();
    let (_, sh) = StripRenderer.natural_size(&cfg);
    let (_, vh) = VerticalRenderer.natural_size(&cfg);
    assert!(vh > sh, "竖版必须比横版高（{sh} → {vh}）");
}

#[test]
fn per_metric_font_size_applies_to_mem() {
    // 回归锁：mfs_mem（逐项字号覆盖，索引 1）必须真实影响 Mem 行缩放。
    // （用户实测"字号调整对内存项目不生效"——渲染管线本身要保证生效。）
    let snap = demo_snap();
    let mut cfg = demo_cfg();
    let (_, _, base) = VerticalRenderer.layout(&snap, &cfg, 400, 400);
    let base_mem = base.iter().find(|c| c.text.as_str() == "Mem").expect("Mem label cell").scale;

    cfg.metric_font_sizes[metric_index(MetricEnum::Mem)] = Some(40.0);
    let (_, _, sized) = VerticalRenderer.layout(&snap, &cfg, 400, 400);
    let mem = sized.iter().find(|c| c.text.as_str() == "Mem").expect("Mem label cell");
    assert!((mem.scale - 40.0 / 16.0).abs() < 1e-3, "Mem 缩放应 = 40/16，实际 {}", mem.scale);
    assert!(mem.scale > base_mem, "覆盖字号必须大于默认");
}

#[test]
fn font_weight_fades_text_and_changes_hash() {
    let snap = demo_snap();
    let mut cfg = demo_cfg();
    cfg.font_weight = 2;
    let h_bold = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
    cfg.font_weight = 0;
    let h_thin = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
    assert_ne!(h_bold, h_thin, "字重变化必须改变帧指纹（否则切档不重绘）");

    let (_, _, cells) = StripRenderer.layout(&snap, &cfg, 400, 36);
    assert!(cells.iter().all(|c| c.color.a < 255), "细档文字必须半透明");
    assert!(cells.iter().all(|c| c.shade.a < 255), "阴影/描边须与文字同步弱化");
    cfg.font_weight = 2;
    let (_, _, cells) = StripRenderer.layout(&snap, &cfg, 400, 36);
    assert!(cells.iter().all(|c| c.color.a == 255), "粗档 = 原样不透明");
}

#[test]
fn gaps_change_hash() {
    let snap = demo_snap();
    let mut cfg = demo_cfg();
    cfg.metric_gap = 24;
    let h1 = StripRenderer.frame_hash(&snap, &cfg, 400, 36, 90);
    let mut cfg2 = cfg;
    cfg2.metric_gap = 8;
    let h2 = StripRenderer.frame_hash(&snap, &cfg2, 400, 36, 90);
    assert_ne!(h1, h2, "指标间距必须改变指纹");

    let mut cfg3 = cfg;
    cfg3.label_value_gap = 20;
    let h3 = StripRenderer.frame_hash(&snap, &cfg3, 400, 36, 90);
    assert_ne!(h1, h3, "名称值间距必须改变指纹");
}

#[test]
fn float_font_measure_matches_render() {
    // 最近邻字形宽度与渲染步进一致（不 panic、正数）
    let s = "CPU";
    let scale = font_scale_of(24.5);
    assert!(measure_scaled_f(s, scale) > 0);
    let mut surf = SoftwareSurface::new(200, 40);
    render_str_scaled_f(
        &mut surf.pixels,
        surf.width,
        surf.height,
        s,
        0,
        0,
        scale,
        Rgba::opaque(255, 255, 255),
    );
    assert!(surf.pixels.chunks_exact(4).any(|p| p[3] > 0), "必须画出内容");
}

#[test]
fn per_metric_font_size_affects_size() {
    let mut cfg = demo_cfg();
    let base = StripRenderer.natural_size(&cfg);
    cfg.metric_font_sizes[metric_index(MetricEnum::Cpu)] = Some(40.0);
    let grown = StripRenderer.natural_size(&cfg);
    assert!(
        grown.0 > base.0,
        "CPU 字号放大后横版必须变宽（{} → {}）",
        base.0,
        grown.0
    );
}

#[test]
fn net_both_row_keeps_download_and_upload_values() {
    // 回归锁：`fmt_rate` 内部会 `buf.clear()`。Both 模式若在同一缓冲上连续
    // 调用两次（中间只 push_str 分隔符），第二次会把刚拼好的
    // 「↓下载值/s ↑」整段清空，最终只剩上传值/s —— 用户实测「双向只显示
    // 一个值，且与 NetU 相同」。必须用独立缓冲分别格式化 rx/tx 再拼接。
    let mut cfg = demo_cfg();
    cfg.net_mode = NetMode::Both;
    cfg.net_unit = aaxsyspulse_core::types::NetUnit::MB;
    let mut snap = Snapshot::placeholder(0);
    snap.net.rx_bps = 11_850_000; // 11.85M（下载）
    snap.net.tx_bps = 240_000; // 0.24M（上传）

    let rows = build_rows(&snap, &cfg);
    let net = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::Net))
        .expect("Net 行必须存在");
    assert_eq!(net.label.as_str(), "Net", "双向模式标签为 Net");
    let s = net.value.as_str();
    assert!(s.contains('\u{2193}'), "双向必须含下载箭头 ↓：{s:?}");
    assert!(s.contains('\u{2191}'), "双向必须含上传箭头 ↑：{s:?}");
    assert!(s.contains("11.85M"), "双向必须含下载值 11.85M：{s:?}");
    assert!(s.contains("0.24M"), "双向必须含上传值 0.24M：{s:?}");
    assert!(
        s.find("11.85M").unwrap() < s.find("0.24M").unwrap(),
        "下载值须排在上传值之前：{s:?}"
    );
}

#[test]
fn net_single_direction_rows_show_only_own_value() {
    // 单向模式只显示对应方向的速率（下载 NetD / 上传 NetU）。
    let mut snap = Snapshot::placeholder(0);
    snap.net.rx_bps = 11_850_000;
    snap.net.tx_bps = 240_000;

    let mut cfg = demo_cfg();
    cfg.net_unit = aaxsyspulse_core::types::NetUnit::MB;
    cfg.net_mode = NetMode::Down;
    let rows = build_rows(&snap, &cfg);
    let d = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Net)).unwrap();
    assert_eq!(d.label.as_str(), "NetD");
    assert!(d.value.as_str().contains("11.85M"));
    assert!(!d.value.as_str().contains("0.24M"));

    cfg.net_mode = NetMode::Up;
    let rows = build_rows(&snap, &cfg);
    let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Net)).unwrap();
    assert_eq!(u.label.as_str(), "NetU");
    assert!(u.value.as_str().contains("0.24M"));
    assert!(!u.value.as_str().contains("11.85M"));
}

#[test]
fn value_cells_are_right_aligned_with_slot_covering_bbox() {
    // 右对齐数值 cell 必须标记 right_align，且包围盒向左覆盖整个预留槽位
    // ——D2D 真字形比位图度量宽时，右对齐修正会让 ink 向左溢出绘制原点 `x`，
    // 包围盒不覆盖就会留下幽灵像素。
    let snap = demo_snap();
    let cfg = demo_cfg();
    let (_, _, cells) = VerticalRenderer.layout(&snap, &cfg, 600, 600);

    let value = cells
        .iter()
        .find(|c| c.text.as_str().contains('%'))
        .expect("CPU 百分比数值 cell");
    assert!(value.right_align, "数值 cell 必须标记右对齐");
    assert!(
        value.bbox_x <= value.x,
        "右对齐数值包围盒左缘不得晚于绘制原点：bbox_x={} x={}",
        value.bbox_x,
        value.x
    );

    let label = cells
        .iter()
        .find(|c| c.text.as_str() == "CPU")
        .expect("CPU 标签 cell");
    assert!(!label.right_align, "标签 cell 不得右对齐");
}

#[test]
fn vertical_value_cells_share_one_right_edge() {
    // 竖版右对齐锚点一致性：所有数值 cell 的「绘制原点 + 位图度量宽」必须相等。
    // D2D 引擎正是按该锚点用 DWrite 实测宽度重定位，才能让整列数值右缘对齐
    // （否则比例字形宽度差异会让各行右缘漂移，网速串最明显）。
    let snap = demo_snap();
    let mut cfg = demo_cfg();
    cfg.net_mode = NetMode::Both;
    cfg.label_value_gap = 64;
    cfg.power_enabled = true;
    let (_, _, cells) = VerticalRenderer.layout(&snap, &cfg, 900, 900);

    let mut anchors: Vec<i32> = cells
        .iter()
        .filter(|c| c.right_align)
        .map(|c| c.x + measure_scaled_f(c.text.as_str(), c.scale))
        .collect();
    assert!(anchors.len() >= 5, "应当有 5 个数值 cell，实际 {}", anchors.len());
    anchors.sort_unstable();
    anchors.dedup();
    assert_eq!(
        anchors.len(),
        1,
        "所有数值 cell 必须共享同一右对齐锚点，实际 {anchors:?}"
    );
}

#[test]
fn time_row_renders_clock_text() {
    // 时钟行：默认显示，标签 "Time"，数值 = HH:MM:SS（槽位 8）。
    crate::clock::set_local(22, 12, 36);
    let cfg = demo_cfg();
    let rows = build_rows(&Snapshot::placeholder(0), &cfg);
    let t = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::Time))
        .expect("默认配置必须包含 Time 行");
    assert_eq!(t.label.as_str(), "Time");
    assert_eq!(t.value.as_str(), "22:12:36");
    assert_eq!(t.slot, TIME_SLOT_WITH_SEC);

    // 关闭秒 → HH:MM（槽位 5）
    let mut cfg2 = cfg;
    cfg2.time_show_seconds = false;
    let rows = build_rows(&Snapshot::placeholder(0), &cfg2);
    let t = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::Time))
        .expect("show_time=true 时必须有 Time 行");
    assert_eq!(t.value.as_str(), "22:12");
    assert_eq!(t.slot, TIME_SLOT_NO_SEC);

    // 完全关闭 → 无 Time 行
    let mut cfg3 = cfg;
    cfg3.show_time = false;
    let rows = build_rows(&Snapshot::placeholder(0), &cfg3);
    assert!(
        !rows.as_slice().iter().any(|r| matches!(r.metric, MetricEnum::Time)),
        "show_time=false 时不得出现 Time 行"
    );
}

#[test]
fn metric_order_drives_display_order() {
    // 顺序必须真实生效：把 Time 排到最前，行序随之改变。
    crate::clock::set_local(1, 2, 3);
    let mut cfg = demo_cfg();
    // 磁盘项（DiskRead/DiskWrite）默认在 metric_order 里，但 show_disk_* 默认关闭
    // → 默认不产出行；本测试用 0 填充末尾，只验证顺序语义。
    cfg.metric_order = [5, 0, 1, 2, 3, 4, 6, 0, 0];
    let rows = build_rows(&Snapshot::placeholder(0), &cfg);
    let seq: Vec<MetricEnum> = rows.as_slice().iter().map(|r| r.metric).collect();
    assert_eq!(
        seq,
        vec![
            MetricEnum::Time,
            MetricEnum::Cpu,
            MetricEnum::Mem,
            MetricEnum::Gpu,
            MetricEnum::Net,
            MetricEnum::Util
        ],
        "metric_order 必须决定显示顺序（Power 默认关，Util 不可用仍显示 --）"
    );

    // 时间文本变化 → 帧指纹必须变化（脏渲染依赖此性质）
    let h1 = StripRenderer.frame_hash(&Snapshot::placeholder(0), &cfg, 400, 36, 90);
    crate::clock::set_local(1, 2, 4);
    let h2 = StripRenderer.frame_hash(&Snapshot::placeholder(0), &cfg, 400, 36, 90);
    assert_ne!(h1, h2, "秒数变化必须改变帧指纹");
    // 同一秒内 → 指纹稳定（跳帧省渲染的前提）
    let h3 = StripRenderer.frame_hash(&Snapshot::placeholder(0), &cfg, 400, 36, 90);
    assert_eq!(h2, h3, "同一秒内指纹必须稳定");
}

#[test]
fn util_row_states_and_colors() {
    // Util 行三态：正常值 / 降级（部分源缺失）/ 不可用（全源缺失）。
    let cfg = demo_cfg();
    assert!(cfg.show_util, "Util 默认显示");

    // 不可用（启动首帧）：显示红色 "--"，行仍占位。
    let rows = build_rows(&Snapshot::placeholder(0), &cfg);
    let u = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::Util))
        .expect("show_util=true 时必须有 Util 行");
    assert_eq!(u.label.as_str(), "Util");
    assert_eq!(u.value.as_str(), "--");
    assert_eq!(u.value_color, Style::default().accent(100.0), "不可用必须红色");

    // 正常值：占用率强调色（47.0% → 绿档；Util 显示一位小数，见 fmt_pct1）。
    let mut snap = Snapshot::placeholder(0);
    snap.util = SnapMetric::Value(47.0);
    snap.util_partial = false;
    let rows = build_rows(&snap, &cfg);
    let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Util)).unwrap();
    assert_eq!(u.value.as_str(), "47.0%");
    assert_eq!(u.value_color, Style::default().accent(47.0));

    // 降级（GPU 缺失重归一化）：陈旧色。
    let mut snap = Snapshot::placeholder(0);
    snap.util = SnapMetric::Value(61.0);
    snap.util_partial = true;
    let rows = build_rows(&snap, &cfg);
    let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Util)).unwrap();
    assert_eq!(u.value_color, Style::default().stale_accent(61.0), "降级值必须用陈旧色");

    // 关闭 → 行消失。
    let mut cfg2 = cfg;
    cfg2.show_util = false;
    let rows = build_rows(&Snapshot::placeholder(0), &cfg2);
    assert!(
        !rows.as_slice().iter().any(|r| matches!(r.metric, MetricEnum::Util)),
        "show_util=false 时不得出现 Util 行"
    );
}

#[test]
fn util_short_label_and_slot() {
    // 简称 U；数值槽为一位小数百分比（"99.9%" 5 字符，见 fmt_pct1）。
    let mut cfg = demo_cfg();
    cfg.short_names = true;
    let mut snap = Snapshot::placeholder(0);
    snap.util = SnapMetric::Value(100.0);
    let rows = build_rows(&snap, &cfg);
    let u = rows.as_slice().iter().find(|r| matches!(r.metric, MetricEnum::Util)).unwrap();
    assert_eq!(u.label.as_str(), "U");
    assert_eq!(u.value.as_str(), "100.0%");
    assert_eq!(u.slot, SLOT_PCT1);
}

/// `layout` 预算门禁（恢复计划 P4.3，照 build_pages 门禁模式）：
/// layout 是渲染线程**每帧**路径（P0 实测 seg_layout p50=6µs / max=16µs），
/// 预算锁回归——有人往 build_rows / cell_sig 循环里塞分配/重活立刻可见，
/// 并在 release 下打印真实耗时供决策参考。
#[test]
fn overlay_layout_is_within_budget() {
    let snap = demo_snap();
    let cfg = demo_cfg();
    // 预热（首跑含查表/分配冷路径，不代表稳态）。
    let _ = StripRenderer.layout(&snap, &cfg, 400, 36);
    let _ = VerticalRenderer.layout(&snap, &cfg, 400, 400);

    // 两个渲染器各测 50 次取最小值（排除调度抖动）。
    let mut best_strip = f64::MAX;
    let mut best_vertical = f64::MAX;
    for _ in 0..50 {
        let t = std::time::Instant::now();
        let _ = StripRenderer.layout(&snap, &cfg, 400, 36);
        best_strip = best_strip.min(t.elapsed().as_secs_f64() * 1e6);

        let t = std::time::Instant::now();
        let _ = VerticalRenderer.layout(&snap, &cfg, 400, 400);
        best_vertical = best_vertical.min(t.elapsed().as_secs_f64() * 1e6);
    }
    println!(
        "overlay layout strip={best_strip:.0}µs vertical={best_vertical:.0}µs (release 下供参考)"
    );
    // 预算先实测再定值：release 实测 ~6µs（P0 seg_layout）。500µs ≈ 80× 余量，
    // debug 测试构建也远够——锁「往每帧路径加重活」的回归即可。
    assert!(best_strip < 500.0, "横版布局超预算：{best_strip:.0}µs");
    assert!(best_vertical < 500.0, "竖版布局超预算：{best_vertical:.0}µs");
}

/// 回归：磁盘读写行**必须占位**。
///
/// 用户报障（2026-09-17）：「磁盘读写在悬浮窗没有占位格 / 悬浮窗不显示」。
/// 旧实现 `if !snap.disk.ready { return None; }` 会在数据未就绪时把整行撤掉
/// ——其他指标（CPU/GPU/Util）不可用时都仍占位显示 `--`，只有磁盘整行消失。
#[test]
fn disk_rows_always_reserve_a_cell() {
    let mut cfg = demo_cfg();
    cfg.show_disk_read = true;
    cfg.show_disk_write = true;
    cfg.metric_order = [7, 8, 0, 1, 2, 6, 3, 5, 4]; // 磁盘排最前，便于定位

    // ① 未就绪（PDH 首轮基线 / 数据源不可用）：仍占位，数值 `--`，槽位仍 9 字符。
    let rows = build_rows(&Snapshot::placeholder(0), &cfg);
    let rd = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::DiskRead))
        .expect("未就绪时磁盘读行必须占位（不能整行消失）");
    let wr = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::DiskWrite))
        .expect("未就绪时磁盘写行必须占位（不能整行消失）");
    assert_eq!(rd.value.as_str(), "--");
    assert_eq!(wr.value.as_str(), "--");
    assert_eq!(rd.slot, 9);
    assert_eq!(wr.slot, 9);
    assert_eq!(rd.label.as_str(), "RDisk");
    assert_eq!(wr.label.as_str(), "WDisk");

    // ② 就绪：显示真实速率，颜色回到常规数值色。
    let mut snap = Snapshot::placeholder(0);
    snap.disk.ready = true;
    snap.disk.read_bps = 1_048_576.0; // 1.0MB/s
    snap.disk.write_bps = 1_073_741_824.0; // 1.00GB/s
    let rows = build_rows(&snap, &cfg);
    let rd = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::DiskRead))
        .unwrap();
    let wr = rows
        .as_slice()
        .iter()
        .find(|r| matches!(r.metric, MetricEnum::DiskWrite))
        .unwrap();
    assert_eq!(rd.value.as_str(), "1.0MB/s");
    assert_eq!(wr.value.as_str(), "1.00GB/s");
    assert_eq!(rd.value_color, Style::derive(Rgb::default(), cfg.fg).ui.value);

    // ③ 显式关掉开关才不占位（只关读，写仍占位）。
    let mut off = cfg;
    off.show_disk_read = false;
    let rows = build_rows(&Snapshot::placeholder(0), &off);
    assert!(
        !rows.as_slice().iter().any(|r| matches!(r.metric, MetricEnum::DiskRead)),
        "show_disk_read=false 时不得出现磁盘读行"
    );
    assert!(
        rows.as_slice().iter().any(|r| matches!(r.metric, MetricEnum::DiskWrite)),
        "只关读时磁盘写行仍应占位"
    );
}
