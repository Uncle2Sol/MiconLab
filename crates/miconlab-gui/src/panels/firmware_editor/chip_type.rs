// 芯片类型定义和自动检测

use super::memory::FirmwareMemory;

/// 支持的芯片类型（决定向量表解析方式、Flash 起始地址、字节序等）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorChipType {
    Auto,
    Stm32,
    Gd32,
    Ch32,
    Esp32,
    Esp8266,
    Arduino,
    Stm8,
    NxpLpc,
    Raw,
}

impl EditorChipType {
    pub fn all() -> &'static [EditorChipType] {
        &[
            EditorChipType::Auto,
            EditorChipType::Stm32,
            EditorChipType::Gd32,
            EditorChipType::Ch32,
            EditorChipType::Esp32,
            EditorChipType::Esp8266,
            EditorChipType::Arduino,
            EditorChipType::Stm8,
            EditorChipType::NxpLpc,
            EditorChipType::Raw,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            EditorChipType::Auto => crate::i18n::t("Auto-detect"),
            EditorChipType::Stm32 => "STM32 (ARM Cortex-M)",
            EditorChipType::Gd32 => "GD32 (ARM Cortex-M)",
            EditorChipType::Ch32 => "CH32 (RISC-V/ARM)",
            EditorChipType::Esp32 => "ESP32 (Xtensa)",
            EditorChipType::Esp8266 => "ESP8266 (Xtensa LX106)",
            EditorChipType::Arduino => "Arduino (AVR)",
            EditorChipType::Stm8 => "STM8 (8-bit)",
            EditorChipType::NxpLpc => "NXP LPC (ARM Cortex-M)",
            EditorChipType::Raw => crate::i18n::t("Raw binary (no parsing)"),
        }
    }

    /// Flash 起始地址（用于 BIN 文件的地址映射）
    pub fn flash_base(self) -> u32 {
        match self {
            EditorChipType::Stm32 | EditorChipType::Gd32 => 0x0800_0000,
            EditorChipType::Ch32 => 0x0800_0000,
            EditorChipType::Esp32 => 0x0000_0000,
            EditorChipType::Esp8266 => 0x0000_0000,
            EditorChipType::Arduino => 0x0000_0000,
            EditorChipType::Stm8 => 0x0080_0000,
            EditorChipType::NxpLpc => 0x0000_0000,
            _ => 0x0000_0000,
        }
    }

    /// 是否为 ARM Cortex-M 芯片（有向量表）
    pub fn is_arm_cortex_m(self) -> bool {
        matches!(
            self,
            EditorChipType::Stm32 | EditorChipType::Gd32 | EditorChipType::Ch32 | EditorChipType::NxpLpc
        )
    }

    /// 字节序：true=小端（ARM/Xtensa），false=大端
    pub fn is_little_endian(self) -> bool {
        !matches!(self, EditorChipType::Stm8)
    }
}

/// 根据固件内容自动检测芯片类型
pub fn auto_detect_chip(mem: &FirmwareMemory) -> EditorChipType {
    // 以数据起始地址为基准读取（HEX 文件数据在真实地址如 0x08000000，地址 0 为空）
    let base = mem.base_addr;
    // STM32/GD32/CH32 检测：检查 ARM Cortex-M 向量表
    if mem.cells.len() >= 8 {
        if let Some(sp) = mem.read_u32_le(base) {
            // STM32F1: SP 通常在 0x2000_0000 - 0x2001_0000
            // STM32F4: SP 通常在 0x2002_0000 - 0x2003_0000
            if sp >= 0x2000_0000 && sp < 0x2004_0000 {
                if let Some(rh) = mem.read_u32_le(base + 4) {
                    // Reset Handler 应该在 Flash 区域且 Thumb 模式（bit0=1）
                    if (rh & 1) != 0
                        && (rh & 0xFFFF_FFFE) >= 0x0800_0000
                        && (rh & 0xFFFF_FFFE) < 0x0820_0000
                    {
                        return EditorChipType::Stm32;
                    }
                }
            }
        }
    }

    // ESP 芯片检测：0xE9 魔数 + 区分 ESP32/ESP8266
    if let Some(magic) = mem.cells.get(&base) {
        if magic.value == 0xE9 {
            let entry_addr = mem.read_u32_le(base + 4).unwrap_or(0);
            let first_seg_addr = mem.read_u32_le(base + 8).unwrap_or(0);

            // ESP8266: entry in IRAM range
            if entry_addr >= 0x4010_0000 && entry_addr < 0x4030_0000 {
                return EditorChipType::Esp8266;
            }
            // ESP32: entry in 0x4007xxxx / 0x400Dxxxx / IROM range
            if (entry_addr >= 0x4007_0000 && entry_addr < 0x4010_0000)
                || (entry_addr >= 0x400D_0000 && entry_addr < 0x4010_0000)
                || (entry_addr >= 0x3F00_0000 && entry_addr < 0x4000_0000)
            {
                return EditorChipType::Esp32;
            }
            // 进一步用 first segment load_addr 判断
            if first_seg_addr >= 0x4020_0000 && first_seg_addr < 0x4030_0000 {
                return EditorChipType::Esp8266;
            }
            if first_seg_addr >= 0x3F40_0000 && first_seg_addr < 0x4000_0000 {
                return EditorChipType::Esp32;
            }
            // 无法确定时默认 ESP32（更常见）
            return EditorChipType::Esp32;
        }
    }

    EditorChipType::Raw
}
