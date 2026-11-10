//! `miconlab modbus` — Modbus RTU 快速请求。

use anyhow::Result;
use miconlab_core::protocol::{ModbusFrame, ModbusFunction, ModbusParser};
use std::time::Duration;

use crate::util::open_port;

pub fn run(
    port_name: &str,
    baud: u32,
    slave_id: u8,
    function: u8,
    address: u16,
    value: u16,
) -> Result<()> {
    // slave_id=0 是 Modbus 广播地址：写命令会被总线上所有从站执行，必须显式禁止
    if !(1..=247).contains(&slave_id) {
        eprintln!("Invalid slave_id {}: must be 1-247 (0 is broadcast, writes would hit ALL slaves)", slave_id);
        return Ok(());
    }
    let mut port = open_port(port_name, baud)?;

    let frame = match function {
        1 => ModbusParser::build_read_request(slave_id, ModbusFunction::ReadCoils, address, value),
        2 => ModbusParser::build_read_request(
            slave_id,
            ModbusFunction::ReadDiscreteInputs,
            address,
            value,
        ),
        3 => ModbusParser::build_read_request(
            slave_id,
            ModbusFunction::ReadHoldingRegisters,
            address,
            value,
        ),
        4 => ModbusParser::build_read_request(
            slave_id,
            ModbusFunction::ReadInputRegisters,
            address,
            value,
        ),
        // FC5 写单线圈：PDU 语义为 0xFF00=ON / 0x0000=OFF，不能用 FC6 的写寄存器帧
        5 => miconlab_core::protocol::modbus_common::build_write_single_coil_frame(
            slave_id,
            address,
            value != 0,
        ),
        6 => ModbusParser::build_write_single(slave_id, address, value),
        _ => {
            eprintln!("Unsupported function code: {}. Use 1-6.", function);
            port.disconnect()?;
            return Ok(());
        }
    };

    let req = frame.to_bytes();
    let req_hex = miconlab_core::hex::format_hex_spaced(&req);
    println!("TX: {}", req_hex);

    port.write(&req)?;
    std::thread::sleep(Duration::from_millis(200));

    let mut buf = [0u8; 256];
    match port.read(&mut buf) {
        Ok(n) if n >= 4 => {
            let resp = &buf[..n];
            let resp_hex = miconlab_core::hex::format_hex_spaced(resp);
            println!("RX: {}", resp_hex);

            if let Ok(f) = ModbusFrame::parse(resp) {
                if f.is_exception() {
                    let code = f.exception_code().unwrap_or(0);
                    let name = exception_name(code);
                    eprintln!("Modbus exception 0x{:02X}: {}", code, name);
                } else if function <= 2 {
                    // FC1/FC2：data[0] = 字节数，其后为按位打包的线圈/离散输入状态（LSB 在前）
                    let data = &f.data;
                    let byte_count = data.first().copied().unwrap_or(0) as usize;
                    let mut coils = Vec::new();
                    for &b in data.get(1..1 + byte_count).unwrap_or(&[]) {
                        for bit in 0..8 {
                            coils.push((b >> bit) & 1 == 1);
                        }
                    }
                    coils.truncate(value as usize);
                    println!("Coils: {:?}", coils);
                } else if function <= 4 {
                    // FC3/FC4：data[0] = 字节数，其后为大端 u16 寄存器值
                    let data = &f.data;
                    let mut i = 1; // skip byte count
                    let mut regs = Vec::new();
                    while i + 1 < data.len() {
                        let val = u16::from_be_bytes([data[i], data[i + 1]]);
                        regs.push(val);
                        i += 2;
                    }
                    println!("Values: {:?}", regs);
                } else {
                    println!("Write OK");
                }
            } else {
                eprintln!("Failed to parse response");
            }
        }
        Ok(n) => {
            let resp_hex = miconlab_core::hex::format_hex_spaced(&buf[..n]);
            eprintln!("Short response ({} bytes): {}", n, resp_hex);
        }
        Err(e) => {
            eprintln!("Read error: {}", e);
        }
    }

    port.disconnect()?;
    Ok(())
}

/// Modbus 异常码到可读名称的映射。
fn exception_name(code: u8) -> &'static str {
    match code {
        0x01 => "Illegal Function",
        0x02 => "Illegal Data Address",
        0x03 => "Illegal Data Value",
        0x04 => "Slave Device Failure",
        0x05 => "Acknowledge",
        0x06 => "Slave Device Busy",
        0x08 => "Memory Parity Error",
        0x0A => "Gateway Path Unavailable",
        0x0B => "Gateway Target Failed to Respond",
        _ => "Unknown",
    }
}
