//! 项目统一错误类型模块。
//!
//! 各子模块（protocol、usb_bridge、config、port、file_transfer 等）各自维护
//! 自身的错误类型以保持向后兼容。`CoreError` 仅作为聚合类型，供未来新代码
//! 统一错误处理使用，现有代码无需迁移。
//!
//! ## 使用示例
//! ```ignore
//! use miconlab_core::{CoreError, CoreResult};
//!
//! fn do_something() -> CoreResult<()> {
//!     // 通过 ? 自动将子模块错误转换为 CoreError
//!     // Ok(())
//! #   Ok(())
//! }
//! ```

use thiserror::Error;

/// 项目统一错误类型，聚合各子模块错误。
///
/// 通过 `#[from]` 自动实现 `From` 转换，调用方只需用 `?` 即可将
/// 子模块错误向上传播为 `CoreError`。现有子模块错误类型保持不变，
/// 本枚举仅提供统一的聚合入口。
#[derive(Error, Debug)]
pub enum CoreError {
    /// Flash 烧录错误（STM32 / ESP32 / ESP8266 / STK500 等）
    #[error("Flash error: {0}")]
    Flash(#[from] crate::protocol::flasher::FlashError),

    /// USB 桥接器错误（CH341A / FT232H 等）
    #[error("USB bridge error: {0}")]
    UsbBridge(#[from] crate::usb_bridge::UsbBridgeError),

    /// Modbus 主机通信错误
    #[error("Modbus error: {0}")]
    Modbus(#[from] crate::protocol::ModbusMasterError),

    /// 配置文件读写 / 序列化错误
    #[error("Config error: {0}")]
    Config(#[from] crate::ConfigError),

    /// 串口通信错误
    #[error("Port error: {0}")]
    Port(#[from] crate::PortError),

    /// 文件传输错误（Xmodem / Ymodem / Zmodem）
    #[error("Transfer error: {0}")]
    Transfer(#[from] crate::TransferError),

    /// 标准 IO 错误
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// 其他未分类错误（兜底变体）
    #[error("{0}")]
    Other(String),
}

/// 统一 Result 别名，固定错误类型为 [`CoreError`]。
pub type CoreResult<T> = Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // From 转换测试：验证各子模块错误可自动转换为 CoreError
    // -------------------------------------------------------------------

    #[test]
    fn test_from_flash_error() {
        let err = crate::protocol::flasher::FlashError::Timeout;
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Flash(_)));
    }

    #[test]
    fn test_from_flash_error_flash_error_variant() {
        let err = crate::protocol::flasher::FlashError::FlashError("boom".to_string());
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Flash(_)));
    }

    #[test]
    fn test_from_usb_bridge_error() {
        let err = crate::usb_bridge::UsbBridgeError::Disconnected;
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::UsbBridge(_)));
    }

    #[test]
    fn test_from_modbus_error() {
        let err = crate::protocol::ModbusMasterError::Timeout;
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Modbus(_)));
    }

    #[test]
    fn test_from_config_error() {
        let err = crate::ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        ));
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Config(_)));
    }

    #[test]
    fn test_from_port_error() {
        let err = crate::PortError::NotConnected;
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Port(_)));
    }

    #[test]
    fn test_from_transfer_error() {
        let err = crate::TransferError::ChecksumMismatch;
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Transfer(_)));
    }

    #[test]
    fn test_from_io_error() {
        let err = std::io::Error::new(std::io::ErrorKind::Other, "io fail");
        let core: CoreError = err.into();
        assert!(matches!(core, CoreError::Io(_)));
    }

    // -------------------------------------------------------------------
    // Display 输出测试
    // -------------------------------------------------------------------

    #[test]
    fn test_display_flash_error() {
        let core = CoreError::from(crate::protocol::flasher::FlashError::Timeout);
        assert_eq!(core.to_string(), "Flash error: Timeout");
    }

    #[test]
    fn test_display_usb_bridge_error() {
        let core = CoreError::from(crate::usb_bridge::UsbBridgeError::Disconnected);
        assert_eq!(core.to_string(), "USB bridge error: Device disconnected");
    }

    #[test]
    fn test_display_modbus_error() {
        let core = CoreError::from(crate::protocol::ModbusMasterError::Timeout);
        assert_eq!(core.to_string(), "Modbus error: Timeout");
    }

    #[test]
    fn test_display_port_error() {
        let core = CoreError::from(crate::PortError::NotConnected);
        assert_eq!(core.to_string(), "Port error: Port is not connected");
    }

    #[test]
    fn test_display_transfer_error() {
        let core = CoreError::from(crate::TransferError::ChecksumMismatch);
        assert_eq!(core.to_string(), "Transfer error: Checksum mismatch");
    }

    #[test]
    fn test_display_other() {
        let core = CoreError::Other("custom message".to_string());
        assert_eq!(core.to_string(), "custom message");
    }

    // -------------------------------------------------------------------
    // ? 运算符集成测试：验证 CoreResult 可用 ? 传播子模块错误
    // -------------------------------------------------------------------

    fn propagate_flash_error() -> CoreResult<()> {
        Err(crate::protocol::flasher::FlashError::Timeout)?;
        Ok(())
    }

    #[test]
    fn test_question_mark_propagation() {
        let result = propagate_flash_error();
        assert!(matches!(
            result,
            Err(CoreError::Flash(crate::protocol::flasher::FlashError::Timeout))
        ));
    }
}
