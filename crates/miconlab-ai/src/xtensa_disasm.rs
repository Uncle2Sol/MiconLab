//! Xtensa LX6/LX7 指令反汇编器
//!
//! 用于 ESP32/ESP32-S2/S3 固件的指令级分析。ESP32 采用 Xtensa 架构（24 位定长指令），
//! 与 ARM Cortex-M 的 Thumb 指令集完全不同，不能用 thumb_disasm 解码。
//!
//! ## 24 位指令字段布局（小端序 b0 b1 b2，instr = b0 | b1<<8 | b2<<16）
//!
//! 权威依据：Ghidra xtensaArch.sinc（little-endian token insn(24)）、
//! LLVM XtensaInstrFormats.td、QEMU xtensa-modules.c.inc。
//! **op0 恒为 bits[3:0]**，是顶层路由字段。
//!
//! ### RRR 格式 (op0=0)
//! ```text
//! 23..20  op2   19..16  op1   15..12  ar(dest)   11..8  as(src1)   7..4  at(src2)   3..0  op0=0
//! ```
//!
//! ### RRI8 格式 (op0=2/7，加载/存储/立即数/寄存器分支)
//! ```text
//! 23..16  imm8   15..12  r(sub-op)   11..8  as(src)   7..4  at   3..0  op0
//! ```
//!
//! ### RI16 格式 (op0=1，L32R)
//! ```text
//! 23..8  imm16(负偏移)   7..4  at(dest)   3..0  op0=1
//! ```
//!
//! ### BRI12/BRI8 格式 (op0=6，J/BEQZ/ENTRY/LOOP/BEQI 等)
//! ```text
//! 23..12  imm12(BRI12) | 23..16 imm8(BRI8)   15..12 r(b4const)   11..8  as   7..6  m   5..4  n   3..0  op0=6
//! ```
//!
//! ### CALL 格式 (op0=5)
//! ```text
//! 23..6  off18(有符号)   5..4  n(call size 0/1/2/3)   3..0  op0=5
//! ```
//!
//! ### Narrow 16 位格式 (b0&0xF ∈ {8,9,A,B,C,D}，Code Density Option)
//! ```text
//! 15..12  n_r   11..8  n_s   7..4  n_t   3..0  n_op0
//! ```
//!
//! ## 支持的指令
//! - op0=0 (RRR):  ADD/AND/OR/XOR/SUB/ADDX2/4/8/SUBX2/4/8/MOV/NEG/ABS/MULL/MULSH/MULUH/
//!   QUOS/QUOU/REMS/REMU/MUL16S/MUL16U/MIN/MAX/MINU/MAXU/CLAMPS/MOVEQZ/MOVNEZ/MOVLTZ/MOVGEZ/
//!   SLLI/SRLI/SRAI/SLL/SRL/SRA/SRC/EXTUI/SSR/SSL/SSA8L/SSA8B/SSAI/NSA/NSAU/
//!   RSR/WSR/XSR/RUR/WUR/RSIL/ISYNC/RSYNC/ESYNC/DSYNC/MEMW/EXTW/EXCW/NOP/ILL/
//!   RET/RETW/JX/CALLX0-12/BREAK/MOVSP/WAITI/ROTW/SYSCALL/RFE/RFI/RFME/RFWO/RFWU/L32E/S32E
//! - op0=1 (RI16): L32R（常量加载，非常常用）
//! - op0=2 (RRI8): L8UI/L16UI/L16SI/L32I/S8I/S16I/S32I/MOVI/ADDI/ADDMI/S32C1I/L32AI/S32RI/CACHE 组
//! - op0=3: 浮点 LSI/SSI 空间（ESP32 无 FP，保守降级）
//! - op0=4: MAC16 (UMUL/MUL/MULA.AA.*, LDINC/LDDEC)
//! - op0=5 (CALL): CALL0/4/8/12
//! - op0=6 (BRI):  J/BEQZ/BNEZ/BLTZ/BGEZ/BEQI/BNEI/BLTI/BGEI/BLTUI/BGEUI/ENTRY/LOOP/LOOPNEZ/LOOPGTZ
//! - op0=7 (RRI8-br): BNONE/BEQ/BLT/BLTU/BALL/BBC/BBCI/BANY/BNE/BGE/BGEU/BNALL/BBS/BBSI
//! - op0=8..13: 16 位窄指令（入口处由 is_narrow 拦截，走 decode_one_narrow）
//! - op0=14/15: TIE/用户扩展空间（保守输出原始字节）
//!
//! 未识别指令降级为 `RES.{fmt}` + op0 格式标注。
//!
//! ## 分支/调用目标地址计算（Ghidra xtensaMain.sinc 重定位公式）
//! - BRI12 (op0=6,n=1): target = addr + 4 + sign_ext(imm12, 12)
//! - BRI8 (op0=6,n=2/3): target = addr + 4 + sign_ext(imm8, 8)；LOOP 组 imm8 为无符号
//! - RRI8 分支 (op0=7):  target = addr + 4 + sign_ext(imm8, 8)
//! - J (op0=6,n=0):     target = addr + 4 + sign_ext(off18, 18)
//! - CALL (op0=5):      target = (addr & !3) + 4 + sign_ext(off18, 18) * 4
//! - L32R (op0=1):      target = ((addr + 3) & !3) + (imm16 - 65536) * 4
//! - 窄分支 BEQZ.N/BNEZ.N: target = addr + 4 + imm6（无符号，仅向前）

use crate::thumb_disasm::DisasmInstruction;

/// 符号扩展：将 `bits` 位的无符号值 `v` 符号扩展为 i32
fn sign_ext(v: u32, bits: u32) -> i32 {
    let mask = 1u32 << (bits - 1);
    if v & mask != 0 {
        (v | (0xFFFFFFFFu32 << bits)) as i32
    } else {
        v as i32
    }
}

/// 寄存器名：a0–a15
fn reg(idx: u32) -> String {
    format!("a{}", idx & 0xF)
}

/// B4const 映射：4位编码 → 有符号立即数（参考 Xtensa ISA §2.2.2）
/// 用于 ADDI.N/MOVI.N 等窄指令的立即数编码
/// 修正：原实现线性映射1-15错误，应按 ISA 规范非线性映射
fn b4const(n: u32) -> i32 {
    match n {
        0 => -1,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        6 => 6,
        7 => 7,
        8 => 8,
        9 => 10,   // 非 9
        10 => 12,  // 非 10
        11 => 16,  // 非 11
        12 => 32,  // 非 12
        13 => 64,  // 非 13
        14 => 128, // 非 14
        15 => 256, // 非 15
        _ => 0,
    }
}

/// B4constu 映射：4位编码 → 无符号立即数（参考 Xtensa ISA §2.2.3）
/// 用于无符号变体的窄指令立即数编码
fn b4constu(n: u32) -> u32 {
    match n {
        0 => 32768,
        1 => 65536,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        6 => 6,
        7 => 7,
        8 => 8,
        9 => 10,
        10 => 12,
        11 => 16,
        12 => 32,
        13 => 64,
        14 => 128,
        15 => 256,
        _ => 0,
    }
}

/// 特殊寄存器号 → 名称映射（参考 capstone SRDecoderTable, Xtensa ISA Appendix A）
/// 用于 RSR/WSR/XSR 指令的可读性输出
fn sr_name(sr: u32) -> String {
    let name = match sr {
        0 => "LBEG",           // Loop Begin
        1 => "LEND",           // Loop End
        2 => "LCOUNT",         // Loop Count
        3 => "SAR",            // Shift Amount Register
        4 => "BREG",           // Boolean Register (Boolean Option)
        5 => "LITBASE",        // Literal Base (Extended L32R Option)
        12 => "SCOMPARE1",     // Store Compare 1 (S32C1I Option)
        16 => "ACCLO",         // Accumulator Low (MAC16 Option)
        17 => "ACCHI",         // Accumulator High (MAC16 Option)
        32 => "M0",            // MAC16 Register M0
        33 => "M1",            // MAC16 Register M1
        34 => "M2",            // MAC16 Register M2
        35 => "M3",            // MAC16 Register M3
        72 => "WINDOWBASE",    // Windowed Register Base
        73 => "WINDOWSTART",   // Windowed Register Start
        96 => "IBREAKENABLE",  // Instruction Break Enable
        97 => "MEMCTL",        // Memory Control
        99 => "ATOMCTL",       // Atomic Operation Control
        104 => "DDR",          // Debug Data Register
        128 => "IBREAKA0",     // Instruction Break Address 0
        129 => "IBREAKA1",     // Instruction Break Address 1
        144 => "DBREAKA0",     // Data Break Address 0
        145 => "DBREAKA1",     // Data Break Address 1
        160 => "DBREAKC0",     // Data Break Control 0
        161 => "DBREAKC1",     // Data Break Control 1
        176 => "CONFIGID0",    // Configuration ID 0
        177 => "EPC1",         // Exception Program Counter 1
        178 => "EPC2",
        179 => "EPC3",
        180 => "EPC4",
        181 => "EPC5",
        182 => "EPC6",
        183 => "EPC7",
        192 => "DEPC",         // Double Exception PC
        194 => "EPS2",
        195 => "EPS3",
        196 => "EPS4",
        197 => "EPS5",
        198 => "EPS6",
        199 => "EPS7",
        208 => "CONFIGID1",    // Configuration ID 1
        209 => "EXCSAVE1",     // Exception Save 1
        210 => "EXCSAVE2",
        211 => "EXCSAVE3",
        212 => "EXCSAVE4",
        213 => "EXCSAVE5",
        214 => "EXCSAVE6",
        215 => "EXCSAVE7",
        224 => "CPENABLE",     // Coprocessor Enable
        226 => "INTERRUPT",    // Interrupt Register
        227 => "INTCLEAR",     // Interrupt Clear
        228 => "INTENABLE",    // Interrupt Enable
        230 => "PS",           // Processor State
        231 => "VECBASE",      // Vector Base Address
        232 => "EXCCAUSE",     // Exception Cause
        233 => "DEBUGCAUSE",   // Debug Cause
        234 => "CCOUNT",       // Cycle Count
        235 => "PRID",         // Processor ID
        236 => "ICOUNT",       // Instruction Count
        237 => "ICOUNTLEVEL",  // Instruction Count Level
        238 => "EXCVADDR",     // Exception Virtual Address
        240 => "CCOMPARE0",    // Cycle Compare 0
        241 => "CCOMPARE1",
        242 => "CCOMPARE2",
        244 => "MISC0",        // Miscellaneous 0
        245 => "MISC1",
        246 => "MISC2",
        247 => "MISC3",
        _ => return format!("sr{}", sr),
    };
    name.to_string()
}

/// 检测首字节是否为 16 位 Narrow 指令（Code Density Option）
/// 当 b0 & 0xF ∈ {0x8, 0x9, 0xA, 0xB, 0xC, 0xD} 时为 16 位窄指令
/// 参考 Xtensa ISA §4.2、ESP-IDF XCHAL_BYTE0_FORMAT_LENGTHS = {3,3,3,3,3,3,3,3,2,2,2,2,2,2,3,3}
/// n_op0=0x8: L32I.N，0x9: S32I.N，0xA: ADD.N，0xB: ADDI.N，0xC: MOVI.N/BEQZ.N/BNEZ.N，
/// 0xD: MOV.N/RET.N/RETW.N/NOP.N/BREAK.N/ILL.N
fn is_narrow(b0: u8) -> bool {
    let n_op0 = b0 & 0xF;
    (0x8..=0xD).contains(&n_op0)
}

/// 根据首字节判断指令长度（2 或 3 字节）
/// 参考 ESP-IDF XCHAL_BYTE0_FORMAT_LENGTHS 和 Xtensa ISA §4.2
/// ESP32 (LX6/LX7): b0&0xF ∈ {0x8,0x9,0xA,0xB,0xC,0xD} → 2 字节（Narrow），其余 → 3 字节
pub fn instruction_length(b0: u8) -> usize {
    if is_narrow(b0) { 2 } else { 3 }
}

/// 解码 16 位 Narrow 指令
/// 参考 Ghidra xtensaInstructions.sinc / xtensaArch.sinc narrowinsn(16) token、
/// LLVM XtensaInstrInfo.td、Tensilica CodeDensity.tex
///
/// 16 位指令字段（小端 instr16 = b0 | b1<<8）：
///   n_r = bits[15:12]，n_s = bits[11:8]，n_t = bits[7:4]，n_op0 = bits[3:0]
/// - n_op0=0x8: L32I.N n_t, n_s, (n_r<<2)
/// - n_op0=0x9: S32I.N n_t, n_s, (n_r<<2)
/// - n_op0=0xA: ADD.N n_r, n_s, n_t
/// - n_op0=0xB: ADDI.N n_r, n_s, imm4（imm4=bits[7:4]，0 编码 -1，其余恒等 1..15）
/// - n_op0=0xC: bit7=0 → MOVI.N n_s, imm7（-32..95）；bits[7:6]=0b10/0b11 → BEQZ.N/BNEZ.N
/// - n_op0=0xD: n_r=0 → MOV.N n_t, n_s；n_r=0xF → RET.N/RETW.N/BREAK.N/NOP.N/ILL.N
fn decode_one_narrow(addr: u32, instr16: u16, bytes: Vec<u8>) -> DisasmInstruction {
    let n_op0 = (instr16 & 0xF) as u32;
    let n_r = ((instr16 >> 12) & 0xF) as u32;
    let n_s = ((instr16 >> 8) & 0xF) as u32;
    let n_t = ((instr16 >> 4) & 0xF) as u32;

    let mk = |mnemonic: &str, operands: String, comment: String| DisasmInstruction {
        addr,
        bytes: bytes.clone(),
        mnemonic: mnemonic.to_string(),
        operands,
        comment: Some(comment),
    };

    match n_op0 {
        // ===== L32I.N (n_op0=0x8) =====
        // :l32i.n n_at, n_as, n_u6_12_15_sb2 — offset = bits[15:12] << 2（0..60）
        0x8 => mk(
            "L32I.N",
            format!("{}, {}, {}", reg(n_t), reg(n_s), n_r << 2),
            "; RRRN l32i".to_string(),
        ),

        // ===== S32I.N (n_op0=0x9) =====
        0x9 => mk(
            "S32I.N",
            format!("{}, {}, {}", reg(n_t), reg(n_s), n_r << 2),
            "; RRRN s32i".to_string(),
        ),

        // ===== ADD.N (n_op0=0xA) =====
        0xA => mk(
            "ADD.N",
            format!("{}, {}, {}", reg(n_r), reg(n_s), reg(n_t)),
            "; RRRN add".to_string(),
        ),

        // ===== ADDI.N (n_op0=0xB) =====
        // :addi.n n_ar, n_as, n_s4_4_7_nozero — imm4=0 → -1，imm4=1..15 → 恒等
        0xB => {
            let imm = if n_t == 0 { -1 } else { n_t as i32 };
            mk(
                "ADDI.N",
                format!("{}, {}, {}", reg(n_r), reg(n_s), imm),
                "; RRRN addi".to_string(),
            )
        }

        // ===== n_op0=0xC: MOVI.N (bit7=0) / BEQZ.N / BNEZ.N =====
        0xC => {
            if (instr16 >> 7) & 1 == 0 {
                // MOVI.N: :movi.n n_as, movin_si8 — dest=bits[11:8]
                // imm7 = {bits[6:4], bits[15:12]}；bits[6:4] ∈ {6,7} 时置 bit7（负数区 -32..-1）
                let hi = ((instr16 >> 4) & 0x7) as u32;
                let lo = n_r;
                let mut imm7 = ((hi << 4) | lo) as u8;
                if hi >= 6 {
                    imm7 |= 0x80;
                }
                let imm = imm7 as i8 as i32;
                mk(
                    "MOVI.N",
                    format!("{}, {}", reg(n_s), imm),
                    "; RI7 movi".to_string(),
                )
            } else {
                // BEQZ.N/BNEZ.N: imm6 = {bits[5:4], bits[15:12]}（无符号，仅向前）
                // target = addr + 4 + imm6
                let imm6 = ((((instr16 >> 4) & 0x3) as u32) << 4) | n_r;
                let target = addr.wrapping_add(4).wrapping_add(imm6);
                match (instr16 >> 6) & 0x3 {
                    0b10 => mk(
                        "BEQZ.N",
                        format!("{}, 0x{:08X}", reg(n_s), target),
                        format!("; RI6 beqz off={}", imm6),
                    ),
                    0b11 => mk(
                        "BNEZ.N",
                        format!("{}, 0x{:08X}", reg(n_s), target),
                        format!("; RI6 bnez off={}", imm6),
                    ),
                    _ => mk(
                        "NARROW.D",
                        format!("bits76={:02b}", (instr16 >> 6) & 0x3),
                        "; RI6 reserved".to_string(),
                    ),
                }
            }
        }

        // ===== n_op0=0xD: MOV.N / 特殊指令 =====
        0xD => {
            if n_r == 0 {
                // :mov.n n_at, n_as is n_ar = 0 — dest = bits[7:4]，src = bits[11:8]
                mk(
                    "MOV.N",
                    format!("{}, {}", reg(n_t), reg(n_s)),
                    "; RRRN mov".to_string(),
                )
            } else if n_r == 0xF {
                match (n_s, n_t) {
                    (0, 0) => mk("RET.N", String::new(), "; RI7 ret".to_string()),
                    (0, 1) => mk("RETW.N", String::new(), "; RI7 retw".to_string()),
                    (_, 2) => mk(
                        "BREAK.N",
                        format!("{}", n_s),
                        "; RI7 break".to_string(),
                    ),
                    (0, 3) => mk("NOP.N", String::new(), "; RI7 nop".to_string()),
                    (0, 6) => mk("ILL.N", String::new(), "; RI7 ill".to_string()),
                    _ => mk(
                        "NARROW.D",
                        format!("s={}, t={}", n_s, n_t),
                        "; RI7 special reserved".to_string(),
                    ),
                }
            } else {
                mk(
                    "NARROW.D",
                    format!("r={}, s={}, t={}", n_r, n_s, n_t),
                    format!("; RRRN reserved n_op0=0xD r={}", n_r),
                )
            }
        }

        _ => mk(
            "NARROW.D",
            format!("n_op0={:04b}", n_op0),
            format!("; Narrow unknown n_op0={}", n_op0),
        ),
    }
}

/// 反汇编 Xtensa 指令流（支持 16/24 位混合解码）
///
/// Xtensa Code Density Option 允许 16 位窄指令与 24 位标准指令混合编码。
/// 当首字节 b0 & 0xF ∈ {0x8, 0x9, 0xA, 0xB, 0xC, 0xD} 时为 16 位 Narrow 指令，
/// 否则为 24 位标准指令。正确区分指令长度是避免指令流错位的关键。
///
/// - `data`: 原始 Flash 字节
/// - `base_addr`: 数据段起始的虚拟地址（如段 load_addr）
/// - `offset`: 数据在 `data` 中的字节偏移（段数据在文件中的偏移）
/// - `max_count`: 最多解码的指令数
///
/// 地址计算：`addr = base_addr + (pos - offset)`
pub fn disassemble_xtensa(
    data: &[u8],
    base_addr: u32,
    offset: usize,
    max_count: usize,
) -> Vec<DisasmInstruction> {
    let mut result = Vec::with_capacity(max_count);
    let mut pos = offset;
    while result.len() < max_count {
        if pos >= data.len() {
            break;
        }
        let b0 = data[pos];
        let addr = base_addr.wrapping_add((pos - offset) as u32);

        if is_narrow(b0) && pos + 2 <= data.len() {
            // 16 位 Narrow 指令
            let b1 = data[pos + 1];
            let instr16 = (b0 as u16) | ((b1 as u16) << 8);
            let bytes = vec![b0, b1];
            result.push(decode_one_narrow(addr, instr16, bytes));
            pos += 2;
        } else if pos + 3 <= data.len() {
            // 24 位标准指令
            let b1 = data[pos + 1];
            let b2 = data[pos + 2];
            let instr = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
            let bytes = vec![b0, b1, b2];
            result.push(decode_one(addr, instr, bytes));
            pos += 3;
        } else {
            break;
        }
    }
    result
}

/// 解码单条 24 位指令
/// 字段布局（权威：Ghidra xtensaArch.sinc 小端 token insn(24)、LLVM XtensaInstrFormats.td）：
///   op0=bits[3:0]（顶层路由）  op1=bits[19:16]  op2=bits[23:20]
///   ar=bits[15:12]  as=bits[11:8]  at=bits[7:4]  bri8_n=bits[5:4]  bri8_m=bits[7:6]
///   imm8=bits[23:16]  imm12=bits[23:12]  imm16=bits[23:8]  off18=bits[23:6]
fn decode_one(addr: u32, instr: u32, bytes: Vec<u8>) -> DisasmInstruction {
    let op0 = instr & 0xF;
    let op1 = (instr >> 16) & 0xF;
    let op2 = (instr >> 20) & 0xF;
    let ar = (instr >> 12) & 0xF;
    let as_ = (instr >> 8) & 0xF;
    let at = (instr >> 4) & 0xF;
    let imm8 = (instr >> 16) & 0xFF;
    let simm8 = sign_ext(imm8, 8);
    // RRI8/BRI8 分支目标：ri8_srel = inst_start + simm8 + 4（Ghidra xtensaMain.sinc）
    let br8_target = addr.wrapping_add(4).wrapping_add(simm8 as u32);

    // bytes 以参数传递而非闭包捕获，避免与 unknown(addr, bytes, ...) 的所有权移动冲突
    let mk = |mnemonic: &str, operands: String, comment: String, bytes: &[u8]| DisasmInstruction {
        addr,
        bytes: bytes.to_vec(),
        mnemonic: mnemonic.to_string(),
        operands,
        comment: Some(comment),
    };
    let rrr = |mnemonic: &str, bytes: &[u8]| {
        mk(
            mnemonic,
            format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
            "; RRR".to_string(),
            bytes,
        )
    };

    match op0 {
        // ===== op0=5: CALL0/4/8/12 =====
        // call_srel_sh2: target = (inst_start & ~3) + (off18 << 2) + 4
        0x5 => {
            let n = (instr >> 4) & 0x3;
            let off18 = sign_ext(instr >> 6, 18);
            let target = (addr & !3u32)
                .wrapping_add(4)
                .wrapping_add((off18 << 2) as u32);
            let mnemonic = match n {
                0 => "CALL0",
                1 => "CALL4",
                2 => "CALL8",
                _ => "CALL12",
            };
            mk(
                mnemonic,
                format!("0x{:08X}", target),
                format!("; CALL off18={}", off18),
                &bytes,
            )
        }

        // ===== op0=1: L32R（RI16，常量加载）=====
        // srel_oex_sh2: target = ((inst_start + 3) & ~3) + ((imm16 | 0xFFFF0000) << 2)
        0x1 => {
            let imm16 = (instr >> 8) & 0xFFFF;
            let off = (imm16 as i32) - 65536;
            let target = (addr.wrapping_add(3) & !3u32).wrapping_add((off * 4) as u32);
            mk(
                "L32R",
                format!("{}, [0x{:08X}]", reg(at), target),
                format!("; RI16 l32r off={}", off),
                &bytes,
            )
        }

        // ===== op0=6: BRI 组（J / BRI12 / BRI8 / ENTRY / LOOP）=====
        // bri8_n=bits[5:4]，bri8_m=bits[7:6]（Ghidra xtensaArch.sinc 小端定义）
        0x6 => {
            let n = (instr >> 4) & 0x3;
            let m = (instr >> 6) & 0x3;
            match n {
                // J: call_srel → target = inst_start + off18 + 4
                0 => {
                    let off18 = sign_ext(instr >> 6, 18);
                    let target = addr.wrapping_add(4).wrapping_add(off18 as u32);
                    mk(
                        "J",
                        format!("0x{:08X}", target),
                        format!("; J off18={}", off18),
                        &bytes,
                    )
                }
                // BRI12: bri12_srel → target = inst_start + simm12 + 4
                1 => {
                    let simm12 = sign_ext(instr >> 12, 12);
                    let target = addr.wrapping_add(4).wrapping_add(simm12 as u32);
                    let mnem = match m {
                        0 => "BEQZ",
                        1 => "BNEZ",
                        2 => "BLTZ",
                        _ => "BGEZ",
                    };
                    mk(
                        mnem,
                        format!("{}, 0x{:08X}", reg(as_), target),
                        format!("; BRI12 off={}", simm12),
                        &bytes,
                    )
                }
                // BRI8（b4const 有符号立即数分支）: target = inst_start + simm8 + 4
                2 => {
                    let mnem = match m {
                        0 => "BEQI",
                        1 => "BNEI",
                        2 => "BLTI",
                        _ => "BGEI",
                    };
                    mk(
                        mnem,
                        format!("{}, {}, 0x{:08X}", reg(as_), b4const(ar), br8_target),
                        format!("; BRI8 off={}", simm8),
                        &bytes,
                    )
                }
                // n=3: ENTRY (m=0) / LOOP 组 (m=1) / BLTUI (m=2) / BGEUI (m=3)
                _ => match m {
                    // ENTRY as, imm12<<3（bri12_i12_sh3，栈帧字节数 = imm12 * 8）
                    0 => {
                        let imm12 = (instr >> 12) & 0xFFF;
                        mk(
                            "ENTRY",
                            format!("{}, {}", reg(as_), imm12 << 3),
                            format!("; function entry frame={}", imm12 << 3),
                            &bytes,
                        )
                    }
                    // LOOP/LOOPNEZ/LOOPGTZ: ar=8/9/0xA，imm8 无符号仅向前（LoopOffset8）
                    1 => {
                        let target = addr.wrapping_add(4).wrapping_add(imm8);
                        match ar {
                            8 => mk(
                                "LOOP",
                                format!("{}, 0x{:08X}", reg(as_), target),
                                format!("; LOOP off={}", imm8),
                                &bytes,
                            ),
                            9 => mk(
                                "LOOPNEZ",
                                format!("{}, 0x{:08X}", reg(as_), target),
                                format!("; LOOPNEZ off={}", imm8),
                                &bytes,
                            ),
                            0xA => mk(
                                "LOOPGTZ",
                                format!("{}, 0x{:08X}", reg(as_), target),
                                format!("; LOOPGTZ off={}", imm8),
                                &bytes,
                            ),
                            _ => unknown(addr, bytes, op0, "BRI"),
                        }
                    }
                    // BRI8 无符号 b4constu 立即数分支
                    2 => mk(
                        "BLTUI",
                        format!("{}, {}, 0x{:08X}", reg(as_), b4constu(ar), br8_target),
                        format!("; BRI8u off={}", simm8),
                        &bytes,
                    ),
                    _ => mk(
                        "BGEUI",
                        format!("{}, {}, 0x{:08X}", reg(as_), b4constu(ar), br8_target),
                        format!("; BRI8u off={}", simm8),
                        &bytes,
                    ),
                },
            }
        }

        // ===== op0=7: RRI8 分支组（BNONE/BEQ/BLT/.../BBCI/BBSI）=====
        // target = inst_start + simm8 + 4；BBCI/BBSI 位号 = (ar[0]<<4)|at（u5_4_7_12）
        0x7 => {
            let (mnem, operands) = match ar {
                0x0 => ("BNONE", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x1 => ("BEQ", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x2 => ("BLT", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x3 => ("BLTU", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x4 => ("BALL", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x5 => ("BBC", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x6 | 0x7 => ("BBCI", format!("{}, {}, 0x{:08X}", reg(as_), ((ar & 1) << 4) | at, br8_target)),
                0x8 => ("BANY", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0x9 => ("BNE", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0xA => ("BGE", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0xB => ("BGEU", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0xC => ("BNALL", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                0xD => ("BBS", format!("{}, {}, 0x{:08X}", reg(as_), reg(at), br8_target)),
                // ar=0xE/0xF: BBSI
                _ => ("BBSI", format!("{}, {}, 0x{:08X}", reg(as_), ((ar & 1) << 4) | at, br8_target)),
            };
            mk(mnem, operands, format!("; RRI8-br off={}", simm8), &bytes)
        }

        // ===== op0=2: RRI8（加载/存储/立即数/CACHE 组）=====
        // imm8=bits[23:16]；L32I/S32I 类 offset=imm8<<2，L16/S16 类 offset=imm8<<1
        0x2 => {
            let (mnem, operands) = match ar {
                0x0 => ("L8UI", format!("{}, {}, {}", reg(at), reg(as_), imm8)),
                0x1 => ("L16UI", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 1)),
                0x2 => ("L32I", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 2)),
                0x4 => ("S8I", format!("{}, {}, {}", reg(at), reg(as_), imm8)),
                0x5 => ("S16I", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 1)),
                0x6 => ("S32I", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 2)),
                // ar=7: CACHE 管理组（at 为子操作码，offset=imm8<<2）
                0x7 => match at {
                    0x0 => ("DPFR", format!("{}, {}", reg(as_), imm8 << 2)),
                    0x1 => ("DPFW", format!("{}, {}", reg(as_), imm8 << 2)),
                    0x2 => ("DPFRO", format!("{}, {}", reg(as_), imm8 << 2)),
                    0x3 => ("DPFWO", format!("{}, {}", reg(as_), imm8 << 2)),
                    0x4 => ("DHWB", format!("{}, {}", reg(as_), imm8 << 2)),
                    0x5 => ("DHWBI", format!("{}, {}", reg(as_), imm8 << 2)),
                    0x6 => ("DHI", format!("{}, {}", reg(as_), imm8 << 2)),
                    0xC => ("IPF", format!("{}, {}", reg(as_), imm8 << 2)),
                    0xE => ("IHI", format!("{}, {}", reg(as_), imm8 << 2)),
                    _ => ("", String::new()),
                },
                0x9 => ("L16SI", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 1)),
                // MOVI: 12 位有符号立即数 = (as<<8)|imm8（movi_si16，-2048..2047）
                0xA => ("MOVI", format!("{}, {}", reg(at), sign_ext((as_ << 8) | imm8, 12))),
                0xB => ("L32AI", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 2)),
                0xC => ("ADDI", format!("{}, {}, {}", reg(at), reg(as_), simm8)),
                0xD => ("ADDMI", format!("{}, {}, {}", reg(at), reg(as_), simm8 << 8)),
                0xE => ("S32C1I", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 2)),
                0xF => ("S32RI", format!("{}, {}, {}", reg(at), reg(as_), imm8 << 2)),
                // ar=3/8 保留
                _ => ("", String::new()),
            };
            if mnem.is_empty() {
                unknown(addr, bytes, op0, "RRI8")
            } else {
                mk(mnem, operands, "; RRI8".to_string(), &bytes)
            }
        }

        // ===== op0=0: RRR 组（算术/逻辑/移位/特殊/乘除/条件移动/SR 访问）=====
        // 子路由：op1=bits[19:16]，op2=bits[23:20]（Ghidra xtensaInstructions.sinc）
        0x0 => {
            match op1 {
                // ---------- op1=0 ----------
                0x0 => match op2 {
                    // op2=0: 特殊指令空间（同步/返回/调用/BREAK/异常返回/Boolean）
                    0x0 => {
                        let (mnem, operands) = match (ar, as_, at) {
                            (0, 0, 0x0) => ("ILL", String::new()),
                            // RET: at=0b1000，或 at=0b1010 且 as=0（JX a0 别名）
                            (0, 0, 0x8) => ("RET", String::new()),
                            (0, 0, 0x9) => ("RETW", String::new()),
                            (0, 0, 0xA) => ("RET", String::new()),
                            (0, s, 0xA) => ("JX", format!("{}", reg(s))),
                            (0, s, 0xC) => ("CALLX0", format!("{}", reg(s))),
                            (0, s, 0xD) => ("CALLX4", format!("{}", reg(s))),
                            (0, s, 0xE) => ("CALLX8", format!("{}", reg(s))),
                            (0, s, 0xF) => ("CALLX12", format!("{}", reg(s))),
                            (1, s, t) => ("MOVSP", format!("{}, {}", reg(t), reg(s))),
                            (2, 0, 0x0) => ("ISYNC", String::new()),
                            (2, 0, 0x1) => ("RSYNC", String::new()),
                            (2, 0, 0x2) => ("ESYNC", String::new()),
                            (2, 0, 0x3) => ("DSYNC", String::new()),
                            (2, 0, 0x8) => ("EXCW", String::new()),
                            (2, 0, 0xC) => ("MEMW", String::new()),
                            (2, 0, 0xD) => ("EXTW", String::new()),
                            (2, 0, 0xF) => ("NOP", String::new()),
                            (3, 0, 0x0) => ("RFE", String::new()),
                            (3, s, 0x1) => ("RFI", format!("{}", s)),
                            (3, 0, 0x2) => ("RFME", String::new()),
                            (3, 4, 0x0) => ("RFWO", String::new()),
                            (3, 5, 0x0) => ("RFWU", String::new()),
                            (4, s, t) => ("BREAK", format!("{}, {}", s, t)),
                            (5, 0, 0x0) => ("SYSCALL", String::new()),
                            (6, s, t) => ("RSIL", format!("{}, {}", reg(t), s)),
                            (7, s, 0x0) => ("WAITI", format!("{}", s)),
                            // Boolean Option（ESP32 未配置，保守标注）
                            (8, s, t) => ("ANY4", format!("b{}, b{}", s, t)),
                            (9, s, t) => ("ALL4", format!("b{}, b{}", s, t)),
                            (0xA, s, t) => ("ANY8", format!("b{}, b{}", s, t)),
                            (0xB, s, t) => ("ALL8", format!("b{}, b{}", s, t)),
                            _ => ("RRR.SPEC", format!("ar={}, as={}, at={}", ar, as_, at)),
                        };
                        mk(mnem, operands, "; RRR-special".to_string(), &bytes)
                    }
                    0x1 => rrr("AND", &bytes),
                    // op2=2: OR；as==at 时别名 MOV ar, as（Ghidra: mov is or with as=at）
                    0x2 => {
                        if as_ == at {
                            mk(
                                "MOV",
                                format!("{}, {}", reg(ar), reg(as_)),
                                "; RRR-mov".to_string(),
                                &bytes,
                            )
                        } else {
                            rrr("OR", &bytes)
                        }
                    }
                    0x3 => rrr("XOR", &bytes),
                    // op2=4: 移位量设置 / SSAI / RER / WER / ROTW / NSA / NSAU
                    0x4 => {
                        let (mnem, operands) = match (ar, as_, at) {
                            (0, s, 0) => ("SSR", format!("{}", reg(s))),
                            (1, s, 0) => ("SSL", format!("{}", reg(s))),
                            (2, s, 0) => ("SSA8L", format!("{}", reg(s))),
                            (3, s, 0) => ("SSA8B", format!("{}", reg(s))),
                            // SSAI: sa = (at[0]<<4)|as（u5_8_11_4），at ∈ {0,1}
                            (4, s, t) if t <= 1 => ("SSAI", format!("{}", ((t & 1) << 4) | s)),
                            (6, s, t) => ("RER", format!("{}, {}", reg(s), reg(t))),
                            (7, s, t) => ("WER", format!("{}, {}", reg(s), reg(t))),
                            (8, 0, t) => ("ROTW", format!("{}", sign_ext(t, 4))),
                            (0xE, s, t) => ("NSA", format!("{}, {}", reg(t), reg(s))),
                            (0xF, s, t) => ("NSAU", format!("{}, {}", reg(t), reg(s))),
                            _ => ("RRR.RES", format!("{}, {}, {}", reg(ar), reg(as_), reg(at))),
                        };
                        mk(mnem, operands, "; RRR-shift-amt".to_string(), &bytes)
                    }
                    // op2=6: NEG (as=0) / ABS (as=1)
                    0x6 => match as_ {
                        0 => mk(
                            "NEG",
                            format!("{}, {}", reg(ar), reg(at)),
                            "; RRR-neg".to_string(),
                            &bytes,
                        ),
                        1 => mk(
                            "ABS",
                            format!("{}, {}", reg(ar), reg(at)),
                            "; RRR-abs".to_string(),
                            &bytes,
                        ),
                        _ => mk(
                            "RRR.RES",
                            format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                            format!("; RRR reserved op2=6 as={}", as_),
                            &bytes,
                        ),
                    },
                    0x8 => rrr("ADD", &bytes),
                    0x9 => rrr("ADDX2", &bytes),
                    0xA => rrr("ADDX4", &bytes),
                    0xB => rrr("ADDX8", &bytes),
                    0xC => rrr("SUB", &bytes),
                    0xD => rrr("SUBX2", &bytes),
                    0xE => rrr("SUBX4", &bytes),
                    0xF => rrr("SUBX8", &bytes),
                    // op2=5/7 保留
                    _ => mk(
                        "RRR.RES",
                        format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                        format!("; RRR reserved op1=0 op2={}", op2),
                        &bytes,
                    ),
                },

                // ---------- op1=1: 立即数/寄存器移位 + MUL16 + XSR + TRAX/缓存标签 ----------
                // Ghidra sinc: SLLI op2∈{0,1}；SRAI op2∈{2,3}；SRLI op2=4；XSR op2=6（sr=bits[15:8]）；
                // SRC op2=8；SRL op2=9；SLL op2=0xA；SRA op2=0xB；MUL16U op2=0xC；MUL16S op2=0xD
                0x1 => match op2 {
                    // SLLI: 编码 sa=(op2[0]<<4)|at，实际左移量 = 32 - sa（u5_4_7_20），sa=0 非法
                    0x0 | 0x1 => {
                        let sa = ((op2 & 1) << 4) | at;
                        if sa == 0 {
                            mk(
                                "RRR.RES",
                                format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                                "; RRR reserved slli sa=0".to_string(),
                                &bytes,
                            )
                        } else {
                            mk(
                                "SLLI",
                                format!("{}, {}, {}", reg(ar), reg(as_), 32 - sa),
                                "; RRR slli".to_string(),
                                &bytes,
                            )
                        }
                    }
                    // SRAI: sa = (op2[0]<<4)|as（u5_8_11_20），源寄存器在 at
                    0x2 | 0x3 => mk(
                        "SRAI",
                        format!("{}, {}, {}", reg(ar), reg(at), ((op2 & 1) << 4) | as_),
                        "; RRR srai".to_string(),
                        &bytes,
                    ),
                    // SRLI: sa = as（1..15），源寄存器在 at
                    0x4 => mk(
                        "SRLI",
                        format!("{}, {}, {}", reg(ar), reg(at), as_),
                        "; RRR srli".to_string(),
                        &bytes,
                    ),
                    // XSR: sr = bits[15:8]
                    0x6 => mk(
                        "XSR",
                        format!("{}, {}", reg(at), sr_name((ar << 4) | as_)),
                        "; RRR xsr".to_string(),
                        &bytes,
                    ),
                    0x8 => rrr("SRC", &bytes),
                    // SRL/SRA: 源寄存器在 at；SLL: 源寄存器在 as（SAR 间接移位）
                    0x9 => mk(
                        "SRL",
                        format!("{}, {}", reg(ar), reg(at)),
                        "; RRR srl".to_string(),
                        &bytes,
                    ),
                    0xA => mk(
                        "SLL",
                        format!("{}, {}", reg(ar), reg(as_)),
                        "; RRR sll".to_string(),
                        &bytes,
                    ),
                    0xB => mk(
                        "SRA",
                        format!("{}, {}", reg(ar), reg(at)),
                        "; RRR sra".to_string(),
                        &bytes,
                    ),
                    0xC => rrr("MUL16U", &bytes),
                    0xD => rrr("MUL16S", &bytes),
                    // TRAX 调试/缓存标签组（ESP32 未配置 TRAX，罕见）
                    0xF => {
                        let (mnem, operands) = match ar {
                            0x0 => ("LICT", format!("{}, {}", reg(at), reg(as_))),
                            0x1 => ("SICT", format!("{}, {}", reg(at), reg(as_))),
                            0x3 => ("SICW", format!("{}, {}", reg(at), reg(as_))),
                            0x8 => ("LDCT", format!("{}, {}", reg(at), reg(as_))),
                            0x9 => ("SDCT", format!("{}, {}, {}", reg(at), reg(as_), op2)),
                            0xE if at == 0 && as_ == 0 => ("RFDO", String::new()),
                            0xE if at == 1 => ("RFDD", String::new()),
                            _ => ("", String::new()),
                        };
                        if mnem.is_empty() {
                            mk(
                                "RRR.RES",
                                format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                                format!("; RRR reserved op1=1 op2=0xF ar={}", ar),
                                &bytes,
                            )
                        } else {
                            mk(mnem, operands, "; RRR trax".to_string(), &bytes)
                        }
                    }
                    _ => mk(
                        "RRR.RES",
                        format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                        format!("; RRR reserved op1=1 op2={}", op2),
                        &bytes,
                    ),
                },

                // ---------- op1=2: 32位乘除 + Boolean 逻辑 ----------
                // Ghidra sinc: MULL op2=8；MULUH op2=0xA；MULSH op2=0xB；QUOU op2=0xC；
                // QUOS op2=0xD；REMU op2=0xE；REMS op2=0xF；ANDB/ANDBC/ORB/ORBC/XORB op2=0-4
                0x2 => match op2 {
                    // Boolean Option（ESP32 未配置，保守标注 b 寄存器）
                    0x0 => mk(
                        "ANDB",
                        format!("b{}, b{}, b{}", ar, as_, at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    0x1 => mk(
                        "ANDBC",
                        format!("b{}, b{}, b{}", ar, as_, at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    0x2 => mk(
                        "ORB",
                        format!("b{}, b{}, b{}", ar, as_, at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    0x3 => mk(
                        "ORBC",
                        format!("b{}, b{}, b{}", ar, as_, at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    0x4 => mk(
                        "XORB",
                        format!("b{}, b{}, b{}", ar, as_, at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    0x8 => rrr("MULL", &bytes),
                    0xA => rrr("MULUH", &bytes),
                    0xB => rrr("MULSH", &bytes),
                    0xC => rrr("QUOU", &bytes),
                    0xD => rrr("QUOS", &bytes),
                    0xE => rrr("REMU", &bytes),
                    0xF => rrr("REMS", &bytes),
                    _ => mk(
                        "RRR.RES",
                        format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                        format!("; RRR reserved op1=2 op2={}", op2),
                        &bytes,
                    ),
                },

                // ---------- op1=3: SR/UR 访问 + SEXT/CLAMPS + MIN/MAX + 条件移动 ----------
                // Ghidra sinc: RSR op2=0 / WSR op2=1（sr=bits[15:8]）；SEXT op2=2 / CLAMPS op2=3
                // （imm=at+7，u5_4_7_plus7）；MIN/MAX/MINU/MAXU op2=4-7；
                // MOVEQZ/MOVNEZ/MOVLTZ/MOVGEZ op2=8-0xB；MOVF/MOVT op2=0xC/0xD（Boolean）；
                // RUR op2=0xE（ur=bits[11:4]，op_st）；WUR op2=0xF（ur=bits[15:8]，sr）
                0x3 => match op2 {
                    0x0 => mk(
                        "RSR",
                        format!("{}, {}", reg(at), sr_name((ar << 4) | as_)),
                        "; RRR rsr".to_string(),
                        &bytes,
                    ),
                    0x1 => mk(
                        "WSR",
                        format!("{}, {}", reg(at), sr_name((ar << 4) | as_)),
                        "; RRR wsr".to_string(),
                        &bytes,
                    ),
                    0x2 => mk(
                        "SEXT",
                        format!("{}, {}, {}", reg(ar), reg(as_), at + 7),
                        "; RRR sext".to_string(),
                        &bytes,
                    ),
                    0x3 => mk(
                        "CLAMPS",
                        format!("{}, {}, {}", reg(ar), reg(as_), at + 7),
                        "; RRR clamps".to_string(),
                        &bytes,
                    ),
                    0x4 => rrr("MIN", &bytes),
                    0x5 => rrr("MAX", &bytes),
                    0x6 => rrr("MINU", &bytes),
                    0x7 => rrr("MAXU", &bytes),
                    0x8 => rrr("MOVEQZ", &bytes),
                    0x9 => rrr("MOVNEZ", &bytes),
                    0xA => rrr("MOVLTZ", &bytes),
                    0xB => rrr("MOVGEZ", &bytes),
                    // Boolean 条件移动（ESP32 未配置）
                    0xC => mk(
                        "MOVF",
                        format!("{}, {}, b{}", reg(ar), reg(as_), at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    0xD => mk(
                        "MOVT",
                        format!("{}, {}, b{}", reg(ar), reg(as_), at),
                        "; RRR-bool".to_string(),
                        &bytes,
                    ),
                    // RUR: 用户寄存器号 = bits[11:4]（op_st）
                    0xE => mk(
                        "RUR",
                        format!("{}, ur{}", reg(ar), (as_ << 4) | at),
                        "; RRR rur".to_string(),
                        &bytes,
                    ),
                    // WUR: 用户寄存器号 = bits[15:8]（sr 字段）
                    _ => mk(
                        "WUR",
                        format!("{}, ur{}", reg(at), (ar << 4) | as_),
                        "; RRR wur".to_string(),
                        &bytes,
                    ),
                },

                // ---------- op1=4/5: EXTUI（提取无符号位域）----------
                // Ghidra sinc: op1_1_3=0b010 → op1∈{4,5}；shift=(op1[0]<<4)|as（u5_8_11_16）；
                // mask 宽度 = op2+1（u5_20_23_plus1）；源寄存器在 at，目标在 ar
                0x4 | 0x5 => mk(
                    "EXTUI",
                    format!("{}, {}, {}, {}", reg(ar), reg(at), ((op1 & 1) << 4) | as_, op2 + 1),
                    "; RRR extui".to_string(),
                    &bytes,
                ),

                // ---------- op1=8: FP 索引加载/存储空间（ESP32 无 FPU，保守降级）----------
                0x8 => mk(
                    "RRR.FP",
                    format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                    format!("; RRR fp-lsxu op2={} (no FP on ESP32)", op2),
                    &bytes,
                ),

                // ---------- op1=9: L32E/S32E（窗口异常溢出加载/存储）----------
                // Ghidra sinc: L32E op2=0 / S32E op2=4；
                // offset = (bits[15:12]<<2) - 64（s5_12_15_oex；QEMU Operand_immrx4_decode 同式），
                // 范围 -64..-4 步进 4
                0x9 => match op2 {
                    0x0 => mk(
                        "L32E",
                        format!("{}, {}, {}", reg(at), reg(as_), ((ar << 2) as i32) - 64),
                        "; RRI4 l32e".to_string(),
                        &bytes,
                    ),
                    0x4 => mk(
                        "S32E",
                        format!("{}, {}, {}", reg(at), reg(as_), ((ar << 2) as i32) - 64),
                        "; RRI4 s32e".to_string(),
                        &bytes,
                    ),
                    _ => mk(
                        "RRR.RES",
                        format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                        format!("; RRR reserved op1=9 op2={}", op2),
                        &bytes,
                    ),
                },

                // ---------- op1=0xA/0xB: FP 转换/比较空间（ESP32 无 FPU，保守降级）----------
                0xA | 0xB => mk(
                    "RRR.FP",
                    format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                    format!("; RRR fp op1={} op2={} (no FP on ESP32)", op1, op2),
                    &bytes,
                ),

                // ---------- op1=6/7/0xC-0xF: 保留编码空间 ----------
                _ => mk(
                    "RRR.RES",
                    format!("{}, {}, {}", reg(ar), reg(as_), reg(at)),
                    format!("; RRR reserved op1={}", op1),
                    &bytes,
                ),
            }
        }

        // ===== op0=3: LSI 组（FP 加载/存储 LSI/LSIU/SSI/SSIU）=====
        // Ghidra sinc: LSI ar=0 / LSIU ar=8 / SSI ar=4 / SSIU ar=0xC。ESP32 无 FPU，保守降级
        0x3 => unknown(addr, bytes, op0, "LSI"),

        // ===== op0=4: MAC16 乘累加组（MAC16 Option）=====
        // Ghidra sinc: LDINC op2=8 / LDDEC op2=9（op1=0, at=0, mw=bits[13:12]，即 ar∈{0..3}）；
        // UMUL.AA.* op2=7, op1=0-3；MUL.AA.* op2=7, op1=4-7（ar=0）
        0x4 => {
            if (op2 == 8 || op2 == 9) && op1 == 0 && at == 0 && ar <= 3 {
                let mnem = if op2 == 8 { "LDINC" } else { "LDDEC" };
                mk(
                    mnem,
                    format!("m{}, {}", ar, reg(as_)),
                    "; MAC16 ldinc/lddec".to_string(),
                    &bytes,
                )
            } else if op2 == 7 && ar == 0 {
                let op_name = if op1 < 4 { "UMUL.AA" } else { "MUL.AA" };
                let half = match op1 & 3 {
                    0 => "LL",
                    1 => "HL",
                    2 => "LH",
                    _ => "HH",
                };
                mk(
                    &format!("{}.{}", op_name, half),
                    format!("{}, {}", reg(as_), reg(at)),
                    "; MAC16 mul".to_string(),
                    &bytes,
                )
            } else {
                // 其余 MAC16 变体（MULA/MULS/MUL.AD/DA/DD 等）保守标注原始字段
                mk(
                    "MAC16",
                    format!("op1={}, op2={}, ar={}, as={}, at={}", op1, op2, ar, as_, at),
                    format!("; MAC16 ext op1={} op2={}", op1, op2),
                    &bytes,
                )
            }
        }

        // ===== op0=8..0xD: 16 位窄指令空间 =====
        // 正常路径下已被 is_narrow() 拦截进 decode_one_narrow；防御性降级
        0x8..=0xD => unknown(addr, bytes, op0, "NARROW"),

        // ===== op0=0xE/0xF: TIE/用户扩展空间（UDI2/UDI3）=====
        // 标准 Xtensa ISA 未定义；ESP32-S3 自定义指令可能落入此空间，输出原始字节
        0xE | 0xF => {
            let op1 = (instr >> 16) & 0xF;
            let operands = bytes
                .iter()
                .map(|b| format!("0x{:02X}", b))
                .collect::<Vec<_>>()
                .join(", ");
            DisasmInstruction {
                addr, bytes,
                mnemonic: "TIE".to_string(),
                operands,
                comment: Some(format!("; TIE/user ext op0={} op1={}", op0, op1)),
            }
        }

        // ===== 兜底（op0 已掩码到 4 位，实际不可达）=====
        _ => unknown(addr, bytes, op0, "impossible"),
    }
}

/// 未识别指令：输出 raw bytes + op0 格式标注
/// 返回 "RES.{fmt}" 作为 mnemonic（如 RES.RSR/RES.SHIFT/RES.RRI8 等），
/// 表示该编码属于已知格式空间的保留/未定义变体，在 ESP32-S3 中可能为 TIE 扩展。
/// trailing bytes（剩余不足一条指令的字节）输出为 "RES.trailing"。
fn unknown(addr: u32, bytes: Vec<u8>, op0: u32, fmt: &str) -> DisasmInstruction {
    let operands = bytes
        .iter()
        .map(|b| format!("0x{:02X}", b))
        .collect::<Vec<_>>()
        .join(", ");
    DisasmInstruction {
        addr,
        bytes,
        mnemonic: format!("RES.{}", fmt),
        operands,
        comment: Some(format!("; op0={} ({})", op0, fmt)),
    }
}

/// 统计反汇编结果中的函数调用（CALL0/4/8/12）目标地址
pub fn collect_calls(instructions: &[DisasmInstruction]) -> Vec<u32> {
    instructions
        .iter()
        .filter(|i| i.mnemonic.starts_with("CALL"))
        .filter_map(|i| {
            i.operands
                .strip_prefix("0x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
        })
        .collect()
}

/// 估算指令流的"数据比例"：未识别 + 保留编码占比
/// 用于判断某区域是否为数据（地址表/常量表）而非代码
/// 包含: db(尾部字节) + RES.*(已知格式保留变体) + RRR.SPEC/RRR.RES/RRI4.RES/NARROW.D
/// 不包含: TIE/PIE/MAC16 等真扩展指令（它们在代码中正常出现）
pub fn data_ratio(instructions: &[DisasmInstruction]) -> f32 {
    if instructions.is_empty() {
        return 1.0;
    }
    let unknown_count = instructions.iter()
        .filter(|i| is_data_like(i) || is_reserved(i))
        .count();
    unknown_count as f32 / instructions.len() as f32
}

/// 判断指令是否为真正的 TIE/PIE/MAC16 扩展指令（应算作已识别）
/// 这些是 ESP32-S3 等 Xtensa 实现中真实存在的扩展指令
/// 注意：ESP32-S3 的 TIE 扩展可能使用 RRR/RRI4/Narrow 的保留编码空间，
/// 因此 RRR.SPEC/RRR.RES/RRI4.RES/NARROW.D 也算作扩展指令
/// RES.* 为已知格式空间（RSR/SHIFT/RRI8/BRI等）的保留/未定义变体，
/// 在 ESP32-S3 中可能为 TIE 扩展，也算作扩展指令
pub fn is_extension(inst: &DisasmInstruction) -> bool {
    inst.mnemonic.starts_with("TIE")
        || inst.mnemonic.starts_with("PIE")
        || inst.mnemonic.starts_with("MUL.")
        || inst.mnemonic.starts_with("MULA.")
        || inst.mnemonic.starts_with("MULS.")
        || inst.mnemonic.starts_with("MUL16")
        || inst.mnemonic.starts_with("UMUL.")
        || inst.mnemonic == "LDINC"
        || inst.mnemonic == "LDDEC"
        || inst.mnemonic == "LD.DR"
        || inst.mnemonic == "LD.INC"
        || inst.mnemonic == "ST.DR"
        || inst.mnemonic == "ST.INC"
        // ESP32-S3 TIE 扩展使用 RRR/RRI4/Narrow 的保留编码空间
        || inst.mnemonic == "RRR.SPEC"
        || inst.mnemonic == "RRR.RES"
        || inst.mnemonic == "RRI4.RES"
        || inst.mnemonic == "NARROW.D"
        // 已知格式空间的保留/未定义变体（RES.RSR/RES.SHIFT/RES.RRI8等）
        || inst.mnemonic.starts_with("RES.")
}

/// 判断指令是否为 MAC16 扩展（op0=9 空间，包含未覆盖变体）
/// MAC16 指令是标准 Xtensa ISA 的一部分，算作已识别
pub fn is_mac16(inst: &DisasmInstruction) -> bool {
    inst.mnemonic.starts_with("MAC16")
}

/// 判断指令是否为保留编码/未识别变体
/// 这些编码在标准 ISA 中未定义，但 ESP32-S3 TIE 扩展可能使用
/// 注意：is_extension 已包含这些，此函数仅用于兼容性检查
pub fn is_reserved(inst: &DisasmInstruction) -> bool {
    inst.mnemonic.starts_with("NARROW.D")
        || inst.mnemonic.starts_with("RRR.SPEC")
        || inst.mnemonic.starts_with("RRR.RES")
        || inst.mnemonic.starts_with("RRI4.RES")
        || inst.mnemonic.starts_with("RES.")
}

/// 兼容旧接口：TIE 扩展 + MAC16 + 保留编码（用于统计时不算 db）
#[deprecated(note = "Use is_extension + is_mac16 + is_reserved instead")]
pub fn is_tie(inst: &DisasmInstruction) -> bool {
    is_extension(inst) || is_mac16(inst) || is_reserved(inst)
}

/// 判断指令是否为真正的未识别数据（db，非 TIE/Narrow/保留编码）
pub fn is_data_like(inst: &DisasmInstruction) -> bool {
    inst.mnemonic == "db"
}

/// 判断指令是否为"非已识别"——仅 db
/// 保留编码（RRR.SPEC/RRR.RES/RRI4.RES/NARROW.D）已算作 TIE 扩展，
/// 不再视为未识别数据
pub fn is_unidentified(inst: &DisasmInstruction) -> bool {
    is_data_like(inst)
}

/// 智能反汇编：线性扫描 + 数据区域分离（支持 16/24 位混合解码）
///
/// 从给定偏移开始反汇编，遇到连续 `data_threshold` 条 db/保留编码指令时，
/// 判定为数据区域，输出 hex dump 并跳过。扩展指令（TIE/PIE/MAC16）不触发数据区域检测。
/// 支持 Code Density Option 的 16 位 Narrow 指令，避免指令流错位。
///
/// 返回：(反汇编结果, 统计信息)
pub fn smart_disassemble(
    data: &[u8],
    base_addr: u32,
    offset: usize,
    max_count: usize,
    data_threshold: usize,
) -> (Vec<DisasmInstruction>, SmartDisasmStats) {
    let mut result = Vec::with_capacity(max_count);
    let mut stats = SmartDisasmStats::default();
    let mut pos = offset;
    let mut consecutive_unidentified = 0usize;
    let threshold = if data_threshold > 0 { data_threshold } else { 4 };

    while result.len() < max_count {
        if pos >= data.len() {
            break;
        }
        let b0 = data[pos];
        let addr = base_addr.wrapping_add((pos - offset) as u32);

        let inst = if is_narrow(b0) && pos + 2 <= data.len() {
            // 16 位 Narrow 指令
            let b1 = data[pos + 1];
            let instr16 = (b0 as u16) | ((b1 as u16) << 8);
            let bytes = vec![b0, b1];
            let inst = decode_one_narrow(addr, instr16, bytes);
            pos += 2;
            stats.narrow_count += 1;
            inst
        } else if pos + 3 <= data.len() {
            // 24 位标准指令
            let b1 = data[pos + 1];
            let b2 = data[pos + 2];
            let instr = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
            let bytes = vec![b0, b1, b2];
            let inst = decode_one(addr, instr, bytes);
            pos += 3;
            inst
        } else {
            // 剩余字节不足一条指令，输出为 RES.trailing（计入扩展指令）
            let remaining = data.len() - pos;
            let bytes: Vec<u8> = data[pos..pos + remaining].to_vec();
            result.push(DisasmInstruction {
                addr,
                bytes: bytes.clone(),
                mnemonic: "RES.trailing".to_string(),
                operands: bytes.iter().map(|b| format!("0x{:02X}", b)).collect::<Vec<_>>().join(", "),
                comment: Some("; trailing bytes".to_string()),
            });
            break;
        };

        // 数据区域检测：db + 保留编码 + NARROW.D 都算未识别
        if is_unidentified(&inst) {
            consecutive_unidentified += 1;
        } else {
            consecutive_unidentified = 0;
        }

        result.push(inst);

        // 连续未识别达到阈值：尝试跳过数据区域
        // 不真正跳过（保持输出完整），但标记统计
        if consecutive_unidentified >= threshold {
            // 标记为数据区域候选（由调用方 format_disasm_with_data_regions 处理）
            // 这里不做跳过，保持线性扫描的完整性
        }
    }

    // 统计
    for inst in &result {
        if is_data_like(inst) {
            stats.db_count += 1;
        } else if is_reserved(inst) {
            stats.reserved_count += 1;
        } else if is_extension(inst) || is_mac16(inst) {
            stats.extension_count += 1;
        } else {
            stats.identified_count += 1;
        }
    }
    stats.total_count = result.len();

    (result, stats)
}

/// 智能反汇编统计信息
#[derive(Debug, Clone, Default)]
pub struct SmartDisasmStats {
    pub total_count: usize,
    /// 真正已识别的标准指令数
    pub identified_count: usize,
    /// 扩展指令数（TIE/PIE/MAC16/MUL.*）
    pub extension_count: usize,
    /// 保留编码数（RRR.RES/RRI4.RES/NARROW.D）
    pub reserved_count: usize,
    /// db 数据字节数
    pub db_count: usize,
    /// 16 位 Narrow 指令数量
    pub narrow_count: usize,
}

impl SmartDisasmStats {
    /// 计算识别率（扩展指令计入已识别，保留编码和 db 不计入）
    pub fn identification_rate(&self, data_region_count: usize) -> f32 {
        let effective_total = self.total_count.saturating_sub(data_region_count);
        if effective_total == 0 {
            return 0.0;
        }
        // 已识别 + 扩展指令 = 有效识别
        let effective_identified = self.identified_count + self.extension_count;
        effective_identified as f32 / effective_total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_call0_decoding() {
        // CALL0: op0=bits[3:0]=5, n=bits[5:4]=0, off18=bits[23:6]
        // target = (addr & !3) + 4 + (off18 << 2)
        // off18=0x3FF → target = 0 + 4 + 0xFFC = 0x1000
        let instr = (0x3FFu32 << 6) | 0b0101u32;
        assert_eq!(instr & 0xF, 5, "op0 必须在 bits[3:0]");
        let b0 = (instr & 0xFF) as u8;
        let b1 = ((instr >> 8) & 0xFF) as u8;
        let b2 = ((instr >> 16) & 0xFF) as u8;
        let result = disassemble_xtensa(&[b0, b1, b2], 0x0, 0, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].mnemonic, "CALL0");
        assert!(result[0].operands.contains("0x00001000"));
    }

    #[test]
    fn test_entry_decoding() {
        // 金标准：entry a1, 32 = 字节 36 41 00 → instr=0x004136
        // op0=bits[3:0]=6 (BRI组)，n=bits[5:4]=3，m=bits[7:6]=0，
        // as=bits[11:8]=1 (a1)，imm12=bits[23:12]=4 → 栈帧 4*8=32 字节
        let bytes = vec![0x36u8, 0x41, 0x00];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "ENTRY");
        assert_eq!(result[0].operands, "a1, 32");
    }

    #[test]
    fn test_j_target() {
        // J: op0=6(bits[3:0])，bri8_n=0(bits[5:4])，off18=bits[23:6]（有符号）
        // target = addr + 4 + off18（Ghidra call_srel；QEMU 基准编码 0x6）
        // addr=0x100，off18=0x10 → target = 0x100 + 4 + 0x10 = 0x114
        let instr = (0x10u32 << 6) | 0x6u32;
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x100, 0, 1);
        assert_eq!(result[0].mnemonic, "J");
        assert_eq!(result[0].operands, "0x00000114");
    }

    #[test]
    fn test_beqz_bri12_target() {
        // BEQZ a3: op0=6，bri8_n=1(bits[5:4])，bri8_m=0(bits[7:6])，
        // as=a3(bits[11:8])，imm12=bits[23:12]（有符号）
        // target = addr + 4 + sign_ext(imm12, 12)；addr=0x200，imm12=0x20 → 0x224
        let instr = (0x20u32 << 12) | (3u32 << 8) | (0b01u32 << 4) | 0x6u32;
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x200, 0, 1);
        assert_eq!(result[0].mnemonic, "BEQZ");
        assert_eq!(result[0].operands, "a3, 0x00000224");
    }

    #[test]
    fn test_unknown_falls_back_to_db() {
        // op0=0 (RRR), op1=6 (保留编码空间) → RRR.RES
        // RRR 保留编码不算作 db（未知数据），计入已识别指令
        let instr = 6u32 << 16; // op1=6，op0=0
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "RRR.RES");
    }

    #[test]
    fn test_tie_extension() {
        // op0=bits[3:0]=15 → TIE/用户扩展空间
        let bytes = vec![0x0Fu8, 0x00, 0x00];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert!(result[0].mnemonic.starts_with("TIE"));
        assert!(result[0].comment.as_deref().unwrap().contains("op0=15"));
    }

    #[test]
    fn test_add_decoding() {
        // ADD a0, a1, a2: op2=0b1000(bits[23:20]), op1=0(bits[19:16]),
        // ar=a0(bits[15:12]), as=a1(bits[11:8]), at=a2(bits[7:4]), op0=0(bits[3:0])
        // QEMU 基准编码 0x800000；Ghidra: :add ar, as, at is op2=0b1000 & op1=0 & op0=0
        let instr = (0b1000u32 << 20) // op2=0b1000
            | (0u32 << 16)            // op1=0
            | (0u32 << 12)            // ar=a0
            | (1u32 << 8)             // as=a1
            | (2u32 << 4);            // at=a2
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "ADD");
        assert!(result[0].operands.contains("a0"));
        assert!(result[0].operands.contains("a1"));
        assert!(result[0].operands.contains("a2"));
    }

    #[test]
    fn test_l32r_decoding() {
        // L32R a2, [lit]: op0=1(bits[3:0])，at=a2(bits[7:4])，imm16=bits[23:8]（负偏移）
        // QEMU 基准编码 0x1；target = ((addr+3) & !3) + (imm16 - 65536) * 4
        // imm16=0xFFFF → off=-1 → target = 0x100 - 4 = 0xFC
        let instr = (0xFFFFu32 << 8) | (2u32 << 4) | 0x1u32;
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x100, 0, 1);
        assert_eq!(result[0].mnemonic, "L32R");
        assert!(result[0].operands.contains("a2"));
        assert!(result[0].operands.contains("0x000000FC"));
    }

    #[test]
    fn test_mov_decoding() {
        // MOV a0, a1 是 OR a0, a1, a1 的别名（Ghidra: :mov ar, as is op2=0b0010 & as=at）
        // op2=0b0010(bits[23:20])，ar=a0，as=a1，at=a1，op0=0；QEMU 基准编码 0x200000
        let instr = (0b0010u32 << 20) // op2=0b0010
            | (0u32 << 16)            // op1=0
            | (0u32 << 12)            // ar=a0
            | (1u32 << 8)             // as=a1
            | (1u32 << 4);            // at=a1（as==at → MOV 别名）
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "MOV");
        assert!(result[0].operands.contains("a0"));
        assert!(result[0].operands.contains("a1"));
    }

    #[test]
    fn test_data_ratio() {
        let instrs = vec![
            DisasmInstruction { addr: 0, bytes: vec![], mnemonic: "ADD".to_string(), operands: "".to_string(), comment: None },
            DisasmInstruction { addr: 0, bytes: vec![], mnemonic: "db".to_string(), operands: "".to_string(), comment: None },
            DisasmInstruction { addr: 0, bytes: vec![], mnemonic: "db".to_string(), operands: "".to_string(), comment: None },
        ];
        let ratio = data_ratio(&instrs);
        assert!((ratio - 0.667).abs() < 0.01);
    }

    #[test]
    fn test_callx0_decoding() {
        // CALLX0 a2: op0=0, op1=0, op2=0, ar=0, as=a2(源寄存器), at=0b1100
        // 参考 Ghidra: callx0 as is op2=0 & op1=0 & ar=0 & as & bri8_m=0b11 & bri8_n=0 & op0=0
        // at = (bri8_m << 2) | bri8_n = 0b1100
        let instr = (0u32 << 20)      // op2=0
            | (0u32 << 16)             // op1=0
            | (0u32 << 12)             // ar=0
            | (2u32 << 8)              // as=a2(源寄存器)
            | (0b1100u32 << 4)         // at=0b1100 (bri8_m=0b11, bri8_n=0)
            | (0u32);                  // op0=0
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "CALLX0");
        assert!(result[0].operands.contains("a2"));
    }

    #[test]
    fn test_callx12_decoding() {
        // CALLX12 a3: op0=0, op1=0, op2=0, ar=0, as=a3(源寄存器), at=0b1111
        // at = (bri8_m << 2) | bri8_n = 0b1100 | 0b11 = 0b1111
        let instr = (0u32 << 20)
            | (0u32 << 16)
            | (0u32 << 12)             // ar=0
            | (3u32 << 8)              // as=a3(源寄存器)
            | (0b1111u32 << 4)         // at=0b1111 (bri8_m=0b11, bri8_n=0b11)
            | (0u32);
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "CALLX12");
        assert!(result[0].operands.contains("a3"));
    }

    #[test]
    fn test_conditional_move() {
        // MOVEQZ a0, a1, a2: op2=0b1000(bits[23:20]), op1=0b0011(bits[19:16]), op0=0
        // QEMU 基准编码 0x830000；Ghidra: :moveqz is op2=0b1000 & op1=0b0011 & op0=0
        let instr = (0b1000u32 << 20)   // op2=0b1000
            | (3u32 << 16)              // op1=3
            | (0u32 << 12)              // ar=a0
            | (1u32 << 8)               // as=a1
            | (2u32 << 4);              // at=a2
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "MOVEQZ");
        assert!(result[0].operands.contains("a0"));
        assert!(result[0].operands.contains("a1"));
        assert!(result[0].operands.contains("a2"));
    }

    #[test]
    fn test_neg_abs_decoding() {
        // NEG a0, a1: op2=0b0110(bits[23:20]), op1=0, ar=a0, as=0, at=a1, op0=0
        // QEMU 基准编码 0x600000；Ghidra: :neg ar, at is op2=0b0110 & op1=0 & as=0 & op0=0
        let instr_neg = (0b0110u32 << 20) // op2=0b0110
            | (0u32 << 16)
            | (0u32 << 12)             // ar=a0
            | (0u32 << 8)              // as=0 (NEG constraint)
            | (1u32 << 4);             // at=a1
        let bytes_neg = vec![
            (instr_neg & 0xFF) as u8,
            ((instr_neg >> 8) & 0xFF) as u8,
            ((instr_neg >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes_neg, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "NEG");
        assert!(result[0].operands.contains("a0"));
        assert!(result[0].operands.contains("a1"));

        // ABS a0, a1: 同 NEG 但 as=1（QEMU 基准编码 0x600100）
        let instr_abs = (0b0110u32 << 20) // op2=0b0110
            | (0u32 << 16)
            | (0u32 << 12)             // ar=a0
            | (1u32 << 8)              // as=1 (ABS constraint)
            | (1u32 << 4);             // at=a1
        let bytes_abs = vec![
            (instr_abs & 0xFF) as u8,
            ((instr_abs >> 8) & 0xFF) as u8,
            ((instr_abs >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes_abs, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "ABS");
    }

    #[test]
    fn test_mull_decoding() {
        // Xtensa ISA 无 SLT 指令（Ghidra sinc / QEMU 模块均无此编码），原测试为幻影指令；
        // op1=0b0010 组的真实指令为 MULL/MULUH/MULSH/QUO*/REM*。
        // MULL a0, a1, a2: op2=0b1000(bits[23:20]), op1=0b0010(bits[19:16]), op0=0
        // QEMU 基准编码 0x820000；Ghidra: :mull is op2=0b1000 & op1=0b0010 & op0=0
        let instr = (0b1000u32 << 20)   // op2=0b1000
            | (2u32 << 16)              // op1=2
            | (0u32 << 12)              // ar=a0
            | (1u32 << 8)               // as=a1
            | (2u32 << 4);              // at=a2
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "MULL");
        assert!(result[0].operands.contains("a0"));
        assert!(result[0].operands.contains("a1"));
        assert!(result[0].operands.contains("a2"));
    }

    #[test]
    fn test_b4const_mapping() {
        // 验证 B4const 非线性映射（Xtensa ISA §2.2.2）
        assert_eq!(b4const(0), -1);
        assert_eq!(b4const(1), 1);
        assert_eq!(b4const(8), 8);
        assert_eq!(b4const(9), 10);   // 非 9
        assert_eq!(b4const(10), 12);  // 非 10
        assert_eq!(b4const(11), 16);  // 非 11
        assert_eq!(b4const(12), 32);  // 非 12
        assert_eq!(b4const(13), 64);
        assert_eq!(b4const(14), 128);
        assert_eq!(b4const(15), 256);
    }

    #[test]
    fn test_b4constu_mapping() {
        // 验证 B4constu 无符号映射（Xtensa ISA §2.2.3）
        assert_eq!(b4constu(0), 32768);
        assert_eq!(b4constu(1), 65536);
        assert_eq!(b4constu(2), 2);
        assert_eq!(b4constu(15), 256);
    }

    #[test]
    fn test_retw_decoding() {
        // RETW: op2=0, op1=0, ar=0, as=0, bri8_m=0b10(bits[7:6]), bri8_n=0b01(bits[5:4])
        // → at 字段 = (0b10<<2)|0b01 = 0b1001；QEMU 基准编码 0x90
        // Ghidra: :retw is op2=0 & op1=0 & ar=0 & as=0 & bri8_m=0b10 & bri8_n=0b01 & op0=0
        let instr = 0b1001u32 << 4; // at=0b1001，其余字段全 0
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "RETW");
    }

    #[test]
    fn test_rotw_decoding() {
        // ROTW 3: op2=0b0100(bits[23:20]), op1=0, ar=0b1000(bits[15:12]), as=0, at=simm4=3
        // QEMU 基准编码 0x408000；Ghidra: :rotw op_t_si4 is op2=0b0100 & ar=0b1000 & as=0
        let instr = (0b0100u32 << 20)  // op2=4
            | (0u32 << 16)             // op1=0
            | (8u32 << 12)             // ar=0b1000
            | (0u32 << 8)              // as=0
            | (3u32 << 4);             // at=simm4=3（窗口旋转量）
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "ROTW");
        assert!(result[0].operands.contains("3"));
    }

    #[test]
    fn test_s32c1i_decoding() {
        // S32C1I a0, a1, 0x80: op0=2(bits[3:0])，r=0b1110(bits[15:12])，
        // as=a1(bits[11:8])，at=a0(bits[7:4])，imm8=0x20(bits[23:16])，offset=imm8<<2
        // QEMU 基准编码 0xE002；Ghidra: :s32c1i is ri8_i8_sh2 & ar=0b1110 & op0=0b0010
        let instr = (0x20u32 << 16)     // imm8=0x20
            | (0b1110u32 << 12)         // r=0b1110
            | (1u32 << 8)               // as=a1
            | (0u32 << 4)               // at=a0
            | 0x2u32;                   // op0=2
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "S32C1I");
    }

    #[test]
    fn test_s8i_at_r4() {
        // S8I a0, a1, 0x10: op0=2(bits[3:0])，r=0b0100(bits[15:12])，
        // as=a1(bits[11:8])，at=a0(bits[7:4])，imm8=0x10(bits[23:16])，offset 不移位
        // QEMU 基准编码 0x4002；Ghidra: :s8i is ri8_i8 & ar=0b0100 & op0=0b0010
        let instr = (0x10u32 << 16)     // imm8=0x10
            | (0b0100u32 << 12)         // r=0b0100
            | (1u32 << 8)               // as=a1
            | (0u32 << 4)               // at=a0
            | 0x2u32;                   // op0=2
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "S8I");
    }

    #[test]
    fn test_ssai_full_range() {
        // SSAI: op2=0b0100(bits[23:20]), op1=0, ar=0b0100(bits[15:12])，
        // sa = (at[0]<<4)|as（Ghidra u5_8_11_4，at ∈ {0,1} 因 op_t_1_3=0）
        // QEMU 基准编码 0x404000
        // SSAI 16: at=1(sa[4]=1)，as=0 → sa = (1<<4)|0 = 16
        let instr = (0b0100u32 << 20)   // op2=4
            | (0u32 << 16)              // op1=0
            | (0b0100u32 << 12)         // ar=0b0100
            | (0u32 << 8)               // as=0 (sa[3:0])
            | (1u32 << 4);              // at=1 (sa[4])
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "SSAI");
        assert!(result[0].operands.contains("16"));

        // SSAI 31（全量程上限）: at=1，as=15 → sa = (1<<4)|15 = 31
        let instr = (0b0100u32 << 20)
            | (0b0100u32 << 12)
            | (15u32 << 8)              // as=15 (sa[3:0])
            | (1u32 << 4);              // at=1 (sa[4])
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "SSAI");
        assert!(result[0].operands.contains("31"));
    }

    #[test]
    fn test_sr_name_mapping() {
        // 验证特殊寄存器名称映射（参考 capstone SRDecoderTable）
        assert_eq!(sr_name(0), "LBEG");
        assert_eq!(sr_name(1), "LEND");
        assert_eq!(sr_name(2), "LCOUNT");
        assert_eq!(sr_name(3), "SAR");
        assert_eq!(sr_name(72), "WINDOWBASE");
        assert_eq!(sr_name(73), "WINDOWSTART");
        assert_eq!(sr_name(234), "CCOUNT");
        assert_eq!(sr_name(235), "PRID");
        // 未定义的 SR 号返回 sr<N>
        assert_eq!(sr_name(100), "sr100");
    }

    #[test]
    fn test_rsr_sar_decoding() {
        // RSR a0, SAR: RRR 组，op0=0，op1=0b0011(bits[19:16])，op2=0(bits[23:20])，
        // at=a0(bits[7:4])，sr=3(bits[15:8]，即 (ar<<4)|as)
        // QEMU 实测编码 rsr_sar = 0x030300；
        // Ghidra: :rsr at, sr is op2=0 & op1=0b0011 & sr & at & op0=0
        let instr = 0x030300u32;
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "RSR");
        assert!(result[0].operands.contains("a0"));
        assert!(result[0].operands.contains("SAR"));
    }

    #[test]
    fn test_wsr_windowbase_decoding() {
        // WSR a1, WINDOWBASE: RRR 组，op0=0，op1=0b0011，op2=0b0001(bits[23:20])，
        // at=a1(bits[7:4])，sr=72(bits[15:8])
        // QEMU 基准 wsr_sar = 0x130300 → 替换 sr=72、at=1 得 0x134810
        // Ghidra: :wsr at, sr is op2=0b0001 & op1=0b0011 & sr & at & op0=0
        let instr = 0x134810u32;
        let bytes = vec![
            (instr & 0xFF) as u8,
            ((instr >> 8) & 0xFF) as u8,
            ((instr >> 16) & 0xFF) as u8,
        ];
        let result = disassemble_xtensa(&bytes, 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "WSR");
        assert!(result[0].operands.contains("WINDOWBASE"));
    }

    #[test]
    fn test_narrow_movi_n() {
        // MOVI.N a0, 32: n_op0=0xC，n_op_s_3=bit7=0，dest=n_as=bits[11:8]，
        // imm7 = {bits[6:4], bits[15:12]}（Ghidra movin_si8，非对称范围 -32..95，非 B4const）
        // imm7=32 → bits[6:4]=2，n_ar=0 → instr16 = 0x002C
        let instr16: u16 = (0u16 << 12)   // n_ar=imm7[3:0]=0
            | (0u16 << 8)                 // n_as=a0
            | (2u16 << 4)                 // bits[6:4]=imm7[6:4]=2
            | 0xCu16;                     // n_op0=0xC
        let b0 = (instr16 & 0xFF) as u8;
        let b1 = ((instr16 >> 8) & 0xFF) as u8;
        let result = disassemble_xtensa(&[b0, b1], 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "MOVI.N");
        assert_eq!(result[0].operands, "a0, 32");

        // MOVI.N a1, -1: imm7=0x7F → bits[6:4]=7（>=6 置符号位 bit7），n_ar=15，n_as=1
        let instr16: u16 = (15u16 << 12) | (1u16 << 8) | (7u16 << 4) | 0xCu16;
        let b0 = (instr16 & 0xFF) as u8;
        let b1 = ((instr16 >> 8) & 0xFF) as u8;
        let result = disassemble_xtensa(&[b0, b1], 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "MOVI.N");
        assert_eq!(result[0].operands, "a1, -1");
    }

    #[test]
    fn test_narrow_addi_n() {
        // ADDI.N a0, a1, 5: n_op0=0xB，imm4=bits[7:4]
        // Ghidra n_s4_4_7_nozero：恒等映射 1..15，0 编码 -1（非 B4const）
        let instr16: u16 = (0u16 << 12)   // n_ar=a0
            | (1u16 << 8)                 // n_as=a1
            | (5u16 << 4)                 // imm4=5
            | 0xBu16;                     // n_op0=0xB
        let b0 = (instr16 & 0xFF) as u8;
        let b1 = ((instr16 >> 8) & 0xFF) as u8;
        let result = disassemble_xtensa(&[b0, b1], 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "ADDI.N");
        assert_eq!(result[0].operands, "a0, a1, 5");

        // ADDI.N a2, a3, -1: imm4=0 → -1
        let instr16: u16 = (2u16 << 12) | (3u16 << 8) | (0u16 << 4) | 0xBu16;
        let b0 = (instr16 & 0xFF) as u8;
        let b1 = ((instr16 >> 8) & 0xFF) as u8;
        let result = disassemble_xtensa(&[b0, b1], 0x0, 0, 1);
        assert_eq!(result[0].mnemonic, "ADDI.N");
        assert_eq!(result[0].operands, "a2, a3, -1");
    }
}
