//! `miconlab monitor` — 持续监听串口数据。

use anyhow::Result;
use miconlab_core::port::PortError;
use std::io::Write;

use crate::util::open_port;

pub fn run(
    port_name: &str,
    baud: u32,
    log_file: Option<&str>,
    timestamp: bool,
    hex: bool,
) -> Result<()> {
    let mut port = open_port(port_name, baud)?;

    println!("Monitoring {} at {} baud", port_name, baud);
    println!("Press Ctrl+C to stop");
    println!();

    let mut log_writer = if let Some(path) = log_file {
        Some(std::fs::File::create(path)?)
    } else {
        None
    };

    let mut buf = [0u8; 1024];
    loop {
        match port.read(&mut buf) {
            Ok(n) => {
                if n > 0 {
                    if timestamp {
                        print!("[{}] ", chrono::Local::now().format("%H:%M:%S%.3f"));
                    }

                    if hex {
                        let hex_str = miconlab_core::hex::format_hex_spaced(&buf[..n]);
                        println!("{}", hex_str);
                    } else {
                        print!("{}", String::from_utf8_lossy(&buf[..n]));
                    }
                    std::io::stdout().flush()?;

                    if let Some(ref mut writer) = log_writer {
                        writer.write_all(&buf[..n])?;
                    }
                }
            }
            Err(ref e) if is_read_timeout(e) => {
                continue;
            }
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }
    }

    port.disconnect()?;
    Ok(())
}

/// 判断 PortError 是否为读超时（兼容底层 error message）。
fn is_read_timeout(e: &PortError) -> bool {
    matches!(e, PortError::ReadError(ref msg) if msg.contains("TimedOut"))
}
