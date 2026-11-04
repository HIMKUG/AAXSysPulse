//! 引导程序构建脚本 —— 第二部分「同源编译机制」。
//! 先把主程序 aaxsyspulse-app 在当前 Cargo workspace 里编译出来，
//! 复制 release exe 到本 crate 的 resources/（供 tauri.conf bundle.resources 打包），
//! 注入主程序版本号到编译时环境变量，最后执行标准 tauri_build::build()。
//!
//! 与提示词草案的偏差（显式标注）：
//!  1) 内层 cargo 指定独立 `--target-dir target/main-dist`：同一 workspace 内嵌套调用
//!     cargo 若共用默认 root/target，会等待外层已持有的构建锁 → 死锁。
//!  2) 草案的 `cargo:rerun-if-changed=../sysoverlay/src` 路径不存在，改为真实源码目录
//!     `../sysoverlay/crates`。
//!  3) 版本源（续篇 802/807/808）：本 crate `version.workspace = true` 继承 workspace，
//!     `CARGO_PKG_VERSION` 就是同一真源 —— 手写 parse_version 解析根 Cargo.toml
//!     （split("[workspace.package]") / strip_prefix("version")）对注释与键序脆弱，
//!     已删除，两个续篇编号随之闭合。
//!  4) cargo 可执行文件优先取外层注入的 $CARGO，回退 PATH 上的 "cargo"。

use std::path::PathBuf;
use std::process::Command;

// 首轮 266 / 续篇 804：资源名从契约取，不再手写 MAIN_EXE_NAME / DRIVER_FILE_NAME
// 的第二、三份字面量副本。
use aaxsyspulse_core::contract as c;

/// 驱动体检最小体积（首轮 266：裸 4096 提为具名常量）。
const MIN_DRIVER_BYTES: usize = 4096;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.join("../..");

    // 续篇 801：cargo check / cargo test（PROFILE=debug）不需要主程序 exe ——
    // 无条件 `cargo build --release` 让每次检查都完整编译 40MB 主程序。
    // 只有 release（打包路径）才做同源编译；debug 路径用 CARGO_PKG_VERSION
    // 注入版本号，保证 lib.rs 的 env! 可编译。
    let is_release = std::env::var("PROFILE").as_deref() == Ok("release");
    if !is_release {
        println!(
            "cargo:warning=非 release 构建：跳过主程序同源编译（仅 tauri build 打包需要），\
             MAIN_PROGRAM_VERSION 取 CARGO_PKG_VERSION"
        );
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
        println!("cargo:rustc-env=MAIN_PROGRAM_VERSION={}", version);
        println!("cargo:rerun-if-changed=build.rs");
        tauri_build::build();
        return;
    }

    // 编译主程序
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "-p",
            "aaxsyspulse-app",
            "--target-dir",
            "target/main-dist",
        ])
        .current_dir(&workspace_root)
        .status()
        .expect("failed to build main program");

    assert!(status.success(), "main program build failed");

    // 复制主程序 exe 到引导程序资源目录（文件名走契约常量）
    let main_exe = workspace_root.join(format!("target/main-dist/release/{}", c::MAIN_EXE_NAME));
    let resource_dir = manifest_dir.join("resources");
    std::fs::create_dir_all(&resource_dir).ok();
    std::fs::copy(&main_exe, resource_dir.join(c::MAIN_EXE_NAME))
        .expect("failed to copy main program");

    // 2.3 版本号同步：版本源 = CARGO_PKG_VERSION（= workspace version，续篇 802）
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION 由 cargo 注入");
    println!("cargo:rustc-env=MAIN_PROGRAM_VERSION={}", version);

    // 三处版本号一致性断言（审查项 4.2.2 / 8B.M）：`guide-app/package.json`、
    // `src-tauri/tauri.conf.json` 必须与 crate 版本一致。
    check_version_consistency(&manifest_dir, &version);
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    // 首轮 266：resources/ 里的驱动副本是体检对象，改动后必须重跑体检。
    println!("cargo:rerun-if-changed=resources");

    // 驱动副本体检（审查项 8B.J / P0-8）：仓库里曾有 `src-tauri/driver/WinRing0x64.sys`
    // 是一个 1 字节的 NUL 占位假文件。tauri-build 只校验 bundle.resources 的**存在性**，
    // 一份被截断/占位的 .sys 照样能编译进安装器，装完才在启动服务时炸掉，且
    // "大小一致即跳过"的判据会让它永远无法自愈。这里在构建期就把体检做掉。
    // 续篇 806：缺资源**不再 panic** —— 该文件被 .gitignore 全局忽略，全新 clone
    // / CI 的 cargo check/test 必然没有它；降级为 cargo:warning 继续构建，
    // 运行期由 tauri 打包校验与安装日志兜底。
    check_driver_resource(&resource_dir.join(c::DRIVER_FILE_NAME));

    println!("cargo:rerun-if-changed=../sysoverlay/crates");
    println!("cargo:rerun-if-changed=build.rs");

    // 必须放在资源复制之后：tauri-build 会校验 bundle.resources 指向的文件存在
    tauri_build::build()
}

/// 三处版本声明必须相等：crate 的 `CARGO_PKG_VERSION`（继承 workspace）、
/// `guide-app/package.json`、`src-tauri/tauri.conf.json`。
/// 不一致 ⇒ 拒绝构建，并在消息里点名该改哪几个文件。
fn check_version_consistency(manifest_dir: &std::path::Path, version: &str) {
    for (label, path) in [
        ("guide-app/package.json", manifest_dir.join("../package.json")),
        ("src-tauri/tauri.conf.json", manifest_dir.join("tauri.conf.json")),
    ] {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {label}：{}（{e}）", path.display()));
        match json_version(&text) {
            Some(v) => assert_eq!(
                v, version,
                "版本漂移：{label} 的 \"version\" = {v}，但 workspace Cargo.toml = {version} \
                 ⇒ 三处版本（workspace / package.json / tauri.conf.json）必须一起改"
            ),
            // 找不到 `"version"` 说明这份 JSON 的写法变了（或字段被删），断言同样失效：
            // 静默放过等于门禁形同虚设，故一并失败。
            None => panic!("{label} 里找不到 \"version\" 字段，构建期版本一致性断言无法工作"),
        }
        // 续篇 759：窗口尺寸在 tokens.css 的 --win-w/--win-h 里有第二份副本 ——
        // 在这里钉死，改 tauri.conf.json 的窗口尺寸时必须同步改 CSS。
        if let Some((w, h)) = json_window_size(&text) {
            assert_eq!(
                (w, h),
                (680, 560),
                "窗口尺寸漂移：{label} = {w}x{h}，但 tokens.css 的 --win-w/--win-h 是 680/560 \
                 ⇒ 两处必须一起改"
            );
        }
    }
}

/// 取 JSON **顶层**的 `"version": "x.y.z"`（续篇 805：全文 split("version") 会命中
/// 任意层级的同名键 —— 只认顶层缩进 2 空格的键行）。
fn json_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if indent != 2 {
            continue;
        }
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("\"version\"") {
            let rest = rest.trim_start().strip_prefix(':')?;
            let v = rest.trim().trim_start_matches('"').split('"').next()?;
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 取顶层 `"width": N` / `"height": N`（app.windows[0]，缩进 4 空格的键行）。
fn json_window_size(text: &str) -> Option<(u32, u32)> {
    let mut w = None;
    let mut h = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if indent != 4 {
            continue;
        }
        let t = line.trim();
        let value_of = |key: &str| -> Option<u32> {
            let rest = t.strip_prefix(key)?;
            let rest = rest.trim_start().strip_prefix(':')?;
            let v: String = rest
                .trim()
                .trim_end_matches(',')
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            v.parse::<u32>().ok()
        };
        if let Some(n) = value_of("\"width\"") {
            w = Some(n);
        }
        if let Some(n) = value_of("\"height\"") {
            h = Some(n);
        }
    }
    Some((w?, h?))
}

/// 内置驱动必须是一个像样的 PE 镜像：`MZ` 头 + e_lfanew 指向 `PE\0\0`
/// （首轮 266：至少校验 PE 头，不再只看 MZ + 裸阈值）。
/// 续篇 806：任何不满足都以 `cargo:warning=` 报告并继续构建（资源被 .gitignore
/// 忽略时全新 clone / CI 必然缺文件，panic 会挂掉全部 check/test）。
fn check_driver_resource(path: &std::path::Path) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "cargo:warning=驱动资源缺失：{}（{e}）—— 构建继续，但本产物无法安装驱动 \
                （需要真实 WinRing0x64.sys，见仓库 README 的运行前提）",
                path.display()
            );
            return;
        }
    };
    let pe_ok = pe_header_ok(&bytes);
    if !(bytes.len() > MIN_DRIVER_BYTES && bytes.starts_with(b"MZ") && pe_ok) {
        println!(
            "cargo:warning=驱动资源不可用：{}（{} 字节，MZ 头={}，PE 头={pe_ok}）—— \
             需要真实签名的 WinRing0x64.sys，占位/截断文件一律拒绝打包",
            path.display(),
            bytes.len(),
            bytes.starts_with(b"MZ")
        );
    }
}

/// DOS 头 0x3C 处的 e_lfanew 指向的 4 字节必须是 `PE\0\0`。
fn pe_header_ok(bytes: &[u8]) -> bool {
    if bytes.len() < 0x40 {
        return false;
    }
    let lfanew = u32::from_le_bytes([bytes[0x3c], bytes[0x3d], bytes[0x3e], bytes[0x3f]]) as usize;
    bytes.get(lfanew..lfanew + 4).map(|s| s == b"PE\0\0").unwrap_or(false)
}
