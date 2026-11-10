//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// 辅助函数
// =====================================================================

/// 检查是否已连接串口
pub(crate) fn check_connected(state: &mut AppState, _is_zh: bool) -> bool {
    if !state.is_connected() {
        state.last_error = Some(crate::i18n::t("Not connected").to_string());
        false
    } else {
        true
    }
}

/// 将 `last_read_data` 保存到 `backup_path` 文件
///
/// 文件格式：
///   - 路径以 .hex 结尾 → 生成 Intel HEX 格式（每 16 字节一条记录）
///   - 其他扩展名 → 原始二进制 .bin
pub(crate) fn save_backup_to_file(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if fs.last_read_data.is_empty() {
        state.last_error = Some(crate::i18n::t("No read data to save, click Read first").to_string());
        return;
    }

    // 路径为空时弹出文件保存对话框让用户选择
    let path = if fs.backup_path.trim().is_empty() {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let chip_name = match fs.read_chip {
            ChipType::Stm32 => "stm32",
            ChipType::Esp32 => "esp32",
            ChipType::Esp8266 => "esp8266",
            ChipType::Arduino => "arduino",
            ChipType::Stm8 => "stm8",
            ChipType::NxpLpc => "lpc",
            ChipType::Gd32 => "gd32",
            ChipType::Ch32 => "ch32",
            _ => "firmware",
        };
        let default_name = format!("{}_backup_{}.bin", chip_name, timestamp);
        match rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Binary", &["bin"])
            .add_filter("Intel HEX", &["hex"])
            .save_file()
        {
            Some(p) => {
                let s = p.display().to_string();
                fs.backup_path = s.clone();
                s
            }
            None => return, // 用户取消
        }
    } else {
        fs.backup_path.trim().to_string()
    };

    let is_hex = path.to_lowercase().ends_with(".hex");

    // 准备文件内容
    let file_content: Vec<u8> = if is_hex {
        // 生成 Intel HEX
        let mut content = String::new();
        let base_addr: u32 = fs.read_addr;
        let mut ext_addr: u32 = 0;
        for (i, chunk) in fs.last_read_data.chunks(16).enumerate() {
            let abs_addr = base_addr + (i * 16) as u32;
            let upper = (abs_addr >> 16) & 0xFFFF;
            if upper != ext_addr {
                let checksum = 0x02u8
                    .wrapping_add((upper >> 8) as u8)
                    .wrapping_add((upper & 0xFF) as u8)
                    .wrapping_add(0x04);
                content.push_str(&format!(
                    ":02000004{:04X}{:02X}\n",
                    upper, (!checksum).wrapping_add(1)
                ));
                ext_addr = upper;
            }
            let addr_lo = (abs_addr & 0xFFFF) as u16;
            let len = chunk.len() as u8;
            let mut checksum = len;
            checksum = checksum.wrapping_add((addr_lo >> 8) as u8);
            checksum = checksum.wrapping_add((addr_lo & 0xFF) as u8);
            checksum = checksum.wrapping_add(0x00);
            for &b in chunk {
                checksum = checksum.wrapping_add(b);
            }
            let cks = (!checksum).wrapping_add(1);
            content.push_str(&format!(":{:02X}{:04X}00", len, addr_lo));
            for &b in chunk {
                content.push_str(&format!("{:02X}", b));
            }
            content.push_str(&format!("{:02X}\n", cks));
        }
        content.push_str(":00000001FF\n"); // EOF
        content.into_bytes()
    } else {
        fs.last_read_data.clone()
    };

    // 尝试写入：用户路径 → 临时目录 → 当前目录
    let primary_path = std::path::PathBuf::from(&path);
    let write_result = std::fs::write(&primary_path, &file_content);

    let (saved_path, result) = match write_result {
        Ok(_) => (primary_path.clone(), Ok(())),
        Err(primary_err) => {
            log_push(fs, format!(
                "<< {} ({}), {}",
                crate::i18n::t("Primary path write failed"),
                primary_path.display(),
                primary_err
            ));
            // 后备 1: 临时目录
            let fallback_name = primary_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("firmware_backup.bin"));
            let tmp_path = std::env::temp_dir().join(fallback_name);
            match std::fs::write(&tmp_path, &file_content) {
                Ok(_) => (tmp_path.clone(), Ok(())),
                Err(tmp_err) => {
                    log_push(fs, format!(
                        "<< {} ({}), {}",
                        crate::i18n::t("Temp dir write failed"),
                        tmp_path.display(),
                        tmp_err
                    ));
                    // 后备 2: 当前工作目录
                    let cwd_path = std::path::PathBuf::from(".").join(fallback_name);
                    match std::fs::write(&cwd_path, &file_content) {
                        Ok(_) => (cwd_path.clone(), Ok(())),
                        Err(cwd_err) => (cwd_path, Err(cwd_err.to_string())),
                    }
                }
            }
        }
    };

    match result {
        Ok(()) => {
            let size = fs.last_read_data.len();
            let saved_str = saved_path.display().to_string();
            fs.backup_path = saved_str.clone();
            log_push(fs, format!(
                "<< Backup saved: {} ({} bytes, {})",
                saved_str, size,
                if is_hex { "Intel HEX" } else { "Binary" }
            ));
            if saved_path != primary_path {
                log_push(fs, crate::i18n::t("⚠ Saved to fallback path (primary path not writable)"));
            }
            state.last_error = None;
            inspect_backup_file(fs, is_zh);
        }
        Err(e) => {
            // 所有路径均失败，将 HEX 数据输出到日志供手动保存
            let msg = format!(
                "{}: {}",
                crate::i18n::t("Failed to save backup"),
                e
            );
            log_push(fs, format!("<< {}", msg));
            // 输出完整 HEX dump 到日志作为最后手段
            if is_hex {
                let hex_str = String::from_utf8_lossy(&file_content);
                log_push(fs, crate::i18n::t(">> HEX data (copy and save as .hex manually):"));
                for line in hex_str.lines() {
                    log_push(fs, line.to_string());
                }
            } else {
                let hex_str: String = fs.last_read_data.iter().map(|b| format!("{:02X}", b)).collect();
                log_push(fs, crate::i18n::t(">> BIN HEX dump (copy and save manually):"));
                for chunk in hex_str.as_bytes().chunks(64) {
                    log_push(fs, String::from_utf8_lossy(chunk).to_string());
                }
            }
            state.last_error = Some(msg);
        }
    }
}

/// 检查字符串是否像真实字符串（过滤 Thumb 指令字节噪音）
/// 规则：必须同时满足
/// 1. 含分隔符（空格/等号/加号/下划线/斜杠/冒号/逗号/连字符/点/反斜杠）
/// 2. 含 4+ 连续字母
/// 无分隔符的纯字母串几乎都是指令字节噪音
pub(crate) fn has_consecutive_alpha(s: &str, min_consecutive: usize) -> bool {
    let has_delim = s.chars().any(|c| matches!(c, ' ' | '=' | '+' | '_' | '/' | ':' | ',' | '-' | '.' | '\\'));
    if !has_delim {
        return false;
    }
    let mut count = 0;
    for c in s.chars() {
        if c.is_alphabetic() {
            count += 1;
            if count >= min_consecutive {
                return true;
            }
        } else {
            count = 0;
        }
    }
    false
}

pub(crate) fn inspect_backup_file(fs: &mut FlasherState, _is_zh: bool) {
    let path = fs.backup_path.trim().to_string();
    if path.is_empty() {
        log_push(fs, crate::i18n::t("<< Inspect failed: empty path"));
        return;
    }

    let p = std::path::Path::new(&path);
    if !p.exists() {
        log_push(fs, crate::i18n::tf("<< Inspect failed: file not found {}",
            &[("{}", "{}", &format!("{}", path))]));
        return;
    }

    let metadata = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => {
            log_push(fs, crate::i18n::tf("<< Inspect failed: cannot read file info: {}",
                &[("{}", "{}", &format!("{}", e))]));
            return;
        }
    };

    let file_size = metadata.len();
    log_push(fs, crate::i18n::tf("=== Firmware inspection: {} ===",
        &[("{}", "{}", &format!("{}", path))]));
    log_push(fs, crate::i18n::tf("  File size: {} bytes ({} KB)",
        &[("{}", "{}", &format!("{}", file_size)), ("{}", "{}", &format!("{}", file_size / 1024))]));

    let is_hex = path.to_lowercase().ends_with(".hex");

    if is_hex {
        // Intel HEX 解读
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                log_push(fs, crate::i18n::tf("<< Failed to read HEX file: {}",
                    &[("{}", "{}", &format!("{}", e))]));
                return;
            }
        };
        let mut record_count = 0u32;
        let mut data_count = 0u32;
        let mut ext_addr_count = 0u32;
        let mut start_addr_count = 0u32;
        let mut eof_count = 0u32;
        let mut min_addr = u32::MAX;
        let mut max_addr = 0u32;
        let mut total_data_bytes = 0u32;
        let mut current_base: u32 = 0;

        for line in content.lines() {
            if !line.starts_with(':') {
                continue;
            }
            record_count += 1;
            if line.len() < 11 {
                continue;
            }
            let rec_type = u8::from_str_radix(&line[7..9], 16).unwrap_or(0);
            match rec_type {
                0x00 => {
                    // 数据记录
                    let len = u8::from_str_radix(&line[1..3], 16).unwrap_or(0) as u32;
                    let addr_lo = u16::from_str_radix(&line[3..7], 16).unwrap_or(0) as u32;
                    let abs_addr = current_base + addr_lo;
                    min_addr = min_addr.min(abs_addr);
                    max_addr = max_addr.max(abs_addr.saturating_add(len));
                    data_count += 1;
                    total_data_bytes += len;
                }
                0x04 => {
                    // 扩展线性地址
                    ext_addr_count += 1;
                    if line.len() >= 13 {
                        let upper = u16::from_str_radix(&line[9..13], 16).unwrap_or(0) as u32;
                        current_base = upper << 16;
                    }
                }
                0x02 => {
                    // 扩展段地址
                    ext_addr_count += 1;
                    if line.len() >= 13 {
                        let seg = u16::from_str_radix(&line[9..13], 16).unwrap_or(0) as u32;
                        current_base = seg << 4;
                    }
                }
                0x03 => start_addr_count += 1,
                0x01 => eof_count += 1,
                _ => {}
            }
        }

        log_push(fs, crate::i18n::t("  File format: Intel HEX"));
        log_push(fs, crate::i18n::tf("  Total records: {}",
            &[("{}", "{}", &format!("{}", record_count))]));
        log_push(fs, crate::i18n::tf("  Data records: {} ({} bytes)",
            &[("{}", "{}", &format!("{}", data_count)), ("{}", "{}", &format!("{}", total_data_bytes))]));
        log_push(fs, crate::i18n::tf("  Extended addresses: {}",
            &[("{}", "{}", &format!("{}", ext_addr_count))]));
        log_push(fs, crate::i18n::tf("  Start addresses: {}",
            &[("{}", "{}", &format!("{}", start_addr_count))]));
        log_push(fs, crate::i18n::tf("  EOF records: {}",
            &[("{}", "{}", &format!("{}", eof_count))]));
        if min_addr != u32::MAX {
            log_push(fs, crate::i18n::tf("  Data address range: 0x{:08X} - 0x{:08X} ({} bytes)",
                &[("{:08X}", "{:08X}", &format!("{:08X}", min_addr)),
                  ("{:08X}", "{:08X}", &format!("{:08X}", max_addr)),
                  ("{}", "{}", &format!("{}", max_addr - min_addr))]));
        }
        if eof_count == 0 {
            log_push(fs, crate::i18n::t("  ⚠ Missing EOF record (0x01), file may be incomplete"));
        }
    } else {
        // 二进制解读
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                log_push(fs, crate::i18n::tf("<< Failed to read BIN file: {}",
                    &[("{}", "{}", &format!("{}", e))]));
                return;
            }
        };
        log_push(fs, crate::i18n::t("  File format: Raw Binary"));

        // STM32 向量表分析（前 16×4=64 字节）
        if data.len() >= 64 {
            let sp = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            let reset_handler = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            log_push(fs, crate::i18n::t("  STM32 vector table inference (first 8 bytes):"));
            log_push(fs, crate::i18n::tf("    Stack pointer (SP): 0x{:08X}",
                &[("{:08X}", "{:08X}", &format!("{:08X}", sp))]));
            log_push(fs, format!("    Reset Handler: 0x{:08X}", reset_handler));

            // 验证 SP 是否合法（通常在 RAM 区域，STM32F1: 0x2000xxxx, STM32F4: 0x2002xxxx）
            if sp >= 0x2000_0000 && sp < 0x3000_0000 {
                let ram_size_kb = (sp - 0x2000_0000) / 1024;
                log_push(fs, crate::i18n::tf("    → SP valid (RAM starts at 0x20000000, inferred RAM >= {}KB)",
                    &[("{}", "{}", &format!("{}", ram_size_kb))]));
            } else if sp == 0xFFFFFFFF || sp == 0 {
                log_push(fs, crate::i18n::t("    → SP abnormal (may be erased flash or non-firmware data)"));
            } else {
                log_push(fs, crate::i18n::t("    → SP outside the typical RAM range, may not be STM32 firmware"));
            }

            // 验证 Reset Handler 是否在 Flash 区域
            if (reset_handler & 1) != 0 {
                let rh_addr = reset_handler & !1;
                if rh_addr >= 0x0800_0000 && rh_addr < 0x0820_0000 {
                    log_push(fs, crate::i18n::tf("    → Reset Handler points to 0x{:08X} (Flash region), Thumb mode ✓",
                        &[("{:08X}", "{:08X}", &format!("{:08X}", rh_addr))]));
                } else if rh_addr >= 0x2000_0000 && rh_addr < 0x2002_0000 {
                    log_push(fs, crate::i18n::tf("    → Reset Handler points to 0x{:08X} (RAM region, possibly a Bootloader)",
                        &[("{:08X}", "{:08X}", &format!("{:08X}", rh_addr))]));
                }
            } else {
                log_push(fs, crate::i18n::t("    → Reset Handler bit 0 is not 1, not an ARM Thumb instruction"));
            }

            // 向量表后续项
            log_push(fs, crate::i18n::t("    Vector table [2-7]:"));
            let names = ["NMI", "HardFault", "MemManage", "BusFault",
                         "UsageFault", "Reserved", "Reserved", "Reserved"];
            for i in 2..8 {
                if i * 4 + 4 <= data.len() {
                    let val = u32::from_le_bytes([
                        data[i*4], data[i*4+1], data[i*4+2], data[i*4+3]
                    ]);
                    log_push(fs, format!("      [{}] {}: 0x{:08X}", i, names.get(i - 2).unwrap_or(&"?"), val));
                }
            }
        }

        // 熵分析（判断是否加密/压缩）
        if data.len() > 256 {
            let mut freq = [0u32; 256];
            for &b in &data {
                freq[b as usize] += 1;
            }
            let total = data.len() as f64;
            let mut entropy = 0.0f64;
            for &count in &freq {
                if count > 0 {
                    let p = count as f64 / total;
                    entropy -= p * p.log2();
                }
            }
            log_push(fs, crate::i18n::tf("  Entropy: {:.2} bits/byte",
                &[("{:.2}", "{:.2}", &format!("{:.2}", entropy))]));
            if entropy > 7.5 {
                log_push(fs, crate::i18n::t("    → High entropy (>7.5): data may be encrypted or compressed"));
            } else if entropy > 6.0 {
                log_push(fs, crate::i18n::t("    → Medium entropy: firmware code + data mix (normal)"));
            } else {
                log_push(fs, crate::i18n::t("    → Low entropy: likely lots of repeated data or empty regions"));
            }
        }

        // ASCII 字符串提取（至少 4 字符的连续可打印串）
        let mut strings: Vec<(usize, String)> = Vec::new();
        let mut current = String::new();
        let mut start = 0;
        for (i, &b) in data.iter().enumerate() {
            if b >= 0x20 && b < 0x7f {
                if current.is_empty() {
                    start = i;
                }
                current.push(b as char);
            } else {
                if current.len() >= 4 {
                    strings.push((start, current.clone()));
                }
                current.clear();
            }
        }
        if current.len() >= 4 {
            strings.push((start, current));
        }
        log_push(fs, crate::i18n::tf("  Strings found: {} (>=4 chars)",
            &[("{}", "{}", &format!("{}", strings.len()))]));
        // 显示前 10 个字符串
        let show_count = strings.len().min(10);
        for (_i, (offset, s)) in strings.iter().take(show_count).enumerate() {
            let display = if s.len() > 60 { format!("{}...", &s[..60]) } else { s.clone() };
            log_push(fs, format!("    @0x{:08X}: \"{}\"", offset, display));
        }
        if strings.len() > show_count {
            log_push(fs, crate::i18n::tf("    ... {} more",
                &[("{}", "{}", &format!("{}", strings.len() - show_count))]));
        }

        // HEX dump（前 256 字节）
        log_push(fs, crate::i18n::t("  HEX dump of first 256 bytes:"));
        let dump_size = data.len().min(256);
        for (i, chunk) in data[..dump_size].chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{:02X}", b)).collect();
            let ascii: String = chunk.iter()
                .map(|&b| if b >= 0x20 && b < 0x7f { b as char } else { '.' })
                .collect();
            log_push(fs, format!("    {:08X}: {:48} {}", i * 16, hex.join(" "), ascii));
        }
    }

    log_push(fs, crate::i18n::t("=== Inspection complete ==="));
}

/// 读取固件文件并解析为字节序列
///
/// 支持 Intel HEX 格式（.hex）和二进制格式（.bin）。
/// 对 .hex 文件调用 flasher::parse_ihex 解析记录，
/// 然后按记录地址拼接为连续字节流（简单实现，不处理地址跳跃）。
pub(crate) fn load_firmware_data(path: &str, _is_zh: bool) -> Result<Vec<u8>, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err(crate::i18n::t("Firmware file path is empty").to_string());
    }

    // 统一以字节方式读取，避免二进制文件触发 UTF-8 解码失败
    let raw = std::fs::read(path).map_err(|e| {
        format!(
            "{}: {}",
            crate::i18n::t("Failed to read file"),
            e
        )
    })?;

    if raw.is_empty() {
        return Err(crate::i18n::t("Firmware file is empty").to_string());
    }

    // 根据扩展名判断格式
    if path.to_lowercase().ends_with(".hex") {
        // Intel HEX 格式：需要将字节转为字符串后解析
        let content = String::from_utf8_lossy(&raw);
        let records = flasher::parse_ihex(&content).map_err(|e| {
            format!(
                "{}: {}",
                crate::i18n::t("HEX parse error"),
                e
            )
        })?;

        // 修复点：正确处理扩展地址记录，按绝对地址装配固件
        // Intel HEX 记录类型：
        //   0x00: 数据记录，绝对地址 = (扩展地址 + 记录地址)
        //   0x01: 文件结束
        //   0x02: 扩展段地址（高 16 位，左移 4 位作为基址）
        //   0x04: 扩展线性地址（高 16 位，左移 16 位作为基址）
        //   0x05: 程序入口地址（不影响数据装配）
        // 缺失地址用 0xFF 填充（flash 擦除态），支持 >64KB 固件
        let mut base_addr: u32 = 0;
        let mut image: std::collections::BTreeMap<u32, u8> = std::collections::BTreeMap::new();
        let mut min_addr: u32 = u32::MAX;
        let mut max_addr: u32 = 0;

        for rec in &records {
            match rec.record_type {
                0x00 => {
                    // 数据记录：绝对地址 = base_addr + rec.address
                    let abs_addr = base_addr.wrapping_add(rec.address as u32);
                    for (i, &b) in rec.data.iter().enumerate() {
                        let a = abs_addr.wrapping_add(i as u32);
                        image.insert(a, b);
                        if a < min_addr {
                            min_addr = a;
                        }
                        if a > max_addr {
                            max_addr = a;
                        }
                    }
                }
                0x02 => {
                    // 扩展段地址：2 字节大端，左移 4 位
                    if rec.data.len() >= 2 {
                        let seg = ((rec.data[0] as u32) << 8) | (rec.data[1] as u32);
                        base_addr = seg << 4;
                    }
                }
                0x04 => {
                    // 扩展线性地址：2 字节大端，左移 16 位
                    if rec.data.len() >= 2 {
                        let upper = ((rec.data[0] as u32) << 8) | (rec.data[1] as u32);
                        base_addr = upper << 16;
                    }
                }
                0x01 => break, // EOF
                _ => {} // 忽略 0x03/0x05 等其他记录
            }
        }

        if image.is_empty() {
            return Err(crate::i18n::t("HEX has no data records").to_string());
        }

        // 装配连续字节流，缺失部分用 0xFF 填充（flash 擦除态）
        let mut data = Vec::with_capacity((max_addr.wrapping_sub(min_addr) as usize) + 1);
        for a in min_addr..=max_addr {
            data.push(*image.get(&a).unwrap_or(&0xFF));
        }
        Ok(data)
    } else {
        // 二进制格式：直接返回原始字节
        Ok(raw)
    }
}

