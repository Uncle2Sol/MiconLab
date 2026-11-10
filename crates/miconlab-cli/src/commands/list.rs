//! `miconlab list` — 列出可用串口。

use anyhow::Result;
use miconlab_core::SerialPort;

pub fn run(format: &str) -> Result<()> {
    let ports = SerialPort::list_ports()?;

    match format {
        "json" => {
            // JSON 格式始终输出有效 JSON 数组（即使为空）
            let json = serde_json::to_string_pretty(&ports)?;
            println!("{}", json);
        }
        _ => {
            if ports.is_empty() {
                println!("No serial ports found");
                return Ok(());
            }
            println!("Available serial ports:");
            println!("{:<20} {:<30} {:<10}", "Port", "Description", "VID:PID");
            println!("{}", "-".repeat(60));
            for port in &ports {
                let vid_pid = match (port.vid, port.pid) {
                    (Some(vid), Some(pid)) => format!("{:04X}:{:04X}", vid, pid),
                    _ => "N/A".to_string(),
                };
                println!(
                    "{:<20} {:<30} {:<10}",
                    port.name,
                    port.description.as_deref().unwrap_or("N/A"),
                    vid_pid
                );
            }
        }
    }

    Ok(())
}
