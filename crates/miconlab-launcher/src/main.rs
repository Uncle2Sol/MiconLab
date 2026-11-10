//! miconlab 快速启动器
//!
//! 精简入口：直接启动 GUI 或列出串口。
//! GUI 内已集成终端、CLI 转发、Modbus/CAN/I2C/Flasher 等全功能面板。
//!
//! # 用法
//!
//! ```text
//! miconlab-launcher              # 启动 GUI（默认）
//! miconlab-launcher ports        # 列出可用串口
//! miconlab-launcher help         # 显示帮助
//! ```

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// GUI 二进制名
const BIN_GUI: &str = "miconlab";
/// CLI 二进制名（仅用于 list-ports）
const BIN_CLI: &str = "miconlab-cli";

/// 当前平台的可执行文件后缀
#[cfg(windows)]
const EXE_SUFFIX: &str = ".exe";
#[cfg(not(windows))]
const EXE_SUFFIX: &str = "";

/// 是否中文环境：跟随 `LC_ALL` / `LC_MESSAGES` / `LANG`。
///
/// 启动器是独立 crate，不依赖 GUI 的 i18n 模块（避免为几条 CLI 文案引入
/// eframe 依赖链），因此这里只做最小化的环境判断。
fn is_zh() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| env::var(k).ok())
        .any(|v| v.to_ascii_lowercase().starts_with("zh"))
}

/// 中英双语选择：`en` 为默认文案，`zh_cn` 在中文环境使用。
fn tr(en: &'static str, zh_cn: &'static str) -> &'static str {
    if is_zh() { zh_cn } else { en }
}

/// 查找二进制路径：先看启动器同目录，再查 PATH。
fn find_bin(name: &str) -> Option<PathBuf> {
    let full_name = format!("{}{}", name, EXE_SUFFIX);

    // 1. 启动器所在目录（target/release 或 target/debug）
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&full_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    // 2. 系统 PATH
    if let Some(path_var) = env::var_os("PATH") {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join(&full_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

/// 同步执行二进制，继承当前 stdio，等待完成。
fn run_bin(name: &str, args: &[String]) -> i32 {
    let path = match find_bin(name) {
        Some(p) => p,
        None => {
            let msg = tr(
                "Error: binary '{}' not found. Run `cargo build --release --workspace` first.",
                "错误：找不到二进制 '{}'。请先运行 `cargo build --release --workspace`。",
            )
            .replace("{}", name);
            eprintln!("{msg}");
            return 1;
        }
    };

    let status = match Command::new(&path).args(args).status() {
        Ok(s) => s,
        Err(e) => {
            let msg = tr(
                "Error: failed to launch '{}': {}",
                "错误：启动 '{}' 失败：{}",
            )
            .replacen("{}", &path.display().to_string(), 1)
            .replacen("{}", &e.to_string(), 1);
            eprintln!("{msg}");
            return 1;
        }
    };

    status.code().unwrap_or(1)
}

/// 打印帮助。
fn print_help() {
    println!("{}", tr("miconlab Quick Launcher", "miconlab 快速启动器"));
    println!();
    println!("{}", tr("Usage:", "用法："));
    println!(
        "{}",
        tr(
            "  miconlab-launcher              Launch GUI (default)",
            "  miconlab-launcher              启动 GUI（默认）",
        )
    );
    println!(
        "{}",
        tr(
            "  miconlab-launcher ports        List available serial ports",
            "  miconlab-launcher ports        列出可用串口",
        )
    );
    println!(
        "{}",
        tr(
            "  miconlab-launcher help         Show this help",
            "  miconlab-launcher help         显示本帮助",
        )
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let code = match args.get(1).map(|s| s.as_str()) {
        // 无参数或 "gui"：直接启动 GUI
        None | Some("gui") => run_bin(BIN_GUI, &[]),
        // 列出串口
        Some("ports") => run_bin(BIN_CLI, &["list".to_string()]),
        // 帮助
        Some("help") | Some("-h") | Some("--help") => {
            print_help();
            0
        }
        // 未知命令
        Some(other) => {
            let msg = tr(
                "Error: unknown command '{}'",
                "错误：未知命令 '{}'",
            )
            .replace("{}", other);
            eprintln!("{msg}");
            eprintln!();
            print_help();
            1
        }
    };

    std::process::exit(code);
}
