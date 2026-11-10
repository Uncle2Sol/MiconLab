// 校准向导子面板
// 功能：通道选择 / 校准方式选择 / 采集原始值 / 校准点管理 / 执行校准 / 显示结果 / 应用到产品
//
// 校准方式：零点校准 / 零点+满量程 / 多点线性拟合 / 多项式拟合
// 状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut CalibState。
// 通过 super::product_mgr::ProductMgrState 访问产品库，将校准系数写回产品配置。

use crate::state::AppState;
use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use miconlab_core::calibration::{
    apply_linear, apply_polynomial, linear_fit, polynomial_fit, zero_calibration,
    zero_span_calibration, CalibrationResult,
};
use miconlab_core::product::{CalibrationData, CalibrationPoint, CalibrationType};

/// 校准方式
#[derive(Clone, PartialEq)]
enum CalibMode {
    /// 零点校准
    Zero,
    /// 零点 + 满量程校准
    ZeroSpan,
    /// 多点线性拟合
    Linear,
    /// 多项式拟合
    Polynomial,
}

impl Default for CalibMode {
    fn default() -> Self {
        CalibMode::Zero
    }
}

/// 校准面板状态
#[derive(Clone)]
pub struct CalibState {
    /// 当前选中的通道索引
    channel: u8,
    /// 校准方式
    mode: CalibMode,
    /// 零点原始值
    zero_raw: f64,
    /// 满量程原始值
    span_raw: f64,
    /// 满量程标准值
    span_value: f64,
    /// 校准点列表 (raw, standard)
    points: Vec<(f64, f64)>,
    /// 新点原始值输入
    new_raw: String,
    /// 新点标准值输入
    new_standard: String,
    /// 多项式拟合阶数
    poly_degree: usize,
    /// 校准结果（线性/零点/零点满量程）
    result: Option<CalibrationResult>,
    /// 多项式系数
    poly_coeffs: Option<Vec<f64>>,
    /// 操作日志
    log: Vec<String>,
    /// 选中的产品 ID（用于应用校准）
    product_id: String,
    /// 采集命令（hex 字符串）
    capture_cmd: String,
    /// 最近采集到的原始值
    last_captured: Option<f64>,
    /// 上次连接会话 ID，用于检测连接变化并清除设备派生数据
    last_session_id: u64,
}

impl Default for CalibState {
    fn default() -> Self {
        Self {
            channel: 0,
            mode: CalibMode::default(),
            zero_raw: 0.0,
            span_raw: 0.0,
            span_value: 100.0,
            points: Vec::new(),
            new_raw: String::new(),
            new_standard: String::new(),
            poly_degree: 2,
            result: None,
            poly_coeffs: None,
            log: Vec::new(),
            product_id: String::new(),
            capture_cmd: "AA 01 55".to_string(),
            last_captured: None,
            last_session_id: 0,
        }
    }
}

impl CalibState {
    /// 向日志追加一条消息
    fn log_push(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        self.log.push(m);
        if self.log.len() > 200 {
            let drop = self.log.len() - 200;
            self.log.drain(0..drop);
        }
    }
}

/// 渲染校准向导子面板
pub fn show(_ctx: &egui::Context, state: &mut AppState, ui: &mut egui::Ui, cal: &mut CalibState, pm: &mut super::product_mgr::ProductMgrState) {
    let zh = state.language == crate::i18n::Language::Zh;

    // 连接会话变化时清除设备派生数据（采集值、校准点、结果），避免跨设备残留
    if cal.last_session_id != state.connection_session_id {
        cal.last_captured = None;
        cal.points.clear();
        cal.result = None;
        cal.poly_coeffs = None;
        cal.zero_raw = 0.0;
        cal.span_raw = 0.0;
        cal.log.clear();
        cal.last_session_id = state.connection_session_id;
    }

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // 副标题：解释面板用途
            ui.label(
                RichText::new(crate::i18n::t("Apply linear/polynomial/two-point calibration to raw sensor data, converting to engineering units"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
            ui.add_space(6.0);

            // ===== 校准方式对比说明（首次展开，之后折叠） =====
            egui::CollapsingHeader::new(crate::i18n::t("📘 Calibration Methods Comparison (click to toggle)"))
            .default_open(true)
            .show(ui, |ui| {
                render_calibration_methods_overview(ui, zh);
            });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);

            // ===== 产品与通道选择 =====
            ui.label(
                RichText::new(crate::i18n::t("Product & Channel"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Product"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                render_product_selector(ui, &mut cal.product_id, pm, zh);
            });
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Channel"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.add(egui::DragValue::new(&mut cal.channel).range(0..=63).prefix("CH"))
                    .on_hover_text(crate::i18n::t("Select the channel to calibrate (0-63)"));
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 采集原始值 =====
            ui.label(
                RichText::new(crate::i18n::t("Capture Raw Value"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Cmd(hex)"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut cal.capture_cmd)
                        .desired_width(120.0)
                        .font(egui::FontId::monospace(12.0)),
                )
                .on_hover_text(crate::i18n::t("HEX Modbus read command (e.g. 01 03 00 00 00 01 84 0A)"));
                let connected = state.is_connected();
                ui.add_enabled_ui(connected, |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(crate::i18n::t("Capture##panels/sensor_dev/calibration.rs:5972"))
                                    .color(Color32::BLACK)
                                    .strong(),
                            )
                            .fill(theme::ACCENT),
                        )
                        .on_hover_text(crate::i18n::t("Send capture command to device, read current raw value as calibration reference"))
                        .clicked()
                    {
                        do_capture(state, cal, zh);
                    }
                });
                if !connected {
                    ui.label(
                        RichText::new(crate::i18n::t("(disconnected)"))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                }
            });
            // 显示最近采集到的值
            if let Some(raw) = cal.last_captured {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Last:"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("{:.4}", raw))
                            .color(theme::CYAN)
                            .strong()
                            .family(egui::FontFamily::Monospace),
                    );
                    // 快捷按钮：填入新点原始值
                    if ui
                        .button(crate::i18n::t("→ raw"))
                        .clicked()
                    {
                        cal.new_raw = format!("{:.4}", raw);
                    }
                    // 零点模式：设为零点
                    if cal.mode == CalibMode::Zero || cal.mode == CalibMode::ZeroSpan {
                        if ui.button(crate::i18n::t("→ zero")).clicked() {
                            cal.zero_raw = raw;
                        }
                    }
                    // 零点满量程模式：设为满量程
                    if cal.mode == CalibMode::ZeroSpan {
                        if ui.button(crate::i18n::t("→ span")).clicked() {
                            cal.span_raw = raw;
                        }
                    }
                });
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 校准方式选择 =====
            ui.label(
                RichText::new(crate::i18n::t("Calibration Mode"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let modes: [(CalibMode, &'static str); 4] = [
                    (CalibMode::Zero, crate::i18n::t("Zero")),
                    (CalibMode::ZeroSpan, crate::i18n::t("Zero+Span")),
                    (CalibMode::Linear, crate::i18n::t("Linear")),
                    (CalibMode::Polynomial, crate::i18n::t("Polynomial")),
                ];
                for (mode, label) in modes {
                    let selected = cal.mode == mode;
                    let tooltip = match mode {
                        CalibMode::Zero => crate::i18n::t("Zero calibration: capture current value as zero offset, eliminates zero drift"),
                        CalibMode::ZeroSpan => crate::i18n::t("Zero+Span: two-point linear calibration, capture zero and span, enter standard value"),
                        CalibMode::Linear => crate::i18n::t("Linear fit: enter multiple (raw, standard) points, least-squares line fit"),
                        CalibMode::Polynomial => crate::i18n::t("Polynomial fit: enter points and select degree, fit n-th order polynomial (for non-linear sensors)"),
                    };
                    let btn = if selected {
                        egui::Button::new(
                            RichText::new(label).color(Color32::BLACK).strong(),
                        )
                        .fill(theme::ACCENT)
                    } else {
                        egui::Button::new(RichText::new(label).color(theme::TEXT_PRIMARY))
                            .fill(theme::BG_ELEVATED)
                    };
                    if ui
                        .add(btn.min_size(egui::Vec2::new(90.0, 24.0)))
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        cal.mode = mode;
                    }
                }
            });

            ui.add_space(8.0);

            // ===== 根据模式显示不同输入 =====
            match cal.mode {
                CalibMode::Zero => render_zero_mode(ui, cal, zh),
                CalibMode::ZeroSpan => render_zero_span_mode(ui, cal, zh),
                CalibMode::Linear => render_linear_mode(ui, cal, zh),
                CalibMode::Polynomial => render_polynomial_mode(ui, cal, zh),
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 校准点列表（线性/多项式模式） =====
            if cal.mode == CalibMode::Linear || cal.mode == CalibMode::Polynomial {
                render_points_list(ui, cal, zh);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
            }

            // ===== 执行校准 =====
            ui.label(
                RichText::new(crate::i18n::t("Execute"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(crate::i18n::t("Calibrate"))
                                .color(Color32::BLACK)
                                .strong(),
                        )
                        .fill(theme::SUCCESS)
                        .min_size(egui::Vec2::new(120.0, 28.0)),
                    )
                    .on_hover_text(crate::i18n::t("Execute calibration based on current mode and input data, generates slope/intercept or polynomial coefficients"))
                    .clicked()
                {
                    do_calibrate(cal, zh);
                }
                if ui
                    .button(crate::i18n::t("Clear##panels/sensor_dev/calibration.rs:12305"))
                    .on_hover_text(crate::i18n::t("Clear zero/span/calibration point data"))
                    .clicked()
                {
                    cal.result = None;
                    cal.poly_coeffs = None;
                }
                if ui
                    .button(crate::i18n::t("Clear Log"))
                    .on_hover_text(crate::i18n::t("Clear operation log"))
                    .clicked()
                {
                    cal.log.clear();
                }
            });

            ui.add_space(8.0);

            // ===== 校准结果 =====
            render_results(ui, cal, zh);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== AI 校准质量评估 =====
            render_ai_assessment(ui, cal, zh);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 应用校准到产品 =====
            ui.label(
                RichText::new(crate::i18n::t("Apply to Product"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let has_result = cal.result.is_some() || cal.poly_coeffs.is_some();
                let has_product = !cal.product_id.is_empty();
                ui.add_enabled_ui(has_result && has_product, |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(crate::i18n::t("Save to Product"))
                                    .color(Color32::BLACK)
                                    .strong(),
                            )
                            .fill(theme::ACCENT),
                        )
                        .on_hover_text(crate::i18n::t("Save calibration result (coefficients + points) to the selected product config for later use"))
                        .clicked()
                    {
                        do_apply(state, cal, pm, zh);
                    }
                });
                if !has_product {
                    ui.label(
                        RichText::new(crate::i18n::t("(select product)"))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                } else if !has_result {
                    ui.label(
                        RichText::new(crate::i18n::t("(calibrate first)"))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 操作日志 =====
            ui.label(
                RichText::new(crate::i18n::t("Log##panels/sensor_dev/calibration.rs:15076"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(2.0);
            render_log(ui, &cal.log);
        });
}

/// 渲染 4 种校准方式的对比说明（适用场景 + 数学原理）
fn render_calibration_methods_overview(ui: &mut egui::Ui, _zh: bool) {
    egui::Frame::group(ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Comparison of four calibration methods - choose the one best suited to your sensor:"))
                .color(theme::TEXT_PRIMARY)
                .small(),
            );
            ui.add_space(6.0);

            // 表头
            egui::Grid::new("calib_methods_grid")
                .num_columns(4)
                .spacing([12.0, 4.0])
                .striped(true)
                .min_col_width(120.0)
                .show(ui, |ui| {
                    // 表头行
                    ui.label(
                        RichText::new(crate::i18n::t("Method"))
                            .color(theme::ACCENT)
                            .strong()
                            .small(),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t("Use Case"))
                            .color(theme::ACCENT)
                            .strong()
                            .small(),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t("Math Principle"))
                            .color(theme::ACCENT)
                            .strong()
                            .small(),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t("Samples"))
                            .color(theme::ACCENT)
                            .strong()
                            .small(),
                    );
                    ui.end_row();

                    // 数据行
                    let rows: &'static [(&'static str, &'static str, &'static str, &'static str)] = &[
                        (
                            "Zero",
                            "Measure zero offset; suitable for single-point zeroing; eliminates zero drift",
                            "y = x - zero_raw (translation only, slope=1)",
                            "1 pt",
                        ),
                        (
                            "Two-Point",
                            "Use zero and full-scale points; linear mapping to engineering units",
                            "y = slope·(x - zero_raw), slope = span_value / (span_raw - zero_raw)",
                            "2 pts",
                        ),
                        (
                            "Linear Fit",
                            "Least-squares line fit for linear sensors with multiple points",
                            "min Σ(y_i - (a·x_i + b))², closed-form for a/b and R²",
                            "≥ 2 pts",
                        ),
                        (
                            "Polynomial",
                            "Higher-order polynomial fit for non-linear sensor curves",
                            "y = a0 + a1·x + ... + an·x^n, least-squares linear system",
                            "≥ n+1 pts",
                        ),
                    ];

                    for (name, scene, math, n) in rows {
                        ui.label(
                            RichText::new(crate::i18n::t(name))
                                .color(theme::CYAN)
                                .strong()
                                .small(),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t(scene))
                                .color(theme::TEXT_PRIMARY)
                                .small(),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t(math))
                                .color(theme::TEXT_SECONDARY)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.label(
                            RichText::new(crate::i18n::t(n))
                                .color(theme::TEXT_SECONDARY)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            // 选用建议
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{2139}").color(theme::CYAN).small());
                ui.label(
                    RichText::new(crate::i18n::t("Tip: For linear sensors prefer Two-Point or Linear Fit. \\\n                         If the sensor response is non-linear (e.g. thermistor, capacitive), \\\n                         use Polynomial fit (typically degree 2-3 is sufficient)."))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            });
        });
}

/// 渲染产品选择下拉框
fn render_product_selector(ui: &mut egui::Ui, product_id: &mut String, pm: &super::product_mgr::ProductMgrState, _zh: bool) {
    // 从 product_mgr 的产品库中读取产品列表
    let products: Vec<(String, String)> = pm
        .library
        .list()
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();

    let selected_text = if product_id.is_empty() {
        crate::i18n::t("Select product").to_string()
    } else {
        products
            .iter()
            .find(|(pid, _)| pid == product_id)
            .map(|(id, name)| {
                if name.is_empty() {
                    id.clone()
                } else {
                    name.clone()
                }
            })
            .unwrap_or_else(|| product_id.clone())
    };

    egui::ComboBox::from_id_salt("cal_product_select")
        .selected_text(selected_text)
        .width(200.0)
        .show_ui(ui, |ui| {
            for (id, name) in &products {
                let label = if name.is_empty() { id.clone() } else { name.clone() };
                ui.selectable_value(product_id, id.clone(), label);
            }
        })
        .response
        .on_hover_text(crate::i18n::t("Select a saved product model to apply calibration parameters"));
}

/// 零点校准模式
fn render_zero_mode(ui: &mut egui::Ui, cal: &mut CalibState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Zero calibration: capture current value as zero offset"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    egui::Grid::new("zero_grid")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Zero Raw"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut cal.zero_raw).speed(0.01))
                .on_hover_text(crate::i18n::t("Raw ADC value at zero input (e.g. 0 or 32768)"));
            ui.end_row();
        });
}

/// 零点+满量程校准模式
fn render_zero_span_mode(ui: &mut egui::Ui, cal: &mut CalibState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Zero+Span: capture zero & span, enter standard value"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    egui::Grid::new("zero_span_grid")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Zero Raw"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut cal.zero_raw).speed(0.01))
                .on_hover_text(crate::i18n::t("Raw ADC value at zero input (e.g. 0 or 32768)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Span Raw"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut cal.span_raw).speed(0.01))
                .on_hover_text(crate::i18n::t("Raw ADC value at full-scale input (e.g. 65535)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Span Value"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut cal.span_value).speed(0.01))
                .on_hover_text(crate::i18n::t("Engineering-unit value at full scale (e.g. 100.0 means 100℃)"));
            ui.end_row();
        });
}

/// 线性拟合模式
fn render_linear_mode(ui: &mut egui::Ui, cal: &mut CalibState, zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Linear fit: enter multiple (raw, standard) points"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    render_add_point_row(ui, cal, zh);
}

/// 多项式拟合模式
fn render_polynomial_mode(ui: &mut egui::Ui, cal: &mut CalibState, zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Polynomial fit: select degree + enter points"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Degree"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        let mut deg = cal.poly_degree as i32;
        ui.add(egui::DragValue::new(&mut deg).range(1..=6).prefix("n="))
            .on_hover_text(crate::i18n::t("Polynomial fit degree (1-5; higher degree fits more precisely but may overfit)"));
        cal.poly_degree = deg.max(1) as usize;
    });
    ui.add_space(4.0);
    render_add_point_row(ui, cal, zh);
}

/// 渲染添加校准点的输入行
fn render_add_point_row(ui: &mut egui::Ui, cal: &mut CalibState, zh: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Raw"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut cal.new_raw)
                .desired_width(80.0)
                .font(egui::FontId::monospace(12.0)),
        )
        .on_hover_text(crate::i18n::t("Raw ADC value of the calibration point"));
        ui.label(
            RichText::new(crate::i18n::t("Std"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut cal.new_standard)
                .desired_width(80.0)
                .font(egui::FontId::monospace(12.0)),
        )
        .on_hover_text(crate::i18n::t("Engineering-unit value of the calibration point"));
        if ui
            .add(
                egui::Button::new(crate::i18n::t("+ Add"))
                    .fill(theme::BG_ELEVATED),
            )
            .on_hover_text(crate::i18n::t("Add calibration point to the list"))
            .clicked()
        {
            add_point(cal, zh);
        }
    });
}

/// 添加校准点
fn add_point(cal: &mut CalibState, _zh: bool) {
    let raw: f64 = match cal.new_raw.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            cal.log_push(crate::i18n::t("Add failed: invalid raw value").to_string());
            return;
        }
    };
    let std: f64 = match cal.new_standard.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            cal.log_push(crate::i18n::t("Add failed: invalid standard value").to_string());
            return;
        }
    };
    cal.points.push((raw, std));
    cal.log_push(format!(
        "+ {}: raw={:.4}, std={:.4} ({} {})",
        crate::i18n::t("Added"),
        raw,
        std,
        crate::i18n::t("total"),
        cal.points.len()
    ));
    cal.new_raw.clear();
    cal.new_standard.clear();
}

/// 渲染校准点列表
fn render_points_list(ui: &mut egui::Ui, cal: &mut CalibState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Calibration Points"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(2.0);

    if cal.points.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No points"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    let mut remove_idx: Option<usize> = None;
    egui::Grid::new("points_grid")
        .num_columns(4)
        .spacing([10.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(
                RichText::new("#")
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .strong(),
            );
            ui.label(
                RichText::new(crate::i18n::t("Raw"))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .strong(),
            );
            ui.label(
                RichText::new(crate::i18n::t("Standard"))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .strong(),
            );
            ui.label(RichText::new("").small());
            ui.end_row();

            for (i, (raw, std)) in cal.points.iter().enumerate() {
                ui.label(
                    RichText::new(format!("{}", i))
                        .color(theme::TEXT_SECONDARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    RichText::new(format!("{:.4}", raw))
                        .color(theme::TEXT_PRIMARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(
                    RichText::new(format!("{:.4}", std))
                        .color(theme::CYAN)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
                if ui
                    .add(
                        egui::Button::new(RichText::new("×").color(theme::ERROR))
                            .min_size(egui::Vec2::new(18.0, 18.0)),
                    )
                    .clicked()
                {
                    remove_idx = Some(i);
                }
                ui.end_row();
            }
        });

    if let Some(i) = remove_idx {
        cal.points.remove(i);
        cal.log_push(format!(
            "{} ({})",
            crate::i18n::t("Removed point"),
            i
        ));
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{}: {}", { crate::i18n::t("Count") }, cal.points.len()))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        if ui.button(crate::i18n::t("Clear All")).clicked() {
            cal.points.clear();
            cal.log_push(crate::i18n::t("Cleared all points").to_string());
        }
    });
}

/// 渲染校准结果
fn render_results(ui: &mut egui::Ui, cal: &CalibState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Result"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    // 线性/零点/零点满量程结果
    if let Some(result) = &cal.result {
        egui::Grid::new("result_grid")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Slope"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.label(
                    RichText::new(format!("{:.6}", result.coefficients.slope))
                        .color(theme::CYAN)
                        .strong()
                        .family(egui::FontFamily::Monospace),
                );
                ui.end_row();

                ui.label(
                    RichText::new(crate::i18n::t("Intercept"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.label(
                    RichText::new(format!("{:.6}", result.coefficients.intercept))
                        .color(theme::CYAN)
                        .strong()
                        .family(egui::FontFamily::Monospace),
                );
                ui.end_row();

                ui.label(
                    RichText::new("R²")
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                let r2_color = if result.r_squared > 0.99 {
                    theme::SUCCESS
                } else if result.r_squared > 0.9 {
                    theme::WARNING
                } else {
                    theme::ERROR
                };
                ui.label(
                    RichText::new(format!("{:.6}", result.r_squared))
                        .color(r2_color)
                        .strong()
                        .family(egui::FontFamily::Monospace),
                );
                ui.end_row();

                if !result.residuals.is_empty() {
                    ui.label(
                        RichText::new(crate::i18n::t("Residuals"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    let resid_str = result
                        .residuals
                        .iter()
                        .map(|r| format!("{:.4}", r))
                        .collect::<Vec<_>>()
                        .join(", ");
                    ui.label(
                        RichText::new(resid_str)
                            .color(theme::TEXT_PRIMARY)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.end_row();
                }
            });

        ui.add_space(4.0);
        // 验证示例
        ui.label(
            RichText::new(crate::i18n::t("Verify (y = slope*x + intercept):"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        let test_values = [0.0, 100.0, 500.0, 1000.0, 4095.0];
        ui.horizontal_wrapped(|ui| {
            for x in &test_values {
                let y = apply_linear(*x, &result.coefficients);
                ui.label(
                    RichText::new(format!("x={:.0}→y={:.2}", x, y))
                        .color(theme::TEXT_PRIMARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
            }
        });
    }

    // 多项式结果
    if let Some(coeffs) = &cal.poly_coeffs {
        ui.label(
            RichText::new(format!(
                "{}: a0 + a1·x + a2·x² + ... + a{}·x^{}",
                crate::i18n::t("Polynomial coeffs"),
                cal.poly_degree,
                cal.poly_degree
            ))
            .color(theme::ACCENT)
            .small(),
        );
        ui.add_space(2.0);
        egui::Grid::new("poly_result_grid")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .striped(true)
            .show(ui, |ui| {
                for (i, c) in coeffs.iter().enumerate() {
                    ui.label(
                        RichText::new(format!("a{}", i))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("{:.6}", c))
                            .color(theme::CYAN)
                            .strong()
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.end_row();
                }
            });

        ui.add_space(4.0);
        // 验证示例
        ui.label(
            RichText::new(crate::i18n::t("Verify:"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        let test_values = [0.0, 100.0, 500.0, 1000.0, 4095.0];
        ui.horizontal_wrapped(|ui| {
            for x in &test_values {
                let y = apply_polynomial(*x, coeffs);
                ui.label(
                    RichText::new(format!("x={:.0}→y={:.2}", x, y))
                        .color(theme::TEXT_PRIMARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
            }
        });
    }

    if cal.result.is_none() && cal.poly_coeffs.is_none() {
        ui.label(
            RichText::new(crate::i18n::t("No calibration yet"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
    }
}

/// 渲染 AI 校准质量评估
fn render_ai_assessment(ui: &mut egui::Ui, cal: &CalibState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("AI Calibration Assessment"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    if cal.points.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("No points to assess"))
                .color(theme::TEXT_DISABLED)
                .small(),
        );
        return;
    }

    let assessment = miconlab_ai::CalibrationAssessment::assess(&cal.points);

    egui::Frame::group(ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            egui::Grid::new("ai_assess_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    // 推荐校准模型
                    ui.label(
                        RichText::new(crate::i18n::t("Recommended Model"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(crate::i18n::t_owned(assessment.recommended_model.label_zh()))
                            .color(theme::ACCENT)
                            .strong(),
                    );
                    ui.end_row();

                    // R²
                    ui.label(
                        RichText::new("R²")
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    let r2_color = if assessment.r_squared > 0.99 {
                        theme::SUCCESS
                    } else if assessment.r_squared > 0.9 {
                        theme::WARNING
                    } else {
                        theme::ERROR
                    };
                    ui.label(
                        RichText::new(format!("{:.6}", assessment.r_squared))
                            .color(r2_color)
                            .strong()
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.end_row();

                    // RMSE
                    ui.label(
                        RichText::new("RMSE")
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("{:.6}", assessment.rmse))
                            .color(theme::CYAN)
                            .strong()
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.end_row();
                });

            ui.add_space(6.0);

            // 点分布均匀性（带进度条）
            ui.label(
                RichText::new(crate::i18n::t("Distribution Uniformity"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            let uniformity = assessment.uniformity.clamp(0.0, 1.0);
            let bar_color = if uniformity > 0.7 {
                theme::SUCCESS
            } else if uniformity > 0.4 {
                theme::WARNING
            } else {
                theme::ERROR
            };
            let bar = egui::ProgressBar::new(uniformity)
                .fill(bar_color)
                .text(format!("{:.1}%", uniformity * 100.0));
            ui.add(bar);

            ui.add_space(6.0);

            // 离群点提示
            if !assessment.outliers.is_empty() {
                let n = assessment.outliers.len();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(
                        RichText::new(crate::i18n::tf("Detected {} outliers (indices: {:?})", &[("{}", "{}", &format!("{}", n)), ("{:?}", "{:?}", &format!("{:?}", assessment.outliers))]))
                        .color(theme::WARNING)
                        .small(),
                    );
                });
            }

            // 间隙区域提示
            if !assessment.gap_regions.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{1F4CD}").color(theme::CYAN));
                    ui.label(
                        RichText::new(crate::i18n::t("Suggested regions to add calibration points:").to_string())
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                    );
                });
                for (lo, hi) in &assessment.gap_regions {
                    ui.label(
                        RichText::new(format!("  [{:.4}, {:.4}]", lo, hi))
                            .color(theme::CYAN)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    );
                }
            }

            ui.add_space(4.0);
            // 评估摘要
            ui.label(
                RichText::new(format!("\u{1F4AC} {}", assessment.summary_zh))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
        });
}

/// 渲染操作日志
fn render_log(ui: &mut egui::Ui, log: &[String]) {
    ScrollArea::vertical()
        .max_height(150.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Frame::none()
                .fill(theme::BG_DEEP)
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                .inner_margin(egui::Margin::same(6.0))
                .show(ui, |ui| {
                    if log.is_empty() {
                        ui.label(
                            RichText::new("(empty)")
                                .color(theme::TEXT_DISABLED)
                                .small(),
                        );
                    } else {
                        for line in log.iter().rev() {
                            ui.label(
                                RichText::new(line)
                                    .color(theme::TEXT_SECONDARY)
                                    .small()
                                    .family(egui::FontFamily::Monospace),
                            );
                        }
                    }
                });
        });
}

/// 执行采集：发送命令并解析响应
fn do_capture(state: &mut AppState, cal: &mut CalibState, _zh: bool) {
    let cmd_str = cal.capture_cmd.trim().to_string();
    if cmd_str.is_empty() {
        cal.log_push(crate::i18n::t("Error: no capture command").to_string());
        state.last_error = Some(crate::i18n::t("No capture command").to_string());
        return;
    }
    let cmd = match miconlab_core::hex::parse_hex(&cmd_str) {
        Ok(b) => b,
        Err(e) => {
            cal.log_push(format!(
                "{}: {}",
                crate::i18n::t("Parse error"),
                e
            ));
            state.last_error = Some(e);
            return;
        }
    };
    cal.log_push(format!(
        "{}: {}",
        { crate::i18n::t("Send") },
        miconlab_core::hex::format_hex_spaced(&cmd)
    ));
    match state.send_and_read(&cmd, 100) {
        Ok(resp) => {
            let hex_str = miconlab_core::hex::format_hex_spaced(&resp);
            // 解析原始值：取前2字节作为 u16 大端
            let raw = if resp.len() >= 2 {
                u16::from_be_bytes([resp[0], resp[1]]) as f64
            } else if resp.len() == 1 {
                resp[0] as f64
            } else {
                cal.log_push(crate::i18n::t("Warning: empty response").to_string());
                0.0
            };
            cal.last_captured = Some(raw);
            cal.new_raw = format!("{:.4}", raw);
            cal.log_push(format!(
                "{}: {} → raw={:.4}",
                crate::i18n::t("Capture OK"),
                hex_str,
                raw
            ));
            state.last_error = None;
        }
        Err(e) => {
            cal.log_push(format!(
                "{}: {}",
                crate::i18n::t("Capture failed"),
                e
            ));
            state.last_error = Some(e);
        }
    }
}

/// 执行校准
fn do_calibrate(cal: &mut CalibState, _zh: bool) {
    match cal.mode {
        CalibMode::Zero => {
            let coeff = zero_calibration(cal.zero_raw);
            cal.result = Some(CalibrationResult {
                coefficients: coeff.clone(),
                r_squared: 1.0,
                residuals: vec![],
                points: vec![(cal.zero_raw, 0.0)],
            });
            cal.poly_coeffs = None;
            cal.log_push(format!(
                "{}: slope={:.6}, intercept={:.6}",
                crate::i18n::t("Zero calibration done"),
                coeff.slope,
                coeff.intercept
            ));
        }
        CalibMode::ZeroSpan => {
            // 检查零点与满量程是否相同
            if (cal.span_raw - cal.zero_raw).abs() < f64::EPSILON {
                cal.log_push(crate::i18n::t("Error: zero and span raw values are identical").to_string());
                return;
            }
            let coeff = zero_span_calibration(cal.zero_raw, cal.span_raw, cal.span_value);
            cal.result = Some(CalibrationResult {
                coefficients: coeff.clone(),
                r_squared: 1.0,
                residuals: vec![],
                points: vec![(cal.zero_raw, 0.0), (cal.span_raw, cal.span_value)],
            });
            cal.poly_coeffs = None;
            cal.log_push(format!(
                "{}: slope={:.6}, intercept={:.6}",
                crate::i18n::t("Zero-span calibration done"),
                coeff.slope,
                coeff.intercept
            ));
        }
        CalibMode::Linear => {
            if cal.points.len() < 2 {
                cal.log_push(crate::i18n::t("Error: linear fit requires at least 2 points").to_string());
                return;
            }
            let result = linear_fit(&cal.points);
            cal.log_push(format!(
                "{}: slope={:.6}, intercept={:.6}, R²={:.6}",
                crate::i18n::t("Linear fit done"),
                result.coefficients.slope,
                result.coefficients.intercept,
                result.r_squared
            ));
            cal.result = Some(result);
            cal.poly_coeffs = None;
        }
        CalibMode::Polynomial => {
            let needed = cal.poly_degree + 1;
            if cal.points.len() < needed {
                cal.log_push(format!(
                    "{}: {} {} {}",
                    crate::i18n::t("Error"),
                    crate::i18n::t("need at least"),
                    needed,
                    crate::i18n::t("points")
                ));
                return;
            }
            let coeffs = polynomial_fit(&cal.points, cal.poly_degree);
            if coeffs.is_empty() {
                cal.log_push(crate::i18n::t("Error: fit failed (singular matrix)").to_string());
                return;
            }
            cal.log_push(format!(
                "{}: [{}]",
                crate::i18n::t("Polynomial fit done"),
                coeffs
                    .iter()
                    .map(|c| format!("{:.6}", c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            cal.poly_coeffs = Some(coeffs);
            cal.result = None;
        }
    }
}

/// 应用校准到产品配置
fn do_apply(state: &mut AppState, cal: &mut CalibState, pm: &mut super::product_mgr::ProductMgrState, _zh: bool) {
    let product_id = cal.product_id.clone();
    if product_id.is_empty() {
        state.last_error = Some(crate::i18n::t("Select a product first").to_string());
        return;
    }

    // 确定校准类型与系数
    let (calib_type, coefficients) = match (&cal.result, &cal.poly_coeffs) {
        (Some(r), None) => {
            let ct = match cal.mode {
                CalibMode::Zero => CalibrationType::Zero,
                CalibMode::ZeroSpan => CalibrationType::ZeroSpan,
                _ => CalibrationType::Linear,
            };
            (ct, vec![r.coefficients.slope, r.coefficients.intercept])
        }
        (None, Some(coeffs)) => {
            (CalibrationType::Polynomial(cal.poly_degree as u8), coeffs.clone())
        }
        _ => {
            state.last_error = Some(crate::i18n::t("No calibration result").to_string());
            return;
        }
    };

    // 构建校准点
    let points: Vec<CalibrationPoint> = cal
        .points
        .iter()
        .map(|(raw, std)| CalibrationPoint {
            raw_value: *raw,
            standard_value: *std,
            description: None,
        })
        .collect();

    // 写入产品库
    let success = if let Some(product) = pm.library.products.get_mut(&product_id) {
        product.calibration = Some(CalibrationData {
            calibration_type: calib_type.clone(),
            coefficients,
            points,
            calibrated_at: Some(chrono::Local::now().to_rfc3339()),
        });
        true
    } else {
        false
    };

    if success {
        state.last_error = None;
        cal.log_push(format!(
            "{}: {} ({})",
            crate::i18n::t("Applied to product"),
            product_id,
            calib_type
        ));
    } else {
        state.last_error = Some(
            crate::i18n::t("Product not found").to_string(),
        );
        cal.log_push(format!(
            "{}: {}",
            crate::i18n::t("Apply failed: product not found"),
            product_id
        ));
    }
}
