//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// STM8 单步操作实现
// =====================================================================

pub(crate) fn do_stm8_connect(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let stm8 = Stm8Flasher::new();
    let cmd = stm8.sync();
    log_push(fs, format!(">> STM8 Sync: {}", format_hex_spaced(&cmd)));
    state.flush_input();
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Stm8Flasher::is_ack(resp.first().copied().unwrap_or(0)) {
                log_push(fs, "<< ACK (0x79) - Bootloader active");
                fs.progress = 0.1;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< NAK: {}", format_hex_spaced(&resp)));
                state.last_error = Some("STM8 NAK".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_stm8_get(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let stm8 = Stm8Flasher::new();
    let cmd = stm8.get_command();
    log_push(fs, ">> STM8 GET");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< GET resp: {}", format_hex_spaced(&resp)));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

pub(crate) fn do_stm8_mass_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let stm8 = Stm8Flasher::new();
    let segments = stm8.mass_erase();
    log_push(fs, ">> STM8 Mass Erase");
    for (i, seg) in segments.iter().enumerate() {
        log_push(fs, format!("   seg[{}]: {}", i, format_hex_spaced(seg)));
        if let Err(e) = state.send_raw(seg) {
            log_push(fs, format!("<< Send error: {}", e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                if Stm8Flasher::is_ack(resp.first().copied().unwrap_or(0)) {
                    log_push(fs, format!("   seg[{}] ACK", i));
                } else {
                    log_push(fs, format!("   seg[{}] NAK", i));
                    state.last_error = Some("STM8 mass erase NAK".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("   seg[{}] timeout: {}", i, e));
                state.last_error = Some(e);
                return;
            }
        }
    }
    log_push(fs, "<< Mass erase complete");
    fs.progress = 0.3;
    state.last_error = None;
}

pub(crate) fn do_stm8_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => { state.last_error = Some(e); return; }
    };
    if data.is_empty() {
        state.last_error = Some("Firmware data is empty".to_string());
        return;
    }
    let stm8 = Stm8Flasher::new();
    let chunk_size = 256usize;
    let total_chunks = (data.len() + chunk_size - 1) / chunk_size;
    log_push(fs, format!(">> STM8 Write ({} bytes, {} chunks)", data.len(), total_chunks));
    for (chunk_idx, chunk) in data.chunks(chunk_size).enumerate() {
        let addr = fs.address + (chunk_idx * chunk_size) as u32;
        let segments = stm8.write_memory(addr, chunk);
        if segments.is_empty() { continue; }
        for (i, seg) in segments.iter().enumerate() {
            if let Err(e) = state.send_raw(seg) {
                log_push(fs, format!("<< Send error: {}", e));
                state.last_error = Some(e);
                return;
            }
            match state.read_response(1000) {
                Ok(resp) => {
                    if !Stm8Flasher::is_ack(resp.first().copied().unwrap_or(0)) {
                        log_push(fs, format!("   chunk[{}] seg[{}] NAK", chunk_idx, i));
                        state.last_error = Some("STM8 write NAK".to_string());
                        return;
                    }
                }
                Err(e) => {
                    log_push(fs, format!("   chunk[{}] seg[{}] timeout: {}", chunk_idx, i, e));
                    state.last_error = Some(e);
                    return;
                }
            }
        }
        fs.progress = 0.3 + 0.5 * ((chunk_idx + 1) as f32 / total_chunks as f32);
    }
    log_push(fs, "<< Write complete");
    fs.progress = 0.8;
    state.last_error = None;
}

pub(crate) fn do_stm8_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    fs.read_chip = ChipType::Stm8;
    let total = fs.read_len.max(1) as usize;
    let stm8 = Stm8Flasher::new();
    fs.last_read_data.clear();
    log_push(fs, format!(">> STM8 Read @ 0x{:08X} total={}", fs.address, total));
    let mut addr = fs.address;
    let mut remaining = total;
    while remaining > 0 {
        // 修复：min(255) 而非 min(256)，因为 256 as u8 会截断为 0 导致死循环
        let chunk = remaining.min(255) as u8;
        let cmd = stm8.read_memory(addr, chunk);
        if let Err(e) = state.send_raw(&cmd) {
            log_push(fs, format!("<< Send error: {}", e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(1000) {
            Ok(resp) => {
                if resp.is_empty() {
                    state.last_error = Some("Empty read response".to_string());
                    return;
                }
                let data_end = if resp.len() >= 1 + chunk as usize
                    && resp.last() == Some(&0x79)
                    && resp.len() == 1 + chunk as usize + 1
                {
                    1 + chunk as usize
                } else if resp.len() >= 1 + chunk as usize {
                    1 + chunk as usize
                } else {
                    resp.len()
                };
                let data = &resp[1..data_end.min(resp.len())];
                fs.last_read_data.extend_from_slice(data);
                fs.response = format_hex_spaced(&resp);
            }
            Err(e) => {
                log_push(fs, format!("<< Read error: {}", e));
                state.last_error = Some(e);
                return;
            }
        }
        addr = addr.wrapping_add(chunk as u32);
        remaining -= chunk as usize;
        fs.progress = 0.1 + 0.8 * (1.0 - remaining as f32 / total as f32);
    }
    log_push(fs, format!("<< Read complete: {} bytes", fs.last_read_data.len()));
    state.last_error = None;
}

pub(crate) fn do_stm8_go(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let stm8 = Stm8Flasher::new();
    let cmd = stm8.go(fs.address);
    log_push(fs, format!(">> STM8 Go @ 0x{:08X}", fs.address));
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

