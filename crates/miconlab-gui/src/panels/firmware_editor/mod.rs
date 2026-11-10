// 固件编辑面板
// 对备份的固件进行解码、编辑、保存，便于二次烧录
//
// 功能：
// - 打开 .bin / .hex 固件文件
// - Intel HEX 解码为统一内存模型（稀疏地址空间）
// - 十六进制编辑器（地址/HEX/ASCII 三栏，可编辑）
// - 芯片适配的向量表解析和编辑（STM32/ESP32/CH32/GD32/Arduino 等）
// - 字符串搜索与查看
// - 校验和修复
// - 差分对比（原始 vs 修改）
// - 保存为 BIN 或 HEX 格式

mod chip_type;
mod memory;
mod periph_map;
mod state;
mod string_search;
mod vector_table;

use chip_type::{auto_detect_chip, EditorChipType};
use memory::FirmwareMemory;
use state::{EditView, FileFormat, FirmwareEditorState};
use string_search::search_strings;
use vector_table::parse_arm_vector_table;

use crate::state::AppState;
use crate::theme;
use egui::{Color32, RichText, ScrollArea, Sense, Ui};
use miconlab_ai::rebuild::generate_rebuild_project;

#[cfg(feature = "ollama")]
use miconlab_ai::{
    EvidenceBundle, FirmwareIdentifyResult, IdentifySource, OllamaClient, OllamaStatus,
};
#[cfg(feature = "ollama")]
use std::sync::atomic::Ordering;
#[cfg(feature = "ollama")]
use std::time::{Duration, Instant};

// ============================================================================
// 面板入口
// ============================================================================

/// 面板入口
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let is_zh = state.language == crate::i18n::Language::Zh;

    // 检查是否从烧录器传来了跳转文件路径
    let pending_load: Option<String> =
        ctx.data(|d| d.get_temp::<String>(egui::Id::new("firmware_editor_file")));
    if pending_load.is_some() {
        ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new("firmware_editor_file")));
    }

    // 从 egui 持久化存储中取出面板状态（克隆）
    let state_id = egui::Id::new("firmware_editor_state");
    let mut fs: FirmwareEditorState = ctx.data(|d| {
        d.get_temp::<FirmwareEditorState>(state_id).unwrap_or_default()
    });

    // ---- Ollama 后台任务回收（feature gate）----
    #[cfg(feature = "ollama")]
    ollama_poll(&mut fs, state, is_zh);

    // 处理待加载文件
    if let Some(path) = pending_load {
        load_firmware_file(&mut fs, &path, is_zh);
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::default().inner_margin(egui::Margin::same(8.0)))
        .show(ctx, |ui| {
            // 标题
            ui.horizontal(|ui| {
                ui.heading(
                    RichText::new(crate::i18n::t("Firmware Editor"))
                        .color(theme::ACCENT)
                        .strong(),
                );
                ui.label(
                    RichText::new(crate::i18n::t("Decode, edit and save firmware for re-flashing"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            });
            // AI 感知：错误诊断（连接失败或操作异常时自动推断根因）
            if let Some(err) = &state.last_error {
                if let Some(hint) = crate::panels::ai_sense::sense_error(err, state.is_connected(), state.config.baud_rate, is_zh) {
                    crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
                    ui.add_space(4.0);
                }
            }
            ui.add_space(4.0);

            // 工具栏
            render_toolbar(ui, &mut fs, is_zh);

            // Ollama 识别结果条（feature gate）
            #[cfg(feature = "ollama")]
            render_ollama_result_bar(ui, &fs, is_zh);

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            // 内容区
            if fs.memory.is_none() {
                // 空状态提示
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(
                        RichText::new(crate::i18n::t("📝 Open a firmware file (.bin / .hex) to start editing"))
                        .color(theme::TEXT_SECONDARY)
                        .size(16.0),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(crate::i18n::t("Features: HEX decode / Hex edit / Vector table / String search / Diff / Checksum fix"))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                    );
                });
            } else {
                // 视图切换标签
                render_view_tabs(ui, &mut fs, is_zh);

                ui.add_space(4.0);

                // 根据视图渲染
                let view = fs.view;
                if view == EditView::PeriphMap {
                    // PeriphMap 需要 state（访问诊断引擎与连接状态）
                    ScrollArea::vertical().show(ui, |ui| {
                        render_periph_map_view(ui, state, &fs, is_zh);
                    });
                } else {
                    ScrollArea::vertical().show(ui, |ui| match view {
                        EditView::Hex => render_hex_editor(ui, &mut fs, is_zh),
                        EditView::VectorTable => render_vector_table(ui, &mut fs, is_zh),
                        EditView::Strings => render_strings_view(ui, &mut fs, is_zh),
                        EditView::Diff => render_diff_view(ui, &mut fs, is_zh),
                        EditView::PeriphMap => unreachable!(),
                    });
                }
            }

            // 状态栏
            ui.add_space(4.0);
            ui.separator();
            ui.horizontal(|ui| {
                if !fs.status_msg.is_empty() {
                    ui.label(
                        RichText::new(&fs.status_msg)
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                }
                if let Some(mem) = &fs.memory {
                    let modified = mem.modified_count();
                    let total = mem.total_bytes();
                    let base_addr = mem.base_addr;
                    let end_addr = mem.end_addr;
                    let fmt_label = fs.file_format_label();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let modified_color = if modified > 0 {
                            Color32::from_rgb(255, 180, 0)
                        } else {
                            theme::TEXT_SECONDARY
                        };
                        ui.label(
                            RichText::new(crate::i18n::tf("Modified: {} / {} bytes", &[("{}", "{}", &format!("{}", modified)), ("{}", "{}", &format!("{}", total))]))
                            .color(modified_color)
                            .small(),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "{} | 0x{:08X} - 0x{:08X}",
                                fmt_label, base_addr, end_addr
                            ))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                        );
                    });
                }
            });
        });

    // 写回持久化状态
    ctx.data_mut(|d| {
        d.insert_temp(state_id, fs);
    });
}

// ============================================================================
// UI 渲染
// ============================================================================

/// 渲染工具栏
fn render_toolbar(ui: &mut Ui, fs: &mut FirmwareEditorState, is_zh: bool) {
    ui.horizontal_wrapped(|ui| {
        // 打开文件
        if ui
            .button(crate::i18n::t("📂 Open"))
            .on_hover_text(crate::i18n::t("Open .bin or .hex firmware file"))
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Firmware", &["bin", "hex"])
                .pick_file()
            {
                load_firmware_file(fs, &path.display().to_string(), is_zh);
            }
        }

        if fs.memory.is_some() {
            // 保存
            if ui
                .button(crate::i18n::t("💾 Save"))
                .on_hover_text(crate::i18n::t("Save to original file (same format)"))
                .clicked()
            {
                save_firmware(fs, is_zh, None);
            }

            // 另存为
            if ui
                .button(crate::i18n::t("Save As..."))
                .on_hover_text(crate::i18n::t("Save as BIN or HEX format"))
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Binary", &["bin"])
                    .add_filter("Intel HEX", &["hex"])
                    .set_file_name("firmware_edited.bin")
                    .save_file()
                {
                    save_firmware(fs, is_zh, Some(path.display().to_string()));
                }
            }

            // 导出可编译工程（反汇编重建）
            if ui
                .button(crate::i18n::t("🔧 Export Project"))
                .on_hover_text(crate::i18n::t("Disassemble firmware into editable compilable assembly project\n(.s + .ld + Makefile). Rebuild with arm-none-eabi-gcc"))
                .clicked()
            {
                export_rebuild_project(fs, is_zh);
            }

            ui.separator();

            // 撤销所有修改
            if ui
                .button(crate::i18n::t("↶ Revert"))
                .on_hover_text(crate::i18n::t("Revert all changes to original"))
                .clicked()
            {
                if let Some(mem) = &mut fs.memory {
                    mem.revert_all();
                    fs.status_msg = crate::i18n::t("All changes reverted")
                    .to_string();
                }
            }

            ui.separator();

            // 芯片类型选择
            ui.label(
                RichText::new(crate::i18n::t("Chip:")).color(theme::TEXT_SECONDARY),
            );
            let current = fs.chip_type;
            egui::ComboBox::from_id_salt("chip_type_combo")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for &chip in EditorChipType::all() {
                        ui.selectable_value(&mut fs.chip_type, chip, chip.label());
                    }
                });

            // 显示自动检测结果
            if fs.chip_type == EditorChipType::Auto {
                ui.label(
                    RichText::new(format!(
                        "({}: {})",
                        crate::i18n::t("Detected"),
                        fs.detected_chip.label()
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            }

            // Ollama 本地模型识别按钮（feature gate）
            #[cfg(feature = "ollama")]
            {
                let ready = matches!(fs.ollama_status, Some(OllamaStatus::Ready(_)));
                let btn_text = if fs.ollama_busy {
                    crate::i18n::t("Identifying…")
                } else { crate::i18n::t("🧠 Local Identify") };
                if ui
                    .add_enabled(ready && !fs.ollama_busy, egui::Button::new(btn_text).small())
                    .on_hover_text(crate::i18n::t("Identify firmware arch/base/chip family with local Ollama model (fully offline)"))
                    .clicked()
                {
                    // 手动触发：不限制规则检测结果，用户明确要求即识别
                    trigger_firmware_identify(fs, true);
                }
            }
        }
    });
}

/// 渲染视图切换标签
fn render_view_tabs(ui: &mut Ui, fs: &mut FirmwareEditorState, _is_zh: bool) {
    ui.horizontal(|ui| {
        let tabs = [
            (EditView::Hex, crate::i18n::t("Hex"), "\u{1F4D8}"),
            (EditView::VectorTable, crate::i18n::t("Vector Table"), "\u{1F522}"),
            (EditView::Strings, crate::i18n::t("Strings"), "\u{1F4DD}"),
            (EditView::Diff, crate::i18n::t("Diff"), "\u{1F50D}"),
            (EditView::PeriphMap, crate::i18n::t("Periph Map"), "\u{1F5A5}"),
        ];
        for (view, label, icon) in tabs {
            let selected = fs.view == view;
            let text = format!("{} {}", icon, label);
            if ui.selectable_label(selected, &text).clicked() {
                fs.view = view;
            }
        }
    });
}

/// 渲染外设映射视图（D.6 新增）
///
/// 从 FirmwareMemory 提取二进制数据，调用 FirmwareAnalysis::analyze 进行启发式扫描，
/// 然后渲染 DMA/IRQ/GPIO 三张表与诊断引擎联动面板。
fn render_periph_map_view(ui: &mut Ui, state: &AppState, fs: &FirmwareEditorState, is_zh: bool) {
    let mem = match &fs.memory {
        Some(m) => m,
        None => {
            ui.label(
                RichText::new(crate::i18n::t("Load a firmware file first"))
                    .color(theme::TEXT_SECONDARY),
            );
            return;
        }
    };

    // 提取二进制数据并分析
    let binary = mem.to_binary();
    if binary.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("Firmware data is empty"))
                .color(theme::TEXT_SECONDARY),
        );
        return;
    }

    // 缓存分析结果（按文件路径 + 大小的 hash 作为 key，避免每帧重新分析）
    // 简单实现：每次重新分析（bin 通常 < 1MB，analyze 是 O(n) 扫描，可接受）
    let fw = miconlab_ai::firmware::FirmwareAnalysis::analyze(&binary);

    periph_map::render_periph_map(ui, state, &fw, is_zh);
}

/// 渲染十六进制编辑器
fn render_hex_editor(ui: &mut Ui, fs: &mut FirmwareEditorState, _is_zh: bool) {
    let bpr = fs.bytes_per_row;
    let total_bytes = fs.memory.as_ref().map_or(0, |m| m.total_bytes());
    let total_rows = (total_bytes + bpr - 1) / bpr;

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::tf("{} bytes ({} rows)", &[("{}", "{}", &format!("{}", total_bytes)), ("{}", "{}", &format!("{}", total_rows))]))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
        ui.separator();
        ui.label(
            RichText::new(crate::i18n::t("Bytes/row:"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        for &size in &[8usize, 16, 32] {
            if ui.small_button(format!("{}", size)).clicked() {
                fs.bytes_per_row = size;
            }
        }
        ui.separator();
        ui.label(
            RichText::new(crate::i18n::t("Goto: 0x"))
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        let goto_clicked = ui
            .add(
                egui::TextEdit::singleline(&mut fs.goto_addr_input)
                    .desired_width(80.0)
                    .clip_text(true),
            )
            .lost_focus();
        if (goto_clicked || ui.small_button(crate::i18n::t("Go")).clicked())
            && !fs.goto_addr_input.trim().is_empty()
        {
            let addr_str = fs
                .goto_addr_input
                .trim()
                .trim_start_matches("0x")
                .trim_start_matches("0X");
            if let Ok(addr) = u32::from_str_radix(addr_str, 16) {
                fs.scroll_to_addr = Some(addr);
            }
        }
    });

    ui.add_space(4.0);

    // HEX 编辑器主体
    ScrollArea::vertical()
        .max_height(500.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            // 表头
            ui.horizontal(|ui| {
                ui.add_sized(
                    [80.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Address"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.add_sized(
                    [bpr as f32 * 23.0 + 10.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Hex"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.label(
                    RichText::new(crate::i18n::t("ASCII"))
                        .color(theme::TEXT_SECONDARY)
                        .strong()
                        .small(),
                );
            });
            ui.separator();

            // 按行渲染 —— 在闭包内借用 fs.memory
            let mem = fs.memory.as_ref().unwrap();
            let addrs: Vec<u32> = mem.cells.keys().copied().collect();

            for row in 0..total_rows {
                let row_start_off = row * bpr;
                if row_start_off >= addrs.len() {
                    break;
                }
                let row_addr = addrs[row_start_off];

                ui.horizontal(|ui| {
                    // 地址列
                    ui.add_sized(
                        [80.0, 18.0],
                        egui::Label::new(
                            RichText::new(format!("{:08X}", row_addr))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        ),
                    );

                    // HEX 列
                    let hex_width = bpr as f32 * 23.0 + 10.0;
                    ui.allocate_ui(egui::vec2(hex_width, 18.0), |ui| {
                        ui.horizontal(|ui| {
                            for col in 0..bpr {
                                let idx = row_start_off + col;
                                if idx >= addrs.len() {
                                    break;
                                }
                                let addr = addrs[idx];
                                let cell = &mem.cells[&addr];
                                let byte_text = format!("{:02X}", cell.value);

                                let color = if cell.modified {
                                    Color32::from_rgb(255, 180, 0)
                                } else {
                                    theme::TEXT_PRIMARY
                                };

                                // 检查是否正在编辑此地址
                                let is_editing = fs.editing_addr == Some(addr);
                                if is_editing {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut fs.edit_buffer)
                                            .desired_width(20.0)
                                            .font(egui::TextStyle::Monospace)
                                            .min_size(egui::vec2(20.0, 16.0)),
                                    );
                                    if resp.lost_focus() {
                                        if let Ok(val) =
                                            u8::from_str_radix(fs.edit_buffer.trim(), 16)
                                        {
                                            fs.pending_write = Some((addr, val));
                                        }
                                        fs.editing_addr = None;
                                        fs.edit_buffer.clear();
                                    }
                                    if !resp.has_focus() {
                                        resp.request_focus();
                                    }
                                } else {
                                    let btn = ui.add(
                                        egui::Label::new(
                                            RichText::new(&byte_text)
                                                .color(color)
                                                .monospace()
                                                .small(),
                                        )
                                        .sense(Sense::click()),
                                    );
                                    if btn.clicked() {
                                        fs.editing_addr = Some(addr);
                                        fs.edit_buffer = byte_text;
                                    }
                                    btn.on_hover_text(format!("Addr: 0x{:08X}", addr));
                                }
                            }
                        });
                    });

                    // ASCII 列
                    ui.add_space(4.0);
                    let ascii: String = (0..bpr)
                        .map(|col| {
                            let idx = row_start_off + col;
                            if idx < addrs.len() {
                                let addr = addrs[idx];
                                let b = mem.cells[&addr].value;
                                if b >= 0x20 && b < 0x7f {
                                    b as char
                                } else {
                                    '.'
                                }
                            } else {
                                ' '
                            }
                        })
                        .collect();
                    let modified_in_row = (0..bpr).any(|col| {
                        let idx = row_start_off + col;
                        idx < addrs.len() && mem.cells[&addrs[idx]].modified
                    });
                    ui.label(
                        RichText::new(ascii)
                            .color(if modified_in_row {
                                Color32::from_rgb(255, 180, 0)
                            } else {
                                theme::TEXT_SECONDARY
                            })
                            .monospace()
                            .small(),
                    );
                });
            }
        });

    // 处理待写入（在 ScrollArea 闭包之外，避免借用冲突）
    if let Some((addr, val)) = fs.pending_write.take() {
        if let Some(mem) = &mut fs.memory {
            mem.write_byte(addr, val);
        }
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("💡 Click a hex byte to edit. Orange = modified. Click Save to write back."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
}

/// 渲染向量表
fn render_vector_table(ui: &mut Ui, fs: &mut FirmwareEditorState, is_zh: bool) {
    let chip = fs.effective_chip();

    if !chip.is_arm_cortex_m() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(crate::i18n::tf("⚠ {} has no ARM vector table", &[("{}", "{}", &format!("{}", chip.label()))]))
                .color(Color32::from_rgb(255, 180, 0)),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(crate::i18n::t("Vector table editing is for ARM Cortex-M chips only (STM32/GD32/CH32/LPC)"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        });
        return;
    }

    let mem = fs.memory.as_ref().unwrap();
    let base = mem.base_addr;
    let count = 59; // STM32F1 标准向量表
    let entries = parse_arm_vector_table(mem, base, count);

    ui.horizontal(|ui| {
        ui.heading(
            RichText::new(crate::i18n::t("ARM Cortex-M Vector Table"))
            .color(theme::ACCENT)
            .strong(),
        );
        ui.label(
            RichText::new(format!(
                "(Base: 0x{:08X}, {} entries)",
                base,
                entries.len()
            ))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
    });

    ui.add_space(4.0);

    // 向量表编辑提示
    ui.collapsing(
        RichText::new(crate::i18n::t("📖 Vector Table Help"))
        .color(theme::TEXT_SECONDARY)
        .small(),
        |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Vector table is the interrupt entry table for ARM Cortex-M, located at Flash base.\n[0] Initial SP — determines RAM usage\n[1] Reset Handler — first instruction after power-on\n[2-15] System exceptions: NMI/HardFault/BusFault etc.\n[16+] Peripheral IRQs: IRQ0~IRQn"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        },
    );

    ui.add_space(4.0);

    ScrollArea::vertical()
        .max_height(400.0)
        .show(ui, |ui| {
            // 表头
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 16.0],
                    egui::Label::new(
                        RichText::new("#").color(theme::TEXT_SECONDARY).strong().small(),
                    ),
                );
                ui.add_sized(
                    [120.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Name"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.add_sized(
                    [100.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Address"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.add_sized(
                    [120.0, 16.0],
                    egui::Label::new(
                        RichText::new("Value")
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.label(
                    RichText::new(crate::i18n::t("Description"))
                        .color(theme::TEXT_SECONDARY)
                        .strong()
                        .small(),
                );
            });
            ui.separator();

            for entry in &entries {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [40.0, 18.0],
                        egui::Label::new(
                            RichText::new(format!("[{}]", entry.index))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        ),
                    );
                    let display_name = if is_zh && !entry.name_zh.is_empty() {
                        format!("{} ({})", entry.name, entry.name_zh)
                    } else {
                        entry.name.to_string()
                    };
                    ui.add_sized(
                        [120.0, 18.0],
                        egui::Label::new(
                            RichText::new(display_name).color(theme::TEXT_PRIMARY).small(),
                        ),
                    );
                    ui.add_sized(
                        [100.0, 18.0],
                        egui::Label::new(
                            RichText::new(format!("0x{:08X}", entry.address))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        ),
                    );

                    // 可编辑的值（前 2 项可编辑）
                    let editable = entry.index <= 1;
                    let is_modified = fs.memory.as_ref().map_or(false, |m| {
                        let orig = m.original.get(&entry.address).copied();
                        let curr = m.read_u32_le(entry.address);
                        orig.map_or(false, |o| {
                            curr.map_or(false, |c| {
                                c != u32::from_le_bytes([
                                    o,
                                    *m.original.get(&(entry.address + 1)).unwrap_or(&0),
                                    *m.original.get(&(entry.address + 2)).unwrap_or(&0),
                                    *m.original.get(&(entry.address + 3)).unwrap_or(&0),
                                ])
                            })
                        })
                    });

                    let value_color = if is_modified {
                        Color32::from_rgb(255, 180, 0)
                    } else {
                        theme::TEXT_PRIMARY
                    };

                    if editable {
                        // 可编辑的十六进制输入
                        let id = egui::Id::new(format!("vec_edit_{}", entry.index));
                        let mut buf =
                            ui.data_mut(|d| d.get_temp_mut_or_default::<String>(id).clone());
                        if buf.is_empty() {
                            buf = format!("{:08X}", entry.value);
                        }
                        let resp = ui.add_sized(
                            [120.0, 18.0],
                            egui::TextEdit::singleline(&mut buf)
                                .desired_width(110.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if resp.changed() {
                            if let Ok(val) = u32::from_str_radix(buf.trim(), 16) {
                                if let Some(mem) = &mut fs.memory {
                                    mem.write_u32_le(entry.address, val);
                                }
                            }
                        }
                        ui.data_mut(|d| {
                            d.insert_temp(id, buf);
                        });
                    } else {
                        ui.add_sized(
                            [120.0, 18.0],
                            egui::Label::new(
                                RichText::new(format!("0x{:08X}", entry.value))
                                    .color(value_color)
                                    .monospace()
                                    .small(),
                            ),
                        );
                    }

                    ui.label(
                        RichText::new(&entry.description)
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                });
            }
        });
}

/// 渲染字符串视图
fn render_strings_view(ui: &mut Ui, fs: &mut FirmwareEditorState, _is_zh: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Search:")).color(theme::TEXT_SECONDARY),
        );
        let resp = ui.add(
            egui::TextEdit::singleline(&mut fs.string_search)
                .desired_width(200.0)
                .hint_text(crate::i18n::t("Filter strings...")),
        );
        if resp.changed() || fs.string_results.is_empty() {
            if let Some(mem) = &fs.memory {
                let all_strings = search_strings(mem, 4);
                let filter = fs.string_search.to_lowercase();
                if filter.is_empty() {
                    fs.string_results = all_strings;
                } else {
                    fs.string_results = all_strings
                        .into_iter()
                        .filter(|(_, s)| s.to_lowercase().contains(&filter))
                        .collect();
                }
            }
        }
        ui.separator();
        ui.label(
            RichText::new(format!(
                "{} {}",
                fs.string_results.len(),
                crate::i18n::t("results")
            ))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
    });

    ui.add_space(4.0);

    ScrollArea::vertical()
        .max_height(500.0)
        .show(ui, |ui| {
            for (addr, s) in &fs.string_results {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [100.0, 16.0],
                        egui::Label::new(
                            RichText::new(format!("0x{:08X}", addr))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        ),
                    );
                    let display = if s.len() > 80 {
                        format!("{}...", &s[..80])
                    } else {
                        s.clone()
                    };
                    ui.label(
                        RichText::new(format!("\"{}\"", display))
                            .color(theme::TEXT_PRIMARY)
                            .monospace()
                            .small(),
                    );
                });
            }
        });
}

/// 渲染差分对比
fn render_diff_view(ui: &mut Ui, fs: &mut FirmwareEditorState, _is_zh: bool) {
    let mem = fs.memory.as_ref().unwrap();
    let modified_count = mem.modified_count();

    if modified_count == 0 {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(crate::i18n::t("✓ No modifications — firmware matches original"))
                .color(Color32::from_rgb(100, 200, 100)),
            );
        });
        return;
    }

    ui.heading(
        RichText::new(crate::i18n::tf("Diff ({} bytes modified)", &[("{}", "{}", &format!("{}", modified_count))]))
        .color(theme::ACCENT)
        .strong(),
    );

    ui.add_space(4.0);

    ScrollArea::vertical()
        .max_height(500.0)
        .show(ui, |ui| {
            // 表头
            ui.horizontal(|ui| {
                ui.add_sized(
                    [100.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Address"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.add_sized(
                    [60.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Original"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.add_sized(
                    [60.0, 16.0],
                    egui::Label::new(
                        RichText::new(crate::i18n::t("Modified"))
                            .color(theme::TEXT_SECONDARY)
                            .strong()
                            .small(),
                    ),
                );
                ui.label(
                    RichText::new("ASCII")
                        .color(theme::TEXT_SECONDARY)
                        .strong()
                        .small(),
                );
            });
            ui.separator();

            for (&addr, cell) in &mem.cells {
                if !cell.modified {
                    continue;
                }
                let orig = mem.original.get(&addr).copied().unwrap_or(0xFF);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [100.0, 18.0],
                        egui::Label::new(
                            RichText::new(format!("0x{:08X}", addr))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        ),
                    );
                    ui.add_sized(
                        [60.0, 18.0],
                        egui::Label::new(
                            RichText::new(format!("{:02X}", orig))
                                .color(theme::TEXT_SECONDARY)
                                .monospace()
                                .small(),
                        ),
                    );
                    ui.add_sized(
                        [60.0, 18.0],
                        egui::Label::new(
                            RichText::new(format!("{:02X}", cell.value))
                                .color(Color32::from_rgb(255, 180, 0))
                                .monospace()
                                .strong()
                                .small(),
                        ),
                    );
                    let orig_ch = if orig >= 0x20 && orig < 0x7f {
                        orig as char
                    } else {
                        '.'
                    };
                    let new_ch = if cell.value >= 0x20 && cell.value < 0x7f {
                        cell.value as char
                    } else {
                        '.'
                    };
                    ui.label(
                        RichText::new(format!("{} → {}", orig_ch, new_ch))
                            .color(theme::TEXT_SECONDARY)
                            .monospace()
                            .small(),
                    );
                });
            }
        });
}

// ============================================================================
// 文件加载/保存
// ============================================================================

fn load_firmware_file(fs: &mut FirmwareEditorState, path: &str, _is_zh: bool) {
    let p = std::path::Path::new(path);
    if !p.exists() {
        fs.status_msg = format!(
            "{}: {}",
            crate::i18n::t("File not found"),
            path
        );
        return;
    }

    let is_hex = path.to_lowercase().ends_with(".hex");
    fs.file_format = if is_hex { FileFormat::Hex } else { FileFormat::Bin };

    if is_hex {
        match std::fs::read_to_string(path) {
            Ok(content) => match FirmwareMemory::from_hex(&content) {
                Ok(mem) => {
                    fs.detected_chip = auto_detect_chip(&mem);
                    fs.memory = Some(mem);
                    fs.file_path = path.to_string();
                    fs.status_msg = format!(
                        "{}: {} ({} bytes)",
                        crate::i18n::t("Loaded HEX"),
                        path,
                        fs.memory.as_ref().unwrap().total_bytes()
                    );
                }
                Err(e) => {
                    fs.status_msg = format!(
                        "{}: {}",
                        crate::i18n::t("HEX parse failed"),
                        e
                    );
                }
            },
            Err(e) => {
                fs.status_msg = format!(
                    "{}: {}",
                    crate::i18n::t("Read failed"),
                    e
                );
            }
        }
    } else {
        match std::fs::read(path) {
            Ok(data) => {
                // 先以 base=0 加载，用于自动检测芯片类型
                let mut mem = FirmwareMemory::from_binary(&data, 0);
                let detected = auto_detect_chip(&mem);
                fs.detected_chip = detected;
                // 根据检测到的芯片类型重新设置基地址
                let base = if fs.chip_type != EditorChipType::Auto {
                    fs.chip_type.flash_base()
                } else {
                    detected.flash_base()
                };
                if base != 0 {
                    mem.rebase(base);
                }
                fs.memory = Some(mem);
                fs.file_path = path.to_string();
                fs.status_msg = format!(
                    "{}: {} ({} bytes, base=0x{:08X}, chip={})",
                    crate::i18n::t("Loaded BIN"),
                    path,
                    data.len(),
                    base,
                    detected.label()
                );
            }
            Err(e) => {
                fs.status_msg = format!(
                    "{}: {}",
                    crate::i18n::t("Read failed"),
                    e
                );
            }
        }
    }

    // 加载后搜索字符串
    if let Some(mem) = &fs.memory {
        fs.string_results = search_strings(mem, 4);
    }

    // Ollama 自动识别：规则引擎检测失败（Raw）且用户未手动指定芯片时触发（feature gate）
    #[cfg(feature = "ollama")]
    if fs.memory.is_some()
        && fs.chip_type == EditorChipType::Auto
        && fs.detected_chip == EditorChipType::Raw
    {
        trigger_firmware_identify(fs, false);
    }
}

// ============================================================================
// Ollama 本地模型固件识别（feature gate）
// ============================================================================

/// Ollama 状态轮询 + 识别结果回收（每帧调用，全部后台线程，不阻塞 UI）
#[cfg(feature = "ollama")]
fn ollama_poll(fs: &mut FirmwareEditorState, state: &AppState, _is_zh: bool) {
    // 设置页联动：签名变化时刷新配置并强制重查状态
    crate::settings::sync_panel_config(
        &mut fs.ollama_config,
        &mut fs.ollama_cfg_sig,
        &mut fs.ollama_status,
        &mut fs.ollama_status_checked_at,
        &state.settings.ollama,
    );
    // 1. 状态检测：到期（30s）且空闲 → 后台线程发起
    let status_due = fs
        .ollama_status_checked_at
        .map_or(true, |t| t.elapsed() > Duration::from_secs(30));
    if status_due && !fs.ollama_status_busy {
        fs.ollama_status_busy = true;
        let slot = fs.ollama_status_slot.clone();
        let cfg = fs.ollama_config.clone();
        std::thread::spawn(move || {
            let status = match OllamaClient::new(cfg) {
                Ok(client) => client.status_blocking(),
                Err(_) => OllamaStatus::NotRunning,
            };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
        });
    }
    if fs.ollama_status_busy {
        let done = fs
            .ollama_status_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(status) = done {
            fs.ollama_status = Some(status);
            fs.ollama_status_checked_at = Some(Instant::now());
            fs.ollama_status_busy = false;
        }
    }

    // 2. 识别结果回收：成功时按置信度回写 detected_chip / base 地址
    if fs.ollama_busy && fs.ollama_done_flag.load(Ordering::Relaxed) {
        let res = fs
            .ollama_result_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(res) = res {
            match res {
                Ok(r) => {
                    let applied = apply_firmware_identify(fs, &r);
                    fs.ollama_applied = applied;
                    if applied {
                        fs.status_msg = crate::i18n::tf("🧠 Local model: {} @ 0x{:08X} (applied, {:.0}%)", &[("{}", "{}", &format!("{}", r.chip_family.as_deref().unwrap_or(&r.arch))), ("{:08X}", "{:08X}", &format!("{:08X}", r.base_addr)), ("{:.0}", "{:.0}", &format!("{:.0}", r.confidence * 100.0))]);
                    }
                    fs.ollama_result = Some(r);
                }
                Err(e) => {
                    // 静默降级：仅记日志
                    log::warn!("{}", crate::i18n::tf("Ollama firmware identification failed (silent fallback): {error}", &[("{error}", "{error}", &format!("{}", e))]));
                }
            }
        }
        fs.ollama_busy = false;
        fs.ollama_done_flag.store(false, Ordering::Relaxed);
    }
}

/// 触发固件识别后台线程。
///
/// `manual` = true 时不限前置条件（用户明确点击）；自动触发要求 Ollama 就绪且不忙。
#[cfg(feature = "ollama")]
fn trigger_firmware_identify(fs: &mut FirmwareEditorState, manual: bool) {
    if fs.ollama_busy {
        return;
    }
    if !matches!(fs.ollama_status, Some(OllamaStatus::Ready(_))) {
        return;
    }
    let Some(ev) = build_firmware_evidence(fs) else {
        return;
    };
    if manual {
        log::info!("{}", crate::i18n::t("Ollama firmware identification: manually triggered"));
    }
    fs.ollama_busy = true;
    fs.ollama_done_flag.store(false, Ordering::Relaxed);
    let slot = fs.ollama_result_slot.clone();
    let flag = fs.ollama_done_flag.clone();
    let cfg = fs.ollama_config.clone();
    std::thread::spawn(move || {
        let res = OllamaClient::new(cfg)
            .map_err(|e| e.to_string())
            .and_then(|client| {
                miconlab_ai::identify::identify_firmware(&client, &ev).map_err(|e| e.to_string())
            });
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
        flag.store(true, Ordering::Relaxed);
    });
}

/// 构造固件识别证据包：头部 HEX + 规则引擎分析摘要 + 提取字符串
#[cfg(feature = "ollama")]
fn build_firmware_evidence(fs: &FirmwareEditorState) -> Option<EvidenceBundle> {
    let mem = fs.memory.as_ref()?;
    let data = mem.to_binary();
    if data.is_empty() {
        return None;
    }
    let mut ev = EvidenceBundle::new();
    // 头部 256 字节（魔数/向量表/入口都在这里）
    let head_len = data.len().min(256);
    ev.sample_hex = data[..head_len]
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ");
    ev.extra_notes.push(crate::i18n::tf(
        "Firmware size: {size} bytes, current base address: {base}, file format: {format}",
        &[
            ("{size}", "{size}", &data.len().to_string()),
            ("{base}", "{base}", &format!("0x{:08X}", mem.base_addr)),
            ("{format}", "{format}", fs.file_format_label()),
        ],
    ));
    // 规则引擎分析摘要（O(n) 扫描，<1MB 可接受）
    let analysis = miconlab_ai::firmware::FirmwareAnalysis::analyze(&data);
    ev.extra_notes.push(crate::i18n::tf(
        "Rule engine analysis: arch={arch}, entry={entry}, code ratio={code}, padding ratio={pad}",
        &[
            ("{arch}", "{arch}", analysis.arch.label_en()),
            ("{entry}", "{entry}", &analysis.entry_addr.map(|a| format!("0x{:08X}", a)).unwrap_or_else(|| "-".to_string())),
            ("{code}", "{code}", &format!("{:.2}", analysis.code_ratio)),
            ("{pad}", "{pad}", &format!("{:.2}", analysis.pad_ratio)),
        ],
    ));
    for s in analysis.strings.iter().take(8) {
        ev.extra_notes.push(crate::i18n::tf("Firmware string: {text}", &[("{text}", "{text}", s)]));
    }
    // 规则检测结果作为参考 hint
    if fs.detected_chip != EditorChipType::Raw {
        ev.rule_hint = Some((fs.detected_chip.label().to_string(), 0.8));
    }
    Some(ev)
}

/// 将 LLM 识别结果应用到面板状态。
///
/// 仅在「规则引擎检测失败（Raw）+ 用户未手动指定 + 置信度 ≥0.6」时回写，
/// 防止覆盖已有正确检测。BIN 文件按 LLM base_addr 修正地址映射。
/// 返回是否发生了回写。
#[cfg(feature = "ollama")]
fn apply_firmware_identify(fs: &mut FirmwareEditorState, r: &FirmwareIdentifyResult) -> bool {
    if r.confidence < 0.6 {
        return false;
    }
    if fs.chip_type != EditorChipType::Auto || fs.detected_chip != EditorChipType::Raw {
        return false;
    }
    let chip = map_to_editor_chip(r);
    if let Some(c) = chip {
        fs.detected_chip = c;
    }
    // BIN 文件：LLM 给出的 base_addr 非零且与当前不同 → rebase
    if fs.file_format == FileFormat::Bin && r.base_addr != 0 {
        if let Some(mem) = &mut fs.memory {
            if mem.base_addr != r.base_addr {
                mem.rebase(r.base_addr);
            }
        }
    }
    chip.is_some()
}

/// LLM 结果（chip_family / arch 字符串）→ 编辑器芯片类型映射
#[cfg(feature = "ollama")]
fn map_to_editor_chip(r: &FirmwareIdentifyResult) -> Option<EditorChipType> {
    let family = r.chip_family.as_deref().unwrap_or("").to_lowercase();
    let arch = r.arch.to_lowercase();
    // chip_family 优先（更具体）
    if family.contains("esp8266") {
        return Some(EditorChipType::Esp8266);
    }
    if family.contains("esp32") {
        return Some(EditorChipType::Esp32);
    }
    if family.contains("gd32") {
        return Some(EditorChipType::Gd32);
    }
    if family.contains("ch32") {
        return Some(EditorChipType::Ch32);
    }
    if family.contains("stm8") {
        return Some(EditorChipType::Stm8);
    }
    if family.contains("stm32") {
        return Some(EditorChipType::Stm32);
    }
    if family.contains("lpc") || family.contains("nxp") {
        return Some(EditorChipType::NxpLpc);
    }
    if family.contains("arduino") || family.contains("avr") {
        return Some(EditorChipType::Arduino);
    }
    // arch 兜底
    if arch.contains("lx106") {
        return Some(EditorChipType::Esp8266);
    }
    if arch.contains("xtensa") {
        return Some(EditorChipType::Esp32);
    }
    if arch.contains("riscv") {
        return Some(EditorChipType::Ch32);
    }
    if arch.contains("avr") {
        return Some(EditorChipType::Arduino);
    }
    if arch.contains("cortex-m") {
        // 无法区分具体 ARM 厂商时按最常见的 STM32 处理
        return Some(EditorChipType::Stm32);
    }
    None
}

/// 渲染 Ollama 识别结果条（仅在有结果时显示）
#[cfg(feature = "ollama")]
fn render_ollama_result_bar(ui: &mut Ui, fs: &FirmwareEditorState, _is_zh: bool) {
    let Some(r) = &fs.ollama_result else {
        return;
    };
    let border = if fs.ollama_applied {
        theme::SUCCESS
    } else {
        theme::WARNING
    };
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, border))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("🧠").size(13.0));
                let summary = crate::i18n::tf(
                    "Local model: {arch} | base {base} | {family} | {confidence}% {suffix}",
                    &[
                        ("{arch}", "{arch}", &format!("{}", r.arch)),
                        ("{base}", "{base}", &format!("0x{:08X}", r.base_addr)),
                        ("{family}", "{family}", r.chip_family.as_deref().unwrap_or("-")),
                        ("{confidence}", "{confidence}", &format!("{:.0}", r.confidence * 100.0)),
                        ("{suffix}", "{suffix}", crate::i18n::t(if fs.ollama_applied { "applied" } else { "not applied" })),
                    ],
                );
                ui.label(RichText::new(summary).color(border).strong().small());
            });
            if let Some(entry) = r.entry {
                ui.label(
                    RichText::new(crate::i18n::tf("Entry: 0x{:08X}", &[("{:08X}", "{:08X}", &format!("{:08X}", entry))]))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
            }
            ui.label(
                RichText::new(&r.reasoning)
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            // 来源标记（区分规则/LLM）
            if r.source == IdentifySource::Ollama {
                ui.label(
                    RichText::new(crate::i18n::t("Source: Ollama local model (advisory, please verify)"))
                    .color(theme::TEXT_DISABLED)
                    .small(),
                );
            }
        });
    ui.add_space(4.0);
}

/// 导出可编译汇编工程（反汇编重建）
///
/// 将当前加载的固件转换为 GNU as 格式的可编辑汇编工程，
/// 包含 firmware.s / firmware.ld / Makefile / README.md。
fn export_rebuild_project(fs: &mut FirmwareEditorState, _is_zh: bool) {
    let mem = match &fs.memory {
        Some(m) => m,
        None => {
            fs.status_msg = crate::i18n::t("No firmware data to export").to_string();
            return;
        }
    };

    // 提取固件数据
    let data = mem.to_binary();
    if data.is_empty() {
        fs.status_msg = crate::i18n::t("Firmware data is empty").to_string();
        return;
    }

    // ── 芯片特征指纹匹配 ──
    let match_result = miconlab_ai::chip_profile::match_profile(&data);
    let profile = match_result.profile;
    let profile_prefix = profile.file_prefix();

    // 选择输出目录（默认目录名带 profile 前缀）
    let default_dir_name = format!("{}project", profile_prefix);
    let dir = rfd::FileDialog::new()
        .set_title(crate::i18n::tf("Select output dir - {} (match {:.0}%)", &[("{}", "{}", &format!("{}", profile.full_name())), ("{:.0}", "{:.0}", &format!("{:.0}", match_result.confidence * 100.0))]))
        .set_directory(&default_dir_name)
        .pick_folder();

    let dir = match dir {
        Some(d) => d,
        None => return,  // 用户取消
    };

    let base_addr = mem.base_addr;

    // 提取初始 SP（向量表第 0 项）
    let sp_init = mem.read_u32_le(base_addr).unwrap_or(0x20003000);

    // 生成可编译工程（可能耗时，但固件通常 < 256KB，可接受）
    fs.status_msg = crate::i18n::tf("Generating project ({})...", &[("{}", "{}", &format!("{}", profile.full_name()))]);
    let project = generate_rebuild_project(&data, base_addr, sp_init);

    // 写入文件（文件名带 profile 前缀，避免不同芯片工程混淆）
    let firmware_s_name = format!("{}firmware.s", profile_prefix);
    let firmware_ld_name = format!("{}firmware.ld", profile_prefix);
    let files = [
        (firmware_s_name.as_str(), &project.firmware_s),
        (firmware_ld_name.as_str(), &project.firmware_ld),
        ("Makefile", &project.makefile),
        ("README.md", &project.readme),
    ];

    let mut written = Vec::new();
    for (name, content) in &files {
        let path = dir.join(name);
        match std::fs::write(&path, content) {
            Ok(_) => written.push(*name),
            Err(e) => {
                fs.status_msg = format!(
                    "{}: {} ({})",
                    crate::i18n::t("Write failed"),
                    name,
                    e
                );
                return;
            }
        }
    }

    let chip_info = if profile.id == "unknown" {
        format!("{} ({:.0}%)", profile.full_name(), match_result.confidence * 100.0)
    } else {
        format!("{} ✓ ({:.0}%)", profile.full_name(), match_result.confidence * 100.0)
    };

    fs.status_msg = format!(
        "{}: {} [{}] ({} {}, {} {}, {} {})",
        crate::i18n::t("Project exported"),
        dir.display(),
        chip_info,
        project.label_count,
        crate::i18n::t("labels"),
        project.ldr_literal_converted,
        crate::i18n::t("LDR conv"),
        project.branch_resolved,
        crate::i18n::t("branches")
    );

    // 尝试在文件管理器中打开目录
    let _ = std::process::Command::new("open")
        .arg(&dir)
        .spawn();
}

fn save_firmware(fs: &mut FirmwareEditorState, _is_zh: bool, save_as_path: Option<String>) {
    let mem = match &fs.memory {
        Some(m) => m,
        None => {
            fs.status_msg = crate::i18n::t("No data to save").to_string();
            return;
        }
    };

    let path = save_as_path.unwrap_or_else(|| fs.file_path.clone());
    if path.is_empty() {
        fs.status_msg = crate::i18n::t("Empty path").to_string();
        return;
    }

    let is_hex = path.to_lowercase().ends_with(".hex");

    if is_hex {
        let hex_content = mem.to_hex();
        match std::fs::write(&path, hex_content) {
            Ok(()) => {
                fs.file_path = path.clone();
                fs.file_format = FileFormat::Hex;
                fs.status_msg = format!(
                    "{}: {} ({} bytes)",
                    crate::i18n::t("Saved HEX"),
                    path,
                    mem.total_bytes()
                );
            }
            Err(e) => {
                fs.status_msg =
                    format!("{}: {}", crate::i18n::t("Save failed"), e);
            }
        }
    } else {
        let bin_data = mem.to_binary();
        match std::fs::write(&path, &bin_data) {
            Ok(()) => {
                fs.file_path = path.clone();
                fs.file_format = FileFormat::Bin;
                fs.status_msg = format!(
                    "{}: {} ({} bytes)",
                    crate::i18n::t("Saved BIN"),
                    path,
                    bin_data.len()
                );
            }
            Err(e) => {
                fs.status_msg =
                    format!("{}: {}", crate::i18n::t("Save failed"), e);
            }
        }
    }
}

#[cfg(all(test, feature = "ollama"))]
mod tests {
    use super::*;
    use miconlab_ai::{IdentifySource, IdentifyTarget};

    fn make_result(
        arch: &str,
        base: u32,
        family: Option<&str>,
        conf: f32,
    ) -> FirmwareIdentifyResult {
        FirmwareIdentifyResult {
            arch: arch.to_string(),
            base_addr: base,
            entry: None,
            chip_family: family.map(|s| s.to_string()),
            confidence: conf,
            reasoning: "test".to_string(),
            source: IdentifySource::Ollama,
        }
    }

    #[test]
    fn test_map_family_priority() {
        assert_eq!(
            map_to_editor_chip(&make_result("cortex-m3", 0x0800_0000, Some("stm32f1"), 0.9)),
            Some(EditorChipType::Stm32)
        );
        assert_eq!(
            map_to_editor_chip(&make_result("xtensa-lx6", 0x1000, Some("esp32"), 0.9)),
            Some(EditorChipType::Esp32)
        );
        // esp8266 必须优先于 esp32 子串误判（"esp8266" 不含 "esp32"，双重保险）
        assert_eq!(
            map_to_editor_chip(&make_result("xtensa-lx106", 0, Some("esp8266"), 0.9)),
            Some(EditorChipType::Esp8266)
        );
        // esp32c3 是 riscv 架构但 family 指明 esp32 系列 → family 优先
        assert_eq!(
            map_to_editor_chip(&make_result("riscv32", 0, Some("esp32c3"), 0.9)),
            Some(EditorChipType::Esp32)
        );
    }

    #[test]
    fn test_map_arch_fallback() {
        assert_eq!(
            map_to_editor_chip(&make_result("cortex-m4", 0x0800_0000, None, 0.8)),
            Some(EditorChipType::Stm32)
        );
        assert_eq!(
            map_to_editor_chip(&make_result("xtensa-lx7", 0, None, 0.8)),
            Some(EditorChipType::Esp32)
        );
        assert_eq!(
            map_to_editor_chip(&make_result("riscv32", 0, None, 0.8)),
            Some(EditorChipType::Ch32)
        );
        assert_eq!(
            map_to_editor_chip(&make_result("avr", 0, None, 0.8)),
            Some(EditorChipType::Arduino)
        );
        assert_eq!(
            map_to_editor_chip(&make_result("m68k", 0, None, 0.8)),
            None
        );
    }

    #[test]
    fn test_apply_requires_high_confidence() {
        let mut fs = FirmwareEditorState::default();
        fs.memory = Some(FirmwareMemory::from_binary(&[0xAA; 64], 0));
        fs.file_format = FileFormat::Bin;
        // detected_chip 默认 Raw，chip_type 默认 Auto
        let r = make_result("cortex-m3", 0x0800_0000, Some("stm32f1"), 0.5);
        assert!(!apply_firmware_identify(&mut fs, &r));
        assert_eq!(fs.detected_chip, EditorChipType::Raw);
    }

    #[test]
    fn test_apply_writes_chip_and_rebases() {
        let mut fs = FirmwareEditorState::default();
        fs.memory = Some(FirmwareMemory::from_binary(&[0xAA; 64], 0));
        fs.file_format = FileFormat::Bin;
        let r = make_result("cortex-m3", 0x0800_0000, Some("stm32f1"), 0.85);
        assert!(apply_firmware_identify(&mut fs, &r));
        assert_eq!(fs.detected_chip, EditorChipType::Stm32);
        assert_eq!(fs.memory.as_ref().unwrap().base_addr, 0x0800_0000);
    }

    #[test]
    fn test_apply_never_overrides_existing_detection() {
        let mut fs = FirmwareEditorState::default();
        fs.memory = Some(FirmwareMemory::from_binary(&[0xAA; 64], 0));
        fs.file_format = FileFormat::Bin;
        fs.detected_chip = EditorChipType::Esp32; // 规则引擎已有结论
        let r = make_result("cortex-m3", 0x0800_0000, Some("stm32f1"), 0.95);
        assert!(!apply_firmware_identify(&mut fs, &r));
        assert_eq!(fs.detected_chip, EditorChipType::Esp32);
    }

    #[test]
    fn test_apply_never_overrides_user_choice() {
        let mut fs = FirmwareEditorState::default();
        fs.memory = Some(FirmwareMemory::from_binary(&[0xAA; 64], 0));
        fs.file_format = FileFormat::Bin;
        fs.chip_type = EditorChipType::Arduino; // 用户手动指定
        let r = make_result("cortex-m3", 0x0800_0000, Some("stm32f1"), 0.95);
        assert!(!apply_firmware_identify(&mut fs, &r));
        assert_eq!(fs.detected_chip, EditorChipType::Raw);
    }

    #[test]
    fn test_apply_skips_rebase_for_hex_format() {
        let mut fs = FirmwareEditorState::default();
        // HEX 文件数据自带真实地址（模拟 0x200 起始）
        fs.memory = Some(FirmwareMemory::from_binary(&[0xAA; 64], 0x200));
        fs.file_format = FileFormat::Hex;
        let r = make_result("cortex-m3", 0x0800_0000, Some("stm32f1"), 0.85);
        assert!(apply_firmware_identify(&mut fs, &r));
        assert_eq!(fs.detected_chip, EditorChipType::Stm32);
        // HEX 不应 rebase
        assert_eq!(fs.memory.as_ref().unwrap().base_addr, 0x200);
    }

    #[test]
    fn test_build_evidence_contains_key_info() {
        let mut fs = FirmwareEditorState::default();
        // 构造带 ARM 向量表特征的假固件（SP=0x20005000, Reset=0x08000101）
        let mut data = vec![0u8; 256];
        data[0..4].copy_from_slice(&0x2000_5000u32.to_le_bytes());
        data[4..8].copy_from_slice(&0x0800_0101u32.to_le_bytes());
        fs.memory = Some(FirmwareMemory::from_binary(&data, 0));
        fs.file_format = FileFormat::Bin;
        let ev = build_firmware_evidence(&fs).unwrap();
        let text = ev.render(IdentifyTarget::Firmware);
        assert!(text.contains("Firmware identification"));
        assert!(text.contains("256 bytes"));
        assert!(text.contains("2000 5000") == false); // HEX 样本是无空格连续大写两位
        assert!(ev.sample_hex.starts_with("00 50 00 20"));
        // 证据文本随当前界面语言变化，断言技术标识而非自然语言
        assert!(ev
            .extra_notes
            .iter()
            .any(|n| n.contains("arch=") || n.contains("架构=")));
        // Raw 检测 → 无 rule_hint
        assert!(ev.rule_hint.is_none() || fs.detected_chip != EditorChipType::Raw);
    }

    #[test]
    fn test_build_evidence_empty_memory() {
        let fs = FirmwareEditorState::default();
        assert!(build_firmware_evidence(&fs).is_none());
    }
}
