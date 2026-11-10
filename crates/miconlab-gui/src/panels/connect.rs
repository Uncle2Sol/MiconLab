// 连接配置面板
// 提供端口选择、串口参数配置、连接/断开控制
// 连接后自动侦测 USB 桥接芯片和 MCU 类型（被动协议分析）

use crate::i18n::L10n;
use crate::panels::sensor_dev::protocol_analyzer::{
    analyze_passive_mcu_comprehensive, identify_by_port_name, identify_usb_bridge,
    PassiveAnalysisResult,
};
use crate::state::AppState;
use crate::state_manager;
use crate::theme;
use egui::{Color32, RichText, Vec2};
use miconlab_core::config::{DataBits, FlowControl, Parity, StopBits};
use std::time::{Duration, Instant};

// ============================================================================
// 连接页 MCU 自动侦测状态（通过 state_manager 持久化，规避 CentralPanel 闭包内 ctx.data_mut() 死锁）
// ============================================================================

/// 连接页芯片侦测状态
#[derive(Default, Clone)]
struct ConnectDetectState {
    /// 上次连接会话 ID（用于检测新连接）
    last_session_id: u64,
    /// 侦测启动时刻
    detect_started_at: Option<Instant>,
    /// 是否已完成被动分析
    passive_done: bool,
    /// 被动分析结果
    passive_result: Option<PassiveAnalysisResult>,
    /// 侦测日志
    log: Vec<String>,
    /// 手动重新侦测标志
    manual_retry: bool,
}

/// 启动连接页 MCU 侦测
fn start_connect_detect(s: &mut ConnectDetectState) {
    s.detect_started_at = Some(Instant::now());
    s.passive_done = false;
    s.passive_result = None;
    s.manual_retry = false;
    s.log.clear();
    s.log.push(crate::i18n::t("──── Auto chip detection after connect ────").to_string());
    s.log.push(crate::i18n::t("Waiting 2s to collect data stream...").to_string());
}

/// 帧驱动：更新连接页 MCU 侦测状态
fn update_connect_detect(state: &mut AppState, s: &mut ConnectDetectState) {
    // 检测连接会话变化
    let session_changed = s.last_session_id != state.connection_session_id;

    if session_changed {
        s.last_session_id = state.connection_session_id;
        if state.is_connected() {
            start_connect_detect(s);
        }
    }

    // 检查手动重试
    let need_start = s.manual_retry && s.detect_started_at.is_none();
    if need_start && state.is_connected() {
        start_connect_detect(s);
    }

    // 如果未连接，清除侦测
    if !state.is_connected() {
        if s.detect_started_at.is_some() {
            s.detect_started_at = None;
            s.passive_done = false;
            s.passive_result = None;
        }
        return;
    }

    // 检查是否该执行被动分析（启动 2 秒后，收集足够数据）
    let should_analyze = !s.passive_done
        && s
            .detect_started_at
            .map_or(false, |t| t.elapsed() >= Duration::from_secs(2));

    if should_analyze {
        // 执行被动分析
        let (intervals, header_rate) = collect_insole_frame_stats(&state.rx_buffer);
        let baud_rate = state.config.baud_rate;
        // USB 桥接芯片识别（与 render_chip_identification 逻辑一致）
        // 传入分析函数：当识别为原生 MCU 时，分析函数内部会对匹配候选 +50%、不匹配 -15%
        let bridge_info = state
            .port_list
            .get(state.selected_port_index)
            .and_then(|p| {
                if let Some(info) = identify_usb_bridge(
                    p.vid,
                    p.pid,
                    p.description.as_deref(),
                    p.manufacturer.as_deref(),
                ) {
                    return Some(info);
                }
                identify_by_port_name(&p.name)
            });
        let result = analyze_passive_mcu_comprehensive(
            &intervals,
            header_rate,
            baud_rate,
            &state.rx_buffer,
            bridge_info.as_ref(),
        );

        s.passive_done = true;
        s.passive_result = Some(result.clone());
        s.log.push(crate::i18n::tf(
            "✓ Collected {frames} frames, header match {rate}%",
            &[
                ("{frames}", "{frames}", &format!("{}", result.sample_count)),
                ("{rate}", "{rate}", &format!("{:.0}", result.header_match_rate * 100.0)),
            ],
        ));
        if result.parsed_frames > 0 {
            s.log.push(crate::i18n::tf(
                "✓ Deep-parsed {frames} frames, ADC {bits}bit, IMU float {status}",
                &[
                    ("{frames}", "{frames}", &format!("{}", result.parsed_frames)),
                    ("{bits}", "{bits}", &format!("{}", result.adc_resolution)),
                    ("{status}", "{status}", crate::i18n::t(if result.imu_float_valid { "valid" } else { "invalid" })),
                ],
            ));
        }
        s.log.push(crate::i18n::tf(
            "→ Best guess: {vendor} {family} ({confidence}%)",
            &[
                ("{vendor}", "{vendor}", &result.vendor),
                ("{family}", "{family}", &result.chip_family),
                ("{confidence}", "{confidence}", &format!("{:.0}", result.confidence * 100.0)),
            ],
        ));
    }
}

/// 将 MCU chip_family 映射到烧录器支持的芯片 hint 字符串
///
/// 返回 (flasher_hint, 是否支持烧录)
/// 烧录器支持 11 种芯片：STM32/ESP32/Arduino/ESP8266/STM8/LPC/GD32/CH32/PIC/SDCard/SpiFlash
fn mcu_family_to_flasher_hint(chip_family: &str) -> Option<&'static str> {
    let family = chip_family.to_uppercase();
    if family.starts_with("STM32") {
        Some("stm32")
    } else if family.starts_with("GD32") {
        Some("gd32")
    } else if family.starts_with("CH32") {
        Some("ch32")
    } else if family.starts_with("ESP32") || family == "ESP32-S3" {
        Some("esp32")
    } else if family.starts_with("ESP8266") {
        Some("esp8266")
    } else if family.starts_with("STM8") {
        Some("stm8")
    } else if family.starts_with("LPC") {
        Some("lpc")
    } else if family.starts_with("ARDUINO") || family.starts_with("AVR") || family.starts_with("ATMEGA") {
        Some("arduino")
    } else {
        None
    }
}

/// 渲染连接页芯片识别区域
fn render_chip_identification(ctx: &egui::Context, ui: &mut egui::Ui, state: &mut AppState, s: &mut ConnectDetectState, _zh: bool) {
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    ui.heading(
        RichText::new(crate::i18n::t("Chip Identification v2.1"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.label(
        RichText::new(crate::i18n::t("Auto-detect USB bridge chip and MCU type after connection (inferred from communication patterns)"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);

    // ===== USB 桥接芯片识别 =====
    let bridge_info = state
        .port_list
        .get(state.selected_port_index)
        .and_then(|p| {
            // 1. 优先通过 VID/PID + 描述/厂商名识别
            if let Some(info) = identify_usb_bridge(p.vid, p.pid, p.description.as_deref(), p.manufacturer.as_deref()) {
                return Some(info);
            }
            // 2. 后备：通过端口名识别（macOS 上 serialport 可能不返回 VID/PID）
            identify_by_port_name(&p.name)
        });

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("USB Bridge:"))
                .color(theme::TEXT_SECONDARY),
        );
        if let Some(ref bridge) = bridge_info {
            let color = if bridge.is_native_mcu {
                theme::SUCCESS
            } else {
                theme::ACCENT
            };
            ui.label(
                RichText::new(format!(
                    "{} {} [{:04X}:{:04X}]",
                    bridge.vendor, bridge.chip_name, bridge.vid, bridge.pid
                ))
                .color(color)
                .strong(),
            );
            if bridge.is_native_mcu {
                ui.label(
                    RichText::new(crate::i18n::t("(Native MCU USB)"))
                        .color(theme::SUCCESS)
                        .small(),
                );
            }
        } else if state.is_connected() {
            ui.label(
                RichText::new(crate::i18n::t("Unknown"))
                    .color(theme::TEXT_DISABLED),
            );
        } else {
            ui.label(
                RichText::new(crate::i18n::t("— (not connected)"))
                    .color(theme::TEXT_DISABLED),
            );
        }
    });

    if let Some(ref bridge) = bridge_info {
        ui.label(
            RichText::new(crate::i18n::tf(
                "  {chip} — {desc}",
                &[
                    ("{chip}", "{chip}", &bridge.chip_name),
                    ("{desc}", "{desc}", if bridge.is_native_mcu {
                        crate::i18n::t("MCU native USB CDC, supports USB Bootloader (e.g. STM32 DFU / ESP32 USB-Download)")
                    } else {
                        crate::i18n::t("USB-to-serial bridge, supports UART Bootloader (STM32/GD32/CH32/ESP32/Arduino etc.)")
                    }),
                ],
            ))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
    }

    // ===== MCU 被动侦测结果 =====
    if state.is_connected() {
        ui.add_space(6.0);

        // 克隆状态数据用于 UI 渲染（后续按钮可能修改 s，需先释放不可变借用）
        let (detect_running, passive_result, log) = (
            s.detect_started_at.is_some() && !s.passive_done,
            s.passive_result.clone(),
            s.log.clone(),
        );

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(crate::i18n::t("MCU Detection:"))
                    .color(theme::TEXT_SECONDARY),
            );
            if detect_running {
                ui.label(
                    RichText::new(crate::i18n::t("Detecting..."))
                        .color(theme::WARNING),
                );
            } else if passive_result.is_some() {
                ui.label(
                    RichText::new(crate::i18n::t("Done"))
                        .color(theme::SUCCESS)
                        .small(),
                );
                // 重新侦测按钮
                if ui
                    .button(RichText::new(crate::i18n::t("↻ Retry")).small())
                    .clicked()
                {
                    s.detect_started_at = None;
                    s.passive_done = false;
                    s.passive_result = None;
                    s.manual_retry = true;
                }
            } else {
                ui.label(
                    RichText::new(crate::i18n::t("Waiting..."))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            }
        });

        // 显示被动分析结果
        if let Some(pa) = &passive_result {
            ui.add_space(4.0);
            let color = if pa.confidence >= 0.5 {
                theme::SUCCESS
            } else if pa.confidence >= 0.35 {
                theme::WARNING
            } else {
                theme::TEXT_SECONDARY
            };

            // 最佳推断
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("→ {} {}", pa.vendor, pa.chip_family))
                        .color(color)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!("({:.0}%)", pa.confidence * 100.0))
                        .color(color)
                        .small(),
                );
            });

            // 多维度分析摘要
            if pa.sample_count > 0 {
                ui.label(
                    RichText::new(crate::i18n::tf(
                        "  Frame interval {avg}±{jitter}ms | Header match {rate}% | {clock}",
                        &[
                            ("{avg}", "{avg}", &format!("{:.1}", pa.avg_interval_ms)),
                            ("{jitter}", "{jitter}", &format!("{:.1}", pa.jitter_ms)),
                            ("{rate}", "{rate}", &format!("{:.0}", pa.header_match_rate * 100.0)),
                            ("{clock}", "{clock}", &pa.clock_source),
                        ],
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
            }
            if pa.parsed_frames > 0 {
                ui.label(
                    RichText::new(crate::i18n::tf(
                        "  Deep-parsed {frames} frames | ADC {bits}bit ({min}-{max}) | IMU {imu} | {order}",
                        &[
                            ("{frames}", "{frames}", &format!("{}", pa.parsed_frames)),
                            ("{bits}", "{bits}", &format!("{}", pa.adc_resolution)),
                            ("{min}", "{min}", &format!("{:04X}", pa.adc_min_value)),
                            ("{max}", "{max}", &format!("{:04X}", pa.adc_max_value)),
                            ("{imu}", "{imu}", &if pa.imu_float_valid {
                                crate::i18n::tf("float valid (|q|={norm})", &[("{norm}", "{norm}", &format!("{:.2}", pa.imu_quaternion_norm))])
                            } else {
                                crate::i18n::t("float invalid").to_string()
                            }),
                            ("{order}", "{order}", &pa.byte_order),
                        ],
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
            }

            // Top 5 候选 MCU 排名 — 使用 ScrollArea 支持上下滚动
            if !pa.candidates.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(crate::i18n::tf("Candidate MCUs (Top {}) — scrollable", &[("{}", "{}", &format!("{}", pa.candidates.len().min(5)))]))
                    .color(theme::CYAN)
                    .strong(),
                );

                // 候选列表 ScrollArea：限制最大高度，超出可上下滚动
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::same(6.0))
                            .show(ui, |ui| {
                                for (i, c) in pa.candidates.iter().take(5).enumerate() {
                                    let c_color = if c.score >= 0.6 {
                                        theme::SUCCESS
                                    } else if c.score >= 0.4 {
                                        theme::WARNING
                                    } else {
                                        theme::TEXT_SECONDARY
                                    };
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{}.", i + 1))
                                                .color(theme::TEXT_DISABLED)
                                                .small(),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{} {}",
                                                c.vendor, c.example_model
                                            ))
                                            .color(c_color)
                                            .strong()
                                            .small(),
                                        );
                                        ui.label(
                                            RichText::new(format!("({:.0}%)", c.score * 100.0))
                                                .color(c_color)
                                                .small(),
                                        );
                                    });
                                    for reason in &c.match_reasons {
                                        ui.label(
                                            RichText::new(format!("  ✓ {}", reason))
                                                .color(theme::SUCCESS)
                                                .small()
                                                .family(egui::FontFamily::Monospace),
                                        );
                                    }
                                    for reason in &c.mismatch_reasons {
                                        ui.label(
                                            RichText::new(format!("  ✗ {}", reason))
                                                .color(theme::ERROR)
                                                .small()
                                                .family(egui::FontFamily::Monospace),
                                        );
                                    }
                                    if i < pa.candidates.len().min(5) - 1 {
                                        ui.separator();
                                    }
                                }
                            });
                    });
            }

            ui.label(
                RichText::new(crate::i18n::tf("  Basis: {evidence}", &[("{evidence}", "{evidence}", &crate::i18n::t_owned(&pa.evidence))]))
                    .color(theme::TEXT_DISABLED)
                    .small(),
            );

            // ===== 跳转到烧录器按钮 =====
            // 注意：此按钮在 ScrollArea 之外，确保始终可见
            if let Some(best) = pa.candidates.first() {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                let hint = mcu_family_to_flasher_hint(&best.chip_family);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Best match: "))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.label(
                        RichText::new(format!("{} {}", best.vendor, best.example_model))
                            .color(theme::ACCENT)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(format!("({:.0}%)", best.score * 100.0))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                });

                if let Some(hint_str) = hint {
                    ui.add_space(4.0);
                    let btn_text = { crate::i18n::tf("⚡ Go to Flasher → {}", &[("{}", "{}", &format!("{}", best.chip_family))]) };
                    let btn_resp = ui.add_sized(
                        [ui.available_width().min(320.0), 36.0],
                        egui::Button::new(
                            RichText::new(btn_text).color(Color32::WHITE).strong(),
                        )
                        .fill(theme::ACCENT),
                    );
                    let clicked = btn_resp.clicked();
                    btn_resp.on_hover_text(crate::i18n::t("Jump to flasher panel, auto-select chip type, ready to read/flash firmware"));
                    if clicked {
                        ctx.data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new("flasher_chip_hint"),
                                hint_str.to_string(),
                            );
                        });
                        state.active_panel = crate::panels::PanelKind::Flasher;
                    }
                    ui.label(
                        RichText::new(crate::i18n::tf("✓ Flasher supports {}, can auto-read firmware backup or flash new firmware", &[("{}", "{}", &format!("{}", best.chip_family))]))
                        .color(theme::SUCCESS)
                        .small(),
                    );
                } else {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(crate::i18n::t("⚠ This chip type is not supported by the flasher"))
                        .color(theme::WARNING),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t("  Supported: STM32 / GD32 / CH32 / ESP32 / ESP8266 / Arduino / STM8 / LPC / PIC / SDCard / SPI Flash"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                    );
                }
            }
        }

        // 侦测日志（折叠）
        if !log.is_empty() {
            ui.add_space(2.0);
            egui::CollapsingHeader::new(
                RichText::new(crate::i18n::t("Detection Log"))
                    .color(theme::CYAN)
                    .small(),
            )
            .default_open(false)
            .show(ui, |ui| {
                for line in &log {
                    let color = if line.contains("✓") {
                        theme::SUCCESS
                    } else if line.contains("✗") {
                        theme::ERROR
                    } else if line.contains("→") {
                        theme::ACCENT
                    } else {
                        theme::TEXT_SECONDARY
                    };
                    ui.label(
                        RichText::new(line)
                            .color(color)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                }
            });
        }
    }
}

/// 从 rx_buffer 收集鞋垫帧间隔和帧头匹配率（连接页直接复用）
fn collect_insole_frame_stats(rx_buffer: &[crate::state::RxEntry]) -> (Vec<u64>, f64) {
    let header = [0xAA, 0x55, 0x03, 0x99];
    let mut frame_times: Vec<u64> = Vec::new();
    let mut total_rx = 0usize;
    let mut matched = 0usize;

    for entry in rx_buffer.iter().filter(|e| e.direction == crate::state::Direction::Rx) {
        total_rx += 1;
        if entry.data.windows(4).any(|w| w == header) {
            matched += 1;
            frame_times.push(entry.timestamp.timestamp_millis() as u64);
        }
    }

    let mut intervals: Vec<u64> = Vec::new();
    for w in frame_times.windows(2) {
        if w[1] > w[0] {
            intervals.push(w[1] - w[0]);
        }
    }

    let rate = if total_rx > 0 {
        matched as f64 / total_rx as f64
    } else {
        0.0
    };

    (intervals, rate)
}

/// 常用波特率
const BAUD_RATES: &[u32] = &[
    9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1500000, // 1500000: 设备专用
];

/// 渲染连接配置面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == crate::i18n::Language::Zh;

    // 在 CentralPanel 之前加载侦测状态（规避 CentralPanel 闭包内 ctx.data_mut() 死锁）
    let detect_id = egui::Id::new("connect_detect_state");
    let mut detect_state = state_manager::load_state::<ConnectDetectState>(ctx, detect_id);

    // 帧驱动：更新连接页 MCU 侦测状态（检测连接变化、自动启动侦测）
    update_connect_detect(state, &mut detect_state);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            ui.heading(
                RichText::new(l10n.t("panel.connect"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(crate::i18n::t("Configure serial parameters and connect - port, baud rate, data bits, parity, flow control"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(if state.is_connected() {
                    l10n.t("connect.connected")
                } else {
                    l10n.t("connect.disconnected")
                })
                .color(if state.is_connected() {
                    theme::SUCCESS
                } else {
                    theme::TEXT_SECONDARY
                })
                .small(),
            );

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // 状态指示灯 + 端口选择
            ui.horizontal(|ui| {
                // 连接状态指示灯
                let dot_color = if state.is_connected() {
                    theme::SUCCESS
                } else {
                    theme::TEXT_DISABLED
                };
                ui.label(RichText::new("\u{25CF}").color(dot_color).size(12.0));

                // 端口下拉选择
                ui.label(
                    RichText::new(format!("{}:", l10n.t("connect.port")))
                        .color(theme::TEXT_SECONDARY),
                );

                let port_names: Vec<String> = state
                    .port_list
                    .iter()
                    .map(|p| {
                        let vid_pid = match (p.vid, p.pid) {
                            (Some(v), Some(pid)) => format!(" [{:04X}:{:04X}]", v, pid),
                            _ => String::new(),
                        };
                        format!("{}{}", p.name, vid_pid)
                    })
                    .collect();

                let selected_text = port_names
                    .get(state.selected_port_index)
                    .cloned()
                    .unwrap_or_else(|| "—".to_string());

                egui::ComboBox::from_id_salt("port_select")
                    .selected_text(selected_text)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if port_names.is_empty() {
                            ui.label(
                                RichText::new(crate::i18n::t("No ports available"))
                                .color(theme::TEXT_DISABLED),
                            );
                        }
                        for (i, name) in port_names.iter().enumerate() {
                            ui.selectable_value(&mut state.selected_port_index, i, name);
                        }
                    })
                    .response
                    .on_hover_text(crate::i18n::t("Serial devices detected by the system, showing name and VID:PID (e.g. USB-to-serial chips). Click refresh to rescan"));

                // 刷新按钮
                if ui
                    .button(RichText::new(format!("\u{21BB} {}", l10n.t("connect.refresh"))))
                    .on_hover_text(crate::i18n::t("Rescan serial devices. New devices will be auto-selected after refresh"))
                    .clicked()
                {
                    state.refresh_ports();
                }

                // 显示刷新结果提示（5 秒内显示）
                if let Some(until) = state.port_refresh_msg_until {
                    if std::time::Instant::now() < until {
                        if let Some(msg) = &state.port_refresh_msg {
                            ui.label(
                                RichText::new(msg)
                                    .color(theme::ACCENT)
                                    .small(),
                            );
                        }
                    }
                }
            });

            // AI 感知：设备识别提示（基于选中端口的 VID/PID 查询知识库）
            if let Some(port) = state.port_list.get(state.selected_port_index) {
                if let (Some(vid), Some(pid)) = (port.vid, port.pid) {
                    if let Some(hint) = crate::panels::ai_sense::sense_device(vid, pid, zh) {
                        crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                        ui.add_space(4.0);
                    }
                }
            }

            // AI 感知：错误诊断（连接失败时自动诊断根因，带上下文：rx 数据/VID:PID/端口名）
            if let Some(err) = &state.last_error {
                let rx_data = crate::panels::ai_sense::collect_recent_rx(&state.rx_buffer, 256);
                let (vid_pid, port_name) = state.port_list.get(state.selected_port_index)
                    .map(|p| (p.vid.zip(p.pid), Some(p.name.as_str())))
                    .unwrap_or((None, None));
                if let Some(hint) = crate::panels::ai_sense::sense_error_full(
                    err, state.is_connected(), state.config.baud_rate,
                    &rx_data, vid_pid, port_name, zh,
                ) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(4.0);
                }
            }

            ui.add_space(8.0);

            // 参数配置网格
            egui::Grid::new("connect_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    // 波特率
                    label_cell(ui, &l10n, "connect.baud");
                    baud_combo(ui, state, zh);
                    ui.end_row();

                    // 数据位
                    label_cell(ui, &l10n, "connect.data_bits");
                    data_bits_combo(ui, state, zh);
                    ui.end_row();

                    // 停止位
                    label_cell(ui, &l10n, "connect.stop_bits");
                    stop_bits_combo(ui, state, zh);
                    ui.end_row();

                    // 校验
                    label_cell(ui, &l10n, "connect.parity");
                    parity_combo(ui, state, &l10n, zh);
                    ui.end_row();

                    // 流控
                    label_cell(ui, &l10n, "connect.flow_control");
                    flow_control_combo(ui, state, &l10n, zh);
                    ui.end_row();

                    // 超时
                    label_cell(ui, &l10n, "connect.timeout");
                    ui.add(
                        egui::DragValue::new(&mut state.config.timeout_ms)
                            .range(1..=60000)
                            .speed(0.2)
                            .suffix(" ms"),
                    )
                    .on_hover_text(crate::i18n::t("Timeout for a single read/write. Short timeouts are responsive but may falsely report no data; long timeouts suit slow devices"));
                    ui.end_row();
                });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(12.0);

            // 串扰检测告警条（CH340 TX 自发自收识别）
            render_crosstalk_summary(ui, state, zh);

            // 解释性引导：为什么按压无反应？
            egui::CollapsingHeader::new(crate::i18n::t("Why cushion press unresponsive?"))
            .default_open(false)
            .show(ui, |ui| {
                egui::Frame::group(&ui.style())
                    .fill(theme::BG_ELEVATED)
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(crate::i18n::t("TX self-loopback: when device TX is not connected to CH340 RX, the RX pin picks up TX crosstalk via impedance coupling."))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(crate::i18n::t("Checklist:"))
                                .color(theme::ACCENT)
                                .small()
                                .strong(),
                        );
                        let steps = [
                            crate::i18n::t("1. Check TX/RX wiring: device TX → CH340 RX, device RX → CH340 TX"),
                            crate::i18n::t("2. Check GND common: device GND and CH340 GND must be connected"),
                            crate::i18n::t("3. Check device power: device needs independent power (CH340 VCC is usually insufficient)"),
                        ];
                        for s in steps {
                            ui.label(RichText::new(s).color(theme::TEXT_PRIMARY).small());
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(crate::i18n::t("If all checks pass, CH340 clock deviation at high baud (921600) may cause garbled data. The 'Apply CH340 compensation' button above will appear in that case."))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                        );
                    });
            });

            // 连接/断开按钮
            ui.horizontal(|ui| {
                let connected = state.is_connected();
                let (btn_text, btn_color) = if connected {
                    (l10n.t("connect.disconnect"), theme::ERROR)
                } else {
                    (l10n.t("connect.connect"), theme::ACCENT)
                };

                let btn = egui::Button::new(
                    RichText::new(btn_text).color(Color32::BLACK).strong(),
                )
                .fill(btn_color)
                .min_size(Vec2::new(120.0, 32.0))
                .stroke(egui::Stroke::NONE);

                let btn_tooltip = if connected {
                    crate::i18n::t("Close serial port connection")
                } else {
                    crate::i18n::t("Open serial port with current configuration; Arduino/ESP32 auto-reset via DTR")
                };
                if ui.add(btn)
                    .on_hover_text(btn_tooltip)
                    .clicked()
                {
                    if connected {
                        state.disconnect();
                    } else {
                        state.connect();
                    }
                }
            });

            // 错误信息显示
            if let Some(err) = &state.last_error {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(
                        RichText::new(err)
                            .color(theme::ERROR)
                            .family(egui::FontFamily::Monospace),
                    );
                });
            }

            // 芯片识别区域（USB 桥接芯片 + MCU 自动侦测）
            render_chip_identification(ctx, ui, state, &mut detect_state, zh);
        });

    // CentralPanel 闭包结束后写回侦测状态
    state_manager::save_state(ctx, detect_id, detect_state);
}

/// 网格中的标签单元格（用 TEXT_SECONDARY 色）
fn label_cell(ui: &mut egui::Ui, l10n: &L10n, key: &'static str) {
    ui.label(RichText::new(l10n.t(key)).color(theme::TEXT_SECONDARY));
}

/// 渲染波特率下拉
fn baud_combo(ui: &mut egui::Ui, state: &mut AppState, _zh: bool) {
    let current = state.config.baud_rate;
    egui::ComboBox::from_id_salt("baud_select")
        .selected_text(format!("{}", current))
        .width(140.0)
        .show_ui(ui, |ui| {
            for &baud in BAUD_RATES {
                ui.selectable_value(&mut state.config.baud_rate, baud, format!("{}", baud));
            }
        })
        .response
        .on_hover_text(crate::i18n::t("Communication rate (bits/sec). Common: 9600 for slow sensors, 115200 general-purpose, 921600 for high-speed streams. Both ends must match"));
}

/// 渲染数据位下拉
fn data_bits_combo(ui: &mut egui::Ui, state: &mut AppState, _zh: bool) {
    let current = match state.config.data_bits {
        DataBits::Five => "5",
        DataBits::Six => "6",
        DataBits::Seven => "7",
        DataBits::Eight => "8",
    };
    egui::ComboBox::from_id_salt("data_bits_select")
        .selected_text(current)
        .width(140.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.config.data_bits, DataBits::Five, "5");
            ui.selectable_value(&mut state.config.data_bits, DataBits::Six, "6");
            ui.selectable_value(&mut state.config.data_bits, DataBits::Seven, "7");
            ui.selectable_value(&mut state.config.data_bits, DataBits::Eight, "8");
        })
        .response
        .on_hover_text(crate::i18n::t("Data bits per frame. 8 is most common; 7 is used for ASCII comms (e.g. Modbus ASCII); 5/6 are rare"));
}

/// 渲染停止位下拉
fn stop_bits_combo(ui: &mut egui::Ui, state: &mut AppState, _zh: bool) {
    let current = match state.config.stop_bits {
        StopBits::One => "1",
        StopBits::Two => "2",
    };
    egui::ComboBox::from_id_salt("stop_bits_select")
        .selected_text(current)
        .width(140.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.config.stop_bits, StopBits::One, "1");
            ui.selectable_value(&mut state.config.stop_bits, StopBits::Two, "2");
        })
        .response
        .on_hover_text(crate::i18n::t("Stop bits at frame end. 1 is the modern standard; 2 is for legacy devices or long-line noise immunity, effectively extending inter-frame gap"));
}

/// 渲染校验下拉
fn parity_combo(ui: &mut egui::Ui, state: &mut AppState, l10n: &L10n, _zh: bool) {
    let current = match state.config.parity {
        Parity::None => l10n.t("common.none"),
        Parity::Odd => "Odd",
        Parity::Even => "Even",
    };
    egui::ComboBox::from_id_salt("parity_select")
        .selected_text(current)
        .width(140.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.config.parity, Parity::None, l10n.t("common.none"));
            ui.selectable_value(&mut state.config.parity, Parity::Odd, "Odd");
            ui.selectable_value(&mut state.config.parity, Parity::Even, "Even");
        })
        .response
        .on_hover_text(crate::i18n::t("Parity: None (most common), Odd, or Even. Enables single-bit error detection at a small throughput cost"));
}

/// 渲染流控下拉
fn flow_control_combo(ui: &mut egui::Ui, state: &mut AppState, l10n: &L10n, _zh: bool) {
    let current = match state.config.flow_control {
        FlowControl::None => l10n.t("common.none"),
        FlowControl::Software => "Software",
        FlowControl::Hardware => "Hardware",
    };
    egui::ComboBox::from_id_salt("flow_control_select")
        .selected_text(current)
        .width(140.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.config.flow_control, FlowControl::None, l10n.t("common.none"));
            ui.selectable_value(&mut state.config.flow_control, FlowControl::Software, "Software");
            ui.selectable_value(&mut state.config.flow_control, FlowControl::Hardware, "Hardware");
        })
        .response
        .on_hover_text(crate::i18n::t("Flow control: None, Software (XON/XOFF chars), or Hardware (RTS/CTS signals). Hardware flow control is recommended for high-speed comms to prevent buffer overflow"));
}

/// 渲染串扰检测告警条
///
/// 当检测到 CH340 TX 自发自收串扰时，显示红色告警条，
/// 提示用户按压无响应可能源于 TX 串扰而非设备未发送。
/// 若检测到位移/高字节模式，追加"一键补偿波特率"按钮。
fn render_crosstalk_summary(ui: &mut egui::Ui, state: &mut AppState, _zh: bool) {
    let report = match &state.crosstalk_report {
        Some(r) if r.is_loopback || r.is_half_echo() || r.has_crosstalk() => r.clone(),
        _ => return,
    };

    ui.add_space(4.0);
    egui::Frame::group(&ui.style())
        .fill(egui::Color32::from_rgba_unmultiplied(80, 20, 20, 200))
        .stroke(egui::Stroke::new(1.0_f32, theme::ERROR))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{26A0}").color(theme::ERROR).size(16.0));
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::tf("TX self-loopback crosstalk detected (echo {:.0}%, RX/TX = {:.2})", &[("{:.0}", "{:.0}", &format!("{:.0}", report.echo_ratio * 100.0)), ("{:.2}", "{:.2}", &format!("{:.2}", report.rx_tx_ratio))]))
                        .color(theme::ERROR)
                        .strong(),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t("Cushion press unresponsive may be caused by this: CH340 RX receives TX crosstalk instead of device data. Check TX/RX wiring, GND common, device power."))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                    );
                    if let Some(lat) = report.latency_us {
                        ui.label(
                            RichText::new(crate::i18n::tf("Echo latency estimate: {} μs", &[("{}", "{}", &format!("{}", lat))]))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                        );
                    }
                    // 一键 CH340 补偿波特率按钮（检测到位移/高字节模式时显示）
                    if matches!(
                        report.pattern,
                        miconlab_core::crosstalk::EchoPattern::BitShifted
                            | miconlab_core::crosstalk::EchoPattern::HighByteDominant
                    ) {
                        ui.add_space(4.0);
                        let current_baud = state.config.baud_rate;
                        let compensated = find_ch340_compensated_baud(current_baud);
                        if let Some((nominal, comp)) = compensated {
                            let btn_text = { crate::i18n::tf("Apply CH340 compensation → {} (nominal {})", &[("{}", "{}", &format!("{}", comp)), ("{}", "{}", &format!("{}", nominal))]) };
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(&btn_text).color(Color32::BLACK))
                                        .fill(theme::WARNING)
                                        .min_size(Vec2::new(0.0, 24.0)),
                                )
                                .on_hover_text(crate::i18n::t("CH340 has clock deviation at high baud rates; setting 2x baud lets the chip downscale to target"))
                                .clicked()
                            {
                                state.config.baud_rate = comp;
                                if state.is_connected() {
                                    state.connect(); // 用新波特率重连
                                }
                            }
                        } else {
                            ui.label(
                                RichText::new(crate::i18n::tf("No CH340 compensation available for baud {}", &[("{}", "{}", &format!("{}", current_baud))]))
                                .color(theme::TEXT_DISABLED)
                                .small(),
                            );
                        }
                    }
                });
            });
        });
    ui.add_space(4.0);
}

/// 查找当前波特率对应的 CH340 补偿波特率
fn find_ch340_compensated_baud(current_baud: u32) -> Option<(u32, u32)> {
    // 优先查找标称值匹配
    for &(nominal, compensated) in miconlab_core::baud_detect::CH340_COMPENSATED_RATES {
        if nominal == current_baud {
            return Some((nominal, compensated));
        }
    }
    // 再查找补偿值匹配（用户可能已设置补偿值）
    for &(nominal, compensated) in miconlab_core::baud_detect::CH340_COMPENSATED_RATES {
        if compensated == current_baud {
            return Some((nominal, compensated));
        }
    }
    None
}
