use crate::config::{DataBits, FlowControl, Parity, SerialConfig, StopBits};
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PortError {
    #[error("Port not found: {0}")]
    PortNotFound(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Read error: {0}")]
    ReadError(String),
    #[error("Write error: {0}")]
    WriteError(String),
    #[error("Port is not connected")]
    NotConnected,
    #[error("IO error: {0}")]
    IoError(String),
}

pub type PortResult<T> = Result<T, PortError>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SerialPortInfo {
    pub name: String,
    pub description: Option<String>,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

pub struct SerialPort {
    port: Option<Box<dyn serialport::SerialPort>>,
    config: SerialConfig,
    is_connected: bool,
}

impl SerialPort {
    pub fn new(config: SerialConfig) -> Self {
        Self {
            port: None,
            config,
            is_connected: false,
        }
    }

    pub fn list_ports() -> PortResult<Vec<SerialPortInfo>> {
        // 1. 先用 serialport::available_ports() 获取端口列表（含 VID/PID 等元数据）
        let serialport_ports = serialport::available_ports().unwrap_or_default();

        let result: Vec<SerialPortInfo> = serialport_ports
            .into_iter()
            .map(|p| {
                let (vid, pid, manufacturer, serial_number, description) = match &p.port_type {
                    serialport::SerialPortType::UsbPort(info) => (
                        Some(info.vid),
                        Some(info.pid),
                        info.manufacturer.clone(),
                        info.serial_number.clone(),
                        Some(format!("USB Device {:04X}:{:04X}", info.vid, info.pid)),
                    ),
                    _ => (None, None, None, None, None),
                };

                SerialPortInfo {
                    // 统一写法（Windows 去掉 `\\.\`，macOS tty.* → cu.*）
                    name: crate::platform::normalize_port_name(&p.port_name),
                    description,
                    manufacturer,
                    serial_number,
                    vid,
                    pid,
                }
            })
            .collect();

        // 2. 规范化后去重：macOS 上同一设备会同时枚举 `tty.*` 与 `cu.*`，
        //    规范化成 `cu.*` 后成为重名；保留带 USB 元数据的那一条。
        let mut merged: Vec<SerialPortInfo> = Vec::with_capacity(result.len());
        for p in result {
            if let Some(existing) = merged.iter_mut().find(|e| e.name == p.name) {
                if existing.vid.is_none() && p.vid.is_some() {
                    *existing = p;
                }
                continue;
            }
            merged.push(p);
        }
        let mut result = merged;

        // 3. 文件系统兜底扫描：仅 macOS/Linux 需要
        //    （Windows 上 available_ports() 每次走 SetupAPI，实时可靠）
        let fs_ports = crate::platform::scan_system_ports();

        // 4. 合并：添加系统枚举遗漏的端口
        let mut existing_names: std::collections::HashSet<String> =
            result.iter().map(|p| p.name.clone()).collect();

        for fs_port in fs_ports {
            let normalized = crate::platform::normalize_port_name(&fs_port.name);
            if existing_names.insert(normalized.clone()) {
                result.push(SerialPortInfo {
                    name: normalized,
                    ..fs_port
                });
            }
        }

        // 5. 固件仿真器（PTY，仅 Unix；仅设备工作区构建）
        #[cfg(feature = "device-workspaces")]
        {
            for sim in simulator_ports() {
                if !result.iter().any(|p| p.name == sim.name) {
                    result.push(sim);
                }
            }
        }

        // 6. 排序：Windows 按 COM 号自然序（COM2 在 COM10 前），Unix 按路径
        result.sort_by(|a, b| {
            crate::platform::port_sort_key(&a.name).cmp(&crate::platform::port_sort_key(&b.name))
        });

        Ok(result)
    }

    pub fn connect(&mut self) -> PortResult<()> {
        if self.is_connected {
            return Ok(());
        }

        // PTY 虚拟串口（如 FA008 固件仿真器）不支持 macOS 的 IOSSIOSPEED
        // 波特率 ioctl，会返回 ENOTTY("Not a typewriter")。PTY 上波特率本身
        // 无意义，serialport 在 baud_rate=0 时会跳过该调用，据此实现回退。
        let open_with_baud = |baud: u32| {
            let builder = serialport::new(&self.config.port_name, baud)
                .data_bits(match self.config.data_bits {
                    DataBits::Five => serialport::DataBits::Five,
                    DataBits::Six => serialport::DataBits::Six,
                    DataBits::Seven => serialport::DataBits::Seven,
                    DataBits::Eight => serialport::DataBits::Eight,
                })
                .stop_bits(match self.config.stop_bits {
                    StopBits::One => serialport::StopBits::One,
                    StopBits::Two => serialport::StopBits::Two,
                })
                .parity(match self.config.parity {
                    Parity::None => serialport::Parity::None,
                    Parity::Odd => serialport::Parity::Odd,
                    Parity::Even => serialport::Parity::Even,
                })
                .flow_control(match self.config.flow_control {
                    FlowControl::None => serialport::FlowControl::None,
                    FlowControl::Software => serialport::FlowControl::Software,
                    FlowControl::Hardware => serialport::FlowControl::Hardware,
                })
                .timeout(Duration::from_millis(self.config.timeout_ms));

            // `exclusive` 只有 Unix 实现；Windows 的共享语义由驱动裁决，
            // 该分支收敛在 platform 层，业务代码不再散落 cfg。
            #[cfg(unix)]
            let builder = builder.exclusive(false); // 允许 esptool 等外部工具同时打开端口

            builder.open()
        };

        let mut port = match open_with_baud(self.config.baud_rate) {
            Ok(p) => p,
            Err(e) if e.to_string().contains("Not a typewriter") => {
                open_with_baud(0).map_err(|e| {
                    PortError::ConnectionFailed(format!(
                        "Failed to open {}: {}",
                        self.config.port_name, e
                    ))
                })?
            }
            Err(e) => {
                return Err(PortError::ConnectionFailed(format!(
                    "Failed to open {}: {}",
                    self.config.port_name, e
                )))
            }
        };

        // Assert DTR on connect (many devices require this;
        // PTY 不支持会静默失败，属预期)
        let _ = port.write_data_terminal_ready(true);
        // RTS 同步置位（对齐 Web Serial API 打开端口的默认行为：DTR+RTS 均 true；
        // 部分设备/桥接用 RTS 判断主机在线。对不检测 RTS 的设备无副作用；
        // 硬件流控模式由 serialport crate 在 open 时自行管理线路状态）
        let _ = port.write_request_to_send(true);

        self.port = Some(port);
        self.is_connected = true;
        Ok(())
    }

    pub fn disconnect(&mut self) -> PortResult<()> {
        self.port = None;
        self.is_connected = false;
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected
    }

    pub fn read(&mut self, buf: &mut [u8]) -> PortResult<usize> {
        let port = self.port.as_mut().ok_or(PortError::NotConnected)?;
        port.read(buf).map_err(|e| PortError::ReadError(e.to_string()))
    }

    pub fn write(&mut self, buf: &[u8]) -> PortResult<usize> {
        let port = self.port.as_mut().ok_or(PortError::NotConnected)?;
        port.write(buf).map_err(|e| PortError::WriteError(e.to_string()))
    }

    pub fn write_string(&mut self, s: &str) -> PortResult<usize> {
        self.write(s.as_bytes())
    }

    pub fn bytes_to_read(&self) -> PortResult<u32> {
        let port = self.port.as_ref().ok_or(PortError::NotConnected)?;
        Ok(port.bytes_to_read().unwrap_or(0))
    }

    pub fn bytes_to_write(&self) -> PortResult<u32> {
        let port = self.port.as_ref().ok_or(PortError::NotConnected)?;
        Ok(port.bytes_to_write().unwrap_or(0))
    }

    pub fn clear_buffer(&self, buffer: ClearBuffer) -> PortResult<()> {
        let port = self.port.as_ref().ok_or(PortError::NotConnected)?;
        let buf = match buffer {
            ClearBuffer::Input => serialport::ClearBuffer::Input,
            ClearBuffer::Output => serialport::ClearBuffer::Output,
            ClearBuffer::All => serialport::ClearBuffer::All,
        };
        port.clear(buf).map_err(|e| PortError::IoError(e.to_string()))
    }

    pub fn config(&self) -> &SerialConfig {
        &self.config
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> PortResult<()> {
        let port = self.port.as_mut().ok_or(PortError::NotConnected)?;
        port.set_timeout(timeout)
            .map_err(|e| PortError::IoError(e.to_string()))
    }

    pub fn write_data_terminal_ready(&mut self, dtr: bool) -> PortResult<()> {
        let port = self.port.as_mut().ok_or(PortError::NotConnected)?;
        port.write_data_terminal_ready(dtr)
            .map_err(|e| PortError::IoError(e.to_string()))
    }

    pub fn write_request_to_send(&mut self, rts: bool) -> PortResult<()> {
        let port = self.port.as_mut().ok_or(PortError::NotConnected)?;
        port.write_request_to_send(rts)
            .map_err(|e| PortError::IoError(e.to_string()))
    }

    /// Take ownership of the inner port handle (for passing to reader thread).
    /// After this call, the SerialPort wrapper is disconnected.
    pub fn take_port(&mut self) -> Option<Box<dyn serialport::SerialPort>> {
        self.is_connected = false;
        self.port.take()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ClearBuffer {
    Input,
    Output,
    All,
}

#[cfg(feature = "device-workspaces")]
include!("port_firmware_sim.rs");


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_ports() {
        let ports = SerialPort::list_ports();
        assert!(ports.is_ok());
    }

    #[test]
    fn test_filesystem_scan_finds_ports() {
        // 平台兜底扫描应能发现设备（至少不 panic）
        let ports = crate::platform::scan_system_ports();
        // 在 macOS CI 上可能没有串口设备，但函数不应 panic
        let _ = ports.len();
    }

    #[test]
    fn test_port_names_normalized() {
        // 枚举结果不应保留平台专属写法
        let ports = SerialPort::list_ports().unwrap_or_default();
        for p in &ports {
            assert!(
                !p.name.starts_with("/dev/tty."),
                "macOS tty.* 应被规范化为 cu.*: {}",
                p.name
            );
            assert!(
                !p.name.starts_with(r"\\.\"),
                "Windows 端口名应去掉 \\\\.\\ 前缀: {}",
                p.name
            );
        }
    }

    /// 设备 PTY 仿真器端到端：仿真器运行中（/tmp/fa008-sim/tty 存在）时，
    /// 验证 端口枚举 + serialport 真实打开 + 文本查询 DEV_INFO 应答。
    /// 仿真器未运行时跳过（CI 环境无仿真器）。
    /// 仅设备工作区构建包含该测试（公开版不编译仿真器模块）。
    #[test]
    #[cfg(feature = "device-workspaces")]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn test_fa008_sim_port_listed_and_connectable() {
        if std::fs::metadata(FA008_SIM_PORT).is_err() {
            eprintln!("skip: FA008 simulator not running ({})", FA008_SIM_PORT);
            return;
        }
        // 1. 端口枚举包含仿真器
        let ports = SerialPort::list_ports().unwrap();
        assert!(
            ports.iter().any(|p| p.name == FA008_SIM_PORT),
            "sim port not listed"
        );

        // 2. 真实打开（serialport 对 PTY 的兼容性验证；波特率对 PTY 无效，任意值）
        let cfg = crate::config::SerialConfig::new(FA008_SIM_PORT).with_baud_rate(115200);
        let mut sp = SerialPort::new(cfg);
        sp.connect().expect("open PTY sim port");

        // 3. 文本查询 → DEV_INFO JSON（含 "XF008"）
        let _ = sp.write_string("AT\r\n");
        let mut got = Vec::new();
        let mut buf = [0u8; 512];
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Ok(n) = sp.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
            }
            if got.windows(5).any(|w| w == b"XF008") {
                break;
            }
        }
        let _ = sp.disconnect();
        assert!(
            got.windows(5).any(|w| w == b"XF008"),
            "expect DEV_INFO JSON with XF008, got: {:?}",
            String::from_utf8_lossy(&got)
        );
    }

    #[test]
    fn test_list_ports_merges_sources() {
        // list_ports() 应合并 serialport 和文件系统扫描的结果
        let ports = SerialPort::list_ports().unwrap();

        // 验证没有重复端口名
        let mut names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        let duplicates: Vec<&str> = names
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0])
            .collect();
        assert!(duplicates.is_empty(), "Duplicate port names: {:?}", duplicates);
    }

    #[test]
    fn test_new_port() {
        let config = SerialConfig::new("COM1");
        let port = SerialPort::new(config);
        assert!(!port.is_connected());
    }

    #[test]
    fn test_port_config() {
        let config = SerialConfig::new("/dev/ttyUSB0").with_baud_rate(9600);
        let port = SerialPort::new(config);
        assert_eq!(port.config().port_name, "/dev/ttyUSB0");
        assert_eq!(port.config().baud_rate, 9600);
    }
}
