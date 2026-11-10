//! 独立反汇编工具：从 bin 文件生成反汇编 .asm 文件
//!
//! Usage:
//!   反汇编: cargo run --release -p miconlab-ai --example disasm_bin -- <input.bin> [output.asm] [base_addr]
//!   重建工程: cargo run --release -p miconlab-ai --example disasm_bin -- --rebuild <input.bin> [output_dir] [base_addr]
//!
//! 芯片特征指纹：自动识别 bin 文件所属芯片/固件，输出文件名使用 {profile_id}_ 前缀隔离

use miconlab_ai::chip_profile::{match_profile, DisasmParams};
use miconlab_ai::rebuild::generate_rebuild_project;
use miconlab_ai::thumb_disasm::{analyze_disassembly, disassemble_thumb_full, DisasmInstruction};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  Disasm:   {} <input.bin> [output.asm] [base_addr]", args[0]);
        eprintln!("  Rebuild:  {} --rebuild <input.bin> [output_dir] [base_addr]", args[0]);
        std::process::exit(1);
    }

    // --rebuild 模式：生成可编译工程
    if args[1] == "--rebuild" {
        if args.len() < 3 {
            eprintln!("Usage: {} --rebuild <input.bin> [output_dir] [base_addr]", args[0]);
            std::process::exit(1);
        }
        run_rebuild(&args[2..]);
        return;
    }

    run_disasm(&args[1..]);
}

/// 执行反汇编模式
fn run_disasm(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: disasm_bin <input.bin> [output.asm] [base_addr]");
        std::process::exit(1);
    }
    let input_path = PathBuf::from(&args[0]);

    let data = match fs::read(&input_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", input_path.display(), e);
            std::process::exit(2);
        }
    };

    // ── 芯片特征指纹匹配 ──
    let match_result = match_profile(&data);
    let profile = match_result.profile;
    let params = DisasmParams::from_profile(profile);

    // 基地址优先级：命令行参数 > profile 参数 > 默认 0x08000000
    let base_addr: u32 = if args.len() >= 3 {
        u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap_or(params.base_addr)
    } else {
        params.base_addr
    };

    // 输出文件名：用户指定 > 自动生成（{profile_id}_disasm_{timestamp}.asm）
    let output_path = if args.len() >= 2 {
        PathBuf::from(&args[1])
    } else {
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dt = chrono_like(ts);
        PathBuf::from(format!("{}disasm_{}.asm", profile.file_prefix(), dt))
    };

    println!("Input file: {} ({} bytes)", input_path.display(), data.len());
    println!("Chip profile: {} (id={})", profile.full_name(), profile.id);
    println!("Match confidence: {:.0}% - {}", match_result.confidence * 100.0, match_result.details);
    if profile.id == "unknown" {
        println!("⚠ No known chip fingerprint matched; using default parameters. To add a new fingerprint, analyze and add it to chip_profile.rs");
    }
    println!("Base address: 0x{:08X} (vector table {} bytes)", base_addr, params.vector_table_size);
    println!("Output file: {}", output_path.display());

    // 全量反汇编
    let instructions = disassemble_thumb_full(&data, base_addr, 0, data.len() / 2 + 1);

    // AI 感知分析（提前到统计之前，以便复用函数计数）
    let analysis = analyze_disassembly(&instructions, base_addr, &data);

    // 统计未解码指令
    let mut undecoded: Vec<&DisasmInstruction> = Vec::new();
    let mut thumb2_count = 0usize;
    let mut thumb16_count = 0usize;
    let mut literal_count = 0usize;
    let mut pad_count = 0usize;
    let mut vector_count = 0usize;
    let mut ascii_count = 0usize;
    let mut data_table_count = 0usize;
    let function_count = analysis.functions.len();

    for inst in &instructions {
        if inst.mnemonic == ".word" {
            if let Some(c) = &inst.comment {
                if c.contains("literal pool") {
                    literal_count += 1;
                } else if c.contains("fill") {
                    pad_count += 1;
                } else if c.contains("Vector") {
                    vector_count += 1;
                } else if c.contains("suspicious data") || c.contains("jump table") || c.contains("data table") {
                    // LDRD R15 等可疑编码，实际是数据表/跳转表数据
                    data_table_count += 1;
                } else {
                    undecoded.push(inst);
                }
            } else {
                undecoded.push(inst);
            }
        } else if inst.mnemonic == ".ascii" {
            ascii_count += 1;
        } else if inst.mnemonic == ".byte" {
            data_table_count += 1;
        } else if inst.mnemonic == ".fill" {
            pad_count += 1;
        } else if inst.mnemonic == ".half" {
            pad_count += 1;
        } else if inst.mnemonic.contains(".W") || inst.bytes.len() == 4 {
            thumb2_count += 1;
        } else if inst.bytes.len() == 2 {
            thumb16_count += 1;
        }
    }

    // 写入文件
    let mut out = String::new();
    out.push_str(&format!("; Disassembly file\n"));
    out.push_str(&format!("; Source file: {}\n", input_path.display()));
    out.push_str(&format!("; File size: {} bytes\n", data.len()));
    out.push_str(&format!("; Chip profile: {} (id={})\n", profile.full_name(), profile.id));
    out.push_str(&format!("; Fingerprint match: {:.0}% - {}\n", match_result.confidence * 100.0, match_result.details));
    if let Some(cal) = &profile.calibration {
        out.push_str(&format!("; Calibration parameter area: SRAM 0x{:08X} ({} bytes)\n", cal.sram_base, cal.size));
        for (off, name) in cal.params {
            out.push_str(&format!(";   +0x{:02X}: {}\n", off, name));
        }
    }
    out.push_str(&format!("; Base address: 0x{:08X}\n", base_addr));
    out.push_str(&format!("; Total instructions: {}\n", instructions.len()));
    out.push_str(&format!("; Functions: {}, Thumb-16: {}, Thumb-2: {}, Vectors: {}, Literal pools: {}, ASCII strings: {}, Data tables: {}, Padding: {}\n",
                          function_count, thumb16_count, thumb2_count, vector_count, literal_count, ascii_count, data_table_count, pad_count));
    out.push_str(&format!("; Undecoded instructions: {}\n", undecoded.len()));

    // AI 感知分析结果输出
    out.push_str(&format!("; \n"));
    out.push_str(&format!("; === AI-Aware Analysis ===\n"));
    out.push_str(&format!("; {}\n", analysis.summary));
    if !analysis.patterns.is_empty() {
        out.push_str(&format!("; \n"));
        out.push_str(&format!("; Code patterns ({}):\n", analysis.patterns.len()));
        for p in &analysis.patterns {
            out.push_str(&format!(";   [{}] {}\n", p.kind_label(), p.description));
        }
    }
    if !analysis.functions.is_empty() {
        out.push_str(&format!("; \n"));
        out.push_str(&format!("; Function list ({} entries, sorted by address):\n", analysis.functions.len()));
        for f in analysis.functions.iter().take(50) {
            let caller_count = f.callers.len();
            let callee_count = f.callees.len();
            let handler_tag = if f.is_handler { " [interrupt handler]" } else { "" };
            out.push_str(&format!(";   0x{:08X} {} ({}B, {} callees, {} callers){}\n",
                                 f.entry, f.name, f.size, callee_count, caller_count, handler_tag));
        }
        if analysis.functions.len() > 50 {
            out.push_str(&format!(";   ... {} more functions\n", analysis.functions.len() - 50));
        }
    }
    out.push_str("; ──────────────────────────────────────\n\n");

    // 向量表（前 0x1C 字节，STM32 标准）
    if data.len() >= 0x1C {
        out.push_str("; Exception vector table\n");
        let vectors = [
            ("Initial SP", 0x00),
            ("Reset", 0x04),
            ("NMI", 0x08),
            ("HardFault", 0x0C),
            ("MemManage", 0x10),
            ("BusFault", 0x14),
            ("UsageFault", 0x18),
        ];
        for (name, off) in vectors {
            let v = u32::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3]]);
            out.push_str(&format!(";   {:<12} @0x{:02X}: 0x{:08X}\n", name, off, v));
        }
        out.push_str("\n");
    }

    // 输出指令
    for inst in &instructions {
        out.push_str(&inst.format());
        out.push('\n');
    }

    // 输出未解码指令清单
    if !undecoded.is_empty() {
        out.push_str("\n; ──────────────────────────────────────\n");
        out.push_str(&format!("; Undecoded instruction list ({} entries)\n", undecoded.len()));
        out.push_str("; ──────────────────────────────────────\n");
        for inst in &undecoded {
            out.push_str(&format!("; 0x{:08X}: {}  {}\n",
                                  inst.addr,
                                  inst.operands,
                                  inst.comment.as_ref().map(|c| c.as_str()).unwrap_or("")));
        }
    }

    match fs::write(&output_path, out) {
        Ok(_) => {
            println!("✓ Wrote disassembly file: {}", output_path.display());
            println!("  Total instructions: {}", instructions.len());
            println!("  Functions: {}, Thumb-16: {}, Thumb-2: {}, Vectors: {}, Literal pools: {}, ASCII strings: {}, Data tables: {}, Padding: {}",
                     function_count, thumb16_count, thumb2_count, vector_count, literal_count, ascii_count, data_table_count, pad_count);
            println!("  Undecoded instructions: {}", undecoded.len());
            if !undecoded.is_empty() {
                println!("\nUndecoded instruction preview (first 20):");
                for (i, inst) in undecoded.iter().take(20).enumerate() {
                    println!("  {}: 0x{:08X}: {} {}",
                             i + 1, inst.addr, inst.operands,
                             inst.comment.as_ref().map(|c| c.as_str()).unwrap_or(""));
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to write file: {}", e);
            std::process::exit(3);
        }
    }
}

/// 执行重建工程模式：生成可编译汇编工程
fn run_rebuild(args: &[String]) {
    let input_path = PathBuf::from(&args[0]);

    let data = match fs::read(&input_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", input_path.display(), e);
            std::process::exit(2);
        }
    };

    // ── 芯片特征指纹匹配 ──
    let match_result = match_profile(&data);
    let profile = match_result.profile;
    let params = DisasmParams::from_profile(profile);

    // 基地址优先级：命令行参数 > profile 参数 > 默认 0x08000000
    let base_addr: u32 = if args.len() >= 3 {
        u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap_or(params.base_addr)
    } else {
        params.base_addr
    };

    // 输出目录：用户指定 > 自动生成（{profile_id}_project）
    let output_dir = if args.len() >= 2 {
        PathBuf::from(&args[1])
    } else {
        PathBuf::from(format!("{}project", profile.file_prefix()))
    };

    // 提取初始 SP（向量表第 0 项）
    let sp_init = if data.len() >= 4 {
        u32::from_le_bytes([data[0], data[1], data[2], data[3]])
    } else {
        0x20003000
    };

    println!("Input file: {} ({} bytes)", input_path.display(), data.len());
    println!("Chip profile: {} (id={})", profile.full_name(), profile.id);
    println!("Match confidence: {:.0}% - {}", match_result.confidence * 100.0, match_result.details);
    if profile.id == "unknown" {
        println!("⚠ No known chip fingerprint matched; using default parameters. To add a new fingerprint, analyze and add it to chip_profile.rs");
    }
    println!("Base address: 0x{:08X}", base_addr);
    println!("Initial SP: 0x{:08X}", sp_init);
    println!("Output directory: {}", output_dir.display());

    // 生成可编译工程
    let project = generate_rebuild_project(&data, base_addr, sp_init);

    // 创建输出目录
    if let Err(e) = fs::create_dir_all(&output_dir) {
        eprintln!("Failed to create directory {}: {}", output_dir.display(), e);
        std::process::exit(4);
    }

    // 写入各文件
    let files = [
        ("firmware.s", &project.firmware_s),
        ("firmware.ld", &project.firmware_ld),
        ("Makefile", &project.makefile),
        ("README.md", &project.readme),
    ];

    for (name, content) in &files {
        let path = output_dir.join(name);
        match fs::write(&path, content) {
            Ok(_) => println!("  ✓ {}", name),
            Err(e) => {
                eprintln!("Failed to write {}: {}", name, e);
                std::process::exit(5);
            }
        }
    }

    println!("\n✓ Rebuild project generated: {}", output_dir.display());
    println!("  Label count: {}", project.label_count);
    println!("  LDR [PC,#imm] conversions: {}", project.ldr_literal_converted);
    println!("  Branch targets resolved: {}", project.branch_resolved);
    println!("\nNext steps:");
    println!("  cd {}", output_dir.display());
    println!("  make    # build firmware.bin");
    println!("  make burn  # flash to chip (stlink required)");
}

/// 简单的时间戳转 "YYYYMMDD_HHMMSS" 格式
fn chrono_like(ts: u64) -> String {
    // UTC 时间，简化实现
    let secs = ts;
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;

    // 从 1970-01-01 计算年月日（简化算法，足够使用）
    let (y, mo, d) = days_to_ymd(days as i64);
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", y, mo, d, h, m, s)
}

fn days_to_ymd(mut days: i64) -> (i64, u32, u32) {
    let mut year: i64 = 1970;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if days < dy { break; }
        days -= dy;
        year += 1;
    }
    let months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month: u32 = 1;
    for (i, &md) in months.iter().enumerate() {
        let md = if i == 1 && is_leap(year) { 29 } else { md };
        if days < md { break; }
        days -= md;
        month += 1;
    }
    (year, month, (days + 1) as u32)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
