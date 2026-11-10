//! `miconlab connect` — 连接串口并进入交互式读写循环。

use anyhow::Result;
use miconlab_core::SerialConfig;
use miconlab_core::config::{DataBits, FlowControl, Parity, StopBits};
use std::io::{self, Write};

use crate::util::{parse_data_bits, parse_flow_control, parse_parity, parse_stop_bits};

pub fn run(
    port_name: &str,
    baud: u32,
    data_bits: u8,
    stop_bits: u8,
    parity: &str,
    flow_control: &str,
) -> Result<()> {
    // 阶段三重构：使用 core 的 FromStr 实现，替代内联 match
    // 同时保留对原始 u8 入参的兼容（CLI 仍以 u8 接收 data_bits/stop_bits）
    // 非法参数告警并回退默认值，避免用户误以为配置已生效
    let data_bits_enum = match parse_data_bits(&data_bits.to_string()) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Warning: invalid data bits '{}', falling back to 8", data_bits);
            DataBits::Eight
        }
    };
    let stop_bits_enum = match parse_stop_bits(&stop_bits.to_string()) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Warning: invalid stop bits '{}', falling back to 1", stop_bits);
            StopBits::One
        }
    };
    let parity_enum = match parse_parity(parity) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Warning: invalid parity '{}', falling back to none", parity);
            Parity::None
        }
    };
    let flow_control_enum = match parse_flow_control(flow_control) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Warning: invalid flow control '{}', falling back to none", flow_control);
            FlowControl::None
        }
    };

    let config = SerialConfig::new(port_name)
        .with_baud_rate(baud)
        .with_data_bits(data_bits_enum)
        .with_stop_bits(stop_bits_enum)
        .with_parity(parity_enum)
        .with_flow_control(flow_control_enum);

    let mut port = miconlab_core::SerialPort::new(config);
    port.connect()?;

    println!("Connected to {} at {} baud", port_name, baud);
    println!("Press Ctrl+C to disconnect");
    println!();

    let mut input = String::new();
    loop {
        input.clear();
        print!("> ");
        io::stdout().flush()?;

        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }

        let data = input.trim();
        if data.is_empty() {
            continue;
        }

        port.write_string(data)?;
        port.write_string("\r\n")?;

        let mut buf = [0u8; 1024];
        match port.read(&mut buf) {
            Ok(n) => {
                let received = String::from_utf8_lossy(&buf[..n]);
                print!("{}", received);
                io::stdout().flush()?;
            }
            Err(e) => {
                eprintln!("Read error: {}", e);
            }
        }
    }

    port.disconnect()?;
    println!("\nDisconnected");
    Ok(())
}
