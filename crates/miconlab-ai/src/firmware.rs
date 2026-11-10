//! 固件智能分析
//!
//! 对二进制固件数据进行结构识别，推断芯片架构、入口地址、内存布局。
//! 纯启发式分析，离线运行，无需网络。

use serde::{Deserialize, Serialize};

/// 固件架构类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirmwareArch {
    /// ARM Cortex-M（向量表识别）
    ArmCortexM,
    /// ESP32/ESP8266 镜像（0xE9 魔数）
    EspImage,
    /// Intel HEX 文本格式
    IntelHex,
    /// AVR/ATmega（猜测）
    Avr,
    /// 未知/原始二进制
    Unknown,
}

impl FirmwareArch {
    pub fn label_zh(&self) -> &'static str {
        match self {
            FirmwareArch::ArmCortexM => "ARM Cortex-M",
            FirmwareArch::EspImage => "ESP Image",
            FirmwareArch::IntelHex => "Intel HEX",
            FirmwareArch::Avr => "AVR/ATmega",
            FirmwareArch::Unknown => "Unknown",
        }
    }

    pub fn label_en(&self) -> &'static str {
        match self {
            FirmwareArch::ArmCortexM => "ARM Cortex-M",
            FirmwareArch::EspImage => "ESP Image",
            FirmwareArch::IntelHex => "Intel HEX",
            FirmwareArch::Avr => "AVR/ATmega",
            FirmwareArch::Unknown => "Unknown",
        }
    }
}

// ============================================================================
// DMA 通道映射
// ============================================================================

/// DMA 通道映射信息（启发式从固件代码扫描得出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmaChannel {
    /// DMA 控制器编号（1 或 2）
    pub dma_controller: u8,
    /// 通道号（1-7）
    pub channel: u8,
    /// 外设数据寄存器地址（CPAR 目标值）
    pub periph_addr: u32,
    /// 推断的外设名称（如 "USART1"）
    pub peripheral: String,
    /// 置信度 (0.0-1.0)
    pub confidence: f32,
}

// ============================================================================
// 中断向量表
// ============================================================================

/// 中断向量表条目（外设相关）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrqPeripheral {
    /// IRQ 编号（0-48，对应向量表索引 16-64）
    pub irq_num: usize,
    /// 中断处理函数地址（Flash 中的 handler 入口）
    pub handler_addr: u32,
    /// 推断的外设名称
    pub peripheral: String,
    /// 是否被启用（handler 指向有效 Flash 地址且非默认值）
    pub enabled: bool,
}

// ============================================================================
// GPIO 配置
// ============================================================================

/// GPIO 模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GpioMode {
    Input,
    Output,
    AlternateFunction,
    Analog,
}

/// GPIO 引脚配置（启发式从固件代码扫描得出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpioPin {
    /// 端口（A/B/C/D/E/F）
    pub port: char,
    /// 引脚号（0-15）
    pub pin: u8,
    /// 模式
    pub mode: GpioMode,
    /// 复用功能编号（0-15）
    pub af: u8,
    /// 推断的标签/外设用途
    pub label: Option<String>,
}

// ============================================================================
// 固件分析结果
// ============================================================================

/// 固件分析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirmwareAnalysis {
    /// 推断的架构
    pub arch: FirmwareArch,
    /// 推断的入口地址（复位向量）
    pub entry_addr: Option<u32>,
    /// 推断的栈指针初始值
    pub sp_init: Option<u32>,
    /// 固件大小（字节）
    pub size: usize,
    /// 提取的可读字符串（最多 16 条，每条最多 64 字符）
    pub strings: Vec<String>,
    /// 代码区比例 (0.0-1.0)
    pub code_ratio: f32,
    /// 数据区比例 (0.0-1.0)
    pub data_ratio: f32,
    /// 填充字节比例 (0.0-1.0，0xFF/0x00 填充)
    pub pad_ratio: f32,
    /// 置信度 (0.0-1.0)
    pub confidence: f32,
    /// 分析摘要（中文）
    pub summary_zh: String,
    /// 分析摘要（英文）
    pub summary_en: String,
    // ---- D.6 新增：外设映射 ----
    /// 推断的 DMA 通道映射
    pub dma_channels: Vec<DmaChannel>,
    /// 推断的中断向量表（启用的外设中断）
    pub irq_table: Vec<IrqPeripheral>,
    /// 推断的 GPIO 配置
    pub gpio_config: Vec<GpioPin>,
}

impl FirmwareAnalysis {
    /// 分析固件数据
    pub fn analyze(data: &[u8]) -> Self {
        let size = data.len();
        if size < 4 {
            return Self {
                arch: FirmwareArch::Unknown,
                entry_addr: None,
                sp_init: None,
                size,
                strings: Vec::new(),
                code_ratio: 0.0,
                data_ratio: 0.0,
                pad_ratio: 0.0,
                confidence: 0.0,
                summary_zh: "Data too short to analyze".to_string(),
                summary_en: "Data too short to analyze".to_string(),
                dma_channels: Vec::new(),
                irq_table: Vec::new(),
                gpio_config: Vec::new(),
            };
        }

        // ---- 架构识别 ----
        let (arch, entry_addr, sp_init, conf) = identify_arch(data);

        // ---- 代码/数据/填充比例 ----
        let (code_ratio, data_ratio, pad_ratio) = analyze_sections(data);

        // ---- 字符串提取 ----
        let strings = extract_strings(data, 16, 64);

        // ---- D.6：外设映射扫描（仅 ARM Cortex-M 启用）----
        let (dma_channels, irq_table, gpio_config) = if arch == FirmwareArch::ArmCortexM {
            let dma = scan_dma_channels(data);
            let irq = scan_irq_table(data);
            let gpio = scan_gpio_config(data);
            (dma, irq, gpio)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        // ---- 摘要 ----
        let (summary_zh, summary_en) = build_summary(&arch, size, entry_addr, sp_init, &strings, conf);

        Self {
            arch,
            entry_addr,
            sp_init,
            size,
            strings,
            code_ratio,
            data_ratio,
            pad_ratio,
            confidence: conf,
            summary_zh,
            summary_en,
            dma_channels,
            irq_table,
            gpio_config,
        }
    }
}

/// 识别固件架构
fn identify_arch(data: &[u8]) -> (FirmwareArch, Option<u32>, Option<u32>, f32) {
    // ESP 镜像魔数 0xE9
    if data[0] == 0xE9 && data.len() >= 8 {
        // ESP32 镜像头：byte0=0xE9, byte1=段数, byte2-3=SPI mode/size
        return (FirmwareArch::EspImage, Some(0x1000), None, 0.9);
    }

    // Intel HEX：以 ':' 开头
    if data[0] == b':' {
        return (FirmwareArch::IntelHex, None, None, 0.95);
    }

    // ARM Cortex-M 向量表：
    // - 前 4 字节 = 初始栈指针（通常 0x2000xxxx，SRAM 区域）
    // - 第 2 个字 = 复位向量（地址低位，bit0=1 表示 Thumb 指令）
    if data.len() >= 8 {
        let sp = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let reset_vec = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

        // 栈指针在 SRAM 区域（0x20000000-0x200FFFFF）
        let sp_valid = (0x20000000..=0x20100000).contains(&sp);
        // 复位向量在 Flash 区域（0x08000000-0x081FFFFF 或 0x00000000-0x001FFFFF），且 bit0=1（Thumb）
        let reset_in_flash = (0x08000000..=0x081FFFFF).contains(&(reset_vec & !1))
            || (0x00000000..=0x001FFFFF).contains(&(reset_vec & !1));
        let thumb = (reset_vec & 1) == 1;

        if sp_valid && reset_in_flash && thumb {
            let entry = reset_vec & !1;
            return (FirmwareArch::ArmCortexM, Some(entry), Some(sp), 0.85);
        }
        // 部分匹配（sp 有效但 reset 不完全匹配），降低置信度
        if sp_valid && thumb {
            let entry = reset_vec & !1;
            return (FirmwareArch::ArmCortexM, Some(entry), Some(sp), 0.5);
        }
    }

    // AVR 启发式：大量跳转指令（0x0C/0x94 开头的 RJMP/ICALL）
    // 简单判断：前 64 字节中有很多 0x94/0x95 字节
    if data.len() >= 64 {
        let avr_like = data[..64]
            .chunks(2)
            .filter(|c| c.len() == 2 && (c[1] == 0x94 || c[1] == 0x95 || c[1] == 0x0C))
            .count();
        if avr_like >= 8 {
            return (FirmwareArch::Avr, None, None, 0.4);
        }
    }

    (FirmwareArch::Unknown, None, None, 0.2)
}

/// 分析代码/数据/填充比例
fn analyze_sections(data: &[u8]) -> (f32, f32, f32) {
    let n = data.len() as f32;
    if n == 0.0 {
        return (0.0, 0.0, 0.0);
    }

    // 填充字节（0x00 或 0xFF 连续 >= 8 字节）
    let mut pad_count = 0u32;
    let mut run = 1u32;
    for i in 1..data.len() {
        if data[i] == data[i - 1] && (data[i] == 0x00 || data[i] == 0xFF) {
            run += 1;
        } else {
            if run >= 8 {
                pad_count += run;
            }
            run = 1;
        }
    }
    if run >= 8 {
        pad_count += run;
    }
    let pad_ratio = pad_count as f32 / n;

    // 代码比例：非填充、非纯 ASCII 的字节比例
    let printable = data
        .iter()
        .filter(|&&b| (0x20..=0x7E).contains(&b))
        .count() as f32;
    let data_ratio = (printable / n).min(0.5); // 数据区最多算 50%
    let code_ratio = (1.0 - pad_ratio - data_ratio).max(0.0);

    (code_ratio, data_ratio, pad_ratio)
}

/// 提取可读字符串（连续 >= 4 个可打印字符）
fn extract_strings(data: &[u8], max_count: usize, max_len: usize) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = Vec::new();

    for &b in data {
        if (0x20..=0x7E).contains(&b) {
            current.push(b);
        } else {
            if current.len() >= 4 {
                let s = String::from_utf8_lossy(&current)
                    .chars()
                    .take(max_len)
                    .collect::<String>();
                if !strings.contains(&s) {
                    strings.push(s);
                    if strings.len() >= max_count {
                        break;
                    }
                }
            }
            current.clear();
        }
    }
    // 处理末尾
    if current.len() >= 4 && strings.len() < max_count {
        let s = String::from_utf8_lossy(&current)
            .chars()
            .take(max_len)
            .collect::<String>();
        if !strings.contains(&s) {
            strings.push(s);
        }
    }

    strings
}

/// 构建分析摘要
fn build_summary(
    arch: &FirmwareArch,
    size: usize,
    entry: Option<u32>,
    sp: Option<u32>,
    strings: &[String],
    conf: f32,
) -> (String, String) {
    let size_str = if size >= 1024 {
        format!("{}KB", size / 1024)
    } else {
        format!("{}B", size)
    };

    let arch_label = arch.label_zh();
    let arch_label_en = arch.label_en();

    let entry_str = entry
        .map(|e| format!("0x{:08X}", e))
        .unwrap_or_else(|| "unknown".to_string());
    let entry_str_en = entry
        .map(|e| format!("0x{:08X}", e))
        .unwrap_or_else(|| "unknown".to_string());

    let sp_str = sp
        .map(|s| format!("0x{:08X}", s))
        .unwrap_or_else(|| "unknown".to_string());
    let sp_str_en = sp
        .map(|s| format!("0x{:08X}", s))
        .unwrap_or_else(|| "unknown".to_string());

    let str_count = strings.len();

    let zh = format!(
        "{} firmware ({}), entry: {}, SP: {}, {} strings extracted, confidence {:.0}%",
        arch_label, size_str, entry_str, sp_str, str_count, conf * 100.0
    );
    let en = format!(
        "{} firmware ({}), entry: {}, SP: {}, {} strings extracted, confidence {:.0}%",
        arch_label_en, size_str, entry_str_en, sp_str_en, str_count, conf * 100.0
    );

    (zh, en)
}

// ============================================================================
// D.6 外设映射启发式扫描
// ============================================================================

/// 已知外设数据寄存器地址（用于 DMA CPAR 匹配）
/// 覆盖 STM32F1/F3 系列常用外设
const PERIPH_DR_TABLE: &[(u32, &str)] = &[
    // USART 数据寄存器 (DR = base + 0x04)
    (0x40013804, "USART1"),
    (0x40004404, "USART2"),
    (0x40004804, "USART3"),
    (0x40004C04, "UART4"),
    (0x40005004, "UART5"),
    // ADC 数据寄存器 (F3 系列 AHB)
    (0x50000040, "ADC1"),
    (0x50000044, "ADC2"),
    (0x50000440, "ADC3"),
    // ADC 数据寄存器 (F1 兼容)
    (0x4001244C, "ADC1_F1"),
    (0x4001284C, "ADC2_F1"),
    // TIM 捕获/比较寄存器 (CCR1 = base + 0x34)
    (0x40000434, "TIM3_CCR1"),
    (0x40000438, "TIM3_CCR2"),
    (0x40000834, "TIM4_CCR1"),
    (0x40000838, "TIM4_CCR2"),
    // SPI 数据寄存器
    (0x4001300C, "SPI1"),
    (0x4000380C, "SPI2"),
    // I2C 数据寄存器
    (0x40005410, "I2C1"),
    (0x40005810, "I2C2"),
];

/// DMA1 通道 CPAR 寄存器地址（CPAR = base + 0x14*(ch-1) + 0x10）
const DMA1_CPAR_TABLE: &[(u32, u8)] = &[
    (0x40020010, 1),
    (0x40020024, 2),
    (0x40020038, 3),
    (0x4002004C, 4),
    (0x40020060, 5),
    (0x40020074, 6),
    (0x40020088, 7),
];

/// DMA2 通道 CPAR 寄存器地址
const DMA2_CPAR_TABLE: &[(u32, u8)] = &[
    (0x40020410, 1),
    (0x40020424, 2),
    (0x40020438, 3),
    (0x4002044C, 4),
    (0x40020460, 5),
];

/// GPIO 端口 MODER 寄存器地址（F3 AHB1 + F1 APB2 兼容）
const GPIO_MODER_TABLE: &[(u32, char)] = &[
    // F3 系列（AHB1）
    (0x48000000, 'A'),
    (0x48000400, 'B'),
    (0x48000800, 'C'),
    (0x48000C00, 'D'),
    (0x48001000, 'E'),
    (0x48001400, 'F'),
    // F1 系列（APB2，用 CRL 代替 MODER，偏移相同）
    (0x40010800, 'A'),
    (0x40010C00, 'B'),
    (0x40011000, 'C'),
    (0x40011400, 'D'),
    (0x40011800, 'E'),
];

/// IRQ 编号到外设名称映射（STM32F3）
fn irq_periph_name(irq: usize) -> &'static str {
    match irq {
        0 => "WWDG",
        1 => "PVD",
        2 => "TAMPER",
        3 => "RTC",
        4 => "FLASH",
        5 => "RCC",
        6 => "EXTI0",
        7 => "EXTI1",
        8 => "EXTI2",
        9 => "EXTI3",
        10 => "EXTI4",
        11 => "DMA1_Ch1",
        12 => "DMA1_Ch2",
        13 => "DMA1_Ch3",
        14 => "DMA1_Ch4",
        15 => "DMA1_Ch5",
        16 => "DMA1_Ch6",
        17 => "DMA1_Ch7",
        18 => "ADC1_2",
        19 => "USB_HP_CAN_TX",
        20 => "USB_LP_CAN_RX0",
        21 => "CAN_RX1",
        22 => "CAN_SCE",
        23 => "EXTI9_5",
        24 => "TIM1_BRK",
        25 => "TIM1_UP",
        26 => "TIM1_TRG_COM",
        27 => "TIM1_CC",
        28 => "TIM2",
        29 => "TIM3",
        30 => "TIM4",
        31 => "I2C1_EV",
        32 => "I2C1_ER",
        33 => "I2C2_EV",
        34 => "I2C2_ER",
        35 => "SPI1",
        36 => "SPI2",
        37 => "USART1",
        38 => "USART2",
        39 => "USART3",
        40 => "EXTI15_10",
        41 => "RTCAlarm",
        42 => "USBWakeUp",
        43 => "TIM8_BRK",
        44 => "TIM8_UP",
        45 => "TIM8_TRG_COM",
        46 => "TIM8_CC",
        47 => "ADC3",
        _ => "Unknown",
    }
}

/// 在 bin 中查找目标 4 字节地址常量的所有出现偏移
/// 步长 2 字节（Thumb 指令 16-bit 对齐，常量池也是 16-bit 对齐）
fn find_addr_offsets(data: &[u8], target: u32) -> Vec<usize> {
    let mut offsets = Vec::new();
    if data.len() < 4 {
        return offsets;
    }
    let bytes = target.to_le_bytes();
    let mut i = 0;
    while i + 4 <= data.len() {
        if data[i] == bytes[0]
            && data[i + 1] == bytes[1]
            && data[i + 2] == bytes[2]
            && data[i + 3] == bytes[3]
        {
            offsets.push(i);
        }
        i += 2;
    }
    offsets
}

/// 扫描 DMA 通道配置
/// 启发式：
/// 1. 优先搜索 DMA 通道 CPAR 寄存器地址（精确到通道号）
/// 2. 补充搜索 DMA 基地址（0x40020000/0x40020400），关联附近外设 DR
/// 固件常量池中 DMA 基地址与外设 DR 地址通常相邻（≤64 字节）
fn scan_dma_channels(data: &[u8]) -> Vec<DmaChannel> {
    let mut channels = Vec::new();

    // 收集所有外设 DR 地址的出现位置
    let periph_hits: Vec<(usize, u32, &str)> = PERIPH_DR_TABLE
        .iter()
        .flat_map(|(addr, name)| {
            find_addr_offsets(data, *addr)
                .into_iter()
                .map(move |off| (off, *addr, *name))
        })
        .collect();

    // 策略1：搜索 DMA 通道 CPAR 寄存器地址（精确到通道）
    for &(cpar_addr, ch) in DMA1_CPAR_TABLE {
        let cpar_offsets = find_addr_offsets(data, cpar_addr);
        if cpar_offsets.is_empty() {
            continue;
        }

        let nearby = find_all_nearby_periphs(&cpar_offsets, &periph_hits, 64);
        if nearby.is_empty() {
            channels.push(DmaChannel {
                dma_controller: 1,
                channel: ch,
                periph_addr: 0,
                peripheral: "Unknown".to_string(),
                confidence: 0.35,
            });
        } else {
            for (_, addr, name) in nearby {
                channels.push(DmaChannel {
                    dma_controller: 1,
                    channel: ch,
                    periph_addr: addr,
                    peripheral: name.to_string(),
                    confidence: 0.7,
                });
            }
        }
    }

    for &(cpar_addr, ch) in DMA2_CPAR_TABLE {
        let cpar_offsets = find_addr_offsets(data, cpar_addr);
        if cpar_offsets.is_empty() {
            continue;
        }

        let nearby = find_all_nearby_periphs(&cpar_offsets, &periph_hits, 64);
        if nearby.is_empty() {
            channels.push(DmaChannel {
                dma_controller: 2,
                channel: ch,
                periph_addr: 0,
                peripheral: "Unknown".to_string(),
                confidence: 0.35,
            });
        } else {
            for (_, addr, name) in nearby {
                channels.push(DmaChannel {
                    dma_controller: 2,
                    channel: ch,
                    periph_addr: addr,
                    peripheral: name.to_string(),
                    confidence: 0.7,
                });
            }
        }
    }

    // 策略2：搜索 DMA 基地址引用（补充策略1的遗漏）
    // 固件通过基地址+偏移访问通道寄存器，不直接引用 CPAR 地址
    let dma1_base_hits = find_addr_offsets(data, 0x40020000);
    let dma2_base_hits = find_addr_offsets(data, 0x40020400);

    let has_dma1 = channels.iter().any(|c| c.dma_controller == 1);
    let has_dma2 = channels.iter().any(|c| c.dma_controller == 2);

    if has_dma1 && !dma1_base_hits.is_empty() {
        // DMA1 已通过 CPAR 命中，但基地址可能关联更多外设
        let mut existing: std::collections::HashSet<u32> = channels
            .iter()
            .filter(|c| c.dma_controller == 1)
            .map(|c| c.periph_addr)
            .collect();
        let nearby = find_all_nearby_periphs(&dma1_base_hits, &periph_hits, 64);
        for (_, addr, name) in nearby {
            if existing.insert(addr) {
                channels.push(DmaChannel {
                    dma_controller: 1,
                    channel: 0,
                    periph_addr: addr,
                    peripheral: name.to_string(),
                    confidence: 0.5,
                });
            }
        }
    } else if !has_dma1 && !dma1_base_hits.is_empty() {
        let nearby = find_all_nearby_periphs(&dma1_base_hits, &periph_hits, 64);
        if nearby.is_empty() {
            channels.push(DmaChannel {
                dma_controller: 1,
                channel: 0,
                periph_addr: 0,
                peripheral: "Unknown".to_string(),
                confidence: 0.3,
            });
        } else {
            for (_, addr, name) in nearby {
                channels.push(DmaChannel {
                    dma_controller: 1,
                    channel: 0,
                    periph_addr: addr,
                    peripheral: name.to_string(),
                    confidence: 0.5,
                });
            }
        }
    }

    if has_dma2 && !dma2_base_hits.is_empty() {
        let mut existing: std::collections::HashSet<u32> = channels
            .iter()
            .filter(|c| c.dma_controller == 2)
            .map(|c| c.periph_addr)
            .collect();
        let nearby = find_all_nearby_periphs(&dma2_base_hits, &periph_hits, 64);
        for (_, addr, name) in nearby {
            if existing.insert(addr) {
                channels.push(DmaChannel {
                    dma_controller: 2,
                    channel: 0,
                    periph_addr: addr,
                    peripheral: name.to_string(),
                    confidence: 0.5,
                });
            }
        }
    } else if !has_dma2 && !dma2_base_hits.is_empty() {
        let nearby = find_all_nearby_periphs(&dma2_base_hits, &periph_hits, 64);
        if nearby.is_empty() {
            channels.push(DmaChannel {
                dma_controller: 2,
                channel: 0,
                periph_addr: 0,
                peripheral: "Unknown".to_string(),
                confidence: 0.3,
            });
        } else {
            for (_, addr, name) in nearby {
                channels.push(DmaChannel {
                    dma_controller: 2,
                    channel: 0,
                    periph_addr: addr,
                    peripheral: name.to_string(),
                    confidence: 0.5,
                });
            }
        }
    }

    channels
}

/// 在地址引用附近找所有外设 DR 引用（去重）
fn find_all_nearby_periphs<'a>(
    offsets: &[usize],
    periph_hits: &[(usize, u32, &'a str)],
    max_dist: i64,
) -> Vec<(usize, u32, &'a str)> {
    let mut results = Vec::new();
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();

    for &off in offsets {
        for &(periph_off, addr, name) in periph_hits {
            let dist = (off as i64 - periph_off as i64).abs();
            if dist < max_dist && seen.insert(addr) {
                results.push((periph_off, addr, name));
            }
        }
    }

    results
}

/// 扫描中断向量表（ARM Cortex-M）
/// 向量表前 16 项为系统异常，从索引 16 开始为外设 IRQ
/// handler 地址在 Flash 区域（0x08000000-0x081FFFFF）且非默认值 → IRQ 被启用
/// 过滤策略：统计 handler 地址出现频率，>5 次视为默认空 handler
fn scan_irq_table(data: &[u8]) -> Vec<IrqPeripheral> {
    let mut irqs = Vec::new();

    if data.len() < 64 * 4 {
        return irqs;
    }

    let max_index = (data.len() / 4).min(64);

    // 先统计所有 handler 地址出现频率，识别默认 handler
    let mut handler_counts: std::collections::HashMap<u32, usize> =
        std::collections::HashMap::new();
    for i in 16..max_index {
        let offset = i * 4;
        if offset + 4 > data.len() {
            break;
        }
        let handler = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        *handler_counts.entry(handler).or_insert(0) += 1;
    }

    // 默认 handler 阈值：同一地址出现 >5 次视为默认空 handler
    const DEFAULT_HANDLER_THRESHOLD: usize = 5;

    for i in 16..max_index {
        let offset = i * 4;
        if offset + 4 > data.len() {
            break;
        }

        let handler = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);

        let in_flash = (0x08000000..=0x081FFFFF).contains(&(handler & 0xFFFF_FFFE));
        let is_zero = handler == 0 || handler == 0xFFFFFFFF;
        let count = handler_counts.get(&handler).copied().unwrap_or(0);
        let is_default_handler = count > DEFAULT_HANDLER_THRESHOLD;

        if in_flash && !is_zero && !is_default_handler {
            let irq_num = i - 16;
            irqs.push(IrqPeripheral {
                irq_num,
                handler_addr: handler,
                peripheral: irq_periph_name(irq_num).to_string(),
                enabled: true,
            });
        }
    }

    irqs
}

/// 扫描 GPIO 配置
/// 启发式：搜索 GPIO 端口 MODER 寄存器地址常量
/// 精确到引脚级需要反汇编 MODER 写入的立即数，此处给出端口级摘要
fn scan_gpio_config(data: &[u8]) -> Vec<GpioPin> {
    let mut pins = Vec::new();
    let mut seen_ports = std::collections::HashSet::new();

    for &(moder_addr, port) in GPIO_MODER_TABLE {
        let offsets = find_addr_offsets(data, moder_addr);
        if !offsets.is_empty() && seen_ports.insert(port) {
            pins.push(GpioPin {
                port,
                pin: 0,
                mode: GpioMode::AlternateFunction,
                af: 0,
                label: Some(format!("Port referenced ({} occurrence(s))", offsets.len())),
            });
        }
    }

    pins
}
