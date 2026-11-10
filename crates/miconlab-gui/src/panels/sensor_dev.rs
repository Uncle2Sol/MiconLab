// 传感器开发引导主面板
// 用标签页切换6个子功能：采集向导、实时监控、产品管理、校准向导、协议分析、力分布图
//
// 注意 egui 0.29 死锁问题：CentralPanel::show 持有 ctx 写锁，
// 闭包内不能调用 ctx.data_mut()，故在闭包前 clone 取出状态，闭包后写回。
// 子面板状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut 引用。

use crate::i18n::L10n;
use crate::state::AppState;
use crate::state_manager;
use crate::theme;
use egui::{Id, RichText};

// 子模块声明：文件位于 panels/sensor_dev/ 目录下
#[path = "sensor_dev/wizard.rs"]
mod wizard;
#[path = "sensor_dev/monitor.rs"]
mod monitor;
#[path = "sensor_dev/product_mgr.rs"]
mod product_mgr;
#[path = "sensor_dev/calibration.rs"]
mod calibration;
#[path = "sensor_dev/protocol_analyzer.rs"]
pub(crate) mod protocol_analyzer;
#[path = "sensor_dev/heatmap.rs"]
mod heatmap;

/// 传感器开发面板持久化状态（仅保存当前激活的标签页索引）
#[derive(Clone, Default)]
struct SensorDevState {
    active_tab: usize,
}

/// 标签页标题
const TABS: &[&str] = &["Wizard", "Monitor", "Products", "Calibration", "Protocol", "Heatmap"];

/// 渲染传感器开发主面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == crate::i18n::Language::Zh;
    let title = crate::i18n::t("Sensor Development");
    let subtitle = crate::i18n::t("Flexible Sensors / Acquirers / Accessories");

    // ===== CentralPanel 前 clone 取出持久化状态 =====
    let mut ps = ctx
        .data_mut(|d| d.get_temp_mut_or_default::<SensorDevState>(Id::new("sensor_dev")).clone());

    // 加载 WizardState（供 wizard / monitor / protocol_analyzer 子面板使用）
    let wizard_id = Id::new("wizard_state");
    let mut wizard_state = state_manager::load_state::<wizard::WizardState>(ctx, wizard_id);

    // 加载 HeatmapState（供 heatmap 子面板使用）
    let heatmap_id = Id::new("heatmap_state");
    let mut heatmap_state = state_manager::load_state::<heatmap::HeatmapState>(ctx, heatmap_id);

    // 加载 ProductMgrState（供 product_mgr / calibration 子面板使用）
    let product_mgr_id = Id::new("product_mgr_state");
    let mut product_mgr_state =
        state_manager::load_state::<product_mgr::ProductMgrState>(ctx, product_mgr_id);

    // 加载 CalibState（供 calibration 子面板使用）
    let calib_id = Id::new("calib_state");
    let mut calib_state = state_manager::load_state::<calibration::CalibState>(ctx, calib_id);

    // 加载 MonitorState（供 monitor 子面板使用）
    let monitor_id = Id::new("monitor_state");
    let mut monitor_state = state_manager::load_state::<monitor::MonitorState>(ctx, monitor_id);

    // 加载 ProtocolAnalyzerAllState（供 protocol_analyzer 子面板使用）
    let pa_id = Id::new("protocol_analyzer_state");
    let mut pa_state =
        state_manager::load_state::<protocol_analyzer::ProtocolAnalyzerAllState>(ctx, pa_id);

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(12.0)),
        )
        .show(ctx, |ui| {
            // 标题
            ui.horizontal(|ui| {
                ui.heading(RichText::new(title).color(theme::TEXT_PRIMARY).strong());
                ui.label(RichText::new(subtitle).color(theme::TEXT_SECONDARY).small());
            });
            // AI 感知：错误诊断（连接失败或操作异常时自动推断根因）
            if let Some(err) = &state.last_error {
                if let Some(hint) = crate::panels::ai_sense::sense_error(err, state.is_connected(), state.config.baud_rate, zh) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(4.0);
                }
            }
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            // 标签页栏
            ui.horizontal(|ui| {
                for (i, tab) in TABS.iter().enumerate() {
                    let selected = ps.active_tab == i;
                    let label = crate::i18n::t(*tab);
                    let btn = if selected {
                        egui::Button::new(
                            RichText::new(label).color(egui::Color32::BLACK).strong(),
                        )
                        .fill(theme::ACCENT)
                    } else {
                        egui::Button::new(RichText::new(label).color(theme::TEXT_SECONDARY))
                            .fill(theme::BG_ELEVATED)
                    };
                    if ui
                        .add(btn.min_size(egui::Vec2::new(90.0, 26.0)))
                        .clicked()
                    {
                        ps.active_tab = i;
                    }
                }
            });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            // 根据选中的标签页渲染子面板
            // 子面板在 CentralPanel 闭包内调用，接收 ui 而非自己创建 CentralPanel
            // wizard_state 传入需要访问向导配置的子面板
            match ps.active_tab {
                0 => wizard::show(ctx, state, ui, &mut wizard_state),
                1 => monitor::show(ctx, state, ui, &mut wizard_state, &mut monitor_state),
                2 => product_mgr::show(ctx, state, ui, &mut product_mgr_state),
                3 => calibration::show(ctx, state, ui, &mut calib_state, &mut product_mgr_state),
                4 => protocol_analyzer::show(ctx, state, ui, &mut wizard_state, &mut pa_state),
                5 => heatmap::show(ctx, state, ui, &mut heatmap_state),
                _ => {}
            }

            // 错误提示（统一显示）
            if let Some(err) = &state.last_error {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("\u{26A0} {}", err))
                        .color(theme::ERROR)
                        .small(),
                );
            }

            // 让未使用警告消除
            let _ = &l10n;
        });

    // ===== CentralPanel 闭包结束后写回持久化状态 =====
    ctx.data_mut(|d| {
        *d.get_temp_mut_or_default::<SensorDevState>(Id::new("sensor_dev")) = ps;
    });

    // 写回 WizardState
    state_manager::save_state(ctx, wizard_id, wizard_state);

    // 写回 HeatmapState
    state_manager::save_state(ctx, heatmap_id, heatmap_state);

    // 写回 ProductMgrState
    state_manager::save_state(ctx, product_mgr_id, product_mgr_state);

    // 写回 CalibState
    state_manager::save_state(ctx, calib_id, calib_state);

    // 写回 MonitorState
    state_manager::save_state(ctx, monitor_id, monitor_state);

    // 写回 ProtocolAnalyzerAllState
    state_manager::save_state(ctx, pa_id, pa_state);
}
