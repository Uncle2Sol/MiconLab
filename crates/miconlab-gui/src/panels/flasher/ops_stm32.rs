//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// STM32 单步操作实现
// =====================================================================

/// STM32 连接（发送 0x7F，等待 ACK）
///
/// 连接流程：
/// 1. 自动尝试两种 DTR/RTS 极性（正常/反相）触发复位进入 Bootloader
/// 2. 每种极性发送 0x7F 同步字节，等待 ACK(0x79)
/// 3. 最多 6 次尝试，覆盖所有常见硬件设计
pub(crate) fn do_stm32_connect(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) { return; }
    let stm32 = Stm32Flasher::new();
    let cmd = stm32.get_command();

    let max_attempts = 6;

    for attempt in 1..=max_attempts {
        // 根据尝试次数选择策略：
        // 1-2: 正常极性 DTR/RTS 复位 (RTS=BOOT0, DTR=NRST)
        // 3-4: 反相极性 DTR/RTS 复位 (DTR=BOOT0, RTS=NRST)
        // 5-6: 仅发送 0x7F（用户可能已手动进入 Bootloader）
        let (use_reset, polarity) = match attempt {
            1..=2 => (true, "normal"),
            3..=4 => (true, "reversed"),
            _ => (false, "none"),
        };

        if use_reset {
            let polarity_desc = if polarity == "normal" {
                crate::i18n::t("Normal: RTS=BOOT0, DTR=NRST")
            } else {
                crate::i18n::t("Reversed: DTR=BOOT0, RTS=NRST")
            };
            log_push(fs, crate::i18n::tf(">> [{}/{}] DTR/RTS auto-reset ({})",
                &[("{}", "{}", &format!("{}", attempt)),
                  ("{}", "{}", &format!("{}", max_attempts)),
                  ("{}", "{}", &format!("{}", polarity_desc))]));

            if polarity == "normal" {
                // 正常极性：RTS 控制 BOOT0，DTR 控制 NRST
                state.set_rts(true);                    // BOOT0=1
                std::thread::sleep(std::time::Duration::from_millis(10));
                state.set_dtr(true);                    // NRST 拉低
                std::thread::sleep(std::time::Duration::from_millis(50));
                state.set_dtr(false);                   // NRST 释放
                std::thread::sleep(std::time::Duration::from_millis(100));
                state.set_rts(false);                   // BOOT0 恢复
            } else {
                // 反相极性：DTR 控制 BOOT0，RTS 控制 NRST
                state.set_dtr(true);                    // BOOT0=1
                std::thread::sleep(std::time::Duration::from_millis(10));
                state.set_rts(true);                    // NRST 拉低
                std::thread::sleep(std::time::Duration::from_millis(50));
                state.set_rts(false);                   // NRST 释放
                std::thread::sleep(std::time::Duration::from_millis(100));
                state.set_dtr(false);                   // BOOT0 恢复
            }

            log_push(fs, crate::i18n::t("   Reset done, sending 0x7F..."));
        } else {
            log_push(fs, crate::i18n::tf(">> [{}/{}] Sending 0x7F directly (no DTR/RTS reset)",
                &[("{}", "{}", &format!("{}", attempt)),
                  ("{}", "{}", &format!("{}", max_attempts))]));
        }

        log_push(fs, format!(">> STM32 Connect [{}/{}]: {}", attempt, max_attempts, format_hex_spaced(&cmd)));

        // 清空输入缓冲区
        state.flush_input();

        if let Err(e) = state.send_raw(&cmd) {
            log_push(fs, format!("<< Send error: {}", e));
            state.last_error = Some(e);
            continue;
        }

        match state.read_response(500) {
            Ok(resp) => {
                fs.response = format_hex_spaced(&resp);
                match Stm32Flasher::parse_ack(&resp) {
                    Ok(()) => {
                        log_push(fs, format!("<< ACK (0x79) - Bootloader active ✓ [attempt {}]", attempt));
                        fs.bootloader_connected = true;
                        state.last_error = None;

                        // 连接成功后，自动发送 GET 命令 (0x00 0xFF) 读取 Bootloader 版本和支持的命令列表
                        // 这是 STM32 UART Bootloader 协议 (AN3155 §3.2) 的标准握手步骤
                        log_push(fs, crate::i18n::t(">> GET (0x00 0xFF) reading Bootloader version..."));
                        let get_cmd = stm32.get_info_command();
                        state.flush_input();
                        if let Err(e) = state.send_raw(&get_cmd) {
                            log_push(fs, format!("<< GET send error: {}", e));
                        } else {
                            match state.read_response(500) {
                                Ok(get_resp) => {
                                    fs.response = format_hex_spaced(&get_resp);
                                    match Stm32Flasher::parse_get_response(&get_resp) {
                                        Ok((version, commands)) => {
                                            let ver_major = (version >> 4) & 0x0F;
                                            let ver_minor = version & 0x0F;
                                            log_push(fs, crate::i18n::tf(
                                                "<< Bootloader v{}.{} | {} commands supported: {}",
                                                &[("{}", "{}", &format!("{}", ver_major)),
                                                  ("{}", "{}", &format!("{}", ver_minor)),
                                                  ("{}", "{}", &format!("{}", commands.len())),
                                                  ("{}", "{}", &format!("{}", commands.iter().map(|c| format!("0x{:02X}", c)).collect::<Vec<_>>().join(" ")))]
                                            ));
                                        }
                                        Err(e) => {
                                            log_push(fs, crate::i18n::tf("<< Failed to parse GET: {} (resp: {})",
                                                &[("{}", "{}", &format!("{}", e)),
                                                  ("{}", "{}", &format!("{}", format_hex_spaced(&get_resp)))]));
                                        }
                                    }
                                }
                                Err(e) => {
                                    log_push(fs, crate::i18n::tf("<< No response to GET: {}",
                                        &[("{}", "{}", &format!("{}", e))]));
                                }
                            }
                        }

                        // 连接（含版本读取）完成，重置进度条
                        // 进度条仅在实际烧录/擦除/读取操作时显示进度，连接是预备步骤
                        fs.progress = 0.0;
                        log_push(fs, crate::i18n::t("✓ Bootloader connected, ready for erase/write/read"));
                        return; // 成功，退出重试循环
                    }
                    Err(e) => {
                        log_push(fs, format!("<< [{}] NAK: {} (resp: {})", attempt, e, format_hex_spaced(&resp)));
                    }
                }
            }
            Err(e) => {
                log_push(fs, format!("<< [{}] No response: {}", attempt, e));
            }
        }

        // 短暂等待后重试
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 6 次均失败
    fs.bootloader_connected = false;
    let err_msg = crate::i18n::t("ACK(0x79) not received after 6 attempts, device may not be in Bootloader mode").to_string();
    log_push(fs, format!("⚠ {}", err_msg));
    state.last_error = Some(err_msg);
}

/// STM32 全片擦除（2 段命令，逐段等待 ACK）
pub(crate) fn do_stm32_mass_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("⚠ Bootloader not connected, attempting anyway (may fail)"));
    }

    let stm32 = Stm32Flasher::new();
    let segments = stm32.mass_erase();
    log_push(fs, ">> STM32 Mass Erase");

    for (i, seg) in segments.iter().enumerate() {
        log_push(fs, format!("   seg[{}]: {}", i, format_hex_spaced(seg)));
        if let Err(e) = state.send_raw(seg) {
            log_push(fs, format!("<< Send error: {}", e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                if Stm32Flasher::is_ack(resp.first().copied().unwrap_or(0)) {
                    log_push(fs, format!("   seg[{}] ACK", i));
                } else {
                    log_push(fs, format!("   seg[{}] NAK: {}", i, format_hex_spaced(&resp)));
                    state.last_error = Some("Mass erase NAK".to_string());
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

/// STM32 页擦除（使用扩展擦除命令）
pub(crate) fn do_stm32_page_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("⚠ Bootloader not connected, attempting anyway (may fail)"));
    }

    let pages = match parse_pages(&fs.pages) {
        Ok(p) => p,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    if pages.is_empty() {
        state.last_error = Some(crate::i18n::t("Page list is empty").to_string());
        return;
    }

    let stm32 = Stm32Flasher::new();
    let segments = stm32.extended_erase_pages(&pages);
    if segments.is_empty() {
        state.last_error = Some(crate::i18n::t("Failed to build page erase command").to_string());
        return;
    }

    log_push(fs, format!(">> STM32 Page Erase: {:?}", pages));

    for (i, seg) in segments.iter().enumerate() {
        log_push(fs, format!("   seg[{}]: {}", i, format_hex_spaced(seg)));
        if let Err(e) = state.send_raw(seg) {
            log_push(fs, format!("<< Send error: {}", e));
            state.last_error = Some(e);
            return;
        }
        match state.read_response(2000) {
            Ok(resp) => {
                if Stm32Flasher::is_ack(resp.first().copied().unwrap_or(0)) {
                    log_push(fs, format!("   seg[{}] ACK", i));
                } else {
                    log_push(fs, format!("   seg[{}] NAK: {}", i, format_hex_spaced(&resp)));
                    state.last_error = Some("Page erase NAK".to_string());
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

    log_push(fs, "<< Page erase complete");
    fs.progress = 0.3;
    state.last_error = None;
}

/// STM32 写内存（3 段命令，逐段等待 ACK）
pub(crate) fn do_stm32_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("⚠ Bootloader not connected, attempting anyway (may fail)"));
    }

    // 读取固件文件并解析
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

    let stm32 = Stm32Flasher::new();
    // STM32 每次最多写 256 字节，分块写入
    let chunk_size = 256usize;
    let total_chunks = (data.len() + chunk_size - 1) / chunk_size;
    log_push(fs, format!(">> STM32 Write @ 0x{:08X} ({} bytes, {} chunks)", fs.address, data.len(), total_chunks));

    for (chunk_idx, chunk) in data.chunks(chunk_size).enumerate() {
        let addr = fs.address + (chunk_idx * chunk_size) as u32;
        let segments = stm32.write_memory(addr, chunk);
        if segments.is_empty() {
            log_push(fs, format!("   chunk[{}] build failed", chunk_idx));
            state.last_error = Some(crate::i18n::t("Write command build failed").to_string());
            return;
        }

        for (i, seg) in segments.iter().enumerate() {
            if let Err(e) = state.send_raw(seg) {
                log_push(fs, format!("<< Send error: {}", e));
                state.last_error = Some(e);
                return;
            }
            match state.read_response(1000) {
                Ok(resp) => {
                    if Stm32Flasher::is_ack(resp.first().copied().unwrap_or(0)) {
                        // ACK
                    } else {
                        log_push(fs, format!(
                            "   chunk[{}] seg[{}] NAK: {}",
                            chunk_idx, i, format_hex_spaced(&resp)
                        ));
                        state.last_error = Some("Write NAK".to_string());
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
        log_push(fs, format!("   chunk[{}] written @ 0x{:08X}", chunk_idx, addr));
    }

    log_push(fs, "<< Write complete");
    fs.progress = 0.8;
    state.last_error = None;
}

/// STM32 读内存
///
/// STM32 UART Bootloader 的 READ_MEMORY 命令单次最多读取 256 字节。
/// 若 `read_len` > 256，自动分块读取并将结果拼接到 `last_read_data`。
pub(crate) fn do_stm32_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("⚠ Bootloader not connected, attempting read anyway (may fail)"));
    }

    // 启动非阻塞读取状态机，避免 GUI 卡死
    let total = fs.read_len.max(1) as usize;
    fs.last_read_data.clear();
    fs.read_running = true;
    fs.read_addr = fs.address;
    fs.read_remaining = total;
    fs.read_total = total;
    fs.read_chip = fs.chip_type; // STM32/GD32 共用此路径，按实际选择记录
    fs.read_phase = 0; // 0 = 发送段0
    fs.read_raw_mode = false;
    fs.read_resp_buf.clear();
    fs.read_wait_until = None;
    fs.progress = 0.0;
    state.flasher_read_busy = true; // 阻止 poll_rx 抢占响应
    log_push(fs, crate::i18n::tf(">> STM32 Read @ 0x{:08X} total={} (non-blocking)",
        &[("{:08X}", "{:08X}", &format!("{:08X}", fs.address)),
          ("{}", "{}", &format!("{}", total))]));
}

/// 非阻塞读取状态机：每帧推进一步，避免 GUI 卡死
/// 支持 STM32 分块读取（每块最多 256 字节）
pub(crate) fn update_flasher_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    // esptool 后备读取运行时，跳过状态机（由 poll_esptool_result 处理）
    if fs.esptool_running {
        return;
    }
    if !fs.read_running {
        // 读取已停止，清除忙标志
        if state.flasher_read_busy {
            state.flasher_read_busy = false;
        }
        return;
    }
    if !state.is_connected() {
        fs.read_running = false;
        state.flasher_read_busy = false;
        log_push(fs, crate::i18n::t("<< Read aborted: connection lost"));
        return;
    }

    match fs.read_chip {
        ChipType::Stm32 | ChipType::Gd32 => update_stm32_read_step(fs, state, is_zh),
        ChipType::Esp32 => update_esp32_read_step(fs, state, is_zh),
        // 其他芯片类型暂不使用状态机（保持原有同步行为）
        _ => {
            fs.read_running = false;
            state.flasher_read_busy = false;
        }
    }

    // 如果状态机内部停止了读取，清除忙标志
    if !fs.read_running {
        state.flasher_read_busy = false;
    }
}

/// STM32 读取状态机单步推进
///
/// 使用 AN3155 标准分段协议，每段发送后等待 ACK：
///   Phase 0: 发送段0 (0x11 0xEE) → Phase 1
///   Phase 1: 读 ACK → Phase 2
///   Phase 2: 发送段1 (addr + checksum) → Phase 3
///   Phase 3: 读 ACK → Phase 4
///   Phase 4: 发送段2 (len + complement) → Phase 5
///   Phase 5: 读 ACK + data(N) + ACK → 回到 Phase 0 (下一块)
pub(crate) fn update_stm32_read_step(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    // 原始读取模式：直接读取串口数据，不依赖 Bootloader 协议
    if fs.read_raw_mode {
        if let Ok(resp) = state.read_response(50) {
            if !resp.is_empty() {
                fs.last_read_data.extend_from_slice(&resp);
                fs.progress = 0.1 + 0.8 * (fs.last_read_data.len() as f32 / fs.read_total as f32).min(1.0);
                if fs.last_read_data.len() % 256 == 0 || fs.last_read_data.len() >= fs.read_total {
                    log_push(fs, crate::i18n::tf(
                        "   @ raw read +{} bytes (total {})",
                        &[("{}", "{}", &format!("{}", resp.len())),
                          ("{}", "{}", &format!("{}", fs.last_read_data.len()))]
                    ));
                }
            }
        }
        // 达到目标字节数或超时则结束
        if fs.last_read_data.len() >= fs.read_total {
            fs.read_running = false;
            fs.progress = 1.0;
            state.flasher_read_busy = false;
            log_push(fs, crate::i18n::tf(
                "<< Raw read complete: {} bytes (note: may contain application data instead of firmware)",
                &[("{}", "{}", &format!("{}", fs.last_read_data.len()))]
            ));
            state.last_error = None;
        } else if fs.read_wait_until.map_or(false, |t| std::time::Instant::now() >= t) {
            fs.read_running = false;
            state.flasher_read_busy = false;
            log_push(fs, crate::i18n::tf(
                "<< Raw read timed out: {}/{} bytes",
                &[("{}", "{}", &format!("{}", fs.last_read_data.len())),
                  ("{}", "{}", &format!("{}", fs.read_total))]
            ));
            if !fs.last_read_data.is_empty() {
                state.last_error = None;
                log_push(fs, crate::i18n::tf(
                    "<< Partial data saved ({} bytes), you can click 'Save Backup'",
                    &[("{}", "{}", &format!("{}", fs.last_read_data.len()))]
                ));
            }
        }
        return;
    }

    let stm32 = Stm32Flasher::new();
    // STM32 协议长度字节：N-1，N 范围 1-256。当 N=256 时长度字节为 0xFF（即 255）。
    // chunk_u8 是传给协议的长度字节，data_len 是实际期望读取的数据字节数。
    let data_len = fs.read_remaining.min(256);
    let chunk_u8 = if data_len == 256 { 0u8 } else { data_len as u8 };

    match fs.read_phase {
        // === 段0: 发送 READ 命令 ===
        0 => {
            // 清空残留数据，确保只读到本次命令的响应
            state.flush_input();
            let segments = stm32.read_memory_segments(fs.read_addr, chunk_u8);
            if let Err(e) = state.send_raw(&segments[0]) {
                log_push(fs, format!("<< Send cmd error: {}", e));
                state.last_error = Some(e);
                fs.read_running = false;
                return;
            }
            fs.read_phase = 1;
            fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(1000));
        }
        // === 段0 ACK ===
        1 => {
            match state.read_response(50) {
                Ok(resp) => {
                    if Stm32Flasher::is_ack(resp[0]) {
                        fs.read_phase = 2;
                    } else {
                        // 设备未进入 Bootloader，回退到原始读取模式
                        log_push(fs, crate::i18n::tf("<< Cmd NAK (not in Bootloader mode): {}",
                            &[("{}", "{}", &format!("{}", format_hex_spaced(&resp)))]));
                        log_push(fs, crate::i18n::t(">> Switching to raw read mode (read serial data directly)"));
                        fs.read_raw_mode = true;
                        fs.read_resp_buf.clear();
                        fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(3000));
                        fs.read_phase = 10; // 原始读取阶段
                    }
                }
                Err(_) => {
                    if fs.read_wait_until.map_or(true, |t| std::time::Instant::now() >= t) {
                        log_push(fs, format!("<< Cmd ACK timeout"));
                        // 超时也回退到原始读取模式
                        log_push(fs, crate::i18n::t(">> Switching to raw read mode (read serial data directly)"));
                        fs.read_raw_mode = true;
                        fs.read_resp_buf.clear();
                        fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(3000));
                        fs.read_phase = 10;
                    }
                }
            }
        }
        // === 段1: 发送地址 + 校验和 ===
        2 => {
            let segments = stm32.read_memory_segments(fs.read_addr, chunk_u8);
            if let Err(e) = state.send_raw(&segments[1]) {
                log_push(fs, format!("<< Send addr error: {}", e));
                state.last_error = Some(e);
                fs.read_running = false;
                return;
            }
            fs.read_phase = 3;
            fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(1000));
        }
        // === 段1 ACK ===
        3 => {
            match state.read_response(50) {
                Ok(resp) => {
                    if Stm32Flasher::is_ack(resp[0]) {
                        fs.read_phase = 4;
                    } else {
                        log_push(fs, format!("<< Addr NAK: {}", format_hex_spaced(&resp)));
                        state.last_error = Some("Read address NAK".to_string());
                        fs.read_running = false;
                    }
                }
                Err(_) => {
                    if fs.read_wait_until.map_or(true, |t| std::time::Instant::now() >= t) {
                        log_push(fs, format!("<< Addr ACK timeout"));
                        state.last_error = Some("Read address ACK timeout".to_string());
                        fs.read_running = false;
                    }
                }
            }
        }
        // === 段2: 发送长度 + 补码 ===
        4 => {
            let segments = stm32.read_memory_segments(fs.read_addr, chunk_u8);
            if let Err(e) = state.send_raw(&segments[2]) {
                log_push(fs, format!("<< Send len error: {}", e));
                state.last_error = Some(e);
                fs.read_running = false;
                return;
            }
            fs.read_resp_buf.clear();
            fs.read_phase = 5;
            fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
        }
        // === 段2 响应: ACK + data(N) + ACK ===
        5 => {
            let expected_len = data_len + 2; // ACK + N bytes + ACK
            let min_len = data_len + 1;      // ACK + N bytes（尾 ACK 可选）

            if let Ok(resp) = state.read_response(50) {
                fs.read_resp_buf.extend_from_slice(&resp);
            }

            let buf_len = fs.read_resp_buf.len();

            // 完整响应（含尾 ACK）或全部数据已到（尾 ACK 可选）
            if buf_len >= min_len && Stm32Flasher::is_ack(fs.read_resp_buf[0]) {
                // 提取数据（跳过首 ACK，取 N 字节）
                let data: Vec<u8> = fs.read_resp_buf[1..1 + data_len].to_vec();
                fs.last_read_data.extend_from_slice(&data);

                // 消费已读字节：如果有尾 ACK 就一起消费，避免残留污染下一条命令
                let consumed = if buf_len >= expected_len { expected_len } else { min_len };
                let trailing_ack_ok = buf_len >= expected_len && Stm32Flasher::is_ack(fs.read_resp_buf[expected_len - 1]);
                fs.response = format_hex_spaced(&fs.read_resp_buf[..consumed]);
                fs.read_resp_buf.drain(..consumed);

                log_push(fs, format!(
                    "   @ 0x{:08X} +{} bytes (total {}){}",
                    fs.read_addr, data.len(), fs.last_read_data.len(),
                    if trailing_ack_ok { "" } else { crate::i18n::t(" (trailing ACK skipped)") }
                ));

                // 推进地址和进度
                fs.read_addr = fs.read_addr.wrapping_add(data_len as u32);
                fs.read_remaining -= data_len;
                fs.progress = 0.1 + 0.8 * (1.0 - fs.read_remaining as f32 / fs.read_total as f32);
                fs.read_phase = 0;

                if fs.read_remaining == 0 {
                    fs.read_running = false;
                    fs.progress = 1.0;
                    log_push(fs, crate::i18n::tf(
                        "<< Read complete: {} bytes (use 'Save Backup' to save)",
                        &[("{}", "{}", &format!("{}", fs.last_read_data.len()))]
                    ));
                    // 内联固件校验：分析读取的数据是否像有效的 STM32 固件
                    inspect_read_data(fs, is_zh);
                    state.last_error = None;
                }
            } else if fs.read_wait_until.map_or(true, |t| std::time::Instant::now() >= t) {
                // 超时：数据不完整
                let partial = fs.read_resp_buf.len();
                if partial >= 1 && Stm32Flasher::is_ack(fs.read_resp_buf[0]) && partial > 1 {
                    // ACK 已到，保存部分数据
                    let got = (partial - 1).min(data_len);
                    let data = &fs.read_resp_buf[1..1 + got];
                    fs.last_read_data.extend_from_slice(data);
                    fs.read_addr = fs.read_addr.wrapping_add(got as u32);
                    fs.read_remaining = fs.read_remaining.saturating_sub(got);
                    log_push(fs, format!(
                        "<< Partial read @ 0x{:08X}: {}/{} bytes",
                        fs.read_addr, got, data_len
                    ));
                }
                log_push(fs, format!(
                    "<< Read timeout @ 0x{:08X} (got {}/{})",
                    fs.read_addr, partial, expected_len
                ));
                state.last_error = Some("Read data timeout".to_string());
                fs.read_running = false;
            }
        }
        _ => {
            fs.read_phase = 0;
        }
    }
}

/// STM32 跳转执行
pub(crate) fn do_stm32_go(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("⚠ Bootloader not connected, attempting anyway (may fail)"));
    }

    let stm32 = Stm32Flasher::new();
    let cmd = stm32.go(fs.address);
    log_push(fs, format!(">> STM32 Go @ 0x{:08X}", fs.address));

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    log_push(fs, "<< Go command sent");
    fs.progress = 1.0;
    state.last_error = None;
}

/// STM32 读 UID
pub(crate) fn do_stm32_read_uid(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("⚠ Bootloader not connected, attempting anyway (may fail)"));
    }

    let stm32 = Stm32Flasher::new();
    let cmd = stm32.read_uid();
    log_push(fs, format!(">> STM32 Read UID: {}", format_hex_spaced(&cmd)));

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< UID: {}", format_hex_spaced(&resp)));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

// =====================================================================

