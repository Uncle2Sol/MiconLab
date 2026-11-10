//! `miconlab agent` — 自动化模式（JSON 输出）。
//!
//! 阶段三重构：
//!   - 14 处 `SerialConfig::new(...).with_baud_rate(...); SerialPort::new(config); port.connect()?` 样板
//!     全部替换为 [`crate::util::open_port`] 等辅助函数
//!   - 错误路径统一通过 [`crate::util::print_agent_error`] / [`agent_error_value`] 返回 JSON
//!   - 缺失 port_name 等可预期错误不再向上传播 anyhow，而是直接输出 JSON 后返回 Ok

use anyhow::Result;
use miconlab_core::recorder::Action;
use miconlab_core::ScriptReplayer;
use std::path::Path;
use std::time::Duration;

use crate::util::{
    agent_error_value, open_port, open_port_baud_timeout, open_port_default,
    open_port_with_timeout, parse_hex, print_agent_success,
};
use crate::AgentAction;

pub fn run(port_name: Option<&str>, action: AgentAction) -> Result<()> {
    match action {
        AgentAction::ListPorts => {
            let ports = miconlab_core::SerialPort::list_ports()?;
            print_agent_success(serde_json::json!({
                "success": true,
                "ports": ports,
            }));
        }
        AgentAction::PortInfo { name } => {
            let ports = miconlab_core::SerialPort::list_ports()?;
            let port = ports.iter().find(|p| p.name == name);
            match port {
                Some(p) => print_agent_success(serde_json::json!({
                    "success": true,
                    "port": p,
                })),
                None => print_agent_success(agent_error_value(
                    &format!("Port '{}' not found", name),
                )),
            }
        }
        AgentAction::Send { data, baud, hex } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            let mut port = match open_port(port_name, baud) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };

            let bytes = if hex {
                match parse_hex(&data) {
                    Ok(b) => b,
                    Err(e) => {
                        print_agent_success(agent_error_value(&e.to_string()));
                        port.disconnect().ok();
                        return Ok(());
                    }
                }
            } else {
                data.as_bytes().to_vec()
            };
            let written = match port.write(&bytes) {
                Ok(n) => n,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    port.disconnect().ok();
                    return Ok(());
                }
            };

            print_agent_success(serde_json::json!({
                "success": true,
                "bytes_written": written,
            }));
            port.disconnect()?;
        }
        AgentAction::SendCommand {
            command,
            timeout,
            baud,
        } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            let mut port = match open_port_baud_timeout(port_name, baud, timeout) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };

            let mut cmd_bytes = command.as_bytes().to_vec();
            if !command.ends_with("\r\n") && !command.ends_with('\n') && !command.ends_with('\r') {
                cmd_bytes.extend_from_slice(b"\r\n");
            }

            if let Err(e) = port.write(&cmd_bytes) {
                print_agent_success(agent_error_value(&e.to_string()));
                port.disconnect().ok();
                return Ok(());
            }

            let mut buf = vec![0u8; 4096];
            match port.read(&mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let hex_data: String = miconlab_core::hex::format_hex_upper(data);
                    print_agent_success(serde_json::json!({
                        "success": true,
                        "bytes_read": n,
                        "data_hex": hex_data,
                        "data_text": String::from_utf8_lossy(data),
                    }));
                }
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                }
            }
            port.disconnect()?;
        }
        AgentAction::Read {
            timeout,
            max_bytes,
        } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            let mut port = match open_port_with_timeout(port_name, timeout) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };

            // 钳制上限，防止超大 max_bytes 分配耗尽内存
            let mut buf = vec![0u8; (max_bytes as usize).min(1 << 20)];
            match port.read(&mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    let hex_data: String = miconlab_core::hex::format_hex_upper(data);
                    print_agent_success(serde_json::json!({
                        "success": true,
                        "bytes_read": n,
                        "data_hex": hex_data,
                        "data_text": String::from_utf8_lossy(data),
                    }));
                }
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                }
            }
            port.disconnect()?;
        }
        AgentAction::SetDtr { value } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            let mut port = match open_port_default(port_name) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };
            match port.write_data_terminal_ready(value) {
                Ok(()) => {
                    print_agent_success(serde_json::json!({
                        "success": true,
                        "dtr": value,
                    }));
                }
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                }
            }
            port.disconnect()?;
        }
        AgentAction::SetRts { value } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            let mut port = match open_port_default(port_name) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };
            match port.write_request_to_send(value) {
                Ok(()) => {
                    print_agent_success(serde_json::json!({
                        "success": true,
                        "rts": value,
                    }));
                }
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                }
            }
            port.disconnect()?;
        }
        AgentAction::ChangeBaud { baud } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            // Note: 重新打开端口而非在已有连接上改波特率（保持原语义）
            let mut port = match open_port(port_name, baud) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };
            print_agent_success(serde_json::json!({
                "success": true,
                "port": port_name,
                "baud_rate": baud,
            }));
            port.disconnect()?;
        }
        AgentAction::RunScript { script } => {
            let Some(port_name) = port_name else {
                print_agent_success(agent_error_value("Port name required"));
                return Ok(());
            };
            let mut port = match open_port_default(port_name) {
                Ok(p) => p,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    return Ok(());
                }
            };

            let mut replayer = match ScriptReplayer::load(Path::new(&script)) {
                Ok(r) => r,
                Err(e) => {
                    print_agent_success(agent_error_value(&e.to_string()));
                    port.disconnect().ok();
                    return Ok(());
                }
            };
            replayer.start();

            let mut commands_executed = 0usize;
            while let Some(cmd) = replayer.next_command() {
                match cmd.action {
                    Action::Send => {
                        if let Some(ref data) = cmd.data {
                            // 错误路径保持 JSON 输出契约（与 load 失败分支一致），
                            // 不能用 `?` 向上传播 anyhow 错误导致非 JSON 输出
                            if let Err(e) = port.write_string(data).and_then(|_| port.write_string("\r\n")) {
                                print_agent_success(agent_error_value(&format!(
                                    "send failed at command {}: {}",
                                    commands_executed + 1,
                                    e
                                )));
                                port.disconnect().ok();
                                return Ok(());
                            }
                        }
                    }
                    Action::Wait => {
                        std::thread::sleep(Duration::from_millis(cmd.delay_ms));
                    }
                    _ => {}
                }
                commands_executed += 1;
                std::thread::sleep(replayer.get_delay());
            }

            print_agent_success(serde_json::json!({
                "success": true,
                "commands_executed": commands_executed,
            }));
            port.disconnect()?;
        }
    }

    Ok(())
}
