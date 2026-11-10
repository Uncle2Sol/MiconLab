// 终端收发面板
// 上方 RX 显示区（深色背景、等宽字体），下方控制栏 + 输入区
// 顶部集成 AI 异常检测告警条（从 AI 助手面板共享读取）

use crate::i18n::L10n;
use crate::state::{AppState, Direction, LineEnding};
use crate::theme;
use egui::{Color32, Id, RichText, ScrollArea, Vec2};

/// 渲染终端面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == crate::i18n::Language::Zh;

    // 读取 AI 助手面板共享的最近异常告警（如果有）
    let shared_anomaly: Option<miconlab_ai::AnomalyAlert> = ctx
        .data_mut(|d| d.get_temp::<Option<miconlab_ai::AnomalyAlert>>(Id::new("ai_shared_anomaly")))
        .unwrap_or(None);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(8.0))
                .fill(theme::BG_SURFACE),
        )
        .show(ctx, |ui| {
            // 副标题
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Serial terminal - send/receive text or HEX data, with auto-wrap, timestamps, and loop send"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            });
            ui.add_space(2.0);

            // AI 感知：协议识别徽章 + 波特率不匹配检测（节流到每 500ms 一次）
            let now = std::time::Instant::now();
            let should_sense = ctx.data(|d| {
                d.get_temp::<Option<std::time::Instant>>(egui::Id::new("terminal_ai_sense_last"))
                    .flatten()
                    .map_or(true, |t| now.duration_since(t) > std::time::Duration::from_millis(500))
            });
            if should_sense {
                ctx.data_mut(|d| {
                    d.insert_temp(egui::Id::new("terminal_ai_sense_last"), Some(now));
                });
                let rx_data = crate::panels::ai_sense::collect_recent_rx(&state.rx_buffer, 256);
                if let Some((class, conf, hint)) = crate::panels::ai_sense::sense_protocol(&rx_data) {
                    ui.horizontal(|ui| {
                        crate::panels::ai_sense::render_protocol_badge(ui, class, conf);
                    });
                    ui.add_space(2.0);
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(2.0);
                }
                if let Some(hint) = crate::panels::ai_sense::sense_baud_mismatch(&rx_data, state.config.baud_rate, zh) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(2.0);
                }
                // AI 感知：二进制数据检测，建议切换 HEX 模式
                if !state.hex_mode {
                    let non_printable = rx_data.iter().filter(|&&b| !(0x20..=0x7E).contains(&b) && b != b'\n' && b != b'\r' && b != b'\t').count();
                    let ratio = non_printable as f32 / rx_data.len().max(1) as f32;
                    if ratio > 0.3 {
                        let hint = crate::panels::ai_sense::AiHint::new(
                            "🔢",
                            theme::CYAN,
                            crate::i18n::tf("Binary data detected ({:.0}% non-printable), switch to HEX mode", &[("{:.0}", "{:.0}", &format!("{:.0}", ratio * 100.0))]),
                        );
                        crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                        ui.add_space(2.0);
                    }
                }
            }

            // AI 异常检测告警条（如果有最近告警）
            if let Some(alert) = &shared_anomaly {
                render_anomaly_banner(ui, alert, ctx);
                ui.add_space(2.0);
            }

            // 上下分栏：上方 70% 显示区，下方 30% 控制栏
            egui::TopBottomPanel::bottom("terminal_bottom")
                .resizable(true)
                .default_height(ui.available_height() * 0.32)
                .min_height(80.0)
                .frame(
                    egui::Frame::group(&ctx.style())
                        .fill(theme::BG_ELEVATED)
                        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                        .inner_margin(egui::Margin::same(6.0)),
                )
                .show_inside(ui, |ui| {
                    render_bottom_bar(ui, state, &l10n, zh);
                });

            // 上方 RX 显示区
            render_rx_area(ui, state, &l10n);
        });
}

/// 渲染 AI 异常检测告警条
fn render_anomaly_banner(ui: &mut egui::Ui, alert: &miconlab_ai::AnomalyAlert, ctx: &egui::Context) {
    use miconlab_ai::AnomalyKind;

    let (icon, color) = match alert.kind {
        AnomalyKind::EntropySpike | AnomalyKind::EntropyDrop => ("📈", theme::WARNING),
        AnomalyKind::DistributionShift => ("🔀", theme::WARNING),
        AnomalyKind::TimingAnomaly => ("⏱", theme::WARNING),
        AnomalyKind::NullBurst => ("⭕", theme::ERROR),
        AnomalyKind::HighByteBurst => ("🔢", theme::WARNING),
    };
    let kind_label = crate::i18n::t_owned(alert.kind.label_en());

    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, color))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(14.0));
                ui.label(RichText::new(format!("AI {}", kind_label)).color(color).strong().small());
                ui.label(
                    RichText::new(&alert.message)
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 跳转到 AI 助手面板的按钮
                    if ui
                        .button(crate::i18n::t("Details"))
                        .clicked()
                    {
                        // 切换到 AI 助手面板（通过设置 state 的 active_panel）
                        // 注意：这里 ui 没有 &mut AppState，需要通过 ctx 传递信号
                        // 简单实现：写入一个共享 flag，app.rs 读取后切换
                        ctx.data_mut(|d| {
                            *d.get_temp_mut_or_default::<bool>(Id::new("ai_navigate_to_assistant")) = true;
                        });
                    }
                    // 关闭告警条
                    if ui.button("✕").clicked() {
                        ctx.data_mut(|d| {
                            *d.get_temp_mut_or_default::<Option<miconlab_ai::AnomalyAlert>>(Id::new("ai_shared_anomaly")) = None;
                        });
                    }
                });
            });
        });
}

/// 渲染 RX 显示区（上方 70%）
fn render_rx_area(ui: &mut egui::Ui, state: &AppState, l10n: &L10n) {
    egui::Frame::group(ui.style())
        .fill(theme::BG_DEEP)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            // 顶部信息栏：标题 + 字节计数
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} / {}",
                        l10n.t("terminal.rx"),
                        l10n.t("terminal.tx")
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}: {}  {}: {}",
                            l10n.t("status.bytes_tx"),
                            state.bytes_tx,
                            l10n.t("status.bytes_rx"),
                            state.bytes_rx
                        ))
                        .color(theme::TEXT_SECONDARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                    );
                });
            });

            ui.separator();

            // 数据显示区
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(state.auto_scroll)
                .show(ui, |ui| {
                    if state.rx_buffer.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(40.0);
                            ui.label(
                                RichText::new(if state.paused {
                                    crate::i18n::t("Paused — click Resume to continue")
                                } else {
                                    crate::i18n::t("Waiting for data...")
                                })
                                .color(theme::TEXT_DISABLED)
                                .family(egui::FontFamily::Monospace),
                            );
                        });
                    } else {
                        // 只渲染最后 500 条，避免大量数据时渲染卡顿
                        const MAX_RENDER: usize = 500;
                        let total = state.rx_buffer.len();
                        let start = total.saturating_sub(MAX_RENDER);
                        if start > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "... {} entries omitted ...",
                                    start
                                ))
                                .color(theme::TEXT_DISABLED)
                                .small()
                                .family(egui::FontFamily::Monospace),
                            );
                        }
                        for entry in &state.rx_buffer[start..] {
                            render_entry(ui, entry, state.show_timestamp, state.hex_mode);
                        }
                    }
                });
        });
}

/// 渲染单条数据条目
/// 安全文本渲染：将二进制数据中的非可打印字符替换为 '.'
///
/// 保留 \t \n \r 和所有可打印 ASCII (0x20-0x7E)，
/// 有效 UTF-8 多字节字符也保留，无效字节替换为 '.'。
/// 这样二进制协议数据不会显示为方块乱码。
fn render_safe_text(data: &[u8]) -> String {
    let mut result = String::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        // 常见控制字符保留
        if b == b'\t' || b == b'\n' || b == b'\r' {
            result.push(b as char);
            i += 1;
            continue;
        }
        // 可打印 ASCII
        if (0x20..=0x7E).contains(&b) {
            result.push(b as char);
            i += 1;
            continue;
        }
        // 尝试 UTF-8 多字节解码（0xC0-0xFD 开头）
        if b >= 0xC0 && i + 1 < data.len() {
            // 判断 UTF-8 序列长度
            let seq_len = if b >= 0xF0 { 4 }
                else if b >= 0xE0 { 3 }
                else { 2 };
            if i + seq_len <= data.len() {
                if let Ok(s) = std::str::from_utf8(&data[i..i + seq_len]) {
                    result.push_str(s);
                    i += seq_len;
                    continue;
                }
            }
        }
        // 其他字节：替换为 '.'
        result.push('.');
        i += 1;
    }
    result
}

fn render_entry(
    ui: &mut egui::Ui,
    entry: &crate::state::RxEntry,
    show_timestamp: bool,
    hex_mode: bool,
) {
    let (color, prefix) = match entry.direction {
        Direction::Rx => (theme::RX_COLOR, ""),
        Direction::Tx => (theme::TX_COLOR, "> "),
    };

    ui.horizontal_wrapped(|ui| {
        // 时间戳
        if show_timestamp {
            ui.label(
                RichText::new(format!(
                    "[{}] ",
                    entry.timestamp.format("%H:%M:%S%.3f")
                ))
                .color(theme::TEXT_SECONDARY)
                .family(egui::FontFamily::Monospace),
            );
        }

        // 方向前缀（TX 用 > 标记）
        if !prefix.is_empty() {
            ui.label(
                RichText::new(prefix)
                    .color(color)
                    .family(egui::FontFamily::Monospace),
            );
        }

        // 数据内容
        let text = if hex_mode {
            crate::state::format_hex(&entry.data)
        } else {
            // 安全渲染：将非可打印字符替换为 '.'，避免方块乱码
            // 保留 \t \n \r，其他控制字符和非 UTF-8 字节替换为 '.'
            render_safe_text(&entry.data)
        };

        ui.label(
            RichText::new(text)
                .color(color)
                .family(egui::FontFamily::Monospace),
        );
    });
}

/// 渲染底部控制栏（下方 30%）
fn render_bottom_bar(ui: &mut egui::Ui, state: &mut AppState, l10n: &L10n, _zh: bool) {
    ui.vertical(|ui| {
        // 工具栏
        ui.horizontal(|ui| {
            // 暂停/恢复接收
            let (pause_label, pause_color) = if state.paused {
                (
                    crate::i18n::t("▶ Resume"),
                    theme::SUCCESS,
                )
            } else {
                (
                    crate::i18n::t("⏸ Pause"),
                    theme::WARNING,
                )
            };
            let pause_btn = egui::Button::new(
                RichText::new(pause_label).color(Color32::BLACK).strong(),
            )
            .fill(pause_color)
            .min_size(Vec2::new(72.0, 22.0))
            .stroke(egui::Stroke::NONE);
            if ui.add(pause_btn)
                .on_hover_text(crate::i18n::t("Pause display (data still flows into the buffer and becomes visible after resuming)"))
                .clicked()
            {
                state.paused = !state.paused;
            }

            ui.separator();

            // HEX 模式切换
            let hex_btn = egui::SelectableLabel::new(state.hex_mode, l10n.t("terminal.hex_mode"));
            if ui
                .add(hex_btn)
                .on_hover_text(crate::i18n::t("Toggle HEX/text display. HEX mode shows bytes as space-separated hex pairs for binary protocol inspection; text mode shows ASCII"))
                .clicked()
            {
                state.hex_mode = !state.hex_mode;
            }

            ui.separator();

            // 时间戳切换
            let ts_btn =
                egui::SelectableLabel::new(state.show_timestamp, l10n.t("terminal.timestamp"));
            if ui.add(ts_btn)
                .on_hover_text(crate::i18n::t("Prefix each line with a timestamp (HH:MM:SS.mmm)"))
                .clicked()
            {
                state.show_timestamp = !state.show_timestamp;
            }

            // 自动滚动切换
            let as_btn =
                egui::SelectableLabel::new(state.auto_scroll, l10n.t("terminal.autoscroll"));
            if ui.add(as_btn)
                .on_hover_text(crate::i18n::t("When disabled, you can scroll manually to view history"))
                .clicked()
            {
                state.auto_scroll = !state.auto_scroll;
            }

            ui.separator();

            // DTR/RTS 硬件信号控制（仅连接时可用）
            // DTR 通常连接 RESET，RTS 通常连接 BOOT0/IO0
            // 点击按钮在 0/1 之间切换，并立即应用到串口
            if state.is_connected() {
                let dtr_label = if state.dtr_enabled { "DTR:1" } else { "DTR:0" };
                let dtr_clicked = ui.add(
                    egui::Button::new(
                        RichText::new(dtr_label).color(Color32::BLACK).strong(),
                    )
                    .fill(if state.dtr_enabled {
                        egui::Color32::from_rgb(60, 120, 60)
                    } else {
                        egui::Color32::from_rgb(60, 60, 60)
                    })
                    .min_size(Vec2::new(56.0, 22.0)),
                )
                .on_hover_text(crate::i18n::t("DTR (Data Terminal Ready) signal. On some boards DTR connects to RESET, pulling low resets the device"))
                .clicked();
                if dtr_clicked {
                    state.dtr_enabled = !state.dtr_enabled;
                    state.set_dtr(state.dtr_enabled);
                }

                let rts_label = if state.rts_enabled { "RTS:1" } else { "RTS:0" };
                let rts_clicked = ui.add(
                    egui::Button::new(
                        RichText::new(rts_label).color(Color32::BLACK).strong(),
                    )
                    .fill(if state.rts_enabled {
                        egui::Color32::from_rgb(60, 120, 60)
                    } else {
                        egui::Color32::from_rgb(60, 60, 60)
                    })
                    .min_size(Vec2::new(56.0, 22.0)),
                )
                .on_hover_text(crate::i18n::t("RTS (Request To Send) signal. On some boards RTS connects to BOOT0, pulling high enters Bootloader mode"))
                .clicked();
                if rts_clicked {
                    state.rts_enabled = !state.rts_enabled;
                    state.set_rts(state.rts_enabled);
                }

                ui.separator();
            }
            ui.label(
                RichText::new(format!("{}:", l10n.t("terminal.line_ending")))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            let line_endings = [LineEnding::None, LineEnding::CR, LineEnding::LF, LineEnding::CRLF];
            egui::ComboBox::from_id_salt("line_ending_select")
                .selected_text(state.line_ending.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for le in line_endings {
                        ui.selectable_value(&mut state.line_ending, le, le.label());
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("Line ending appended on send: None, CR=\\r (old Mac), LF=\\n (Unix/Linux), CRLF=\\r\\n (Windows/Modbus RTU)"));

            ui.separator();

            // 清空按钮
            if ui
                .button(RichText::new(l10n.t("terminal.clear")).color(theme::TEXT_SECONDARY))
                .on_hover_text(crate::i18n::t("Clear the display buffer (does not affect the underlying receive buffer)"))
                .clicked()
            {
                state.clear_terminal();
            }
        });

        ui.add_space(4.0);

        // 输入区 + 发送按钮
        ui.horizontal(|ui| {
            // 多行文本输入框
            let input_response = ui.add_sized(
                Vec2::new(ui.available_width() - 90.0, 60.0),
                egui::TextEdit::multiline(&mut state.tx_input)
                    .font(egui::FontId::monospace(12.5))
                    .desired_width(f32::MAX)
                    .hint_text(l10n.t("terminal.input_placeholder"))
                    .frame(true),
            );

            // 发送后重新聚焦
            if state.terminal_input_focused {
                input_response.request_focus();
                state.terminal_input_focused = false;
            }

            // Ctrl+Enter 发送
            if input_response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl)
            {
                state.send_tx();
            }

            // 发送按钮
            let send_btn = egui::Button::new(
                RichText::new(l10n.t("terminal.send")).color(Color32::BLACK).strong(),
            )
            .fill(theme::ACCENT)
            .min_size(Vec2::new(80.0, 60.0))
            .stroke(egui::Stroke::NONE);

            if ui
                .add(send_btn)
                .on_hover_text(crate::i18n::t("Send the input box content to the serial port (selected line ending is auto-appended). Ctrl+Enter also sends"))
                .clicked()
            {
                state.send_tx();
            }
        });

        // 错误提示
        if let Some(err) = &state.last_error {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("\u{26A0} {}", err))
                    .color(theme::ERROR)
                    .small(),
            );
        }
    });
}
