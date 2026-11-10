//! `miconlab send` — 单次发送数据到串口。

use anyhow::Result;
use std::time::Duration;

use crate::util::{open_port, parse_hex};

pub fn run(port_name: &str, data: &str, baud: u32, hex: bool) -> Result<()> {
    let mut port = open_port(port_name, baud)?;

    let bytes = if hex {
        parse_hex(data)?
    } else {
        data.as_bytes().to_vec()
    };

    port.write(&bytes)?;
    println!("Sent {} bytes to {}", bytes.len(), port_name);

    let mut buf = [0u8; 1024];
    std::thread::sleep(Duration::from_millis(100));
    match port.read(&mut buf) {
        Ok(n) => {
            if n > 0 {
                let received = String::from_utf8_lossy(&buf[..n]);
                println!("Received: {}", received);
            }
        }
        Err(_) => {}
    }

    port.disconnect()?;
    Ok(())
}
