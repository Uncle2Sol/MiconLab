// Modbus TCP/RTU 桥接 + Modbus 仿真器面板
// 两个 Tab 切换：桥接模式、仿真器模式
//
// 设计说明：
// - 面板状态（Clone+Send+Sync 的字段，含 Option<Arc<AtomicBool>>）通过 ctx.data_mut() 持久化。
// - mpsc::Receiver 不能 Clone 也不满足 Send+Sync，故用 thread_local RefCell 暂存
//   （与 transfer.rs 中 ScriptRecorder/ScriptReplayer 的处理方式一致）。
// - 每帧从 Receiver 非阻塞 try_recv() 抽取日志追加到 ps.log。
// - 启动时调用 start_bridge/start_simulator，存 stop_flag 到 ps，存 Receiver 到 thread_local。
// - 停止时设置 stop_flag=true 并清空 Option 与 thread_local。

use crate::i18n::{Language, L10n};
use crate::state::AppState;
use crate::theme;
use egui::{Color32, RichText, ScrollArea, Vec2};
use miconlab_core::protocol::{
    start_bridge, BridgeConfig, BridgeLogEntry, start_simulator, SimulatorConfig,
    SimulatorLogEntry, SimulatorMode,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

// 持久化面板状态（必须 Clone + Send + Sync，才能存入 egui::IdTypeMap）
#[derive(Clone)]
struct BridgePanelState {
    active_tab: usize,
    // ---- 桥接配置 ----
    bridge_tcp_port: u16,
    bridge_serial_port: String,
    bridge_baud_rate: u32,
    bridge_timeout_ms: u64,
    bridge_running: bool,
    bridge_stop_flag: Option<Arc<AtomicBool>>,
    bridge_log: Vec<String>,
    // ---- 仿真器配置 ----
    sim_mode: SimulatorMode,
    sim_tcp_port: u16,
    sim_serial_port: String,
    sim_baud_rate: u32,
    sim_slave_id: u8,
    sim_running: bool,
    sim_stop_flag: Option<Arc<AtomicBool>>,
    sim_log: Vec<String>,
}

impl Default for BridgePanelState {
    fn default() -> Self {
        Self {
            active_tab: 0,
            bridge_tcp_port: 502,
            bridge_serial_port: String::new(),
            bridge_baud_rate: 9600,
            bridge_timeout_ms: 500,
            bridge_running: false,
            bridge_stop_flag: None,
            bridge_log: Vec::new(),
            sim_mode: SimulatorMode::TcpServer,
            sim_tcp_port: 502,
            sim_serial_port: String::new(),
            sim_baud_rate: 9600,
            sim_slave_id: 1,
            sim_running: false,
            sim_stop_flag: None,
            sim_log: Vec::new(),
        }
    }
}

// 非持久化对象（mpsc::Receiver 非 Clone+Send+Sync）：用 thread_local 暂存
thread_local! {
    static BR_LOG_RX: RefCell<Option<mpsc::Receiver<BridgeLogEntry>>> = RefCell::new(None);
    static BR_ERR_RX: RefCell<Option<mpsc::Receiver<String>>> = RefCell::new(None);
    static SIM_LOG_RX: RefCell<Option<mpsc::Receiver<SimulatorLogEntry>>> = RefCell::new(None);
    static SIM_ERR_RX: RefCell<Option<mpsc::Receiver<String>>> = RefCell::new(None);
}

/// 渲染桥接与仿真器面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let l10n = L10n::new(state.language);
    let is_zh = state.language == Language::Zh;

    // 注意：egui 0.29 的 CentralPanel::show 持有 Context 内部 RwLock 的写锁，
    // 闭包内再调用 ctx.data_mut() 会重入写锁导致永久死锁。
    // 因此在 CentralPanel 之前取出持久化状态的克隆，闭包内使用克隆，
    // 闭包结束后再写回。

    // CentralPanel 之前：克隆出持久化状态
    let mut ps = ctx
        .data_mut(|data| data.get_temp_mut_or_default::<BridgePanelState>(egui::Id::NULL).clone());

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
        *data.get_temp_mut_or_default::<BridgePanelState>(egui::Id::NULL) = ps;
    });
}

/// 渲染整个面板：标题 + Tab 切换 + 子面板 + 错误条
fn render_panel(
    ui: &mut egui::Ui,
    ps: &mut BridgePanelState,
    state: &mut AppState,
    is_zh: bool,
    l10n: &L10n,
) {
    ui.heading(
        RichText::new(l10n.t("panel.bridge"))
            .color(theme::TEXT_PRIMARY)
            .strong(),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(crate::i18n::t("Bridge data between two serial ports, or emulate a Modbus slave responding to master requests"))
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
    ui.add_space(8.0);

    // 每帧抽取后台日志
    poll_logs(ps);

    // Tab 切换
    ui.horizontal(|ui| {
        if ui
            .selectable_label(
                ps.active_tab == 0,
                RichText::new(crate::i18n::t("Bridge")).color(
                    if ps.active_tab == 0 {
                        theme::ACCENT
                    } else {
                        theme::TEXT_SECONDARY
                    },
                ),
            )
            .clicked()
        {
            ps.active_tab = 0;
        }
        if ui
            .selectable_label(
                ps.active_tab == 1,
                RichText::new(crate::i18n::t("Simulator")).color(
                    if ps.active_tab == 1 {
                        theme::ACCENT
                    } else {
                        theme::TEXT_SECONDARY
                    },
                ),
            )
            .clicked()
        {
            ps.active_tab = 1;
        }
    });
    ui.separator();
    ui.add_space(6.0);

    if ps.active_tab == 0 {
        render_bridge(ui, ps, state, is_zh);
    } else {
        render_simulator(ui, ps, state, is_zh);
    }

    // 底部错误条
    if let Some(err) = &state.last_error {
        ui.add_space(6.0);
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
// 桥接 Tab
// ---------------------------------------------------------------------------

fn render_bridge(
    ui: &mut egui::Ui,
    ps: &mut BridgePanelState,
    state: &mut AppState,
    is_zh: bool,
) {
    // 预取串口名列表，避免在 Grid 闭包内同时借用 state 与 ps
    let port_names: Vec<String> = state.port_list.iter().map(|p| p.name.clone()).collect();
    let running = ps.bridge_running;

    egui::Grid::new("br_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("TCP Port"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::DragValue::new(&mut ps.bridge_tcp_port)
                    .range(1..=65535u16)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("TCP listen port (0 = pure serial bridge, no TCP)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Serial Port"))
                    .color(theme::TEXT_SECONDARY),
            );
            let selected = if ps.bridge_serial_port.is_empty() {
                crate::i18n::t("Select port").to_string()
            } else {
                ps.bridge_serial_port.clone()
            };
            egui::ComboBox::from_id_salt("br_serial_port")
                .selected_text(selected)
                .width(200.0)
                .show_ui(ui, |ui| {
                    for name in &port_names {
                        ui.selectable_value(&mut ps.bridge_serial_port, name.clone(), name);
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("Select the peer serial port to bridge with the connected source port"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Baud Rate"))
                    .color(theme::TEXT_SECONDARY),
            );
            egui::ComboBox::from_id_salt("br_baud")
                .selected_text(format!("{}", ps.bridge_baud_rate))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for &b in &[9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                        ui.selectable_value(&mut ps.bridge_baud_rate, b, format!("{}", b));
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("Peer serial baud rate (may differ from the source port)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Timeout (ms)"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::DragValue::new(&mut ps.bridge_timeout_ms)
                    .range(50..=10000u64)
                    .speed(1.0),
            )
            .on_hover_text(crate::i18n::t("Idle timeout before disconnect (seconds, 0 = never)"));
            ui.end_row();
        });

    ui.add_space(6.0);

    // 启动 / 停止按钮
    ui.horizontal(|ui| {
        if !running {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("Start Bridge"))
                    .color(Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ACCENT)
            .min_size(Vec2::new(120.0, 28.0))
            .stroke(egui::Stroke::NONE);
            if ui
                .add_enabled(!ps.bridge_serial_port.is_empty(), btn)
                .on_hover_text(crate::i18n::t("Start TCP→RTU bridging: listen on the TCP port, convert Modbus TCP requests to RTU frames on serial, and relay RTU responses back to TCP. Runs in a background thread using the actor model to avoid serial blocking"))
                .clicked()
            {
                start_bridge_task(ps, state, is_zh);
            }
        } else {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("Stop Bridge"))
                    .color(Color32::WHITE)
                    .strong(),
            )
            .fill(theme::ERROR)
            .min_size(Vec2::new(120.0, 28.0))
            .stroke(egui::Stroke::NONE);
            if ui.add(btn)
                .on_hover_text(crate::i18n::t("Stop bridging and close both serial ports and the TCP listener"))
                .clicked()
            {
                stop_bridge_task(ps);
            }
        }

        ui.separator();
        if ui
            .button(
                RichText::new(crate::i18n::t("Clear Log"))
                    .color(theme::TEXT_SECONDARY),
            )
            .clicked()
        {
            ps.bridge_log.clear();
        }

        // 运行状态指示
        if running {
            ui.label(
                RichText::new(crate::i18n::t("● Running"))
                    .color(theme::SUCCESS)
                    .small(),
            );
        } else {
            ui.label(
                RichText::new(crate::i18n::t("○ Stopped"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
        }
    });

    ui.add_space(6.0);

    // 日志区
    render_log_area(ui, "br_log", &ps.bridge_log, 240.0);
}

/// 启动桥接任务：调用 start_bridge，存 stop_flag 到 ps，存 Receiver 到 thread_local
///
/// 修复点：启动前检查所选串口是否与 GUI 主连接冲突。
/// Bridge 会在后台线程独立打开串口，若与 GUI 已连接的端口相同，
/// 会导致串口被重复打开（多数系统不允许），因此提前拦截并提示。
fn start_bridge_task(ps: &mut BridgePanelState, state: &mut AppState, _is_zh: bool) {
    // 串口冲突检测：若 GUI 主连接已占用该端口，提示用户
    if state.is_connected() && state.config.port_name == ps.bridge_serial_port {
        let msg = { crate::i18n::tf("Serial port conflict: '{}' is held by the GUI main connection. Disconnect it first or pick another port", &[("{}", "{}", &format!("{}", ps.bridge_serial_port))]) };
        state.last_error = Some(msg.clone());
        ps.bridge_log.push(format!("[ERR] {}", msg));
        return;
    }

    let cfg = BridgeConfig {
        tcp_port: ps.bridge_tcp_port,
        serial_port_name: ps.bridge_serial_port.clone(),
        baud_rate: ps.bridge_baud_rate,
        timeout_ms: ps.bridge_timeout_ms,
    };
    match start_bridge(cfg) {
        Ok((stop, log_rx, err_rx)) => {
            ps.bridge_stop_flag = Some(stop);
            ps.bridge_running = true;
            BR_LOG_RX.with(|c| *c.borrow_mut() = Some(log_rx));
            BR_ERR_RX.with(|c| *c.borrow_mut() = Some(err_rx));
            ps.bridge_log.push(format!(
                "[{}] {}",
                format_now_ts(),
                crate::i18n::t("Bridge started")
            ));
            state.last_error = None;
        }
        Err(e) => {
            state.last_error = Some(e.clone());
            ps.bridge_log.push(format!("[ERR] {}", e));
        }
    }
}

/// 停止桥接任务：设置 stop_flag=true，清空 thread_local Receiver
fn stop_bridge_task(ps: &mut BridgePanelState) {
    if let Some(flag) = ps.bridge_stop_flag.take() {
        flag.store(true, Ordering::SeqCst);
    }
    ps.bridge_running = false;
    BR_LOG_RX.with(|c| *c.borrow_mut() = None);
    BR_ERR_RX.with(|c| *c.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// 仿真器 Tab
// ---------------------------------------------------------------------------

fn render_simulator(
    ui: &mut egui::Ui,
    ps: &mut BridgePanelState,
    state: &mut AppState,
    is_zh: bool,
) {
    let port_names: Vec<String> = state.port_list.iter().map(|p| p.name.clone()).collect();
    let running = ps.sim_running;
    let is_tcp = ps.sim_mode == SimulatorMode::TcpServer;

    egui::Grid::new("sim_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(
                RichText::new(crate::i18n::t("Mode")).color(theme::TEXT_SECONDARY),
            );
            let cur = match ps.sim_mode {
                SimulatorMode::TcpServer => {
                    crate::i18n::t("TCP Server")
                }
                SimulatorMode::RtuSlave => {
                    crate::i18n::t("RTU Slave")
                }
            };
            egui::ComboBox::from_id_salt("sim_mode")
                .selected_text(cur)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut ps.sim_mode,
                        SimulatorMode::TcpServer,
                        crate::i18n::t("TCP Server"),
                    );
                    ui.selectable_value(
                        &mut ps.sim_mode,
                        SimulatorMode::RtuSlave,
                        crate::i18n::t("RTU Slave"),
                    );
                })
                .response
                .on_hover_text(crate::i18n::t("TCP Server: listen on a TCP port as a Modbus slave for TCP master testing; RTU Slave: respond to RTU master requests over serial, requires a serial port"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("TCP Port"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_enabled(
                is_tcp,
                egui::DragValue::new(&mut ps.sim_tcp_port).range(1..=65535u16).speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Modbus TCP listen port (standard 502)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Serial Port"))
                    .color(theme::TEXT_SECONDARY),
            );
            let selected = if ps.sim_serial_port.is_empty() {
                crate::i18n::t("Select port").to_string()
            } else {
                ps.sim_serial_port.clone()
            };
            egui::ComboBox::from_id_salt("sim_serial_port")
                .selected_text(selected)
                .width(200.0)
                .show_ui(ui, |ui| {
                    for name in &port_names {
                        ui.selectable_value(&mut ps.sim_serial_port, name.clone(), name);
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("Modbus RTU serial port (usable together with TCP)"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Baud Rate"))
                    .color(theme::TEXT_SECONDARY),
            );
            egui::ComboBox::from_id_salt("sim_baud")
                .selected_text(format!("{}", ps.sim_baud_rate))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for &b in &[9600u32, 19200, 38400, 57600, 115200, 230400, 460800, 921600] {
                        ui.selectable_value(&mut ps.sim_baud_rate, b, format!("{}", b));
                    }
                })
                .response
                .on_hover_text(crate::i18n::t("RTU serial baud rate"));
            ui.end_row();

            ui.label(
                RichText::new(crate::i18n::t("Slave ID"))
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add(
                egui::DragValue::new(&mut ps.sim_slave_id)
                    .range(1..=247u8)
                    .speed(0.1),
            )
            .on_hover_text(crate::i18n::t("Modbus slave address (1-247). The simulator only responds to master requests carrying this address; other addresses are ignored"));
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::t("Registers/coils use defaults (holding 0-9=0, coils 0-15=OFF)."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    ui.add_space(6.0);

    // 启动 / 停止按钮
    ui.horizontal(|ui| {
        let can_start = is_tcp || !ps.sim_serial_port.is_empty();
        if !running {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("Start Simulator"))
                    .color(Color32::BLACK)
                    .strong(),
            )
            .fill(theme::ACCENT)
            .min_size(Vec2::new(130.0, 28.0))
            .stroke(egui::Stroke::NONE);
            if ui
                .add_enabled(can_start, btn)
                .on_hover_text(crate::i18n::t("Start the Modbus slave simulator, responding to read-coil/read-holding/write-single/write-multiple requests. Registers initialize to 0 for functional testing"))
                .clicked() {
                start_simulator_task(ps, state, is_zh);
            }
        } else {
            let btn = egui::Button::new(
                RichText::new(crate::i18n::t("Stop Simulator"))
                    .color(Color32::WHITE)
                    .strong(),
            )
            .fill(theme::ERROR)
            .min_size(Vec2::new(130.0, 28.0))
            .stroke(egui::Stroke::NONE);
            if ui.add(btn)
                .on_hover_text(crate::i18n::t("Stop the simulator and release the TCP port and serial port"))
                .clicked()
            {
                stop_simulator_task(ps);
            }
        }

        ui.separator();
        if ui
            .button(
                RichText::new(crate::i18n::t("Clear Log"))
                    .color(theme::TEXT_SECONDARY),
            )
            .clicked()
        {
            ps.sim_log.clear();
        }

        if running {
            ui.label(
                RichText::new(crate::i18n::t("● Running"))
                    .color(theme::SUCCESS)
                    .small(),
            );
        } else {
            ui.label(
                RichText::new(crate::i18n::t("○ Stopped"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
        }
    });

    ui.add_space(6.0);

    // 日志区
    render_log_area(ui, "sim_log", &ps.sim_log, 240.0);
}

/// 启动仿真器任务：用 default() 保留默认寄存器/线圈，覆盖用户配置字段
///
/// 修复点：RTU 从站模式下，启动前检查所选串口是否与 GUI 主连接冲突。
/// TCP 服务器模式不使用串口，无需检测。
fn start_simulator_task(ps: &mut BridgePanelState, state: &mut AppState, _is_zh: bool) {
    // 仅 RTU 从站模式需要串口，检查与 GUI 主连接的冲突
    if ps.sim_mode == SimulatorMode::RtuSlave
        && state.is_connected()
        && state.config.port_name == ps.sim_serial_port
    {
        let msg = { crate::i18n::tf("Serial port conflict: '{}' is held by the GUI main connection. Disconnect it first or pick another port", &[("{}", "{}", &format!("{}", ps.sim_serial_port))]) };
        state.last_error = Some(msg.clone());
        ps.sim_log.push(format!("[ERR] {}", msg));
        return;
    }

    let cfg = SimulatorConfig {
        mode: ps.sim_mode,
        tcp_port: ps.sim_tcp_port,
        serial_port_name: ps.sim_serial_port.clone(),
        baud_rate: ps.sim_baud_rate,
        slave_id: ps.sim_slave_id,
        ..SimulatorConfig::default()
    };
    match start_simulator(cfg) {
        Ok((stop, log_rx, err_rx)) => {
            ps.sim_stop_flag = Some(stop);
            ps.sim_running = true;
            SIM_LOG_RX.with(|c| *c.borrow_mut() = Some(log_rx));
            SIM_ERR_RX.with(|c| *c.borrow_mut() = Some(err_rx));
            ps.sim_log.push(format!(
                "[{}] {}",
                format_now_ts(),
                crate::i18n::t("Simulator started")
            ));
            state.last_error = None;
        }
        Err(e) => {
            state.last_error = Some(e.clone());
            ps.sim_log.push(format!("[ERR] {}", e));
        }
    }
}

/// 停止仿真器任务：设置 stop_flag=true，清空 thread_local Receiver
fn stop_simulator_task(ps: &mut BridgePanelState) {
    if let Some(flag) = ps.sim_stop_flag.take() {
        flag.store(true, Ordering::SeqCst);
    }
    ps.sim_running = false;
    SIM_LOG_RX.with(|c| *c.borrow_mut() = None);
    SIM_ERR_RX.with(|c| *c.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// 日志工具
// ---------------------------------------------------------------------------

/// 每帧从后台 thread_local Receiver 抽取日志，追加到 ps
fn poll_logs(ps: &mut BridgePanelState) {
    // ---- 桥接日志 ----
    let mut br_entries: Vec<String> = Vec::new();
    BR_LOG_RX.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(rx) = opt.as_mut() {
            while let Ok(entry) = rx.try_recv() {
                br_entries.push(format_bridge_log(&entry));
            }
        }
    });
    BR_ERR_RX.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(rx) = opt.as_mut() {
            while let Ok(msg) = rx.try_recv() {
                br_entries.push(format!("[{}] {}", format_now_ts(), msg));
            }
        }
    });
    for line in br_entries {
        ps.bridge_log.push(line);
    }
    while ps.bridge_log.len() > 300 {
        ps.bridge_log.remove(0);
    }

    // ---- 仿真器日志 ----
    let mut sim_entries: Vec<String> = Vec::new();
    SIM_LOG_RX.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(rx) = opt.as_mut() {
            while let Ok(entry) = rx.try_recv() {
                sim_entries.push(format_sim_log(&entry));
            }
        }
    });
    SIM_ERR_RX.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(rx) = opt.as_mut() {
            while let Ok(msg) = rx.try_recv() {
                sim_entries.push(format!("[{}] {}", format_now_ts(), msg));
            }
        }
    });
    for line in sim_entries {
        ps.sim_log.push(line);
    }
    while ps.sim_log.len() > 300 {
        ps.sim_log.remove(0);
    }
}

/// 格式化桥接日志条目
fn format_bridge_log(entry: &BridgeLogEntry) -> String {
    let ts = format_ts_ms(entry.timestamp);
    let status = if entry.success { "OK" } else { "FAIL" };
    if entry.response_hex.is_empty() {
        format!(
            "[{}] {} {} REQ={} [{}]",
            ts, entry.client_addr, entry.direction, entry.request_hex, status
        )
    } else {
        format!(
            "[{}] {} {} REQ={} RESP={} [{}]",
            ts,
            entry.client_addr,
            entry.direction,
            entry.request_hex,
            entry.response_hex,
            status
        )
    }
}

/// 格式化仿真器日志条目
fn format_sim_log(entry: &SimulatorLogEntry) -> String {
    let ts = format_ts_ms(entry.timestamp);
    let status = if entry.success { "OK" } else { "FAIL" };
    format!(
        "[{}] {} {} | {} [{}]",
        ts, entry.direction, entry.hex, entry.decoded, status
    )
}

/// 将毫秒时间戳格式化为 HH:MM:SS.mmm（本地时区）
fn format_ts_ms(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    if let Some(dt) = Local.timestamp_millis_opt(ms).single() {
        dt.format("%H:%M:%S%.3f").to_string()
    } else {
        "--:--:--.---".to_string()
    }
}

/// 当前本地时间格式化为 HH:MM:SS.mmm
fn format_now_ts() -> String {
    use chrono::Local;
    Local::now().format("%H:%M:%S%.3f").to_string()
}

/// 通用日志显示区：Frame + ScrollArea + monospace 小字体
fn render_log_area(ui: &mut egui::Ui, id: &str, log: &[String], height: f32) {
    egui::Frame::group(ui.style())
        .fill(theme::BG_DEEP)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_min_height(height);
            if log.is_empty() {
                ui.label(
                    RichText::new("...")
                        .color(theme::TEXT_DISABLED)
                        .family(egui::FontFamily::Monospace)
                        .small(),
                );
            } else {
                ScrollArea::vertical()
                    .max_height(height)
                    .id_salt(id)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in log {
                            ui.label(
                                RichText::new(line)
                                    .color(theme::TEXT_PRIMARY)
                                    .family(egui::FontFamily::Monospace)
                                    .small(),
                            );
                        }
                    });
            }
        });
}
