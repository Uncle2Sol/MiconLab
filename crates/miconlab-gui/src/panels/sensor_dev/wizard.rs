// 采集向导子面板：分步引导用户完成传感器采集配置
//
// 步骤 0: 选择采集方式（ADC/I2C/SPI/串口帧/Modbus）
// 步骤 1: 选择芯片/产品（ESP32/STM32/自定义 或从产品库选择）
// 步骤 2: 配置采集参数（通道数、采样率、引脚/地址、量程）
// 步骤 3: 确认配置 → 生成配置摘要
// 步骤 4: 开始采集（提示用户切换到"实时监控"标签页）
//
// 状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut WizardState。

use crate::state::AppState;
use crate::theme;
use egui::{Color32, RichText};
use miconlab_core::acquisition::{AcquisitionConfig, ChannelConfig};
use miconlab_core::product::{
    ChecksumType, DataFormat, FrameFormat, ProtocolConfig, ProtocolType, RegisterMapping,
};

/// 采集方式选项
const ACQUIRE_METHODS: &[(&str, &str)] = &[
    ("ADC", "ADC direct read (analog sampling)"),
    ("I2C", "I2C digital sensor"),
    ("SPI", "SPI digital sensor"),
    ("SerialFrame", "Custom serial frame"),
    ("Modbus", "Modbus RTU"),
];

/// 芯片/产品预设
const CHIPS: &[(&str, &str)] = &[
    ("ESP32", "ESP32 (12-bit ADC, 3.3V reference)"),
    ("STM32", "STM32F103 (12-bit ADC, 3.3V reference)"),
    ("Custom", "Custom chip"),
];

/// 颜色调色板：为多通道波形分配区分色
const CHANNEL_COLORS: &[[u8; 3]] = &[
    [0x4F, 0xC3, 0xF7], // 蓝
    [0x81, 0xC7, 0x84], // 绿
    [0xFF, 0xB0, 0x00], // 琥珀
    [0xF8, 0x51, 0x49], // 红
    [0xBA, 0x68, 0xC8], // 紫
    [0x4D, 0xD0, 0xE1], // 青
    [0xFF, 0xD5, 0x4F], // 黄
    [0xA1, 0xE8, 0x8E], // 浅绿
];

/// 向导持久化状态
#[derive(Clone)]
pub struct WizardState {
    /// 当前步骤（0..=4）
    step: usize,
    /// 选中的采集方式索引
    method_idx: usize,
    /// 选中的芯片索引
    chip_idx: usize,
    /// 通道数
    channel_count: usize,
    /// 采样率 (Hz)
    sample_rate: f64,
    /// 引脚/地址描述（如 "GPIO34" 或 "0x48"）
    pin: String,
    /// 量程下限
    range_min: f64,
    /// 量程上限
    range_max: f64,
    /// 单位
    unit: String,
    /// 数据格式（SerialFrame/Modbus 用）
    data_format: DataFormat,
    /// 帧头 hex（SerialFrame 用，如 "AA 55"）
    header_hex: String,
    /// 帧尾 hex（SerialFrame 用，如 "55 AA"）
    footer_hex: String,
    /// 校验类型
    checksum_type: ChecksumType,
    /// Modbus 从站 ID
    slave_id: u8,
    /// Modbus 寄存器起始地址
    register_addr: u16,
    /// Modbus 寄存器数量
    register_count: u16,
    /// Modbus 功能码（3=读保持寄存器，4=读输入寄存器）
    function_code: u8,
    /// 是否已完成（用于显示"切换到监控"提示）
    completed: bool,
    /// 用户是否已在监控页开始过采样（仅用于标记，不锁死向导导航）
    monitor_started: bool,
    /// 当前串口波特率（由 show() 从 AppState 同步，供 build_protocol_config 使用）
    baud_rate: u32,
    /// ADC 分辨率（位，随芯片选择变化）
    adc_resolution: u16,
    /// ADC 参考电压（V，随芯片选择变化）
    adc_ref_voltage: f64,
}

impl WizardState {
    fn default_fields() -> Self {
        Self {
            step: 0,
            method_idx: 0,
            chip_idx: 0,
            channel_count: 1,
            sample_rate: 100.0,
            pin: "GPIO34".to_string(),
            range_min: 0.0,
            range_max: 3.3,
            unit: "V".to_string(),
            data_format: DataFormat::U16LE,
            header_hex: "AA 55".to_string(),
            footer_hex: "55 AA".to_string(),
            checksum_type: ChecksumType::Sum8,
            slave_id: 1,
            register_addr: 0,
            register_count: 1,
            function_code: 3,
            completed: false,
            monitor_started: false,
            baud_rate: 115200,
            adc_resolution: 12,
            adc_ref_voltage: 3.3,
        }
    }
}

impl Default for WizardState {
    fn default() -> Self {
        Self::default_fields()
    }
}

/// 渲染采集向导子面板
pub fn show(_ctx: &egui::Context, state: &mut AppState, ui: &mut egui::Ui, s: &mut WizardState) {
    let zh = state.language == crate::i18n::Language::Zh;

    // 从 AppState 同步当前串口波特率，供 build_protocol_config 使用
    s.baud_rate = state.config.baud_rate;

        // 副标题
        ui.label(RichText::new(crate::i18n::t("5-step sensor acquisition setup: choose method → select chip → configure channels → confirm → start")).color(theme::TEXT_SECONDARY).small());
        ui.add_space(4.0);

        // 步骤指示器
        render_step_indicator(ui, s.step);
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // 预留底部导航按钮区域（分隔线 + 间距 + 按钮约 56px），
        // 避免 ScrollArea 占满剩余空间导致导航按钮被挤出可视区
        const NAV_RESERVED: f32 = 56.0;
        let scroll_max = (ui.available_height() - NAV_RESERVED).max(120.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(scroll_max)
            .show(ui, |ui| match s.step {
                0 => render_step_method(ui, s, zh),
                1 => render_step_chip(ui, s, zh),
                2 => render_step_params(ui, state, s, zh),
                3 => render_step_summary(ui, s, zh),
                4 => render_step_finish(ui, s, zh),
                _ => {}
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        // 步骤导航按钮
        ui.horizontal(|ui| {
            // 上一步
            if s.step > 0 && !s.completed {
                if ui
                    .button(RichText::new(crate::i18n::t("Previous")).strong())
                    .on_hover_text(crate::i18n::t("Go back to the previous step (parameters are retained)"))
                    .clicked()
                {
                    s.step = s.step.saturating_sub(1);
                }
            }
            ui.add_space(8.0);
            // 下一步 / 完成
            if !s.completed && s.step < 4 {
                let label = if s.step == 3 {
                    crate::i18n::t("Finish")
                } else {
                    crate::i18n::t("Next")
                };
                let tip = if s.step == 3 {
                    crate::i18n::t("Confirm the configuration, generate acquisition config and proceed")
                } else {
                    crate::i18n::t("Save current step parameters and proceed to the next step")
                };
                let btn = egui::Button::new(
                    RichText::new(label).color(Color32::BLACK).strong(),
                )
                .fill(theme::ACCENT)
                .min_size(egui::Vec2::new(110.0, 28.0))
                .stroke(egui::Stroke::NONE);
                if ui.add(btn).on_hover_text(tip).clicked() {
                    if s.step == 3 {
                        s.completed = true;
                    }
                    s.step = (s.step + 1).min(4);
                }
            }
            // 重新开始
            if s.completed {
                if ui
                    .button(
                        RichText::new(crate::i18n::t("Restart")).strong(),
                    )
                    .on_hover_text(crate::i18n::t("Reset the wizard to its initial state and start over (current config is not saved)"))
                    .clicked()
                {
                    *s = WizardState::default();
                }
            }
        });
}

/// 渲染步骤指示器（步骤 0..=4）
fn render_step_indicator(ui: &mut egui::Ui, current: usize) {
    let names: &[&str] = &[
        crate::i18n::t("Method"),
        crate::i18n::t("Chip"),
        crate::i18n::t("Params"),
        crate::i18n::t("Confirm"),
        crate::i18n::t("Start"),
    ];
    ui.horizontal(|ui| {
        for (i, name) in names.iter().enumerate() {
            let done = i < current;
            let active = i == current;
            let (dot_color, text_color) = if active {
                (theme::ACCENT, theme::ACCENT)
            } else if done {
                (theme::SUCCESS, theme::SUCCESS)
            } else {
                (theme::TEXT_DISABLED, theme::TEXT_SECONDARY)
            };
            let dot = if done { "\u{25CF}" } else { "\u{25CB}" };
            ui.label(RichText::new(dot).color(dot_color).size(14.0));
            ui.label(RichText::new(*name).color(text_color).small().strong());
            if i + 1 < names.len() {
                ui.label(
                    RichText::new("\u{2192}")
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
                ui.add_space(4.0);
            }
        }
    });
}

/// 步骤 0：选择采集方式
/// 点击选项后自动进入下一步（单选向导的常见交互模式）
fn render_step_method(ui: &mut egui::Ui, s: &mut WizardState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Select Acquisition Method"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Click any method below to select and proceed to the next step"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(6.0);
    ui.vertical(|ui| {
        for (i, (name, desc)) in ACQUIRE_METHODS.iter().enumerate() {
            let selected = s.method_idx == i;
            let btn = if selected {
                egui::Button::new(
                    RichText::new(format!("{}  {}", name, crate::i18n::t(*desc)))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .min_size(egui::Vec2::new(380.0, 28.0))
            } else {
                egui::Button::new(
                    RichText::new(format!("{}  {}", name, crate::i18n::t(*desc))).color(theme::TEXT_PRIMARY),
                )
                .fill(theme::BG_ELEVATED)
                .min_size(egui::Vec2::new(380.0, 28.0))
            };
            if ui.add(btn).clicked() {
                s.method_idx = i;
                // 自动进入下一步
                s.step = 1;
            }
            ui.add_space(2.0);
        }
    });
}

/// 步骤 1：选择芯片/产品
/// 点击选项后自动进入下一步
fn render_step_chip(ui: &mut egui::Ui, s: &mut WizardState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Select Chip/Product"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Click any chip/product below to select and proceed to the next step"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(6.0);
    ui.vertical(|ui| {
        for (i, (name, desc)) in CHIPS.iter().enumerate() {
            let selected = s.chip_idx == i;
            let btn = if selected {
                egui::Button::new(
                    RichText::new(format!("{}  {}", name, crate::i18n::t(*desc)))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .min_size(egui::Vec2::new(380.0, 28.0))
            } else {
                egui::Button::new(
                    RichText::new(format!("{}  {}", name, crate::i18n::t(*desc))).color(theme::TEXT_PRIMARY),
                )
                .fill(theme::BG_ELEVATED)
                .min_size(egui::Vec2::new(380.0, 28.0))
            };
            if ui.add(btn).clicked() {
                s.chip_idx = i;
                // 根据芯片预设默认引脚与 ADC 参数
                match name {
                    &"ESP32" => {
                        s.pin = "GPIO34".to_string();
                        s.adc_resolution = 12;
                        s.adc_ref_voltage = 3.3;
                    }
                    &"STM32" => {
                        s.pin = "PA0".to_string();
                        s.adc_resolution = 12;
                        s.adc_ref_voltage = 3.3;
                    }
                    _ => {
                        s.pin = String::new();
                        s.adc_resolution = 16;
                        s.adc_ref_voltage = 5.0;
                    }
                };
                // 自动进入下一步
                s.step = 2;
            }
            ui.add_space(2.0);
        }
    });
}

/// 步骤 2：配置采集参数
fn render_step_params(ui: &mut egui::Ui, state: &AppState, s: &mut WizardState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Configure Parameters"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(6.0);

    // AI 智能推荐采集配置：已连接设备且有接收数据时，基于协议识别推荐采集方式与参数
    if state.is_connected() {
        let rx_data = crate::panels::ai_sense::collect_recent_rx(&state.rx_buffer, 256);
        if rx_data.len() >= 16 {
            let classifier = miconlab_ai::ProtocolClassifier::with_builtin_templates();
            let features = miconlab_ai::StreamFeatures::extract(&rx_data, None);
            let (class, conf) = classifier.predict(&features);

            // 根据识别结果推荐采集方式（与 ACQUIRE_METHODS 顺序对应）
            let recommended_method: Option<usize> = match class {
                miconlab_ai::ProtocolClass::ModbusRtu
                | miconlab_ai::ProtocolClass::ModbusAscii => Some(4), // Modbus
                miconlab_ai::ProtocolClass::PlainText
                | miconlab_ai::ProtocolClass::Nmea
                | miconlab_ai::ProtocolClass::InsoleSensor => Some(3), // SerialFrame
                _ => None,
            };

            if let Some(method_idx) = recommended_method {
                let hint = crate::panels::ai_sense::AiHint::new(
                    "🤖",
                    theme::ACCENT,
                    format!(
                        "{} {} ({:.0}%) → {}",
                        { crate::i18n::t("Detected") },
                        crate::i18n::t_owned(class.label_zh()),
                        conf * 100.0,
                        ACQUIRE_METHODS[method_idx].0
                    ),
                );
                crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);

                // 应用 AI 推荐按钮
                let apply_btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Apply AI Suggestion"))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .min_size(egui::Vec2::new(150.0, 24.0));
                if ui
                    .add(apply_btn)
                    .on_hover_text(crate::i18n::t("Auto-fill recommended method and parameters based on AI protocol detection"))
                    .clicked()
                {
                    s.method_idx = method_idx;
                    // 根据协议推荐其他参数
                    match class {
                        miconlab_ai::ProtocolClass::ModbusRtu
                        | miconlab_ai::ProtocolClass::ModbusAscii => {
                            s.channel_count = 1;
                            s.sample_rate = 10.0;
                            s.unit = String::new();
                            s.range_min = 0.0;
                            s.range_max = 65535.0;
                            s.slave_id = 1;
                            s.register_addr = 0;
                            s.register_count = 1;
                            s.function_code = 3;
                            s.data_format = DataFormat::U16BE;
                        }
                        miconlab_ai::ProtocolClass::InsoleSensor => {
                            // 鞋垫压力传感器：AA 55 03 99 帧头，多通道压力矩阵 + IMU
                            s.channel_count = 16;
                            s.sample_rate = 100.0;
                            s.unit = "N/cm²".to_string();
                            s.range_min = 0.0;
                            s.range_max = 500.0;
                            s.header_hex = "AA 55 03 99".to_string();
                            s.footer_hex = String::new();
                            s.checksum_type = ChecksumType::Sum8;
                            s.data_format = DataFormat::U16LE;
                        }
                        _ => {
                            // PlainText / Nmea 等：使用 SerialFrame 默认配置
                            s.channel_count = 1;
                            s.sample_rate = 10.0;
                        }
                    }
                }
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
            }
        }
    }

    egui::Grid::new("wizard_params_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Channels##panels/sensor_dev/wizard.rs:15747"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut s.channel_count).range(1..=64))
                .on_hover_text(crate::i18n::t("Number of acquisition channels (1-64); determines how many sensor channels are sampled simultaneously"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Sample Rate (Hz)"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::DragValue::new(&mut s.sample_rate)
                    .range(1.0..=100000.0)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Samples per second (Hz); higher values give denser data but increase CPU load"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Pin/Address"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut s.pin)
                    .desired_width(160.0)
                    .font(egui::FontId::monospace(12.5)),
            )
            .on_hover_text(crate::i18n::t("ADC pin name (e.g. GPIO34) or I2C device address (e.g. 0x48)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Range Min"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut s.range_min).speed(0.01))
                .on_hover_text(crate::i18n::t("Sensor range minimum (physical quantity corresponding to raw ADC value)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Range Max"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(egui::DragValue::new(&mut s.range_max).speed(0.01))
                .on_hover_text(crate::i18n::t("Sensor range maximum (physical quantity corresponding to raw ADC value)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Unit")).color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut s.unit)
                    .desired_width(80.0)
                    .font(egui::FontId::monospace(12.5)),
            )
            .on_hover_text(crate::i18n::t("Engineering unit (e.g. V / degC / Pa / kg)"));
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // 协议参数（根据采集方式动态显示）
    let method = ACQUIRE_METHODS[s.method_idx].0;
    ui.label(
        RichText::new(crate::i18n::t("Protocol Parameters"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(4.0);

    match method {
        "ADC" => {
            egui::Grid::new("wizard_proto_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("ADC Resolution"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.adc_resolution)
                            .range(8..=32)
                            .suffix(" bit"),
                    )
                    .on_hover_text(crate::i18n::t("ADC resolution in bits (8/10/12/16/24); determines quantization precision"));
                    ui.end_row();
                    ui.label(
                        RichText::new(crate::i18n::t("Ref Voltage"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.adc_ref_voltage)
                            .range(0.1..=24.0)
                            .speed(0.05)
                            .suffix(" V"),
                    )
                    .on_hover_text(crate::i18n::t("ADC reference voltage (V); ESP32=3.3V, STM32=3.3V, external references may differ"));
                    ui.end_row();
                });
        }
        "I2C" => {
            ui.label(
                RichText::new(crate::i18n::t("Set I2C device address in Pin/Address field (e.g. 0x48)"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        }
        "SPI" => {
            egui::Grid::new("wizard_proto_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("SPI Mode"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.label(
                        RichText::new("Mode 0 (CPOL=0, CPHA=0)")
                            .color(theme::TEXT_PRIMARY)
                            .family(egui::FontFamily::Monospace),
                    );
                    ui.end_row();
                });
        }
        "SerialFrame" => {
            egui::Grid::new("wizard_proto_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Data Format"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("wizard_df_combo")
                        .selected_text(format!("{}", s.data_format))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.data_format, DataFormat::U8, "U8");
                            ui.selectable_value(&mut s.data_format, DataFormat::U16LE, "U16LE");
                            ui.selectable_value(&mut s.data_format, DataFormat::U16BE, "U16BE");
                            ui.selectable_value(&mut s.data_format, DataFormat::U32LE, "U32LE");
                            ui.selectable_value(&mut s.data_format, DataFormat::U32BE, "U32BE");
                            ui.selectable_value(&mut s.data_format, DataFormat::F32LE, "F32LE");
                            ui.selectable_value(&mut s.data_format, DataFormat::F32BE, "F32BE");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("U8/U16LE/U16BE/U32LE/U32BE/F32LE/F32BE; determines how frame data is parsed"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Header (hex)"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut s.header_hex)
                                .desired_width(120.0)
                                .font(egui::FontId::monospace(12.5)),
                        )
                        .on_hover_text(crate::i18n::t("Header in HEX (e.g. AA 55); used for frame synchronisation"));
                        if !s.header_hex.trim().is_empty()
                            && miconlab_core::hex::parse_hex(&s.header_hex).is_err()
                        {
                            ui.label(
                                RichText::new(crate::i18n::t("✗ invalid"))
                                    .color(theme::ERROR)
                                    .small(),
                            );
                        }
                    });
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Footer (hex)"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut s.footer_hex)
                                .desired_width(120.0)
                                .font(egui::FontId::monospace(12.5)),
                        )
                        .on_hover_text(crate::i18n::t("Footer in HEX (e.g. 55 AA); optional"));
                        if !s.footer_hex.trim().is_empty()
                            && miconlab_core::hex::parse_hex(&s.footer_hex).is_err()
                        {
                            ui.label(
                                RichText::new(crate::i18n::t("✗ invalid"))
                                    .color(theme::ERROR)
                                    .small(),
                            );
                        }
                    });
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Checksum"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("wizard_ck_combo")
                        .selected_text(format!("{}", s.checksum_type))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.checksum_type, ChecksumType::None, crate::i18n::t("None"));
                            ui.selectable_value(&mut s.checksum_type, ChecksumType::Sum8, crate::i18n::t("8-bit sum"));
                            ui.selectable_value(&mut s.checksum_type, ChecksumType::Xor, crate::i18n::t("XOR"));
                            ui.selectable_value(&mut s.checksum_type, ChecksumType::Crc16, "CRC16");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("None / Sum8 / XOR / CRC16; frame checksum algorithm"));
                    ui.end_row();
                });
        }
        "Modbus" => {
            egui::Grid::new("wizard_proto_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("Slave ID"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(egui::DragValue::new(&mut s.slave_id).range(1..=247))
                        .on_hover_text(crate::i18n::t("Modbus slave address (1-247)"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Register Addr"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.register_addr)
                            .range(0..=65535)
                            .speed(0.1),
                    )
                    .on_hover_text(crate::i18n::t("Modbus starting register address (0-65535)"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Register Count"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.register_count)
                            .range(1..=125)
                            .speed(0.1),
                    )
                    .on_hover_text(crate::i18n::t("Registers read per channel (1=single register, 2=32-bit data)"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Function Code"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("wizard_mb_fc_combo")
                        .selected_text(format!(
                            "{} ({})",
                            s.function_code,
                            if s.function_code == 3 {
                                "Holding"
                            } else {
                                "Input"
                            }
                        ))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.function_code, 3, "3 (Holding Register)");
                            ui.selectable_value(&mut s.function_code, 4, "4 (Input Register)");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("3=Read Holding Registers, 4=Read Input Registers, 2=Read Discrete Inputs, 1=Read Coils"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Data Format"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("wizard_mb_df_combo")
                        .selected_text(format!("{}", s.data_format))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.data_format, DataFormat::U16LE, "U16LE");
                            ui.selectable_value(&mut s.data_format, DataFormat::U16BE, "U16BE");
                            ui.selectable_value(&mut s.data_format, DataFormat::U32LE, "U32LE");
                            ui.selectable_value(&mut s.data_format, DataFormat::U32BE, "U32BE");
                            ui.selectable_value(&mut s.data_format, DataFormat::F32LE, "F32LE");
                            ui.selectable_value(&mut s.data_format, DataFormat::F32BE, "F32BE");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("U16LE/U16BE/U32LE/U32BE/F32LE/F32BE; how Modbus register data is parsed"));
                    ui.end_row();
                });
        }
        _ => {}
    }
}

/// 步骤 3：确认配置 → 生成摘要
fn render_step_summary(ui: &mut egui::Ui, s: &WizardState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Configuration Summary"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.add_space(6.0);

    let method = ACQUIRE_METHODS[s.method_idx].0;
    let chip = CHIPS[s.chip_idx].0;

    egui::Grid::new("wizard_summary_grid")
        .num_columns(2)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            summary_row(
                ui,
                crate::i18n::t("Method"),
                method.to_string(),
            );
            summary_row(ui, crate::i18n::t("Chip##panels/sensor_dev/wizard.rs:30020"), chip.to_string());
            summary_row(
                ui,
                crate::i18n::t("Channels##panels/sensor_dev/wizard.rs:30135"),
                format!("{}", s.channel_count),
            );
            summary_row(
                ui,
                crate::i18n::t("Sample Rate"),
                format!("{:.1} Hz", s.sample_rate),
            );
            summary_row(
                ui,
                crate::i18n::t("Pin/Address"),
                s.pin.clone(),
            );
            summary_row(
                ui,
                crate::i18n::t("Range##panels/sensor_dev/wizard.rs:30593"),
                format!("{:.3} ~ {:.3} {}", s.range_min, s.range_max, s.unit),
            );

            // 协议特定参数摘要
            match method {
                "ADC" => {
                    summary_row(
                        ui,
                        crate::i18n::t("ADC Resolution"),
                        format!("{} bit", s.adc_resolution),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Ref Voltage"),
                        format!("{:.2} V", s.adc_ref_voltage),
                    );
                }
                "I2C" => {
                    summary_row(
                        ui,
                        crate::i18n::t("I2C Address"),
                        s.pin.clone(),
                    );
                }
                "SerialFrame" => {
                    summary_row(
                        ui,
                        crate::i18n::t("Data Format"),
                        format!("{}", s.data_format),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Header"),
                        s.header_hex.clone(),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Footer"),
                        s.footer_hex.clone(),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Checksum"),
                        format!("{}", s.checksum_type),
                    );
                }
                "Modbus" => {
                    summary_row(
                        ui,
                        crate::i18n::t("Slave ID"),
                        format!("{}", s.slave_id),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Register Addr"),
                        format!("0x{:04X}", s.register_addr),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Register Count"),
                        format!("{}", s.register_count),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Function Code"),
                        format!(
                            "{} ({})",
                            s.function_code,
                            if s.function_code == 3 { "Holding" } else { "Input" }
                        ),
                    );
                    summary_row(
                        ui,
                        crate::i18n::t("Data Format"),
                        format!("{}", s.data_format),
                    );
                }
                _ => {}
            }
        });

    ui.add_space(8.0);
    ui.label(
        RichText::new(crate::i18n::t("Click Finish to generate acquisition config and proceed."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
}

/// 步骤 4：开始采集提示
fn render_step_finish(ui: &mut egui::Ui, s: &mut WizardState, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Configuration Complete"))
            .color(theme::SUCCESS)
            .strong(),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(crate::i18n::t("Acquisition config generated. Switch to the Monitor tab to view data."))
        .color(theme::TEXT_PRIMARY),
    );
    ui.add_space(8.0);

    // 显示生成的配置概览
    ui.label(
        RichText::new(crate::i18n::t("Generated Config:"))
            .color(theme::CYAN)
            .small(),
    );
    let cfg = build_config(s);
    ui.label(
        RichText::new(format!(
            "{} {} @ {:.0}Hz, {} {}",
            cfg.channels.len(),
            crate::i18n::t("channels"),
            cfg.sample_rate,
            s.pin,
            s.unit
        ))
        .color(theme::TEXT_PRIMARY)
        .family(egui::FontFamily::Monospace),
    );

    // 已完成标记
    s.completed = true;
}

/// 根据向导状态构建 AcquisitionConfig
fn build_config(s: &WizardState) -> AcquisitionConfig {
    let channels: Vec<ChannelConfig> = (0..s.channel_count)
        .map(|i| {
            let color = CHANNEL_COLORS[i % CHANNEL_COLORS.len()];
            ChannelConfig {
                name: format!("CH{}", i),
                channel_index: i as u8,
                gain: 1.0,
                offset: 0.0,
                unit: s.unit.clone(),
                calibration: None,
                enabled: true,
                color,
            }
        })
        .collect();

    AcquisitionConfig {
        sample_rate: s.sample_rate,
        channels,
        trigger: None,
        buffer_size: 10000,
        auto_record: false,
        record_path: None,
    }
}

/// 摘要表格行：标签 + 值
fn summary_row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(RichText::new(label).color(theme::TEXT_SECONDARY).small());
    ui.label(
        RichText::new(value)
            .color(theme::TEXT_PRIMARY)
            .family(egui::FontFamily::Monospace),
    );
    ui.end_row();
}

// ===== 公共 API 供其他子面板使用 =====

/// 取出当前向导生成的采集配置（若向导未完成则返回默认配置）。
/// 在 monitor.rs 中调用以获取用户配置的通道数与采样率。
pub fn current_config(s: &WizardState) -> AcquisitionConfig {
    build_config(s)
}

/// 通知向导用户已在监控中开始采集（仅作标记，不锁死向导导航）
pub fn mark_started(s: &mut WizardState) {
    s.monitor_started = true;
}

/// 取出当前向导生成的协议配置（包含协议类型、帧格式、Modbus 寄存器映射等）。
/// 在 monitor.rs 中调用以按协议解析串口数据。
pub fn current_protocol_config(s: &WizardState) -> ProtocolConfig {
    build_protocol_config(s)
}

/// 应用协议分析器的识别结果到向导配置。
/// 在 protocol_analyzer.rs 的「应用识别结果」流程中调用，
/// 使后续 monitor.rs 通过 current_protocol_config() 获取到识别到的协议参数。
pub fn apply_detected_protocol(
    s: &mut WizardState,
    protocol_type: ProtocolType,
    header: &[u8],
    footer: &[u8],
    checksum_type: ChecksumType,
    data_format: DataFormat,
    channel_count: usize,
) {
    // 根据协议类型设置 method_idx（与 ACQUIRE_METHODS 顺序一致）
    s.method_idx = match protocol_type {
        ProtocolType::AdcDirect => 0,
        ProtocolType::I2c => 1,
        ProtocolType::Spi => 2,
        ProtocolType::SerialFrame => 3,
        ProtocolType::ModbusRtu => 4,
    };
    // 更新协议参数
    s.data_format = data_format;
    s.header_hex = miconlab_core::hex::format_hex_spaced(header);
    s.footer_hex = miconlab_core::hex::format_hex_spaced(footer);
    s.checksum_type = checksum_type;
    // 钳制到 [1, 64]，避免 as u8 截断和超出 DragValue 范围
    s.channel_count = channel_count.clamp(1, 64);
}

// 根据向导状态生成 ProtocolConfig（使用用户配置的协议参数）
fn build_protocol_config(s: &WizardState) -> ProtocolConfig {
    let method = ACQUIRE_METHODS[s.method_idx].0;
    let protocol_type = match method {
        "ADC" => ProtocolType::AdcDirect,
        "I2C" => ProtocolType::I2c,
        "SPI" => ProtocolType::Spi,
        "SerialFrame" => ProtocolType::SerialFrame,
        "Modbus" => ProtocolType::ModbusRtu,
        _ => ProtocolType::AdcDirect,
    };

    // 解析帧头/帧尾 hex 字符串
    let header = miconlab_core::hex::parse_hex(&s.header_hex).unwrap_or_default();
    let footer = miconlab_core::hex::parse_hex(&s.footer_hex).unwrap_or_default();

    ProtocolConfig {
        protocol_type,
        baud_rate: s.baud_rate,
        slave_id: if method == "Modbus" { Some(s.slave_id) } else { None },
        frame_format: if method == "SerialFrame" {
            Some(FrameFormat {
                header,
                footer,
                length_offset: None,
                length_bytes: 0,
                data_offset: 0,
                checksum_type: s.checksum_type.clone(),
                checksum_offset: None,
                channel_count: s.channel_count,
                data_format: s.data_format.clone(),
            })
        } else {
            None
        },
        commands: Vec::new(),
        register_map: if method == "Modbus" {
            // 按 channel_count 生成 N 条映射，每条对应一个通道
            // 寄存器地址按 register_count 递增（每个通道占 register_count 个寄存器）
            (0..s.channel_count)
                .map(|i| RegisterMapping {
                    register_addr: s.register_addr.saturating_add(
                        (i as u16).saturating_mul(s.register_count),
                    ),
                    channel_name: format!("CH{}", i),
                    quantity: s.register_count,
                    scale: 1.0,
                    offset: 0.0,
                    unit: s.unit.clone(),
                    function_code: s.function_code,
                })
                .collect()
        } else {
            Vec::new()
        },
        i2c_address: if method == "I2C" {
            parse_i2c_address(&s.pin)
        } else {
            None
        },
        adc_resolution: if method == "ADC" { Some(s.adc_resolution) } else { None },
        adc_ref_voltage: if method == "ADC" { Some(s.adc_ref_voltage) } else { None },
    }
}

/// 解析 I2C 地址，支持多种格式：
/// - "0x48" / "0X48"（十六进制带前缀）
/// - "72"（十进制无前缀）
/// - "0x100" 等越界值返回 None
fn parse_i2c_address(input: &str) -> Option<u8> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 优先尝试 0x/0X 十六进制前缀
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return u8::from_str_radix(hex, 16).ok();
    }
    // 尝试十进制
    trimmed.parse::<u8>().ok()
}
