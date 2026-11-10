// 固件烧录面板（STM32 / ESP32 / Arduino AVR / ESP8266 / STM8 / LPC / CH32 / PIC / SD / SPI Flash）
// 提供单步操作按钮：连接、擦除、写入、读取、跳转、保存备份等
// 支持 Intel HEX 文件解析，进度条 + 日志区显示操作状态
//
// 模块化拆分：本文件仅包含 show() 入口与类型定义，各芯片协议实现见对应 ops_*.rs，
// 固件解读/反汇编见 inspect_*.rs，UI 渲染见 ui.rs / ui_ops.rs。

use crate::i18n::L10n;
use crate::panels::PanelKind;
use crate::state::AppState;
use crate::theme;
use egui::{RichText, ScrollArea};
use miconlab_core::hex::format_hex_spaced;
use miconlab_core::protocol::flasher::{
    self, Ch32Flasher, Esp32Flasher, Esp8266Flasher, LpcIspFlasher, PicIcspFlasher, Stk500Flasher,
    Stm32Flasher, Stm8Flasher,
};
use miconlab_ai::thumb_disasm;
use miconlab_ai::xtensa_disasm;
use miconlab_ai::chip_profile::{self, ChipArch};

// 子模块声明
mod state;
mod common;
mod ui;
mod ui_ops;
mod ops_stm32;
mod ops_esp32;
mod ops_esptool;
mod ops_esp8266;
mod ops_arduino;
mod ops_stm8;
mod ops_lpc;
mod ops_ch32;
mod ops_pic;
mod ops_sd_card;
mod ops_spi_flash;
mod inspect;
mod inspect_esp32;
mod inspect_arm;
mod report_readable;

// 导出子模块的公共项
pub(crate) use state::{ChipType, FlasherState, log_push, parse_pages};
pub(crate) use common::*;
pub(crate) use ui::*;
pub(crate) use ui_ops::*;
pub(crate) use ops_stm32::*;
pub(crate) use ops_esp32::*;
pub(crate) use ops_esptool::*;
pub(crate) use ops_esp8266::*;
pub(crate) use ops_arduino::*;
pub(crate) use ops_stm8::*;
pub(crate) use ops_lpc::*;
pub(crate) use ops_ch32::*;
pub(crate) use ops_pic::*;
pub(crate) use ops_sd_card::*;
pub(crate) use ops_spi_flash::*;
pub(crate) use inspect::*;
pub(crate) use inspect_esp32::*;
pub(crate) use inspect_arm::*;
pub(crate) use report_readable::*;

pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let is_zh = state.language == crate::i18n::Language::Zh;

    // 从 egui 持久化存储中取出面板状态
    let mut fs = ctx.data(|data| {
        data.get_temp::<FlasherState>(egui::Id::new("flasher_state"))
            .unwrap_or_default()
    });

    // 连接会话变化时清除设备派生数据（芯片识别结果、读取数据、日志），避免跨设备残留
    // 但跳过 esptool 后备读取期间的重连（esptool 需要断开/重连串口）
    if fs.last_session_id != state.connection_session_id && !fs.esptool_running {
        fs.arduino_chip_name.clear();
        fs.arduino_flash_kb = 32; // 重置为默认值
        fs.spi_jedec_info.clear();
        fs.spi_flash_capacity = 0;
        fs.spi_flash_data.clear();
        fs.last_read_data.clear();
        fs.response.clear();
        fs.log.clear();
        fs.progress = 0.0;
        // 同步当前连接波特率到烧录器
        fs.baud_current = state.config.baud_rate;
        fs.baud_new = state.config.baud_rate;
        fs.last_session_id = state.connection_session_id;
    }

    // 检查来自协议分析器的跳转提示（chip_hint + action_hint）
    let chip_hint = ctx.data(|data| {
        data.get_temp::<String>(egui::Id::new("flasher_chip_hint"))
    });
    let action_hint = ctx.data(|data| {
        data.get_temp::<String>(egui::Id::new("flasher_action_hint"))
    });
    if let Some(hint) = &chip_hint {
        match hint.as_str() {
            "stm32" => {
                fs.chip_type = ChipType::Stm32;
            }
            "esp32" => {
                fs.chip_type = ChipType::Esp32;
            }
            "arduino" => {
                fs.chip_type = ChipType::Arduino;
            }
            "esp8266" => {
                fs.chip_type = ChipType::Esp8266;
            }
            "stm8" => {
                fs.chip_type = ChipType::Stm8;
            }
            "lpc" => {
                fs.chip_type = ChipType::NxpLpc;
            }
            "gd32" => {
                fs.chip_type = ChipType::Gd32;
            }
            "ch32" => {
                fs.chip_type = ChipType::Ch32;
            }
            "pic" => {
                fs.chip_type = ChipType::Pic;
            }
            "sdcard" => {
                fs.chip_type = ChipType::SdCard;
            }
            "spi_flash" => {
                fs.chip_type = ChipType::SpiFlash;
            }
            _ => {}
        }
        // 消费后清除 hint
        ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new("flasher_chip_hint")));
    }
    // 将 action_hint 转为一条日志提示（不自动执行，避免未连接时出错）
    if let Some(action) = &action_hint {
        match action.as_str() {
            "read" => {
                log_push(&mut fs, crate::i18n::t("← From Protocol Analyzer: click Read button to backup firmware").to_string());
            }
            "write" => {
                log_push(&mut fs, crate::i18n::t("← From Protocol Analyzer: select firmware file then click Write").to_string());
            }
            _ => {}
        }
        ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new("flasher_action_hint")));
    }

    // 非阻塞读取状态机：每帧推进一步，避免 GUI 卡死
    update_flasher_read(&mut fs, state, is_zh);

    // 轮询 esptool.py 后备读取结果
    poll_esptool_result(&mut fs, state, is_zh);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(12.0)),
        )
        .show(ctx, |ui| {
            // 标题
            ui.heading(
                RichText::new(l10n.t("panel.flasher"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(crate::i18n::t("STM32 / ESP32 / Arduino Firmware Flasher (Step-by-Step)"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );

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

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // ---- 通用配置区 ----
                    render_common_config(ctx, ui, &mut fs, state, is_zh);

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ---- 芯片特定操作按钮 ----
                    match fs.chip_type {
                        ChipType::Stm32 => render_stm32_ops(ui, &mut fs, state, is_zh),
                        ChipType::Esp32 => render_esp32_ops(ui, &mut fs, state, is_zh),
                        ChipType::Arduino => render_arduino_ops(ui, &mut fs, state, is_zh),
                        ChipType::Esp8266 => render_esp8266_ops(ui, &mut fs, state, is_zh),
                        ChipType::Stm8 => render_stm8_ops(ui, &mut fs, state, is_zh),
                    ChipType::NxpLpc => render_lpc_ops(ui, &mut fs, state, is_zh),
                    ChipType::Gd32 => render_stm32_ops(ui, &mut fs, state, is_zh),
                    ChipType::Ch32 => render_ch32_ops(ui, &mut fs, state, is_zh),
                    ChipType::Pic => render_pic_ops(ui, &mut fs, state, is_zh),
                    ChipType::SdCard => render_sd_card_ops(ui, &mut fs, state, is_zh),
                    ChipType::SpiFlash => render_spi_flash_ops(ui, &mut fs, is_zh),
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ---- 进度条 + 日志区 ----
                    render_progress_and_log(ui, &fs, is_zh);
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
        data.insert_temp(egui::Id::new("flasher_state"), fs);
    });
}

