//! USB 直连桥接器驱动（CH341A / FT232H）
//!
//! 提供 SPI 总线直接访问能力，用于读写外挂 SPI Flash 固件。
//! 与 `port::SerialPort`（UART 桥接）不同，本模块通过 USB bulk 传输
//! 直接控制桥接器芯片的 GPIO/SPI 引脚。
//!
//! ## 支持的桥接器
//! - **CH341A**（VID=0x1A86, PID=0x5512）：并行/SPI/I2C 模式
//! - **FT232H**（VID=0x0403, PID=0x6014）：MPSSE 模式
//!
//! ## 同步包装
//! nusb 是异步 API，本模块通过内部 tokio runtime 包装为同步调用，
//! 对上层暴露同步接口，便于在 egui 帧驱动模型中使用。

use std::sync::LazyLock;
use thiserror::Error;

/// 内部 tokio runtime，用于驱动 nusb 异步 API
static USB_RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create USB runtime")
});

/// 在内部 runtime 上同步执行 future
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    USB_RUNTIME.block_on(f)
}

#[derive(Error, Debug)]
pub enum UsbBridgeError {
    #[error("USB device not found: VID={vid:04X} PID={pid:04X}")]
    DeviceNotFound { vid: u16, pid: u16 },
    #[error("Failed to open USB device: {0}")]
    OpenFailed(String),
    #[error("Failed to claim interface: {0}")]
    ClaimFailed(String),
    #[error("USB transfer failed: {0}")]
    TransferFailed(String),
    #[error("Device disconnected")]
    Disconnected,
    #[error("SPI transfer too large: {size} > {max}")]
    TransferTooLarge { size: usize, max: usize },
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    #[error("MPSSE sync failed: device did not echo 0x5A")]
    MpsseSyncFailed,
    #[error("Verify failed at offset {offset}: expected {expected:#04X}, got {got:#04X}")]
    VerifyFailed {
        offset: usize,
        expected: u8,
        got: u8,
    },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type UsbResult<T> = Result<T, UsbBridgeError>;

// ============================================================================
// 桥接器类型
// ============================================================================

/// USB 桥接器类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeKind {
    /// CH341A（VID=0x1A86 PID=0x5512）
    Ch341A,
    /// FT232H（VID=0x0403 PID=0x6014）
    Ft232H,
}

impl BridgeKind {
    pub fn vid_pid(self) -> (u16, u16) {
        match self {
            BridgeKind::Ch341A => (0x1A86, 0x5512),
            BridgeKind::Ft232H => (0x0403, 0x6014),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BridgeKind::Ch341A => "CH341A",
            BridgeKind::Ft232H => "FT232H",
        }
    }
}

/// 列举当前连接的 USB 桥接器
pub fn list_bridges() -> Vec<(BridgeKind, String)> {
    let mut result = Vec::new();
    block_on(async {
        let devices = match nusb::list_devices() {
            Ok(d) => d,
            Err(_) => return,
        };
        for dev in devices {
            for &kind in &[BridgeKind::Ch341A, BridgeKind::Ft232H] {
                let (vid, pid) = kind.vid_pid();
                if dev.vendor_id() == vid && dev.product_id() == pid {
                    let name = format!(
                        "{} (VID={:04X} PID={:04X})",
                        kind.name(),
                        vid,
                        pid
                    );
                    result.push((kind, name));
                    break;
                }
            }
        }
    });
    result
}

// ============================================================================
// 统一桥接器接口
// ============================================================================

/// USB 桥接器统一接口
pub trait UsbBridge {
    /// 桥接器类型
    fn kind(&self) -> BridgeKind;

    /// SPI 全双工传输：同时发送 `data_out` 并读取相同长度的数据
    /// 返回读取到的字节
    fn spi_xfer(&mut self, data_out: &[u8]) -> UsbResult<Vec<u8>>;

    /// 仅 SPI 输出（不关心返回数据，等价于 spi_xfer 但丢弃输入）
    fn spi_write(&mut self, data: &[u8]) -> UsbResult<()> {
        self.spi_xfer(data).map(|_| ())
    }

    /// SPI 读取（输出 0x00 作为时钟源，读取 `len` 字节）
    fn spi_read(&mut self, len: usize) -> UsbResult<Vec<u8>> {
        self.spi_xfer(&vec![0x00u8; len])
    }

    /// 设置 CS（片选）电平
    fn set_cs(&mut self, asserted: bool) -> UsbResult<()>;

    /// 关闭设备
    fn close(&mut self);
}

// ============================================================================
// CH341A 实现
// ============================================================================

/// CH341A USB 端点
#[allow(dead_code)]
mod ch341_const {
    /// CH341A bulk OUT 端点
    pub const EP_OUT: u8 = 0x02;
    /// CH341A bulk IN 端点
    pub const EP_IN: u8 = 0x82;
    /// CH341A 命令：版本号
    pub const CH341_VERSION: u8 = 0x5A;
    /// CH341A 命令：设置流模式（启用 SPI）
    pub const CH341A_CMD_SET_STREAM: u8 = 0xA9;
    /// CH341A 命令：SPI 数据流输出（带 CS 控制）
    pub const CH341A_CMD_SPI_OUT: u8 = 0xAE;
    /// CH341A 命令：SPI 数据流双向
    pub const CH341A_CMD_SPI_XFER: u8 = 0xAC;
    /// CH341A 命令：设置 GPIO（用于 CS）
    pub const CH341A_CMD_SET_GPIO: u8 = 0xA1;
    /// CH341A 默认流模式：SPI 流模式，CS 自动控制
    pub const CH341A_STREAM_SPI: u8 = 0x80;
    /// CS 引脚 bit（D7/CS0）
    pub const CS_BIT: u8 = 0x80;
}

/// CH341A USB 桥接器
pub struct Ch341ABridge {
    device: Option<nusb::Device>,
    interface: Option<nusb::Interface>,
}

impl Ch341ABridge {
    /// 打开 CH341A 设备
    pub fn open() -> UsbResult<Self> {
        let (vid, pid) = BridgeKind::Ch341A.vid_pid();
        block_on(async {
            let device = nusb::list_devices()
                .map_err(|e| UsbBridgeError::OpenFailed(format!("list: {}", e)))?
                .find(|d| d.vendor_id() == vid && d.product_id() == pid)
                .ok_or(UsbBridgeError::DeviceNotFound { vid, pid })?;

            let device = device
                .open()
                .map_err(|e| UsbBridgeError::OpenFailed(format!("open: {}", e)))?;

            // macOS/Linux 需要先 detach kernel driver
            #[cfg(target_os = "linux")]
            {
                let _ = device.detach_kernel_driver(0);
            }

            let interface = device
                .claim_interface(0)
                .map_err(|e| UsbBridgeError::ClaimFailed(format!("{}", e)))?;

            let mut bridge = Self {
                device: Some(device),
                interface: Some(interface),
            };

            // 初始化 SPI 流模式
            bridge.init_spi_mode()?;

            Ok(bridge)
        })
    }

    /// 初始化 CH341A SPI 模式
    fn init_spi_mode(&mut self) -> UsbResult<()> {
        // 设置 SPI 流模式：0xA9 0x80
        let cmd = [ch341_const::CH341A_CMD_SET_STREAM, ch341_const::CH341A_STREAM_SPI];
        self.bulk_out(&cmd)?;
        // 丢弃可能的响应
        let _ = self.bulk_in(8, std::time::Duration::from_millis(100));
        Ok(())
    }

    /// bulk OUT 传输
    fn bulk_out(&mut self, data: &[u8]) -> UsbResult<usize> {
        let interface = self
            .interface
            .as_mut()
            .ok_or(UsbBridgeError::Disconnected)?;
        let sent_len = data.len();
        block_on(async {
            let completion = interface
                .bulk_out(ch341_const::EP_OUT, data.to_vec())
                .await;
            completion.status.map_err(|e| {
                UsbBridgeError::TransferFailed(format!("bulk_out: {:?}", e))
            })?;
            // 假设全部发送成功（CH341A bulk 传输要么全成功要么失败）
            let _ = sent_len;
            Ok(sent_len)
        })
    }

    /// bulk IN 传输
    fn bulk_in(&mut self, len: usize, timeout: std::time::Duration) -> UsbResult<Vec<u8>> {
        let interface = self
            .interface
            .as_mut()
            .ok_or(UsbBridgeError::Disconnected)?;
        block_on(async {
            use nusb::transfer::RequestBuffer;
            let buf = RequestBuffer::new(len);
            let result = tokio::time::timeout(
                timeout,
                interface.bulk_in(ch341_const::EP_IN, buf),
            )
            .await;
            let completion = match result {
                Ok(c) => c,
                Err(_) => {
                    return Err(UsbBridgeError::TransferFailed("bulk_in timeout".to_string()));
                }
            };
            completion.status.map_err(|e| {
                UsbBridgeError::TransferFailed(format!("bulk_in: {:?}", e))
            })?;
            Ok(completion.data)
        })
    }
}

impl UsbBridge for Ch341ABridge {
    fn kind(&self) -> BridgeKind {
        BridgeKind::Ch341A
    }

    fn spi_xfer(&mut self, data_out: &[u8]) -> UsbResult<Vec<u8>> {
        if data_out.is_empty() {
            return Ok(Vec::new());
        }
        if data_out.len() > 0x1000 {
            return Err(UsbBridgeError::TransferTooLarge {
                size: data_out.len(),
                max: 0x1000,
            });
        }

        // CH341A SPI 流命令：0xAC <len_hi> <len_lo> <data...>
        // 0xAC = 流模式 SPI 双向传输，CS 自动拉低
        let mut cmd = Vec::with_capacity(data_out.len() + 3);
        cmd.push(ch341_const::CH341A_CMD_SPI_XFER);
        let len = data_out.len() as u16;
        cmd.push((len & 0xFF) as u8); // 长度低字节（CH341A 文档为小端）
        cmd.push((len >> 8) as u8);
        cmd.extend_from_slice(data_out);

        self.bulk_out(&cmd)?;
        // CH341A 返回的数据前 3 字节为流头（len_lo len_hi status），之后是实际读到的数据
        let resp = self.bulk_in(data_out.len() + 8, std::time::Duration::from_millis(500))?;

        // 解析：前 2 字节是长度回显，第 3 字节是状态，剩余是数据
        if resp.len() < 3 {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "response too short: {} bytes",
                resp.len()
            )));
        }
        // 实际数据从第 3 字节开始（部分固件从第 2 字节开始）
        // 经验值：CH341A 流模式返回 0x00 + 0x00 + data
        let data_start = if resp.len() >= data_out.len() + 3 { 3 } else { 2 };
        let actual = resp.len() - data_start;
        let take = actual.min(data_out.len());
        Ok(resp[data_start..data_start + take].to_vec())
    }

    fn set_cs(&mut self, asserted: bool) -> UsbResult<()> {
        // CH341A 通过 SET_GPIO 命令控制 CS（D7）
        // 0xA1 <data> <dir>
        let data = if asserted { 0x00 } else { ch341_const::CS_BIT };
        let cmd = [ch341_const::CH341A_CMD_SET_GPIO, data, ch341_const::CS_BIT];
        self.bulk_out(&cmd)?;
        let _ = self.bulk_in(8, std::time::Duration::from_millis(100));
        Ok(())
    }

    fn close(&mut self) {
        // 释放 interface 和 device（Drop 会自动处理）
        self.interface = None;
        self.device = None;
    }
}

impl Drop for Ch341ABridge {
    fn drop(&mut self) {
        self.close();
    }
}

// ============================================================================
// FT232H MPSSE 实现
// ============================================================================

/// FT232H MPSSE 命令码
#[allow(dead_code)]
mod mpsse_const {
    /// 设置低字节 GPIO 方向/值
    pub const SET_BITS_LOW: u8 = 0x80;
    /// 读取低字节 GPIO
    pub const GET_BITS_LOW: u8 = 0x81;
    /// 设置时钟分频
    pub const SET_TCK_DIVISOR: u8 = 0x86;
    /// 立即返回数据
    pub const SEND_IMMEDIATE: u8 = 0x87;
    /// 禁用回环
    pub const LOOPBACK_END: u8 = 0x85;
    /// 时钟输出（负沿，MSB first）
    pub const CLOCK_DATA_OUT: u8 = 0x11;
    /// 时钟输入（负沿，MSB first）
    pub const CLOCK_DATA_IN: u8 = 0x24;
    /// 时钟读写（负沿，MSB first，全双工）
    pub const CLOCK_DATA_RDWR: u8 = 0x31;
    /// MPSSE 同步字节（0xAA，期望返回 0x55 0xAA 0x00）
    pub const BAD_COMMAND: u8 = 0xAA;

    /// CS 引脚（CDBUS3/GPIOL3）
    pub const CS_BIT: u8 = 0x08;
    /// SCLK 引脚（CDBUS0）
    pub const SCLK_BIT: u8 = 0x01;
    /// MOSI 引脚（CDBUS1）
    pub const MOSI_BIT: u8 = 0x02;
    /// MISO 引脚（CDBUS2 输入）
    pub const MISO_BIT: u8 = 0x04;

    /// 默认 GPIO 方向：CS/SCLK/MOSI 输出，MISO 输入
    pub const GPIO_DIR_DEFAULT: u8 = CS_BIT | SCLK_BIT | MOSI_BIT;
    /// 默认 GPIO 值：CS 默认高（释放），其他低
    pub const GPIO_VAL_CS_HIGH: u8 = CS_BIT;
}

/// FT232H USB 端点
mod ft232h_const {
    pub const EP_OUT: u8 = 0x02;
    pub const EP_IN: u8 = 0x81;
    /// FT232H 接口 A（MPSSE 模式）
    pub const INTERFACE_NUM: u8 = 0;
    /// FTDI 命令：重置
    pub const FTDI_RESET: u16 = 0;
    /// FTDI 命令：设置 bitmode
    pub const FTDI_SET_BITMODE: u16 = 0x0B;
    /// MPSSE 模式 bitmode 码
    pub const BITMODE_MPSSE: u8 = 0x02;
    /// 清空缓冲区
    pub const FTDI_RESET_PURGE_RX: u16 = 1;
    pub const FTDI_RESET_PURGE_TX: u16 = 2;
}

/// FT232H USB 桥接器（MPSSE 模式）
pub struct Ft232HBridge {
    device: Option<nusb::Device>,
    interface: Option<nusb::Interface>,
}

impl Ft232HBridge {
    /// 打开 FT232H 设备
    pub fn open() -> UsbResult<Self> {
        let (vid, pid) = BridgeKind::Ft232H.vid_pid();
        block_on(async {
            let device = nusb::list_devices()
                .map_err(|e| UsbBridgeError::OpenFailed(format!("list: {}", e)))?
                .find(|d| d.vendor_id() == vid && d.product_id() == pid)
                .ok_or(UsbBridgeError::DeviceNotFound { vid, pid })?;

            let device = device
                .open()
                .map_err(|e| UsbBridgeError::OpenFailed(format!("open: {}", e)))?;

            #[cfg(target_os = "linux")]
            {
                let _ = device.detach_kernel_driver(ft232h_const::INTERFACE_NUM);
            }

            let interface = device
                .claim_interface(ft232h_const::INTERFACE_NUM)
                .map_err(|e| UsbBridgeError::ClaimFailed(format!("{}", e)))?;

            let mut bridge = Self {
                device: Some(device),
                interface: Some(interface),
            };

            bridge.init_mpsse()?;
            Ok(bridge)
        })
    }

    /// 初始化 MPSSE 模式
    fn init_mpsse(&mut self) -> UsbResult<()> {
        // 1. 复位 USB
        self.control_write(ft232h_const::FTDI_RESET, 0)?;
        // 2. 清空 RX/TX 缓冲区
        self.control_write(ft232h_const::FTDI_RESET_PURGE_RX, 0)?;
        self.control_write(ft232h_const::FTDI_RESET_PURGE_TX, 0)?;
        // 3. 设置 MPSSE bitmode：value=0x02 << 8 | 0x02, index=interface
        let bitmode_value = (ft232h_const::BITMODE_MPSSE as u16) << 8 | 0x02;
        self.control_write(ft232h_const::FTDI_SET_BITMODE, bitmode_value)?;
        // 4. 设置时钟分频（默认 ~1MHz: divisor=2 → 6MHz/((2+1)*2)=1MHz）
        let set_clk = vec![
            mpsse_const::SET_TCK_DIVISOR,
            0x02,
            0x00, // divisor=2
            mpsse_const::LOOPBACK_END, // 禁用回环
            // 设置 GPIO 默认方向（CS/SCLK/MOSI 输出，MISO 输入），CS 高电平
            mpsse_const::SET_BITS_LOW,
            mpsse_const::GPIO_VAL_CS_HIGH,
            mpsse_const::GPIO_DIR_DEFAULT,
        ];
        self.bulk_out(&set_clk)?;
        // 5. MPSSE 同步检查：发送 BAD_COMMAND，期望返回 0xFA 0xAA 0x55 0xAA 0x00
        let sync = vec![mpsse_const::BAD_COMMAND];
        self.bulk_out(&sync)?;
        let resp = self.bulk_in(8, std::time::Duration::from_millis(200))?;
        // 检查响应中是否包含 0xFA 0xAA 0x55
        let mut ok = false;
        for i in 0..resp.len().saturating_sub(2) {
            if resp[i] == 0xFA && resp[i + 1] == 0xAA && resp[i + 2] == 0x55 {
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(UsbBridgeError::MpsseSyncFailed);
        }
        Ok(())
    }

    /// FTDI 控制传输（vendor request OUT）
    ///
    /// 平台差异：nusb 在 Unix 上允许在 `Device` 上直接发控制传输，
    /// 但 Windows（WinUSB）必须在已认领的 `Interface` 上发。
    /// 两条分支语义相同（同一个 vendor request），不是两套协议。
    fn control_write(&mut self, request: u16, value: u16) -> UsbResult<()> {
        let req = nusb::transfer::ControlOut {
            control_type: nusb::transfer::ControlType::Vendor,
            recipient: nusb::transfer::Recipient::Device,
            request: request as u8,
            value,
            index: ft232h_const::INTERFACE_NUM as u16,
            data: &[][..],
        };

        #[cfg(not(target_os = "windows"))]
        let completion = {
            let device = self.device.as_ref().ok_or(UsbBridgeError::Disconnected)?;
            block_on(device.control_out(req))
        };

        #[cfg(target_os = "windows")]
        let completion = {
            let interface = self
                .interface
                .as_ref()
                .ok_or(UsbBridgeError::Disconnected)?;
            block_on(interface.control_out(req))
        };

        completion.status.map_err(|e| {
            UsbBridgeError::TransferFailed(format!("control_out(req={}): {:?}", request, e))
        })?;
        Ok(())
    }

    /// bulk OUT
    fn bulk_out(&mut self, data: &[u8]) -> UsbResult<usize> {
        let interface = self
            .interface
            .as_mut()
            .ok_or(UsbBridgeError::Disconnected)?;
        let sent_len = data.len();
        block_on(async {
            let completion = interface
                .bulk_out(ft232h_const::EP_OUT, data.to_vec())
                .await;
            completion.status.map_err(|e| {
                UsbBridgeError::TransferFailed(format!("bulk_out: {:?}", e))
            })?;
            Ok(sent_len)
        })
    }

    /// bulk IN（带超时）
    fn bulk_in(&mut self, len: usize, timeout: std::time::Duration) -> UsbResult<Vec<u8>> {
        let interface = self
            .interface
            .as_mut()
            .ok_or(UsbBridgeError::Disconnected)?;
        block_on(async {
            use nusb::transfer::RequestBuffer;
            let buf = RequestBuffer::new(len);
            let result = tokio::time::timeout(
                timeout,
                interface.bulk_in(ft232h_const::EP_IN, buf),
            )
            .await;
            let completion = match result {
                Ok(c) => c,
                Err(_) => {
                    return Err(UsbBridgeError::TransferFailed("bulk_in timeout".to_string()));
                }
            };
            completion.status.map_err(|e| {
                UsbBridgeError::TransferFailed(format!("bulk_in: {:?}", e))
            })?;
            Ok(completion.data)
        })
    }
}

impl UsbBridge for Ft232HBridge {
    fn kind(&self) -> BridgeKind {
        BridgeKind::Ft232H
    }

    fn spi_xfer(&mut self, data_out: &[u8]) -> UsbResult<Vec<u8>> {
        if data_out.is_empty() {
            return Ok(Vec::new());
        }
        if data_out.len() > 0x10000 {
            return Err(UsbBridgeError::TransferTooLarge {
                size: data_out.len(),
                max: 0x10000,
            });
        }

        // 拉低 CS
        self.set_cs(true)?;

        // 构建 MPSSE 双向传输命令
        let len = (data_out.len() - 1) as u16;
        let mut cmd = Vec::with_capacity(3 + data_out.len() + 1);
        cmd.push(mpsse_const::CLOCK_DATA_RDWR);
        cmd.push((len & 0xFF) as u8);
        cmd.push((len >> 8) as u8);
        cmd.extend_from_slice(data_out);
        cmd.push(mpsse_const::SEND_IMMEDIATE);

        self.bulk_out(&cmd)?;

        // 读取返回数据
        let resp = self.bulk_in(data_out.len() + 4, std::time::Duration::from_millis(500))?;

        // 释放 CS
        self.set_cs(false)?;

        if resp.len() < data_out.len() {
            return Err(UsbBridgeError::InvalidResponse(format!(
                "MPSSE response too short: {} < {}",
                resp.len(),
                data_out.len()
            )));
        }
        // MPSSE 不返回命令 echo，数据从第 0 字节开始
        Ok(resp[..data_out.len()].to_vec())
    }

    fn set_cs(&mut self, asserted: bool) -> UsbResult<()> {
        // 设置 GPIO 低字节：CS=D3，其他保持默认
        let value = if asserted {
            // CS 拉低（清除 CS_BIT），其他输出位保持低
            0x00
        } else {
            // CS 拉高
            mpsse_const::CS_BIT
        };
        let cmd = [
            mpsse_const::SET_BITS_LOW,
            value,
            mpsse_const::GPIO_DIR_DEFAULT,
        ];
        self.bulk_out(&cmd)?;
        Ok(())
    }

    fn close(&mut self) {
        // 释放前恢复 GPIO 默认状态
        if self.interface.is_some() {
            let _ = self.set_cs(false);
        }
        self.interface = None;
        self.device = None;
    }
}

impl Drop for Ft232HBridge {
    fn drop(&mut self) {
        self.close();
    }
}

// ============================================================================
// 工厂函数
// ============================================================================

/// 根据桥接器类型打开对应设备
pub fn open_bridge(kind: BridgeKind) -> UsbResult<Box<dyn UsbBridge>> {
    match kind {
        BridgeKind::Ch341A => Ok(Box::new(Ch341ABridge::open()?)),
        BridgeKind::Ft232H => Ok(Box::new(Ft232HBridge::open()?)),
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_kind_vid_pid() {
        let (v, p) = BridgeKind::Ch341A.vid_pid();
        assert_eq!(v, 0x1A86);
        assert_eq!(p, 0x5512);

        let (v, p) = BridgeKind::Ft232H.vid_pid();
        assert_eq!(v, 0x0403);
        assert_eq!(p, 0x6014);
    }

    #[test]
    fn test_bridge_kind_name() {
        assert_eq!(BridgeKind::Ch341A.name(), "CH341A");
        assert_eq!(BridgeKind::Ft232H.name(), "FT232H");
    }

    #[test]
    fn test_list_bridges_does_not_panic() {
        // 不应 panic，即使没有 USB 设备连接
        let _ = list_bridges();
    }

    #[test]
    fn test_usb_runtime_lazy_init() {
        // 验证 runtime 可正常初始化
        let v = block_on(async { 42 });
        assert_eq!(v, 42);
    }

    #[test]
    fn test_ch341_constants() {
        assert_eq!(ch341_const::EP_OUT, 0x02);
        assert_eq!(ch341_const::EP_IN, 0x82);
        assert_eq!(ch341_const::CS_BIT, 0x80);
    }

    #[test]
    fn test_mpsse_constants() {
        assert_eq!(mpsse_const::SET_BITS_LOW, 0x80);
        assert_eq!(mpsse_const::CLOCK_DATA_RDWR, 0x31);
        assert_eq!(mpsse_const::CS_BIT, 0x08);
        assert_eq!(mpsse_const::SCLK_BIT, 0x01);
        assert_eq!(mpsse_const::MOSI_BIT, 0x02);
        assert_eq!(mpsse_const::MISO_BIT, 0x04);
    }
}
