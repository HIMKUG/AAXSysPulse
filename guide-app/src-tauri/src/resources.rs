//! 资源文件模块
//! 运行时从 Tauri 打包资源读取主程序与驱动文件（不再 include_bytes! 内嵌，
//! 见第二部分：主程序由 build.rs 同源编译后作为 bundle.resources 打包）。
//!
//! 偏差标注：tauri.conf.json 的 bundle.resources 采用「映射」形式
//! （"resources/AAXSysPulse.exe" → "AAXSysPulse.exe"，产物平铺在资源根），
//! 故运行时相对路径是裸文件名，而非草案中的 "resources/AAXSysPulse.exe"。

use crate::install::InstallError;
use tauri::{AppHandle, Manager};

/// 主程序 AAXSysPulse.exe 的字节数据（资源名 = 契约常量，见文件尾的 JSON 对齐测试）
pub fn get_main_program_bytes(app: &AppHandle) -> Result<Vec<u8>, InstallError> {
    get_resource_bytes(app, aaxsyspulse_core::contract::MAIN_EXE_NAME)
}

/// 驱动文件 WinRing0x64.sys 的字节数据
pub fn get_driver_bytes(app: &AppHandle) -> Result<Vec<u8>, InstallError> {
    get_resource_bytes(app, aaxsyspulse_core::contract::DRIVER_FILE_NAME)
}

fn get_resource_bytes(app: &AppHandle, name: &str) -> Result<Vec<u8>, InstallError> {
    let resource_path = app.path().resolve(
        name,
        tauri::path::BaseDirectory::Resource,
    ).map_err(|e| InstallError::Resource(format!("{name}: {e}")))?;
    std::fs::read(resource_path).map_err(InstallError::from)
}

#[cfg(test)]
mod tests {
    use aaxsyspulse_core::contract as c;

    /// 打包资源配置（`tauri.conf.json` 的 `bundle.resources`）是**唯一一处**无法
    /// 引用 Rust 常量的副本 —— JSON 吃不了常量。于是把它钉进测试：
    /// 改契约里的 exe / 驱动文件名而不改 conf，安装会在运行期才炸，这条测试先红。
    /// （审查项 7.1.2 / P1-22 的收尾。）
    #[test]
    fn bundled_resource_names_match_contract() {
        let conf = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
            .expect("tauri.conf.json 必须与 crate 同目录");
        for name in [c::MAIN_EXE_NAME, c::DRIVER_FILE_NAME] {
            assert!(
                conf.contains(&format!("\"resources/{name}\": \"{name}\"")),
                "bundle.resources 缺少 {name} 的映射，运行期取资源必失败"
            );
        }
    }

    /// 审查项 7.1.2 的同类回归（P0-1 就是这个形状）：`capabilities/*.json` 的
    /// `windows` 数组必须覆盖 `tauri.conf.json` 里**每一个**窗口 label。
    ///
    /// 为什么值得钉：capability 的 `windows` 决定"哪些窗口能拿到 invoke 权限"。
    /// 改窗口 label（或新增窗口）而忘了同步 capability，症状是**前端静默拿不到
    /// 任何命令返回** —— 没有编译错误、没有运行期 panic，只有"按钮点了没反应"。
    /// 这正是 P0-1 排查了整轮的那类问题，所以把它变成一条会红的测试。
    #[test]
    fn capability_windows_cover_conf_windows() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let conf: serde_json::Value =
            serde_json::from_str(&read_at(dir, "tauri.conf.json")).expect("tauri.conf.json 不是合法 JSON");
        let labels: Vec<&str> = conf["app"]["windows"]
            .as_array()
            .unwrap_or_else(|| panic!("tauri.conf.json 没有 app.windows 数组"))
            .iter()
            .filter_map(|w| w["label"].as_str())
            .collect();
        assert!(!labels.is_empty(), "app.windows 为空或全部缺 label —— 断言会假通过");

        let caps_dir = dir.join("capabilities");
        let caps = std::fs::read_dir(&caps_dir)
            .unwrap_or_else(|e| panic!("读不到 {:?}：{e}", caps_dir));
        let mut checked = 0usize;
        for entry in caps.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("capability 文件读失败");
            let json: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} 不是合法 JSON：{e}", path.display()));
            let allowed: Vec<&str> = json["windows"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            for label in &labels {
                assert!(
                    allowed.contains(label),
                    "{} 的 windows={:?} 不含窗口 label {label:?} ⇒ 该窗口调不到任何 \
                     command（P0-1 复现路径）。改 label 请同步 capability。",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    allowed
                );
            }
            checked += 1;
        }
        assert!(checked > 0, "capabilities/ 下没有任何 .json，上面的覆盖断言全为空转");
    }

    fn read_at(dir: &std::path::Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("读不到 {name}：{e}"))
    }
}
