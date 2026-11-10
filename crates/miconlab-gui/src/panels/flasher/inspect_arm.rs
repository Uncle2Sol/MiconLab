//! flasher 子模块：ARM/Cortex-M 固件解读与反汇编。
//!
//! 包含：
//!   - `inspect_arm_firmware`：STM32/GD32/CH32 等 Cortex-M 芯片的向量表校验、
//!     SP/Reset 分析、异常向量解析、默认中断处理统计、数据特征分析、字符串提取、
//!     HEX dump、Reset Handler Thumb 反汇编、深度函数分析（调用图）。
//!   - `export_disasm_to_file`：导出完整 ARM Thumb 反汇编到 .asm 文件。
//!   - `analyze_functions`：函数边界检测与调用图构建（供 inspect_arm_firmware 调用）。

use super::*;

/// STM32/Cortex-M 外设地址符号化
/// 将裸外设地址转换为可读名称（如 0x40013800 → "USART1"）
/// 覆盖 STM32F1/F3/F4 系列常见外设
pub(crate) fn peripheral_name(addr: u32) -> Option<&'static str> {
    // Cortex-M 系统寄存器 (0xE000xxxx)
    match addr {
        0xE000ED08 => return Some("SCB_VTOR"),
        0xE000ED0C => return Some("SCB_CCR"),
        0xE000ED88 => return Some("SCB_CPACR"),
        0xE000E400 => return Some("NVIC_IPR0"),
        0xE000E100 => return Some("NVIC_ISER0"),
        0xE000E180 => return Some("NVIC_ICER0"),
        0xE000E200 => return Some("NVIC_ISPR0"),
        0xE000E280 => return Some("NVIC_ICPR0"),
        0xE000EF00 => return Some("STK_CSR"),  // SysTick
        0xE000EF04 => return Some("STK_RVR"),
        0xE000EF08 => return Some("STK_CVR"),
        _ => {}
    }
    // APB1 外设 (0x40000000-0x40007FFF)，每 0x400 (1KB) 一个外设
    match addr & 0xFFFFFC00 {
        0x40000000 => return Some("TIM2"),
        0x40000400 => return Some("TIM3"),
        0x40000800 => return Some("TIM4"),
        0x40000C00 => return Some("TIM5"),
        0x40001000 => return Some("TIM6"),
        0x40001400 => return Some("TIM7"),
        0x40001800 => return Some("TIM12"),
        0x40001C00 => return Some("TIM13"),
        0x40002000 => return Some("TIM14"),
        0x40002800 => return Some("RTC"),
        0x40002C00 => return Some("WWDG"),
        0x40003000 => return Some("IWDG"),
        0x40003800 => return Some("SPI2"),
        0x40003C00 => return Some("SPI3"),
        0x40004400 => return Some("USART2"),
        0x40004800 => return Some("USART3"),
        0x40004C00 => return Some("UART4"),
        0x40005000 => return Some("UART5"),
        0x40005400 => return Some("I2C1"),
        0x40005800 => return Some("I2C2"),
        0x40005C00 => return Some("USB/CAN"),
        0x40006400 => return Some("DAC1"),  // F3: DAC1, F1: USB/CAN SRAM
        0x40007000 => return Some("PWR"),
        0x40007400 => return Some("DAC2"),
        _ => {}
    }
    // APB2 外设 (0x40010000-0x40013FFF)，每 0x400 (1KB) 一个外设
    match addr & 0xFFFFFC00 {
        0x40010000 => return Some("AFIO/SYSCFG"),
        0x40010400 => return Some("EXTI"),
        0x40010800 => return Some("GPIOA"),
        0x40010C00 => return Some("GPIOB"),
        0x40011000 => return Some("GPIOC"),
        0x40011400 => return Some("GPIOD"),
        0x40011800 => return Some("GPIOE"),
        0x40011C00 => return Some("GPIOF"),
        0x40012000 => return Some("GPIOG"),
        0x40012400 => return Some("ADC1"),
        0x40012800 => return Some("ADC2"),
        0x40012C00 => return Some("TIM1"),
        0x40013000 => return Some("SPI1"),
        0x40013400 => return Some("TIM8"),
        0x40013800 => return Some("USART1"),
        0x40013C00 => return Some("ADC3"),
        _ => {}
    }
    // AHB 外设 (0x40018000-0x40023FFF)
    match addr & 0xFFFFF000 {
        0x40018000 => return Some("TIM9_14"),  // F4 APB2
        0x40020000 => return Some("DMA1"),
        0x40020400 => return Some("DMA2"),
        0x40020800 => return Some("RCC"),
        0x40020C00 => return Some("RCC"),  // F1: RCC at 0x40021000
        0x40021000 => return Some("RCC"),
        0x40022000 => return Some("Flash_IF"),
        0x40023000 => return Some("CRC"),
        0x40024000 => return Some("ETH"),  // F4: ETH
        _ => {}
    }
    // F4 GPIO (0x4002xxxx → AHB1) 和 F1/F3 GPIO 已经在 APB2 处理
    // F4 GPIO: 0x40020000(ETH/CRC)/0x4002xxxx，实际 F4 GPIO 在 0x48000000
    match addr & 0xFFFF0000 {
        0x48000000 => return Some("GPIOA_F4"),  // F4 AHB1 GPIO
        0x48010000 => return Some("GPIOH_F4"),
        _ => {}
    }
    None
}

/// 格式化地址值标注：Flash/SRAM/外设/系统寄存器
pub(crate) fn format_addr_value(val: u32, base_addr: u32, data_len: usize, func_labels: &std::collections::HashMap<u32, String>) -> String {
    if val >= base_addr && val < base_addr + data_len as u32 {
        let func_name = func_labels.get(&(val & !1)).map(|s| s.as_str()).unwrap_or("sub");
        format!(" = 0x{:08X} → {}", val, func_name)
    } else if (val >> 24) == 0x20 {
        format!(" = 0x{:08X} (SRAM)", val)
    } else if (val >> 28) == 0xE {
        // Cortex-M 系统寄存器
        if let Some(name) = peripheral_name(val) {
            format!(" = 0x{:08X} ({})", val, name)
        } else {
            format!(" = 0x{:08X} (System)", val)
        }
    } else if (val >> 28) == 0x4 {
        if let Some(name) = peripheral_name(val) {
            format!(" = 0x{:08X} ({})", val, name)
        } else {
            format!(" = 0x{:08X} (Peripheral)", val)
        }
    } else {
        format!(" = 0x{:08X}", val)
    }
}

/// 检测 AT 命令字符串（蓝牙/WiFi 模块配置）
/// 返回 Some(描述) 如果字符串是 AT 命令
pub(crate) fn detect_at_command(s: &str) -> Option<&'static str> {
    if s.starts_with("AT+BAUD") || s.starts_with("AT+BAUDRATE") {
        return Some(crate::i18n::t("Bluetooth module baud rate config"));
    }
    if s.starts_with("AT+BNAME") || s.starts_with("AT+NAME") {
        return Some(crate::i18n::t("Bluetooth module name config"));
    }
    if s.starts_with("AT+PIN") || s.starts_with("AT+PASS") {
        return Some(crate::i18n::t("Bluetooth module pairing password"));
    }
    if s.starts_with("AT+BIND") {
        return Some(crate::i18n::t("Bluetooth module bind address"));
    }
    if s.starts_with("AT+UART") {
        return Some(crate::i18n::t("UART parameter config"));
    }
    if s.starts_with("AT+VERSION") || s.starts_with("AT+VERRSION") {
        return Some(crate::i18n::t("Module version query"));
    }
    if s.starts_with("AT+ROLE") {
        return Some(crate::i18n::t("Bluetooth master/slave mode config"));
    }
    if s.starts_with("AT+RESET") || s.starts_with("AT+RESTART") {
        return Some(crate::i18n::t("Module reset"));
    }
    if s.starts_with("AT+") {
        return Some(crate::i18n::t("AT command"));
    }
    None
}

/// 检测固件中的 FPU 启用代码
/// 搜索 CPACR (0xE000ED88) 写入 0x700000 的模式（启用 CP10/CP11）
pub(crate) fn detect_fpu_enable(data: &[u8], _base_addr: u32) -> bool {
    // 在前 2KB 代码中搜索 CPACR 启用模式
    let scan_len = data.len().min(2048);
    // Thumb-2 指令模式：LDR R0, [PC, #imm] → LDR R0, [0xE000ED88] → ORR R0, R0, #0x700000 → STR R0, [...]
    // 文字池中包含 0xE000ED88 (88 ED 00 E0)
    for i in 0..scan_len.saturating_sub(3) {
        if data[i] == 0x88 && data[i+1] == 0xED && data[i+2] == 0x00 && data[i+3] == 0xE0 {
            return true;
        }
    }
    false
}

/// 统计 SRAM 地址引用频率，返回高频地址列表
/// 用于自动命名频繁访问的全局变量
/// 扫描 LDR [PC, #imm] 指令解析文字池中的 SRAM 地址（0x2000xxxx）
pub(crate) fn collect_sram_refs(
    instructions: &[miconlab_ai::thumb_disasm::DisasmInstruction],
    data: &[u8],
    base_addr: u32,
) -> std::collections::HashMap<u32, usize> {
    let mut refs = std::collections::HashMap::new();
    for inst in instructions {
        // LDR [PC, #imm] 指令的 comment 包含文字池地址 "→ [0xXXXXXXXX]"
        if inst.mnemonic != "LDR" || !inst.operands.contains("[PC,") {
            // 同时扫描注释中已解析出的 SRAM 地址（兼容 LDR.W 等变体）
            if let Some(comment) = &inst.comment {
                collect_sram_from_str(comment, &mut refs);
            }
            continue;
        }
        if let Some(comment) = &inst.comment {
            if let Some(addr_str) = comment.find("0x") {
                let addr_part = &comment[addr_str..];
                let addr_end = addr_part.find(']').unwrap_or(addr_part.len());
                let target_addr_str = &addr_part[..addr_end];
                if let Ok(target_addr) = u32::from_str_radix(target_addr_str.trim_start_matches("0x"), 16) {
                    let target_offset = (target_addr.wrapping_sub(base_addr)) as usize;
                    if target_offset + 4 <= data.len() {
                        let val = u32::from_le_bytes([
                            data[target_offset], data[target_offset+1],
                            data[target_offset+2], data[target_offset+3],
                        ]);
                        if val >= 0x20000000 && val < 0x20010000 {
                            *refs.entry(val).or_insert(0) += 1;
                        }
                        // 同时记录文字池地址本身（若指向 SRAM）
                        if target_addr >= 0x20000000 && target_addr < 0x20010000 {
                            *refs.entry(target_addr).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }
    refs
}

/// 从字符串中提取 SRAM 地址并计入引用计数
fn collect_sram_from_str(s: &str, refs: &mut std::collections::HashMap<u32, usize>) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 6 <= bytes.len() {
        // 查找 "0x2000" 模式
        if &bytes[i..i+6] == b"0x2000" {
            let start = i + 6;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
                end += 1;
            }
            let hex = &s[i+2..end];
            if let Ok(addr) = u32::from_str_radix(hex, 16) {
                if addr >= 0x20000000 && addr < 0x20010000 {
                    *refs.entry(addr).or_insert(0) += 1;
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
}

/// 为高频 SRAM 地址生成自动变量名
/// ref_count >= 2 时命名（频繁访问的全局变量）
pub(crate) fn name_sram_var(addr: u32, ref_count: usize) -> Option<String> {
    if ref_count >= 2 && addr >= 0x20000000 && addr < 0x20010000 {
        Some(format!("g_var_{:04X}", addr & 0xFFFF))
    } else {
        None
    }
}

/// 估算 Flash 大小：固件大小给出下限，向上取整到常见 Flash 规格
/// 常见规格：16/32/64/128/256/512/1024/2048 KB
pub(crate) fn estimate_flash_size(fw_size: usize) -> u32 {
    let kb = (fw_size as f64 / 1024.0).ceil() as u32;
    // 向上取整到最近的常见规格
    for &cap in &[16, 32, 64, 128, 256, 512, 1024, 2048] {
        if kb <= cap {
            return cap;
        }
    }
    kb
}

/// 统计向量表中有效的外部中断向量数量
/// 用于区分芯片子型号（如 STM32F103C8=43 IRQ vs STM32F103RC=60 IRQ）
pub(crate) fn count_active_irqs(data: &[u8], base_addr: u32, sp: u32) -> usize {
    // 跳过前 16 个核心异常向量（SP + 15 个异常），从外部中断开始计数
    let mut count = 0;
    let max_vectors = (data.len().min(512)) / 4;
    for i in 16..max_vectors {
        let offset = i * 4;
        if offset + 4 > data.len() { break; }
        let handler = u32::from_le_bytes([
            data[offset], data[offset+1], data[offset+2], data[offset+3]
        ]);
        if handler == 0 || handler == sp { break; } // 0 = 向量表结束
        let code_addr = handler & !1;
        // 只统计指向 Flash 的合法处理程序
        if code_addr < base_addr || code_addr >= base_addr + data.len() as u32 {
            break;
        }
        count += 1;
    }
    count
}

/// 综合判定 STM32 芯片型号：RAM + Flash + FPU + IRQ 数量
/// 返回最可能的芯片型号字符串
pub(crate) fn identify_stm32_model(ram_kb: u32, flash_kb: u32, has_fpu: bool, irq_count: usize) -> &'static str {
    // GD32/CH32 兼容芯片：IRQ 数量与 STM32 相同但 Flash/RAM 配置可能不同
    // 优先通过 FPU 区分架构
    if has_fpu {
        // Cortex-M4F 系列
        match (ram_kb, flash_kb) {
            (0..=6, 16..=64) => crate::i18n::t("STM32G030/G031 (Cortex-M0+, no FPU?) or GD32E230"),
            (0..=6, _) => crate::i18n::t("STM32G0/G4 series (small SRAM, M4F)"),
            (7..=12, 64..=128) => "STM32G431/STM32F303CB (Cortex-M4F, 32KB SRAM)",
            (7..=12, 256..=512) => "STM32F303RC/VC (Cortex-M4F, 40KB SRAM)",
            (13..=32, 128..=256) => "STM32F303CB/CC (Cortex-M4F, 32KB SRAM)",
            (13..=32, 257..=512) => "STM32F303RC/VC (Cortex-M4F, 32-40KB SRAM)",
            (33..=64, 256..=512) => "STM32F407VE/VG (Cortex-M4F, 64KB SRAM)",
            // 注意：上一行已覆盖 (33..=64, 512)，此处拆分以保持范围互斥且语义不变
            (33..=64, 513..=1024) => "STM32F407VG/ZG (Cortex-M4F, 96-128KB SRAM)",
            (65..=96, 512..=1024) => "STM32F407VG/ZG (Cortex-M4F, 96-128KB SRAM)",
            (97..=192, 512..=1024) => "STM32F429/F439 (Cortex-M4F, 192KB SRAM)",
            (193..=256, 1024..=2048) => "STM32F7/F427 (Cortex-M7, 256KB+ SRAM)",
            (257.., 1024..=2048) => crate::i18n::t("STM32H7 series (Cortex-M7, large SRAM)"),
            _ => crate::i18n::t("STM32F4/G4 series (Cortex-M4F)"),
        }
    } else {
        // Cortex-M3 (无 FPU) - 主要是 STM32F1 系列 + GD32F1/CH32F1 兼容芯片
        // 注意：RAM 范围互斥，避免 match 重叠警告
        match (ram_kb, flash_kb, irq_count) {
            // 超小容量 (Cortex-M0)
            (0..=10, 16..=32, _) => "STM32F030/F031/F042 (Cortex-M0)",
            (0..=10, 33..=64, _) => "STM32F051 (Cortex-M0, 8KB/64KB)",
            // 小容量：C8 = 20KB RAM, 64KB Flash, 43 IRQ
            (17..=24, 32..=64, 35..=45) => crate::i18n::t("STM32F103C8 (Cortex-M3, 20KB/64KB) or GD32F103C8"),
            (17..=24, 65..=128, 35..=45) => crate::i18n::t("STM32F103CB (Cortex-M3, 20KB/128KB) or CH32F103C8"),
            // 中/大容量：RC/VC = 48KB RAM, 256KB Flash, 60 IRQ
            (40..=56, 128..=256, 50..=60) => crate::i18n::t("STM32F103RC/VC (Cortex-M3, 48KB/256KB) or GD32F103RC"),
            (40..=56, 257..=512, 50..=60) => "STM32F103RD/RE/VE (Cortex-M3, 48KB/384-512KB)",
            // 超大容量：ZE/ZG = 64-96KB RAM, 512-1024KB Flash
            (64..=96, 512..=1024, 55..=65) => "STM32F103ZE/ZG (Cortex-M3, 64-96KB/512-1024KB)",
            _ if irq_count <= 32 => crate::i18n::t("STM32F030 series (Cortex-M0, few IRQs)"),
            _ => crate::i18n::t("STM32F103 series (Cortex-M3, or F105/F107 connectivity line)"),
        }
    }
}

/// 检测疑似数据区域：连续 ≥2 条可疑指令（DATA?/UNPREDICTABLE/.word）判定为数据
/// 返回数据区域起始地址集合，用于在反汇编输出中标记
pub(crate) fn detect_data_regions(instructions: &[miconlab_ai::thumb_disasm::DisasmInstruction]) -> std::collections::HashSet<u32> {
    let mut regions = std::collections::HashSet::new();
    let mut suspicious_start: Option<u32> = None;
    let mut suspicious_count = 0usize;
    let mut last_addr: Option<u32> = None;

    for inst in instructions {
        let is_suspicious = inst.comment.as_ref().map(|c| {
            c.contains("DATA?") || c.contains("UNPREDICTABLE")
        }).unwrap_or(false) || inst.mnemonic.starts_with(".word");

        if is_suspicious {
            if suspicious_start.is_none() {
                suspicious_start = Some(inst.addr);
            }
            suspicious_count += 1;
        } else {
            // 连续可疑指令达到阈值，标记为数据区域
            if suspicious_count >= 2 {
                if let Some(start) = suspicious_start {
                    regions.insert(start);
                }
            }
            suspicious_start = None;
            suspicious_count = 0;
        }
        last_addr = Some(inst.addr);
    }
    // 处理末尾的数据区域
    if suspicious_count >= 2 {
        if let Some(start) = suspicious_start {
            regions.insert(start);
        }
    }
    let _ = last_addr;
    regions
}

/// ARM/Cortex-M 固件解读：向量表校验 + 反汇编 + 深度分析
///
/// 由 inspect::inspect_read_data 在检测到非 ESP32 固件后调用。
/// 参数 `data` 为完整固件字节，`base_addr` 为 Flash 起始地址。
pub(crate) fn inspect_arm_firmware(fs: &mut FlasherState, data: &[u8], base_addr: u32) {
    let size = data.len();

    // === STM32 向量表分析 ===
    // 向量表（小端）：[0:4]=SP, [4:8]=Reset, [8:12]=NMI, [12:16]=HardFault, ...
    let sp = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let reset_raw = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let reset_handler = reset_raw & !1; // Thumb 模式：bit0=1，实际地址需清零 bit0
    let is_thumb = reset_raw & 1 == 1;

    let sp_valid = (sp >> 24) == 0x20; // 0x20xxxxxx = SRAM
    let rh_valid = (reset_handler >> 16) == 0x0800 || (reset_handler >> 16) == 0x0801
        || (reset_handler >> 16) == 0x0802;

    log_push(fs, format!("  {} SP=0x{:08X} Reset=0x{:08X}",
        crate::i18n::t("Vector table"), sp, reset_raw));

    if !sp_valid && !rh_valid {
        log_push(fs, crate::i18n::t("⚠ Data may not be valid firmware (SP and Reset Handler abnormal)"));
        return;
    }

    if sp_valid {
        // 从 SP 推断 RAM 大小：SP 通常指向 RAM 末尾
        // 0x20000000 + NKB → RAM 大约 NKB
        let ram_end = sp - 0x20000000;
        let ram_kb = if ram_end > 0 { (ram_end as f64 / 1024.0).ceil() as u32 } else { 0 };
        // 检测 FPU 启用代码（CPACR 写入 0xE000ED88）
        let has_fpu = detect_fpu_enable(data, base_addr);

        // 估算 Flash 大小（固件大小给出下限，向上取整到常见规格）
        let flash_kb = estimate_flash_size(data.len());

        // 统计外部中断向量数量（区分芯片子型号）
        let irq_count = count_active_irqs(data, base_addr, sp);

        // 综合判定：RAM + Flash + FPU + IRQ 数量
        let chip_guess = identify_stm32_model(ram_kb, flash_kb, has_fpu, irq_count);
        let fpu_note = if has_fpu { crate::i18n::t(" [FPU enabled]") } else { "" };
        log_push(fs, format!("  → SP {} SRAM (0x20000000 + {}KB, {}{})",
            crate::i18n::t("valid ->"), ram_kb, chip_guess, fpu_note));
        log_push(fs, format!("    Flash ≥ {}KB, IRQ {} vectors", flash_kb, irq_count));
        if has_fpu {
            log_push(fs, crate::i18n::t("  ★ FPU enable detected (CPACR |= 0x700000), Cortex-M4F"));
        }
    } else {
        log_push(fs, crate::i18n::tf("  ⚠ Stack pointer abnormal (0x{:08X}), expected 0x20xxxxxx (SRAM)", &[("{:08X}", "{:08X}", &format!("{:08X}", sp))]));
    }

    if rh_valid {
        log_push(fs, format!("  → Reset Handler {} Flash 0x{:08X}{}",
            crate::i18n::t("valid ->"),
            reset_handler,
            if is_thumb { crate::i18n::t(", Thumb ✓") } else { "" }));
    } else {
        log_push(fs, crate::i18n::tf("  ⚠ Reset Handler abnormal (0x{:08X}), expected 0x0800xxxx (Flash)", &[("{:08X}", "{:08X}", &format!("{:08X}", reset_handler))]));
    }

    // 解析向量表 [2..7]：NMI, HardFault, MemManage, BusFault, UsageFault
    if data.len() >= 32 {
        // 异常向量名是 ARM 架构标识，不参与翻译
        let irq_names = ["NMI", "HardFault", "MemManage", "BusFault", "UsageFault"];
        log_push(fs, crate::i18n::t("  Exception vectors:"));
        for (i, name) in irq_names.iter().enumerate() {
            let offset = 8 + i * 4;
            if offset + 4 <= data.len() {
                let handler = u32::from_le_bytes([
                    data[offset], data[offset+1], data[offset+2], data[offset+3]
                ]);
                if handler != 0 {
                    log_push(fs, format!("    [{}] {}: 0x{:08X}", i + 2, name, handler));
                }
            }
        }
    }

    if sp_valid && rh_valid {
        log_push(fs, format!("✓ {}", crate::i18n::t("Valid firmware")));
    }

    // === 向量表默认处理程序统计 ===
    // STM32 向量表有 16 个系统异常 + N 个外部中断
    // 未使用的中断通常指向同一个默认处理程序（B self 死循环）
    if data.len() >= 256 {
        use std::collections::HashMap;
        let mut handler_counts: HashMap<u32, u32> = HashMap::new();
        // STM32F103 向量表最多 60 个向量（16 异常 + 44 中断 = 240 字节）
        // 超出部分是代码区，不是向量表
        let max_vectors = (data.len().min(240)) / 4;
        for i in 0..max_vectors {
            let offset = i * 4;
            if offset + 4 > data.len() { break; }
            let handler = u32::from_le_bytes([
                data[offset], data[offset+1], data[offset+2], data[offset+3]
            ]);
            if handler == 0 || handler == sp { continue; }
            // 只统计指向 Flash 区域的合法处理程序地址
            let code_addr = handler & !1;
            if code_addr < base_addr || code_addr >= base_addr + data.len() as u32 {
                break; // 超出向量表范围，进入代码区
            }
            *handler_counts.entry(handler).or_insert(0) += 1;
        }
        if let Some((&default_addr, &count)) = handler_counts.iter().max_by_key(|(_, &c)| c) {
            if count >= 5 {
                let code_addr = default_addr & !1;
                log_push(fs, crate::i18n::tf("  {} 0x{:08X} ({} {} share the default handler)",
                    &[("{}", "{}", &format!("{}", crate::i18n::t("Default IRQ handler:"))),
                      ("{:08X}", "{:08X}", &format!("{:08X}", code_addr)),
                      ("{}", "{}", &format!("{}", count)),
                      ("{}", "{}", &format!("{}", crate::i18n::t(" vectors")))]));
            }
        }
    }

    // === 数据特征分析 ===
    // 统计字节分布
    let mut printable = 0usize;
    let mut zero_count = 0usize;
    let mut ff_count = 0usize;
    for &b in data {
        if (0x20..=0x7E).contains(&b) { printable += 1; }
        if b == 0 { zero_count += 1; }
        if b == 0xFF { ff_count += 1; }
    }
    let pct = |n: usize| (n * 100 / size) as u32;

    // 香农熵
    let mut freq = [0u32; 256];
    for &b in data { freq[b as usize] += 1; }
    let entropy: f64 = freq.iter()
        .filter(|&&c| c > 0)
        .map(|&c| { let p = c as f64 / size as f64; -p * p.log2() })
        .sum();
    let entropy_str = if entropy > 7.0 {
        crate::i18n::t("high (compressed/encrypted?)")
    } else if entropy < 3.0 {
        crate::i18n::t("low (repetitive)")
    } else {
        crate::i18n::t("normal (code+data mix)")
    };

    log_push(fs, format!("  {} printable={}%, 0x00={}%, 0xFF={}%, entropy={:.2} ({})",
        crate::i18n::t("Stats:"),
        pct(printable), pct(zero_count), pct(ff_count), entropy, entropy_str));

    // === Flash 使用率分析 ===
    if ff_count > 0 {
        let code_bytes = size - ff_count;
        // 找到最后一个非 0xFF 的位置
        let last_valid = data.iter().rposition(|&b| b != 0xFF).unwrap_or(0);
        let used_kb = (last_valid + 1 + 1023) / 1024;
        let total_kb = (size + 1023) / 1024;
        log_push(fs, format!("  {} 0x{:08X}-0x{:08X} ({}KB/{}KB, {}%{})",
            crate::i18n::t("Code region:"),
            base_addr, base_addr + last_valid as u32,
            used_kb, total_kb, pct(code_bytes),
            if ff_count > size * 3 / 4 {
                crate::i18n::t(", mostly empty")
            } else { "" }));
    }

    // 提取可打印字符串（长度 >= 6，且含 4+ 连续字母，有效过滤指令字节噪音）
    // 增强：检测 AT 命令字符串（蓝牙/WiFi 模块配置命令）
    let mut strings_found = 0;
    let mut at_commands_found = 0;
    let mut current = String::new();
    for &b in data {
        if (0x20..=0x7E).contains(&b) {
            current.push(b as char);
        } else {
            if current.len() >= 6 && has_consecutive_alpha(&current, 4) {
                // 检测 AT 命令字符串
                if let Some(desc) = detect_at_command(&current) {
                    if strings_found < 5 {
                        log_push(fs, crate::i18n::tf("  '{}'  ; [AT command] {}",
                            &[("{}", "{}", &format!("{}", current)), ("{}", "{}", &format!("{}", desc))]));
                    }
                    at_commands_found += 1;
                } else {
                    if strings_found < 5 {
                        log_push(fs, format!("  \"{}\"", current));
                    }
                }
                strings_found += 1;
            }
            current.clear();
        }
    }
    if current.len() >= 6 && has_consecutive_alpha(&current, 4) {
        if let Some(desc) = detect_at_command(&current) {
            if strings_found < 5 {
                log_push(fs, crate::i18n::tf("  '{}'  ; [AT command] {}",
                    &[("{}", "{}", &format!("{}", current)), ("{}", "{}", &format!("{}", desc))]));
            }
            at_commands_found += 1;
        } else {
            if strings_found < 5 {
                log_push(fs, format!("  \"{}\"", current));
            }
        }
        strings_found += 1;
    }
    if at_commands_found > 0 {
        log_push(fs, crate::i18n::tf("  ★ {} AT commands found (BT/WiFi module config)", &[("{}", "{}", &format!("{}", at_commands_found))]));
    }
    if strings_found > 5 {
        log_push(fs, crate::i18n::tf("  ... {} strings total (showing first 5)", &[("{}", "{}", &format!("{}", strings_found))]));
    } else if strings_found == 0 {
        log_push(fs, crate::i18n::t("  No printable strings"));
    }

    // HEX dump（前 32 字节）
    let preview_len = data.len().min(32);
    let hex_preview: String = data[..preview_len]
        .iter()
        .map(|b| format!("{:02X} ", b))
        .collect();
    log_push(fs, format!("<< HEX [0x{:08X}]: {}", base_addr, hex_preview.trim_end()));

    // === ARM Thumb 反汇编 ===
    // 从 Reset Handler 地址开始反汇编前 20 条指令
    if sp_valid && rh_valid {
        // checked_sub：读取基址高于链接地址时 reset_handler < base_addr，
        // 直接相减在 debug 构建下 u32 下溢 panic
        let reset_offset = reset_handler.checked_sub(base_addr).map(|v| v as usize);
        match reset_offset {
            Some(off) if off + 2 <= data.len() => {
                log_push(fs, format!("  {} (0x{:08X}):",
                    crate::i18n::t("Disasm Reset Handler"),
                    reset_handler));
                let instructions = thumb_disasm::disassemble_thumb(data, base_addr, off, 20);
                for inst in &instructions {
                    log_push(fs, inst.format());
                }

                // 统计函数调用 (BL/BLX)
                let calls: Vec<_> = instructions.iter()
                    .filter(|i| i.mnemonic == "BL" || i.mnemonic == "BLX")
                    .collect();
                if !calls.is_empty() {
                    log_push(fs, format!("  {} {}",
                        crate::i18n::t("Function calls:"),
                        calls.len()));
                    for call in calls {
                        log_push(fs, format!("    {} → {}", call.mnemonic, call.operands));
                    }
                }
            }
            _ => {
                log_push(fs, crate::i18n::tf("  ⚠ Reset Handler 0x{:08X} is outside the read range ({} bytes), increase the read length",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", reset_handler)),
                      ("{}", "{}", &format!("{}", data.len()))]));
            }
        }
    }

    // === 深度分析：函数检测 + 调用图 ===
    if sp_valid && rh_valid && data.len() > 32 {
        analyze_functions(fs, data, base_addr, reset_handler);
    }
}

/// 导出 ARM Thumb 反汇编到文本文件（含函数列表、调用图、完整反汇编代码）
pub(crate) fn export_disasm_to_file(fs: &mut FlasherState, is_zh: bool) {
    if fs.last_read_data.is_empty() {
        log_push(fs, crate::i18n::t("<< No read data, read Flash first"));
        return;
    }

    let data = &fs.last_read_data;
    let base_addr = fs.address;

    // 按芯片类型路由反汇编器
    let arch = match fs.chip_type {
        ChipType::Esp32 | ChipType::Esp8266 => ChipArch::Xtensa,
        ChipType::Arduino => ChipArch::Avr,
        ChipType::Stm32 | ChipType::Gd32 | ChipType::Ch32 | ChipType::NxpLpc => ChipArch::ArmThumb,
        _ => {
            // 自动检测：通过 chip_profile 匹配
            let match_result = chip_profile::match_profile(&data[..]);
            match_result.profile.arch
        }
    };

    // ESP32/Xtensa 路径
    if arch == ChipArch::Xtensa {
        export_esp32_disasm_to_file(fs, is_zh);
        return;
    }

    // ARM Cortex-M 路径（现有逻辑）
    // 检测向量表有效性
    if data.len() < 8 {
        log_push(fs, crate::i18n::t("<< Data < 8 bytes"));
        return;
    }
    let sp = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let reset_raw = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let reset_handler = reset_raw & !1;

    // 找到代码区结束位置（连续 256 字节 0xFF 视为代码区结束）
    let code_end = {
        let mut end = data.len();
        for i in 0..data.len().saturating_sub(256) {
            if data[i..i+256].iter().all(|&b| b == 0xFF) {
                end = i;
                break;
            }
        }
        end
    };
    let code_size = code_end;

    // 检测向量表结束位置（跳过 0 值向量，在第一个非 Flash 地址处停止）
    let vector_table_size = {
        let mut vt_end = 16 * 4; // 最少 16 个核心异常
        for i in 16..(code_size / 4) {
            let offset = i * 4;
            if offset + 4 > data.len() { break; }
            let val = u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]);
            if val == 0 { continue; } // 跳过未使用的 IRQ（不停止）
            let code_addr = val & !1;
            if code_addr < base_addr || code_addr >= base_addr + code_size as u32 {
                break; // 非 Flash 地址 = 向量表结束，代码开始
            }
            vt_end = offset + 4;
        }
        vt_end
    };

    // 全固件反汇编（使用 full 模式，从向量表结束位置开始，到代码区结束位置停止）
    let code_data = &data[..code_size];
    let start_offset = vector_table_size;
    let max_insts = 10000;
    let instructions = thumb_disasm::disassemble_thumb_full(code_data, base_addr, start_offset, max_insts);

    // 收集函数入口和调用图
    use std::collections::HashMap;
    let mut call_targets: HashMap<u32, u32> = HashMap::new();
    let mut func_labels: HashMap<u32, String> = HashMap::new();

    // 向量表中的处理程序也是函数入口
    // irq_names[0] = "SP" 对应向量表索引 0（跳过），irq_names[1] = "Reset" 对应索引 1
    let max_vectors = (code_size.min(240)) / 4;
    let irq_names = ["SP", "Reset", "NMI", "HardFault", "MemManage", "BusFault",
                     "UsageFault", "Reserved", "Reserved", "Reserved", "Reserved",
                     "SVC", "DebugMon", "Reserved", "PendSV", "SysTick"];
    let stm32_irq_names = [
        "WWDG", "PVD", "TAMPER", "RTC", "FLASH", "RCC", "EXTI0", "EXTI1",
        "EXTI2", "EXTI3", "EXTI4", "DMA1_Ch1", "DMA1_Ch2", "DMA1_Ch3", "DMA1_Ch4",
        "DMA1_Ch5", "DMA1_Ch6", "DMA1_Ch7", "ADC1_2", "USB_HP_CAN_TX", "USB_LP_CAN_RX0",
        "CAN_RX1", "CAN_SCE", "EXTI9_5", "TIM1_BRK", "TIM1_UP", "TIM1_TRG_COM",
        "TIM1_CC", "TIM2", "TIM3", "TIM4", "I2C1_EV", "I2C1_ER", "I2C2_EV", "I2C2_ER",
        "SPI1", "SPI2", "USART1", "USART2", "USART3", "EXTI15_10", "RTCAlarm", "USBWakeUp",
        "TIM8_BRK", "TIM8_UP", "TIM8_TRG_COM", "TIM8_CC", "ADC3", "FSMC", "SDIO",
        "TIM5", "SPI3", "UART4", "UART5", "TIM6", "TIM7", "DMA2_Ch1", "DMA2_Ch2",
        "DMA2_Ch3", "DMA2_Ch4_5", "ETH", "ETH_WKUP", "CAN2_TX", "CAN2_RX0", "CAN2_RX1",
        "CAN2_SCE", "OTG_FS"
    ];
    for i in 0..max_vectors {
        let offset = i * 4;
        if offset + 4 > code_size { break; }
        let handler = u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]);
        if handler == 0 || handler == sp { continue; }
        let code_addr = handler & !1;
        if code_addr >= base_addr && code_addr < base_addr + code_size as u32 {
            let name = if i < irq_names.len() {
                irq_names[i]
            } else if i < 16 + stm32_irq_names.len() {
                stm32_irq_names[i - 16]
            } else {
                "IRQ"
            };
            func_labels.insert(code_addr, format!("{}_Handler", name));
        }
    }

    // PUSH {LR} = 函数入口
    let mut func_id = 0;
    for inst in &instructions {
        if inst.mnemonic == "PUSH" && inst.operands.contains("LR") {
            func_labels.entry(inst.addr).or_insert_with(|| {
                func_id += 1;
                format!("func_{}", func_id)
            });
        }
        if inst.mnemonic == "BL" {
            if let Ok(target) = u32::from_str_radix(inst.operands.trim_start_matches("0x"), 16) {
                *call_targets.entry(target).or_insert(0) += 1;
                func_labels.entry(target).or_insert_with(|| {
                    func_id += 1;
                    format!("func_{}", func_id)
                });
            }
        }
    }

    // 识别无 PUSH {LR} 的函数入口（SystemInit 模式：LDR.W SP, [PC, #imm]）
    for inst in &instructions {
        if inst.mnemonic == "LDR.W" && inst.operands.contains("SP, [PC") {
            func_labels.entry(inst.addr).or_insert_with(|| {
                func_id += 1;
                format!("func_{}", func_id)
            });
        }
    }

    // === SRAM 变量交叉引用 ===
    // 扫描 LDR [PC, #imm] 文字池，收集频繁访问的 SRAM 地址并自动命名
    let sram_refs = collect_sram_refs(&instructions, data, base_addr);
    let sram_vars: HashMap<u32, String> = sram_refs.iter()
        .filter_map(|(&addr, &count)| name_sram_var(addr, count).map(|name| (addr, name)))
        .collect();

    // === 数据区域检测 ===
    // 检测连续可疑指令（DATA?/UNPREDICTABLE/.word）标记为疑似数据区域
    let data_regions = detect_data_regions(&instructions);

    // 构建输出文本
    let mut output = String::new();
    output.push_str(&format!("; ======================================\n"));
    output.push_str(crate::i18n::t("disasm.header.title"));
    output.push_str(&format!("; ======================================\n"));
    output.push_str(&crate::i18n::tf("disasm.header.base_addr", &[("{:08X}", "{:08X}", &format!("{:08X}", base_addr))]));
    output.push_str(&crate::i18n::tf("disasm.header.code_size", &[("{}", "{}", &format!("{}", code_size)), ("{}", "{}", &format!("{}", (code_size + 1023) / 1024))]));
    output.push_str(&format!("; SP = 0x{:08X}\n", sp));
    output.push_str(&format!("; Reset Handler = 0x{:08X}\n", reset_handler));
    output.push_str(&crate::i18n::tf("disasm.header.functions", &[("{}", "{}", &format!("{}", func_labels.len()))]));
    output.push_str(&crate::i18n::tf("disasm.header.instructions", &[("{}", "{}", &format!("{}", instructions.len()))]));
    output.push_str(&crate::i18n::tf("disasm.header.sram_vars", &[("{}", "{}", &format!("{}", sram_vars.len()))]));
    output.push_str(&crate::i18n::tf("disasm.header.data_regions", &[("{}", "{}", &format!("{}", data_regions.len()))]));
    output.push_str(&format!("; ======================================\n\n"));

    // 函数列表
    output.push_str(crate::i18n::t("disasm.section.functions"));
    let mut sorted_labels: Vec<(u32, &String)> = func_labels.iter().map(|(k, v)| (*k, v)).collect();
    sorted_labels.sort_by_key(|(a, _)| *a);
    for (addr, label) in &sorted_labels {
        let calls = call_targets.get(addr).copied().unwrap_or(0);
        output.push_str(&format!(";   0x{:08X} {} ({} calls)\n", addr, label, calls));
    }
    output.push_str("\n");

    // 调用图
    output.push_str(crate::i18n::t("disasm.section.call_graph"));
    // 构建地址→函数名映射（用于查找 BL 指令所在的函数）
    let mut sorted_func_addrs: Vec<u32> = func_labels.keys().copied().collect();
    sorted_func_addrs.sort();
    for inst in &instructions {
        if inst.mnemonic == "BL" {
            if let Ok(target) = u32::from_str_radix(inst.operands.trim_start_matches("0x"), 16) {
                // 查找 BL 指令所在的函数（最近的函数入口）
                let from_label = sorted_func_addrs.binary_search(&inst.addr)
                    .map(|idx| func_labels[&sorted_func_addrs[idx]].as_str())
                    .unwrap_or_else(|idx| {
                        if idx > 0 {
                            func_labels[&sorted_func_addrs[idx - 1]].as_str()
                        } else {
                            "?"
                        }
                    });
                let to_label = func_labels.get(&target).map(|s| s.as_str()).unwrap_or("sub_0x");
                output.push_str(&format!(";   {} (0x{:08X}) → {} (0x{:08X})\n", from_label, inst.addr, to_label, target));
            }
        }
    }
    output.push_str("\n");

    // SRAM 变量交叉引用表
    if !sram_vars.is_empty() {
        output.push_str(crate::i18n::t("disasm.section.sram_xref"));
        let mut sorted_sram: Vec<(u32, &String)> = sram_vars.iter().map(|(k, v)| (*k, v)).collect();
        sorted_sram.sort_by_key(|(a, _)| *a);
        for (addr, name) in &sorted_sram {
            let count = sram_refs.get(addr).copied().unwrap_or(0);
            output.push_str(&format!(";   0x{:08X} {} ({} refs)\n", addr, name, count));
        }
        output.push_str("\n");
    }

    // 反汇编代码
    output.push_str(crate::i18n::t("disasm.section.code"));
    for inst in &instructions {
        // 数据区域标记
        if data_regions.contains(&inst.addr) {
            output.push_str(&crate::i18n::tf("disasm.data_region", &[("{:08X}", "{:08X}", &format!("{:08X}", inst.addr))]));
        }
        // 函数标签
        if let Some(label) = func_labels.get(&inst.addr) {
            output.push_str(&format!("\n; ──────────────────────────────────────\n"));
            output.push_str(&format!("{}:  ; 0x{:08X}\n", label, inst.addr));
            output.push_str(&format!("; ──────────────────────────────────────\n"));
        }
        // 对 LDR [PC, #imm] 指令，解析加载值
        let line = if inst.mnemonic == "LDR" && inst.operands.contains("[PC,") {
            // 提取目标地址（在 comment 中已有 → [0xXXXXXXXX]）
            if let Some(comment) = &inst.comment {
                if let Some(addr_str) = comment.find("0x") {
                    let addr_part = &comment[addr_str..];
                    let addr_end = addr_part.find(']').unwrap_or(addr_part.len());
                    let target_addr_str = &addr_part[..addr_end];
                    if let Ok(target_addr) = u32::from_str_radix(target_addr_str.trim_start_matches("0x"), 16) {
                        let target_offset = (target_addr.wrapping_sub(base_addr)) as usize;
                        if target_offset + 4 <= data.len() {
                            let val = u32::from_le_bytes([data[target_offset], data[target_offset+1], data[target_offset+2], data[target_offset+3]]);
                            // 使用符号化函数标注地址值（外设名/系统寄存器/函数指针等）
                            let val_note = format_addr_value(val, base_addr, data.len(), &func_labels);
                            // 若加载值指向已命名的 SRAM 变量，追加变量名
                            let sram_tag = sram_vars.get(&val).map(|n| format!(" <{}>", n)).unwrap_or_default();
                            inst.format().replace(&format!("→ [{}]", target_addr_str), &format!("→ [{}{}{}]", target_addr_str, val_note, sram_tag))
                        } else {
                            inst.format()
                        }
                    } else {
                        inst.format()
                    }
                } else {
                    inst.format()
                }
            } else {
                inst.format()
            }
        } else {
            inst.format()
        };
        output.push_str(&format!("  {}\n", line));
    }

    // 保存到文件
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let chip_prefix = match fs.chip_type {
        ChipType::Esp32 => "esp32",
        ChipType::Esp8266 => "esp8266",
        ChipType::Arduino => "avr",
        ChipType::Stm8 => "stm8",
        ChipType::NxpLpc => "lpc",
        ChipType::Gd32 => "gd32",
        ChipType::Ch32 => "ch32",
        ChipType::Pic => "pic",
        _ => "stm32",
    };
    let default_name = format!("{}_disasm_{}.asm", chip_prefix, timestamp);

    let path = if let Some(p) = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .add_filter("Assembly", &["asm", "txt"])
        .save_file()
    {
        p
    } else {
        // 后备路径
        let mut tmp = std::env::temp_dir();
        tmp.push(&default_name);
        tmp
    };

    match std::fs::write(&path, &output) {
        Ok(()) => {
            log_push(fs, crate::i18n::tf("<< Disasm exported: {} ({} insts, {} functions)", &[("{}", "{}", &format!("{}", path.display())), ("{}", "{}", &format!("{}", instructions.len())), ("{}", "{}", &format!("{}", func_labels.len()))]));
        }
        Err(e) => {
            log_push(fs, crate::i18n::tf("<< Export failed: {}", &[("{}", "{}", &format!("{}", e))]));
        }
    }
}

/// 深度函数分析：检测函数边界、构建调用图
pub(crate) fn analyze_functions(fs: &mut FlasherState, data: &[u8], base_addr: u32, reset_handler: u32) {
    // checked_sub：reset_handler < base_addr 时直接相减会在 debug 构建下下溢 panic
    let Some(code_offset) = reset_handler.checked_sub(base_addr).map(|v| v as usize) else {
        return;
    };
    // 找到代码区结束位置（最后一个非 0xFF 的字节）
    let code_end = data.iter().rposition(|&b| b != 0xFF).unwrap_or(0);
    if code_end <= code_offset {
        return;
    }

    log_push(fs, format!("\n=== {} ===", crate::i18n::t("Deep Function Analysis")));

    // 1. 全固件反汇编（限制最多 500 条指令，避免日志过长）
    let max_insts = 500;
    let instructions = thumb_disasm::disassemble_thumb(data, base_addr, code_offset, max_insts);

    // 2. 收集 BL 调用目标（函数入口点）
    use std::collections::HashMap;
    let mut call_targets: HashMap<u32, u32> = HashMap::new(); // addr -> call_count
    let mut bl_list: Vec<(u32, u32)> = Vec::new(); // (from_addr, to_addr)
    for inst in &instructions {
        if inst.mnemonic == "BL" {
            if let Ok(target) = u32::from_str_radix(inst.operands.trim_start_matches("0x"), 16) {
                *call_targets.entry(target).or_insert(0) += 1;
                bl_list.push((inst.addr, target));
            }
        }
    }

    // 3. 检测函数边界：PUSH {LR} = 函数入口，BX LR / POP {PC} = 函数出口
    let mut func_starts: Vec<(u32, &str)> = Vec::new(); // (addr, label_hint)
    for inst in &instructions {
        // PUSH {LR} (0xB500-0xB5FF with bit 8 set) = 函数入口
        if inst.mnemonic == "PUSH" && inst.operands.contains("LR") {
            func_starts.push((inst.addr, "PUSH {LR}"));
        }
        // BL 目标也是函数入口
        if inst.mnemonic == "BL" {
            if let Ok(target) = u32::from_str_radix(inst.operands.trim_start_matches("0x"), 16) {
                if !func_starts.iter().any(|(a, _)| *a == target) {
                    func_starts.push((target, "BL target"));
                }
            }
        }
    }
    func_starts.sort_by_key(|(a, _)| *a);
    func_starts.dedup_by_key(|(a, _)| *a);

    log_push(fs, format!("  {} {}",
        crate::i18n::t("Function entries:"),
        func_starts.len()));

    // 显示前 20 个函数入口
    for (i, (addr, hint)) in func_starts.iter().enumerate() {
        if i >= 20 {
            log_push(fs, crate::i18n::tf("  ... {} functions total (showing first 20)", &[("{}", "{}", &format!("{}", func_starts.len()))]));
            break;
        }
        let call_count = call_targets.get(addr).copied().unwrap_or(0);
        let call_info = if call_count > 0 {
            format!(" ({} {})", call_count, crate::i18n::t("calls"))
        } else {
            String::new()
        };
        log_push(fs, format!("    func_0x{:08X} {}{}", addr, hint, call_info));
    }

    // 4. 调用图摘要
    if !bl_list.is_empty() {
        log_push(fs, format!("\n  {} ({} {}):",
            crate::i18n::t("Call graph"),
            bl_list.len(),
            { crate::i18n::t("calls") }));
        // 显示前 15 个调用
        for (i, (from, to)) in bl_list.iter().enumerate() {
            if i >= 15 {
                log_push(fs, crate::i18n::tf("  ... {} calls total", &[("{}", "{}", &format!("{}", bl_list.len()))]));
                break;
            }
            log_push(fs, format!("    0x{:08X} → 0x{:08X} (BL)", from, to));
        }
    }

    // 5. 导出完整反汇编到文件
    log_push(fs, format!("\n  {} ({} {})",
        crate::i18n::t("Tip: Click 'Export Disasm' to save full disassembly"),
        instructions.len(),
        crate::i18n::t("instructions")));
}

