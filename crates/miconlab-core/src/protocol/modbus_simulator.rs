/// Modbus HMI Simulator — virtual Modbus slave.
///
/// Simulates an HMI panel with configurable holding registers, coils,
/// input registers, and discrete inputs. Responds to Modbus TCP or RTU requests.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::modbus::{ModbusFrame, ModbusFunction};
use super::modbus_tcp::ModbusTcpFrame;

/// Virtual device configuration.
pub struct SimulatorConfig {
    pub mode: SimulatorMode,
    pub tcp_port: u16,
    pub serial_port_name: String,
    pub baud_rate: u32,
    pub slave_id: u8,
    pub holding_registers: HashMap<u16, u16>,
    pub input_registers: HashMap<u16, u16>,
    pub coils: HashMap<u16, bool>,
    pub discrete_inputs: HashMap<u16, bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimulatorMode {
    TcpServer,
    RtuSlave,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        let mut holding_registers = HashMap::new();
        for i in 0..10 {
            holding_registers.insert(i, 0);
        }
        let mut coils = HashMap::new();
        for i in 0..16 {
            coils.insert(i, false);
        }
        Self {
            mode: SimulatorMode::TcpServer,
            tcp_port: 502,
            serial_port_name: String::new(),
            baud_rate: 9600,
            slave_id: 1,
            holding_registers,
            input_registers: HashMap::new(),
            coils,
            discrete_inputs: HashMap::new(),
        }
    }
}

pub struct SimulatorLogEntry {
    pub timestamp: i64,
    pub direction: String, // "RX" or "TX"
    pub hex: String,
    pub decoded: String,
    pub success: bool,
}

/// Start the HMI simulator.
/// Returns (stop_flag, log_receiver, error_receiver).
pub fn start_simulator(
    config: SimulatorConfig,
) -> Result<(Arc<AtomicBool>, mpsc::Receiver<SimulatorLogEntry>, mpsc::Receiver<String>), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (log_tx, log_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();

    let registers = Arc::new(Mutex::new(SimulatorState {
        holding: config.holding_registers.clone(),
        input: config.input_registers.clone(),
        coils: config.coils.clone(),
        discrete: config.discrete_inputs.clone(),
        slave_id: config.slave_id,
    }));

    let stop_clone = stop.clone();
    let log_tx_clone = log_tx.clone();
    let err_tx_clone = err_tx.clone();
    let registers_clone = registers.clone();

    match config.mode {
        SimulatorMode::TcpServer => {
            std::thread::spawn(move || {
                run_tcp_server(config.tcp_port, stop_clone, registers_clone, log_tx_clone, err_tx_clone);
            });
        }
        SimulatorMode::RtuSlave => {
            if config.serial_port_name.is_empty() {
                return Err("No serial port selected".into());
            }
            std::thread::spawn(move || {
                run_rtu_slave(
                    config.serial_port_name,
                    config.baud_rate,
                    stop_clone,
                    registers_clone,
                    log_tx_clone,
                    err_tx_clone,
                );
            });
        }
    }

    Ok((stop, log_rx, err_rx))
}

/// Update a holding register value (called from GUI).
pub fn update_holding_register(
    registers: &Arc<Mutex<SimulatorState>>,
    addr: u16,
    value: u16,
) {
    if let Ok(mut state) = registers.lock() {
        state.holding.insert(addr, value);
    }
}

/// Update a coil value (called from GUI).
pub fn update_coil(
    registers: &Arc<Mutex<SimulatorState>>,
    addr: u16,
    value: bool,
) {
    if let Ok(mut state) = registers.lock() {
        state.coils.insert(addr, value);
    }
}

pub struct SimulatorState {
    pub holding: HashMap<u16, u16>,
    pub input: HashMap<u16, u16>,
    pub coils: HashMap<u16, bool>,
    pub discrete: HashMap<u16, bool>,
    pub slave_id: u8,
}

fn run_tcp_server(
    tcp_port: u16,
    stop: Arc<AtomicBool>,
    registers: Arc<Mutex<SimulatorState>>,
    log_tx: mpsc::Sender<SimulatorLogEntry>,
    err_tx: mpsc::Sender<String>,
) {
    let addr = format!("0.0.0.0:{}", tcp_port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            let _ = err_tx.send(format!("Failed to bind {}: {}", addr, e));
            return;
        }
    };
    listener.set_nonblocking(true).ok();
    let _ = err_tx.send(format!("Simulator listening on {}", addr));

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_millis(500))).ok();
                stream.set_write_timeout(Some(Duration::from_millis(500))).ok();

                let stop_c = stop.clone();
                let reg_c = registers.clone();
                let log_c = log_tx.clone();

                std::thread::spawn(move || {
                    handle_tcp_client_sim(stream, stop_c, reg_c, log_c);
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
    }
}

fn handle_tcp_client_sim(
    mut stream: TcpStream,
    stop: Arc<AtomicBool>,
    registers: Arc<Mutex<SimulatorState>>,
    log_tx: mpsc::Sender<SimulatorLogEntry>,
) {
    let mut header = [0u8; 6];

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        }

        let transaction_id = u16::from_be_bytes([header[0], header[1]]);
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;

        if length < 1 || length > 256 {
            break;
        }

        let mut pdu = vec![0u8; length];
        if stream.read_exact(&mut pdu).is_err() {
            break;
        }

        let mut full_tcp = Vec::with_capacity(6 + length);
        full_tcp.extend_from_slice(&header);
        full_tcp.extend_from_slice(&pdu);

        let tcp_frame = match ModbusTcpFrame::parse(&full_tcp) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let req_hex = hex_str(&full_tcp);
        let decoded = format!("FC={:02X} Addr={:04X}", tcp_frame.function.to_code(), decode_addr(&tcp_frame));

        log_tx.send(SimulatorLogEntry {
            timestamp: chrono::Utc::now().timestamp_millis(),
            direction: "RX".into(),
            hex: req_hex,
            decoded,
            success: true,
        }).ok();

        // Process request
        let mut state = registers.lock().unwrap_or_else(|e| e.into_inner());
        let response_data = process_request(&tcp_frame, &mut state);
        drop(state);

        // Build response
        let resp_function = if response_data.is_err() {
            ModbusFunction::Other(tcp_frame.function.to_code() | 0x80)
        } else {
            tcp_frame.function.clone()
        };
        let resp_data = match response_data {
            Ok(d) => d,
            Err(code) => vec![code],
        };

        let resp_frame = ModbusTcpFrame::new(transaction_id, tcp_frame.unit_id, resp_function, resp_data);
        let resp_bytes = resp_frame.to_bytes();

        let resp_hex = hex_str(&resp_bytes);
        log_tx.send(SimulatorLogEntry {
            timestamp: chrono::Utc::now().timestamp_millis(),
            direction: "TX".into(),
            hex: resp_hex,
            decoded: format!("Resp FC={:02X}", resp_frame.function.to_code()),
            success: true,
        }).ok();

        if stream.write_all(&resp_bytes).is_err() {
            break;
        }
    }
}

fn run_rtu_slave(
    serial_port_name: String,
    baud_rate: u32,
    stop: Arc<AtomicBool>,
    registers: Arc<Mutex<SimulatorState>>,
    log_tx: mpsc::Sender<SimulatorLogEntry>,
    err_tx: mpsc::Sender<String>,
) {
    let serial_config = crate::config::SerialConfig {
        port_name: serial_port_name,
        baud_rate,
        ..Default::default()
    };
    let mut port = crate::SerialPort::new(serial_config);

    if port.connect().is_err() {
        let _ = err_tx.send("Failed to open serial port".into());
        return;
    }
    let _ = err_tx.send("RTU slave started".into());

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let mut buf = [0u8; 256];
        match port.read(&mut buf) {
            Ok(n) if n >= 4 => {
                let data = &buf[..n];
                let req_hex = hex_str(data);

                match ModbusFrame::parse(data) {
                    Ok(rtu_frame) => {
                        let mut state = registers.lock().unwrap_or_else(|e| e.into_inner());
                        let tcp_frame = ModbusTcpFrame::from_rtu_frame(&rtu_frame, 0);
                        let response_data = process_request(&tcp_frame, &mut state);
                        drop(state);

                        let resp_function = if response_data.is_err() {
                            ModbusFunction::Other(rtu_frame.function.to_code() | 0x80)
                        } else {
                            rtu_frame.function.clone()
                        };
                        let resp_data = match response_data {
                            Ok(d) => d,
                            Err(code) => vec![code],
                        };

                        let resp_rtu = ModbusFrame::new(rtu_frame.slave_id, resp_function, resp_data);
                        let resp_bytes = resp_rtu.to_bytes();

                        log_tx.send(SimulatorLogEntry {
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            direction: "RX".into(),
                            hex: req_hex,
                            decoded: format!("FC={:02X}", rtu_frame.function.to_code()),
                            success: true,
                        }).ok();

                        let _ = port.write(&resp_bytes);

                        log_tx.send(SimulatorLogEntry {
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            direction: "TX".into(),
                            hex: hex_str(&resp_bytes),
                            decoded: format!("Resp FC={:02X}", resp_rtu.function.to_code()),
                            success: true,
                        }).ok();
                    }
                    Err(_) => {}
                }
            }
            _ => {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    let _ = port.disconnect();
}

fn process_request(frame: &ModbusTcpFrame, state: &mut SimulatorState) -> Result<Vec<u8>, u8> {
    if frame.unit_id != state.slave_id && state.slave_id != 0 {
        return Err(0x06); // Slave device failure
    }

    match frame.function {
        ModbusFunction::ReadHoldingRegisters => {
            if frame.data.len() < 4 { return Err(0x02); }
            let start = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let count = u16::from_be_bytes([frame.data[2], frame.data[3]]);
            // Modbus spec: max 125 registers per read. Reject larger counts
            // to avoid `count * 2` overflowing `u8` when count > 127.
            if count > 125 { return Err(0x03); } // Illegal data value
            let byte_count = (count * 2) as u8;
            let mut result = vec![byte_count];
            for i in 0..count {
                let addr = start.checked_add(i).ok_or(0x02)?;
                let val = state.holding.get(&addr).copied().unwrap_or(0);
                result.extend_from_slice(&val.to_be_bytes());
            }
            Ok(result)
        }
        ModbusFunction::ReadInputRegisters => {
            if frame.data.len() < 4 { return Err(0x02); }
            let start = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let count = u16::from_be_bytes([frame.data[2], frame.data[3]]);
            if count > 125 { return Err(0x03); }
            let byte_count = (count * 2) as u8;
            let mut result = vec![byte_count];
            for i in 0..count {
                let addr = start.checked_add(i).ok_or(0x02)?;
                let val = state.input.get(&addr).copied().unwrap_or(0);
                result.extend_from_slice(&val.to_be_bytes());
            }
            Ok(result)
        }
        ModbusFunction::ReadCoils => {
            if frame.data.len() < 4 { return Err(0x02); }
            let start = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let count = u16::from_be_bytes([frame.data[2], frame.data[3]]);
            // Modbus spec: max 2000 coils per read. Also guards against
            // `((count + 7) / 8)` overflowing `u8` when count > 2040.
            if count > 2000 { return Err(0x03); }
            let byte_count = ((count as usize + 7) / 8) as u8;
            let mut result = vec![byte_count];
            let mut bits = 0u8;
            let mut bit_idx = 0;
            for i in 0..count {
                let addr = start.checked_add(i).ok_or(0x02)?;
                let val = state.coils.get(&addr).copied().unwrap_or(false);
                if val { bits |= 1 << bit_idx; }
                bit_idx += 1;
                if bit_idx == 8 {
                    result.push(bits);
                    bits = 0;
                    bit_idx = 0;
                }
            }
            if bit_idx > 0 { result.push(bits); }
            Ok(result)
        }
        ModbusFunction::ReadDiscreteInputs => {
            if frame.data.len() < 4 { return Err(0x02); }
            let start = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let count = u16::from_be_bytes([frame.data[2], frame.data[3]]);
            if count > 2000 { return Err(0x03); }
            let byte_count = ((count as usize + 7) / 8) as u8;
            let mut result = vec![byte_count];
            let mut bits = 0u8;
            let mut bit_idx = 0;
            for i in 0..count {
                let addr = start.checked_add(i).ok_or(0x02)?;
                let val = state.discrete.get(&addr).copied().unwrap_or(false);
                if val { bits |= 1 << bit_idx; }
                bit_idx += 1;
                if bit_idx == 8 {
                    result.push(bits);
                    bits = 0;
                    bit_idx = 0;
                }
            }
            if bit_idx > 0 { result.push(bits); }
            Ok(result)
        }
        ModbusFunction::WriteSingleRegister => {
            if frame.data.len() < 4 { return Err(0x02); }
            let addr = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let val = u16::from_be_bytes([frame.data[2], frame.data[3]]);
            state.holding.insert(addr, val);
            Ok(frame.data[0..4].to_vec())
        }
        ModbusFunction::WriteSingleCoil => {
            if frame.data.len() < 4 { return Err(0x02); }
            let addr = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let val = u16::from_be_bytes([frame.data[2], frame.data[3]]);
            state.coils.insert(addr, val != 0);
            Ok(frame.data[0..4].to_vec())
        }
        ModbusFunction::WriteMultipleRegisters => {
            if frame.data.len() < 5 { return Err(0x02); }
            let start = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let count = u16::from_be_bytes([frame.data[2], frame.data[3]]) as usize;
            let byte_count = frame.data[4] as usize;
            // Validate byte_count matches count, and that the payload fits.
            // Previously byte_count was read but never validated, allowing
            // malformed requests to read out-of-bounds or desync.
            if count > 123 { return Err(0x03); } // spec max for write multiple
            let expected_bytes = count * 2;
            if byte_count != expected_bytes {
                return Err(0x03); // Illegal data value
            }
            if frame.data.len() < 5 + expected_bytes { return Err(0x02); }
            for i in 0..count {
                let offset = 5 + i * 2;
                let val = u16::from_be_bytes([frame.data[offset], frame.data[offset + 1]]);
                let addr = start.checked_add(i as u16).ok_or(0x02)?;
                state.holding.insert(addr, val);
            }
            // 响应回显「起始地址 + 数量」。count 是 usize（64 位 8 字节），
            // to_be_bytes()[0..2] 取到的是恒 0 的高字节，必须直接回显请求中的原始字节。
            Ok(vec![frame.data[0], frame.data[1], frame.data[2], frame.data[3]])
        }
        ModbusFunction::WriteMultipleCoils => {
            if frame.data.len() < 5 { return Err(0x02); }
            let start = u16::from_be_bytes([frame.data[0], frame.data[1]]);
            let count = u16::from_be_bytes([frame.data[2], frame.data[3]]) as usize;
            let byte_count = frame.data[4] as usize;
            if count > 1968 { return Err(0x03); } // spec max for write multiple coils
            let expected_bytes = (count + 7) / 8;
            if byte_count != expected_bytes {
                return Err(0x03);
            }
            if frame.data.len() < 5 + expected_bytes { return Err(0x02); }
            for i in 0..count {
                let byte_idx = 5 + i / 8;
                let bit_idx = i % 8;
                let val = (frame.data[byte_idx] >> bit_idx) & 1 == 1;
                let addr = start.checked_add(i as u16).ok_or(0x02)?;
                state.coils.insert(addr, val);
            }
            Ok(vec![frame.data[0], frame.data[1], frame.data[2], frame.data[3]])
        }
        _ => Err(0x01), // Illegal function
    }
}

fn decode_addr(frame: &ModbusTcpFrame) -> u16 {
    if frame.data.len() >= 2 {
        u16::from_be_bytes([frame.data[0], frame.data[1]])
    } else {
        0
    }
}

fn hex_str(bytes: &[u8]) -> String {
    crate::hex::format_hex_spaced(bytes)
}
