// CAN 总线面板
// 提供 SLCAN 会话控制、CAN 帧发送、接收帧列表解析与统计、任意 SLCAN 命令输入。
//
// 因 CanAnalyzer 未实现 Clone，无法直接存入 egui 0.29 的 temp storage
// （要求 T: Clone + Send + Sync），故整体包裹在 Arc<Mutex<>> 中。
// 这是 egui 官方对非 Clone 大对象的推荐做法（见 IdTypeMap 文档注释）。

use crate::i18n::{Language, L10n};
use crate::state::{AppState, Direction};
use crate::theme;
use egui::{Id, RichText};
use miconlab_core::hex;
use miconlab_core::protocol::can::{
    slcan_bit_rate_label, slcan_close, slcan_open, CanAnalyzer, CanFrame,
};
use std::sync::{Arc, Mutex};

/// CAN 面板持久化状态。
struct CanPanelState {
    analyzer: CanAnalyzer,
    // 发送区
    send_id: String,
    send_data: String,
    is_extended: bool,
    is_fd: bool,
    brs: bool,
    // SLCAN 命令输入
    slcan_input: String,
    // 接收帧日志（显示用，最近 N 条）
    frame_log: Vec<String>,
    // 会话
    speed_index: usize,
    session_open: bool,
    // 增量解析 rx_buffer 的游标：跟踪已处理的 bytes_rx 偏移。
    // bytes_rx 是单调递增的全局接收字节计数器，不受 rx_buffer drain 裁剪影响，
    // 用它做增量跟踪可避免绝对索引在 drain 后错位。
    line_accumulator: String,
    last_bytes_rx: u64,
    /// 上次连接会话 ID，用于检测连接变化并清除设备派生数据
    last_session_id: u64,
}

impl Default for CanPanelState {
    fn default() -> Self {
        Self {
            analyzer: CanAnalyzer::new(),
            send_id: String::from("123"),
            send_data: String::from("AA BB"),
            is_extended: false,
            is_fd: false,
            brs: false,
            slcan_input: String::new(),
            frame_log: Vec::new(),
            speed_index: 6, // 默认 500 kbps
            session_open: false,
            line_accumulator: String::new(),
            last_bytes_rx: 0,
            last_session_id: 0,
        }
    }
}

/// 帧日志最大条数
const MAX_FRAME_LOG: usize = 500;
/// 可选波特率索引（0-8 对应 10kbps - 1Mbps）
const SPEED_OPTIONS: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7, 8];

/// 从 rx_buffer 增量提取新数据，按 \r 切分完整 SLCAN 行并解析入 analyzer。
///
/// 使用 `last_bytes_rx` 跟踪已处理的 `bytes_rx` 偏移。`bytes_rx` 是单调递增的
/// 全局接收字节计数器，不受 `rx_buffer` 在达到 MAX_ENTRIES 时 `drain` 旧条目
/// 的影响，因此可避免绝对索引在 drain 后错位（跳过未处理条目或重复解析）。
fn poll_rx_frames(st: &mut CanPanelState, state: &AppState) {
    // 防御：bytes_rx 不应回退（除非被外部重置），重置基准避免永久失效
    if state.bytes_rx < st.last_bytes_rx {
        st.last_bytes_rx = state.bytes_rx;
        st.line_accumulator.clear();
        return;
    }
    if state.bytes_rx <= st.last_bytes_rx {
        return;
    }
    let new_count = (state.bytes_rx - st.last_bytes_rx) as usize;
    st.last_bytes_rx = state.bytes_rx;

    // 从 rx_buffer 末尾向前收集恰好 new_count 个 RX 方向字节所属的条目。
    // 仅取 RX 方向（跳过本机发送的 Tx 回显），与原逻辑一致。
    let mut new_entries: Vec<_> = Vec::new();
    let mut collected = 0usize;
    for entry in state.rx_buffer.iter().rev() {
        if collected >= new_count {
            break;
        }
        if entry.direction != Direction::Rx {
            continue;
        }
        new_entries.push(entry);
        collected += entry.data.len();
    }
    new_entries.reverse();

    for entry in new_entries {
        // SLCAN 协议使用 ASCII 文本。检测数据是否为有效 ASCII：
        // 允许 \r \n \t 和 0x20-0x7E 的可打印字符
        let is_ascii_text = entry.data.iter().all(|&b| {
            b == b'\r' || b == b'\n' || b == b'\t' || (0x20..=0x7E).contains(&b)
        });

        if !is_ascii_text {
            // 二进制数据（非 SLCAN），以 HEX 格式记录，不尝试解析
            // 同时清空行缓冲（二进制数据不应混入 SLCAN 行）
            st.line_accumulator.clear();
            st.frame_log.push(format!(
                "< [BIN] {}",
                hex::format_hex_spaced(&entry.data)
            ));
            if st.frame_log.len() > MAX_FRAME_LOG {
                let drop = st.frame_log.len() - MAX_FRAME_LOG;
                st.frame_log.drain(0..drop);
            }
            continue;
        }

        // 累积到行缓冲
        st.line_accumulator
            .push_str(&String::from_utf8_lossy(&entry.data));

        // 防止行缓冲无限增长（无 \r 的数据最多累积 256 字节）
        if st.line_accumulator.len() > 256 {
            let drained: String = st.line_accumulator.drain(..).collect();
            st.frame_log.push(format!("< [TRUNC] {}", drained.trim()));
            if st.frame_log.len() > MAX_FRAME_LOG {
                let drop = st.frame_log.len() - MAX_FRAME_LOG;
                st.frame_log.drain(0..drop);
            }
        }

        // 按 \r 切分完整 SLCAN 行
        while let Some(pos) = st.line_accumulator.find('\r') {
            let line: String = st.line_accumulator.drain(..=pos).collect();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // 尝试解析为 SLCAN 帧
            match st.analyzer.parse_slcan(line) {
                Ok(frame) => {
                    // format_frame 必须在 add_frame 消费 frame 之前调用
                    let display = CanAnalyzer::format_frame(&frame);
                    st.analyzer.add_frame(frame);
                    st.frame_log.push(display);
                }
                Err(_) => {
                    // 非帧行（如 OK / 错误响应）也记录
                    st.frame_log.push(format!("< {}", line));
                }
            }

            // 限制日志长度
            if st.frame_log.len() > MAX_FRAME_LOG {
                let drop = st.frame_log.len() - MAX_FRAME_LOG;
                st.frame_log.drain(0..drop);
            }
        }
    }
}

/// 打开 SLCAN 会话：发送 C / S<n> / O 命令序列
fn open_session(st: &mut CanPanelState, state: &mut AppState, _zh: bool) {
    let speed = SPEED_OPTIONS[st.speed_index.min(SPEED_OPTIONS.len() - 1)];
    match slcan_open(speed) {
        Ok(cmds) => {
            for cmd in &cmds {
                let mut bytes = cmd.as_bytes().to_vec();
                bytes.push(b'\r');
                if let Err(e) = state.send_raw(&bytes) {
                    state.last_error = Some(e);
                    return;
                }
            }
            st.session_open = true;
        }
        Err(e) => {
            state.last_error = Some(crate::i18n::tf("open session failed: {}", &[("{}", "{}", &format!("{}", e))]));
        }
    }
}

/// 关闭 SLCAN 会话：发送 C 命令
fn close_session(st: &mut CanPanelState, state: &mut AppState) {
    let cmd = slcan_close();
    let mut bytes = cmd.as_bytes().to_vec();
    bytes.push(b'\r');
    if let Err(e) = state.send_raw(&bytes) {
        state.last_error = Some(e);
    } else {
        st.session_open = false;
    }
}

/// 发送一个 CAN 帧
fn send_can_frame(st: &mut CanPanelState, state: &mut AppState, _zh: bool) {
    // 解析 ID（支持 0x 前缀十六进制或十进制）
    let id_str = st.send_id.trim();
    let id = if let Some(h) = id_str.strip_prefix("0x").or_else(|| id_str.strip_prefix("0X")) {
        match u32::from_str_radix(h, 16) {
            Ok(v) => v,
            Err(_) => {
                state.last_error = Some(crate::i18n::t("ID parse failed").to_string());
                return;
            }
        }
    } else {
        match id_str.parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                state.last_error = Some(crate::i18n::t("ID parse failed").to_string());
                return;
            }
        }
    };

    // 解析数据 hex
    let data = match hex::parse_hex(&st.send_data) {
        Ok(d) => d,
        Err(e) => {
            state.last_error = Some(crate::i18n::tf("data parse failed: {}", &[("{}", "{}", &format!("{}", e))]));
            return;
        }
    };

    // 构造帧（format_frame 必须在 add_frame 消费前调用）
    let frame = if st.is_fd {
        CanFrame::new_fd(id, st.is_extended, data, st.brs)
    } else if st.is_extended {
        CanFrame::new_extended(id, data)
    } else {
        CanFrame::new_standard(id, data)
    };
    let display = CanAnalyzer::format_frame(&frame);
    let slcan = frame.to_slcan();

    // 发送 SLCAN 帧 + \r
    let mut bytes = slcan.into_bytes();
    bytes.push(b'\r');
    match state.send_raw(&bytes) {
        Ok(_) => {
            // 本地回显到帧日志
            st.frame_log.push(format!("> {}", display));
            if st.frame_log.len() > MAX_FRAME_LOG {
                let drop = st.frame_log.len() - MAX_FRAME_LOG;
                st.frame_log.drain(0..drop);
            }
        }
        Err(e) => {
            state.last_error = Some(e);
        }
    }
}

/// 渲染 CAN 面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let lang = state.language;
    let zh = lang == Language::Zh;

    // 因 CanAnalyzer 不实现 Clone，使用 Arc<Mutex<>> 包裹（egui 官方推荐做法）。
    // 在 CentralPanel 闭包外获取 Arc，避免嵌套闭包的借用冲突。
    let st_arc: Arc<Mutex<CanPanelState>> = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<CanPanelState>>>(Id::new("can_state_v1"))
            .clone()
    });

    // 连接会话变化时清除设备派生数据（帧日志、统计、行累加器），避免跨设备混合
    {
        let mut st = st_arc.lock().unwrap_or_else(|e| e.into_inner());
        if st.last_session_id != state.connection_session_id {
            st.frame_log.clear();
            st.line_accumulator.clear();
            st.analyzer = CanAnalyzer::new();
            st.session_open = false;
            st.last_bytes_rx = state.bytes_rx;
            st.last_session_id = state.connection_session_id;
        }
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            ui.heading(
                RichText::new(l10n.t("panel.can"))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(crate::i18n::t("Interact with CAN adapters via SLCAN command set, send/receive CAN frames"))
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

            // 未连接提示
            if !state.is_connected() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(
                        RichText::new(crate::i18n::t("Not connected. Open a port in the Connection panel first."))
                        .color(theme::WARNING),
                    );
                });
                ui.add_space(8.0);
            }

            let mut st = st_arc.lock().unwrap_or_else(|e| e.into_inner());

            // 增量解析接收数据
            poll_rx_frames(&mut st, state);

            // ---- 会话控制区 ----
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Bit Rate"))
                        .color(theme::TEXT_SECONDARY),
                );
                let cur_speed = SPEED_OPTIONS[st.speed_index.min(SPEED_OPTIONS.len() - 1)];
                let cur_label = slcan_bit_rate_label(cur_speed).unwrap_or("—");
                egui::ComboBox::from_id_salt("can_speed_select")
                    .selected_text(cur_label)
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for (i, &sp) in SPEED_OPTIONS.iter().enumerate() {
                            if let Some(lbl) = slcan_bit_rate_label(sp) {
                                ui.selectable_value(&mut st.speed_index, i, lbl);
                            }
                        }
                    })
                    .response
                    .on_hover_text(crate::i18n::t("CAN bus baud rate (distinct from the serial baud rate)"));

                let connected = state.is_connected();
                if st.session_open {
                    let btn = egui::Button::new(
                        RichText::new(crate::i18n::t("Close Session"))
                            .color(egui::Color32::BLACK)
                            .strong(),
                    )
                    .fill(theme::ERROR)
                    .stroke(egui::Stroke::NONE);
                    if ui.add_enabled(connected, btn)
                        .on_hover_text(crate::i18n::t("Send the C SLCAN command to close the CAN controller"))
                        .clicked()
                    {
                        close_session(&mut st, state);
                    }
                } else {
                    let btn = egui::Button::new(
                        RichText::new(crate::i18n::t("Open Session"))
                            .color(egui::Color32::BLACK)
                            .strong(),
                    )
                    .fill(theme::ACCENT)
                    .stroke(egui::Stroke::NONE);
                    if ui.add_enabled(connected, btn)
                        .on_hover_text(crate::i18n::t("Send the O SLCAN command to open the CAN controller"))
                        .clicked()
                    {
                        open_session(&mut st, state, zh);
                    }
                }

                // 状态指示灯
                let (dot, dot_color) = if st.session_open {
                    ("\u{25CF}", theme::SUCCESS)
                } else {
                    ("\u{25CB}", theme::TEXT_DISABLED)
                };
                ui.label(RichText::new(dot).color(dot_color).size(12.0));
                ui.label(
                    RichText::new(if st.session_open {
                        crate::i18n::t("Session open")
                    } else { crate::i18n::t("Session closed") })
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );

                // 清空按钮
                if ui
                    .button(RichText::new(crate::i18n::t("Clear")).color(theme::TEXT_PRIMARY))
                    .on_hover_text(crate::i18n::t("Clear the received frame log"))
                    .clicked()
                {
                    st.analyzer.clear();
                    st.frame_log.clear();
                    st.line_accumulator.clear();
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ---- 帧发送区 ----
            ui.label(
                RichText::new(crate::i18n::t("Send CAN Frame"))
                    .color(theme::TEXT_SECONDARY)
                    .strong(),
            );
            ui.add_space(4.0);
            egui::Grid::new("can_send_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("ID").color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut st.send_id)
                            .desired_width(160.0)
                            .code_editor(),
                    )
                    .on_hover_text(crate::i18n::t("CAN frame ID: 11-bit standard (0x000-0x7FF) or 29-bit extended (0x00000000-0x1FFFFFFF), 0x prefix supported"));
                    ui.end_row();

                    ui.label(
                        RichText::new(crate::i18n::t("Data (hex)"))
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut st.send_data)
                            .desired_width(320.0)
                            .code_editor(),
                    )
                    .on_hover_text(crate::i18n::t("CAN frame data payload (hex), up to 8 bytes for standard, 64 bytes for CAN FD"));
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut st.is_extended,
                    crate::i18n::t("Extended (29-bit)"),
                )
                .on_hover_text(crate::i18n::t("Extended frame uses 29-bit ID, standard frame uses 11-bit ID"));
                ui.checkbox(&mut st.is_fd, "CAN FD")
                    .on_hover_text(crate::i18n::t("CAN FD (Flexible Data-rate): supports higher data rates and up to 64 byte payload"));
                if st.is_fd {
                    ui.checkbox(&mut st.brs, "BRS")
                        .on_hover_text(crate::i18n::t("BRS (Bit Rate Switch): switch to higher baud rate in CAN FD data phase"));
                }

                let can_send = state.is_connected() && st.session_open;
                let btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Send Frame"))
                        .color(egui::Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .stroke(egui::Stroke::NONE);
                if ui.add_enabled(can_send, btn)
                    .on_hover_text(crate::i18n::t("Build an SLCAN t/T command and send a CAN frame"))
                    .clicked()
                {
                    send_can_frame(&mut st, state, zh);
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ---- 接收帧列表 ----
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::i18n::t("Received Frames"))
                        .color(theme::TEXT_SECONDARY)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        crate::i18n::t("Total"),
                        st.analyzer.frame_count()
                    ))
                    .color(theme::TEXT_DISABLED)
                    .small(),
                );
            });
            ui.add_space(4.0);

            egui::Frame::group(ui.style())
                .fill(theme::BG_DEEP)
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_min_height(160.0);
                    if st.frame_log.is_empty() {
                        ui.label(
                            RichText::new(crate::i18n::t("Waiting for frames..."))
                            .color(theme::TEXT_DISABLED)
                            .family(egui::FontFamily::Monospace),
                        );
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for line in &st.frame_log {
                                    let color = if line.starts_with('>') {
                                        theme::ACCENT_DIM
                                    } else if line.starts_with('<') {
                                        theme::TEXT_SECONDARY
                                    } else {
                                        theme::CYAN
                                    };
                                    ui.label(
                                        RichText::new(line.as_str())
                                            .color(color)
                                            .family(egui::FontFamily::Monospace)
                                            .small(),
                                    );
                                }
                            });
                    }
                });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ---- 帧统计区 ----
            ui.label(
                RichText::new(crate::i18n::t("Frame Statistics (per ID)"))
                    .color(theme::TEXT_SECONDARY)
                    .strong(),
            );
            ui.add_space(4.0);

            // 将统计克隆为 owned 数据，以便释放 analyzer 借用后继续渲染
            let stats_owned: Vec<(u32, usize, f64, Vec<u8>)> = st
                .analyzer
                .all_stats()
                .iter()
                .map(|(id, s)| (*id, s.count, s.avg_interval_ms, s.last_data.clone()))
                .collect();

            if stats_owned.is_empty() {
                ui.label(
                    RichText::new(crate::i18n::t("No statistics yet"))
                        .color(theme::TEXT_DISABLED)
                        .small(),
                );
            } else {
                egui::Frame::group(ui.style())
                    .fill(theme::BG_DEEP)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        egui::Grid::new("can_stats_grid")
                            .num_columns(4)
                            .spacing([16.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("ID")
                                        .color(theme::TEXT_SECONDARY)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(crate::i18n::t("Count"))
                                        .color(theme::TEXT_SECONDARY)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(crate::i18n::t("Avg Interval(ms)"))
                                        .color(theme::TEXT_SECONDARY)
                                        .strong(),
                                );
                                ui.label(
                                    RichText::new(crate::i18n::t("Last Data"))
                                        .color(theme::TEXT_SECONDARY)
                                        .strong(),
                                );
                                ui.end_row();

                                for (id, count, avg, last_data) in &stats_owned {
                                    ui.label(
                                        RichText::new(format!("0x{:08X}", id))
                                            .color(theme::CYAN)
                                            .family(egui::FontFamily::Monospace),
                                    );
                                    ui.label(
                                        RichText::new(format!("{}", count))
                                            .color(theme::TEXT_PRIMARY),
                                    );
                                    ui.label(
                                        RichText::new(format!("{:.2}", avg))
                                            .color(theme::TEXT_PRIMARY),
                                    );
                                    ui.label(
                                        RichText::new(hex::format_hex_spaced(last_data))
                                            .color(theme::TEXT_PRIMARY)
                                            .family(egui::FontFamily::Monospace),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ---- 任意 SLCAN 命令输入 ----
            ui.label(
                RichText::new(crate::i18n::t("SLCAN Command"))
                    .color(theme::TEXT_SECONDARY)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let connected = state.is_connected();
                ui.add(
                    egui::TextEdit::singleline(&mut st.slcan_input)
                        .desired_width(320.0)
                        .code_editor(),
                )
                .on_hover_text(crate::i18n::t("Enter any SLCAN command (e.g. O=open, C=close, S8=set 500Kbps, t1L8D...=send standard frame). Trailing \\r is auto-appended on send"));
                let btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Send"))
                        .color(egui::Color32::BLACK)
                        .strong(),
                )
                .fill(theme::ACCENT)
                .stroke(egui::Stroke::NONE);
                if ui
                    .add_enabled(connected, btn)
                    .on_hover_text(crate::i18n::t("Send the typed SLCAN command to the CAN adapter (serial connection required). \\r is auto-appended"))
                    .clicked() {
                    let mut line = st.slcan_input.trim().to_string();
                    if !line.ends_with('\r') {
                        line.push('\r');
                    }
                    if let Err(e) = state.send_raw(line.as_bytes()) {
                        state.last_error = Some(e);
                    } else {
                        st.slcan_input.clear();
                    }
                }
            });

            // ---- 底部错误信息 ----
            if let Some(err) = &state.last_error {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
                    ui.label(RichText::new(err).color(theme::ERROR));
                });
            }
        });
}
