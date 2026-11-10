// I2C/SPI 调试面板（基于 CH341A 协议）
// 提供 I2C 速度设置、读写、寄存器读、总线扫描，以及 SPI 传输
// 所有命令发送前用 ch341_packetize 分包，结果显示为 HEX 格式

use crate::i18n::L10n;
use crate::state::AppState;
use crate::theme;
use egui::{RichText, ScrollArea};
use miconlab_core::hex::format_hex_spaced;
use miconlab_core::protocol::i2c_spi::{
    self, ch341_packetize, ch341_set_speed, i2c_device_name, I2cSpeed,
};

// ---- 可选速度列表 ----
const SPEEDS: &[I2cSpeed] = &[
    I2cSpeed::Low,
    I2cSpeed::Standard,
    I2cSpeed::Fast,
    I2cSpeed::High,
];

/// 速度选项的显示标签（含频率）
fn speed_label(speed: I2cSpeed) -> String {
    match speed {
        I2cSpeed::Low => format!("Low ({} Hz)", speed.hz()),
        I2cSpeed::Standard => format!("Standard ({} Hz)", speed.hz()),
        I2cSpeed::Fast => format!("Fast ({} Hz)", speed.hz()),
        I2cSpeed::High => format!("High ({} Hz)", speed.hz()),
    }
}

/// 持久化面板状态（通过 ctx.data_mut 存储）
#[derive(Default, Clone)]
struct I2cSpiState {
    address: String,         // I2C 设备地址（hex 字符串，如 "50" 或 "0x50"）
    write_data: String,      // I2C 写入数据（hex 字符串）
    reg_data: String,        // I2C 寄存器地址（hex 字符串）
    read_len: usize,         // I2C 读取长度
    speed_idx: usize,        // 速度索引（对应 SPEEDS）
    scan_results: Vec<String>, // I2C 扫描结果
    spi_tx_data: String,     // SPI 发送数据（hex 字符串）
    spi_cs: u8,              // SPI CS 引脚
    spi_mode: u8,            // SPI 模式（0-3）
    response: String,        // 响应显示区（HEX 格式）
    /// 上次连接会话 ID，用于检测连接变化并清除设备派生数据
    last_session_id: u64,
}

/// 解析 hex 地址字符串为 u8（支持 "50"、"0x50" 等格式）
fn parse_address(s: &str) -> Result<u8, String> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    let val = u8::from_str_radix(s, 16).map_err(|e| crate::i18n::tf("Invalid address: {error}", &[("{error}", "{error}", &format!("{}", e))]))?;
    if val < 0x08 || val > 0x77 {
        return Err(crate::i18n::t("Address range: 0x08-0x77").to_string());
    }
    Ok(val)
}

/// 解析 hex 数据字符串为 Vec<u8>
fn parse_hex_data(s: &str) -> Result<Vec<u8>, String> {
    miconlab_core::hex::parse_hex(s)
}

/// 在 I2C 读写操作前发送速度设置命令。
/// CH341A I2C 需要先设置 SCL 频率，否则使用上一次/默认速度可能通信失败。
fn send_i2c_speed(ps: &I2cSpiState, state: &mut AppState) -> Result<(), String> {
    let speed = SPEEDS[ps.speed_idx.min(SPEEDS.len() - 1)];
    let speed_cmd = ch341_set_speed(speed);
    for pkt in ch341_packetize(&speed_cmd) {
        state.send_raw(&pkt)?;
    }
    Ok(())
}

/// 渲染 I2C/SPI 面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let is_zh = state.language == crate::i18n::Language::Zh;

    // 从 egui 持久化存储中取出面板状态（克隆出来使用，避免与渲染争用 ctx 写锁）
    let mut ps = ctx.data(|data| {
        data.get_temp::<I2cSpiState>(egui::Id::new("i2c_spi_state"))
            .unwrap_or_default()
    });

    // 连接会话变化时清除设备派生数据（扫描结果、响应），避免跨设备残留
    if ps.last_session_id != state.connection_session_id {
        ps.scan_results.clear();
        ps.response.clear();
        ps.last_session_id = state.connection_session_id;
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(12.0)),
        )
        .show(ctx, |ui| {
            // 标题
            ui.heading(
                RichText::new(l10n.t("panel.i2c_spi"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(crate::i18n::t("Scan I2C devices and access SPI bus via CH341A/FT232H USB adapters"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );

            // AI 感知：错误诊断（连接失败或操作异常时自动推断根因）
            if let Some(err) = &state.last_error {
                if let Some(hint) = crate::panels::ai_sense::sense_error(err, state.is_connected(), state.config.baud_rate, is_zh) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(4.0);
                }
            }

            // 连接状态提示
            ui.add_space(4.0);
            if !state.is_connected() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(
                        RichText::new(crate::i18n::t("Not connected - please connect in the Connection panel first"))
                        .color(theme::WARNING),
                    );
                });
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // 使用滚动布局容纳所有控件
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // ---- I2C 配置区 ----
                    render_i2c_section(ui, &mut ps, state, is_zh);

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ---- SPI 配置区 ----
                    render_spi_section(ui, &mut ps, state, is_zh);

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ---- 响应显示区 ----
                    render_response_area(ui, &ps, is_zh);
                });

            // 错误信息显示在底部
            if let Some(err) = &state.last_error {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(RichText::new(err).color(theme::ERROR));
                });
            }
        });

    // 写回持久化状态
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("i2c_spi_state"), ps);
    });
}

/// 渲染 I2C 配置与操作区
fn render_i2c_section(ui: &mut egui::Ui, ps: &mut I2cSpiState, state: &mut AppState, is_zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("I2C Bus"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("i2c_config_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            // 速度选择
            ui.label(
                RichText::new(crate::i18n::t("Speed"))
                    .color(theme::TEXT_SECONDARY),
            );
            let speed_idx = ps.speed_idx.min(SPEEDS.len() - 1);
            let selected_label = speed_label(SPEEDS[speed_idx]);
            egui::ComboBox::from_id_salt("i2c_speed_select")
                .selected_text(&selected_label)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for (i, sp) in SPEEDS.iter().enumerate() {
                        ui.selectable_value(&mut ps.speed_idx, i, speed_label(*sp));
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("I2C SCL frequency. Low 10kHz / Standard 100kHz / Fast 400kHz / High 1MHz. Must match what the slave device supports"));
            ui.end_row();

            // 设备地址
            ui.label(
                RichText::new(crate::i18n::t("Device Address"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut ps.address)
                        .desired_width(80.0)
                        .hint_text("0x50"),
                )
                .on_hover_text(crate::i18n::t("I2C 7-bit device address (hex, range 0x08-0x77). 0x00-0x07 and 0x78-0x7F are reserved and cannot be used"));
                ui.label(
                    RichText::new("(hex, 0x08-0x77)")
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            });
            ui.end_row();
        });

    ui.add_space(6.0);

    // 写操作
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Write Data"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut ps.write_data)
                .desired_width(200.0)
                .hint_text("00 01 02"),
        )
        .on_hover_text(crate::i18n::t("HEX format, bytes separated by spaces (e.g. 55 AA 01)"));
        let btn = egui::Button::new(
            RichText::new(crate::i18n::t("I2C Write"))
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(80.0, 24.0));
        if ui
            .add(btn)
            .on_hover_text(crate::i18n::t("Write data to the given I2C device address. CH341A sets SCL speed first, then issues START + addr(W) + data + STOP"))
            .clicked() {
            do_i2c_write(ps, state, is_zh);
        }
    });

    ui.add_space(4.0);

    // 读操作
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Read Length"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(egui::DragValue::new(&mut ps.read_len).range(1..=256).speed(0.1))
            .on_hover_text(crate::i18n::t("Maximum bytes to read (CH341A supports up to 256 per transfer)"));
        let btn = egui::Button::new(
            RichText::new(crate::i18n::t("I2C Read"))
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(80.0, 24.0));
        if ui
            .add(btn)
            .on_hover_text(crate::i18n::t("Read N bytes from the given I2C device address. CH341A issues START + addr(R) + read + NACK + STOP"))
            .clicked() {
            do_i2c_read(ps, state, is_zh);
        }
    });

    ui.add_space(4.0);

    // 寄存器读（先写寄存器地址再读）
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Register"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut ps.reg_data)
                .desired_width(80.0)
                .hint_text("00"),
        )
        .on_hover_text(crate::i18n::t("8-bit register address in HEX (e.g. 00)"));
        let btn = egui::Button::new(
            RichText::new(crate::i18n::t("I2C Read Reg"))
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(egui::Vec2::new(110.0, 24.0));
        if ui
            .add(btn)
            .on_hover_text(crate::i18n::t("Write register address then read data (Restart mode): START + addr(W) + reg + ReSTART + addr(R) + read + STOP. Used for reading sensor registers"))
            .clicked() {
            do_i2c_write_read(ps, state, is_zh);
        }
    });

    ui.add_space(4.0);

    // 总线扫描
    ui.horizontal(|ui| {
        let btn = egui::Button::new(
            RichText::new(crate::i18n::t("Scan Bus"))
                .color(egui::Color32::BLACK)
                .strong(),
        )
        .fill(theme::CYAN)
        .min_size(egui::Vec2::new(110.0, 24.0));
        if ui
            .add(btn)
            .on_hover_text(crate::i18n::t("Probe addresses 0x08-0x77 one by one to list all online I2C devices and common models. CH341A detects presence via short-timeout reads"))
            .clicked() {
            do_i2c_scan(ps, state, is_zh);
        }
    });

    // 扫描结果显示
    if !ps.scan_results.is_empty() {
        ui.add_space(4.0);
        egui::Frame::group(ui.style())
            .fill(theme::BG_DEEP)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(crate::i18n::tf("Online devices ({}):", &[("{}", "{}", &format!("{}", ps.scan_results.len()))]))
                    .color(theme::SUCCESS)
                    .small(),
                );
                for line in &ps.scan_results {
                    ui.label(
                        RichText::new(line)
                            .color(theme::CYAN)
                            .family(egui::FontFamily::Monospace)
                            .small(),
                    );
                }
            });
    }
}

/// 执行 I2C 写操作
fn do_i2c_write(ps: &mut I2cSpiState, state: &mut AppState, _is_zh: bool) {
    if !state.is_connected() {
        state.last_error = Some(crate::i18n::t("Not connected").to_string());
        return;
    }

    let addr = match parse_address(&ps.address) {
        Ok(a) => a,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    let data = match parse_hex_data(&ps.write_data) {
        Ok(d) => d,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    // 先发送 I2C 速度设置命令
    if let Err(e) = send_i2c_speed(ps, state) {
        state.last_error = Some(e);
        return;
    }

    // 构造子命令流并分包发送
    let sub_cmd = i2c_spi::ch341_i2c_write(addr, &data);
    let packets = ch341_packetize(&sub_cmd);
    ps.response = format!(
        ">> I2C Write addr=0x{:02X} data=[{}]\n",
        addr,
        format_hex_spaced(&data)
    );
    for pkt in &packets {
        if let Err(e) = state.send_raw(pkt) {
            state.last_error = Some(e);
            return;
        }
    }
    ps.response.push_str(&format!(
        "<< TX: {}\n",
        format_hex_spaced(&packets.concat())
    ));
    state.last_error = None;
}

/// 执行 I2C 读操作
fn do_i2c_read(ps: &mut I2cSpiState, state: &mut AppState, _is_zh: bool) {
    if !state.is_connected() {
        state.last_error = Some(crate::i18n::t("Not connected").to_string());
        return;
    }

    let addr = match parse_address(&ps.address) {
        Ok(a) => a,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    let len = ps.read_len.max(1);

    // 先发送 I2C 速度设置命令
    if let Err(e) = send_i2c_speed(ps, state) {
        state.last_error = Some(e);
        return;
    }

    let sub_cmd = i2c_spi::ch341_i2c_read(addr, len);
    let packets = ch341_packetize(&sub_cmd);
    ps.response = format!(">> I2C Read addr=0x{:02X} len={}\n", addr, len);

    // 发送所有分包
    for pkt in &packets {
        if let Err(e) = state.send_raw(pkt) {
            state.last_error = Some(e);
            return;
        }
    }

    // 读取响应（CH341A 会在 USB IN 端点返回读取的数据）
    match state.read_response(200) {
        Ok(resp) => {
            ps.response
                .push_str(&format!("<< RX ({} bytes): {}\n", resp.len(), format_hex_spaced(&resp)));
        }
        Err(e) => {
            ps.response
                .push_str(&format!("<< Read error: {}\n", e));
        }
    }
    state.last_error = None;
}

/// 执行 I2C 寄存器读（先写寄存器地址再读数据）
fn do_i2c_write_read(ps: &mut I2cSpiState, state: &mut AppState, _is_zh: bool) {
    if !state.is_connected() {
        state.last_error = Some(crate::i18n::t("Not connected").to_string());
        return;
    }

    let addr = match parse_address(&ps.address) {
        Ok(a) => a,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    let reg_data = match parse_hex_data(&ps.reg_data) {
        Ok(d) => d,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    let read_len = ps.read_len.max(1);

    // 先发送 I2C 速度设置命令
    if let Err(e) = send_i2c_speed(ps, state) {
        state.last_error = Some(e);
        return;
    }

    let sub_cmd = i2c_spi::ch341_i2c_write_read(addr, &reg_data, read_len);
    let packets = ch341_packetize(&sub_cmd);
    ps.response = format!(
        ">> I2C WriteRead addr=0x{:02X} reg=[{}] read_len={}\n",
        addr,
        format_hex_spaced(&reg_data),
        read_len
    );

    for pkt in &packets {
        if let Err(e) = state.send_raw(pkt) {
            state.last_error = Some(e);
            return;
        }
    }

    match state.read_response(200) {
        Ok(resp) => {
            ps.response
                .push_str(&format!("<< RX ({} bytes): {}\n", resp.len(), format_hex_spaced(&resp)));
        }
        Err(e) => {
            ps.response
                .push_str(&format!("<< Read error: {}\n", e));
        }
    }
    state.last_error = None;
}

/// 执行 I2C 总线扫描（0x08-0x77）
fn do_i2c_scan(ps: &mut I2cSpiState, state: &mut AppState, _is_zh: bool) {
    if !state.is_connected() {
        state.last_error = Some(crate::i18n::t("Not connected").to_string());
        return;
    }

    // 先设置速度
    let speed = SPEEDS[ps.speed_idx.min(SPEEDS.len() - 1)];
    let speed_cmd = ch341_set_speed(speed);
    let speed_packets = ch341_packetize(&speed_cmd);
    for pkt in &speed_packets {
        if let Err(e) = state.send_raw(pkt) {
            state.last_error = Some(e);
            return;
        }
    }

    // 逐个地址探测
    let scan_cmds = i2c_spi::ch341_i2c_scan_commands();
    let mut found: Vec<String> = Vec::new();
    ps.response = format!(">> I2C Bus Scan (0x08-0x77), speed={} Hz\n", speed.hz());

    for (addr, sub_cmd) in &scan_cmds {
        let packets = ch341_packetize(sub_cmd);
        for pkt in &packets {
            if state.send_raw(pkt).is_err() {
                continue;
            }
        }
        // 短超时探测：有响应即认为设备在线
        if let Ok(resp) = state.read_response(20) {
            if !resp.is_empty() {
                let name = i2c_device_name(*addr).unwrap_or("Unknown");
                found.push(format!("0x{:02X}  {}", addr, name));
            }
        }
    }

    if found.is_empty() {
        ps.response
            .push_str(crate::i18n::t("<< No I2C devices found\n"));
    } else {
        ps.response
            .push_str(&format!("<< Found {} device(s)\n", found.len()));
    }

    ps.scan_results = found;
    state.last_error = None;
}

/// 渲染 SPI 配置与操作区
fn render_spi_section(ui: &mut egui::Ui, ps: &mut I2cSpiState, state: &mut AppState, is_zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("SPI Bus"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    egui::Grid::new("spi_config_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            // CS 引脚
            ui.label(
                RichText::new(crate::i18n::t("CS Pin"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::DragValue::new(&mut ps.spi_cs)
                    .range(0..=15)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("CS0/CS1 selects the CH341A chip-select pin"));
            ui.end_row();

            // SPI 模式
            ui.label(
                RichText::new(crate::i18n::t("SPI Mode"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::DragValue::new(&mut ps.spi_mode)
                    .range(0..=3)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("CPOL/CPHA modes 0/1/2/3; must match the slave device"));
            ui.end_row();

            // 发送数据
            ui.label(
                RichText::new(crate::i18n::t("TX Data"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut ps.spi_tx_data)
                    .desired_width(200.0)
                    .hint_text("FF 00 AA"),
            )
            .on_hover_text(crate::i18n::t("HEX format, full-duplex MOSI↔MISO transfer"));
            ui.end_row();
        });

    ui.add_space(4.0);

    // SPI 传输按钮
    let btn = egui::Button::new(
        RichText::new(crate::i18n::t("SPI Transfer"))
            .color(egui::Color32::BLACK)
            .strong(),
    )
    .fill(theme::ACCENT)
    .min_size(egui::Vec2::new(120.0, 24.0));
    if ui
        .add(btn)
        .on_hover_text(crate::i18n::t("Full-duplex SPI transfer: set CPOL/CPHA mode, assert CS to select slave, send TX data while reading MISO simultaneously, then release CS"))
        .clicked() {
        do_spi_transfer(ps, state, is_zh);
    }
}

/// 执行 SPI 传输
fn do_spi_transfer(ps: &mut I2cSpiState, state: &mut AppState, _is_zh: bool) {
    if !state.is_connected() {
        state.last_error = Some(crate::i18n::t("Not connected").to_string());
        return;
    }

    let tx_data = match parse_hex_data(&ps.spi_tx_data) {
        Ok(d) => d,
        Err(e) => {
            state.last_error = Some(e);
            return;
        }
    };

    if tx_data.is_empty() {
        state.last_error = Some(crate::i18n::t("SPI TX data is empty").to_string());
        return;
    }

    ps.response = format!(
        ">> SPI Transfer cs={} mode={} tx=[{}]\n",
        ps.spi_cs,
        ps.spi_mode,
        format_hex_spaced(&tx_data)
    );

    // 1. 设置 SPI 模式（CPOL/CPHA，模式 0-3）
    let mode_cmd = [0xA8u8, ps.spi_mode & 0x03];
    if let Err(e) = state.send_raw(&mode_cmd) {
        state.last_error = Some(e);
        return;
    }

    // 2. 拉低 CS 引脚（选中从设备）
    let cs_assert = [0xA4u8, ps.spi_cs & 0x0F];
    if let Err(e) = state.send_raw(&cs_assert) {
        state.last_error = Some(e);
        return;
    }

    // 3. SPI 全双工数据传输（0xAB + 数据，沿用 core 层 spi_transfer 实现）
    let cmd = i2c_spi::spi_transfer(&tx_data, ps.spi_cs, ps.spi_mode);
    if let Err(e) = state.send_raw(&cmd) {
        state.last_error = Some(e);
        return;
    }

    // SPI 为全双工，读取 MISO 返回数据
    match state.read_response(200) {
        Ok(resp) => {
            ps.response
                .push_str(&format!("<< RX ({} bytes): {}\n", resp.len(), format_hex_spaced(&resp)));
        }
        Err(e) => {
            ps.response
                .push_str(&format!("<< Read error: {}\n", e));
        }
    }

    // 4. 拉高 CS 引脚（释放从设备）；释放失败不影响已读数据
    let cs_deassert = [0xA4u8, (ps.spi_cs & 0x0F) | 0x80];
    let _ = state.send_raw(&cs_deassert);

    state.last_error = None;
}

/// 渲染响应显示区
fn render_response_area(ui: &mut egui::Ui, ps: &I2cSpiState, _is_zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Response"))
            .color(theme::TEXT_SECONDARY),
    );
    ui.add_space(2.0);

    egui::Frame::group(ui.style())
        .fill(theme::BG_DEEP)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_min_height(100.0);
            if ps.response.is_empty() {
                ui.label(
                    RichText::new(crate::i18n::t("Waiting for operation..."))
                    .color(theme::TEXT_DISABLED)
                    .family(egui::FontFamily::Monospace),
                );
            } else {
                ui.label(
                    RichText::new(ps.response.as_str())
                        .color(theme::CYAN)
                        .family(egui::FontFamily::Monospace)
                        .small(),
                );
            }
        });
}
