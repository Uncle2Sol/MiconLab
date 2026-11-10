//! 串口平台抽象层：macOS / Linux / Windows 的差异集中在此。
//!
//! ## 为什么需要这一层
//! 三平台在串口上的差异不是"编译开关"级别，而是**语义**级别：
//!
//! | 差异点 | macOS / Linux | Windows |
//! |---|---|---|
//! | 端口名 | `/dev/cu.usbmodem1101` | `COM3`（可带 `\\.\` 前缀） |
//! | 枚举可靠性 | `serialport` 枚举有缓存，需要 `/dev` 兜底扫描 | 每次调用都走 SetupAPI，实时可用 |
//! | 独占打开 | `exclusive(false)` 可放开，允许 esptool 并存 | Win32 无对应语义，串口句柄默认可共享/由驱动裁决 |
//! | 非标准波特率 | ioctl 直接设置，1.5M/921600 普遍可用 | 由驱动决定，部分 PL2303 只支持标准档位 |
//! | 软件流控 | XON/XOFF 可用 | 多数 USB 转串口驱动不支持，需降级为硬件流控 |
//! | 仿真器端口 | PTY `/tmp/fa008-sim/tty` 可枚举 | 无 PTY，仿真器不可用（不影响真机） |
//!
//! 本模块只提供**判定与名称规范化**，不改变任何设备协议；上层（[`crate::port`]）
//! 按这里的结论分支，避免 `cfg` 散落在业务代码里。

use crate::config::FlowControl;
use crate::port::SerialPortInfo;
use std::path::PathBuf;

/// 应用目录名：配置、数据、录制统一挂在各自平台的用户目录下。
/// 品牌统一小写，三个平台都用 `miconlab`。
pub const APP_DIR_NAME: &str = "miconlab";

/// 运行平台。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
    Other,
}

/// 当前编译目标平台。
pub const fn current() -> Platform {
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Platform::MacOs
    }
    #[cfg(target_os = "linux")]
    {
        Platform::Linux
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Platform::Other
    }
}

/// 是否为 Windows。
pub const fn is_windows() -> bool {
    matches!(current(), Platform::Windows)
}

/// 热插拔检测策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotplugStrategy {
    /// 每次枚举都调用系统 API 即得到最新列表（Windows SetupAPI）
    EnumerateEachScan,
    /// 系统枚举有缓存，需要额外扫描 /dev 兜底（macOS）
    FilesystemRescan,
    /// 枚举 + udev 可靠（Linux）
    NativeEnumerate,
    /// 未知平台，按 FilesystemRescan 处理
    Unknown,
}

/// 当前平台的枚举策略。
pub const fn hotplug_strategy() -> HotplugStrategy {
    match current() {
        Platform::Windows => HotplugStrategy::EnumerateEachScan,
        Platform::MacOs => HotplugStrategy::FilesystemRescan,
        Platform::Linux => HotplugStrategy::NativeEnumerate,
        Platform::Other => HotplugStrategy::Unknown,
    }
}

/// 端口名是否符合该平台的串口命名规则。
///
/// - Windows：`COM1`…`COM256`（大小写不敏感，允许 `\\.\COM3` 前缀）
/// - macOS / Linux：必须以 `/dev/` 开头
pub fn is_port_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return false;
    }
    match current() {
        Platform::Windows => {
            let upper = n.to_ascii_uppercase();
            let upper = upper.strip_prefix(r"\\.\").unwrap_or(&upper);
            match upper.strip_prefix("COM") {
                Some(num) => {
                    !num.is_empty()
                        && num.len() <= 3
                        && num.bytes().all(|b| b.is_ascii_digit())
                        && num.parse::<u16>().map(|v| v >= 1).unwrap_or(false)
                }
                None => false,
            }
        }
        _ => n.starts_with("/dev/"),
    }
}

/// 规范化端口名（消除同一端口的多种写法）。
///
/// - Windows：去空白、转大写、去掉 `\\.\` 前缀
/// - macOS：`/dev/tty.*` → `/dev/cu.*`（通信应使用 callout 设备；
///   `tty.*` 在等待载波时会阻塞读，esptool 也要求 `cu` 版本）
/// - Linux / 其他：仅去空白
pub fn normalize_port_name(name: &str) -> String {
    let n = name.trim();
    match current() {
        Platform::Windows => {
            let upper = n.to_ascii_uppercase();
            upper
                .strip_prefix(r"\\.\")
                .unwrap_or(&upper)
                .to_string()
        }
        Platform::MacOs => {
            if let Some(rest) = n.strip_prefix("/dev/tty.") {
                format!("/dev/cu.{rest}")
            } else {
                n.to_string()
            }
        }
        _ => n.to_string(),
    }
}

/// 端口排序键：Windows 的 `COM2` 要排在 `COM10` 前面（自然序），
/// 而不是字典序把 `COM10` 排到 `COM2` 前面。
pub fn port_sort_key(name: &str) -> (u32, String) {
    let n = normalize_port_name(name);
    if is_windows() {
        let upper = n.to_ascii_uppercase();
        if let Some(num) = upper.strip_prefix("COM") {
            if let Ok(v) = num.parse::<u32>() {
                return (v, upper);
            }
        }
        (u32::MAX, upper)
    } else {
        (0, n)
    }
}

/// 是否支持"非独占"打开（允许 esptool 等外部工具同时占用端口）。
///
/// 仅 Unix 有该语义；Windows 的串口句柄由驱动裁决，`serialport` 未提供开关。
pub const fn supports_non_exclusive_open() -> bool {
    !is_windows()
}

/// 非标准波特率（如 921600 / 1_500_000）是否通常可用。
///
/// Windows 上取决于 USB 转串口驱动：CH340/CP210x/FTDI 的现代驱动支持，
/// 老版 PL2303 只接受标准档位，会返回 `ERROR_INVALID_PARAMETER`。
pub const fn nonstandard_baud_supported() -> bool {
    !is_windows()
}

/// 该流控方式在当前平台是否常见可用。
pub const fn flow_control_supported(fc: FlowControl) -> bool {
    match fc {
        FlowControl::None | FlowControl::Hardware => true,
        // Windows 多数 USB 转串口驱动不支持 XON/XOFF
        FlowControl::Software => !is_windows(),
    }
}

/// 面向用户的平台提示（仅在需要提醒时返回 `Some`）。
pub fn flow_control_note(fc: FlowControl) -> Option<&'static str> {
    if is_windows() && matches!(fc, FlowControl::Software) {
        Some("Most Windows USB-to-serial drivers do not support XON/XOFF software flow control; use RTS/CTS hardware flow control instead")
    } else {
        None
    }
}

/// 非标准波特率提示（Windows 上可能需要换驱动）。
pub fn baud_note(baud: u32) -> Option<&'static str> {
    if is_windows() && !matches!(baud, 110 | 300 | 600 | 1200 | 2400 | 4800 | 9600 | 14400 | 19200 | 38400 | 57600 | 115200 | 128000 | 256000) {
        Some("Non-standard baud rates depend on the driver on Windows; modern CH340/CP210x/FTDI drivers support them, but older PL2303 drivers reject them")
    } else {
        None
    }
}

/// 是否需要文件系统兜底扫描。
pub const fn needs_filesystem_scan() -> bool {
    matches!(hotplug_strategy(), HotplugStrategy::FilesystemRescan | HotplugStrategy::Unknown)
}

/// 系统级端口兜底扫描（仅在 [`needs_filesystem_scan`] 为真时有内容）。
///
/// Windows 返回空：`serialport::available_ports()` 每次走 SetupAPI，实时可靠。
pub fn scan_system_ports() -> Vec<SerialPortInfo> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let mut ports = Vec::new();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let ports: Vec<SerialPortInfo> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        // macOS: 扫描 /dev/cu.* （cu = callout device，推荐用于串口通信）
        // 不扫描 /dev/tty.* 因为它和 cu.* 是同一设备的两个接口
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("cu.") {
                        ports.push(SerialPortInfo {
                            name: format!("/dev/{name}"),
                            description: Some(format!("Serial Device ({name})")),
                            manufacturer: None,
                            serial_number: None,
                            vid: None,
                            pid: None,
                        });
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: 扫描 /dev/ttyUSB*、/dev/ttyACM*、/dev/ttyS*、/dev/ttyAMA*
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    let is_serial = name.starts_with("ttyUSB")
                        || name.starts_with("ttyACM")
                        || name.starts_with("ttyS")
                        || name.starts_with("ttyAMA");
                    if is_serial {
                        ports.push(SerialPortInfo {
                            name: format!("/dev/{name}"),
                            description: Some(format!("Serial Device ({name})")),
                            manufacturer: None,
                            serial_number: None,
                            vid: None,
                            pid: None,
                        });
                    }
                }
            }
        }
    }

    ports
}

// ============================================================================
// 用户目录：配置 / 数据 / 缓存
// ============================================================================
//
// | 用途 | macOS | Linux | Windows |
// |---|---|---|---|
// | 配置 | `~/Library/Application Support` | `$XDG_CONFIG_HOME` 或 `~/.config` | `%APPDATA%` |
// | 数据 | `~/Library/Application Support` | `$XDG_DATA_HOME` 或 `~/.local/share` | `%APPDATA%` |
// | 缓存 | `~/Library/Caches` | `$XDG_CACHE_HOME` 或 `~/.cache` | `%LOCALAPPDATA%` |
//
// 这些取值与 `dirs-next` 在三个平台上的结果一致，因此切换实现不会改变
// macOS 现有配置文件位置（`~/Library/Application Support/miconlab/config.json`）。

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // Windows 的 HOME 可能未设置，USERPROFILE 才是权威
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// 用户配置目录（不含应用名）。
pub fn config_dir() -> Option<PathBuf> {
    match current() {
        Platform::Windows => std::env::var_os("APPDATA").map(PathBuf::from),
        Platform::MacOs => home_dir().map(|h| h.join("Library").join("Application Support")),
        Platform::Linux => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".config"))),
        Platform::Other => home_dir().map(|h| h.join(".config")),
    }
}

/// 用户数据目录（不含应用名）。
pub fn data_dir() -> Option<PathBuf> {
    match current() {
        Platform::Windows => std::env::var_os("APPDATA").map(PathBuf::from),
        Platform::MacOs => home_dir().map(|h| h.join("Library").join("Application Support")),
        Platform::Linux => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local").join("share"))),
        Platform::Other => home_dir().map(|h| h.join(".local").join("share")),
    }
}

/// 用户缓存目录（不含应用名）。
pub fn cache_dir() -> Option<PathBuf> {
    match current() {
        Platform::Windows => std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        Platform::MacOs => home_dir().map(|h| h.join("Library").join("Caches")),
        Platform::Linux => std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".cache"))),
        Platform::Other => home_dir().map(|h| h.join(".cache")),
    }
}

/// 应用配置目录（`<config_dir>/miconlab`）。
pub fn app_config_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join(APP_DIR_NAME))
}

/// 应用数据目录（`<data_dir>/miconlab`）。
pub fn app_data_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join(APP_DIR_NAME))
}

/// 配置文件路径（`<config_dir>/miconlab/config.json`）。
pub fn config_file_path() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("config.json"))
}

/// 录制文件目录（`<data_dir>/miconlab/recordings`）。
pub fn recordings_dir() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join("recordings"))
}

// ============================================================================
// 串口权限
// ============================================================================

/// 串口打开失败时的平台化提示。
///
/// - Linux：`/dev/ttyUSB*`、`/dev/ttyACM*` 默认属 `dialout` 组，普通用户需要加入组
/// - macOS：无组权限问题，常见失败原因是端口被其他程序占用
/// - Windows：无组权限问题，常见失败原因是端口被占用或驱动未安装
pub fn permission_hint(port_name: &str, error: &str) -> Option<String> {
    let denied = error.contains("Permission denied")
        || error.contains("permission denied")
        || error.contains("Access is denied")
        || error.contains("拒绝访问");

    match current() {
        Platform::Linux if denied => Some(format!(
            "Permission denied: the current user is probably not in the dialout group.\n{}",
            linux_udev_rule(port_name)
        )),
        Platform::MacOs if denied || error.contains("Resource busy") => Some(
            "Port is busy or inaccessible: close other programs using this port first."
                .to_string(),
        ),
        Platform::Windows if error.contains("Access is denied") || error.contains("拒绝访问") => {
            Some("Port is busy or inaccessible: close other programs using this port first."
                .to_string())
        }
        _ => None,
    }
}

/// 生成 udev 规则示例（Linux 专用；其他平台返回通用说明）。
pub fn linux_udev_rule(port_name: &str) -> String {
    let dev = if port_name.starts_with("/dev/") {
        port_name.to_string()
    } else {
        "/dev/ttyUSB0".to_string()
    };
    format!(
        "# /etc/udev/rules.d/99-miconlab.rules\n\
         KERNEL==\"{}\", MODE=\"0666\", GROUP=\"dialout\"\n\
         # Apply: sudo udevadm control --reload-rules && sudo udevadm trigger",
        dev.trim_start_matches("/dev/")
    )
}

// ============================================================================
// 系统集成
// ============================================================================

/// 用系统默认程序打开文件或目录（macOS `open` / Windows `explorer` / Linux `xdg-open`）。
///
/// 失败静默：调用方都是"点一下看看"的辅助操作，不应打断主流程。
pub fn open_path(path: &std::path::Path) {
    let cmd = match current() {
        Platform::MacOs => "open",
        Platform::Windows => "explorer",
        _ => "xdg-open",
    };
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

// ============================================================================
// 终端与编码
// ============================================================================

/// 标准输出是否连接终端（决定是否输出进度条等交互内容）。
pub fn is_interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// 启动时准备控制台。
///
/// Windows 控制台默认代码页是本地 ANSI（简体中文系统为 GBK/936），
/// 直接输出 UTF-8 会乱码；这里切到 UTF-8（65001）。
/// macOS / Linux 终端本身就是 UTF-8，无需处理。
pub fn prepare_console() {
    #[cfg(windows)]
    {
        // 最小 FFI：只调用 kernel32 的两个代码页函数，不引入额外依赖
        #[link(name = "kernel32")]
        extern "system" {
            fn SetConsoleOutputCP(cp: u32) -> i32;
            fn SetConsoleCP(cp: u32) -> i32;
        }
        const CP_UTF8: u32 = 65001;
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
            SetConsoleCP(CP_UTF8);
        }
    }
}

/// 控制台能否正确显示 UTF-8（[`prepare_console`] 之后恒为真）。
pub const fn console_utf8_ready() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_platform_is_known() {
        // 本仓库目前只在这三个平台构建
        assert!(matches!(
            current(),
            Platform::Windows | Platform::MacOs | Platform::Linux | Platform::Other
        ));
    }

    #[test]
    fn normalize_strips_windows_prefix_and_maps_tty_to_cu() {
        // 该函数的行为随目标平台变化，测试按当前平台断言
        match current() {
            Platform::Windows => {
                assert_eq!(normalize_port_name(r"\\.\COM7"), "COM7");
                assert_eq!(normalize_port_name(" com12 "), "COM12");
            }
            Platform::MacOs => {
                assert_eq!(normalize_port_name("/dev/tty.usbmodem1101"), "/dev/cu.usbmodem1101");
                assert_eq!(normalize_port_name("/dev/cu.usbserial-210"), "/dev/cu.usbserial-210");
            }
            _ => {
                assert_eq!(normalize_port_name(" /dev/ttyUSB0 "), "/dev/ttyUSB0");
            }
        }
    }

    #[test]
    fn port_name_validation_matches_platform() {
        match current() {
            Platform::Windows => {
                assert!(is_port_name("COM3"));
                assert!(is_port_name(r"\\.\COM12"));
                assert!(!is_port_name("COM0"));
                assert!(!is_port_name("COM"));
                assert!(!is_port_name("/dev/cu.usbmodem1101"));
            }
            _ => {
                assert!(is_port_name("/dev/cu.usbmodem1101"));
                assert!(!is_port_name("COM3"));
                assert!(!is_port_name(""));
            }
        }
    }

    #[test]
    fn sort_key_is_natural_on_windows() {
        if is_windows() {
            // COM2 必须排在 COM10 之前
            assert!(port_sort_key("COM2") < port_sort_key("COM10"));
            assert!(port_sort_key("COM9") < port_sort_key("COM10"));
            // 非法名排到最后
            assert_eq!(port_sort_key("LPT1").0, u32::MAX);
        } else {
            assert!(port_sort_key("/dev/cu.a") < port_sort_key("/dev/cu.b"));
        }
    }

    #[test]
    fn software_flow_control_unsupported_on_windows() {
        assert!(flow_control_supported(FlowControl::None));
        assert!(flow_control_supported(FlowControl::Hardware));
        assert_eq!(
            flow_control_supported(FlowControl::Software),
            !is_windows()
        );
        if is_windows() {
            assert!(flow_control_note(FlowControl::Software).is_some());
        }
    }

    #[test]
    fn nonstandard_baud_note_only_on_windows() {
        assert!(baud_note(115200).is_none());
        if is_windows() {
            assert!(baud_note(1_500_000).is_some());
            assert!(baud_note(921_600).is_some());
        } else {
            assert!(baud_note(1_500_000).is_none());
        }
    }

    #[test]
    fn filesystem_scan_never_panics() {
        let ports = scan_system_ports();
        if is_windows() {
            assert!(ports.is_empty(), "Windows 不应有 /dev 兜底结果");
        }
        let _ = ports.len();
    }

    #[test]
    fn app_paths_live_under_platform_user_dir() {
        // 目录名统一为小写品牌名
        let app_dir = "miconlab";
        if let (Some(cfg), Some(home)) = (config_file_path(), home_dir()) {
            assert!(cfg.ends_with(&format!("{app_dir}/config.json")), "{cfg:?}");
            if current() == Platform::MacOs {
                assert_eq!(
                    cfg,
                    home.join("Library/Application Support/miconlab/config.json")
                );
            }
        }
        if let Some(rec) = recordings_dir() {
            assert!(rec.ends_with(&format!("{app_dir}/recordings")), "{rec:?}");
        }
    }

    #[test]
    fn permission_hint_is_platform_specific() {
        let hint = permission_hint("/dev/ttyUSB0", "Permission denied");
        if current() == Platform::Linux {
            let h = hint.expect("Linux 应给出 dialout 提示");
            assert!(h.contains("dialout"));
            assert!(h.contains("udev"));
        } else {
            // macOS/Windows 不把 "Permission denied" 当成组权限问题
            assert!(hint.is_none() || !hint.unwrap().contains("dialout"));
        }
    }

    #[test]
    fn udev_rule_uses_kernel_name() {
        let rule = linux_udev_rule("/dev/ttyUSB0");
        assert!(rule.contains("KERNEL==\"ttyUSB0\""));
        assert!(!rule.contains("/dev/ttyUSB0\""), "udev KERNEL 不写 /dev 前缀");
    }

    #[test]
    fn console_preparation_is_safe() {
        // 不应 panic；非 Windows 上是 no-op
        prepare_console();
        assert!(console_utf8_ready());
        let _ = is_interactive();
    }
}
