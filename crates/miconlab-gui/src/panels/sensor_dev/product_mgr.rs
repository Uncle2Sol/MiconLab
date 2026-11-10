// 产品注册管理子面板
// 功能：产品列表(搜索) / 详情 / 新增 / 编辑 / 删除 / JSON导入导出 / 加载内置库
//
// 状态通过 state_manager 在 CentralPanel 外部加载/保存，闭包内传入 &mut ProductMgrState。
// ProductMgrState 对 crate 内公开，供 calibration 子面板"应用校准"时访问产品库。

use crate::state::AppState;
use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use miconlab_core::product::*;

/// 产品管理面板持久化状态
#[derive(Clone)]
pub(crate) struct ProductMgrState {
    /// 产品库（pub(crate) 供 calibration 子面板访问）
    pub library: ProductLibrary,
    /// 当前选中的产品 ID
    selected_id: Option<String>,
    /// 搜索关键字
    search: String,
    /// 编辑中的产品（Some 时显示编辑表单）
    editing: Option<EditState>,
    /// 导入文件路径
    import_path: String,
    /// 导出文件路径
    export_path: String,
    /// 面板状态消息（文本 + 颜色）
    status_msg: Option<(String, Color32)>,
}

impl Default for ProductMgrState {
    fn default() -> Self {
        Self {
            library: ProductLibrary::with_builtin(),
            selected_id: None,
            search: String::new(),
            editing: None,
            import_path: String::new(),
            export_path: String::new(),
            status_msg: None,
        }
    }
}

/// 编辑状态：持有产品副本 + 是否为新建
#[derive(Clone)]
struct EditState {
    product: Product,
    is_new: bool,
}

/// 编辑表单的动作
enum EditAction {
    None,
    Save,
    Cancel,
}

// ===== 辅助函数 =====

/// 生成新产品 ID
fn gen_id() -> String {
    format!("custom-{}", chrono::Local::now().timestamp_millis())
}

/// 设置面板状态消息
fn set_status(pm: &mut ProductMgrState, msg: impl Into<String>, color: Color32) {
    pm.status_msg = Some((msg.into(), color));
}

/// 渲染产品管理子面板
pub fn show(_ctx: &egui::Context, state: &mut AppState, ui: &mut egui::Ui, pm: &mut ProductMgrState) {
    let zh = state.language == crate::i18n::Language::Zh;

    // 副标题
    ui.label(
        RichText::new(crate::i18n::t("Manage the sensor product model library - channel definitions, protocol config, calibration params"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // 顶部工具栏
    render_toolbar(ui, state, pm, zh);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // 状态消息
    if let Some((msg, color)) = pm.status_msg.as_ref() {
        ui.label(RichText::new(msg).color(*color).small());
        ui.add_space(2.0);
    }

    // 左右分栏：左 30% 列表，右 70% 详情/编辑
    let avail = ui.available_size();
    let left_w = (avail.x * 0.30).max(200.0).min(350.0);
    let right_w = (avail.x - left_w - 12.0).max(100.0);

    ui.horizontal(|ui| {
        // 左侧：产品列表
        ui.allocate_ui(egui::Vec2::new(left_w, avail.y), |ui| {
            render_list_panel(ui, pm, zh);
        });
        ui.separator();
        // 右侧：详情或编辑表单
        ui.allocate_ui(egui::Vec2::new(right_w, avail.y), |ui| {
            render_right_panel(ui, state, pm, zh);
        });
    });
}

/// 渲染顶部工具栏
fn render_toolbar(ui: &mut egui::Ui, state: &mut AppState, pm: &mut ProductMgrState, zh: bool) {
    ui.horizontal(|ui| {
        let is_editing = pm.editing.is_some();
        let has_selection = pm.selected_id.is_some();

        // 新增按钮
        if ui
            .add(
                egui::Button::new(
                    RichText::new(crate::i18n::t("+ New"))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::SUCCESS),
            )
            .on_hover_text(crate::i18n::t("Create a blank product template, fill in name/model/channels/protocol/calibration in the edit form and save"))
            .clicked()
        {
            let p = Product {
                id: gen_id(),
                created_at: chrono::Local::now().to_rfc3339(),
                unit: "N".to_string(),
                ..Default::default()
            };
            pm.editing = Some(EditState { product: p, is_new: true });
            state.last_error = None;
        }

        // 编辑按钮（仅在有选中且未在编辑时可用）
        ui.add_enabled_ui(has_selection && !is_editing, |ui| {
            if ui.button(crate::i18n::t("Edit")).clicked() {
                let product = pm
                    .selected_id
                    .as_ref()
                    .and_then(|id| pm.library.get(id).cloned());
                if let Some(p) = product {
                    pm.editing = Some(EditState { product: p, is_new: false });
                }
            }
        });

        // 删除按钮
        ui.add_enabled_ui(has_selection && !is_editing, |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(crate::i18n::t("Delete")).color(Color32::BLACK),
                    )
                    .fill(theme::ERROR),
                )
                .on_hover_text(crate::i18n::t("Permanently delete the selected product from the library (cannot be undone)"))
                .clicked()
            {
                if let Some(id) = pm.selected_id.take() {
                    pm.library.remove(&id);
                    set_status(
                        pm,
                        crate::i18n::t("Product deleted"),
                        theme::WARNING,
                    );
                }
            }
        });

        ui.separator();

        // 导入
        ui.label(
            RichText::new(crate::i18n::t("Import:"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut pm.import_path)
                .desired_width(110.0)
                .hint_text(crate::i18n::t("JSON path")),
        );
        if ui
            .button(crate::i18n::t("Import"))
            .on_hover_text(crate::i18n::t("Import a single product from the given JSON file into the library and auto-select it. JSON must match the Product serialization format"))
            .clicked()
        {
            do_import(state, pm, zh);
        }

        // 导出
        ui.label(
            RichText::new(crate::i18n::t("Export:"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut pm.export_path)
                .desired_width(110.0)
                .hint_text(crate::i18n::t("JSON path")),
        );
        if ui
            .button(crate::i18n::t("Export"))
            .on_hover_text(crate::i18n::t("Export the selected product to a JSON file for backup or sharing across machines"))
            .clicked()
        {
            do_export(state, pm, zh);
        }

        ui.separator();

        // 加载内置库
        if ui
            .button(crate::i18n::t("Load Builtin"))
            .on_hover_text(crate::i18n::t("Reset the library to the 5 built-in sample products (all custom products are discarded; export a backup first)"))
            .clicked()
        {
            pm.library = ProductLibrary::with_builtin();
            pm.selected_id = None;
            set_status(
                pm,
                crate::i18n::t("Loaded 5 builtin products"),
                theme::SUCCESS,
            );
            state.last_error = None;
        }

        // 产品数量（右对齐）
        let count = pm.library.products.len();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "{}: {}",
                    crate::i18n::t("Products"),
                    count
                ))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        });
    });
}

/// 执行导入
fn do_import(state: &mut AppState, pm: &mut ProductMgrState, _zh: bool) {
    let path = pm.import_path.clone();
    if path.is_empty() {
        state.last_error = Some(crate::i18n::t("Enter file path").to_string());
        set_status(
            pm,
            crate::i18n::t("Enter file path"),
            theme::ERROR,
        );
        return;
    }
    match pm.library.import_product(std::path::Path::new(&path)) {
        Ok(product) => {
            let id = product.id.clone();
            pm.library.add(product);
            pm.selected_id = Some(id);
            state.last_error = None;
            set_status(pm, crate::i18n::t("Import OK"), theme::SUCCESS);
        }
        Err(e) => {
            state.last_error = Some(e.clone());
            set_status(
                pm,
                format!("{}: {}", crate::i18n::t("Import failed"), e),
                theme::ERROR,
            );
        }
    }
}

/// 执行导出
fn do_export(state: &mut AppState, pm: &mut ProductMgrState, _zh: bool) {
    let path = pm.export_path.clone();
    if path.is_empty() {
        state.last_error = Some(crate::i18n::t("Enter file path").to_string());
        set_status(
            pm,
            crate::i18n::t("Enter file path"),
            theme::ERROR,
        );
        return;
    }
    let id = match pm.selected_id.clone() {
        Some(id) => id,
        None => {
            state.last_error =
                Some(crate::i18n::t("Select a product first").to_string());
            set_status(
                pm,
                crate::i18n::t("Select a product first"),
                theme::ERROR,
            );
            return;
        }
    };
    match pm.library.export_product(&id, std::path::Path::new(&path)) {
        Ok(()) => {
            set_status(pm, crate::i18n::t("Export OK"), theme::SUCCESS);
        }
        Err(e) => {
            state.last_error = Some(e.clone());
            set_status(
                pm,
                format!("{}: {}", crate::i18n::t("Export failed"), e),
                theme::ERROR,
            );
        }
    }
}

/// 渲染左侧产品列表
fn render_list_panel(ui: &mut egui::Ui, pm: &mut ProductMgrState, _zh: bool) {
    // 搜索框
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Search"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add(
            egui::TextEdit::singleline(&mut pm.search)
                .desired_width(140.0)
                .hint_text(crate::i18n::t("Name/Model")),
        )
        .on_hover_text(crate::i18n::t("Fuzzy search by name or model"));
    });
    ui.add_space(4.0);

    // 取出过滤后的产品列表（owned 数据，避免在渲染时持有借用）
    let search = pm.search.to_lowercase();
    let selected_id = pm.selected_id.clone();
    let products: Vec<(String, String, String, String)> = pm
        .library
        .list()
        .iter()
        .filter_map(|p| {
            if search.is_empty()
                || p.name.to_lowercase().contains(&search)
                || p.model.to_lowercase().contains(&search)
            {
                Some((
                    p.id.clone(),
                    p.name.clone(),
                    p.model.clone(),
                    p.category.to_string(),
                ))
            } else {
                None
            }
        })
        .collect();

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if products.is_empty() {
                ui.label(
                    RichText::new(crate::i18n::t("No products"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            }
            for (id, name, model, category) in products {
                let is_selected = selected_id.as_ref() == Some(&id);
                let frame = if is_selected {
                    egui::Frame::group(ui.style())
                        .fill(theme::ACCENT_DIM)
                        .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT))
                        .inner_margin(egui::Margin::same(6.0))
                } else {
                    egui::Frame::group(ui.style())
                        .fill(theme::BG_ELEVATED)
                        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                        .inner_margin(egui::Margin::same(6.0))
                };
                let resp = frame.show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(if name.is_empty() {
                                format!("({})", crate::i18n::t("unnamed"))
                            } else {
                                name.clone()
                            })
                            .color(theme::TEXT_PRIMARY)
                            .strong(),
                        );
                        ui.label(
                            RichText::new(format!("{} | {}", model, category))
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );
                    });
                });
                if resp.response.clicked() {
                    pm.selected_id = Some(id);
                }
            }
        });
}

/// 渲染右侧面板：编辑表单或产品详情
fn render_right_panel(ui: &mut egui::Ui, state: &mut AppState, pm: &mut ProductMgrState, zh: bool) {
    let is_editing = pm.editing.is_some();
    if is_editing {
        let action = render_edit_form(ui, pm, zh);
        match action {
            EditAction::Save => {
                let saved_id = pm.editing.take().map(|es| {
                    let id = es.product.id.clone();
                    pm.library.add(es.product);
                    id
                });
                if let Some(id) = saved_id {
                    pm.selected_id = Some(id);
                    set_status(pm, crate::i18n::t("Saved"), theme::SUCCESS);
                    state.last_error = None;
                }
            }
            EditAction::Cancel => {
                pm.editing = None;
                set_status(
                    pm,
                    crate::i18n::t("Edit cancelled"),
                    theme::TEXT_SECONDARY,
                );
            }
            EditAction::None => {}
        }
    } else {
        render_detail(ui, pm, zh);
    }
}

/// 渲染编辑表单
fn render_edit_form(ui: &mut egui::Ui, pm: &mut ProductMgrState, zh: bool) -> EditAction {
    let mut action = EditAction::None;
    let es = match pm.editing.as_mut() {
        Some(es) => es,
        None => return action,
    };
    let product = &mut es.product;

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let title = if es.is_new {
                crate::i18n::t("New Product")
            } else {
                crate::i18n::t("Edit Product")
            };
            ui.label(RichText::new(title).color(theme::CYAN).strong());
            ui.add_space(8.0);

            // ===== 基本信息 =====
            ui.label(RichText::new(crate::i18n::t("Basic Info")).color(theme::ACCENT).strong());
            ui.add_space(2.0);
            egui::Grid::new("edit_basic")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(crate::i18n::t("ID")).color(theme::TEXT_SECONDARY));
                    if es.is_new {
                        ui.add(
                            egui::TextEdit::singleline(&mut product.id)
                                .desired_width(220.0)
                                .font(egui::FontId::monospace(12.0)),
                        )
                        .on_hover_text(crate::i18n::t("Unique product identifier (alphanumeric, e.g. SHT31_PRO)"));
                    } else {
                        ui.label(
                            RichText::new(&product.id)
                                .color(theme::TEXT_DISABLED)
                                .small()
                                .family(egui::FontFamily::Monospace),
                        );
                    }
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Name")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut product.name).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Product display name"));
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Model")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut product.model).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Product model (e.g. SHT31, BMP280)"));
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Manufacturer")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut product.manufacturer).desired_width(220.0))
                        .on_hover_text(crate::i18n::t("Manufacturer name"));
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Category")).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        render_category_combo(ui, &mut product.category, zh);
                    });
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Sensor Type")).color(theme::TEXT_SECONDARY));
                    ui.horizontal(|ui| {
                        render_sensor_type_combo(ui, &mut product.sensor_type, zh);
                    });
                    ui.end_row();
                });

            ui.add_space(8.0);

            // ===== 量程参数 =====
            ui.label(RichText::new(crate::i18n::t("Range")).color(theme::ACCENT).strong());
            ui.add_space(2.0);
            egui::Grid::new("edit_range")
                .num_columns(4)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(crate::i18n::t("Min")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::DragValue::new(&mut product.range_min).speed(0.01))
                        .on_hover_text(crate::i18n::t("Sensor measurement range (physical quantity)"));
                    ui.label(RichText::new(crate::i18n::t("Max##panels/sensor_dev/product_mgr.rs:18023")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::DragValue::new(&mut product.range_max).speed(0.01))
                        .on_hover_text(crate::i18n::t("Sensor measurement range (physical quantity)"));
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Sensitivity")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::DragValue::new(&mut product.sensitivity).speed(0.01))
                        .on_hover_text(crate::i18n::t("Sensor sensitivity coefficient"));
                    ui.label(RichText::new(crate::i18n::t("Unit")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::TextEdit::singleline(&mut product.unit).desired_width(60.0))
                        .on_hover_text(crate::i18n::t("Unit of the measured physical quantity"));
                    ui.end_row();
                });

            ui.add_space(8.0);

            // ===== 描述与备注 =====
            ui.label(RichText::new(crate::i18n::t("Description")).color(theme::ACCENT).strong());
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut product.description)
                    .desired_width(400.0)
                    .desired_rows(2),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(crate::i18n::t("Notes")).color(theme::ACCENT).strong());
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut product.notes)
                    .desired_width(400.0)
                    .desired_rows(2),
            );

            ui.add_space(8.0);

            // ===== 通道列表 =====
            render_channels_editor(ui, &mut product.channels, zh);

            ui.add_space(8.0);

            // ===== 协议配置 =====
            render_protocol_editor(ui, &mut product.protocol, zh);

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            // ===== 保存/取消按钮 =====
            ui.horizontal(|ui| {
                let can_save = !product.id.is_empty();
                ui.add_enabled_ui(can_save, |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(crate::i18n::t("Save"))
                                    .color(Color32::BLACK)
                                    .strong(),
                            )
                            .fill(theme::SUCCESS)
                            .min_size(egui::Vec2::new(100.0, 28.0)),
                        )
                        .on_hover_text(crate::i18n::t("Save changes to the product library (in-memory; export to persist)"))
                        .clicked()
                    {
                        action = EditAction::Save;
                    }
                });
                if ui
                    .add(
                        egui::Button::new(crate::i18n::t("Cancel"))
                            .min_size(egui::Vec2::new(80.0, 28.0)),
                    )
                    .on_hover_text(crate::i18n::t("Discard changes and exit edit mode"))
                    .clicked()
                {
                    action = EditAction::Cancel;
                }
            });
        });
    action
}

/// 渲染类别下拉框（含自定义选项）
fn render_category_combo(ui: &mut egui::Ui, category: &mut ProductCategory, _zh: bool) {
    let selected = category.to_string();
    egui::ComboBox::from_id_salt("edit_category")
        .selected_text(selected)
        .width(120.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(category, ProductCategory::FlexibleSensor, crate::i18n::t("Flexible sensor"));
            ui.selectable_value(category, ProductCategory::PressureArray, crate::i18n::t("Pressure array"));
            ui.selectable_value(category, ProductCategory::AcquisitionModule, crate::i18n::t("Acquisition module"));
            ui.selectable_value(category, ProductCategory::Accessory, crate::i18n::t("Accessory"));
            let is_custom = matches!(category, ProductCategory::Custom(_));
            if ui
                .selectable_label(is_custom, crate::i18n::t("Custom"))
                .clicked()
            {
                *category = ProductCategory::Custom(String::new());
            }
        })
        .response
        .on_hover_text(crate::i18n::t("Product category (sensor / actuator / display, etc.)"));
    // 自定义类别名称输入
    if let ProductCategory::Custom(ref mut s) = category {
        ui.add(
            egui::TextEdit::singleline(s)
                .desired_width(80.0)
                .hint_text(crate::i18n::t("name")),
        );
    }
}

/// 渲染传感器类型下拉框（含其他选项）
fn render_sensor_type_combo(ui: &mut egui::Ui, sensor_type: &mut SensorType, _zh: bool) {
    let selected = sensor_type.to_string();
    egui::ComboBox::from_id_salt("edit_sensor_type")
        .selected_text(selected)
        .width(120.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(sensor_type, SensorType::Resistive, crate::i18n::t("Resistive"));
            ui.selectable_value(sensor_type, SensorType::Capacitive, crate::i18n::t("Capacitive"));
            ui.selectable_value(sensor_type, SensorType::Piezoelectric, crate::i18n::t("Piezoelectric"));
            ui.selectable_value(sensor_type, SensorType::Optical, crate::i18n::t("Optical"));
            ui.selectable_value(sensor_type, SensorType::StrainGauge, crate::i18n::t("Strain gauge"));
            let is_other = matches!(sensor_type, SensorType::Other(_));
            if ui
                .selectable_label(is_other, crate::i18n::t("Other"))
                .clicked()
            {
                *sensor_type = SensorType::Other(String::new());
            }
        })
        .response
        .on_hover_text(crate::i18n::t("Sensor working principle (temperature / humidity / pressure / IMU, etc.)"));
    if let SensorType::Other(ref mut s) = sensor_type {
        ui.add(
            egui::TextEdit::singleline(s)
                .desired_width(80.0)
                .hint_text(crate::i18n::t("name")),
        );
    }
}

/// 渲染通道列表编辑器
fn render_channels_editor(ui: &mut egui::Ui, channels: &mut Vec<ChannelDef>, _zh: bool) {
    ui.label(RichText::new(crate::i18n::t("Channels")).color(theme::ACCENT).strong());
    ui.add_space(2.0);

    let mut remove_idx: Option<usize> = None;
    for (i, ch) in channels.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("#{}", i))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
            );
            ui.add(
                egui::TextEdit::singleline(&mut ch.name)
                    .desired_width(60.0)
                    .hint_text("name"),
            )
            .on_hover_text(crate::i18n::t("Channel name (e.g. CH0, TEMP, PRESS)"));
            egui::ComboBox::from_id_salt(format!("ch_type_{}", i))
                .selected_text(ch.channel_type.to_string())
                .width(70.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut ch.channel_type, ChannelType::Adc, "ADC");
                    ui.selectable_value(&mut ch.channel_type, ChannelType::I2c, "I2C");
                    ui.selectable_value(&mut ch.channel_type, ChannelType::Spi, "SPI");
                    ui.selectable_value(&mut ch.channel_type, ChannelType::Serial, "Serial");
                    ui.selectable_value(&mut ch.channel_type, ChannelType::Modbus, "Modbus");
                })
                .response
                .on_hover_text(crate::i18n::t("Channel acquisition method (ADC / I2C / SPI / Serial / Modbus)"));
            ui.add(
                egui::TextEdit::singleline(&mut ch.pin)
                    .desired_width(50.0)
                    .hint_text("pin"),
            )
            .on_hover_text(crate::i18n::t("Pin name or bus address for this channel"));
            ui.add(egui::DragValue::new(&mut ch.channel_index).range(0..=255).prefix("idx:"))
                .on_hover_text(crate::i18n::t("Channel index (0-255); used to distinguish channels in the data frame"));
            ui.add(egui::DragValue::new(&mut ch.gain).speed(0.01).prefix("g:"))
                .on_hover_text(crate::i18n::t("Gain coefficient (raw value × gain = physical quantity)"));
            ui.add(egui::DragValue::new(&mut ch.offset).speed(0.01).prefix("o:"))
                .on_hover_text(crate::i18n::t("Offset (raw value × gain + offset = physical quantity)"));
            ui.add(
                egui::TextEdit::singleline(&mut ch.unit)
                    .desired_width(35.0)
                    .hint_text("unit"),
            )
            .on_hover_text(crate::i18n::t("Engineering unit of the channel measurement"));
            ui.checkbox(&mut ch.enabled, crate::i18n::t("on"))
                .on_hover_text(crate::i18n::t("When checked the channel is sampled; otherwise skipped"));
            if ui
                .add(
                    egui::Button::new(RichText::new("×").color(theme::ERROR))
                        .min_size(egui::Vec2::new(20.0, 20.0)),
                )
                .on_hover_text(crate::i18n::t("Remove this channel"))
                .clicked()
            {
                remove_idx = Some(i);
            }
        });
        ui.add_space(1.0);
    }
    if let Some(i) = remove_idx {
        channels.remove(i);
    }
    if ui
        .add(
            egui::Button::new(crate::i18n::t("+ Add Channel"))
                .fill(theme::BG_ELEVATED),
        )
        .clicked()
    {
        channels.push(ChannelDef {
            name: format!("CH{}", channels.len()),
            ..Default::default()
        });
    }
}

/// 渲染协议配置编辑器
fn render_protocol_editor(ui: &mut egui::Ui, protocol: &mut ProtocolConfig, _zh: bool) {
    ui.label(
        RichText::new(crate::i18n::t("Protocol Config"))
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(2.0);

    egui::Grid::new("edit_protocol")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::t("Protocol")).color(theme::TEXT_SECONDARY));
            egui::ComboBox::from_id_salt("edit_proto_type")
                .selected_text(protocol.protocol_type.to_string())
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut protocol.protocol_type, ProtocolType::AdcDirect, crate::i18n::t("ADC direct"));
                    ui.selectable_value(&mut protocol.protocol_type, ProtocolType::I2c, "I2C");
                    ui.selectable_value(&mut protocol.protocol_type, ProtocolType::Spi, "SPI");
                    ui.selectable_value(&mut protocol.protocol_type, ProtocolType::SerialFrame, crate::i18n::t("Serial frame"));
                    ui.selectable_value(&mut protocol.protocol_type, ProtocolType::ModbusRtu, "Modbus RTU");
                })
                .response
                .on_hover_text(crate::i18n::t("Protocol type: ADC Direct / I2C / SPI / Serial Frame / Modbus RTU"));
            ui.end_row();

            // 根据协议类型显示不同字段
            match protocol.protocol_type {
                ProtocolType::AdcDirect => {
                    ui.label(RichText::new(crate::i18n::t("ADC Res")).color(theme::TEXT_SECONDARY));
                    let mut res = protocol.adc_resolution.unwrap_or(12);
                    ui.add(egui::DragValue::new(&mut res).range(8..=24).suffix(" bit"))
                        .on_hover_text(crate::i18n::t("ADC resolution in bits (8/10/12/16/24); determines quantization precision"));
                    protocol.adc_resolution = Some(res);
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Ref Voltage")).color(theme::TEXT_SECONDARY));
                    let mut vref = protocol.adc_ref_voltage.unwrap_or(3.3);
                    ui.add(egui::DragValue::new(&mut vref).speed(0.01).suffix(" V"))
                        .on_hover_text(crate::i18n::t("ADC reference voltage (V); ESP32=3.3V, STM32=3.3V, external references may differ"));
                    protocol.adc_ref_voltage = Some(vref);
                    ui.end_row();
                }
                ProtocolType::I2c => {
                    ui.label(RichText::new(crate::i18n::t("I2C Addr")).color(theme::TEXT_SECONDARY));
                    let mut addr = protocol.i2c_address.unwrap_or(0x48);
                    ui.add(
                        egui::DragValue::new(&mut addr)
                            .range(0..=127)
                            .custom_formatter(|n, _| format!("0x{:02X}", n as u8)),
                    )
                    .on_hover_text(crate::i18n::t("I2C device 7-bit address (0x00-0x7F)"));
                    protocol.i2c_address = Some(addr);
                    ui.end_row();
                }
                ProtocolType::Spi => {
                    ui.label(
                        RichText::new(crate::i18n::t("(no special params)"))
                            .color(theme::TEXT_DISABLED)
                            .small(),
                    );
                    ui.end_row();
                }
                ProtocolType::SerialFrame => {
                    ui.label(RichText::new(crate::i18n::t("Baud")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::DragValue::new(&mut protocol.baud_rate).range(1200..=921600))
                        .on_hover_text(crate::i18n::t("Serial communication baud rate (bps)"));
                    ui.end_row();
                }
                ProtocolType::ModbusRtu => {
                    ui.label(RichText::new(crate::i18n::t("Baud")).color(theme::TEXT_SECONDARY));
                    ui.add(egui::DragValue::new(&mut protocol.baud_rate).range(1200..=921600))
                        .on_hover_text(crate::i18n::t("Modbus serial communication baud rate (bps)"));
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Slave ID")).color(theme::TEXT_SECONDARY));
                    let mut sid = protocol.slave_id.unwrap_or(1);
                    ui.add(egui::DragValue::new(&mut sid).range(1..=247))
                        .on_hover_text(crate::i18n::t("Modbus slave address (1-247)"));
                    protocol.slave_id = Some(sid);
                    ui.end_row();
                }
            }
        });
}

/// 渲染产品详情（只读）
fn render_detail(ui: &mut egui::Ui, pm: &ProductMgrState, _zh: bool) {
    let product: Option<Product> = pm
        .selected_id
        .as_ref()
        .and_then(|id| pm.library.get(id).cloned());

    let product = match product {
        Some(p) => p,
        None => {
            ui.label(
                RichText::new(crate::i18n::t("Select a product from the left"))
                .color(theme::TEXT_DISABLED),
            );
            return;
        }
    };

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // 标题
            ui.label(
                RichText::new(if product.name.is_empty() {
                    format!("({})", crate::i18n::t("unnamed"))
                } else {
                    product.name.clone()
                })
                .color(theme::TEXT_PRIMARY)
                .strong()
                .size(16.0),
            );
            ui.label(
                RichText::new(format!("{} | {}", product.model, product.manufacturer))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add_space(8.0);

            // 基本信息
            detail_grid(ui, "detail_basic", crate::i18n::t("Basic Info"), |ui| {
                detail_row(ui, "ID", &product.id);
                detail_row(ui, crate::i18n::t("Model"), &product.model);
                detail_row(ui, crate::i18n::t("Manufacturer"), &product.manufacturer);
                detail_row(ui, crate::i18n::t("Category"), &product.category.to_string());
                detail_row(
                    ui,
                    crate::i18n::t("Sensor Type"),
                    &product.sensor_type.to_string(),
                );
                detail_row(
                    ui,
                    crate::i18n::t("Created"),
                    &product.created_at,
                );
            });

            // 量程参数
            detail_grid(ui, "detail_range", crate::i18n::t("Range"), |ui| {
                detail_row(
                    ui,
                    crate::i18n::t("Range##panels/sensor_dev/product_mgr.rs:34431"),
                    &format!("{:.3} ~ {:.3} {}", product.range_min, product.range_max, product.unit),
                );
                detail_row(
                    ui,
                    crate::i18n::t("Sensitivity"),
                    &format!("{:.4}", product.sensitivity),
                );
            });

            // 通道列表
            detail_grid(ui, "detail_channels", crate::i18n::t("Channels"), |ui| {
                detail_row(
                    ui,
                    crate::i18n::t("Count##panels/sensor_dev/product_mgr.rs:34958"),
                    &product.channels.len().to_string(),
                );
            });
            if !product.channels.is_empty() {
                ui.add_space(2.0);
                for ch in &product.channels {
                    let status = if ch.enabled { "✓" } else { "✗" };
                    ui.label(
                        RichText::new(format!(
                            "  {} {} [{}] pin={} idx={} gain={:.3} offset={:.3} {}",
                            status,
                            ch.name,
                            ch.channel_type,
                            ch.pin,
                            ch.channel_index,
                            ch.gain,
                            ch.offset,
                            ch.unit,
                        ))
                        .color(if ch.enabled {
                            theme::TEXT_SECONDARY
                        } else {
                            theme::TEXT_DISABLED
                        })
                        .small()
                        .family(egui::FontFamily::Monospace),
                    );
                }
            }

            // 协议配置
            detail_grid(ui, "detail_protocol", crate::i18n::t("Protocol"), |ui| {
                detail_row(
                    ui,
                    crate::i18n::t("Type"),
                    &product.protocol.protocol_type.to_string(),
                );
                if product.protocol.baud_rate > 0 {
                    detail_row(
                        ui,
                        crate::i18n::t("Baud"),
                        &product.protocol.baud_rate.to_string(),
                    );
                }
                if let Some(id) = product.protocol.slave_id {
                    detail_row(ui, crate::i18n::t("Slave ID"), &id.to_string());
                }
                if let Some(addr) = product.protocol.i2c_address {
                    detail_row(
                        ui,
                        crate::i18n::t("I2C Addr"),
                        &format!("0x{:02X}", addr),
                    );
                }
                if let Some(res) = product.protocol.adc_resolution {
                    detail_row(
                        ui,
                        crate::i18n::t("ADC Res"),
                        &format!("{} bit", res),
                    );
                }
                if let Some(vref) = product.protocol.adc_ref_voltage {
                    detail_row(
                        ui,
                        crate::i18n::t("Ref Voltage"),
                        &format!("{:.2} V", vref),
                    );
                }
            });

            // 描述
            if !product.description.is_empty() {
                detail_grid(ui, "detail_desc", crate::i18n::t("Description"), |ui| {
                    ui.label(
                        RichText::new(&product.description)
                            .color(theme::TEXT_PRIMARY)
                            .small(),
                    );
                });
            }

            // 备注
            if !product.notes.is_empty() {
                detail_grid(ui, "detail_notes", crate::i18n::t("Notes"), |ui| {
                    ui.label(
                        RichText::new(&product.notes)
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                });
            }

            // 校准数据
            if let Some(cal) = &product.calibration {
                detail_grid(ui, "detail_cal", crate::i18n::t("Calibration"), |ui| {
                    detail_row(
                        ui,
                        crate::i18n::t("Type"),
                        &cal.calibration_type.to_string(),
                    );
                    detail_row(
                        ui,
                        crate::i18n::t("Coefficients"),
                        &cal
                            .coefficients
                            .iter()
                            .map(|c| format!("{:.6}", c))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    detail_row(
                        ui,
                        crate::i18n::t("Points##panels/sensor_dev/product_mgr.rs:39296"),
                        &cal.points.len().to_string(),
                    );
                    if let Some(t) = &cal.calibrated_at {
                        detail_row(ui, crate::i18n::t("Calibrated"), t);
                    }
                });
            }
        });
}

/// 详情区段：带标题的网格
fn detail_grid(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    content: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).color(theme::ACCENT).strong());
    ui.add_space(2.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            content(ui);
        });
    ui.add_space(4.0);
}

/// 详情行：标签 + 值
fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).color(theme::TEXT_SECONDARY).small());
    ui.label(
        RichText::new(value)
            .color(theme::TEXT_PRIMARY)
            .small()
            .family(egui::FontFamily::Monospace),
    );
    ui.end_row();
}

/// 渲染占位面板（供尚未实现的子面板 protocol_analyzer / heatmap 复用）
#[allow(dead_code)]
pub(crate) fn render_placeholder(ui: &mut egui::Ui, title: &str, desc: &str, _zh: bool) {
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(title)
                .color(theme::ACCENT)
                .strong()
                .heading(),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(desc)
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
    });
}
