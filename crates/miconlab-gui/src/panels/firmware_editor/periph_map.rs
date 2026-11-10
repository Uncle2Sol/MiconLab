//! 外设映射标签页（D.6 新增）
//!
//! 可视化展示固件分析得出的 DMA 通道映射、中断向量表、GPIO 配置，
//! 并与诊断引擎联动高亮冲突项。
//!
//! 数据来源：miconlab_ai::firmware::FirmwareAnalysis
//! 触发条件：FirmwareEditor 加载 bin 文件后调用 FirmwareAnalysis::analyze()

use crate::theme;
use crate::state::AppState;
use egui::{Color32, RichText, Ui};
use miconlab_ai::firmware::{DmaChannel, FirmwareAnalysis, GpioMode, GpioPin, IrqPeripheral};
use std::collections::HashMap;

/// 渲染外设映射标签页
///
/// 参数：
/// - `ui`: egui UI 上下文
/// - `state`: 全局 AppState（用于访问诊断引擎）
/// - `fw`: 固件分析结果
/// - `is_zh`: 是否中文
pub fn render_periph_map(ui: &mut Ui, state: &AppState, fw: &FirmwareAnalysis, is_zh: bool) {
    ui.horizontal(|ui| {
        ui.heading(
            RichText::new(crate::i18n::t("Peripheral Map"))
                .color(theme::ACCENT)
                .strong(),
        );
        ui.label(
            RichText::new(crate::i18n::tf("DMA channels: {} | IRQs: {} | GPIO ports: {}", &[("{}", "{}", &format!("{}", fw.dma_channels.len())), ("{}", "{}", &format!("{}", fw.irq_table.len())), ("{}", "{}", &format!("{}", fw.gpio_config.len()))]))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
    });

    ui.add_space(4.0);

    // 说明折叠区
    ui.collapsing(
        RichText::new(crate::i18n::t("📖 Peripheral Map Help"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        |ui| {
            ui.label(
                RichText::new(crate::i18n::t("This page scans firmware binary to extract peripheral config:\n• DMA: search DMA register addresses, correlate with peripheral DR\n• IRQ: parse ARM vector table [16+], filter default handlers\n• GPIO: search MODER register references\n\nConflicts in red, matches in green."))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        },
    );

    ui.add_space(8.0);

    // 检测 DMA 冲突
    let dma_conflicts = detect_dma_conflicts(&fw.dma_channels);

    // 渲染三张表
    render_dma_table(ui, &fw.dma_channels, &dma_conflicts, is_zh);
    ui.add_space(12.0);
    render_irq_table(ui, &fw.irq_table, is_zh);
    ui.add_space(12.0);
    render_gpio_table(ui, &fw.gpio_config, is_zh);

    ui.add_space(12.0);

    // 底部：与诊断引擎联动
    render_diagnosis_panel(ui, state, fw, &dma_conflicts, is_zh);
}

// ============================================================================
// DMA 通道表
// ============================================================================

fn render_dma_table(
    ui: &mut Ui,
    channels: &[DmaChannel],
    conflicts: &HashMap<(u8, u8), Vec<String>>,
    _is_zh: bool,
) {
    ui.heading(
        RichText::new(crate::i18n::t("DMA Channel Mapping"))
            .color(theme::ACCENT)
            .strong(),
    );

    if channels.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No DMA channel configuration detected (firmware may not use DMA)"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        return;
    }

    egui::Frame::group(&ui.style())
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            egui::Grid::new("dma_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    // 表头
                    ui.label(RichText::new(crate::i18n::t("Ctrl")).strong());
                    ui.label(RichText::new(crate::i18n::t("Channel")).strong());
                    ui.label(RichText::new(crate::i18n::t("Peripheral")).strong());
                    ui.label(RichText::new(crate::i18n::t("Addr")).strong());
                    ui.label(RichText::new(crate::i18n::t("Conf")).strong());
                    ui.label(RichText::new(crate::i18n::t("Status")).strong());
                    ui.end_row();

                    // 数据行
                    for ch in channels {
                        let key = (ch.dma_controller, ch.channel);
                        let is_conflict = ch.channel != 0 && conflicts.contains_key(&key);

                        let ctrl_color = if is_conflict {
                            Color32::from_rgb(255, 100, 100)
                        } else {
                            Color32::from_rgb(100, 255, 100)
                        };

                        ui.label(RichText::new(format!("DMA{}", ch.dma_controller)).color(ctrl_color));
                        ui.label(
                            RichText::new(if ch.channel == 0 {
                                "?".to_string()
                            } else {
                                format!("Ch{}", ch.channel)
                            })
                            .color(ctrl_color),
                        );

                        let periph_text = if ch.peripheral == "Unknown" {
                            crate::i18n::t("Unknown").to_string()
                        } else {
                            ch.peripheral.clone()
                        };
                        ui.label(RichText::new(periph_text).color(ctrl_color));

                        ui.label(
                            RichText::new(if ch.periph_addr == 0 {
                                "-".to_string()
                            } else {
                                format!("0x{:08X}", ch.periph_addr)
                            })
                            .color(theme::TEXT_SECONDARY)
                            .monospace(),
                        );

                        ui.label(
                            RichText::new(format!("{:.0}%", ch.confidence * 100.0))
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );

                        if is_conflict {
                            ui.label(
                                RichText::new(crate::i18n::t("⚠ Conflict"))
                                    .color(Color32::from_rgb(255, 100, 100))
                                    .strong(),
                            );
                        } else if ch.channel == 0 {
                            ui.label(
                                RichText::new(crate::i18n::t("Base"))
                                    .color(theme::TEXT_SECONDARY)
                                    .small(),
                            );
                        } else {
                            ui.label(
                                RichText::new(crate::i18n::t("✓ OK"))
                                    .color(Color32::from_rgb(100, 255, 100)),
                            );
                        }
                        ui.end_row();
                    }
                });
        });

    // 冲突详情
    if !conflicts.is_empty() {
        ui.add_space(4.0);
        for ((controller, channel), periphs) in conflicts {
            ui.label(
                RichText::new(crate::i18n::tf("⚠ DMA{} Channel{} shared by multiple peripherals: {}", &[("{}", "{}", &format!("{}", controller)), ("{}", "{}", &format!("{}", channel)), ("{}", "{}", &format!("{}", periphs.join(", ")))]))
                .color(Color32::from_rgb(255, 100, 100))
                .small(),
            );
        }
    }
}

// ============================================================================
// 中断向量表
// ============================================================================

fn render_irq_table(ui: &mut Ui, irqs: &[IrqPeripheral], _is_zh: bool) {
    ui.heading(
        RichText::new(crate::i18n::t("IRQ Vector Table (Enabled)"))
            .color(theme::ACCENT)
            .strong(),
    );

    if irqs.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No enabled peripheral IRQs detected"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        return;
    }

    egui::Frame::group(&ui.style())
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            egui::Grid::new("irq_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("IRQ").strong());
                    ui.label(RichText::new(crate::i18n::t("Peripheral")).strong());
                    ui.label(RichText::new(crate::i18n::t("Handler Addr")).strong());
                    ui.end_row();

                    for irq in irqs {
                        ui.label(RichText::new(format!("IRQ{}", irq.irq_num)).monospace());
                        ui.label(RichText::new(&irq.peripheral).color(Color32::from_rgb(100, 200, 255)));
                        ui.label(
                            RichText::new(format!("0x{:08X}", irq.handler_addr))
                                .color(theme::TEXT_SECONDARY)
                                .monospace(),
                        );
                        ui.end_row();
                    }
                });
        });
}

// ============================================================================
// GPIO 配置表
// ============================================================================

fn render_gpio_table(ui: &mut Ui, pins: &[GpioPin], _is_zh: bool) {
    ui.heading(
        RichText::new(crate::i18n::t("GPIO Config (Port Summary)"))
            .color(theme::ACCENT)
            .strong(),
    );

    if pins.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No GPIO MODER register references detected"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        return;
    }

    egui::Frame::group(&ui.style())
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            egui::Grid::new("gpio_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(crate::i18n::t("Port")).strong());
                    ui.label(RichText::new(crate::i18n::t("Mode")).strong());
                    ui.label(RichText::new(crate::i18n::t("AF")).strong());
                    ui.label(RichText::new(crate::i18n::t("Label")).strong());
                    ui.end_row();

                    for pin in pins {
                        ui.label(RichText::new(format!("GPIO{}", pin.port)).color(Color32::from_rgb(255, 200, 100)));

                        let mode_text = match pin.mode {
                            GpioMode::Input => crate::i18n::t("Input"),
                            GpioMode::Output => crate::i18n::t("Output"),
                            GpioMode::AlternateFunction => crate::i18n::t("AF"),
                            GpioMode::Analog => crate::i18n::t("Analog"),
                        };
                        ui.label(RichText::new(mode_text).color(theme::TEXT_SECONDARY));

                        ui.label(
                            RichText::new(if pin.af == 0 { "-".to_string() } else { format!("AF{}", pin.af) })
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );

                        ui.label(
                            RichText::new(pin.label.as_deref().unwrap_or("-"))
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );
                        ui.end_row();
                    }
                });
        });
}

// ============================================================================
// 诊断引擎联动
// ============================================================================

fn render_diagnosis_panel(
    ui: &mut Ui,
    state: &AppState,
    fw: &FirmwareAnalysis,
    dma_conflicts: &HashMap<(u8, u8), Vec<String>>,
    _is_zh: bool,
) {
    ui.heading(
        RichText::new(crate::i18n::t("Diagnosis"))
            .color(theme::ACCENT)
            .strong(),
    );

    ui.add_space(4.0);

    // 从当前选中端口提取 vid_pid（用于推断 USB 桥接芯片）
    let vid_pid: Option<(u16, u16)> = state
        .port_list
        .get(state.selected_port_index)
        .and_then(|p| match (p.vid, p.pid) {
            (Some(v), Some(p)) => Some((v, p)),
            _ => None,
        });

    // 从 vid_pid 推断 chip_hint（CH340/CP210x 等）
    let chip_hint = vid_pid.map(|(vid, _)| match vid {
        0x1A86 => "CH340".to_string(),
        0x10C4 => "CP210x".to_string(),
        0x0403 => "FTDI".to_string(),
        _ => format!("VID_{:04X}", vid),
    });

    // 构造诊断上下文并运行推理
    let ctx = miconlab_ai::diagnose::DiagnosticContext {
        is_connected: state.is_connected(),
        baud_rate: state.config.baud_rate,
        vid_pid,
        chip_hint,
        firmware_analysis: Some(fw.clone()),
        ..Default::default()
    };

    let engine = miconlab_ai::diagnose::DiagnosticEngine::with_builtin_rules();
    let results = engine.diagnose(&ctx);

    // 只显示 D.6 相关规则
    let fw_rules: Vec<_> = results
        .iter()
        .filter(|r| r.rule_id.starts_with("FW_"))
        .collect();

    if fw_rules.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("✓ No firmware-related diagnostic rules triggered"))
            .color(Color32::from_rgb(100, 255, 100))
            .small(),
        );
        return;
    }

    for result in fw_rules {
        let color = match result.severity {
            miconlab_ai::diagnose::Severity::Critical => Color32::from_rgb(255, 80, 80),
            miconlab_ai::diagnose::Severity::Error => Color32::from_rgb(255, 150, 80),
            miconlab_ai::diagnose::Severity::Warning => Color32::from_rgb(255, 200, 80),
            miconlab_ai::diagnose::Severity::Info => Color32::from_rgb(100, 180, 255),
        };

        egui::Frame::group(&ui.style())
            .stroke(egui::Stroke::new(1.0_f32, color))
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("[{}]", result.rule_id)).color(color).strong());
                    ui.label(RichText::new(crate::i18n::t_owned(result.severity.label_zh())).color(color).small());
                    ui.label(
                        RichText::new(format!("({:.0}%)", result.confidence * 100.0))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                });
                ui.label(RichText::new(crate::i18n::t_owned(&result.diagnosis)).color(theme::TEXT_PRIMARY));
                ui.add_space(2.0);
                for s in &result.suggestions {
                    ui.label(RichText::new(format!("• {}", crate::i18n::t_owned(s))).color(theme::TEXT_SECONDARY).small());
                }
            });
        ui.add_space(4.0);
    }

    // DMA 冲突补充显示
    if !dma_conflicts.is_empty() {
        ui.label(
            RichText::new(crate::i18n::tf("{} DMA channel conflicts detected, see DMA table above", &[("{}", "{}", &format!("{}", dma_conflicts.len()))]))
            .color(Color32::from_rgb(255, 100, 100))
            .small(),
        );
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 检测 DMA 通道冲突
/// 返回：(DMA 控制器, 通道号) → 外设列表
fn detect_dma_conflicts(channels: &[DmaChannel]) -> HashMap<(u8, u8), Vec<String>> {
    let mut channel_map: HashMap<(u8, u8), Vec<String>> = HashMap::new();

    for ch in channels {
        if ch.channel == 0 {
            continue; // 通道号 0 表示基地址关联，跳过
        }
        let key = (ch.dma_controller, ch.channel);
        channel_map
            .entry(key)
            .or_default()
            .push(ch.peripheral.clone());
    }

    // 过滤出有冲突的（≥2 个不同外设）
    let mut conflicts = HashMap::new();
    for (key, periphs) in channel_map {
        let unique: std::collections::HashSet<&String> = periphs.iter().collect();
        if unique.len() >= 2 {
            conflicts.insert(key, periphs);
        }
    }

    conflicts
}
