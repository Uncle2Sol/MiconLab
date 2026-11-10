//! 智能诊断规则引擎
//!
//! 基于上下文的根因推理，针对串口通信中常见问题给出诊断结论和操作建议。
//!
//! 诊断维度：
//! - **连接错误**：权限不足、端口不存在、设备断开等
//! - **通信异常**：波特率不匹配、协议错误、Bootloader 未进入等
//! - **数据异常**：空数据、乱码、协议偏离等
//! - **设备识别**：VID/PID 匹配、芯片类型推断、面板跳转建议
//!
//! 推理方式：规则匹配 + 优先级排序。每条规则检查上下文中的特定模式，
//! 命中后返回诊断结论、严重程度和操作建议。

use crate::firmware::FirmwareAnalysis;
use crate::protocol_fp::ProtocolClass;
use serde::{Deserialize, Serialize};

/// 回环模式（与 miconlab-core::EchoPattern 镜像，由 GUI 层转换）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EchoPattern {
    /// 精确匹配（RX 与 TX 字节序列高度一致）
    Exact,
    /// 位移匹配（波特率不匹配导致的位错位）
    BitShifted,
    /// 高字节主导（波特率严重不匹配）
    HighByteDominant,
    /// 无匹配
    None,
}

impl Default for EchoPattern {
    fn default() -> Self {
        EchoPattern::None
    }
}

/// 串扰检测报告（与 miconlab-core::CrosstalkReport 镜像，由 GUI 层转换）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrosstalkReport {
    /// RX 中匹配 TX 的字节占比 (0.0-1.0)
    pub echo_ratio: f32,
    /// 接收总字节 / 发送总字节
    pub rx_tx_ratio: f32,
    /// 是否判定为 TX 自发自收回环
    pub is_loopback: bool,
    /// 回串延迟估算（微秒）
    pub latency_us: Option<u64>,
    /// 回环模式
    pub pattern: EchoPattern,
    /// 高字节(≥0x80)在 RX 中的占比
    pub high_byte_ratio: f32,
    /// TX 总字节数
    pub tx_total: u64,
    /// RX 总字节数
    pub rx_total: u64,
}

impl CrosstalkReport {
    /// 约 50% 串扰（典型 CH340 TX 阻抗回串）
    pub fn is_half_echo(&self) -> bool {
        (0.4..=0.6).contains(&self.rx_tx_ratio) && self.tx_total >= 100
    }
}

/// 诊断严重程度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// 信息提示
    Info,
    /// 警告
    Warning,
    /// 错误
    Error,
    /// 严重错误
    Critical,
}

impl Severity {
    pub fn label_zh(self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Warning => "Warning",
            Severity::Error => "Error",
            Severity::Critical => "Critical",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Warning => "Warning",
            Severity::Error => "Error",
            Severity::Critical => "Critical",
        }
    }
}

/// 当前操作类型（用于诊断上下文）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    Connecting,
    Reading,
    Writing,
    Erasing,
    Flashing,
    Idle,
}

/// 诊断上下文：当前系统状态的快照
///
/// 由 GUI 层在需要诊断时构建，传入引擎进行推理。
#[derive(Debug, Clone, Default)]
pub struct DiagnosticContext {
    /// 最近错误消息
    pub error_message: Option<String>,
    /// 是否已连接
    pub is_connected: bool,
    /// 当前波特率
    pub baud_rate: u32,
    /// 端口名
    pub port_name: Option<String>,
    /// USB VID/PID
    pub vid_pid: Option<(u16, u16)>,
    /// 芯片提示
    pub chip_hint: Option<String>,
    /// 最近接收数据（最多 256 字节）
    pub last_rx_data: Vec<u8>,
    /// 已识别的协议类型
    pub protocol_class: Option<ProtocolClass>,
    /// 当前操作
    pub operation: Option<Operation>,
    /// 协议识别置信度
    pub protocol_confidence: f32,
    /// 是否在 Bootloader 连接尝试中
    pub bootloader_attempt: bool,
    /// 最近发送数据（最多 256 字节）
    pub last_tx_data: Vec<u8>,
    /// TX 总字节数
    pub tx_bytes: u64,
    /// RX 总字节数
    pub rx_bytes: u64,
    /// 串扰检测报告
    pub crosstalk: Option<CrosstalkReport>,
    /// 固件分析提示（阶段4 反哺，自由文本，向后兼容）
    pub firmware_hint: Option<String>,
    /// 固件结构化分析（D.6 反哺，比 firmware_hint 更丰富）
    pub firmware_analysis: Option<FirmwareAnalysis>,
}

impl DiagnosticContext {
    /// 创建空上下文
    pub fn new() -> Self {
        Self::default()
    }
}

/// 诊断结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosisResult {
    /// 规则 ID
    pub rule_id: String,
    /// 严重程度
    pub severity: Severity,
    /// 诊断结论（中文）
    pub diagnosis: String,
    /// 操作建议列表（中文）
    pub suggestions: Vec<String>,
    /// 置信度 0.0-1.0
    pub confidence: f32,
}

/// 诊断规则
#[derive(Debug, Clone)]
pub struct DiagnosticRule {
    /// 规则 ID
    pub id: &'static str,
    /// 规则描述
    pub description: &'static str,
}

/// 诊断规则引擎
#[derive(Clone)]
pub struct DiagnosticEngine {
    rules: Vec<DiagnosticRule>,
}

impl Default for DiagnosticEngine {
    fn default() -> Self {
        Self::with_builtin_rules()
    }
}

impl DiagnosticEngine {
    /// 创建引擎，预置所有诊断规则
    pub fn with_builtin_rules() -> Self {
        Self {
            rules: builtin_rules(),
        }
    }

    /// 执行诊断推理
    ///
    /// 按优先级顺序检查所有规则，返回命中的诊断结果列表。
    /// 结果按严重程度降序排列。
    pub fn diagnose(&self, ctx: &DiagnosticContext) -> Vec<DiagnosisResult> {
        let mut results = Vec::new();

        // ===== 连接错误诊断 =====
        if let Some(err) = &ctx.error_message {
            let err_lower = err.to_lowercase();

            if err_lower.contains("permission denied") || err_lower.contains("access denied") {
                results.push(DiagnosisResult {
                    rule_id: "CONN_PERMISSION".to_string(),
                    severity: Severity::Error,
                    diagnosis: "Insufficient serial port access permission".to_string(),
                    suggestions: vec![
                        "The serial port may be occupied by another program; close software using it (e.g. Arduino IDE, PuTTY)".to_string(),
                        "macOS/Linux: try sudo chmod 666 /dev/tty.YOUR_DEVICE".to_string(),
                        "macOS: check System Settings → Privacy & Security → whether serial device access is allowed".to_string(),
                    ],
                    confidence: 0.9,
                });
            }

            if err_lower.contains("no such file") || err_lower.contains("no such device") {
                results.push(DiagnosisResult {
                    rule_id: "CONN_NO_DEVICE".to_string(),
                    severity: Severity::Error,
                    diagnosis: "Serial device does not exist".to_string(),
                    suggestions: vec![
                        "The device may have been unplugged; check the USB connection".to_string(),
                        "Click the \"Refresh\" button to rescan available ports".to_string(),
                        "macOS: check /dev/cu.* and /dev/tty.* for the corresponding device".to_string(),
                    ],
                    confidence: 0.95,
                });
            }

            if err_lower.contains("broken pipe") {
                // 基于 last_rx_data 区分：通讯建立后断开 vs 从未通讯就断开
                let had_communication = !ctx.last_rx_data.is_empty();
                let is_ch340 = ctx.vid_pid.map_or(false, |(vid, _)| vid == 0x1A86);
                let is_cp210x = ctx.vid_pid.map_or(false, |(vid, _)| vid == 0x10C4);
                let is_high_baud = ctx.baud_rate >= 230_400;

                if had_communication {
                    // 通讯建立后断开：最常见原因是波特率过高导致设备端 UART 异常
                    let mut suggestions = Vec::new();

                    if is_high_baud {
                        // 高波特率场景：降低波特率是首选方案
                        suggestions.push(
                            format!(
                                "Current baud rate {} is too high; the device UART may not support it. Lower it to 115200/57600/38400/9600 and retry",
                                ctx.baud_rate
                            ),
                        );
                        suggestions.push(
                            "Low-speed MCUs or software UARTs typically max out at 115200; for domestic chips (e.g. CH55x/STM8) use ≤57600".to_string(),
                        );
                    } else {
                        suggestions.push(
                            "A baud mismatch may cause the device to disconnect; try other common baud rates (9600/19200/38400/57600/115200)".to_string(),
                        );
                    }

                    suggestions.push(
                        "The device may disconnect intentionally (firmware timeout/reset/low-power); check the device firmware logic".to_string(),
                    );

                    if is_ch340 {
                        suggestions.push(
                            "CH340/CH340N often drops packets or disconnects with STM32 at high baud rates (>230400); lower to 115200 or below".to_string(),
                        );
                        suggestions.push(
                            "STM32 UART has larger error with the internal HSI clock; configure an external HSE crystal or lower the baud rate".to_string(),
                        );
                    } else if is_cp210x {
                        suggestions.push(
                            "CP210x driver occasionally disconnects on macOS; try replugging or using another USB port".to_string(),
                        );
                    }
                    suggestions.push("Insufficient USB power may reset the device; try connecting directly to a motherboard USB port".to_string());
                    suggestions.push("Try reconnecting the serial port".to_string());

                    let confidence = if is_high_baud { 0.85 } else { 0.7 };

                    results.push(DiagnosisResult {
                        rule_id: "CONN_BROKEN_PIPE".to_string(),
                        severity: Severity::Critical,
                        diagnosis: "Device disconnected after communication was established (Broken pipe)".to_string(),
                        suggestions,
                        confidence,
                    });
                } else {
                    // 从未收到数据就断开：更可能是线缆/物理连接问题
                    results.push(DiagnosisResult {
                        rule_id: "CONN_BROKEN_PIPE".to_string(),
                        severity: Severity::Critical,
                        diagnosis: "Device connection lost (Broken pipe)".to_string(),
                        suggestions: vec![
                            "The USB cable may be loose or damaged; replug it".to_string(),
                            "The device may have reset or lost power; check the power supply".to_string(),
                            "Try reconnecting the serial port".to_string(),
                        ],
                        confidence: 0.85,
                    });
                }
            }

            if err_lower.contains("input/output error") {
                results.push(DiagnosisResult {
                    rule_id: "CONN_IO_ERROR".to_string(),
                    severity: Severity::Critical,
                    diagnosis: "Serial I/O error (device communication interrupted)".to_string(),
                    suggestions: vec![
                        "The USB cable or adapter may be faulty; try another cable".to_string(),
                        "The device may have lost power or reset; check its state".to_string(),
                        "The USB port may not supply enough power; try another port".to_string(),
                    ],
                    confidence: 0.9,
                });
            }

            if err_lower.contains("bad file descriptor") {
                results.push(DiagnosisResult {
                    rule_id: "CONN_BAD_FD".to_string(),
                    severity: Severity::Critical,
                    diagnosis: "Invalid file descriptor (internal error)".to_string(),
                    suggestions: vec![
                        "The serial handle is invalid; disconnect and reconnect".to_string(),
                        "If this recurs, restart the app".to_string(),
                    ],
                    confidence: 0.85,
                });
            }
        }

        // ===== Bootloader 诊断 =====
        if ctx.bootloader_attempt {
            if let Some(err) = &ctx.error_message {
                if err.contains("NAK") || err.contains("ACK") || err.contains("0x79") || err.contains("Bootloader") {
                    let is_high_baud = ctx.baud_rate >= 230_400;
                    let mut suggestions = vec![
                        "【Step 1】Set BOOT0=1 (high): locate the BOOT0 jumper/switch on the board and connect BOOT0 to VCC (3.3V)".to_string(),
                        "  · No jumper: short the BOOT0 pin to 3.3V with a jumper wire, or hold the BOOT0 button (on some boards)".to_string(),
                        "  · Common boards: Blue Pill set BOOT0 jumper to 1; Nucleo adjust SB2 solder bridge; minimal boards toggle BOOT0 switch to ON".to_string(),
                        "【Step 2】Trigger reset: press the RESET button on the board, or briefly touch NRST to GND with a jumper wire then release".to_string(),
                        "  · No RESET button: unplug USB and power-cycle (set BOOT0=1 first; it enters Bootloader at power-up)".to_string(),
                        "【Step 3】Return to the flashing page and click \"Connect (0x7F)\"; the device should reply 0x79 (ACK), indicating Bootloader entry succeeded".to_string(),
                        "  · Timing: send 0x7F within 100ms after reset; click Connect immediately after reset".to_string(),
                        "  · Still failing: confirm BOOT0 is really high (measure with a multimeter) and NRST reset is effective".to_string(),
                    ];
                    if is_high_baud {
                        suggestions.push(
                            format!(
                                "Current baud rate {} is too high; STM32 system Bootloader recommends 115200. Lower it and retry",
                                ctx.baud_rate
                            ),
                        );
                    } else {
                        suggestions.push(
                            "Confirm the baud rate: STM32 system Bootloader usually uses 115200 (auto-baud detection supported)".to_string(),
                        );
                    }
                    suggestions.push("Check TX/RX wiring: CH340N TX→STM32 RX, CH340N RX→STM32 TX".to_string());
                    suggestions.push("Use DTR/RTS to trigger reset: the terminal panel can manually control DTR/RTS levels".to_string());
                    suggestions.push("Confirm the device is STM32/GD32/CH32 series (ESP32/ESP8266 use a different Bootloader protocol)".to_string());

                    results.push(DiagnosisResult {
                        rule_id: "BOOTLOADER_NO_ACK".to_string(),
                        severity: Severity::Warning,
                        diagnosis: "STM32 Bootloader did not respond with ACK(0x79); the device may not be in Bootloader mode".to_string(),
                        suggestions,
                        confidence: 0.85,
                    });
                }
            }

            // 收到非 0x7F 响应数据
            if !ctx.last_rx_data.is_empty() {
                let has_0x7f = ctx.last_rx_data.contains(&0x7F);
                let has_0x79 = ctx.last_rx_data.contains(&0x79);
                if !has_0x7f && !has_0x79 && ctx.last_rx_data.len() > 4 {
                    results.push(DiagnosisResult {
                        rule_id: "BOOTLOADER_WRONG_DATA".to_string(),
                        severity: Severity::Warning,
                        diagnosis: "The device returned non-Bootloader data (it may be running in application mode)".to_string(),
                        suggestions: vec![
                            "The device is running the application, not the Bootloader".to_string(),
                            "Put the device into Bootloader mode before connecting".to_string(),
                            "Or use \"Raw Read\" mode to read application data directly".to_string(),
                        ],
                        confidence: 0.75,
                    });
                }
            }
        }

        // ===== 数据流诊断 =====
        if ctx.is_connected && !ctx.last_rx_data.is_empty() {
            let data = &ctx.last_rx_data;
            let null_count = data.iter().filter(|&&b| b == 0).count();
            let null_ratio = null_count as f32 / data.len() as f32;

            // 全零数据
            if null_ratio > 0.9 {
                results.push(DiagnosisResult {
                    rule_id: "DATA_ALL_ZERO".to_string(),
                    severity: Severity::Warning,
                    diagnosis: "Received a large number of null bytes (0x00)".to_string(),
                    suggestions: vec![
                        "The device may not have started or is in an abnormal state".to_string(),
                        "The line may be shorted or have poor contact".to_string(),
                        "The baud rate may be mismatched; try other common rates (9600/115200)".to_string(),
                    ],
                    confidence: 0.7,
                });
            }

            // 0xFF 填充（Flash 擦除后状态）
            let ff_count = data.iter().filter(|&&b| b == 0xFF).count();
            let ff_ratio = ff_count as f32 / data.len() as f32;
            if ff_ratio > 0.9 {
                results.push(DiagnosisResult {
                    rule_id: "DATA_ALL_FF".to_string(),
                    severity: Severity::Info,
                    diagnosis: "Received a large number of 0xFF bytes (possibly unprogrammed Flash)".to_string(),
                    suggestions: vec![
                        "If reading firmware: this region may be unprogrammed (Flash reads 0xFF after erase)".to_string(),
                        "Confirm the read address is correct".to_string(),
                    ],
                    confidence: 0.6,
                });
            }
        }

        // 连接但无数据
        if ctx.is_connected
            && ctx.last_rx_data.is_empty()
            && ctx.operation == Some(Operation::Connecting)
        {
            results.push(DiagnosisResult {
                rule_id: "CONN_NO_DATA".to_string(),
                severity: Severity::Info,
                diagnosis: "Connected but no data received".to_string(),
                suggestions: vec![
                    "The device may not send data proactively; try sending a command".to_string(),
                    "Check TX/RX wiring (TX↔RX cross-connection)".to_string(),
                    "Confirm the baud rate matches the device configuration".to_string(),
                    "For sensor devices, a trigger command may be required to output data".to_string(),
                ],
                confidence: 0.5,
            });
        }

        // ===== 协议识别建议 =====
        if let Some(class) = ctx.protocol_class {
            if ctx.protocol_confidence > 0.5 {
                let (diagnosis, suggestion) = match class {
                    ProtocolClass::ModbusRtu => (
                        "Detected Modbus RTU protocol".to_string(),
                        "Switch to the \"Modbus\" panel for RTU master debugging".to_string(),
                    ),
                    ProtocolClass::InsoleSensor => (
                        "Detected insole pressure sensor protocol".to_string(),
                        "Switch to the \"Sensor Dev\" panel for protocol analysis and heatmap viewing".to_string(),
                    ),
                    ProtocolClass::Stm32Bootloader => (
                        "Detected STM32 Bootloader communication".to_string(),
                        "Switch to the \"Firmware Flashing\" panel to read/write firmware".to_string(),
                    ),
                    ProtocolClass::EspImage => (
                        "Detected ESP image data".to_string(),
                        "Switch to the \"Firmware Flashing\" panel to flash ESP32/ESP8266".to_string(),
                    ),
                    ProtocolClass::Nmea => (
                        "Detected NMEA GPS sentence".to_string(),
                        "The device is a GPS module; baud rate is usually 4800 or 9600".to_string(),
                    ),
                    ProtocolClass::Slcan => (
                        "Detected SLCAN protocol".to_string(),
                        "Switch to the \"CAN Bus\" panel for SLCAN command interaction".to_string(),
                    ),
                    _ => (String::new(), String::new()),
                };

                if !diagnosis.is_empty() {
                    results.push(DiagnosisResult {
                        rule_id: format!("PROTO_{}", class.label_en().replace(' ', "_")),
                        severity: Severity::Info,
                        diagnosis,
                        suggestions: vec![suggestion],
                        confidence: ctx.protocol_confidence,
                    });
                }
            }
        }

        // ===== 波特率建议 =====
        if ctx.is_connected && !ctx.last_rx_data.is_empty() {
            let data = &ctx.last_rx_data;
            let printable = data
                .iter()
                .filter(|&&b| (0x20..=0x7E).contains(&b))
                .count();
            let printable_ratio = printable as f32 / data.len() as f32;
            let high_byte = data.iter().filter(|&&b| b >= 0x80).count();
            let high_ratio = high_byte as f32 / data.len() as f32;

            // 乱码特征：高比例高字节 + 低可打印 + 非已知协议
            if high_ratio > 0.4 && printable_ratio < 0.3
                && ctx.protocol_class != Some(ProtocolClass::InsoleSensor)
                && ctx.protocol_class != Some(ProtocolClass::EspImage)
            {
                results.push(DiagnosisResult {
                    rule_id: "BAUD_MISMATCH".to_string(),
                    severity: Severity::Warning,
                    diagnosis: "Data may be garbled (baud rate mismatch?)".to_string(),
                    suggestions: vec![
                        format!("Current baud rate {}; garbled data may indicate a wrong baud rate", ctx.baud_rate),
                        "Try common baud rates: 9600, 19200, 38400, 57600, 115200, 921600".to_string(),
                        "Use the baud rate auto-detect feature in the \"Toolbox\" panel".to_string(),
                        "If the device has documentation, check its default baud configuration".to_string(),
                    ],
                    confidence: 0.55,
                });
            }
        }

        // ===== VID/PID 设备识别建议 =====
        if let Some((vid, pid)) = ctx.vid_pid {
            if let Some((chip, hint)) = lookup_vid_pid(vid, pid) {
                results.push(DiagnosisResult {
                    rule_id: "DEVICE_IDENTIFIED".to_string(),
                    severity: Severity::Info,
                    diagnosis: format!("Identified USB bridge chip: {} (VID={:04X} PID={:04X})", chip, vid, pid),
                    suggestions: vec![hint],
                    confidence: 0.9,
                });
            }
        }

        // ===== 串扰检测诊断（阶段1 新增）=====
        if let Some(ct) = &ctx.crosstalk {
            // TX_LOOPBACK：精确回环检测
            if ct.is_loopback {
                let is_ch340 = ctx.vid_pid.map_or(false, |(vid, _)| vid == 0x1A86);
                let mut suggestions = vec![
                    "CH340 TX/RX may be shorted or the device is unpowered; RX is receiving TX self-loopback crosstalk".to_string(),
                    "Check TX/RX wiring: CH340 RX should connect to device TX; CH340 TX to device RX".to_string(),
                    "Confirm the device is powered and shares GND with CH340".to_string(),
                    "Passive listening (no TX) should receive 0 bytes; any data indicates crosstalk".to_string(),
                ];
                if is_ch340 {
                    suggestions.push(
                        "CH340 tends to produce TX crosstalk at high baud rates (>230400); lower to 115200".to_string(),
                    );
                }
                suggestions.push(format!(
                    "TX/RX ratio: {:.2} (TX={}B, RX={}B), echo_ratio={:.2}",
                    ct.rx_tx_ratio, ct.tx_total, ct.rx_total, ct.echo_ratio
                ));

                results.push(DiagnosisResult {
                    rule_id: "TX_LOOPBACK".to_string(),
                    severity: Severity::Critical,
                    diagnosis: "Detected TX self-loopback (CH340 RX is receiving TX crosstalk, not device data)".to_string(),
                    suggestions,
                    confidence: 0.9,
                });
            }

            // RX_TX_RATIO_HALF：约 50% 串扰
            if ct.is_half_echo() && !ct.is_loopback {
                results.push(DiagnosisResult {
                    rule_id: "RX_TX_RATIO_HALF".to_string(),
                    severity: Severity::Warning,
                    diagnosis: "TX/RX ratio ~50%, suspected TX impedance reflection (device TX not connected to CH340 RX)".to_string(),
                    suggestions: vec![
                        "Received data is ~50% of sent data, a typical TX crosstalk signature".to_string(),
                        "The device TX pin may not be connected to CH340 RX".to_string(),
                        "Check whether the device TX pin is correctly connected to CH340 RX".to_string(),
                        "Try passive listening (no TX); 0 bytes confirms crosstalk".to_string(),
                    ],
                    confidence: 0.75,
                });
            }

            // HIGH_BYTE_DOMINANT：高字节主导（波特率不匹配）
            if ct.high_byte_ratio > 0.4 && ct.pattern == EchoPattern::HighByteDominant {
                let is_ch340 = ctx.vid_pid.map_or(false, |(vid, _)| vid == 0x1A86);
                let is_high_baud = ctx.baud_rate >= 230_400;
                let mut suggestions = vec![
                    format!(
                        "High-byte (≥0x80) ratio {:.0}%, data concentrated at 0xFF/0xFE/0xF3; baud mismatch or CH340 clock drift",
                        ct.high_byte_ratio * 100.0
                    ),
                    "Try other baud rates: 9600, 57600, 115200, 460800, 921600".to_string(),
                ];
                if is_ch340 && is_high_baud {
                    suggestions.push(
                        format!(
                            "CH340 may have clock drift at {} baud; try over-sampling compensation (e.g. {}) or lower to 115200",
                            ctx.baud_rate,
                            ctx.baud_rate * 2
                        ),
                    );
                    suggestions.push(
                        "CH340 macOS driver may down-clock at high baud rates; try setting baud to 2× the nominal value".to_string(),
                    );
                }
                suggestions.push("Use the baud rate auto-detect feature in the \"Toolbox\" panel".to_string());

                results.push(DiagnosisResult {
                    rule_id: "HIGH_BYTE_DOMINANT".to_string(),
                    severity: Severity::Warning,
                    diagnosis: "Data is high-byte dominant (baud mismatch or CH340 clock drift)".to_string(),
                    suggestions,
                    confidence: 0.7,
                });
            }

            // BAUD_DOUBLE_FREQ：倍频特征检测
            if ct.pattern == EchoPattern::BitShifted {
                results.push(DiagnosisResult {
                    rule_id: "BAUD_DOUBLE_FREQ".to_string(),
                    severity: Severity::Warning,
                    diagnosis: "Detected bit-shift match (baud over-sampling mismatch)".to_string(),
                    suggestions: vec![
                        "RX data is a bit-shifted version of TX data; baud may be 1/2 or 2× nominal".to_string(),
                        format!("Try baud rate: {} or {}", ctx.baud_rate / 2, ctx.baud_rate * 2),
                        "CH340 may auto down-clock at high baud; setting 2× baud lets CH340 down-clock to the target".to_string(),
                    ],
                    confidence: 0.65,
                });
            }
        }

        // ===== 固件提示诊断（阶段4 反哺）=====
        if let Some(fw_hint) = &ctx.firmware_hint {
            results.push(DiagnosisResult {
                rule_id: "FW_PERIPHERAL_HINT".to_string(),
                severity: Severity::Info,
                diagnosis: format!("Firmware analysis hint: {}", fw_hint),
                suggestions: vec![
                    "Check peripheral wiring and baud rate against the firmware configuration".to_string(),
                    "View details in the \"Firmware Editor\" panel".to_string(),
                ],
                confidence: 0.8,
            });
        }

        // ===== D.6 固件结构化反哺诊断 =====
        if let Some(fw) = &ctx.firmware_analysis {
            // 收集固件中使用的外设
            let fw_usarts = collect_fw_usarts(fw);
            let fw_baud_hint = extract_baud_hint(fw);

            // FW_PERIPHERAL_MISMATCH：固件使用的 USART 与用户连接的端口不匹配
            if !fw_usarts.is_empty() {
                if let Some(chip) = &ctx.chip_hint {
                    if let Some(mismatch) = check_periph_mismatch(&fw_usarts, chip, ctx.baud_rate) {
                        results.push(mismatch);
                    }
                }
            }

            // FW_DMA_CONFLICT：DMA 通道冲突（同一通道映射到多个外设）
            if let Some(conflict) = check_dma_conflict(&fw.dma_channels) {
                results.push(conflict);
            }

            // FW_BAUD_HINT：固件波特率提示
            if let Some(hint_baud) = fw_baud_hint {
                if ctx.baud_rate != 0 && ctx.baud_rate != hint_baud {
                    let ratio = ctx.baud_rate as f32 / hint_baud as f32;
                    let is_near = (0.9..=1.1).contains(&ratio);

                    if !is_near {
                        let severity = if ctx.baud_rate < hint_baud / 2
                            || ctx.baud_rate > hint_baud * 2
                        {
                            Severity::Warning
                        } else {
                            Severity::Info
                        };

                        results.push(DiagnosisResult {
                            rule_id: "FW_BAUD_HINT".to_string(),
                            severity,
                            diagnosis: format!(
                                "Firmware suggests baud {}; current setting is {}",
                                hint_baud, ctx.baud_rate
                            ),
                            suggestions: vec![
                                format!(
                                    "Firmware analysis suggests the device may use {} baud; current {} may not match",
                                    hint_baud, ctx.baud_rate
                                ),
                                format!("Recommend switching to {} and reconnecting", hint_baud),
                                "If the baud rate is confirmed correct, ignore this hint".to_string(),
                            ],
                            confidence: 0.7,
                        });
                    }
                }
            }
        }

        // 按严重程度排序（Critical > Error > Warning > Info）
        results.sort_by(|a, b| {
            let order = |s: Severity| match s {
                Severity::Critical => 0,
                Severity::Error => 1,
                Severity::Warning => 2,
                Severity::Info => 3,
            };
            order(a.severity).cmp(&order(b.severity))
        });

        results
    }

    /// 获取所有规则
    pub fn rules(&self) -> &[DiagnosticRule] {
        &self.rules
    }
}

/// VID/PID 查表
fn lookup_vid_pid(vid: u16, pid: u16) -> Option<(&'static str, String)> {
    match (vid, pid) {
        (0x1A86, 0x7523) => Some((
            "CH340",
            "CH340 USB-to-serial chip, common on Arduino clones and ESP32 dev boards".to_string(),
        )),
        (0x1A86, 0x5523) => Some((
            "CH341",
            "CH341 USB-to-serial/I2C/SPI chip, usable for I2C/SPI panel operations".to_string(),
        )),
        (0x1A86, 0x5512) => Some((
            "CH341A",
            "CH341A programmer supporting SPI Flash read/write (SPI Flash panel)".to_string(),
        )),
        (0x0403, 0x6001) => Some((
            "FT232R",
            "FTDI FT232R USB-to-serial chip, high-quality serial adapter".to_string(),
        )),
        (0x0403, 0x6014) => Some((
            "FT232H",
            "FTDI FT232H supporting SPI/I2C/JTAG, usable in the SPI Flash panel".to_string(),
        )),
        (0x10C4, 0xEA60) => Some((
            "CP210x",
            "Silicon Labs CP210x USB-to-serial chip, common on ESP32/STM32 dev boards".to_string(),
        )),
        (0x303A, _) => Some((
            "ESP32 Native USB",
            "ESP32 native USB (TinyUF2/USB-CDC), no external bridge chip needed".to_string(),
        )),
        _ => None,
    }
}

/// 内置规则列表（用于 UI 展示）
fn builtin_rules() -> Vec<DiagnosticRule> {
    vec![
        DiagnosticRule { id: "CONN_PERMISSION", description: "Serial permission issue diagnosis" },
        DiagnosticRule { id: "CONN_NO_DEVICE", description: "Device not found diagnosis" },
        DiagnosticRule { id: "CONN_BROKEN_PIPE", description: "Connection lost diagnosis" },
        DiagnosticRule { id: "CONN_IO_ERROR", description: "I/O error diagnosis" },
        DiagnosticRule { id: "CONN_BAD_FD", description: "Invalid file descriptor diagnosis" },
        DiagnosticRule { id: "BOOTLOADER_NO_ACK", description: "Bootloader did not respond with ACK" },
        DiagnosticRule { id: "BOOTLOADER_WRONG_DATA", description: "Bootloader returned wrong data" },
        DiagnosticRule { id: "DATA_ALL_ZERO", description: "Null byte burst diagnosis" },
        DiagnosticRule { id: "DATA_ALL_FF", description: "0xFF fill diagnosis" },
        DiagnosticRule { id: "CONN_NO_DATA", description: "No data received diagnosis" },
        DiagnosticRule { id: "BAUD_MISMATCH", description: "Baud mismatch diagnosis" },
        DiagnosticRule { id: "DEVICE_IDENTIFIED", description: "Device identified hint" },
        // 阶段1 新增：串扰检测规则
        DiagnosticRule { id: "TX_LOOPBACK", description: "TX self-loopback diagnosis" },
        DiagnosticRule { id: "RX_TX_RATIO_HALF", description: "50% crosstalk ratio diagnosis" },
        DiagnosticRule { id: "HIGH_BYTE_DOMINANT", description: "High-byte dominant pattern diagnosis" },
        DiagnosticRule { id: "BAUD_DOUBLE_FREQ", description: "Baud over-sampling mismatch diagnosis" },
        // 阶段4 新增：固件反哺诊断
        DiagnosticRule { id: "FW_PERIPHERAL_HINT", description: "Firmware peripheral hint diagnosis" },
        // D.6 新增：固件结构化反哺诊断
        DiagnosticRule { id: "FW_PERIPHERAL_MISMATCH", description: "Firmware peripheral vs. connection mismatch diagnosis" },
        DiagnosticRule { id: "FW_DMA_CONFLICT", description: "DMA channel conflict diagnosis" },
        DiagnosticRule { id: "FW_BAUD_HINT", description: "Firmware baud hint diagnosis" },
    ]
}

// ============================================================================
// D.6 固件反哺诊断辅助函数
// ============================================================================

/// 从固件分析中提取启用的 USART 外设列表
/// 优先从 IRQ 启用列表和 DMA 通道映射中提取
fn collect_fw_usarts(fw: &FirmwareAnalysis) -> Vec<String> {
    let mut usarts = std::collections::HashSet::new();

    // 从中断向量表提取启用的 USART
    for irq in &fw.irq_table {
        if irq.peripheral.starts_with("USART") || irq.peripheral.starts_with("UART") {
            usarts.insert(irq.peripheral.clone());
        }
    }

    // 从 DMA 通道映射提取 USART
    for dma in &fw.dma_channels {
        if dma.peripheral.starts_with("USART") || dma.peripheral.starts_with("UART") {
            usarts.insert(dma.peripheral.clone());
        }
    }

    usarts.into_iter().collect()
}

/// 从固件分析中提取波特率提示
/// 启发式：根据启用的 USART 外设和常见 STM32 时钟配置推断
/// USART1 在 STM32F3 通常挂 APB2(72MHz)，常见配置 921600 或 115200
/// USART2/USART3 挂 APB1(36MHz)，常见 115200 或 57600
fn extract_baud_hint(fw: &FirmwareAnalysis) -> Option<u32> {
    let usarts = collect_fw_usarts(fw);

    // 优先级：USART1 > USART3 > USART2
    if usarts.iter().any(|u| u == "USART1") {
        return Some(921600);
    }
    if usarts.iter().any(|u| u == "USART3") {
        return Some(115200);
    }
    if usarts.iter().any(|u| u == "USART2") {
        return Some(115200);
    }

    None
}

/// 检查固件外设与用户连接是否匹配
/// chip_hint 格式示例："CH340" 或 "CP210x" 或 "STM32F303"
/// 此规则检查用户是否连接了与固件配置不同的 USART
fn check_periph_mismatch(
    fw_usarts: &[String],
    chip_hint: &str,
    _current_baud: u32,
) -> Option<DiagnosisResult> {
    // 如果固件使用 USART3 但用户连接的是 USART1 引脚（通过 chip_hint 推断）
    // 这里采用简化策略：如果固件只用了 USART2/USART3 而没有 USART1，
    // 提示用户可能需要连接到对应引脚

    let has_usart1 = fw_usarts.iter().any(|u| u == "USART1");
    let has_usart2 = fw_usarts.iter().any(|u| u == "USART2");
    let has_usart3 = fw_usarts.iter().any(|u| u == "USART3");

    // STM32 + CH340 场景：CH340 通常连接到 USART1（PA9/PA10）
    // 如果固件只用 USART3 而没用 USART1，可能接线错误
    if chip_hint.contains("CH340") || chip_hint.contains("CP210x") {
        if !has_usart1 && (has_usart2 || has_usart3) {
            let active_usart = if has_usart2 { "USART2 (PA2/PA3)" } else { "USART3 (PB10/PB11)" };
            return Some(DiagnosisResult {
                rule_id: "FW_PERIPHERAL_MISMATCH".to_string(),
                severity: Severity::Warning,
                diagnosis: format!(
                    "Firmware uses {} for serial communication, but USB bridge chips usually connect to USART1 (PA9/PA10)",
                    active_usart
                ),
                suggestions: vec![
                    format!(
                        "Firmware enables {}, but your CH340/CP210x may be wired to USART1 pins",
                        active_usart
                    ),
                    format!(
                        "Check wiring: CH340 RX should connect to STM32 {} TX pin",
                        active_usart
                    ),
                    "Or modify the firmware to use USART1 for USB communication".to_string(),
                    "View the full peripheral map in the \"Firmware Editor\" panel".to_string(),
                ],
                confidence: 0.7,
            });
        }
    }

    None
}

/// 检查 DMA 通道冲突
/// 规则：同一 DMA 控制器的同一通道号不应映射到多个不同外设
fn check_dma_conflict(channels: &[crate::firmware::DmaChannel]) -> Option<DiagnosisResult> {
    use std::collections::HashMap;
    let mut channel_map: HashMap<(u8, u8), Vec<&str>> = HashMap::new();

    for ch in channels {
        if ch.channel == 0 {
            continue; // 通道号 0 表示基地址关联，跳过
        }
        let key = (ch.dma_controller, ch.channel);
        channel_map
            .entry(key)
            .or_default()
            .push(ch.peripheral.as_str());
    }

    // 找到有冲突的通道（同一通道映射到 ≥2 个不同外设）
    let mut conflicts: Vec<String> = Vec::new();
    for ((controller, channel), periphs) in &channel_map {
        let unique: std::collections::HashSet<&&str> = periphs.iter().collect();
        if unique.len() >= 2 {
            conflicts.push(format!(
                "DMA{} channel{}: {}",
                controller,
                channel,
                periphs.join(", ")
            ));
        }
    }

    if !conflicts.is_empty() {
        Some(DiagnosisResult {
            rule_id: "FW_DMA_CONFLICT".to_string(),
            severity: Severity::Warning,
            diagnosis: format!("Detected DMA channel conflict: {}", conflicts.join("; ")),
            suggestions: vec![
                "The same DMA channel is configured for multiple peripherals, which may cause data corruption".to_string(),
                "Check the firmware DMA init code to ensure each channel serves only one peripheral".to_string(),
                "View full DMA channel allocation in the \"Firmware Editor → Peripheral Map\" panel".to_string(),
            ],
            confidence: 0.6,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware::{DmaChannel, FirmwareAnalysis, FirmwareArch, IrqPeripheral};

    /// 构造测试用固件分析结果
    fn make_fw(
        irq_periphs: Vec<&str>,
        dma_periphs: Vec<(&str, u8, u8)>,
    ) -> FirmwareAnalysis {
        FirmwareAnalysis {
            arch: FirmwareArch::ArmCortexM,
            entry_addr: Some(0x08000000),
            sp_init: Some(0x20001000),
            size: 65536,
            strings: Vec::new(),
            code_ratio: 0.7,
            data_ratio: 0.2,
            pad_ratio: 0.1,
            confidence: 0.85,
            summary_zh: "测试固件".to_string(),
            summary_en: "Test firmware".to_string(),
            dma_channels: dma_periphs
                .iter()
                .map(|(p, ctrl, ch)| DmaChannel {
                    dma_controller: *ctrl,
                    channel: *ch,
                    periph_addr: 0,
                    peripheral: p.to_string(),
                    confidence: 0.7,
                })
                .collect(),
            irq_table: irq_periphs
                .iter()
                .enumerate()
                .map(|(i, p)| IrqPeripheral {
                    irq_num: i,
                    handler_addr: 0x08000000 + (i as u32) * 4,
                    peripheral: p.to_string(),
                    enabled: true,
                })
                .collect(),
            gpio_config: Vec::new(),
        }
    }

    #[test]
    fn test_collect_fw_usarts_from_irq() {
        let fw = make_fw(vec!["USART1", "TIM3", "DMA1_Ch1"], vec![]);
        let usarts = collect_fw_usarts(&fw);
        assert_eq!(usarts.len(), 1);
        assert!(usarts.contains(&"USART1".to_string()));
    }

    #[test]
    fn test_collect_fw_usarts_from_dma() {
        let fw = make_fw(vec![], vec![("USART3", 1, 2), ("ADC1", 1, 1)]);
        let usarts = collect_fw_usarts(&fw);
        assert_eq!(usarts.len(), 1);
        assert!(usarts.contains(&"USART3".to_string()));
    }

    #[test]
    fn test_extract_baud_hint_usart1() {
        let fw = make_fw(vec!["USART1"], vec![]);
        assert_eq!(extract_baud_hint(&fw), Some(921600));
    }

    #[test]
    fn test_extract_baud_hint_usart3() {
        let fw = make_fw(vec!["USART3"], vec![]);
        assert_eq!(extract_baud_hint(&fw), Some(115200));
    }

    #[test]
    fn test_extract_baud_hint_no_usart() {
        let fw = make_fw(vec!["TIM3"], vec![]);
        assert_eq!(extract_baud_hint(&fw), None);
    }

    #[test]
    fn test_check_periph_mismatch_ch340_without_usart1() {
        // 固件用 USART3，但 CH340 通常连 USART1 → 应触发警告
        let fw_usarts = vec!["USART3".to_string()];
        let result = check_periph_mismatch(&fw_usarts, "CH340", 115200);
        assert!(result.is_some());
        let diag = result.unwrap();
        assert_eq!(diag.rule_id, "FW_PERIPHERAL_MISMATCH");
        assert!(diag.diagnosis.contains("USART3"));
    }

    #[test]
    fn test_check_periph_mismatch_ch340_with_usart1() {
        // 固件用 USART1，CH340 连 USART1 → 不触发
        let fw_usarts = vec!["USART1".to_string()];
        let result = check_periph_mismatch(&fw_usarts, "CH340", 921600);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_dma_conflict_no_conflict() {
        // 每个通道只映射到一个外设 → 无冲突
        let fw = make_fw(vec![], vec![("USART1", 1, 4), ("USART3", 1, 2)]);
        let result = check_dma_conflict(&fw.dma_channels);
        assert!(result.is_none());
    }

    #[test]
    fn test_check_dma_conflict_with_conflict() {
        // 同一 DMA1 通道 2 映射到 USART3 和 ADC1 → 冲突
        let fw = make_fw(
            vec![],
            vec![("USART3", 1, 2), ("ADC1", 1, 2)],
        );
        let result = check_dma_conflict(&fw.dma_channels);
        assert!(result.is_some());
        let diag = result.unwrap();
        assert_eq!(diag.rule_id, "FW_DMA_CONFLICT");
        assert!(diag.diagnosis.contains("DMA1 channel2"));
    }

    #[test]
    fn test_fw_baud_hint_rule_triggers() {
        // 固件用 USART1 (推断 921600)，当前 baud=115200 → 应触发 FW_BAUD_HINT
        let fw = make_fw(vec!["USART1"], vec![]);
        let ctx = DiagnosticContext {
            is_connected: true,
            baud_rate: 115200,
            chip_hint: Some("CH340".to_string()),
            firmware_analysis: Some(fw),
            ..Default::default()
        };
        let engine = DiagnosticEngine::with_builtin_rules();
        let results = engine.diagnose(&ctx);
        let baud_hint = results.iter().find(|r| r.rule_id == "FW_BAUD_HINT");
        assert!(baud_hint.is_some());
        let diag = baud_hint.unwrap();
        assert!(diag.diagnosis.contains("921600"));
        assert!(diag.diagnosis.contains("115200"));
    }

    #[test]
    fn test_fw_baud_hint_rule_no_trigger_when_matched() {
        // 固件推断 921600，当前 baud=921600 → 不触发
        let fw = make_fw(vec!["USART1"], vec![]);
        let ctx = DiagnosticContext {
            is_connected: true,
            baud_rate: 921600,
            chip_hint: Some("CH340".to_string()),
            firmware_analysis: Some(fw),
            ..Default::default()
        };
        let engine = DiagnosticEngine::with_builtin_rules();
        let results = engine.diagnose(&ctx);
        let baud_hint = results.iter().find(|r| r.rule_id == "FW_BAUD_HINT");
        assert!(baud_hint.is_none());
    }

    #[test]
    fn test_fw_peripheral_mismatch_rule_triggers() {
        // 固件只用 USART3，CH340 通常连 USART1 → 应触发
        let fw = make_fw(vec!["USART3", "TIM3"], vec![]);
        let ctx = DiagnosticContext {
            is_connected: true,
            baud_rate: 115200,
            chip_hint: Some("CH340".to_string()),
            firmware_analysis: Some(fw),
            ..Default::default()
        };
        let engine = DiagnosticEngine::with_builtin_rules();
        let results = engine.diagnose(&ctx);
        let mismatch = results.iter().find(|r| r.rule_id == "FW_PERIPHERAL_MISMATCH");
        assert!(mismatch.is_some());
    }

    #[test]
    fn test_fw_dma_conflict_rule_triggers() {
        let fw = make_fw(vec![], vec![("USART1", 1, 4), ("ADC1", 1, 4)]);
        let ctx = DiagnosticContext {
            is_connected: true,
            baud_rate: 921600,
            firmware_analysis: Some(fw),
            ..Default::default()
        };
        let engine = DiagnosticEngine::with_builtin_rules();
        let results = engine.diagnose(&ctx);
        let conflict = results.iter().find(|r| r.rule_id == "FW_DMA_CONFLICT");
        assert!(conflict.is_some());
    }
}
