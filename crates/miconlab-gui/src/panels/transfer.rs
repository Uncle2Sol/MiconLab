// 文件传输 + 脚本录制/回放面板
// 三个子区域：文件传输、脚本录制、脚本回放
//
// 设计说明：
// - 面板状态（可持久化、Clone+Send+Sync 的字段）通过 ctx.data_mut() 持久化。
// - ScriptRecorder / ScriptReplayer 非 Clone，无法放入 IdTypeMap，
//   故使用 thread_local RefCell 暂存（与现有 modbus.rs 骨架一致）。
// - 文件传输采用同步执行（会短暂卡 UI），通过 RefCell 包装 &mut state.port
//   以便 write/read 两个闭包共享串口可变引用（send_file 顺序调用二者，无并发）。

use crate::i18n::{Language, L10n};
use crate::state::{self, AppState, Direction};
use crate::theme;
use egui::{Color32, RichText, ScrollArea, Vec2};
use miconlab_core::file_transfer::{
    FileTransfer, TransferError, TransferProtocol, TransferProgress,
};
use miconlab_core::recorder::{Action, ScriptRecorder, ScriptReplayer};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

// 持久化面板状态（必须 Clone + Send + Sync，才能存入 egui::IdTypeMap）
#[derive(Clone)]
struct TransferPanelState {
    // 文件传输
    protocol: TransferProtocol,
    file_path: String,
    progress: f32, // 0.0..=1.0
    speed_bps: f64,
    eta_secs: Option<f64>,
    log: Vec<String>,
    // 脚本录制
    rec_name: String,
    rec_desc: String,
    recording: bool,
    rec_save_path: String,
    last_recorded_ts_ms: i64,
    // 脚本回放
    script_path: String,
    replaying: bool,
    replay_progress: f32,
    replay_current: String,
    replay_log: Vec<String>,
    replay_next_time_ms: i64,
}

impl Default for TransferPanelState {
    fn default() -> Self {
        Self {
            protocol: TransferProtocol::Xmodem,
            file_path: String::new(),
            progress: 0.0,
            speed_bps: 0.0,
            eta_secs: None,
            log: Vec::new(),
            rec_name: "script".to_string(),
            rec_desc: String::new(),
            recording: false,
            rec_save_path: String::new(),
            last_recorded_ts_ms: 0,
            script_path: String::new(),
            replaying: false,
            replay_progress: 0.0,
            replay_current: String::new(),
            replay_log: Vec::new(),
            replay_next_time_ms: 0,
        }
    }
}

impl TransferPanelState {
    /// 推送一条传输日志，并限制日志长度
    fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        while self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    /// 推送一条回放日志，并限制日志长度
    fn push_replay_log(&mut self, line: impl Into<String>) {
        self.replay_log.push(line.into());
        while self.replay_log.len() > 200 {
            self.replay_log.remove(0);
        }
    }
}

// 非持久化对象（非 Clone）：录制器与回放器
thread_local! {
    static RECORDER: RefCell<Option<ScriptRecorder>> = RefCell::new(None);
    static REPLAYER: RefCell<Option<ScriptReplayer>> = RefCell::new(None);
}

/// 渲染文件传输 / 脚本录制 / 脚本回放面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let is_zh = state.language == Language::Zh;

    // 注意：egui 0.29 的 CentralPanel::show 持有 Context 内部 RwLock 的写锁，
    // 闭包内再调用 ctx.data_mut() 会重入写锁导致永久死锁。
    // 因此在 CentralPanel 之前取出持久化状态的克隆，闭包内使用克隆，
    // 闭包结束后再写回。

    // CentralPanel 之前：克隆出持久化状态
    let mut ps = ctx
        .data_mut(|data| data.get_temp_mut_or_default::<TransferPanelState>(egui::Id::NULL).clone());

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style())
                .inner_margin(egui::Margin::same(12.0))
                .fill(theme::BG_SURFACE),
        )
        .show(ctx, |ui| {
            // 闭包内直接使用克隆出的状态，不再调用 ctx.data_mut（否则死锁）
            render_panel(ui, &mut ps, state, is_zh, &l10n);
        });

    // CentralPanel 之后：写回持久化状态
    ctx.data_mut(|data| {
        *data.get_temp_mut_or_default::<TransferPanelState>(egui::Id::NULL) = ps;
    });
}

/// 渲染整个面板：标题 + 三个折叠区 + 错误条
fn render_panel(
    ui: &mut egui::Ui,
    ps: &mut TransferPanelState,
    state: &mut AppState,
    is_zh: bool,
    l10n: &L10n,
) {
    ui.heading(
        RichText::new(l10n.t("panel.transfer"))
            .color(theme::TEXT_PRIMARY)
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("Transfer files over serial using XMODEM/YMODEM/ZMODEM protocols"))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    // AI 感知：错误诊断（连接失败或操作异常时自动推断根因）
    if let Some(err) = &state.last_error {
        if let Some(hint) = crate::panels::ai_sense::sense_error(err, state.is_connected(), state.config.baud_rate, is_zh) {
            crate::panels::ai_sense::render_ai_hint_bar(ui, &hint);
            ui.add_space(4.0);
        }
    }

    // 未连接提示（文件传输依赖当前串口）
    if !state.is_connected() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
            ui.label(
                RichText::new(crate::i18n::t("Not connected: file transfer and replay-send unavailable."))
                .color(theme::WARNING)
                .small(),
            );
        });
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // 1. 文件传输
    egui::CollapsingHeader::new(crate::i18n::t("File Transfer"))
        .default_open(true)
        .show(ui, |ui| {
            render_file_transfer(ui, ps, state, is_zh);
        });

    ui.add_space(4.0);

    // 2. 脚本录制
    egui::CollapsingHeader::new(crate::i18n::t("Record Script"))
        .default_open(true)
        .show(ui, |ui| {
            render_recorder(ui, ps, state, is_zh);
        });

    ui.add_space(4.0);

    // 3. 脚本回放
    egui::CollapsingHeader::new(crate::i18n::t("Replay Script"))
        .default_open(true)
        .show(ui, |ui| {
            render_replayer(ui, ps, state, is_zh);
        });

    ui.add_space(8.0);

    // 底部错误条
    if let Some(err) = &state.last_error {
        ui.separator();
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("\u{26A0}").color(theme::WARNING));
            ui.label(
                RichText::new(err)
                    .color(theme::ERROR)
                    .family(egui::FontFamily::Monospace)
                    .small(),
            );
        });
    }
}

// ---------------------------------------------------------------------------
// 文件传输
// ---------------------------------------------------------------------------

/// 协议下拉选项文本
fn protocol_label_direct(p: TransferProtocol) -> &'static str {
    match p {
        TransferProtocol::Xmodem => "Xmodem",
        TransferProtocol::XmodemCrc => "Xmodem-CRC",
        TransferProtocol::Ymodem => "Ymodem",
        TransferProtocol::Zmodem => "Zmodem",
    }
}

fn render_file_transfer(
    ui: &mut egui::Ui,
    ps: &mut TransferPanelState,
    state: &mut AppState,
    is_zh: bool,
) {
    egui::Grid::new("ft_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            // 协议选择
            ui.label(
                RichText::new(crate::i18n::t("Protocol##panels/transfer.rs:6864"))
                    .color(theme::TEXT_SECONDARY),
            );
            let cur = protocol_label_direct(ps.protocol);
            egui::ComboBox::from_id_salt("ft_protocol")
                .selected_text(cur)
                .width(160.0)
                .show_ui(ui, |ui| {
                    for p in [
                        TransferProtocol::Xmodem,
                        TransferProtocol::XmodemCrc,
                        TransferProtocol::Ymodem,
                        TransferProtocol::Zmodem,
                    ] {
                        ui.selectable_value(&mut ps.protocol, p, protocol_label_direct(p));
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("Xmodem: 128B/packet, simplest and most compatible; Xmodem-CRC: 128B + CRC16; Ymodem: 1024B/packet with filename support; Zmodem: streaming adaptive packet size, highest efficiency"));
            ui.end_row();

            // 文件路径
            ui.label(
                RichText::new(crate::i18n::t("File Path"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut ps.file_path)
                    .desired_width(320.0)
                    .hint_text("/path/to/file.bin"),
            )
            .on_hover_text(crate::i18n::t("Local file path (absolute or relative). When sending, the file is read and packetized per protocol; when receiving, incoming data is written to this file"));
            ui.end_row();
        });

    ui.add_space(4.0);

    // 发送 / 接收按钮
    let connected = state.is_connected();
    ui.horizontal(|ui| {
        let send_btn = egui::Button::new(
            RichText::new(crate::i18n::t("Send File"))
                .color(Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(Vec2::new(110.0, 28.0))
        .stroke(egui::Stroke::NONE);
        let recv_btn = egui::Button::new(
            RichText::new(crate::i18n::t("Receive File"))
                .color(theme::TEXT_PRIMARY),
        )
        .fill(theme::BG_ELEVATED)
        .min_size(Vec2::new(110.0, 28.0))
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER));

        let send_clicked = ui
            .add_enabled(connected, send_btn)
            .on_hover_text(crate::i18n::t("Send the local file to the receiver using the selected protocol. The receiver must be in receive mode first (e.g. type rz for Zmodem)"))
            .clicked();
        let recv_clicked = ui
            .add_enabled(connected, recv_btn)
            .on_hover_text(crate::i18n::t("Wait for the peer to send a file and save it to the specified path. The sender must initiate transfer first (e.g. type sz file)"))
            .clicked();

        // 同步执行发送 / 接收（会短暂卡顿 UI）
        if send_clicked {
            do_transfer(ps, state, is_zh, true);
        }
        if recv_clicked {
            do_transfer(ps, state, is_zh, false);
        }

        ui.separator();
        if ui
            .button(RichText::new(crate::i18n::t("Clear Log")).color(theme::TEXT_SECONDARY))
            .on_hover_text(crate::i18n::t("Clear transfer log"))
            .clicked()
        {
            ps.log.clear();
            ps.progress = 0.0;
            ps.speed_bps = 0.0;
            ps.eta_secs = None;
        }
    });

    ui.add_space(4.0);

    // 进度条 + 速度 + ETA
    ui.horizontal(|ui| {
        ui.add_sized(
            [260.0, 16.0],
            egui::ProgressBar::new(ps.progress)
                .text(format!("{:.1}%", ps.progress * 100.0)),
        );
        ui.label(
            RichText::new(format!("{:.1} KB/s", ps.speed_bps / 1024.0))
                .color(theme::CYAN)
                .small()
                .family(egui::FontFamily::Monospace),
        );
        if let Some(eta) = ps.eta_secs {
            ui.label(
                RichText::new(format!("ETA {:.0}s", eta))
                    .color(theme::TEXT_SECONDARY)
                    .small()
                    .family(egui::FontFamily::Monospace),
            );
        }
    });

    ui.add_space(4.0);

    // 日志区
    render_log_area(ui, "ft_log", &ps.log, 120.0);
}

/// 同步执行文件发送 / 接收
///
/// 用 RefCell 包装 `&mut state.port`，使 write/read 两个闭包能共享串口的可变引用。
/// send_file/receive_file 顺序调用闭包，无并发，故 borrow_mut 不会 panic。
fn do_transfer(
    ps: &mut TransferPanelState,
    state: &mut AppState,
    _is_zh: bool,
    send: bool,
) {
    if ps.file_path.trim().is_empty() {
        ps.push_log(crate::i18n::t("File path empty"));
        return;
    }
    let path = PathBuf::from(&ps.file_path);
    ps.push_log(format!(
        "{} {} ({})",
        { crate::i18n::t("Start") },
        if send { "send" } else { "receive" },
        path.display()
    ));
    ps.progress = 0.0;

    // 临时设长超时，便于协议握手读取 ACK
    let _ = state.port.set_timeout(Duration::from_millis(2000));
    let transfer = FileTransfer::new(ps.protocol);

    // 内层块：闭包持有 &mut state.port 与 &mut ps，块结束后释放，便于后续访问
    let result: Result<(), TransferError> = {
        let port_ref = RefCell::new(&mut state.port);

        let mut write_fn = |bytes: &[u8]| -> Result<(), TransferError> {
            let mut p = port_ref.borrow_mut();
            p.write(bytes)
                .map(|_| ())
                .map_err(|e| TransferError::TransferFailed(e.to_string()))
        };
        let mut read_fn = || -> Result<u8, TransferError> {
            let mut p = port_ref.borrow_mut();
            let mut buf = [0u8; 1];
            match p.read(&mut buf) {
                Ok(1) => Ok(buf[0]),
                Ok(_) => Err(TransferError::TransferFailed("read timeout".into())),
                Err(e) => Err(TransferError::TransferFailed(e.to_string())),
            }
        };
        let mut progress_fn = |prog: TransferProgress| {
            ps.progress = (prog.percentage() / 100.0).clamp(0.0, 1.0);
            ps.speed_bps = prog.speed_bps;
            ps.eta_secs = prog.eta.map(|d| d.as_secs_f64());
        };

        if send {
            transfer.send_file(&path, &mut write_fn, &mut read_fn, &mut progress_fn)
        } else {
            // 接收时 write_fn 用于发送 ACK/NAK 确认帧（与发送共享同一串口引用）
            transfer.receive_file(&path, &mut write_fn, &mut read_fn, &mut progress_fn)
        }
    };

    // 恢复短超时
    let _ = state.port.set_timeout(Duration::from_millis(10));

    match result {
        Ok(()) => {
            ps.progress = 1.0;
            ps.push_log(crate::i18n::t("Transfer done"));
            state.last_error = None;
        }
        Err(e) => {
            let msg = e.to_string();
            ps.push_log(format!("{}: {}", crate::i18n::t("Failed"), msg));
            state.last_error = Some(msg);
        }
    }
}

// ---------------------------------------------------------------------------
// 脚本录制
// ---------------------------------------------------------------------------

fn render_recorder(
    ui: &mut egui::Ui,
    ps: &mut TransferPanelState,
    state: &mut AppState,
    is_zh: bool,
) {
    egui::Grid::new("rec_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Name")).color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut ps.rec_name)
                    .desired_width(180.0)
                    .hint_text("script"),
            )
            .on_hover_text(crate::i18n::t("Identifier name of the recorded script"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Description"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut ps.rec_desc)
                    .desired_width(320.0)
                    .hint_text(crate::i18n::t("optional")),
            )
            .on_hover_text(crate::i18n::t("Script purpose description (optional)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Save Path"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut ps.rec_save_path)
                    .desired_width(320.0)
                    .hint_text("/path/to/script.json"),
            )
            .on_hover_text(crate::i18n::t("Save the script as a JSON file"));
            ui.end_row();
        });

    ui.add_space(4.0);

    // 开始 / 停止 / 保存 按钮
    ui.horizontal(|ui| {
        let (start_txt, start_col) = if ps.recording {
            (crate::i18n::t("Recording..."), theme::ERROR)
        } else {
            ({ crate::i18n::t("Record##panels/transfer.rs:15459") }, theme::ACCENT)
        };
        let start_btn = egui::Button::new(
            RichText::new(start_txt).color(Color32::BLACK).strong(),
        )
        .fill(start_col)
        .min_size(Vec2::new(110.0, 28.0))
        .stroke(egui::Stroke::NONE);

        if ui.add_enabled(!ps.recording, start_btn)
            .on_hover_text(crate::i18n::t("Start recording serial send/receive data"))
            .clicked()
        {
            start_recording(ps);
        }

        let stop_btn = egui::Button::new(
            RichText::new(crate::i18n::t("Stop##panels/transfer.rs:15992")).color(theme::TEXT_PRIMARY),
        )
        .fill(theme::BG_ELEVATED)
        .min_size(Vec2::new(90.0, 28.0))
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER));
        if ui.add_enabled(ps.recording, stop_btn)
            .on_hover_text(crate::i18n::t("Stop recording and save automatically"))
            .clicked()
        {
            stop_recording(ps, is_zh);
        }

        let save_btn = egui::Button::new(
            RichText::new(crate::i18n::t("Save")).color(theme::TEXT_PRIMARY),
        )
        .fill(theme::BG_ELEVATED)
        .min_size(Vec2::new(90.0, 28.0))
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER));
        if ui.add_enabled(!ps.recording && !ps.rec_save_path.is_empty(), save_btn)
            .on_hover_text(crate::i18n::t("Save recorded data to a JSON file"))
            .clicked()
        {
            save_script(ps, is_zh);
        }
    });

    ui.add_space(4.0);

    // 录制时持续采集 state.rx_buffer 中的 Tx 条目
    if ps.recording {
        poll_tx_for_recording(ps, state, is_zh);
    }

    // 录制状态信息
    let cmd_count = RECORDER.with(|r| {
        r.borrow()
            .as_ref()
            .map(|rec| rec.script().commands.len())
            .unwrap_or(0)
    });
    ui.label(
        RichText::new(format!(
            "{} {} {}",
            crate::i18n::t("Recorded"),
            cmd_count,
            crate::i18n::t("commands")
        ))
        .color(if ps.recording { theme::ACCENT } else { theme::TEXT_SECONDARY })
        .small(),
    );
}

/// 开始录制：创建新录制器并重置时间戳基线
fn start_recording(ps: &mut TransferPanelState) {
    let name = if ps.rec_name.trim().is_empty() {
        "script".to_string()
    } else {
        ps.rec_name.clone()
    };
    let mut rec = ScriptRecorder::new(&name, &ps.rec_desc);
    rec.start();
    // 以当前时刻为基线，只记录此后产生的 Tx
    ps.last_recorded_ts_ms = chrono::Local::now().timestamp_millis();
    RECORDER.with(|r| *r.borrow_mut() = Some(rec));
    ps.recording = true;
}

/// 停止录制
fn stop_recording(ps: &mut TransferPanelState, _is_zh: bool) {
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.stop();
        }
    });
    ps.recording = false;
    ps.push_log(crate::i18n::t("Recording stopped"));
}

/// 保存脚本为 JSON
fn save_script(ps: &mut TransferPanelState, _is_zh: bool) {
    let path = PathBuf::from(&ps.rec_save_path);
    let result = RECORDER.with(|r| {
        r.borrow()
            .as_ref()
            .ok_or_else(|| "no recorder".to_string())
            .and_then(|rec| rec.save(&path).map_err(|e| e.to_string()))
    });
    match result {
        Ok(()) => {
            ps.push_log(format!("{}: {}", crate::i18n::t("Saved"), path.display()));
        }
        Err(e) => {
            ps.push_log(format!("{}: {}", crate::i18n::t("Save failed"), e));
        }
    }
}

/// 扫描 state.rx_buffer 中新增的 Tx 条目，写入录制器
fn poll_tx_for_recording(ps: &mut TransferPanelState, state: &AppState, _is_zh: bool) {
    let mut new_items: Vec<(i64, String)> = Vec::new();
    let mut max_ts = ps.last_recorded_ts_ms;
    for entry in &state.rx_buffer {
        if entry.direction == Direction::Tx {
            let ts = entry.timestamp.timestamp_millis();
            if ts > ps.last_recorded_ts_ms {
                new_items.push((ts, state::format_hex(&entry.data)));
                if ts > max_ts {
                    max_ts = ts;
                }
            }
        }
    }
    if new_items.is_empty() {
        return;
    }
    ps.last_recorded_ts_ms = max_ts;
    let n = new_items.len();
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            for (_, hex) in &new_items {
                rec.record_send(hex);
            }
        }
    });
    ps.push_log(format!("{} {} Tx", crate::i18n::t("Recorded"), n));
}

// ---------------------------------------------------------------------------
// 脚本回放
// ---------------------------------------------------------------------------

fn render_replayer(
    ui: &mut egui::Ui,
    ps: &mut TransferPanelState,
    state: &mut AppState,
    is_zh: bool,
) {
    egui::Grid::new("rep_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Script Path"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut ps.script_path)
                        .desired_width(260.0)
                        .hint_text("/path/to/script.json"),
                )
                .on_hover_text(crate::i18n::t("Path to the recorded JSON script file"));
                let load_btn = egui::Button::new(
                    RichText::new(crate::i18n::t("Load")).color(theme::TEXT_PRIMARY),
                )
                .fill(theme::BG_ELEVATED)
                .stroke(egui::Stroke::new(1.0_f32, theme::BORDER));
                if ui.add(load_btn)
                    .on_hover_text(crate::i18n::t("Load the script and show a summary"))
                    .clicked()
                {
                    load_script(ps, is_zh);
                }
            });
            ui.end_row();
        });

    ui.add_space(4.0);

    // 开始 / 停止按钮
    ui.horizontal(|ui| {
        let play_btn = egui::Button::new(
            RichText::new(crate::i18n::t("Play"))
                .color(Color32::BLACK)
                .strong(),
        )
        .fill(theme::ACCENT)
        .min_size(Vec2::new(110.0, 28.0))
        .stroke(egui::Stroke::NONE);
        let stop_btn = egui::Button::new(
            RichText::new(crate::i18n::t("Stop")).color(theme::TEXT_PRIMARY),
        )
        .fill(theme::BG_ELEVATED)
        .min_size(Vec2::new(90.0, 28.0))
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER));

        let has_script = REPLAYER.with(|r| r.borrow().is_some());
        if ui.add_enabled(has_script && !ps.replaying, play_btn)
            .on_hover_text(crate::i18n::t("Replay sent data using the recorded timing"))
            .clicked()
        {
            start_replay(ps, is_zh);
        }
        if ui.add_enabled(ps.replaying, stop_btn)
            .on_hover_text(crate::i18n::t("Stop replay"))
            .clicked()
        {
            stop_replay(ps, is_zh);
        }
    });

    ui.add_space(4.0);

    // 回放进度条
    ui.add_sized(
        [ui.available_width().max(200.0), 16.0],
        egui::ProgressBar::new(ps.replay_progress)
            .text(format!("{:.0}%", ps.replay_progress * 100.0)),
    );

    ui.add_space(2.0);
    ui.label(
        RichText::new(if ps.replay_current.is_empty() {
            "—"
        } else {
            &ps.replay_current
        })
        .color(theme::CYAN)
        .small()
        .family(egui::FontFamily::Monospace),
    );

    ui.add_space(4.0);

    // 每帧推进回放
    if ps.replaying {
        step_replay(ps, state, is_zh);
    }

    // 回放日志
    render_log_area(ui, "rep_log", &ps.replay_log, 100.0);
}

/// 加载脚本文件
fn load_script(ps: &mut TransferPanelState, _is_zh: bool) {
    if ps.script_path.trim().is_empty() {
        ps.push_replay_log(crate::i18n::t("Empty path"));
        return;
    }
    let path = PathBuf::from(&ps.script_path);
    match ScriptReplayer::load(&path) {
        Ok(rep) => {
            let total = rep.script().commands.len();
            REPLAYER.with(|r| *r.borrow_mut() = Some(rep));
            ps.replay_progress = 0.0;
            ps.replay_current.clear();
            ps.push_replay_log(format!(
                "{} ({} {})",
                crate::i18n::t("Loaded"),
                total,
                crate::i18n::t("cmds")
            ));
        }
        Err(e) => {
            ps.push_replay_log(format!("{}: {}", crate::i18n::t("Load failed"), e));
        }
    }
}

/// 开始回放
fn start_replay(ps: &mut TransferPanelState, _is_zh: bool) {
    let started = REPLAYER.with(|r| {
        if let Some(rep) = r.borrow_mut().as_mut() {
            rep.start();
            true
        } else {
            false
        }
    });
    if started {
        ps.replaying = true;
        ps.replay_progress = 0.0;
        // 立即可执行第一条
        ps.replay_next_time_ms = 0;
        ps.push_replay_log(crate::i18n::t("Replay started"));
    } else {
        ps.push_replay_log(crate::i18n::t("No script loaded"));
    }
}

/// 停止回放
fn stop_replay(ps: &mut TransferPanelState, _is_zh: bool) {
    REPLAYER.with(|r| {
        if let Some(rep) = r.borrow_mut().as_mut() {
            rep.stop();
        }
    });
    ps.replaying = false;
    ps.push_replay_log(crate::i18n::t("Replay stopped"));
}

/// 每帧推进一条命令（按 delay_ms 等待，非阻塞）
fn step_replay(ps: &mut TransferPanelState, state: &mut AppState, _is_zh: bool) {
    let now_ms = chrono::Local::now().timestamp_millis();
    if now_ms < ps.replay_next_time_ms {
        return;
    }

    // 取下一条命令并克隆所需字段（不可跨 thread_local 边界借用）
    let next: Option<(u64, Action, Option<String>)> = REPLAYER.with(|r| {
        if let Some(rep) = r.borrow_mut().as_mut() {
            rep.next_command()
                .map(|c| (c.delay_ms, c.action.clone(), c.data.clone()))
        } else {
            None
        }
    });

    match next {
        None => {
            ps.replaying = false;
            ps.replay_progress = 1.0;
            ps.push_replay_log(crate::i18n::t("Replay done"));
        }
        Some((delay_ms, action, data)) => {
            // 显示当前命令
            ps.replay_current = format!("{:?} {}", action, data.as_deref().unwrap_or(""));
            match action {
                Action::Send => {
                    if let Some(hex) = data {
                        match state::parse_hex(&hex) {
                            Ok(bytes) => {
                                if !bytes.is_empty() {
                                    match state.send_raw(&bytes) {
                                        Ok(n) => ps.push_replay_log(format!(
                                            "{} {}B",
                                            crate::i18n::t("Send"),
                                            n
                                        )),
                                        Err(e) => ps.push_replay_log(format!(
                                            "{}: {}",
                                            crate::i18n::t("Send err"),
                                            e
                                        )),
                                    }
                                }
                            }
                            Err(e) => ps.push_replay_log(format!(
                                "{}: {}",
                                crate::i18n::t("Hex err"),
                                e
                            )),
                        }
                    }
                }
                Action::Wait => {
                    ps.push_replay_log(format!("wait {}ms", delay_ms));
                }
                Action::Read => {
                    ps.push_replay_log(crate::i18n::t("read (skip)"));
                }
                Action::Comment => {
                    if let Some(c) = data {
                        ps.push_replay_log(format!("# {}", c));
                    }
                }
            }
            // 更新进度
            ps.replay_progress = REPLAYER
                .with(|r| r.borrow().as_ref().map(|rep| rep.progress()).unwrap_or(0.0));
            // 下一条按 delay_ms 等待（delay_ms 为 0 则下一帧立即执行）
            ps.replay_next_time_ms = now_ms + delay_ms as i64;
        }
    }
}

// ---------------------------------------------------------------------------
// 通用日志渲染
// ---------------------------------------------------------------------------

fn render_log_area(ui: &mut egui::Ui, id: &str, log: &[String], height: f32) {
    egui::Frame::group(ui.style())
        .fill(theme::BG_DEEP)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt(id)
                .max_height(height)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if log.is_empty() {
                        ui.label(
                            RichText::new("—")
                                .color(theme::TEXT_DISABLED)
                                .family(egui::FontFamily::Monospace),
                        );
                    } else {
                        for line in log {
                            ui.label(
                                RichText::new(line)
                                    .color(theme::TEXT_SECONDARY)
                                    .family(egui::FontFamily::Monospace)
                                    .small(),
                            );
                        }
                    }
                });
        });
}
