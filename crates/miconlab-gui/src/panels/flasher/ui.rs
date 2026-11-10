//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
pub(crate) fn render_common_config(ctx: &egui::Context, ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Common Config"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("flasher_common_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            // 芯片类型
            ui.label(
                RichText::new(crate::i18n::t("Chip Type"))
                    .color(theme::TEXT_SECONDARY),
            );
            egui::ComboBox::from_id_salt("flasher_chip_select")
                .selected_text(fs.chip_type.label(is_zh))
                .width(220.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut fs.chip_type, ChipType::Stm32, "STM32")
                        .on_hover_text(crate::i18n::t("STM32 UART Bootloader (AN3155), 0x7F sync, supports read/write/erase/go"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Esp32, "ESP32")
                        .on_hover_text(crate::i18n::t("ESP32 ROM Bootloader, SLIP encoding + CRC-32C, supports Flash R/W"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Esp8266, "ESP8266")
                        .on_hover_text(crate::i18n::t("ESP8266 ROM Bootloader, SLIP encoding, GPIO0 low to enter download mode"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Arduino, "Arduino (AVR/STK500)")
                        .on_hover_text(crate::i18n::t("ATmega/ATtiny STK500v1 protocol, page-based R/W, DTR triggers reset"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Stm8, "STM8")
                        .on_hover_text(crate::i18n::t("STM8 SWIM protocol, single-wire debug interface"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::NxpLpc, "NXP LPC (ISP)")
                        .on_hover_text(crate::i18n::t("NXP LPC ISP protocol (UM10295), '?' sync, U/J commands"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Gd32, crate::i18n::t("GD32 (STM32-compatible, China)"))
                        .on_hover_text(crate::i18n::t("GD32 STM32-compatible, UART Bootloader"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Ch32, crate::i18n::t("CH32 (WCH, China)"))
                        .on_hover_text(crate::i18n::t("WCH ISP protocol, 0x57 0xAB header, full IDENTIFY packet"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::Pic, "PIC (ICSP)")
                        .on_hover_text(crate::i18n::t("PIC ICSP requires dedicated programmer (PicKit/ICD), panel shows command sequence only"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::SdCard, crate::i18n::t("SBC SD card image"))
                        .on_hover_text(crate::i18n::t("SD card flashing via system diskutil/dd commands, requires sudo"));
                    ui.selectable_value(&mut fs.chip_type, ChipType::SpiFlash, crate::i18n::t("SPI Flash (USB Programmer)"))
                        .on_hover_text(crate::i18n::t("SPI Flash via CH341A/FT232H USB programmer, independent of UART"));
                });
            // 检测芯片类型切换：自动设置读取长度和起始地址，同步波特率
            if fs.chip_type != fs.prev_chip_type {
                fs.read_len = fs.chip_type.default_flash_size();
                fs.address = fs.chip_type.flash_base_addr();
                // 同步当前连接波特率到烧录器
                if state.is_connected() {
                    fs.baud_current = state.config.baud_rate;
                    fs.baud_new = state.config.baud_rate;
                }
                let label = fs.chip_type.default_flash_label(is_zh);
                log_push(fs, crate::i18n::tf(">> Chip switched to {}, auto-set: read {} @ 0x{:08X}", &[("{}", "{}", &format!("{}", fs.chip_type.label(is_zh))), ("{}", "{}", &format!("{}", label)), ("{:08X}", "{:08X}", &format!("{:08X}", fs.address))]));
                fs.prev_chip_type = fs.chip_type;
            }
            ui.end_row();

            // 固件文件路径
            ui.label(
                RichText::new(crate::i18n::t("Firmware File"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut fs.file_path)
                    .desired_width(280.0)
                    .hint_text(crate::i18n::t("/path/to/firmware.hex")),
            )
            .on_hover_text(crate::i18n::t("Supports .bin / .hex / .rom firmware files"));
            ui.end_row();

            // 起始地址
            ui.label(
                RichText::new(crate::i18n::t("Address"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut fs.address)
                        .hexadecimal(8, false, true)
                        .range(0..=0xFFFF_FFFFu32)
                        .speed(1.0),
                )
                .on_hover_text(crate::i18n::t("Flash write start address (HEX, e.g. 0x08000000)"));
                ui.label(
                    RichText::new(crate::i18n::t("(hex)"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            });
            ui.end_row();

            // 读取长度（STM32 读取会自动分块，每块 256 字节）
            ui.label(
                RichText::new(crate::i18n::t("Read Length"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut fs.read_len)
                        .range(1..=0x0040_0000) // 最大 4MB（ESP32 Flash）
                        .speed(1.0),
                )
                .on_hover_text(crate::i18n::t("Bytes to read (auto-set when switching chip type)"));
                // 显示当前芯片默认容量提示
                let default_label = fs.chip_type.default_flash_label(is_zh);
                ui.label(
                    RichText::new(format!("({}: {})", crate::i18n::t("default"), default_label))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
                // 手动重置匹配按钮（用于从旧持久化状态恢复时重新匹配）
                if ui.add(egui::Button::new("↻").min_size(egui::Vec2::new(20.0, 18.0)))
                    .on_hover_text(crate::i18n::t("Re-match chip default size and address"))
                    .clicked()
                {
                    fs.read_len = fs.chip_type.default_flash_size();
                    fs.address = fs.chip_type.flash_base_addr();
                    log_push(fs, crate::i18n::tf(">> Reset: read {} @ 0x{:08X}", &[("{}", "{}", &format!("{}", default_label)), ("{:08X}", "{:08X}", &format!("{:08X}", fs.address))]));
                }
            });
            // Flash 容量快捷按钮
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::t("Quick:")).color(theme::TEXT_DISABLED).small());
                let sizes: &[(u32, &str)] = &[
                    (256, "256B"),
                    (1024, "1KB"),
                    (16384, "16K"),
                    (32768, "32K"),
                    (65536, "64K"),
                    (131072, "128K"),
                    (262144, "256K"),
                    (524288, "512K"),
                ];
                for &(sz, label) in sizes {
                    if ui.add(egui::Button::new(RichText::new(label).small())
                        .min_size(egui::Vec2::new(0.0, 18.0))).clicked() {
                        fs.read_len = sz;
                    }
                }
            });
            ui.end_row();

            // 备份文件路径（用于读取后保存）
            ui.label(
                RichText::new(crate::i18n::t("Backup File"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut fs.backup_path)
                    .desired_width(240.0)
                    .hint_text(crate::i18n::t("Auto-generate if empty")),
            )
            .on_hover_text(crate::i18n::t("File path to save Flash readback (.bin or .hex), auto-generated if empty"));
            if ui.button(crate::i18n::t("Browse..."))
                .on_hover_text(crate::i18n::t("Choose save location"))
                .clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Binary", &["bin"])
                    .add_filter("Intel HEX", &["hex"])
                    .set_file_name("firmware_backup.bin")
                    .save_file()
                {
                    fs.backup_path = path.display().to_string();
                }
            }
            if !fs.backup_path.is_empty() {
                if ui.button(crate::i18n::t("Open"))
                    .on_hover_text(crate::i18n::t("Reveal backup file in file manager"))
                    .clicked() {
                    let p = std::path::Path::new(&fs.backup_path);
                    if p.exists() {
                        miconlab_core::platform::open_path(p.parent().unwrap_or(p));
                    }
                }
                if ui.button(crate::i18n::t("Inspect"))
                    .on_hover_text(crate::i18n::t("Analyze backup firmware: format, size, HEX records, STM32 vector table, strings"))
                    .clicked() {
                    inspect_backup_file(fs, is_zh);
                }
                let edit_clicked = ui.button(crate::i18n::t("✏ Edit"))
                    .on_hover_text(crate::i18n::t("Jump to firmware editor for hex editing, vector table modification, and re-flash save"))
                    .clicked();
                if edit_clicked {
                    if fs.backup_path.trim().is_empty() || !std::path::Path::new(&fs.backup_path).exists() {
                        log_push(fs, crate::i18n::t("<< Save backup first before editing"));
                        state.last_error = Some(crate::i18n::t("Save backup first").to_string());
                    } else {
                        ctx.data_mut(|d| d.insert_temp(
                            egui::Id::new("firmware_editor_file"),
                            fs.backup_path.clone(),
                        ));
                        state.active_panel = PanelKind::FirmwareEditor;
                    }
                }
            }
            ui.end_row();
        });

    // AI 感知：固件智能分析（文件路径非空时分析前 4KB 推断架构/入口地址）
    if !fs.file_path.trim().is_empty() {
        let fw_id = egui::Id::new("flasher_ai_fw_analysis");
        let cached_path = ctx.data(|d| d.get_temp::<String>(egui::Id::new("flasher_ai_fw_path"))).unwrap_or_default();
        if cached_path != fs.file_path {
            // 路径变化，重新分析
            if let Ok(data) = std::fs::read(&fs.file_path) {
                let sample = if data.len() > 4096 { &data[..4096] } else { &data };
                let analysis = miconlab_ai::FirmwareAnalysis::analyze(sample);
                ctx.data_mut(|d| {
                    d.insert_temp(egui::Id::new("flasher_ai_fw_path"), fs.file_path.clone());
                    d.insert_temp(fw_id, analysis);
                });
            }
        }
        if let Some(analysis) = ctx.data(|d| d.get_temp::<miconlab_ai::FirmwareAnalysis>(fw_id)) {
            if analysis.confidence >= 0.3 {
                let summary = crate::panels::ai_sense::firmware_summary(&analysis);
                let hint = crate::panels::ai_sense::AiHint::new("📦", theme::ACCENT, summary);
                crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                ui.add_space(4.0);
            }
        }
    }

    // AI 感知：错误诊断（带上下文：rx 数据/VID:PID）
    if let Some(err) = &state.last_error {
        let rx_data = crate::panels::ai_sense::collect_recent_rx(&state.rx_buffer, 256);
        let (vid_pid, port_name) = state.port_list.get(state.selected_port_index)
            .map(|p| (p.vid.zip(p.pid), Some(p.name.as_str())))
            .unwrap_or((None, None));
        if let Some(hint) = crate::panels::ai_sense::sense_error_full(
            err, state.is_connected(), state.config.baud_rate,
            &rx_data, vid_pid, port_name, is_zh,
        ) {
            crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
            ui.add_space(4.0);
        }
    }
}

/// 渲染统一的操作步骤引导框
/// `title` 为 (中文, 英文) 标题，`steps` 为 (中文, 英文) 步骤文本切片
pub(crate) fn render_step_guide(ui: &mut egui::Ui, title: &'static str, steps: &[&'static str]) {
    egui::Frame::group(ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t(title))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(2.0);
            for (i, step) in steps.iter().enumerate() {
                ui.label(
                    RichText::new(format!("{}. {}", i + 1, crate::i18n::t(step)))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            }
        });
    ui.add_space(6.0);
}

/// STM32 操作步骤引导
pub(crate) fn chip_step_guide_stm32(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (default 115200, some chips support 9600)",
        "Enter Bootloader mode: BOOT0=1, then reset",
        "Click \"Connect (0x7F)\" button, wait for 0x79 (ACK)",
        "Select Intel HEX or BIN firmware file",
        "Click \"Write\" button, wait for writing to complete",
        "Click \"Go\" to start the program",
    ]);
}

/// ESP32 操作步骤引导
pub(crate) fn chip_step_guide_esp32(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (default 115200, up to 921600)",
        "Click \"Sync\" for auto DTR/RTS reset into bootloader (works without buttons)",
        "If auto-reset fails: hold BOOT+RST, release RST then BOOT",
        "Sync success shows \"● Bootloader connected\"",
        "Read chip ID to confirm connection",
        "Select firmware file (.bin supported)",
        "Click \"Write\" button, wait for flash_begin + flash_data to complete",
        "Click \"Reset\" to start the program",
    ]);
}

/// Arduino (STK500) 操作步骤引导
pub(crate) fn chip_step_guide_arduino(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (AVR bootloader typically 57600)",
        "Auto reset (DTR signal) or manual reset",
        "Click \"Sync\" button, wait for STK_INSYNC response",
        "Select HEX firmware file",
        "Click \"Write\" button, page programming (word addressed)",
        "Auto verify after writing completes",
    ]);
}

/// ESP8266 操作步骤引导
pub(crate) fn chip_step_guide_esp8266(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (default 115200)",
        "Pull GPIO0 low then reset to enter download mode",
        "Click \"Sync\" button",
        "Read chip ID",
        "Select firmware file (.bin)",
        "Click \"Write\" button",
        "Reset to run",
    ]);
}

/// STM8 操作步骤引导
pub(crate) fn chip_step_guide_stm8(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (STM8 bootloader typically 38400 or 115200)",
        "Enter Bootloader mode: send 0x7F immediately after reset",
        "Click \"Sync\" button",
        "Select firmware file (.hex)",
        "Click \"Write\" button",
        "Note: avoid writing Option Bytes, may cause lockup",
    ]);
}

/// NXP LPC 操作步骤引导
pub(crate) fn chip_step_guide_lpc(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (default 115200)",
        "Enter ISP mode: pull P0_14 low, then reset",
        "Click \"Sync\" button, send '?' for auto baud detection",
        "Confirm chip ID",
        "Configure sector range (lpc_sector_start/end)",
        "Select firmware file",
        "Click \"Write\" button, check return code",
        "Go to run",
    ]);
}

/// GD32 操作步骤引导（复用 STM32 协议）
pub(crate) fn chip_step_guide_gd32(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (GD32 default 115200, STM32-compatible)",
        "Enter Bootloader mode: BOOT0=1, then reset",
        "Click \"Connect (0x7F)\" button, wait for 0x79 (ACK)",
        "Select Intel HEX or BIN firmware file",
        "Click \"Write\" button, wait for writing to complete",
        "Click \"Go\" to start the program",
    ]);
}

/// CH32 (WCH ISP) 操作步骤引导
pub(crate) fn chip_step_guide_ch32(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Set baud rate (WCH ISP typically 115200)",
        "Enter ISP mode: BOOT0=1, then reset",
        "Click \"Identify\" button, send IDENTIFY packet (0x57 0xAB 0xA1...)",
        "Confirm chip model",
        "Select firmware file (.hex or .bin)",
        "Click \"Write\" button",
        "Click \"Go\" to run",
    ]);
}

/// PIC (ICSP) 操作步骤引导
pub(crate) fn chip_step_guide_pic(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Note: PIC ICSP flashing requires a dedicated programmer (PicKit/ICD)",
        "This panel only shows command sequence, cannot actually flash",
        "Recommended to use MPLAB IPE tool for flashing",
        "View generated command sequence as reference",
    ]);
}

/// SD 卡烧录操作步骤引导
pub(crate) fn chip_step_guide_sd_card(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Note: this operation uses system diskutil/dd commands, requires sudo",
        "⚠️ Must verify disk device name to avoid writing to system disk",
        "Select image file (.img or .iso)",
        "Select target disk device",
        "Click \"Write\" button",
        "Wait for dd command to complete (may be slow)",
    ]);
}

/// SPI Flash (USB 编程器) 操作步骤引导
pub(crate) fn chip_step_guide_spi_flash(ui: &mut egui::Ui, _is_zh: bool) {
    render_step_guide(ui, "Steps", &[
        "Select USB bridge (CH341A or FT232H)",
        "Click \"Scan Device\" to find connected programmer",
        "Click \"Connect & Identify\" to read JEDEC ID",
        "Confirm chip model (e.g. W25Q128)",
        "Set start address and read length",
        "Click \"Read Flash\" or \"Write Firmware\"",
        "Wiring: CS→CS0, SCK→SCK, MISO→MISO, MOSI→MOSI, VCC→3.3V, GND→GND",
        "Large chips (≥32MB) auto-enable 4-byte address mode",
    ]);
}

/// 渲染进度条 + 日志区
pub(crate) fn render_progress_and_log(ui: &mut egui::Ui, fs: &FlasherState, _is_zh: bool) {
    // 进度条
    let progress_label = if fs.progress >= 1.0 {
        crate::i18n::t("Done").to_string()
    } else if fs.progress > 0.0 {
        format!("{:.0}%", fs.progress * 100.0)
    } else { crate::i18n::t("Idle").to_string() };

    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("Progress:"));
        ui.add(egui::ProgressBar::new(fs.progress).text(progress_label));
    });

    ui.add_space(6.0);

    // 日志区（可滚动）+ 一键复制全部日志（方便贴给 AI 助手/issue）
    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("Log:"));
        let can_copy = !fs.log.is_empty();
        let clicked = ui
            .add_enabled(can_copy, egui::Button::new(crate::i18n::t("📋 Copy All Logs")).small())
            .clicked();
        if clicked {
            ui.output_mut(|o| o.copied_text = fs.log.join("\n"));
            ui.colored_label(
                theme::SUCCESS,
                crate::i18n::t("✓ Copied to clipboard"),
            );
        }
    });
    ScrollArea::vertical()
        .max_height(300.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &fs.log {
                ui.label(line);
            }
        });
}


