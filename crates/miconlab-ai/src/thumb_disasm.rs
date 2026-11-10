//! 基础 ARM Thumb-16 指令反汇编器
//!
//! 解码 Thumb-16 指令集（Cortex-M0/M3 常用指令），用于固件初步分析。
//! 不依赖外部库，纯 Rust 实现。
//!
//! 支持的指令类型：
//! - 分支: B, B<cond>, BX, BL (Thumb-2 扩展)
//! - 加载/存储: LDR, LDR[lit], STR, LDRB, STRB, LDRH, STRH
//! - 算术: ADD, SUB, MUL, CMP
//! - 逻辑: AND, ORR, EOR, BIC, MVN
//! - 移位: LSL, LSR, ASR, ROR
//! - 栈操作: PUSH, POP
//! - 数据移动: MOV, LDR[lit]
//! - 特殊: NOP, BKPT, SVC, CPS

/// 反汇编结果
#[derive(Debug, Clone)]
pub struct DisasmInstruction {
    /// 指令地址
    pub addr: u32,
    /// 原始字节
    pub bytes: Vec<u8>,
    /// 助记符 (如 "MOV", "B", "LDR")
    pub mnemonic: String,
    /// 操作数 (如 "R0, R1", "#0x10", "[R0, #4]")
    pub operands: String,
    /// 注释 (如 "; 跳转到 0x08000168")
    pub comment: Option<String>,
}

impl DisasmInstruction {
    pub fn format(&self) -> String {
        let bytes_hex: String = self.bytes.iter().map(|b| format!("{:02X} ", b)).collect();
        let bytes_hex = format!("{:<12}", bytes_hex.trim_end());
        let comment = self.comment.as_ref().map(|c| {
            if c.starts_with(';') {
                format!("  {}", c)
            } else {
                format!("  ; {}", c)
            }
        }).unwrap_or_default();
        format!("  0x{:08X}: {} {:<8} {}{}", self.addr, bytes_hex, self.mnemonic, self.operands, comment)
    }
}

// ============================================================================
// AI 感知分析层
// ============================================================================

/// 函数信息（AI 感知分析结果）
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    /// 函数入口地址
    pub entry: u32,
    /// 函数名称（从向量表或启发式推断）
    pub name: String,
    /// 函数结束地址（BX LR / POP {PC} 后）
    pub end: Option<u32>,
    /// 函数大小（字节）
    pub size: usize,
    /// 调用此函数的 BL 指令地址列表
    pub callers: Vec<u32>,
    /// 此函数调用的函数地址列表
    pub callees: Vec<u32>,
    /// 是否为中断/异常处理函数
    pub is_handler: bool,
}

/// 代码模式类型
#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// switch 语句（TBB/TBH 跳转表）
    SwitchStatement,
    /// 原子操作（LDREX/STREX 序列）
    AtomicOperation,
    /// 临界区（CPS i / CPS ie 序列）
    CriticalSection,
    /// 字符串引用（LDR PC-relative 到 ASCII 数据）
    StringReference,
    /// 循环（B 到自身或向后分支）
    Loop,
    /// 空闲等待（WFE/WFI 循环）
    IdleWait,
}

/// 代码模式（AI 感知识别的代码结构）
#[derive(Debug, Clone)]
pub struct CodePattern {
    pub kind: PatternKind,
    pub addr: u32,
    pub description: String,
}

impl CodePattern {
    /// 返回模式类型的简短标签（用于输出）
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            PatternKind::SwitchStatement => "switch",
            PatternKind::AtomicOperation => "atomic",
            PatternKind::CriticalSection => "critical",
            PatternKind::StringReference => "string",
            PatternKind::Loop => "loop",
            PatternKind::IdleWait => "idle",
        }
    }
}

/// 反汇编分析结果（AI 感知分析）
#[derive(Debug, Clone)]
pub struct DisasmAnalysis {
    /// 识别到的函数列表
    pub functions: Vec<FunctionInfo>,
    /// 识别到的代码模式
    pub patterns: Vec<CodePattern>,
    /// 指令总数
    pub total_instructions: usize,
    /// 函数总数
    pub total_functions: usize,
    /// 跳转表总数
    pub total_jump_tables: usize,
    /// 文字池总数
    pub total_literal_pools: usize,
    /// 数据表总数
    pub total_data_tables: usize,
    /// 分析摘要（中文）
    pub summary: String,
}

/// 分析反汇编结果，提供 AI 感知的高层洞察
///
/// 此函数在反汇编完成后调用，对结果进行二次分析：
/// - 识别函数边界和调用关系（调用图）
/// - 识别代码模式（switch 语句、原子操作、临界区等）
/// - 生成统计摘要
///
/// 参考：
/// - Ghidra 的 FunctionAnalyzer 和 SwitchAnalyzer
/// - radare2 的 aaaa（自动分析）
/// - smda-rs 的函数发现和调用图构建
pub fn analyze_disassembly(instructions: &[DisasmInstruction], base_addr: u32, data: &[u8]) -> DisasmAnalysis {
    let mut functions: Vec<FunctionInfo> = Vec::new();
    let mut patterns: Vec<CodePattern> = Vec::new();
    let mut bl_targets: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();  // target -> [caller_addr]
    let mut b_self_addrs: Vec<u32> = Vec::new();  // B . 指令地址（待合并）
    let mut jump_table_count = 0;
    let mut literal_pool_count = 0;
    let mut data_table_count = 0;

    // 第一遍：收集 BL 调用目标、识别代码模式
    for inst in instructions {
        // 统计数据类型
        if inst.mnemonic == ".word" {
            if let Some(c) = &inst.comment {
                if c.contains("literal pool") { literal_pool_count += 1; }
                else if c.contains("data table") || c.contains("suspicious data") || c.contains("jump table") { data_table_count += 1; }
            }
        } else if inst.mnemonic == ".byte" {
            if let Some(c) = &inst.comment {
                if c.contains("jump table") { jump_table_count += 1; }
                else { data_table_count += 1; }
            }
        }

        // 收集 BL 调用目标
        if inst.mnemonic == "BL" {
            // 解析目标地址: "BL 0x080001E4 ; 函数调用"
            if let Some(target_str) = inst.operands.split_whitespace().next() {
                if let Ok(target) = u32::from_str_radix(target_str.trim_start_matches("0x"), 16) {
                    bl_targets.entry(target).or_default().push(inst.addr);
                }
            }
        }

        // 识别代码模式
        // 1. switch 语句（TBB/TBH）
        if inst.mnemonic == "TBB" || inst.mnemonic == "TBH" {
            patterns.push(CodePattern {
                kind: PatternKind::SwitchStatement,
                addr: inst.addr,
                description: format!("{} jump table (switch statement) at 0x{:08X}", inst.mnemonic, inst.addr),
            });
        }

        // 2. 原子操作（LDREX/STREX）
        if inst.mnemonic.starts_with("LDREX") {
            patterns.push(CodePattern {
                kind: PatternKind::AtomicOperation,
                addr: inst.addr,
                description: format!("Atomic load LDREX at 0x{:08X} (usually followed by STREX to implement atomic ops)", inst.addr),
            });
        }

        // 3. 临界区（CPS 指令）
        if inst.mnemonic == "CPS" {
            let is_enter = inst.operands.contains("i") && !inst.operands.contains("ie");
            patterns.push(CodePattern {
                kind: PatternKind::CriticalSection,
                addr: inst.addr,
                description: format!("{} at 0x{:08X}", if is_enter { "Enter critical section (interrupts off)" } else { "Exit critical section (interrupts on)" }, inst.addr),
            });
        }

        // 4. 字符串引用（LDR PC-relative 到 ASCII 数据）
        if inst.mnemonic == "LDR" && inst.comment.as_ref().map(|c| c.contains("→")).unwrap_or(false) {
            // 检查目标是否是 ASCII 字符串数据
            if let Some(c) = &inst.comment {
                if let Some(addr_str) = c.split("→").nth(1) {
                    if let Ok(target) = u32::from_str_radix(addr_str.trim().trim_start_matches("[").trim_end_matches("]"), 16) {
                        let target_off = (target as i64 - base_addr as i64) as usize;
                        if target_off < data.len() {
                            // 检查是否是 ASCII 字符串
                            if is_ascii_string_at(data, target_off) {
                                patterns.push(CodePattern {
                                    kind: PatternKind::StringReference,
                                    addr: inst.addr,
                                    description: format!("String reference at 0x{:08X} → 0x{:08X}", inst.addr, target),
                                });
                            }
                        }
                    }
                }
            }
        }

        // 5. 循环（B 到自身）：收集地址，稍后合并连续区域
        if inst.mnemonic == "B" {
            if let Some(target_str) = inst.operands.split_whitespace().next() {
                if let Ok(target) = u32::from_str_radix(target_str.trim_start_matches("0x"), 16) {
                    if target == inst.addr {
                        b_self_addrs.push(inst.addr);
                    }
                }
            }
        }

        // 6. 空闲等待（WFE/WFI）
        if inst.mnemonic == "WFE" || inst.mnemonic == "WFI" {
            patterns.push(CodePattern {
                kind: PatternKind::IdleWait,
                addr: inst.addr,
                description: format!("{} idle wait at 0x{:08X}", inst.mnemonic, inst.addr),
            });
        }
    }

    // 合并连续的 B . 指令（地址差=2）为单个 Loop 模式
    // 连续的 B . 通常是默认中断处理填充区域，不应每个都报告
    b_self_addrs.sort();
    let mut idx = 0;
    while idx < b_self_addrs.len() {
        let start = b_self_addrs[idx];
        let mut end = start;
        let mut count = 1;
        while idx + count < b_self_addrs.len() && b_self_addrs[idx + count] == end + 2 {
            end = b_self_addrs[idx + count];
            count += 1;
        }
        if count >= 2 {
            // 连续多个 B .：识别为默认中断处理填充区域
            patterns.push(CodePattern {
                kind: PatternKind::Loop,
                addr: start,
                description: format!(
                    "Default interrupt handler fill region 0x{:08X}-0x{:08X} ({} B . infinite loops)",
                    start, end, count
                ),
            });
        } else {
            // 单个 B .：可能是默认中断处理或错误陷阱
            patterns.push(CodePattern {
                kind: PatternKind::Loop,
                addr: start,
                description: format!("Infinite loop B . at 0x{:08X} (possibly default interrupt handler or fault trap)", start),
            });
        }
        idx += count;
    }

    // 第二遍：识别函数边界
    // 函数入口来源：BL 目标、向量表入口、PUSH {LR} 序言
    let mut func_entries: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for target in bl_targets.keys() {
        func_entries.insert(*target);
    }

    // 扫描 PUSH {LR} 和 PUSH.W {LR} 作为函数入口
    for inst in instructions {
        if inst.mnemonic == "PUSH" && inst.operands.contains("LR") {
            func_entries.insert(inst.addr);
        }
        if inst.mnemonic == "PUSH.W" && inst.operands.contains("LR") {
            func_entries.insert(inst.addr);
        }
    }

    // 从向量表提取入口（如果是 STM32 固件）
    if data.len() >= 0x40 {
        let vt_size = detect_vector_table_size(data, base_addr);
        for i in 0..vt_size / 4 {
            let vec_val = u32::from_le_bytes([data[i*4], data[i*4+1], data[i*4+2], data[i*4+3]]);
            if vec_val >= base_addr && vec_val < base_addr + data.len() as u32 {
                func_entries.insert(vec_val & !1);  // 清除 Thumb 位
            }
        }
    }

    // 构建函数信息
    let mut entry_list: Vec<u32> = func_entries.iter().copied().collect();
    entry_list.sort();

    for (idx, &entry) in entry_list.iter().enumerate() {
        // 查找函数结束（下一个函数入口前，或 BX LR / POP {PC}）
        let next_entry = if idx + 1 < entry_list.len() {
            Some(entry_list[idx + 1])
        } else {
            None
        };

        let mut end_addr: Option<u32> = None;
        let mut func_callees: Vec<u32> = Vec::new();

        for inst in instructions {
            if inst.addr < entry { continue; }
            if let Some(next) = next_entry {
                if inst.addr >= next { break; }
            }
            // 收集函数内的 BL 调用
            if inst.mnemonic == "BL" {
                if let Some(target_str) = inst.operands.split_whitespace().next() {
                    if let Ok(target) = u32::from_str_radix(target_str.trim_start_matches("0x"), 16) {
                        if !func_callees.contains(&target) {
                            func_callees.push(target);
                        }
                    }
                }
            }
            // 检测函数返回
            if end_addr.is_none() && (inst.mnemonic == "BX" && inst.operands.contains("LR") ||
               inst.mnemonic == "POP" && inst.operands.contains("PC") ||
               inst.mnemonic == "POP.W" && inst.operands.contains("PC")) {
                end_addr = Some(inst.addr + inst.bytes.len() as u32);
            }
        }

        let func_size = if let Some(end) = end_addr {
            (end - entry) as usize
        } else if let Some(next) = next_entry {
            (next - entry) as usize
        } else {
            0
        };

        // 推断函数名称
        let name = infer_function_name(entry, base_addr, data);

        // 判断是否为中断/异常处理函数（来自向量表的函数）
        // 包括 ARM Cortex-M 标准异常处理和外部中断
        let is_handler = is_exception_handler(&name);

        let callers = bl_targets.get(&entry).cloned().unwrap_or_default();

        functions.push(FunctionInfo {
            entry,
            name,
            end: end_addr,
            size: func_size,
            callers,
            callees: func_callees,
            is_handler,
        });
    }

    // 生成统计摘要
    let total_funcs = functions.len();
    let total_insts = instructions.len();
    let handler_count = functions.iter().filter(|f| f.is_handler).count();
    let switch_count = patterns.iter().filter(|p| p.kind == PatternKind::SwitchStatement).count();
    let atomic_count = patterns.iter().filter(|p| p.kind == PatternKind::AtomicOperation).count();
    let critical_count = patterns.iter().filter(|p| p.kind == PatternKind::CriticalSection).count();

    let summary = format!(
        "Analysis complete: {} instructions, {} functions ({} interrupt handlers), {} jump tables, {} literal pools, {} data tables. Code patterns: {} switch statements, {} atomic operations, {} critical sections.",
        total_insts, total_funcs, handler_count,
        jump_table_count, literal_pool_count, data_table_count,
        switch_count, atomic_count, critical_count
    );

    DisasmAnalysis {
        functions,
        patterns,
        total_instructions: total_insts,
        total_functions: total_funcs,
        total_jump_tables: jump_table_count,
        total_literal_pools: literal_pool_count,
        total_data_tables: data_table_count,
        summary,
    }
}

/// 判断函数名是否是异常/中断处理函数
fn is_exception_handler(name: &str) -> bool {
    // ARM Cortex-M 标准异常处理（与 vector_name 返回值对应）
    matches!(
        name,
        "Reset" | "NMI" | "HardFault" | "MemManage" | "BusFault"
        | "UsageFault" | "SVCall" | "Debug Monitor" | "PendSV" | "SysTick"
        | "IRQ (external interrupt)"
    )
}

/// 检查指定偏移处是否是 ASCII 字符串
fn is_ascii_string_at(data: &[u8], offset: usize) -> bool {
    if offset >= data.len() { return false; }
    let mut len = 0;
    for i in offset..data.len().min(offset + 64) {
        let b = data[i];
        if b == 0 && len >= 4 { return true; }  // NUL 终止，至少4字符
        if b < 0x20 || b > 0x7E { return false; }
        len += 1;
    }
    len >= 4
}

/// 推断函数名称
fn infer_function_name(entry: u32, base_addr: u32, data: &[u8]) -> String {
    // 从向量表查找函数名称
    if data.len() >= 0x40 {
        let vt_size = detect_vector_table_size(data, base_addr);
        for i in 0..vt_size / 4 {
            let vec_val = u32::from_le_bytes([data[i*4], data[i*4+1], data[i*4+2], data[i*4+3]]);
            if (vec_val & !1) == entry {
                return vector_name(i).to_string();
            }
        }
    }
    // 默认名称：func_<偏移>
    let offset = entry - base_addr;
    format!("func_{:X}", offset)
}

/// 反汇编 Thumb 指令
///
/// `data` 是原始 Flash 数据，`base_addr` 是 Flash 起始地址，`offset` 是开始反汇编的偏移量，`max_count` 是最大指令数。
///
/// 智能特性：
/// - **文字池检测**：跟踪 LDR [PC, #imm] 引用的地址，将文字池数据显示为 `.word` 而非错误解码为指令
/// - **死循环检测**：遇到无条件 B 到自身（默认中断处理）时停止反汇编
pub fn disassemble_thumb(data: &[u8], base_addr: u32, offset: usize, max_count: usize) -> Vec<DisasmInstruction> {
    disassemble_thumb_inner(data, base_addr, offset, max_count, true)
}

/// 全量反汇编（不在死循环处停止，用于完整反汇编导出）
pub fn disassemble_thumb_full(data: &[u8], base_addr: u32, offset: usize, max_count: usize) -> Vec<DisasmInstruction> {
    disassemble_thumb_inner(data, base_addr, offset, max_count, false)
}

fn disassemble_thumb_inner(data: &[u8], base_addr: u32, offset: usize, max_count: usize, stop_at_loop: bool) -> Vec<DisasmInstruction> {
    let mut result = Vec::with_capacity(max_count);
    let mut pos = offset;
    // 跟踪上一次是否是函数返回指令（BX LR / POP {PC}）
    // 用于函数边界检测：返回指令后的非跳转目标地址可能是数据表
    let mut after_return = false;

    // 文字池地址集合（LDR [PC, #imm] 引用的数据地址）
    let mut literal_pool: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // 跳转目标地址集合（B/BL/CBZ/CBNZ 的跳转目标，这些地址必须是代码）
    let mut jump_targets: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // 函数入口点集合（BL 调用目标 + 向量表入口 + push {lr} 序言）
    // 参考 argXtract 论文 Algorithm 4 和 smda-rs 的三阶段函数发现
    let mut function_starts: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // 跳转表数据地址集合（TBB/TBH 后跟的跳转表条目，这些地址是数据不是代码）
    // 参考 capstone 的 TBB/TBH 跳转表分析：递归下降反汇编器需要跟踪 TBB/TBH
    // 指令并标记其后的跳转表数据，否则跳转表数据会被误解码为指令
    let mut jump_table_data: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // 跳转表信息（用于 AI 感知分析）
    // 存储 (指令地址, 表起始地址, 表大小, 类型)
    let mut jump_tables: Vec<(u32, u32, usize, &str)> = Vec::new();
    // 预扫描：收集所有 LDR [PC, #imm] 目标地址作为文字池，以及所有跳转目标
    // 注意：必须跳过 Thumb-2 指令的第二个半字（hw2），否则会把 hw2 误认为独立的指令
    let mut scan_pos = offset;
    while scan_pos + 2 <= data.len() {
        let hw = u16::from_le_bytes([data[scan_pos], data[scan_pos + 1]]);
        // 检查是否为 Thumb-2 32位指令前缀 (0xE800-0xFFFF，即 bits[15:11] >= 11101)
        let is_thumb2_prefix = (hw >> 11) >= 0b11101 && (hw >> 11) <= 0b11111;

        // LDR literal: 0x4800-0x4FFF (bits [15:11] = 01001)
        if (hw >> 11) == 0b01001 {
            let imm8 = (hw & 0xFF) as u32;
            let target_addr = ((base_addr + scan_pos as u32 + 4) & !3u32) + imm8 * 4;
            literal_pool.insert(target_addr);
        }
        // LDR.W (literal, PC-relative): 0xF8DF (U=1) / 0xF85F (U=0) + hw2
        // 32位指令，需要4字节
        if scan_pos + 4 <= data.len() && (hw & 0xFF7F) == 0xF85F {
            let hw2 = u16::from_le_bytes([data[scan_pos + 2], data[scan_pos + 3]]);
            let imm12 = (hw2 & 0xFFF) as u32;
            let u = (hw >> 7) & 1;
            let pc_aligned = (base_addr + scan_pos as u32 + 4) & !3u32;
            let target_addr = if u == 1 {
                pc_aligned + imm12
            } else {
                pc_aligned.wrapping_sub(imm12)
            };
            literal_pool.insert(target_addr);
        }
        // 收集跳转目标地址（B/BL/CBZ/CBNZ）
        // Thumb-16 B (unconditional): 0xE000-0xE7FF
        if (hw >> 11) == 0b11100 {
            let offset = sign_extend11(hw & 0x7FF) * 2;
            let target = (base_addr + scan_pos as u32 + 4).wrapping_add(offset as u32);
            jump_targets.insert(target);
        }
        // Thumb-16 B<cond>: 0xD000-0xDFFF (排除 SVC 0xDFxx)
        if (hw >> 12) == 0b1101 && (hw >> 8) != 0xDF {
            let offset = sign_extend8(hw & 0xFF) * 2;
            let target = (base_addr + scan_pos as u32 + 4).wrapping_add(offset as u32);
            jump_targets.insert(target);
        }
        // Thumb-16 CBZ/CBNZ: 0xB100-0xB1FF (CBZ) / 0xB900-0xB9FF (CBNZ)
        if (hw & 0xF500) == 0xB100 {
            let i = (hw >> 9) & 1;
            let imm5 = (hw >> 3) & 0x1F;
            let offset = ((i as i32) << 6 | (imm5 as i32) << 1) as i32;
            let target = (base_addr + scan_pos as u32 + 4).wrapping_add(offset as u32);
            jump_targets.insert(target);
        }
        // Thumb-2 BL/BLX/B.W: 0xF000-0xF7FF prefix
        if scan_pos + 4 <= data.len() && is_thumb2_prefix {
            let hw2 = u16::from_le_bytes([data[scan_pos + 2], data[scan_pos + 3]]);
            // BL: hw1=0xF000-0xF7FF, hw2 bits[15]=1, bits[14]=1, bits[12]=1
            // 匹配 0xDxxx (J1=0) 和 0xFxxx (J1=1)
            // 注意：原代码 (hw2 >> 12) == 0b1101 只匹配 0xDxxx，漏掉了 0xFxxx
            if (hw2 & 0xD000) == 0xD000 {
                let s = (hw >> 10) & 1;
                let j1 = (hw2 >> 13) & 1;
                let j2 = (hw2 >> 11) & 1;
                let i1 = !(j1 ^ s) & 1;
                let i2 = !(j2 ^ s) & 1;
                let imm10 = (hw & 0x3FF) as u32;
                let imm11 = (hw2 & 0x7FF) as u32;
                let offset = ((s as i32) << 24)
                    | ((i1 as i32) << 23)
                    | ((i2 as i32) << 22)
                    | ((imm10 as i32) << 12)
                    | ((imm11 as i32) << 1);
                let offset = if s == 1 { offset | (-1i32 << 25) } else { offset };
                let target = (base_addr + scan_pos as u32 + 4).wrapping_add(offset as u32);
                jump_targets.insert(target);
                // BL 目标是函数入口点（参考 argXtract: bl 目标加入函数候选集）
                function_starts.insert(target);
            }
            // B.W (unconditional): hw2 bits[15]=1, bits[14]=0, bits[12]=1
            // 匹配 0x9xxx (J1=0) 和 0xBxxx (J1=1)
            // 注意：原代码 (hw2 >> 12) == 0b1000/0b1010 只匹配 0x8xxx/0xAxxx，漏掉了 0x9xxx/0xBxxx
            if (hw2 & 0xD000) == 0x9000 {
                let s = (hw >> 10) & 1;
                let j1 = (hw2 >> 13) & 1;
                let j2 = (hw2 >> 11) & 1;
                let i1 = !(j1 ^ s) & 1;
                let i2 = !(j2 ^ s) & 1;
                let imm10 = (hw & 0x3FF) as u32;
                let imm11 = (hw2 & 0x7FF) as u32;
                let offset = ((s as i32) << 24)
                    | ((i1 as i32) << 23)
                    | ((i2 as i32) << 22)
                    | ((imm10 as i32) << 12)
                    | ((imm11 as i32) << 1);
                let offset = if s == 1 { offset | (-1i32 << 25) } else { offset };
                let target = (base_addr + scan_pos as u32 + 4).wrapping_add(offset as u32);
                jump_targets.insert(target);
            }
            // B.W (conditional): hw2 bits[15]=1, bits[14]=0, bits[12]=0
            // 匹配 0x8xxx (J1=0) 和 0xAxxx (J1=1)
            if (hw2 & 0xD000) == 0x8000 {
                let s = (hw >> 10) & 1;
                let j1 = (hw2 >> 13) & 1;
                let j2 = (hw2 >> 11) & 1;
                let i1 = !(j1 ^ s) & 1;
                let i2 = !(j2 ^ s) & 1;
                let imm10 = (hw & 0x3FF) as u32;
                let imm6 = ((hw2 >> 6) & 0x3F) as u32;  // conditional B.W uses imm6
                let imm11 = (hw2 & 0x7FF) as u32;
                let offset = ((s as i32) << 24)
                    | ((i1 as i32) << 23)
                    | ((i2 as i32) << 22)
                    | ((imm10 as i32) << 12)
                    | ((imm6 as i32) << 6)
                    | (((imm11 & 0x3F) as i32) << 1);
                let offset = if s == 1 { offset | (-1i32 << 25) } else { offset };
                let target = (base_addr + scan_pos as u32 + 4).wrapping_add(offset as u32);
                jump_targets.insert(target);
            }
        }

        // 跳过 Thumb-2 指令的第二个半字（hw2），避免把 hw2 误认为独立的指令
        if is_thumb2_prefix && scan_pos + 4 <= data.len() {
            scan_pos += 4;
        } else {
            scan_pos += 2;
        }
    }

    // 从文字池中移除所有跳转目标地址（跳转目标必须是代码，不是数据）
    for target in &jump_targets {
        literal_pool.remove(target);
    }

    // 向量表检测：仅在从基地址开始反汇编时启用
    // STM32 向量表特征：连续的4字节地址，指向 Flash(0x0800xxxx)、SRAM(0x2000xxxx) 或为 0
    // 至少包含16个核心异常向量才认为是向量表
    let vector_table_size: usize = if offset == 0 && data.len() >= 0x40 {
        detect_vector_table_size(data, base_addr)
    } else {
        0
    };

    // 收集向量表中的入口点地址，加入跳转目标集合
    // 参考 rust-embedded/cortex-m-rt：向量表条目指向 Flash 的 Thumb 地址是函数入口
    let entry_points = collect_entry_points(data, vector_table_size, base_addr);
    for ep in &entry_points {
        jump_targets.insert(*ep);
        function_starts.insert(*ep);
        // 从文字池中移除入口点（入口点是代码，不是数据）
        literal_pool.remove(ep);
    }

    // 函数序言扫描：检测 push {..., lr} 模式作为函数入口
    // 参考 argXtract 论文：b 目标处的指令必须是 push/sub sp/stmdb 才认为是函数
    // 参考 smda-rs：序言扫描是函数发现的三阶段之一
    // Thumb-16 push {regs, LR}: 0xB500-0xBDFF (bit[10]=1 表示包含 LR)
    // Thumb-32 push.w {regs, LR}: 0xE92D xxxx (STMDB SP!, {regs})
    {
        let mut scan = offset;
        while scan + 2 <= data.len() {
            let hw = u16::from_le_bytes([data[scan], data[scan + 1]]);
            let scan_addr = base_addr + scan as u32;
            let is_thumb2 = (hw >> 11) >= 0b11101 && (hw >> 11) <= 0b11111;

            // Thumb-16 push {..., LR}: 0xB500-0xBDFF
            // 编码: 1011 0101 RRRR RRRR (bit[10]=1 表示 LR)
            if (hw & 0xFF00) == 0xB500 && (hw & 0x0100) != 0 {
                function_starts.insert(scan_addr);
            }
            // Thumb-32 STMDB SP!, {..., LR}: 0xE92D xxxx (push.w)
            // 其中 hw2 的 bit[14]=1 表示包含 LR
            if is_thumb2 && scan + 4 <= data.len() && hw == 0xE92D {
                let hw2 = u16::from_le_bytes([data[scan + 2], data[scan + 3]]);
                if hw2 & 0x4000 != 0 {  // bit[14] = LR
                    function_starts.insert(scan_addr);
                }
            }

            if is_thumb2 && scan + 4 <= data.len() {
                scan += 4;
            } else {
                scan += 2;
            }
        }
    }

    // 扩展文字池范围：对每个 LDR 引用的地址，向前扫描连续的数据字
    // 参考 compuphase/ARM-disassembler 的字面池范围检测算法：
    //   LDR 指令引用的文字池可能包含多个连续的数据字，
    //   向前扫描直到遇到跳转目标（代码）或看起来不像数据的字
    let mut extended_pool: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &lit_addr in literal_pool.iter() {
        if lit_addr < base_addr { continue; }
        let lit_offset = (lit_addr - base_addr) as usize;
        let mut scan = lit_offset + 4;  // 跳过已知的文字池条目
        let mut consecutive_data = 0;
        while scan + 4 <= data.len() {
            let scan_addr = base_addr + scan as u32;
            // 如果是跳转目标，停止（跳转目标是代码）
            if jump_targets.contains(&scan_addr) { break; }
            // 如果已经是文字池条目，继续扫描
            if literal_pool.contains(&scan_addr) { scan += 4; consecutive_data += 1; continue; }
            // 检查是否看起来像数据
            let word = u32::from_le_bytes([data[scan], data[scan+1], data[scan+2], data[scan+3]]);
            if is_likely_data_word(word, base_addr) {
                extended_pool.insert(scan_addr);
                scan += 4;
                consecutive_data += 1;
                // 最多向前扩展 16 个字（64 字节），避免过度扩展
                if consecutive_data >= 16 { break; }
            } else {
                break;
            }
        }
    }
    literal_pool.extend(extended_pool);

    // TBB/TBH 跳转表追踪（独立扫描，在 jump_targets 和 function_starts 完整填充后执行）
    // 参考 Ghidra 的 SwitchAnalyzer 和 capstone 的 TBB/TBH 处理：
    // 递归下降反汇编器需要跟踪 TBB/TBH 指令并标记其后的跳转表数据，
    // 否则跳转表数据会被误解码为指令（如 0xE8DF 0xF000 被误判为 LDRD R15）
    {
        let mut scan = offset;
        while scan + 4 <= data.len() {
            let hw1 = u16::from_le_bytes([data[scan], data[scan + 1]]);
            if hw1 != 0xE8DF {
                // 跳过 Thumb-2 指令的第二个半字
                let is_t2 = (hw1 >> 11) >= 0b11101 && (hw1 >> 11) <= 0b11111;
                scan += if is_t2 { 4 } else { 2 };
                continue;
            }
            let hw2 = u16::from_le_bytes([data[scan + 2], data[scan + 3]]);
            let rn = (hw2 & 0xF) as u8;
            let inst_addr = base_addr + scan as u32;
            let table_start = inst_addr + 4;  // PC+4

            if rn == 15 {  // PC-relative: 可以静态确定表位置
                if (hw2 & 0xFF00) == 0x0000 {
                    // TBB [PC, Rm]: 8位条目，每个条目1字节
                    let mut table_size = 0usize;
                    let mut tbl_pos = scan + 4;
                    while tbl_pos < data.len() && table_size < 256 {
                        let entry = data[tbl_pos];
                        let target = table_start.wrapping_add((entry as u32) * 2);
                        // 目标必须在 Flash 范围内
                        if target < base_addr || target >= base_addr + data.len() as u32 {
                            break;
                        }
                        let tbl_addr = base_addr + tbl_pos as u32;
                        // 遇到跳转目标或函数入口，表结束
                        if tbl_addr != table_start && jump_targets.contains(&tbl_addr) { break; }
                        if function_starts.contains(&tbl_addr) { break; }
                        // 检查是否遇到函数序言（PUSH {LR}）
                        let hw_scan = u16::from_le_bytes([data[tbl_pos], data[tbl_pos + 1]]);
                        if (hw_scan & 0xFF00) == 0xB500 && (hw_scan & 0x0100) != 0 { break; }
                        table_size += 1;
                        tbl_pos += 1;
                    }
                    for i in 0..table_size {
                        jump_table_data.insert(table_start + i as u32);
                    }
                    if table_size > 0 {
                        jump_tables.push((inst_addr, table_start, table_size, "TBB"));
                    }
                } else if (hw2 & 0xFF00) == 0x0100 {
                    // TBH [PC, Rm, LSL #1]: 16位条目，每个条目2字节
                    let mut table_size = 0usize;
                    let mut tbl_pos = scan + 4;
                    while tbl_pos + 2 <= data.len() && table_size < 128 {
                        let entry = u16::from_le_bytes([data[tbl_pos], data[tbl_pos + 1]]);
                        let target = table_start.wrapping_add((entry as u32) * 2);
                        if target < base_addr || target >= base_addr + data.len() as u32 {
                            break;
                        }
                        let tbl_addr = base_addr + tbl_pos as u32;
                        if tbl_addr != table_start && jump_targets.contains(&tbl_addr) { break; }
                        if function_starts.contains(&tbl_addr) { break; }
                        let hw_scan = u16::from_le_bytes([data[tbl_pos], data[tbl_pos + 1]]);
                        if (hw_scan & 0xFF00) == 0xB500 && (hw_scan & 0x0100) != 0 { break; }
                        table_size += 1;
                        tbl_pos += 2;
                    }
                    for i in 0..table_size * 2 {
                        jump_table_data.insert(table_start + i as u32);
                    }
                    if table_size > 0 {
                        jump_tables.push((inst_addr, table_start, table_size * 2, "TBH"));
                    }
                }
            }
            scan += 4;
        }
    }

    while pos + 2 <= data.len() && result.len() < max_count {
        let addr = base_addr + pos as u32;

        // 注意：函数入口标签由调用方（flasher.rs / disasm_bin.rs）负责输出，
        // thumb_disasm 作为底层反汇编器只负责指令解码，避免标签重复。
        // function_starts 集合仍然保留用于数据表检测等内部逻辑。

        // 函数入口点：重置 after_return 标志
        // 如果当前地址是函数入口（BL 目标、向量表入口、PUSH {LR} 序言），
        // 则不是函数返回后的数据区域
        if function_starts.contains(&addr) {
            after_return = false;
        }

        // 检查当前地址是否在跳转表数据区域内（TBB/TBH 后跟的跳转表条目）
        // 这些数据是跳转偏移量表，不是代码
        if jump_table_data.contains(&addr) {
            // 查找所属的跳转表信息
            let mut table_info: Option<(u32, u32, usize, &str)> = None;  // (inst_addr, tbl_start, tbl_size, kind)
            for &(inst_addr, tbl_start, tbl_size, kind) in &jump_tables {
                if addr >= tbl_start && addr < tbl_start + tbl_size as u32 {
                    table_info = Some((inst_addr, tbl_start, tbl_size, kind));
                    break;
                }
            }
            if let Some((inst_addr, tbl_start, tbl_size, kind)) = table_info {
                let tbl_offset = (addr - base_addr) as usize;
                let tbl_end = tbl_start + tbl_size as u32;
                let remaining = (tbl_end - addr) as usize;
                let bytes_to_show = remaining.min(data.len() - tbl_offset);
                // 输出跳转表数据块（一次性输出剩余的所有字节）
                let show_bytes = bytes_to_show.min(64);  // 限制单次输出最多64字节
                let mut bytes_vec = Vec::with_capacity(show_bytes);
                for i in 0..show_bytes {
                    if tbl_offset + i < data.len() {
                        bytes_vec.push(data[tbl_offset + i]);
                    }
                }
                let end_addr = addr + show_bytes as u32 - 1;
                result.push(DisasmInstruction {
                    addr,
                    bytes: bytes_vec,
                    mnemonic: ".byte".into(),
                    operands: format!("0x{:08X}-0x{:08X} ({} bytes)", addr, end_addr, show_bytes),
                    comment: Some(format!("; {} jump table data ({} bytes total, from 0x{:08X})", kind, tbl_size, inst_addr)),
                });
                pos += show_bytes;
                continue;
            }
        }

        // 检查当前地址是否在向量表区域内（数据，不是代码）
        if pos < vector_table_size && pos + 4 <= data.len() {
            let vec_idx = pos / 4;
            let vec_val = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
            let vec_name = vector_name(vec_idx);
            let comment = if vec_val == 0 {
                format!("; Vector #{}: {} (unused)", vec_idx, vec_name)
            } else {
                format!("; Vector #{}: {} → 0x{:08X}", vec_idx, vec_name, vec_val)
            };
            result.push(DisasmInstruction {
                addr,
                bytes: vec![data[pos], data[pos+1], data[pos+2], data[pos+3]],
                mnemonic: ".word".into(),
                operands: format!("0x{:08X}", vec_val),
                comment: Some(comment),
            });
            pos += 4;
            continue;
        }

        // 检查当前地址是否在文字池中（数据，不是代码）
        if literal_pool.contains(&addr) {
            // 显示为 .word 数据
            if pos + 4 <= data.len() {
                let word = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]);
                result.push(DisasmInstruction {
                    addr,
                    bytes: vec![data[pos], data[pos+1], data[pos+2], data[pos+3]],
                    mnemonic: ".word".into(),
                    operands: format!("0x{:08X}", word),
                    comment: Some("; literal pool data".into()),
                });
                pos += 4;
            } else {
                let hw = u16::from_le_bytes([data[pos], data[pos + 1]]);
                result.push(DisasmInstruction {
                    addr,
                    bytes: vec![data[pos], data[pos + 1]],
                    mnemonic: ".half".into(),
                    operands: format!("0x{:04X}", hw),
                    comment: Some("; literal pool data".into()),
                });
                pos += 2;
            }
            continue;
        }

        let hw = u16::from_le_bytes([data[pos], data[pos + 1]]);

        // 检测连续 0x0000 填充（4字节以上）：标记为填充数据而非 LSL R0, R0, #0
        if hw == 0 && pos + 4 <= data.len() {
            let hw2 = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            if hw2 == 0 {
                // 统计连续0x0000的字节数（hw和hw2已经是2个0x0000半字=4字节）
                let mut zero_bytes = 4;
                let mut scan = pos + 4;
                while scan + 2 <= data.len() {
                    let hw_next = u16::from_le_bytes([data[scan], data[scan + 1]]);
                    if hw_next == 0 {
                        zero_bytes += 2;
                        scan += 2;
                    } else {
                        break;
                    }
                }
                // 4字节以上0x0000视为填充
                if zero_bytes >= 8 {
                    // 8字节以上合并显示为 .fill
                    let end_addr = addr + zero_bytes as u32;
                    result.push(DisasmInstruction {
                        addr,
                        bytes: vec![0; 4],
                        mnemonic: ".fill".into(),
                        operands: format!("0x{:08X}-0x{:08X} ({} bytes)", addr, end_addr, zero_bytes),
                        comment: Some("; 0x00 fill/alignment data".into()),
                    });
                    pos += zero_bytes;
                    continue;
                } else {
                    // 4字节显示为 .word
                    result.push(DisasmInstruction {
                        addr,
                        bytes: vec![0, 0, 0, 0],
                        mnemonic: ".word".into(),
                        operands: "0x00000000".into(),
                        comment: Some("; fill/alignment data".into()),
                    });
                    pos += 4;
                    continue;
                }
            }
        }

        // 检测单条 0x0000 对齐填充（函数返回后或文字池前的2字节填充）
        // 0x0000 是 LSL R0, R0, #0 的编码，但以下场景中通常是对齐填充：
        // 1. 函数返回后（after_return=true）
        // 2. 下一个地址是文字池条目（0x0000 是文字池前的对齐填充）
        if hw == 0 && (after_return || (pos + 4 <= data.len() && literal_pool.contains(&(addr + 2)))) {
            result.push(DisasmInstruction {
                addr,
                bytes: vec![0, 0],
                mnemonic: ".half".into(),
                operands: "0x0000".into(),
                comment: Some("; alignment fill".into()),
            });
            pos += 2;
            continue;
        }

        // 检测连续 0xFFFF 填充（Flash 未编程区域，STM32 擦除后默认值）
        // 0xFFFF 在 Thumb-16 中是 B <self>（死循环），但实际是未编程 Flash
        if hw == 0xFFFF && pos + 4 <= data.len() {
            let hw2 = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            if hw2 == 0xFFFF {
                // 统计连续 0xFFFF 的字节数
                let mut ff_bytes = 4;
                let mut scan = pos + 4;
                while scan + 2 <= data.len() {
                    let hw_next = u16::from_le_bytes([data[scan], data[scan + 1]]);
                    if hw_next == 0xFFFF {
                        ff_bytes += 2;
                        scan += 2;
                    } else {
                        break;
                    }
                }
                // 8字节以上 0xFFFF 视为未编程区域，合并显示
                if ff_bytes >= 8 {
                    let end_addr = addr + ff_bytes as u32;
                    result.push(DisasmInstruction {
                        addr,
                        bytes: vec![0xFF; 4],  // 只显示前4字节作为示例
                        mnemonic: ".fill".into(),
                        operands: format!("0x{:08X}-0x{:08X} ({} bytes)", addr, end_addr, ff_bytes),
                        comment: Some("; Flash unprogrammed region (0xFF fill)".into()),
                    });
                    pos += ff_bytes;
                    continue;
                }
            }
        }

        // ASCII 字符串检测（参考 compuphase/ARM-disassembler 和 binwalk）
        // 如果当前地址不是跳转目标/入口点，且字节序列是可打印 ASCII 字符串，
        // 则显示为字符串而非解码为指令
        // 典型场景：AT 命令、日志字符串、配置参数等存储在 Flash 中的字符串
        if !jump_targets.contains(&addr) {
            if let Some((len, s)) = detect_ascii_string(data, pos) {
                let bytes: Vec<u8> = data[pos..pos + len].to_vec();
                let display_str = s.replace('\\', "\\\\").replace('"', "\\\"");
                result.push(DisasmInstruction {
                    addr,
                    bytes,
                    mnemonic: ".ascii".into(),
                    operands: format!("\"{}\"", display_str),
                    comment: Some(format!("; ASCII string ({} bytes, including NUL)", len)),
                });
                pos += len;
                // 对齐到 2 字节边界（Thumb 指令是 2 字节对齐的）
                if pos % 2 != 0 { pos += 1; }
                // 检测字符串后的 0x00 对齐填充（2字节）
                if pos + 2 <= data.len() {
                    let pad_hw = u16::from_le_bytes([data[pos], data[pos + 1]]);
                    if pad_hw == 0 {
                        let pad_addr = base_addr + pos as u32;
                        result.push(DisasmInstruction {
                            addr: pad_addr,
                            bytes: vec![0, 0],
                            mnemonic: ".half".into(),
                            operands: "0x0000".into(),
                            comment: Some("; alignment fill".into()),
                        });
                        pos += 2;
                    }
                }
                continue;
            }
        }

        // 数据表检测：如果当前地址不是跳转目标，且连续 8 字节都看起来像数据，
        // 则显示为数据表。参考 compuphase/ARM-disassembler 的 "code pool mapping" 技术
        //
        // 关键改进：
        // 1. 优先使用 4 字节（32位）检测，识别 Flash/SRAM/外设地址指针表
        // 2. 退回到 2 字节（16位）检测，识别小常数表
        // 3. 数据表扩展限制增加到 1024 字节（原 128 字节太小）
        // 4. 函数边界检测：在 BX LR / POP {PC} 后，降低检测阈值
        // 5. 激进模式：在 BX LR 后，如果下一个地址不是跳转目标且不是函数入口，
        //    直接识别为数据表（不要求检测阈值），限制最多 64 字节
        if !jump_targets.contains(&addr) && !function_starts.contains(&addr) {
            // 策略 0：激进模式 - 函数返回后的非跳转目标地址直接识别为数据表
            // 这可以捕获数据表起始处的字节（看起来像有效指令但实际是数据）
            if after_return && pos + 4 <= data.len() {
                // 向前扫描，直到遇到跳转目标、函数入口或看起来像代码的字节
                let mut data_len = 0;
                let mut scan = pos;
                while scan + 2 <= data.len() && data_len < 64 {
                    let scan_addr = base_addr + scan as u32;
                    if jump_targets.contains(&scan_addr) { break; }
                    if function_starts.contains(&scan_addr) { break; }
                    // 检查是否是有效的函数序言（PUSH {LR} 等）
                    let hw_scan = u16::from_le_bytes([data[scan], data[scan + 1]]);
                    if (hw_scan & 0xFF00) == 0xB500 && (hw_scan & 0x0100) != 0 {
                        break;  // PUSH {LR} 是函数入口
                    }
                    if hw_scan == 0xE92D && scan + 4 <= data.len() {
                        let hw2_scan = u16::from_le_bytes([data[scan + 2], data[scan + 3]]);
                        if hw2_scan & 0x4000 != 0 {
                            break;  // PUSH.W {LR} 是函数入口
                        }
                    }
                    data_len += 2;
                    scan += 2;
                }
                if data_len >= 4 {
                    let end_addr = addr + data_len as u32;
                    result.push(DisasmInstruction {
                        addr,
                        bytes: vec![data[pos], data[pos+1], data[pos+2], data[pos+3]],
                        mnemonic: ".byte".into(),
                        operands: format!("0x{:08X}-0x{:08X} ({} bytes)", addr, end_addr, data_len),
                        comment: Some("; data table (after function return)".into()),
                    });
                    after_return = false;
                    pos += data_len;
                    continue;
                }
            }

            if pos + 8 <= data.len() {
            // 策略 1：检测 2 个连续的字（8 字节），使用 4 字节检测
            // 适用于 Flash 地址指针表、SRAM 地址指针表等
            let mut data_like_words = 0;
            for i in 0..2 {
                if pos + (i + 1) * 4 <= data.len() {
                    let word = u32::from_le_bytes([
                        data[pos + i * 4],
                        data[pos + i * 4 + 1],
                        data[pos + i * 4 + 2],
                        data[pos + i * 4 + 3],
                    ]);
                    if is_likely_data_word(word, base_addr) {
                        data_like_words += 1;
                    } else {
                        break;
                    }
                }
            }

            // 策略 2：如果 4 字节检测失败，退回到 2 字节检测
            // 适用于小常数表（连续多个 < 0x1000 的半字）
            let mut data_like_halfwords = 0;
            if data_like_words < 2 {
                for i in 0..4 {
                    let hw_check = u16::from_le_bytes([data[pos + i * 2], data[pos + i * 2 + 1]]);
                    if is_likely_data_halfword(hw_check, base_addr) {
                        data_like_halfwords += 1;
                    } else {
                        break;
                    }
                }
            }

            // 检测阈值：
            // - 正常情况：4 字节检测 >= 2 或 2 字节检测 >= 4
            // - 函数返回后：4 字节检测 >= 1 或 2 字节检测 >= 2（降低阈值）
            //   因为函数返回后的非跳转目标地址很可能是数据表
            let threshold_words = if after_return { 1 } else { 2 };
            let threshold_halfwords = if after_return { 2 } else { 4 };

            // 两种策略任一通过即认为是数据表
            if data_like_words >= threshold_words || data_like_halfwords >= threshold_halfwords {
                // 向前扫描连续的数据，扩展数据表范围
                let mut data_len = 8;  // 已经检查了 8 字节
                let mut scan = pos + 8;
                while scan + 4 <= data.len() && data_len < 1024 {
                    let scan_addr = base_addr + scan as u32;
                    if jump_targets.contains(&scan_addr) { break; }
                    // 优先使用 4 字节检测
                    let word_next = u32::from_le_bytes([
                        data[scan],
                        data[scan + 1],
                        data[scan + 2],
                        data[scan + 3],
                    ]);
                    if is_likely_data_word(word_next, base_addr) {
                        data_len += 4;
                        scan += 4;
                        continue;
                    }
                    // 退回到 2 字节检测
                    if scan + 2 <= data.len() {
                        let hw_next = u16::from_le_bytes([data[scan], data[scan + 1]]);
                        if is_likely_data_halfword(hw_next, base_addr) {
                            data_len += 2;
                            scan += 2;
                            continue;
                        }
                    }
                    break;
                }
                // 对齐到 2 字节边界（Thumb 指令是 2 字节对齐的）
                // 注意：不强制对齐到 4 字节边界，避免包含非数据字节（如 ASCII 字符串起始）
                if data_len % 2 != 0 { data_len += 1; }
                let end_addr = addr + data_len as u32;
                result.push(DisasmInstruction {
                    addr,
                    bytes: vec![data[pos], data[pos+1], data[pos+2], data[pos+3]],
                    mnemonic: ".byte".into(),
                    operands: format!("0x{:08X}-0x{:08X} ({} bytes)", addr, end_addr, data_len),
                    comment: Some("; data table (non-code region)".into()),
                });
                after_return = false;  // 数据表不是函数返回，重置标志
                pos += data_len;
                continue;
            }
            }  // end if pos + 8 <= data.len()
        }

        // 检查是否为 Thumb-2 32 位指令 (0xF000-0xF7FF 前缀)
        if (hw >> 11) >= 0b11101 && (hw >> 11) <= 0b11111 {
            if pos + 4 <= data.len() {
                let hw2 = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
                let inst32 = decode_thumb32(hw, hw2, addr);
                // 检测 POP.W {PC} (0xE8BD xxxx, hw2 bit[15]=1 表示 PC)
                // 这是函数返回指令，下一个非跳转目标地址可能是数据表
                after_return = (hw == 0xE8BD) && (hw2 & 0x8000 != 0);
                result.push(inst32);
                pos += 4;
                continue;
            }
        }

        let inst = decode_thumb16(hw, addr);
        // 检测 BX LR (0x4770) 和 POP {PC} (0xBD00-0xBDFF, bit[8]=1)
        after_return = hw == 0x4770 || ((hw & 0xFF00) == 0xBD00 && (hw & 0x0100) != 0);
        result.push(inst);

        // 死循环检测：无条件 B 到自身（默认中断处理）
        // 0xE7FE = B self (offset = -2, target = addr)
        if hw == 0xE7FE {
            // 死循环不是函数返回，重置标志
            after_return = false;
            if stop_at_loop {
                result.push(DisasmInstruction {
                    addr: addr + 2,
                    bytes: vec![],
                    mnemonic: "; -- infinite loop; following is data/other functions --".into(),
                    operands: "".into(),
                    comment: None,
                });
                break;
            }
            // 全量模式：继续反汇编（死循环后可能是其他函数）
        }

        pos += 2;
    }

    result
}

/// 解码 Thumb-16 指令
fn decode_thumb16(hw: u16, addr: u32) -> DisasmInstruction {
    let bytes = vec![(hw & 0xFF) as u8, (hw >> 8) as u8];

    // NOP
    if hw == 0xBF00 {
        return DisasmInstruction { addr, bytes, mnemonic: "NOP".into(), operands: "".into(), comment: None };
    }

    // 按指令格式分类解码
    let (mnemonic, operands, comment) = match hw >> 12 {
        // 0000-0011: 移位/算术立即数
        0x0 => decode_shift_imm(hw),
        0x1 => decode_add_sub_imm(hw),
        // 0010-0011: MOV/CMP/ADD/SUB 立即数
        // 0x2000-0x27FF: MOV Rdn, #imm8 (bit11=0)
        // 0x2800-0x2FFF: CMP Rn, #imm8  (bit11=1)
        // 0x3000-0x37FF: ADD Rdn, #imm8 (bit11=0)
        // 0x3800-0x3FFF: SUB Rdn, #imm8 (bit11=1)
        0x2 => decode_mov_cmp_imm(hw, if (hw >> 11) & 1 == 1 { "CMP" } else { "MOV" }),
        0x3 => decode_mov_cmp_imm(hw, if (hw >> 11) & 1 == 1 { "SUB" } else { "ADD" }),
        // 0100: ALU / 高寄存器操作 / LDR 文字池
        0x4 => {
            let sub = (hw >> 10) & 0x3;
            match sub {
                0 => decode_alu(hw),                    // 0x4000-0x43FF: ALU 操作
                1 => decode_high_reg(hw, addr),         // 0x4400-0x47FF: 高寄存器 MOV/CMP/ADD/BX/BLX
                _ => {                                   // 0x4800-0x4FFF: LDR (PC-relative, literal pool)
                    let rd = (hw >> 8) & 7;
                    let imm8 = (hw & 0xFF) as u32;
                    let target = ((addr + 4) & !3) + imm8 * 4;
                    ("LDR".into(), format!("R{}, [PC, #{}]", rd, imm8 * 4), Some(format!("; → [0x{:08X}]", target)))
                }
            }
        }
        // 0101: LDR/STR (register offset) / LDRB / STRB / LDRH / STRH
        0x5 => decode_load_store_reg(hw),
        // 0110-0111: LDR/STR (immediate offset)
        0x6 => decode_ldr_str_imm(hw, false),
        0x7 => decode_ldr_str_imm(hw, true),
        // 1000: LDR/STR halfword
        0x8 => decode_ldr_str_hw_imm(hw),
        // 1001: STR/LDR (SP-relative)
        0x9 => {
            let is_load = (hw >> 11) & 1 == 1;
            let rd = (hw >> 8) & 7;
            let imm8 = (hw & 0xFF) as u32 * 4;
            let mnem = if is_load { "LDR" } else { "STR" };
            (mnem.into(), format!("R{}, [SP, #{}]", rd, imm8), None)
        }
        // 1010: ADR (ADD PC) / ADD SP
        0xA => {
            let rd = (hw >> 8) & 7;
            let imm8 = (hw & 0xFF) as u32 * 4;
            if (hw >> 11) & 1 == 0 {
                let target = ((addr + 4) & !3u32) + imm8;
                (format!("ADR"), format!("R{}, [PC, #{}]", rd, imm8), Some(format!("; → 0x{:08X}", target)))
            } else {
                (format!("ADD"), format!("R{}, SP, #{}", rd, imm8), None)
            }
        }
        // 1011: 特殊指令 (SP 调整 / PUSH / POP / BX / CBZ / CBNZ)
        0xB => decode_special(hw, addr),
        // 1100: STM/LDM (multiple load/store)
        0xC => {
            let is_load = (hw >> 11) & 1 == 1;
            let rd = (hw >> 8) & 7;
            let reg_list = hw & 0xFF;
            let mnem = if is_load { "LDM" } else { "STM" };
            (mnem.into(), format!("R{}!, {{{}}}", rd, format_reg_list(reg_list, false)), None)
        }
        // 1101: B (conditional) / SVC
        0xD => {
            if (hw >> 8) == 0xDF {
                (format!("SVC"), format!("#{}", hw & 0xFF), None)
            } else {
                let cond = (hw >> 8) & 0xF;
                let offset = sign_extend8(hw & 0xFF) * 2;
                let target = (addr as i32 + 4 + offset) as u32;
                let cond_str = cond_name(cond);
                (format!("B{}", cond_str), format!("0x{:08X}", target), None)
            }
        }
        // 1110: B (unconditional)
        0xE => {
            let offset = sign_extend11(hw & 0x7FF) * 2;
            let target = (addr as i32 + 4 + offset) as u32;
            (format!("B"), format!("0x{:08X}", target), None)
        }
        // 1111: BL/BLX prefix (Thumb-2) — 单独处理
        0xF => (".word".into(), format!("0x{:04X}", hw), Some("; Thumb-2 prefix".into())),
        _ => (".word".into(), format!("0x{:04X}", hw), None),
    };

    DisasmInstruction { addr, bytes, mnemonic, operands, comment }
}

// 辅助函数

/// 检测向量表大小（字节数）
/// 启发式：从基地址开始扫描4字节条目，符合 Flash/SRAM/0 地址特征的连续条目数
/// 至少16个核心异常向量才认为是向量表
fn detect_vector_table_size(data: &[u8], base_addr: u32) -> usize {
    let mut vec_count = 0usize;
    let mut i = 0usize;
    // 最多扫描256个向量（1KB），覆盖大多数STM32型号
    while i + 4 <= data.len() && i < 0x400 {
        let v = u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]]);
        if is_valid_vector(v, base_addr) {
            vec_count += 1;
            i += 4;
        } else {
            break;
        }
    }
    // 至少16个核心异常向量才认为是向量表
    if vec_count >= 16 { vec_count * 4 } else { 0 }
}

/// 判断是否为有效的向量表条目
fn is_valid_vector(v: u32, base_addr: u32) -> bool {
    if v == 0 { return true; }  // 未使用
    // Flash地址范围（base_addr ~ base_addr + 0x100000，bit0=1表示Thumb模式）
    if v >= base_addr && v < base_addr + 0x100000 && (v & 1) == 1 { return true; }
    // SRAM地址范围 0x20000000-0x2000FFFF（初始SP等）
    if v >= 0x20000000 && v < 0x20010000 { return true; }
    false
}

/// 获取向量名称（Cortex-M 核心异常 + 通用IRQ）
fn vector_name(idx: usize) -> &'static str {
    match idx {
        0 => "Initial SP",
        1 => "Reset",
        2 => "NMI",
        3 => "HardFault",
        4 => "MemManage",
        5 => "BusFault",
        6 => "UsageFault",
        7 | 8 | 9 | 10 => "Reserved",
        11 => "SVCall",
        12 => "Debug Monitor",
        13 => "Reserved",
        14 => "PendSV",
        15 => "SysTick",
        _ => "IRQ (external interrupt)",
    }
}

/// ThumbExpandImm: 将 12 位 modified immediate 编码解码为 32 位立即数
///
/// 编码格式: imm12 = (rotation/2) << 8 | imm8
/// - 若 imm8 == 0: 根据 imm12<11:8> 重复填充字节模式
/// - 若 imm8 != 0: imm8 循环右移 (rotation/2 * 2) 位
fn thumb_expand_imm(imm12: u32) -> u32 {
    let imm8 = imm12 & 0xFF;
    let rot = (imm12 >> 8) & 0xF;  // rotation/2 (0-15)
    if imm8 == 0 {
        // imm8 == 0: 重复填充模式
        match rot {
            0 => 0,
            1 => 0x000000FF,
            2 => 0x0000FFFF,
            3 => 0x00FFFFFF,
            4 => 0xFFFFFFFF,
            8 => 0x00FF00FF,
            9 => 0xFF00FF00,
            _ => 0,  // reserved / 未定义，返回 0
        }
    } else {
        // imm8 != 0: 循环右移 (rot * 2) 位
        let shift = (rot * 2) as u32;
        if shift == 0 {
            imm8
        } else {
            // ROR: (imm8 >> shift) | (imm8 << (32 - shift))
            (imm8 >> shift) | (imm8 << (32 - shift))
        }
    }
}

fn sign_extend8(v: u16) -> i32 {
    let v = v as i8;
    v as i32
}

fn sign_extend11(v: u16) -> i32 {
    let v = v as i32;
    if v & 0x400 != 0 { v | (-2048i32) } else { v }
}

/// 检测 ASCII 字符串（参考 compuphase/ARM-disassembler 和 binwalk 的字符串检测思路）
///
/// 在固件中，AT 命令、日志字符串等常以 ASCII 形式存储在 Flash 中。
/// 这些字节序列如果被当作 Thumb 指令解码，会产生无意义的 LSL/LSR/STRB 等指令。
///
/// 检测条件：
/// - 至少 8 个连续可打印 ASCII 字符 (0x20-0x7E)，避免误识别短指令为字符串
/// - 以 NUL (0x00) 结尾或遇到非可打印字节
/// - 当前地址不能是跳转目标（跳转目标是代码）
fn detect_ascii_string(data: &[u8], pos: usize) -> Option<(usize, String)> {
    if pos + 8 > data.len() { return None; }
    let mut end = pos;
    while end < data.len() {
        let b = data[end];
        if b == 0 { break; }  // NUL 终止符
        if b >= 0x20 && b <= 0x7E {
            end += 1;
        } else {
            return None;  // 非可打印字节，不是字符串
        }
    }
    let len = end - pos;
    // 至少 8 个可打印字符才算字符串（避免误识别 Thumb-16 指令对为字符串）
    // 例如 0x4621 0x4628 = "!" 'F' '(' 'F' 是两条 MOV 指令，不是字符串
    if len < 8 { return None; }
    // 包含 NUL 终止符（如果有）
    let total_len = if end < data.len() && data[end] == 0 {
        len + 1
    } else {
        len
    };
    let s = String::from_utf8_lossy(&data[pos..end]).to_string();
    Some((total_len, s))
}

/// 判断一个 32 位字是否看起来像数据而非代码
/// 参考 compuphase/ARM-disassembler 的字面池范围检测启发式
///
/// 数据特征：
/// - Flash 地址指针 (0x0800xxxx，含函数入口 bit0=1 和数据指针 bit0=0)
/// - SRAM 地址指针 (0x2000xxxx)
/// - 外设地址指针 (0x4000xxxx)
/// - 小常数 (< 0x10000)
fn is_likely_data_word(word: u32, base_addr: u32) -> bool {
    // 全零（空指针、零常数）
    if word == 0 { return true; }
    // 注意：不把 0xFFFFFFFF 当作数据，它是 Flash 未编程区域的填充
    // 由 0xFF 填充检测逻辑统一处理
    // Flash 地址指针 (base_addr ~ base_addr + 0x100000)
    // 既包含函数入口（bit0=1，Thumb 模式），也包含数据指针（bit0=0，如字符串指针）
    if word >= base_addr && word < base_addr + 0x100000 { return true; }
    // SRAM 地址指针 (0x20000000-0x2000FFFF)
    if word >= 0x20000000 && word < 0x20010000 { return true; }
    // 外设地址指针 (0x40000000-0x4007FFFF)
    if word >= 0x40000000 && word < 0x40080000 { return true; }
    // 小常数 (0-0xFFFF)
    if word < 0x10000 { return true; }
    false
}

/// 判断单个半字是否看起来像数据而非代码
/// 参考 compuphase/ARM-disassembler 的 "code pool mapping" 技术
///
/// 检测策略：
/// - 半字很小 (< 0x1000)：可能是数据（0x0000-0x0FFF 是移位立即数指令范围，
///   但连续多个移位指令而无其他类型极可能是数据）
/// - 半字为 0：明确的填充/数据
fn is_likely_data_halfword(hw: u16, _base_addr: u32) -> bool {
    // 半字为 0：填充数据
    if hw == 0 { return true; }
    // 半字很小 (< 0x1000)：可能是数据表中的小值
    // 注意：0x0000-0x0FFF 也是 Thumb-16 移位立即数指令范围，
    // 但要求连续 4 个才触发数据表检测，降低误识别风险
    if hw < 0x1000 { return true; }
    false
}

/// 收集向量表中的入口点地址（指向 Flash 的 Thumb 地址）
/// 参考 rust-embedded/cortex-m-rt 的向量表布局
fn collect_entry_points(data: &[u8], vector_table_size: usize, base_addr: u32) -> std::collections::HashSet<u32> {
    let mut entries = std::collections::HashSet::new();
    if vector_table_size == 0 { return entries; }
    let count = vector_table_size / 4;
    for i in 0..count {
        let offset = i * 4;
        if offset + 4 > data.len() { break; }
        let v = u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]);
        // 跳过第 0 项（初始 SP，不是代码地址）和 0 值
        if i == 0 || v == 0 { continue; }
        // Thumb 地址：bit0=1，指向 Flash
        if v >= base_addr && v < base_addr + 0x100000 && (v & 1) == 1 {
            entries.insert(v & !1u32);  // 清除 Thumb 位，得到实际地址
        }
    }
    entries
}

fn cond_name(cond: u16) -> &'static str {
    match cond {
        0x0 => "EQ", 0x1 => "NE", 0x2 => "CS", 0x3 => "CC",
        0x4 => "MI", 0x5 => "PL", 0x6 => "VS", 0x7 => "VC",
        0x8 => "HI", 0x9 => "LS", 0xA => "GE", 0xB => "LT",
        0xC => "GT", 0xD => "LE", 0xE => "",   0xF => "??",
        _ => "??",
    }
}

fn reg_name(r: u16) -> String {
    match r {
        13 => "SP".into(), 14 => "LR".into(), 15 => "PC".into(),
        _ => format!("R{}", r),
    }
}

fn format_reg_list(list: u16, include_lr_pc: bool) -> String {
    let mut regs = Vec::new();
    for i in 0..8 {
        if list & (1 << i) != 0 { regs.push(format!("R{}", i)); }
    }
    if include_lr_pc && list & 0x100 != 0 { regs.push("LR".into()); }
    if include_lr_pc && list & 0x200 != 0 { regs.push("PC".into()); }
    regs.join(", ")
}

fn decode_shift_imm(hw: u16) -> (String, String, Option<String>) {
    let op = (hw >> 11) & 3;
    let imm5 = (hw >> 6) & 0x1F;
    let rm = (hw >> 3) & 7;
    let rd = hw & 7;
    let mnem = match op { 0 => "LSL", 1 => "LSR", 2 => "ASR", _ => "ROR" };
    (mnem.into(), format!("R{}, R{}, #{}", rd, rm, imm5), None)
}

fn decode_add_sub_imm(hw: u16) -> (String, String, Option<String>) {
    let op = (hw >> 9) & 3;
    let imm3 = (hw >> 6) & 7;
    let rn = (hw >> 3) & 7;
    let rd = hw & 7;
    let mnem = match op { 0 => "ADD", 1 => "SUB", 2 => "ADD", _ => "SUB" };
    let src = if op >= 2 { format!("#{}", imm3) } else { format!("R{}", imm3) };
    (mnem.into(), format!("R{}, R{}, {}", rd, rn, src), None)
}

fn decode_mov_cmp_imm(hw: u16, mnem: &str) -> (String, String, Option<String>) {
    let rd = (hw >> 8) & 7;
    let imm8 = hw & 0xFF;
    (mnem.into(), format!("R{}, #{}", rd, imm8), None)
}

/// 高寄存器操作 (0x4400-0x47FF): ADD/CMP/MOV/BX/BLX
fn decode_high_reg(hw: u16, _addr: u32) -> (String, String, Option<String>) {
    let op = (hw >> 8) & 0x3;
    let d = (hw >> 7) & 1;      // Rd/Rn 高位
    let rm = (hw >> 3) & 0xF;   // 4-bit Rm
    let rdn = hw & 0x7;         // Rd/Rn 低 3 位
    let rd = (d << 3) | rdn;    // 完整 4-bit Rd

    match op {
        0 => ("ADD".into(), format!("{}, {}", reg_name(rd), reg_name(rm)), None),
        1 => ("CMP".into(), format!("{}, {}", reg_name(rd), reg_name(rm)), None),
        2 => ("MOV".into(), format!("{}, {}", reg_name(rd), reg_name(rm)), None),
        _ => {
            // BX/BLX (op=3)
            if d == 0 {
                // BX Rm
                if rm == 14 {
                    ("BX".into(), "LR".into(), Some("; function return".into()))
                } else {
                    ("BX".into(), reg_name(rm), None)
                }
            } else {
                // BLX Rm
                ("BLX".into(), reg_name(rm), Some("; indirect call".into()))
            }
        }
    }
}

fn decode_alu(hw: u16) -> (String, String, Option<String>) {
    let sub_op = (hw >> 6) & 0xF;
    let rm = (hw >> 3) & 7;
    let rd = hw & 7;

    match sub_op {
        0x0 => ("AND".into(), format!("R{}, R{}", rd, rm), None),
        0x1 => ("EOR".into(), format!("R{}, R{}", rd, rm), None),
        0x2 => ("LSL".into(), format!("R{}, R{}", rd, rm), None),
        0x3 => ("LSR".into(), format!("R{}, R{}", rd, rm), None),
        0x4 => ("ASR".into(), format!("R{}, R{}", rd, rm), None),
        0x5 => ("ADC".into(), format!("R{}, R{}", rd, rm), None),
        0x6 => ("SBC".into(), format!("R{}, R{}", rd, rm), None),
        0x7 => ("ROR".into(), format!("R{}, R{}", rd, rm), None),
        0x8 => ("TST".into(), format!("R{}, R{}", rd, rm), None),
        0x9 => ("RSB".into(), format!("R{}, R{}, #0", rd, rm), None),
        0xA => ("CMP".into(), format!("R{}, R{}", rd, rm), None),
        0xB => ("CMN".into(), format!("R{}, R{}", rd, rm), None),
        0xC => ("ORR".into(), format!("R{}, R{}", rd, rm), None),
        0xD => ("MUL".into(), format!("R{}, R{}", rd, rm), None),
        0xE => ("BIC".into(), format!("R{}, R{}", rd, rm), None),
        0xF => ("MVN".into(), format!("R{}, R{}", rd, rm), None),
        _ => (".word".into(), format!("0x{:04X}", hw), None),
    }
}

fn decode_load_store_reg(hw: u16) -> (String, String, Option<String>) {
    let op = (hw >> 9) & 7;
    let rm = (hw >> 6) & 7;
    let rn = (hw >> 3) & 7;
    let rd = hw & 7;
    let (mnem, suffix) = match op {
        0 => ("STR", ""), 1 => ("STRH", ""), 2 => ("STRB", ""),
        3 => ("LDRSB", "S"), 4 => ("LDR", ""), 5 => ("LDRH", ""),
        6 => ("LDRB", ""), 7 => ("LDRSH", "S"),
        _ => ("?", "?"),
    };
    (mnem.into(), format!("R{}, [R{}, R{}{}]", rd, rn, rm, suffix), None)
}

fn decode_ldr_str_imm(hw: u16, is_byte: bool) -> (String, String, Option<String>) {
    let is_load = (hw >> 11) & 1 == 1;
    let offset = ((hw >> 6) & 0x1F) as u32 * if is_byte { 1 } else { 4 };
    let rn = (hw >> 3) & 7;
    let rd = hw & 7;
    let mnem = if is_load { "LDR" } else { "STR" };
    let suffix = if is_byte { "B" } else { "" };
    (format!("{}{}", mnem, suffix), format!("R{}, [R{}, #{}]", rd, rn, offset), None)
}

fn decode_ldr_str_hw_imm(hw: u16) -> (String, String, Option<String>) {
    let is_load = (hw >> 11) & 1 == 1;
    let offset = ((hw >> 6) & 0x1F) as u32 * 2;
    let rn = (hw >> 3) & 7;
    let rd = hw & 7;
    let mnem = if is_load { "LDRH" } else { "STRH" };
    (mnem.into(), format!("R{}, [R{}, #{}]", rd, rn, offset), None)
}

fn decode_special(hw: u16, addr: u32) -> (String, String, Option<String>) {
    // BX LR / BX Rm (0x47XX)
    if (hw & 0xFF80) == 0x4700 {
        let rm = (hw >> 3) & 0xF;
        if rm == 14 {
            return ("BX".into(), "LR".into(), Some("; function return".into()));
        }
        return ("BX".into(), reg_name(rm), None);
    }
    // BLX Rm (0x47XX with bit 7 set)
    if (hw & 0xFF80) == 0x4780 {
        let rm = (hw >> 3) & 0xF;
        return ("BLX".into(), reg_name(rm), Some("; indirect call".into()));
    }
    // PUSH {reglist} (0xB400-0xB5FF)
    if (hw & 0xFE00) == 0xB400 {
        let reg_list = hw & 0x1FF;
        return ("PUSH".into(), format!("{{{}}}", format_reg_list(reg_list, true)), None);
    }
    // POP {reglist} (0xBC00-0xBDFF)
    if (hw & 0xFE00) == 0xBC00 {
        let reg_list = hw & 0x1FF;
        return ("POP".into(), format!("{{{}}}", format_reg_list(reg_list, true)), None);
    }
    // CBZ / CBNZ (0xB100-0xB1FF / 0xB900-0xB9FF)
    // 编码: 1011 op 0 i1 1 imm5 Rn (op=0:CBZ, op=1:CBNZ)
    // bits[15:12] = 1011, bits[11] = op, bits[10] = 0 (固定), bits[9] = i1, bits[8] = 1 (固定)
    // bits[7:3] = imm5, bits[2:0] = Rn
    // 注意: op 位是 bit[11] 不是 bit[10]! CBZ=0xB1xx, CBNZ=0xB9xx
    if (hw & 0xF500) == 0xB100 {
        let i = (hw >> 9) & 1;        // i1 = bit[9]
        let imm5 = (hw >> 3) & 0x1F;  // imm5 = bits[7:3]
        let rn = hw & 7;              // Rn = bits[2:0]
        let offset = ((i as u32) << 6) | ((imm5 as u32) << 1);
        let target = addr + 4 + offset;
        let mnem = if (hw >> 11) & 1 == 1 { "CBNZ" } else { "CBZ" };
        return (mnem.into(), format!("R{}, 0x{:08X}", rn, target), None);
    }
    // ADD SP, #imm (0xB000-0xB0FF)
    if (hw & 0xFF80) == 0xB000 {
        let imm7 = (hw & 0x7F) as u32 * 4;
        return ("ADD".into(), format!("SP, SP, #{}", imm7), None);
    }
    // SUB SP, #imm (0xB080-0xB0FF)
    if (hw & 0xFF80) == 0xB080 {
        let imm7 = (hw & 0x7F) as u32 * 4;
        return ("SUB".into(), format!("SP, SP, #{}", imm7), None);
    }
    // SXTH/SXTB/UXTH/UXTB (0xB200-0xB2FF)
    // 编码: 1011 0010 op(2) Rm(3) Rd(3)
    // op=00 SXTH, 01 UXTH, 10 SXTB, 11 UXTB
    if (hw & 0xFF00) == 0xB200 {
        let op = (hw >> 6) & 0x3;
        let rm = (hw >> 3) & 0x7;
        let rd = hw & 0x7;
        let mnem = match op {
            0 => "SXTH", 1 => "UXTH", 2 => "SXTB", _ => "UXTB",
        };
        return (mnem.into(), format!("R{}, R{}", rd, rm), None);
    }
    // REV/REV16/REVSH (0xBA00-0xBAFF)
    // 编码: 1011 1010 op(2) Rm(3) Rd(3)
    // op=00 REV, 01 REV16, 10 (保留), 11 REVSH
    if (hw & 0xFF00) == 0xBA00 {
        let op = (hw >> 6) & 0x3;
        let rm = (hw >> 3) & 0x7;
        let rd = hw & 0x7;
        let mnem = match op {
            0 => "REV", 1 => "REV16", 3 => "REVSH", _ => "REV??",
        };
        return (mnem.into(), format!("R{}, R{}", rd, rm), None);
    }
    // NOP (0xBF00)
    if hw == 0xBF00 {
        return ("NOP".into(), "".into(), None);
    }
    // BKPT (0xBE00-0xBEFF)
    if (hw & 0xFF00) == 0xBE00 {
        return ("BKPT".into(), format!("#{}", hw & 0xFF), None);
    }
    // SVC (0xDF00 handled elsewhere, 0xBF00-0xBFFF IT/hints)
    if (hw & 0xFF00) == 0xBF00 {
        return (".word".into(), format!("0x{:04X}", hw), Some("; IT/hint instruction".into()));
    }

    (".word".into(), format!("0x{:04X}", hw), None)
}

/// 解码 Thumb-2 (32位) 指令 — 仅处理 BL/BLX
fn decode_thumb32(hw1: u16, hw2: u16, addr: u32) -> DisasmInstruction {
    let bytes = vec![
        (hw1 & 0xFF) as u8, (hw1 >> 8) as u8,
        (hw2 & 0xFF) as u8, (hw2 >> 8) as u8,
    ];

    // BL (branch with link): 11110 S imm11 11 J1 1 J2 imm11
    if (hw1 & 0xF800) == 0xF000 && (hw2 & 0xD000) == 0xD000 {
        let s = ((hw1 >> 10) & 1) as u32;
        let j1 = ((hw2 >> 13) & 1) as u32;
        let j2 = ((hw2 >> 11) & 1) as u32;
        let imm10 = (hw1 & 0x3FF) as u32;
        let imm11 = (hw2 & 0x7FF) as u32;

        let i1 = !(j1 ^ s) & 1;
        let i2 = !(j2 ^ s) & 1;

        let mut offset = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1);
        if s == 1 { offset |= 0xFE000000u32; }

        // wrapping 语义：负偏移（S=1）时 addr+4+offset 会 u32 溢出，debug 构建直接 panic
        let target = addr.wrapping_add(4).wrapping_add(offset);
        return DisasmInstruction {
            addr, bytes,
            mnemonic: "BL".into(),
            operands: format!("0x{:08X}", target),
            comment: Some("; function call".into()),
        };
    }

    // BLX (branch with link and exchange): 11110 S imm10H 11 J1 0 J2 imm10L 0
    if (hw1 & 0xF800) == 0xF000 && (hw2 & 0xD000) == 0xC000 {
        let s = ((hw1 >> 10) & 1) as u32;
        let j1 = ((hw2 >> 13) & 1) as u32;
        let j2 = ((hw2 >> 11) & 1) as u32;
        let imm10h = (hw1 & 0x3FF) as u32;
        let imm10l = ((hw2 >> 1) & 0x3FF) as u32;

        let i1 = !(j1 ^ s) & 1;
        let i2 = !(j2 ^ s) & 1;

        let mut offset = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10h << 12) | (imm10l << 2);
        if s == 1 { offset |= 0xFE000000u32; }

        let target = addr.wrapping_add(4).wrapping_add(offset) & !3;
        return DisasmInstruction {
            addr, bytes,
            mnemonic: "BLX".into(),
            operands: format!("0x{:08X}", target),
            comment: Some("; function call".into()),
        };
    }

    // B.W (unconditional branch, 32-bit): 11110 S imm10 10 J1 1 J2 imm11
    if (hw1 & 0xF800) == 0xF000 && (hw2 & 0xD000) == 0x9000 {
        let s = ((hw1 >> 10) & 1) as u32;
        let j1 = ((hw2 >> 13) & 1) as u32;
        let j2 = ((hw2 >> 11) & 1) as u32;
        let imm10 = (hw1 & 0x3FF) as u32;
        let imm11 = (hw2 & 0x7FF) as u32;
        let i1 = !(j1 ^ s) & 1;
        let i2 = !(j2 ^ s) & 1;
        let mut offset = (s << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1);
        if s == 1 { offset |= 0xFE000000u32; }
        let target = addr.wrapping_add(4).wrapping_add(offset);
        return DisasmInstruction { addr, bytes, mnemonic: "B.W".into(), operands: format!("0x{:08X}", target), comment: None };
    }

    // B.W (conditional, 32-bit): 11110 S cond imm6 0 J1 0 J2 imm11
    // 注意: cond=0b1110 (AL) 时为提示指令(NOP.W/YIELD/WFE/WFI/SEV)，不是 B.W
    // 注意: cond=0b1111 (NV/reserved) 时为 MRS/MSR/屏障指令，也不是 B.W
    //       这些指令的 hw1 为 0xF3AF/0xF3BF/0xF3EF/0xF380，cond 字段为 0b1111
    //       必须排除，否则 MRS 等指令会被误解码为 B.W (conditional)
    if (hw1 & 0xF800) == 0xF000 && (hw2 & 0xD000) == 0x8000
        && ((hw1 >> 6) & 0xF) != 0b1110
        && ((hw1 >> 6) & 0xF) != 0b1111 {
        let s = ((hw1 >> 10) & 1) as u32;
        let j1 = ((hw2 >> 13) & 1) as u32;
        let j2 = ((hw2 >> 11) & 1) as u32;
        let imm6 = (hw1 & 0x3F) as u32;
        let imm11 = (hw2 & 0x7FF) as u32;
        let i1 = !(j1 ^ s) & 1;
        let i2 = !(j2 ^ s) & 1;
        let mut offset = (s << 20) | (i1 << 19) | (i2 << 18) | (imm6 << 12) | (imm11 << 1);
        if s == 1 { offset |= 0xFFE00000u32; }
        let target = addr.wrapping_add(4).wrapping_add(offset);
        let cond = (hw1 >> 6) & 0xF;
        return DisasmInstruction { addr, bytes, mnemonic: format!("B{}", cond_name(cond)).into(), operands: format!("0x{:08X}", target), comment: None };
    }

    // 数据处理指令 (shifted register): 1110 101x op(4) S Rn(4) / imm3 Rd(4) imm2 type Rm(4)
    // 涵盖: AND.W, ORR.W, EOR.W, BIC.W, ADD.W, ADC.W, SBC.W, SUB.W, RSB.W, etc.
    // 0xEA00-0xEBFF: bits[15:9] = 1110101
    if (hw1 & 0xFE00) == 0xEA00 {
        let op = (hw1 >> 5) & 0xF;  // bits [8:5]
        let s = (hw1 >> 4) & 1;     // bit 4
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        // 测试指令别名：Rd=PC(15) 且 S=1
        // TST=ANDS PC, Rn, Rm / TEQ=EORS PC, Rn, Rm / CMN=ADDS PC, Rn, Rm / CMP=SUBS PC, Rn, Rm
        if rd == 15 && s == 1 {
            if let Some(test_mnem) = match op {
                0x0 => Some("TST"),
                0x4 => Some("TEQ"),
                0x8 => Some("CMN"),
                0xD => Some("CMP"),
                _ => None,
            } {
                return DisasmInstruction {
                    addr, bytes,
                    mnemonic: test_mnem.into(),
                    operands: format!("R{}, R{}", rn, rm),
                    comment: None,
                };
            }
        }
        let (mnem, _) = match op {
            0x0 => ("AND", false),
            0x1 => ("BIC", false),
            0x2 => ("ORR", false),
            0x3 => ("ORN", false),
            0x4 => ("EOR", false),
            0x6 => ("PKH", false),
            0x8 => ("ADD", false),
            0xA => ("ADC", false),
            0xB => ("SBC", false),
            0xD => ("SUB", false),
            0xE => ("RSB", false),
            _ => ("?", false),
        };
        let suffix = if s == 1 { "S" } else { "" };
        return DisasmInstruction {
            addr, bytes,
            mnemonic: format!("{}{}.W", mnem, suffix),
            operands: format!("R{}, R{}, R{}", rd, rn, rm),
            comment: None,
        };
    }

    // PLD (预取数据): 1111 1000 1001 Rn | 1111 imm12
    // PLI (预取指令): 1111 1001 1001 Rn | 1111 imm12
    // 与 LDRB.W 编码相同（F89x），仅 Rt=1111 区分；必须先于 LDRB.W 检查。
    if (hw1 & 0xFFF0) == 0xF890 && ((hw2 >> 12) & 0xF) == 0xF {
        let rn = (hw1 & 0xF) as u8;
        let imm12 = hw2 & 0xFFF;
        let u = (hw1 >> 7) & 1;
        let off_str = if u == 0 { format!("-{}", imm12) } else { format!("{}", imm12) };
        let rn_str = if rn == 15 { "PC".to_string() } else { format!("R{}", rn) };
        return DisasmInstruction {
            addr, bytes, mnemonic: "PLD".into(),
            operands: format!("[{}, #{}]", rn_str, off_str),
            comment: Some("; data prefetch".into()),
        };
    }
    if (hw1 & 0xFFF0) == 0xF990 && ((hw2 >> 12) & 0xF) == 0xF {
        let rn = (hw1 & 0xF) as u8;
        let imm12 = hw2 & 0xFFF;
        let u = (hw1 >> 7) & 1;
        let off_str = if u == 0 { format!("-{}", imm12) } else { format!("{}", imm12) };
        let rn_str = if rn == 15 { "PC".to_string() } else { format!("R{}", rn) };
        return DisasmInstruction {
            addr, bytes, mnemonic: "PLI".into(),
            operands: format!("[{}, #{}]", rn_str, off_str),
            comment: Some("; instruction prefetch".into()),
        };
    }

    // LDRB.W / STRB.W (immediate, post-index): 11111 00 Rn Rt imm12
    // LDRB.W: 0xF810 / STRB.W: 0xF800
    if (hw1 & 0xFF70) == 0xF810 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        // 后变址 (P=0, W=1): [Rn], #imm8
        if (hw2 & 0x0F00) == 0x0B00 {
            return DisasmInstruction { addr, bytes, mnemonic: "LDRB.W".into(), operands: format!("R{}, [R{}], #{}", rt, rn, imm8), comment: None };
        }
        // 前变址 (P=1, W=1): [Rn, #imm8]!
        if (hw2 & 0x0F00) == 0x0F00 {
            return DisasmInstruction { addr, bytes, mnemonic: "LDRB.W".into(), operands: format!("R{}, [R{}, #{}]!", rt, rn, imm8), comment: None };
        }
        // 偏移 (P=1, W=0): [Rn, #imm8]
        return DisasmInstruction { addr, bytes, mnemonic: "LDRB.W".into(), operands: format!("R{}, [R{}, #{}]", rt, rn, imm8), comment: None };
    }
    if (hw1 & 0xFF70) == 0xF800 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        if (hw2 & 0x0F00) == 0x0B00 {
            return DisasmInstruction { addr, bytes, mnemonic: "STRB.W".into(), operands: format!("R{}, [R{}], #{}", rt, rn, imm8), comment: None };
        }
        if (hw2 & 0x0F00) == 0x0F00 {
            return DisasmInstruction { addr, bytes, mnemonic: "STRB.W".into(), operands: format!("R{}, [R{}, #{}]!", rt, rn, imm8), comment: None };
        }
        return DisasmInstruction { addr, bytes, mnemonic: "STRB.W".into(), operands: format!("R{}, [R{}, #{}]", rt, rn, imm8), comment: None };
    }

    // LDR.W (literal, PC-relative): 1111 1000 U101 1111 | Rt imm12
    // 必须在 T3 (0xF8D0) 之前检查，因为 0xF8DF (Rn=PC=15) 也匹配 T3 掩码 0xFFF0
    // U=1: 0xF8DF (positive offset), U=0: 0xF85F (negative offset)
    if (hw1 & 0xFF7F) == 0xF85F {
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm12 = hw2 & 0xFFF;
        let u = (hw1 >> 7) & 1;
        let target = if u == 1 {
            ((addr + 4) & !3u32) + imm12 as u32
        } else {
            ((addr + 4) & !3u32).wrapping_sub(imm12 as u32)
        };
        let rt_str = match rt { 13 => "SP", 15 => "PC", _ => "" };
        let off_str = if u == 1 { format!("+{}", imm12) } else { format!("-{}", imm12) };
        if rt_str.is_empty() {
            return DisasmInstruction { addr, bytes, mnemonic: "LDR.W".into(), operands: format!("R{}, [PC, #{}]", rt, off_str), comment: Some(format!("; R{} → [0x{:08X}]", rt, target)) };
        }
        return DisasmInstruction { addr, bytes, mnemonic: "LDR.W".into(), operands: format!("{}, [PC, #{}]", rt_str, off_str), comment: Some(format!("; {} → [0x{:08X}]", rt_str, target)) };
    }

    // LDR.W (immediate, T3): 1111 1000 1101 Rn | Rt imm12 (Rn≠PC, PC 已由 literal 处理)
    if (hw1 & 0xFFF0) == 0xF8D0 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm12 = hw2 & 0xFFF;
        return DisasmInstruction { addr, bytes, mnemonic: "LDR.W".into(), operands: format!("R{}, [R{}, #{}]", rt, rn, imm12), comment: None };
    }
    // STR.W (immediate, T3): 1111 1000 1100 Rn | Rt imm12
    if (hw1 & 0xFFF0) == 0xF8C0 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm12 = hw2 & 0xFFF;
        return DisasmInstruction { addr, bytes, mnemonic: "STR.W".into(), operands: format!("R{}, [R{}, #{}]", rt, rn, imm12), comment: None };
    }

    // NOP.W: 0xF3AF 0x8000
    if hw1 == 0xF3AF && hw2 == 0x8000 {
        return DisasmInstruction { addr, bytes, mnemonic: "NOP.W".into(), operands: "".into(), comment: None };
    }

    // Hint 指令 (0xF3AF 0x8xxx): NOP.W/YIELD/WFE/WFI/SEV
    // 0xF3AF 0x8000: NOP.W, 0xF3AF 0x8001: YIELD, 0xF3AF 0x8002: WFE
    // 0xF3AF 0x8003: WFI, 0xF3AF 0x8004: SEV
    if hw1 == 0xF3AF && (hw2 & 0xFF0F) == 0x8000 {
        let hint = match hw2 & 0xF0 {
            0x00 => "NOP.W",
            0x10 => "YIELD",
            0x20 => "WFE",
            0x30 => "WFI",
            0x40 => "SEV",
            _ => {
                let word = (hw1 as u32) | ((hw2 as u32) << 16);
                return DisasmInstruction {
                    addr, bytes, mnemonic: ".word".into(),
                    operands: format!("0x{:08X}", word),
                    comment: Some("; unknown hint instruction".into()),
                };
            }
        };
        return DisasmInstruction { addr, bytes, mnemonic: hint.into(), operands: "".into(), comment: None };
    }

    // 内存屏障指令 (0xF3BF 0x8Fxx): DMB/DSB/ISB/CLREX
    // DMB: 0xF3BF 0x8F4F (option=0xF) 或 0xF3BF 0x8F5x (option)
    // DSB: 0xF3BF 0x8F4x (option)
    // ISB: 0xF3BF 0x8F6x (option)
    // CLREX: 0xF3BF 0x8F2F
    if hw1 == 0xF3BF && (hw2 & 0xFF00) == 0x8F00 {
        let option = hw2 & 0xFF;
        let mnem = match option {
            0x4F | 0x40 | 0x41 | 0x42 | 0x43 | 0x44 => "DSB",
            0x5F | 0x50 | 0x51 | 0x52 | 0x53 | 0x54 => "DMB",
            0x6F | 0x60 | 0x61 | 0x62 | 0x63 | 0x64 => "ISB",
            0x2F => "CLREX",
            _ => {
                let word = (hw1 as u32) | ((hw2 as u32) << 16);
                return DisasmInstruction {
                    addr, bytes, mnemonic: ".word".into(),
                    operands: format!("0x{:08X}", word),
                    comment: Some(format!("; unknown barrier instruction (option=0x{:02X})", option)),
                };
            }
        };
        let operands = if mnem == "CLREX" {
            "".to_string()
        } else if option == 0x4F || option == 0x5F || option == 0x6F {
            "SY".to_string()  // 全系统屏障
        } else {
            format!("#0x{:02X}", option)
        };
        return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands, comment: None };
    }

    // MRS (读特殊寄存器): 0xF3EF 0x8000 + SYSm
    // MRS Rt, SYSm: 1111 0011 1110 1111 1000 Rd(4) 0000 SYSm(8)
    // 注意: bits[15:12]=1000 (固定), bits[11:8]=Rd, bits[7:0]=SYSm
    // 掩码应检查 bits[15:12]=0x8: (hw2 & 0xF000) == 0x8000
    // 之前的 (hw2 & 0x0F00) == 0x0000 只匹配 Rd=0，且会误匹配其他指令
    if hw1 == 0xF3EF && (hw2 & 0xF000) == 0x8000 {
        let rt = ((hw2 >> 8) & 0xF) as u8;
        let sysm = hw2 & 0xFF;
        let sysm_name = match sysm {
            0x00 => "APSR",
            0x01 => "IAPSR",
            0x02 => "EAPSR",
            0x03 => "XPSR",
            0x04 => "IPSR",
            0x05 => "EPSR",
            0x06 => "IEPSR",
            0x08 => "MSP",
            0x09 => "PSP",
            0x10 => "PRIMASK",
            0x11 => "BASEPRI",
            0x12 => "BASEPRI_MAX",
            0x13 => "FAULTMASK",
            0x14 => "CONTROL",
            _ => "SYSm",
        };
        let operands = if sysm_name == "SYSm" {
            format!("R{}, SYSm_0x{:02X}", rt, sysm)
        } else {
            format!("R{}, {}", rt, sysm_name)
        };
        return DisasmInstruction { addr, bytes, mnemonic: "MRS".into(), operands, comment: None };
    }

    // MSR (写特殊寄存器): 0xF380 0x8000 + SYSm
    // MSR SYSm, Rt: 1111 0011 1000 1000 1000 SYSm(8) Rt(4) 0000
    if hw1 == 0xF380 && (hw2 & 0xF000) == 0x8000 {
        let rt = ((hw2 >> 8) & 0xF) as u8;
        let sysm = hw2 & 0xFF;
        let sysm_name = match sysm {
            0x00 => "APSR",
            0x01 => "IAPSR",
            0x02 => "EAPSR",
            0x03 => "XPSR",
            0x04 => "IPSR",
            0x05 => "EPSR",
            0x06 => "IEPSR",
            0x08 => "MSP",
            0x09 => "PSP",
            0x10 => "PRIMASK",
            0x11 => "BASEPRI",
            0x12 => "BASEPRI_MAX",
            0x13 => "FAULTMASK",
            0x14 => "CONTROL",
            _ => "SYSm",
        };
        let operands = if sysm_name == "SYSm" {
            format!("SYSm_0x{:02X}, R{}", sysm, rt)
        } else {
            format!("{}, R{}", sysm_name, rt)
        };
        return DisasmInstruction { addr, bytes, mnemonic: "MSR".into(), operands, comment: None };
    }

    // CMP.W (immediate): 0xF1B0 (modified imm, Rn!=PC)
    // 注意: MOV.W (immediate) 的 0xF04F/0xF44F (op=ORR, Rn=PC, bit[9]=0) 走 modified imm 分支统一处理
    if (hw1 & 0xFBFF) == 0xF1B0 {
        let rn = (hw1 & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        return DisasmInstruction { addr, bytes, mnemonic: "CMP.W".into(), operands: format!("R{}, #0x{:02X}", rn, imm8), comment: None };
    }

    // PUSH.W: 0xE92D reg_list
    if hw1 == 0xE92D {
        let mut regs = Vec::new();
        for bit in 0..16u32 {
            if hw2 & (1 << bit) as u16 != 0 {
                if bit == 14 { regs.push("LR".into()); }
                else if bit == 15 { regs.push("PC".into()); }
                else { regs.push(format!("R{}", bit)); }
            }
        }
        return DisasmInstruction { addr, bytes, mnemonic: "PUSH.W".into(), operands: format!("{{{}}}", regs.join(", ")), comment: None };
    }
    // POP.W: 0xE8BD reg_list
    if hw1 == 0xE8BD {
        let mut regs = Vec::new();
        for bit in 0..16u32 {
            if hw2 & (1 << bit) as u16 != 0 {
                if bit == 14 { regs.push("LR".into()); }
                else if bit == 15 { regs.push("PC".into()); }
                else { regs.push(format!("R{}", bit)); }
            }
        }
        return DisasmInstruction { addr, bytes, mnemonic: "POP.W".into(), operands: format!("{{{}}}", regs.join(", ")), comment: None };
    }

    // 数据处理 (immediate) 旧分支已删除，由后面的 modified imm 分支统一处理

    // MOV.W (16-bit immediate, T2): 11110 i 100100 11111 | 0 imm3 Rd imm8
    // 掩码 0xFBFF: 保留所有位除了 bit[10](i)；Rn=11111(固定)
    // 0xF24F(i=0) / 0xF64F(i=1) 都匹配
    if (hw1 & 0xFBFF) == 0xF24F {
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        let imm3 = (hw2 >> 12) & 0x7;
        let i = (hw1 >> 10) & 1;
        let imm16 = (i << 11) | (imm3 << 8) | imm8;
        return DisasmInstruction { addr, bytes, mnemonic: "MOV.W".into(), operands: format!("R{}, #0x{:04X}", rd, imm16), comment: None };
    }
    // MOVT: 11110 i 101100 Rn | 0 imm3 Rd imm8
    // 掩码 0xFEDF: 保留 bits[15:9]=1111001, bit[7]=1, bit[6]=0, bits[5:4]=0
    if (hw1 & 0xFEDF) == 0xF2C0 {
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        let imm3 = (hw2 >> 12) & 0x7;
        let i = (hw1 >> 10) & 1;
        let imm16 = (i << 11) | (imm3 << 8) | imm8;
        return DisasmInstruction { addr, bytes, mnemonic: "MOVT".into(), operands: format!("R{}, #0x{:04X}", rd, imm16), comment: None };
    }

    // ADD.W/SUB.W (imm, T4 encoding, plain binary immediate)
    // ADD T4: 11110 i 1 0000 0 Rn | 0 imm3 Rd imm8 (op=0000, hw1=0xF200/0xF600)
    // SUB T4: 11110 i 1 1101 0 Rn | 0 imm3 Rd imm8 (op=1101, hw1=0xF3A0/0xF7A0)
    // 掩码 0xFBF0: 忽略 bit[10](i) 和 bits[3:0](Rn)
    // 特殊: ADD Rn=PC 等价于 ADR
    if (hw1 & 0xFBF0) == 0xF200 || (hw1 & 0xFBF0) == 0xF3A0 {
        let s = (hw1 >> 4) & 1;
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        let imm3 = (hw2 >> 12) & 0x7;
        let i = (hw1 >> 10) & 1;
        let imm12 = ((i << 11) | (imm3 << 8) | imm8) as u32;
        let op = (hw1 >> 5) & 0xF;
        let suffix = if s == 1 { "S" } else { "" };
        if op == 0x0 {
            // ADD T4
            if rn == 15 {
                // ADR (PC-relative): target = (PC & ~3) + imm12
                let target = ((addr + 4) & !3u32) + imm12;
                return DisasmInstruction { addr, bytes, mnemonic: "ADR.W".into(), operands: format!("R{}, 0x{:08X}", rd, target), comment: Some(format!("; ADD R{}, PC, #0x{:03X}", rd, imm12)) };
            }
            return DisasmInstruction { addr, bytes, mnemonic: format!("ADD{}.W", suffix), operands: format!("R{}, R{}, #0x{:03X}", rd, rn, imm12), comment: Some("; T4 plain imm".into()) };
        } else {
            // SUB T4
            return DisasmInstruction { addr, bytes, mnemonic: format!("SUB{}.W", suffix), operands: format!("R{}, R{}, #0x{:03X}", rd, rn, imm12), comment: Some("; T4 plain imm".into()) };
        }
    }

    // 数据处理 (modified immediate, ThumbExpandImm): 11110 i 0 op(4) S Rn / 0 imm3 Rd imm8
    // bit[9]=0 表示 modified immediate (ThumbExpandImm 编码)
    // 覆盖: AND/BIC/ORR/ORN/EOR/ADD/ADC/SBC/SUB/RSB.W (modified imm)
    // 特殊: op=ORR, Rn=PC → MOV.W; op=ORN, Rn=PC → MVN.W
    // 注意: MOV.W(T2, 16-bit imm, 0xF24F/0xF64F bit[9]=1) 和 MOVT 已在前面的专用检查中处理
    if (hw1 & 0xF800) == 0xF000 && (hw1 & 0x0200) == 0 && (hw2 & 0x8000) == 0 {
        let op = (hw1 >> 5) & 0xF;
        let s = (hw1 >> 4) & 1;
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        let imm3 = (hw2 >> 12) & 0x7;
        let i = (hw1 >> 10) & 1;
        let imm12 = ((i << 11) | (imm3 << 8) | imm8) as u32;
        let expanded = thumb_expand_imm(imm12);
        if rn == 15 {
            match op {
                0x2 => return DisasmInstruction { addr, bytes, mnemonic: format!("MOV{}.W", if s == 1 { "S" } else { "" }), operands: format!("R{}, #0x{:X}", rd, expanded), comment: Some(format!("; modified imm (raw 0x{:03X})", imm12)) },
                0x6 => return DisasmInstruction { addr, bytes, mnemonic: format!("MVN{}.W", if s == 1 { "S" } else { "" }), operands: format!("R{}, #0x{:X}", rd, expanded), comment: Some(format!("; modified imm (raw 0x{:03X})", imm12)) },
                _ => {}
            }
        }
        // 测试指令别名：Rd=PC(15) 且 S=1
        // TST=ANDS PC, Rn, #imm / TEQ=EORS PC, Rn, #imm / CMN=ADDS PC, Rn, #imm / CMP=SUBS PC, Rn, #imm
        if rd == 15 && s == 1 {
            if let Some(test_mnem) = match op {
                0x0 => Some("TST"),
                0x4 => Some("TEQ"),
                0x8 => Some("CMN"),
                0xD => Some("CMP"),
                _ => None,
            } {
                return DisasmInstruction { addr, bytes, mnemonic: test_mnem.into(), operands: format!("R{}, #0x{:X}", rn, expanded), comment: Some(format!("; modified imm (raw 0x{:03X})", imm12)) };
            }
        }
        let mnem = match op {
            0x0 => "AND", 0x1 => "BIC", 0x2 => "ORR", 0x3 => "ORN",
            0x4 => "EOR", 0x8 => "ADD", 0xA => "ADC", 0xB => "SBC",
            0xD => "SUB", 0xE => "RSB", _ => "?",
        };
        let suffix = if s == 1 { "S" } else { "" };
        return DisasmInstruction { addr, bytes, mnemonic: format!("{}{}.W", mnem, suffix), operands: format!("R{}, R{}, #0x{:X}", rd, rn, expanded), comment: Some(format!("; modified imm (raw 0x{:03X})", imm12)) };
    }

    // MUL/MLA/SDIV/UDIV/SMULL/UMULL 等 0xFB00-0xFBFF 指令在后面统一处理

    // 数据处理 (plain binary immediate): 11110 i 1 op(4) S Rn / 0 imm3 Rd imm8
    // bit[9]=1 表示 plain binary immediate (12-bit 不解码立即数)
    // 注意: ADD T4 (op=0000), SUB T4 (op=1101), MOV T2 (op=0010, Rn=PC), MOVT 已在前面处理
    // 剩余: ORR (op=0010, Rn!=PC), ORN/MVN (op=0011), SSAT (op=0110), SBFX (op=1110)
    if (hw1 & 0xF800) == 0xF000 && (hw1 & 0x0200) != 0 && (hw2 & 0x8000) == 0 {
        let op = (hw1 >> 5) & 0xF;
        let s = (hw1 >> 4) & 1;
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        let imm3 = (hw2 >> 12) & 0x7;
        let i = (hw1 >> 10) & 1;
        let imm12 = ((i << 11) | (imm3 << 8) | imm8) as u32;
        // ORR (plain imm, Rn!=PC)
        if op == 0x2 && rn != 15 {
            let suffix = if s == 1 { "S" } else { "" };
            return DisasmInstruction { addr, bytes, mnemonic: format!("ORR{}.W", suffix), operands: format!("R{}, R{}, #0x{:03X}", rd, rn, imm12), comment: Some("; plain imm".into()) };
        }
        // ORN (plain imm, Rn!=PC) / MVN (Rn=PC)
        if op == 0x3 {
            if rn == 15 {
                let suffix = if s == 1 { "S" } else { "" };
                return DisasmInstruction { addr, bytes, mnemonic: format!("MVN{}.W", suffix), operands: format!("R{}, #0x{:03X}", rd, imm12), comment: Some("; plain imm".into()) };
            }
            let suffix = if s == 1 { "S" } else { "" };
            return DisasmInstruction { addr, bytes, mnemonic: format!("ORN{}.W", suffix), operands: format!("R{}, R{}, #0x{:03X}", rd, rn, imm12), comment: Some("; plain imm".into()) };
        }
        // SSAT (op=0110) 编码复杂，暂不处理
        // SBFX/UBFX 已在前面专用检查中处理 (op=1100/1110)
    }

    // LSL.W/LSR.W/ASR.W/ROR.W (register): 0xFA00/0xFA20/0xFA40/0FA60
    if (hw1 & 0xFF80) == 0xFA00 {
        let op = (hw1 >> 5) & 0x3;
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        let s = (hw1 >> 4) & 1;
        let mnem = match op { 0 => "LSL", 1 => "LSR", 2 => "ASR", 3 => "ROR", _ => "?" };
        let suffix = if s == 1 { "S" } else { "" };
        return DisasmInstruction { addr, bytes, mnemonic: format!("{}{}.W", mnem, suffix), operands: format!("R{}, R{}, R{}", rd, rn, rm), comment: None };
    }

    // LDR.W/STR.W (register offset): 0xF850/0xF840
    if (hw1 & 0xFFF0) == 0xF850 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        let shift = (hw2 >> 4) & 0x3;
        if (hw2 & 0x0FC0) == 0 {
            return DisasmInstruction { addr, bytes, mnemonic: "LDR.W".into(), operands: format!("R{}, [R{}, R{}]", rt, rn, rm), comment: None };
        }
        return DisasmInstruction { addr, bytes, mnemonic: "LDR.W".into(), operands: format!("R{}, [R{}, R{}, LSL #{}]", rt, rn, rm, shift), comment: None };
    }
    if (hw1 & 0xFFF0) == 0xF840 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rm = (hw2 & 0xF) as u8;
        let shift = (hw2 >> 4) & 0x3;
        if (hw2 & 0x0FC0) == 0 {
            return DisasmInstruction { addr, bytes, mnemonic: "STR.W".into(), operands: format!("R{}, [R{}, R{}]", rt, rn, rm), comment: None };
        }
        return DisasmInstruction { addr, bytes, mnemonic: "STR.W".into(), operands: format!("R{}, [R{}, R{}, LSL #{}]", rt, rn, rm, shift), comment: None };
    }

    // CBZ/CBNZ: actually 16-bit, handled elsewhere

    // BFI: 0xF360-0xF36F (掩码 0xFFF0)
    if (hw1 & 0xFFF0) == 0xF360 {
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let lsb = ((hw2 >> 6) & 0x1F) as u32;
        let msb = (hw2 & 0x1F) as u32;
        let width = if msb >= lsb { msb - lsb + 1 } else { 1 };
        return DisasmInstruction { addr, bytes, mnemonic: "BFI".into(), operands: format!("R{}, R{}, #{}, #{}", rd, rn, lsb, width), comment: None };
    }
    // UBFX: 11110 i 1 1110 0 Rn | Rd imm3 0 imm2 widthm1
    // op=1110=14, hw1=0xF3C0/0xF7C0, 掩码 0xFBF0 (忽略 i 和 Rn)
    if (hw1 & 0xFBF0) == 0xF3C0 {
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let lsb = ((hw2 >> 6) & 0x1F) as u32;
        let width = (hw2 & 0x1F) as u32 + 1;
        return DisasmInstruction { addr, bytes, mnemonic: "UBFX".into(), operands: format!("R{}, R{}, #{}, #{}", rd, rn, lsb, width), comment: None };
    }
    // SBFX: 11110 i 1 1100 0 Rn | Rd imm3 0 imm2 widthm1
    // op=1100=12, hw1=0xF380/0xF780, 掩码 0xFBF0 (忽略 i 和 Rn)
    if (hw1 & 0xFBF0) == 0xF380 {
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        let lsb = ((hw2 >> 6) & 0x1F) as u32;
        let width = (hw2 & 0x1F) as u32 + 1;
        return DisasmInstruction { addr, bytes, mnemonic: "SBFX".into(), operands: format!("R{}, R{}, #{}, #{}", rd, rn, lsb, width), comment: None };
    }

    // STM.W / LDM.W (T1): 1110 100P U 0 W L Rn | register_list
    // 位布局: bit[8]=P, bit[7]=U, bit[6]=0(SBZ), bit[5]=W, bit[4]=L, bits[3:0]=Rn
    // 0xE880: STMIA (P=0,U=1,W=0,L=0), 0xE890: LDMIA (P=0,U=1,W=0,L=1)
    // 0xE920: STMDB! (P=1,U=0,W=1,L=0), 0xE930: LDMDB! (P=1,U=0,W=1,L=1)
    if (hw1 & 0xFE40) == 0xE800 {
        let is_load = (hw1 & 0x0010) != 0;  // L bit = bit[4]
        let w = (hw1 >> 5) & 1;             // W bit = bit[5]
        let p = (hw1 >> 8) & 1;             // P bit = bit[8]
        let u = (hw1 >> 7) & 1;             // U bit = bit[7]
        let rn = (hw1 & 0xF) as u8;
        let reg_list = hw2;
        let mut regs = Vec::new();
        for bit in 0..16u32 {
            if reg_list & (1 << bit) as u16 != 0 {
                if bit == 14 { regs.push("LR".into()); }
                else if bit == 15 { regs.push("PC".into()); }
                else { regs.push(format!("R{}", bit)); }
            }
        }
        let writeback = if w == 1 { "!" } else { "" };
        // 后缀: IA/IB/DA/DB 根据 P/U
        let suffix = match (p, u) {
            (0, 1) => "IA",  // Increment After
            (1, 1) => "IB",  // Increment Before
            (0, 0) => "DA",  // Decrement After
            (1, 0) => "DB",  // Decrement Before
            _ => "IA",
        };
        let mnem = if is_load { format!("LDM{}.W", suffix) } else { format!("STM{}.W", suffix) };
        return DisasmInstruction { addr, bytes, mnemonic: mnem, operands: format!("R{}{}, {{{}}}", rn, writeback, regs.join(", ")), comment: None };
    }

    // TBB / TBH (表分支指令，用于 switch 语句)
    // 编码: 1110 1000 1101 1111 | (0/1) 0000 Rm Rn
    // TBB: hw1=0xE8DF, hw2 bits[15:8]=0x00, bits[7:4]=Rm, bits[3:0]=Rn
    // TBH: hw1=0xE8DF, hw2 bit[8]=1, bits[15:9]=0, bits[7:4]=Rm, bits[3:0]=Rn
    // 语义: PC = (PC+4) + Mem[Rn + Rm] * 2 (TBB: 8位, TBH: 16位)
    // 注意: 必须在 LDRD 之前检查，因为 0xE8DF 也匹配 LDRD Rn=PC 的编码
    if hw1 == 0xE8DF && (hw2 & 0xFF00) == 0x0000 {
        let rm = ((hw2 >> 4) & 0xF) as u8;
        let rn = (hw2 & 0xF) as u8;
        let rn_str = if rn == 15 { "PC".to_string() } else { format!("R{}", rn) };
        return DisasmInstruction {
            addr, bytes, mnemonic: "TBB".into(),
            operands: format!("[{}, R{}]", rn_str, rm),
            comment: Some("; table branch (8-bit)".into()),
        };
    }
    if hw1 == 0xE8DF && (hw2 & 0xFF00) == 0x0100 {
        let rm = ((hw2 >> 4) & 0xF) as u8;
        let rn = (hw2 & 0xF) as u8;
        let rn_str = if rn == 15 { "PC".to_string() } else { format!("R{}", rn) };
        return DisasmInstruction {
            addr, bytes, mnemonic: "TBH".into(),
            operands: format!("[{}, R{}, LSL #1]", rn_str, rm),
            comment: Some("; table branch (16-bit)".into()),
        };
    }

    // LDRD / STRD: 1110 100P U1W1 Rn | Rt Rt2 imm8
    // P(bit8)/U(bit7)/W(bit5) 为可变位，掩码 0xFE50 后仅剩 L(bit4)：
    // 0xE840 = STRD，0xE850 = LDRD（覆盖 E84x/E85x/E94x/E95x/E8Cx/E8Dx/E9Cx/E9Dx）
    if (hw1 & 0xFE50) == 0xE840 || (hw1 & 0xFE50) == 0xE850 {
        let is_load = (hw1 & 0x0010) != 0;  // L bit
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let rt2 = ((hw2 >> 8) & 0xF) as u8;
        let imm8 = (hw2 & 0xFF) as u32 * 4;
        // 可疑指令检测: LDRD 加载到 PC (R15) 在用户代码中极罕见
        // 通常这是数据被误解码为指令（如跳转表数据）
        if is_load && (rt == 15 || rt2 == 15) {
            let word = (hw1 as u32) | ((hw2 as u32) << 16);
            return DisasmInstruction {
                addr, bytes, mnemonic: ".word".into(),
                operands: format!("0x{:08X}", word),
                comment: Some("; suspicious data (LDRD R15 encoding, possibly jump table data)".into()),
            };
        }
        let mnem = if is_load { "LDRD" } else { "STRD" };
        return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands: format!("R{}, R{}, [R{}, #{}]", rt, rt2, rn, imm8), comment: None };
    }

    // STRH.W / LDRH.W (immediate, T3): 1111 1000 | P U 1 W L Rn(4) | Rt(4) imm8
    // 位布局: bits[15:8]=0xF8(固定), bit[7]=P, bit[6]=U, bit[5]=0(固定), bit[4]=W, bit[3]=L? 
    // 实际验证: bits[7:5]=x01(STRH/LDRH标志), bit[4]=L(0=STRH,1=LDRH), bits[3:0]=Rn
    // 0xF82x: STRH (P=0,U=1), 0xF83x: LDRH (P=0,U=1)
    // 0xF8Ax: STRH (P=1,U=0), 0xF8Bx: LDRH (P=1,U=0)
    if (hw1 & 0xFF60) == 0xF820 {
        let is_load = (hw1 & 0x0010) != 0;  // L bit = bit[4]
        let _p = (hw1 >> 7) & 1;            // P bit = bit[7]
        let u = (hw1 >> 6) & 1;             // U bit = bit[6]
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        let off_str = if u == 0 { format!("-{}", imm8) } else { format!("{}", imm8) };
        let mnem = if is_load { "LDRH.W" } else { "STRH.W" };
        return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands: format!("R{}, [R{}, #{}]", rt, rn, off_str), comment: None };
    }

    // STR.W / LDR.W (immediate, T4): 1111 1000 0 Rn Rt 1 1 0 0 imm8
    // 0xF800: STR.W, 0xF810: LDRB.W, 0xF840: STR.W (register), 0xF850: LDR.W (register)
    // STR.W (T4): hw1=0xF8x0, hw2 bit[11:8]=1100
    if (hw1 & 0xFF00) == 0xF800 && (hw2 & 0x0F00) == 0x0C00 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        return DisasmInstruction { addr, bytes, mnemonic: "STR.W".into(), operands: format!("R{}, [R{}, #{}]", rt, rn, imm8), comment: None };
    }
    if (hw1 & 0xFFF0) == 0xF810 && (hw2 & 0x0F00) == 0x0C00 {
        let rn = (hw1 & 0xF) as u8;
        let rt = ((hw2 >> 12) & 0xF) as u8;
        let imm8 = hw2 & 0xFF;
        return DisasmInstruction { addr, bytes, mnemonic: "LDRB.W".into(), operands: format!("R{}, [R{}, #{}]", rt, rn, imm8), comment: None };
    }

    // CLZ: 0xFAB0
    if (hw1 & 0xFFF0) == 0xFAB0 {
        let rm = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        return DisasmInstruction { addr, bytes, mnemonic: "CLZ".into(), operands: format!("R{}, R{}", rd, rm), comment: None };
    }

    // REV.W / REV16.W / REVSH.W: 0xFA90 (REV), 0xFAA0 (REV16), 0xFAB0 (REVSH)
    // RBIT: 0xFAA0 with bit[4]=1
    if (hw1 & 0xFFF0) == 0xFA90 {
        let rm = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        return DisasmInstruction { addr, bytes, mnemonic: "REV.W".into(), operands: format!("R{}, R{}", rd, rm), comment: None };
    }
    if (hw1 & 0xFFF0) == 0xFAA0 {
        let rm = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        // RBIT: hw2 bit[4]=1, REV16: hw2 bit[4]=0
        if (hw2 & 0x10) != 0 {
            return DisasmInstruction { addr, bytes, mnemonic: "RBIT".into(), operands: format!("R{}, R{}", rd, rm), comment: None };
        }
        return DisasmInstruction { addr, bytes, mnemonic: "REV16.W".into(), operands: format!("R{}, R{}", rd, rm), comment: None };
    }
    if (hw1 & 0xFFF0) == 0xFAB0 && (hw2 & 0xF0) == 0 {
        let rm = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 8) & 0xF) as u8;
        return DisasmInstruction { addr, bytes, mnemonic: "REVSH.W".into(), operands: format!("R{}, R{}", rd, rm), comment: None };
    }

    // 乘法与除法指令 (0xFB00-0xFBFF):
    // MUL:   1111 1011 0000 Rn | 0000 Rd 0000 Rm  (op=0x0, Ra=0)
    // MLA:   1111 1011 0000 Rn | 0000 Rd Ra  Rm  (op=0x0, Ra≠0)
    // MLS:   1111 1011 0001 Rn | 0000 Rd Ra  Rm  (op=0x1)
    // SMULL: 1111 1011 1000 Rn | 0000 RdLo RdHi Rm (op=0x8)
    // SDIV:  1111 1011 1001 Rn | 0000 Rd 1111 Rm (op=0x9)
    // UMULL: 1111 1011 1010 Rn | 0000 RdLo RdHi Rm (op=0xA)
    // UDIV:  1111 1011 1011 Rn | 0000 Rd 1111 Rm (op=0xB)
    // SMLAL: 1111 1011 1100 Rn | 0000 RdLo RdHi Rm (op=0xC)
    // UMLAL: 1111 1011 1110 Rn | 0000 RdLo RdHi Rm (op=0xE)
    if (hw1 & 0xFF00) == 0xFB00 {
        let rn = (hw1 & 0xF) as u8;
        let rd = ((hw2 >> 12) & 0xF) as u8;  // bits[15:12] = Rd / RdLo
        let ra = ((hw2 >> 8) & 0xF) as u8;   // bits[11:8] = Ra / RdHi
        let op2 = ((hw2 >> 4) & 0xF) as u8;  // bits[7:4] = op2
        let rm = (hw2 & 0xF) as u8;          // bits[3:0] = Rm
        let op1 = (hw1 >> 4) & 0xF;          // bits[7:4] of hw1 = op1
        match op1 {
            0x0 => {
                // MUL/MLA/MLS 共享 op1=0000，由 op2 区分
                if op2 == 0x1 {
                    // MLS: Rd = Ra - Rn * Rm
                    return DisasmInstruction { addr, bytes, mnemonic: "MLS".into(), operands: format!("R{}, R{}, R{}, R{}", rd, rn, rm, ra), comment: Some("; multiply-subtract Rd = Ra - Rn*Rm".into()) };
                }
                if ra == 0 {
                    // MUL Rd=PC 或 Rn=PC 或 Rm=PC 为 UNPREDICTABLE，添加警告
                    let warn = if rd == 15 || rn == 15 || rm == 15 {
                        Some("; ⚠ DATA? MUL PC=UNPREDICTABLE (data possibly misread as code)".into())
                    } else {
                        Some("; 32-bit multiply Rd = Rn*Rm".into())
                    };
                    return DisasmInstruction { addr, bytes, mnemonic: "MUL".into(), operands: format!("R{}, R{}, R{}", rd, rn, rm), comment: warn };
                }
                // MLA Rd=PC 或 Ra=PC 为 UNPREDICTABLE，添加警告
                let warn = if rd == 15 || ra == 15 {
                    Some("; ⚠ DATA? MLA R15=UNPREDICTABLE (data possibly misread as code)".into())
                } else {
                    Some("; 32-bit multiply-accumulate Rd = Ra + Rn*Rm".into())
                };
                return DisasmInstruction { addr, bytes, mnemonic: "MLA".into(), operands: format!("R{}, R{}, R{}, R{}", rd, rn, rm, ra), comment: warn };
            }
            0x8 => {
                return DisasmInstruction { addr, bytes, mnemonic: "SMULL".into(), operands: format!("R{}, R{}, R{}, R{}", rd, ra, rn, rm), comment: Some("; 64-bit signed multiply [Rd:Ra] = Rn*Rm".into()) };
            }
            0x9 => {
                // SDIV: hw2 = 1111 Rd 1111 Rm（Rd 在 bits[11:8]，非 bits[15:12]）
                // 与 MUL/MLA 不同，SDIV/UDIV 的 bits[15:12] 固定为 1111
                let rd_div = ((hw2 >> 8) & 0xF) as u8;
                return DisasmInstruction { addr, bytes, mnemonic: "SDIV".into(), operands: format!("R{}, R{}, R{}", rd_div, rn, rm), comment: Some("; signed divide Rd = Rn/Rm".into()) };
            }
            0xA => {
                return DisasmInstruction { addr, bytes, mnemonic: "UMULL".into(), operands: format!("R{}, R{}, R{}, R{}", rd, ra, rn, rm), comment: Some("; 64-bit unsigned multiply [Rd:Ra] = Rn*Rm".into()) };
            }
            0xB => {
                // UDIV: hw2 = 1111 Rd 1111 Rm（Rd 在 bits[11:8]，非 bits[15:12]）
                // 与 MUL/MLA 不同，SDIV/UDIV 的 bits[15:12] 固定为 1111
                let rd_div = ((hw2 >> 8) & 0xF) as u8;
                return DisasmInstruction { addr, bytes, mnemonic: "UDIV".into(), operands: format!("R{}, R{}, R{}", rd_div, rn, rm), comment: Some("; unsigned divide Rd = Rn/Rm".into()) };
            }
            0xC => {
                return DisasmInstruction { addr, bytes, mnemonic: "SMLAL".into(), operands: format!("R{}, R{}, R{}, R{}", rd, ra, rn, rm), comment: Some("; 64-bit signed multiply-accumulate [Rd:Ra] += Rn*Rm".into()) };
            }
            0xE => {
                return DisasmInstruction { addr, bytes, mnemonic: "UMLAL".into(), operands: format!("R{}, R{}, R{}, R{}", rd, ra, rn, rm), comment: Some("; 64-bit unsigned multiply-accumulate [Rd:Ra] += Rn*Rm".into()) };
            }
            _ => {}
        }
    }

    // VFP/NEON 协处理器指令 (0xECxx-0xEFxx):
    // 0xECxx: NEON 数据处理 / VLDM/VSTM
    // 0xEDxx: VFP/NEON 加载/存储 (VLDR/VSTR)
    // 0xEExx: VFP 数据处理 (VADD/VSUB/VMUL/VDIV/VMOV/VCMP/VCVT 等)
    // 0xEFxx: NEON 数据处理
    if (hw1 & 0xFF00) >= 0xEC00 && (hw1 & 0xFF00) <= 0xEF00 {
        let category = match (hw1 >> 8) & 0xF {
            0xC | 0xF => "NEON",
            0xD => "VFP.LS",
            0xE => "VFP.DP",
            _ => "VFP",
        };

        // VFP 数据处理字段提取（基于 ARMv7-M ARM A5.3）:
        //   hw1[7]   = bit[23] = op_main (0=算术运算, 1=VDIV/VFMA/VCVT 等)
        //   hw1[6]   = bit[22] = D (Vd 高位 / 半部分选择位)
        //   hw1[5:4] = bit[21:20] = op1
        //   hw1[3:0] = bit[19:16] = Vn[3:0]
        //   hw2[15:12] = bit[15:12] = Vd[3:0]
        //   hw2[11:9]  = bit[11:9]  = 101 (固定, 协处理器)
        //   hw2[8]   = bit[8]  = sz (0=F32 单精度, 1=F64 双精度)
        //   hw2[7]   = bit[7]  = N (Vn 高位 / sx 符号位)
        //   hw2[6]   = bit[6]  = op2[1] (操作码高位, 区分 VADD/VSUB 等)
        //   hw2[5]   = bit[5]  = M (Vm 高位)
        //   hw2[4]   = bit[4]  = 0 (固定)
        //   hw2[3:0] = bit[3:0] = Vm[3:0]
        // 单精度寄存器编号 (VFPv3-D16): Sx = 2*V_lo + half (half = D/N/M 位)
        // 双精度寄存器编号: Dx = (D << 4) | V_lo
        let op_main = (hw1 >> 7) & 1;
        let d_bit = ((hw1 >> 6) & 1) as u8;
        let op1 = (hw1 >> 4) & 0x3;
        let vn_lo = (hw1 & 0xF) as u8;
        let vd_lo = ((hw2 >> 12) & 0xF) as u8;
        let sz = (hw2 >> 8) & 1;
        let n_bit = ((hw2 >> 7) & 1) as u8;
        let op2_hi = (hw2 >> 6) & 1;
        let m_bit = ((hw2 >> 4) & 1) as u8;
        let vm_lo = (hw2 & 0xF) as u8;
        // 单精度寄存器编号 (VFPv3-D16): Sx = 2*V_lo + half
        let sd = 2 * vd_lo + d_bit;
        let sn = 2 * vn_lo + n_bit;
        let sm = 2 * vm_lo + m_bit;
        // 双精度寄存器编号: Dx = (D << 4) | V_lo
        let dd = (d_bit << 4) | vd_lo;
        let dm = (m_bit << 4) | vm_lo;

        // 0xEExx 系列: VFP 数据处理
        if (hw1 & 0xFF00) == 0xEE00 {
            // VMRS: 0xEEF1 0xFA10 (读 FPSCR 到 APSR)
            if hw1 == 0xEEF1 && hw2 == 0xFA10 {
                return DisasmInstruction { addr, bytes, mnemonic: "VMRS".into(), operands: "APSR, FPSCR".into(), comment: Some("; read floating-point status register to APSR(NZCV)".into()) };
            }
            // VMOV (core reg ↔ fp reg): 0xEE00/0xEE10 + hw2 0x?A10/0x?A90
            //   编码: 1110 1110 000 l:1 Vn(4) Rt(4) 1010 N(1) 001 0000
            //   l = hw1[4] (bit[20]): 0 = ARM→VFP (vmov Sn, Rt), 1 = VFP→ARM (vmov Rt, Sn)
            //   Rt = hw2[15:12], Vn_lo = hw1[3:0], N = hw2[7]
            //   Sn = 2*Vn_lo + N
            //   hw2 掩码 0x08F1: 清零 Rt[15:12] 和 N[7], 保留 bit[11:8]=1010, bit[6:5]=00, bit[4]=1, bit[3:0]=0000
            if (hw1 & 0xFFEF) == 0xEE00 && (hw2 & 0x08F1) == 0x0810 {
                let l = (hw1 >> 4) & 1;
                let rt = ((hw2 >> 12) & 0xF) as u8;
                if l == 0 {
                    return DisasmInstruction { addr, bytes, mnemonic: "VMOV".into(), operands: format!("S{}, R{}", sn, rt), comment: Some("; ARM core register → VFP register".into()) };
                } else {
                    return DisasmInstruction { addr, bytes, mnemonic: "VMOV".into(), operands: format!("R{}, S{}", rt, sn), comment: Some("; VFP register → ARM core register".into()) };
                }
            }

            // 算术运算指令 (op_main=0 或 op_main=1 且 op1 != 11)
            // op_main=0: VMLA/VMUL/VADD/VSUB (op1=00/10/11), op2_hi 区分变体
            // op_main=1, op1!=11: VDIV/VFMA/VFNMA
            // 注意: op_main=1, op1=11 是 VCVT 系列, 不在这里处理
            let cop_is_sp = (hw2 & 0x0F00) == 0x0A00;
            if cop_is_sp && (op_main == 0 || op1 != 0b11) {
                // (助记符, 是否三寄存器操作数, 中文语义注释)
                let (mnem, is_3reg, zh): (&str, bool, &str) = match (op_main, op1, op2_hi) {
                    (0, 0b00, 0) => ("VMLA.F32", true, "Single-precision FP multiply-accumulate Sd = Sd + Sn*Sm"),
                    (0, 0b00, 1) => ("VMLS.F32", true, "Single-precision FP multiply-subtract Sd = Sd - Sn*Sm"),
                    (0, 0b10, 0) => ("VMUL.F32", true, "Single-precision FP multiply Sd = Sn*Sm"),
                    (0, 0b10, 1) => ("VNMUL.F32", true, "Single-precision FP negate-multiply Sd = -(Sn*Sm)"),
                    (0, 0b11, 0) => ("VADD.F32", true, "Single-precision FP add Sd = Sn + Sm"),
                    (0, 0b11, 1) => ("VSUB.F32", true, "Single-precision FP subtract Sd = Sn - Sm"),
                    (1, 0b00, 0) => ("VDIV.F32", true, "Single-precision FP divide Sd = Sn / Sm"),
                    (1, 0b01, 0) => ("VFNMA.F32", true, "Single-precision FP negate-multiply-accumulate Sd = -Sd - Sn*Sm"),
                    (1, 0b01, 1) => ("VFNMS.F32", true, "Single-precision FP negate-multiply-subtract-accumulate Sd = -Sd + Sn*Sm"),
                    (1, 0b10, 0) => ("VFMA.F32", true, "Single-precision FP fused multiply-accumulate Sd = Sd + Sn*Sm"),
                    (1, 0b10, 1) => ("VFMS.F32", true, "Single-precision FP fused multiply-subtract Sd = Sd - Sn*Sm"),
                    _ => ("", false, ""),
                };
                if !mnem.is_empty() && is_3reg {
                    return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands: format!("S{}, S{}, S{}", sd, sn, sm), comment: Some(format!("; {}", zh).into()) };
                }
            }

            // op_main=1, op1=11: VMOV/VABS/VNEG/VSQRT/VCMP/VCVT 系列
            // 由 Vn (vn_lo) 和 op2 (bit[6:5]) 区分
            if op_main == 1 && op1 == 0b11 {
                let op2 = (hw2 >> 5) & 0x3;
                match vn_lo {
                    0 => match op2 {
                        0b10 => return DisasmInstruction { addr, bytes, mnemonic: "VMOV.F32".into(), operands: format!("S{}, S{}", sd, sm), comment: Some("; single-precision FP register move".into()) },
                        0b11 => return DisasmInstruction { addr, bytes, mnemonic: "VABS.F32".into(), operands: format!("S{}, S{}", sd, sm), comment: Some("; single-precision FP absolute value".into()) },
                        _ => {}
                    },
                    1 => match op2 {
                        0b10 => return DisasmInstruction { addr, bytes, mnemonic: "VNEG.F32".into(), operands: format!("S{}, S{}", sd, sm), comment: Some("; single-precision FP negate".into()) },
                        0b11 => return DisasmInstruction { addr, bytes, mnemonic: "VSQRT.F32".into(), operands: format!("S{}, S{}", sd, sm), comment: Some("; single-precision FP square root".into()) },
                        _ => {}
                    },
                    4 if op2 == 0b10 => return DisasmInstruction { addr, bytes, mnemonic: "VCMP.F32".into(), operands: format!("S{}, S{}", sd, sm), comment: Some("; single-precision FP compare".into()) },
                    // VCVT between F32 and F64: Vn=7, op2=11
                    //   sz=0 (D=0): VCVT.F64.F32 Dd, Sm
                    //   sz=1 (D=1): VCVT.F32.F64 Sd, Dm
                    7 if op2 == 0b11 => {
                        if sz == 0 {
                            return DisasmInstruction { addr, bytes, mnemonic: "VCVT.F64.F32".into(), operands: format!("D{}, S{}", dd, sm), comment: Some("; single → double precision FP conversion".into()) };
                        } else {
                            return DisasmInstruction { addr, bytes, mnemonic: "VCVT.F32.F64".into(), operands: format!("S{}, D{}", sd, dm), comment: Some("; double → single precision FP conversion".into()) };
                        }
                    }
                    // VCVT int↔float: Vn=8 (int→float), Vn=12 (float→int)
                    //   bit[7] (N): 0=unsigned, 1=signed
                    //   bit[5] (M): Vm 高位
                    8 if op2 >= 0b10 => {
                        let mnem = if n_bit == 0 { "VCVT.F32.U32" } else { "VCVT.F32.S32" };
                        let zh = if n_bit == 0 { "unsigned 32-bit integer → single-precision FP" } else { "signed 32-bit integer → single-precision FP" };
                        return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands: format!("S{}, S{}", sd, sm), comment: Some(format!("; {}", zh).into()) };
                    }
                    12 if op2 >= 0b10 => {
                        let mnem = if n_bit == 0 { "VCVTR.U32.F32" } else { "VCVT.U32.F32" };
                        let zh = if n_bit == 0 { "single-precision FP → unsigned 32-bit integer (rounded)" } else { "single-precision FP → signed 32-bit integer (truncated)" };
                        return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands: format!("S{}, S{}", sd, sm), comment: Some(format!("; {}", zh).into()) };
                    }
                    _ => {}
                }
            }
        }
        // 0xEDxx 系列: VFP 加载/存储 (VLDR/VSTR)
        // 编码: 1110 1101 U D W L Rn | Vd 1010 imm8 (单精度)
        //   U = hw1[7] (bit[23]): 1=正偏移, 0=负偏移
        //   D = hw1[6] (bit[22]): 半部分选择位
        //   L = hw1[4] (bit[20]): 1=加载 (VLDR), 0=存储 (VSTR)
        //   Sd = 2*Vd_lo + D
        if (hw1 & 0xFF00) == 0xED00 {
            let rn = (hw1 & 0xF) as u8;
            let u = (hw1 >> 7) & 1;
            let d = ((hw1 >> 6) & 1) as u8;
            let l = (hw1 >> 4) & 1;
            let vd_lo_vldr = ((hw2 >> 12) & 0xF) as u8;
            let imm8 = hw2 & 0xFF;
            let sd = 2 * vd_lo_vldr + d;
            let offset = (imm8 as u32) * 4;
            let mnem = if l == 1 { "VLDR" } else { "VSTR" };
            let operands = if offset == 0 {
                format!("S{}, [R{}]", sd, rn)
            } else if u == 1 {
                format!("S{}, [R{}, #0x{:X}]", sd, rn, offset)
            } else {
                format!("S{}, [R{}, #-0x{:X}]", sd, rn, offset)
            };
            let zh = if l == 1 { "; single-precision FP register load" } else { "; single-precision FP register store" };
            return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands, comment: Some(zh.into()) };
        }
        // 0xECxx 系列: VLDM/VSTM
        // 位布局: 1110 110P U1W0 Rn(4) | D Vd(4) 1011 imm8
        // 单精度: Vd = D:Vd*2, 数量 = imm8/2
        if (hw1 & 0xFF10) == 0xEC00 {
            let rn = (hw1 & 0xF) as u8;
            let d = ((hw1 >> 7) & 1) as u8;
            let vd = ((d << 4) | ((hw2 >> 12) & 0xF) as u8) as u8;
            let imm8 = (hw2 & 0xFF) as u8;
            let is_load = (hw1 & 0x0010) != 0;
            let mnem = if is_load { "VLDM" } else { "VSTM" };
            let count = imm8 / 2;
            let zh = if is_load { "; batch load single-precision FP registers" } else { "; batch store single-precision FP registers" };
            return DisasmInstruction { addr, bytes, mnemonic: mnem.into(), operands: format!("R{}!, {{S{}-S{}}}", rn, vd, vd + count), comment: Some(zh.into()) };
        }
        // 未分类的 VFP/NEON 指令
        return DisasmInstruction {
            addr, bytes,
            mnemonic: format!("{}.??", category),
            operands: format!("0x{:04X} 0x{:04X}", hw1, hw2),
            comment: Some("; VFP/NEON instruction".into()),
        };
    }

    // 其他未识别的 Thumb-2 指令
    DisasmInstruction {
        addr, bytes,
        mnemonic: ".word".into(),
        operands: format!("0x{:04X} 0x{:04X}", hw1, hw2),
        comment: Some("; Thumb-2 instruction".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nop() {
        let inst = decode_thumb16(0xBF00, 0x08000000);
        assert_eq!(inst.mnemonic, "NOP");
    }

    #[test]
    fn test_mov_imm() {
        let inst = decode_thumb16(0x2001, 0x08000000); // MOVS R0, #1
        assert_eq!(inst.mnemonic, "MOV");
        assert!(inst.operands.contains("R0"));
    }

    #[test]
    fn test_data_proc_imm8() {
        // Thumb-16 立即数数据处理指令（0x2000-0x3FFF）
        // 验证 MOV/CMP/ADD/SUB 四种指令的正确解码
        let mov = decode_thumb16(0x2010, 0x08000000); // MOV R0, #16
        assert_eq!(mov.mnemonic, "MOV");
        assert_eq!(mov.operands, "R0, #16");

        let cmp = decode_thumb16(0x2810, 0x08000000); // CMP R0, #16
        assert_eq!(cmp.mnemonic, "CMP");
        assert_eq!(cmp.operands, "R0, #16");

        let add = decode_thumb16(0x3010, 0x08000000); // ADD R0, #16
        assert_eq!(add.mnemonic, "ADD");
        assert_eq!(add.operands, "R0, #16");

        let sub = decode_thumb16(0x3810, 0x08000000); // SUB R0, #16
        assert_eq!(sub.mnemonic, "SUB");
        assert_eq!(sub.operands, "R0, #16");

        // 验证高位寄存器编号也正确（R4-R7）
        let cmp_r4 = decode_thumb16(0x2C10, 0x08000000); // CMP R4, #16 (0x2C10)
        assert_eq!(cmp_r4.mnemonic, "CMP");
        assert_eq!(cmp_r4.operands, "R4, #16");

        let cmp_r6 = decode_thumb16(0x2E05, 0x08000000); // CMP R6, #5 (0x2E05)
        assert_eq!(cmp_r6.mnemonic, "CMP");
        assert_eq!(cmp_r6.operands, "R6, #5");

        let add_r1 = decode_thumb16(0x3110, 0x08000000); // ADD R1, #16 (0x3110)
        assert_eq!(add_r1.mnemonic, "ADD");
        assert_eq!(add_r1.operands, "R1, #16");
    }

    #[test]
    fn test_b_unconditional() {
        let inst = decode_thumb16(0xE7FD, 0x08000000); // B (infinite loop)
        assert_eq!(inst.mnemonic, "B");
    }

    #[test]
    fn test_push_pop() {
        let push = decode_thumb16(0xB500, 0x08000000); // PUSH {LR}
        assert_eq!(push.mnemonic, "PUSH");

        let pop = decode_thumb16(0xBD00, 0x08000000); // POP {PC}
        assert_eq!(pop.mnemonic, "POP");
    }

    #[test]
    fn test_bl() {
        let inst = disassemble_thumb(&[0x00, 0xF0, 0x00, 0xF8], 0x08000000, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "BL");
    }

    #[test]
    fn test_bl_negative_offset_no_overflow() {
        // 向后跳转（S=1 负偏移）：旧实现 addr + 4 + offset 为 u32 加法，
        // debug 构建下必 panic（attempt to add with overflow）
        let inst = disassemble_thumb(&[0x00, 0xF4, 0x00, 0xF8], 0x08000000, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "BL");
        assert_eq!(inst[0].operands, "0x07C00004");
    }

    #[test]
    fn test_ldr_w_literal() {
        // LDR.W SP, [PC, #16]: 0xF8DF 0xD010
        // 这个测试确保 LDR.W (literal) 的掩码正确（曾经有 Bug: 0xF8DF & 0xFF7F == 0xF85F 而非 0xF8DF）
        let inst = disassemble_thumb(&[0xDF, 0xF8, 0x10, 0xD0], 0x08000150, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "LDR.W");
        assert!(inst[0].operands.contains("SP"), "应该解码 SP 寄存器");
        assert!(inst[0].operands.contains("PC"), "应该是 PC 相对寻址");
    }

    #[test]
    fn test_nop_w() {
        // NOP.W: 0xF3AF 0x8000
        // 这个测试确保 NOP.W 不被 B.W (conditional) 误匹配
        let inst = disassemble_thumb(&[0xAF, 0xF3, 0x00, 0x80], 0x08000160, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "NOP.W", "0xF3AF 0x8000 应该是 NOP.W，不是 B.W");
    }

    #[test]
    fn test_push_w_pop_w() {
        // PUSH.W {R4-R7, LR}: 0xE92D 0x40F0
        let inst = disassemble_thumb(&[0x2D, 0xE9, 0xF0, 0x40], 0x08000000, 0, 10);
        // 函数序言检测不再添加函数标签，直接查找 PUSH.W 指令
        let push_inst = inst.iter().find(|i| i.mnemonic == "PUSH.W").expect("应有 PUSH.W 指令");
        assert!(push_inst.operands.contains("LR"));

        // POP.W {R4-R7, PC}: 0xE8BD 0x80F0
        let inst = disassemble_thumb(&[0xBD, 0xE8, 0xF0, 0x80], 0x08000000, 0, 10);
        let pop_inst = inst.iter().find(|i| i.mnemonic == "POP.W").expect("应有 POP.W 指令");
        assert!(pop_inst.operands.contains("PC"));
    }

    #[test]
    fn test_zero_fill_detection() {
        // 连续 8 字节 0x0000 应该被识别为填充数据，而非 LSL R0, R0, #0
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let inst = disassemble_thumb(&data, 0x08000000, 0, 10);
        // 8字节以上0x0000合并显示为 .fill
        assert!(inst.len() >= 1);
        assert_eq!(inst[0].mnemonic, ".fill", "连续8字节0x0000应合并为.fill");
        assert!(inst[0].operands.contains("8 bytes"), "应显示字节数");
    }

    #[test]
    fn test_zero_fill_4bytes() {
        // 4字节 0x0000 应显示为 .word 0x00000000
        let data = [0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];
        let inst = disassemble_thumb(&data, 0x08000000, 0, 10);
        assert!(inst.len() >= 1);
        assert_eq!(inst[0].mnemonic, ".word", "4字节0x0000应显示为.word");
        assert_eq!(inst[0].operands, "0x00000000");
    }

    #[test]
    fn test_tbb_tbh() {
        // TBB [PC, R0]: 0xE8DF 0x000F (Rn=15=PC, Rm=0)
        // 字节序: DF E8 0F 00
        let inst = disassemble_thumb(&[0xDF, 0xE8, 0x0F, 0x00], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "TBB", "0xE8DF 0x000F 应解码为 TBB，不是 LDRD");
        assert!(inst[0].operands.contains("PC"), "TBB 应使用 PC 寄存器");
        assert!(inst[0].operands.contains("R0"), "TBB 应使用 R0 作为索引");

        // TBH [PC, R0, LSL #1]: 0xE8DF 0x010F (Rn=15=PC, Rm=0, bit[8]=1)
        // 字节序: DF E8 0F 01
        let inst = disassemble_thumb(&[0xDF, 0xE8, 0x0F, 0x01], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "TBH", "0xE8DF 0x010F 应解码为 TBH，不是 LDRD");
        assert!(inst[0].operands.contains("LSL #1"), "TBH 应有 LSL #1 标记");

        // TBB [R1, R2]: 0xE8DF 0x0021 (Rn=1, Rm=2)
        // 字节序: DF E8 21 00
        let inst = disassemble_thumb(&[0xDF, 0xE8, 0x21, 0x00], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "TBB");
        assert!(inst[0].operands.contains("R1") && inst[0].operands.contains("R2"));
    }

    #[test]
    fn test_memory_barriers() {
        // DSB SY: 0xF3BF 0x8F4F
        // 字节序: BF F3 4F 8F
        let inst = disassemble_thumb(&[0xBF, 0xF3, 0x4F, 0x8F], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "DSB");
        assert_eq!(inst[0].operands, "SY");

        // DMB SY: 0xF3BF 0x8F5F
        let inst = disassemble_thumb(&[0xBF, 0xF3, 0x5F, 0x8F], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "DMB");
        assert_eq!(inst[0].operands, "SY");

        // ISB SY: 0xF3BF 0x8F6F
        let inst = disassemble_thumb(&[0xBF, 0xF3, 0x6F, 0x8F], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "ISB");
        assert_eq!(inst[0].operands, "SY");

        // CLREX: 0xF3BF 0x8F2F
        let inst = disassemble_thumb(&[0xBF, 0xF3, 0x2F, 0x8F], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "CLREX");
        assert_eq!(inst[0].operands, "");
    }

    #[test]
    fn test_mrs_msr() {
        // MRS R0, MSP: 0xF3EF 0x8008 (SYSm=0x08=MSP)
        // 字节序: EF F3 00 88 (Rt=0, SYSm=0x08)
        // 注意: hw2 = 0x8008, Rt = (hw2>>8)&0xF = 0x08>>0... wait
        // MRS 编码: 0xF3EF 0x8000 | (Rt<<8) | SYSm
        // 所以 Rt=R0, SYSm=0x08: hw2 = 0x8008
        // 字节序: EF F3 08 80
        let inst = disassemble_thumb(&[0xEF, 0xF3, 0x08, 0x80], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "MRS");
        assert!(inst[0].operands.contains("R0"));

        // MSR MSP, R0: 0xF380 0x8008 (SYSm=0x08=MSP, Rt=0)
        // 字节序: 80 F3 08 80
        let inst = disassemble_thumb(&[0x80, 0xF3, 0x08, 0x80], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "MSR");
        assert!(inst[0].operands.contains("R0"));
    }

    #[test]
    fn test_ldrd_r15_suspicious() {
        // LDRD R15, R0, [R15, #0]: 0xE8DF 0xF000
        // 这种编码在用户代码中极罕见，通常是数据被误解码为指令
        // 字节序: DF E8 00 F0
        let inst = disassemble_thumb(&[0xDF, 0xE8, 0x00, 0xF0], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, ".word", "LDRD R15 应标记为可疑数据");
        assert!(inst[0].comment.as_ref().unwrap().contains("suspicious data"),
               "应有可疑数据注释");
    }

    #[test]
    fn test_tbb_jump_table_tracking() {
        // TBB [PC, R0] + 跳转表数据
        // TBB 指令: 0xE8DF 0x000F (Rn=15=PC, Rm=0)
        // 字节序: DF E8 0F 00
        // 跳转表起始: 0x08000104 (PC+4)
        // 跳转表条目: 0x02, 0x04, 0x06, 0x08 (每个1字节，目标 = PC+4 + entry*2)
        //   entry=0x02 -> 0x08000108
        //   entry=0x04 -> 0x0800010C
        //   entry=0x06 -> 0x08000110
        //   entry=0x08 -> 0x08000114
        // 然后是 PUSH {LR} (0xB500) 作为函数入口，终止跳转表扫描
        let data = [
            0xDF, 0xE8, 0x0F, 0x00,  // TBB [PC, R0]
            0x02, 0x04, 0x06, 0x08,  // 跳转表数据 (4条目)
            0x00, 0xB5,              // PUSH {LR} (函数入口，终止扫描)
            0x00, 0xBF,              // NOP
        ];
        let inst = disassemble_thumb_full(&data, 0x08000100, 0, 20);
        // 应该有 TBB 指令 + 跳转表数据 + PUSH + NOP
        let tbb = inst.iter().find(|i| i.mnemonic == "TBB");
        assert!(tbb.is_some(), "应解码出 TBB 指令");
        let jt = inst.iter().find(|i| i.mnemonic == ".byte" && i.comment.as_ref().map(|c| c.contains("jump table")).unwrap_or(false));
        assert!(jt.is_some(), "应识别跳转表数据");
        let jt_inst = jt.unwrap();
        assert!(jt_inst.comment.as_ref().unwrap().contains("TBB"), "注释应标明 TBB 类型");
        assert!(jt_inst.comment.as_ref().unwrap().contains("0x08000100"), "注释应标明来源指令地址");
    }

    #[test]
    fn test_tbh_jump_table_tracking() {
        // TBH [PC, R0, LSL #1] + 跳转表数据
        // TBH 指令: 0xE8DF 0x010F (Rn=15=PC, Rm=0, bit[8]=1)
        // 字节序: DF E8 0F 01
        // 跳转表起始: 0x08000104 (PC+4)
        // 跳转表条目: 0x0002, 0x0004, 0x0006 (每个2字节，目标 = PC+4 + entry*2)
        let data = [
            0xDF, 0xE8, 0x0F, 0x01,  // TBH [PC, R0, LSL #1]
            0x02, 0x00, 0x04, 0x00, 0x06, 0x00,  // 跳转表数据 (3条目×2字节)
            0x00, 0xB5,              // PUSH {LR} (函数入口，终止扫描)
        ];
        let inst = disassemble_thumb_full(&data, 0x08000100, 0, 20);
        let tbh = inst.iter().find(|i| i.mnemonic == "TBH");
        assert!(tbh.is_some(), "应解码出 TBH 指令");
        let jt = inst.iter().find(|i| i.mnemonic == ".byte" && i.comment.as_ref().map(|c| c.contains("jump table")).unwrap_or(false));
        assert!(jt.is_some(), "应识别 TBH 跳转表数据");
        assert!(jt.unwrap().comment.as_ref().unwrap().contains("TBH"), "注释应标明 TBH 类型");
    }

    #[test]
    fn test_pld_pli_prefetch_decode() {
        // PLD [R0, #4]: 0xF890 0xF004（与 LDRB.W 同编码，Rt=1111 区分预取）
        let inst = disassemble_thumb(&[0x90, 0xF8, 0x04, 0xF0], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "PLD");
        assert_eq!(inst[0].operands, "[R0, #4]");

        // PLI [R0, #8]: 0xF990 0xF008
        let inst = disassemble_thumb(&[0x90, 0xF9, 0x08, 0xF0], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "PLI");
        assert_eq!(inst[0].operands, "[R0, #8]");
    }

    #[test]
    fn test_pld_does_not_swallow_ldrb() {
        // LDRB.W R2, [R0, #1]: 0xF890 0x2301（Rt=2，不是预取指令）
        let inst = disassemble_thumb(&[0x90, 0xF8, 0x01, 0x23], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "LDRB.W");
        assert_eq!(inst[0].operands, "R2, [R0, #1]");
    }

    #[test]
    fn test_ldrd_strd_addressing_forms() {
        // LDRD R2, R3, [R1, #16]: 0xE9D1 0x2304（P=1,U=1,W=1 形式）
        let inst = disassemble_thumb(&[0xD1, 0xE9, 0x04, 0x23], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "LDRD");
        assert_eq!(inst[0].operands, "R2, R3, [R1, #16]");

        // STRD R2, R3, [R1, #16]: 0xE9C1 0x2304
        let inst = disassemble_thumb(&[0xC1, 0xE9, 0x04, 0x23], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "STRD");
        assert_eq!(inst[0].operands, "R2, R3, [R1, #16]");

        // LDRD R2, R3, [R1, #16] 的另一寻址形式（P=0,U=0,W=0）: 0xE851 0x2304
        let inst = disassemble_thumb(&[0x51, 0xE8, 0x04, 0x23], 0x08000100, 0, 1);
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].mnemonic, "LDRD");
        assert_eq!(inst[0].operands, "R2, R3, [R1, #16]");
    }
}
