// CSV 数据记录面板：文件路径、开始/停止、手动录入、状态显示
// DataLogger 持有文件句柄且非 Clone，用 Arc<Mutex<Option<DataLogger>>> 包装存入 IdTypeMap。

use crate::i18n::L10n;
use crate::state::AppState;
use crate::theme;
use egui::{Color32, Id, RichText, Vec2};
use miconlab_core::data_logger::DataLogger;
use std::sync::{Arc, Mutex};

/// 包装后的记录器存储类型
type LoggerSlot = Arc<Mutex<Option<DataLogger>>>;

/// 渲染数据记录面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == crate::i18n::Language::Zh;

    // 取出持久化的 Clone 字段
    let mut file_path: String = ctx
        .data_mut(|d| d.get_temp::<String>(Id::new("log_path")))
        .unwrap_or_else(|| "data_log.csv".to_string());
    let mut source: String = ctx
        .data_mut(|d| d.get_temp::<String>(Id::new("log_source")))
        .unwrap_or_default();
    let mut raw_value: String = ctx
        .data_mut(|d| d.get_temp::<String>(Id::new("log_raw")))
        .unwrap_or_default();
    let mut scaled_value: String = ctx
        .data_mut(|d| d.get_temp::<String>(Id::new("log_scaled")))
        .unwrap_or_default();
    let mut unit: String = ctx
        .data_mut(|d| d.get_temp::<String>(Id::new("log_unit")))
        .unwrap_or_default();
    let mut log_count: u64 = ctx
        .data_mut(|d| d.get_temp::<u64>(Id::new("log_count")))
        .unwrap_or(0);
    let mut flush_threshold: usize = ctx
        .data_mut(|d| d.get_temp::<usize>(Id::new("log_flush")))
        .unwrap_or(100);

    // 取出（或创建）记录器实例
    let logger_slot: LoggerSlot = ctx
        .data_mut(|d| d.get_temp::<LoggerSlot>(Id::new("log_logger")))
        .unwrap_or_else(|| Arc::new(Mutex::new(None)));

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            ui.heading(
                RichText::new(l10n.t("panel.logger"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            // 副标题：解释面板用途
            ui.label(
                RichText::new(crate::i18n::t("Log serial data to file with timestamps, supports text/binary/CSV formats"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );

            // AI 感知：错误诊断（连接失败或操作异常时自动推断根因）
            if let Some(err) = &state.last_error {
                if let Some(hint) = crate::panels::ai_sense::sense_error(err, state.is_connected(), state.config.baud_rate, zh) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(4.0);
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // 文件路径
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("File Path"))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut file_path)
                        .desired_width(f32::MAX)
                        .font(egui::FontId::monospace(12.5)),
                )
                .on_hover_text(crate::i18n::t("Log file path, supports .csv / .txt / .bin formats"));
            });

            ui.add_space(6.0);
            // 自动刷新阈值
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Auto Flush Threshold"))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add(
                    egui::DragValue::new(&mut flush_threshold)
                        .range(1..=10000)
                        .speed(0.2),
                )
                .on_hover_text(crate::i18n::t("Auto-flush to disk when buffer reaches this count, preventing data loss"));
            });

            ui.add_space(8.0);
            // 开始 / 停止按钮
            ui.horizontal(|ui| {
                let started = logger_slot
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|l| l.is_started())
                    .unwrap_or(false);
                let (btn_text, btn_color) = if started {
                    (
                        { crate::i18n::t("Stop") }.to_string(),
                        theme::ERROR,
                    )
                } else {
                    (
                        crate::i18n::t("Start").to_string(),
                        theme::SUCCESS,
                    )
                };
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(btn_text).color(Color32::BLACK).strong(),
                        )
                        .fill(btn_color)
                        .min_size(Vec2::new(130.0, 30.0))
                        .stroke(egui::Stroke::NONE),
                    )
                    .on_hover_text(crate::i18n::t("Start: creates log file and auto-flushes by threshold; Stop closes handle and flushes remaining data"))
                    .clicked()
                {
                    if started {
                        // 停止并丢弃句柄
                        if let Some(l) = logger_slot.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                            if let Err(e) = l.stop() {
                                state.last_error = Some(e.to_string());
                            }
                        }
                    } else {
                        // 以当前路径 + 阈值创建新记录器
                        let mut l = DataLogger::new(file_path.clone())
                            .with_auto_flush_threshold(flush_threshold);
                        match l.start() {
                            Ok(()) => {
                                *logger_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
                                state.last_error = None;
                            }
                            Err(e) => {
                                state.last_error = Some(e.to_string());
                            }
                        }
                    }
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // 手动录入区
            ui.label(
                RichText::new(crate::i18n::t("Manual Entry"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            egui::Grid::new("log_entry_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Source").color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut source).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Data source identifier (e.g. CH0/CH1/Sensor1)"));
                    ui.end_row();

                    ui.label(RichText::new("Raw").color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut raw_value).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Raw value (numeric or HEX)"));
                    ui.end_row();

                    ui.label(RichText::new("Scaled").color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut scaled_value).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Value converted to engineering units"));
                    ui.end_row();

                    ui.label(RichText::new("Unit").color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut unit).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Engineering unit (e.g. V/℃/Pa)"));
                    ui.end_row();
                });

            ui.add_space(6.0);
            // 录入按钮
            ui.horizontal(|ui| {
                let log_btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Log"))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .stroke(egui::Stroke::NONE);
                if ui
                    .add(log_btn)
                    .on_hover_text(crate::i18n::t("Manually log one data entry to the file (start recording first)"))
                    .clicked()
                {
                    let mut guard = logger_slot.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(l) = guard.as_mut() {
                        match l.log_value(&source, &raw_value, &scaled_value, &unit) {
                            Ok(()) => {
                                log_count += 1;
                                state.last_error = None;
                            }
                            Err(e) => {
                                state.last_error = Some(e.to_string());
                            }
                        }
                    } else {
                        state.last_error = Some(
                            crate::i18n::t("Logger not started").to_string(),
                        );
                    }
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // 状态显示
            let (started, buffered) = {
                let guard = logger_slot.lock().unwrap_or_else(|e| e.into_inner());
                (
                    guard.as_ref().map(|l| l.is_started()).unwrap_or(false),
                    guard.as_ref().map(|l| l.buffered_count()).unwrap_or(0),
                )
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        crate::i18n::t("Recording"),
                        if started { "\u{25CF}" } else { "\u{25CB}" }
                    ))
                    .color(if started { theme::SUCCESS } else { theme::TEXT_DISABLED }),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("{}: {}", crate::i18n::t("Buffered"), buffered))
                        .color(theme::TEXT_SECONDARY)
                        .family(egui::FontFamily::Monospace),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("{}: {}", { crate::i18n::t("Total##panels/logger.rs:10445") }, log_count))
                        .color(theme::TEXT_PRIMARY)
                        .family(egui::FontFamily::Monospace),
                );
            });

            // 错误提示
            if let Some(err) = &state.last_error {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("\u{26A0} {}", err)).color(theme::ERROR).small());
            }
        });

    // 帧末写回持久化状态
    ctx.data_mut(|d| d.insert_temp(Id::new("log_path"), file_path));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_source"), source));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_raw"), raw_value));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_scaled"), scaled_value));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_unit"), unit));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_count"), log_count));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_flush"), flush_threshold));
    ctx.data_mut(|d| d.insert_temp(Id::new("log_logger"), logger_slot));
}
