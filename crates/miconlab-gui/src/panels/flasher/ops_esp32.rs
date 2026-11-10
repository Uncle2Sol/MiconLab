//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// ESP32 单步操作实现
// =====================================================================

/// 通过 DTR/RTS 信号自动复位 ESP32 进入 Bootloader 模式
///
/// ESP32 开发板通常有自动复位电路：DTR→EN(复位)，RTS→GPIO0(Boot 模式)
/// 无物理按键的 PCB 也可通过此方式进入下载模式
///
/// 尝试多种 DTR/RTS 复位序列进入 Bootloader
///
/// ESP32-S3 原生 USB-CDC 的 USB-Serial-JTAG 控制器使用与 UART 自动复位电路
/// 完全不同的信号映射，需要尝试多种序列
pub(crate) fn reset_esp32_to_bootloader(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) -> bool {
    let esp = Esp32Flasher::new();
    // 2 个 SYNC 包足够（esptool 实际只发 1 个）；
    // 多发包 = ROM 多回 OHAI 帧 = 排水负担，7 个是 OHAI 风暴的主要来源
    let packets = esp.sync_repeated(2);

    // 先清空残留的应用固件数据
    state.flush_input();

    // 策略 1: esptool USB-JTAG 序列（ESP32-S3 原生 USB 专用）
    // 序列: RTS=True,DTR=False → RTS=False,DTR=True → DTR=False → wait → sync
    // 这是 esptool 对 ESP32-S3 USB-CDC 的标准复位方式
    log_push(fs, crate::i18n::t(">> USB-JTAG reset sequence (esptool S3 standard)..."));
    if try_usb_jtag_reset(fs, state, &packets, true) {
        return true;
    }

    // 策略 2: esptool USB-JTAG 反相序列
    log_push(fs, crate::i18n::t(">> USB-JTAG reset sequence (inverted)..."));
    if try_usb_jtag_reset(fs, state, &packets, false) {
        return true;
    }

    // 策略 3: 标准自动复位（DTR=EN 反相, RTS=GPIO0 反相）
    log_push(fs, crate::i18n::t(">> Auto reset (standard polarity: DTR=EN, RTS=GPIO0)..."));
    if try_esp32_reset_and_sync(fs, state, &packets, |s, en_low, gpio0_low| {
        s.set_dtr(en_low);
        s.set_rts(gpio0_low);
    }) {
        return true;
    }

    // 策略 4: 反相自动复位（RTS=EN, DTR=GPIO0）
    log_push(fs, crate::i18n::t(">> Auto reset (inverted polarity: RTS=EN, DTR=GPIO0)..."));
    if try_esp32_reset_and_sync(fs, state, &packets, |s, en_low, gpio0_low| {
        s.set_rts(en_low);
        s.set_dtr(gpio0_low);
    }) {
        return true;
    }

    false
}

/// esptool USB-JTAG 复位序列（ESP32-S3 原生 USB-CDC 专用）
///
/// 标准序列: RTS↑ DTR↓ → RTS↓ DTR↑ → DTR↓ → wait → sync
/// 反相序列: DTR↑ RTS↓ → DTR↓ RTS↑ → RTS↓ → wait → sync
pub(crate) fn try_usb_jtag_reset(
    fs: &mut FlasherState,
    state: &mut AppState,
    packets: &[Vec<u8>],
    standard: bool,
) -> bool {
    // 阶段 1: 拉低 EN（复位），释放 GPIO0
    if standard {
        state.set_rts(true);  // EN=LOW (reset)
        state.set_dtr(false); // GPIO0=HIGH
    } else {
        state.set_dtr(true);  // EN=LOW (reset)
        state.set_rts(false); // GPIO0=HIGH
    }
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 阶段 2: 释放 EN（启动），同时拉低 GPIO0（Bootloader 模式）
    if standard {
        state.set_rts(false); // EN=HIGH (release reset)
        state.set_dtr(true);  // GPIO0=LOW (bootloader)
    } else {
        state.set_dtr(false); // EN=HIGH (release reset)
        state.set_rts(true);  // GPIO0=LOW (bootloader)
    }
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 阶段 3: 释放 GPIO0
    if standard {
        state.set_dtr(false); // GPIO0=HIGH
    } else {
        state.set_rts(false); // GPIO0=HIGH
    }
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 发送同步包，检测 SLIP 响应
    for (i, pkt) in packets.iter().enumerate() {
        if let Err(e) = state.send_raw(pkt) {
            log_push(fs, format!("   sync[{}] send error: {}", i, e));
            continue;
        }
        if let Ok(resp) = state.read_response(100) {
            if resp.contains(&0xC0) {
                log_push(fs, format!("<< Sync OK (USB-JTAG sync[{}] SLIP, {}B)", i, resp.len()));
                fs.bootloader_connected = true;
                fs.progress = 0.1;
                state.last_error = None;
                state.set_dtr(false);
                state.set_rts(false);
                return true;
            }
        }
    }

    // 释放信号
    state.set_dtr(false);
    state.set_rts(false);
    std::thread::sleep(std::time::Duration::from_millis(50));
    log_push(fs, crate::i18n::t("   No SLIP response detected"));
    false
}

/// 执行一次 DTR/RTS 复位序列 + 同步检测
pub(crate) fn try_esp32_reset_and_sync<F>(
    fs: &mut FlasherState,
    state: &mut AppState,
    packets: &[Vec<u8>],
    set_signals: F,
) -> bool
where
    F: Fn(&mut AppState, bool, bool), // (state, en_low, gpio0_low)
{
    // 步骤 1: GPIO0=LOW (Bootloader 模式), EN=HIGH (准备)
    set_signals(state, false, true);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 步骤 2: EN=LOW (复位), GPIO0 保持 LOW
    set_signals(state, true, true);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // 步骤 3: EN=HIGH (释放复位 → 以 GPIO0=LOW 启动 → Bootloader)
    // GPIO0 保持 LOW，不复位后 flush（会冲掉 bootloader 响应）
    set_signals(state, false, true);
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 步骤 4: 发送同步包，检测 SLIP 响应（不 flush_input！）
    for (i, pkt) in packets.iter().enumerate() {
        if let Err(e) = state.send_raw(pkt) {
            log_push(fs, format!("   sync[{}] send error: {}", i, e));
            continue;
        }
        if let Ok(resp) = state.read_response(100) {
            if resp.contains(&0xC0) {
                log_push(fs, format!("<< Sync OK (sync[{}] SLIP frame, {}B)", i, resp.len()));
                fs.bootloader_connected = true;
                fs.progress = 0.1;
                state.last_error = None;
                // 释放 GPIO0
                set_signals(state, false, false);
                return true;
            }
        }
    }

    // 释放信号
    set_signals(state, false, false);
    std::thread::sleep(std::time::Duration::from_millis(50));
    log_push(fs, crate::i18n::t("   No SLIP response detected"));
    false
}

/// ESP32 同步（先尝试 DTR/RTS 自动复位，再发送同步包）
pub(crate) fn do_esp32_sync(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }

    log_push(fs, ">> ESP32 Sync");
    fs.bootloader_connected = false;

    // 尝试 DTR/RTS 自动复位进入 Bootloader（无按键 PCB 也能用）
    if reset_esp32_to_bootloader(fs, state, is_zh) {
        return; // 已同步成功
    }

    // 自动复位失败，尝试直接同步（用户可能已手动进入 Bootloader）
    log_push(fs, crate::i18n::t(">> Syncing directly (no auto reset)..."));
    let esp = Esp32Flasher::new();
    // 2 个 SYNC 包足够（esptool 实际只发 1 个）；
    // 多发包 = ROM 多回 OHAI 帧 = 排水负担，7 个是 OHAI 风暴的主要来源
    let packets = esp.sync_repeated(2);
    state.flush_input();

    for (i, pkt) in packets.iter().enumerate() {
        if let Err(e) = state.send_raw(pkt) {
            log_push(fs, format!("<< Send error [{}]: {}", i, e));
            state.last_error = Some(e);
            return;
        }
        if let Ok(resp) = state.read_response(100) {
            if !resp.is_empty() {
                log_push(fs, format!("   sync[{}] resp: {}", i, format_hex_spaced(&resp)));
                if resp.contains(&0xC0) {
                    log_push(fs, "<< Sync OK (SLIP frame detected)");
                    fs.bootloader_connected = true;
                    fs.progress = 0.1;
                    state.last_error = None;
                    return;
                } else {
                    log_push(fs, crate::i18n::tf("   sync[{}] is not a SLIP frame (may be app firmware output), retrying...",
                        &[("{}", "{}", &format!("{}", i))]));
                }
            }
        }
    }

    log_push(fs, crate::i18n::t("<< Sync failed: no SLIP response. Hold BOOT, press RST to enter download mode"));
    state.last_error = Some(crate::i18n::t("ESP32 sync failed: enter Bootloader mode manually (BOOT+RST)").to_string());
}

/// ESP32 获取芯片 ID
pub(crate) fn do_esp32_chip_id(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("<< Click Sync first to enter Bootloader mode"));
        return;
    }

    let esp = Esp32Flasher::new();
    let cmd = esp.chip_id();
    log_push(fs, ">> ESP32 Chip ID");

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            fs.response = format_hex_spaced(&resp);
            log_push(fs, format!("<< Chip ID resp: {}", format_hex_spaced(&resp)));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// ESP32 改波特率
pub(crate) fn do_esp32_change_baud(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }

    let esp = Esp32Flasher::new();
    let cmd = esp.change_baudrate(fs.baud_new, fs.baud_current);
    log_push(fs, format!(">> ESP32 Change Baud: {} -> {}", fs.baud_current, fs.baud_new));

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            log_push(fs, format!("<< Baud change resp: {}", format_hex_spaced(&resp)));
            log_push(fs, crate::i18n::t("  Note: change serial baud rate manually"));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// ESP32 SPI Attach
pub(crate) fn do_esp32_spi_attach(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !check_connected(state, _is_zh) {
        return;
    }

    let esp = Esp32Flasher::new();
    let cmd = esp.spi_attach(0); // 默认 SPI 配置
    log_push(fs, ">> ESP32 SPI Attach (config=0)");

    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match state.read_response(500) {
        Ok(resp) => {
            log_push(fs, format!("<< SPI Attach resp: {}", format_hex_spaced(&resp)));
            state.last_error = None;
        }
        Err(e) => {
            log_push(fs, format!("<< Read error: {}", e));
            state.last_error = Some(e);
        }
    }
}

/// ESP32 擦除（flash_begin）
pub(crate) fn do_esp32_erase(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }

    // 读取固件文件确定擦除大小
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

    let erase_size = data.len() as u32;
    let num_sectors = (erase_size + 4095) / 4096;
    let esp = Esp32Flasher::new();
    let cmd = esp.flash_begin(erase_size, num_sectors, fs.address);
    log_push(fs, format!(">> ESP32 Flash Begin: size={} sectors={}", erase_size, num_sectors));

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

/// ESP32 写入数据
///
/// ESP32 烧录协议流程（修复点：补全 flash_begin 前置步骤）：
///   1. flash_begin：设置总写入大小、块数、起始地址（payload 12 字节）
///   2. flash_data：逐块发送数据（每块 4KB，含序列号与 CRC32C 校验）
///   3. flash_end：完成写入（由独立的"结束"按钮触发）
pub(crate) fn do_esp32_write(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
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

    let esp = Esp32Flasher::new();
    let block_size = 4096usize;
    let total_blocks = ((data.len() + block_size - 1) / block_size) as u32;

    // ---- 步骤 1: flash_begin —— 设置写入总大小、块数与起始偏移（修复点：原实现缺失 offset）----
    let total_size = data.len() as u32;
    let begin_cmd = esp.flash_begin(total_size, total_blocks, fs.address);
    log_push(fs, format!(
        ">> ESP32 Flash Begin: size={} blocks={} @ 0x{:08X}",
        total_size, total_blocks, fs.address
    ));
    if let Err(e) = state.send_raw(&begin_cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }
    match state.read_response(2000) {
        Ok(resp) => {
            log_push(fs, format!("<< Flash Begin resp: {}", format_hex_spaced(&resp)));
        }
        Err(e) => {
            log_push(fs, format!("<< Flash Begin timeout: {}", e));
            state.last_error = Some(e);
            return;
        }
    }

    // ---- 步骤 2: flash_data —— 逐块发送数据 ----
    log_push(fs, format!(
        ">> ESP32 Write @ 0x{:08X} ({} bytes, {} blocks)",
        fs.address, data.len(), total_blocks
    ));

    for (block_idx, block) in data.chunks(block_size).enumerate() {
        // ESP_FLASH_DATA 标准格式：size(4) + seq(4) + 0(4) + 0(4) + data
        // （地址由 flash_begin 的 offset 指定，块序号驱动 ROM 内部写入位置递增）
        let cmd = esp.flash_data(block, block_idx as u32);

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

/// ESP32 MD5 校验
pub(crate) fn do_esp32_md5(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
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

    let esp = Esp32Flasher::new();
    let size = data.len() as u32;
    let cmd = esp.spi_flash_md5(fs.address, size);
    log_push(fs, format!(">> ESP32 MD5 @ 0x{:08X} size={}", fs.address, size));

    // 发送前清残留；读取时命令-响应严格配对（跳过残留 SYNC 帧，USB-JTAG 响应快且多帧）
    state.flush_input();
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, format!("<< Send error: {}", e));
        state.last_error = Some(e);
        return;
    }

    match read_slip_response_for(fs, state, 2000, Some(0x13)) {
        Some(body) => {
            match Esp32Flasher::parse_md5_response(&body) {
                Ok(m) => {
                    log_push(fs, format!("<< MD5: {}", format_hex_spaced(&m)));
                }
                Err(e) => {
                    log_push(fs, format!("<< MD5 parse error: {} (raw: {})", e, format_hex_spaced(&body)));
                }
            }
            state.last_error = None;
        }
        None => {
            log_push(fs, crate::i18n::t("<< No response (enter bootloader first)"));
            state.last_error = Some("No response".to_string());
        }
    }
}

/// ESP32 一键在线体检（识别 + 固件验证）
///
/// 完整闭环，无需外部工具：
///   Phase A: RTS 复位 → 采集启动日志（ROM 行 / boot mode / 崩溃检测）
///   Phase B: 自动复位进 Bootloader → SECURITY_INFO(chip_id/加密状态)
///            → SPI_ATTACH + MD5（bootloader 区/分区表区非空校验）
///            → FLASH_END 复位回应用固件
///
/// 参考实现: crates/miconlab-core/examples/esp_probe.rs
pub(crate) fn do_esp32_inspect(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }

    log_push(fs, crate::i18n::t("════ One-click inspection (~10s, non-destructive) ════"));

    // 阻止主循环 poll_rx 抢读串口数据（体检期间串口由本函数独占）
    state.flasher_read_busy = true;

    // ---------- Phase A: 复位采集启动日志 ----------
    log_push(fs, crate::i18n::t("-- Phase A: reset & capture boot log (3s) --"));
    state.flush_input();
    let _ = state.set_rts(true); // EN=LOW 复位
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = state.set_rts(false); // EN=HIGH 正常启动
    std::thread::sleep(std::time::Duration::from_millis(100));

    // 采集 3 秒启动日志（复用 read_response 累积）
    let mut boot_log: Vec<u8> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if let Ok(chunk) = state.read_response(100) {
            boot_log.extend_from_slice(&chunk);
        }
    }
    let boot_text = String::from_utf8_lossy(&boot_log).to_string();

    let rom_line = boot_text.lines().find(|l| l.contains("ESP-ROM:")).map(|s| s.trim().to_string());
    let boot_mode = boot_text.lines().find(|l| l.contains("boot:")).map(|s| s.trim().to_string());
    let crash_loop = boot_text.matches("rst:").count() >= 3;
    let guru = boot_text.contains("Guru Meditation");
    let invalid_header = boot_text.contains("invalid header");
    // 二进制帧流检测：固件不打印文本日志时，同步字帧流 = 固件存活信号
    let fw_stream = detect_binary_stream(&boot_log, is_zh);

    if boot_text.trim().is_empty() {
        log_push(fs, crate::i18n::t("   (no UART output)"));
    } else {
        log_push(fs, format!("{} {}B", crate::i18n::t("   boot log"), boot_text.len()));
        if let Some(r) = &rom_line {
            log_push(fs, format!("   · {}", r));
        }
        if let Some(b) = &boot_mode {
            log_push(fs, format!("   · {}", b));
        }
        if let Some(s) = &fw_stream {
            log_push(fs, format!("   · {}：{}", crate::i18n::t("firmware alive"), s));
        }
        if rom_line.is_none() && boot_mode.is_none() && fw_stream.is_none() {
            // 无标准启动行且无帧流：打印原始字节 hex 样本辅助诊断
            // （乱码/重复模式用 UTF-8 有损转换会丢失真实字节，hex 才能还原）
            let n = boot_log.len().min(24);
            log_push(fs, crate::i18n::tf(
                "   · {} (hex first {}B): {}",
                &[("{}", "{}", &format!("{}", crate::i18n::t("sample"))),
                  ("{}", "{}", &format!("{}", n)),
                  ("{}", "{}", &format!("{}", format_hex_spaced(&boot_log[..n])))]
            ));
        }
    }

    // ---------- Phase B: 进 Bootloader 识别 ----------
    log_push(fs, crate::i18n::t("-- Phase B: enter ROM bootloader --"));
    fs.bootloader_connected = false;
    if !reset_esp32_to_bootloader(fs, state, is_zh) {
        log_push(fs, crate::i18n::t("Cannot enter bootloader (no SYNC response) -- Phase A only"));
        state.flasher_read_busy = false;
        report_inspect(fs, is_zh, &rom_line, &boot_mode, None, None, false, None, None, fw_stream.as_deref(), None, None, crash_loop, guru, invalid_header, false, None);
        return;
    }

    // 排水到静默：ROM 对每个收到的 SYNC 包回一帧 OHAI，帧在 USB 管道异步到达，
    // flush_input 只能清"已到达"数据。循环读取直到 250ms 无新数据，
    // 确保后续命令-响应配对干净；顺带捕获 OHAI 帧 value（ROM 标识）
    let rom_tag = drain_until_quiet(fs, state);

    let esp = Esp32Flasher::new();

    // USB VID/PID 旁证识别（Espressif 官方分配：全系 USB-Serial-JTAG 均为 303A:1001）
    let usb_ident: Option<String> = state
        .port_list
        .get(state.selected_port_index)
        .and_then(|p| match (p.vid, p.pid) {
            (Some(0x303A), Some(0x1001)) => Some(crate::i18n::t("Espressif native USB-Serial-JTAG (S3/C3/C5/C6/H2/P4 series)").to_string()),
            (Some(0x303A), Some(0x0002)) => Some(crate::i18n::t("Espressif USB-OTG CDC (ESP32-S2)").to_string()),
            (Some(v), Some(p)) => Some(crate::i18n::tf("USB VID:PID = 0x{:04X}:0x{:04X} (UART bridge)",
                &[("{:04X}", "{:04X}", &format!("{:04X}", v)),
                  ("{:04X}", "{:04X}", &format!("{:04X}", p))])),
            _ => None,
        });

    // SECURITY_INFO (0x14)：chip_id / 加密计数 / 安全标志 / eco 版本
    // 命令号 0x14 为 esptool 官方标准（ESP32-S2 及更新的 ROM 支持）；
    // 历史错误：曾用 0x18（ROM 对未知命令静默丢弃），且响应过滤器也误配 0x18，
    // 导致即使发送 0x14 也永远匹配不到应答。现已全部修正。
    let mut sec: Option<flasher::SecurityInfo> = None;
    log_push(fs, ">> GET_SECURITY_INFO (0x14)");
    state.flush_input();
    if let Err(e) = state.send_raw(&esp.security_info()) {
        log_push(fs, format!("<< Send error: {}", e));
    } else if let Some(body) = read_slip_response_for(fs, state, 1500, Some(0x14)) {
        log_push(fs, format!("   resp: {}", format_hex_spaced(&body)));
        match esp_response_payload(&body) {
            Some((payload, true)) => match Esp32Flasher::parse_security_info(payload) {
                Ok(info) => sec = Some(info),
                Err(e) => log_push(fs, crate::i18n::tf("   ⚠ Failed to parse SECURITY_INFO: {}",
                    &[("{}", "{}", &format!("{}", e))])),
            },
            Some((_, false)) => {
                log_push(fs, crate::i18n::t("   ROM rejected command (SECURITY_INFO unsupported)"));
            }
            None => log_push(fs, crate::i18n::t("   malformed response")),
        }
    }
    if sec.is_none() {
        if let Some(tag) = rom_tag {
            log_push(fs, crate::i18n::tf(
                "   ⚠ No SECURITY_INFO response; ROM tag 0x{:08X} captured from OHAI",
                &[("{:08X}", "{:08X}", &format!("{:08X}", tag))]
            ));
        } else {
            log_push(fs, crate::i18n::t("   SECURITY_INFO no valid response"));
        }
    }

    // 芯片型号识别：SECURITY_INFO chip_id 优先，静默时读魔数寄存器 0x40001000
    let mut chip_name: Option<String> = None;
    let mut chip_via_sec = false;
    if let Some(info) = &sec {
        if let Some(id) = info.chip_id {
            chip_name = Some(Esp32Flasher::chip_name_from_security_id(id).to_string());
            chip_via_sec = true;
        }
    }
    if chip_name.is_none() {
        log_push(fs, crate::i18n::t(">> READ_REG chip magic register (0x40001000)"));
        if let Some(m) = read_reg_value(fs, state, &esp, Esp32Flasher::CHIP_DETECT_MAGIC_REG) {
            let name = Esp32Flasher::chip_name_from_magic(m);
            if name.is_empty() {
                log_push(fs, crate::i18n::tf("   ⚠ Unknown magic 0x{:08X}",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", m))]));
            } else {
                log_push(fs, crate::i18n::tf("   → magic = 0x{:08X} = {}",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", m)),
                      ("{}", "{}", &format!("{}", name))]));
                chip_name = Some(name.to_string());
            }
        } else {
            log_push(fs, crate::i18n::t("   ⚠ Failed to read magic register"));
        }
    }

    // S3 USB-JTAG：禁用 RTC WDT / SWD 看门狗（esptool _post_connect 标准流程）。
    // USB-JTAG 进入下载模式时看门狗不复位，烧录/擦除等长操作期间可能超时复位芯片。
    // 仅在已确认芯片为 S3 时执行（寄存器布局为 S3 专属，误写其他芯片有风险）。
    if chip_name.as_deref() == Some("ESP32-S3") {
        log_push(fs, crate::i18n::t(">> S3: disabling RTC WDT / SWD watchdogs (esptool standard)"));
        for cmd in esp.s3_disable_watchdogs() {
            if state.send_raw(&cmd).is_ok() {
                let _ = read_slip_response_for(fs, state, 300, Some(0x09));
            }
        }
    }

    // SPI_ATTACH (0x0D)：挂载 SPI Flash（后续 MD5 校验的前提）
    let mut md5_boot: Option<[u8; 16]> = None;
    let mut md5_ptable: Option<[u8; 16]> = None;
    let mut spi_ok = false;
    log_push(fs, ">> SPI_ATTACH (0x0D)");
    state.flush_input();
    let attach = esp.spi_attach(0);
    if state.send_raw(&attach).is_ok() {
        match read_slip_response_for(fs, state, 1000, Some(0x0D)) {
            Some(body) => {
                spi_ok = matches!(esp_response_payload(&body), Some((_, true)));
                if spi_ok {
                    log_push(fs, crate::i18n::t("   SPI flash attached"));
                } else {
                    log_push(fs, crate::i18n::tf("   ⚠ SPI_ATTACH failed: {}",
                        &[("{}", "{}", &format!("{}", format_hex_spaced(&body)))]));
                }
            }
            None => log_push(fs, crate::i18n::t("   ⚠ SPI_ATTACH no response")),
        }
    } else {
        log_push(fs, crate::i18n::t("   ⚠ SPI_ATTACH send failed"));
    }

    // MD5：bootloader 区 @0x0 / 分区表区 @0x8000 非空校验
    if spi_ok {
        md5_boot = read_region_md5(fs, state, &esp, 0x0);
        md5_ptable = read_region_md5(fs, state, &esp, 0x8000);
    }

    // MAC 地址（eFuse BLK1）：mac0=read(base), mac1=read(base+4)，
    // MAC = [m1>>8, m1, m0>>24, m0>>16, m0>>8, m0]（esptool esp32s3.py read_mac 官方公式）
    let mut mac_str: Option<String> = None;
    if let Some(name) = &chip_name {
        if let Some(base) = Esp32Flasher::mac_efuse_addr(name) {
            log_push(fs, format!(">> READ_REG MAC eFuse @ 0x{base:08X}"));
            let m0 = read_reg_value(fs, state, &esp, base);
            let m1 = read_reg_value(fs, state, &esp, base + 4);
            if let (Some(m0), Some(m1)) = (m0, m1) {
                let mac = [
                    (m1 >> 8) as u8,
                    m1 as u8,
                    (m0 >> 24) as u8,
                    (m0 >> 16) as u8,
                    (m0 >> 8) as u8,
                    m0 as u8,
                ];
                mac_str = Some(mac.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(":"));
                log_push(fs, format!("   MAC = {}", mac_str.as_deref().unwrap_or_default()));
            } else {
                log_push(fs, crate::i18n::t("   ⚠ Failed to read MAC eFuse"));
            }
        }
    }

    // S3 芯片修订/封装/内置存储：读 eFuse BLK1/BLK2（esptool get_chip_description 公式）。
    // 注意：SECURITY_INFO 的 api_version 不是 eco 修订版本，真实修订必须读 eFuse。
    let mut chip_rev_str: Option<String> = None;
    if chip_name.as_deref() == Some("ESP32-S3") {
        log_push(fs, crate::i18n::t(">> READ_REG S3 revision/package eFuse (BLK1/BLK2)"));
        let w3 = read_reg_value(fs, state, &esp, Esp32Flasher::S3_EFUSE_BLK1_W3);
        let w4 = read_reg_value(fs, state, &esp, Esp32Flasher::S3_EFUSE_BLK1_W4);
        let w5 = read_reg_value(fs, state, &esp, Esp32Flasher::S3_EFUSE_BLK1_W5);
        let b2 = read_reg_value(fs, state, &esp, Esp32Flasher::S3_EFUSE_BLK2_W4);
        if let (Some(w3), Some(w4), Some(w5), Some(b2)) = (w3, w4, w5, b2) {
            let v = Esp32Flasher::parse_s3_chip_version(w3, w4, w5, b2);
            chip_rev_str = Some(format!(
                "v{}.{}{}",
                v.major,
                v.minor,
                if v.eco0_workaround { " (eco0)" } else { "" }
            ));
            log_push(fs, crate::i18n::tf("   → Chip revision = v{}.{}",
                &[("{}", "{}", &format!("{}", v.major)), ("{}", "{}", &format!("{}", v.minor))]));
            log_push(fs, crate::i18n::tf("   → Package = {}",
                &[("{}", "{}", &format!("{}", Esp32Flasher::s3_pkg_name(v.pkg_version)))]));
            match Esp32Flasher::s3_embedded_flash(v.flash_cap) {
                Some(cap) => log_push(fs, crate::i18n::tf(
                    "   → Embedded Flash = {} ({})",
                    &[("{}", "{}", &format!("{}", cap)),
                      ("{}", "{}", &format!("{}", if v.flash_vendor.is_empty() { crate::i18n::t("Unknown vendor") } else { v.flash_vendor }))]
                )),
                None => log_push(fs, crate::i18n::t("   → Flash = external (no embedded)")),
            }
            if let Some(cap) = Esp32Flasher::s3_embedded_psram(v.psram_cap) {
                log_push(fs, crate::i18n::tf(
                    "   → Embedded PSRAM = {} ({})",
                    &[("{}", "{}", &format!("{}", cap)),
                      ("{}", "{}", &format!("{}", if v.psram_vendor.is_empty() { crate::i18n::t("Unknown") } else { v.psram_vendor }))]
                ));
            }
        } else {
            log_push(fs, crate::i18n::t("   ⚠ Failed to read revision/package eFuse"));
        }
    }

    // S3：清除 RTC_CNTL_OPTION1 的 FORCE_DOWNLOAD 位（esptool hard_reset 官方 workaround，
    // 避免 USB-JTAG 复位后芯片卡在下载模式，参见 arduino-esp32#6762）
    if chip_name.as_deref() == Some("ESP32-S3") {
        let _ = state.send_raw(&esp.write_reg(0x6000_812C, 0, 0x1, 0));
        let _ = read_slip_response_for(fs, state, 300, Some(0x09));
    }
    // FLASH_END 结束下载会话，随后 RTS 硬复位回应用固件
    // （FLASH_END 本身不复位芯片，必须显式硬复位才能离开下载模式）
    state.flush_input();
    if state.send_raw(&esp.flash_finish()).is_ok() {
        let _ = read_slip_response_for(fs, state, 500, Some(0x04));
    }
    let _ = state.set_rts(true); // EN=LOW 复位
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = state.set_rts(false); // 释放 → 正常启动
    fs.bootloader_connected = false;
    state.flasher_read_busy = false;

    report_inspect(fs, is_zh, &rom_line, &boot_mode, sec.as_ref(), chip_name.as_deref(), chip_via_sec, chip_rev_str.as_deref(), mac_str.as_deref(), fw_stream.as_deref(), md5_boot, md5_ptable, crash_loop, guru, invalid_header, true, usb_ident);
}

/// 读取指定 Flash 区域的 MD5（32 字节），用于固件区非空校验。
///
/// 命令-响应严格配对：只接受 command=0x13 的应答帧，残留 SYNC 帧自动跳过。
fn read_region_md5(fs: &mut FlasherState, state: &mut AppState, esp: &Esp32Flasher, addr: u32) -> Option<[u8; 16]> {
    state.flush_input();
    let cmd = esp.spi_flash_md5(addr, 32);
    if let Err(e) = state.send_raw(&cmd) {
        log_push(fs, crate::i18n::tf("   ⚠ MD5@0x{:04X} send failed: {}",
            &[("{:04X}", "{:04X}", &format!("{:04X}", addr)), ("{}", "{}", &format!("{}", e))]));
        return None;
    }
    match read_slip_response_for(fs, state, 2000, Some(0x13)) {
        Some(body) => match Esp32Flasher::parse_md5_response(&body) {
            Ok(m) => {
                let hex: String = m.iter().map(|b| format!("{:02x}", b)).collect();
                log_push(fs, format!("   MD5 @ 0x{:04X} = {}", addr, hex));
                Some(m)
            }
            Err(e) => {
                log_push(fs, crate::i18n::tf("   ⚠ MD5@0x{:04X} parse failed: {} (raw: {})",
                    &[("{:04X}", "{:04X}", &format!("{:04X}", addr)),
                      ("{}", "{}", &format!("{}", e)),
                      ("{}", "{}", &format!("{}", format_hex_spaced(&body)))]));
                None
            }
        },
        None => {
            log_push(fs, crate::i18n::tf("   ⚠ MD5@0x{:04X} no response",
                &[("{:04X}", "{:04X}", &format!("{:04X}", addr))]));
            None
        }
    }
}

/// READ_REG (0x0A) 读取 32 位寄存器，返回寄存器值。
///
/// 官方协议：READ_REG 的结果位于响应帧头偏移 4..8 的 Value 字段（非 data 载荷）；
/// data 仅含 4 字节状态（ROM：2 有效 + 2 保留）。
fn read_reg_value(fs: &mut FlasherState, state: &mut AppState, esp: &Esp32Flasher, addr: u32) -> Option<u32> {
    state.flush_input();
    if let Err(e) = state.send_raw(&esp.read_reg(addr)) {
        log_push(fs, crate::i18n::tf("   ⚠ READ_REG@0x{:08X} send failed: {}",
            &[("{:08X}", "{:08X}", &format!("{:08X}", addr)), ("{}", "{}", &format!("{}", e))]));
        return None;
    }
    match read_slip_response_for(fs, state, 800, Some(0x0A)) {
        Some(body) => {
            if body.len() < 8 {
                log_push(fs, crate::i18n::tf("   ⚠ READ_REG@0x{:08X} response too short: {}",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", addr)),
                      ("{}", "{}", &format!("{}", format_hex_spaced(&body)))]));
                return None;
            }
            if matches!(esp_response_payload(&body), Some((_, true))) {
                Some(u32::from_le_bytes([body[4], body[5], body[6], body[7]]))
            } else {
                log_push(fs, crate::i18n::tf("   ⚠ READ_REG@0x{:08X} failed: {}",
                    &[("{:08X}", "{:08X}", &format!("{:08X}", addr)),
                      ("{}", "{}", &format!("{}", format_hex_spaced(&body)))]));
                None
            }
        }
        None => {
            log_push(fs, crate::i18n::tf("   ⚠ READ_REG@0x{:08X} no response",
                &[("{:08X}", "{:08X}", &format!("{:08X}", addr))]));
            None
        }
    }
}

/// 解析 ROM 响应帧（slip 解码后的完整 body）。
///
/// 帧结构：dir(1)=0x01 + cmd(1) + size(2 LE) + value(4) + data(size)。
/// ESP32 ROM 的 status 位于 data 尾部 4 字节（前 2 字节有效：
/// data[size-4]=status, data[size-3]=error，后 2 字节保留）。
///
/// 返回 Some((payload, status_ok))：payload 为剥离 status 后的纯载荷。
fn esp_response_payload(body: &[u8]) -> Option<(&[u8], bool)> {
    if body.len() < 8 || body[0] != 0x01 {
        return None;
    }
    let size = u16::from_le_bytes([body[2], body[3]]) as usize;
    if body.len() < 8 + size {
        return None;
    }
    if size < 4 {
        // 无 status 字段（部分 ROM 响应），视为成功
        return Some((&body[8..8 + size], true));
    }
    let status = body[8 + size - 4];
    let error = body[8 + size - 3];
    Some((&body[8..8 + size - 4], status == 0 && error == 0))
}

/// SYNC 成功后排水：读走全部 OHAI 残留帧直到链路静默。
///
/// 根因：ROM 对每个收到的 SYNC 包回一帧 OHAI，帧在 USB 管道异步到达；
/// flush_input 只能清"已到达"的数据，清不掉传输中的帧。
/// 循环读取（50ms 粒度）直到 250ms 无新数据或总时长 1.5s 封顶。
///
/// 返回值：首帧 OHAI 的 value 字段（ROM 标识，如 0x20120707），
/// 供 SECURITY_INFO 静默时的降级识别展示。
fn drain_until_quiet(fs: &mut FlasherState, state: &mut AppState) -> Option<u32> {
    let t0 = std::time::Instant::now();
    let mut quiet_ms: u64 = 0;
    let mut rom_tag: Option<u32> = None;
    let mut acc: Vec<u8> = Vec::new();
    let mut frames_seen: usize = 0;
    while t0.elapsed() < std::time::Duration::from_millis(1500) {
        match state.read_response(50) {
            Ok(chunk) if !chunk.is_empty() => {
                quiet_ms = 0;
                acc.extend_from_slice(&chunk);
                while let Some((body, consumed)) = slip_decode_frame(&acc) {
                    frames_seen += 1;
                    if rom_tag.is_none()
                        && body.len() >= 8
                        && body[0] == 0x01
                        && body[1] == 0x08
                    {
                        rom_tag = Some(u32::from_le_bytes([
                            body[4], body[5], body[6], body[7],
                        ]));
                    }
                    acc.drain(..consumed);
                }
            }
            _ => {
                quiet_ms += 50;
                if quiet_ms >= 250 {
                    break; // 链路静默，排水完成
                }
            }
        }
    }
    if frames_seen > 0 {
        log_push(fs, crate::i18n::tf(
            "   (drained {} leftover OHAI frames, link now quiet)",
            &[("{}", "{}", &format!("{}", frames_seen))]
        ));
    }
    rom_tag
}

/// 读取一帧 SLIP 响应（命令-响应严格配对版）。
///
/// 返回解码后的完整包体：dir(1) + cmd(1) + size(2) + value(4) + data...，
/// data 尾部 2 字节为 status（esptool 协议：status 内嵌在 data 末尾）。
///
/// 根因修复：ESP32-S3 USB-JTAG 模式下 ROM 收到 SYNC 后连发多帧 OHAI 响应，
/// 帧在 USB 管道中异步到达，flush_input 无法清除"还在路上"的数据，
/// 导致后续命令（SECURITY_INFO/SPI_ATTACH/MD5）全部配对到旧 SYNC 帧
/// （表现为"无有效响应"）。此处逐帧校验 command 字节，
/// 不匹配的残留帧直接丢弃，直到命中期望命令的响应或超时。
fn read_slip_response_for(
    fs: &mut FlasherState,
    state: &mut AppState,
    timeout_ms: u64,
    expected_cmd: Option<u8>,
) -> Option<Vec<u8>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut acc: Vec<u8> = Vec::new();
    // 诊断信息：被丢弃帧的命令号列表 + 首帧 hex（用于定位"8 帧残留"真身）
    let mut dropped_cmds: Vec<u8> = Vec::new();
    let mut first_dropped_hex: Option<String> = None;
    while std::time::Instant::now() < deadline {
        if let Ok(chunk) = state.read_response(50) {
            acc.extend_from_slice(&chunk);
        }
        // 从缓冲头部逐帧取出校验（slip_decode_frame 会跳过帧前垃圾字节）
        while let Some((body, consumed)) = slip_decode_frame(&acc) {
            let matched = match expected_cmd {
                Some(cmd) => body.len() >= 2 && body[0] == 0x01 && body[1] == cmd,
                None => true,
            };
            if matched {
                if !dropped_cmds.is_empty() {
                    log_push(fs, crate::i18n::tf(
                        "   (skipped {} stale response frames [{}], matched reply for command 0x{:02X})",
                        &[("{}", "{}", &format!("{}", dropped_cmds.len())),
                          ("{}", "{}", &format!("{}", summarize_dropped_cmds(&dropped_cmds))),
                          ("{:02X}", "{:02X}", &format!("{:02X}", expected_cmd.unwrap_or(0)))]
                    ));
                }
                return Some(body);
            }
            if first_dropped_hex.is_none() {
                let n = body.len().min(24);
                first_dropped_hex = Some(format_hex_spaced(&body[..n]));
            }
            dropped_cmds.push(if body.len() >= 2 { body[1] } else { 0xFF });
            acc.drain(..consumed);
        }
    }
    if !dropped_cmds.is_empty() {
        let mut msg = crate::i18n::tf(
            "   (timed out after skipping {} stale response frames [{}], no reply for command 0x{:02X}",
            &[("{}", "{}", &format!("{}", dropped_cmds.len())),
              ("{}", "{}", &format!("{}", summarize_dropped_cmds(&dropped_cmds))),
              ("{:02X}", "{:02X}", &format!("{:02X}", expected_cmd.unwrap_or(0)))]
        );
        if let Some(h) = first_dropped_hex {
            msg.push_str(&crate::i18n::tf("; first frame: {}",
                &[("{}", "{}", &format!("{}", h))]));
        }
        log_push(fs, msg);
    }
    None
}

/// 被丢弃帧的命令号直方图摘要，如 "08×6, 18×2"。
fn summarize_dropped_cmds(cmds: &[u8]) -> String {
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let c = sorted[i];
        let mut n = 1;
        while i + n < sorted.len() && sorted[i + n] == c {
            n += 1;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&format!("{:02X}×{}", c, n));
        i += n;
    }
    out
}

/// 检测应用固件的二进制帧流（同步字重复出现 = 固件在持续输出结构化数据）。
///
/// 实测样例（ESP32-S3，2026-08-23）：应用固件以 ~102 帧/s 持续输出 28B 心跳帧
/// `A5 5A 01 1A 00 01 … 83 CA`。这类固件不打印文本日志，若不识别会被误判为
/// "无 UART 输出"或乱码，实际是固件存活的强信号。
fn detect_binary_stream(data: &[u8], _is_zh: bool) -> Option<String> {
    const SYNC_WORDS: [[u8; 2]; 4] = [[0xA5, 0x5A], [0x5A, 0xA5], [0xAA, 0x55], [0x55, 0xAA]];
    if data.len() < 12 {
        return None;
    }
    for sw in SYNC_WORDS {
        let positions: Vec<usize> = data
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] == sw[0] && w[1] == sw[1])
            .map(|(i, _)| i)
            .take(1000)
            .collect();
        if positions.len() < 3 {
            continue;
        }
        // 帧长 = 相邻同步字间距的中位数（过滤 <4B 的误命中）
        let mut lens: Vec<usize> = positions
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|&d| d >= 4)
            .collect();
        if lens.is_empty() {
            continue;
        }
        lens.sort_unstable();
        let median = lens[lens.len() / 2];
        let fps = positions.len() as f64 / 3.0; // Phase A 固定采集 3s
        let name = format!("{:02X} {:02X}", sw[0], sw[1]);
        return Some(crate::i18n::tf("binary stream sync {} x{} (~{}B/frame, ~{:.0} fps)", &[("{}", "{}", &format!("{}", name)), ("{}", "{}", &format!("{}", positions.len())), ("{}", "{}", &format!("{}", median)), ("{:.0}", "{:.0}", &format!("{:.0}", fps))]));
    }
    None
}

/// 输出体检报告（汇总硬件识别 + 固件状态）
#[allow(clippy::too_many_arguments)]
fn report_inspect(
    fs: &mut FlasherState,
    _is_zh: bool,
    rom_line: &Option<String>,
    boot_mode: &Option<String>,
    sec: Option<&flasher::SecurityInfo>,
    chip_name: Option<&str>,
    chip_via_sec: bool,
    chip_rev: Option<&str>,
    mac: Option<&str>,
    fw_stream: Option<&str>,
    md5_boot: Option<[u8; 16]>,
    md5_ptable: Option<[u8; 16]>,
    crash_loop: bool,
    guru: bool,
    invalid_header: bool,
    bootloader_ok: bool,
    usb_ident: Option<String>,
) {
    log_push(fs, crate::i18n::t("---- Report ----"));

    // 在线状态（Phase B 是否成功进入 ROM Bootloader）
    if bootloader_ok {
        log_push(fs, crate::i18n::t("ROM bootloader online (SLIP sync OK)"));
    }
    if let Some(u) = &usb_ident {
        log_push(fs, format!("✅ {}: {}", crate::i18n::t("USB"), u));
    }
    if chip_name.is_none() && bootloader_ok && usb_ident.is_none() && rom_line.is_none() {
        log_push(fs, crate::i18n::t("Bootloader online only -- chip model not identified"));
    }

    // 硬件识别
    if let Some(name) = chip_name {
        let via = if chip_via_sec { "SECURITY_INFO" } else { crate::i18n::t("magic reg") };
        log_push(fs, crate::i18n::tf("✅ {} = {} ({})",
            &[("{}", "{}", &format!("{}", crate::i18n::t("Chip"))),
              ("{}", "{}", &format!("{}", name)),
              ("{}", "{}", &format!("{}", via))]));
    }
    if let Some(rev) = chip_rev {
        log_push(fs, format!("✅ {} = {}", crate::i18n::t("Chip rev"), rev));
    }
    if let Some(m) = mac {
        log_push(fs, format!("✅ {} = {}", crate::i18n::t("MAC"), m));
    }
    if let Some(r) = rom_line {
        log_push(fs, format!("✅ {} = {}", crate::i18n::t("ROM"), r));
    }
    if let Some(f) = sec.map(|s| s.flags) {
        let secure_boot = f & 0x2 != 0;
        let flash_enc = f & 0x4 != 0;
        log_push(fs, format!(
            "{} {}: SecureBoot={} FlashCrypt={} (flags=0x{:08X})",
            if flash_enc || secure_boot { "🔒" } else { "🔓" },
            crate::i18n::t("Security"),
            if secure_boot { "ON" } else { "OFF" },
            if flash_enc { "ON" } else { "OFF" },
            f
        ));
    }
    if let Some(c) = sec.map(|s| s.flash_crypt_cnt) {
        log_push(fs, crate::i18n::tf("   Flash encryption count = {} ({})",
            &[("{}", "{}", &format!("{}", c)),
              ("{}", "{}", &format!("{}", if c % 2 == 1 { crate::i18n::t("odd→encrypted") } else { crate::i18n::t("even→disabled") }))]));
    }
    if let Some(b) = boot_mode {
        log_push(fs, crate::i18n::tf("   Boot mode: {}",
            &[("{}", "{}", &format!("{}", b))]));
    }

    // 固件状态
    let mut firmware_ok = true;
    if guru {
        firmware_ok = false;
        log_push(fs, crate::i18n::t("Guru Meditation -- firmware crashed"));
    }
    if crash_loop {
        firmware_ok = false;
        log_push(fs, crate::i18n::t("Reset loop (>=3 resets)"));
    }
    if invalid_header {
        firmware_ok = false;
        log_push(fs, crate::i18n::t("invalid header -- no valid app"));
    }
    if let (Some(a), Some(b)) = (&md5_boot, &md5_ptable) {
        if a == b {
            firmware_ok = false;
            log_push(fs, crate::i18n::t("MD5 of both regions identical -- blank flash (no firmware)"));
        } else {
            log_push(fs, format!("✅ {} bootloader@0x0: {}", { crate::i18n::t("Flash") }, a.iter().map(|b| format!("{:02x}", b)).collect::<String>()));
            log_push(fs, format!("✅ {} partition@0x8000: {}", { crate::i18n::t("Flash") }, b.iter().map(|b| format!("{:02x}", b)).collect::<String>()));
            log_push(fs, crate::i18n::t("Flash contains firmware (regions differ)"));
        }
    }
    if firmware_ok && !guru && !crash_loop && !invalid_header {
        if let Some(s) = fw_stream {
            log_push(fs, crate::i18n::tf("✅ {} ({})",
                &[("{}", "{}", &format!("{}", crate::i18n::t("Firmware alive"))),
                  ("{}", "{}", &format!("{}", s))]));
        } else if rom_line.is_some() {
            log_push(fs, crate::i18n::t("Boot flow normal (no crash logs)"));
        }
    }
    log_push(fs, crate::i18n::t("==== Done ===="));
}

/// ESP32 结束烧录
pub(crate) fn do_esp32_finish(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !check_connected(state, _is_zh) {
        return;
    }

    let esp = Esp32Flasher::new();
    let cmd = esp.flash_finish();
    log_push(fs, ">> ESP32 Flash Finish");

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

/// ESP32 读取 Flash（备份）
///
/// ESP32 ROM 协议读取流程（esptool.py 实现）：
///   1. ESP_FLASH_BEGIN (0x02)：声明 read_size + num_blocks，告知 ROM 即将读取
///   2. ESP_FLASH_READ  (0x07)：每块发送 (address, size) + padding(8 字节)
///      ROM 返回 SLIP 编码的 (header + payload)，payload 为该块数据
///   3. 拼接所有块到 `last_read_data`
///
/// 注意：本实现为简化版本，假设 ROM 已在同步状态。
pub(crate) fn do_esp32_read(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    if !check_connected(state, is_zh) {
        return;
    }
    // 必须先同步进入 Bootloader 模式，否则读到的是应用固件串口输出而非 Flash 数据
    if !fs.bootloader_connected {
        log_push(fs, crate::i18n::t("<< Not in bootloader mode, trying esptool.py fallback..."));
        read_flash_via_esptool(fs, state, is_zh);
        return;
    }
    fs.read_chip = ChipType::Esp32;

    // 启动非阻塞读取状态机，避免 GUI 卡死
    let total = fs.read_len.max(1) as usize;
    fs.last_read_data.clear();
    fs.read_running = true;
    fs.read_addr = fs.address;
    fs.read_remaining = total;
    fs.read_total = total;
    fs.read_phase = 0; // 0 = 发送 flash_begin
    fs.read_raw_mode = false;
    fs.read_resp_buf.clear();
    fs.read_wait_until = None;
    fs.progress = 0.0;
    state.flasher_read_busy = true;
    let total_blocks = (total + 4095) / 4096;
    log_push(fs, crate::i18n::tf(
        ">> ESP32 Read Flash @ 0x{:08X} total={} ({} blocks, non-blocking)",
        &[("{:08X}", "{:08X}", &format!("{:08X}", fs.address)),
          ("{}", "{}", &format!("{}", total)),
          ("{}", "{}", &format!("{}", total_blocks))]
    ));
}

/// 使用 esptool.py 作为后备读取 ESP32 Flash（非阻塞，后台线程执行）
///
/// 当 DTR/RTS 自动复位失败（应用固件接管 USB-CDC）时，esptool 有更底层的
/// USB reset 能力。流程：断开串口 → 后台调用 esptool read_flash → 重新连接
/// （read_flash_via_esptool 实现位于 ops_esptool.rs）

pub(crate) fn slip_decode_frame(buf: &[u8]) -> Option<(Vec<u8>, usize)> {
    // 找第一个 0xC0（帧起始）
    let start = buf.iter().position(|&b| b == 0xC0)?;
    // 找第二个 0xC0（帧结束）
    let end = buf[start + 1..].iter().position(|&b| b == 0xC0)?;
    let raw_body = &buf[start + 1..start + 1 + end];

    // 解码转义序列
    let mut decoded = Vec::with_capacity(raw_body.len());
    let mut i = 0;
    while i < raw_body.len() {
        match raw_body[i] {
            0xDB if i + 1 < raw_body.len() => {
                match raw_body[i + 1] {
                    0xDC => decoded.push(0xC0),
                    0xDD => decoded.push(0xDB),
                    _ => decoded.push(raw_body[i]), // 无效转义，保留
                }
                i += 2;
            }
            b => {
                decoded.push(b);
                i += 1;
            }
        }
    }

    let consumed = start + 1 + end + 1; // 起始 0xC0 + body + 结束 0xC0
    Some((decoded, consumed))
}

/// ESP32 非阻塞读取状态机：每帧推进一步
///
/// 阶段：
/// - 0: 发送 flash_begin → 1
/// - 1: 等待 flash_begin 响应 → 2
/// - 2: 发送 read_flash（当前块）→ 3
/// - 3: 累积原始数据，解码 SLIP 帧提取 payload → 2（下一块）或完成
pub(crate) fn update_esp32_read_step(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    const BLOCK_SIZE: usize = 4096;
    let esp = Esp32Flasher::new();
    let total_blocks = (fs.read_total + BLOCK_SIZE - 1) / BLOCK_SIZE;

    match fs.read_phase {
        // === 发送 flash_begin ===
        0 => {
            state.flush_input();
            let total = fs.read_total as u32;
            let total_blocks_u32 = ((total as u32 + BLOCK_SIZE as u32 - 1) / BLOCK_SIZE as u32) as u32;
            let begin_cmd = esp.read_flash_begin(total, total_blocks_u32);
            if let Err(e) = state.send_raw(&begin_cmd) {
                log_push(fs, format!("<< Flash Begin send error: {}", e));
                state.last_error = Some(e);
                fs.read_running = false;
                return;
            }
            fs.read_phase = 1;
            fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
        }
        // === 等待 flash_begin 响应 ===
        1 => {
            match state.read_response(50) {
                Ok(resp) => {
                    log_push(fs, format!("<< Flash Begin resp: {}", format_hex_spaced(&resp)));
                    fs.read_phase = 2; // 进入逐块读取
                }
                Err(_) => {
                    if fs.read_wait_until.map_or(true, |t| std::time::Instant::now() >= t) {
                        log_push(fs, "<< Flash Begin timeout");
                        state.last_error = Some("Flash Begin timeout".to_string());
                        fs.read_running = false;
                    }
                }
            }
        }
        // === 发送 read_flash（当前块）===
        2 => {
            let chunk = fs.read_remaining.min(BLOCK_SIZE) as u32;
            let cmd = esp.read_flash(fs.read_addr, chunk);
            if let Err(e) = state.send_raw(&cmd) {
                log_push(fs, format!("<< Read send error: {}", e));
                state.last_error = Some(e);
                fs.read_running = false;
                return;
            }
            fs.read_resp_buf.clear();
            fs.read_phase = 3;
            fs.read_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(3000));
        }
        // === 累积 read_flash 响应（SLIP 解码）===
        3 => {
            let chunk = fs.read_remaining.min(BLOCK_SIZE);
            let block_idx = (fs.read_total - fs.read_remaining) / BLOCK_SIZE;
            let hard_timeout = fs.read_wait_until.map_or(false, |t| std::time::Instant::now() >= t);

            // 读取原始数据（50ms 短超时，不阻塞 UI）
            if let Ok(resp) = state.read_response(50) {
                fs.read_resp_buf.extend_from_slice(&resp);
            }

            // 尝试从缓冲区解码一个完整 SLIP 帧
            if let Some((body, consumed)) = slip_decode_frame(&fs.read_resp_buf) {
                // 移除已消费的原始字节
                fs.read_resp_buf.drain(..consumed);

                // 解析 body: direction(1) + command(1) + size(4 LE) + checksum(4 LE) + payload
                if body.len() >= 10 {
                    let direction = body[0];
                    let payload = &body[10..];
                    let take = payload.len().min(chunk);
                    fs.last_read_data.extend_from_slice(&payload[..take]);
                    log_push(fs, format!(
                        "   block[{}] @ 0x{:08X} +{} (total {}, dir=0x{:02X})",
                        block_idx, fs.read_addr, take, fs.last_read_data.len(), direction
                    ));
                    fs.response = format_hex_spaced(&body);
                } else if !body.is_empty() {
                    log_push(fs, format!(
                        "   block[{}] short frame: {}",
                        block_idx, format_hex_spaced(&body)
                    ));
                }

                // 推进地址和进度
                fs.read_addr = fs.read_addr.wrapping_add(chunk as u32);
                fs.read_remaining -= chunk;
                fs.progress = 0.1 + 0.85 * ((block_idx + 1) as f32 / total_blocks as f32);

                if fs.read_remaining == 0 {
                    fs.read_running = false;
                    fs.progress = 1.0;
                    log_push(fs, crate::i18n::tf(
                        "<< Read complete: {} bytes (use 'Save Backup' to save)",
                        &[("{}", "{}", &format!("{}", fs.last_read_data.len()))]
                    ));
                    state.last_error = None;
                } else {
                    fs.read_phase = 2; // 下一块
                }
            } else {
                // 未找到完整 SLIP 帧
                // 缓冲区过大（> 64KB 无帧）或硬超时 → 报错退出
                if fs.read_resp_buf.len() > 65536 {
                    log_push(fs, crate::i18n::tf(
                        "<< block[{}] buffer overflow ({}B) with no SLIP frame, ESP32 may not be in Bootloader",
                        &[("{}", "{}", &format!("{}", block_idx)),
                          ("{}", "{}", &format!("{}", fs.read_resp_buf.len()))]
                    ));
                    state.last_error = Some("No SLIP frame: ESP32 not in bootloader mode".to_string());
                    fs.read_running = false;
                } else if hard_timeout {
                    log_push(fs, crate::i18n::tf(
                        "<< block[{}] read timed out ({}B, no complete frame)",
                        &[("{}", "{}", &format!("{}", block_idx)),
                          ("{}", "{}", &format!("{}", fs.read_resp_buf.len()))]
                    ));
                    state.last_error = Some("Read flash timeout".to_string());
                    fs.read_running = false;
                }
                // else: 继续等待数据
            }
        }
        _ => {
            fs.read_running = false;
        }
    }
}

// =====================================================================

