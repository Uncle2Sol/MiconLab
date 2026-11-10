//! WebSocket 桥模块
//!
//! 提供 WebSocket 服务器，让浏览器端的 HTML 可视化软件（如坐垫可视化软件.html）
//! 通过 ws://localhost:7831 连接到 miconlab 的 Rust 后端，共享串口数据和 AI 诊断结果。
//!
//! 架构：
//! - WebSocket 服务器在独立的 tokio 运行时上运行
//! - GUI 主线程通过 `broadcast_rx/broadcast_tx` 和 `command_rx` 与服务器通信
//! - 所有 API 同步非阻塞，与 egui 帧驱动兼容
//!
//! 协议：
//! - 服务端 → 客户端：JSON 消息（rx_data / diag_report / status）
//! - 客户端 → 服务端：JSON 命令（connect / send / set_baud / disconnect）

use crate::crosstalk::CrosstalkReport;
use serde::{Deserialize, Serialize};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use tokio::sync::broadcast;

/// WebSocket 桥默认端口
pub const DEFAULT_WEB_BRIDGE_PORT: u16 = 7831;

/// 从 HTML 客户端发来的命令
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebCommand {
    /// 连接串口
    #[serde(rename = "connect")]
    Connect { port: String, baud: u32 },
    /// 发送数据（hex 字符串）
    #[serde(rename = "send")]
    Send { data: String },
    /// 设置波特率
    #[serde(rename = "set_baud")]
    SetBaud { baud: u32 },
    /// 断开连接
    #[serde(rename = "disconnect")]
    Disconnect,
    /// 请求当前状态
    #[serde(rename = "status")]
    GetStatus,
}

/// 从服务端发给 HTML 客户端的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebMessage {
    /// RX 数据（hex 字符串）
    #[serde(rename = "rx_data")]
    RxData { data: String, timestamp: u64 },
    /// TX 数据（hex 字符串，回显给 HTML 显示）
    #[serde(rename = "tx_data")]
    TxData { data: String, timestamp: u64 },
    /// 诊断报告
    #[serde(rename = "diag")]
    Diag { report: DiagReport },
    /// 状态更新
    #[serde(rename = "status")]
    Status {
        connected: bool,
        port: Option<String>,
        baud: u32,
        tx_bytes: u64,
        rx_bytes: u64,
    },
    /// 错误消息
    #[serde(rename = "error")]
    Error { message: String },
}

/// 诊断报告（JSON 序列化，发给 HTML）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagReport {
    pub is_loopback: bool,
    pub echo_ratio: f32,
    pub rx_tx_ratio: f32,
    pub high_byte_ratio: f32,
    pub pattern: String,
    pub tx_total: u64,
    pub rx_total: u64,
    pub diagnosis: String,
    pub suggestions: Vec<String>,
}

impl From<&CrosstalkReport> for DiagReport {
    fn from(ct: &CrosstalkReport) -> Self {
        DiagReport {
            is_loopback: ct.is_loopback,
            echo_ratio: ct.echo_ratio,
            rx_tx_ratio: ct.rx_tx_ratio,
            high_byte_ratio: ct.high_byte_ratio,
            pattern: format!("{:?}", ct.pattern),
            tx_total: ct.tx_total,
            rx_total: ct.rx_total,
            diagnosis: if ct.is_loopback {
                "Detected TX self-echo loopback".to_string()
            } else if ct.high_byte_ratio > 0.4 {
                "High-byte dominant pattern (baud rate mismatch)".to_string()
            } else {
                "Normal".to_string()
            },
            suggestions: vec![],
        }
    }
}

/// WebSocket 桥句柄
///
/// GUI 主线程持有此句柄，通过 `broadcast` 发送数据给 HTML 客户端，
/// 通过 `poll_commands` 接收 HTML 发来的命令。
pub struct WebBridge {
    /// 广播通道发送端（发给所有 WebSocket 客户端）
    broadcast_tx: broadcast::Sender<WebMessage>,
    /// 命令通道接收端（从 HTML 客户端接收命令）
    command_rx: mpsc::Receiver<WebCommand>,
    /// 运行时句柄（用于关闭）
    runtime_handle: Option<thread::JoinHandle<()>>,
    /// 当前连接的客户端数
    client_count: Arc<Mutex<usize>>,
}

impl WebBridge {
    /// 启动 WebSocket 服务器
    ///
    /// 在独立线程中创建 tokio 运行时，监听指定端口。
    /// 失败时返回 Error，GUI 层应降级为"HTML 原生 Web Serial"模式。
    pub fn start(port: u16) -> Result<Self, std::io::Error> {
        let (broadcast_tx, _) = broadcast::channel::<WebMessage>(256);
        // 命令通道容量 64：防止 GUI 处理不及时导致内存无限增长
        let (command_tx, command_rx) = mpsc::sync_channel::<WebCommand>(64);
        let client_count = Arc::new(Mutex::new(0usize));
        let tx_clone = broadcast_tx.clone();
        let count_clone = client_count.clone();

        let handle = thread::Builder::new()
            .name("web-bridge".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        log::error!("[WebBridge] tokio runtime start failed: {}", e);
                        return;
                    }
                };

                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(
                        format!("127.0.0.1:{}", port).as_str(),
                    )
                    .await
                    {
                        Ok(l) => {
                            log::info!("[WebBridge] Listening on 127.0.0.1:{}", port);
                            l
                        }
                        Err(e) => {
                            log::error!("[WebBridge] Failed to bind port {}: {}", port, e);
                            return;
                        }
                    };

                    loop {
                        let (stream, addr) = match listener.accept().await {
                            Ok(s) => s,
                            Err(e) => {
                                log::warn!("[WebBridge] accept failed: {}", e);
                                continue;
                            }
                        };

                        let tx = tx_clone.clone();
                        let cmd_tx = command_tx.clone();
                        let count = count_clone.clone();

                        tokio::spawn(async move {
                            log::info!("[WebBridge] Client connected: {}", addr);
                            {
                                let mut c = count.lock().unwrap();
                                *c += 1;
                            }

                            let ws_stream = match tokio_tungstenite::accept_async(stream).await {
                                Ok(ws) => ws,
                                Err(e) => {
                                    log::warn!("[WebBridge] WebSocket handshake failed: {}", e);
                                    return;
                                }
                            };

                            use futures_util::{SinkExt, StreamExt};
                            let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();
                            let mut rx_sub = tx.subscribe();

                            // 发送欢迎消息
                            let welcome = WebMessage::Status {
                                connected: false,
                                port: None,
                                baud: 0,
                                tx_bytes: 0,
                                rx_bytes: 0,
                            };
                            if let Ok(json) = serde_json::to_string(&welcome) {
                                let _ = ws_sink
                                    .send(tokio_tungstenite::tungstenite::Message::Text(json))
                                    .await;
                            }

                            // 广播循环：接收 broadcast 并转发给客户端
                            let broadcast_task = tokio::spawn(async move {
                                while let Ok(msg) = rx_sub.recv().await {
                                    if let Ok(json) = serde_json::to_string(&msg) {
                                        if ws_sink
                                            .send(tokio_tungstenite::tungstenite::Message::Text(
                                                json,
                                            ))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            });

                            // 接收循环：从客户端接收命令
                            while let Some(msg) = ws_stream_rx.next().await {
                                match msg {
                                    Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                                        if let Ok(cmd) = serde_json::from_str::<WebCommand>(&text) {
                                            // sync_channel 满时丢弃命令，避免阻塞 WebSocket 接收循环
                                            let _ = cmd_tx.try_send(cmd);
                                        }
                                    }
                                    Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => {
                                        break;
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::debug!("[WebBridge] Client {} disconnected: {}", addr, e);
                                        break;
                                    }
                                }
                            }

                            broadcast_task.abort();
                            {
                                let mut c = count.lock().unwrap();
                                *c = c.saturating_sub(1);
                            }
                            log::info!("[WebBridge] Client disconnected: {}", addr);
                        });
                    }
                });
            })?;

        Ok(Self {
            broadcast_tx,
            command_rx,
            runtime_handle: Some(handle),
            client_count,
        })
    }

    /// 广播 RX 数据给所有连接的 HTML 客户端
    pub fn broadcast_rx(&self, data: &[u8], timestamp_us: u64) {
        let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
        let msg = WebMessage::RxData {
            data: hex,
            timestamp: timestamp_us,
        };
        let _ = self.broadcast_tx.send(msg);
    }

    /// 广播 TX 数据（回显给 HTML 显示）
    pub fn broadcast_tx(&self, data: &[u8], timestamp_us: u64) {
        let hex: String = data.iter().map(|b| format!("{:02X}", b)).collect();
        let msg = WebMessage::TxData {
            data: hex,
            timestamp: timestamp_us,
        };
        let _ = self.broadcast_tx.send(msg);
    }

    /// 广播诊断报告
    pub fn broadcast_diag(&self, report: &CrosstalkReport) {
        let diag: DiagReport = report.into();
        let msg = WebMessage::Diag { report: diag };
        let _ = self.broadcast_tx.send(msg);
    }

    /// 广播状态更新
    pub fn broadcast_status(
        &self,
        connected: bool,
        port: Option<&str>,
        baud: u32,
        tx_bytes: u64,
        rx_bytes: u64,
    ) {
        let msg = WebMessage::Status {
            connected,
            port: port.map(|s| s.to_string()),
            baud,
            tx_bytes,
            rx_bytes,
        };
        let _ = self.broadcast_tx.send(msg);
    }

    /// 广播错误消息
    pub fn broadcast_error(&self, message: impl Into<String>) {
        let msg = WebMessage::Error {
            message: message.into(),
        };
        let _ = self.broadcast_tx.send(msg);
    }

    /// 轮询 HTML 客户端发来的命令（非阻塞）
    ///
    /// 在 egui 每帧调用，返回所有待处理的命令。
    pub fn poll_commands(&mut self) -> Vec<WebCommand> {
        let mut cmds = Vec::new();
        while let Ok(cmd) = self.command_rx.try_recv() {
            cmds.push(cmd);
        }
        cmds
    }

    /// 当前连接的客户端数
    pub fn client_count(&self) -> usize {
        self.client_count.lock().map(|c| *c).unwrap_or(0)
    }

    /// 是否有客户端连接
    pub fn has_clients(&self) -> bool {
        self.client_count() > 0
    }
}

impl Drop for WebBridge {
    fn drop(&mut self) {
        // 线程会在运行时结束时自然退出（因为 block_on 会一直循环）
        // 这里只标记日志，不强制 join（避免阻塞 GUI）
        if let Some(handle) = self.runtime_handle.take() {
            log::info!("[WebBridge] Server shutting down (thread ID: {:?})", handle.thread().id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_command_serialize() {
        let cmd = WebCommand::Connect {
            port: "/dev/cu.usbserial-210".to_string(),
            baud: 921600,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("connect"));
        assert!(json.contains("921600"));

        let parsed: WebCommand = serde_json::from_str(&json).unwrap();
        match parsed {
            WebCommand::Connect { port, baud } => {
                assert_eq!(port, "/dev/cu.usbserial-210");
                assert_eq!(baud, 921600);
            }
            _ => panic!("解析错误"),
        }
    }

    #[test]
    fn test_web_message_serialize() {
        let msg = WebMessage::RxData {
            data: "A5FF".to_string(),
            timestamp: 1000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("rx_data"));
        assert!(json.contains("A5FF"));
    }

    #[test]
    fn test_diag_report_from_crosstalk() {
        let ct = CrosstalkReport {
            echo_ratio: 0.8,
            rx_tx_ratio: 0.5,
            is_loopback: true,
            latency_us: Some(100),
            pattern: crate::crosstalk::EchoPattern::Exact,
            high_byte_ratio: 0.9,
            tx_total: 528,
            rx_total: 264,
        };
        let diag: DiagReport = (&ct).into();
        assert!(diag.is_loopback);
        assert!((diag.echo_ratio - 0.8).abs() < 0.01);
        assert!(diag.diagnosis.contains("loopback"));
    }

    #[test]
    fn test_web_bridge_start_and_poll() {
        // 尝试启动服务器（端口可能被占用，测试容错）
        match WebBridge::start(17831) {
            Ok(mut bridge) => {
                // 无客户端时 poll_commands 应返回空
                let cmds = bridge.poll_commands();
                assert!(cmds.is_empty());
                assert_eq!(bridge.client_count(), 0);
            }
            Err(_) => {
                // 端口绑定失败可接受（CI 环境可能无网络权限）
            }
        }
    }
}
