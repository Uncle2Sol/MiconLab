//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// NXP LPC ISP 单步操作实现
// =====================================================================

pub(crate) fn do_lpc_sync(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let lpc = LpcIspFlasher::new();
    let cmd = lpc.sync();
    log_push(fs, ">> LPC Sync (?×12)");
    state.flush_input();
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(1000) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            let text = String::from_utf8_lossy(&resp);
            if text.contains("Synchronized") {
                log_push(fs, "<< Synchronized OK");
                fs.progress = 0.1;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Unexpected resp: {}", text.trim()));
                state.last_error = Some("LPC sync failed".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_lpc_unlock(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let lpc = LpcIspFlasher::new();
    let cmd = lpc.unlock();
    log_push(fs, ">> LPC Unlock (U 23130)");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if LpcIspFlasher::is_ok(&resp) {
                log_push(fs, "<< Unlock OK");
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Unlock resp: {}", String::from_utf8_lossy(&resp).trim()));
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_lpc_read_part_id(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let lpc = LpcIspFlasher::new();
    let cmd = lpc.read_part_id();
    log_push(fs, ">> LPC Read Part ID (J)");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< Part ID resp: {}", String::from_utf8_lossy(&resp).trim()));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_lpc_read_uid(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let lpc = LpcIspFlasher::new();
    let cmd = lpc.read_uid();
    log_push(fs, ">> LPC Read UID (N)");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< UID resp: {}", String::from_utf8_lossy(&resp).trim()));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_lpc_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let lpc = LpcIspFlasher::new();
    // 先准备扇区，再擦除
    let prep = lpc.prepare_sectors(fs.lpc_sector_start, fs.lpc_sector_end);
    log_push(fs, format!(">> LPC Prepare Sectors {}..{}", fs.lpc_sector_start, fs.lpc_sector_end));
    if let Err(e) = state.send_raw(&prep) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            if !LpcIspFlasher::is_ok(&resp) {
                log_push(fs, format!("<< Prepare failed: {}", String::from_utf8_lossy(&resp).trim()));
                state.last_error = Some("LPC prepare failed".to_string());
                return;
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Prepare timeout: {}", e));
            state.last_error = Some(e);
            return;
        }
    }
    let erase = lpc.erase_sectors(fs.lpc_sector_start, fs.lpc_sector_end);
    log_push(fs, format!(">> LPC Erase Sectors {}..{}", fs.lpc_sector_start, fs.lpc_sector_end));
    if let Err(e) = state.send_raw(&erase) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(3000) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if LpcIspFlasher::is_ok(&resp) {
                log_push(fs, "<< Erase OK");
                fs.progress = 0.3;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Erase failed: {}", String::from_utf8_lossy(&resp).trim()));
                state.last_error = Some("LPC erase failed".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_lpc_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => { state.last_error = Some(e); return; }
    };
    if data.is_empty() {
        state.last_error = Some("Firmware data is empty".to_string());
        return;
    }
    let lpc = LpcIspFlasher::new();
    let chunk_size = 4096usize;
    let total_chunks = (data.len() + chunk_size - 1) / chunk_size;
    log_push(fs, format!(">> LPC Write ({} bytes, {} chunks)", data.len(), total_chunks));
    // 准备所有扇区（使用 UI 配置的扇区范围，而非硬编码 27 仅适用 LPC1768）
    let prep = lpc.prepare_sectors(fs.lpc_sector_start, fs.lpc_sector_end);
    log_push(fs, format!(">> LPC Prepare Sectors {}..{}", fs.lpc_sector_start, fs.lpc_sector_end));
    if let Err(e) = state.send_raw(&prep) {
        log_push(fs, format!("<< Prepare send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    // 检查 prepare_sectors 返回码（原实现静默丢弃响应，导致后续擦除/写入在未授权状态下失败）
    match state.read_response(500) {
        Ok(resp) => {
            if !LpcIspFlasher::is_ok(&resp) {
                let msg = format!("LPC prepare sectors failed: {}", String::from_utf8_lossy(&resp).trim());
                log_push(fs, format!("<< {}", msg));
                state.last_error = Some(msg);
                return;
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Prepare timeout: {}", e));
            state.last_error = Some(e);
            return;
        }
    }

    for (chunk_idx, chunk) in data.chunks(chunk_size).enumerate() {
        let ram_addr = fs.lpc_ram_addr + (chunk_idx * chunk_size) as u32;
        // 1. 写 RAM
        let w_cmd = lpc.write_ram(ram_addr, chunk.len() as u32);
        if let Err(e) = state.send_raw(&w_cmd) {
            log_push(fs, format!("<< W cmd[{}] error: {}", chunk_idx, e));
            state.last_error = Some(e);
            return;
        }
        // 等待 OK 响应
        match state.read_response(500) {
            Ok(resp) => {
                if !LpcIspFlasher::is_ok(&resp) {
                    log_push(fs, format!("<< W[{}] NAK: {}", chunk_idx, String::from_utf8_lossy(&resp).trim()));
                    state.last_error = Some("LPC write RAM failed".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< W[{}] timeout: {}", chunk_idx, e));
                state.last_error = Some(e);
                return;
            }
        }
        // 2. 发送数据 (UUD encoded - 简化版直接发原始字节，实际 LPC ISP 需要 UUD 编码)
        if let Err(e) = state.send_raw(chunk) {
            log_push(fs, format!("<< Data[{}] send error: {}", chunk_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(1000) {
            Ok(resp) => {
                if !LpcIspFlasher::is_ok(&resp) {
                    // 修复：Data 阶段 NAK 也应中断，否则后续 program_flash 在脏 RAM 上操作必然失败
                    log_push(fs, format!("<< Data[{}] NAK: {}", chunk_idx, String::from_utf8_lossy(&resp).trim()));
                    state.last_error = Some("LPC data stage NAK".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Data[{}] timeout: {}", chunk_idx, e));
                state.last_error = Some(e);
                return;
            }
        }
        // 3. 烧录 Flash
        let prog_cmd = lpc.program_flash(fs.address + (chunk_idx * chunk_size) as u32, chunk.len() as u32);
        if let Err(e) = state.send_raw(&prog_cmd) {
            log_push(fs, format!("<< Prog[{}] error: {}", chunk_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                if !LpcIspFlasher::is_ok(&resp) {
                    log_push(fs, format!("<< Prog[{}] NAK", chunk_idx));
                    state.last_error = Some("LPC program failed".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Prog[{}] timeout: {}", chunk_idx, e));
                state.last_error = Some(e);
                return;
            }
        }
        fs.progress = 0.3 + 0.5 * ((chunk_idx + 1) as f32 / total_chunks as f32);
    }
    log_push(fs, "<< Write complete");
    fs.progress = 0.8;
    state.last_error = None;
}

pub(crate) fn do_lpc_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    fs.read_chip = ChipType::NxpLpc;
    let total = fs.read_len.max(1) as u32;
    let lpc = LpcIspFlasher::new();
    let cmd = lpc.read_memory(fs.address, total);
    log_push(fs, format!(">> LPC Read @ 0x{:08X} total={}", fs.address, total));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(3000) {
        Ok(resp) => {
            // LPC ISP 读取响应格式: <ret_code>\r\n<UUD encoded data>\r\n<ret_code>\r\n
            // 简化处理: 跳过首行返回码，剩余视为数据
            let text = String::from_utf8_lossy(&resp);
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() >= 2 {
                let data_line = lines[1];
                // UUD 解码（每行 45 字节 = 90 hex chars）简化版
                let mut decoded = Vec::new();
                for i in (0..data_line.len()).step_by(2) {
                    if i + 2 <= data_line.len() {
                        if let Ok(b) = u8::from_str_radix(&data_line[i..i+2], 16) {
                            decoded.push(b);
                        }
                    }
                }
                fs.last_read_data = decoded;
            }
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< Read {} bytes", fs.last_read_data.len()));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_lpc_go(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let lpc = LpcIspFlasher::new();
    let cmd = lpc.go(fs.address, true);
    log_push(fs, format!(">> LPC Go @ 0x{:08X} (Thumb)", fs.address));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    log_push(fs, "<< Go command sent");
    fs.progress = 1.0;
    state.last_error = None;
}

// =====================================================================

