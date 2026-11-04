//! `gdi` 脏矩形路径的单测（批次 6·⑩ P2-13：从 `gdi.rs` 原样搬出，未改一条断言）。
//!
//! 与父模块同义：`use super::*` 直接取 `gdi.rs` 的私有项（脏区累计、快照比较等）。
use super::*;
use aaxsyspulse_core::units::StrBuf;
use aaxsyspulse_ui::Rgba;

/// 最小可比对 CellSig：脏判定只看 x/y/bbox_x/w/h/hash。
fn cell(x: i32, hash: u64) -> CellSig {
    cell_ra(x, x, false, hash)
}

/// 审查 556/557：可独立指定 bbox_x / right_align 的构造器 —— 旧夹具把
/// bbox_x 恒写成 x，无法构造右对齐格"墨迹左缘与绘制原点分离"的情形。
fn cell_ra(x: i32, bbox_x: i32, right_align: bool, hash: u64) -> CellSig {
    CellSig {
        x,
        y: 8,
        w: 40,
        h: 28,
        bbox_x,
        right_align,
        hash,
        text: StrBuf::new(),
        color: Rgba::default(),
        shade: Rgba::default(),
        shadow: false,
        outline: false,
        scale: 1.0,
    }
}

fn fresh_state() -> (OnceLock<Mutex<Vec<CellSig>>>, AtomicU8) {
    (OnceLock::new(), AtomicU8::new(0))
}

fn run(
    state: &(OnceLock<Mutex<Vec<CellSig>>>, AtomicU8),
    cells: &[CellSig],
) -> (Dirty, Vec<bool>) {
    compute_dirty_in(&state.0, &state.1, cells, 255)
}

#[test]
fn first_frame_is_full() {
    let st = fresh_state();
    let (d, changed) = run(&st, &[cell(0, 1), cell(100, 2)]);
    assert!(d.is_full());
    assert!(changed.iter().all(|&c| c), "首帧必须全部重画");
}

#[test]
fn two_far_changed_cells_yield_two_rects() {
    let st = fresh_state();
    run(&st, &[cell(0, 1), cell(100, 2), cell(300, 3)]); // 建立基线
    // 第 0、2 格变化（相距远，中间隔第 1 格）→ 2 个矩形，空隙不上传
    let cells = [cell(0, 99), cell(100, 2), cell(300, 98)];
    let (d, changed) = run(&st, &cells);
    let Dirty::Rects(rs) = d else { panic!("应为 Rects，实际 {}", fmt_dirty(&d)) };
    assert_eq!(rs.len(), 2, "相距远的变化格不得合并");
    assert!(rs[0].right <= rs[1].left, "矩形之间必须隔开空隙");
    assert!(changed[0] && changed[2] && !changed[1], "邻接传播不得标记不相交邻居");
}

#[test]
fn adjacent_changed_cells_merge_into_one_rect() {
    let st = fresh_state();
    // ⚠️ cells_intersect 用半开区间严格判定（right/bottom 边界不算相交）：
    // 「相触」（0..40 与 40..80）**不算**相交，必须真正重叠才并成一簇。
    run(&st, &[cell(0, 1), cell(30, 2)]); // 包围盒 0..40 与 30..70 重叠
    let cells = [cell(0, 99), cell(30, 98)];
    let (d, _) = run(&st, &cells);
    let Dirty::Rects(rs) = d else { panic!("应为 Rects") };
    assert_eq!(rs.len(), 1, "重叠的变化格必须并成一簇");
    assert_eq!((rs[0].left, rs[0].right), (0, 70));
}

#[test]
fn unchanged_neighbor_is_not_uploaded() {
    // 传播邻居（changed=true）不应扩大上传矩形：上传只由内容变化格构成
    let st = fresh_state();
    // 两格重叠：0..40 与 20..60（真重叠才会触发邻接传播）；基线后只改第 0 格
    run(&st, &[cell(0, 1), cell(20, 2)]);
    let cells = [cell(0, 99), cell(20, 2)];
    let (d, changed) = run(&st, &cells);
    let Dirty::Rects(rs) = d else { panic!("应为 Rects") };
    assert_eq!(rs.len(), 1);
    assert_eq!((rs[0].left, rs[0].right), (0, 40), "未变化的邻居格不得进入上传矩形");
    assert!(changed[1], "但 paint 位图要标记邻居（clear_rect 擦除防线）");
}

#[test]
fn no_change_yields_empty_rects_not_full() {
    let st = fresh_state();
    let cells = [cell(0, 1), cell(100, 2)];
    run(&st, &cells);
    let (d, _) = run(&st, &cells); // 同内容再来一帧
    let Dirty::Rects(rs) = d else { panic!("应为 Rects") };
    assert!(rs.is_empty(), "无变化必须是空矩形列表（跳过上屏）");
}

#[test]
fn alpha_change_is_full() {
    let st = fresh_state();
    run(&st, &[cell(0, 1)]);
    let (d, _) = compute_dirty_in(&st.0, &st.1, &[cell(0, 1)], 128); // alpha 变了
    assert!(d.is_full());
}

#[test]
fn position_drift_is_full() {
    let st = fresh_state();
    run(&st, &[cell(0, 1)]);
    let (d, _) = run(&st, &[cell(10, 1)]); // x 漂移
    assert!(d.is_full());
}

/// 审查 559：右对齐数值格的墨迹左缘（bbox_x）单独漂移也必须整窗
/// （gdi.rs 布局漂移比较补 bbox_x/w/h 的直接回归锁）。
#[test]
fn bbox_only_drift_is_full() {
    let st = fresh_state();
    run(&st, &[cell_ra(10, 0, true, 1)]);
    let (d, _) = run(&st, &[cell_ra(10, -8, true, 1)]); // 仅 bbox_x 变
    assert!(d.is_full(), "bbox_x 单独漂移必须整窗");
}

/// 审查 557：右对齐格（right_align=true）的脏区行为覆盖 ——
/// bbox_x < x（墨迹左溢）时不误判、bbox_x 变化时整窗。
#[test]
fn right_align_cells_dirty_behavior() {
    let st = fresh_state();
    // 基线：右对齐格，绘制原点 x=100、墨迹左缘 bbox_x=60
    run(&st, &[cell_ra(100, 60, true, 1)]);
    // 同几何同内容 → 空 Rects
    let (d, _) = run(&st, &[cell_ra(100, 60, true, 1)]);
    let Dirty::Rects(rs) = &d else { panic!("应为 Rects") };
    assert!(rs.is_empty(), "未变的右对齐格必须跳过上屏");
    // 仅 bbox_x 变（槽位变化导致墨迹左移）→ 整窗
    let (d, _) = run(&st, &[cell_ra(100, 52, true, 1)]);
    assert!(d.is_full(), "右对齐格 bbox_x 漂移必须整窗");
}

#[test]
fn too_many_components_folds_to_full() {
    let st = fresh_state();
    let base: Vec<CellSig> = (0..10).map(|i| cell(i * 100, i as u64 + 1)).collect();
    run(&st, &base);
    let changed: Vec<CellSig> = (0..10).map(|i| cell(i * 100, i as u64 + 100)).collect();
    let (d, _) = run(&st, &changed);
    assert!(d.is_full(), "超过 MAX_DIRTY_RECTS(8) 必须折叠整窗");
}

/// 审查 560：恰好 7/8/9 个互不相邻变化格的折叠边界（上限 8）。
#[test]
fn dirty_rect_count_boundaries() {
    for (n, expect_full) in [(7usize, false), (8usize, false), (9usize, true)] {
        let st = fresh_state();
        let base: Vec<CellSig> = (0..n).map(|i| cell((i * 100) as i32, i as u64 + 1)).collect();
        run(&st, &base);
        let changed: Vec<CellSig> =
            (0..n).map(|i| cell((i * 100) as i32, i as u64 + 100)).collect();
        let (d, _) = run(&st, &changed);
        assert_eq!(d.is_full(), expect_full, "n={n} 的折叠判定错误");
    }
}

#[test]
fn upload_failure_gate_forces_full_next_frame() {
    // 审查 561：断言中途 panic 会把真实全局闸门留在 true、污染后续测试
    // —— 用 RAII 保证复位（其余状态都用 fresh_state，唯独这里是全局）。
    struct ResetGate;
    impl Drop for ResetGate {
        fn drop(&mut self) {
            NEEDS_FULL_REUPLOAD.store(false, AtomicOrdering::Relaxed);
        }
    }
    let _guard = ResetGate;
    let st = fresh_state();
    let cells = [cell(0, 1), cell(100, 2)];
    run(&st, &cells);
    run(&st, &cells);
    NEEDS_FULL_REUPLOAD.store(true, AtomicOrdering::Relaxed);
    let (d, changed) = run(&st, &cells);
    assert!(d.is_full(), "闸门置位后下一帧必须整窗");
    assert!(changed.iter().all(|&c| c));
    // 闸门一次性：消费后回到正常路径
    assert!(!NEEDS_FULL_REUPLOAD.load(AtomicOrdering::Relaxed));
}

#[test]
fn dirty_autocutoff_boundary() {
    assert!(!should_autocutoff(DIRTY_AUTOCUTOFF_STREAK - 1));
    assert!(should_autocutoff(DIRTY_AUTOCUTOFF_STREAK));
    assert!(should_autocutoff(u32::MAX));
}
