// 协议分析子面板（反向研发）
// 功能：协议配置 / 发送命令 / 响应解析 / 帧结构可视化（彩色字节单元格）
//       保存命令列表 / 协议日志
//
// 状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut ProtocolAnalyzerAllState。
// 发送操作需调用 state.send_and_read()，通过标志位在状态借用外执行。

use crate::panels::PanelKind;
use crate::state::AppState;
use crate::state::Direction;
use crate::state::RxEntry;
use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use miconlab_core::bus_probe;
use miconlab_core::checksum::crc16_modbus;
use miconlab_core::hex::{format_hex_spaced, parse_hex};
use miconlab_core::product::{
    ChecksumType, DataFormat, FrameFormat, ParsedData, ProtocolConfig, ProtocolParser,
    ProtocolType,
};

#[cfg(feature = "ollama")]
use miconlab_ai::{ChipIdentifyResult, EvidenceBundle, OllamaClient, OllamaConfig, OllamaStatus};
#[cfg(feature = "ollama")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "ollama")]
use std::sync::{Arc, Mutex};

/// 字节角色（用于帧结构着色）
#[derive(Clone, Copy)]
enum ByteRole {
    Header,
    Footer,
    Length,
    Data,
    Checksum,
    Other,
}

/// 保存的命令
#[derive(Clone)]
struct SavedCommand {
    name: String,
    bytes: Vec<u8>,
}

/// 日志条目
#[derive(Clone)]
struct LogEntry {
    timestamp: String,
    direction: &'static str, // "TX" / "RX"
    data: Vec<u8>,
}

/// 协议自动识别结果
#[derive(Clone)]
struct DetectResult {
    protocol_type: ProtocolType,
    header: Vec<u8>,
    footer: Vec<u8>,
    checksum_type: ChecksumType,
    data_format: DataFormat,
    channel_count: usize,
    /// 置信度 0.0-1.0
    confidence: f32,
    /// 识别说明
    description: String,
}

/// 单次探测结果
#[derive(Clone)]
struct ProbeResult {
    name: String,
    tx: Vec<u8>,
    rx: Vec<u8>,
    detect: Option<DetectResult>,
    note: String,
}

/// 自动侦测状态
#[derive(Clone)]
pub struct AutoDetectState {
    /// 是否正在侦测
    running: bool,
    /// 当前探测命令索引
    probe_index: usize,
    /// 等待响应的截止时间
    wait_until: Option<std::time::Instant>,
    /// 发送命令时 rx_buffer 的起始位置
    rx_start_seq: u64,
    /// 探测结果列表
    results: Vec<ProbeResult>,
    /// 最佳识别结果
    best_result: Option<DetectResult>,
}

impl Default for AutoDetectState {
    fn default() -> Self {
        Self {
            running: false,
            probe_index: 0,
            wait_until: None,
            rx_start_seq: 0,
            results: Vec::new(),
            best_result: None,
        }
    }
}

/// 协议分析器状态
#[derive(Clone)]
pub struct ProtocolAnalyzerState {
    // 模式切换：false = 自动侦测模式，true = 手动模式
    manual_mode: bool,
    // 协议配置
    protocol_type: ProtocolType,
    baud_rate: u32,
    slave_id: u8,
    // 帧格式
    header_hex: String,
    footer_hex: String,
    length_offset: i32, // -1 = 无
    length_bytes: usize,
    data_offset: usize,
    checksum_type: ChecksumType,
    checksum_offset: i32, // -1 = 无
    channel_count: usize,
    data_format: DataFormat,
    // 发送
    command_input: String,
    command_name: String,
    wait_ms: u64,
    // 响应
    last_response: Vec<u8>,
    last_parsed: Option<ParsedData>,
    // 保存的命令
    saved_commands: Vec<SavedCommand>,
    // 日志
    log: Vec<LogEntry>,
    // 协议自动识别
    auto_detect: bool,
    detect_result: Option<DetectResult>,
}

impl Default for ProtocolAnalyzerState {
    fn default() -> Self {
        Self {
            manual_mode: false,
            protocol_type: ProtocolType::SerialFrame,
            baud_rate: 115200,
            slave_id: 1,
            header_hex: "AA 55".to_string(),
            footer_hex: "55 AA".to_string(),
            length_offset: -1,
            length_bytes: 0,
            data_offset: 0,
            checksum_type: ChecksumType::Sum8,
            checksum_offset: -1,
            channel_count: 2,
            data_format: DataFormat::U16LE,
            command_input: "AA 55 00 01 00 02".to_string(),
            command_name: String::new(),
            wait_ms: 100,
            last_response: Vec::new(),
            last_parsed: None,
            saved_commands: Vec::new(),
            log: Vec::new(),
            auto_detect: true,
            detect_result: None,
        }
    }
}

// ===== 芯片方案自动识别 =====

/// 芯片识别结果
#[derive(Clone)]
struct ChipDetectResult {
    /// 芯片系列（"STM32" / "ESP32" / "ESP8266" / "Arduino/AVR" / "AT 设备" 等）
    chip_family: String,
    /// 详细描述
    chip_description: String,
    /// 置信度 0.0-1.0
    confidence: f32,
    /// 识别证据（人可读）
    evidence: String,
}

/// USB 桥接芯片识别结果
#[derive(Clone)]
pub(crate) struct UsbBridgeInfo {
    /// 芯片型号（"CH340" / "CP2102" / "FT232R" / "PL2303" 等）
    pub(crate) chip_name: String,
    /// 厂商
    pub(crate) vendor: String,
    /// VID
    pub(crate) vid: u16,
    /// PID
    pub(crate) pid: u16,
    /// USB 描述（产品名）
    pub(crate) description: Option<String>,
    /// 是否为 MCU 原生 USB（而非桥接芯片）
    pub(crate) is_native_mcu: bool,
}

/// USB 转串口芯片 VID/PID 数据库
/// (VID, PID, 芯片型号, 厂商, 是否MCU原生USB)
const USB_BRIDGE_CHIPS: &[(u16, u16, &str, &str, bool)] = &[
    // WCH (南京沁恒)
    (0x1A86, 0x7523, "CH340", "WCH (QinHeng)", false),
    (0x1A86, 0x5523, "CH341", "WCH (QinHeng)", false),
    (0x1A86, 0x7522, "CH340K", "WCH (QinHeng)", false),
    (0x1A86, 0x752D, "CH340N", "WCH (QinHeng)", false),
    (0x1A86, 0x752E, "CH340B", "WCH (QinHeng)", false),
    (0x1A86, 0x1A02, "CH9102", "WCH (QinHeng)", false),
    // Silicon Labs
    (0x10C4, 0xEA60, "CP2102", "Silicon Labs", false),
    (0x10C4, 0xEA63, "CP2104", "Silicon Labs", false),
    (0x10C4, 0xEA70, "CP2105", "Silicon Labs", false),
    (0x10C4, 0xEA71, "CP2108", "Silicon Labs", false),
    // FTDI
    (0x0403, 0x6001, "FT232R", "FTDI", false),
    (0x0403, 0x6010, "FT2232", "FTDI", false),
    (0x0403, 0x6014, "FT232H", "FTDI", false),
    (0x0403, 0x6015, "FT231X", "FTDI", false),
    (0x0403, 0x6011, "FT4232", "FTDI", false),
    // Prolific
    (0x067B, 0x2303, "PL2303", "Prolific", false),
    (0x067B, 0x23A3, "PL2303TA", "Prolific", false),
    (0x067B, 0x23D3, "PL2303RA", "Prolific", false),
    // QinHeng (MH CH340 系列变种)
    (0x1A86, 0x7523, "CH340", "WCH (QinHeng)", false),
    // MCU 原生 USB CDC（非桥接芯片）
    (0x0483, 0x5740, "STM32 CDC", "STMicroelectronics", true),
    (0x0483, 0x374B, "STM32 HID-CDC", "STMicroelectronics", true),
    // ESP32 USB CDC：PID 因芯片型号和模式不同
    //   0x0001 = 旧版/USB-Serial-JTAG, 0x0002 = USB-NCM
    //   0x1001 = ESP32-S2/S3 USB CDC (Download Mode / 运行模式)
    //   0x1002 = ESP32-S3 USB NCM, 0x0003 = ESP32-S3 USB JTAG/Serial
    (0x303A, 0x0001, "ESP32 USB-CDC", "Espressif", true),
    (0x303A, 0x0002, "ESP32 USB-NCM", "Espressif", true),
    (0x303A, 0x0003, "ESP32-S3 USB-JTAG", "Espressif", true),
    (0x303A, 0x1001, "ESP32-S2/S3 USB-CDC", "Espressif", true),
    (0x303A, 0x1002, "ESP32-S3 USB-NCM", "Espressif", true),
    // Arduino
    (0x2341, 0x0043, "Arduino Uno", "Arduino", true),
    (0x2341, 0x0010, "Arduino Mega", "Arduino", true),
    (0x2341, 0x003F, "Arduino Mega2560", "Arduino", true),
    (0x2341, 0x0001, "Arduino Serial", "Arduino", true),
    (0x2A03, 0x0043, "Arduino Uno (clone)", "Arduino", true),
    // Microchip
    (0x04D8, 0x000A, "PIC18 CDC", "Microchip", true),
];

/// 从描述字符串中解析 VID:PID（如 "USB Device 1A86:7523"）
fn parse_vid_pid_from_desc(s: &str) -> Option<(u16, u16)> {
    // 查找 "xxxx:xxxx" 模式（4位十六进制:4位十六进制）
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 8 < bytes.len() {
        // 查找冒号位置
        if bytes[i + 4] == b':' {
            let hex_chars = &bytes[i..i + 9];
            // 验证 4位HEX : 4位HEX 格式
            if hex_chars[..4].iter().all(|c| c.is_ascii_hexdigit())
                && hex_chars[5..9].iter().all(|c| c.is_ascii_hexdigit())
            {
                let vid_str = std::str::from_utf8(&hex_chars[..4]).ok()?;
                let pid_str = std::str::from_utf8(&hex_chars[5..9]).ok()?;
                let vid = u16::from_str_radix(vid_str, 16).ok()?;
                let pid = u16::from_str_radix(pid_str, 16).ok()?;
                return Some((vid, pid));
            }
        }
        i += 1;
    }
    None
}

/// 通过端口名推断 USB 转串口芯片信息
///
/// macOS/Linux 上常见端口名模式：
/// - wchusbserial* → CH340/CH9102 (WCH)
/// - SLAB_USBtoUART* → CP210x (Silicon Labs)
/// - usbserial-* → FT232 (FTDI)
/// - ProlificSerial* → PL2303 (Prolific)
/// - STM32* → STM32 CDC
/// - ESP32*/ESP* → ESP32 USB CDC
pub(crate) fn identify_by_port_name(port_name: &str) -> Option<UsbBridgeInfo> {
    let name_lower = port_name.to_lowercase();

    // WCH CH340/CH9102 系列
    if name_lower.contains("wchusbserial") || name_lower.contains("wchlink") {
        return Some(UsbBridgeInfo {
            chip_name: "CH340".to_string(),
            vendor: "WCH (QinHeng)".to_string(),
            vid: 0x1A86,
            pid: 0x7523,
            description: Some("Identified by port name (wchusbserial)".to_string()),
            is_native_mcu: false,
        });
    }

    // MXSPP 系列（CH340N + 自定义产品名，常见于传感器设备）
    if name_lower.contains("mxspp") {
        return Some(UsbBridgeInfo {
            chip_name: "CH340N".to_string(),
            vendor: "WCH (QinHeng)".to_string(),
            vid: 0x1A86,
            pid: 0x7523,
            description: Some("Identified by port name (MXSPP, custom CH340N product name)".to_string()),
            is_native_mcu: false,
        });
    }

    // CP210x
    if name_lower.contains("slab_usbtouart") || name_lower.contains("cp210") {
        return Some(UsbBridgeInfo {
            chip_name: "CP210x".to_string(),
            vendor: "Silicon Labs".to_string(),
            vid: 0x10C4,
            pid: 0xEA60,
            description: Some("Identified by port name (SLAB_USBtoUART)".to_string()),
            is_native_mcu: false,
        });
    }

    // FT232
    if name_lower.contains("ftdi") || (name_lower.contains("usbserial") && !name_lower.contains("wch")) {
        return Some(UsbBridgeInfo {
            chip_name: "FT232".to_string(),
            vendor: "FTDI".to_string(),
            vid: 0x0403,
            pid: 0x6001,
            description: Some("Identified by port name (usbserial)".to_string()),
            is_native_mcu: false,
        });
    }

    // STM32 CDC
    if name_lower.contains("stm32") {
        return Some(UsbBridgeInfo {
            chip_name: "STM32 CDC".to_string(),
            vendor: "STMicroelectronics".to_string(),
            vid: 0x0483,
            pid: 0x374B,
            description: Some("Identified by port name (STM32)".to_string()),
            is_native_mcu: true,
        });
    }

    // ESP32
    if name_lower.contains("esp32") || name_lower.contains("espressif") {
        return Some(UsbBridgeInfo {
            chip_name: "ESP32 USB".to_string(),
            vendor: "Espressif".to_string(),
            vid: 0x303A,
            pid: 0x0001,
            description: Some("Identified by port name (ESP32)".to_string()),
            is_native_mcu: true,
        });
    }

    None
}

/// 通过 VID/PID 识别 USB 转串口芯片
pub(crate) fn identify_usb_bridge(
    vid: Option<u16>,
    pid: Option<u16>,
    description: Option<&str>,
    manufacturer: Option<&str>,
) -> Option<UsbBridgeInfo> {
    // 优先通过 VID/PID 匹配
    if let (Some(v), Some(p)) = (vid, pid) {
        for &(db_vid, db_pid, name, vendor, is_native) in USB_BRIDGE_CHIPS {
            if db_vid == v && db_pid == p {
                return Some(UsbBridgeInfo {
                    chip_name: name.to_string(),
                    vendor: vendor.to_string(),
                    vid: v,
                    pid: p,
                    description: description.map(|s| s.to_string()),
                    is_native_mcu: is_native,
                });
            }
        }
        // VID 前缀回退：VID 已知但 PID 不在数据库中
        // Espressif (0x303A) 的 PID 变种很多，所有 0x303A:xxxx 都视为 ESP32
        if v == 0x303A {
            return Some(UsbBridgeInfo {
                chip_name: "ESP32 USB (unknown PID)".to_string(),
                vendor: "Espressif".to_string(),
                vid: v,
                pid: p,
                description: description.map(|s| s.to_string()),
                is_native_mcu: true,
            });
        }
        // STMicroelectronics (0x0483) 的 CDC 变种也很多
        if v == 0x0483 {
            return Some(UsbBridgeInfo {
                chip_name: "STM32 CDC (unknown PID)".to_string(),
                vendor: "STMicroelectronics".to_string(),
                vid: v,
                pid: p,
                description: description.map(|s| s.to_string()),
                is_native_mcu: true,
            });
        }
    }

    // VID/PID 未匹配，尝试通过描述/厂商名识别
    let desc_lower = description.unwrap_or("").to_lowercase();
    let mfr_lower = manufacturer.unwrap_or("").to_lowercase();
    let combined = format!("{} {}", desc_lower, mfr_lower);

    // 检查描述中是否包含 VID:PID 字符串（如 "USB Device 1A86:7523"）
    if vid.is_none() || pid.is_none() {
        if let Some(parsed) = parse_vid_pid_from_desc(&combined) {
            let (pv, pp) = parsed;
            for &(db_vid, db_pid, name, vendor, is_native) in USB_BRIDGE_CHIPS {
                if db_vid == pv && db_pid == pp {
                    return Some(UsbBridgeInfo {
                        chip_name: name.to_string(),
                        vendor: vendor.to_string(),
                        vid: pv,
                        pid: pp,
                        description: description.map(|s| s.to_string()),
                        is_native_mcu: is_native,
                    });
                }
            }
        }
    }

    if combined.contains("ch340") || combined.contains("wch") {
        return Some(UsbBridgeInfo {
            chip_name: "CH340".to_string(),
            vendor: "WCH".to_string(),
            vid: vid.unwrap_or(0),
            pid: pid.unwrap_or(0),
            description: description.map(|s| s.to_string()),
            is_native_mcu: false,
        });
    }
    if combined.contains("cp210") {
        return Some(UsbBridgeInfo {
            chip_name: "CP210x".to_string(),
            vendor: "Silicon Labs".to_string(),
            vid: vid.unwrap_or(0),
            pid: pid.unwrap_or(0),
            description: description.map(|s| s.to_string()),
            is_native_mcu: false,
        });
    }
    if combined.contains("ft232") || combined.contains("ftdi") {
        return Some(UsbBridgeInfo {
            chip_name: "FT232".to_string(),
            vendor: "FTDI".to_string(),
            vid: vid.unwrap_or(0),
            pid: pid.unwrap_or(0),
            description: description.map(|s| s.to_string()),
            is_native_mcu: false,
        });
    }
    if combined.contains("pl2303") || combined.contains("prolific") {
        return Some(UsbBridgeInfo {
            chip_name: "PL2303".to_string(),
            vendor: "Prolific".to_string(),
            vid: vid.unwrap_or(0),
            pid: pid.unwrap_or(0),
            description: description.map(|s| s.to_string()),
            is_native_mcu: false,
        });
    }
    if combined.contains("stm32") {
        return Some(UsbBridgeInfo {
            chip_name: "STM32 CDC".to_string(),
            vendor: "STMicroelectronics".to_string(),
            vid: vid.unwrap_or(0),
            pid: pid.unwrap_or(0),
            description: description.map(|s| s.to_string()),
            is_native_mcu: true,
        });
    }
    if combined.contains("esp32") || combined.contains("espressif") {
        return Some(UsbBridgeInfo {
            chip_name: "ESP32 USB".to_string(),
            vendor: "Espressif".to_string(),
            vid: vid.unwrap_or(0),
            pid: pid.unwrap_or(0),
            description: description.map(|s| s.to_string()),
            is_native_mcu: true,
        });
    }

    None
}

/// 检查当前桥接器是否支持 UART 烧录
/// 返回 (是否支持, 提示消息)
fn check_bridge_support(state: &crate::state::AppState, _zh: bool) -> (bool, String) {
    if !state.is_connected() {
        return (false, crate::i18n::t("⚠ Not connected. Please connect USB-Serial adapter (CH340/CP2102/FT232R) first").to_string());
    }

    // 获取当前选中端口的 VID/PID
    if let Some(info) = state.port_list.get(state.selected_port_index) {
        if let (Some(vid), Some(pid)) = (info.vid, info.pid) {
            if let Some(bridge) = identify_usb_bridge(Some(vid), Some(pid), info.description.as_deref(), info.manufacturer.as_deref()) {
                if bridge.is_native_mcu {
                    return (true, { crate::i18n::tf("✓ Connected {} ({:04X}:{:04X}) - MCU native USB CDC\n  Note: MCU native USB. Supports USB Bootloader if available (STM32 DFU/ESP32 USB-Download)", &[("{}", "{}", &format!("{}", bridge.chip_name)), ("{:04X}", "{:04X}", &format!("{:04X}", vid)), ("{:04X}", "{:04X}", &format!("{:04X}", pid))]) });
                } else {
                    return (true, { crate::i18n::tf("✓ Connected {} ({:04X}:{:04X}) - USB-Serial bridge\n  Supports UART Bootloader flashing (STM32/GD32/CH32/ESP32/Arduino etc.)", &[("{}", "{}", &format!("{}", bridge.chip_name)), ("{:04X}", "{:04X}", &format!("{:04X}", vid)), ("{:04X}", "{:04X}", &format!("{:04X}", pid))]) });
                }
            }
        }
    }

    (true, crate::i18n::t("✓ Connected (bridge type unknown, may try UART Bootloader)").to_string())
}

/// 单次芯片探测结果
#[derive(Clone)]
struct ChipProbeResult {
    name: String,
    tx: Vec<u8>,
    rx: Vec<u8>,
    detect: Option<ChipDetectResult>,
    note: String,
}

/// 芯片识别状态
#[derive(Clone)]
pub struct ChipDetectState {
    /// 是否正在侦测
    running: bool,
    /// 当前探测命令索引
    probe_index: usize,
    /// 等待响应的截止时间
    wait_until: Option<std::time::Instant>,
    /// 发送命令时 rx_buffer 的起始位置
    rx_start_seq: u64,
    /// 探测结果列表
    results: Vec<ChipProbeResult>,
    /// 最佳识别结果（协议探测）
    best_result: Option<ChipDetectResult>,
    /// USB 桥接芯片识别结果（通过 VID/PID，无需协议探测）
    usb_bridge: Option<UsbBridgeInfo>,
}

impl Default for ChipDetectState {
    fn default() -> Self {
        Self {
            running: false,
            probe_index: 0,
            wait_until: None,
            rx_start_seq: 0,
            results: Vec::new(),
            best_result: None,
            usb_bridge: None,
        }
    }
}

// ===== Ollama 本地模型芯片识别（feature `ollama`，Phase 3b）=====

/// 芯片识别任务的触发场景（决定结果回写目标）
#[cfg(feature = "ollama")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OllamaChipTask {
    /// VID:PID 未知 → 识别 USB 桥接芯片，结果合成进 usb_bridge
    VidPid,
    /// MCU 协议探测全部失败 → 识别目标 MCU，结果填充 best_result
    McuProbe,
}

/// Ollama 芯片识别状态（与 AI 助手/固件编辑器相同的后台任务模式）
///
/// 识别失败静默降级（仅日志）；结果回写有置信度门槛，且仅回填规则引擎失败的空位，
/// 绝不覆盖规则引擎已有结论。
#[cfg(feature = "ollama")]
#[derive(Clone)]
pub struct OllamaChipState {
    /// Ollama 配置（默认值，Phase 4 设置页可编辑）
    pub config: OllamaConfig,
    /// 服务状态（None = 尚未检测）
    pub status: Option<OllamaStatus>,
    /// 状态最近检测时刻（30s 轮询节流）
    pub status_checked_at: Option<std::time::Instant>,
    /// 状态检测后台任务结果槽
    status_slot: Arc<Mutex<Option<OllamaStatus>>>,
    /// 状态检测进行中
    pub status_busy: bool,
    /// 识别任务结果槽
    result_slot: Arc<Mutex<Option<Result<ChipIdentifyResult, String>>>>,
    /// 识别任务完成标志
    done_flag: Arc<AtomicBool>,
    /// 识别进行中
    pub busy: bool,
    /// 最近一次识别结果
    pub result: Option<ChipIdentifyResult>,
    /// 当前任务场景
    pub task: Option<OllamaChipTask>,
    /// 已触发过的自动识别 key（防同场景重复触发）
    pub last_auto_key: Option<String>,
    /// 结果是否已回写（usb_bridge 合成或 best_result 填充）
    pub applied: bool,
    /// Ollama 配置签名（设置页联动检测，Phase 4）
    pub cfg_sig: Option<String>,
}

#[cfg(feature = "ollama")]
impl Default for OllamaChipState {
    fn default() -> Self {
        Self {
            config: OllamaConfig::default(),
            status: None,
            status_checked_at: None,
            status_slot: Arc::new(Mutex::new(None)),
            status_busy: false,
            result_slot: Arc::new(Mutex::new(None)),
            done_flag: Arc::new(AtomicBool::new(false)),
            busy: false,
            result: None,
            task: None,
            last_auto_key: None,
            applied: false,
            cfg_sig: None,
        }
    }
}

// ===== 通用 Cortex-M3 自动探测 =====

/// 通用 Cortex-M3 自动探测状态（用于 JTAG/SWD 识别后自动判断芯片厂商）
#[derive(Clone)]
pub struct CortexM3ProbeState {
    /// 是否正在运行
    running: bool,
    /// 当前探测命令索引
    probe_index: usize,
    /// 等待超时时间
    wait_until: Option<std::time::Instant>,
    /// 接收缓冲区起始位置
    rx_start_seq: u64,
    /// 探测结果列表：(命令名, 发送数据, 接收数据, 识别结果hint)
    results: Vec<(&'static str, Vec<u8>, Vec<u8>, Option<&'static str>)>,
    /// 最佳探测结果对应的 hint（如 "stm32"/"lpc"/"ch32"）
    best_hint: Option<&'static str>,
    /// 跳转动作（"jump"/"read"/"write"）
    action: String,
    /// 探测日志
    log: Vec<String>,
}

impl Default for CortexM3ProbeState {
    fn default() -> Self {
        Self {
            running: false,
            probe_index: 0,
            wait_until: None,
            rx_start_seq: 0,
            results: Vec::new(),
            best_hint: None,
            action: "jump".to_string(),
            log: Vec::new(),
        }
    }
}

/// Cortex-M3 自动探测命令序列
/// (命令名, 命令数据, 成功响应特征, 对应 hint)
const CORTEX_M3_PROBES: &[(&str, &[u8], &[u8], &str)] = &[
    // 1. STM32/GD32/CH32 UART Bootloader 唤醒：发送 0x7F，期望 ACK 0x79
    ("STM32/GD32/CH32 Bootloader (0x7F)", &[0x7F], &[0x79], "stm32"),
    // 2. NXP LPC ISP 同步：发送 12 个 '?'，期望 "Synchronized" 响应
    ("NXP LPC ISP sync (????????????)", b"????????????", b"Synchronized", "lpc"),
    // 3. WCH ISP 识别：发送完整 IDENTIFY 包 0x57 0xAB 0xA1 0x00 0x00 0xA1
    //    （原仅发送帧头 0x57 0xAB 不完整，CH32 不会响应；IDENTIFY 包格式：
    //    帧头 + cmd + len_hi + len_lo + sum，sum=cmd+len=0xA1）
    ("WCH ISP IDENTIFY (0x57 0xAB 0xA1)", &[0x57, 0xAB, 0xA1, 0x00, 0x00, 0xA1], &[0x57, 0xAB, 0xA1], "ch32"),
];

// ===== 深度总线探测 =====

/// 深度探测扫描类型
#[derive(Clone, Copy, PartialEq)]
enum DeepScanType {
    /// Modbus 从站扫描
    ModbusSlave,
    /// Modbus 寄存器扫描
    ModbusRegister,
    /// UART 协议分析
    UartAnalyze,
    /// I2C 设备参考表
    I2cReference,
    /// SPI Flash 参考表
    SpiReference,
    /// JTAG/SWD/1-Wire 参考
    JtagReference,
}

/// 深度探测状态
#[derive(Clone)]
pub struct DeepProbeState {
    /// 是否正在扫描
    running: bool,
    /// 当前扫描类型
    scan_type: DeepScanType,
    /// Modbus 从站扫描：起始/结束地址
    modbus_start: u8,
    modbus_end: u8,
    /// Modbus 寄存器扫描：从站 ID / 起始 / 结束寄存器
    modbus_slave_id: u8,
    modbus_reg_start: u16,
    modbus_reg_end: u16,
    /// 当前扫描进度（当前 slave_id 或寄存器地址）
    progress: usize,
    /// 扫描总数（用于进度条）
    total: usize,
    /// 等待响应的截止时间
    wait_until: Option<std::time::Instant>,
    /// 发送命令时 rx_buffer 的起始位置
    rx_start_seq: u64,
    /// 扫描结果
    results: Vec<bus_probe::DeepProbeResult>,
    /// 扫描日志
    log: Vec<String>,
    /// SPI Flash 手动输入的 JEDEC ID（格式: "MFG CAP"，如 "EF 4017"）
    spi_jedec_input: String,
    /// JTAG IDCODE 手动输入（hex，如 "0BA9542F"）
    jtag_idcode_input: String,
    /// 1-Wire 家族码手动输入（hex，如 "28"）
    onewire_family_input: String,
}

impl Default for DeepProbeState {
    fn default() -> Self {
        Self {
            running: false,
            scan_type: DeepScanType::ModbusSlave,
            modbus_start: 1,
            modbus_end: 247,
            modbus_slave_id: 1,
            modbus_reg_start: 0,
            modbus_reg_end: 99,
            progress: 0,
            total: 0,
            wait_until: None,
            rx_start_seq: 0,
            results: Vec::new(),
            log: Vec::new(),
            spi_jedec_input: "EF 4017".to_string(),
            jtag_idcode_input: "0BA9542F".to_string(),
            onewire_family_input: "28".to_string(),
        }
    }
}

// ===== MCU Boot ROM 探测状态机（鞋垫/床垫协议检测后自动触发） =====

/// MCU Boot 探测状态
#[derive(Default, Clone)]
pub struct McuProbeState {
    /// 是否正在运行探测
    running: bool,
    /// 当前探测项索引（对应 McuBootProbeKind::PROBE_SEQUENCE）
    probe_index: usize,
    /// 已发送命令（等待应答中）
    sent: bool,
    /// 等待应答的截止时刻
    wait_until: Option<std::time::Instant>,
    /// 发送同步字节前的 rx_buffer 长度（用于提取之后的 RX 数据）
    rx_start_seq: u64,
    /// 探测结果列表
    results: Vec<bus_probe::McuBootProbeResult>,
    /// 日志
    log: Vec<String>,
    /// 是否由鞋垫协议检测自动触发
    triggered_by_insole: bool,
    /// 是否已自动触发过（避免重复自动触发）
    auto_triggered: bool,

    // ===== DTR/RTS 预触发状态机（非阻塞） =====
    // 设备运行用户固件时需通过 DTR/RTS 信号触发 Boot 模式：
    //   STM32/GD32/CH32: RTS=BOOT0(高), DTR=RESET(低→高脉冲)
    //   ESP32/ESP8266:   DTR=GPIO0(低), RTS=RESET(低→高脉冲)
    //   Arduino:         DTR 脉冲自动复位
    // 阶段流转: 0=未启动 → 1=设置 BOOT0+RESET(低) → 2=释放 RESET(高) → 3=完成进入发送
    pre_trigger_phase: u8,
    /// 预触发当前阶段截止时刻
    pre_trigger_until: Option<std::time::Instant>,
    /// 是否已完成 DTR/RTS 触发（全局只触发一次，避免重复复位）
    dtr_rts_tried: bool,

    // ===== 被动协议特征分析（Boot 探测全部失败后的回退方案） =====
    // 通过分析鞋垫数据流的帧间隔抖动和波特率精度推断 MCU 类型：
    //   - STM32 DMA+定时器: 帧间隔抖动 < 0.5ms（高精度定时器）
    //   - ESP32 FreeRTOS:   帧间隔抖动 > 2ms（任务调度抖动）
    //   - STM8 轮询:        帧间隔抖动中等（1-3ms）
    //   - 外部晶振:          波特率误差 < 0.5%
    //   - 内部 RC:           波特率误差 > 1%
    /// 收集到的帧间隔（毫秒，基于 RxEntry 时间戳）
    frame_intervals_ms: Vec<u64>,
    /// 被动分析结果文本（None=未分析，Some=分析完成）
    passive_result: Option<PassiveAnalysisResult>,
    /// USB 桥接识别结果（从 chip_detect 同步，用于被动分析加分）
    pub(crate) usb_bridge: Option<UsbBridgeInfo>,
}

/// 被动协议特征分析结果
#[derive(Debug, Clone)]
pub(crate) struct PassiveAnalysisResult {
    /// 推断的 MCU 厂商
    pub(crate) vendor: String,
    /// 推断的芯片系列
    pub(crate) chip_family: String,
    /// 置信度 0.0-1.0（被动分析置信度上限 0.75）
    pub(crate) confidence: f32,
    /// 分析依据
    pub(crate) evidence: String,
    /// 帧间隔统计（毫秒）
    pub(crate) avg_interval_ms: f64,
    /// 帧间隔抖动（标准差，毫秒）
    pub(crate) jitter_ms: f64,
    /// 采集的帧数
    pub(crate) sample_count: usize,

    // ===== 多维度分析结果 =====
    /// 帧头匹配率（0.0-1.0，越高说明波特率越精确/外部晶振）
    pub(crate) header_match_rate: f64,
    /// 推断的时钟源类型
    pub(crate) clock_source: String,
    /// 是否需要硬件 FPU（IMU 四元数 float32 运算）
    pub(crate) needs_fpu: bool,
    /// 最小 RAM 需求（字节）
    pub(crate) min_ram_bytes: usize,
    /// 推断的 ADC 能力描述
    pub(crate) adc_capability: String,
    /// 数据吞吐量（字节/秒）
    pub(crate) throughput_bps: u64,
    /// 当前波特率
    pub(crate) baud_rate: u32,
    /// 候选 MCU 列表（按综合评分排序）
    pub(crate) candidates: Vec<McuCandidate>,

    // ===== 深度协议分析结果 =====
    /// 推断的 ADC 分辨率（位），0 表示未分析
    pub(crate) adc_resolution: u8,
    /// ADC 最大采样值
    pub(crate) adc_max_value: u16,
    /// ADC 最小采样值
    pub(crate) adc_min_value: u16,
    /// IMU 四元数是否为有效 IEEE754 float32
    pub(crate) imu_float_valid: bool,
    /// IMU 四元数模长（接近 1.0 为有效）
    pub(crate) imu_quaternion_norm: f32,
    /// 推断的字节序 ("little" / "big" / "unknown")
    pub(crate) byte_order: String,
    /// 帧结构一致性（SeqTag 配对率，0.0-1.0）
    pub(crate) frame_consistency: f64,
    /// DMA 传输特征评分（0.0-1.0，越高越像 DMA）
    pub(crate) dma_likelihood: f64,
    /// 解析的完整帧数
    pub(crate) parsed_frames: usize,
}

/// 单个候选 MCU 的匹配评分
#[derive(Debug, Clone)]
pub(crate) struct McuCandidate {
    /// 厂商
    pub(crate) vendor: String,
    /// 芯片系列
    pub(crate) chip_family: String,
    /// 典型型号示例
    pub(crate) example_model: String,
    /// 综合评分 0.0-1.0
    pub(crate) score: f32,
    /// 匹配的理由
    pub(crate) match_reasons: Vec<String>,
    /// 不匹配的理由
    pub(crate) mismatch_reasons: Vec<String>,
}

impl Default for PassiveAnalysisResult {
    fn default() -> Self {
        Self {
            vendor: String::new(),
            chip_family: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            avg_interval_ms: 0.0,
            jitter_ms: 0.0,
            sample_count: 0,
            header_match_rate: 0.0,
            clock_source: String::new(),
            needs_fpu: false,
            min_ram_bytes: 0,
            adc_capability: String::new(),
            throughput_bps: 0,
            baud_rate: 0,
            candidates: Vec::new(),
            adc_resolution: 0,
            adc_max_value: 0,
            adc_min_value: 0,
            imu_float_valid: false,
            imu_quaternion_norm: 0.0,
            byte_order: String::new(),
            frame_consistency: 0.0,
            dma_likelihood: 0.0,
            parsed_frames: 0,
        }
    }
}

/// 协议分析器全部状态（组合存储）
///
/// 通过 state_manager 在 CentralPanel 外部加载/保存，避免在闭包内调用 ctx.data_mut() 死锁。
/// 将 7 个原 thread_local 状态合并为单个结构体，减少 load/save 调用次数。
#[derive(Clone, Default)]
pub struct ProtocolAnalyzerAllState {
    /// 主状态（原 STATE）
    pub main_state: ProtocolAnalyzerState,
    /// 上次连接会话 ID（原 LAST_SESSION_ID），用于检测连接变化
    pub last_session_id: u64,
    /// 自动侦测状态（原 DETECT）
    pub detect: AutoDetectState,
    /// 芯片检测状态（原 CHIP_DETECT）
    pub chip_detect: ChipDetectState,
    /// Cortex-M3 探测状态（原 CORTEX_M3_PROBE）
    pub cortex_m3_probe: CortexM3ProbeState,
    /// 深度探测状态（原 DEEP_PROBE）
    pub deep_probe: DeepProbeState,
    /// MCU 探测状态（原 MCU_PROBE）
    pub mcu_probe: McuProbeState,
    /// Ollama 本地模型芯片识别状态（Phase 3b）
    #[cfg(feature = "ollama")]
    pub ollama_chip: OllamaChipState,
}

/// 芯片方案识别探测命令序列
///
/// 按特定芯片的 bootloader / 固件协议特征发送探测命令：
/// - STM32: USART Bootloader 通过 0x7F 唤醒，回应 0x79 (ACK) 或 0x1F (NACK)
/// - ESP32: ROM Bootloader 通过 SYNC (0x07×2 0x12 0x20 + 8×0x55) 激活
/// - ESP32/ESP8266 AT 固件: AT+GMR 返回固件版本（含 ESP 标识）
/// - Arduino/AVR: STK500 协议 GET_SYNC (0x30 0x20)，回应 0x14 0x10
/// - 通用 AT: AT 命令返回 OK（低置信度，可能是任意 AT 设备）
/// - SCPI: *IDN? 返回设备标识（可能包含芯片型号）
const CHIP_PROBES: &[(&str, &[u8])] = &[
    // 1. STM32 USART Bootloader 唤醒
    ("STM32 Bootloader wakeup (0x7F)", &[0x7F]),
    // 2. ESP32 ROM Bootloader SYNC
    ("ESP32 ROM SYNC", &[0x07, 0x07, 0x12, 0x20, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55]),
    // 3. ESP32/ESP8266 AT 固件版本查询
    ("AT+GMR (ESP firmware version)", b"AT+GMR\r\n"),
    // 4. Arduino STK500 GET_SYNC
    ("Arduino STK GET_SYNC (0x30 0x20)", &[0x30, 0x20]),
    // 5. 通用 AT 命令
    ("AT generic command", b"AT\r\n"),
    // 6. SCPI 标识查询（可能返回芯片信息）
    ("*IDN? SCPI identity query", b"*IDN?\r\n"),
];

/// 自动侦测使用的探测命令序列
const PROBES: &[(&str, &[u8])] = &[
    ("Modbus RTU read holding registers (slave 1)", &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A]),
    ("Modbus RTU read input registers (slave 1)", &[0x01, 0x04, 0x00, 0x00, 0x00, 0x01, 0x31, 0xCA]),
    ("Modbus RTU read holding registers (slave 2)", &[0x02, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x39]),
    ("Modbus RTU read holding registers (slave 1, address 0x10)", &[0x01, 0x03, 0x00, 0x10, 0x00, 0x01, 0xC5, 0xCE]),
    ("AT command", b"AT\r\n"),
    ("SCPI identity query", b"*IDN?\r\n"),
    ("SLCAN version", b"V\r"),
    ("Generic newline", b"\r\n"),
];

/// 渲染协议分析子面板
pub fn show(
    ctx: &egui::Context,
    state: &mut AppState,
    ui: &mut egui::Ui,
    ws: &mut super::wizard::WizardState,
    all: &mut ProtocolAnalyzerAllState,
) {
    let zh = state.language == crate::i18n::Language::Zh;

    // 标题
    ui.heading(
        RichText::new(crate::i18n::t("Protocol Analyzer"))
            .color(theme::TEXT_PRIMARY)
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("Auto-detect serial frame format, baud rate / data bits / parity, and identify chip type"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // ===== 快速开始引导（首次展开，之后折叠，由 egui 持久化） =====
    egui::CollapsingHeader::new(crate::i18n::t("👉 Quick Start (click to toggle)"))
    .default_open(true)
    .show(ui, |ui| {
        render_quick_start_guide(ui, zh);
    });

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    let connected = state.is_connected();

    // 解构组合状态为各字段可变引用，便于在子函数中按需传递
    let ProtocolAnalyzerAllState {
        main_state: s,
        last_session_id,
        detect,
        chip_detect,
        cortex_m3_probe,
        deep_probe,
        mcu_probe,
        #[cfg(feature = "ollama")]
        ollama_chip,
    } = all;

    // ===== 连接会话变化检测：新连接时清除所有识别结果，避免旧设备信息残留 =====
    // 使用 connection_session_id（每次 connect() 成功时递增）而非端口名：
    //   - 同一端口断开后重新连接 → ID 变化 → 清除（解决"换设备但端口名相同"问题）
    //   - 不同端口切换 → ID 变化 → 清除
    //   - 连接后马上断开 → 下次连接 ID 变化 → 清除
    let current_session_id = state.connection_session_id;
    if *last_session_id != current_session_id {
        // 连接会话已变化，清除所有识别状态
        detect.running = false;
        detect.probe_index = 0;
        detect.wait_until = None;
        detect.results.clear();
        detect.best_result = None;

        chip_detect.running = false;
        chip_detect.probe_index = 0;
        chip_detect.wait_until = None;
        chip_detect.results.clear();
        chip_detect.best_result = None;
        chip_detect.usb_bridge = None; // 清除旧 USB 桥接识别

        // Ollama 识别结果一并清除，避免跨设备残留（feature gate）
        #[cfg(feature = "ollama")]
        {
            ollama_chip.result = None;
            ollama_chip.applied = false;
            ollama_chip.last_auto_key = None;
        }

        cortex_m3_probe.running = false;
        cortex_m3_probe.probe_index = 0;
        cortex_m3_probe.wait_until = None;
        cortex_m3_probe.results.clear();
        cortex_m3_probe.best_hint = None;
        cortex_m3_probe.log.clear();

        deep_probe.running = false;
        deep_probe.wait_until = None;
        deep_probe.results.clear();
        deep_probe.log.clear();

        mcu_probe.running = false;
        mcu_probe.probe_index = 0;
        mcu_probe.sent = false;
        mcu_probe.wait_until = None;
        mcu_probe.rx_start_seq = 0;
        mcu_probe.results.clear();
        mcu_probe.log.clear();
        mcu_probe.triggered_by_insole = false;
        mcu_probe.auto_triggered = false; // 连接变化时重置，允许再次自动触发
        // 清除 DTR/RTS 预触发和被动分析状态
        mcu_probe.pre_trigger_phase = 0;
        mcu_probe.pre_trigger_until = None;
        mcu_probe.dtr_rts_tried = false;
        mcu_probe.frame_intervals_ms.clear();
        mcu_probe.passive_result = None;

        // 同步清除 main_state 中的识别结果
        s.detect_result = None;
        s.last_response.clear();

        *last_session_id = current_session_id;
    }

    // 1. 帧驱动自动侦测状态机（需要 &mut AppState）
    update_auto_detect(state, detect);
    update_chip_detect(state, chip_detect);
    update_cortex_m3_probe(state, zh, cortex_m3_probe);
    // 深度总线探测帧驱动状态机（与芯片识别独立运行，避免互相干扰）
    update_deep_probe(state, deep_probe);

    // 1.5 同步 USB 桥接芯片识别（通过 VID/PID，无需协议探测）
    // 注意：必须在 update_mcu_probe 之前执行，否则被动分析时拿不到 USB 识别结果
    let usb_bridge_info = state
        .port_list
        .get(state.selected_port_index)
        .and_then(|p| {
            identify_usb_bridge(
                p.vid,
                p.pid,
                p.description.as_deref(),
                p.manufacturer.as_deref(),
            )
        });
    chip_detect.usb_bridge = usb_bridge_info.clone();

    // ===== Ollama 本地模型芯片识别（feature gate，Phase 3b）=====
    // 必须在 update_mcu_probe 之前完成 usb_bridge 兜底合成，
    // 否则被动分析拿不到 LLM 合成的桥接信息。
    #[cfg(feature = "ollama")]
    ollama_chip_update(state, chip_detect, ollama_chip);

    // MCU Boot ROM 探测（鞋垫/床垫协议检测后自动触发，或手动启动）
    update_mcu_probe(state, mcu_probe, deep_probe, chip_detect.usb_bridge.as_ref());

    // ★ USB 原生 MCU 后处理：直接修改被动分析结果，不依赖参数传递链路
    // 当 USB 识别为原生 MCU（非桥接芯片）时，这是芯片类型的直接证据
    // 优先级高于被动协议分析，应大幅加分
    if let Some(usb) = &chip_detect.usb_bridge {
        if usb.is_native_mcu {
            if let Some(result) = &mut mcu_probe.passive_result {
                // 检查是否已经做过 USB 后处理（避免重复加分）
                let usb_tag = crate::i18n::t("USB native MCU");
                let already_processed = result.candidates.iter().any(|c|
                    c.match_reasons.iter().any(|r| r.contains(usb_tag)) ||
                    c.mismatch_reasons.iter().any(|r| r.contains(usb_tag))
                );
                if !already_processed {
                    for candidate in &mut result.candidates {
                        let usb_match = usb.vendor == candidate.vendor
                            || (usb.chip_name.contains("ESP32") && candidate.vendor == "Espressif")
                            || (usb.chip_name.contains("STM32") && candidate.vendor == "STMicroelectronics");
                        if usb_match {
                            candidate.score = (candidate.score + 0.50).min(0.99);
                            candidate.match_reasons.push(crate::i18n::tf(
                                "USB native MCU match: {vendor} {chip} {vid}:{pid}",
                                &[
                                    ("{vendor}", "{vendor}", &crate::i18n::t_owned(&usb.vendor)),
                                    ("{chip}", "{chip}", &usb.chip_name),
                                    ("{vid}", "{vid}", &format!("{:04X}", usb.vid)),
                                    ("{pid}", "{pid}", &format!("{:04X}", usb.pid)),
                                ],
                            ));
                        } else {
                            candidate.score = (candidate.score - 0.15).max(0.0);
                            candidate.mismatch_reasons.push(crate::i18n::tf(
                                "USB native MCU points to {vendor} {chip}, does not match {candidate}",
                                &[
                                    ("{vendor}", "{vendor}", &crate::i18n::t_owned(&usb.vendor)),
                                    ("{chip}", "{chip}", &usb.chip_name),
                                    ("{candidate}", "{candidate}", &candidate.vendor),
                                ],
                            ));
                        }
                    }
                    // 重新排序（降序）
                    result.candidates.sort_by(|a, b|
                        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
                    );
                    // 更新 top candidate 信息
                    if let Some(top) = result.candidates.first() {
                        result.vendor = top.vendor.clone();
                        result.chip_family = top.chip_family.clone();
                        result.confidence = top.score;
                    }
                }
            }
        }
    }

    // 2. 同步最佳识别结果到 main_state
    if let Some(best) = &detect.best_result {
        s.detect_result = Some(best.clone());
        // 用最后一条有响应的探测结果更新 last_response
        if let Some(last_rx) = detect.results.iter().rev().find(|r| !r.rx.is_empty()) {
            s.last_response = last_rx.rx.clone();
        }
    }

    // 3. 渲染 UI
    let mut apply_detect = false;
    let mut send_clicked = false;
    {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 模式切换：自动侦测 / 手动模式
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Mode:"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.radio_value(
                        &mut s.manual_mode,
                        false,
                        crate::i18n::t("Auto Detect"),
                    );
                    ui.radio_value(
                        &mut s.manual_mode,
                        true,
                        crate::i18n::t("Manual"),
                    );
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                if s.manual_mode {
                    // ===== 手动模式：协议配置 + 手动发送 + 命令列表 + 响应解析 + 日志 =====
                    render_config_section(ui, s, zh);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    send_clicked = render_send_section(ui, s, zh, connected);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    render_command_list(ui, s, zh);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    render_response_section(ui, s, zh);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    render_log_section(ui, s, zh);
                } else {
                    // ===== 自动侦测模式 =====
                    render_auto_detect_section(ui, zh, connected, detect);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // AI 增强协议识别（与规则侦测结果并列对比）
                    render_ai_protocol_section(ui, &state.rx_buffer, detect.best_result.as_ref());
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // 芯片方案自动识别
                    #[cfg(feature = "ollama")]
                    render_chip_detect_section(ui, state, zh, connected, chip_detect, ollama_chip);
                    #[cfg(not(feature = "ollama"))]
                    render_chip_detect_section(ui, state, zh, connected, chip_detect);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // 深度总线探测
                    render_deep_probe_section(ui, ctx, state, zh, connected, deep_probe, chip_detect, cortex_m3_probe, mcu_probe);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    render_response_section(ui, s, zh);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    render_detect_section(ui, s, zh, &mut apply_detect, chip_detect);
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    render_log_section(ui, s, zh);
                }
            });
    }

    // 执行手动发送（需在状态借用外调用，因为需要 &mut AppState）
    if send_clicked {
        do_send(state, zh, s);
    }

    // 应用识别结果到配置（同时同步到向导）
    if apply_detect {
        apply_detect_result(s);
        apply_detect_to_wizard(ws, detect);
    }

    // 错误提示
    if let Some(err) = &state.last_error {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("\u{26A0} {}", err))
                .color(theme::ERROR)
                .small(),
        );
    }
}

/// 渲染「快速开始」引导区（6 步流程）
fn render_quick_start_guide(ui: &mut egui::Ui, _zh: bool) {
    egui::Frame::group(ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Follow these steps to perform protocol analysis and chip identification:"))
                .color(theme::TEXT_PRIMARY)
                .small(),
            );
            ui.add_space(6.0);

            // 步骤列表
            let steps: &[(&str, &str)] = &[
                ("1", "Connect the serial device (in the Connection panel)"),
                ("2", "Select the correct baud rate (or use auto-detect)"),
                ("3", "Click the Start Auto Detect button"),
                ("4", "Wait for analysis results (frame format, data bits, parity)"),
                ("5", "To identify the chip, click Chip Identification"),
                ("6", "After successful identification, jump to the corresponding flasher panel"),
            ];

            egui::Grid::new("quick_start_steps_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for (num, text) in steps {
                        ui.label(
                            RichText::new(format!("[{}]", num))
                                .color(theme::ACCENT)
                                .strong()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t(*text))
                                .color(theme::TEXT_PRIMARY)
                                .small(),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // 提示
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{2139}").color(theme::CYAN).small());
                ui.label(
                    RichText::new(crate::i18n::t("Tip: Auto-detect sends multiple probe commands (Modbus, SLCAN, AT, SCPI, etc.) \\\n                         in sequence. Ensure the device is powered and connected to the correct port."))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            });
        });
}

/// 帧驱动自动侦测状态机
/// 需要单独的 &mut AppState 参数（用于串口 I/O）
fn update_auto_detect(state: &mut AppState, ad: &mut AutoDetectState) {
    if !ad.running {
        return;
    }

    if ad.wait_until.is_none() {
        // 发送下一条探测命令
        if ad.probe_index >= PROBES.len() {
            ad.running = false;
            // 选择最佳结果
            if ad.best_result.is_none() {
                ad.best_result = ad.results.iter()
                    .filter_map(|r| r.detect.clone())
                    .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                    .clone();
            }
            return;
        }
        let (_name, cmd) = PROBES[ad.probe_index];
        ad.rx_start_seq = state.rx_entry_seq;
        let _ = state.send_raw(cmd);
        ad.wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
    } else {
        let until = ad.wait_until.unwrap();
        if std::time::Instant::now() >= until {
            // 超时，收集响应
            let rx = collect_rx_since(state, ad.rx_start_seq);

            let (name, cmd) = PROBES[ad.probe_index];
            let detect = if rx.is_empty() {
                None
            } else {
                auto_detect_protocol(cmd, &rx)
            };

            let note = if rx.is_empty() {
                crate::i18n::t("No response received").to_string()
            } else if let Some(d) = &detect {
                if d.confidence >= 0.7 {
                    crate::i18n::tf("✓ Identified ({pct}%)", &[
                        ("{pct}", "{pct}", &format!("{:.0}", d.confidence * 100.0)),
                    ])
                } else {
                    crate::i18n::tf("Confidence {pct}%", &[
                        ("{pct}", "{pct}", &format!("{:.0}", d.confidence * 100.0)),
                    ])
                }
            } else {
                crate::i18n::t("Not identified").to_string()
            };

            // 如果识别成功（置信度 >= 0.7），保存最佳结果并停止
            let mut stop = false;
            if let Some(d) = &detect {
                if d.confidence >= 0.7 {
                    ad.best_result = Some(d.clone());
                    stop = true;
                }
            }

            ad.results.push(ProbeResult {
                name: name.to_string(),
                tx: cmd.to_vec(),
                rx: rx.clone(),
                detect,
                note,
            });

            ad.probe_index += 1;
            ad.wait_until = None;

            if stop || ad.probe_index >= PROBES.len() {
                ad.running = false;
                if ad.best_result.is_none() {
                    ad.best_result = ad.results.iter()
                        .filter_map(|r| r.detect.clone())
                        .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
                        .clone();
                }
            }
        }
    }
}

/// 帧驱动芯片方案识别状态机
/// 必须在状态借用外调用，因为需要 &mut AppState
fn update_chip_detect(state: &mut AppState, cd: &mut ChipDetectState) {
    if !cd.running {
        return;
    }

    if cd.wait_until.is_none() {
        // 发送下一条探测命令
        if cd.probe_index >= CHIP_PROBES.len() {
            cd.running = false;
            // 选择最佳结果
            if cd.best_result.is_none() {
                cd.best_result = cd
                    .results
                    .iter()
                    .filter_map(|r| r.detect.clone())
                    .max_by(|a, b| {
                        a.confidence
                            .partial_cmp(&b.confidence)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
            }
            return;
        }
        let (_name, cmd) = CHIP_PROBES[cd.probe_index];
        cd.rx_start_seq = state.rx_entry_seq;
        let _ = state.send_raw(cmd);
        // STM32 bootloader 响应较慢，ESP32 SYNC 需要握手，给 500ms
        cd.wait_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
    } else {
        let until = cd.wait_until.unwrap();
        if std::time::Instant::now() >= until {
            // 超时，收集响应
            let rx = collect_rx_since(state, cd.rx_start_seq);

            let (name, cmd) = CHIP_PROBES[cd.probe_index];
            let detect = if rx.is_empty() {
                None
            } else {
                detect_chip(cmd, &rx)
            };

            let note = if rx.is_empty() {
                crate::i18n::t("No response received").to_string()
            } else if let Some(d) = &detect {
                if d.confidence >= 0.7 {
                    crate::i18n::tf("✓ {family} ({pct}%)", &[
                        ("{family}", "{family}", &crate::i18n::t_owned(&d.chip_family)),
                        ("{pct}", "{pct}", &format!("{:.0}", d.confidence * 100.0)),
                    ])
                } else {
                    crate::i18n::tf("Suspected {family} ({pct}%)", &[
                        ("{family}", "{family}", &crate::i18n::t_owned(&d.chip_family)),
                        ("{pct}", "{pct}", &format!("{:.0}", d.confidence * 100.0)),
                    ])
                }
            } else {
                crate::i18n::t("No matching chip signature").to_string()
            };

            // 如果识别成功（置信度 >= 0.7），保存最佳结果并停止
            let mut stop = false;
            if let Some(d) = &detect {
                if d.confidence >= 0.7 {
                    cd.best_result = Some(d.clone());
                    stop = true;
                }
            }

            cd.results.push(ChipProbeResult {
                name: name.to_string(),
                tx: cmd.to_vec(),
                rx: rx.clone(),
                detect,
                note,
            });

            cd.probe_index += 1;
            cd.wait_until = None;

            if stop || cd.probe_index >= CHIP_PROBES.len() {
                cd.running = false;
                if cd.best_result.is_none() {
                    cd.best_result = cd
                        .results
                        .iter()
                        .filter_map(|r| r.detect.clone())
                        .max_by(|a, b| {
                            a.confidence
                                .partial_cmp(&b.confidence)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                }
            }
        }
    }
}

// ============================================================================
// Ollama 本地模型芯片识别（feature gate，Phase 3b）
// 触发场景：A. VID:PID 未知（规则库未收录）→ 识别桥接芯片
//          B. MCU 协议探测全部失败 → 识别目标 MCU
// 回写原则：规则引擎优先，LLM 仅回填规则失败的空位，置信度门槛防幻觉
// ============================================================================

/// Ollama 芯片识别主流程（show() 每帧调用）：状态轮询 + 结果回收 + 自动触发 + 兜底合成
#[cfg(feature = "ollama")]
fn ollama_chip_update(
    state: &mut AppState,
    cd: &mut ChipDetectState,
    o: &mut OllamaChipState,
) {
    // 设置页联动：签名变化时刷新配置并强制重查状态
    crate::settings::sync_panel_config(
        &mut o.config,
        &mut o.cfg_sig,
        &mut o.status,
        &mut o.status_checked_at,
        &state.settings.ollama,
    );
    // ---- 1. 服务状态轮询（30s 节流，后台线程，不阻塞 UI）----
    let status_due = o
        .status_checked_at
        .map_or(true, |t| t.elapsed() > std::time::Duration::from_secs(30));
    if status_due && !o.status_busy {
        o.status_busy = true;
        let slot = o.status_slot.clone();
        let cfg = o.config.clone();
        std::thread::spawn(move || {
            let status = OllamaClient::new(cfg)
                .map(|c| c.status_blocking())
                .unwrap_or(OllamaStatus::NotRunning);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
        });
    }
    if o.status_busy {
        if let Some(st) = o.status_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            o.status = Some(st);
            o.status_checked_at = Some(std::time::Instant::now());
            o.status_busy = false;
        }
    }

    // ---- 2. 结果回收 + 回写（McuProbe 场景 → best_result）----
    if o.busy && o.done_flag.load(Ordering::Relaxed) {
        if let Some(res) = o.result_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            match res {
                Ok(r) => {
                    // MCU 探测场景：置信度 ≥0.6 且规则引擎无结论 → 填充 best_result
                    if matches!(o.task, Some(OllamaChipTask::McuProbe) | None)
                        && r.confidence >= 0.6
                        && cd.best_result.is_none()
                    {
                        if let Some(d) = mcu_result_to_detect(&r) {
                            cd.best_result = Some(d);
                            o.applied = true;
                        }
                    }
                    o.result = Some(r);
                }
                Err(e) => {
                    // 静默降级：仅记日志，不打扰用户
                    log::warn!("{}", crate::i18n::tf("Ollama chip identification failed (silent fallback): {error}", &[("{error}", "{error}", &format!("{}", e))]));
                }
            }
        }
        o.busy = false;
        o.done_flag.store(false, Ordering::Relaxed);
    }

    // ---- 3. 自动触发（防同场景重复：last_auto_key）----
    // 场景 A：端口有 VID:PID 但规则库未收录
    let key_a: Option<String> = cd.usb_bridge.is_none().then(|| {
        state
            .port_list
            .get(state.selected_port_index)
            .and_then(|p| p.vid.zip(p.pid))
            .map(|(v, d)| format!("vidpid:{:04X}:{:04X}", v, d))
    }).flatten();
    // 场景 B：MCU 协议探测全部完成但未识别到任何芯片
    let key_b: Option<String> = (!cd.running
        && cd.probe_index >= CHIP_PROBES.len()
        && cd.best_result.is_none()
        && !cd.results.is_empty())
        .then(|| format!("mcuprobe:{}", state.connection_session_id));
    let auto = key_a
        .map(|k| (k, OllamaChipTask::VidPid))
        .or_else(|| key_b.map(|k| (k, OllamaChipTask::McuProbe)));
    if let Some((key, task)) = auto {
        if o.last_auto_key.as_deref() != Some(key.as_str()) {
            let ev = build_chip_evidence(state, cd);
            if trigger_chip_identify(o, ev, task) {
                o.last_auto_key = Some(key);
            }
        }
    }

    // ---- 4. usb_bridge 兜底合成（每帧执行：规则引擎每帧重算，LLM 结果需持续兜底）----
    // 仅当场景 A（有 VID:PID）且 LLM 置信度 ≥0.7；合成后自动流入下游逻辑
    // （被动分析加分、CH341 I2C 判断等），与规则引擎行为完全一致。
    if cd.usb_bridge.is_none() {
        if let (Some(r), Some((vid, pid))) = (
            &o.result,
            state
                .port_list
                .get(state.selected_port_index)
                .and_then(|p| p.vid.zip(p.pid)),
        ) {
            if matches!(o.task, Some(OllamaChipTask::VidPid) | None) {
                if let Some(synth) = synthesize_bridge_info(r, vid, pid) {
                    cd.usb_bridge = Some(synth);
                    o.applied = true;
                }
            }
        }
    }
}

/// 构造芯片识别证据包：USB 描述符 + MCU 探测响应 + 规则 hint
#[cfg(feature = "ollama")]
fn build_chip_evidence(state: &AppState, cd: &ChipDetectState) -> EvidenceBundle {
    let mut ev = EvidenceBundle::new();
    if let Some(p) = state.port_list.get(state.selected_port_index) {
        ev.vid_pid = p.vid.zip(p.pid);
        ev.port_name = Some(p.name.clone());
        if let Some(d) = &p.description {
            ev.extra_notes.push(crate::i18n::tf("USB product description: {desc}", &[
                ("{desc}", "{desc}", &format!("{}", d)),
            ]));
        }
        if let Some(m) = &p.manufacturer {
            ev.extra_notes.push(crate::i18n::tf("USB manufacturer string: {mfr}", &[
                ("{mfr}", "{mfr}", &format!("{}", m)),
            ]));
        }
    }
    // MCU 协议探测响应（有响应的才提供，最多 8 条，每条 48 字节 HEX）
    for r in cd.results.iter().take(8) {
        if !r.rx.is_empty() {
            let hex: String = r
                .rx
                .iter()
                .take(48)
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" ");
            ev.probe_responses.push((r.name.clone(), hex));
        }
    }
    if let Some(best) = &cd.best_result {
        ev.rule_hint = Some((best.chip_family.clone(), best.confidence));
    }
    ev
}

/// 启动芯片识别后台线程。返回 false 表示未启动（未就绪/进行中）。
#[cfg(feature = "ollama")]
fn trigger_chip_identify(
    o: &mut OllamaChipState,
    ev: EvidenceBundle,
    task: OllamaChipTask,
) -> bool {
    if o.busy {
        return false;
    }
    if !matches!(o.status, Some(OllamaStatus::Ready(_))) {
        return false;
    }
    o.busy = true;
    o.task = Some(task);
    o.applied = false;
    o.done_flag.store(false, Ordering::Relaxed);
    let slot = o.result_slot.clone();
    let flag = o.done_flag.clone();
    let cfg = o.config.clone();
    std::thread::spawn(move || {
        let res = OllamaClient::new(cfg)
            .map_err(|e| e.to_string())
            .and_then(|c| {
                miconlab_ai::identify::identify_chip(&c, &ev).map_err(|e| e.to_string())
            });
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
        flag.store(true, Ordering::Relaxed);
    });
    true
}

/// LLM 结果 → UsbBridgeInfo 合成（纯函数，便于测试）。
///
/// 门槛：置信度 ≥0.7（合成结果会流入下游被动分析，要求高置信）。
/// `is_native_mcu` 由 family 推断：非 "usb-uart" 族视为 MCU 原生 USB。
#[cfg(feature = "ollama")]
fn synthesize_bridge_info(r: &ChipIdentifyResult, vid: u16, pid: u16) -> Option<UsbBridgeInfo> {
    if r.confidence < 0.7 {
        return None;
    }
    let is_native = !r.family.to_lowercase().contains("usb-uart");
    Some(UsbBridgeInfo {
        chip_name: r.chip.clone(),
        vendor: r.vendor.clone(),
        vid,
        pid,
        description: None,
        is_native_mcu: is_native,
    })
}

/// LLM 结果 → ChipDetectResult（McuProbe 场景回填，纯函数，便于测试）。
///
/// 门槛：置信度 ≥0.6。
#[cfg(feature = "ollama")]
fn mcu_result_to_detect(r: &ChipIdentifyResult) -> Option<ChipDetectResult> {
    if r.confidence < 0.6 {
        return None;
    }
    Some(ChipDetectResult {
        chip_family: r.chip.clone(),
        chip_description: crate::i18n::tf("{vendor} / {family} (identified by local Ollama model, advisory result)", &[
            ("{vendor}", "{vendor}", &crate::i18n::t_owned(&r.vendor)),
            ("{family}", "{family}", &crate::i18n::t_owned(&r.family)),
        ]),
        confidence: r.confidence,
        evidence: r.reasoning.clone(),
    })
}

/// 通用 Cortex-M3 自动探测状态机
/// 在 show() 中每帧调用，需要 &mut AppState
fn update_cortex_m3_probe(state: &mut AppState, _zh: bool, p: &mut CortexM3ProbeState) {
    if !p.running {
        return;
    }

    if p.wait_until.is_none() {
        // 发送下一条探测命令
        if p.probe_index >= CORTEX_M3_PROBES.len() {
            // 所有探测完成
            p.running = false;
            let log_msg = if p.best_hint.is_some() {
                crate::i18n::tf("✓ Auto-probe complete: chip identified (hint={})", &[("{}", "{}", &format!("{}", p.best_hint.unwrap()))])
            } else { crate::i18n::t("✗ Auto-probe complete: no chip identified, will show manual dialog").to_string() };
            p.log.push(log_msg);
            return;
        }
        let (name, cmd, _expected, _hint) = CORTEX_M3_PROBES[p.probe_index];
        p.rx_start_seq = state.rx_entry_seq;
        let _ = state.send_raw(cmd);
        p.wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
        let log_msg = { crate::i18n::tf("→ [{}] Sending probe...", &[("{}", "{}", &format!("{}", name))]) };
        p.log.push(log_msg);
    } else {
        let until = p.wait_until.unwrap();
        if std::time::Instant::now() >= until {
            // 超时，收集响应
            let rx = collect_rx_since(state, p.rx_start_seq);

            let (name, cmd, expected, hint) = CORTEX_M3_PROBES[p.probe_index];

            // 判断是否匹配
            let matched = if rx.is_empty() {
                false
            } else if name.starts_with("NXP LPC") {
                // LPC 响应包含 "Synchronized" 字符串
                String::from_utf8_lossy(&rx).contains(String::from_utf8_lossy(expected).as_ref())
            } else if name.starts_with("WCH ISP") {
                // WCH ISP 响应格式：0x57 0xAB <cmd> ...，要求响应以 expected 序列开头
                // （修复：原逻辑仅检查任意位置是否出现 0x82，与协议不符）
                rx.len() >= expected.len() && rx[..expected.len()] == *expected
            } else {
                // STM32: 响应首字节为 0x79 (ACK) 或 0x1F (NACK)
                rx[0] == expected[0]
            };

            let log_msg = if rx.is_empty() {
                crate::i18n::tf("  ✗ [{}] No response", &[("{}", "{}", &format!("{}", name))])
            } else if matched {
                crate::i18n::tf("  ✓ [{}] Matched! → hint={}", &[("{}", "{}", &format!("{}", name)), ("{}", "{}", &format!("{}", hint))])
            } else {
                let rx_hex: String = rx.iter().take(16).map(|b| format!("{:02X} ", b)).collect();
                crate::i18n::tf("  ✗ [{}] No match: {}", &[("{}", "{}", &format!("{}", name)), ("{}", "{}", &format!("{}", rx_hex))])
            };
            p.log.push(log_msg);

            p.results.push((name, cmd.to_vec(), rx.clone(), if matched { Some(hint) } else { None }));

            if matched {
                p.best_hint = Some(hint);
                p.running = false;
                return;
            }

            p.probe_index += 1;
            p.wait_until = None;

            if p.probe_index >= CORTEX_M3_PROBES.len() {
                p.running = false;
                let log_msg = crate::i18n::t("✗ Auto-probe complete: no chip identified, will show manual dialog").to_string();
                p.log.push(log_msg);
            }
        }
    }
}

/// 根据探测命令和响应识别芯片方案
///
/// 识别策略：
/// - STM32 Bootloader: 发送 0x7F，回应 0x79 (ACK) 或 0x1F (NACK)
/// - ESP32 ROM Bootloader: SYNC 回应包含 0x55 0x00 0x07 0x07 0x12 0x20
/// - ESP32/ESP8266 AT 固件: AT+GMR 响应包含 "ESP"
/// - Arduino/AVR: STK500 GET_SYNC 回应 0x14 0x10
/// - 通用 AT: AT 返回 OK（低置信度）
/// - SCPI: *IDN? 返回芯片型号标识
fn detect_chip(tx: &[u8], rx: &[u8]) -> Option<ChipDetectResult> {
    if rx.is_empty() {
        return None;
    }

    // 1. STM32 USART Bootloader: 发送 0x7F，回应 0x79 (ACK) 或 0x1F (NACK)
    if tx == [0x7F] {
        if rx[0] == 0x79 {
            return Some(ChipDetectResult {
                chip_family: "STM32".to_string(),
                chip_description: crate::i18n::t("STM32 series (USART Bootloader active, BOOT0=1)").to_string(),
                confidence: 0.95,
                evidence: crate::i18n::t("Received ACK (0x79), STM32 ROM Bootloader ready").to_string(),
            });
        }
        if rx[0] == 0x1F {
            return Some(ChipDetectResult {
                chip_family: "STM32".to_string(),
                chip_description: crate::i18n::t("STM32 series (Bootloader returned NACK, user firmware may be running)")
                    .to_string(),
                confidence: 0.7,
                evidence: crate::i18n::t("Received NACK (0x1F), STM32 ROM Bootloader detected but not ready").to_string(),
            });
        }
    }

    // 2. ESP32 ROM Bootloader SYNC: 回应包含 0x55 0x00 0x07 0x07 0x12 0x20
    if tx.starts_with(&[0x07, 0x07, 0x12, 0x20]) {
        let sync_resp: [u8; 6] = [0x55, 0x00, 0x07, 0x07, 0x12, 0x20];
        if rx.windows(6).any(|w| w == sync_resp) {
            return Some(ChipDetectResult {
                chip_family: "ESP32".to_string(),
                chip_description: crate::i18n::t("ESP32 series (ROM Bootloader SYNC succeeded)").to_string(),
                confidence: 0.95,
                evidence: crate::i18n::t("Received SYNC response 55 00 07 07 12 20").to_string(),
            });
        }
        // ESP32 也可能回应 0x55 0x00 0x07 0x07 0x12 0x21 等
        if rx.windows(4).any(|w| w == [0x07, 0x07, 0x12, 0x20]) {
            return Some(ChipDetectResult {
                chip_family: "ESP32".to_string(),
                chip_description: crate::i18n::t("ESP32 series (partial ROM Bootloader response)").to_string(),
                confidence: 0.7,
                evidence: crate::i18n::t("Received partial SYNC response signature").to_string(),
            });
        }
    }

    // 3. ESP32/ESP8266 AT+GMR: 响应包含 "ESP"
    if tx == b"AT+GMR\r\n" {
        let rx_str = String::from_utf8_lossy(rx);
        let upper = rx_str.to_uppercase();
        if upper.contains("ESP32") {
            return Some(ChipDetectResult {
                chip_family: "ESP32".to_string(),
                chip_description: crate::i18n::tf("ESP32 AT firmware (version: {version})", &[
                    ("{version}", "{version}", rx_str.trim()),
                ]),
                confidence: 0.9,
                evidence: crate::i18n::t("AT+GMR response contains ESP32 signature").to_string(),
            });
        }
        if upper.contains("ESP8266") {
            return Some(ChipDetectResult {
                chip_family: "ESP8266".to_string(),
                chip_description: crate::i18n::tf("ESP8266 AT firmware (version: {version})", &[
                    ("{version}", "{version}", rx_str.trim()),
                ]),
                confidence: 0.9,
                evidence: crate::i18n::t("AT+GMR response contains ESP8266 signature").to_string(),
            });
        }
        if upper.contains("ESP") {
            return Some(ChipDetectResult {
                chip_family: "ESP series".to_string(),
                chip_description: crate::i18n::tf("ESP series AT firmware (version: {version})", &[
                    ("{version}", "{version}", rx_str.trim()),
                ]),
                confidence: 0.8,
                evidence: crate::i18n::t("AT+GMR response contains ESP signature").to_string(),
            });
        }
    }

    // 4. Arduino STK500: 发送 0x30 0x20，回应 0x14 0x10 (STK_INSYNC + STK_OK)
    if tx == [0x30, 0x20] {
        if rx.len() >= 2 && rx[0] == 0x14 && rx[1] == 0x10 {
            return Some(ChipDetectResult {
                chip_family: "Arduino/AVR".to_string(),
                chip_description: crate::i18n::t("Arduino series (AVR MCU, STK500 protocol)").to_string(),
                confidence: 0.85,
                evidence: crate::i18n::t("Received STK_INSYNC (0x14) + STK_OK (0x10)").to_string(),
            });
        }
    }

    // 5. 通用 AT: 回应 "OK"
    if tx == b"AT\r\n" {
        let rx_str = String::from_utf8_lossy(rx);
        if rx_str.contains("OK") {
            return Some(ChipDetectResult {
                chip_family: "AT device".to_string(),
                chip_description: crate::i18n::t("AT command device (possibly ESP / Bluetooth module / modem)").to_string(),
                confidence: 0.5,
                evidence: crate::i18n::t("AT command returned OK (further model identification needed)").to_string(),
            });
        }
    }

    // 6. SCPI *IDN?: 可能返回芯片型号标识
    if tx == b"*IDN?\r\n" {
        let rx_str = String::from_utf8_lossy(rx);
        let upper = rx_str.to_uppercase();
        if upper.contains("STM32") || upper.contains("ST,") || upper.contains("STMICRO") {
            return Some(ChipDetectResult {
                chip_family: "STM32".to_string(),
                chip_description: crate::i18n::tf("STM32 device (SCPI ID: {id})", &[
                    ("{id}", "{id}", rx_str.trim()),
                ]),
                confidence: 0.8,
                evidence: crate::i18n::t("*IDN? response contains STM32/ST signature").to_string(),
            });
        }
        if upper.contains("ESP32") {
            return Some(ChipDetectResult {
                chip_family: "ESP32".to_string(),
                chip_description: crate::i18n::tf("ESP32 device (SCPI ID: {id})", &[
                    ("{id}", "{id}", rx_str.trim()),
                ]),
                confidence: 0.8,
                evidence: crate::i18n::t("*IDN? response contains ESP32 signature").to_string(),
            });
        }
        if upper.contains("ESP8266") {
            return Some(ChipDetectResult {
                chip_family: "ESP8266".to_string(),
                chip_description: crate::i18n::tf("ESP8266 device (SCPI ID: {id})", &[
                    ("{id}", "{id}", rx_str.trim()),
                ]),
                confidence: 0.8,
                evidence: crate::i18n::t("*IDN? response contains ESP8266 signature").to_string(),
            });
        }
        if upper.contains("ARDUINO") {
            return Some(ChipDetectResult {
                chip_family: "Arduino/AVR".to_string(),
                chip_description: crate::i18n::tf("Arduino device (SCPI ID: {id})", &[
                    ("{id}", "{id}", rx_str.trim()),
                ]),
                confidence: 0.75,
                evidence: crate::i18n::t("*IDN? response contains Arduino signature").to_string(),
            });
        }
    }

    None
}

/// 渲染自动侦测控制区
fn render_auto_detect_section(ui: &mut egui::Ui, _zh: bool, connected: bool, ad: &mut AutoDetectState) {
    ui.label(
        RichText::new(crate::i18n::t("Auto Protocol Detection"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(4.0);

    // 未连接提示
    if !connected {
        ui.horizontal(|ui| {
            ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
            ui.label(
                RichText::new(crate::i18n::t("Not connected. Open a port in the Connection panel first."))
                .color(theme::WARNING)
                .small(),
            );
        });
        return;
    }

    // 开始/停止按钮
    ui.horizontal(|ui| {
        if ad.running {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("⏹ Stop"))
                    .color(egui::Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ERROR)
            .min_size(egui::Vec2::new(120.0, 28.0));
            if ui.add(btn).clicked() {
                ad.running = false;
            }
        } else {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("▶ Start Auto Detect"))
                .color(egui::Color32::BLACK)
                .strong(),
            )
            .fill(theme::SUCCESS)
            .min_size(egui::Vec2::new(140.0, 28.0));
            if ui
                .add(btn)
                .on_hover_text(crate::i18n::t("Sequentially sends probe commands (0x7F / 0x57 0xAB / JEDEC ID / IDCODE etc.) and auto-determines frame format, baud rate, data bits, parity and chip type from responses"))
                .clicked() {
                ad.running = true;
                ad.probe_index = 0;
                ad.results.clear();
                ad.best_result = None;
                ad.wait_until = None;
            }
        }

        // 进度显示
        if ad.results.len() > 0 || ad.running {
            let progress = if ad.running {
                crate::i18n::tf("{index}/{total} — {probe}", &[
                    ("{index}", "{index}", &format!("{}", ad.probe_index + 1)),
                    ("{total}", "{total}", &format!("{}", PROBES.len())),
                    ("{probe}", "{probe}", if ad.probe_index < PROBES.len() { crate::i18n::t(PROBES[ad.probe_index].0) } else { "" }),
                ])
            } else {
                crate::i18n::tf("Done ({done}/{total})", &[
                    ("{done}", "{done}", &format!("{}", ad.results.len())),
                    ("{total}", "{total}", &format!("{}", PROBES.len())),
                ])
            };
            ui.label(
                RichText::new(crate::i18n::tf("Progress: {}", &[("{}", "{}", &format!("{}", progress))]))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        }
    });

    // 探测进度条
    if ad.running || !ad.results.is_empty() {
        ui.add_space(4.0);
        let progress = ad.results.len() as f32 / PROBES.len() as f32;
        ui.add(egui::ProgressBar::new(progress).desired_width(200.0));
    }

    // 探测结果列表
    if !ad.results.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(crate::i18n::t("Probe Log"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);

        egui::Frame::none()
            .fill(theme::BG_DEEP)
            .inner_margin(egui::Margin::same(6.0))
            .rounding(4.0)
            .show(ui, |ui| {
                for (i, r) in ad.results.iter().enumerate() {
                    let color = if r.note.contains("✓") {
                        theme::SUCCESS
                    } else if r.rx.is_empty() {
                        theme::TEXT_DISABLED
                    } else {
                        theme::TEXT_SECONDARY
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{}.", i + 1))
                                .color(theme::TEXT_DISABLED)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t_owned(&r.name))
                                .color(theme::TEXT_PRIMARY)
                                .small(),
                        );
                        ui.label(
                            RichText::new(&r.note)
                                .color(color)
                                .small()
                                .strong(),
                        );
                    });

                    // TX/RX HEX 简要
                    let tx_hex: String = r.tx.iter().take(16).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                    let tx_more = if r.tx.len() > 16 { "..." } else { "" };
                    ui.label(
                        RichText::new(format!("  TX: {}{}", tx_hex, tx_more))
                            .color(theme::ACCENT_DIM)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                    if !r.rx.is_empty() {
                        let rx_hex: String = r.rx.iter().take(16).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                        let rx_more = if r.rx.len() > 16 { "..." } else { "" };
                        ui.label(
                            RichText::new(format!("  RX: {}{}", rx_hex, rx_more))
                                .color(theme::CYAN)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                    }
                    ui.add_space(2.0);
                }
            });

        // 最佳结果
        if let Some(best) = &ad.best_result {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::t("Best Match")).color(theme::SUCCESS).strong());
                ui.label(
                    RichText::new(format!("{}", best.protocol_type))
                        .color(theme::SUCCESS)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!("({:.0}%)", best.confidence * 100.0))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            });
        }
    }
}

/// 渲染 AI 增强协议识别区（与规则侦测结果并列对比）
///
/// 使用 miconlab-ai 的 KNN 协议分类器对最近接收数据进行分析，
/// 与规则侦测结果形成"规则侦测 vs AI 识别"对比展示。
fn render_ai_protocol_section(
    ui: &mut egui::Ui,
    rx_buffer: &[RxEntry],
    rule_result: Option<&DetectResult>,
) {
    ui.label(
        RichText::new(crate::i18n::t("AI Enhanced Protocol Recognition"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    // 收集最近 256 字节接收数据用于 AI 分析
    let rx_data = crate::panels::ai_sense::collect_recent_rx(rx_buffer, 256);

    if rx_data.len() < 16 {
        ui.horizontal(|ui| {
            ui.label(RichText::new("\u{1F4E1}").small());
            ui.label(
                RichText::new(crate::i18n::t("Insufficient data (<16 bytes), waiting for more data for AI recognition"))
                .color(theme::TEXT_DISABLED)
                .small(),
            );
        });
        return;
    }

    // AI 协议分类（KNN + 内置模板）
    let classifier = miconlab_ai::ProtocolClassifier::with_builtin_templates();
    let features = miconlab_ai::StreamFeatures::extract(&rx_data, None);
    let (class, conf) = classifier.predict(&features);

    // 置信度颜色编码（与 ai_sense::render_protocol_badge 保持一致）
    let conf_color = if conf >= 0.7 {
        theme::SUCCESS
    } else if conf >= 0.4 {
        theme::WARNING
    } else {
        theme::TEXT_DISABLED
    };

    egui::Frame::group(ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            if class == miconlab_ai::ProtocolClass::Unknown || conf < 0.3 {
                // AI 未能识别
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{1F916}").small());
                    ui.label(
                        RichText::new(crate::i18n::t("AI could not identify the protocol. Collect more data or adjust baud rate"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                    );
                });
                ui.label(
                    RichText::new(crate::i18n::tf("Sample size: {} bytes", &[("{}", "{}", &format!("{}", rx_data.len()))]))
                    .color(theme::TEXT_DISABLED)
                    .small(),
                );
                return;
            }

            // 识别成功：渲染协议类型 + 置信度
            let label = crate::i18n::t_owned(class.label_en());
            let desc = crate::i18n::t_owned(&class.description_zh());

            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{1F916}").small());
                ui.label(
                    RichText::new(crate::i18n::t("AI: "))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.label(RichText::new(&label).color(conf_color).strong());
                ui.label(
                    RichText::new(format!("({:.0}%)", conf * 100.0))
                        .color(conf_color)
                        .small(),
                );
            });

            // 协议描述
            ui.add_space(2.0);
            ui.label(
                RichText::new(desc)
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );

            // 推荐波特率
            if let Some(baud) = class.recommended_baud() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Recommended baud: "))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("{} baud", baud))
                            .color(theme::ACCENT)
                            .small()
                            .strong(),
                    );
                });
            }

            // 与规则侦测结果对比
            if let Some(rule) = rule_result {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);

                let rule_label = format!("{}", rule.protocol_type);
                let relation = compare_ai_with_rule(class, &rule.protocol_type);

                let (cmp_text, cmp_color) = match relation {
                    AiRuleRelation::Agree => (
                        crate::i18n::t("✓ Consistent with rule detection"),
                        theme::SUCCESS,
                    ),
                    AiRuleRelation::Complementary => (
                        crate::i18n::t("💡 AI provides more precise identification"),
                        theme::CYAN,
                    ),
                    AiRuleRelation::Differs => (
                        crate::i18n::t("⚠ Differs from rule detection"),
                        theme::WARNING,
                    ),
                };

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Compare: "))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(RichText::new(cmp_text).color(cmp_color).small().strong());
                });

                // 并列展示规则 vs AI
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Rule: "))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("{} ({:.0}%)", rule_label, rule.confidence * 100.0))
                            .color(theme::TEXT_PRIMARY)
                            .small(),
                    );
                    ui.label(RichText::new("|").color(theme::TEXT_DISABLED).small());
                    ui.label(
                        RichText::new(crate::i18n::t("AI: "))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("{} ({:.0}%)", label, conf * 100.0))
                            .color(conf_color)
                            .small(),
                    );
                });
            }
        });
}

/// AI 识别与规则侦测结果的关系
enum AiRuleRelation {
    /// 一致
    Agree,
    /// AI 提供更精确识别（规则为通用类型，AI 给出具体协议）
    Complementary,
    /// 存在差异
    Differs,
}

/// 判断 AI 协议分类与规则侦测结果的一致性
///
/// 规则侦测仅产生 `ModbusRtu` 或 `SerialFrame`（通用串口帧）两种类型：
/// - 规则 ModbusRtu + AI ModbusRtu/ModbusAscii → 一致
/// - 规则 ModbusRtu + AI 其他 → 存在差异
/// - 规则 SerialFrame + AI 任意具体协议 → AI 提供更精确识别
fn compare_ai_with_rule(ai_class: miconlab_ai::ProtocolClass, rule_type: &ProtocolType) -> AiRuleRelation {
    use miconlab_ai::ProtocolClass;
    match (ai_class, rule_type) {
        (ProtocolClass::ModbusRtu | ProtocolClass::ModbusAscii, ProtocolType::ModbusRtu) => {
            AiRuleRelation::Agree
        }
        (_, ProtocolType::ModbusRtu) => AiRuleRelation::Differs,
        (_, ProtocolType::SerialFrame) => AiRuleRelation::Complementary,
        // 其他协议类型（ADC/I2C/SPI）非串口协议，按默认一致处理
        _ => AiRuleRelation::Agree,
    }
}

// ===== 辅助函数 =====

/// 数据格式对应的字节数
fn data_format_size(format: &DataFormat) -> usize {
    match format {
        DataFormat::U8 => 1,
        DataFormat::U16BE | DataFormat::U16LE => 2,
        DataFormat::U32BE | DataFormat::U32LE | DataFormat::F32BE | DataFormat::F32LE => 4,
    }
}

/// 校验类型对应的字节数
fn checksum_size(t: &ChecksumType) -> usize {
    match t {
        ChecksumType::None => 0,
        ChecksumType::Sum8 | ChecksumType::Xor => 1,
        ChecksumType::Crc16 => 2,
    }
}

/// 从状态构建帧格式
fn build_frame_format(s: &ProtocolAnalyzerState) -> Option<FrameFormat> {
    let header = parse_hex(&s.header_hex).unwrap_or_default();
    let footer = parse_hex(&s.footer_hex).unwrap_or_default();

    Some(FrameFormat {
        header,
        footer,
        length_offset: if s.length_offset < 0 {
            None
        } else {
            Some(s.length_offset as usize)
        },
        length_bytes: s.length_bytes,
        data_offset: s.data_offset,
        checksum_type: s.checksum_type.clone(),
        checksum_offset: if s.checksum_offset < 0 {
            None
        } else {
            Some(s.checksum_offset as usize)
        },
        channel_count: s.channel_count,
        data_format: s.data_format.clone(),
    })
}

/// 从状态构建协议配置
fn build_config(s: &ProtocolAnalyzerState) -> ProtocolConfig {
    let frame_format = build_frame_format(s);
    ProtocolConfig {
        protocol_type: s.protocol_type.clone(),
        baud_rate: s.baud_rate,
        slave_id: Some(s.slave_id),
        frame_format,
        commands: Vec::new(),
        register_map: Vec::new(),
        i2c_address: None,
        adc_resolution: None,
        adc_ref_voltage: None,
    }
}

/// 协议自动识别：分析请求和响应数据，推断协议类型和帧格式
///
/// 识别策略（按优先级）：
/// 1. ASCII 文本协议（数据 80% 以上可打印）：NMEA / Modbus ASCII / SLCAN / JSON / AT / 通用文本
/// 2. Modbus RTU：从站ID/功能码匹配 + CRC16 校验通过
/// 3. 自定义二进制帧：扩展帧头 + 帧尾 + 多种校验(Sum8/XOR/CRC16LE/CRC16BE/LRC) + 智能数据格式推断
fn auto_detect_protocol(tx: &[u8], rx: &[u8]) -> Option<DetectResult> {
    if rx.is_empty() || rx.len() < 2 {
        return None;
    }

    // ===== 1. 优先检测 ASCII 文本协议（80% 以上可打印）=====
    if is_mostly_ascii(rx) {
        if let Some(r) = try_detect_nmea(rx) {
            return Some(r);
        }
        if let Some(r) = try_detect_modbus_ascii(rx) {
            return Some(r);
        }
        if let Some(r) = try_detect_slcan(rx) {
            return Some(r);
        }
        if let Some(r) = try_detect_json(rx) {
            return Some(r);
        }
        if let Some(r) = try_detect_at_response(rx) {
            return Some(r);
        }
        if let Some(r) = try_detect_ascii_text(rx) {
            return Some(r);
        }
    }

    // ===== 2. Modbus RTU =====
    if let Some(r) = try_detect_modbus_rtu(tx, rx) {
        return Some(r);
    }

    // ===== 3. 自定义二进制帧 =====
    if let Some(r) = try_detect_custom_frame(rx) {
        return Some(r);
    }

    None
}

/// 判断数据是否 80% 以上为 ASCII 可打印字符
fn is_mostly_ascii(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let printable = data
        .iter()
        .filter(|&&b| (0x20..=0x7E).contains(&b) || b == b'\r' || b == b'\n' || b == b'\t')
        .count();
    (printable as f32 / data.len() as f32) >= 0.8
}

/// 检测 NMEA 0183 协议（$GPGGA,...*XX\r\n）
fn try_detect_nmea(rx: &[u8]) -> Option<DetectResult> {
    // 必须以 $ 开头
    if rx.is_empty() || rx[0] != b'$' {
        return None;
    }
    // 查找 * 校验分隔符
    let star_pos = rx.iter().position(|&b| b == b'*')?;
    // * 之后至少 2 个 hex 字符 + 换行
    if rx.len() < star_pos + 4 {
        return None;
    }
    let hex_bytes = &rx[star_pos + 1..star_pos + 3];
    let hex_str = std::str::from_utf8(hex_bytes).ok()?;
    let stored = u8::from_str_radix(hex_str, 16).ok()?;
    // 必须以 \r\n 或 \n 结尾
    let tail_ok = rx.ends_with(b"\r\n") || rx.ends_with(b"\n");
    if !tail_ok {
        return None;
    }
    // XOR 校验：$ 和 * 之间（不含两者）的所有字节
    let calc: u8 = rx[1..star_pos].iter().fold(0u8, |a, &b| a ^ b);
    if calc != stored {
        return None;
    }
    let footer = if rx.ends_with(b"\r\n") {
        vec![b'\r', b'\n']
    } else {
        vec![b'\n']
    };
    let sentence = std::str::from_utf8(&rx[1..star_pos]).unwrap_or("").to_string();
    Some(DetectResult {
        protocol_type: ProtocolType::SerialFrame,
        header: vec![b'$'],
        footer,
        checksum_type: ChecksumType::Xor,
        data_format: DataFormat::U8,
        channel_count: 1,
        confidence: 0.95,
        description: crate::i18n::tf("NMEA 0183 — XOR checksum OK, sentence: ${sentence}*", &[
            ("{sentence}", "{sentence}", &sentence),
        ]),
    })
}

/// 检测 Modbus ASCII 协议（:AABB...XX\r\n，LRC 校验）
fn try_detect_modbus_ascii(rx: &[u8]) -> Option<DetectResult> {
    // 必须以 : 开头，\r\n 结尾
    if rx.is_empty() || rx[0] != b':' {
        return None;
    }
    if !rx.ends_with(b"\r\n") {
        return None;
    }
    // 中间必须是偶数个 hex 字符
    let body = &rx[1..rx.len() - 2];
    if body.len() < 4 || body.len() % 2 != 0 {
        return None;
    }
    if !body.iter().all(|&b| b.is_ascii_hexdigit()) {
        return None;
    }
    // 解码 hex
    let mut decoded = Vec::with_capacity(body.len() / 2);
    for chunk in body.chunks(2) {
        let s = std::str::from_utf8(chunk).ok()?;
        let v = u8::from_str_radix(s, 16).ok()?;
        decoded.push(v);
    }
    if decoded.len() < 2 {
        return None;
    }
    // LRC 校验：最后 1 字节 = -(前面所有字节求和) & 0xFF
    let sum: u8 = decoded[..decoded.len() - 1]
        .iter()
        .fold(0u8, |a, &b| a.wrapping_add(b));
    let lrc = 0u8.wrapping_sub(sum);
    if lrc != decoded[decoded.len() - 1] {
        return None;
    }
    Some(DetectResult {
        protocol_type: ProtocolType::ModbusRtu,
        header: vec![b':'],
        footer: vec![b'\r', b'\n'],
        checksum_type: ChecksumType::Sum8,
        data_format: DataFormat::U8,
        channel_count: 1,
        confidence: 0.95,
        description: crate::i18n::tf(
            "Modbus ASCII — LRC checksum OK, slave ID=0x{slave:02X}, function=0x{func:02X}",
            &[
                ("{slave:02X}", "{slave:02X}", &format!("{:02X}", decoded[0])),
                ("{func:02X}", "{func:02X}", &format!("{:02X}", decoded[1])),
            ],
        ),
    })
}

/// 检测 SLCAN 协议（t/T/r/R/o/O/c/C/s/S/v/V/b/B/F/M/G/W 开头 + \r 结尾）
fn try_detect_slcan(rx: &[u8]) -> Option<DetectResult> {
    if rx.is_empty() {
        return None;
    }
    const SLCAN_CMDS: &[u8] = b"tTrRoOcCsSvVbBFMGW";
    if !SLCAN_CMDS.contains(&rx[0]) {
        return None;
    }
    // 必须以 \r 结尾
    if !rx.ends_with(b"\r") {
        return None;
    }
    if rx.len() < 2 {
        return None;
    }
    // 命令字符后的 body 应为空或全 hex 字符
    let body = &rx[1..rx.len() - 1];
    if !body.is_empty() && !body.iter().all(|&b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(DetectResult {
        protocol_type: ProtocolType::SerialFrame,
        header: vec![rx[0]],
        footer: vec![b'\r'],
        checksum_type: ChecksumType::None,
        data_format: DataFormat::U8,
        channel_count: 1,
        confidence: 0.85,
        description: crate::i18n::tf("SLCAN — command starts with '{cmd}', ends with CR", &[
            ("{cmd}", "{cmd}", &format!("{}", rx[0] as char)),
        ]),
    })
}

/// 检测 JSON 协议（{ 开头 + } 结尾 + 含 :）
fn try_detect_json(rx: &[u8]) -> Option<DetectResult> {
    if rx.is_empty() || rx[0] != b'{' {
        return None;
    }
    // 必须以 }、}\n 或 }\r\n 结尾
    let ends_ok = rx.ends_with(b"}") || rx.ends_with(b"}\n") || rx.ends_with(b"}\r\n");
    if !ends_ok {
        return None;
    }
    let s = std::str::from_utf8(rx).ok()?;
    if !s.contains(':') {
        return None;
    }
    let footer = if rx.ends_with(b"\r\n") {
        vec![b'\r', b'\n']
    } else if rx.ends_with(b"\n") {
        vec![b'\n']
    } else {
        vec![b'}']
    };
    Some(DetectResult {
        protocol_type: ProtocolType::SerialFrame,
        header: vec![b'{'],
        footer,
        checksum_type: ChecksumType::None,
        data_format: DataFormat::U8,
        channel_count: 1,
        confidence: 0.8,
        description: crate::i18n::t("JSON text protocol — { ... } structure").to_string(),
    })
}

/// 检测 AT 命令响应（OK/ERROR/+XXX 开头）
fn try_detect_at_response(rx: &[u8]) -> Option<DetectResult> {
    if rx.is_empty() {
        return None;
    }
    let s = std::str::from_utf8(rx).ok()?;
    let trimmed = s.trim_start();
    let prefix = if trimmed.starts_with("OK") {
        "OK"
    } else if trimmed.starts_with("ERROR") {
        "ERROR"
    } else if trimmed.starts_with('+') {
        "+XXX"
    } else {
        return None;
    };
    let footer = if rx.ends_with(b"\r\n") {
        vec![b'\r', b'\n']
    } else if rx.ends_with(b"\n") {
        vec![b'\n']
    } else if rx.ends_with(b"\r") {
        vec![b'\r']
    } else {
        Vec::new()
    };
    Some(DetectResult {
        protocol_type: ProtocolType::SerialFrame,
        header: Vec::new(),
        footer,
        checksum_type: ChecksumType::None,
        data_format: DataFormat::U8,
        channel_count: 1,
        confidence: 0.8,
        description: crate::i18n::tf("AT command response — starts with {prefix}", &[
            ("{prefix}", "{prefix}", prefix),
        ]),
    })
}

/// 检测通用 ASCII 文本协议（90% 以上可打印 + 换行结尾）
fn try_detect_ascii_text(rx: &[u8]) -> Option<DetectResult> {
    if rx.is_empty() {
        return None;
    }
    let printable = rx
        .iter()
        .filter(|&&b| (0x20..=0x7E).contains(&b) || b == b'\r' || b == b'\n' || b == b'\t')
        .count();
    let ratio = printable as f32 / rx.len() as f32;
    if ratio < 0.9 {
        return None;
    }
    let ends_with_newline =
        rx.ends_with(b"\r\n") || rx.ends_with(b"\n") || rx.ends_with(b"\r");
    if !ends_with_newline {
        return None;
    }
    let footer = if rx.ends_with(b"\r\n") {
        vec![b'\r', b'\n']
    } else if rx.ends_with(b"\n") {
        vec![b'\n']
    } else {
        vec![b'\r']
    };
    Some(DetectResult {
        protocol_type: ProtocolType::SerialFrame,
        header: Vec::new(),
        footer,
        checksum_type: ChecksumType::None,
        data_format: DataFormat::U8,
        channel_count: 1,
        confidence: 0.5,
        description: crate::i18n::tf("Generic ASCII text protocol — printable ratio {ratio}%", &[
            ("{ratio}", "{ratio}", &format!("{:.0}", ratio * 100.0)),
        ]),
    })
}

/// 检测 Modbus RTU（从站ID/功能码匹配 + CRC16LE 校验）
fn try_detect_modbus_rtu(tx: &[u8], rx: &[u8]) -> Option<DetectResult> {
    // Modbus RTU 响应: [slave_id][func_code][data...][crc_lo][crc_hi]
    // 异常响应: [slave_id][func_code|0x80][exception_code][crc_lo][crc_hi]
    if rx.len() < 5 || tx.len() < 2 {
        return None;
    }
    let id_match = tx[0] == rx[0];
    let fc_ok = rx[1] == tx[1] || rx[1] == (tx[1] | 0x80);
    if !id_match || !fc_ok {
        return None;
    }
    // 验证 CRC16（最后 2 字节，小端序）
    let payload = &rx[..rx.len() - 2];
    let crc_stored = u16::from_le_bytes([rx[rx.len() - 2], rx[rx.len() - 1]]);
    let crc_calc = crc16_modbus(payload);
    if crc_stored != crc_calc {
        return None;
    }
    let is_exception = rx[1] & 0x80 != 0;
    // 读寄存器响应: [id][fc][byte_count][data...][crc]，每寄存器 2 字节
    let ch = if !is_exception && rx.len() >= 4 && rx[1] >= 0x03 && rx[1] <= 0x04 {
        (rx[2] as usize) / 2
    } else {
        1
    };
    Some(DetectResult {
        protocol_type: ProtocolType::ModbusRtu,
        header: Vec::new(),
        footer: Vec::new(),
        checksum_type: ChecksumType::Crc16,
        data_format: DataFormat::U16BE,
        channel_count: ch.max(1),
        confidence: 0.95,
        description: crate::i18n::tf(
            "Modbus RTU — CRC16 checksum OK, slave ID={slave}, function=0x{func:02X}{suffix}",
            &[
                ("{slave}", "{slave}", &format!("{}", rx[0])),
                ("{func:02X}", "{func:02X}", &format!("{:02X}", tx[1])),
                ("{suffix}", "{suffix}", if is_exception { crate::i18n::t(" (exception response)") } else { "" }),
            ],
        ),
    })
}

/// 检测自定义二进制帧（增强版）
/// - 扩展帧头列表：AA55/55AA/5AA5/A5A5/AABB/CC33/EB90/02/FF/7E/1002/01/A0/FAFB/59/53594E
/// - 帧尾：0D0A/0A/0D/03/55AA/AA55
/// - 校验：Sum8/XOR/CRC16LE/CRC16BE（每种都检查末尾1或2字节）
/// - 智能数据格式推断（检查 ADC 范围 0-4095、16位范围、F32 合理性）
fn try_detect_custom_frame(rx: &[u8]) -> Option<DetectResult> {
    if rx.len() < 3 {
        return None;
    }

    // ===== 扩展帧头列表 =====
    let common_headers: &[(&[u8], &str)] = &[
        (&[0xAA, 0x55], "AA 55"),
        (&[0x55, 0xAA], "55 AA"),
        (&[0x5A, 0xA5], "5A A5"),
        (&[0xA5, 0x5A], "A5 5A"),
        (&[0xAA, 0xBB], "AA BB"),
        (&[0xCC, 0x33], "CC 33"),
        (&[0xEB, 0x90], "EB 90"),
        (&[0x02], "STX(0x02)"),
        (&[0xFF], "FF"),
        (&[0x7E], "7E"),
        (&[0x10, 0x02], "10 02"),
        (&[0x01], "01"),
        (&[0xA0], "A0"),
        (&[0xFA, 0xFB], "FA FB"),
        (&[0x59], "59"),
        (&[0x53, 0x59, 0x4E], "SYN"),
    ];

    let mut detected_header: Vec<u8> = Vec::new();
    let mut header_name = String::new();
    for (hdr, name) in common_headers {
        if rx.starts_with(hdr) && rx.len() > hdr.len() {
            detected_header = hdr.to_vec();
            header_name = name.to_string();
            break;
        }
    }

    // ===== 帧尾 =====
    let common_footers: &[(&[u8], &str)] = &[
        (&[0x0D, 0x0A], "CR LF"),
        (&[0x0A], "LF"),
        (&[0x0D], "CR"),
        (&[0x03], "ETX(0x03)"),
        (&[0x55, 0xAA], "55 AA"),
        (&[0xAA, 0x55], "AA 55"),
    ];

    let mut detected_footer: Vec<u8> = Vec::new();
    for (ftr, _) in common_footers {
        if rx.len() > ftr.len() + detected_header.len() && rx.ends_with(ftr) {
            detected_footer = ftr.to_vec();
            break;
        }
    }

    // ===== 去掉帧头帧尾后的中间部分 =====
    let hdr_len = detected_header.len();
    let ftr_len = detected_footer.len();
    let mid_end = rx.len() - ftr_len;
    if mid_end <= hdr_len {
        // 帧头帧尾重叠或数据为空，无法识别
        return None;
    }
    let mid = &rx[hdr_len..mid_end];

    // ===== 检查多种校验 =====
    let mut detected_checksum = ChecksumType::None;
    let mut checksum_name = "None".to_string();
    let mut data_end = mid.len(); // 数据区结束位置（相对于 mid）

    // Sum8：最后 1 字节 = 前面所有字节求和（低 8 位）
    if mid.len() >= 2 {
        let sum: u8 = mid[..mid.len() - 1].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        if sum == mid[mid.len() - 1] {
            detected_checksum = ChecksumType::Sum8;
            checksum_name = "Sum8".to_string();
            data_end = mid.len() - 1;
        }
    }
    // XOR：最后 1 字节 = 前面所有字节异或
    if detected_checksum == ChecksumType::None && mid.len() >= 2 {
        let xor: u8 = mid[..mid.len() - 1].iter().fold(0u8, |a, &b| a ^ b);
        if xor == mid[mid.len() - 1] {
            detected_checksum = ChecksumType::Xor;
            checksum_name = "XOR".to_string();
            data_end = mid.len() - 1;
        }
    }
    // CRC16LE：最后 2 字节 = CRC16/MODBUS（小端序）
    if detected_checksum == ChecksumType::None && mid.len() >= 3 {
        let crc_stored = u16::from_le_bytes([mid[mid.len() - 2], mid[mid.len() - 1]]);
        let crc_calc = crc16_modbus(&mid[..mid.len() - 2]);
        if crc_stored == crc_calc {
            detected_checksum = ChecksumType::Crc16;
            checksum_name = "CRC16LE".to_string();
            data_end = mid.len() - 2;
        }
    }
    // CRC16BE：最后 2 字节 = CRC16/MODBUS（大端序）
    if detected_checksum == ChecksumType::None && mid.len() >= 3 {
        let crc_stored = u16::from_be_bytes([mid[mid.len() - 2], mid[mid.len() - 1]]);
        let crc_calc = crc16_modbus(&mid[..mid.len() - 2]);
        if crc_stored == crc_calc {
            detected_checksum = ChecksumType::Crc16;
            checksum_name = "CRC16BE".to_string();
            data_end = mid.len() - 2;
        }
    }
    // LRC：最后 1 字节 = -(前面所有字节求和) & 0xFF（用 Sum8 近似表示）
    if detected_checksum == ChecksumType::None && mid.len() >= 2 {
        let sum: u8 = mid[..mid.len() - 1].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        let lrc = 0u8.wrapping_sub(sum);
        if lrc == mid[mid.len() - 1] {
            detected_checksum = ChecksumType::Sum8;
            checksum_name = "LRC".to_string();
            data_end = mid.len() - 1;
        }
    }

    // ===== 智能数据格式推断 =====
    let data = &mid[..data_end];
    let (data_format, channel_count, format_name) = infer_data_format(data);

    // ===== 计算置信度 =====
    let mut confidence: f32 = 0.3;
    if !detected_header.is_empty() {
        confidence += 0.2;
    }
    if !detected_footer.is_empty() {
        confidence += 0.15;
    }
    if detected_checksum != ChecksumType::None {
        confidence += 0.25;
    }
    confidence = confidence.min(1.0);

    // ===== 构建描述 =====
    let mut desc_parts: Vec<String> = Vec::new();
    if !detected_header.is_empty() {
        desc_parts.push(crate::i18n::tf("Header: {header}", &[
            ("{header}", "{header}", &header_name),
        ]));
    } else {
        desc_parts.push(crate::i18n::t("No fixed header").to_string());
    }
    if !detected_footer.is_empty() {
        desc_parts.push(crate::i18n::tf(
            "Footer: {footer}",
            &[("{footer}", "{footer}", &detected_footer
                .iter()
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" "))],
        ));
    }
    if detected_checksum != ChecksumType::None {
        desc_parts.push(crate::i18n::tf("Checksum: {checksum} (verified)", &[
            ("{checksum}", "{checksum}", &checksum_name),
        ]));
    } else {
        desc_parts.push(crate::i18n::t("No checksum or unrecognized").to_string());
    }
    desc_parts.push(crate::i18n::tf("Data: {bytes} bytes → {format} × {channels} channels", &[
        ("{bytes}", "{bytes}", &format!("{}", data_end)),
        ("{format}", "{format}", &format_name),
        ("{channels}", "{channels}", &format!("{}", channel_count)),
    ]));

    Some(DetectResult {
        protocol_type: ProtocolType::SerialFrame,
        header: detected_header,
        footer: detected_footer,
        checksum_type: detected_checksum,
        data_format,
        channel_count: channel_count.max(1),
        confidence,
        description: desc_parts.join(crate::i18n::t(", ")),
    })
}

/// 智能推断数据格式和通道数
/// - 2字节对齐：检查 u16 LE/BE 值是否在 0-4095（12位ADC）或 0-65535 范围
/// - 4字节对齐：检查 f32 LE 值是否为有限数且 |v| < 1e6
/// 返回 (DataFormat, channel_count, format_description)
fn infer_data_format(data: &[u8]) -> (DataFormat, usize, String) {
    if data.is_empty() {
        return (DataFormat::U8, 0, crate::i18n::t("No data").to_string());
    }
    // 4 字节对齐：可能是 F32 或 U32
    if data.len() % 4 == 0 && data.len() >= 4 {
        // 检查 F32 LE 合理性：所有 4 字节块均为有限数且 |v| < 1e6
        let all_finite_le = data.chunks_exact(4).all(|c| {
            let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            f.is_finite() && f.abs() < 1e6
        });
        if all_finite_le {
            return (DataFormat::F32LE, data.len() / 4, "F32LE".to_string());
        }
        // 检查 F32 BE 合理性
        let all_finite_be = data.chunks_exact(4).all(|c| {
            let f = f32::from_be_bytes([c[0], c[1], c[2], c[3]]);
            f.is_finite() && f.abs() < 1e6
        });
        if all_finite_be {
            return (DataFormat::F32BE, data.len() / 4, "F32BE".to_string());
        }
    }
    // 2 字节对齐：检查 u16 范围
    if data.len() % 2 == 0 && data.len() >= 2 {
        // 检查 U16 LE 是否在 12 位 ADC 范围 (0-4095) 内
        let all_adc_le = data.chunks_exact(2).all(|c| {
            let v = u16::from_le_bytes([c[0], c[1]]);
            v <= 4095
        });
        if all_adc_le {
            return (DataFormat::U16LE, data.len() / 2, "U16LE(ADC)".to_string());
        }
        // 检查 U16 BE 是否在 12 位 ADC 范围内
        let all_adc_be = data.chunks_exact(2).all(|c| {
            let v = u16::from_be_bytes([c[0], c[1]]);
            v <= 4095
        });
        if all_adc_be {
            return (DataFormat::U16BE, data.len() / 2, "U16BE(ADC)".to_string());
        }
        // 默认 U16 LE（传感器最常用格式）
        return (DataFormat::U16LE, data.len() / 2, "U16LE".to_string());
    }
    // 奇数字节：U8
    (DataFormat::U8, data.len(), "U8".to_string())
}

/// 判断字节在帧中的角色
fn byte_role(index: usize, frame: &[u8], ff: &FrameFormat) -> ByteRole {
    let header_len = if ff.header.is_empty() { 0 } else { ff.header.len() };
    let footer_len = if ff.footer.is_empty() { 0 } else { ff.footer.len() };
    let cs_size = checksum_size(&ff.checksum_type);
    let data_size = data_format_size(&ff.data_format);
    let total_data = ff.channel_count * data_size;
    let frame_len = frame.len();

    // 帧头
    if index < header_len {
        return ByteRole::Header;
    }

    // 帧尾
    if footer_len > 0 && index >= frame_len.saturating_sub(footer_len) {
        return ByteRole::Footer;
    }

    // 校验字段（位于帧尾之前）
    if cs_size > 0
        && index >= frame_len.saturating_sub(footer_len + cs_size)
        && index < frame_len.saturating_sub(footer_len)
    {
        return ByteRole::Checksum;
    }

    // 长度字段
    if let Some(len_off) = ff.length_offset {
        let len_start = header_len + len_off;
        let len_end = len_start + ff.length_bytes;
        if index >= len_start && index < len_end {
            return ByteRole::Length;
        }
    }

    // 数据区
    let data_start = header_len + ff.data_offset;
    let data_end = data_start + total_data;
    if index >= data_start && index < data_end {
        return ByteRole::Data;
    }

    ByteRole::Other
}

/// 角色对应的颜色
fn role_color(role: ByteRole) -> Color32 {
    match role {
        ByteRole::Header => theme::ACCENT,
        ByteRole::Footer => theme::CYAN,
        ByteRole::Length => theme::WARNING,
        ByteRole::Data => theme::SUCCESS,
        ByteRole::Checksum => theme::ERROR,
        ByteRole::Other => Color32::from_rgb(0x60, 0x60, 0x60),
    }
}

/// 角色对应的标签
fn role_label(role: ByteRole, _zh: bool) -> &'static str {
    match role {
        ByteRole::Header => crate::i18n::t("Header"),
        ByteRole::Footer => crate::i18n::t("Footer"),
        ByteRole::Length => crate::i18n::t("Length"),
        ByteRole::Data => crate::i18n::t("Data"),
        ByteRole::Checksum => crate::i18n::t("Checksum"),
        ByteRole::Other => { crate::i18n::t("Other") },
    }
}

/// 执行发送命令（在状态借用外调用）
fn do_send(state: &mut AppState, _zh: bool, s: &mut ProtocolAnalyzerState) {
    // 1. 读取发送参数
    let (bytes, wait_ms) = {
        let bytes = parse_hex(&s.command_input);
        let wait_ms = s.wait_ms;
        (bytes, wait_ms)
    };

    let bytes = match bytes {
        Ok(b) if !b.is_empty() => b,
        Ok(_) => {
            state.last_error =
                Some(crate::i18n::t("Empty command").to_string());
            return;
        }
        Err(e) => {
            state.last_error = Some(format!(
                "{}: {}",
                crate::i18n::t("Parse error"),
                e
            ));
            return;
        }
    };

    let now = chrono::Local::now()
        .format("%H:%M:%S%.3f")
        .to_string();

    // 2. 发送并读取响应
    match state.send_and_read(&bytes, wait_ms) {
        Ok(resp) => {
            // 3. 更新状态
            // 日志：TX + RX
            s.log.push(LogEntry {
                timestamp: now.clone(),
                direction: "TX",
                data: bytes.clone(),
            });
            s.log.push(LogEntry {
                timestamp: now,
                direction: "RX",
                data: resp.clone(),
            });
            // 限制日志大小
            if s.log.len() > 200 {
                let drop = s.log.len() - 200;
                s.log.drain(0..drop);
            }
            // 存储响应
            s.last_response = resp.clone();
            // 自动识别协议
            if s.auto_detect {
                s.detect_result = auto_detect_protocol(&bytes, &resp);
            }
            // 解析响应帧
            let config = build_config(s);
            let parser = ProtocolParser::new(config);
            s.last_parsed = match parser.parse_frame(&resp) {
                Ok(p) => Some(p),
                Err(e) => Some(ParsedData {
                    channel_values: Vec::new(),
                    raw_bytes: resp,
                    timestamp: 0.0,
                    valid: false,
                    error: Some(e),
                }),
            };
            state.last_error = None;
        }
        Err(e) => {
            state.last_error = Some(e);
        }
    }
}

/// 渲染芯片方案识别区
fn render_chip_detect_section(
    ui: &mut egui::Ui,
    state: &AppState,
    _zh: bool,
    connected: bool,
    cd: &mut ChipDetectState,
    #[cfg(feature = "ollama")] ollama: &mut OllamaChipState,
) {
    // feature 关闭时参数未使用
    #[cfg(not(feature = "ollama"))]
    let _ = (state, connected);
    ui.label(
        RichText::new(crate::i18n::t("Chip Identification"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("USB bridge chip identified by VID/PID; target MCU identified by bootloader protocol probing"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);

    // ===== USB 桥接芯片识别（通过 VID/PID，无需连接） =====
    if let Some(usb) = &cd.usb_bridge {
        ui.label(
            RichText::new(crate::i18n::t("USB Bridge Chip"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);

        let (border_color, bg_color, type_label) = if usb.is_native_mcu {
            (
                theme::SUCCESS,
                Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80),
                crate::i18n::t("MCU Native USB"),
            )
        } else {
            (
                theme::ACCENT,
                Color32::from_rgba_premultiplied(0x18, 0x24, 0x10, 0x80),
                crate::i18n::t("USB-Serial Bridge"),
            )
        };
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.5_f32, border_color))
            .fill(bg_color)
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&usb.chip_name)
                            .color(border_color)
                            .strong()
                            .size(15.0),
                    );
                    ui.label(
                        RichText::new(format!("({})", type_label))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                });
                ui.label(
                    RichText::new(format!(
                        "{}: {}  |  VID: {:04X}  PID: {:04X}",
                        crate::i18n::t("Vendor"),
                        usb.vendor,
                        usb.vid,
                        usb.pid
                    ))
                    .color(theme::TEXT_PRIMARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
                if let Some(desc) = &usb.description {
                    if !desc.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "{}: {}",
                                crate::i18n::t("USB Desc"),
                                crate::i18n::t_owned(desc)
                            ))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                        );
                    }
                }
            });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
    } else {
        ui.label(
            RichText::new(crate::i18n::t("USB Bridge: not identified (select a port in Connection panel)"))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
        ui.add_space(4.0);
    }

    // ===== Ollama 本地模型识别（feature gate，Phase 3b）=====
    #[cfg(feature = "ollama")]
    {
        // 状态行 + 手动识别按钮
        ui.horizontal(|ui| {
            let (dot, color, status_text) = match &ollama.status {
                Some(OllamaStatus::Ready(_)) => (
                    "🟢",
                    theme::SUCCESS,
                    crate::i18n::t("Local model ready"),
                ),
                Some(OllamaStatus::ModelMissing(_)) => (
                    "🟡",
                    theme::WARNING,
                    crate::i18n::t("Model missing"),
                ),
                Some(OllamaStatus::NotRunning) => (
                    "⚪",
                    theme::TEXT_DISABLED,
                    crate::i18n::t("Not running"),
                ),
                None => (
                    "⚪",
                    theme::TEXT_DISABLED,
                    crate::i18n::t("Detecting…"),
                ),
            };
            ui.label(RichText::new(format!("{} {}", dot, status_text)).color(color).small())
                .on_hover_text(&ollama.config.base_url);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = matches!(ollama.status, Some(OllamaStatus::Ready(_)));
                let btn_text = if ollama.busy {
                    crate::i18n::t("Identifying…")
                } else { crate::i18n::t("🧠 Local Identify") };
                if ui
                    .add_enabled(ready && !ollama.busy, egui::Button::new(btn_text).small())
                    .on_hover_text(crate::i18n::t("Identify chip with local Ollama model (fully offline)"))
                    .clicked()
                {
                    // 手动触发：按当前上下文选择回写目标
                    let task = if cd.usb_bridge.is_none() {
                        OllamaChipTask::VidPid
                    } else {
                        OllamaChipTask::McuProbe
                    };
                    let ev = build_chip_evidence(state, cd);
                    trigger_chip_identify(ollama, ev, task);
                }
            });
        });

        // 结果卡片
        if let Some(r) = &ollama.result {
            ui.add_space(4.0);
            let border = if ollama.applied {
                theme::SUCCESS
            } else {
                theme::WARNING
            };
            egui::Frame::group(ui.style())
                .fill(theme::BG_ELEVATED)
                .stroke(egui::Stroke::new(1.0_f32, border))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🧠").size(13.0));
                        ui.label(
                            RichText::new(crate::i18n::t("Local Model"))
                                .color(theme::ACCENT)
                                .strong()
                                .small(),
                        );
                        ui.label(
                            RichText::new(if ollama.applied {
                                crate::i18n::t("(applied)")
                            } else {
                                crate::i18n::t("(not applied)")
                            })
                            .color(theme::TEXT_DISABLED)
                            .small(),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&r.chip).color(border).strong().size(15.0));
                        ui.label(
                            RichText::new(format!("({})", r.vendor))
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );
                        let conf_color = if r.confidence >= 0.7 {
                            theme::SUCCESS
                        } else if r.confidence >= 0.4 {
                            theme::WARNING
                        } else {
                            theme::ERROR
                        };
                        ui.label(
                            RichText::new(format!("{:.0}%", r.confidence * 100.0))
                                .color(conf_color)
                                .strong()
                                .small(),
                        );
                        ui.label(
                            RichText::new(format!("family: {}", r.family))
                                .color(theme::TEXT_DISABLED)
                                .small(),
                        );
                    });
                    if let Some(driver) = &r.suggested_driver {
                        ui.label(
                            RichText::new(crate::i18n::tf("💡 Suggested driver: {}", &[("{}", "{}", &format!("{}", driver))]))
                            .color(theme::CYAN)
                            .small(),
                        );
                    }
                    ui.label(
                        RichText::new(&r.reasoning)
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t("Source: Ollama local model (advisory, please verify)"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                    );
                });
        }
        ui.add_space(4.0);
    }

    // ===== 目标 MCU 协议探测 =====
    ui.label(
        RichText::new(crate::i18n::t("Target MCU Protocol Probing"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);

    // 未连接提示
    if !connected {
        ui.horizontal(|ui| {
            ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
            ui.label(
                RichText::new(crate::i18n::t("Not connected. Open a port in the Connection panel first."))
                .color(theme::WARNING)
                .small(),
            );
        });
        return;
    }

    // 开始/停止按钮
    ui.horizontal(|ui| {
        if cd.running {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("⏹ Stop##panels/sensor_dev/protocol_analyzer.rs:113897"))
                    .color(egui::Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ERROR)
            .min_size(egui::Vec2::new(120.0, 28.0));
            if ui.add(btn).clicked() {
                cd.running = false;
            }
        } else {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("▶ Start Chip ID"))
                .color(egui::Color32::BLACK)
                .strong(),
            )
            .fill(theme::SUCCESS)
            .min_size(egui::Vec2::new(140.0, 28.0));
            if ui
                .add(btn)
                .on_hover_text(crate::i18n::t("Probe the target MCU model via Bootloader/firmware protocol. On success, jump to the corresponding flasher panel with chip type auto-selected"))
                .clicked() {
                cd.running = true;
                cd.probe_index = 0;
                cd.results.clear();
                cd.best_result = None;
                cd.wait_until = None;
            }
        }

        // 进度显示
        if !cd.results.is_empty() || cd.running {
            let progress = if cd.running {
                crate::i18n::tf("{index}/{total} — {probe}", &[
                    ("{index}", "{index}", &format!("{}", cd.probe_index + 1)),
                    ("{total}", "{total}", &format!("{}", CHIP_PROBES.len())),
                    ("{probe}", "{probe}", if cd.probe_index < CHIP_PROBES.len() {
                        crate::i18n::t(CHIP_PROBES[cd.probe_index].0)
                    } else {
                        ""
                    }),
                ])
            } else {
                crate::i18n::tf("Done ({done}/{total})", &[
                    ("{done}", "{done}", &format!("{}", cd.results.len())),
                    ("{total}", "{total}", &format!("{}", CHIP_PROBES.len())),
                ])
            };
            ui.label(
                RichText::new(crate::i18n::tf("Progress: {}##panels/sensor_dev/protocol_analyzer.rs:115632", &[("{}", "{}", &format!("{}", progress))]))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        }
    });

    // 进度条
    if cd.running || !cd.results.is_empty() {
        ui.add_space(4.0);
        let progress = cd.results.len() as f32 / CHIP_PROBES.len() as f32;
        ui.add(egui::ProgressBar::new(progress).desired_width(200.0));
    }

    // 最佳识别结果（高亮显示）
    if let Some(best) = &cd.best_result {
        ui.add_space(6.0);
        let (border_color, bg_color) = if best.confidence >= 0.8 {
            (theme::SUCCESS, Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
        } else if best.confidence >= 0.5 {
            (theme::WARNING, Color32::from_rgba_premultiplied(0x30, 0x28, 0x10, 0x80))
        } else {
            (theme::ERROR, Color32::from_rgba_premultiplied(0x30, 0x10, 0x10, 0x80))
        };
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(2.0_f32, border_color))
            .fill(bg_color)
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Chip:"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(&best.chip_family)
                            .color(border_color)
                            .strong()
                            .size(16.0),
                    );
                    ui.label(
                        RichText::new(format!("({:.0}%)", best.confidence * 100.0))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                });
                ui.label(
                    RichText::new(&best.chip_description)
                        .color(theme::TEXT_PRIMARY)
                        .small(),
                );
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        crate::i18n::t("Evidence"),
                        crate::i18n::t_owned(&best.evidence)
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
            });
    }

    // 探测日志
    if !cd.results.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(crate::i18n::t("Probe Log"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);

        egui::Frame::none()
            .fill(theme::BG_DEEP)
            .inner_margin(egui::Margin::same(6.0))
            .rounding(4.0)
            .show(ui, |ui| {
                for (i, r) in cd.results.iter().enumerate() {
                    let color = if r.note.contains("✓") {
                        theme::SUCCESS
                    } else if r.note.contains(crate::i18n::t("Suspected")) {
                        theme::WARNING
                    } else if r.rx.is_empty() {
                        theme::TEXT_DISABLED
                    } else {
                        theme::TEXT_SECONDARY
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{}.", i + 1))
                                .color(theme::TEXT_DISABLED)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t_owned(&r.name))
                                .color(theme::TEXT_PRIMARY)
                                .small(),
                        );
                        ui.label(
                            RichText::new(&r.note)
                                .color(color)
                                .small()
                                .strong(),
                        );
                    });

                    // TX/RX HEX 简要
                    let tx_hex: String = r
                        .tx
                        .iter()
                        .take(16)
                        .map(|b| format!("{:02X}", b))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let tx_more = if r.tx.len() > 16 { "..." } else { "" };
                    ui.label(
                        RichText::new(format!("  TX: {}{}", tx_hex, tx_more))
                            .color(theme::ACCENT_DIM)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                    if !r.rx.is_empty() {
                        let rx_hex: String = r
                            .rx
                            .iter()
                            .take(16)
                            .map(|b| format!("{:02X}", b))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let rx_more = if r.rx.len() > 16 { "..." } else { "" };
                        ui.label(
                            RichText::new(format!("  RX: {}{}", rx_hex, rx_more))
                                .color(theme::CYAN)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                    }
                    ui.add_space(2.0);
                }
            });
    }
}

// ===== 深度总线探测辅助函数 =====

/// 构建 Modbus RTU 读保持寄存器命令帧
/// [slave_id][0x03][addr_hi][addr_lo][count_hi][count_lo][crc_lo][crc_hi]
fn build_modbus_read_holding(slave_id: u8, addr: u16, count: u16) -> Vec<u8> {
    let mut frame = vec![
        slave_id,
        0x03,
        (addr >> 8) as u8,
        (addr & 0xFF) as u8,
        (count >> 8) as u8,
        (count & 0xFF) as u8,
    ];
    let crc = crc16_modbus(&frame);
    frame.extend_from_slice(&crc.to_le_bytes());
    frame
}

/// 验证 Modbus RTU 响应：CRC 校验 + 从站 ID / 功能码匹配
fn validate_modbus_response(rx: &[u8], slave_id: u8, func_code: u8) -> bool {
    if rx.len() < 5 {
        return false;
    }
    if rx[0] != slave_id {
        return false;
    }
    // 正常响应功能码匹配，或异常响应（功能码 | 0x80）
    if rx[1] != func_code && rx[1] != (func_code | 0x80) {
        return false;
    }
    let payload = &rx[..rx.len() - 2];
    let crc_stored = u16::from_le_bytes([rx[rx.len() - 2], rx[rx.len() - 1]]);
    let crc_calc = crc16_modbus(payload);
    crc_stored == crc_calc
}

/// 解析 Modbus RTU 读寄存器响应中的寄存器值
fn parse_modbus_registers(rx: &[u8]) -> Vec<u16> {
    if rx.len() < 5 {
        return Vec::new();
    }
    // [slave_id][func_code][byte_count][data...][crc_lo][crc_hi]
    let byte_count = rx[2] as usize;
    let data_end = 3 + byte_count;
    if data_end + 2 > rx.len() {
        return Vec::new();
    }
    rx[3..data_end]
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect()
}

/// I2C 设备分类标签
fn i2c_category_label(cat: bus_probe::I2cDeviceCategory, _zh: bool) -> &'static str {
    match cat {
        bus_probe::I2cDeviceCategory::Sensor => crate::i18n::t("Sensor"),
        bus_probe::I2cDeviceCategory::Display => crate::i18n::t("Display"),
        bus_probe::I2cDeviceCategory::Eeprom => "EEPROM",
        bus_probe::I2cDeviceCategory::IoExpander => crate::i18n::t("IO Expander"),
        bus_probe::I2cDeviceCategory::Adc => "ADC",
        bus_probe::I2cDeviceCategory::Rtc => "RTC",
        bus_probe::I2cDeviceCategory::Imu => "IMU",
        bus_probe::I2cDeviceCategory::Other => { crate::i18n::t("Other") },
    }
}

/// 解析 JEDEC ID 输入（格式: "MFG CAP"，如 "EF 4017"）
fn parse_jedec_input(input: &str) -> Option<(u8, u16)> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }
    let strip = |s: &str| -> String {
        s.trim_start_matches("0x").trim_start_matches("0X").to_string()
    };
    let mfg = u8::from_str_radix(&strip(parts[0]), 16).ok()?;
    let cap = u16::from_str_radix(&strip(parts[1]), 16).ok()?;
    Some((mfg, cap))
}

/// 解析 u32 hex 输入
fn parse_u32_hex(input: &str) -> Option<u32> {
    let s = input.trim_start_matches("0x").trim_start_matches("0X").trim();
    u32::from_str_radix(s, 16).ok()
}

/// 解析 u8 hex 输入
fn parse_u8_hex(input: &str) -> Option<u8> {
    let s = input.trim_start_matches("0x").trim_start_matches("0X").trim();
    u8::from_str_radix(s, 16).ok()
}

/// 收集 rx_entry_seq 序号之后的 RX 数据。
/// 从尾部按序号差取值，免疫 rx_buffer 满后从头部 drain 导致的索引漂移。
fn collect_rx_since(state: &AppState, since_seq: u64) -> Vec<u8> {
    let new_count = state.rx_entry_seq.saturating_sub(since_seq) as usize;
    let mut segments: Vec<&[u8]> = Vec::new();
    for entry in state.rx_buffer.iter().rev().take(new_count) {
        if entry.direction == Direction::Rx {
            segments.push(&entry.data);
        }
    }
    segments.reverse();
    segments.concat()
}

/// 帧驱动深度总线探测状态机
/// 在 show() 中每帧调用，需要 &mut AppState
fn update_deep_probe(state: &mut AppState, dp: &mut DeepProbeState) {
        if !dp.running {
            return;
        }

        match dp.scan_type {
            DeepScanType::ModbusSlave => {
                // Modbus 从站扫描：逐个地址发送读保持寄存器命令（FC 0x03）
                if dp.wait_until.is_none() {
                    // 检查是否完成
                    if dp.progress > dp.modbus_end as usize {
                        dp.running = false;
                        let start = dp.modbus_start;
                        let end = dp.modbus_end;
                        let found = dp.results.len();
                        dp.log.push(format!(
                            "{} ({}-{}){} {} {}",
                            { crate::i18n::t("✓ Slave scan done") },
                            start,
                            end,
                            { crate::i18n::t(", found") },
                            found,
                            { crate::i18n::t("slaves") },
                        ));
                        return;
                    }
                    let slave_id = dp.progress as u8;
                    let frame = build_modbus_read_holding(slave_id, 0x0000, 1);
                    dp.rx_start_seq = state.rx_entry_seq;
                    let _ = state.send_raw(&frame);
                    dp.wait_until = Some(
                        std::time::Instant::now() + std::time::Duration::from_millis(200),
                    );
                } else {
                    let until = dp.wait_until.unwrap();
                    if std::time::Instant::now() >= until {
                        let rx = collect_rx_since(state, dp.rx_start_seq);
                        let slave_id = dp.progress as u8;
                        if !rx.is_empty() && validate_modbus_response(&rx, slave_id, 0x03) {
                            let regs = parse_modbus_registers(&rx);
                            let regs_str = if regs.is_empty() {
                                "—".to_string()
                            } else {
                                regs.iter()
                                    .map(|v| format!("0x{:04X}", v))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            dp.results.push(bus_probe::DeepProbeResult {
                                probe_type: bus_probe::ProbeType::ModbusScan,
                                name: crate::i18n::tf("Modbus slave #{id}", &[
                                    ("{id}", "{id}", &format!("{}", slave_id)),
                                ]),
                                description: format!(
                                    "{} {} {}",
                                    { crate::i18n::t("Slave##panels/sensor_dev/protocol_analyzer.rs:127543") },
                                    slave_id,
                                    { crate::i18n::t("online") },
                                ),
                                confidence: 0.95,
                                details: format!(
                                    "{}: {}",
                                    { crate::i18n::t("Registers") },
                                    regs_str
                                ),
                                raw_data: rx,
                            });
                            dp.log.push(format!("✓ {} #{} {}", { crate::i18n::t("Slave##panels/sensor_dev/protocol_analyzer.rs:128205") }, slave_id, { crate::i18n::t("responded") }));
                        } else if rx.is_empty() {
                            if slave_id % 20 == 0 || slave_id == dp.modbus_end {
                                dp.log.push(format!("· {} #{} {}", { crate::i18n::t("Slave##panels/sensor_dev/protocol_analyzer.rs:128503") }, slave_id, { crate::i18n::t("no response") }));
                            }
                        } else {
                            dp.log.push(format!("✗ {} #{} {}", { crate::i18n::t("Slave##panels/sensor_dev/protocol_analyzer.rs:128730") }, slave_id, { crate::i18n::t("CRC fail") }));
                        }
                        dp.progress += 1;
                        dp.wait_until = None;
                    }
                }
            }
            DeepScanType::ModbusRegister => {
                // Modbus 寄存器扫描：逐个地址读取保持寄存器
                if dp.wait_until.is_none() {
                    if dp.progress > dp.modbus_reg_end as usize {
                        dp.running = false;
                        let found = dp.results.iter().filter(|r| r.confidence > 0.5).count();
                        let reg_start = dp.modbus_reg_start;
                        let reg_end = dp.modbus_reg_end;
                        dp.log.push(format!(
                            "{} (0x{:04X}-0x{:04X}){} {} {}",
                            { crate::i18n::t("✓ Register scan done") },
                            reg_start,
                            reg_end,
                            { crate::i18n::t(", ") },
                            found,
                            { crate::i18n::t("readable") },
                        ));
                        return;
                    }
                    let addr = dp.progress as u16;
                    let frame = build_modbus_read_holding(dp.modbus_slave_id, addr, 1);
                    dp.rx_start_seq = state.rx_entry_seq;
                    let _ = state.send_raw(&frame);
                    dp.wait_until = Some(
                        std::time::Instant::now() + std::time::Duration::from_millis(200),
                    );
                } else {
                    let until = dp.wait_until.unwrap();
                    if std::time::Instant::now() >= until {
                        let rx = collect_rx_since(state, dp.rx_start_seq);
                        let addr = dp.progress as u16;
                        let slave_id = dp.modbus_slave_id;
                        if !rx.is_empty() && validate_modbus_response(&rx, slave_id, 0x03) {
                            let is_exception = rx.len() >= 2 && rx[1] & 0x80 != 0;
                            if !is_exception {
                                let regs = parse_modbus_registers(&rx);
                                let val = regs.first().copied();
                                dp.results.push(bus_probe::DeepProbeResult {
                                    probe_type: bus_probe::ProbeType::ModbusScan,
                                    name: format!("{} 0x{:04X}", { crate::i18n::t("Reg") }, addr),
                                    description: format!(
                                        "{} {} = {}",
                                        { crate::i18n::t("Addr") },
                                        addr,
                                        val.map(|v| format!("0x{:04X} ({})", v, v))
                                            .unwrap_or_else(|| { crate::i18n::t("—").to_string() })
                                    ),
                                    confidence: 0.9,
                                    details: format!(
                                        "{} #{} {} {} = {:?}",
                                        { crate::i18n::t("Slave##panels/sensor_dev/protocol_analyzer.rs:132048") },
                                        slave_id,
                                        { crate::i18n::t("reg") },
                                        addr,
                                        val
                                    ),
                                    raw_data: rx,
                                });
                                dp.log.push(format!(
                                    "✓ {} {} = {}",
                                    { crate::i18n::t("Reg") },
                                    addr,
                                    val.map(|v| format!("0x{:04X}", v)).unwrap_or_else(|| "—".to_string())
                                ));
                            } else {
                                dp.results.push(bus_probe::DeepProbeResult {
                                    probe_type: bus_probe::ProbeType::ModbusScan,
                                    name: format!("{} 0x{:04X} ({})", { crate::i18n::t("Reg") }, addr, { crate::i18n::t("err") }),
                                    description: format!("{} {} {}", { crate::i18n::t("Addr") }, addr, { crate::i18n::t("exception") }),
                                    confidence: 0.3,
                                    details: { crate::i18n::t("Modbus exception").to_string() },
                                    raw_data: Vec::new(),
                                });
                                dp.log.push(format!("✗ {} {} {}", { crate::i18n::t("Reg") }, addr, { crate::i18n::t("exception##panels/sensor_dev/protocol_analyzer.rs:133706") }));
                            }
                        } else if rx.is_empty() {
                            if addr % 10 == 0 {
                                dp.log.push(format!("· {} {} {}", { crate::i18n::t("Reg") }, addr, { crate::i18n::t("no response") }));
                            }
                        }
                        dp.progress += 1;
                        dp.wait_until = None;
                    }
                }
            }
            DeepScanType::UartAnalyze => {
                // UART 协议分析：读取 rx_buffer 最新 256 字节，一次性分析
                let rx_data: Vec<u8> = {
                    let mut all: Vec<u8> = Vec::new();
                    for entry in state.rx_buffer.iter() {
                        if entry.direction == Direction::Rx {
                            all.extend_from_slice(&entry.data);
                        }
                    }
                    let start = all.len().saturating_sub(256);
                    all[start..].to_vec()
                };
                if rx_data.is_empty() {
                    dp.log.push(crate::i18n::t("✗ No RX data").to_string());
                } else {
                    let results = bus_probe::analyze_uart_data(&rx_data);
                    dp.log.push(format!(
                        "{} {} {} {} {}",
                        { crate::i18n::t("Analyzed") },
                        rx_data.len(),
                        { crate::i18n::t("bytes,") },
                        results.len(),
                        { crate::i18n::t("protocols") },
                    ));
                    if results.is_empty() {
                        dp.log.push(crate::i18n::t("  → No known protocol matched").to_string());
                    } else {
                        for r in results {
                            dp.log.push(format!(
                                "  → {} ({:.0}%): {}",
                                crate::i18n::t_owned(&r.protocol_name),
                                r.confidence * 100.0,
                                crate::i18n::t_owned(&r.evidence)
                            ));
                            dp.results.push(bus_probe::DeepProbeResult {
                                probe_type: bus_probe::ProbeType::UartProbe,
                                name: r.protocol_name,
                                description: r.description,
                                confidence: r.confidence,
                                details: r.evidence,
                                raw_data: rx_data.clone(),
                            });
                        }
                    }
                }
                dp.running = false;
            }
            _ => {
                // 参考表模式不执行扫描
                dp.running = false;
            }
        }
}

// ===== MCU Boot ROM 探测状态机 =====

/// 启动 MCU Boot 探测
///
/// 在检测到鞋垫/床垫协议后自动触发，或用户手动启动。
/// 依次发送各厂商 Boot ROM 同步字节（STM32/ESP32/CH32/GD32/ESP8266/STM8/LPC/Arduino），
/// 从 rx_buffer 中提取应答数据校验，识别主控 MCU 芯片类型。
fn start_mcu_probe(triggered_by_insole: bool, s: &mut McuProbeState) {
    s.running = true;
    s.probe_index = 0;
    s.sent = false;
    s.wait_until = None;
    s.rx_start_seq = 0;
    s.results.clear();
    s.log.clear();
    s.triggered_by_insole = triggered_by_insole;
    if triggered_by_insole {
        s.auto_triggered = true;
    }
    // 重置 DTR/RTS 预触发状态（允许新一轮触发）
    s.pre_trigger_phase = 0;
    s.pre_trigger_until = None;
    s.dtr_rts_tried = false;
    // usb_bridge 由外部同步，不在此重置
    // 清空被动分析数据
    s.frame_intervals_ms.clear();
    s.passive_result = None;
    s.log.push(crate::i18n::t("report.protocol.section.mcu").to_string());
    s.log.push(crate::i18n::tf("Will try {count} Boot ROM sync bytes in sequence", &[
        ("{count}", "{count}", &format!("{}", bus_probe::McuBootProbeKind::PROBE_SEQUENCE.len())),
    ]));
    s.log.push(crate::i18n::t("Tip: if the device is not in Boot mode, all probes may get no response").to_string());
    s.log.push(crate::i18n::t("Tip: DTR/RTS boot-mode triggering will be attempted automatically (hardware support required)").to_string());
}

/// 帧驱动 MCU Boot 探测状态机
///
/// 工作流程（非阻塞，每帧执行一次）：
/// 1. DTR/RTS 预触发阶段（首次执行时）：
///    - Phase 1: 设置 BOOT0=高 + RESET=低，等待 10ms
///    - Phase 2: 释放 RESET（拉高），等待 100ms 让设备进入 Boot
///    - Phase 3: 进入正常发送流程
/// 2. 发送阶段：清空串口输入缓冲区 → 记录 rx_buffer 位置 → 发送同步字节 → 设置超时
/// 3. 等待阶段：检查是否超时（不读取数据，让 poll_rx 自然积累 RX 到 rx_buffer）
/// 4. 超时后：从 rx_buffer 提取发送后的 RX 数据 → 校验 → 推进到下一条
/// 5. 全部完成：输出汇总结果；若全部失败，自动执行被动协议特征分析
fn update_mcu_probe(state: &mut AppState, s: &mut McuProbeState, dp: &mut DeepProbeState, usb_bridge: Option<&UsbBridgeInfo>) {
    use miconlab_core::port::ClearBuffer;
    use std::time::{Duration, Instant};

    // === 自动触发：深度探测检测到鞋垫协议后自动启动 MCU 识别（仅设备工作区构建） ===
    #[cfg(feature = "device-workspaces")]
    if !s.running && !s.auto_triggered && state.port.is_connected() {
        let has_insole = !dp.running
            && dp
                .results
                .iter()
                .any(|r| r.name == bus_probe::INSOLE_PROTOCOL_NAME);
        if has_insole {
            start_mcu_probe(true, s);
        }
    }

    // 主状态机使用 labeled block 以保留原 closure 内 return 的语义
    // （原 MCU_PROBE.with 闭包内的 return 仅退出闭包，闭包后继续执行被动分析）
    'main: {
        if !s.running {
            break 'main;
        }

        let sequence = bus_probe::McuBootProbeKind::PROBE_SEQUENCE;

        // ===== 检查是否已完成所有探测 =====
        if s.probe_index >= sequence.len() {
            s.running = false;
            let hit_count = s.results.iter().filter(|r| r.confidence >= 0.7).count();
            if hit_count > 0 {
                s.log.push(crate::i18n::tf("✓ Identification complete, {count} chip type(s) matched", &[
                    ("{count}", "{count}", &format!("{}", hit_count)),
                ]));
            } else {
                s.log.push(crate::i18n::t("✗ No response to any Boot probe").to_string());
                s.log.push(crate::i18n::t("  → The device may not be in Boot mode, or DTR/RTS is not wired to BOOT0/RESET").to_string());
                s.log.push(crate::i18n::t("  → Starting passive protocol signature analysis...").to_string());
                // 触发被动分析（在块外执行，因为需要访问 state.rx_buffer）
                s.pre_trigger_phase = 99; // 标记需要被动分析
            }
            break 'main;
        }

        // ===== DTR/RTS 预触发状态机（非阻塞） =====
        // 只在第一个探测前执行一次（dtr_rts_tried=false）
        if !s.dtr_rts_tried && s.probe_index == 0 && s.pre_trigger_phase < 3 {
            let now = Instant::now();
            match s.pre_trigger_phase {
                0 => {
                    // Phase 0 → 1: 设置 BOOT0=高(RTS), RESET=低(DTR)
                    s.log.push(crate::i18n::t("▸ DTR/RTS trigger: RTS=high (BOOT0=1), DTR=low (RESET)").to_string());
                    let _ = state.port.write_request_to_send(true);  // RTS=BOOT0
                    let _ = state.port.write_data_terminal_ready(false); // DTR=RESET低
                    s.pre_trigger_phase = 1;
                    s.pre_trigger_until = Some(now + Duration::from_millis(10));
                    break 'main;
                }
                1 => {
                    // 等待 Phase 1 超时
                    if let Some(deadline) = s.pre_trigger_until {
                        if now < deadline {
                            break 'main;
                        }
                    }
                    // Phase 1 → 2: 释放 RESET（DTR 拉高），保持 BOOT0
                    s.log.push(crate::i18n::t("▸ DTR/RTS trigger: DTR=high (RESET released), waiting for the device to enter Boot").to_string());
                    let _ = state.port.write_data_terminal_ready(true); // DTR=RESET高
                    s.pre_trigger_phase = 2;
                    s.pre_trigger_until = Some(now + Duration::from_millis(150));
                    break 'main;
                }
                2 => {
                    // 等待 Phase 2 超时（设备复位完成）
                    if let Some(deadline) = s.pre_trigger_until {
                        if now < deadline {
                            break 'main;
                        }
                    }
                    // Phase 2 → 3: 完成，恢复 DTR/RTS 到默认状态
                    s.log.push(crate::i18n::t("▸ DTR/RTS trigger complete, sending sync bytes").to_string());
                    let _ = state.port.write_request_to_send(false); // RTS 恢复低
                    s.pre_trigger_phase = 3;
                    s.dtr_rts_tried = true;
                    // 不 return，继续执行下面的发送逻辑
                }
                _ => {}
            }
        }

        let kind = sequence[s.probe_index];

        if !s.sent {
            // === 发送阶段 ===
            // 记录当前 rx_buffer 长度（用于后续提取新数据）
            s.rx_start_seq = state.rx_entry_seq;
            // 清空串口输入缓冲区（丢弃旧的鞋垫数据）
            let _ = state.port.clear_buffer(ClearBuffer::Input);
            // 构造并发送同步字节
            let cmd = kind.command();
            let hex_preview: String = cmd
                .iter()
                .take(12)
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" ");
            let more = if cmd.len() > 12 { "..." } else { "" };
            let idx = s.probe_index + 1;
            let total = sequence.len();
            let kind_name = kind.name();
            s.log.push(crate::i18n::tf("→ [{index}/{total}] {name} send ({len}): {hex}{more}", &[
                ("{index}", "{index}", &format!("{}", idx)),
                ("{total}", "{total}", &format!("{}", total)),
                ("{name}", "{name}", &crate::i18n::t_owned(kind_name)),
                ("{len}", "{len}", &format!("{}", cmd.len())),
                ("{hex}", "{hex}", &hex_preview),
                ("{more}", "{more}", more),
            ]));
            match state.send_raw(&cmd) {
                Ok(_) => {
                    s.sent = true;
                    s.wait_until =
                        Some(Instant::now() + Duration::from_millis(kind.timeout_ms()));
                }
                Err(e) => {
                    s.log.push(crate::i18n::tf("  ✗ Send failed: {error}", &[
                        ("{error}", "{error}", &format!("{}", e)),
                    ]));
                    s.probe_index += 1;
                    s.sent = false;
                }
            }
            break 'main;
        }

        // === 等待应答阶段 ===
        let now = Instant::now();
        let deadline = s.wait_until.unwrap_or(now);

        if now < deadline {
            // 还在等待中，不读取数据（让 poll_rx 自然积累 RX 到 rx_buffer）
            break 'main;
        }

        // === 超时后：从 rx_buffer 提取发送后的 RX 数据 ===
        let rx_data: Vec<u8> = collect_rx_since(state, s.rx_start_seq);

        if rx_data.is_empty() {
            s.log.push(crate::i18n::tf("  ✗ No response (timeout {ms}ms)", &[
                ("{ms}", "{ms}", &format!("{}", kind.timeout_ms())),
            ]));
        } else {
            let hex_preview: String = rx_data
                .iter()
                .take(16)
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" ");
            let more = if rx_data.len() > 16 { "..." } else { "" };
            s.log.push(crate::i18n::tf("  ← Received {count} bytes: {hex}{more}", &[
                ("{count}", "{count}", &format!("{}", rx_data.len())),
                ("{hex}", "{hex}", &hex_preview),
                ("{more}", "{more}", more),
            ]));

            // 校验应答
            if let Some(result) = kind.validate(&rx_data) {
                let confidence_pct = (result.confidence * 100.0) as u8;
                if result.confidence >= 0.7 {
                    s.log.push(crate::i18n::tf("  ✓ Identified: {vendor} {family} ({pct}%)", &[
                        ("{vendor}", "{vendor}", &crate::i18n::t_owned(&result.vendor)),
                        ("{family}", "{family}", &crate::i18n::t_owned(&result.chip_family)),
                        ("{pct}", "{pct}", &format!("{}", confidence_pct)),
                    ]));
                } else {
                    s.log.push(crate::i18n::tf("  △ Partial match: {vendor} {family} ({pct}%) - {evidence}", &[
                        ("{vendor}", "{vendor}", &crate::i18n::t_owned(&result.vendor)),
                        ("{family}", "{family}", &crate::i18n::t_owned(&result.chip_family)),
                        ("{pct}", "{pct}", &format!("{}", confidence_pct)),
                        ("{evidence}", "{evidence}", &crate::i18n::t_owned(&result.evidence)),
                    ]));
                }
                s.results.push(result);
            } else {
                s.log.push(crate::i18n::tf("  ○ Response does not match {name} signature", &[
                    ("{name}", "{name}", &crate::i18n::t_owned(kind.name())),
                ]));
            }
        }

        // 推进到下一条探测
        s.probe_index += 1;
        s.sent = false;
    } // end 'main

    // ===== 被动协议特征分析（所有 Boot 探测失败后触发） =====
    // pre_trigger_phase=99 是标记位，表示需要执行被动分析
    let need_passive = s.passive_result.is_none() && s.pre_trigger_phase == 99;
    if need_passive {
        // 调试日志：确认 USB 识别结果是否到达被动分析
        if let Some(usb) = usb_bridge {
            s.log.push(format!(
                "  [USB] {} {} VID={:04X} PID={:04X} native={}",
                usb.vendor, usb.chip_name, usb.vid, usb.pid, usb.is_native_mcu
            ));
        } else {
            s.log.push(crate::i18n::t("  [USB] No USB bridge chip identified (None)").to_string());
        }
        // 从 rx_buffer 收集鞋垫帧间隔和帧头匹配率
        let (intervals, header_rate) = collect_insole_frame_stats(&state.rx_buffer);
        let baud_rate = state.config.baud_rate;
        let result = analyze_passive_mcu_comprehensive(&intervals, header_rate, baud_rate, &state.rx_buffer, usb_bridge);
        s.frame_intervals_ms = intervals.clone();
        s.passive_result = Some(result.clone());
        s.pre_trigger_phase = 0; // 清除标记
        // 输出被动分析结果到日志
        s.log.push(crate::i18n::t("report.protocol.section.passive").to_string());
        if intervals.is_empty() && header_rate == 0.0 {
            s.log.push(crate::i18n::t("  ✗ No insole frame data collected").to_string());
        } else {
            s.log.push(crate::i18n::tf("  [1] Frame interval: {frames} frames, avg {avg}ms, jitter {jitter}ms", &[
                ("{frames}", "{frames}", &format!("{}", result.sample_count)),
                ("{avg}", "{avg}", &format!("{:.2}", result.avg_interval_ms)),
                ("{jitter}", "{jitter}", &format!("{:.2}", result.jitter_ms)),
            ]));
            s.log.push(crate::i18n::tf("  [2] Header match rate: {rate}% → {source}", &[
                ("{rate}", "{rate}", &format!("{:.1}", result.header_match_rate * 100.0)),
                ("{source}", "{source}", &result.clock_source),
            ]));
            s.log.push(crate::i18n::tf("  [3] Protocol features: FPU {fpu}, min RAM {ram}B, {adc}", &[
                ("{fpu}", "{fpu}", if result.needs_fpu { crate::i18n::t("required") } else { crate::i18n::t("not required") }),
                ("{ram}", "{ram}", &format!("{}", result.min_ram_bytes)),
                ("{adc}", "{adc}", &result.adc_capability),
            ]));
            s.log.push(crate::i18n::tf("  [4] Throughput: {bps} B/s (baud {baud})", &[
                ("{bps}", "{bps}", &format!("{}", result.throughput_bps)),
                ("{baud}", "{baud}", &format!("{}", result.baud_rate)),
            ]));
            // 深度协议分析结果
            if result.parsed_frames > 0 {
                s.log.push(crate::i18n::tf("  [5] Deep parse: {frames} frames complete, SeqTag pairing {seq}%, DMA signature {dma}%", &[
                    ("{frames}", "{frames}", &format!("{}", result.parsed_frames)),
                    ("{seq}", "{seq}", &format!("{:.0}", result.frame_consistency * 100.0)),
                    ("{dma}", "{dma}", &format!("{:.0}", result.dma_likelihood * 100.0)),
                ]));
                if result.adc_resolution > 0 {
                    s.log.push(crate::i18n::tf("  [6] ADC: {bits}bit (range {min}-{max})", &[
                        ("{bits}", "{bits}", &format!("{}", result.adc_resolution)),
                        ("{min}", "{min}", &format!("{}", result.adc_min_value)),
                        ("{max}", "{max}", &format!("{}", result.adc_max_value)),
                    ]));
                }
                if result.imu_quaternion_norm > 0.0 {
                    s.log.push(crate::i18n::tf("  [7] IMU: float {valid} (|q|={norm}), byte order {order}", &[
                        ("{valid}", "{valid}", if result.imu_float_valid { crate::i18n::t("valid") } else { crate::i18n::t("invalid") }),
                        ("{norm}", "{norm}", &format!("{:.3}", result.imu_quaternion_norm)),
                        ("{order}", "{order}", &crate::i18n::t_owned(&result.byte_order)),
                    ]));
                }
            }
            // 输出候选 MCU 排名
            s.log.push(crate::i18n::t("report.protocol.section.candidates").to_string());
            for (i, c) in result.candidates.iter().take(5).enumerate() {
                let pct = (c.score * 100.0) as u8;
                s.log.push(format!(
                    "  {}. {} {} ({}%) — {}",
                    i + 1, c.vendor, c.example_model, pct, c.chip_family
                ));
                for reason in &c.match_reasons {
                    s.log.push(format!("      ✓ {}", reason));
                }
                for reason in &c.mismatch_reasons {
                    s.log.push(format!("      ✗ {}", reason));
                }
            }
            s.log.push(crate::i18n::tf("  → Best inference: {vendor} {family} ({pct}%)", &[
                ("{vendor}", "{vendor}", &crate::i18n::t_owned(&result.vendor)),
                ("{family}", "{family}", &crate::i18n::t_owned(&result.chip_family)),
                ("{pct}", "{pct}", &format!("{}", (result.confidence * 100.0) as u8)),
            ]));
        }
        s.log.push(crate::i18n::t("  Note: the MCU silkscreen may be erased; the above is inferred from communication characteristics").to_string());
    }
}

/// 从 rx_buffer 收集鞋垫帧间隔和帧头匹配率
///
/// 返回 (帧间隔列表ms, 帧头匹配率0.0-1.0)
/// 帧头匹配率 = 包含 AA 55 03 99 的 Rx 条目数 / 总 Rx 条目数
/// 高匹配率说明波特率精确（外部晶振），低匹配率说明波特率误差大（内部 RC）
fn collect_insole_frame_stats(rx_buffer: &[RxEntry]) -> (Vec<u64>, f64) {
    let header = [0xAA, 0x55, 0x03, 0x99];
    let mut frame_times: Vec<u64> = Vec::new();
    let mut total_rx_entries = 0usize;
    let mut matched_entries = 0usize;

    for entry in rx_buffer.iter().filter(|e| e.direction == Direction::Rx) {
        total_rx_entries += 1;
        // 在数据中搜索帧头
        if entry.data.windows(4).any(|w| w == header) {
            matched_entries += 1;
            let ms = entry.timestamp.timestamp_millis() as u64;
            frame_times.push(ms);
        }
    }

    // 计算相邻帧的时间间隔
    let mut intervals: Vec<u64> = Vec::new();
    for w in frame_times.windows(2) {
        if w[1] > w[0] {
            intervals.push(w[1] - w[0]);
        }
    }

    let match_rate = if total_rx_entries > 0 {
        matched_entries as f64 / total_rx_entries as f64
    } else {
        0.0
    };

    (intervals, match_rate)
}

// ============================================================================
// 深度协议分析：解析实际数据帧内容推断 MCU 硬件特征
// ============================================================================

/// 鞋垫协议帧结构常量
const INSOLE_HEADER: [u8; 4] = [0xAA, 0x55, 0x03, 0x99];
/// Packet1 类型：128 字节 ADC 前半帧（64 个传感器 × 2 字节）
const INSOLE_PKT1_TYPE: u8 = 0x01;
/// Packet2 类型：128 字节 ADC 后半帧 + 16 字节 IMU
const INSOLE_PKT2_TYPE: u8 = 0x02;
/// Packet1 载荷长度
const INSOLE_PKT1_PAYLOAD: usize = 128;
/// Packet2 载荷长度（ADC 128 + IMU 16）
const INSOLE_PKT2_PAYLOAD: usize = 144;

/// 从 rx_buffer 中提取并解析鞋垫协议帧
///
/// 返回所有完整解析出的帧数据（Packet1+Packet2 配对后的完整帧）
/// 每个帧包含 256 字节 ADC 数据 + 16 字节 IMU 数据
fn parse_insole_frames(rx_buffer: &[RxEntry]) -> Vec<(Vec<u8>, Vec<u8>, u8)> {
    // 先把所有 RX 数据拼接成一个连续缓冲区
    let mut raw: Vec<u8> = Vec::new();
    for entry in rx_buffer.iter().filter(|e| e.direction == Direction::Rx) {
        raw.extend_from_slice(&entry.data);
    }

    let mut frames: Vec<(Vec<u8>, Vec<u8>, u8)> = Vec::new();
    let mut pending_pkt1: Option<(Vec<u8>, u8)> = None; // (ADC前半帧, SeqTag)

    let mut i = 0;
    while i + 6 < raw.len() {
        // 寻找帧头 AA 55 03 99
        if raw[i..i + 4] != INSOLE_HEADER {
            i += 1;
            continue;
        }

        // 帧头后：[类型(1)] [SeqTag(1)] [长度(2, LE)] [载荷...] [校验(2)]
        if i + 8 > raw.len() {
            break;
        }
        let pkt_type = raw[i + 4];
        let seq_tag = raw[i + 5];
        let payload_len = u16::from_le_bytes([raw[i + 6], raw[i + 7]]) as usize;

        // 校验包长合理性
        let expected_len = if pkt_type == INSOLE_PKT1_TYPE {
            INSOLE_PKT1_PAYLOAD
        } else if pkt_type == INSOLE_PKT2_TYPE {
            INSOLE_PKT2_PAYLOAD
        } else {
            i += 4;
            continue;
        };

        if payload_len != expected_len {
            i += 4;
            continue;
        }

        let total_len = 8 + payload_len + 2; // 头4+类型1+seq1+长度2 + 载荷 + 校验2
        if i + total_len > raw.len() {
            break;
        }

        let payload = &raw[i + 8..i + 8 + payload_len];

        if pkt_type == INSOLE_PKT1_TYPE {
            // 存储待配对的 Packet1
            pending_pkt1 = Some((payload.to_vec(), seq_tag));
        } else if pkt_type == INSOLE_PKT2_TYPE {
            // 配对 Packet2 与待匹配的 Packet1
            if let Some((pkt1_data, pkt1_seq)) = pending_pkt1.take() {
                if pkt1_seq == seq_tag {
                    // SeqTag 匹配，组装完整帧
                    let pkt2_adc = &payload[..128];
                    let pkt2_imu = &payload[128..144];
                    let mut full_adc = pkt1_data.clone();
                    full_adc.extend_from_slice(pkt2_adc);
                    frames.push((full_adc, pkt2_imu.to_vec(), seq_tag));
                }
            }
            pending_pkt1 = None;
        }

        i += total_len;
    }

    frames
}

/// 分析 ADC 分辨率
///
/// 解析 256 字节 ADC 数据（128 个 16-bit 采样值），根据数值范围推断 ADC 分辨率：
/// - 8-bit: 0-255
/// - 10-bit: 0-1023
/// - 12-bit: 0-4095
/// - 16-bit: 0-65535
///
/// 返回 (分辨率位, 最大值, 最小值)
fn analyze_adc_resolution(adc_data: &[u8]) -> (u8, u16, u16) {
    if adc_data.len() < 2 {
        return (0, 0, 0);
    }

    let mut max_val: u16 = 0;
    let mut min_val: u16 = 0xFFFF;

    // 按 16-bit little-endian 解析采样值
    for chunk in adc_data.chunks(2) {
        if chunk.len() == 2 {
            let val = u16::from_le_bytes([chunk[0], chunk[1]]);
            if val > max_val {
                max_val = val;
            }
            if val < min_val {
                min_val = val;
            }
        }
    }

    // 根据最大值推断分辨率
    let resolution = if max_val <= 255 {
        8
    } else if max_val <= 1023 {
        10
    } else if max_val <= 4095 {
        12
    } else {
        16
    };

    (resolution, max_val, min_val)
}

/// 分析 IMU 四元数浮点数据
///
/// 16 字节 IMU 数据 = 4 × float32 四元数 (w, x, y, z)
/// 验证是否为有效 IEEE754 float32，并检查四元数模长是否接近 1.0
///
/// 返回 (是否有效float, 四元数模长, 字节序)
fn analyze_imu_quaternion(imu_data: &[u8]) -> (bool, f32, &'static str) {
    if imu_data.len() < 16 {
        return (false, 0.0, "unknown");
    }

    // 尝试小端解析
    let le_w = f32::from_le_bytes([imu_data[0], imu_data[1], imu_data[2], imu_data[3]]);
    let le_x = f32::from_le_bytes([imu_data[4], imu_data[5], imu_data[6], imu_data[7]]);
    let le_y = f32::from_le_bytes([imu_data[8], imu_data[9], imu_data[10], imu_data[11]]);
    let le_z = f32::from_le_bytes([imu_data[12], imu_data[13], imu_data[14], imu_data[15]]);

    // 尝试大端解析
    let be_w = f32::from_be_bytes([imu_data[0], imu_data[1], imu_data[2], imu_data[3]]);
    let be_x = f32::from_be_bytes([imu_data[4], imu_data[5], imu_data[6], imu_data[7]]);
    let be_y = f32::from_be_bytes([imu_data[8], imu_data[9], imu_data[10], imu_data[11]]);
    let be_z = f32::from_be_bytes([imu_data[12], imu_data[13], imu_data[14], imu_data[15]]);

    // 判断哪种字节序产生有效 float
    let le_all_valid = is_valid_float(le_w) && is_valid_float(le_x)
        && is_valid_float(le_y) && is_valid_float(le_z);
    let be_all_valid = is_valid_float(be_w) && is_valid_float(be_x)
        && is_valid_float(be_y) && is_valid_float(be_z);

    if !le_all_valid && !be_all_valid {
        return (false, 0.0, "unknown");
    }

    // 计算四元数模长 |q| = sqrt(w² + x² + y² + z²)，有效四元数模长 ≈ 1.0
    let (norm, byte_order) = if le_all_valid {
        let norm = (le_w * le_w + le_x * le_x + le_y * le_y + le_z * le_z).sqrt();
        (norm, "little")
    } else {
        let norm = (be_w * be_w + be_x * be_x + be_y * be_y + be_z * be_z).sqrt();
        (norm, "big")
    };

    // 模长在 0.9-1.1 范围内认为是有效四元数
    let is_valid = norm > 0.7 && norm < 1.3;

    (is_valid, norm, byte_order)
}

/// 检查 float 值是否为有效 IEEE754 数（非 NaN/非无穷/在合理范围）
fn is_valid_float(v: f32) -> bool {
    v.is_finite() && v.abs() < 100.0
}

/// 分析帧结构一致性（SeqTag 配对率）
///
/// 统计 Packet1 和 Packet2 的 SeqTag 配对情况
/// 高配对率说明 MCU 的帧组装逻辑稳定，反映固件质量
fn analyze_frame_consistency(rx_buffer: &[RxEntry]) -> (f64, usize) {
    let mut raw: Vec<u8> = Vec::new();
    for entry in rx_buffer.iter().filter(|e| e.direction == Direction::Rx) {
        raw.extend_from_slice(&entry.data);
    }

    let mut pkt1_count = 0usize;
    let mut pkt2_count = 0usize;
    let mut paired_count = 0usize;
    let mut pending_pkt1_seq: Option<u8> = None;

    let mut i = 0;
    while i + 8 < raw.len() {
        if raw[i..i + 4] != INSOLE_HEADER {
            i += 1;
            continue;
        }

        let pkt_type = raw[i + 4];
        let seq_tag = raw[i + 5];
        let payload_len = u16::from_le_bytes([raw[i + 6], raw[i + 7]]) as usize;

        let expected_len = if pkt_type == INSOLE_PKT1_TYPE {
            INSOLE_PKT1_PAYLOAD
        } else if pkt_type == INSOLE_PKT2_TYPE {
            INSOLE_PKT2_PAYLOAD
        } else {
            i += 4;
            continue;
        };

        if payload_len != expected_len {
            i += 4;
            continue;
        }

        let total_len = 8 + payload_len + 2;
        if i + total_len > raw.len() {
            break;
        }

        if pkt_type == INSOLE_PKT1_TYPE {
            pkt1_count += 1;
            pending_pkt1_seq = Some(seq_tag);
        } else if pkt_type == INSOLE_PKT2_TYPE {
            pkt2_count += 1;
            if let Some(p1_seq) = pending_pkt1_seq {
                if p1_seq == seq_tag {
                    paired_count += 1;
                }
            }
            pending_pkt1_seq = None;
        }

        i += total_len;
    }

    let total_pairs = pkt1_count.min(pkt2_count);
    let consistency = if total_pairs > 0 {
        paired_count as f64 / total_pairs as f64
    } else {
        0.0
    };

    (consistency, paired_count)
}

/// 分析 DMA 传输特征
///
/// DMA 传输特征：同一帧内字节到达间隔极其均匀（硬件搬运）
/// 轮询/中断传输：字节间隔可能有波动（CPU 介入）
///
/// 通过分析同一 RxEntry 内的数据块大小和频率推断传输模式
/// DMA 通常一次传输完整帧或半帧，轮询可能零散
fn analyze_dma_pattern(rx_buffer: &[RxEntry]) -> f64 {
    let rx_entries: Vec<&RxEntry> = rx_buffer
        .iter()
        .filter(|e| e.direction == Direction::Rx)
        .collect();

    if rx_entries.len() < 5 {
        return 0.0;
    }

    // 分析数据块大小分布
    let sizes: Vec<usize> = rx_entries.iter().map(|e| e.data.len()).collect();
    let avg_size = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;

    // 计算块大小的变异系数（CV = 标准差/均值）
    let variance: f64 = sizes
        .iter()
        .map(|&s| { let d = s as f64 - avg_size; d * d })
        .sum::<f64>() / sizes.len() as f64;
    let std_dev = variance.sqrt();
    let cv = if avg_size > 0.0 { std_dev / avg_size } else { 1.0 };

    // DMA 特征：块大小均匀（CV 小），且块较大（一次传输完整帧）
    // 轮询特征：块大小不均匀（CV 大），块较小
    let size_score: f64 = if cv < 0.1 {
        0.8 // 非常均匀 → 强 DMA 特征
    } else if cv < 0.3 {
        0.5
    } else if cv < 0.6 {
        0.3
    } else {
        0.1 // 不均匀 → 轮询特征
    };

    // DMA 通常传输较大块（≥64 字节）
    let bulk_score: f64 = if avg_size >= 128.0 {
        0.2
    } else if avg_size >= 64.0 {
        0.1
    } else {
        0.0
    };

    (size_score + bulk_score).min(1.0_f64)
}

/// 计算指定 MCU 时钟源在目标波特率下的分频误差（百分比）
///
/// 误差 < 0.5% → 外部晶振（HSE）
/// 误差 0.5%-2% → 可能内部 RC 或非标准晶振
/// 误差 > 2% → 内部 RC（HSI）
fn calc_baud_error(clock_mhz: f64, baud: u32) -> f64 {
    // USART 分频器：USARTDIV = fclk / (16 * baud)
    let ideal_div = clock_mhz * 1e6 / (16.0 * baud as f64);
    // 实际分频值（4位整数+4位小数，精度 1/16）
    let actual_div = (ideal_div * 16.0).round() / 16.0;
    if actual_div == 0.0 {
        return 100.0;
    }
    let actual_baud = clock_mhz * 1e6 / (16.0 * actual_div);
    ((actual_baud - baud as f64) / baud as f64).abs() * 100.0
}

/// 综合被动协议特征分析：多维度推断 MCU 类型
///
/// 分析维度：
/// 1. 帧间隔抖动 — 反映 MCU 调度方式（DMA/RTOS/轮询）
/// 2. 帧头匹配率 — 反映波特率精度（外部晶振 vs 内部 RC）
/// 3. 波特率分频误差 — 不同时钟源在各 MCU 上的分频精度
/// 4. 协议特征 — FPU 需求（IMU float 四元数）、RAM 需求、ADC 能力
/// 5. 数据吞吐量 — 处理能力推断
pub(crate) fn analyze_passive_mcu_comprehensive(
    intervals: &[u64],
    header_match_rate: f64,
    baud_rate: u32,
    rx_buffer: &[RxEntry],
    usb_bridge: Option<&UsbBridgeInfo>,
) -> PassiveAnalysisResult {
    let mut result = PassiveAnalysisResult::default();
    result.baud_rate = baud_rate;
    result.header_match_rate = header_match_rate;

    // ===== 1. 帧间隔统计 =====
    if !intervals.is_empty() {
        let n = intervals.len() as f64;
        result.avg_interval_ms = intervals.iter().sum::<u64>() as f64 / n;
        let variance: f64 = intervals
            .iter()
            .map(|&v| { let d = v as f64 - result.avg_interval_ms; d * d })
            .sum::<f64>() / n;
        result.jitter_ms = variance.sqrt();
        result.sample_count = intervals.len();

        // 吞吐量：帧大小(272B) × 帧率
        if result.avg_interval_ms > 0.0 {
            let fps = 1000.0 / result.avg_interval_ms;
            result.throughput_bps = (272.0 * fps) as u64;
        }
    }

    // ===== 2. 时钟源推断（基于帧头匹配率）=====
    if header_match_rate >= 0.95 {
        result.clock_source = crate::i18n::t("External crystal (HSE), baud rate error < 0.5%").to_string();
    } else if header_match_rate >= 0.80 {
        result.clock_source = crate::i18n::t("External crystal (HSE) or high-precision internal RC").to_string();
    } else if header_match_rate >= 0.50 {
        result.clock_source = crate::i18n::t("Internal RC (HSI) or non-standard crystal, large baud rate error").to_string();
    } else {
        result.clock_source = crate::i18n::t("Baud rate mismatch or large internal RC error").to_string();
    }

    // ===== 3. 协议特征分析 =====
    // 鞋垫协议：双包结构 272B payload + 16B IMU (4×float32 四元数)
    // needs_fpu 不再硬编码，改为在深度解析中根据 IMU 浮点数据有效性动态判定
    result.min_ram_bytes = 512; // 双包缓冲 + 帧组装
    result.adc_capability = crate::i18n::t("16×16=256 ADC samples (requires multiplexer or multiple ADCs)").to_string();

    // ===== 3.5 深度协议分析：解析实际帧数据 =====
    let parsed_frames = parse_insole_frames(rx_buffer);
    result.parsed_frames = parsed_frames.len();

    // 帧结构一致性
    let (consistency, paired_count) = analyze_frame_consistency(rx_buffer);
    result.frame_consistency = consistency;

    // DMA 传输特征
    result.dma_likelihood = analyze_dma_pattern(rx_buffer);

    // 分析 ADC 分辨率（取所有帧中 ADC 数据的最大/最小值）
    if !parsed_frames.is_empty() {
        let mut all_max: u16 = 0;
        let mut all_min: u16 = 0xFFFF;
        for (adc_data, imu_data, _seq) in &parsed_frames {
            let (res, max_v, min_v) = analyze_adc_resolution(adc_data);
            if res > 0 {
                if max_v > all_max { all_max = max_v; }
                if min_v < all_min { all_min = min_v; }
                if result.adc_resolution == 0 || res < result.adc_resolution {
                    result.adc_resolution = res;
                }
            }

            // 分析 IMU 四元数（只分析第一帧即可）
            if result.imu_quaternion_norm == 0.0 {
                let (valid, norm, byte_order) = analyze_imu_quaternion(imu_data);
                result.imu_float_valid = valid;
                result.imu_quaternion_norm = norm;
                result.byte_order = byte_order.to_string();
                // 仅在实际检测到有效 IMU 浮点数据时才要求 FPU
                // 避免无数据时 LPC1768(有FPU) 不合理地胜过 STM32F103(无FPU)
                if valid {
                    result.needs_fpu = true;
                }
            }
        }
        result.adc_max_value = all_max;
        result.adc_min_value = all_min;

        // 更新 ADC 能力描述
        if result.adc_resolution > 0 {
            result.adc_capability = crate::i18n::tf("{bits}bit ADC (range {min}-{max}, 256 samples/frame)", &[
                ("{bits}", "{bits}", &format!("{}", result.adc_resolution)),
                ("{min}", "{min}", &format!("{}", result.adc_min_value)),
                ("{max}", "{max}", &format!("{}", result.adc_max_value)),
            ]);
        }
    }

    // ===== 4. 候选 MCU 评分 =====
    // 定义 MCU 配置库：(厂商, 系列, 型号, 时钟MHz, 有FPU, RAM KB, ADC通道, ADC分辨率)
    let mcu_profiles: &[(&str, &str, &str, f64, bool, usize, usize, u8)] = &[
        // STM32 系列
        ("STMicroelectronics", "STM32F1", "STM32F103C8T6", 72.0, false, 20, 16, 12),
        ("STMicroelectronics", "STM32F1", "STM32F103RET6", 72.0, false, 64, 16, 12),
        ("STMicroelectronics", "STM32F3", "STM32F303C8T6", 72.0, true, 12, 16, 12),
        ("STMicroelectronics", "STM32F3", "STM32F303CBT6", 72.0, true, 16, 16, 12),
        ("STMicroelectronics", "STM32F3", "STM32F303RCT6", 72.0, true, 40, 20, 12),
        ("STMicroelectronics", "STM32F4", "STM32F407VGT6", 84.0, true, 192, 24, 12),
        ("STMicroelectronics", "STM32F0", "STM32F030C8", 48.0, false, 64, 16, 12),
        ("STMicroelectronics", "STM32L4", "STM32L432KC", 80.0, true, 64, 16, 12),
        // GD32 系列（兼容 STM32）
        ("GigaDevice", "GD32F1", "GD32F103C8T6", 108.0, false, 20, 16, 12),
        ("GigaDevice", "GD32F3", "GD32F305RG", 108.0, true, 96, 16, 12),
        ("GigaDevice", "GD32F3", "GD32F303C8T6", 108.0, true, 20, 16, 12),
        // CH32 系列（WCH，兼容 STM32）
        ("WCH", "CH32F1", "CH32F103C8T6", 72.0, false, 20, 16, 12),
        ("WCH", "CH32V2", "CH32V203", 144.0, true, 64, 16, 12),
        // ESP 系列
        ("Espressif", "ESP32", "ESP32-WROOM-32", 40.0, true, 320, 18, 12),
        ("Espressif", "ESP32-S3", "ESP32-S3", 40.0, true, 512, 20, 12),
        ("Espressif", "ESP8266", "ESP-12F", 40.0, false, 80, 1, 10),
        // STM8 系列
        ("STMicroelectronics", "STM8S", "STM8S207R8", 16.0, false, 6, 16, 10),
        // LPC 系列
        ("NXP", "LPC17xx", "LPC1768", 100.0, true, 64, 8, 12),
        ("NXP", "LPC11xx", "LPC1114", 48.0, false, 8, 8, 10),
    ];

    for &(vendor, family, model, clock_mhz, has_fpu, ram_kb, adc_ch, adc_res) in mcu_profiles {
        let mut candidate = McuCandidate {
            vendor: vendor.to_string(),
            chip_family: family.to_string(),
            example_model: model.to_string(),
            score: 0.0,
            match_reasons: Vec::new(),
            mismatch_reasons: Vec::new(),
        };

        let mut score: f64 = 0.0;

        // --- 维度1: 波特率分频误差 ---
        let baud_err = calc_baud_error(clock_mhz, baud_rate);
        if baud_err < 0.1 {
            score += 0.20;
            candidate.match_reasons.push(crate::i18n::tf("{mhz}MHz clock divider error {err}% (extremely precise)", &[
                ("{mhz}", "{mhz}", &format!("{}", clock_mhz)),
                ("{err}", "{err}", &format!("{:.3}", baud_err)),
            ]));
        } else if baud_err < 0.5 {
            score += 0.15;
            candidate.match_reasons.push(crate::i18n::tf("{mhz}MHz clock divider error {err}% (external crystal signature)", &[
                ("{mhz}", "{mhz}", &format!("{}", clock_mhz)),
                ("{err}", "{err}", &format!("{:.2}", baud_err)),
            ]));
        } else if baud_err < 1.0 {
            score += 0.08;
            candidate.match_reasons.push(crate::i18n::tf("{mhz}MHz clock divider error {err}% (acceptable)", &[
                ("{mhz}", "{mhz}", &format!("{}", clock_mhz)),
                ("{err}", "{err}", &format!("{:.2}", baud_err)),
            ]));
        } else if baud_err < 3.0 {
            score += 0.03;
            candidate.mismatch_reasons.push(crate::i18n::tf("{mhz}MHz clock divider error {err}% (too large, external crystal required)", &[
                ("{mhz}", "{mhz}", &format!("{}", clock_mhz)),
                ("{err}", "{err}", &format!("{:.1}", baud_err)),
            ]));
        } else {
            candidate.mismatch_reasons.push(crate::i18n::tf("{mhz}MHz clock divider error {err}% (excessive, unlikely)", &[
                ("{mhz}", "{mhz}", &format!("{}", clock_mhz)),
                ("{err}", "{err}", &format!("{:.1}", baud_err)),
            ]));
        }

        // --- 维度2: 帧头匹配率 × 时钟源一致性 ---
        if header_match_rate >= 0.95 && baud_err < 0.5 {
            score += 0.15;
            candidate.match_reasons.push(crate::i18n::t("Very high header match rate + small divider error → external crystal").to_string());
        } else if header_match_rate < 0.5 && baud_err > 1.0 {
            score += 0.05;
            candidate.mismatch_reasons.push(crate::i18n::t("Low header match rate → baud rate may be mismatched").to_string());
        }

        // --- 维度3: FPU 需求 ---
        if result.needs_fpu {
            if has_fpu {
                score += 0.15;
                candidate.match_reasons.push(crate::i18n::t("Hardware FPU present, suitable for float quaternion math").to_string());
            } else {
                // 无 FPU 也能跑，但性能受限
                score += 0.05;
                candidate.mismatch_reasons.push(crate::i18n::t("No hardware FPU; float math must be emulated in software (performance limited)").to_string());
            }
        } else {
            // needs_fpu 未确定（未检测到 IMU 浮点数据）时，
            // 给有 FPU 的芯片小幅加分：FPU 是"有备无患"的优势，
            // 现代 Cortex-M4F 设计（F303/G431 等）即使当前不跑浮点也优于无 FPU 的 M3
            if has_fpu {
                score += 0.03;
                candidate.match_reasons.push(crate::i18n::t("Hardware FPU present (Cortex-M4F, future float math can be accelerated)").to_string());
            }
        }

        // --- 维度4: RAM 需求 ---
        let ram_bytes = ram_kb * 1024;
        if ram_bytes >= result.min_ram_bytes {
            score += 0.10;
            if ram_bytes >= result.min_ram_bytes * 4 {
                candidate.match_reasons.push(crate::i18n::tf("RAM {kb}KB ample", &[
                    ("{kb}", "{kb}", &format!("{}", ram_kb)),
                ]));
            } else {
                candidate.match_reasons.push(crate::i18n::tf("RAM {kb}KB sufficient", &[
                    ("{kb}", "{kb}", &format!("{}", ram_kb)),
                ]));
            }
        } else {
            candidate.mismatch_reasons.push(crate::i18n::tf("RAM {kb}KB insufficient (need ≥{need}B)", &[
                ("{kb}", "{kb}", &format!("{}", ram_kb)),
                ("{need}", "{need}", &format!("{}", result.min_ram_bytes)),
            ]));
        }

        // --- 维度5: 帧间隔抖动匹配 ---
        if result.sample_count > 0 {
            let jitter = result.jitter_ms;
            let avg = result.avg_interval_ms;
            // Cortex-M DMA 特征：抖动 < 1ms
            if family.starts_with("STM32") || family.starts_with("GD32") || family.starts_with("CH32") || family.starts_with("LPC") {
                if jitter < 1.0 {
                    score += 0.20;
                    candidate.match_reasons.push(crate::i18n::tf("Jitter {jitter}ms matches Cortex-M DMA + timer signature", &[
                        ("{jitter}", "{jitter}", &format!("{:.2}", jitter)),
                    ]));
                } else if jitter < 3.0 {
                    score += 0.08;
                    candidate.mismatch_reasons.push(crate::i18n::tf("Jitter {jitter}ms is large (Cortex-M DMA is usually < 1ms)", &[
                        ("{jitter}", "{jitter}", &format!("{:.1}", jitter)),
                    ]));
                } else {
                    candidate.mismatch_reasons.push(crate::i18n::tf("Jitter {jitter}ms is excessive, unlikely to be Cortex-M DMA", &[
                        ("{jitter}", "{jitter}", &format!("{:.1}", jitter)),
                    ]));
                }
            }
            // ESP32 FreeRTOS 特征：抖动 > 2ms
            if family.starts_with("ESP32") {
                if jitter > 1.5 {
                    score += 0.15;
                    candidate.match_reasons.push(crate::i18n::tf("Jitter {jitter}ms matches ESP32 FreeRTOS scheduling signature", &[
                        ("{jitter}", "{jitter}", &format!("{:.1}", jitter)),
                    ]));
                } else if jitter < 0.5 {
                    score += 0.05;
                    candidate.mismatch_reasons.push(crate::i18n::tf("Jitter {jitter}ms is small (ESP32 is usually > 1.5ms)", &[
                        ("{jitter}", "{jitter}", &format!("{:.1}", jitter)),
                    ]));
                }
            }
            // ESP8266 特征：抖动通常 > 3ms
            if family == "ESP8266" {
                if jitter > 2.0 {
                    score += 0.10;
                    candidate.match_reasons.push(crate::i18n::tf("Jitter {jitter}ms matches ESP8266 signature", &[
                        ("{jitter}", "{jitter}", &format!("{:.1}", jitter)),
                    ]));
                }
            }
            // STM8 特征：帧间隔 > 50ms（低速）
            if family.starts_with("STM8") {
                if avg > 50.0 {
                    score += 0.15;
                    candidate.match_reasons.push(crate::i18n::tf("Frame interval {avg}ms matches STM8 low-speed signature", &[
                        ("{avg}", "{avg}", &format!("{:.0}", avg)),
                    ]));
                } else {
                    candidate.mismatch_reasons.push(crate::i18n::tf("Frame interval {avg}ms is too fast for STM8", &[
                        ("{avg}", "{avg}", &format!("{:.0}", avg)),
                    ]));
                }
            }
        }

        // --- 维度6: ADC 通道数 ---
        // 256 个采样点需要 16:1 多路复用器或多个 ADC
        if adc_ch >= 16 {
            score += 0.08;
            candidate.match_reasons.push(crate::i18n::tf("{ch} ADC channels (with a multiplexer, 256 points achievable)", &[
                ("{ch}", "{ch}", &format!("{}", adc_ch)),
            ]));
        } else if adc_ch >= 8 {
            score += 0.04;
            candidate.match_reasons.push(crate::i18n::tf("{ch} ADC channels (external multiplexer required)", &[
                ("{ch}", "{ch}", &format!("{}", adc_ch)),
            ]));
        } else if adc_ch <= 1 {
            score += 0.01;
            candidate.mismatch_reasons.push(crate::i18n::tf("Only {ch} ADC channels, many external components required", &[
                ("{ch}", "{ch}", &format!("{}", adc_ch)),
            ]));
        }

        // --- 维度7: ADC 分辨率（库定义值 vs 实测值） ---
        if result.adc_resolution > 0 {
            if adc_res == result.adc_resolution {
                score += 0.12;
                candidate.match_reasons.push(crate::i18n::tf(
                    "ADC resolution {bits}bit matches measurement", &[
                        ("{bits}", "{bits}", &format!("{}", adc_res)),
                    ],
                ));
            } else if adc_res > result.adc_resolution {
                // 库定义 > 实测：MCU 的 ADC 可能降级使用
                score += 0.06;
                candidate.match_reasons.push(crate::i18n::tf(
                    "ADC {bits}bit can be used at {used}bit", &[
                        ("{bits}", "{bits}", &format!("{}", adc_res)),
                        ("{used}", "{used}", &format!("{}", result.adc_resolution)),
                    ],
                ));
            } else {
                // 库定义 < 实测：MCU 的 ADC 不够
                candidate.mismatch_reasons.push(crate::i18n::tf(
                    "ADC {bits}bit is below the measured {measured}bit", &[
                        ("{bits}", "{bits}", &format!("{}", adc_res)),
                        ("{measured}", "{measured}", &format!("{}", result.adc_resolution)),
                    ],
                ));
            }
        } else if adc_res >= 12 {
            score += 0.05;
        } else if adc_res <= 10 {
            candidate.mismatch_reasons.push(crate::i18n::tf("ADC {bits}bit, pressure matrix precision is low", &[
                ("{bits}", "{bits}", &format!("{}", adc_res)),
            ]));
        }

        // --- 维度8: IMU 浮点验证 ---
        if result.imu_float_valid {
            if has_fpu {
                score += 0.10;
                candidate.match_reasons.push(crate::i18n::tf(
                    "IMU quaternion valid (|q|={norm}), handled efficiently by hardware FPU", &[
                        ("{norm}", "{norm}", &format!("{:.3}", result.imu_quaternion_norm)),
                    ],
                ));
            } else {
                score += 0.04;
                candidate.mismatch_reasons.push(
                    crate::i18n::t("IMU float data valid but no FPU; software float performance is limited").to_string()
                );
            }
        } else if result.parsed_frames > 0 {
            // 解析到帧但 IMU float 无效
            if family.starts_with("ESP") || family.starts_with("STM32") {
                score += 0.02;
                candidate.mismatch_reasons.push(crate::i18n::t("IMU float data invalid, possibly a non-standard quaternion format").to_string());
            }
        }

        // --- 维度9: 字节序 ---
        if result.byte_order == "little" {
            // ARM Cortex-M / ESP32 / ESP8266 都是小端
            if family.starts_with("STM32") || family.starts_with("GD32")
                || family.starts_with("CH32") || family.starts_with("ESP")
                || family.starts_with("LPC") {
                score += 0.05;
                candidate.match_reasons.push(crate::i18n::t("Little-endian matches ARM/Xtensa architecture").to_string());
            }
        } else if result.byte_order == "big" {
            // STM8 是大端
            if family.starts_with("STM8") {
                score += 0.08;
                candidate.match_reasons.push(crate::i18n::t("Big-endian matches STM8 architecture").to_string());
            } else {
                candidate.mismatch_reasons.push(crate::i18n::t("Big-endian does not match ARM/Xtensa").to_string());
            }
        }

        // --- 维度10: DMA 传输特征 ---
        if result.dma_likelihood > 0.7 {
            // 强 DMA 特征 → Cortex-M with DMA
            if family.starts_with("STM32") || family.starts_with("GD32")
                || family.starts_with("CH32") || family.starts_with("LPC") {
                score += 0.10;
                candidate.match_reasons.push(crate::i18n::tf(
                    "DMA signature {pct}% → Cortex-M USART DMA", &[
                        ("{pct}", "{pct}", &format!("{:.0}", result.dma_likelihood * 100.0)),
                    ],
                ));
            } else if family.starts_with("ESP") {
                score += 0.04;
                candidate.mismatch_reasons.push(crate::i18n::t("Strong DMA signature; ESP series usually uses UART FIFO instead of DMA").to_string());
            }
        } else if result.dma_likelihood < 0.3 && result.dma_likelihood > 0.0 {
            // 轮询特征 → ESP8266/STM8
            if family.starts_with("ESP8266") || family.starts_with("STM8") {
                score += 0.06;
                candidate.match_reasons.push(crate::i18n::t("Uneven data blocks → polling/interrupt transfer signature").to_string());
            }
        }

        // --- 维度11: 帧结构一致性 ---
        if result.frame_consistency > 0.9 && paired_count > 5 {
            score += 0.08;
            candidate.match_reasons.push(crate::i18n::tf(
                "SeqTag pairing {pct}% → stable firmware frame assembly", &[
                    ("{pct}", "{pct}", &format!("{:.0}", result.frame_consistency * 100.0)),
                ],
            ));
        } else if result.frame_consistency < 0.5 && paired_count > 0 {
            candidate.mismatch_reasons.push(crate::i18n::tf(
                "SeqTag pairing only {pct}% → frame loss or insufficient buffering", &[
                    ("{pct}", "{pct}", &format!("{:.0}", result.frame_consistency * 100.0)),
                ],
            ));
        }

        // --- 维度N: USB 原生 MCU 识别加分（强信号）---
        // 当 USB 识别为原生 MCU（非桥接芯片）时，这是芯片类型的直接证据
        // 优先级高于被动协议分析，应大幅加分
        // 注意：用厂商名匹配而非 VID，因为 macOS 上 serialport 库可能不提供 VID/PID
        if let Some(usb) = usb_bridge {
            if usb.is_native_mcu {
                // 厂商名匹配（跨平台可靠）
                let usb_match = usb.vendor == vendor;
                // 补充：chip_name 包含 ESP32 且候选是 Espressif
                let usb_match = usb_match || (usb.chip_name.contains("ESP32") && vendor == "Espressif");
                // 补充：chip_name 包含 STM32 且候选是 STMicroelectronics
                let usb_match = usb_match || (usb.chip_name.contains("STM32") && vendor == "STMicroelectronics");
                // 补充：VID 匹配（如果 VID 可用）
                let usb_match = usb_match || (usb.vid == 0x303A && vendor == "Espressif");
                let usb_match = usb_match || (usb.vid == 0x0483 && vendor == "STMicroelectronics");

                if usb_match {
                    score += 0.50;  // 强信号：原生 USB 匹配，+50%
                    candidate.match_reasons.push(crate::i18n::tf(
                        "USB native MCU match: {vendor} {chip} {vid}:{pid}",
                        &[
                            ("{vendor}", "{vendor}", &crate::i18n::t_owned(&usb.vendor)),
                            ("{chip}", "{chip}", &usb.chip_name),
                            ("{vid}", "{vid}", &format!("{:04X}", usb.vid)),
                            ("{pid}", "{pid}", &format!("{:04X}", usb.pid)),
                        ],
                    ));
                } else {
                    // USB 识别为其他厂商的原生 MCU，当前候选不匹配，降分
                    score -= 0.15;
                    candidate.mismatch_reasons.push(crate::i18n::tf(
                        "USB native MCU points to {vendor} {chip}, does not match {candidate}",
                        &[
                            ("{vendor}", "{vendor}", &crate::i18n::t_owned(&usb.vendor)),
                            ("{chip}", "{chip}", &usb.chip_name),
                            ("{candidate}", "{candidate}", &vendor),
                        ],
                    ));
                }
            } else {
                // --- 桥接芯片亲和度加分（弱信号）---
                // CH340/CH9102 (WCH) 常配 STM32/GD32/CH32（同厂商生态）
                // CP2102 (Silicon Labs) 常配 ESP32/STM32
                // FT232 (FTDI) / PL2303 (Prolific) 为通用桥接，不加亲和度分
                if usb.chip_name.contains("CH340") || usb.chip_name.contains("CH9102") {
                    if vendor == "STMicroelectronics" || vendor == "GigaDevice" || vendor == "WCH" {
                        score += 0.05;
                        candidate.match_reasons.push(crate::i18n::tf(
                            "{bridge} bridges are commonly paired with {family} MCUs (WCH ecosystem affinity)",
                            &[
                                ("{bridge}", "{bridge}", &usb.chip_name),
                                ("{family}", "{family}", &family),
                            ],
                        ));
                    }
                }
            }
        }

        candidate.score = score.min(0.99) as f32;
        result.candidates.push(candidate);
    }

    // 按评分排序
    result.candidates.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    });

    // ===== 5. 选择最佳候选 =====
    if let Some(best) = result.candidates.first() {
        result.vendor = best.vendor.clone();
        result.chip_family = best.chip_family.clone();
        result.confidence = best.score;

        let mut evidence_parts: Vec<String> = Vec::new();
        evidence_parts.push(crate::i18n::tf("Overall score {pct}%", &[
            ("{pct}", "{pct}", &format!("{:.0}", best.score * 100.0)),
        ]));
        if result.sample_count > 0 {
            evidence_parts.push(crate::i18n::tf("Frame interval {avg}±{jitter}ms", &[
                ("{avg}", "{avg}", &format!("{:.1}", result.avg_interval_ms)),
                ("{jitter}", "{jitter}", &format!("{:.1}", result.jitter_ms)),
            ]));
        }
        evidence_parts.push(crate::i18n::tf("Header match {pct}%", &[
            ("{pct}", "{pct}", &format!("{:.0}", result.header_match_rate * 100.0)),
        ]));
        let clock_str = if best.example_model.contains("F407") { "84" }
            else if best.example_model.contains("ESP32") { "40" }
            else if best.example_model.contains("GD32") { "108" }
            else if best.example_model.contains("STM8") { "16" }
            else { "72" };
        evidence_parts.push(crate::i18n::tf("{mhz}MHz divider error {err}%", &[
            ("{mhz}", "{mhz}", clock_str),
            ("{err}", "{err}", &format!("{:.2}", calc_baud_error(
                if best.example_model.contains("F407") { 84.0 }
                else if best.example_model.contains("ESP32") { 40.0 }
                else if best.example_model.contains("GD32") { 108.0 }
                else if best.example_model.contains("STM8") { 16.0 }
                else if best.example_model.contains("LPC1768") { 100.0 }
                else if best.example_model.contains("LPC1114") { 48.0 }
                else if best.example_model.contains("F030") { 48.0 }
                else if best.example_model.contains("L432") { 80.0 }
                else if best.example_model.contains("V203") { 144.0 }
                else if best.example_model.contains("S3") { 40.0 }
                else if best.example_model.contains("8266") { 40.0 }
                else if best.example_model.contains("F305") { 108.0 }
                else if best.example_model.contains("S207") { 16.0 }
                else { 72.0 },
                baud_rate
            ))),
        ]));
        if !best.match_reasons.is_empty() {
            evidence_parts.push(best.match_reasons[0].clone());
        }
        result.evidence = evidence_parts.join(crate::i18n::t("; "));
    } else {
        result.vendor = crate::i18n::t("Unknown vendor").to_string();
        result.chip_family = crate::i18n::t("Unknown family").to_string();
        result.confidence = 0.0;
        result.evidence = crate::i18n::t("No candidate MCU").to_string();
    }

    result
}

// ===== 深度总线探测渲染 =====

/// 渲染深度总线探测区
fn render_deep_probe_section(ui: &mut egui::Ui, ctx: &egui::Context, state: &mut AppState, zh: bool, connected: bool, dp: &mut DeepProbeState, chip_detect: &ChipDetectState, cortex_m3_probe: &mut CortexM3ProbeState, mcu_probe: &mut McuProbeState) {
    ui.label(
        RichText::new(crate::i18n::t("Deep Bus Probe"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("Multi-bus deep scan: Modbus slave/register scan, UART protocol analysis, I2C/SPI/JTAG/SWD/1-Wire reference"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(6.0);

    // ===== 探测类型选择按钮组 =====
    ui.horizontal_wrapped(|ui| {
            let types: [(DeepScanType, &str); 6] = [
                (DeepScanType::ModbusSlave, crate::i18n::t("Modbus Slave Scan")),
                (DeepScanType::ModbusRegister, crate::i18n::t("Modbus Reg Scan")),
                (DeepScanType::UartAnalyze, crate::i18n::t("UART Protocol Analysis")),
                (DeepScanType::I2cReference, crate::i18n::t("I2C Reference")),
                (DeepScanType::SpiReference, crate::i18n::t("SPI Flash Ref")),
                (DeepScanType::JtagReference, crate::i18n::t("JTAG/SWD Ref")),
            ];
            for (st, label) in types {
                let selected = dp.scan_type == st;
                let btn = if selected {
                    egui::Button::new(RichText::new(label).color(Color32::BLACK).strong())
                        .fill(theme::ACCENT)
                        .min_size(egui::Vec2::new(125.0, 24.0))
                } else {
                    egui::Button::new(RichText::new(label).color(theme::TEXT_PRIMARY))
                        .min_size(egui::Vec2::new(125.0, 24.0))
                };
                if ui.add(btn).clicked() && !dp.running {
                    dp.scan_type = st;
                }
            }
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        match dp.scan_type {
            DeepScanType::ModbusSlave
            | DeepScanType::ModbusRegister
            | DeepScanType::UartAnalyze => {
                render_deep_probe_scan(ui, zh, connected, dp, mcu_probe);
            }
            DeepScanType::I2cReference => {
                render_i2c_reference(ui, zh, dp, chip_detect);
            }
            DeepScanType::SpiReference => {
                render_spi_reference(ui, zh, dp);
            }
            DeepScanType::JtagReference => {
                render_jtag_reference(ui, ctx, state, zh, dp, cortex_m3_probe);
            }
        }
}

/// 渲染扫描模式（Modbus 从站/寄存器扫描、UART 协议分析）
fn render_deep_probe_scan(ui: &mut egui::Ui, zh: bool, connected: bool, dp: &mut DeepProbeState, mcu_probe: &mut McuProbeState) {
    // ===== 探测参数 =====
    match dp.scan_type {
        DeepScanType::ModbusSlave => {
            ui.label(
                RichText::new(crate::i18n::t("Slave Scan Range"))
                    .color(theme::CYAN)
                    .small()
                    .strong(),
            );
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::t("Start##panels/sensor_dev/protocol_analyzer.rs:183285")).color(theme::TEXT_SECONDARY).small());
                ui.add_enabled(!dp.running, egui::DragValue::new(&mut dp.modbus_start).range(1..=247).speed(0.1));
                ui.label(RichText::new(crate::i18n::t("End")).color(theme::TEXT_SECONDARY).small());
                ui.add_enabled(!dp.running, egui::DragValue::new(&mut dp.modbus_end).range(1..=247).speed(0.1));
            });
        }
        DeepScanType::ModbusRegister => {
            ui.label(
                RichText::new(crate::i18n::t("Register Scan Params"))
                    .color(theme::CYAN)
                    .small()
                    .strong(),
            );
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::t("Slave")).color(theme::TEXT_SECONDARY).small());
                ui.add_enabled(!dp.running, egui::DragValue::new(&mut dp.modbus_slave_id).range(1..=247).speed(0.1));
                ui.label(RichText::new(crate::i18n::t("Start##panels/sensor_dev/protocol_analyzer.rs:184254")).color(theme::TEXT_SECONDARY).small());
                ui.add_enabled(!dp.running, egui::DragValue::new(&mut dp.modbus_reg_start).range(0..=65535).speed(0.1));
                ui.label(RichText::new(crate::i18n::t("End")).color(theme::TEXT_SECONDARY).small());
                ui.add_enabled(!dp.running, egui::DragValue::new(&mut dp.modbus_reg_end).range(0..=65535).speed(0.1));
            });
        }
        DeepScanType::UartAnalyze => {
            ui.label(
                RichText::new(crate::i18n::t("Auto-read latest 256 bytes from rx_buffer and analyze protocol"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        }
        _ => {}
    }
    ui.add_space(4.0);

    // 未连接提示
    if !connected {
        ui.horizontal(|ui| {
            ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
            ui.label(
                RichText::new(crate::i18n::t("Not connected. Open a port in the Connection panel first."))
                .color(theme::WARNING)
                .small(),
            );
        });
        return;
    }

    // ===== 开始/停止按钮 =====
    ui.horizontal(|ui| {
        if dp.running {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("⏹ Stop##panels/sensor_dev/protocol_analyzer.rs:185533"))
                    .color(Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ERROR)
            .min_size(egui::Vec2::new(120.0, 28.0));
            if ui.add(btn).clicked() {
                dp.running = false;
                dp.wait_until = None;
                dp.log.push(crate::i18n::t("Scan stopped").to_string());
            }
        } else {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("▶ Start Scan"))
                    .color(Color32::BLACK)
                    .strong(),
            )
            .fill(theme::SUCCESS)
            .min_size(egui::Vec2::new(120.0, 28.0));
            if ui
                .add(btn)
                .on_hover_text(crate::i18n::t("Deep bus probe: Modbus slave/register scan discovers devices and readable registers on the bus; UART protocol analysis auto-detects frame format. Reference modes show chip ID info without connection"))
                .clicked() {
                dp.running = true;
                dp.results.clear();
                dp.log.clear();
                dp.wait_until = None;
                match dp.scan_type {
                    DeepScanType::ModbusSlave => {
                        dp.progress = dp.modbus_start as usize;
                        dp.total = (dp.modbus_end as usize).saturating_sub(dp.modbus_start as usize) + 1;
                        dp.log.push(format!(
                            "{} ({}-{})",
                            crate::i18n::t("Start Modbus slave scan"),
                            dp.modbus_start, dp.modbus_end
                        ));
                    }
                    DeepScanType::ModbusRegister => {
                        dp.progress = dp.modbus_reg_start as usize;
                        dp.total = (dp.modbus_reg_end as usize).saturating_sub(dp.modbus_reg_start as usize) + 1;
                        dp.log.push(format!(
                            "{} ({} #{}, 0x{:04X}-0x{:04X})",
                            crate::i18n::t("Start register scan"),
                            crate::i18n::t("slave"),
                            dp.modbus_slave_id, dp.modbus_reg_start, dp.modbus_reg_end
                        ));
                    }
                    DeepScanType::UartAnalyze => {
                        dp.total = 1;
                        dp.log.push(crate::i18n::t("Start UART protocol analysis").to_string());
                    }
                    _ => {}
                }
            }
        }

        // 清除结果按钮
        if !dp.results.is_empty() && !dp.running {
            if ui.button(crate::i18n::t("Clear##panels/sensor_dev/protocol_analyzer.rs:188214")).clicked() {
                dp.results.clear();
                dp.log.clear();
            }
        }
    });

    // ===== 进度条 + 状态 =====
    if dp.running || !dp.results.is_empty() || !dp.log.is_empty() {
        ui.add_space(4.0);
        let progress = if dp.total > 0 {
            let done = match dp.scan_type {
                DeepScanType::ModbusSlave => {
                    dp.progress.saturating_sub(dp.modbus_start as usize)
                }
                DeepScanType::ModbusRegister => {
                    dp.progress.saturating_sub(dp.modbus_reg_start as usize)
                }
                _ => dp.results.len(),
            };
            (done as f32 / dp.total as f32).min(1.0)
        } else {
            0.0
        };
        ui.add(egui::ProgressBar::new(progress).desired_width(200.0));
        ui.label(
            RichText::new(if dp.running {
                format!(
                    "{} {:.0}%",
                    crate::i18n::t("Scanning..."),
                    progress * 100.0
                )
            } else { crate::i18n::t("Scan complete").to_string() })
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
    }

    // ===== 扫描日志 =====
    if !dp.log.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(crate::i18n::t("Scan Log"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);
        egui::Frame::none()
            .fill(theme::BG_DEEP)
            .inner_margin(egui::Margin::same(6.0))
            .rounding(4.0)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .max_height(120.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for line in dp.log.iter().rev() {
                            let color = if line.contains("✓") {
                                theme::SUCCESS
                            } else if line.contains("✗") {
                                theme::ERROR
                            } else if line.contains("→") {
                                theme::CYAN
                            } else {
                                theme::TEXT_SECONDARY
                            };
                            ui.label(
                                RichText::new(line)
                                    .color(color)
                                    .small()
                                    .family(egui::FontFamily::Monospace),
                            );
                        }
                    });
            });
    }

    // ===== 扫描结果卡片 =====
    if !dp.results.is_empty() {
        ui.add_space(6.0);
        ui.label(
            RichText::new(crate::i18n::t("Scan Results"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);
        for r in dp.results.iter() {
            let (border, bg) = if r.confidence >= 0.8 {
                (theme::SUCCESS, Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
            } else if r.confidence >= 0.5 {
                (theme::WARNING, Color32::from_rgba_premultiplied(0x30, 0x28, 0x10, 0x80))
            } else {
                (theme::TEXT_DISABLED, Color32::from_rgba_premultiplied(0x20, 0x20, 0x20, 0x80))
            };
            egui::Frame::group(ui.style())
                .stroke(egui::Stroke::new(1.0_f32, border))
                .fill(bg)
                .inner_margin(egui::Margin::same(5.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(crate::i18n::t_owned(&r.name))
                                .color(border)
                                .strong()
                                .small(),
                        );
                        ui.label(
                            RichText::new(format!("({:.0}%)", r.confidence * 100.0))
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );
                    });
                    ui.label(
                        RichText::new(&crate::i18n::t_owned(&r.description))
                            .color(theme::TEXT_PRIMARY)
                            .small(),
                    );
                    if !r.details.is_empty() {
                        ui.label(
                            RichText::new(&r.details)
                                .color(theme::TEXT_SECONDARY)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                    }
                    if !r.raw_data.is_empty() {
                        let hex: String = r.raw_data.iter().take(24).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                        let more = if r.raw_data.len() > 24 { "..." } else { "" };
                        ui.label(
                            RichText::new(format!("RX: {}{}", hex, more))
                                .color(theme::CYAN)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                    }

                    // ===== 鞋垫压力传感器专用渲染（仅设备工作区构建） =====
                    // 当协议名为「鞋垫压力传感器」时，展示 ADC 矩阵预览、IMU 四元数和压力统计
                    #[cfg(feature = "device-workspaces")]
                    if r.name == bus_probe::INSOLE_PROTOCOL_NAME {
                        render_insole_protocol_details(ui, zh, &r.raw_data, mcu_probe);
                    }
                });
            ui.add_space(2.0);
        }
    }
}

#[cfg(feature = "device-workspaces")]
include!("protocol_analyzer_device.rs");

/// 渲染 I2C 设备参考表（含 CH341 I2C 扫描命令构建模拟展示）
fn render_i2c_reference(ui: &mut egui::Ui, zh: bool, _dp: &mut DeepProbeState, chip_detect: &ChipDetectState) {
    // ===== CH341 I2C 扫描命令构建展示（模拟） =====
    let usb_is_ch341 = chip_detect
        .usb_bridge
        .as_ref()
        .map(|u| u.chip_name.contains("CH34"))
        .unwrap_or(false);

    if usb_is_ch341 {
        ui.label(
            RichText::new(crate::i18n::t("CH341 I2C Scan Command (Simulation)"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new(crate::i18n::t("CH341/CH340 bridge detected. I2C scan command construction shown below (USB direct access requires special permissions, simulation only)"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        ui.add_space(4.0);

        egui::Frame::none()
            .fill(theme::BG_DEEP)
            .inner_margin(egui::Margin::same(6.0))
            .rounding(4.0)
            .show(ui, |ui| {
                // 展示几个典型地址的扫描命令
                let sample_addrs: [u8; 4] = [0x20, 0x50, 0x68, 0x76];
                for addr in sample_addrs {
                    let cmd = bus_probe::ch341::build_i2c_scan_cmd(addr);
                    let hex: String = cmd.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                    let devices = bus_probe::lookup_i2c_device(addr);
                    let dev_name = if devices.is_empty() {
                        crate::i18n::t(" (unknown)")
                    } else {
                        devices[0].chip_name
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("0x{:02X}", addr))
                                .color(theme::ACCENT)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(hex)
                                .color(theme::TEXT_PRIMARY)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(dev_name)
                                .color(theme::CYAN)
                                .small(),
                        );
                    });
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new(crate::i18n::t("Scan range: 0x03-0x77 (valid 7-bit I2C addresses), send START+write_addr+STOP per address, detect ACK"))
                    .color(theme::TEXT_DISABLED)
                    .small(),
                );
            });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
    } else {
        ui.label(
            RichText::new(crate::i18n::t("Tip: I2C scan command simulation shown when CH341/CH340 bridge detected. Current bridge does not support USB-direct I2C."))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
        ui.add_space(4.0);
    }

    // ===== I2C 设备识别参考表 =====
    ui.label(
        RichText::new(crate::i18n::t("I2C Device Reference Table"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);

    ScrollArea::vertical()
        .max_height(300.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("deep_i2c_ref_grid")
                .num_columns(4)
                .spacing([10.0, 3.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new(crate::i18n::t("Addr")).color(theme::TEXT_SECONDARY).small().strong());
                    ui.label(RichText::new(crate::i18n::t("Chip")).color(theme::TEXT_SECONDARY).small().strong());
                    ui.label(RichText::new(crate::i18n::t("Desc")).color(theme::TEXT_SECONDARY).small().strong());
                    ui.label(RichText::new(crate::i18n::t("Cat")).color(theme::TEXT_SECONDARY).small().strong());
                    ui.end_row();

                    // 遍历有效 I2C 地址范围，显示已知设备
                    for addr in 0x03..=0x77u8 {
                        let devices = bus_probe::lookup_i2c_device(addr);
                        for dev in &devices {
                            ui.label(
                                RichText::new(format!("0x{:02X}", addr))
                                    .color(theme::ACCENT)
                                    .small()
                                    .family(egui::FontFamily::Monospace),
                            );
                            ui.label(
                                RichText::new(dev.chip_name)
                                    .color(theme::TEXT_PRIMARY)
                                    .small(),
                            );
                            ui.label(
                                RichText::new(crate::i18n::t_owned(&dev.description))
                                    .color(theme::TEXT_SECONDARY)
                                    .small(),
                            );
                            ui.label(
                                RichText::new(i2c_category_label(dev.category, zh))
                                    .color(theme::CYAN)
                                    .small(),
                            );
                            ui.end_row();
                        }
                    }
                });
        });
}

/// 渲染 SPI Flash 参考表（含 JEDEC ID 手动识别）
fn render_spi_reference(ui: &mut egui::Ui, _zh: bool, dp: &mut DeepProbeState) {
    // ===== SPI Flash JEDEC ID 读取命令展示 =====
    ui.label(
        RichText::new(crate::i18n::t("SPI Flash JEDEC ID Read Command"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("Send 0x9F command to read 3-byte JEDEC ID (manufacturer + capacity + type). USB direct access requires special permissions"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);

    // CH341 SPI JEDEC 命令
    let ch341_cmd = bus_probe::ch341::build_spi_jedec_cmd();
    let ch341_hex: String = ch341_cmd.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
    // MPSSE SPI JEDEC 命令序列
    let mpsse_cmds = bus_probe::mpsse::build_spi_jedec_cmds();

    egui::Frame::none()
        .fill(theme::BG_DEEP)
        .inner_margin(egui::Margin::same(6.0))
        .rounding(4.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("CH341 cmd:"))
                    .color(theme::ACCENT)
                    .small(),
            );
            ui.label(
                RichText::new(&ch341_hex)
                    .color(theme::TEXT_PRIMARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(crate::i18n::t("MPSSE (FT232H) cmds:"))
                    .color(theme::ACCENT)
                    .small(),
            );
            for (i, cmd) in mpsse_cmds.iter().enumerate() {
                let hex: String = cmd.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                ui.label(
                    RichText::new(format!("  [{}] {}", i + 1, hex))
                        .color(theme::TEXT_PRIMARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
            }
        });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // ===== 手动输入 JEDEC ID 进行识别 =====
    ui.label(
        RichText::new(crate::i18n::t("Manual SPI Flash ID"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("JEDEC ID (MFG CAP):"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut dp.spi_jedec_input)
                .desired_width(120.0)
                .font(egui::FontId::monospace(12.0)),
        );
    });

    // 显示识别结果
    if let Some((mfg_id, cap_id)) = parse_jedec_input(&dp.spi_jedec_input) {
        let vendor = bus_probe::spi_flash_vendor_name(mfg_id);
        ui.add_space(4.0);
        if let Some(info) = bus_probe::lookup_spi_flash(mfg_id, cap_id) {
            egui::Frame::group(ui.style())
                .stroke(egui::Stroke::new(1.5_f32, theme::SUCCESS))
                .fill(Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(info.part_number.to_string()).color(theme::SUCCESS).strong().size(15.0));
                        ui.label(RichText::new(format!("({} {}Mbit {})", info.manufacturer, info.capacity_mbit, crate::i18n::t_owned(info.voltage.name()))).color(theme::TEXT_SECONDARY).small());
                    });
                    ui.label(
                        RichText::new(format!(
                            "JEDEC: 0x{:02X}{:04X}  |  {}  |  {}  |  {}Mbit  |  {}",
                            mfg_id, cap_id, info.manufacturer, info.part_number, info.capacity_mbit, info.voltage.range_str()
                        ))
                        .color(theme::TEXT_PRIMARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                    );
                    // 1.8V 芯片电压警告
                    if info.voltage == miconlab_core::bus_probe::SpiFlashVoltage::V1_8 {
                        ui.label(RichText::new(crate::i18n::t("⚠ 1.8V low-voltage chip; CH341A/FT232H outputs 3.3V, level shifting required!")).color(theme::WARNING).small());
                    }
                });
        } else {
            egui::Frame::group(ui.style())
                .stroke(egui::Stroke::new(1.0_f32, theme::WARNING))
                .fill(Color32::from_rgba_premultiplied(0x30, 0x28, 0x10, 0x80))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::tf("No match in database (MFG=0x{:02X} {}, CAP=0x{:04X})", &[("{:02X}", "{:02X}", &format!("{:02X}", mfg_id)), ("{}", "{}", &format!("{}", vendor)), ("{:04X}", "{:04X}", &format!("{:04X}", cap_id))]))
                        .color(theme::WARNING)
                        .small(),
                    );
                });
        }
    } else if !dp.spi_jedec_input.trim().is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("Invalid format, use: MFG CAP (e.g. EF 4017)"))
                .color(theme::ERROR)
                .small(),
        );
    }
    ui.add_space(6.0);

    // ===== SPI Flash 厂商参考 =====
    ui.label(
        RichText::new(crate::i18n::t("SPI Flash Vendor Reference"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    egui::Grid::new("deep_spi_vendor_grid")
        .num_columns(2)
        .spacing([10.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::t("MFG ID")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Vendor")).color(theme::TEXT_SECONDARY).small().strong());
            ui.end_row();
            for &mfg_id in &[0xEFu8, 0xC2, 0x20, 0x01, 0xC8, 0x9D, 0x1F, 0x1C, 0x85, 0xBF] {
                ui.label(
                    RichText::new(format!("0x{:02X}", mfg_id))
                        .color(theme::ACCENT)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    RichText::new(bus_probe::spi_flash_vendor_name(mfg_id))
                        .color(theme::TEXT_PRIMARY)
                        .small(),
                );
                ui.end_row();
            }
        });
}

/// 渲染 JTAG/SWD/1-Wire 参考（详细解读 + 深层通讯入口 + 固件读取/刷写跳转）
fn render_jtag_reference(ui: &mut egui::Ui, ctx: &egui::Context, state: &mut AppState, zh: bool, dp: &mut DeepProbeState, cortex_m3_probe: &mut CortexM3ProbeState) {
    // ===== JTAG/SWD IDCODE 深度识别 =====
    ui.label(
        RichText::new(crate::i18n::t("JTAG/SWD IDCODE Deep Identification"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("IDCODE (hex):"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut dp.jtag_idcode_input)
                .desired_width(140.0)
                .font(egui::FontId::monospace(12.0)),
        );
        if !dp.jtag_idcode_input.trim().is_empty() {
            ui.label(
                RichText::new(crate::i18n::t("(8-digit hex, e.g. 0BA9542F)"))
                    .color(theme::TEXT_DISABLED)
                    .small(),
            );
        }
    });

    if let Some(idcode) = parse_u32_hex(&dp.jtag_idcode_input) {
        ui.add_space(6.0);

        // ===== 字段级详细解读 =====
        let fields = bus_probe::parse_idcode(idcode);
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT))
            .fill(Color32::from_rgba_premultiplied(0x20, 0x18, 0x00, 0x60))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Field-level Analysis (IEEE 1149.1 JTAG)"))
                        .color(theme::ACCENT)
                        .small()
                        .strong(),
                );
                ui.add_space(2.0);

                egui::Grid::new("jtag_idcode_fields_grid")
                    .num_columns(4)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new(crate::i18n::t("Field")).color(theme::TEXT_SECONDARY).small().strong());
                        ui.label(RichText::new(crate::i18n::t("Bits")).color(theme::TEXT_SECONDARY).small().strong());
                        ui.label(RichText::new(crate::i18n::t("Value")).color(theme::TEXT_SECONDARY).small().strong());
                        ui.label(RichText::new(crate::i18n::t("Meaning")).color(theme::TEXT_SECONDARY).small().strong());
                        ui.end_row();

                        // bit[0] — 固定值
                        ui.label(RichText::new(crate::i18n::t("Fixed")).color(theme::TEXT_PRIMARY).small());
                        ui.label(RichText::new("[0]").color(theme::TEXT_DISABLED).small().monospace());
                        ui.label(RichText::new(format!("{}", fields.bit0)).color(theme::CYAN).small().monospace());
                        let bit0_ok = fields.bit0 == 1;
                        ui.label(RichText::new(crate::i18n::tf("Must be 1 (IDCODE register marker) — {status}", &[
                            ("{status}", "{status}", if bit0_ok { crate::i18n::t("✓ valid") } else { crate::i18n::t("✗ not valid") }),
                        ])).color(if bit0_ok { theme::SUCCESS } else { theme::ERROR }).small());
                        ui.end_row();

                        // Manufacturer ID [11:1]
                        ui.label(RichText::new(crate::i18n::t("Manufacturer")).color(theme::TEXT_PRIMARY).small());
                        ui.label(RichText::new("[11:1]").color(theme::TEXT_DISABLED).small().monospace());
                        ui.label(RichText::new(format!("0x{:03X}", fields.manufacturer_id)).color(theme::CYAN).small().monospace());
                        ui.label(RichText::new(crate::i18n::tf("{name} (JEP106 code, 11-bit)", &[
                            ("{name}", "{name}", &fields.manufacturer_name),
                        ])).color(theme::TEXT_PRIMARY).small());
                        ui.end_row();

                        // Part Number [27:12]
                        ui.label(RichText::new(crate::i18n::t("Part Number")).color(theme::TEXT_PRIMARY).small());
                        ui.label(RichText::new("[27:12]").color(theme::TEXT_DISABLED).small().monospace());
                        ui.label(RichText::new(format!("0x{:04X}", fields.part_number)).color(theme::CYAN).small().monospace());
                        ui.label(RichText::new(crate::i18n::t("Vendor-defined part number (16-bit)")).color(theme::TEXT_PRIMARY).small());
                        ui.end_row();

                        // Version [31:28]
                        ui.label(RichText::new(crate::i18n::t("Version")).color(theme::TEXT_PRIMARY).small());
                        ui.label(RichText::new("[31:28]").color(theme::TEXT_DISABLED).small().monospace());
                        ui.label(RichText::new(format!("0x{:X}", fields.version)).color(theme::CYAN).small().monospace());
                        ui.label(RichText::new(crate::i18n::t("Chip version/revision (vendor-defined)")).color(theme::TEXT_PRIMARY).small());
                        ui.end_row();
                    });

                ui.add_space(2.0);
                ui.label(
                    RichText::new(crate::i18n::tf("Raw: 0x{raw:08X}  (bin: {bin:032b})", &[
                        ("{raw:08X}", "{raw:08X}", &format!("{:08X}", fields.raw)),
                        ("{bin:032b}", "{bin:032b}", &format!("{:032b}", fields.raw)),
                    ]))
                    .color(theme::TEXT_DISABLED)
                    .small()
                    .monospace(),
                );
            });

        ui.add_space(6.0);

        // ===== 识别结果（JTAG + SWD 双重识别）=====
        let jtag_info = bus_probe::lookup_jtag_chip(idcode);
        let swd_info = bus_probe::swd::identify_swd_idcode(idcode);

        if jtag_info.is_some() || swd_info.is_some() {
            egui::Frame::group(ui.style())
                .stroke(egui::Stroke::new(1.5_f32, theme::SUCCESS))
                .fill(Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Identification Result"))
                            .color(theme::SUCCESS)
                            .small()
                            .strong(),
                    );
                    ui.add_space(2.0);

                    if let Some(info) = &jtag_info {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("JTAG:").color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(info.part_number).color(theme::SUCCESS).strong().size(15.0));
                            ui.label(RichText::new(format!("({})", info.manufacturer)).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(format!("— {}", info.core)).color(theme::CYAN).small());
                        });
                    }

                    if let Some((mfg, part)) = &swd_info {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("SWD:").color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(*part).color(theme::CYAN).small().strong());
                            ui.label(RichText::new(format!("({})", mfg)).color(theme::TEXT_SECONDARY).small());
                        });
                    }
                });
        } else {
            ui.label(
                RichText::new(crate::i18n::tf("⚠ Unknown IDCODE 0x{:08X} (fields parsed, but no match in database)", &[("{:08X}", "{:08X}", &format!("{:08X}", idcode))]))
                .color(theme::WARNING)
                .small(),
            );
        }

        ui.add_space(6.0);

        // ===== 芯片详细技术信息 + 深层通讯入口 =====
        let chip_detail = jtag_info.as_ref().and_then(|info| {
            bus_probe::lookup_chip_detail(info.part_number)
                .or_else(|| bus_probe::generic_chip_info(info.manufacturer))
        });

        if let Some(detail) = chip_detail {
            egui::Frame::group(ui.style())
                .stroke(egui::Stroke::new(1.0_f32, theme::CYAN))
                .fill(Color32::from_rgba_premultiplied(0x10, 0x20, 0x30, 0x80))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Chip Detailed Technical Info"))
                            .color(theme::CYAN)
                            .small()
                            .strong(),
                    );
                    ui.add_space(2.0);

                    egui::Grid::new("jtag_chip_detail_grid")
                        .num_columns(2)
                        .spacing([10.0, 2.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new(crate::i18n::t("Chip")).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.chip_name).color(theme::TEXT_PRIMARY).small().strong());
                            ui.end_row();

                            ui.label(RichText::new(crate::i18n::t("Manufacturer##panels/sensor_dev/protocol_analyzer.rs:237176")).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.manufacturer).color(theme::TEXT_PRIMARY).small());
                            ui.end_row();

                            ui.label(RichText::new(crate::i18n::t("Architecture")).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.architecture).color(theme::CYAN).small());
                            ui.end_row();

                            ui.label(RichText::new(crate::i18n::t("Debug Iface")).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.debug_interface).color(theme::TEXT_PRIMARY).small());
                            ui.end_row();

                            ui.label(RichText::new("Flash").color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(format!(
                                "0x{:08X}  {}",
                                detail.flash_base,
                                if detail.flash_size_kb > 0 {
                                    format!("({} KB)", detail.flash_size_kb)
                                } else { crate::i18n::t("(external/unknown)").to_string() }
                            )).color(theme::ACCENT).small().monospace());
                            ui.end_row();

                            ui.label(RichText::new("RAM").color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(format!(
                                "0x{:08X}  {}",
                                detail.ram_base,
                                if detail.ram_size_kb > 0 {
                                    format!("({} KB)", detail.ram_size_kb)
                                } else { crate::i18n::t("(unknown)").to_string() }
                            )).color(theme::ACCENT).small().monospace());
                            ui.end_row();

                            ui.label(RichText::new(crate::i18n::t("Flash Protocol")).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.flash_protocol).color(theme::TEXT_PRIMARY).small());
                            ui.end_row();

                            ui.label(RichText::new("Bootloader").color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.bootloader).color(theme::TEXT_PRIMARY).small());
                            ui.end_row();

                            ui.label(RichText::new(crate::i18n::t("Features")).color(theme::TEXT_SECONDARY).small());
                            ui.label(RichText::new(detail.features).color(theme::TEXT_PRIMARY).small());
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // ===== 深层通讯入口 =====
                    ui.label(
                        RichText::new(crate::i18n::t("Deep Communication Entry"))
                            .color(theme::ACCENT)
                            .small()
                            .strong(),
                    );
                    ui.add_space(2.0);

                    let (recommendation, hint_str, show_jump) = match detail.flasher_target {
                        bus_probe::FlasherTarget::Stm32Bootloader => (
                            { crate::i18n::t("Recommended: Read/erase/flash via UART Bootloader (0x7F wakeup)\n  1. Set BOOT0=H, BOOT1=L, reset to enter System Bootloader\n  2. Select STM32 in Flasher, send 0x7F\n  3. Read (backup) / erase / write firmware") },
                            "stm32",
                            true,
                        ),
                        bus_probe::FlasherTarget::Esp32RomBootloader => (
                            { crate::i18n::t("Recommended: Read/erase/flash via ROM Bootloader (SLIP SYNC)\n  1. Hold GPIO0 low and reset (or hold BOOT) to enter download mode\n  2. Select ESP32 in Flasher, send SYNC\n  3. Read SPI Flash / erase / write / MD5 verify") },
                            "esp32",
                            true,
                        ),
                        bus_probe::FlasherTarget::Esp32AtFirmware => (
                            crate::i18n::t("Recommended: Read firmware version / flash AT firmware via AT commands"),
                            "esp32",
                            true,
                        ),
                        bus_probe::FlasherTarget::ArduinoStk500 => (
                            crate::i18n::t("Recommended: Read/flash via STK500 v1 / Optiboot protocol (0x30 0x20)"),
                            "arduino",
                            true,
                        ),
                        bus_probe::FlasherTarget::GenericUart => (
                            crate::i18n::t("Recommended: Read/flash via generic UART (vendor-specific protocol)"),
                            "stm32",
                            true,
                        ),
                        bus_probe::FlasherTarget::JtagDirect => (
                            { crate::i18n::t("Recommended: Download bitstream / read config Flash via JTAG\n  Note: Requires dedicated JTAG adapter (e.g. FT2232H), miconlab currently supports UART only") },
                            "",
                            false,
                        ),
                        bus_probe::FlasherTarget::SwdDirect => (
                            { crate::i18n::t("Recommended: Read/write Flash via SWD\n\n⚠ Important:\n  • Identified as generic ARM Cortex-M3 core (manufacturer: ARM, not vendor-specific)\n  • SWD requires adapter (ST-Link, J-Link), miconlab currently supports UART only\n  • Generic Cortex-M3 could be STM32 / GD32 / CH32 / LPC1768 from any vendor\n  • Click button below to confirm chip model, then jump to corresponding flasher\n  • If chip supports UART Bootloader (e.g. STM32/GD32/CH32), switch to UART mode") },
                            "cortex_m3_confirm",
                            true,
                        ),
                        bus_probe::FlasherTarget::Esp8266RomBootloader => (
                            { crate::i18n::t("Recommended: Read/erase/flash via ESP8266 ROM Bootloader\n  1. Hold GPIO0 low and reset to enter download mode\n  2. Select ESP8266 in Flasher, click Sync\n  3. Read Flash / erase / write firmware") },
                            "esp8266",
                            true,
                        ),
                        bus_probe::FlasherTarget::Stm8Bootloader => (
                            { crate::i18n::t("Recommended: Read/erase/flash via STM8 UART Bootloader (UM0560)\n  1. Pull SWIM low to enter UART Bootloader\n  2. Select STM8 in Flasher, send 0x7F\n  3. Read / erase / write firmware") },
                            "stm8",
                            true,
                        ),
                        bus_probe::FlasherTarget::NxpLpcIsp => (
                            { crate::i18n::t("Recommended: Read/erase/flash via NXP LPC ISP (UM10295)\n  1. Hold P0.14 low on reset to enter ISP mode\n  2. Select NXP LPC in Flasher, click Sync (?)\n  3. Unlock → Prepare sectors → Erase / Write / Read") },
                            "lpc",
                            true,
                        ),
                        bus_probe::FlasherTarget::Gd32Bootloader => (
                            { crate::i18n::t("Recommended: GD32 is STM32-compatible, reuse STM32 flow\n  1. Set BOOT0=H, BOOT1=L, reset to enter Bootloader\n  2. Select GD32 in Flasher, send 0x7F\n  3. Read / erase / write firmware") },
                            "gd32",
                            true,
                        ),
                        bus_probe::FlasherTarget::Ch32Isp => (
                            { crate::i18n::t("Recommended: Read/erase/flash via WCH ISP protocol (0x57 0xAB)\n  1. CH32 uses WCH ISP protocol over USB/UART\n  2. Select CH32 in Flasher, click Identify\n  3. Read Chip ID → Erase / Write / Verify / Reset") },
                            "ch32",
                            true,
                        ),
                        bus_probe::FlasherTarget::PicIcsp => (
                            { crate::i18n::t("Recommended: PIC ICSP requires PicKit/ICD programmer\n  Note: UART cannot drive ICSP timing (PGC/PGD)\n  This panel shows command sequence only, use MPLAB IPE for real flashing") },
                            "pic",
                            true,
                        ),
                        bus_probe::FlasherTarget::SdCardImage => (
                            { crate::i18n::t("Recommended: Use SD Card Image Flasher (RPi/Jetson Nano)\n  1. Select SBC SD Card Image in Flasher\n  2. Scan Disks → select SD device → select image file\n  3. Unmount → Flash Image → Eject") },
                            "sdcard",
                            true,
                        ),
                        bus_probe::FlasherTarget::NotSupported => (
                            crate::i18n::t("Auto-flashing not supported, refer to chip datasheet"),
                            "",
                            false,
                        ),
                    };

                    ui.label(
                        RichText::new(recommendation)
                            .color(theme::TEXT_PRIMARY)
                            .small(),
                    );

                    if show_jump {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let chip_short = detail.chip_name.split_whitespace().next().unwrap_or(detail.chip_name);
                            let jump_label = { crate::i18n::tf("→ Go to Flasher ({})", &[("{}", "{}", &format!("{}", chip_short))]) };
                            if ui.add(
                                egui::Button::new(RichText::new(jump_label).color(Color32::BLACK).strong())
                                    .fill(theme::ACCENT)
                                    .min_size(egui::Vec2::new(180.0, 28.0))
                            ).clicked() {
                                if hint_str == "cortex_m3_confirm" {
                                    // 通用 Cortex-M3：先尝试自动探测，失败回退手动对话框
                                    let bridge_check = check_bridge_support(state, zh);
                                    if bridge_check.0 && state.is_connected() {
                                        // 已连接且桥接器支持：启动自动探测
                                        {
                                            let p = &mut *cortex_m3_probe;
                                            p.running = true;
                                            p.probe_index = 0;
                                            p.wait_until = None;
                                            p.results.clear();
                                            p.best_hint = None;
                                            p.action = "jump".to_string();
                                            p.log.clear();
                                            p.log.push(crate::i18n::t("=== Start Cortex-M3 vendor auto-probe ===").to_string());
                                            p.log.push(bridge_check.1.clone());
                                        }
                                    } else {
                                        // 未连接或不支持：直接打开手动选择对话框
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_open"), true));
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_action"), "jump".to_string()));
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_bridge_check"), bridge_check));
                                    }
                                } else {
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_chip_hint"),
                                        hint_str.to_string(),
                                    ));
                                    state.active_panel = PanelKind::Flasher;
                                }
                            }

                            if ui.add(
                                egui::Button::new(RichText::new(crate::i18n::t("Read Firmware (Backup)")).color(theme::TEXT_PRIMARY))
                                    .min_size(egui::Vec2::new(150.0, 28.0))
                            ).clicked() {
                                if hint_str == "cortex_m3_confirm" {
                                    // 通用 Cortex-M3：先尝试自动探测，失败回退手动对话框
                                    let bridge_check = check_bridge_support(state, zh);
                                    if bridge_check.0 && state.is_connected() {
                                        {
                                            let p = &mut *cortex_m3_probe;
                                            p.running = true;
                                            p.probe_index = 0;
                                            p.wait_until = None;
                                            p.results.clear();
                                            p.best_hint = None;
                                            p.action = "read".to_string();
                                            p.log.clear();
                                            p.log.push(crate::i18n::t("=== Start Cortex-M3 vendor auto-probe ===").to_string());
                                            p.log.push(bridge_check.1.clone());
                                        }
                                    } else {
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_open"), true));
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_action"), "read".to_string()));
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_bridge_check"), bridge_check));
                                    }
                                } else {
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_chip_hint"),
                                        hint_str.to_string(),
                                    ));
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_action_hint"),
                                        "read".to_string(),
                                    ));
                                    state.active_panel = PanelKind::Flasher;
                                }
                            }

                            if ui.add(
                                egui::Button::new(RichText::new(crate::i18n::t("Flash Firmware")).color(theme::TEXT_PRIMARY))
                                    .min_size(egui::Vec2::new(120.0, 28.0))
                            ).clicked() {
                                if hint_str == "cortex_m3_confirm" {
                                    // 通用 Cortex-M3：先尝试自动探测，失败回退手动对话框
                                    let bridge_check = check_bridge_support(state, zh);
                                    if bridge_check.0 && state.is_connected() {
                                        {
                                            let p = &mut *cortex_m3_probe;
                                            p.running = true;
                                            p.probe_index = 0;
                                            p.wait_until = None;
                                            p.results.clear();
                                            p.best_hint = None;
                                            p.action = "write".to_string();
                                            p.log.clear();
                                            p.log.push(crate::i18n::t("=== Start Cortex-M3 vendor auto-probe ===").to_string());
                                            p.log.push(bridge_check.1.clone());
                                        }
                                    } else {
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_open"), true));
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_action"), "write".to_string()));
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_bridge_check"), bridge_check));
                                    }
                                } else {
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_chip_hint"),
                                        hint_str.to_string(),
                                    ));
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_action_hint"),
                                        "write".to_string(),
                                    ));
                                    state.active_panel = PanelKind::Flasher;
                                }
                            }
                        });
                    }
                });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
    } else if !dp.jtag_idcode_input.trim().is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("Invalid format, enter 8-digit hex (e.g. 0BA9542F)"))
                .color(theme::ERROR)
                .small(),
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
    }

    // ===== SWD 补充说明 =====
    ui.label(
        RichText::new(crate::i18n::t("SWD Notes"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("SWD (Serial Wire Debug) uses the same IDCODE format as JTAG.\nRead DPIDR register via SWD protocol to identify chip.\nARM Cortex-M Part Number starts with 0xBA: 0xBA95=M3, 0xBA01=M0+, 0xBA04=M4, 0xBA07=M7"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // ===== 1-Wire 设备识别 =====
    ui.label(
        RichText::new(crate::i18n::t("1-Wire Family Code Identification"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Family (hex):"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut dp.onewire_family_input)
                .desired_width(60.0)
                .font(egui::FontId::monospace(12.0)),
        );
    });

    if let Some(family) = parse_u8_hex(&dp.onewire_family_input) {
        ui.add_space(4.0);
        if let Some(info) = bus_probe::lookup_onewire_device(family) {
            egui::Frame::group(ui.style())
                .stroke(egui::Stroke::new(1.5_f32, theme::SUCCESS))
                .fill(Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(info.chip_name).color(theme::SUCCESS).strong().size(15.0));
                        ui.label(RichText::new(format!("(0x{:02X})", family)).color(theme::TEXT_SECONDARY).small());
                    });
                    ui.label(
                        RichText::new(info.description)
                            .color(theme::TEXT_PRIMARY)
                            .small(),
                    );
                });
        } else {
            ui.label(
                RichText::new(crate::i18n::tf("Unknown family 0x{:02X}", &[("{:02X}", "{:02X}", &format!("{:02X}", family))]))
                    .color(theme::WARNING)
                    .small(),
            );
        }
    } else if !dp.onewire_family_input.trim().is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("Invalid format, enter hex (e.g. 28)"))
                .color(theme::ERROR)
                .small(),
        );
    }
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    // ===== 支持的芯片参考列表 =====
    ui.label(
        RichText::new(crate::i18n::t("Supported Identification Reference"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);
    egui::Grid::new("deep_jtag_ref_grid")
        .num_columns(3)
        .spacing([12.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::t("Bus")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Supported")).color(theme::TEXT_SECONDARY).small().strong());
            ui.label(RichText::new(crate::i18n::t("Note")).color(theme::TEXT_SECONDARY).small().strong());
            ui.end_row();

            ui.label(RichText::new("JTAG").color(theme::ACCENT).small());
            ui.label(RichText::new(crate::i18n::t("ARM Cortex-M / STM32 / Xilinx / Altera / ESP32 / NXP / TI")).color(theme::TEXT_PRIMARY).small());
            ui.label(RichText::new(crate::i18n::t("Via IDCODE")).color(theme::TEXT_SECONDARY).small());
            ui.end_row();

            ui.label(RichText::new("SWD").color(theme::ACCENT).small());
            ui.label(RichText::new(crate::i18n::t("ARM Cortex-M0/M0+/M3/M4/M7")).color(theme::TEXT_PRIMARY).small());
            ui.label(RichText::new(crate::i18n::t("Via DPIDR")).color(theme::TEXT_SECONDARY).small());
            ui.end_row();

            ui.label(RichText::new("1-Wire").color(theme::ACCENT).small());
            ui.label(RichText::new(crate::i18n::t("DS18B20 / DS18S20 / DS2401 / DS2431 etc.")).color(theme::TEXT_PRIMARY).small());
            ui.label(RichText::new(crate::i18n::t("Via family code")).color(theme::TEXT_SECONDARY).small());
            ui.end_row();

            ui.label(RichText::new("I2C").color(theme::ACCENT).small());
            ui.label(RichText::new(crate::i18n::t("MPU6050 / BME280 / SSD1306 / AT24Cxx / PCF8574 etc.")).color(theme::TEXT_PRIMARY).small());
            ui.label(RichText::new(crate::i18n::t("Via 7-bit address")).color(theme::TEXT_SECONDARY).small());
            ui.end_row();

            ui.label(RichText::new("SPI Flash").color(theme::ACCENT).small());
            ui.label(RichText::new(crate::i18n::t("Winbond / Macronix / GigaDevice / Micron etc.")).color(theme::TEXT_PRIMARY).small());
            ui.label(RichText::new(crate::i18n::t("Via JEDEC ID")).color(theme::TEXT_SECONDARY).small());
            ui.end_row();
        });

    // ===== Cortex-M3 自动探测进度显示和结果处理 =====
    let probe_active = cortex_m3_probe.running;
    let probe_done_with_result = !cortex_m3_probe.running && cortex_m3_probe.best_hint.is_some();
    let probe_done_no_result = {
        // 修复 M3：用 log 是否非空作为判定依据（results 已被清空，但 log 保留供查看）
        !cortex_m3_probe.running && !cortex_m3_probe.log.is_empty() && cortex_m3_probe.best_hint.is_none()
    };

    if probe_active || probe_done_with_result || probe_done_no_result {
        ui.add_space(4.0);
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT))
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                ui.label(RichText::new(crate::i18n::t("Cortex-M3 Vendor Auto-Probe"))
                    .color(theme::ACCENT).strong());
                ui.add_space(4.0);

                let p = &*cortex_m3_probe;

                // 进度
                if p.running {
                    let progress = p.probe_index as f32 / CORTEX_M3_PROBES.len() as f32;
                    ui.label(RichText::new(crate::i18n::tf("Probing... ({}/{})##panels/sensor_dev/protocol_analyzer.rs:259985", &[("{}", "{}", &format!("{}", p.probe_index + 1)), ("{}", "{}", &format!("{}", CORTEX_M3_PROBES.len()))])).color(theme::TEXT_PRIMARY).small());
                    ui.add_space(2.0);
                    ui.add(egui::ProgressBar::new(progress));
                }

                // 日志
                ui.add_space(4.0);
                for line in &p.log {
                    let color = if line.contains('✓') {
                        theme::SUCCESS
                    } else if line.contains('✗') {
                        theme::ERROR
                    } else if line.contains('→') {
                        theme::CYAN
                    } else {
                        theme::TEXT_SECONDARY
                    };
                    ui.label(RichText::new(line).color(color).small().monospace());
                }
            });
        ui.add_space(4.0);
    }

    // 探测成功：自动跳转
    if probe_done_with_result {
        let hint = cortex_m3_probe.best_hint.map(|s| s.to_string());
        let action = cortex_m3_probe.action.clone();
        if let Some(hint_str) = hint {
            // 自动跳转到对应烧录器
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("flasher_chip_hint"), hint_str));
            match action.as_str() {
                "read" => {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("flasher_action_hint"), "read".to_string()));
                }
                "write" => {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("flasher_action_hint"), "write".to_string()));
                }
                _ => {}
            }
            state.active_panel = PanelKind::Flasher;
            // 重置探测状态
            let p = &mut *cortex_m3_probe;
            p.running = false;
            p.best_hint = None;
            p.results.clear();
            p.log.clear();
        }
    }

    // 探测失败：回退到手动选择对话框
    if probe_done_no_result {
        let action = cortex_m3_probe.action.clone();
        // 打开手动选择对话框
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_open"), true));
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_action"), action));
        // 修复 M4：失败消息应使用 ERROR 颜色（bridge_check.0 = false 表示警告/错误）
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_bridge_check"), (false, crate::i18n::t("⚠ Auto-probe failed, please select chip model manually").to_string())));
        // 修复 M3：保留 log 供用户查看，仅重置 running/best_hint/results。
        // log 由对话框关闭时统一清理（避免日志立即消失）。
        let p = &mut *cortex_m3_probe;
        p.running = false;
        p.best_hint = None;
        p.results.clear();
    }

    // ===== 芯片型号确认对话框（用于通用 Cortex-M3 识别后跳转） =====
    let mut dialog_open = ctx.data(|d| d.get_temp::<bool>(egui::Id::new("cortex_m3_dialog_open")).unwrap_or(false));
    if dialog_open {
        let bridge_check = ctx.data(|d|
            d.get_temp::<(bool, String)>(egui::Id::new("cortex_m3_bridge_check"))
                .unwrap_or((true, String::new()))
        );
        let action = ctx.data(|d|
            d.get_temp::<String>(egui::Id::new("cortex_m3_dialog_action"))
                .unwrap_or_else(|| "jump".to_string())
        );

        let mut open = dialog_open;
        let mut should_close = false;
        egui::Window::new(crate::i18n::t("Confirm Chip Model"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .min_width(460.0)
            .show(ctx, |ui| {
                // 1. 桥接器状态
                ui.label(RichText::new(crate::i18n::t("Bridge Status"))
                    .color(theme::ACCENT).strong());
                ui.label(RichText::new(&bridge_check.1)
                    .color(if bridge_check.0 { theme::SUCCESS } else { theme::ERROR })
                    .small());
                ui.add_space(8.0);

                // 2. 说明
                ui.label(RichText::new(crate::i18n::t("Identified as generic ARM Cortex-M3 core (ARM). Please select actual chip model:")).color(theme::TEXT_PRIMARY).small());
                ui.add_space(6.0);

                // 3. 芯片选项
                let action_clone = action.clone();
                let options: &[(&str, &str, &str)] = &[
                    ("STM32F103 (STMicroelectronics)", "stm32", "Flash 0x08000000, UART Bootloader (0x7F wake)"),
                    ("GD32F103 (GigaDevice)", "gd32", "STM32-compatible, Flash 0x08000000"),
                    ("CH32F103 (WCH)", "ch32", "STM32 / WCH ISP (0x57 0xAB) compatible"),
                    ("LPC1768 (NXP)", "lpc", "Cortex-M3, NXP LPC ISP (UM10295)"),
                    ("STM32F4xx (STMicroelectronics)", "stm32", "Cortex-M4, reuses STM32 UART Bootloader"),
                    ("STM32L0/L1/L4 (STMicroelectronics)", "stm32", "Low-power series, reuses STM32 UART Bootloader"),
                ];

                for (name, hint, desc) in options {
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new(*name).color(theme::TEXT_PRIMARY)).clicked() {
                            ctx.data_mut(|d| d.insert_temp(
                                egui::Id::new("flasher_chip_hint"),
                                hint.to_string(),
                            ));
                            // 设置 action hint
                            match action_clone.as_str() {
                                "read" => {
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_action_hint"),
                                        "read".to_string(),
                                    ));
                                }
                                "write" => {
                                    ctx.data_mut(|d| d.insert_temp(
                                        egui::Id::new("flasher_action_hint"),
                                        "write".to_string(),
                                    ));
                                }
                                _ => {}
                            }
                            state.active_panel = PanelKind::Flasher;
                            should_close = true;
                        }
                        ui.label(RichText::new(crate::i18n::t(*desc)).color(theme::TEXT_SECONDARY).small());
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // 4. 取消按钮
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::t("Cancel")).clicked() {
                        should_close = true;
                    }
                });
            });

        if should_close {
            open = false;
        }
        dialog_open = open;
        if !dialog_open {
            ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("cortex_m3_dialog_open")));
            // 对话框关闭时清理探测日志，避免残留状态影响下次探测
            let p = &mut *cortex_m3_probe;
            p.log.clear();
            p.probe_index = 0;
            p.wait_until = None;
            p.rx_start_seq = 0;
        } else {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("cortex_m3_dialog_open"), true));
        }
    }
}

// ===== 渲染函数 =====

/// 渲染协议配置区
fn render_config_section(ui: &mut egui::Ui, s: &mut ProtocolAnalyzerState, _zh: bool) {
    // 自动识别开关
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut s.auto_detect,
            RichText::new(crate::i18n::t("Auto-detect after send"))
            .color(theme::ACCENT)
            .strong(),
        )
        .on_hover_text(crate::i18n::t("Automatically analyze response data after sending to infer the protocol format"));
        if s.auto_detect {
            ui.label(
                RichText::new(crate::i18n::t("(auto-analyze response to infer protocol & frame format)"))
                .color(theme::TEXT_DISABLED)
                .small(),
            );
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label(
        RichText::new(crate::i18n::t("Protocol Config"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("pa_config_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            // 协议类型
            ui.label(
                RichText::new(crate::i18n::t("Type"))
                    .color(theme::TEXT_SECONDARY),
            );
            egui::ComboBox::from_id_salt("pa_proto_type")
                .selected_text(format!("{}", s.protocol_type))
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut s.protocol_type,
                        ProtocolType::AdcDirect,
                        crate::i18n::t("ADC Direct"),
                    );
                    ui.selectable_value(&mut s.protocol_type, ProtocolType::I2c, "I2C");
                    ui.selectable_value(&mut s.protocol_type, ProtocolType::Spi, "SPI");
                    ui.selectable_value(
                        &mut s.protocol_type,
                        ProtocolType::SerialFrame,
                        crate::i18n::t("Serial Frame"),
                    );
                    ui.selectable_value(
                        &mut s.protocol_type,
                        ProtocolType::ModbusRtu,
                        "Modbus RTU",
                    );
                })
                .response
                .on_hover_text(crate::i18n::t("Protocol type: ADC Direct / I2C / SPI / Serial Frame / Modbus RTU"));
            ui.end_row();

            // 波特率
            ui.label(
                RichText::new(crate::i18n::t("Baud"))
                    .color(theme::TEXT_SECONDARY),
            );
            egui::ComboBox::from_id_salt("pa_baud")
                .selected_text(format!("{}", s.baud_rate))
                .width(140.0)
                .show_ui(ui, |ui| {
                    for &b in &[
                        1200u32, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400,
                        460800, 921600,
                    ] {
                        ui.selectable_value(&mut s.baud_rate, b, format!("{}", b));
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("Serial baud rate used for protocol parsing"));
            ui.end_row();

            // 从站 ID（Modbus 用）
            if s.protocol_type == ProtocolType::ModbusRtu {
                ui.label(
                    RichText::new(crate::i18n::t("Slave ID"))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add(
                    egui::DragValue::new(&mut s.slave_id)
                        .range(1..=247)
                        .speed(0.1),
                )
                .on_hover_text(crate::i18n::t("Modbus slave address (1-247)"));
                ui.end_row();
            }
        });

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Frame Format"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    egui::Grid::new("pa_frame_grid")
        .num_columns(4)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            // 帧头
            ui.label(
                RichText::new(crate::i18n::t("Header"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut s.header_hex)
                    .desired_width(80.0)
                    .font(egui::FontId::monospace(12.0)),
            )
            .on_hover_text(crate::i18n::t("Header in HEX (e.g. AA 55); used for frame synchronisation"));
            // 帧尾
            ui.label(
                RichText::new(crate::i18n::t("Footer"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut s.footer_hex)
                    .desired_width(80.0)
                    .font(egui::FontId::monospace(12.0)),
            )
            .on_hover_text(crate::i18n::t("Footer in HEX (e.g. 55 AA); optional"));
            ui.end_row();

            // 长度偏移
            ui.label(
                RichText::new(crate::i18n::t("Len Offset"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut s.length_offset)
                    .range(-1..=255)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Byte offset of the length field in the frame (-1 = no length field)"));
            // 长度字节数
            ui.label(
                RichText::new(crate::i18n::t("Len Bytes"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut s.length_bytes)
                    .range(0..=4)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Number of bytes in the length field (0-4, big-endian)"));
            ui.end_row();

            // 数据偏移
            ui.label(
                RichText::new(crate::i18n::t("Data Offset"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut s.data_offset)
                    .range(0..=255)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Starting byte offset of the data region in the frame"));
            // 通道数
            ui.label(
                RichText::new(crate::i18n::t("Channels##panels/sensor_dev/protocol_analyzer.rs:279475"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut s.channel_count)
                    .range(1..=64)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Number of channels in the data region (1-64)"));
            ui.end_row();

            // 校验类型
            ui.label(
                RichText::new(crate::i18n::t("Checksum"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            egui::ComboBox::from_id_salt("pa_checksum")
                .selected_text(format!("{}", s.checksum_type))
                .width(100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.checksum_type, ChecksumType::None, crate::i18n::t("None"));
                    ui.selectable_value(&mut s.checksum_type, ChecksumType::Sum8, "Sum8");
                    ui.selectable_value(&mut s.checksum_type, ChecksumType::Crc16, "CRC16");
                    ui.selectable_value(&mut s.checksum_type, ChecksumType::Xor, "Xor");
                })
                .response
                .on_hover_text(crate::i18n::t("Checksum algorithm: None / Sum8 / XOR / CRC16-Modbus"));
            // 校验偏移
            ui.label(
                RichText::new(crate::i18n::t("CS Offset"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut s.checksum_offset)
                    .range(-1..=255)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Offset of the checksum field in the frame (-1 = auto-calculate up to before the footer)"));
            ui.end_row();

            // 数据格式
            ui.label(
                RichText::new(crate::i18n::t("Data Format"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            egui::ComboBox::from_id_salt("pa_data_format")
                .selected_text(format!("{}", s.data_format))
                .width(100.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.data_format, DataFormat::U8, "U8");
                    ui.selectable_value(&mut s.data_format, DataFormat::U16BE, "U16BE");
                    ui.selectable_value(&mut s.data_format, DataFormat::U16LE, "U16LE");
                    ui.selectable_value(&mut s.data_format, DataFormat::U32BE, "U32BE");
                    ui.selectable_value(&mut s.data_format, DataFormat::U32LE, "U32LE");
                    ui.selectable_value(&mut s.data_format, DataFormat::F32BE, "F32BE");
                    ui.selectable_value(&mut s.data_format, DataFormat::F32LE, "F32LE");
                })
                .response
                .on_hover_text(crate::i18n::t("Data parsing format: U8 / U16BE / U16LE / U32BE / U32LE / F32BE / F32LE"));
            ui.end_row();
        });
}

/// 渲染发送命令区，返回是否请求发送
fn render_send_section(
    ui: &mut egui::Ui,
    s: &mut ProtocolAnalyzerState,
    _zh: bool,
    connected: bool,
) -> bool {
    ui.label(
        RichText::new(crate::i18n::t("Send Command"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    let mut send_clicked = false;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Cmd(hex)"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut s.command_input)
                .desired_width(260.0)
                .font(egui::FontId::monospace(12.5)),
        )
        .on_hover_text(crate::i18n::t("Command in HEX (e.g. 01 03 00 00 00 01 84 0A)"));

        // 发送按钮（未连接时禁用）
        ui.add_enabled_ui(connected, |ui| {
            let send_btn = egui::Button::new(
                RichText::new(crate::i18n::t("Send"))
                    .color(Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(80.0, 26.0))
            .stroke(egui::Stroke::NONE);
            if ui.add(send_btn)
                .on_hover_text(crate::i18n::t("Parse HEX and send; reads the response after the wait time"))
                .clicked()
            {
                send_clicked = true;
            }
        });

        ui.separator();

        // 保存命令
        ui.label(
            RichText::new(crate::i18n::t("Name"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut s.command_name)
                .desired_width(80.0)
                .hint_text(crate::i18n::t("name##panels/sensor_dev/protocol_analyzer.rs:284280")),
        )
        .on_hover_text(crate::i18n::t("Name label for saving the command"));
        if ui.button(crate::i18n::t("Save"))
            .on_hover_text(crate::i18n::t("Save the current command to the command list"))
            .clicked()
        {
            let bytes = parse_hex(&s.command_input).unwrap_or_default();
            if !bytes.is_empty() {
                let name = if s.command_name.trim().is_empty() {
                    format_hex_spaced(&bytes)
                } else {
                    s.command_name.clone()
                };
                s.saved_commands.push(SavedCommand { name, bytes });
                s.command_name.clear();
            }
        }
    });

    // 等待时间 + 连接状态
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Wait(ms)"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::DragValue::new(&mut s.wait_ms)
                .range(10..=5000)
                .speed(0.5),
        )
        .on_hover_text(crate::i18n::t("Timeout to wait for a response after sending (ms)"));
        ui.add_space(8.0);

        // 连接状态指示
        let (dot, dot_color, status_text) = if connected {
            (
                "\u{25CF}",
                theme::SUCCESS,
                crate::i18n::t("Connected"),
            )
        } else {
            (
                "\u{25CB}",
                theme::TEXT_DISABLED,
                crate::i18n::t("Disconnected"),
            )
        };
        ui.label(
            RichText::new(format!("{} {}", dot, status_text))
                .color(dot_color)
                .small(),
        );
    });

    send_clicked
}

/// 渲染响应解析区
fn render_response_section(ui: &mut egui::Ui, s: &ProtocolAnalyzerState, zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Response"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    if s.last_response.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No response"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    // 原始十六进制
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Raw:"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.label(
            RichText::new(format_hex_spaced(&s.last_response))
                .color(theme::RX_COLOR)
                .family(egui::FontFamily::Monospace)
                .small(),
        );
        ui.label(
            RichText::new(format!("({} bytes)", s.last_response.len()))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
    });

    // 解析结果
    if let Some(parsed) = &s.last_parsed {
        ui.add_space(4.0);
        if parsed.valid {
            ui.label(
                RichText::new(crate::i18n::t("✓ Valid"))
                    .color(theme::SUCCESS)
                    .small(),
            );
            // 通道值
            if !parsed.channel_values.is_empty() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Channels:"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    for (i, v) in parsed.channel_values.iter().enumerate() {
                        ui.label(
                            RichText::new(format!("CH{}={:.3}", i, v))
                                .color(theme::TEXT_PRIMARY)
                                .family(egui::FontFamily::Monospace)
                                .small(),
                        );
                        ui.add_space(4.0);
                    }
                });
            }
        } else {
            ui.label(
                RichText::new(format!(
                    "\u{2717} {}",
                    parsed.error.as_deref().unwrap_or(crate::i18n::t("Parse failed"))
                ))
                .color(theme::ERROR)
                .small(),
            );
        }
    }

    // 帧结构可视化
    ui.add_space(6.0);
    render_frame_visualization(ui, s, zh);
}

/// 渲染帧结构可视化（彩色字节单元格）
fn render_frame_visualization(ui: &mut egui::Ui, s: &ProtocolAnalyzerState, zh: bool) {
    if s.last_response.is_empty() {
        return;
    }

    ui.label(
        RichText::new(crate::i18n::t("Frame Structure"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    let bytes = &s.last_response;
    let ff = match build_frame_format(s) {
        Some(f) => f,
        None => return,
    };

    // 图例
    ui.horizontal(|ui| {
        for role in [
            ByteRole::Header,
            ByteRole::Length,
            ByteRole::Data,
            ByteRole::Checksum,
            ByteRole::Footer,
            ByteRole::Other,
        ] {
            ui.label(RichText::new("\u{25A0}").color(role_color(role)));
            ui.label(
                RichText::new(role_label(role, zh))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add_space(6.0);
        }
    });
    ui.add_space(4.0);

    // 字节单元格
    let cell_w = 26.0_f32;
    let cell_h = 24.0_f32;
    let cells_per_row = 16usize.min(bytes.len().max(1));
    let n = bytes.len();
    let rows = ((n + cells_per_row - 1) / cells_per_row).max(1);

    let total_w = 40.0 + cells_per_row as f32 * cell_w;
    let total_h = rows as f32 * (cell_h + 4.0) + 4.0;

    let (resp, painter) =
        ui.allocate_painter(egui::Vec2::new(total_w, total_h), egui::Sense::hover());
    let rect = resp.rect;

    // 行偏移标签 + 字节单元格
    for row_idx in 0..rows {
        let y = rect.top() + 4.0 + row_idx as f32 * (cell_h + 4.0);
        // 偏移标签
        painter.text(
            egui::Pos2::new(rect.left() + 2.0, y + cell_h * 0.5),
            egui::Align2::LEFT_CENTER,
            format!("{:04X}", row_idx * cells_per_row),
            egui::FontId::new(9.0, egui::FontFamily::Monospace),
            theme::TEXT_DISABLED,
        );

        for col_idx in 0..cells_per_row {
            let i = row_idx * cells_per_row + col_idx;
            if i >= n {
                break;
            }
            let x = rect.left() + 40.0 + col_idx as f32 * cell_w;
            let cell_rect = egui::Rect::from_min_size(
                egui::Pos2::new(x, y),
                egui::Vec2::new(cell_w - 2.0, cell_h),
            );

            let role = byte_role(i, bytes, &ff);
            let color = role_color(role);

            painter.rect_filled(cell_rect, 2.0, color);
            painter.rect_stroke(cell_rect, 2.0, egui::Stroke::new(0.5_f32, theme::BORDER));

            // 字节值
            painter.text(
                cell_rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{:02X}", bytes[i]),
                egui::FontId::new(11.0, egui::FontFamily::Monospace),
                Color32::BLACK,
            );
        }
    }
}

/// 渲染协议自动识别结果区
fn render_detect_section(
    ui: &mut egui::Ui,
    s: &mut ProtocolAnalyzerState,
    _zh: bool,
    apply: &mut bool,
    chip_detect: &ChipDetectState,
) {
    ui.label(
        RichText::new(crate::i18n::t("Detection Results"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(4.0);

    // ===== 芯片方案识别结果 =====
    let chip_result: Option<ChipDetectResult> = chip_detect.best_result.clone();
    let usb_info: Option<UsbBridgeInfo> = chip_detect.usb_bridge.clone();

    // USB 桥接芯片（通过 VID/PID 识别，100% 置信度）
    if let Some(usb) = &usb_info {
        ui.label(
            RichText::new(crate::i18n::t("USB Bridge Chip"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);
        let (border_color, bg_color) = if usb.is_native_mcu {
            (theme::SUCCESS, Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
        } else {
            (theme::ACCENT, Color32::from_rgba_premultiplied(0x18, 0x24, 0x10, 0x80))
        };
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.5_f32, border_color))
            .fill(bg_color)
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&usb.chip_name)
                            .color(border_color)
                            .strong()
                            .size(15.0),
                    );
                    ui.label(
                        RichText::new(format!(
                            "({})",
                            if usb.is_native_mcu {
                                crate::i18n::t("MCU Native")
                            } else {
                                crate::i18n::t("Bridge")
                            }
                        ))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                    );
                    ui.add_space(8.0);
                    // 100% 置信度条
                    let bar_w = 100.0;
                    let (rect, _) = ui.allocate_exact_size(
                        egui::Vec2::new(bar_w, 10.0),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, theme::BG_DEEP);
                    painter.rect_filled(
                        egui::Rect::from_min_size(rect.min, egui::Vec2::new(bar_w, 10.0)),
                        2.0,
                        border_color,
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "100%",
                        egui::FontId::new(9.0, egui::FontFamily::Monospace),
                        Color32::WHITE,
                    );
                });
                ui.label(
                    RichText::new(format!(
                        "{}: {}  |  VID: {:04X}  PID: {:04X}",
                        crate::i18n::t("Vendor"),
                        usb.vendor,
                        usb.vid,
                        usb.pid
                    ))
                    .color(theme::TEXT_PRIMARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
                if let Some(desc) = &usb.description {
                    if !desc.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "{}: {}",
                                crate::i18n::t("USB Desc"),
                                crate::i18n::t_owned(desc)
                            ))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                        );
                    }
                }
            });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
    }

    // 目标 MCU（协议探测识别）
    if let Some(chip) = &chip_result {
        ui.label(
            RichText::new(crate::i18n::t("Target MCU (Protocol)"))
                .color(theme::CYAN)
                .small()
                .strong(),
        );
        ui.add_space(2.0);

        let (border_color, bg_color) = if chip.confidence >= 0.8 {
            (theme::SUCCESS, Color32::from_rgba_premultiplied(0x10, 0x30, 0x18, 0x80))
        } else if chip.confidence >= 0.5 {
            (theme::WARNING, Color32::from_rgba_premultiplied(0x30, 0x28, 0x10, 0x80))
        } else {
            (theme::ERROR, Color32::from_rgba_premultiplied(0x30, 0x10, 0x10, 0x80))
        };
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.5_f32, border_color))
            .fill(bg_color)
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                // 第一行：芯片系列 + 置信度条
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&chip.chip_family)
                            .color(border_color)
                            .strong()
                            .size(15.0),
                    );
                    ui.add_space(8.0);
                    // 置信度条
                    let bar_w = 100.0;
                    let (rect, _) = ui.allocate_exact_size(
                        egui::Vec2::new(bar_w, 10.0),
                        egui::Sense::hover(),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, theme::BG_DEEP);
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            rect.min,
                            egui::Vec2::new(bar_w * chip.confidence, 10.0),
                        ),
                        2.0,
                        border_color,
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("{:.0}%", chip.confidence * 100.0),
                        egui::FontId::new(9.0, egui::FontFamily::Monospace),
                        Color32::WHITE,
                    );
                });
                // 第二行：详细描述
                ui.label(
                    RichText::new(&chip.chip_description)
                        .color(theme::TEXT_PRIMARY)
                        .small(),
                );
                // 第三行：识别证据
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        crate::i18n::t("Evidence"),
                        crate::i18n::t_owned(&chip.evidence)
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
            });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
    } else if usb_info.is_none() {
        // 既没有 USB 桥接芯片信息，也没有 MCU 识别结果
        ui.label(
            RichText::new(crate::i18n::t("Chip: not identified — select a port and click Start Chip ID"))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
        ui.add_space(4.0);
    } else {
        // 有 USB 桥接芯片但没有 MCU 识别结果
        ui.label(
            RichText::new(crate::i18n::t("Target MCU: not identified — click Start Chip ID to probe"))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
        ui.add_space(4.0);
    }

    // ===== 协议识别结果 =====
    ui.label(
        RichText::new(crate::i18n::t("Protocol Type"))
            .color(theme::CYAN)
            .small()
            .strong(),
    );
    ui.add_space(2.0);

    let result = match &s.detect_result {
        None => {
            ui.label(
                RichText::new(crate::i18n::t("No result yet — enable auto-detect and send a command to analyze response"))
                .color(theme::TEXT_DISABLED)
                .small(),
            );
            return;
        }
        Some(r) => r,
    };

    // 置信度条
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Confidence"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        let pct = result.confidence;
        let bar_color = if pct >= 0.8 {
            theme::SUCCESS
        } else if pct >= 0.5 {
            theme::WARNING
        } else {
            theme::ERROR
        };
        let bar_w = 120.0;
        let (rect, _) =
            ui.allocate_exact_size(egui::Vec2::new(bar_w, 12.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 2.0, theme::BG_DEEP);
        let fill_w = bar_w * pct;
        painter.rect_filled(
            egui::Rect::from_min_size(
                rect.min,
                egui::Vec2::new(fill_w, 12.0),
            ),
            2.0,
            bar_color,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{:.0}%", pct * 100.0),
            egui::FontId::new(10.0, egui::FontFamily::Monospace),
            Color32::WHITE,
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("{}", result.protocol_type))
                .color(bar_color)
                .strong(),
        );
    });

    // 识别描述
    ui.add_space(2.0);
    ui.label(
        RichText::new(&crate::i18n::t_owned(&result.description))
            .color(theme::TEXT_PRIMARY)
            .small(),
    );

    // 识别到的参数表格
    ui.add_space(4.0);
    egui::Grid::new("pa_detect_grid")
        .num_columns(4)
        .spacing([8.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            // 帧头
            ui.label(
                RichText::new(crate::i18n::t("Header"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            let hdr_str = if result.header.is_empty() {
                "—".to_string()
            } else {
                result
                    .header
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            ui.label(
                RichText::new(hdr_str)
                    .color(theme::ACCENT)
                    .family(egui::FontFamily::Monospace)
                    .small(),
            );
            // 帧尾
            ui.label(
                RichText::new(crate::i18n::t("Footer"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            let ftr_str = if result.footer.is_empty() {
                "—".to_string()
            } else {
                result
                    .footer
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            ui.label(
                RichText::new(ftr_str)
                    .color(theme::CYAN)
                    .family(egui::FontFamily::Monospace)
                    .small(),
            );
            ui.end_row();

            // 校验
            ui.label(
                RichText::new(crate::i18n::t("Checksum"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.label(
                RichText::new(format!("{}", result.checksum_type))
                    .color(theme::ERROR)
                    .small(),
            );
            // 数据格式
            ui.label(
                RichText::new(crate::i18n::t("Format"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.label(
                RichText::new(format!("{}", result.data_format))
                    .color(theme::SUCCESS)
                    .small(),
            );
            ui.end_row();

            // 通道数
            ui.label(
                RichText::new(crate::i18n::t("Channels##panels/sensor_dev/protocol_analyzer.rs:303554"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.label(
                RichText::new(format!("{}", result.channel_count))
                    .color(theme::TEXT_PRIMARY)
                    .small(),
            );
            ui.end_row();
        });

    // 应用按钮
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let apply_btn = egui::Button::new(
            RichText::new(crate::i18n::t("✔ Apply to config"))
            .color(Color32::BLACK)
            .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(160.0, 26.0))
        .stroke(egui::Stroke::NONE);
        if ui.add(apply_btn).clicked() {
            *apply = true;
        }
        ui.label(
            RichText::new(crate::i18n::t("(overwrites current protocol config)"))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
    });
}

/// 将识别结果应用到协议配置
fn apply_detect_result(s: &mut ProtocolAnalyzerState) {
    // 先把识别结果各字段 clone 出来，避免借用冲突
    let (proto, header, footer, checksum, format, ch) = match &s.detect_result {
        None => return,
        Some(r) => (
            r.protocol_type.clone(),
            r.header.clone(),
            r.footer.clone(),
            r.checksum_type.clone(),
            r.data_format.clone(),
            r.channel_count,
        ),
    };
    s.protocol_type = proto;
    s.header_hex = if header.is_empty() {
        String::new()
    } else {
        header
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    };
    s.footer_hex = if footer.is_empty() {
        String::new()
    } else {
        footer
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    };
    s.checksum_type = checksum;
    s.data_format = format;
    s.channel_count = ch;
}

/// 将识别结果应用到向导配置（同步到 wizard / monitor）
/// 从 DETECT 状态读取最佳识别结果，写入 WizardState，
/// 这样用户切换到「实时监控」面板时即可按识别到的协议解析数据。
fn apply_detect_to_wizard(ws: &mut super::wizard::WizardState, ad: &AutoDetectState) {
    if let Some(best) = &ad.best_result {
        super::wizard::apply_detected_protocol(
            ws,
            best.protocol_type.clone(),
            &best.header,
            &best.footer,
            best.checksum_type.clone(),
            best.data_format.clone(),
            best.channel_count,
        );
    }
}

/// 渲染保存的命令列表
fn render_command_list(ui: &mut egui::Ui, s: &mut ProtocolAnalyzerState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Saved Commands"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    if s.saved_commands.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No saved commands"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    // 收集待执行操作（避免迭代时修改）
    let mut resend_idx: Option<usize> = None;
    let mut delete_idx: Option<usize> = None;

    egui::Grid::new("pa_cmd_list")
        .num_columns(4)
        .spacing([8.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            // 表头
            ui.label(
                RichText::new(crate::i18n::t("Name"))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .strong(),
            );
            ui.label(
                RichText::new(crate::i18n::t("Data"))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .strong(),
            );
            ui.label(RichText::new("").small());
            ui.label(RichText::new("").small());
            ui.end_row();

            for (i, cmd) in s.saved_commands.iter().enumerate() {
                ui.label(
                    RichText::new(&cmd.name)
                        .color(theme::TEXT_PRIMARY)
                        .small(),
                );
                ui.label(
                    RichText::new(format_hex_spaced(&cmd.bytes))
                        .color(theme::TX_COLOR)
                        .family(egui::FontFamily::Monospace)
                        .small(),
                );
                if ui
                    .button(RichText::new(crate::i18n::t("Resend")).small())
                    .on_hover_text(crate::i18n::t("Load this command into the input box"))
                    .clicked()
                {
                    resend_idx = Some(i);
                }
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(crate::i18n::t("Delete"))
                                .color(Color32::BLACK)
                                .small(),
                        )
                        .fill(theme::ERROR),
                    )
                    .on_hover_text(crate::i18n::t("Remove this command from the list"))
                    .clicked()
                {
                    delete_idx = Some(i);
                }
                ui.end_row();
            }
        });

    // 先执行重发（索引不变），再执行删除
    if let Some(i) = resend_idx {
        if i < s.saved_commands.len() {
            s.command_input = format_hex_spaced(&s.saved_commands[i].bytes);
        }
    }
    if let Some(i) = delete_idx {
        if i < s.saved_commands.len() {
            s.saved_commands.remove(i);
        }
    }
}

/// 渲染协议日志
fn render_log_section(ui: &mut egui::Ui, s: &mut ProtocolAnalyzerState, _zh: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Log"))
                .color(theme::CYAN)
                .strong(),
        );
        ui.add_space(8.0);
        if ui
            .button(RichText::new(crate::i18n::t("Clear")).small())
            .on_hover_text(crate::i18n::t("Clear all protocol logs"))
            .clicked()
        {
            s.log.clear();
        }
    });

    if s.log.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No logs"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    ui.add_space(2.0);

    ScrollArea::vertical()
        .max_height(180.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for entry in s.log.iter().rev() {
                let color = if entry.direction == "TX" {
                    theme::TX_COLOR
                } else {
                    theme::RX_COLOR
                };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&entry.timestamp)
                            .color(theme::TEXT_DISABLED)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.label(
                        RichText::new(entry.direction)
                            .color(color)
                            .small()
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format_hex_spaced(&entry.data))
                            .color(theme::TEXT_PRIMARY)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.label(
                        RichText::new(format!("({})", entry.data.len()))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    /// 辅助函数：构造空 rx_buffer
    fn empty_rx() -> Vec<RxEntry> { Vec::new() }

    // ===== Ollama 芯片识别（feature gate）=====

    #[cfg(feature = "ollama")]
    fn chip_result(chip: &str, vendor: &str, family: &str, conf: f32) -> ChipIdentifyResult {
        ChipIdentifyResult {
            chip: chip.to_string(),
            vendor: vendor.to_string(),
            family: family.to_string(),
            confidence: conf,
            reasoning: "测试".to_string(),
            suggested_driver: Some("wch-ch34x".to_string()),
            source: miconlab_ai::IdentifySource::Ollama,
        }
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_synth_bridge_confidence_gate() {
        // 置信度不足 0.7 → 不合成
        assert!(synthesize_bridge_info(&chip_result("CH9102", "WCH", "usb-uart", 0.69), 0x1A86, 0x1A02).is_none());
        assert!(synthesize_bridge_info(&chip_result("CH9102", "WCH", "usb-uart", 0.7), 0x1A86, 0x1A02).is_some());
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_synth_bridge_bridge_family_is_not_native() {
        let synth = synthesize_bridge_info(&chip_result("CH9102", "WCH", "usb-uart", 0.9), 0x1A86, 0x1A02)
            .unwrap();
        assert_eq!(synth.chip_name, "CH9102");
        assert_eq!(synth.vendor, "WCH");
        assert_eq!(synth.vid, 0x1A86);
        assert_eq!(synth.pid, 0x1A02);
        assert!(!synth.is_native_mcu); // usb-uart 族 = 桥接芯片
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_synth_bridge_native_mcu_family() {
        // family 非 usb-uart（如 esp32 原生 USB）→ 视为 MCU 原生 USB
        let synth = synthesize_bridge_info(&chip_result("ESP32-S3", "Espressif", "xtensa", 0.85), 0x303A, 0x0009)
            .unwrap();
        assert!(synth.is_native_mcu);
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_mcu_result_confidence_gate() {
        assert!(mcu_result_to_detect(&chip_result("STM32F103", "ST", "cortex-m3", 0.59)).is_none());
        let d = mcu_result_to_detect(&chip_result("STM32F103", "ST", "cortex-m3", 0.6)).unwrap();
        assert_eq!(d.chip_family, "STM32F103");
        assert!((d.confidence - 0.6).abs() < 1e-6);
        assert!(d.chip_description.contains("Ollama"));
        assert_eq!(d.evidence, "测试");
    }

    /// 测试被动分析：空数据
    #[test]
    fn test_passive_analysis_empty() {
        let result = analyze_passive_mcu_comprehensive(&[], 0.0, 921600, &empty_rx(), None);
        assert_eq!(result.sample_count, 0);
        assert!(!result.candidates.is_empty());
    }

    /// 测试被动分析：STM32 特征（极低抖动，921600 波特率）
    #[test]
    fn test_passive_analysis_stm32_low_jitter() {
        let intervals: Vec<u64> = vec![10, 10, 10, 10, 10, 10, 10, 10, 10, 10];
        let result = analyze_passive_mcu_comprehensive(&intervals, 0.95, 921600, &empty_rx(), None);
        assert!(result.jitter_ms < 0.5);
        assert!(result.confidence > 0.5);
        assert!(result.candidates.iter().any(|c| c.example_model.contains("STM32")));
    }

    /// 测试被动分析：ESP32 特征（高抖动 > 2ms）
    #[test]
    fn test_passive_analysis_esp32_high_jitter() {
        let intervals: Vec<u64> = vec![12, 18, 15, 13, 17, 16, 14, 18, 12, 16];
        let result = analyze_passive_mcu_comprehensive(&intervals, 0.90, 921600, &empty_rx(), None);
        assert!(result.jitter_ms > 2.0);
        let esp32_score = result.candidates.iter()
            .find(|c| c.chip_family.starts_with("ESP32"))
            .map(|c| c.score)
            .unwrap_or(0.0);
        assert!(esp32_score > 0.3);
    }

    /// 测试被动分析：STM8 特征（长间隔 > 100ms）
    #[test]
    fn test_passive_analysis_stm8_long_interval() {
        let intervals: Vec<u64> = vec![120, 121, 119, 120, 122, 118, 120, 121];
        let result = analyze_passive_mcu_comprehensive(&intervals, 0.80, 921600, &empty_rx(), None);
        assert!(result.avg_interval_ms > 100.0);
    }

    /// 测试被动分析：置信度上限 0.95
    #[test]
    fn test_passive_analysis_confidence_cap() {
        let intervals: Vec<u64> = vec![10; 100];
        let result = analyze_passive_mcu_comprehensive(&intervals, 1.0, 921600, &empty_rx(), None);
        assert!(result.confidence <= 0.95);
    }

    /// 测试波特率分频误差计算
    #[test]
    fn test_baud_error_calculation() {
        let err = calc_baud_error(72.0, 921600);
        assert!(err < 0.5, "72MHz @ 921600 error should be < 0.5%, got {:.3}%", err);
        let err_esp = calc_baud_error(40.0, 921600);
        assert!(err_esp < 2.0, "40MHz @ 921600 error should be < 2%, got {:.3}%", err_esp);
        let err_stm8 = calc_baud_error(16.0, 921600);
        assert!(err_stm8 > 0.5, "16MHz @ 921600 error should be > 0.5%, got {:.3}%", err_stm8);
    }

    /// 测试鞋垫帧间隔采集：正确识别 AA 55 03 99 帧头
    #[test]
    fn test_collect_insole_frame_stats() {
        let header = [0xAA, 0x55, 0x03, 0x99];
        let base = Local::now();

        let entries = vec![
            RxEntry {
                timestamp: base,
                data: { let mut d = vec![0x00; 10]; d.extend_from_slice(&header); d },
                direction: Direction::Rx,
            },
            RxEntry {
                timestamp: base + chrono::Duration::milliseconds(10),
                data: { let mut d = vec![0x00; 5]; d.extend_from_slice(&header); d },
                direction: Direction::Rx,
            },
            RxEntry {
                timestamp: base + chrono::Duration::milliseconds(20),
                data: { let mut d = vec![0x00; 8]; d.extend_from_slice(&header); d },
                direction: Direction::Rx,
            },
            RxEntry {
                timestamp: base + chrono::Duration::milliseconds(25),
                data: header.to_vec(),
                direction: Direction::Tx,
            },
        ];

        let (intervals, match_rate) = collect_insole_frame_stats(&entries);
        assert_eq!(intervals.len(), 2);
        assert!((match_rate - 1.0).abs() < 0.01);
    }

    /// 测试鞋垫帧间隔采集：无帧头数据
    #[test]
    fn test_collect_insole_stats_no_header() {
        let entries = vec![RxEntry {
            timestamp: Local::now(),
            data: vec![0x00; 100],
            direction: Direction::Rx,
        }];
        let (intervals, match_rate) = collect_insole_frame_stats(&entries);
        assert!(intervals.is_empty());
        assert_eq!(match_rate, 0.0);
    }

    /// 测试候选 MCU 排序：高分在前
    #[test]
    fn test_candidate_sorting() {
        let intervals: Vec<u64> = vec![10; 20];
        let result = analyze_passive_mcu_comprehensive(&intervals, 1.0, 921600, &empty_rx(), None);
        assert!(result.candidates.len() > 5);
        for w in result.candidates.windows(2) {
            assert!(w[0].score >= w[1].score, "Candidates should be sorted by score descending");
        }
    }

    /// 测试不同波特率下的候选 MCU 差异
    #[test]
    fn test_baud_rate_affects_ranking() {
        let intervals: Vec<u64> = vec![10; 20];
        let result_921k = analyze_passive_mcu_comprehensive(&intervals, 1.0, 921600, &empty_rx(), None);
        let result_115k = analyze_passive_mcu_comprehensive(&intervals, 1.0, 115200, &empty_rx(), None);
        assert!(!result_921k.candidates.is_empty());
        assert!(!result_115k.candidates.is_empty());
    }

    /// 测试 ADC 分辨率分析：12-bit 范围
    #[test]
    fn test_adc_resolution_12bit() {
        // 12-bit ADC 最大值 4095，构造 4 个采样值
        let adc_data: Vec<u8> = vec![
            0xFF, 0x0F, // 4095
            0x00, 0x00, // 0
            0x80, 0x02, // 640
            0xFF, 0x0F, // 4095
        ];
        let (res, max_v, min_v) = analyze_adc_resolution(&adc_data);
        assert_eq!(res, 12);
        assert_eq!(max_v, 4095);
        assert_eq!(min_v, 0);
    }

    /// 测试 ADC 分辨率分析：10-bit 范围
    #[test]
    fn test_adc_resolution_10bit() {
        let adc_data: Vec<u8> = vec![
            0xFF, 0x03, // 1023
            0x00, 0x00, // 0
        ];
        let (res, max_v, _) = analyze_adc_resolution(&adc_data);
        assert_eq!(res, 10);
        assert_eq!(max_v, 1023);
    }

    /// 测试 IMU 四元数分析：有效 IEEE754 小端
    #[test]
    fn test_imu_quaternion_valid_le() {
        // 构造单位四元数 w=1.0, x=0, y=0, z=0 (小端 float32)
        let w_bytes = 1.0_f32.to_le_bytes();
        let zero_bytes = 0.0_f32.to_le_bytes();
        let mut imu_data = Vec::new();
        imu_data.extend_from_slice(&w_bytes);
        imu_data.extend_from_slice(&zero_bytes);
        imu_data.extend_from_slice(&zero_bytes);
        imu_data.extend_from_slice(&zero_bytes);

        let (valid, norm, byte_order) = analyze_imu_quaternion(&imu_data);
        assert!(valid);
        assert!((norm - 1.0).abs() < 0.01, "norm should be ~1.0, got {}", norm);
        assert_eq!(byte_order, "little");
    }

    /// 测试 IMU 四元数分析：无效数据
    #[test]
    fn test_imu_quaternion_invalid() {
        // 全 0xFF 是 NaN/Inf，不应视为有效
        let imu_data = vec![0xFF; 16];
        let (valid, _, byte_order) = analyze_imu_quaternion(&imu_data);
        assert!(!valid);
        assert_eq!(byte_order, "unknown");
    }

    /// 测试帧结构一致性分析
    #[test]
    fn test_frame_consistency_paired() {
        // 构造两对配对的 Packet1+Packet2（SeqTag 相同）
        let mut raw: Vec<u8> = Vec::new();
        for seq in 0..2u8 {
            // Packet1
            raw.extend_from_slice(&INSOLE_HEADER);
            raw.push(INSOLE_PKT1_TYPE);
            raw.push(seq);
            raw.extend_from_slice(&(INSOLE_PKT1_PAYLOAD as u16).to_le_bytes());
            raw.extend_from_slice(&vec![0x80; INSOLE_PKT1_PAYLOAD]); // ADC 数据
            raw.extend_from_slice(&[0x00, 0x00]); // 校验
            // Packet2
            raw.extend_from_slice(&INSOLE_HEADER);
            raw.push(INSOLE_PKT2_TYPE);
            raw.push(seq);
            raw.extend_from_slice(&(INSOLE_PKT2_PAYLOAD as u16).to_le_bytes());
            raw.extend_from_slice(&vec![0x80; INSOLE_PKT2_PAYLOAD]); // ADC+IMU
            raw.extend_from_slice(&[0x00, 0x00]); // 校验
        }

        let entries = vec![RxEntry {
            timestamp: Local::now(),
            data: raw,
            direction: Direction::Rx,
        }];

        let (consistency, paired) = analyze_frame_consistency(&entries);
        assert_eq!(paired, 2);
        assert!((consistency - 1.0).abs() < 0.01);
    }

    /// 测试完整帧解析
    #[test]
    fn test_parse_insole_frames() {
        let mut raw: Vec<u8> = Vec::new();
        // 构造一对配对帧
        raw.extend_from_slice(&INSOLE_HEADER);
        raw.push(INSOLE_PKT1_TYPE);
        raw.push(0x42);
        raw.extend_from_slice(&(INSOLE_PKT1_PAYLOAD as u16).to_le_bytes());
        raw.extend_from_slice(&vec![0x80; INSOLE_PKT1_PAYLOAD]);
        raw.extend_from_slice(&[0x00, 0x00]);
        raw.extend_from_slice(&INSOLE_HEADER);
        raw.push(INSOLE_PKT2_TYPE);
        raw.push(0x42);
        raw.extend_from_slice(&(INSOLE_PKT2_PAYLOAD as u16).to_le_bytes());
        let mut pkt2 = vec![0x80; 128]; // ADC
        pkt2.extend_from_slice(&1.0_f32.to_le_bytes()); // IMU w=1.0
        pkt2.extend_from_slice(&0.0_f32.to_le_bytes()); // x=0
        pkt2.extend_from_slice(&0.0_f32.to_le_bytes()); // y=0
        pkt2.extend_from_slice(&0.0_f32.to_le_bytes()); // z=0
        raw.extend_from_slice(&pkt2);
        raw.extend_from_slice(&[0x00, 0x00]);

        let entries = vec![RxEntry {
            timestamp: Local::now(),
            data: raw,
            direction: Direction::Rx,
        }];

        let frames = parse_insole_frames(&entries);
        assert_eq!(frames.len(), 1);
        let (adc, imu, seq) = &frames[0];
        assert_eq!(adc.len(), 256);
        assert_eq!(imu.len(), 16);
        assert_eq!(*seq, 0x42);

        // 验证 IMU 四元数
        let (valid, norm, byte_order) = analyze_imu_quaternion(imu);
        assert!(valid);
        assert!((norm - 1.0).abs() < 0.01);
        assert_eq!(byte_order, "little");
    }

    /// 测试 DMA 传输特征分析：均匀大块数据
    #[test]
    fn test_dma_pattern_uniform() {
        let entries: Vec<RxEntry> = (0..10)
            .map(|_| RxEntry {
                timestamp: Local::now(),
                data: vec![0x80; 272], // 均匀的 272 字节块
                direction: Direction::Rx,
            })
            .collect();

        let score = analyze_dma_pattern(&entries);
        assert!(score > 0.5, "Uniform large blocks should score high for DMA, got {}", score);
    }

    /// 测试 DMA 传输特征分析：不均匀小块数据
    #[test]
    fn test_dma_pattern_irregular() {
        let entries: Vec<RxEntry> = vec![
            RxEntry { timestamp: Local::now(), data: vec![0x00; 10], direction: Direction::Rx },
            RxEntry { timestamp: Local::now(), data: vec![0x00; 200], direction: Direction::Rx },
            RxEntry { timestamp: Local::now(), data: vec![0x00; 5], direction: Direction::Rx },
            RxEntry { timestamp: Local::now(), data: vec![0x00; 100], direction: Direction::Rx },
            RxEntry { timestamp: Local::now(), data: vec![0x00; 3], direction: Direction::Rx },
        ];

        let score = analyze_dma_pattern(&entries);
        assert!(score < 0.5, "Irregular small blocks should score low for DMA, got {}", score);
    }

    /// 测试深度分析整合：带完整帧数据的综合分析
    #[test]
    fn test_comprehensive_with_frames() {
        // 构造包含完整帧的 rx_buffer
        let mut raw: Vec<u8> = Vec::new();
        for seq in 0..3u8 {
            // Packet1
            raw.extend_from_slice(&INSOLE_HEADER);
            raw.push(INSOLE_PKT1_TYPE);
            raw.push(seq);
            raw.extend_from_slice(&(INSOLE_PKT1_PAYLOAD as u16).to_le_bytes());
            // 12-bit ADC 值（0-4095）
            for i in 0..64 {
                let val = (i * 60) as u16; // max = 63*60 = 3780 < 4095
                raw.extend_from_slice(&val.to_le_bytes());
            }
            raw.extend_from_slice(&[0x00, 0x00]);
            // Packet2
            raw.extend_from_slice(&INSOLE_HEADER);
            raw.push(INSOLE_PKT2_TYPE);
            raw.push(seq);
            raw.extend_from_slice(&(INSOLE_PKT2_PAYLOAD as u16).to_le_bytes());
            for i in 0..64 {
                let val = (i * 60 + 50) as u16; // max = 63*60+50 = 3830 < 4095
                raw.extend_from_slice(&val.to_le_bytes());
            }
            // IMU 单位四元数
            raw.extend_from_slice(&1.0_f32.to_le_bytes());
            raw.extend_from_slice(&0.0_f32.to_le_bytes());
            raw.extend_from_slice(&0.0_f32.to_le_bytes());
            raw.extend_from_slice(&0.0_f32.to_le_bytes());
            raw.extend_from_slice(&[0x00, 0x00]);
        }

        let entries = vec![RxEntry {
            timestamp: Local::now(),
            data: raw,
            direction: Direction::Rx,
        }];

        let result = analyze_passive_mcu_comprehensive(&[10, 10, 10], 0.95, 921600, &entries, None);
        assert_eq!(result.parsed_frames, 3);
        assert_eq!(result.adc_resolution, 12);
        assert!(result.imu_float_valid);
        assert!((result.imu_quaternion_norm - 1.0).abs() < 0.01);
        assert_eq!(result.byte_order, "little");
        assert!((result.frame_consistency - 1.0).abs() < 0.01);
    }
}


