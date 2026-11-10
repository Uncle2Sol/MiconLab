// 应用状态管理
// 持有串口、配置、终端数据，提供连接/读取/发送等操作
//
// 设计要点：SerialPort 不是 Send，因此不创建后台线程。
// 改为在每帧 update() 中用非阻塞方式读取串口（短超时），追加到 rx_buffer。

#[cfg(feature = "device-workspaces")]
use crate::devices::DeviceId;
use crate::i18n::Language;
use crate::panels::PanelKind;
use chrono::Local;
use miconlab_core::config::{
    DataBits, FlowControl, Parity, SerialConfig, StopBits,
};
use miconlab_core::crosstalk::{CrosstalkDetector, CrosstalkReport};
use miconlab_core::port::{PortError, SerialPort, SerialPortInfo};
use miconlab_core::web_bridge::{WebBridge, WebCommand, DEFAULT_WEB_BRIDGE_PORT};

/// 行尾类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    None,
    CR,
    LF,
    CRLF,
}

impl LineEnding {
    /// 返回当前行尾对应的字节
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            LineEnding::None => &[],
            LineEnding::CR => b"\r",
            LineEnding::LF => b"\n",
            LineEnding::CRLF => b"\r\n",
        }
    }

    /// 显示用标签
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::None => "None",
            LineEnding::CR => "CR (\\r)",
            LineEnding::LF => "LF (\\n)",
            LineEnding::CRLF => "CRLF (\\r\\n)",
        }
    }
}

/// 单条接收/发送数据条目（带时间戳和方向）
#[derive(Debug, Clone)]
pub struct RxEntry {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub data: Vec<u8>,
    pub direction: Direction,
}

/// 数据方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 接收
    Rx,
    /// 发送
    Tx,
}

/// 应用全局状态
pub struct AppState {
    // 串口
    pub port: SerialPort,
    pub port_list: Vec<SerialPortInfo>,
    pub selected_port_index: usize,

    // 配置（用于 UI 编辑，连接时拷贝到 port）
    pub config: SerialConfig,

    // 终端数据
    pub rx_buffer: Vec<RxEntry>,
    /// 全局单调递增的条目序号（每 push 一条 +1，drain/clear 不复位），
    /// 供 AI 助手等面板做增量跟踪，避免 rx_buffer 头部丢弃导致索引漂移
    pub rx_entry_seq: u64,
    pub tx_input: String,
    pub bytes_tx: u64,
    pub bytes_rx: u64,

    // 终端选项
    pub hex_mode: bool,
    pub show_timestamp: bool,
    pub auto_scroll: bool,
    pub line_ending: LineEnding,
    /// 暂停接收（仍读取串口清空缓冲区，但不追加到 rx_buffer）
    pub paused: bool,

    // UI 状态
    pub language: Language,
    pub active_panel: PanelKind,
    /// 当前设备工作区（None = 未选择设备，停留在设备总入口/通用工具）
    #[cfg(feature = "device-workspaces")]
    pub active_device: Option<DeviceId>,

    // 错误/通知
    pub last_error: Option<String>,
    pub status_message: String,

    // 终端输入框是否获得过焦点（用于发送后重新聚焦）
    pub terminal_input_focused: bool,

    // DTR/RTS 信号状态（用于终端面板的硬件流控按钮）
    pub dtr_enabled: bool,
    pub rts_enabled: bool,

    // 首次运行标志（true 时显示 Welcome 屏幕，关闭后置 false）
    pub first_run: bool,
    /// 是否显示 About 窗口（品牌名 + 统一副标题 + 版本）
    pub show_about: bool,

    /// 连接成功时刻（用于连接稳定期判断，避免设备复位期间的错误被误判为致命错误）
    /// Arduino/ESP32 等设备在 DTR 变化时会触发复位，复位期间读取可能返回 Broken pipe
    pub connected_at: Option<std::time::Instant>,

    /// 连接会话 ID（每次成功 connect() 递增，用于子面板检测连接变化并清除残留状态）
    /// protocol_analyzer 等面板通过比较此 ID 判断是否为新连接会话
    pub connection_session_id: u64,

    /// 端口刷新提示消息（刷新后显示"发现 N 个新设备"或"无新设备"）
    pub port_refresh_msg: Option<String>,
    /// 端口刷新提示消息的显示截止时刻（超过后不再显示）
    pub port_refresh_msg_until: Option<std::time::Instant>,
    /// 延迟重试刷新的时刻（USB 设备插入后系统注册有延迟，首次未发现新设备时 500ms 后自动重试）
    pub pending_refresh_at: Option<std::time::Instant>,

    /// 烧录器读取忙标志（true 时跳过 poll_rx，避免抢占烧录器响应数据）
    pub flasher_read_busy: bool,

    /// 应用设置（磁盘持久化，启动时加载，修改后由设置页保存；feature 关闭时暂无读取方）
    #[cfg_attr(not(feature = "ollama"), allow(dead_code))]
    pub settings: crate::settings::AppSettings,

    /// 串扰检测器（CH340 TX 自发自收识别）
    pub crosstalk: CrosstalkDetector,
    /// 最近一次串扰检测报告（每 ~30 帧更新一次，避免每帧 detect 开销）
    pub crosstalk_report: Option<CrosstalkReport>,
    /// WebSocket 桥（None 表示未启动或启动失败，HTML 将无法通过 WS 连接）
    pub web_bridge: Option<WebBridge>,
    /// 帧计数器（用于 broadcast_diag 节流）
    frame_counter: u64,
    /// 软件逻辑分析仪（D.5 协议解码：TX/RX 时序关联分析）
    pub logic_analyzer: miconlab_core::protocol::logic_analyzer::SoftwareLogicAnalyzer,
}

impl AppState {
    /// 初始化默认状态
    pub fn new() -> Self {
        let config = SerialConfig::new("").with_baud_rate(115200).with_timeout(10);
        let port = SerialPort::new(config.clone());
        let port_list = SerialPort::list_ports().unwrap_or_default();

        // 语言：配置里有合法代码则用手动选择，否则跟随系统
        let settings = crate::settings::AppSettings::load();
        let language = settings
            .language
            .as_deref()
            .and_then(Language::from_code)
            .unwrap_or_else(Language::system);
        crate::i18n::set_language(language);

        Self {
            port,
            port_list,
            selected_port_index: 0,
            config,
            rx_buffer: Vec::new(),
            rx_entry_seq: 0,
            tx_input: String::new(),
            bytes_tx: 0,
            bytes_rx: 0,
            hex_mode: false,
            show_timestamp: true,
            auto_scroll: true,
            line_ending: LineEnding::CRLF,
            paused: false,
            language,
            active_panel: PanelKind::DeviceHub,
            #[cfg(feature = "device-workspaces")]
            active_device: None,
            last_error: None,
            status_message: String::new(),
            terminal_input_focused: false,
            dtr_enabled: false,
            rts_enabled: false,
            first_run: true,
            show_about: false,
            connected_at: None,
            connection_session_id: 0,
            port_refresh_msg: None,
            port_refresh_msg_until: None,
            pending_refresh_at: None,
            flasher_read_busy: false,
            settings,
            crosstalk: CrosstalkDetector::new(),
            crosstalk_report: None,
            web_bridge: None,
            frame_counter: 0,
            logic_analyzer: {
                use miconlab_core::protocol::logic_analyzer::*;
                let mut la = SoftwareLogicAnalyzer::new();
                #[cfg(feature = "device-workspaces")]
                {
                    la.add_scanner(Box::new(CushionFrameScanner));
                    la.add_scanner(Box::new(CustomHeaderScanner {
                        header: vec![0xA5, 0x0A],
                        min_len: 4,
                        max_len: 1024,
                        name: "Cushion0xA5".to_string(),
                    }));
                }
                la
            },
        }
    }

    /// 启动 WebSocket 桥（在 AppRoot::new 中调用）
    ///
    /// 失败时置 None 并记录日志，HTML 将无法通过 WS 连接，
    /// GUI 其他功能不受影响。
    pub fn start_web_bridge(&mut self) {
        match WebBridge::start(DEFAULT_WEB_BRIDGE_PORT) {
            Ok(bridge) => {
                log::info!(
                    "{}",
                    crate::i18n::tf("WebSocket bridge started on port {port}", &[("{port}", "{port}", &DEFAULT_WEB_BRIDGE_PORT.to_string())])
                );
                self.web_bridge = Some(bridge);
            }
            Err(e) => {
                log::warn!(
                    "{}",
                    crate::i18n::tf(
                        "WebSocket bridge failed to start on port {port}: {error}; HTML clients cannot connect over WS",
                        &[
                            ("{port}", "{port}", &DEFAULT_WEB_BRIDGE_PORT.to_string()),
                            ("{error}", "{error}", &format!("{}", e)),
                        ],
                    )
                );
                self.web_bridge = None;
            }
        }
    }

    /// 处理来自 HTML 客户端的 WebSocket 命令
    ///
    /// 在 app.rs update() 每帧调用，返回待处理的命令列表。
    pub fn poll_web_commands(&mut self) -> Vec<WebCommand> {
        if let Some(ref mut wb) = self.web_bridge {
            wb.poll_commands()
        } else {
            Vec::new()
        }
    }

    /// 应用一条来自 HTML 客户端的 WebSocket 命令
    ///
    /// 由 app.rs 在 poll_web_commands 后逐条调用。
    pub fn apply_web_command(&mut self, cmd: WebCommand) {
        match cmd {
            WebCommand::Connect { port, baud } => {
                // 在 port_list 中查找匹配端口
                if let Some(idx) = self.port_list.iter().position(|p| p.name == port) {
                    self.selected_port_index = idx;
                    self.config.baud_rate = baud;
                    self.connect();
                    self.broadcast_status_to_web();
                } else {
                    self.broadcast_error_to_web(crate::i18n::tf("Port not found: {port}", &[("{port}", "{port}", &port)]));
                }
            }
            WebCommand::Send { data } => {
                if !self.port.is_connected() {
                    self.broadcast_error_to_web(crate::i18n::t("Serial port not connected; cannot send data").to_string());
                    return;
                }
                match miconlab_core::parse_hex(&data) {
                    Ok(bytes) if !bytes.is_empty() => {
                        match self.port.write(&bytes) {
                            Ok(n) => {
                                self.bytes_tx += n as u64;
                                let ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_micros() as u64)
                                    .ok();
                                self.crosstalk.push_tx(&bytes, ts);
                                if let Some(ref wb) = self.web_bridge {
                                    wb.broadcast_tx(&bytes, ts.unwrap_or(0));
                                }
                            }
                            Err(e) => {
                                self.broadcast_error_to_web(crate::i18n::tf("Send failed: {error}", &[("{error}", "{error}", &format!("{}", e))]));
                            }
                        }
                    }
                    Ok(_) => {
                        self.broadcast_error_to_web(crate::i18n::t("Send data is empty").to_string());
                    }
                    Err(e) => {
                        self.broadcast_error_to_web(crate::i18n::tf("Hex parse failed: {error}", &[("{error}", "{error}", &format!("{}", e))]));
                    }
                }
            }
            WebCommand::SetBaud { baud } => {
                let was_connected = self.port.is_connected();
                self.config.baud_rate = baud;
                if was_connected {
                    self.connect(); // 用新波特率重连
                }
                self.broadcast_status_to_web();
            }
            WebCommand::Disconnect => {
                self.disconnect();
                self.broadcast_status_to_web();
            }
            WebCommand::GetStatus => {
                self.broadcast_status_to_web();
            }
        }
    }

    /// 广播当前连接状态给 HTML 客户端
    pub fn broadcast_status_to_web(&self) {
        if let Some(ref wb) = self.web_bridge {
            wb.broadcast_status(
                self.port.is_connected(),
                self.selected_port_name(),
                self.config.baud_rate,
                self.bytes_tx,
                self.bytes_rx,
            );
        }
    }

    /// 广播错误消息给 HTML 客户端
    pub fn broadcast_error_to_web(&self, message: String) {
        if let Some(ref wb) = self.web_bridge {
            wb.broadcast_error(message);
        }
    }

    /// 刷新可用串口列表
    /// 检测新增端口并自动选中第一个新端口，显示提示消息
    /// 如果首次未发现新设备，设置 500ms 后自动重试（macOS USB 设备注册有延迟）
    pub fn refresh_ports(&mut self) {
        // 记录旧端口名集合，用于检测新增端口
        let old_port_names: std::collections::HashSet<String> = self
            .port_list
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let old_count = old_port_names.len();

        match SerialPort::list_ports() {
            Ok(mut ports) => {
                // 过滤幽灵端口：检查设备节点是否真实存在
                // macOS 上 serialport::available_ports() 可能返回已拔掉设备的缓存条目
                // 注意：不能用 open() 验证，因为某些串口驱动的 open() 会阻塞等待 DCD 信号
                ports.retain(|p| std::fs::metadata(&p.name).is_ok());
                // 先提取新端口名（避免借用冲突：ports 被借用后不能直接 move）
                let new_port_names: Vec<String> = ports
                    .iter()
                    .filter(|p| !old_port_names.contains(&p.name))
                    .map(|p| p.name.clone())
                    .collect();

                let new_count = new_port_names.len();
                self.port_list = ports;

                // 如果有新端口，自动选中第一个新端口
                if new_count > 0 {
                    if let Some(first_new_name) = new_port_names.first() {
                        if let Some(idx) = self.port_list.iter().position(|p| p.name == *first_new_name) {
                            self.selected_port_index = idx;
                        }
                    }
                    let _zh = self.language == crate::i18n::Language::Zh;
                    self.port_refresh_msg = Some(crate::i18n::tf("✓ Found {} new device(s), auto-selected", &[("{}", "{}", &format!("{}", new_count))]));
                    // 发现新设备，取消延迟重试
                    self.pending_refresh_at = None;
                } else {
                    let _zh = self.language == crate::i18n::Language::Zh;
                    let total = self.port_list.len();
                    self.port_refresh_msg = Some(crate::i18n::tf("No new devices found ({} ports total)", &[("{}", "{}", &format!("{}", total))]));
                    // 如果之前有端口（非首次扫描），设置 500ms 后自动重试
                    // macOS/Linux/Windows 上 USB 设备注册可能有 1-2 秒延迟
                    if old_count > 0 || total > 0 {
                        self.pending_refresh_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(500)
                        );
                    }
                }
                self.port_refresh_msg_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));

                if self.selected_port_index >= self.port_list.len() {
                    self.selected_port_index = self.port_list.len().saturating_sub(1);
                }

                // 检查当前已连接的端口是否还在列表中
                // 如果设备已拔掉（端口从列表中移除），自动断开连接
                if self.is_connected() {
                    let connected_name = self.config.port_name.clone();
                    let still_exists = self.port_list.iter().any(|p| p.name == connected_name);
                    if !still_exists {
                        let _zh = self.language == crate::i18n::Language::Zh;
                        let _ = self.port.disconnect();
                        self.connected_at = None;
                        self.last_error = Some(crate::i18n::tf("Device {} disconnected (port removed)", &[("{}", "{}", &format!("{}", connected_name))]));
                    } else {
                        self.last_error = None;
                    }
                } else {
                    self.last_error = None;
                }
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
    }

    /// 检查并执行延迟重试刷新（在 app update 中每帧调用）
    pub fn check_pending_refresh(&mut self) {
        if let Some(at) = self.pending_refresh_at {
            if std::time::Instant::now() >= at {
                self.pending_refresh_at = None;
                // 静默刷新（不覆盖之前的提示消息，除非发现了新设备）
                let old_names: std::collections::HashSet<String> = self
                    .port_list
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();

                if let Ok(mut ports) = SerialPort::list_ports() {
                    // 过滤幽灵端口（同 refresh_ports，用 metadata 检查避免阻塞）
                    ports.retain(|p| std::fs::metadata(&p.name).is_ok());
                    // 先提取新端口名（避免借用冲突）
                    let new_port_names: Vec<String> = ports
                        .iter()
                        .filter(|p| !old_names.contains(&p.name))
                        .map(|p| p.name.clone())
                        .collect();

                    if !new_port_names.is_empty() {
                        // 延迟重试发现了新设备
                        let new_count = new_port_names.len();
                        self.port_list = ports;
                        if let Some(first_new_name) = new_port_names.first() {
                            if let Some(idx) = self.port_list.iter().position(|p| p.name == *first_new_name) {
                                self.selected_port_index = idx;
                            }
                        }
                        let _zh = self.language == crate::i18n::Language::Zh;
                        self.port_refresh_msg = Some(crate::i18n::tf("✓ Found {} new device(s), auto-selected", &[("{}", "{}", &format!("{}", new_count))]));
                        self.port_refresh_msg_until = Some(
                            std::time::Instant::now() + std::time::Duration::from_secs(5)
                        );
                    }
                    // 如果仍未发现新设备，不再继续重试（避免无限循环）
                    if self.selected_port_index >= self.port_list.len() {
                        self.selected_port_index = self.port_list.len().saturating_sub(1);
                    }
                }
            }
        }
    }

    /// 当前选中的端口名
    pub fn selected_port_name(&self) -> Option<&str> {
        self.port_list
            .get(self.selected_port_index)
            .map(|p| p.name.as_str())
    }

    /// 连接串口：用当前 config 创建新 SerialPort 并连接
    pub fn connect(&mut self) {
        // 若已连接，先断开
        if self.port.is_connected() {
            let _ = self.port.disconnect();
        }

        // 从端口列表获取选中的端口名
        let port_name = match self.selected_port_name() {
            Some(name) => name.to_string(),
            None => {
                self.last_error = Some("No port selected".to_string());
                return;
            }
        };

        // 用当前配置构建新的 SerialConfig
        let config = SerialConfig::new(port_name)
            .with_baud_rate(self.config.baud_rate)
            .with_data_bits(self.config.data_bits.clone())
            .with_stop_bits(self.config.stop_bits.clone())
            .with_parity(self.config.parity.clone())
            .with_flow_control(self.config.flow_control.clone())
            .with_timeout(10); // 连接后用短超时，便于非阻塞读取

        self.port = SerialPort::new(config);

        match self.port.connect() {
            Ok(()) => {
                self.config = self.port.config().clone();
                self.last_error = None;
                // 记录连接时刻，用于稳定期判断（设备 DTR 复位期间避免误判致命错误）
                self.connected_at = Some(std::time::Instant::now());
                // 递增连接会话 ID，通知子面板清除旧识别结果
                self.connection_session_id = self.connection_session_id.wrapping_add(1);
                // 记住该设备实际使用的端口与波特率（三设备配置互相独立）
                #[cfg(feature = "device-workspaces")]
                self.save_active_device_profile();
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
    }

    /// 断开串口
    pub fn disconnect(&mut self) {
        match self.port.disconnect() {
            Ok(()) => {
                self.last_error = None;
                self.connected_at = None;
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
    }

    /// 是否已连接
    pub fn is_connected(&self) -> bool {
        self.port.is_connected()
    }

    /// 进入设备工作区
    ///
    /// 行为：
    /// 1. 若当前已连接，先断开（避免把上一台设备的会话带到另一台设备）；
    /// 2. 保存上一台设备的端口/波特率；
    /// 3. 载入目标设备自己的端口/波特率（三台设备配置互相隔离）；
    /// 4. 切换到该设备的主面板。
    #[cfg(feature = "device-workspaces")]
    pub fn enter_device(&mut self, dev: DeviceId) {
        if self.is_connected() {
            self.disconnect();
        }
        self.save_active_device_profile();

        self.active_device = Some(dev);
        let profile = self.settings.devices.get(dev).clone();

        // 端口：优先使用该设备上次的端口；找不到则沿用当前选择
        self.config.baud_rate = profile.baud;
        if let Some(name) = profile.port {
            if let Some(idx) = self
                .port_list
                .iter()
                .position(|p| p.name == name)
            {
                self.selected_port_index = idx;
            }
            self.config.port_name = name;
        } else if let Some(name) = self.selected_port_name() {
            self.config.port_name = name.to_string();
        }

        self.active_panel = dev.meta().panel;
        self.status_message = crate::i18n::tf("Entered {device} workspace", &[("{device}", "{device}", dev.name())]);
    }

    /// 切换界面语言并立即持久化。
    ///
    /// `code = None` 表示跟随系统语言（保存为“无手动选择”）。
    /// 切换只改全局语言索引，下一帧所有 `t()` 调用立刻返回新语言，无需重启。
    pub fn set_language(&mut self, code: Option<&str>) {
        self.settings.language = code.map(|c| c.to_string());
        let lang = code
            .and_then(Language::from_code)
            .unwrap_or_else(Language::system);
        self.language = lang;
        crate::i18n::set_language(lang);
        self.settings.save();
    }

    /// 当前是否“跟随系统语言”（没有手动指定）。
    pub fn language_follows_system(&self) -> bool {
        self.settings.language.is_none()
    }

    /// 把当前端口/波特率写回「当前设备」的独立配置（未选择设备时不做任何事）
    #[cfg(feature = "device-workspaces")]
    pub fn save_active_device_profile(&mut self) {
        let Some(dev) = self.active_device else {
            return;
        };
        let port = self.selected_port_name().map(|s| s.to_string());
        let baud = self.config.baud_rate;
        let profile = self.settings.devices.get_mut(dev);
        if profile.port != port || profile.baud != baud {
            profile.port = port;
            profile.baud = baud;
            self.settings.save();
        }
    }

    /// 非阻塞读取串口数据（timeout 已设为 10ms），追加到 rx_buffer
    /// 在每帧 update() 中调用
    pub fn poll_rx(&mut self) {
        if !self.port.is_connected() {
            return;
        }

        // 连接稳定期判断：连接后 1.5 秒内忽略所有读取错误
        // Arduino/ESP32 等设备在 DTR 变化时触发复位，复位期间读取会返回
        // Broken pipe / Input/output error 等，这些是暂时性的，不应断开连接
        let in_settling = self
            .connected_at
            .map_or(false, |t| t.elapsed() < std::time::Duration::from_millis(1500));

        let mut buf = [0u8; 4096];
        match self.port.read(&mut buf) {
            Ok(n) if n > 0 => {
                let data = buf[..n].to_vec();
                self.bytes_rx += n as u64;

                // 串扰检测：喂入 RX 数据
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_micros() as u64)
                    .ok();
                self.crosstalk.push_rx(&data, ts);
                // 软件逻辑分析仪：喂入 RX 字节（D.5 协议解码）
                {
                    let ts_us = ts.unwrap_or(0);
                    let dir = miconlab_core::protocol::logic_analyzer::Direction::Rx;
                    self.logic_analyzer.ingest_slice(&data, dir, ts_us);
                }
                // WebSocket 桥：广播 RX 数据给 HTML 客户端
                if let Some(ref wb) = self.web_bridge {
                    wb.broadcast_rx(&data, ts.unwrap_or(0));
                }
                // 节流：每 30 帧 detect 一次并广播诊断报告
                self.frame_counter = self.frame_counter.wrapping_add(1);
                if self.frame_counter % 30 == 0 {
                    let report = self.crosstalk.detect();
                    if let Some(ref wb) = self.web_bridge {
                        wb.broadcast_diag(&report);
                    }
                    self.crosstalk_report = Some(report);
                }

                // 暂停时仍计数 bytes_rx，但不追加到显示缓冲区
                if self.paused {
                    return;
                }

                // 自动检测二进制数据：若不可打印字符占比 > 30%，切换到 HEX 模式
                if !self.hex_mode {
                    let non_printable = data.iter().filter(|&&b| {
                        b < 0x20 && b != b'\r' && b != b'\n' && b != b'\t'
                            || b == 0xFF
                            || b >= 0x80
                    }).count();
                    if non_printable * 10 > data.len() * 3 {
                        self.hex_mode = true;
                    }
                }

                self.rx_buffer.push(RxEntry {
                    timestamp: Local::now(),
                    data,
                    direction: Direction::Rx,
                });
                self.rx_entry_seq = self.rx_entry_seq.wrapping_add(1);
                // 限制缓冲区大小，避免无限增长和渲染卡顿
                const MAX_ENTRIES: usize = 2000;
                if self.rx_buffer.len() > MAX_ENTRIES {
                    let drop = self.rx_buffer.len() - MAX_ENTRIES;
                    self.rx_buffer.drain(0..drop);
                }

                // 成功读取数据时清除之前的临时错误（如偶尔的 EAGAIN）
                if self.last_error.as_deref().map_or(false, |e| {
                    !is_fatal_read_error(e)
                }) {
                    self.last_error = None;
                }
            }
            Ok(_) => {}
            Err(PortError::ReadError(msg)) if msg.contains("TimedOut") || msg.contains("timed out") => {
                // 读取超时是正常的，忽略
            }
            Err(PortError::ReadError(ref msg)) if in_settling => {
                // 连接稳定期内：设备可能正在 DTR 复位，忽略所有错误，不设置 last_error
                // 仅在 debug 模式下可考虑 log::debug!("settling read error: {}", msg)
            }
            Err(PortError::ReadError(ref msg)) if is_fatal_read_error(msg) => {
                // 致命错误：设备已断开/被拔出/权限丢失等，自动断开连接
                let _zh = self.language == crate::i18n::Language::Zh;
                let hint = { crate::i18n::tf("Device disconnected ({}). Please reconnect", &[("{}", "{}", &format!("{}", msg))]) };
                let _ = self.port.disconnect();
                self.connected_at = None;
                self.last_error = Some(hint);
            }
            Err(PortError::ReadError(msg)) => {
                // 非致命读取错误（如 EAGAIN/资源暂时不可用）：仅在错误内容变化时更新，
                // 避免状态栏持续刷新相同消息
                let new_err = format!("Read error: {}", msg);
                if self.last_error.as_deref() != Some(&new_err) {
                    self.last_error = Some(new_err);
                }
            }
            Err(e) => {
                if !in_settling {
                    self.last_error = Some(e.to_string());
                }
            }
        }
    }

    /// 发送 tx_input 内容，更新 bytes_tx，并把发送数据追加到显示
    pub fn send_tx(&mut self) {
        if !self.port.is_connected() {
            self.last_error = Some("Not connected".to_string());
            return;
        }

        let bytes = if self.hex_mode {
            match parse_hex(&self.tx_input) {
                Ok(b) => b,
                Err(e) => {
                    self.last_error = Some(format!("Hex parse error: {}", e));
                    return;
                }
            }
        } else {
            let mut data = self.tx_input.as_bytes().to_vec();
            data.extend_from_slice(self.line_ending.as_bytes());
            data
        };

        if bytes.is_empty() {
            return;
        }

        match self.port.write(&bytes) {
            Ok(n) => {
                self.bytes_tx += n as u64;

                // 串扰检测：喂入 TX 数据
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_micros() as u64)
                    .ok();
                self.crosstalk.push_tx(&bytes, ts);
                // 软件逻辑分析仪：喂入 TX 字节（D.5 协议解码）
                {
                    let ts_us = ts.unwrap_or(0);
                    let dir = miconlab_core::protocol::logic_analyzer::Direction::Tx;
                    self.logic_analyzer.ingest_slice(&bytes, dir, ts_us);
                }
                // WebSocket 桥：广播 TX 数据（回显给 HTML 显示）
                if let Some(ref wb) = self.web_bridge {
                    wb.broadcast_tx(&bytes, ts.unwrap_or(0));
                }

                self.rx_buffer.push(RxEntry {
                    timestamp: Local::now(),
                    data: bytes,
                    direction: Direction::Tx,
                });
                if self.rx_buffer.len() > 5000 {
                    let excess = self.rx_buffer.len() - 5000;
                    self.rx_buffer.drain(..excess);
                }
                self.rx_entry_seq = self.rx_entry_seq.wrapping_add(1);
                self.tx_input.clear();
                self.last_error = None;
                // 标记需要重新聚焦输入框
                self.terminal_input_focused = true;
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
    }

    /// 清空终端显示
    ///
    /// 仅清空 rx_buffer 显示缓冲区，不重置 bytes_tx/bytes_rx 累计计数器。
    /// 计数器是全局累计值，重置会导致 monitor 等面板的增量跟踪失效。
    pub fn clear_terminal(&mut self) {
        self.rx_buffer.clear();
    }

    // ---- 面板共享方法 ----
    // 以下方法供 Modbus/CAN/I2C/Flasher 等面板复用

    /// 发送原始字节到串口，返回写入字节数。
    /// 同时更新 bytes_tx 并把发送数据回显到终端。
    pub fn send_raw(&mut self, data: &[u8]) -> Result<usize, String> {
        if !self.port.is_connected() {
            return Err("Not connected".to_string());
        }
        match self.port.write(data) {
            Ok(n) => {
                self.bytes_tx += n as u64;
                self.rx_buffer.push(RxEntry {
                    timestamp: Local::now(),
                    data: data.to_vec(),
                    direction: Direction::Tx,
                });
                self.rx_entry_seq = self.rx_entry_seq.wrapping_add(1);
                const MAX_ENTRIES: usize = 5000;
                if self.rx_buffer.len() > MAX_ENTRIES {
                    let drop = self.rx_buffer.len() - MAX_ENTRIES;
                    self.rx_buffer.drain(0..drop);
                }
                Ok(n)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// 阻塞读取串口响应（临时设为长超时），返回读取到的字节。
    /// 读取完成后恢复短超时（10ms）。
    ///
    /// 暂停状态下仍可读取数据（返回给调用者），但不追加到 rx_buffer 显示缓冲区，
    /// 避免暂停时终端被新数据污染。bytes_rx 计数器仍正常更新以保持增量跟踪一致性。
    pub fn read_response(&mut self, timeout_ms: u64) -> Result<Vec<u8>, String> {
        if !self.port.is_connected() {
            return Err("Not connected".to_string());
        }
        // 临时设长超时
        let _ = self.port.set_timeout(std::time::Duration::from_millis(timeout_ms));
        let mut buf = [0u8; 4096];
        let result = self.port.read(&mut buf);
        // 恢复短超时
        let _ = self.port.set_timeout(std::time::Duration::from_millis(10));
        match result {
            Ok(n) if n > 0 => {
                let data = buf[..n].to_vec();
                self.bytes_rx += n as u64;
                // 暂停时不追加到 rx_buffer，避免终端显示被污染
                if !self.paused {
                    self.rx_buffer.push(RxEntry {
                        timestamp: Local::now(),
                        data: data.clone(),
                        direction: Direction::Rx,
                    });
                    self.rx_entry_seq = self.rx_entry_seq.wrapping_add(1);
                    const MAX_ENTRIES: usize = 5000;
                    if self.rx_buffer.len() > MAX_ENTRIES {
                        let drop = self.rx_buffer.len() - MAX_ENTRIES;
                        self.rx_buffer.drain(0..drop);
                    }
                }
                Ok(data)
            }
            Ok(_) => Err("No response".to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// 发送数据并等待响应（便捷组合）。
    pub fn send_and_read(&mut self, data: &[u8], wait_ms: u64) -> Result<Vec<u8>, String> {
        self.send_raw(data)?;
        std::thread::sleep(std::time::Duration::from_millis(wait_ms));
        self.read_response(wait_ms * 2)
    }

    /// 清空串口输入缓冲区（丢弃所有已接收但未读取的数据）。
    /// 在发送 Bootloader 命令前调用，避免残留数据干扰响应解析。
    pub fn flush_input(&self) {
        use miconlab_core::port::ClearBuffer;
        let _ = self.port.clear_buffer(ClearBuffer::Input);
    }

    /// 设置 DTR 信号（Data Terminal Ready）。
    /// 多数 STM32/Arduino 板的 DTR 连接 RESET 引脚（低电平复位）。
    pub fn set_dtr(&mut self, on: bool) {
        let _ = self.port.write_data_terminal_ready(on);
    }

    /// 设置 RTS 信号（Request To Send）。
    /// 多数 STM32 板的 RTS 连接 BOOT0 引脚（高电平进入 Bootloader）。
    pub fn set_rts(&mut self, on: bool) {
        let _ = self.port.write_request_to_send(on);
    }

    // ---- 配置 setter ----
    // 这些方法当前未被 UI 调用，预留供批次 3 的 GUI 面板扩展
    // （连接配置面板、参数调节对话框等）使用。

    /// 设置波特率
    #[allow(dead_code)]
    pub fn set_baud_rate(&mut self, baud: u32) {
        self.config.baud_rate = baud;
    }

    /// 设置数据位
    #[allow(dead_code)]
    pub fn set_data_bits(&mut self, bits: DataBits) {
        self.config.data_bits = bits;
    }

    /// 设置停止位
    #[allow(dead_code)]
    pub fn set_stop_bits(&mut self, bits: StopBits) {
        self.config.stop_bits = bits;
    }

    /// 设置校验
    #[allow(dead_code)]
    pub fn set_parity(&mut self, parity: Parity) {
        self.config.parity = parity;
    }

    /// 设置流控
    #[allow(dead_code)]
    pub fn set_flow_control(&mut self, fc: FlowControl) {
        self.config.flow_control = fc;
    }

    /// 设置超时
    #[allow(dead_code)]
    pub fn set_timeout_ms(&mut self, ms: u64) {
        self.config.timeout_ms = ms;
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serialport::SerialPort as _;
    use std::io::Read as _;

    #[test]
    fn test_send_tx_bounds_terminal_entries() {
        let (mut master, slave) = serialport::TTYPort::pair().unwrap();
        let port_name = slave.name().unwrap();
        drop(slave);

        let mut state = AppState::new();
        state.config.port_name = port_name;
        state.port = SerialPort::new(state.config.clone());
        state.port.connect().unwrap();
        state.line_ending = LineEnding::None;
        for _ in 0..5001 {
            state.tx_input = "x".to_string();
            state.send_tx();
            let mut drain = [0u8; 16];
            let _ = master.read(&mut drain);
        }

        assert_eq!(state.rx_buffer.len(), 5000);
        assert_eq!(state.rx_buffer.last().unwrap().data, b"x");
        state.port.disconnect().unwrap();
        drop(master);
    }
}

/// 解析十六进制字符串为字节向量
///
/// 委托到 [`miconlab_core::hex::parse_hex`] 统一实现，
/// 支持 : - 分隔符与偏移上下文错误信息。
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    miconlab_core::hex::parse_hex(input)
}

/// 将字节格式化为十六进制字符串（空格分隔）
pub fn format_hex(data: &[u8]) -> String {
    miconlab_core::hex::format_hex_spaced(data)
}

/// 判断读取错误是否为致命错误（设备已断开/不可恢复）
///
/// 致命错误意味着串口连接已失效，需要自动断开并提示用户重新连接：
/// - Broken pipe / EPIPE：对端关闭连接（设备拔出/重置）
/// - No such device / ENXIO：设备节点消失（USB 拔出）
/// - Permission denied / EACCES：权限丢失
/// - Operation not permitted / EPERM：操作被拒绝
/// - Bad file descriptor / EBADF：文件描述符无效
/// - Input/output error / EIO：I/O 错误
/// - No such file or directory / ENOENT：设备路径不存在
/// - Resource temporarily unavailable 仅在非阻塞模式下是临时的，此处不视为致命
fn is_fatal_read_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    // 致命错误关键字（设备已断开/不可恢复）
    const FATAL_KEYWORDS: &[&str] = &[
        "broken pipe",
        "no such device",
        "no such file or directory",
        "permission denied",
        "operation not permitted",
        "bad file descriptor",
        "input/output error",
        "device not configured",
        "operation timed out", // 完整的 Operation timed out（区别于 TimedOut）
        "no device",
        "exchange full",
        "not connected",
    ];
    FATAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
}
