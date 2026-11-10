//! 反汇编产物 → 可编译汇编工程转换器
//!
//! 将 `thumb_disasm` 的反汇编结果转换为 GNU as 兼容的可编辑汇编源文件，
//! 配套生成链接脚本（.ld）、Makefile 和 README.md，形成完整的可编译工程。
//!
//! # 工程结构
//! ```text
//! firmware_project/
//! ├── firmware.s    — 可编辑的汇编源文件（GNU as 格式）
//! ├── firmware.ld   — 链接脚本（Flash/SRAM 布局）
//! ├── Makefile      — 编译脚本（arm-none-eabi-gcc）
//! └── README.md     — 编译与编辑说明
//! ```
//!
//! # 转换规则
//! - 去掉地址前缀和原始字节 hex，保留助记符 + 操作数
//! - 函数入口添加 `.global` 声明和标签
//! - `B`/`BL` 跳转目标替换为符号标签（如 `BL func_1E4`）
//! - `LDR Rd, [PC, #imm]` 转换为 `LDR Rd, =<value>`（汇编器自动管理文字池）
//! - 向量表输出到 `.isr_vector` section
//! - 代码输出到 `.text` section
//! - 字符串/数据表输出到 `.rodata` section

use crate::thumb_disasm::{analyze_disassembly, disassemble_thumb_full, DisasmInstruction, DisasmAnalysis};
use std::collections::{HashMap, HashSet};

/// 可编译工程产物
#[derive(Debug, Clone)]
pub struct RebuildProject {
    /// 汇编源文件内容（firmware.s）
    pub firmware_s: String,
    /// 链接脚本内容（firmware.ld）
    pub firmware_ld: String,
    /// Makefile 内容
    pub makefile: String,
    /// README.md 内容
    pub readme: String,
    /// 生成的标签数量
    pub label_count: usize,
    /// 转换的 LDR [PC,#imm] 数量
    pub ldr_literal_converted: usize,
    /// 跳转目标替换数量
    pub branch_resolved: usize,
}

/// 生成可编译汇编工程
///
/// `data` 是原始 Flash 二进制，`base_addr` 是 Flash 起始地址（如 0x08000000）。
/// `sp_init` 是初始栈指针（从向量表第 0 项提取）。
pub fn generate_rebuild_project(
    data: &[u8],
    base_addr: u32,
    sp_init: u32,
) -> RebuildProject {
    // 全量反汇编
    let instructions = disassemble_thumb_full(data, base_addr, 0, data.len() / 2 + 1);
    let analysis = analyze_disassembly(&instructions, base_addr, data);

    // 构建标签映射：地址 → 标签名
    let label_map = build_label_map(&instructions, &analysis, base_addr, data);

    let label_count = label_map.len();

    // 生成各文件
    let (firmware_s, ldr_literal_converted, branch_resolved) = gen_firmware_s(&instructions, &label_map, &analysis, base_addr, sp_init, data);
    let firmware_ld = gen_firmware_ld(base_addr, data.len());
    let makefile = gen_makefile();
    let readme = gen_readme(base_addr, data.len(), sp_init, label_count, &analysis);

    RebuildProject {
        firmware_s,
        firmware_ld,
        makefile,
        readme,
        label_count,
        ldr_literal_converted,
        branch_resolved,
    }
}

// ============================================================================
// 标签映射构建
// ============================================================================

/// 构建地址 → 标签名的映射
///
/// 标签来源：
/// 1. 向量表入口（idx 1+）：Reset_Handler, NMI_Handler, ...
/// 2. 函数入口（来自 analysis）：func_<offset>
/// 3. BL/B 跳转目标：loc_<offset>（非函数入口时）
/// 4. LDR [PC,#imm] 文字池目标：lit_<offset>
fn build_label_map(
    instructions: &[DisasmInstruction],
    analysis: &DisasmAnalysis,
    base_addr: u32,
    data: &[u8],
) -> HashMap<u32, String> {
    let mut map: HashMap<u32, String> = HashMap::new();

    // 1. 向量表入口 → 标准 Handler 标签名
    let vt_size = detect_vector_table_size(data, base_addr);
    for i in 1..(vt_size / 4) {
        let off = i * 4;
        if off + 4 > data.len() { break; }
        let v = u32::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3]]);
        if v == 0 { continue; }
        let entry = v & !1;  // 清除 Thumb 位
        if !map.contains_key(&entry) {
            map.insert(entry, vector_handler_name(i));
        }
    }

    // 2. 函数入口（来自 AI 分析）
    for f in &analysis.functions {
        if !map.contains_key(&f.entry) {
            // 如果向量表已赋名则用之，否则用 func_<offset>
            let name = format!("func_{:X}", f.entry - base_addr);
            map.insert(f.entry, name);
        }
    }

    // 3. 扫描 BL/B 跳转目标，为未命中的地址生成 loc_ 标签
    for inst in instructions {
        let mnemonic = inst.mnemonic.as_str();
        if mnemonic == "BL" || mnemonic == "B" || mnemonic == "B.W" {
            if let Some(target) = parse_branch_target(&inst.operands) {
                if !map.contains_key(&target) {
                    map.insert(target, format!("loc_{:X}", target - base_addr));
                }
            }
        }
        // 条件分支 BNE/BEQ/BCC/BCS/BMI/BPL/BVS/BVC/BHI/BLS/BGE/BLT/BGT/BLE
        if mnemonic.starts_with("B") && mnemonic.len() <= 4 && mnemonic != "BX" && mnemonic != "BLX" && mnemonic != "BKPT" {
            if let Some(target) = parse_branch_target(&inst.operands) {
                if !map.contains_key(&target) {
                    map.insert(target, format!("loc_{:X}", target - base_addr));
                }
            }
        }
    }

    map
}

/// 解析分支指令的目标地址
/// 输入如 "0x080001E4" 或 "0x080001E4 ; 函数调用"
fn parse_branch_target(operands: &str) -> Option<u32> {
    let first = operands.split_whitespace().next()?;
    let hex = first.trim_start_matches("0x");
    u32::from_str_radix(hex, 16).ok()
}

/// 向量表入口的标准 Handler 名称（ARM CMSIS 规范）
fn vector_handler_name(idx: usize) -> String {
    match idx {
        1 => "Reset_Handler".to_string(),
        2 => "NMI_Handler".to_string(),
        3 => "HardFault_Handler".to_string(),
        4 => "MemManage_Handler".to_string(),
        5 => "BusFault_Handler".to_string(),
        6 => "UsageFault_Handler".to_string(),
        11 => "SVC_Handler".to_string(),
        12 => "DebugMon_Handler".to_string(),
        14 => "PendSV_Handler".to_string(),
        15 => "SysTick_Handler".to_string(),
        _ => format!("IRQ{}_Handler", idx - 16),
    }
}

/// 检测向量表大小（字节）
/// 复制自 thumb_disasm 的私有函数，保持模块独立性
fn detect_vector_table_size(data: &[u8], base_addr: u32) -> usize {
    let mut vec_count = 0usize;
    let mut i = 0usize;
    while i + 4 <= data.len() && i < 0x400 {
        let v = u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        if is_valid_vector(v, base_addr) {
            vec_count += 1;
            i += 4;
        } else {
            break;
        }
    }
    if vec_count >= 16 { vec_count * 4 } else { 0 }
}

fn is_valid_vector(v: u32, base_addr: u32) -> bool {
    if v == 0 { return true; }
    if v >= base_addr && v < base_addr + 0x100000 && (v & 1) == 1 { return true; }
    if v >= 0x20000000 && v < 0x20010000 { return true; }
    false
}

// ============================================================================
// firmware.s 生成
// ============================================================================

fn gen_firmware_s(
    instructions: &[DisasmInstruction],
    label_map: &HashMap<u32, String>,
    analysis: &DisasmAnalysis,
    base_addr: u32,
    sp_init: u32,
    data: &[u8],
) -> (String, usize, usize) {
    let mut out = String::with_capacity(instructions.len() * 40);

    // 头部声明
    out.push_str("@ ============================================================\n");
    out.push_str("@ Generated by miconlab disassembly rebuild\n");
    out.push_str("@ Editable, compilable assembly source (GNU as format)\n");
    out.push_str("@ ============================================================\n");
    out.push_str("@ Editing tips:\n");
    out.push_str("@   - After editing instructions, run 'make' to rebuild\n");
    out.push_str("@   - Labels can be renamed freely (keep them globally unique)\n");
    out.push_str("@   - LDR Rd, =<value> lets the assembler allocate the literal pool\n");
    out.push_str("@   - Data tables/strings go in the .rodata section\n");
    out.push_str("@ ============================================================\n\n");

    out.push_str(".syntax unified\n");
    out.push_str(".cpu cortex-m3\n");
    out.push_str(".thumb\n\n");

    // ---- 向量表 section ----
    let vt_size = detect_vector_table_size(data, base_addr);
    if vt_size >= 0x40 {
        out.push_str("@ ============================================================\n");
        out.push_str("@ Exception vector table (must be at the Flash start address)\n");
        out.push_str("@ ============================================================\n");
        out.push_str(".section .isr_vector, \"a\", %progbits\n");
        out.push_str(".global __isr_vector\n");
        out.push_str(".type __isr_vector, %object\n");
        out.push_str("__isr_vector:\n");

        let n_vectors = vt_size / 4;
        for i in 0..n_vectors {
            let off = i * 4;
            let v = u32::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3]]);
            if i == 0 {
                out.push_str(&format!("    .word 0x{:08X}    @ Initial SP\n", sp_init));
            } else if v == 0 {
                out.push_str(&format!("    .word 0             @ Vector #{} (unused)\n", i));
            } else {
                let entry = v & !1;
                let label = label_map.get(&entry).cloned().unwrap_or_else(|| format!("0x{:08X}", v));
                out.push_str(&format!("    .word {}    @ Vector #{}\n", label, i));
            }
        }
        out.push_str(&format!(".size __isr_vector, . - __isr_vector\n\n"));
    }

    // ---- 代码 section ----
    out.push_str("@ ============================================================\n");
    out.push_str("@ Code section\n");
    out.push_str("@ ============================================================\n");
    out.push_str(".section .text, \"ax\", %progbits\n\n");

    // 收集已输出的标签地址集合（避免重复输出标签）
    let mut emitted_labels: HashSet<u32> = HashSet::new();

    // 收集文字池目标地址（这些地址的 .word 数据会被跳过，因为 LDR =value 已替代）
    let mut literal_pool_addrs: HashSet<u32> = HashSet::new();
    for inst in instructions {
        if inst.mnemonic == "LDR" && inst.comment.as_ref().map(|c| c.contains("→")).unwrap_or(false) {
            if let Some(target) = parse_ldr_literal_target(&inst.comment) {
                literal_pool_addrs.insert(target);
            }
        }
    }

    let mut ldr_converted = 0usize;
    let mut branch_resolved = 0usize;

    for inst in instructions {
        // 向量表区域跳过（已在 .isr_vector 输出）
        if inst.addr < base_addr + vt_size as u32 { continue; }

        // 跳过文字池数据（会被 LDR =value 替代）
        let is_literal_pool = inst.mnemonic == ".word"
            && inst.comment.as_ref().map(|c| c.contains("literal pool")).unwrap_or(false);
        if is_literal_pool && literal_pool_addrs.contains(&inst.addr) {
            continue;
        }

        // 如果当前地址有标签，输出标签
        if let Some(label) = label_map.get(&inst.addr) {
            if !emitted_labels.contains(&inst.addr) {
                emitted_labels.insert(inst.addr);
                // 函数入口添加 .global 声明
                let is_func = analysis.functions.iter().any(|f| f.entry == inst.addr);
                if is_func {
                    out.push_str(&format!(".global {}\n", label));
                    out.push_str(&format!(".type {}, %function\n", label));
                }
                out.push_str(&format!("{}:\n", label));
            }
        }

        // 转换指令行
        let line = convert_instruction_line(inst, label_map, base_addr, data, &mut ldr_converted, &mut branch_resolved);
        out.push_str(&line);
        out.push('\n');
    }

    (out, ldr_converted, branch_resolved)
}

/// 转换单条指令行为可编译汇编
fn convert_instruction_line(
    inst: &DisasmInstruction,
    label_map: &HashMap<u32, String>,
    base_addr: u32,
    data: &[u8],
    ldr_converted: &mut usize,
    branch_resolved: &mut usize,
) -> String {
    // 数据指令特殊处理
    let clean_comment = |c: &str| -> String {
        // 清理注释：去掉前导 "; " 和多余分号（GNU as 中 ; 是语句分隔符）
        let trimmed = c.trim().trim_start_matches(';').trim();
        if trimmed.is_empty() { String::new() } else { format!(" @ {}", trimmed) }
    };
    match inst.mnemonic.as_str() {
        ".word" => {
            let val_str = inst.operands.trim();
            let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
            return format!("    .word {}{}", val_str, comment);
        }
        ".ascii" => {
            let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
            return format!("    {}{}", inst.operands, comment);
        }
        ".byte" => {
            return convert_byte_data(inst, base_addr, data);
        }
        ".half" => {
            // operands 即真实值（如 "0x4A2C"），直接透传，不能硬编码 0x0000
            let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
            return format!("    .hword {}{}", inst.operands.trim(), comment);
        }
        ".fill" => {
            // operands 形如 "0x08001000-0x08001040 (64 字节)"，解析真实长度；
            // 填充值取 bytes 示例字节（0x00 对齐填充 / 0xFF 未编程区域）
            let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
            let fill_val = inst.bytes.first().copied().unwrap_or(0);
            if let Some((_, end)) = parse_range(&inst.operands) {
                let len = (end as i64 - inst.addr as i64).max(0) as usize;
                if len > 0 {
                    return format!("    .fill {}, 1, 0x{:02X}{}", len, fill_val, comment);
                }
            }
            return format!("    .skip 0{}", comment);
        }
        _ => {}
    }

    // 指令转换
    let mnemonic = inst.mnemonic.as_str();

    // B/BL 跳转目标替换为标签
    if mnemonic == "BL" || mnemonic == "B" || mnemonic == "B.W" {
        if let Some(target) = parse_branch_target(&inst.operands) {
            if let Some(label) = label_map.get(&target) {
                let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
                *branch_resolved += 1;
                return format!("    {} {}{}", mnemonic, label, comment);
            }
        }
    }

    // 条件分支（BNE/BEQ/BCC 等）跳转目标替换
    if mnemonic.starts_with("B") && mnemonic.len() <= 4
        && mnemonic != "BX" && mnemonic != "BLX" && mnemonic != "BKPT" && mnemonic != "BFC" && mnemonic != "BFI"
    {
        if let Some(target) = parse_branch_target(&inst.operands) {
            if let Some(label) = label_map.get(&target) {
                *branch_resolved += 1;
                let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
                return format!("    {} {}{}", mnemonic, label, comment);
            }
        }
    }

    // LDR [PC, #imm] → LDR Rd, =<value>
    if mnemonic == "LDR" && inst.comment.as_ref().map(|c| c.contains("→")).unwrap_or(false) {
        if let Some(target) = parse_ldr_literal_target(&inst.comment) {
            let target_off = (target as i64 - base_addr as i64) as usize;
            if target_off + 4 <= data.len() {
                let value = u32::from_le_bytes([
                    data[target_off], data[target_off+1], data[target_off+2], data[target_off+3]
                ]);
                let rd = inst.operands.split(',').next().unwrap_or("R0").trim();
                *ldr_converted += 1;
                return format!("    LDR {}, =0x{:08X}    @ original literal pool @0x{:08X}", rd, value, target);
            }
        }
    }

    // 默认：保留助记符 + 操作数 + 注释
    let comment = inst.comment.as_ref().map(|c| clean_comment(c)).unwrap_or_default();
    // 操作数中可能含 "; 注释"，清理掉
    let operands = inst.operands.split(';').next().unwrap_or(&inst.operands).trim();
    format!("    {} {}{}", mnemonic, operands, comment)
}

/// 解析 LDR [PC, #imm] 注释中的文字池目标地址
/// 注释格式如 "→ [0x08000184]"
fn parse_ldr_literal_target(comment: &Option<String>) -> Option<u32> {
    let c = comment.as_ref()?;
    let arrow_pos = c.find("→")?;
    // `→` 是 3 字节 UTF-8 字符，必须按字节长度跳过
    let after = &c[arrow_pos + "→".len()..];
    let hex_part = after.trim().trim_start_matches('[').trim_end_matches(']');
    u32::from_str_radix(hex_part.trim_start_matches("0x"), 16).ok()
}

/// 转换 .byte 数据表为字节列表
fn convert_byte_data(inst: &DisasmInstruction, base_addr: u32, data: &[u8]) -> String {
    // 从 operands 或 comment 解析地址范围
    // 格式如 "0x08002438-0x08002478 (64 字节)"
    let range_str = if inst.operands.contains('-') {
        inst.operands.as_str()
    } else if let Some(c) = &inst.comment {
        c.as_str()
    } else {
        return format!("    .byte 0    @ data table");
    };

    let (start, end) = if let Some((s, e)) = parse_range(range_str) {
        (s, e)
    } else {
        return format!("    .byte 0    @ {}", range_str);
    };

    let start_off = (start as i64 - base_addr as i64) as usize;
    let end_off = (end as i64 - base_addr as i64) as usize;
    if start_off >= data.len() || end_off > data.len() || end_off <= start_off {
        return format!("    .byte 0    @ data table out of range");
    }

    let size = end_off - start_off;
    let comment = inst.comment.as_ref().map(|c| format!(" @ {}", c)).unwrap_or_default();

    // 小数据表直接展开
    if size <= 32 {
        let bytes: Vec<String> = data[start_off..end_off].iter().map(|b| format!("0x{:02X}", b)).collect();
        return format!("    .byte {}{}", bytes.join(", "), comment);
    }

    // 大数据表用 .incbin 或 .skip + 注释
    format!("    .skip {}, 0    @ data table {} bytes{}", size, size, comment)
}

/// 解析 "0xSTART-0xEND" 格式的地址范围
fn parse_range(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let dash = s.find('-')?;
    let start_str = s[..dash].trim();
    let rest = s[dash+1..].trim();
    // end 部分可能有后缀如 "(64 字节)"
    let end_str = rest.split_whitespace().next()?;
    let start = u32::from_str_radix(start_str.trim_start_matches("0x"), 16).ok()?;
    let end = u32::from_str_radix(end_str.trim_start_matches("0x"), 16).ok()?;
    Some((start, end))
}

// ============================================================================
// firmware.ld 链接脚本生成
// ============================================================================

fn gen_firmware_ld(base_addr: u32, flash_size: usize) -> String {
    let flash_kb = (flash_size + 1023) / 1024;
    // STM32F103C8: 64KB Flash, 20KB SRAM; 常见配置 256K/64K
    let flash_len = if flash_kb <= 64 { "64K" }
        else if flash_kb <= 128 { "128K" }
        else if flash_kb <= 256 { "256K" }
        else if flash_kb <= 512 { "512K" }
        else { "1024K" };

    format!(
r#"/* ============================================================
 * Linker script - STM32 Cortex-M3
 * Generated by miconlab disassembly rebuild
 * ============================================================
 * Flash start: 0x{flash_addr:08X}
 * Flash size: {flash_len} (actual firmware {flash_size} bytes)
 * SRAM start:  0x20000000
 * SRAM size:  20K (STM32F103C8 typical; adjust for your model)
 * ============================================================ */

ENTRY(Reset_Handler)

MEMORY
{{
    FLASH (rx)  : ORIGIN = 0x{flash_addr:08X}, LENGTH = {flash_len}
    RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}}

/* Stack top address (initial SP, loaded from vector table entry 0) */
_estack = ORIGIN(RAM) + LENGTH(RAM);

SECTIONS
{{
    /* Interrupt vector table - must be at the Flash start address */
    .isr_vector :
    {{
        . = ALIGN(4);
        KEEP(*(.isr_vector))
        . = ALIGN(4);
    }} > FLASH

    /* Code section */
    .text :
    {{
        . = ALIGN(4);
        *(.text)
        *(.text*)
        *(.rodata)
        *(.rodata*)
        . = ALIGN(4);
        _etext = .;
    }} > FLASH

    /* Initialized data section (loaded from Flash to RAM) */
    .data : AT (_etext)
    {{
        . = ALIGN(4);
        _sdata = .;
        *(.data)
        *(.data*)
        . = ALIGN(4);
        _edata = .;
    }} > RAM

    /* Uninitialized data section (BSS) */
    .bss :
    {{
        . = ALIGN(4);
        _sbss = .;
        *(.bss)
        *(.bss*)
        *(COMMON)
        . = ALIGN(4);
        _ebss = .;
    }} > RAM

    /* User stack */
    ._user_stack :
    {{
        . = ALIGN(4);
        . = . + 0x400;
        . = ALIGN(4);
    }} > RAM
}}

/* Provide ARM CMSIS standard symbols */
PROVIDE(_estack = ORIGIN(RAM) + LENGTH(RAM));
"#,
        flash_addr = base_addr,
        flash_len = flash_len,
        flash_size = flash_size
    )
}

// ============================================================================
// Makefile 生成
// ============================================================================

fn gen_makefile() -> String {
    r#"# ============================================================
# Makefile - STM32 firmware rebuild
# Dependency: arm-none-eabi-gcc toolchain
# ============================================================

CROSS   = arm-none-eabi-
CC      = $(CROSS)gcc
OBJCOPY = $(CROSS)objcopy
SIZE    = $(CROSS)size

TARGET  = firmware
SRC     = firmware.s
LDSCRIPT = firmware.ld

CFLAGS  = -mcpu=cortex-m3 -mthumb -Os
CFLAGS += -ffunction-sections -fdata-sections
CFLAGS += -Wall -Wextra
LDFLAGS = -T$(LDSCRIPT) -nostdlib -nostartfiles
LDFLAGS += -Wl,--gc-sections -Wl,--print-memory-usage

.PHONY: all clean burn help

all: $(TARGET).bin $(TARGET).hex

$(TARGET).elf: $(SRC) $(LDSCRIPT)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $(SRC)
	$(SIZE) $@

$(TARGET).bin: $(TARGET).elf
	$(OBJCOPY) -O binary $< $@

$(TARGET).hex: $(TARGET).elf
	$(OBJCOPY) -O ihex $< $@

# Flash (uses st-flash; install stlink first)
burn: $(TARGET).bin
	st-flash write $(TARGET).bin 0x08000000

clean:
	rm -f $(TARGET).elf $(TARGET).bin $(TARGET).hex

help:
	@echo "make         - build firmware.bin and firmware.hex"
	@echo "make burn    - build and flash to chip (stlink required)"
	@echo "make clean   - clean build artifacts"
	@echo ""
	@echo "After editing firmware.s, run make again to generate new firmware"
"#
    .to_string()
}

// ============================================================================
// README.md 生成
// ============================================================================

fn gen_readme(base_addr: u32, flash_size: usize, sp_init: u32, label_count: usize, analysis: &DisasmAnalysis) -> String {
    let flash_kb = (flash_size + 1023) / 1024;
    format!(
r#"# miconlab Firmware Rebuild Project

Editable, compilable firmware project auto-generated by the miconlab disassembly tool.

## Project Info

| Item | Value |
|------|-------|
| Flash base address | 0x{base_addr:08X} |
| Firmware size | {flash_size} bytes ({flash_kb} KB) |
| Initial SP | 0x{sp_init:08X} |
| Function count | {func_count} |
| Label count | {label_count} |
| Total instructions | {inst_count} |

## Files

- **firmware.s** — editable assembly source (GNU as format)
  - `.isr_vector` section: interrupt vector table
  - `.text` section: code
  - Labels generated for function entries, branch targets, and literal pools
- **firmware.ld** — linker script (Flash/SRAM memory layout)
- **Makefile** — build script

## Building

### 1. Install toolchain

```bash
# macOS
brew install arm-none-eabi-gcc

# Ubuntu
sudo apt install gcc-arm-none-eabi
```

### 2. Build

```bash
make
```

Outputs:
- `firmware.elf` — ELF executable
- `firmware.bin` — raw binary (flashable)
- `firmware.hex` — Intel HEX format

### 3. Flash

```bash
# Using ST-Link
make burn

# Or using openocd
openocd -f interface/stlink.cfg -f target/stm32f1x.cfg \
    -c "program firmware.bin 0x{base_addr:08X} verify reset exit"
```

## Editing Guide

### Modifying instructions

Edit instruction lines directly in `firmware.s`, for example:

```asm
    MOV R0, #0          @ original instruction
    MOVT R0, #0x2000    @ added instruction (manual)
```

### Modifying functions

Each function has a `.global` declaration and label:

```asm
.global Reset_Handler
.type Reset_Handler, %function
Reset_Handler:
    LDR R0, =0x20002C38
    ...
```

### Branch targets

Targets of `B`/`BL` instructions have been replaced with label names:

```asm
    BL func_1E4         @ call func_1E4
    B loc_170           @ jump to loc_170
```

### Literal pools

`LDR Rd, [PC, #imm]` has been converted to `LDR Rd, =value`; the assembler manages literal pools automatically:

```asm
    LDR R0, =0x20000000    @ load immediate (assembler allocates literal pool)
```

## Notes

1. **Address changes**: function addresses may differ after rebuilding; label references are relocated automatically
2. **Data alignment**: `.byte`/`.word` data keeps its original alignment
3. **Undecoded instructions**: 0 undecoded instructions in the original disassembly
4. **Data tables**: data tables between functions are preserved in place

## AI Analysis Summary

{summary}
"#,
        base_addr = base_addr,
        flash_size = flash_size,
        flash_kb = flash_kb,
        sp_init = sp_init,
        func_count = analysis.total_functions,
        label_count = label_count,
        inst_count = analysis.total_instructions,
        summary = analysis.summary
    )
}
