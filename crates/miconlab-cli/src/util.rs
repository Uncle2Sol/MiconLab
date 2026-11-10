//! CLI 公共辅助：串口打开、hex 解析、agent 模式 JSON 输出。
//!
//! 阶段三重构目标：
//!   - 消除 14 处 `SerialConfig::new(...).with_baud_rate(...); SerialPort::new(config); port.connect()?` 样板
//!   - `parse_hex` 修复奇数长度 panic 风险
//!   - agent 模式错误路径统一返回 JSON（避免 anyhow 错误信息裸打印破坏自动化）

use anyhow::{anyhow, Result};
use miconlab_core::config::{DataBits, FlowControl, Parity, StopBits};
use miconlab_core::{SerialConfig, SerialPort};
use std::str::FromStr;

// ----------------------------------------------------------------------------
// 串口打开辅助
// ----------------------------------------------------------------------------

/// 打开串口（仅指定端口名与波特率，使用默认配置）。
///
/// 替代 CLI 中重复 14 次的：
/// ```ignore
/// let config = SerialConfig::new(port_name).with_baud_rate(baud);
/// let mut port = SerialPort::new(config);
/// port.connect()?;
/// ```
pub fn open_port(port_name: &str, baud: u32) -> Result<SerialPort> {
    let config = SerialConfig::new(port_name).with_baud_rate(baud);
    let mut port = SerialPort::new(config);
    port.connect()?;
    Ok(port)
}

/// 打开串口（仅端口名，使用完全默认配置）。
pub fn open_port_default(port_name: &str) -> Result<SerialPort> {
    let config = SerialConfig::new(port_name);
    let mut port = SerialPort::new(config);
    port.connect()?;
    Ok(port)
}

/// 打开串口（指定端口名、波特率、超时）。
pub fn open_port_baud_timeout(
    port_name: &str,
    baud: u32,
    timeout_ms: u64,
) -> Result<SerialPort> {
    let config = SerialConfig::new(port_name)
        .with_baud_rate(baud)
        .with_timeout(timeout_ms);
    let mut port = SerialPort::new(config);
    port.connect()?;
    Ok(port)
}

/// 打开串口（指定端口名与超时，无波特率覆盖）。
pub fn open_port_with_timeout(port_name: &str, timeout_ms: u64) -> Result<SerialPort> {
    let config = SerialConfig::new(port_name).with_timeout(timeout_ms);
    let mut port = SerialPort::new(config);
    port.connect()?;
    Ok(port)
}

// ----------------------------------------------------------------------------
// 枚举字符串解析辅助（基于 core 的 FromStr 实现）
// ----------------------------------------------------------------------------

pub fn parse_data_bits(s: &str) -> Result<DataBits> {
    DataBits::from_str(s).map_err(|e| anyhow!("{}", e))
}

pub fn parse_stop_bits(s: &str) -> Result<StopBits> {
    StopBits::from_str(s).map_err(|e| anyhow!("{}", e))
}

pub fn parse_parity(s: &str) -> Result<Parity> {
    Parity::from_str(s).map_err(|e| anyhow!("{}", e))
}

pub fn parse_flow_control(s: &str) -> Result<FlowControl> {
    FlowControl::from_str(s).map_err(|e| anyhow!("{}", e))
}

// ----------------------------------------------------------------------------
// Hex 解析
// ----------------------------------------------------------------------------

/// 解析 16 进制字符串为字节数组。
///
/// 委托到 [`miconlab_core::hex::parse_hex`] 统一实现。
/// 详见该函数文档。
pub fn parse_hex(hex_str: &str) -> Result<Vec<u8>> {
    miconlab_core::hex::parse_hex(hex_str)
        .map_err(|e| anyhow!("{}", e))
}

// ----------------------------------------------------------------------------
// Agent 模式 JSON 输出
// ----------------------------------------------------------------------------

/// 输出 agent 模式的成功 JSON。
pub fn print_agent_success(value: serde_json::Value) {
    match serde_json::to_string_pretty(&value) {
        Ok(s) => println!("{}", s),
        Err(e) => eprintln!("failed to serialize agent success output: {}", e),
    }
}

/// 构造 agent 错误 JSON 值（不打印，便于调用方进一步处理）。
///
/// 调用方通常配合 [`print_agent_success`] 使用：
/// ```ignore
/// print_agent_success(agent_error_value("port name required"));
/// ```
pub fn agent_error_value(error: &str) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "error": error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_with_spaces() {
        let bytes = parse_hex("48 65 6C 6C 6F").unwrap();
        assert_eq!(bytes, b"Hello");
    }

    #[test]
    fn test_parse_hex_no_spaces() {
        let bytes = parse_hex("48656C6C6F").unwrap();
        assert_eq!(bytes, b"Hello");
    }

    #[test]
    fn test_parse_hex_0x_prefix() {
        let bytes = parse_hex("0x48 0x65").unwrap();
        assert_eq!(bytes, b"He");
    }

    #[test]
    fn test_parse_hex_odd_length_rejected() {
        // 修复点：奇数长度不再 panic，而是返回明确错误
        let result = parse_hex("486");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("odd length"));
    }

    #[test]
    fn test_parse_hex_empty() {
        let bytes = parse_hex("").unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_parse_hex_invalid_chars() {
        let result = parse_hex("ZZ");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("offset 0"));
    }

    #[test]
    fn test_parse_hex_mixed_case() {
        let bytes = parse_hex("4A 4b 4C").unwrap();
        assert_eq!(bytes, [0x4A, 0x4B, 0x4C]);
    }

    #[test]
    fn test_parse_data_bits_ok() {
        assert_eq!(parse_data_bits("8").unwrap(), DataBits::Eight);
        assert_eq!(parse_data_bits("seven").unwrap(), DataBits::Seven);
    }

    #[test]
    fn test_parse_data_bits_err() {
        assert!(parse_data_bits("9").is_err());
    }

    #[test]
    fn test_parse_parity_aliases() {
        assert_eq!(parse_parity("n").unwrap(), Parity::None);
        assert_eq!(parse_parity("ODD").unwrap(), Parity::Odd);
        assert_eq!(parse_parity("even").unwrap(), Parity::Even);
    }

    #[test]
    fn test_parse_flow_control_aliases() {
        assert_eq!(parse_flow_control("xonxoff").unwrap(), FlowControl::Software);
        assert_eq!(parse_flow_control("rtscts").unwrap(), FlowControl::Hardware);
    }

    #[test]
    fn test_agent_error_value_structure() {
        let v = agent_error_value("boom");
        assert_eq!(v["success"], serde_json::json!(false));
        assert_eq!(v["error"], serde_json::json!("boom"));
    }
}
