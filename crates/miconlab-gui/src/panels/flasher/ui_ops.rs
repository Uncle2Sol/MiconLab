//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;

/// 渲染 STM32 操作按钮组
pub(crate) fn render_stm32_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    match fs.chip_type {
        ChipType::Gd32 => chip_step_guide_gd32(ui, is_zh),
        _ => chip_step_guide_stm32(ui, is_zh),
    }
    ui.label(
        RichText::new(crate::i18n::t("STM32 Operations"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    // 页擦除输入
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Erase Pages"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut fs.pages)
                .desired_width(160.0)
                .hint_text("0,1,2,10"),
        )
        .on_hover_text(crate::i18n::t("Comma-separated page list (e.g. 0,1,2,10), HEX format"));
    });
    ui.add_space(4.0);

    // 按钮网格：两行四列
    egui::Grid::new("stm32_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            // 第一行
            if stm32_btn(ui, crate::i18n::t("Connect (0x7F)"))
                .on_hover_text(crate::i18n::t("Send 0x7F sync byte to establish Bootloader connection"))
                .clicked() {
                do_stm32_connect(fs, state, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Mass Erase"))
                .on_hover_text(crate::i18n::t("Send extended erase 0x44 + 0xFF 0xFF (global erase), takes several seconds"))
                .clicked() {
                do_stm32_mass_erase(fs, state, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Page Erase"))
                .on_hover_text(crate::i18n::t("Erase specified pages (0x44 command + page list)"))
                .clicked() {
                do_stm32_page_erase(fs, state, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write firmware page by page (0x31 command), auto-segmented"))
                .clicked() {
                do_stm32_write(fs, state, is_zh);
            }
            ui.end_row();

            // 第二行
            let read_label = if fs.read_running {
                crate::i18n::t("Reading...")
            } else {
                crate::i18n::t("Read")
            };
            if stm32_btn(ui, read_label)
                .on_hover_text(crate::i18n::t("Read data from Flash (0x11 command), non-blocking chunked read"))
                .clicked() {
                if !fs.read_running {
                    do_stm32_read(fs, state, is_zh);
                }
            }
            if stm32_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read data as .bin or .hex file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Go"))
                .on_hover_text(crate::i18n::t("Send GO command (0x21) to jump to start address and run user program"))
                .clicked() {
                do_stm32_go(fs, state, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Read UID"))
                .on_hover_text(crate::i18n::t("Read chip unique ID (96-bit)"))
                .clicked() {
                do_stm32_read_uid(fs, state, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Export Disasm"))
                .on_hover_text(crate::i18n::t("Export full disassembly to text file. Auto-detects architecture: STM32→ARM Thumb, ESP32→Xtensa LX6/LX7 (parses 0xE9 image header + segments + entry disasm)"))
                .clicked() {
                export_disasm_to_file(fs, is_zh);
            }
            if stm32_btn(ui, crate::i18n::t("Readable Report"))
                .on_hover_text(crate::i18n::t("Translate firmware into plain language (Markdown): summary, built-in app info, partitions, segments, frameworks, classified strings, health check"))
                .clicked() {
                export_readable_report(fs, is_zh);
            }
            ui.end_row();
        });
}

/// STM32 操作按钮样式
pub(crate) fn stm32_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 ESP32 操作按钮组
pub(crate) fn render_esp32_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_esp32(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("ESP32 Operations"))
            .color(theme::CYAN)
            .strong(),
    );
    // Bootloader 连接状态指示
    ui.horizontal(|ui| {
        let (status_text, status_color) = if fs.bootloader_connected {
            (
                crate::i18n::t("● Bootloader connected"),
                theme::SUCCESS,
            )
        } else {
            (
                crate::i18n::t("○ Not in bootloader (click Sync or BOOT+RST)"),
                theme::WARNING,
            )
        };
        ui.label(RichText::new(status_text).color(status_color).small());
    });
    ui.add_space(4.0);

    // 波特率输入
    egui::Grid::new("esp32_baud_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("New Baud"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut fs.baud_new)
                    .range(9600..=4_000_000)
                    .speed(100.0),
            );
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Current Baud"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut fs.baud_current)
                    .range(9600..=4_000_000)
                    .speed(100.0),
            );
            ui.end_row();
        });

    ui.add_space(4.0);

    // 按钮网格：三行四列
    egui::Grid::new("esp32_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            // 第一行
            if esp32_btn(ui, crate::i18n::t("Inspect"))
                .on_hover_text(crate::i18n::t("Online identify chip/ROM/security + firmware health check (~10s, non-destructive)"))
                .clicked() {
                do_esp32_inspect(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Sync"))
                .on_hover_text(crate::i18n::t("Send ESP_SYNC (0x08) sync packet, 7 retries to improve success rate"))
                .clicked() {
                do_esp32_sync(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Chip ID"))
                .on_hover_text(crate::i18n::t("Read ESP32 MAC address (ESP_READ_MAC = 0x0B)"))
                .clicked() {
                do_esp32_chip_id(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Change Baud"))
                .on_hover_text(crate::i18n::t("Switch serial baudrate at runtime (ESP_CHANGE_BAUDRATE = 0x0F)"))
                .clicked() {
                do_esp32_change_baud(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("SPI Attach"))
                .on_hover_text(crate::i18n::t("Attach SPI Flash controller (ESP_SPI_ATTACH = 0x0D)"))
                .clicked() {
                do_esp32_spi_attach(fs, state, is_zh);
            }
            ui.end_row();

            // 第二行
            if esp32_btn(ui, crate::i18n::t("Erase"))
                .on_hover_text(crate::i18n::t("Erase Flash region (ESP_ERASE_DATA = 0xD0)"))
                .clicked() {
                do_esp32_erase(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write firmware in blocks (ESP_FLASH_DATA = 0x03), 4KB/block"))
                .clicked() {
                do_esp32_write(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("MD5 Verify"))
                .on_hover_text(crate::i18n::t("Compute MD5 of Flash region (ESP_SPI_FLASH_MD5 = 0x13)"))
                .clicked() {
                do_esp32_md5(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Finish"))
                .on_hover_text(crate::i18n::t("End Flash operations and reset (ESP_FLASH_END = 0x04)"))
                .clicked() {
                do_esp32_finish(fs, state, is_zh);
            }
            ui.end_row();

            // 第三行：读取 Flash（备份） + 保存备份 + 导出反汇编
            if esp32_btn(ui, crate::i18n::t("Read Flash"))
                .on_hover_text(crate::i18n::t("Read Flash data in blocks (ESP_READ_FLASH = 0x0E)"))
                .clicked() {
                do_esp32_read(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read Flash data as .bin file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Export Disasm"))
                .on_hover_text(crate::i18n::t("Export ESP32 firmware disassembly to text file: parses 0xE9 image header + segments + Xtensa LX6/LX7 disasm + call stats. Read Flash first"))
                .clicked() {
                export_disasm_to_file(fs, is_zh);
            }
            if esp32_btn(ui, crate::i18n::t("Readable Report"))
                .on_hover_text(crate::i18n::t("Translate firmware into plain language: chip, app name/version/build time, partition table, segments, framework detection, classified strings, health check. Outputs Markdown"))
                .clicked() {
                export_readable_report(fs, is_zh);
            }
            ui.end_row();
        });
}

/// ESP32 操作按钮样式
pub(crate) fn esp32_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 Arduino (STK500 v1 / Optiboot) 操作按钮组
///
/// 协议流程（标准 Arduino Bootloader）：
///   1. 上电后 1.5s 内发送 GET_SYNC 进入编程模式
///   2. READ_SIGN 读取 3 字节芯片签名（识别 ATmega328P 等）
///   3. LOAD_ADDRESS + PROG_PAGE 分页写入 Flash
///   4. LOAD_ADDRESS + READ_PAGE 分页读取 Flash（备份）
///   5. LEAVE_PROGMODE 退出编程模式
pub(crate) fn render_arduino_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_arduino(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("Arduino Operations (STK500v1/Optiboot)"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    // Arduino 配置
    egui::Grid::new("arduino_cfg_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Page Size"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut fs.arduino_page_size)
                    .range(16..=512)
                    .speed(1.0),
            )
            .on_hover_text(crate::i18n::t("AVR page size (bytes), ATmega328P=128, ATmega2560=256"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Flash (KB)"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut fs.arduino_flash_kb)
                        .range(1..=512)
                        .speed(1.0),
                )
                .on_hover_text(crate::i18n::t("Chip Flash capacity (KB), ATmega328P=32, ATmega2560=256"));
                if !fs.arduino_chip_name.is_empty() {
                    ui.label(
                        RichText::new(format!("({})", fs.arduino_chip_name))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                }
            });
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Tip: send Get Sync within 1.5s after Arduino reset"))
            .color(theme::TEXT_DISABLED)
            .small(),
    );
    ui.add_space(4.0);

    // 按钮网格：两行四列
    egui::Grid::new("arduino_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            // 第一行
            if arduino_btn(ui, crate::i18n::t("Get Sync"))
                .on_hover_text(crate::i18n::t("Send STK_GET_SYNC (0x30 0x20) to establish STK500 connection"))
                .clicked() {
                do_arduino_get_sync(fs, state, is_zh);
            }
            if arduino_btn(ui, crate::i18n::t("Read Sign"))
                .on_hover_text(crate::i18n::t("Read chip signature (STK_READ_SIGN = 0x75 0x20 0x20 0x20) to identify chip"))
                .clicked() {
                do_arduino_read_sign(fs, state, is_zh);
            }
            if arduino_btn(ui, crate::i18n::t("Enter Prog"))
                .on_hover_text(crate::i18n::t("Send STK_ENTER_PROGMODE (0x50 0x20)"))
                .clicked() {
                do_arduino_enter_progmode(fs, state, is_zh);
            }
            if arduino_btn(ui, crate::i18n::t("Leave Prog"))
                .on_hover_text(crate::i18n::t("Send STK_LEAVE_PROGMODE (0x51 0x20)"))
                .clicked() {
                do_arduino_leave_progmode(fs, state, is_zh);
            }
            ui.end_row();

            // 第二行
            if arduino_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write firmware page by page (STK_PROG_PAGE = 0x64), auto cross-page"))
                .clicked() {
                do_arduino_write(fs, state, is_zh);
            }
            if arduino_btn(ui, crate::i18n::t("Read"))
                .on_hover_text(crate::i18n::t("Read Flash page by page (STK_READ_PAGE = 0x74)"))
                .clicked() {
                do_arduino_read(fs, state, is_zh);
            }
            if arduino_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read data as .hex file (Intel HEX format)"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            ui.end_row();
        });
}

/// Arduino 操作按钮样式
pub(crate) fn arduino_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 ESP8266 操作按钮组
///
/// ESP8266 与 ESP32 协议相近，复用部分 ESP32 思路但使用独立 Esp8266Flasher
pub(crate) fn render_esp8266_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_esp8266(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("ESP8266 Operations"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Tip: hold GPIO0 low at power-on to enter download mode"))
            .color(theme::TEXT_DISABLED)
            .small(),
    );
    ui.add_space(4.0);

    egui::Grid::new("esp8266_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            if esp8266_btn(ui, crate::i18n::t("Sync"))
                .on_hover_text(crate::i18n::t("Send ESP_SYNC (0x08) sync packet, multiple retries to improve success rate"))
                .clicked() {
                do_esp8266_sync(fs, state, is_zh);
            }
            if esp8266_btn(ui, crate::i18n::t("Read MAC"))
                .on_hover_text(crate::i18n::t("Read ESP8266 MAC address (ESP_READ_MAC = 0x0B)"))
                .clicked() {
                do_esp8266_read_mac(fs, state, is_zh);
            }
            if esp8266_btn(ui, crate::i18n::t("Erase"))
                .on_hover_text(crate::i18n::t("Erase Flash region (ESP_ERASE_DATA = 0xD0)"))
                .clicked() {
                do_esp8266_erase(fs, state, is_zh);
            }
            if esp8266_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write firmware in blocks (ESP_FLASH_DATA = 0x03), 4KB/block"))
                .clicked() {
                do_esp8266_write(fs, state, is_zh);
            }
            ui.end_row();

            if esp8266_btn(ui, crate::i18n::t("Read"))
                .on_hover_text(crate::i18n::t("Read Flash data in blocks (ESP_READ_FLASH = 0x0E)"))
                .clicked() {
                do_esp8266_read(fs, state, is_zh);
            }
            if esp8266_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read Flash data as .bin file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            if esp8266_btn(ui, crate::i18n::t("Finish"))
                .on_hover_text(crate::i18n::t("End Flash operations and reset (ESP_FLASH_END = 0x04)"))
                .clicked() {
                do_esp8266_finish(fs, state, is_zh);
            }
            ui.end_row();
        });
}

pub(crate) fn esp8266_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(egui::Color32::BLACK).strong())
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 STM8 操作按钮组
pub(crate) fn render_stm8_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_stm8(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("STM8 Operations"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("stm8_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            if stm8_btn(ui, crate::i18n::t("Connect (0x7F)"))
                .on_hover_text(crate::i18n::t("Send 0x7F sync byte to establish STM8 UART Bootloader connection"))
                .clicked() {
                do_stm8_connect(fs, state, is_zh);
            }
            if stm8_btn(ui, crate::i18n::t("GET Version"))
                .on_hover_text(crate::i18n::t("Read Bootloader version and supported commands (GET = 0x00)"))
                .clicked() {
                do_stm8_get(fs, state, is_zh);
            }
            if stm8_btn(ui, crate::i18n::t("Mass Erase"))
                .on_hover_text(crate::i18n::t("Erase entire Flash (global mass erase command)"))
                .clicked() {
                do_stm8_mass_erase(fs, state, is_zh);
            }
            if stm8_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write firmware to STM8 Flash block by block"))
                .clicked() {
                do_stm8_write(fs, state, is_zh);
            }
            ui.end_row();

            if stm8_btn(ui, crate::i18n::t("Read"))
                .on_hover_text(crate::i18n::t("Read data from STM8 Flash, save to backup file"))
                .clicked() {
                do_stm8_read(fs, state, is_zh);
            }
            if stm8_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read data as .bin or .hex file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            if stm8_btn(ui, crate::i18n::t("Go"))
                .on_hover_text(crate::i18n::t("Jump to start address to run user program (GO command)"))
                .clicked() {
                do_stm8_go(fs, state, is_zh);
            }
            ui.end_row();
        });
}

pub(crate) fn stm8_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(egui::Color32::BLACK).strong())
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 NXP LPC ISP 操作按钮组
pub(crate) fn render_lpc_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_lpc(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("NXP LPC ISP Operations (UM10295)"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("lpc_cfg_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Sector Range"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut fs.lpc_sector_start).range(0..=1023).speed(0.5));
                ui.label("..");
                ui.add(egui::DragValue::new(&mut fs.lpc_sector_end).range(0..=1023).speed(0.5));
            });
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("RAM Addr"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add(
                egui::DragValue::new(&mut fs.lpc_ram_addr)
                    .hexadecimal(8, false, true)
                    .range(0..=0xFFFF_FFFFu32)
                    .speed(1.0),
            );
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Tip: LPC ISP synced by sending '?', hold P0.14 low on reset to enter ISP mode"))
            .color(theme::TEXT_DISABLED)
            .small(),
    );
    ui.add_space(4.0);

    egui::Grid::new("lpc_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            if lpc_btn(ui, crate::i18n::t("Sync (?)"))
                .on_hover_text(crate::i18n::t("Send '?' character to synchronize LPC ISP (UM10295)"))
                .clicked() {
                do_lpc_sync(fs, state, is_zh);
            }
            if lpc_btn(ui, crate::i18n::t("Unlock"))
                .on_hover_text(crate::i18n::t("Send Unlock command U 23130 to enable write/erase"))
                .clicked() {
                do_lpc_unlock(fs, state, is_zh);
            }
            if lpc_btn(ui, crate::i18n::t("Part ID"))
                .on_hover_text(crate::i18n::t("Read Part ID (J command) to identify LPC chip"))
                .clicked() {
                do_lpc_read_part_id(fs, state, is_zh);
            }
            if lpc_btn(ui, crate::i18n::t("Read UID"))
                .on_hover_text(crate::i18n::t("Read unique ID (N command)"))
                .clicked() {
                do_lpc_read_uid(fs, state, is_zh);
            }
            ui.end_row();

            if lpc_btn(ui, crate::i18n::t("Erase"))
                .on_hover_text(crate::i18n::t("Erase sectors (P command + sector range)"))
                .clicked() {
                do_lpc_erase(fs, state, is_zh);
            }
            if lpc_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write to RAM then copy to Flash (W command)"))
                .clicked() {
                do_lpc_write(fs, state, is_zh);
            }
            if lpc_btn(ui, crate::i18n::t("Read"))
                .on_hover_text(crate::i18n::t("Read Flash data (R command)"))
                .clicked() {
                do_lpc_read(fs, state, is_zh);
            }
            if lpc_btn(ui, crate::i18n::t("Go"))
                .on_hover_text(crate::i18n::t("Jump to address to run program (G command)"))
                .clicked() {
                do_lpc_go(fs, state, is_zh);
            }
            ui.end_row();

            if lpc_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read data as .bin or .hex file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            ui.end_row();
        });
}

pub(crate) fn lpc_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(egui::Color32::BLACK).strong())
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 CH32 (WCH) 操作按钮组
pub(crate) fn render_ch32_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_ch32(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("CH32 Operations (WCH ISP)"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Tip: CH32 uses the WCH ISP protocol (0x57 0xAB frame) over USB or UART"))
            .color(theme::TEXT_DISABLED)
            .small(),
    );
    ui.add_space(4.0);

    egui::Grid::new("ch32_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            if ch32_btn(ui, crate::i18n::t("Identify"))
                .on_hover_text(crate::i18n::t("Send WCH ISP identify packet (0x57 0xAB header + IDENTIFY)"))
                .clicked() {
                do_ch32_identify(fs, state, is_zh);
            }
            if ch32_btn(ui, crate::i18n::t("Chip ID"))
                .on_hover_text(crate::i18n::t("Read CH32 chip ID to identify model"))
                .clicked() {
                do_ch32_read_chip_id(fs, state, is_zh);
            }
            if ch32_btn(ui, crate::i18n::t("Erase"))
                .on_hover_text(crate::i18n::t("Erase Flash region via WCH ISP"))
                .clicked() {
                do_ch32_erase(fs, state, is_zh);
            }
            if ch32_btn(ui, crate::i18n::t("Write"))
                .on_hover_text(crate::i18n::t("Write firmware to Flash via WCH ISP"))
                .clicked() {
                do_ch32_write(fs, state, is_zh);
            }
            ui.end_row();

            if ch32_btn(ui, crate::i18n::t("Read"))
                .on_hover_text(crate::i18n::t("Read Flash data via WCH ISP"))
                .clicked() {
                do_ch32_read(fs, state, is_zh);
            }
            if ch32_btn(ui, crate::i18n::t("Verify"))
                .on_hover_text(crate::i18n::t("Verify Flash data against firmware"))
                .clicked() {
                do_ch32_verify(fs, state, is_zh);
            }
            if ch32_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read data as .bin or .hex file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            if ch32_btn(ui, crate::i18n::t("Reset"))
                .on_hover_text(crate::i18n::t("Reset CH32 chip (ISP reset command)"))
                .clicked() {
                do_ch32_reset(fs, state, is_zh);
            }
            ui.end_row();
        });
}

pub(crate) fn ch32_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(egui::Color32::BLACK).strong())
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 PIC ICSP 操作按钮组
///
/// 注意：PIC ICSP 需要 PicKit/ICD 等专用编程器硬件，miconlab 通过 UART
/// 无法直接驱动 ICSP 时序（PGC/PGD）。本面板仅提供命令构造展示和签名识别，
/// 实际烧录建议使用 Microchip MPLAB IPE 或 PicKit 软件。
pub(crate) fn render_pic_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_pic(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("PIC ICSP Operations"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Warning: PIC ICSP requires PicKit/ICD programmer. UART cannot drive ICSP timing.\n  This panel shows command sequence only, use MPLAB IPE for real flashing"))
        .color(theme::WARNING)
        .small(),
    );
    ui.add_space(4.0);

    egui::Grid::new("pic_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            if pic_btn(ui, crate::i18n::t("Load Config"))
                .on_hover_text(crate::i18n::t("Load config registers (requires PicKit/ICD programmer)"))
                .clicked() {
                do_pic_load_config(fs, state, is_zh);
            }
            if pic_btn(ui, crate::i18n::t("Bulk Erase"))
                .on_hover_text(crate::i18n::t("Bulk erase (requires PicKit/ICD programmer)"))
                .clicked() {
                do_pic_bulk_erase(fs, state, is_zh);
            }
            if pic_btn(ui, crate::i18n::t("Read Page"))
                .on_hover_text(crate::i18n::t("Read Flash page (requires PicKit/ICD programmer)"))
                .clicked() {
                do_pic_read_page(fs, state, is_zh);
            }
            if pic_btn(ui, crate::i18n::t("Write Page"))
                .on_hover_text(crate::i18n::t("Write Flash page (requires PicKit/ICD programmer)"))
                .clicked() {
                do_pic_write_page(fs, state, is_zh);
            }
            ui.end_row();

            if pic_btn(ui, crate::i18n::t("Save Backup"))
                .on_hover_text(crate::i18n::t("Save read data as .bin or .hex file"))
                .clicked() {
                save_backup_to_file(fs, state, is_zh);
            }
            ui.end_row();
        });
}

pub(crate) fn pic_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(egui::Color32::BLACK).strong())
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 SD 卡镜像烧录面板
///
/// 用于 Raspberry Pi / Jetson Nano / Orange Pi 等 Linux SBC 开发板的镜像烧录
/// 流程：列出磁盘 → 用户选择 SD 卡 → 选择镜像 → 卸载分区 → dd 写入 → 弹出
pub(crate) fn render_sd_card_ops(ui: &mut egui::Ui, fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    chip_step_guide_sd_card(ui, is_zh);
    ui.label(
        RichText::new(crate::i18n::t("SD Card Image Flasher (RPi/Jetson Nano)"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    // 显示磁盘列表
    egui::Grid::new("sd_cfg_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("SD Card Device"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut fs.sd_card_disk)
                    .desired_width(180.0)
                    .hint_text("/dev/disk2"),
            );
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Image File"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut fs.sd_card_image_path)
                    .desired_width(280.0)
                    .hint_text("/path/to/raspberry.img"),
            );
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Need sudo"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.checkbox(&mut fs.sd_card_need_sudo, crate::i18n::t("use sudo"));
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("flasher.ui.sd_card.warning"))
            .color(theme::WARNING)
            .small(),
    );
    ui.add_space(4.0);

    egui::Grid::new("sd_btn_grid")
        .num_columns(4)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            if sd_btn(ui, crate::i18n::t("Scan Disks"))
                .on_hover_text(crate::i18n::t("List available disks via system diskutil/diskpart"))
                .clicked() {
                do_sd_scan_disks(fs, state, is_zh);
            }
            if sd_btn(ui, crate::i18n::t("Unmount"))
                .on_hover_text(crate::i18n::t("Unmount SD card partitions (diskutil unmount), required before flashing"))
                .clicked() {
                do_sd_unmount(fs, state, is_zh);
            }
            if sd_btn(ui, crate::i18n::t("Flash Image"))
                .on_hover_text(crate::i18n::t("Write image via dd command, needs sudo, WARNING: erases all data on target disk"))
                .clicked() {
                do_sd_flash_image(fs, state, is_zh);
            }
            if sd_btn(ui, crate::i18n::t("Eject"))
                .on_hover_text(crate::i18n::t("Eject SD card (diskutil eject), safe removal"))
                .clicked() {
                do_sd_eject(fs, state, is_zh);
            }
            ui.end_row();
        });

    ui.add_space(4.0);
    if !fs.sd_card_disk_list.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("Disk List"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        egui::Frame::group(ui.style())
            .fill(theme::BG_DEEP)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.set_min_height(80.0);
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&fs.sd_card_disk_list)
                                .color(theme::TEXT_PRIMARY)
                                .family(egui::FontFamily::Monospace)
                                .small(),
                        );
                    });
            });
    }
    let _ = state;
}

pub(crate) fn sd_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(egui::Color32::BLACK).strong())
            .fill(theme::ACCENT)
            .min_size(egui::Vec2::new(100.0, 26.0)),
    )
}

/// 渲染 SPI Flash 操作面板
pub(crate) fn render_spi_flash_ops(ui: &mut egui::Ui, fs: &mut FlasherState, is_zh: bool) {
    chip_step_guide_spi_flash(ui, is_zh);
    ui.heading(crate::i18n::t("SPI Flash Read/Write (USB Programmer)"));
    ui.add_space(4.0);
    ui.label(crate::i18n::t("flasher.ui.spi_flash.intro"));
    ui.add_space(8.0);

    // 桥接器类型选择
    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("USB Programmer:"));
        egui::ComboBox::from_id_salt("spi_bridge_combo")
            .selected_text(fs.spi_bridge_kind.as_str())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut fs.spi_bridge_kind, "CH341A".to_string(), "CH341A (VID=1A86 PID=5512)");
                ui.selectable_value(&mut fs.spi_bridge_kind, "FT232H".to_string(), "FT232H (VID=0403 PID=6014)");
            })
            .response
            .on_hover_text(crate::i18n::t("CH341A (VID=1A86 PID=5512) or FT232H (VID=0403 PID=6014)"));
        if ui.button(crate::i18n::t("Scan"))
            .on_hover_text(crate::i18n::t("Scan connected USB programmer devices"))
            .clicked() {
            let bridges = miconlab_core::usb_bridge::list_bridges();
            if bridges.is_empty() {
                log_push(fs, crate::i18n::t("No USB programmer found"));
            } else {
                for (kind, name) in bridges {
                    log_push(fs, format!("{}: {}", crate::i18n::t("Found"), name));
                    fs.spi_bridge_kind = kind.name().to_string();
                }
            }
        }
        if ui.button(crate::i18n::t("Connect & Identify"))
            .on_hover_text(crate::i18n::t("Open USB device and read JEDEC ID (0x9F) to identify chip, with SFDP/RES fallback"))
            .clicked() {
            do_spi_flash_connect(fs, is_zh);
        }
    });

    // JEDEC ID 显示
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("JEDEC ID:"));
        if fs.spi_jedec_info.is_empty() {
            ui.label(crate::i18n::t("(not identified)"));
        } else {
            ui.colored_label(egui::Color32::from_rgb(0x4C, 0xAF, 0x50), &fs.spi_jedec_info);
        }
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // 地址与长度配置
    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("Start Addr (HEX):"));
        let mut addr_str = format!("{:06X}", fs.spi_flash_addr);
        ui.add(egui::TextEdit::singleline(&mut addr_str).desired_width(80.0))
            .on_hover_text(crate::i18n::t("Flash start address in HEX (e.g. 0x000000)"));
        if let Ok(v) = u32::from_str_radix(addr_str.trim_start_matches("0x"), 16) {
            fs.spi_flash_addr = v;
        }
        ui.label(crate::i18n::t("Length (KB):"));
        ui.add(egui::DragValue::new(&mut fs.spi_flash_len_kb).range(1..=16384))
            .on_hover_text(crate::i18n::t("Flash read length (KB), should not exceed chip capacity"));
    });

    ui.add_space(4.0);

    // 读取操作
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Read Flash"))
            .on_hover_text(crate::i18n::t("Read in 4KB blocks via 0x03 Read Data command"))
            .clicked() {
            do_spi_flash_read(fs, is_zh);
        }
        if ui.button(crate::i18n::t("Save Backup"))
            .on_hover_text(crate::i18n::t("Save read data as .bin or .hex file"))
            .clicked() {
            do_spi_flash_save_backup(fs, is_zh);
        }
        if !fs.spi_flash_data.is_empty() {
            ui.label(
                crate::i18n::tf("{} bytes read", &[("{}", "{}", &format!("{}", fs.spi_flash_data.len()))]),
            );
        }
    });

    ui.add_space(4.0);

    // 写入操作
    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("Firmware:"));
        ui.add(egui::TextEdit::singleline(&mut fs.spi_flash_image_path).desired_width(280.0))
            .on_hover_text(crate::i18n::t("Path to .bin/.hex/.rom firmware file to write"));
        if ui.button(crate::i18n::t("Browse..."))
            .on_hover_text(crate::i18n::t("Open file picker dialog"))
            .clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(crate::i18n::t("Firmware"), &["bin", "hex", "rom"])
                .pick_file()
            {
                fs.spi_flash_image_path = path.display().to_string();
            }
        }
    });

    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Write Firmware"))
            .on_hover_text(crate::i18n::t("Auto-erase target range + page program (256B/page) + read-back verify"))
            .clicked() {
            do_spi_flash_write(fs, is_zh);
        }
        if ui.button(crate::i18n::t("Chip Erase"))
            .on_hover_text(crate::i18n::t("Send 0xC7 chip erase command (large chips may take minutes)"))
            .clicked() {
            do_spi_flash_erase(fs, is_zh);
        }
    });

    ui.add_space(8.0);

    // 提示信息
    ui.label(
        egui::RichText::new(crate::i18n::t("flasher.ui.spi_flash.write_hint"))
            .color(egui::Color32::from_rgb(0xE6, 0x5C, 0x00)),
    );

    // 进度与日志
    render_progress_and_log(ui, fs, is_zh);
}


