//! flasher 子模块：SPI Flash 协议操作（通过 CH341A/FT232H USB 编程器直连）。
//!
//! 仅包含协议操作函数（连接/读/写/擦除/保存备份），
//! UI 渲染逻辑 `render_spi_flash_ops` 位于 ui_ops.rs。

use super::*;
// =====================================================================
// SPI Flash 操作（通过 CH341A/FT232H USB 编程器直连）
// =====================================================================

/// 打开 USB 编程器并读取 JEDEC ID 识别芯片
pub(crate) fn do_spi_flash_connect(fs: &mut FlasherState, _is_zh: bool) {
    let kind = match fs.spi_bridge_kind.as_str() {
        "FT232H" => miconlab_core::usb_bridge::BridgeKind::Ft232H,
        _ => miconlab_core::usb_bridge::BridgeKind::Ch341A,
    };

    log_push(fs, format!("{}: {}",
        crate::i18n::t("Opening USB programmer"),
        kind.name()));

    match miconlab_core::usb_bridge::open_bridge(kind) {
        Ok(bridge) => {
            let mut driver = miconlab_core::spi_flash::SpiFlashDriver::new(bridge);
            // 使用综合识别：JEDEC ID → SFDP → RES 后备链
            match driver.identify_chip() {
                Ok((chip, method)) => {
                    let capacity_mb = (chip.capacity_mbit as f64) / 8.0;
                    fs.spi_jedec_info = format!(
                        "{} ({}) | JEDEC: 0x{:02X}{:04X} | {} Mbit ({:.1} MB) | {}",
                        chip.part_number,
                        chip.manufacturer,
                        (chip.jedec_id >> 16) & 0xFF,
                        chip.jedec_id & 0xFFFF,
                        chip.capacity_mbit,
                        capacity_mb,
                        crate::i18n::t_owned(chip.voltage.name()),
                    );
                    fs.spi_flash_capacity = (chip.capacity_mbit as u64) * 1024 * 1024 / 8;
                    log_push(fs, format!("{}: {} ({}, {} Mbit, {}) [{}]",
                        crate::i18n::t("Identified"),
                        chip.part_number,
                        chip.manufacturer,
                        chip.capacity_mbit,
                        crate::i18n::t_owned(chip.voltage.name()),
                        method));
                    if chip.part_number == miconlab_core::spi_flash::UNKNOWN_PART_NUMBER {
                        log_push(fs, crate::i18n::t("Tip: chip not in database, but read/write still works (verify capacity manually)"));
                    }
                    // 1.8V 芯片电压警告（CH341A/FT232H 默认 3.3V，需要电平转换）
                    if chip.voltage == miconlab_core::bus_probe::SpiFlashVoltage::V1_8 {
                        log_push(fs, crate::i18n::t("⚠ WARNING: This is a 1.8V low-voltage chip. CH341A/FT232H outputs 3.3V. A level shifter is required! Direct connection may damage the chip"));
                    }
                }
                Err(e) => {
                    let msg = format!("{}: {:?}", crate::i18n::t("Chip identify failed"), e);
                    log_push(fs, msg);
                    fs.spi_jedec_info.clear();
                }
            }
        }
        Err(e) => {
            let msg = format!("{}: {:?}",
                crate::i18n::t("Open USB device failed"),
                e);
            log_push(fs, msg);
            fs.spi_jedec_info.clear();
        }
    }
}

/// 读取 SPI Flash 数据
pub(crate) fn do_spi_flash_read(fs: &mut FlasherState, _is_zh: bool) {
    fs.read_chip = ChipType::SpiFlash;
    let kind = match fs.spi_bridge_kind.as_str() {
        "FT232H" => miconlab_core::usb_bridge::BridgeKind::Ft232H,
        _ => miconlab_core::usb_bridge::BridgeKind::Ch341A,
    };
    let len = (fs.spi_flash_len_kb as usize) * 1024;
    let addr = fs.spi_flash_addr;

    log_push(fs, format!("{}: 0x{:06X}, {} KB",
        crate::i18n::t("Reading"),
        addr,
        fs.spi_flash_len_kb));

    fs.progress = 0.0;
    match miconlab_core::usb_bridge::open_bridge(kind) {
        Ok(bridge) => {
            let mut driver = miconlab_core::spi_flash::SpiFlashDriver::new(bridge);
            // 先唤醒（释放 Power-down）
            let _ = driver.release_power_down();
            std::thread::sleep(std::time::Duration::from_millis(10));

            // 分块读取并更新进度
            const CHUNK_KB: usize = 4;
            let chunk_size = CHUNK_KB * 1024;
            let mut data = Vec::with_capacity(len);
            let mut read = 0usize;
            let mut error = None;
            while read < len {
                let this_len = (len - read).min(chunk_size);
                match driver.read_flash(addr + read as u32, this_len) {
                    Ok(chunk) => {
                        data.extend_from_slice(&chunk);
                        read += this_len;
                        fs.progress = read as f32 / len as f32;
                    }
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            match error {
                None => {
                    fs.spi_flash_data = data;
                    fs.progress = 1.0;
                    log_push(fs, crate::i18n::tf("{}: {} bytes",
                        &[("{}", "{}", &format!("{}", crate::i18n::t("Read OK"))),
                          ("{}", "{}", &format!("{}", fs.spi_flash_data.len()))]));
                }
                Some(e) => {
                    fs.spi_flash_data = data;
                    let msg = format!("{}: {:?}", crate::i18n::t("Read failed"), e);
                    log_push(fs, msg);
                }
            }
        }
        Err(e) => {
            let msg = format!("{}: {:?}",
                crate::i18n::t("Open USB failed"),
                e);
            log_push(fs, msg);
        }
    }
}

/// 写入固件到 SPI Flash
pub(crate) fn do_spi_flash_write(fs: &mut FlasherState, _is_zh: bool) {
    if fs.spi_flash_image_path.is_empty() {
        log_push(fs, crate::i18n::t("Please select firmware file"));
        return;
    }
    let kind = match fs.spi_bridge_kind.as_str() {
        "FT232H" => miconlab_core::usb_bridge::BridgeKind::Ft232H,
        _ => miconlab_core::usb_bridge::BridgeKind::Ch341A,
    };

    // 读取固件文件
    let firmware = match std::fs::read(&fs.spi_flash_image_path) {
        Ok(d) => d,
        Err(e) => {
            log_push(fs, format!("{}: {}", crate::i18n::t("Read file failed"), e));
            return;
        }
    };
    let total = firmware.len();
    log_push(fs, crate::i18n::tf("{}: {} ({} bytes)",
        &[("{}", "{}", &format!("{}", crate::i18n::t("Writing"))),
          ("{}", "{}", &format!("{}", fs.spi_flash_image_path)),
          ("{}", "{}", &format!("{}", total))]));

    fs.progress = 0.0;
    match miconlab_core::usb_bridge::open_bridge(kind) {
        Ok(bridge) => {
            let mut driver = miconlab_core::spi_flash::SpiFlashDriver::new(bridge);
            let _ = driver.release_power_down();
            std::thread::sleep(std::time::Duration::from_millis(10));

            // 先擦除覆盖范围
            let end_addr = fs.spi_flash_addr + total as u32 - 1;
            log_push(fs, crate::i18n::t("Erasing..."));
            if let Err(e) = driver.erase_range(fs.spi_flash_addr, end_addr) {
                log_push(fs, format!("{}: {:?}", crate::i18n::t("Erase failed"), e));
                return;
            }
            log_push(fs, crate::i18n::t("Erase done, programming..."));

            // 分块写入并更新进度
            const CHUNK_KB: usize = 4;
            let chunk_size = CHUNK_KB * 1024;
            let mut written = 0usize;
            let mut error = None;
            while written < total {
                let this_len = (total - written).min(chunk_size);
                if let Err(e) = driver.page_program(fs.spi_flash_addr + written as u32, &firmware[written..written + this_len]) {
                    error = Some(e);
                    break;
                }
                written += this_len;
                fs.progress = written as f32 / total as f32;
            }
            match error {
                None => {
                    fs.progress = 1.0;
                    log_push(fs, crate::i18n::tf("{}: {} bytes",
                        &[("{}", "{}", &format!("{}", crate::i18n::t("Write OK"))),
                          ("{}", "{}", &format!("{}", written))]));
                }
                Some(e) => {
                    let msg = crate::i18n::tf("{}: {:?} (failed after writing {} bytes)",
                        &[("{}", "{}", &format!("{}", crate::i18n::t("Write failed"))),
                          ("{:?}", "{:?}", &format!("{:?}", e)),
                          ("{}", "{}", &format!("{}", written))]);
                    log_push(fs, msg);
                }
            }
        }
        Err(e) => {
            let msg = format!("{}: {:?}",
                crate::i18n::t("Open USB failed"),
                e);
            log_push(fs, msg);
        }
    }
}

/// 整片擦除
pub(crate) fn do_spi_flash_erase(fs: &mut FlasherState, _is_zh: bool) {
    let kind = match fs.spi_bridge_kind.as_str() {
        "FT232H" => miconlab_core::usb_bridge::BridgeKind::Ft232H,
        _ => miconlab_core::usb_bridge::BridgeKind::Ch341A,
    };

    log_push(fs, crate::i18n::t("Chip erasing (may take tens of seconds)..."));
    fs.progress = 0.0;

    match miconlab_core::usb_bridge::open_bridge(kind) {
        Ok(bridge) => {
            let mut driver = miconlab_core::spi_flash::SpiFlashDriver::new(bridge);
            let _ = driver.release_power_down();
            std::thread::sleep(std::time::Duration::from_millis(10));

            match driver.chip_erase() {
                Ok(()) => {
                    fs.progress = 1.0;
                    log_push(fs, crate::i18n::t("Chip erase done"));
                }
                Err(e) => {
                    let msg = format!("{}: {:?}", crate::i18n::t("Erase failed"), e);
                    log_push(fs, msg);
                }
            }
        }
        Err(e) => {
            let msg = format!("{}: {:?}",
                crate::i18n::t("Open USB failed"),
                e);
            log_push(fs, msg);
        }
    }
}

/// 保存已读取的 SPI Flash 数据到备份文件
pub(crate) fn do_spi_flash_save_backup(fs: &mut FlasherState, _is_zh: bool) {
    if fs.spi_flash_data.is_empty() {
        log_push(fs, crate::i18n::t("No data to save, click Read Flash first"));
        return;
    }

    let path = if fs.spi_flash_backup_path.is_empty() {
        let default_name = format!("spi_flash_backup_0x{:06X}_{}.bin",
            fs.spi_flash_addr,
            chrono::Local::now().format("%Y%m%d_%H%M%S"));
        match rfd::FileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Binary", &["bin"])
            .add_filter("Intel HEX", &["hex"])
            .save_file()
        {
            Some(p) => p,
            None => return,
        }
    } else {
        std::path::PathBuf::from(&fs.spi_flash_backup_path)
    };

    let is_hex = path.extension()
        .map(|e| e == "hex" || e == "HEX")
        .unwrap_or(false);

    let result = if is_hex {
        let hex = miconlab_core::protocol::flasher::bin_to_ihex(&fs.spi_flash_data, fs.spi_flash_addr);
        std::fs::write(&path, hex)
    } else {
        std::fs::write(&path, &fs.spi_flash_data)
    };

    match result {
        Ok(()) => {
            fs.spi_flash_backup_path = path.display().to_string();
            log_push(fs, crate::i18n::tf("{}: {} ({} bytes)",
                &[("{}", "{}", &format!("{}", crate::i18n::t("Backup saved"))),
                  ("{}", "{}", &format!("{}", path.display())),
                  ("{}", "{}", &format!("{}", fs.spi_flash_data.len()))]));
        }
        Err(e) => {
            log_push(fs, format!("{}: {}", crate::i18n::t("Save failed"), e));
        }
    }
}

// =====================================================================

