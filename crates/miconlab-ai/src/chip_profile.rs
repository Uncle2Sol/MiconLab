//! 芯片特征指纹数据库
//!
//! 为不同芯片/固件建立特征指纹，用于：
//! - 自动识别新 bin 文件属于哪个已知芯片/固件
//! - 隔离不同芯片的反编译参数（基地址、向量表大小等）
//! - 隔离不同固件的标定逻辑解读（SRAM 地址、函数地址等）
//! - 输出文件命名隔离（{profile_id}_ 前缀）
//!
//! 指纹匹配基于多维特征：
//! - Flash 基地址和大小
//! - SRAM 布局（SP 初始值范围）
//! - Reset Handler 地址范围
//! - 向量表大小
//! - 关键函数签名（可选）

/// 芯片指令集架构（决定反汇编器选择）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipArch {
    /// ARM Thumb-16 / Thumb-2 (Cortex-M 系列)
    ArmThumb,
    /// Xtensa LX6/LX7 (ESP32/ESP32-S2/S3 系列)
    Xtensa,
    /// AVR (ATmega 系列)
    Avr,
    /// RISC-V (CH32V 等)
    RiscV,
    /// 未知架构
    Unknown,
}

/// 已知函数签名（用于精确匹配固件版本）
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    /// 函数名称
    pub name: &'static str,
    /// 函数入口地址（Flash 绝对地址）
    pub entry: u32,
    /// 函数前 N 字节的哈希（可选，用于版本精确匹配）
    pub hash: Option<u32>,
}

/// 标定参数布局（描述固件中标定参数在 SRAM 中的位置）
#[derive(Debug, Clone)]
pub struct CalibrationLayout {
    /// 标定参数区起始 SRAM 地址
    pub sram_base: u32,
    /// 标定参数区长度（字节）
    pub size: usize,
    /// 关键参数偏移描述（offset, name）
    pub params: &'static [(u16, &'static str)],
}

/// 芯片特征指纹
#[derive(Debug, Clone)]
pub struct ChipProfile {
    /// 指纹唯一标识（用于文件名前缀，如 "mx"、"unknown"）
    pub id: &'static str,
    /// 芯片族（如 "STM32F1"、"ESP32"）
    pub chip_family: &'static str,
    /// 固件标签（如 "柔性压力传感器"、"未知固件"）
    pub firmware_tag: &'static str,
    /// 指令集架构（决定反汇编器选择）
    pub arch: ChipArch,
    /// Flash 基地址
    pub flash_base: u32,
    /// Flash 大小（字节）
    pub flash_size: usize,
    /// SRAM 基地址
    pub sram_base: u32,
    /// SRAM 大小（字节）
    pub sram_size: usize,
    /// SP 初始值期望范围 (min, max)
    pub sp_range: (u32, u32),
    /// Reset Handler 地址期望范围 (min, max)，已清除 Thumb 位
    pub reset_handler_range: (u32, u32),
    /// 向量表大小（字节数，0 表示自动检测）
    pub vector_table_size: usize,
    /// 已知函数签名（可选，用于精确匹配）
    pub known_functions: &'static [FunctionSignature],
    /// 标定参数布局（可选，仅标定类固件有）
    pub calibration: Option<CalibrationLayout>,
    /// 描述文本
    pub description: &'static str,
}

impl ChipProfile {
    /// 返回完整名称（芯片族 + 固件标签）
    pub fn full_name(&self) -> String {
        format!("{} / {}", self.chip_family, self.firmware_tag)
    }

    /// 返回输出文件前缀（如 "mx_"、"unknown_"）
    pub fn file_prefix(&self) -> String {
        format!("{}_", self.id)
    }
}

// ============================================================================
// 已知芯片指纹库
// ============================================================================

/// MX 固件指纹：STM32F103 柔性压力传感器
///
/// 从 stm32_backup_20260725_210807.bin 提取的特征：
/// - Flash: 64KB @ 0x08000000
/// - SRAM: 20KB @ 0x20000000 (SP=0x20002C38)
/// - Reset Handler: 0x08000169 (Thumb)
/// - 向量表: 84 项 (336 字节)
/// - 标定参数区: SRAM 0x200000B4-0x200000C3
/// - 关键函数: main@0x080017D8, 标定计算@0x08001A58, 浮点标定@0x08001904, 定点标定@0x0800197C
pub static MX_PROFILE: ChipProfile = ChipProfile {
    id: "mx",
    chip_family: "STM32F1",
    firmware_tag: "Flexible Pressure Sensor",
    arch: ChipArch::ArmThumb,
    flash_base: 0x0800_0000,
    flash_size: 64 * 1024,
    sram_base: 0x2000_0000,
    sram_size: 20 * 1024,
    sp_range: (0x2000_2000, 0x2000_5000),
    reset_handler_range: (0x0800_0100, 0x0800_0200),
    vector_table_size: 84 * 4,
    known_functions: &[
        FunctionSignature { name: "main", entry: 0x0800_17D8, hash: None },
        FunctionSignature { name: "Calibration Coefficient Calculation (Least Squares)", entry: 0x0800_1A58, hash: None },
        FunctionSignature { name: "Floating-point Calibration Path", entry: 0x0800_1904, hash: None },
        FunctionSignature { name: "Fixed-point Calibration Path", entry: 0x0800_197C, hash: None },
        FunctionSignature { name: "Sampling + Calibration Core", entry: 0x0800_1F00, hash: None },
        FunctionSignature { name: "ADC Initialization", entry: 0x0800_179C, hash: None },
        FunctionSignature { name: "DMA Configuration", entry: 0x0800_1704, hash: None },
        FunctionSignature { name: "Timer Initialization", entry: 0x0800_1EE0, hash: None },
    ],
    calibration: Some(CalibrationLayout {
        sram_base: 0x2000_00B4,
        size: 16,
        params: &[
            (0x00, "k1 (integer coefficient 1, u16)"),
            (0x02, "k2 (integer coefficient 2/reference, u16)"),
            (0x04, "max value (u16)"),
            (0x06, "threshold (u16)"),
            (0x08, "floating-point slope k (f32)"),
            (0x0C, "floating-point offset b (f32)"),
        ],
    }),
    description: "chip.mx.desc",
};

/// ESP32 坐垫固件指纹：ESP32-S2/S3 柔性压力传感器坐垫（16×16 矩阵）
///
/// 从坐垫可视化软件 668b v1.2 反推的特征（2026-08-25 澄清：该 16×16
/// 设备是坐垫而非鞋垫；id esp32_insole 为历史命名）：
/// - 通讯协议: AA 55 03 99 帧头, 256B ADC + 16B IMU 四元数
/// - Flash: 通常 4MB @ 0x00000000 (ESP32 外部 SPI Flash)
/// - 固件魔数: 0xE9 (ESP32 image header)
/// - 双 UART: 左右脚各自独立串口
/// - 传感器: 256通道 ADC (16x16网格) + IMU
pub static ESP32_INSOLE_PROFILE: ChipProfile = ChipProfile {
    id: "esp32_insole",
    chip_family: "ESP32",
    firmware_tag: "Insole Pressure Sensor",
    arch: ChipArch::Xtensa,
    flash_base: 0x0000_0000,
    flash_size: 4 * 1024 * 1024, // 4MB (典型 ESP32 Flash)
    sram_base: 0x3FF8_0000,      // ESP32 SRAM 起始
    sram_size: 320 * 1024,       // 320KB (ESP32 SRAM)
    // ESP32 无传统向量表，SP/Reset 不适用，使用宽松范围
    sp_range: (0, 0xFFFF_FFFF),
    reset_handler_range: (0, 0xFFFF_FFFF),
    vector_table_size: 0, // ESP32 使用 image header，非向量表
    known_functions: &[],
    calibration: None, // ESP32 固件标定逻辑待读取 bin 后分析
    description: "chip.esp32_insole.desc",
};

/// 已知芯片指纹库
pub static KNOWN_PROFILES: &[&ChipProfile] = &[
    &MX_PROFILE,
    &ESP32_INSOLE_PROFILE,
];

/// 未知固件的默认指纹
pub static UNKNOWN_PROFILE: ChipProfile = ChipProfile {
    id: "unknown",
    chip_family: "Unknown",
    firmware_tag: "Unknown Firmware",
    arch: ChipArch::Unknown,
    flash_base: 0x0800_0000,
    flash_size: 0,
    sram_base: 0x2000_0000,
    sram_size: 0,
    sp_range: (0, 0xFFFF_FFFF),
    reset_handler_range: (0, 0xFFFF_FFFF),
    vector_table_size: 0,
    known_functions: &[],
    calibration: None,
    description: "chip.unknown.desc",
};

// ============================================================================
// 指纹匹配引擎
// ============================================================================

/// 匹配结果
#[derive(Debug, Clone)]
pub struct MatchResult {
    /// 匹配到的 profile（未知时返回 UNKNOWN_PROFILE 的引用）
    pub profile: &'static ChipProfile,
    /// 匹配置信度 (0.0-1.0)
    pub confidence: f32,
    /// 匹配详情
    pub details: String,
}

/// 从 bin 数据提取指纹特征并与已知指纹库比对
///
/// 比对维度：
/// 1. SP 初始值是否在已知 profile 的 sp_range 内
/// 2. Reset Handler 是否在已知 profile 的 reset_handler_range 内
/// 3. Flash 大小是否匹配
/// 4. 向量表大小是否匹配
/// 5. 已知函数签名是否匹配（精确匹配）
///
/// 返回匹配置信度最高的 profile，若无匹配则返回 UNKNOWN_PROFILE
pub fn match_profile(data: &[u8]) -> MatchResult {
    if data.len() < 8 {
        return MatchResult {
            profile: &UNKNOWN_PROFILE,
            confidence: 0.0,
            details: "Data too short to extract fingerprint".into(),
        };
    }

    // ESP32 固件检测：0xE9 魔数 (ESP32 image header)
    // ESP32 固件不以 ARM 向量表开头，而是以 0xE9 魔数 + image header 开头
    if data[0] == 0xE9 {
        // 解析 ESP32 image header
        // byte[0] = 0xE9 (magic)
        // byte[1] = segment count
        // byte[2-3] = SPI mode + SPI speed/size
        // byte[4-7] = entry address
        let entry = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let seg_count = data[1];

        // 匹配 ESP32_INSOLE_PROFILE
        let profile = &ESP32_INSOLE_PROFILE;
        let mut score = 0.6f32; // 魔数匹配基础分
        let mut details = vec![
            format!("ESP32 image magic 0xE9 ✓"),
            format!("Entry address: 0x{:08X}, segments: {}", entry, seg_count),
        ];

        // 检查 entry 地址是否在 ESP32 有效范围
        // ESP32: 0x4007xxxx (IROM), 0x400Dxxxx, ESP32-S2: 0x4000xxxx, ESP32-S3: 0x4200xxxx
        let is_esp32_entry = (entry >= 0x4007_0000 && entry < 0x4010_0000)
            || (entry >= 0x400D_0000 && entry < 0x4010_0000)
            || (entry >= 0x3F00_0000 && entry < 0x4000_0000)
            || (entry >= 0x4000_0000 && entry < 0x4008_0000) // ESP32-S2
            || (entry >= 0x4200_0000 && entry < 0x4280_0000); // ESP32-S3
        if is_esp32_entry {
            score += 0.2;
            details.push(format!("Entry address within ESP32 valid range ✓"));
        }

        // Flash 大小匹配（4MB 典型值）
        if data.len() <= profile.flash_size {
            score += 0.1;
            details.push(format!("Flash size {} bytes ≤ expected {} bytes ✓", data.len(), profile.flash_size));
        }

        let confidence = score.min(1.0);
        return MatchResult {
            profile,
            confidence,
            details: details.join("; "),
        };
    }

    // ARM Cortex-M 固件检测：通过向量表 SP + Reset Handler
    // 提取 bin 文件特征
    let sp = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let reset_raw = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let reset_handler = reset_raw & !1; // 清除 Thumb 位
    let flash_size = data.len();

    let mut best_match: Option<(usize, f32, String)> = None;

    for (idx, profile) in KNOWN_PROFILES.iter().enumerate() {
        // ESP32/Xtensa profile 不参与 ARM 向量表匹配（已通过 0xE9 魔数分支处理）
        if profile.arch != ChipArch::ArmThumb {
            continue;
        }

        let mut score = 0.0f32;
        let mut max_score = 0.0f32;
        let mut details = Vec::new();

        // 1. SP 范围匹配（权重 0.3）
        max_score += 0.3;
        if sp >= profile.sp_range.0 && sp <= profile.sp_range.1 {
            score += 0.3;
            details.push(format!("SP 0x{:08X} in range [0x{:08X}, 0x{:08X}] ✓", sp, profile.sp_range.0, profile.sp_range.1));
        } else {
            details.push(format!("SP 0x{:08X} out of range [0x{:08X}, 0x{:08X}] ✗", sp, profile.sp_range.0, profile.sp_range.1));
        }

        // 2. Reset Handler 范围匹配（权重 0.3）
        max_score += 0.3;
        if reset_handler >= profile.reset_handler_range.0 && reset_handler <= profile.reset_handler_range.1 {
            score += 0.3;
            details.push(format!("Reset 0x{:08X} in range [0x{:08X}, 0x{:08X}] ✓", reset_handler, profile.reset_handler_range.0, profile.reset_handler_range.1));
        } else {
            details.push(format!("Reset 0x{:08X} out of range [0x{:08X}, 0x{:08X}] ✗", reset_handler, profile.reset_handler_range.0, profile.reset_handler_range.1));
        }

        // 3. Flash 大小匹配（权重 0.2）
        max_score += 0.2;
        if profile.flash_size > 0 && flash_size == profile.flash_size {
            score += 0.2;
            details.push(format!("Flash size {} bytes matches ✓", flash_size));
        } else if profile.flash_size > 0 && flash_size <= profile.flash_size {
            // 小于等于也算部分匹配（可能是部分读取）
            score += 0.1;
            details.push(format!("Flash size {} bytes ≤ expected {} bytes (partial match)", flash_size, profile.flash_size));
        } else {
            details.push(format!("Flash size {} bytes ≠ expected {} bytes ✗", flash_size, profile.flash_size));
        }

        // 4. 向量表大小匹配（权重 0.1）
        max_score += 0.1;
        if profile.vector_table_size > 0 && data.len() >= profile.vector_table_size {
            // 检查向量表条目是否都指向 Flash 区域
            let vt_entries = profile.vector_table_size / 4;
            let mut valid_entries = 0;
            for i in 0..vt_entries {
                let off = i * 4;
                if off + 4 > data.len() { break; }
                let v = u32::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3]]);
                if v == 0 { continue; } // 未使用
                let v_clean = v & !1;
                if v_clean >= profile.flash_base && v_clean < profile.flash_base + profile.flash_size as u32 {
                    valid_entries += 1;
                }
            }
            if valid_entries >= vt_entries / 2 {
                score += 0.1;
                details.push(format!("Vector table: {} of {} entries valid ✓", vt_entries, valid_entries));
            } else {
                details.push(format!("Vector table: only {} of {} entries valid ✗", vt_entries, valid_entries));
            }
        }

        // 5. 已知函数签名匹配（权重 0.1）
        max_score += 0.1;
        if !profile.known_functions.is_empty() {
            let mut matched_funcs = 0;
            for func in profile.known_functions {
                let off = (func.entry as i64 - profile.flash_base as i64) as usize;
                if off + 4 <= data.len() {
                    // 检查函数入口处是否是 PUSH 指令（常见函数序言）
                    let hw = u16::from_le_bytes([data[off], data[off+1]]);
                    // PUSH {R4, LR} = 0xB510, PUSH {R3, LR} = 0xB508, PUSH {LR} = 0xB500
                    // PUSH.W = 0xE92D...
                    if (hw & 0xFF00) == 0xB500 || (hw & 0xFFFF) == 0xE92D {
                        matched_funcs += 1;
                    }
                }
            }
            let func_ratio = matched_funcs as f32 / profile.known_functions.len() as f32;
            score += 0.1 * func_ratio;
            if matched_funcs > 0 {
                details.push(format!("Known function signatures matched {}/{} ✓", matched_funcs, profile.known_functions.len()));
            } else {
                details.push(format!("Known function signatures matched 0/{} ✗", profile.known_functions.len()));
            }
        } else {
            // 无函数签名要求，给满分
            score += 0.1;
        }

        let confidence = if max_score > 0.0 { score / max_score } else { 0.0 };
        let detail_str = details.join("; ");

        let is_better = best_match.as_ref().map_or(true, |(_, prev_conf, _)| confidence > *prev_conf);
        if is_better {
            best_match = Some((idx, confidence, detail_str));
        }
    }

    match best_match {
        Some((idx, confidence, details)) if confidence >= 0.6 => {
            MatchResult {
                profile: KNOWN_PROFILES[idx],
                confidence,
                details,
            }
        }
        _ => MatchResult {
            profile: &UNKNOWN_PROFILE,
            confidence: 0.0,
            details: format!(
                "SP=0x{:08X}, Reset=0x{:08X}, Size={} bytes, no matching known fingerprint",
                sp, reset_handler, flash_size
            ),
        },
    }
}

/// 根据 profile 获取反编译参数
pub struct DisasmParams {
    /// 基地址
    pub base_addr: u32,
    /// 向量表大小（字节，ESP32 为 0）
    pub vector_table_size: usize,
    /// 是否启用 Thumb-2 解码（仅 ARM）
    pub thumb2_enabled: bool,
    /// 指令集架构（决定反汇编器选择）
    pub arch: ChipArch,
}

impl DisasmParams {
    /// 从 ChipProfile 获取反编译参数
    pub fn from_profile(profile: &ChipProfile) -> Self {
        Self {
            base_addr: profile.flash_base,
            vector_table_size: profile.vector_table_size,
            thumb2_enabled: profile.arch == ChipArch::ArmThumb,
            arch: profile.arch,
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mx_profile_match() {
        // 尝试读取实际 MX 固件 bin 文件，找不到则用构造数据测试
        let candidates = [
            "stm32_backup_20260725_210807.bin",
            "../../stm32_backup_20260725_210807.bin",
        ];
        let data = candidates.iter()
            .find_map(|p| std::fs::read(p).ok())
            .unwrap_or_else(|| {
                // 构造符合 MX 指纹的虚拟 bin 文件（64KB）
                let mut data = vec![0xFFu8; 64 * 1024];
                // 向量表第 0 项：SP = 0x20002C38（在 MX sp_range 内）
                data[0..4].copy_from_slice(&0x2000_2C38u32.to_le_bytes());
                // 向量表第 1 项：Reset Handler = 0x08000169（Thumb，在 reset_handler_range 内）
                data[4..8].copy_from_slice(&0x0800_0169u32.to_le_bytes());
                // 在 main 入口 0x080017D8 处放 PUSH {R3, LR} (0xB508)
                let main_off = 0x17D8;
                data[main_off..main_off+2].copy_from_slice(&0xB508u16.to_le_bytes());
                // 在标定计算入口 0x08001A58 处放 PUSH {R4, LR} (0xB510)
                let cal_off = 0x1A58;
                data[cal_off..cal_off+2].copy_from_slice(&0xB510u16.to_le_bytes());
                data
            });
        let result = match_profile(&data);
        assert_eq!(result.profile.id, "mx");
        assert!(result.confidence >= 0.6, "置信度应 ≥ 0.6, 实际: {}", result.confidence);
        println!("MX 匹配: confidence={:.2}, details={}", result.confidence, result.details);
    }

    #[test]
    fn test_unknown_bin_no_match() {
        // 构造一个明显不是 MX 的 bin（非 ESP32 魔数，SP 不在 MX 范围内）
        let mut data = vec![0u8; 4096];
        // SP = 0x80000000，不在 SRAM 范围
        data[0] = 0x00;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x80; // SP = 0x80000000，不在 SRAM 范围
        let result = match_profile(&data);
        assert_eq!(result.profile.id, "unknown");
        println!("未知 bin: confidence={:.2}, details={}", result.confidence, result.details);
    }

    #[test]
    fn test_esp32_insole_profile_match() {
        // 构造 ESP32 固件 bin（0xE9 魔数 + image header）
        let mut data = vec![0xFFu8; 4 * 1024 * 1024];
        // ESP32 image header
        data[0] = 0xE9;       // magic
        data[1] = 3;          // segment count
        data[2] = 0x02;       // SPI mode QIO
        data[3] = 0x10;       // SPI speed/size
        // entry address: ESP32 IROM 0x400D0000 (典型值)
        data[4..8].copy_from_slice(&0x400D_0000u32.to_le_bytes());

        let result = match_profile(&data);
        assert_eq!(result.profile.id, "esp32_insole");
        assert!(result.confidence >= 0.6, "ESP32 置信度应 ≥ 0.6, 实际: {}", result.confidence);
        assert_eq!(result.profile.arch, ChipArch::Xtensa);
        println!("ESP32 匹配: confidence={:.2}, details={}", result.confidence, result.details);
    }

    #[test]
    fn test_esp32_s2_entry_match() {
        // ESP32-S2 入口地址范围测试
        let mut data = vec![0xFFu8; 1024 * 1024];
        data[0] = 0xE9;
        data[1] = 2;
        // ESP32-S2 entry: 0x4000xxxx
        data[4..8].copy_from_slice(&0x4002_0000u32.to_le_bytes());
        let result = match_profile(&data);
        assert_eq!(result.profile.id, "esp32_insole");
        println!("ESP32-S2 匹配: confidence={:.2}, details={}", result.confidence, result.details);
    }

    #[test]
    fn test_short_data() {
        let data = vec![0u8; 4];
        let result = match_profile(&data);
        assert_eq!(result.profile.id, "unknown");
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn test_disasm_params_from_profile() {
        let params = DisasmParams::from_profile(&MX_PROFILE);
        assert_eq!(params.base_addr, 0x0800_0000);
        assert_eq!(params.vector_table_size, 84 * 4);
        assert!(params.thumb2_enabled);
        assert_eq!(params.arch, ChipArch::ArmThumb);

        // ESP32 参数测试
        let esp_params = DisasmParams::from_profile(&ESP32_INSOLE_PROFILE);
        assert_eq!(esp_params.base_addr, 0x0000_0000);
        assert_eq!(esp_params.vector_table_size, 0);
        assert!(!esp_params.thumb2_enabled);
        assert_eq!(esp_params.arch, ChipArch::Xtensa);
    }

    #[test]
    fn test_file_prefix() {
        assert_eq!(MX_PROFILE.file_prefix(), "mx_");
        assert_eq!(UNKNOWN_PROFILE.file_prefix(), "unknown_");
    }

    #[test]
    fn test_calibration_layout() {
        let cal = MX_PROFILE.calibration.as_ref().unwrap();
        assert_eq!(cal.sram_base, 0x2000_00B4);
        assert_eq!(cal.size, 16);
        assert_eq!(cal.params.len(), 6);
        // 验证参数偏移
        assert_eq!(cal.params[0].1, "k1 (integer coefficient 1, u16)");
        assert_eq!(cal.params[4].1, "floating-point slope k (f32)");
    }
}
