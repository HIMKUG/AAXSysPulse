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
//!  3) 版本源：workspace 合并后版本号在根 Cargo.toml 的 [workspace.package]，
//!     草案读取的 sysoverlay/Cargo.toml 已不含 version 字段。
//!  4) cargo 可执行文件优先取外层注入的 $CARGO，回退 PATH 上的 "cargo"。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.join("../..");

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

    // 复制主程序 exe 到引导程序资源目录
    let main_exe = workspace_root.join("target/main-dist/release/AAXSysPulse.exe");
    let resource_dir = manifest_dir.join("resources");
    std::fs::create_dir_all(&resource_dir).ok();
    std::fs::copy(&main_exe, resource_dir.join("AAXSysPulse.exe"))
        .expect("failed to copy main program");

    // 2.3 版本号同步：读取主程序版本号，注入引导程序编译时环境变量
    let main_cargo = std::fs::read_to_string(workspace_root.join("Cargo.toml"))
        .expect("failed to read workspace root Cargo.toml");
    let version = parse_version(&main_cargo).expect("failed to parse main program version");
    println!("cargo:rustc-env=MAIN_PROGRAM_VERSION={}", version);

    // 三处版本号一致性断言（审查项 4.2.2 / 8B.M）：根 `Cargo.toml`
    // （`[workspace.package] version`，即上面注入的 `version`）、`guide-app/package.json`、
    // `src-tauri/tauri.conf.json`。三者此前**没有任何机制**保证同步，全靠人记得改，
    // 而 `tauri.conf.json` 的版本就是产物 `version` 字段（安装器/卸载注册表读它）。
    check_version_consistency(&manifest_dir, &version);
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=tauri.conf.json");

    // 驱动副本体检（审查项 8B.J / P0-8）：仓库里曾有 `src-tauri/driver/WinRing0x64.sys`
    // 是一个 1 字节的 NUL 占位假文件。tauri-build 只校验 bundle.resources 的**存在性**，
    // 一份被截断/占位的 .sys 照样能编译进安装器，装完才在启动服务时炸掉，且
    // "大小一致即跳过"的判据会让它永远无法自愈。这里在构建期就把它挡下来。
    check_driver_resource(&resource_dir.join("WinRing0x64.sys"));

    println!("cargo:rerun-if-changed=../sysoverlay/crates");
    println!("cargo:rerun-if-changed=build.rs");

    // 必须放在资源复制之后：tauri-build 会校验 bundle.resources 指向的文件存在
    tauri_build::build()
}

/// 三处版本声明必须相等：workspace `Cargo.toml`（`ws`）、`guide-app/package.json`、
/// `src-tauri/tauri.conf.json`，外加本 crate 的 `CARGO_PKG_VERSION`。
/// 不一致 ⇒ 拒绝构建，并在消息里点名该改哪几个文件。
fn check_version_consistency(manifest_dir: &std::path::Path, ws: &str) {
    let crates_version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        ws, crates_version,
        "版本漂移：workspace Cargo.toml [workspace.package] = {ws}，但 guide-app crate 的 \
         CARGO_PKG_VERSION = {crates_version}（该 crate 应继承 workspace 版本，勿单独写 version）"
    );
    for (label, path) in [
        ("guide-app/package.json", manifest_dir.join("../package.json")),
        ("src-tauri/tauri.conf.json", manifest_dir.join("tauri.conf.json")),
    ] {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不到 {label}：{}（{e}）", path.display()));
        match json_version(&text) {
            Some(v) => assert_eq!(
                v, ws,
                "版本漂移：{label} 的 \"version\" = {v}，但 workspace Cargo.toml = {ws} \
                 ⇒ 三处版本（workspace / package.json / tauri.conf.json）必须一起改"
            ),
            // 找不到 `"version"` 说明这份 JSON 的写法变了（或字段被删），断言同样失效：
            // 静默放过等于门禁形同虚设，故一并失败。
            None => panic!("{label} 里找不到 \"version\" 字段，构建期版本一致性断言无法工作"),
        }
    }
}

/// 取 JSON 顶层的第一个 `"version": "x.y.z"`（两份文件都把它放在靠前位置，
/// 手写解析而不引 serde_json：build.rs 不该为一次断言引入依赖）。
fn json_version(text: &str) -> Option<String> {
    let after = text.split("\"version\"").nth(1)?;
    let after = after.split_once(':')?.1;
    let v = after.trim().trim_start_matches('"').split('"').next()?;
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// 内置驱动必须是一个像样的 PE 镜像：存在、非占位、以 `MZ` 开头。
fn check_driver_resource(path: &std::path::Path) {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("驱动资源缺失：{}（{e}）", path.display()));
    assert!(
        bytes.starts_with(b"MZ") && bytes.len() > 4096,
        "驱动资源不可用：{}（{} 字节，MZ 头={}）—— 需要真实签名的 WinRing0x64.sys，占位/截断文件一律拒绝构建",
        path.display(),
        bytes.len(),
        bytes.starts_with(b"MZ")
    );
}

/// 取 [workspace.package] 段内的第一个 `version = "x.y.z"`
fn parse_version(cargo_toml: &str) -> Option<String> {
    let after = cargo_toml.split("[workspace.package]").nth(1)?;
    for line in after.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            break;
        }
        if let Some(rest) = t.strip_prefix("version") {
            let rest = rest.trim_start().strip_prefix('=')?;
            let v = rest.trim().trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}
