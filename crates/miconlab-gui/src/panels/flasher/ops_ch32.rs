//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// CH32 (WCH ISP) 单步操作实现
// =====================================================================

pub(crate) fn do_ch32_identify(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let ch32 = Ch32Flasher::new();
    let cmd = ch32.identify();
    log_push(fs, format!(">> CH32 Identify: {}", format_hex_spaced(&cmd)));
    state.flush_input();
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Ch32Flasher::validate_packet(&resp) {
                log_push(fs, "<< Identify OK (valid WCH packet)");
                fs.progress = 0.1;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Invalid WCH packet: {}", format_hex_spaced(&resp)));
                state.last_error = Some("Invalid WCH packet".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_ch32_read_chip_id(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let ch32 = Ch32Flasher::new();
    let cmd = ch32.read_chip_id();
    log_push(fs, ">> CH32 Read Chip ID");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            match Ch32Flasher::extract_payload(&resp) {
                Ok(payload) => {
                    if payload.len() >= 4 {
                        let chip_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                        log_push(fs, format!("<< Chip ID: 0x{:08X}", chip_id));
                        match Ch32Flasher::identify_chip_id(chip_id) {
                            Some((name, kb)) => {
                                log_push(fs, format!("   Identified: {} ({} KB Flash)", name, kb));
                            }
                            None => {
                                log_push(fs, "   Unknown chip ID");
                            }
                        }
                        fs.progress = 0.2;
                        state.last_error = None;
                    } else {
                        log_push(fs, format!("<< Short payload: {} bytes", payload.len()));
                    }
                }
                Err(e) => {
                    log_push(fs, format!("<< Parse error: {}", e));
                    state.last_error = Some(e.to_string());
                }
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_ch32_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let ch32 = Ch32Flasher::new();
    let size = fs.read_len.max(1) as u32 * 1024; // KB → bytes
    let cmd = ch32.erase(fs.address, size);
    log_push(fs, format!(">> CH32 Erase @ 0x{:08X} size={}", fs.address, size));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(3000) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Ch32Flasher::validate_packet(&resp) {
                log_push(fs, "<< Erase OK");
                fs.progress = 0.3;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Erase resp: {}", format_hex_spaced(&resp)));
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_ch32_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => { state.last_error = Some(e); return; }
    };
    if data.is_empty() {
        state.last_error = Some("Firmware data is empty".to_string());
        return;
    }
    let ch32 = Ch32Flasher::new();
    let chunk_size = 256usize;
    let total_chunks = (data.len() + chunk_size - 1) / chunk_size;
    log_push(fs, format!(">> CH32 Write ({} bytes, {} chunks)", data.len(), total_chunks));
    for (chunk_idx, chunk) in data.chunks(chunk_size).enumerate() {
        let addr = fs.address + (chunk_idx * chunk_size) as u32;
        let cmd = ch32.write(addr, chunk);
        if let Err(e) = state.send_raw(&cmd) {
            log_push(fs, format!("<< Write[{}] error: {}", chunk_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(1000) {
            Ok(resp) => {
                // 修复：写入失败必须中断，否则后续 chunk 在错误状态下继续写入会破坏 Flash
                if !Ch32Flasher::validate_packet(&resp) {
                    let rx_hex: String = resp.iter().take(16).map(|b| format!("{:02X} ", b)).collect();
                    log_push(fs, format!("<< Write[{}] invalid resp: {}", chunk_idx, rx_hex));
                    state.last_error = Some(format!("CH32 write[{}] failed: invalid response", chunk_idx));
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Write[{}] timeout: {}", chunk_idx, e));
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

pub(crate) fn do_ch32_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    fs.read_chip = ChipType::Ch32;
    let total = fs.read_len.max(1) as u32;
    let ch32 = Ch32Flasher::new();
    let chunk_size: u32 = 256;
    let total_chunks = (total + chunk_size - 1) / chunk_size;
    log_push(fs, format!(">> CH32 Read @ 0x{:08X} total={}", fs.address, total));
    fs.last_read_data.clear();
    let mut remaining = total;
    let mut addr = fs.address;
    for chunk_idx in 0..total_chunks {
        let chunk = remaining.min(chunk_size);
        let cmd = ch32.read(addr, chunk);
        if let Err(e) = state.send_raw(&cmd) {
            log_push(fs, format!("<< Read[{}] error: {}", chunk_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                if let Ok(payload) = Ch32Flasher::extract_payload(&resp) {
                    // payload 前 4 字节是地址，后为数据
                    if payload.len() >= 4 {
                        let data = &payload[4..];
                        let take = data.len().min(chunk as usize);
                        fs.last_read_data.extend_from_slice(&data[..take]);
                    }
                }
                fs.response = format_hex_spaced(&resp);
            }
            Err(e) => {
                log_push(fs, format!("<< Read[{}] timeout: {}", chunk_idx, e));
                state.last_error = Some(e);
                return;
            }
        }
        addr = addr.wrapping_add(chunk);
        remaining -= chunk;
        fs.progress = 0.1 + 0.85 * ((chunk_idx + 1) as f32 / total_chunks as f32);
    }
    log_push(fs, format!("<< Read complete: {} bytes", fs.last_read_data.len()));
    fs.progress = 1.0;
    state.last_error = None;
}

pub(crate) fn do_ch32_verify(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => { state.last_error = Some(e); return; }
    };
    if data.is_empty() {
        state.last_error = Some("Firmware data is empty".to_string());
        return;
    }
    let ch32 = Ch32Flasher::new();
    let cmd = ch32.verify(fs.address, &data);
    log_push(fs, format!(">> CH32 Verify @ 0x{:08X} ({} bytes)", fs.address, data.len()));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(2000) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Ch32Flasher::validate_packet(&resp) {
                log_push(fs, "<< Verify OK");
                state.last_error = None;
            } else {
                log_push(fs, "<< Verify failed");
                state.last_error = Some("CH32 verify failed".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_ch32_reset(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let ch32 = Ch32Flasher::new();
    let cmd = ch32.reset();
    log_push(fs, ">> CH32 Reset");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            log_push(fs, format!("<< Reset resp: {}", format_hex_spaced(&resp)));
            fs.progress = 1.0;
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

// =====================================================================

