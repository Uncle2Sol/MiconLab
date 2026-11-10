//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// ESP8266 单步操作实现
// =====================================================================

/// ESP8266 同步（连续发送多次同步包）
pub(crate) fn do_esp8266_sync(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let esp = Esp8266Flasher::new();
    let packets = esp.sync_repeated(7);
    log_push(fs, ">> ESP8266 Sync (7 attempts)");
    for (i, pkt) in packets.iter().enumerate() {
        if let Err(e) = state.send_raw(pkt) {
            log_push(fs, format!("<< Send error [{}]: {}", i, e));
            state.last_error = Some(e);
            return;
        }
        if let Ok(resp) = state.read_response(100) {
            if !resp.is_empty() {
                log_push(fs, format!("   sync[{}] resp: {}", i, format_hex_spaced(&resp)));
                log_push(fs, "<< Sync OK");
                fs.progress = 0.1;
                state.last_error = None;
                return;
            }
        }
    }
    log_push(fs, "<< Sync: no response");
    state.last_error = Some("ESP8266 sync no response".to_string());
}

/// ESP8266 读取 MAC
pub(crate) fn do_esp8266_read_mac(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let esp = Esp8266Flasher::new();
    let cmd = esp.read_mac();
    log_push(fs, ">> ESP8266 Read MAC");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< MAC resp: {}", format_hex_spaced(&resp)));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// ESP8266 擦除
pub(crate) fn do_esp8266_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => { state.last_error = Some(e); return; }
    };
    if data.is_empty() {
        state.last_error = Some("Firmware data is empty".to_string());
        return;
    }
    let erase_size = data.len() as u32;
    let num_sectors = (erase_size + 4095) / 4096;
    let esp = Esp8266Flasher::new();
    let cmd = esp.flash_begin(erase_size, num_sectors);
    log_push(fs, format!(">> ESP8266 Flash Begin: size={} sectors={}", erase_size, num_sectors));
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(2000) {
        Ok(resp) => {
            log_push(fs, format!("<< Flash Begin resp: {}", format_hex_spaced(&resp)));
            fs.progress = 0.3;
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// ESP8266 写入
pub(crate) fn do_esp8266_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => { state.last_error = Some(e); return; }
    };
    if data.is_empty() {
        state.last_error = Some("Firmware data is empty".to_string());
        return;
    }
    let esp = Esp8266Flasher::new();
    let block_size = 4096usize;
    let total_blocks = ((data.len() + block_size - 1) / block_size) as u32;
    let total_size = data.len() as u32;
    let begin_cmd = esp.flash_begin(total_size, total_blocks);
    log_push(fs, format!(">> ESP8266 Flash Begin: size={} blocks={}", total_size, total_blocks));
    if let Err(e) = state.send_raw(&begin_cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(2000) {
        Ok(resp) => { log_push(fs, format!("<< Flash Begin resp: {}", format_hex_spaced(&resp))); }
        Err(e) => {
            log_push(fs, format!("<< Flash Begin timeout: {}", e));
            state.last_error = Some(e);
            return;
        }
    }
    log_push(fs, format!(">> ESP8266 Write ({} bytes, {} blocks)", data.len(), total_blocks));
    for (block_idx, block) in data.chunks(block_size).enumerate() {
        let cmd = esp.flash_data(block, block_idx as u32, total_blocks);
        if let Err(e) = state.send_raw(&cmd) {
            log_push(fs, format!("<< Send error: {}", e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                if resp.is_empty() {
                    log_push(fs, format!("   block[{}] no response", block_idx));
                }
            }
            Err(e) => {
                log_push(fs, format!("   block[{}] timeout: {}", block_idx, e));
            }
        }
        fs.progress = 0.3 + 0.5 * ((block_idx + 1) as f32 / total_blocks as f32);
    }
    log_push(fs, "<< Write complete");
    fs.progress = 0.8;
    state.last_error = None;
}

/// ESP8266 读取 Flash（备份）
pub(crate) fn do_esp8266_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    fs.read_chip = ChipType::Esp8266;
    let total = fs.read_len.max(1) as u32;
    let esp = Esp8266Flasher::new();
    let block_size: u32 = 4096;
    let total_blocks = (total + block_size - 1) / block_size;
    log_push(fs, format!(">> ESP8266 Read Flash total={} ({} blocks)", total, total_blocks));
    fs.last_read_data.clear();
    let mut remaining = total;
    let mut addr = fs.address;
    for block_idx in 0..total_blocks {
        let chunk = remaining.min(block_size);
        let cmd = esp.read_flash(addr, chunk);
        if let Err(e) = state.send_raw(&cmd) {
            log_push(fs, format!("<< Read[{}] send error: {}", block_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(3000) {
            Ok(resp) => {
                if resp.len() >= 10 {
                    let payload = &resp[10..];
                    let take = payload.len().min(chunk as usize);
                    fs.last_read_data.extend_from_slice(&payload[..take]);
                    log_push(fs, format!("   block[{}] +{} (total {})", block_idx, take, fs.last_read_data.len()));
                }
                fs.response = format_hex_spaced(&resp);
            }
            Err(e) => {
                log_push(fs, format!("<< Read[{}] timeout: {}", block_idx, e));
                state.last_error = Some(e);
                return;
            }
        }
        addr = addr.wrapping_add(chunk);
        remaining -= chunk;
        fs.progress = 0.1 + 0.85 * ((block_idx + 1) as f32 / total_blocks as f32);
    }
    log_push(fs, format!("<< Read complete: {} bytes", fs.last_read_data.len()));
    fs.progress = 1.0;
    state.last_error = None;
}

/// ESP8266 结束
pub(crate) fn do_esp8266_finish(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let esp = Esp8266Flasher::new();
    let cmd = esp.flash_end(true);
    log_push(fs, ">> ESP8266 Flash End (reboot)");
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(500) {
        Ok(resp) => {
            log_push(fs, format!("<< Finish resp: {}", format_hex_spaced(&resp)));
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

