//! Build script for aaxsyspulse-app
//!
//! 只做一件事：把应用图标（`crates/win/assets/app_icon.ico`，多尺寸 16/64/128）
//! 编成 Windows 资源链接进 exe —— 这样资源管理器、任务栏、属性面板看到的都是真实图标，
//! 而不是系统默认的 exe 图标。
//!
//! 曾经这里还有一段"检查 `../driver-installer/bin/driver-installer.exe` 是否存在，
//! 并提示运行 `driver-installer/build.bat` 去构建"的逻辑（批次 7 删除，审查项 9.6 的
//! **真正源头**）：
//!
//! - 那个 exe 从未存在过 ⇒ `else` 分支**每次构建都发一条 warning**，让每个编译过
//!   本仓库的人（含写 README 的那位）都以为存在一套 Zig/CMake 的 `build.bat` 构建流程；
//! - 注释承诺的"真正的嵌入走 `include_bytes!`"没有对应代码（全仓 `include_bytes!` 只有
//!   图标两处），唯一的消费者 `driver_installer.rs` 又从没被 `mod` 声明（死文件，见 §11 决策）；
//! - 实际的驱动安装由**引导程序**做：`guide-app/src-tauri/src/driver.rs` +
//!   `install.rs`，主程序只负责运行期打开 `\\.\WinRing0_1_2_0`（`win/src/msr.rs`）。

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    embed_app_icon();
}

/// 用 Windows SDK 的 `rc.exe` 把 ICO 编成 `.res`，再作为链接参数交给 link.exe。
///
/// 任何一步失败都只发 warning 并继续 —— 图标缺失不影响程序构建与运行。
fn embed_app_icon() {
    let ico = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("win")
        .join("assets")
        .join("app_icon.ico");
    if !ico.exists() {
        println!("cargo:warning=app icon not found at {}", ico.display());
        return;
    }

    let Some(rc) = find_rc_exe() else {
        println!("cargo:warning=rc.exe not found; skipping exe icon resource");
        return;
    };

    let Ok(out_dir) = env::var("OUT_DIR") else { return };
    let out_dir = PathBuf::from(out_dir);
    let rc_file = out_dir.join("app_icon.rc");
    let res_file = out_dir.join("app_icon.res");

    // rc.exe 对路径分隔符宽容，但统一用 `/` 最稳；路径用绝对路径避免 cwd 依赖。
    let ico_abs = ico.to_string_lossy().replace('\\', "/");
    if std::fs::write(&rc_file, format!("1 ICON \"{}\"\n", ico_abs)).is_err() {
        println!("cargo:warning=failed to write {}", rc_file.display());
        return;
    }

    let ok = Command::new(&rc)
        .args([
            "/nologo",
            "/fo",
            res_file.to_str().unwrap_or_default(),
            rc_file.to_str().unwrap_or_default(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ok && res_file.exists() {
        // 只给 bin 目标链接（本 crate 没有 lib 目标，用 -bins 更明确）。
        println!("cargo:rustc-link-arg-bins={}", res_file.display());
        println!("cargo:rerun-if-changed={}", ico.display());
    } else {
        println!("cargo:warning=rc.exe failed to compile {}", rc_file.display());
    }
}

/// 在 Windows SDK 里找 x64 的 `rc.exe`，取版本号最大的那个。
fn find_rc_exe() -> Option<PathBuf> {
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let entries = std::fs::read_dir(kits).ok()?;
    let mut best: Option<(u32, PathBuf)> = None;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // 形如 10.0.22621.0 —— 取第三段作为 build 号排序
        let parts: Vec<u32> = name.split('.').filter_map(|p| p.parse().ok()).collect();
        let build = parts.get(2).copied().unwrap_or(0);
        let p = e.path().join("x64").join("rc.exe");
        if p.exists() && best.as_ref().map(|(b, _)| build > *b).unwrap_or(true) {
            best = Some((build, p));
        }
    }
    best.map(|(_, p)| p)
}
