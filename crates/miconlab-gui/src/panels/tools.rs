// 工具箱面板：校验计算器 + 波特率自动检测 + AI 本地模型设置
// 两个子区域用 Separator 隔开，工业风暗色紧凑布局。

use crate::i18n::L10n;
use crate::state::AppState;
use crate::theme;
use egui::{Color32, Id, RichText, Vec2};
use miconlab_core::checksum::ChecksumResults;
use miconlab_core::config::{DataBits, Parity, StopBits};
use miconlab_core::hex::parse_hex;
use miconlab_core::{detect_baud_rate, BaudDetectResult};
use std::sync::{Arc, Mutex};

#[cfg(feature = "ollama")]
use miconlab_ai::{OllamaClient, OllamaStatus};
#[cfg(feature = "ollama")]
use std::sync::atomic::{AtomicBool, Ordering};

/// 检测线程的结果槽类型
type DetectSlot = Arc<Mutex<Option<Result<Vec<BaudDetectResult>, String>>>>;

/// Ollama 设置面板状态（IdTypeMap 持久化，含后台任务共享槽）
#[cfg(feature = "ollama")]
#[derive(Clone, Default)]
struct OllamaPanelState {
    /// 服务状态（None = 未检测）
    status: Option<OllamaStatus>,
    /// 状态检测时刻（30s 节流）
    status_checked_at: Option<std::time::Instant>,
    /// 状态检测后台槽
    status_slot: Arc<Mutex<Option<OllamaStatus>>>,
    /// 状态检测进行中
    status_busy: bool,
    /// 配置签名（base_url|model，变化时强制重查）
    cfg_sig: Option<String>,
    /// 拉取进行中
    pulling: bool,
    /// 拉取进度共享槽（0..1, 状态文本），后台线程持续写、UI 每帧读
    pull_progress: Arc<Mutex<(f32, String)>>,
    /// 拉取完成标志
    pull_done: Arc<AtomicBool>,
    /// 拉取结果槽（Err = 失败原因）
    pull_slot: Arc<Mutex<Option<Result<(), String>>>>,
}

/// 渲染工具箱面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let zh = state.language == crate::i18n::Language::Zh;

    // 从持久化存储取出状态（IdTypeMap 的 *_temp 方法要求 T: Clone）
    let mut input_text: String = ctx
        .data_mut(|d| d.get_temp::<String>(Id::new("tools_input_text")))
        .unwrap_or_default();
    let mut is_hex: bool = ctx
        .data_mut(|d| d.get_temp::<bool>(Id::new("tools_is_hex")))
        .unwrap_or(false);
    let mut results: Option<ChecksumResults> = ctx
        .data_mut(|d| d.get_temp::<Option<ChecksumResults>>(Id::new("tools_results")))
        .unwrap_or(None);
    let mut detect_port_idx: usize = ctx
        .data_mut(|d| d.get_temp::<usize>(Id::new("tools_detect_port_idx")))
        .unwrap_or(0);
    let mut detect_results: Vec<BaudDetectResult> = ctx
        .data_mut(|d| d.get_temp::<Vec<BaudDetectResult>>(Id::new("tools_detect_results")))
        .unwrap_or_default();
    let mut detecting: bool = ctx
        .data_mut(|d| d.get_temp::<bool>(Id::new("tools_detecting")))
        .unwrap_or(false);
    let mut cfg_data_bits: DataBits = ctx
        .data_mut(|d| d.get_temp::<DataBits>(Id::new("tools_db")))
        .unwrap_or(DataBits::Eight);
    let mut cfg_parity: Parity = ctx
        .data_mut(|d| d.get_temp::<Parity>(Id::new("tools_par")))
        .unwrap_or(Parity::None);
    let mut cfg_stop_bits: StopBits = ctx
        .data_mut(|d| d.get_temp::<StopBits>(Id::new("tools_sb")))
        .unwrap_or(StopBits::One);

    // 连接会话变化时清除波特率检测结果（避免旧设备结果误导新设备连接决策）
    let last_session_id: u64 = ctx
        .data_mut(|d| d.get_temp::<u64>(Id::new("tools_last_session_id")))
        .unwrap_or(0);
    if last_session_id != state.connection_session_id {
        detect_results.clear();
        detecting = false;
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("tools_last_session_id"), state.connection_session_id);
        });
    }

    // 检测线程共享结果槽（Arc<Mutex> 是 Clone，可存入 IdTypeMap）
    let detect_slot: DetectSlot = ctx
        .data_mut(|d| d.get_temp::<DetectSlot>(Id::new("tools_detect_slot")))
        .unwrap_or_else(|| Arc::new(Mutex::new(None)));

    // Ollama 设置面板状态（feature gate）
    #[cfg(feature = "ollama")]
    let mut ollama_panel: OllamaPanelState = ctx
        .data_mut(|d| d.get_temp::<OllamaPanelState>(Id::new("tools_ollama_panel")))
        .unwrap_or_default();

    // 正在检测时轮询结果
    if detecting {
        if let Some(res) = detect_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            detecting = false;
            match res {
                Ok(list) => {
                    detect_results = list;
                    state.last_error = None;
                }
                Err(e) => {
                    state.last_error = Some(e);
                }
            }
        }
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            ui.heading(
                RichText::new(l10n.t("panel.tools"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            // 副标题：解释面板用途
            ui.label(
                RichText::new(crate::i18n::t("Toolkit - baud detection, protocol analysis, CRC calculation and other dev utilities"))
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

            // ===== 校验计算器 =====
            ui.label(
                RichText::new(crate::i18n::t("Checksum Calculator"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(6.0);

            // 输入模式切换 + 计算按钮
            ui.horizontal(|ui| {
                let txt_btn = egui::SelectableLabel::new(!is_hex, crate::i18n::t("Text"));
                if ui
                    .add(txt_btn)
                    .on_hover_text(crate::i18n::t("Switch to text input mode, calculate checksums by ASCII characters"))
                    .clicked()
                {
                    is_hex = false;
                }
                let hex_btn = egui::SelectableLabel::new(is_hex, "HEX");
                if ui
                    .add(hex_btn)
                    .on_hover_text(crate::i18n::t("Switch to HEX input mode, calculate checksums by hex bytes (e.g. 48 65 6C 6C 6F)"))
                    .clicked()
                {
                    is_hex = true;
                }
                ui.separator();
                let calc_btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Calculate"))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .stroke(egui::Stroke::NONE);
                if ui
                    .add(calc_btn)
                    .on_hover_text(crate::i18n::t("Calculate CRC16/CRC32/LRC/SUM and other checksums for the input data"))
                    .clicked()
                {
                    let data: Vec<u8> = if is_hex {
                        match parse_hex(&input_text) {
                            Ok(b) => b,
                            Err(e) => {
                                state.last_error = Some(format!("Hex: {}", e));
                                Vec::new()
                            }
                        }
                    } else {
                        input_text.as_bytes().to_vec()
                    };
                    if !data.is_empty() {
                        results = Some(ChecksumResults::calculate_all(&data));
                        state.last_error = None;
                    }
                }
            });

            ui.add_space(4.0);
            // 输入框
            ui.add_sized(
                [ui.available_width(), 64.0],
                egui::TextEdit::multiline(&mut input_text)
                    .font(egui::FontId::monospace(12.5))
                    .desired_width(f32::MAX)
                    .hint_text(if is_hex { "48 65 6C 6C 6F" } else { "Hello" }),
            )
            .on_hover_text(crate::i18n::t("Supports text or HEX (auto-detected; HEX must be space-separated)"));

            ui.add_space(8.0);
            // 结果显示
            if let Some(r) = &results {
                egui::Grid::new("checksum_grid")
                    .num_columns(3)
                    .spacing([10.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        checksum_row(ui, "CRC16-MODBUS", format!("0x{:04X}", r.crc16_modbus), zh);
                        checksum_row(ui, "CRC16-CCITT", format!("0x{:04X}", r.crc16_ccitt), zh);
                        checksum_row(ui, "CRC16-XMODEM", format!("0x{:04X}", r.crc16_xmodem), zh);
                        checksum_row(ui, "CRC32", format!("0x{:08X}", r.crc32), zh);
                        checksum_row(ui, "LRC", format!("0x{:02X}", r.lrc), zh);
                        checksum_row(ui, "SUM8", format!("0x{:02X}", r.checksum8), zh);
                        checksum_row(ui, "SUM16", format!("0x{:04X}", r.checksum16), zh);
                    });
            } else {
                ui.label(
                    RichText::new(crate::i18n::t("Click Calculate to see results"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            // ===== 波特率自动检测 =====
            ui.label(
                RichText::new(crate::i18n::t("Baud Rate Detection"))
                    .color(theme::CYAN)
                    .strong(),
            );
            ui.add_space(6.0);

            // 端口选择
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(l10n.t("connect.port")).color(theme::TEXT_SECONDARY),
                );
                let port_names: Vec<String> =
                    state.port_list.iter().map(|p| p.name.clone()).collect();
                if detect_port_idx >= port_names.len() {
                    detect_port_idx = 0;
                }
                let selected = port_names
                    .get(detect_port_idx)
                    .cloned()
                    .unwrap_or_else(|| "—".to_string());
                egui::ComboBox::from_id_salt("tools_port_select")
                    .selected_text(selected)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        if port_names.is_empty() {
                            ui.label(
                                RichText::new(crate::i18n::t("No ports"))
                                    .color(theme::TEXT_DISABLED),
                            );
                        }
                        for (i, n) in port_names.iter().enumerate() {
                            ui.selectable_value(&mut detect_port_idx, i, n);
                        }
                    })
                    .response
                    .on_hover_text(crate::i18n::t("Select the serial port to detect baud rate"));
            });

            ui.add_space(4.0);
            // 数据位 / 校验 / 停止位
            egui::Grid::new("tools_baud_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(l10n.t("connect.data_bits")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("tools_db_select")
                        .selected_text(format!("{}", cfg_data_bits.bits()))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg_data_bits, DataBits::Five, "5");
                            ui.selectable_value(&mut cfg_data_bits, DataBits::Six, "6");
                            ui.selectable_value(&mut cfg_data_bits, DataBits::Seven, "7");
                            ui.selectable_value(&mut cfg_data_bits, DataBits::Eight, "8");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("Data bits: bits per character, commonly 8 (standard UART)"));
                    ui.end_row();

                    ui.label(
                        RichText::new(l10n.t("connect.parity")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("tools_par_select")
                        .selected_text(parity_label(cfg_parity))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg_parity, Parity::None, "None");
                            ui.selectable_value(&mut cfg_parity, Parity::Even, "Even");
                            ui.selectable_value(&mut cfg_parity, Parity::Odd, "Odd");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("Parity: None / Even / Odd"));
                    ui.end_row();

                    ui.label(
                        RichText::new(l10n.t("connect.stop_bits")).color(theme::TEXT_SECONDARY),
                    );
                    egui::ComboBox::from_id_salt("tools_sb_select")
                        .selected_text(format!("{}", cfg_stop_bits.bits()))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg_stop_bits, StopBits::One, "1");
                            ui.selectable_value(&mut cfg_stop_bits, StopBits::Two, "2");
                        })
                        .response
                        .on_hover_text(crate::i18n::t("Stop bits: marks end of character, 1 is most common, 2 for low-speed"));
                    ui.end_row();
                });

            ui.add_space(6.0);
            // 检测按钮
            ui.horizontal(|ui| {
                let btn_text = if detecting {
                    crate::i18n::t("Detecting...")
                } else { crate::i18n::t("Start Detect") };
                let btn = egui::Button::new(
                    RichText::new(btn_text).color(Color32::BLACK).strong(),
                )
                .fill(theme::ACCENT)
                .min_size(Vec2::new(120.0, 30.0))
                .stroke(egui::Stroke::NONE);
                if ui
                    .add(btn)
                    .on_hover_text(crate::i18n::t("Auto-detect baud rate: sends probe commands and analyzes responses, ranks by confidence"))
                    .clicked()
                    && !detecting
                {
                    let port_name = state.port_list.get(detect_port_idx).map(|p| p.name.clone());
                    if let Some(name) = port_name {
                        detecting = true;
                        // 重置结果槽并启动后台线程（避免阻塞 UI）
                        *detect_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        let slot = detect_slot.clone();
                        let (db, par, sb) = (cfg_data_bits, cfg_parity, cfg_stop_bits);
                        std::thread::spawn(move || {
                            let res = detect_baud_rate(&name, db, par, sb)
                                .map_err(|e| e.to_string());
                            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
                        });
                    } else {
                        state.last_error = Some(
                            crate::i18n::t("No port selected").to_string(),
                        );
                    }
                }
            });

            ui.add_space(8.0);
            // 检测结果列表（已按置信度降序排序）
            ui.label(
                RichText::new(crate::i18n::t("Results (sorted by confidence)"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("baud_results_grid")
                        .num_columns(3)
                        .spacing([16.0, 3.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label(RichText::new("Baud").color(theme::TEXT_SECONDARY).small().strong());
                            ui.label(RichText::new("Bytes").color(theme::TEXT_SECONDARY).small().strong());
                            ui.label(RichText::new("Confidence").color(theme::TEXT_SECONDARY).small().strong());
                            ui.end_row();
                            for r in &detect_results {
                                let is_top = r.confidence > 0.5;
                                let color = if is_top { theme::SUCCESS } else { theme::TEXT_PRIMARY };
                                ui.label(
                                    RichText::new(format!("{}", r.baud_rate))
                                        .color(color)
                                        .family(egui::FontFamily::Monospace),
                                );
                                ui.label(
                                    RichText::new(format!("{}", r.bytes_read))
                                        .color(theme::TEXT_PRIMARY)
                                        .family(egui::FontFamily::Monospace),
                                );
                                ui.label(
                                    RichText::new(format!("{:.1}%", r.confidence * 100.0))
                                        .color(color)
                                        .family(egui::FontFamily::Monospace),
                                );
                                ui.end_row();
                            }
                        });
                });

            // ===== AI 本地模型（Ollama）设置（feature gate）=====
            #[cfg(feature = "ollama")]
            {
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                render_ollama_settings(ui, state, &mut ollama_panel, zh);
            }

            // 错误提示
            if let Some(err) = &state.last_error {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("\u{26A0} {}", err)).color(theme::ERROR).small());
            }
        });

    // 帧末写回持久化状态
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_input_text"), input_text));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_is_hex"), is_hex));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_results"), results));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_detect_port_idx"), detect_port_idx));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_detect_results"), detect_results));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_detecting"), detecting));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_db"), cfg_data_bits));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_par"), cfg_parity));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_sb"), cfg_stop_bits));
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_detect_slot"), detect_slot));
    #[cfg(feature = "ollama")]
    ctx.data_mut(|d| d.insert_temp(Id::new("tools_ollama_panel"), ollama_panel));
}

/// 校验结果单行：名称 + 值 + 复制按钮
fn checksum_row(ui: &mut egui::Ui, name: &str, value: String, _zh: bool) {
    ui.label(RichText::new(name).color(theme::TEXT_SECONDARY).small());
    ui.label(
        RichText::new(value.clone())
            .color(theme::TEXT_PRIMARY)
            .family(egui::FontFamily::Monospace),
    );
    if ui
        .button(RichText::new(crate::i18n::t("Copy")).small())
        .on_hover_text(crate::i18n::t("Copy checksum result to clipboard"))
        .clicked()
    {
        ui.ctx().copy_text(value);
    }
    ui.end_row();
}

/// 校验位显示标签
fn parity_label(p: Parity) -> &'static str {
    match p {
        Parity::None => "None",
        Parity::Even => "Even",
        Parity::Odd => "Odd",
    }
}

// ============================================================================
// AI 本地模型（Ollama）设置分组（feature gate，Phase 4）
// 配置持久化到 ~/Library/Application Support/miconlab/config.json，
// 修改即时保存（文本框失焦/下拉选择/开关切换时），三面板自动联动。
// ============================================================================

/// 渲染 Ollama 设置分组（每帧调用：状态轮询回收 + 配置编辑 + 拉取引导）
#[cfg(feature = "ollama")]
fn render_ollama_settings(
    ui: &mut egui::Ui,
    state: &mut AppState,
    panel: &mut OllamaPanelState,
    _zh: bool,
) {
    // ---- 0. 配置签名变化（含从磁盘加载的首帧）→ 强制重查状态 ----
    let sig = state.settings.ollama.signature();
    if panel.cfg_sig.as_deref() != Some(sig.as_str()) {
        panel.cfg_sig = Some(sig);
        panel.status = None;
        panel.status_checked_at = None;
    }

    // ---- 1. 状态轮询（30s 节流，后台线程）----
    let due = panel
        .status_checked_at
        .map_or(true, |t| t.elapsed() > std::time::Duration::from_secs(30));
    if due && !panel.status_busy && !panel.pulling {
        panel.status_busy = true;
        let slot = panel.status_slot.clone();
        let cfg = state.settings.ollama.to_ollama_config();
        std::thread::spawn(move || {
            let st = OllamaClient::new(cfg)
                .map(|c| c.status_blocking())
                .unwrap_or(OllamaStatus::NotRunning);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(st);
        });
    }
    if panel.status_busy {
        if let Some(st) = panel.status_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            panel.status = Some(st);
            panel.status_checked_at = Some(std::time::Instant::now());
            panel.status_busy = false;
        }
    }

    // ---- 2. 拉取结果回收 ----
    if panel.pulling && panel.pull_done.load(Ordering::Relaxed) {
        let res = panel.pull_slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        panel.pulling = false;
        panel.pull_done.store(false, Ordering::Relaxed);
        match res {
            Some(Ok(())) => {
                // 拉取成功：立即重查状态（应变为 Ready）
                panel.status = None;
                panel.status_checked_at = None;
            }
            Some(Err(e)) => {
                state.last_error = Some(format!("Ollama pull: {}", e));
            }
            None => {}
        }
    }

    // ---- 3. UI ----
    ui.label(
        RichText::new(crate::i18n::t("AI Local Model (Ollama)"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.label(
        RichText::new(crate::i18n::t("Local LLM auto-identifies chips/firmware/protocols (fully offline). Falls back to rule engine on failure."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );
    ui.add_space(6.0);

    egui::Grid::new("ollama_cfg_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            // 服务地址
            ui.label(RichText::new(crate::i18n::t("Base URL")).color(theme::TEXT_SECONDARY).small());
            let url_resp = ui.add(
                egui::TextEdit::singleline(&mut state.settings.ollama.base_url)
                    .desired_width(300.0)
                    .hint_text("http://127.0.0.1:11434"),
            );
            if url_resp.changed() {
                state.settings.save();
                panel.cfg_sig = Some(state.settings.ollama.signature());
                panel.status = None;
                panel.status_checked_at = None;
            }
            if url_resp.lost_focus() {
                // 失焦去空格再存
                let t = state.settings.ollama.base_url.trim().to_string();
                if t != state.settings.ollama.base_url {
                    state.settings.ollama.base_url = t;
                    state.settings.save();
                    panel.cfg_sig = Some(state.settings.ollama.signature());
                    panel.status = None;
                    panel.status_checked_at = None;
                }
            }
            ui.end_row();

            // 识别模型：已安装下拉 + 手动输入
            ui.label(RichText::new(crate::i18n::t("Model##panels/tools.rs:23959")).color(theme::TEXT_SECONDARY).small());
            ui.horizontal(|ui| {
                let installed: Vec<String> = match &panel.status {
                    Some(OllamaStatus::Ready(names)) | Some(OllamaStatus::ModelMissing(names)) => {
                        names.clone()
                    }
                    _ => Vec::new(),
                };
                let selected = state.settings.ollama.model.clone();
                egui::ComboBox::from_id_salt("ollama_model_combo")
                    .selected_text(if installed.is_empty() {
                        crate::i18n::t("(no installed models)").to_string()
                    } else {
                        selected.clone()
                    })
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for name in &installed {
                            ui.selectable_value(&mut state.settings.ollama.model, name.clone(), name);
                        }
                    });
                let model_resp = ui.add(
                    egui::TextEdit::singleline(&mut state.settings.ollama.model)
                        .desired_width(180.0)
                        .hint_text("qwen2.5:7b-instruct"),
                );
                if model_resp.changed() {
                    state.settings.save();
                    panel.cfg_sig = Some(state.settings.ollama.signature());
                    panel.status = None;
                    panel.status_checked_at = None;
                }
                let _ = selected;
            });
            ui.end_row();

            // 自动增强开关
            ui.label(RichText::new(crate::i18n::t("Auto Enhance")).color(theme::TEXT_SECONDARY).small());
            let auto_resp = ui.add_enabled(
                !panel.pulling,
                egui::Checkbox::new(
                    &mut state.settings.ollama.auto_enabled,
                    crate::i18n::t("Auto-run local model when rule engine confidence is low"),
                ),
            );
            if auto_resp.changed() {
                state.settings.save();
            }
            ui.end_row();
        });

    ui.add_space(6.0);

    // ---- 4. 状态行 + 拉取引导 ----
    ui.horizontal(|ui| {
        match &panel.status {
            Some(OllamaStatus::Ready(names)) => {
                ui.label(
                    RichText::new(format!(
                        "🟢 {} ({})",
                        { crate::i18n::t("Ready") },
                        { crate::i18n::tf("{} models", &[("{}", "{}", &format!("{}", names.len()))]) }
                    ))
                    .color(theme::SUCCESS)
                    .small(),
                );
            }
            Some(OllamaStatus::ModelMissing(_)) => {
                ui.label(
                    RichText::new(crate::i18n::t("🟡 Service running, model not pulled"))
                    .color(theme::WARNING)
                    .small(),
                );
                let pull_btn = egui::Button::new(
                    RichText::new(crate::i18n::tf("⬇ Pull {} (~4.7GB)", &[("{}", "{}", &format!("{}", state.settings.ollama.model))]))
                    .color(Color32::BLACK)
                    .strong(),
                )
                .fill(theme::ACCENT);
                if !panel.pulling && ui.add(pull_btn).clicked() {
                    start_pull(panel, state);
                }
            }
            Some(OllamaStatus::NotRunning) => {
                ui.label(
                    RichText::new(crate::i18n::t("⚪ Not running")).color(theme::TEXT_DISABLED).small(),
                );
                ui.label(
                    RichText::new(crate::i18n::t("Install: brew install ollama && ollama serve (or download from ollama.com)"))
                    .color(theme::TEXT_DISABLED)
                    .small(),
                );
            }
            None => {
                ui.label(
                    RichText::new(crate::i18n::t("⚪ Detecting…"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            }
        }
    });

    // 拉取进度条
    if panel.pulling {
        ui.add_space(4.0);
        let (frac, text) = panel
            .pull_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let frac = frac.clamp(0.0, 1.0);
        let text = if text.is_empty() {
            crate::i18n::t("connecting…").to_string()
        } else {
            text
        };
        ui.add(
            egui::ProgressBar::new(frac)
                .show_percentage()
                .text(format!("{} {}", crate::i18n::t("Pulling"), text)),
        );
        // 拉取中持续重绘 UI（进度更新）
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }

    // 配置文件路径提示
    if let Some(path) = crate::settings::AppSettings::config_path() {
        ui.add_space(4.0);
        ui.label(
            RichText::new(crate::i18n::tf("Config: {}", &[("{}", "{}", &format!("{}", path.display()))]))
            .color(theme::TEXT_DISABLED)
            .small(),
        );
    }
}

/// 启动模型拉取后台线程（流式进度）
#[cfg(feature = "ollama")]
fn start_pull(panel: &mut OllamaPanelState, state: &AppState) {
    panel.pulling = true;
    panel.pull_done.store(false, Ordering::Relaxed);
    *panel.pull_progress.lock().unwrap_or_else(|e| e.into_inner()) = (0.0, String::new());
    let cfg = state.settings.ollama.to_ollama_config();
    let done = panel.pull_done.clone();
    let slot = panel.pull_slot.clone();
    let prog = panel.pull_progress.clone();
    let model = state.settings.ollama.model.clone();
    std::thread::spawn(move || {
        let res = match OllamaClient::new(cfg) {
            Ok(client) => client
                .pull_model_blocking(move |frac, status| {
                    *prog.lock().unwrap_or_else(|e| e.into_inner()) = (frac, status);
                })
                .map_err(|e| e.to_string())
                .map(|_| {
                    log::info!("{}", crate::i18n::tf("Ollama model pull complete: {model}", &[("{model}", "{model}", &model)]));
                }),
            Err(e) => Err(e.to_string()),
        };
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
        done.store(true, Ordering::Relaxed);
    });
}
