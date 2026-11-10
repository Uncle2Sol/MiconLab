//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// Arduino (STK500 v1 / Optiboot) 单步操作实现
// =====================================================================

/// Arduino 获取同步
///
/// 发送 GET_SYNC 命令，期望响应 [STK_INSYNC, STK_OK] = [0x14, 0x10]
pub(crate) fn do_arduino_get_sync(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !check_connected(state, _is_zh) {
        return;
    }

    let stk = Stk500Flasher::new();
    let cmd = stk.get_sync();
    log_push(fs, format!(">> STK500 Get Sync: {}", format_hex_spaced(&cmd)));

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Stk500Flasher::is_ok(&resp) {
                log_push(fs, "<< Sync OK (0x14 0x10)");
                fs.progress = 0.1;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Sync unexpected resp: {}", format_hex_spaced(&resp)));
                state.last_error = Some("STK500 sync failed".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// Arduino 读签名（识别 AVR 芯片型号）
pub(crate) fn do_arduino_read_sign(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !check_connected(state, _is_zh) {
        return;
    }

    let stk = Stk500Flasher::new();
    let cmd = stk.read_sign();
    log_push(fs, ">> STK500 Read Sign");

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            match Stk500Flasher::parse_sign_response(&resp) {
                Ok(sign) => {
                    log_push(fs, format!(
                        "<< Signature: 0x{:02X} 0x{:02X} 0x{:02X}",
                        sign[0], sign[1], sign[2]
                    ));
                    match Stk500Flasher::identify_signature(&sign) {
                        Some((name, kb)) => {
                            log_push(fs, format!("   Identified: {} ({} KB Flash)", name, kb));
                            fs.arduino_chip_name = name.to_string();
                            fs.arduino_flash_kb = kb;
                            fs.progress = 0.2;
                            state.last_error = None;
                        }
                        None => {
                            log_push(fs, "   Unknown signature (not in AVR database)");
                            state.last_error = Some("Unknown AVR signature".to_string());
                        }
                    }
                }
                Err(e) => {
                    log_push(fs, format!("<< Sign parse error: {}", e));
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

/// Arduino 进入编程模式
pub(crate) fn do_arduino_enter_progmode(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !check_connected(state, _is_zh) {
        return;
    }

    let stk = Stk500Flasher::new();
    let cmd = stk.enter_progmode();
    log_push(fs, ">> STK500 Enter Progmode");

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Stk500Flasher::is_ok(&resp) {
                log_push(fs, "<< Enter Progmode OK");
                fs.progress = 0.25;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Unexpected resp: {}", format_hex_spaced(&resp)));
                state.last_error = Some("Enter progmode failed".to_string());
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// Arduino 退出编程模式
pub(crate) fn do_arduino_leave_progmode(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !check_connected(state, _is_zh) {
        return;
    }

    let stk = Stk500Flasher::new();
    let cmd = stk.leave_progmode();
    log_push(fs, ">> STK500 Leave Progmode");

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            if Stk500Flasher::is_ok(&resp) {
                log_push(fs, "<< Leave Progmode OK");
                fs.progress = 1.0;
                state.last_error = None;
            } else {
                log_push(fs, format!("<< Unexpected resp: {}", format_hex_spaced(&resp)));
            }
        }
        Err(e) => {
            log_push(fs, format!("<< No response: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// Arduino 写入 Flash（分页写入）
///
/// 流程：
///   1. 进入编程模式（由用户先点击「进入编程」）
///   2. 按 page_size 分块，每块 LOAD_ADDRESS + PROG_PAGE
///   3. AVR Flash 以 word 为单位寻址，byte_addr / 2 = word_addr
pub(crate) fn do_arduino_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }

    let data = match load_firmware_data(&fs.file_path, is_zh) {
        Ok(d) => d,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    if data.is_empty() {
        state.last_error = Some(crate::i18n::t("Firmware data is empty").to_string());
        return;
    }

    let stk = Stk500Flasher::new();
    let page_size = fs.arduino_page_size.max(16) as usize;
    let total_pages = (data.len() + page_size - 1) / page_size;

    log_push(fs, format!(
        ">> Arduino Write: {} bytes, {} pages (page={})",
        data.len(), total_pages, page_size
    ));

    for (page_idx, page) in data.chunks(page_size).enumerate() {
        // word address = byte address / 2
        let byte_addr = (page_idx * page_size) as u32;
        let word_addr = (byte_addr / 2) as u16;

        // 1. LOAD_ADDRESS
        let addr_cmd = stk.load_address(word_addr);
        if let Err(e) = state.send_raw(&addr_cmd) {
            log_push(fs, format!("<< Load addr[{}] error: {}", page_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(500) {
            Ok(resp) => {
                if !Stk500Flasher::is_ok(&resp) {
                    log_push(fs, format!(
                        "<< Load addr[{}] NAK: {}",
                        page_idx, format_hex_spaced(&resp)
                    ));
                    state.last_error = Some("Load address failed".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Load addr[{}] timeout: {}", page_idx, e));
                state.last_error = Some(e);
                return;
            }
        }

        // 2. PROG_PAGE
        let prog_cmd = stk.prog_page(flasher::stk500::MEM_FLASH, page);
        if let Err(e) = state.send_raw(&prog_cmd) {
            log_push(fs, format!("<< Prog page[{}] error: {}", page_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(1000) {
            Ok(resp) => {
                if !Stk500Flasher::is_ok(&resp) {
                    log_push(fs, format!(
                        "<< Prog page[{}] NAK: {}",
                        page_idx, format_hex_spaced(&resp)
                    ));
                    state.last_error = Some("Prog page failed".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Prog page[{}] timeout: {}", page_idx, e));
                state.last_error = Some(e);
                return;
            }
        }

        fs.progress = 0.25 + 0.7 * ((page_idx + 1) as f32 / total_pages as f32);
    }

    log_push(fs, "<< Write complete");
    fs.progress = 0.95;
    state.last_error = None;
}

/// Arduino 读取 Flash（备份）
///
/// 流程：
///   1. 进入编程模式（由用户先点击「进入编程」）
///   2. 按 page_size 分块，每块 LOAD_ADDRESS + READ_PAGE
///   3. 响应格式：[STK_INSYNC, data..., STK_OK]
pub(crate) fn do_arduino_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    fs.read_chip = ChipType::Arduino;

    let total_kb = fs.arduino_flash_kb.max(1) as u32;
    let total_bytes = total_kb * 1024;
    let page_size = fs.arduino_page_size.max(16) as u32;
    let total_pages = (total_bytes + page_size - 1) / page_size;

    let stk = Stk500Flasher::new();
    fs.last_read_data.clear();
    log_push(fs, format!(
        ">> Arduino Read: {} KB ({} pages, page={})",
        total_kb, total_pages, page_size
    ));

    for page_idx in 0..total_pages {
        let byte_addr = page_idx * page_size;
        let word_addr = (byte_addr / 2) as u16;

        // 1. LOAD_ADDRESS
        let addr_cmd = stk.load_address(word_addr);
        if let Err(e) = state.send_raw(&addr_cmd) {
            log_push(fs, format!("<< Load addr[{}] error: {}", page_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(500) {
            Ok(resp) => {
                if !Stk500Flasher::is_ok(&resp) {
                    log_push(fs, format!(
                        "<< Load addr[{}] NAK: {}",
                        page_idx, format_hex_spaced(&resp)
                    ));
                    state.last_error = Some("Load address failed".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Load addr[{}] timeout: {}", page_idx, e));
                state.last_error = Some(e);
                return;
            }
        }

        // 2. READ_PAGE
        let read_cmd = stk.read_page(flasher::stk500::MEM_FLASH, page_size as u16);
        if let Err(e) = state.send_raw(&read_cmd) {
            log_push(fs, format!("<< Read page[{}] error: {}", page_idx, e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                // 响应：[STK_INSYNC, data..., STK_OK]
                if resp.len() >= 2 && resp[0] == flasher::stk500::STK_INSYNC {
                    // 数据在 resp[1..len-1]，最后是 STK_OK
                    let data_end = if resp.last() == Some(&flasher::stk500::STK_OK) {
                        resp.len() - 1
                    } else {
                        resp.len()
                    };
                    let take = (data_end - 1).min(page_size as usize);
                    fs.last_read_data.extend_from_slice(&resp[1..1 + take]);
                    fs.response = format_hex_spaced(&resp);
                } else {
                    log_push(fs, format!(
                        "<< Read page[{}] unexpected: {}",
                        page_idx, format_hex_spaced(&resp)
                    ));
                    state.last_error = Some("Read page unexpected response".to_string());
                    return;
                }
            }
            Err(e) => {
                log_push(fs, format!("<< Read page[{}] timeout: {}", page_idx, e));
                state.last_error = Some(e);
                return;
            }
        }

        fs.progress = 0.1 + 0.85 * ((page_idx + 1) as f32 / total_pages as f32);

        // 每 16 页输出一次进度
        if page_idx % 16 == 0 {
            log_push(fs, format!(
                "   page[{}] @ 0x{:06X} (total {} bytes)",
                page_idx, byte_addr, fs.last_read_data.len()
            ));
        }
    }

    log_push(fs, crate::i18n::tf(
        "<< Read complete: {} bytes (use 'Save Backup' to save)",
        &[("{}", "{}", &format!("{}", fs.last_read_data.len()))],
    ));
    fs.progress = 1.0;
    state.last_error = None;
}

// =====================================================================

