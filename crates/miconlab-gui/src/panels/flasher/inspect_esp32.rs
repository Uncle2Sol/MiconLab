//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
use miconlab_ai::thumb_disasm::DisasmInstruction;
pub(crate) fn inspect_esp32_firmware(fs: &mut FlasherState, data: &[u8], base_addr: u32, _zh: bool) {
    let size = data.len();

    // ===== Image Header 解析 =====
    let seg_count = data[1];
    let spi_mode = data[2];
    let spi_speed_size = data[3];
    let entry = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

    // SPI mode 解读
    let spi_mode_str = match spi_mode {
        0 => "QIO",
        1 => "QOUT",
        2 => "DIO",
        3 => "DOUT",
        _ => "Unknown",
    };
    // SPI speed（高 4 位）+ size（低 4 位）
    let spi_speed = (spi_speed_size >> 4) & 0xF;
    let spi_size = spi_speed_size & 0xF;
    let spi_speed_str = match spi_speed {
        0 => "40MHz",
        1 => "26MHz",
        2 => "20MHz",
        3 => "80MHz",
        _ => "Unknown",
    };
    let spi_size_str = match spi_size {
        0 => "1MB",
        1 => "2MB",
        2 => "4MB",
        3 => "8MB",
        4 => "16MB",
        _ => "Unknown",
    };

    // chip ID（bytes 12-13, little-endian u16）
    let chip_id = if data.len() >= 14 {
        u16::from_le_bytes([data[12], data[13]])
    } else {
        0
    };
    let chip_name = match chip_id {
        0x0000 => "ESP32",
        0x0002 => "ESP32-S2",
        0x0009 => "ESP32-S3",
        0x000D => "ESP32-C3",
        0x0012 => "ESP32-H2",
        0x002D => "ESP32-C2",
        _ => crate::i18n::t("ESP32 (unknown model)"),
    };

    log_push(fs, format!("  {} 0xE9 (ESP32 image)",
        crate::i18n::t("Magic:")));
    log_push(fs, format!("  {} {} (chip_id=0x{:04X})",
        crate::i18n::t("Chip:"), chip_name, chip_id));
    log_push(fs, format!("  {} {}",
        crate::i18n::t("Segments:"), seg_count));
    log_push(fs, format!("  {} SPI {} / {}",
        { crate::i18n::t("Flash:") }, spi_mode_str, format!("{} {}", spi_speed_str, spi_size_str)));
    log_push(fs, format!("  {} 0x{:08X}",
        crate::i18n::t("Entry:"), entry));

    // 扩展 header
    if data.len() >= 24 {
        let wp_pin = data[8];
        let min_chip_rev = if data.len() >= 16 {
            u16::from_le_bytes([data[14], data[15]])
        } else {
            0
        };
        log_push(fs, format!("  {} WP=0x{:02X}, min_rev={}",
            crate::i18n::t("Ext:"), wp_pin, min_chip_rev));
    }

    // ===== 芯片指纹匹配 =====
    let mr = chip_profile::match_profile(data);
    log_push(fs, format!("  {}: {} ({:.0}%)",
        crate::i18n::t("Profile"),
        mr.profile.full_name(), mr.confidence * 100.0));

    // ===== 段信息解析 =====
    // 扩展 header 后（offset=24）开始段头，每个段头 8 字节
    let mut offset = 24usize;
    let mut seg_infos: Vec<(u32, usize, usize)> = Vec::new(); // (load_addr, data_len, header_offset_in_file)
    log_push(fs, crate::i18n::t("  Segments:"));
    for i in 0..seg_count.min(16) {
        if offset + 8 > data.len() {
            log_push(fs, crate::i18n::tf("    [{}] ⚠ Segment header exceeds the data range",
                &[("{}", "{}", &format!("{}", i))]));
            break;
        }
        let load_addr = u32::from_le_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
        ]);
        let seg_len = u32::from_le_bytes([
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ]) as usize;
        let seg_data_offset = offset + 8;
        log_push(fs, format!("    [{}] load=0x{:08X}, size={}{}",
            i, load_addr, seg_len,
            if seg_data_offset + seg_len > data.len() {
                crate::i18n::t(" (incomplete)")
            } else { "" }));
        seg_infos.push((load_addr, seg_len, seg_data_offset));
        offset = seg_data_offset + seg_len;
    }

    // ===== 入口点 Xtensa 反汇编 =====
    // 在段信息中查找包含 entry 地址的段，计算其在文件中的偏移
    let entry_file_offset: Option<usize> = seg_infos.iter().find_map(|(load, len, data_off)| {
        if entry >= *load && entry < *load + *len as u32 {
            let off = *data_off + (entry - *load) as usize;
            if off < data.len() { Some(off) } else { None }
        } else {
            None
        }
    });

    if let Some(start_off) = entry_file_offset {
        log_push(fs, format!("  {} (0x{:08X}):",
            crate::i18n::t("Disasm Entry"), entry));
        // base_addr 应传 entry 的虚拟地址，offset 传 entry 在文件中的偏移
        // disassemble_xtensa 内部计算 addr = base_addr + (pos - offset) = entry + 段内偏移
        let instructions = xtensa_disasm::disassemble_xtensa(data, entry, start_off, 24);
        for inst in &instructions {
            log_push(fs, inst.format());
        }

        // 函数调用统计（CALL0/4/8/12）
        let call_targets = xtensa_disasm::collect_calls(&instructions);
        if !call_targets.is_empty() {
            log_push(fs, format!("  {} {}",
                crate::i18n::t("Function calls:"), call_targets.len()));
            for tgt in call_targets.iter().take(8) {
                log_push(fs, format!("    CALL → 0x{:08X}", tgt));
            }
            if call_targets.len() > 8 {
                log_push(fs, crate::i18n::tf("    ... {} calls total (showing first 8)", &[("{}", "{}", &format!("{}", call_targets.len()))]));
            }
        }
    } else {
        // 入口不在任何段中：优先找第一个 IRAM（代码）段做反汇编预览，避免把 DRAM 数据段当代码
        let code_seg = seg_infos.iter().find(|(load, _, _)| is_iram_addr(*load));
        let target_seg = code_seg.or_else(|| seg_infos.first());
        if let Some((load, len, data_off)) = target_seg {
            let preview_len = (*len).min(72);
            let seg_label = if is_iram_addr(*load) {
                crate::i18n::t("code segment")
            } else {
                crate::i18n::t("segment 0 (data)")
            };
            log_push(fs, format!("  {} ({} @0x{:08X}):",
                crate::i18n::t("Disasm"),
                seg_label, *load));
            // base_addr 传段的 load_addr，disassemble_xtensa 计算 addr = load + 段内偏移
            let instructions = xtensa_disasm::disassemble_xtensa(
                data, *load, *data_off, preview_len / 3,
            );
            for inst in &instructions {
                log_push(fs, inst.format());
            }
        } else {
            log_push(fs, crate::i18n::t("  ⚠ No segment to disassemble"));
        }
    }

    // ===== 数据特征分析 =====
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

    // ===== Flash 使用率 =====
    if ff_count > 0 {
        let last_valid = data.iter().rposition(|&b| b != 0xFF).unwrap_or(0);
        let used_kb = (last_valid + 1 + 1023) / 1024;
        let total_kb = (size + 1023) / 1024;
        log_push(fs, format!("  {} 0x{:08X}-0x{:08X} ({}KB/{}KB, {}%{})",
            crate::i18n::t("Code region:"),
            base_addr, base_addr + last_valid as u32,
            used_kb, total_kb, pct(size - ff_count),
            if ff_count > size * 3 / 4 {
                crate::i18n::t(", mostly empty")
            } else { "" }));
    }

    // ===== 字符串提取（长度 >= 6，含 4+ 连续字母）=====
    let mut strings_found = 0;
    let mut current = String::new();
    for &b in data {
        if (0x20..=0x7E).contains(&b) {
            current.push(b as char);
        } else {
            if current.len() >= 6 && has_consecutive_alpha(&current, 4) {
                if strings_found < 5 {
                    log_push(fs, format!("  \"{}\"", current));
                }
                strings_found += 1;
            }
            current.clear();
        }
    }
    if current.len() >= 6 && has_consecutive_alpha(&current, 4) {
        if strings_found < 5 {
            log_push(fs, format!("  \"{}\"", current));
        }
        strings_found += 1;
    }
    if strings_found > 5 {
        log_push(fs, crate::i18n::tf("  ... {} strings total (showing first 5)", &[("{}", "{}", &format!("{}", strings_found))]));
    } else if strings_found == 0 {
        log_push(fs, crate::i18n::t("  No printable strings"));
    }

    // ===== HEX dump 预览（前 32 字节）=====
    let preview_len = data.len().min(32);
    let hex_preview: String = data[..preview_len]
        .iter()
        .map(|b| format!("{:02X} ", b))
        .collect();
    log_push(fs, format!("<< HEX [0x{:08X}]: {}", base_addr, hex_preview.trim_end()));

    log_push(fs, crate::i18n::t("✓ ESP32 firmware analysis done"));
}

/// 导出 ESP32/Xtensa 固件反汇编到文本文件
///
/// ESP32 固件格式与 ARM 不同：
/// - 以 0xE9 魔数开头（ESP32 image header）
/// - 没有 ARM 向量表
/// - 指令集为 Xtensa LX6/LX7（非 ARM Thumb）
/// 输出：image header 解析 + 段信息 + 各段 Xtensa 指令反汇编 + 函数调用统计
pub(crate) fn export_esp32_disasm_to_file(fs: &mut FlasherState, is_zh: bool) {
    let data = &fs.last_read_data;
    let base_addr = fs.address;

    let mut output = String::new();

    // 文件头
    output.push_str(crate::i18n::t("disasm.esp32.title"));
    output.push_str(&crate::i18n::tf("disasm.esp32.file_size", &[("{}", "{}", &format!("{}", data.len()))]));
    output.push_str(&crate::i18n::tf("disasm.header.base_addr", &[("{:08X}", "{:08X}", &format!("{:08X}", base_addr))]));
    output.push_str(&crate::i18n::tf("disasm.esp32.chip_type", &[("{}", "{}", &format!("{}", fs.chip_type.label(is_zh)))]));
    output.push_str(crate::i18n::t("disasm.esp32.isa"));
    output.push_str("; ──────────────────────────────────────\n\n");

    // 未能识别为 ESP32 image 时直接降级为 hex dump
    if !(data.len() >= 8 && data[0] == 0xE9) {
        output.push_str(crate::i18n::t("disasm.esp32.no_header"));
        output.push_str(crate::i18n::t("disasm.esp32.not_firmware"));
        output.push_str("; === Hex Dump ===\n");
        let dump_len = data.len().min(4096);
        for i in (0..dump_len).step_by(16) {
            let end = (i + 16).min(dump_len);
            let hex: String = data[i..end].iter().map(|b| format!("{:02X} ", b)).collect();
            let ascii: String = data[i..end].iter()
                .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                .collect();
            output.push_str(&format!("  0x{:08X}: {:<48} ; {}\n", base_addr + i as u32, hex.trim_end(), ascii));
        }
        if data.len() > 4096 {
            output.push_str(&crate::i18n::tf("disasm.esp32.truncated", &[("{}", "{}", &format!("{}", data.len()))]));
        }
        write_esp32_disasm_file(fs, is_zh, &output);
        return;
    }

    // ===== ESP32 image header 解析 =====
    let seg_count = data[1];
    let spi_mode = data[2];
    let spi_speed_size = data[3];
    let entry = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    // chip_id（bytes 12-13, little-endian u16），与 inspect_esp32_firmware 保持一致
    let chip_id = if data.len() >= 14 {
        u16::from_le_bytes([data[12], data[13]])
    } else {
        0
    };
    let chip_name = match chip_id {
        0x0000 => "ESP32",
        0x0002 => "ESP32-S2",
        0x0009 => "ESP32-S3",
        0x000D => "ESP32-C3",
        0x0012 => "ESP32-H2",
        0x002D => "ESP32-C2",
        _ => crate::i18n::t("ESP32 (unknown model)"),
    };

    output.push_str("; === ESP32 Image Header ===\n");
    output.push_str(&format!(";   Magic:        0xE9 (ESP32 image)\n"));
    output.push_str(&format!(";   Chip:         {} (chip_id=0x{:04X})\n", chip_name, chip_id));
    output.push_str(&format!(";   Segments:     {}\n", seg_count));
    output.push_str(&format!(";   SPI Mode:     0x{:02X}\n", spi_mode));
    output.push_str(&format!(";   SPI Speed/Size: 0x{:02X}\n", spi_speed_size));
    output.push_str(&format!(";   Entry Addr:   0x{:08X}\n", entry));
    if data.len() >= 16 {
        output.push_str(&format!(";   WP Pin:       0x{:02X}\n", data[8]));
        output.push_str(&format!(";   Drive Settings: 0x{:02X} 0x{:02X} 0x{:02X} 0x{:02X}\n",
            data[9], data[10], data[11], data[12]));
    }

    // 匹配芯片指纹
    let match_result = chip_profile::match_profile(&data[..]);
    output.push_str(crate::i18n::t("disasm.esp32.profile_section"));
    output.push_str(&format!(";   Profile: {} ({})\n", match_result.profile.id, match_result.profile.full_name()));
    output.push_str(&crate::i18n::tf("disasm.esp32.confidence", &[("{:.0}", "{:.0}", &format!("{:.0}", match_result.confidence * 100.0))]));
    output.push_str(&crate::i18n::tf("disasm.esp32.details", &[("{}", "{}", &format!("{}", match_result.details))]));

    // ===== 段解析 + 反汇编 =====
    // 每个段头 8 字节：load_addr(4) + data_len(4)
    let mut offset = 24usize; // 扩展 header 后开始
    let mut seg_infos: Vec<(u32, usize, usize)> = Vec::new(); // (load_addr, seg_len, data_offset)
    output.push_str(crate::i18n::t("disasm.esp32.segments_section"));
    for i in 0..seg_count.min(16) {
        if offset + 8 > data.len() { break; }
        let load_addr = u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]);
        let seg_len = u32::from_le_bytes([data[offset+4], data[offset+5], data[offset+6], data[offset+7]]) as usize;
        let seg_data_offset = offset + 8;
        output.push_str(&crate::i18n::tf("disasm.esp32.segment_line",
            &[("{}", "{}", &format!("{}", i)),
              ("{:08X}", "{:08X}", &format!("{:08X}", load_addr)),
              ("{}", "{}", &format!("{}", seg_len)),
              ("{}", "{}", &format!("{}", if seg_data_offset + seg_len > data.len() { crate::i18n::t(" ⚠ data incomplete") } else { "" }))]));
        seg_infos.push((load_addr, seg_len, seg_data_offset));
        offset = seg_data_offset + seg_len;
    }

    // ===== 各段 Xtensa 反汇编 =====
    // ESP32 段 load_addr 地址范围判断段类型：
    //   IRAM (代码段): 0x40370000-0x403E0000 (ESP32-S3) / 0x40070000-0x40100000 (ESP32)
    //   DRAM (数据段): 0x3FCA0000-0x3FCE0000 / 0x3FF80000-0x40000000
    //   DROM (Flash 数据): 0x3C000000-0x3C800000
    //
    // 代码段反汇编策略：
    //   1. 先对段开头采样 32 条指令，计算 unknown 比例（data_ratio）
    //   2. 若 data_ratio > 0.5，判定段开头为数据区域（地址表/常量表），
    //      输出 hex dump 并跳到 entry point 开始反汇编
    //   3. 若 entry point 在该段内，从 entry point 开始反汇编（真正的代码入口）
    //   4. 否则从段开头开始反汇编
    let mut total_calls = 0u32;
    for (i, (load, len, data_off)) in seg_infos.iter().enumerate() {
        if *len == 0 || *data_off >= data.len() { continue; }
        let avail = (*len).min(data.len().saturating_sub(*data_off));

        // 判断段类型：IRAM 地址范围视为代码段，其余视为数据段
        let is_code = is_iram_addr(*load);

        if is_code {
            let max_insts = (avail / 3).min(4096);
            if max_insts == 0 { continue; }

            // 0. 检测段开头的填充区域（0xFF/0x00 填充，常见于 Flash 未使用区域）
            // 0xFF 填充会被解码为 TIE-E (op0=15)，导致 data_ratio 检测失效
            // 需要先跳过填充字节，找到真正的代码起始位置
            let mut fill_skip = 0usize;
            let seg_data_start = &data[*data_off..*data_off + avail.min(256)];
            for &b in seg_data_start {
                if b == 0xFF || b == 0x00 {
                    fill_skip += 1;
                } else {
                    break;
                }
            }
            // 如果连续填充 >= 8 字节（至少 2 条指令），判定为填充区域
            let has_fill_at_start = fill_skip >= 8;

            // 0b. 检测段开头的地址表（函数指针表/常量表）
            // 地址表特征：连续多个4字节值，都在合理的内存范围内
            // 地址表会被解码为各种指令（PIE/MAC16/ISYNC等），导致 data_ratio 检测失效
            // 改进：1) 扩大地址范围(含外设0x60000000-0x60100000)
            //       2) 扩大地址范围(含带标志Flash XIP地址 0xBF000000-0xBFFFFFFF)
            //       3) 允许中间夹杂少量非地址值（tolerance=3），不立即break
            //       4) 使用 last_addr_end 记录最后一个有效地址位置，跳过整个地址表
            let mut addr_table_skip = 0usize;
            let check_len = avail.min(512);  // 检查前512字节（128个地址）
            if *data_off + check_len <= data.len() && check_len >= 4 {
                let mut addr_count = 0usize;
                let mut last_addr_end = 0usize;  // 最后一个有效地址的结束偏移
                let mut consecutive_non_addr = 0usize;
                for off in (0..check_len).step_by(4) {
                    if *data_off + off + 4 > data.len() { break; }
                    let val = u32::from_le_bytes([
                        data[*data_off + off],
                        data[*data_off + off + 1],
                        data[*data_off + off + 2],
                        data[*data_off + off + 3],
                    ]);
                    // 排除明显的填充值（0x00000000/0xFFFFFFFF）
                    if val == 0 || val == 0xFFFFFFFF {
                        consecutive_non_addr += 1;
                        if consecutive_non_addr >= 4 { break; }
                        continue;
                    }
                    // 检查是否为合理的内存地址
                    // DRAM/DROM: 0x3F000000-0x3FE00000
                    // IRAM/IROM: 0x40000000-0x40400000
                    // 外设:      0x60000000-0x60100000
                    // 带标志Flash XIP: 0xBF000000-0xBFFFFFFF (ESP32 cache标志位)
                    //   (0x3F000000-0x3FE00000 | 0x80000000)
                    let is_dram = (0x3F000000..=0x3FE00000).contains(&val);
                    let is_iram = (0x40000000..=0x40400000).contains(&val);
                    let is_periph = (0x60000000..=0x60100000).contains(&val);
                    let is_flagged_flash = (0xBF000000..=0xBFFFFFFF).contains(&val);
                    let is_valid_addr = is_dram || is_iram || is_periph || is_flagged_flash;

                    if is_valid_addr {
                        addr_count += 1;
                        consecutive_non_addr = 0;
                        last_addr_end = off + 4;
                    } else {
                        consecutive_non_addr += 1;
                        // 连续4个非地址值，判定地址表结束
                        if consecutive_non_addr >= 4 {
                            break;
                        }
                    }
                }
                // 有效地址 >= 4 个（16字节），判定为地址表
                // 跳过范围到最后一个有效地址的结束位置（包含中间夹杂的非地址值）
                if addr_count >= 4 && last_addr_end >= 16 {
                    addr_table_skip = last_addr_end;
                    // 对齐到4字节边界
                    addr_table_skip = (addr_table_skip + 3) & !3;
                }
            }
            let has_addr_table_at_start = addr_table_skip >= 16;

            // 综合判断：有填充或地址表
            let data_skip = fill_skip.max(addr_table_skip);
            let has_data_at_start = has_fill_at_start || has_addr_table_at_start;
            let fill_adjusted_off = if has_data_at_start {
                *data_off + data_skip
            } else {
                *data_off
            };
            let fill_adjusted_avail = if has_data_at_start {
                avail.saturating_sub(data_skip)
            } else {
                avail
            };
            let fill_adjusted_load = if has_data_at_start {
                *load + data_skip as u32
            } else {
                *load
            };

            // 1. 采样段开头 32 条指令，检测是否为数据区域（地址表）
            // 使用填充调整后的偏移
            let sample_max = (fill_adjusted_avail / 3).min(32);
            let sample = if sample_max > 0 {
                xtensa_disasm::disassemble_xtensa(data, fill_adjusted_load, fill_adjusted_off, sample_max)
            } else {
                Vec::new()
            };
            let ratio = xtensa_disasm::data_ratio(&sample);

            // 2. 计算 entry point 在该段内的偏移（如果有）
            let entry_off_in_seg: Option<usize> = if entry >= *load && entry < *load + *len as u32 {
                Some((entry - *load) as usize)
            } else {
                None
            };

            // 3. 判断段开头是否为数据区域（填充、地址表或高unknown比例）
            let is_data_at_start = has_data_at_start || ratio > 0.5;

            if is_data_at_start {
                // 段开头是数据（地址表/填充），输出 hex dump 标注
                let data_end = entry_off_in_seg.unwrap_or(data_skip.max(avail.min(256)));
                // clamp 到实际剩余字节数：固件可能被截断（声明 len > 实际 avail），越界切片会 panic
                let dump_len = data_end.min(256).min(avail);
                output.push_str(&crate::i18n::tf("disasm.esp32.code_seg_data_head",
                    &[("{}", "{}", &format!("{}", i)),
                      ("{:08X}", "{:08X}", &format!("{:08X}", *load)),
                      ("{}", "{}", &format!("{}", avail))]));
                if has_addr_table_at_start {
                    output.push_str(&crate::i18n::tf("disasm.esp32.addr_table_skip",
                        &[("{}", "{}", &format!("{}", addr_table_skip))]));
                } else if has_fill_at_start {
                    output.push_str(&crate::i18n::tf("disasm.esp32.fill_skip",
                        &[("{}", "{}", &format!("{}", fill_skip))]));
                } else {
                    output.push_str(&crate::i18n::tf("disasm.esp32.table_skip",
                        &[("{}", "{}", &format!("{}", dump_len)),
                          ("{:.0}", "{:.0}", &format!("{:.0}", ratio * 100.0))]));
                }
                let seg_data = &data[*data_off..*data_off + dump_len];
                for off in (0..dump_len).step_by(16) {
                    let end = (off + 16).min(dump_len);
                    let hex: String = seg_data[off..end].iter().map(|b| format!("{:02X} ", b)).collect();
                    let ascii: String = seg_data[off..end].iter()
                        .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                        .collect();
                    output.push_str(&format!(";   0x{:08X}: {:<48} ; {}\n",
                        load + off as u32, hex.trim_end(), ascii));
                }

                // 如果 entry point 在该段内，从 entry point 开始反汇编
                if let Some(entry_off) = entry_off_in_seg {
                    let entry_data_off = *data_off + entry_off;
                    let entry_avail = avail.saturating_sub(entry_off);
                    let entry_max_insts = (entry_avail / 3).min(4096);
                    if entry_max_insts > 0 {
                        output.push_str(&crate::i18n::tf("disasm.esp32.seg_disasm_entry",
                            &[("{}", "{}", &format!("{}", i)),
                              ("{:08X}", "{:08X}", &format!("{:08X}", entry)),
                              ("{}", "{}", &format!("{}", entry_avail))]));
                        let instructions = xtensa_disasm::disassemble_xtensa(
                            data, entry, entry_data_off, entry_max_insts,
                        );
                        // 智能数据区域检测（阈值=2：连续>=2条未识别/填充指令输出hex dump）
                        let (output_lines, data_region_count, scatter_db, seg_calls) =
                            format_disasm_with_data_regions(&instructions, data, entry_data_off, 2);
                        for line in &output_lines {
                            output.push_str(line);
                            output.push('\n');
                        }

                        let ext_count = instructions.iter()
                            .filter(|i| xtensa_disasm::is_extension(i) || xtensa_disasm::is_mac16(i))
                            .count();
                        // 保留编码(RRR.SPEC/RRR.RES/RRI4.RES/NARROW.D/RES.*)已算作TIE扩展
                        // scatter_db 仅含 trailing bytes（不完整指令），非扩展指令
                        let ident_count = instructions.len() - data_region_count - scatter_db - ext_count;
                        // 识别率 = (已识别 + 扩展指令) / (总指令 - 数据区域)
                        // 扩展指令含 TIE/PIE/MAC16/RES.* 及个别 TIE-E 填充
                        let effective_total = instructions.len() - data_region_count;
                        let rate = if effective_total > 0 {
                            (ident_count + ext_count) * 100 / effective_total
                        } else { 0 };
                        output.push_str(&crate::i18n::tf("disasm.esp32.seg_stats",
                            &[("{}", "{}", &format!("{}", i)),
                              ("{}", "{}", &format!("{}", instructions.len())),
                              ("{}", "{}", &format!("{}", ident_count)),
                              ("{}", "{}", &format!("{}", ext_count)),
                              ("{}", "{}", &format!("{}", data_region_count)),
                              ("{}", "{}", &format!("{}", scatter_db)),
                              ("{}", "{}", &format!("{}", rate))]));

                        if !seg_calls.is_empty() {
                            output.push_str(&crate::i18n::tf("disasm.esp32.seg_calls",
                                &[("{}", "{}", &format!("{}", i)),
                                  ("{}", "{}", &format!("{}", seg_calls.len()))]));
                            for tgt in seg_calls.iter().take(64) {
                                output.push_str(&format!(";   CALL → 0x{:08X}\n", tgt));
                            }
                            if seg_calls.len() > 64 {
                                output.push_str(&crate::i18n::tf("disasm.esp32.calls_truncated",
                                    &[("{}", "{}", &format!("{}", seg_calls.len()))]));
                            }
                            total_calls = total_calls.saturating_add(seg_calls.len() as u32);
                        }
                    }
                    continue;
                }
                // entry 不在该段：如果有填充/地址表，从数据区域后开始反汇编；否则跳过该段
                if !has_data_at_start {
                    continue;
                }
                // 有数据区域但无 entry：fall through 到标准反汇编，使用调整后的偏移
            }

            // 标准反汇编：从段开头（或 entry point，或数据区域后）开始
            let (disasm_base, disasm_off, disasm_label) = if let Some(entry_off) = entry_off_in_seg {
                let entry_data_off = *data_off + entry_off;
                let entry_avail = avail.saturating_sub(entry_off);
                (entry, entry_data_off, crate::i18n::tf("starting at entry 0x{:08X}, {} bytes",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", entry)),
                      ("{}", "{}", &format!("{}", entry_avail))]))
            } else if has_data_at_start {
                (fill_adjusted_load, fill_adjusted_off, crate::i18n::tf("after skipping {} bytes of data, {} bytes, code segment",
                    &[("{}", "{}", &format!("{}", data_skip)),
                      ("{}", "{}", &format!("{}", fill_adjusted_avail))]))
            } else {
                (*load, *data_off, crate::i18n::tf("{} bytes, code segment",
                    &[("{}", "{}", &format!("{}", avail))]))
            };

            let disasm_avail = avail.saturating_sub(disasm_off - *data_off);
            let disasm_max = (disasm_avail / 3).min(4096);
            if disasm_max == 0 { continue; }

            output.push_str(&crate::i18n::tf("disasm.esp32.seg_disasm",
                &[("{}", "{}", &format!("{}", i)),
                  ("{:08X}", "{:08X}", &format!("{:08X}", disasm_base)),
                  ("{}", "{}", &format!("{}", disasm_label))]));
            let instructions = xtensa_disasm::disassemble_xtensa(data, disasm_base, disasm_off, disasm_max);

            // 智能数据区域检测（阈值=2：连续>=2条未识别/填充指令输出hex dump）
            let (output_lines, data_region_count, scatter_db, seg_calls) = format_disasm_with_data_regions(
                &instructions, data, disasm_off, 2,
            );
            for line in &output_lines {
                output.push_str(line);
                output.push('\n');
            }

            // 统计信息
            let ext_count = instructions.iter()
                .filter(|i| xtensa_disasm::is_extension(i) || xtensa_disasm::is_mac16(i))
                .count();
            // 保留编码(RRR.SPEC/RRR.RES/RRI4.RES/NARROW.D/RES.*)已算作TIE扩展
            // scatter_db 仅含 trailing bytes（不完整指令），非扩展指令
            let ident_count = instructions.len() - data_region_count - scatter_db - ext_count;
            // 识别率 = (已识别 + 扩展指令) / (总指令 - 数据区域)
            // 扩展指令含 TIE/PIE/MAC16/RES.* 及个别 TIE-E 填充
            let effective_total = instructions.len() - data_region_count;
            let rate = if effective_total > 0 {
                (ident_count + ext_count) * 100 / effective_total
            } else { 0 };
            output.push_str(&crate::i18n::tf("disasm.esp32.seg_stats",
                &[("{}", "{}", &format!("{}", i)),
                  ("{}", "{}", &format!("{}", instructions.len())),
                  ("{}", "{}", &format!("{}", ident_count)),
                  ("{}", "{}", &format!("{}", ext_count)),
                  ("{}", "{}", &format!("{}", data_region_count)),
                  ("{}", "{}", &format!("{}", scatter_db)),
                  ("{}", "{}", &format!("{}", rate))]));

            // 函数调用统计
            if !seg_calls.is_empty() {
                output.push_str(&crate::i18n::tf("disasm.esp32.seg_calls",
                    &[("{}", "{}", &format!("{}", i)),
                      ("{}", "{}", &format!("{}", seg_calls.len()))]));
                for tgt in seg_calls.iter().take(64) {
                    output.push_str(&format!(";   CALL → 0x{:08X}\n", tgt));
                }
                if seg_calls.len() > 64 {
                    output.push_str(&crate::i18n::tf("disasm.esp32.calls_truncated",
                        &[("{}", "{}", &format!("{}", seg_calls.len()))]));
                }
                total_calls = total_calls.saturating_add(seg_calls.len() as u32);
            }
        } else {
            // 数据段：不反汇编，输出类型标注 + ASCII 字符串预览 + 前 64 字节 hex dump
            let seg_type = segment_type_name(*load);
            output.push_str(&crate::i18n::tf("disasm.esp32.data_seg",
                &[("{}", "{}", &format!("{}", i)),
                  ("{:08X}", "{:08X}", &format!("{:08X}", *load)),
                  ("{}", "{}", &format!("{}", avail)),
                  ("{}", "{}", &format!("{}", seg_type))]));
            // 提取可打印字符串（长度 >= 4）
            let seg_data = &data[*data_off..*data_off + avail];
            let mut strings: Vec<String> = Vec::new();
            let mut current = String::new();
            for &b in seg_data {
                if (0x20..=0x7E).contains(&b) {
                    current.push(b as char);
                } else {
                    if current.len() >= 4 {
                        strings.push(current.clone());
                        if strings.len() >= 32 { break; }
                    }
                    current.clear();
                }
            }
            if current.len() >= 4 && strings.len() < 32 {
                strings.push(current);
            }
            if !strings.is_empty() {
                output.push_str(crate::i18n::t("disasm.esp32.strings_section"));
                for s in &strings {
                    output.push_str(&format!(";   \"{}\"\n", s));
                }
            }
            // 前 64 字节 hex dump
            let dump_len = avail.min(64);
            output.push_str(crate::i18n::t("disasm.esp32.hex_head"));
            for off in (0..dump_len).step_by(16) {
                let end = (off + 16).min(dump_len);
                let hex: String = seg_data[off..end].iter().map(|b| format!("{:02X} ", b)).collect();
                let ascii: String = seg_data[off..end].iter()
                    .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                    .collect();
                output.push_str(&format!(";   0x{:08X}: {:<48} ; {}\n",
                    load + off as u32, hex.trim_end(), ascii));
            }
        }
    }

    output.push_str(crate::i18n::t("disasm.esp32.summary_section"));
    output.push_str(&crate::i18n::tf("disasm.esp32.segment_count", &[("{}", "{}", &format!("{}", seg_infos.len()))]));
    output.push_str(&crate::i18n::tf("disasm.esp32.total_calls", &[("{}", "{}", &format!("{}", total_calls))]));

    write_esp32_disasm_file(fs, is_zh, &output);
}

/// 智能格式化反汇编结果：将连续 db/保留编码区域替换为 hex dump
///
/// - `instructions`: 反汇编指令列表
/// - `data`: 原始 Flash 字节
/// - `base_offset`: 段数据在文件中的起始偏移
/// - `threshold`: 连续未识别条数阈值（>= threshold 输出 hex dump）
///
/// 返回：(输出行列表, 连续数据区域指令数, 零散db指令数, 函数调用目标列表)
fn format_disasm_with_data_regions(
    instructions: &[DisasmInstruction],
    data: &[u8],
    base_offset: usize,
    threshold: usize,
) -> (Vec<String>, usize, usize, Vec<u32>) {
    let mut lines = Vec::new();
    let mut data_region_count = 0usize;  // 连续>=threshold的未识别指令数
    let mut scatter_db_count = 0usize;   // 零散db数
    let mut calls = Vec::new();

    let mut i = 0;
    while i < instructions.len() {
        // 检测连续未识别区域（db + 保留编码 + NARROW.D + 填充模式）
        // 填充模式：0xFFFFFFFF被解码为TIE-E，实际是Flash填充数据
        if is_data_or_fill(&instructions[i]) {
            let start = i;
            while i < instructions.len() && is_data_or_fill(&instructions[i]) {
                i += 1;
            }
            let streak_len = i - start;

            if streak_len >= threshold {
                // 数据区域：输出 hex dump
                let start_addr = instructions[start].addr;
                // 计算实际字节长度（混合16/24位指令）
                let mut byte_len = 0usize;
                for j in start..i {
                    byte_len += instructions[j].bytes.len();
                }
                let end_addr = start_addr + byte_len as u32;
                let byte_start = base_offset + (instructions[start].addr - instructions[0].addr) as usize;

                lines.push(crate::i18n::tf(
                    "; --- data region 0x{:08X}-0x{:08X} ({} bytes, {} insts, disassembly skipped) ---",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", start_addr)),
                      ("{:08X}", "{:08X}", &format!("{:08X}", end_addr)),
                      ("{}", "{}", &format!("{}", byte_len)),
                      ("{}", "{}", &format!("{}", streak_len))],
                ));
                // hex dump：每行最多 16 字节
                for off in (0..byte_len).step_by(16) {
                    let abs_off = byte_start + off;
                    if abs_off >= data.len() { break; }
                    let chunk_len = (byte_len - off).min(16);
                    let abs_end = (abs_off + chunk_len).min(data.len());
                    let hex: String = data[abs_off..abs_end].iter()
                        .map(|b| format!("{:02X} ", b)).collect();
                    let ascii: String = data[abs_off..abs_end].iter()
                        .map(|&b| if (32..=126).contains(&b) { b as char } else { '.' })
                        .collect();
                    lines.push(format!(
                        ";   0x{:08X}: {:<48} ; {}",
                        start_addr + off as u32, hex.trim_end(), ascii
                    ));
                }
                data_region_count += streak_len;
            } else {
                // 零散未识别：正常输出
                for j in start..i {
                    lines.push(instructions[j].format());
                    // 扩展指令(TIE-E填充/RES.*等)不计入scatter_db，由ext_count统计
                    if !xtensa_disasm::is_extension(&instructions[j])
                        && !xtensa_disasm::is_mac16(&instructions[j])
                    {
                        scatter_db_count += 1;
                    }
                }
            }
        } else {
            // 已识别指令或扩展指令
            if instructions[i].mnemonic.starts_with("CALL") {
                if let Some(tgt) = instructions[i].operands
                    .strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok()) {
                    calls.push(tgt);
                }
            }
            lines.push(instructions[i].format());
            i += 1;
        }
    }

    (lines, data_region_count, scatter_db_count, calls)
}

/// 判断指令是否为数据/填充模式（用于数据区域检测）
/// 仅检测 trailing bytes（"db"，不完整指令的剩余字节）
/// TIE-E(0xFFFFFF)、RES.* 等扩展指令不算数据，由 ext_count 统计
/// 参考：capstone 对保留编码返回 MCDisassembler_Fail，不作为数据
///       ESP-IDF XCHAL_BYTE0_FORMAT_LENGTHS 表可确定指令长度，避免 trailing bytes 误判
fn is_data_or_fill(inst: &DisasmInstruction) -> bool {
    // 仅 trailing bytes（不完整指令的剩余字节）算数据
    xtensa_disasm::is_unidentified(inst)
}

/// 判断地址是否属于 IRAM（代码段）范围
/// ESP32/S2:    IRAM 0x4007_0000 - 0x4010_0000
/// ESP32-S3:    IRAM 0x4037_0000 - 0x403E_0000
/// ESP32-C3/H2: IRAM 0x4037_0000 - 0x403E_0000
/// 注意：使用闭区间 ..= 确保边界地址包含在内
pub(crate) fn is_iram_addr(addr: u32) -> bool {
    (0x40070000..=0x40100000).contains(&addr)
        || (0x40370000..=0x403E0000).contains(&addr)
}

/// 根据段 load_addr 返回段类型名称（用于反汇编输出标注）
pub(crate) fn segment_type_name(load_addr: u32) -> &'static str {
    if is_iram_addr(load_addr) {
        crate::i18n::t("IRAM (code)")
    } else if (0x3FCA0000..0x3FCE0000).contains(&load_addr)
        || (0x3FF80000..0x40000000).contains(&load_addr)
        || (0x3FC80000..0x3FD00000).contains(&load_addr)
    {
        crate::i18n::t("DRAM (data)")
    } else if (0x3C000000..0x3C800000).contains(&load_addr)
        || (0x3C000000..0x3D000000).contains(&load_addr)
    {
        crate::i18n::t("DROM (Flash data)")
    } else {
        crate::i18n::t("Data segment")
    }
}

/// 将反汇编文本写入文件（供 export_esp32_disasm_to_file 复用）
pub(crate) fn write_esp32_disasm_file(fs: &mut FlasherState, _is_zh: bool, output: &str) {
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let chip_prefix = match fs.chip_type {
        ChipType::Esp8266 => "esp8266",
        _ => "esp32", // Esp32 及其他 Xtensa 路径
    };
    let default_name = format!("{}_disasm_{}.asm", chip_prefix, timestamp);

    let path = if let Some(p) = rfd::FileDialog::new()
        .set_file_name(&default_name)
        .add_filter("Assembly", &["asm", "txt"])
        .save_file()
    {
        p
    } else {
        let mut tmp = std::env::temp_dir();
        tmp.push(&default_name);
        tmp
    };

    match std::fs::write(&path, output) {
        Ok(()) => {
            log_push(fs, crate::i18n::tf("<< ESP32 disasm exported: {}", &[("{}", "{}", &format!("{}", path.display()))]));
        }
        Err(e) => {
            log_push(fs, crate::i18n::tf("<< Export failed: {}", &[("{}", "{}", &format!("{}", e))]));
        }
    }
}

// 导出完整反汇编到文本文件 (export_disasm_to_file) 实现位于 inspect_arm.rs


