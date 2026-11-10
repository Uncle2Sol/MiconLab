// 串口示波器面板：配置 + 数字波形渲染 + 统计 + 采集控制
// SerialScope 非 Clone（持有 Vec<SamplePoint>），用 Arc<Mutex<SerialScope>> 包装存入 IdTypeMap。
// 借阅检查规避：在 .show 闭包前把所有持久化状态取到 owned 局部，闭包内只读/操作局部，帧末统一写回。

use crate::i18n::L10n;
use crate::state::AppState;
use crate::theme;
use egui::{Color32, Id, Pos2, RichText, Stroke};
use miconlab_core::protocol::{ScopeConfig, ScopeStats, ScopeStopBits, SerialScope};
use std::sync::{Arc, Mutex};

/// 示波器实例存储类型（Arc<Mutex<T>> 是 Clone + Send + Sync，可存入 IdTypeMap）
type ScopeSlot = Arc<Mutex<SerialScope>>;

/// 渲染串口示波器面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == crate::i18n::Language::Zh;

    // ===== 取出持久化的 Clone 字段 =====
    let mut capturing: bool = ctx
        .data_mut(|d| d.get_temp::<bool>(Id::new("scope_capturing")))
        .unwrap_or(false);
    let mut cfg_baud: u32 = ctx
        .data_mut(|d| d.get_temp::<u32>(Id::new("scope_baud")))
        .unwrap_or(115200);
    let mut cfg_data_bits: u8 = ctx
        .data_mut(|d| d.get_temp::<u8>(Id::new("scope_db")))
        .unwrap_or(8);
    let mut cfg_stop: ScopeStopBits = ctx
        .data_mut(|d| d.get_temp::<ScopeStopBits>(Id::new("scope_stop")))
        .unwrap_or(ScopeStopBits::One);
    let mut cfg_parity: bool = ctx
        .data_mut(|d| d.get_temp::<bool>(Id::new("scope_par")))
        .unwrap_or(false);
    let mut cfg_sample: u32 = ctx
        .data_mut(|d| d.get_temp::<u32>(Id::new("scope_sample")))
        .unwrap_or(1);
    let mut last_bytes_rx: u64 = ctx
        .data_mut(|d| d.get_temp::<u64>(Id::new("scope_last_rx")))
        .unwrap_or(0);

    // 取出（或创建）示波器实例
    let scope_slot: ScopeSlot = ctx
        .data_mut(|d| d.get_temp::<ScopeSlot>(Id::new("scope_slot")))
        .unwrap_or_else(|| Arc::new(Mutex::new(SerialScope::new())));

    // 连接会话变化时清除旧设备的波形数据，避免跨设备混合显示
    let last_session_id: u64 = ctx
        .data_mut(|d| d.get_temp::<u64>(Id::new("scope_last_session_id")))
        .unwrap_or(0);
    if last_session_id != state.connection_session_id {
        // 重置 scope 内部缓冲
        let cfg = scope_slot.lock().unwrap_or_else(|e| e.into_inner()).config().clone();
        *scope_slot.lock().unwrap_or_else(|e| e.into_inner()) = SerialScope::with_config(cfg);
        last_bytes_rx = state.bytes_rx;
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("scope_last_session_id"), state.connection_session_id);
        });
    }

    // ===== 配置变更检测：若任一参数变化则用新配置重建 scope（同时清空旧数据）=====
    let needs_rebuild = {
        let guard = scope_slot.lock().unwrap_or_else(|e| e.into_inner());
        let c = guard.config();
        c.baud_rate != cfg_baud
            || c.data_bits != cfg_data_bits
            || c.stop_bits != cfg_stop
            || c.parity != cfg_parity
            || c.sample_multiplier != cfg_sample
    };
    if needs_rebuild {
        let new_cfg = ScopeConfig {
            baud_rate: cfg_baud,
            data_bits: cfg_data_bits,
            stop_bits: cfg_stop,
            parity: cfg_parity,
            sample_multiplier: cfg_sample,
        };
        *scope_slot.lock().unwrap_or_else(|e| e.into_inner()) = SerialScope::with_config(new_cfg);
    }

    // ===== 喂入新数据（基于 bytes_rx 增量去重，避免重复/无界增长）=====
    if capturing && state.bytes_rx != last_bytes_rx {
        // 计算自上次以来新增的字节数
        let new_count = state.bytes_rx.saturating_sub(last_bytes_rx) as usize;
        // 从 rx_buffer 末尾向前收集恰好 new_count 个字节。
        // 段级收集后对「段列表」reverse；不能对拼接后的 flat 数组 reverse，
        // 那会翻转每段内部的字节序。
        let mut segments: Vec<&[u8]> = Vec::new();
        let mut collected = 0usize;
        for entry in state.rx_buffer.iter().rev() {
            if collected >= new_count {
                break;
            }
            // 只收集 RX 数据：TX 条目也存在于 rx_buffer 且 bytes_rx 只统计 RX，
            // 混入 TX 会把自己发送的字节画成波形、并丢失等量真实 RX 数据
            if entry.direction != crate::state::Direction::Rx {
                continue;
            }
            let needed = new_count - collected;
            if entry.data.len() <= needed {
                segments.push(&entry.data);
                collected += entry.data.len();
            } else {
                // 只取该条目的最后 needed 个字节
                let start = entry.data.len() - needed;
                segments.push(&entry.data[start..]);
                collected += needed;
            }
        }
        segments.reverse(); // 恢复时间顺序
        let new_bytes: Vec<u8> = segments.concat();
        if !new_bytes.is_empty() {
            // 以当前已捕获的总时长作为新数据的起始时间，保证时间轴连续
            let start_time = scope_slot.lock().unwrap_or_else(|e| e.into_inner()).stats().total_duration_us;
            scope_slot
                .lock()
                .unwrap()
                .process_raw_data(&new_bytes, start_time);
        }
        last_bytes_rx = state.bytes_rx;
    }

    // ===== 预取出统计与波形点为 owned 局部，避免在 UI 闭包内持锁 =====
    let (stats, points): (ScopeStats, Vec<(f64, bool)>) = {
        let guard = scope_slot.lock().unwrap_or_else(|e| e.into_inner());
        (guard.stats(), guard.to_waveform_points())
    };

    // ===== 渲染 =====
    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            ui.heading(
                RichText::new(l10n.t("panel.scope"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            // 副标题：解释面板用途
            ui.label(
                RichText::new(crate::i18n::t("Capture serial bytes in real time, estimate bit periods from baud rate, render digital waveform and level stats"))
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

            // ===== 配置区 =====
            ui.label(
                RichText::new(crate::i18n::t("Capture Config"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(6.0);
            egui::Grid::new("scope_cfg_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(l10n.t("connect.baud")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("scope_baud_select")
                        .selected_text(format!("{}", cfg_baud))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for &b in &[
                                1200u32, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400,
                                460800, 921600,
                            ] {
                                ui.selectable_value(&mut cfg_baud, b, format!("{}", b));
                            }
                        })
                        .response
                        .on_hover_text(crate::i18n::t("Used to derive the bit period (1/baud seconds); affects waveform rendering accuracy"));
                    ui.end_row();

                    ui.label(
                        RichText::new(l10n.t("connect.data_bits")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("scope_db_select")
                        .selected_text(format!("{}", cfg_data_bits))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for d in 5u8..=8 {
                                ui.selectable_value(&mut cfg_data_bits, d, format!("{}", d));
                            }
                        })
                        .response
                        .on_hover_text(crate::i18n::t("5/6/7/8 bits; sets the number of data bits per frame"));
                    ui.end_row();

                    ui.label(
                        RichText::new(l10n.t("connect.stop_bits")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("scope_stop_select")
                        .selected_text(stop_label(cfg_stop))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg_stop, ScopeStopBits::One, "1");
                            ui.selectable_value(
                                &mut cfg_stop,
                                ScopeStopBits::OnePointFive,
                                "1.5",
                            );
                            ui.selectable_value(&mut cfg_stop, ScopeStopBits::Two, "2");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("1/1.5/2 stop bit durations"));
                    ui.end_row();

                    ui.label(
                        RichText::new(l10n.t("connect.parity")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("scope_par_select")
                        .selected_text(if cfg_parity { "Odd" } else { "None" })
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg_parity, false, "None");
                            ui.selectable_value(&mut cfg_parity, true, "Odd");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("None/Even/Odd/Mark/Space; affects the parity bit waveform"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Sample Mult."))
                            .color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("scope_sample_select")
                        .selected_text(format!("{}x", cfg_sample))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for s in [1u32, 2, 4, 8, 16] {
                                ui.selectable_value(&mut cfg_sample, s, format!("{}x", s));
                            }
                        })
                        .response
                        .on_hover_text(crate::i18n::t("Samples per bit (1/4/8/16); higher values give finer waveforms"));
                    ui.end_row();
                });

            ui.add_space(8.0);
            // 采集 / 停止 / 清空 按钮
            ui.horizontal(|ui| {
                let (btn_text, btn_color) = if capturing {
                    (
                        { crate::i18n::t("Stop##panels/scope.rs:10789") }.to_string(),
                        theme::ERROR,
                    )
                } else {
                    (
                        { crate::i18n::t("Capture##panels/scope.rs:10966") }.to_string(),
                        theme::SUCCESS,
                    )
                };
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(btn_text).color(Color32::BLACK).strong(),
                        )
                        .fill(btn_color)
                        .min_size(egui::Vec2::new(130.0, 30.0))
                        .stroke(egui::Stroke::NONE),
                    )
                    .on_hover_text(crate::i18n::t("Start/Stop capturing serial data. Bit periods are estimated from current config during capture"))
                    .clicked()
                {
                    capturing = !capturing;
                    if capturing {
                        // 开始采集时以当前 bytes_rx 为基准，避免回填历史数据
                        last_bytes_rx = state.bytes_rx;
                    }
                }

                ui.add_space(8.0);
                if ui
                    .button(RichText::new(crate::i18n::t("Clear")).strong())
                    .on_hover_text(crate::i18n::t("Clear captured waveform data and statistics"))
                    .clicked()
                {
                    scope_slot.lock().unwrap_or_else(|e| e.into_inner()).clear();
                }

                ui.add_space(12.0);
                // 采集状态指示
                let (dot, dot_color) = if capturing {
                    ("\u{25CF}", theme::SUCCESS)
                } else {
                    ("\u{25CB}", theme::TEXT_DISABLED)
                };
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        dot,
                        if capturing {
                            crate::i18n::t("Capturing")
                        } else {
                            crate::i18n::t("Stopped")
                        }
                    ))
                    .color(dot_color)
                    .small(),
                );
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 波形显示区 =====
            ui.label(
                RichText::new(crate::i18n::t("Waveform"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);

            let avail = ui.available_size();
            let (resp, painter) =
                ui.allocate_painter(avail.min([f32::MAX, 240.0].into()), egui::Sense::hover());
            let rect = resp.rect;

            // 背景填充
            painter.rect_filled(rect, 0.0, theme::BG_DEEP);

            // 中线参考
            painter.line_segment(
                [
                    Pos2::new(rect.center().x, rect.top()),
                    Pos2::new(rect.center().x, rect.bottom()),
                ],
                Stroke::new(0.5_f32, theme::BORDER),
            );

            // 高/低电平参考线（虚线效果用细线模拟）
            let y_high = rect.top() + rect.height() * 0.2;
            let y_low = rect.top() + rect.height() * 0.8;
            painter.line_segment(
                [
                    Pos2::new(rect.left(), y_high),
                    Pos2::new(rect.right(), y_high),
                ],
                Stroke::new(0.5_f32, theme::BORDER),
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left(), y_low),
                    Pos2::new(rect.right(), y_low),
                ],
                Stroke::new(0.5_f32, theme::BORDER),
            );

            // 电平标签
            painter.text(
                Pos2::new(rect.left() + 4.0, y_high),
                egui::Align2::LEFT_CENTER,
                "H",
                egui::FontId::new(10.0, egui::FontFamily::Monospace),
                theme::TEXT_SECONDARY,
            );
            painter.text(
                Pos2::new(rect.left() + 4.0, y_low),
                egui::Align2::LEFT_CENTER,
                "L",
                egui::FontId::new(10.0, egui::FontFamily::Monospace),
                theme::TEXT_SECONDARY,
            );

            // 数字波形：水平保持 + 垂直过渡
            if !points.is_empty() {
                let t_min = points.first().map(|p| p.0).unwrap_or(0.0);
                let t_max_raw = points.last().map(|p| p.0).unwrap_or(1.0);
                let t_max = t_max_raw.max(t_min + 1.0);
                let t_range = t_max - t_min;
                let x_for = |t: f64| -> f32 {
                    rect.left() + ((t - t_min) / t_range) as f32 * rect.width()
                };
                let y_for = |level: bool| -> f32 { if level { y_high } else { y_low } };

                let first = &points[0];
                let mut prev_x = x_for(first.0);
                let mut prev_y = y_for(first.1);
                // 起点标记
                painter.circle_filled(Pos2::new(prev_x, prev_y), 1.5, theme::CYAN);

                for w in points.windows(2) {
                    let (_t0, l0) = w[0];
                    let (t1, _l1) = w[1];
                    let x1 = x_for(t1);
                    let y0 = y_for(l0);
                    // 水平保持段：从 prev_x 推进到 x1，保持上一电平 l0
                    painter.line_segment(
                        [Pos2::new(prev_x, prev_y), Pos2::new(x1, y0)],
                        Stroke::new(1.5_f32, theme::CYAN),
                    );
                    // 垂直过渡段：电平变化时在 x1 处垂直跳变
                    let y1 = y_for(w[1].1);
                    if (y0 - y1).abs() > 0.01 {
                        painter.line_segment(
                            [Pos2::new(x1, y0), Pos2::new(x1, y1)],
                            Stroke::new(1.5_f32, theme::CYAN),
                        );
                    }
                    prev_x = x1;
                    prev_y = y1;
                }

                // 末端标记
                painter.circle_filled(Pos2::new(prev_x, prev_y), 1.5, theme::ACCENT);
            } else {
                // 空状态提示
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    crate::i18n::t("No data (click Capture)"),
                    egui::FontId::new(13.0, egui::FontFamily::Proportional),
                    theme::TEXT_DISABLED,
                );
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 统计区 =====
            ui.label(
                RichText::new(crate::i18n::t("Statistics"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            egui::Grid::new("scope_stats_grid")
                .num_columns(2)
                .spacing([16.0, 3.0])
                .striped(true)
                .show(ui, |ui| {
                    stat_label(
                        ui,
                        crate::i18n::t("Total Bytes"),
                        format!("{}", stats.total_bytes),
                    );
                    stat_label(
                        ui,
                        crate::i18n::t("Avg Bit Period (μs)"),
                        format!("{:.2}", stats.avg_bit_period_us),
                    );
                    stat_label(
                        ui,
                        crate::i18n::t("Min Bit Period (μs)"),
                        format!("{:.2}", stats.min_bit_period_us),
                    );
                    stat_label(
                        ui,
                        crate::i18n::t("Max Bit Period (μs)"),
                        format!("{:.2}", stats.max_bit_period_us),
                    );
                    stat_label(
                        ui,
                        crate::i18n::t("Framing Errors"),
                        format!("{}", stats.framing_errors),
                    );
                    stat_label(
                        ui,
                        crate::i18n::t("Total Duration (μs)"),
                        format!("{:.1}", stats.total_duration_us),
                    );
                });

            // 错误提示
            if let Some(err) = &state.last_error {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("\u{26A0} {}", err)).color(theme::ERROR).small(),
                );
            }

            // ===== D.5 协议解码区 =====
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            render_protocol_decode(ui, state, zh);
        });

    // ===== 帧末写回持久化状态 =====
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_capturing"), capturing));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_baud"), cfg_baud));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_db"), cfg_data_bits));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_stop"), cfg_stop));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_par"), cfg_parity));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_sample"), cfg_sample));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_last_rx"), last_bytes_rx));
    ctx.data_mut(|d| d.insert_temp(Id::new("scope_slot"), scope_slot));
}

/// 统计行：标签 + 值
fn stat_label(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(RichText::new(label).color(theme::TEXT_SECONDARY).small());
    ui.label(
        RichText::new(value)
            .color(theme::TEXT_PRIMARY)
            .family(egui::FontFamily::Monospace),
    );
    ui.end_row();
}

/// 停止位显示标签
fn stop_label(s: ScopeStopBits) -> &'static str {
    match s {
        ScopeStopBits::One => "1",
        ScopeStopBits::OnePointFive => "1.5",
        ScopeStopBits::Two => "2",
    }
}

// ============================================================================
// D.5 协议解码区（软件逻辑分析仪）
// ============================================================================

/// 渲染协议解码区：TX/RX 时序轴 + 解码帧列表 + 命令-响应映射表
fn render_protocol_decode(ui: &mut egui::Ui, state: &AppState, zh: bool) {
    ui.heading(
        RichText::new(crate::i18n::t("Protocol Decode (SW Logic Analyzer)"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.label(
        RichText::new(crate::i18n::t("Analyze TX/RX timing, decode protocol frames and build command-response map. Replaces DSLogic hardware."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    ui.add_space(6.0);

    // 已注册的扫描器
    let scanner_names = state.logic_analyzer.scanner_names();
    if !scanner_names.is_empty() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(crate::i18n::t("Scanners:"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            for name in &scanner_names {
                ui.label(
                    RichText::new(format!("[{}]", name))
                        .color(theme::ACCENT)
                        .small()
                        .monospace(),
                );
            }
        });
    }

    ui.add_space(8.0);

    // 扫描所有协议帧
    let frames = state.logic_analyzer.scan_all();
    // 关联命令-响应
    // 注意：correlate 需要 &mut，但 state 是 &AppState（只读）
    // 这里用 scan_all 的结果手动展示，correlate 在需要时通过 state 的克隆进行
    // 为避免破坏 state 不可变性，此处仅展示帧列表和时序轴

    // ===== TX/RX 时序轴 =====
    render_timeline_axis(ui, state, zh);
    ui.add_space(8.0);

    // ===== 解码帧列表 =====
    render_decoded_frames(ui, &frames, zh);
    ui.add_space(8.0);

    // ===== 命令-响应映射表（简化版：从帧列表推断） =====
    render_command_response_map(ui, &frames, zh);
}

/// 渲染 TX/RX 时序轴
fn render_timeline_axis(ui: &mut egui::Ui, state: &AppState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("TX/RX Timeline"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(2.0);

    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        avail.min([f32::MAX, 120.0].into()),
        egui::Sense::hover(),
    );
    let rect = resp.rect;

    // 背景
    painter.rect_filled(rect, 0.0, theme::BG_DEEP);

    // 上轨 TX（蓝），下轨 RX（橙）
    let tx_y = rect.top() + rect.height() * 0.25;
    let rx_y = rect.top() + rect.height() * 0.75;

    // 轨道标签
    painter.text(
        Pos2::new(rect.left() + 4.0, tx_y),
        egui::Align2::LEFT_CENTER,
        "TX",
        egui::FontId::new(10.0, egui::FontFamily::Monospace),
        Color32::from_rgb(100, 180, 255),
    );
    painter.text(
        Pos2::new(rect.left() + 4.0, rx_y),
        egui::Align2::LEFT_CENTER,
        "RX",
        egui::FontId::new(10.0, egui::FontFamily::Monospace),
        Color32::from_rgb(255, 180, 100),
    );

    // 轨道分隔线
    painter.line_segment(
        [Pos2::new(rect.left() + 30.0, tx_y), Pos2::new(rect.right(), tx_y)],
        Stroke::new(0.5_f32, Color32::from_rgba_unmultiplied(100, 180, 255, 80)),
    );
    painter.line_segment(
        [Pos2::new(rect.left() + 30.0, rx_y), Pos2::new(rect.right(), rx_y)],
        Stroke::new(0.5_f32, Color32::from_rgba_unmultiplied(255, 180, 100, 80)),
    );

    // 获取 timeline 中的最近 N 个字节用于渲染
    // 由于 state.logic_analyzer 的 timeline 是私有的，我们通过 scan_all 的帧来推断时序
    // 如果有帧数据，用帧的时间戳绘制脉冲
    let frames = state.logic_analyzer.scan_all();
    if frames.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            crate::i18n::t("No data"),
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            theme::TEXT_DISABLED,
        );
        return;
    }

    // 找时间范围
    let t_min = frames.first().map(|f| f.start_us).unwrap_or(0);
    let t_max_raw = frames.last().map(|f| f.end_us).unwrap_or(t_min + 1);
    let t_max = t_max_raw.max(t_min + 1);
    let t_range = (t_max - t_max_raw) as f32 + (t_max - t_min) as f32;
    let t_range = if t_range > 0.0 { t_range } else { 1.0 };

    let x_for = |t: u64| -> f32 {
        rect.left() + 30.0 + ((t - t_min) as f32 / t_range) * (rect.width() - 30.0)
    };

    // 绘制帧脉冲
    for frame in &frames {
        let x_start = x_for(frame.start_us);
        let x_end = x_for(frame.end_us).max(x_start + 2.0);
        let y = if frame.direction == miconlab_core::protocol::logic_analyzer::Direction::Tx {
            tx_y
        } else {
            rx_y
        };
        let color = if frame.direction == miconlab_core::protocol::logic_analyzer::Direction::Tx {
            Color32::from_rgb(100, 180, 255)
        } else {
            Color32::from_rgb(255, 180, 100)
        };

        // 脉冲条
        painter.line_segment(
            [Pos2::new(x_start, y - 8.0), Pos2::new(x_end, y - 8.0)],
            Stroke::new(2.0_f32, color),
        );
        // 帧标签（仅显示前 8 字节 hex）
        if x_end - x_start > 30.0 {
            let preview: String = frame.data.iter().take(4).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
            painter.text(
                Pos2::new((x_start + x_end) / 2.0, y - 16.0),
                egui::Align2::CENTER_CENTER,
                &preview,
                egui::FontId::new(8.0, egui::FontFamily::Monospace),
                color,
            );
        }
    }
}

/// 渲染解码帧列表
fn render_decoded_frames(
    ui: &mut egui::Ui,
    frames: &[miconlab_core::protocol::logic_analyzer::DecodedFrame],
    _zh: bool,
) {
    ui.label(
        RichText::new(crate::i18n::t("Decoded Frames"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(2.0);

    if frames.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No protocol frames detected (capture TX/RX data first, data must match registered scanners)"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        return;
    }

    // 限制显示前 50 帧
    let display_count = frames.len().min(50);
    egui::ScrollArea::vertical()
        .max_height(200.0)
        .show(ui, |ui| {
            egui::Grid::new("decoded_frames_grid")
                .striped(true)
                .min_col_width(60.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(crate::i18n::t("Dir")).strong());
                    ui.label(RichText::new(crate::i18n::t("Proto")).strong());
                    ui.label(RichText::new(crate::i18n::t("Start(μs)")).strong());
                    ui.label(RichText::new(crate::i18n::t("Dur(μs)")).strong());
                    ui.label(RichText::new(crate::i18n::t("Len")).strong());
                    ui.label(RichText::new(crate::i18n::t("Data(first 16B)")).strong());
                    ui.label(RichText::new(crate::i18n::t("Summary")).strong());
                    ui.end_row();

                    for frame in frames.iter().take(display_count) {
                        let dir_text = if frame.direction == miconlab_core::protocol::logic_analyzer::Direction::Tx {
                            "TX"
                        } else {
                            "RX"
                        };
                        let dir_color = if frame.direction == miconlab_core::protocol::logic_analyzer::Direction::Tx {
                            Color32::from_rgb(100, 180, 255)
                        } else {
                            Color32::from_rgb(255, 180, 100)
                        };

                        ui.label(RichText::new(dir_text).color(dir_color).strong());
                        ui.label(RichText::new(frame.protocol).color(theme::TEXT_SECONDARY).small());

                        ui.label(
                            RichText::new(format!("{}", frame.start_us))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        );
                        ui.label(
                            RichText::new(format!("{}", frame.end_us - frame.start_us))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        );
                        ui.label(
                            RichText::new(format!("{}B", frame.data.len()))
                                .color(theme::TEXT_PRIMARY)
                                .monospace(),
                        );

                        let hex_preview: String = frame.data.iter().take(16).map(|b| format!("{:02X} ", b)).collect::<String>().trim_end().to_string();
                        ui.label(
                            RichText::new(hex_preview)
                                .color(theme::TEXT_PRIMARY)
                                .monospace()
                                .small(),
                        );

                        ui.label(RichText::new(&frame.summary).color(theme::TEXT_SECONDARY).small());
                        ui.end_row();
                    }
                });
        });

    if frames.len() > display_count {
        ui.label(
            RichText::new(crate::i18n::tf("(Showing first {} of {} frames)", &[("{}", "{}", &format!("{}", display_count)), ("{}", "{}", &format!("{}", frames.len()))]))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
    }
}

/// 渲染命令-响应映射表（从帧列表推断 TX→RX 配对）
fn render_command_response_map(
    ui: &mut egui::Ui,
    frames: &[miconlab_core::protocol::logic_analyzer::DecodedFrame],
    _zh: bool,
) {
    ui.label(
        RichText::new(crate::i18n::t("Command-Response Map"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(2.0);

    // 简化配对逻辑：每个 TX 帧找最近的 RX 帧（在 TX 之后 100ms 内）
    let pairs = pair_commands_responses(frames);

    if pairs.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No command-response pairs detected (need both TX and RX data)"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        return;
    }

    egui::Grid::new("cmd_resp_grid")
        .striped(true)
        .min_col_width(80.0)
        .show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::t("TX Command")).strong());
            ui.label(RichText::new(crate::i18n::t("RX Response")).strong());
            ui.label(RichText::new(crate::i18n::t("Latency(μs)")).strong());
            ui.end_row();

            for pair in pairs.iter().take(30) {
                let tx_hex: String = pair.tx.data.iter().take(8).map(|b| format!("{:02X} ", b)).collect::<String>().trim_end().to_string();
                let rx_hex: String = pair.rx.data.iter().take(8).map(|b| format!("{:02X} ", b)).collect::<String>().trim_end().to_string();

                ui.label(RichText::new(tx_hex).color(Color32::from_rgb(100, 180, 255)).monospace().small());
                ui.label(RichText::new(rx_hex).color(Color32::from_rgb(255, 180, 100)).monospace().small());

                let latency_color = if pair.latency_us < 1000 {
                    theme::SUCCESS
                } else if pair.latency_us < 10000 {
                    theme::WARNING
                } else {
                    theme::ERROR
                };
                ui.label(
                    RichText::new(format!("{}", pair.latency_us))
                        .color(latency_color)
                        .monospace()
                        .small(),
                );
                ui.end_row();
            }
        });
}

/// 命令-响应对
struct CommandResponsePair<'a> {
    tx: &'a miconlab_core::protocol::logic_analyzer::DecodedFrame,
    rx: &'a miconlab_core::protocol::logic_analyzer::DecodedFrame,
    latency_us: u64,
}

/// 配对 TX 命令与最近的 RX 响应
fn pair_commands_responses<'a>(
    frames: &'a [miconlab_core::protocol::logic_analyzer::DecodedFrame],
) -> Vec<CommandResponsePair<'a>> {
    use miconlab_core::protocol::logic_analyzer::Direction;

    let mut pairs = Vec::new();
    let max_latency_us: u64 = 100_000; // 100ms 内的响应当作配对

    for (i, tx_frame) in frames.iter().enumerate() {
        if tx_frame.direction != Direction::Tx {
            continue;
        }

        // 找 TX 之后最近的 RX 帧
        let mut best_rx: Option<&miconlab_core::protocol::logic_analyzer::DecodedFrame> = None;
        let mut best_latency = u64::MAX;

        for rx_frame in frames.iter().skip(i + 1) {
            if rx_frame.direction != Direction::Rx {
                continue;
            }
            if rx_frame.start_us < tx_frame.end_us {
                continue;
            }
            let latency = rx_frame.start_us - tx_frame.end_us;
            if latency > max_latency_us {
                break;
            }
            if latency < best_latency {
                best_latency = latency;
                best_rx = Some(rx_frame);
            }
        }

        if let Some(rx) = best_rx {
            pairs.push(CommandResponsePair {
                tx: tx_frame,
                rx,
                latency_us: best_latency,
            });
        }
    }

    pairs
}
