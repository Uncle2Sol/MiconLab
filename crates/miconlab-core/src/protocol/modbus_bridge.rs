//! Modbus TCP/RTU Bridge (Actor 模型重构版)。
//!
//! 接受 Modbus TCP 连接，将请求转发到串口 RTU 设备。
//!
//! 架构：
//! - **串口 actor 线程**：独占串口，串行处理所有客户端请求。从 mpsc channel
//!   接收 `SerialRequest`，执行 write → 等待 → read，通过 per-request
//!   response channel 回传结果。
//! - **TCP 监听线程**：accept 客户端，为每个客户端 spawn 一个线程。
//! - **TCP 客户端线程**：读 MBAP 头 → 解析帧 → 把请求发给串口 actor →
//!   等待响应 → 转换回 TCP 帧写回客户端。
//!
//! 修复点：原实现 TCP 客户端线程持锁 `Arc<Mutex<SerialPort>>` 期间执行
//! `thread::sleep + port.read`，会阻塞所有其他客户端访问串口。Actor 模型
//! 将串口访问串行化到单一专用线程，客户端线程只持有 mpsc sender，不再
//! 互相阻塞。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::modbus::ModbusFrame;
use super::modbus_tcp::ModbusTcpFrame;

pub struct BridgeConfig {
    pub tcp_port: u16,
    pub serial_port_name: String,
    pub baud_rate: u32,
    pub timeout_ms: u64,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            tcp_port: 502,
            serial_port_name: String::new(),
            baud_rate: 9600,
            timeout_ms: 500,
        }
    }
}

pub struct BridgeLogEntry {
    pub timestamp: i64,
    pub client_addr: String,
    pub direction: String, // "TCP->RTU" or "RTU->TCP"
    pub request_hex: String,
    pub response_hex: String,
    pub success: bool,
}

/// 串口 actor 请求：TCP 客户端线程通过 mpsc::Sender 发送此结构给串口线程。
/// `response_tx` 是一个 per-request mpsc channel，串口线程处理完后通过它回传结果。
struct SerialRequest {
    req_bytes: Vec<u8>,
    response_tx: mpsc::Sender<Result<Vec<u8>, String>>,
}

/// Start the Modbus TCP/RTU bridge.
/// Returns (stop_flag, log_receiver, error_receiver).
pub fn start_bridge(
    config: BridgeConfig,
) -> Result<(Arc<AtomicBool>, mpsc::Receiver<BridgeLogEntry>, mpsc::Receiver<String>), String> {
    if config.serial_port_name.is_empty() {
        return Err("No serial port selected".into());
    }

    let stop = Arc::new(AtomicBool::new(false));
    let (log_tx, log_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    // 串口请求 channel：所有 TCP 客户端 -> 串口 actor
    let (serial_req_tx, serial_req_rx) = mpsc::channel::<SerialRequest>();

    let stop_clone = stop.clone();
    let serial_port_name = config.serial_port_name.clone();
    let baud_rate = config.baud_rate;
    let timeout_ms = config.timeout_ms;

    // ========================================================================
    // 串口 actor 线程：独占串口，串行处理请求
    // ========================================================================
    {
        let stop_c = stop.clone();
        let err_tx_c = err_tx.clone();
        let serial_port_name_c = serial_port_name.clone();
        thread::spawn(move || {
            let serial_config = crate::config::SerialConfig {
                port_name: serial_port_name_c,
                baud_rate,
                ..Default::default()
            };
            let mut port = crate::SerialPort::new(serial_config);
            if port.connect().is_err() {
                let _ = err_tx_c.send("Failed to open serial port".into());
                return;
            }
            let _ = err_tx_c.send("Serial actor ready".into());

            // 主循环：从 channel 取请求，串行处理
            while !stop_c.load(Ordering::Relaxed) {
                let req = match serial_req_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(r) => r,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                };

                // 串行处理：write → 累积读取完整 RTU 帧（修复点：原固定 sleep(50ms)+单次 read，
                // 9600 baud 下较长响应（如 FC03 读 25+ 寄存器）必被截断，且残留字节污染下一请求）
                if port.write(&req.req_bytes).is_err() {
                    let _ = req.response_tx.send(Err("Serial write failed".into()));
                    continue;
                }
                // 短超时轮询读取；帧完整性用 T3.5 静默间隔判定（3.5 个字符时间）
                let _ = port.set_timeout(Duration::from_millis(10));
                let gap_ms = ((3_500u64 * 11) / baud_rate.max(300) as u64).clamp(5, 50);
                let gap = Duration::from_millis(gap_ms);
                let deadline = Duration::from_millis(timeout_ms.max(200));
                let start = Instant::now();
                let mut last_data = Instant::now();
                let mut got_any = false;
                let mut acc: Vec<u8> = Vec::with_capacity(256);
                let mut buf = [0u8; 256];
                loop {
                    match port.read(&mut buf) {
                        Ok(n) if n > 0 => {
                            acc.extend_from_slice(&buf[..n]);
                            last_data = Instant::now();
                            got_any = true;
                        }
                        _ => {
                            // 收到过数据且静默超过 T3.5 → 帧完整
                            if got_any && last_data.elapsed() >= gap {
                                break;
                            }
                            if start.elapsed() >= deadline {
                                break;
                            }
                        }
                    }
                }
                let result = if acc.len() >= 4 {
                    Ok(acc)
                } else {
                    Err(format!("Insufficient bytes read: {}", acc.len()))
                };
                let _ = req.response_tx.send(result);
            }
            let _ = port.disconnect();
        });
    }

    // ========================================================================
    // TCP 监听线程：accept 客户端并为每个客户端 spawn 处理线程
    // ========================================================================
    thread::spawn(move || {
        let addr = format!("0.0.0.0:{}", config.tcp_port);
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                let _ = err_tx.send(format!("Failed to bind {}: {}", addr, e));
                return;
            }
        };
        listener.set_nonblocking(true).ok();

        let _ = err_tx.send(format!("Bridge listening on {}", addr));

        loop {
            if stop_clone.load(Ordering::Relaxed) {
                break;
            }

            match listener.accept() {
                Ok((stream, addr)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
                        .ok();
                    stream
                        .set_write_timeout(Some(Duration::from_millis(timeout_ms)))
                        .ok();

                    let stop_c = stop_clone.clone();
                    let serial_req_tx_c = serial_req_tx.clone();
                    let log_c = log_tx.clone();
                    let addr_str = addr.to_string();
                    let timeout_c = timeout_ms;

                    thread::spawn(move || {
                        handle_tcp_client(
                            stream,
                            addr_str,
                            stop_c,
                            serial_req_tx_c,
                            log_c,
                            timeout_c,
                        );
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        }
    });

    Ok((stop, log_rx, err_rx))
}

fn handle_tcp_client(
    mut stream: TcpStream,
    client_addr: String,
    stop: Arc<AtomicBool>,
    serial_req_tx: mpsc::Sender<SerialRequest>,
    log_tx: mpsc::Sender<BridgeLogEntry>,
    timeout_ms: u64,
) {
    let mut header = [0u8; 6]; // MBAP header (without unit_id)

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Read MBAP header
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break, // Client disconnected
        }

        let transaction_id = u16::from_be_bytes([header[0], header[1]]);
        let _protocol_id = u16::from_be_bytes([header[2], header[3]]);
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;

        if length < 1 || length > 256 {
            break;
        }

        let mut pdu = vec![0u8; length];
        if stream.read_exact(&mut pdu).is_err() {
            break;
        }

        // Reconstruct full TCP frame
        let mut full_tcp = Vec::with_capacity(6 + length);
        full_tcp.extend_from_slice(&header);
        full_tcp.extend_from_slice(&pdu);

        let tcp_frame = match ModbusTcpFrame::parse(&full_tcp) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let rtu_frame = tcp_frame.to_rtu_frame();
        let req_bytes = rtu_frame.to_bytes();
        let req_hex = hex_str(&req_bytes);

        // === Actor 模型关键修复点 ===
        // 不再直接 `serial_port.lock()` + `sleep` + `read`，
        // 而是把请求发给串口 actor，通过 per-request channel 等待响应。
        let (response_tx, response_rx) = mpsc::channel::<Result<Vec<u8>, String>>();
        let serial_req = SerialRequest {
            req_bytes: req_bytes.clone(),
            response_tx,
        };
        if serial_req_tx.send(serial_req).is_err() {
            // 串口线程已退出
            break;
        }

        // 等待响应：超时设为单次超时的 3 倍（允许串口 actor 排队）
        let response = match response_rx.recv_timeout(Duration::from_millis(timeout_ms * 3)) {
            Ok(Ok(r)) => r,
            Ok(Err(_)) | Err(RecvTimeoutError::Timeout) => {
                log_tx
                    .send(BridgeLogEntry {
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        client_addr: client_addr.clone(),
                        direction: "TCP->RTU".into(),
                        request_hex: req_hex.clone(),
                        response_hex: String::new(),
                        success: false,
                    })
                    .ok();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };

        let resp_hex = hex_str(&response);

        // Convert RTU response to TCP response
        let rtu_resp = match ModbusFrame::parse(&response) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let tcp_resp = ModbusTcpFrame::from_rtu_frame(&rtu_resp, transaction_id);
        let resp_bytes = tcp_resp.to_bytes();

        if stream.write_all(&resp_bytes).is_err() {
            break;
        }

        log_tx
            .send(BridgeLogEntry {
                timestamp: chrono::Utc::now().timestamp_millis(),
                client_addr: client_addr.clone(),
                direction: "TCP->RTU".into(),
                request_hex: req_hex,
                response_hex: resp_hex,
                success: true,
            })
            .ok();
    }
}

fn hex_str(bytes: &[u8]) -> String {
    crate::hex::format_hex_spaced(bytes)
}
