pub mod acquisition;
pub mod product;
pub mod baud_detect;
pub mod bus_probe;
pub mod calibration;
pub mod checksum;
pub mod config;
pub mod crosstalk;
pub mod data_logger;
#[cfg(feature = "device-workspaces")]
pub mod device;
pub mod error;
pub mod file_transfer;
pub mod hex;
#[cfg(feature = "device-workspaces")]
pub mod insole_protocol;
pub mod platform;
pub mod plugin;
pub mod port;
pub mod protocol;
pub mod recorder;
pub mod spi_flash;
pub mod usb_bridge;
pub mod web_bridge;

pub use baud_detect::{
    BaudDetectResult, CH340_COMPENSATED_RATES, COMMON_BAUD_RATES, detect_baud_double,
    detect_baud_rate, detect_baud_with_chip, try_baud_fine,
};
pub use config::{
    AppConfig, ConfigError, ConfigParseError, DataBits, FlowControl, Parity, StopBits, SerialConfig,
};
pub use file_transfer::{FileTransfer, TransferProtocol, TransferProgress, TransferError};
pub use port::{ClearBuffer, PortError, PortResult, SerialPort, SerialPortInfo};
pub use plugin::{LoadedPlugin, PluginManager};
pub use recorder::{ScriptRecorder, ScriptReplayer};

// === 补全模块 re-export ===

// checksum：纯函数，无冲突
pub use checksum::{
    checksum8, checksum16, crc16_ccitt, crc16_modbus, crc16_xmodem, crc32, crc32c, lrc,
};

// error：统一错误聚合类型
pub use error::{CoreError, CoreResult};

// hex：纯函数，无冲突
pub use hex::{format_hex_lower, format_hex_spaced, format_hex_upper, parse_hex};

// calibration：校准算法
pub use calibration::{
    apply_linear, apply_polynomial, evaluate_fit, linear_fit, polynomial_fit,
    zero_calibration, zero_span_calibration, CalibrationCoefficients, CalibrationResult,
};

// acquisition：采集配置与采样缓冲区
// 注意：CalibrationCoefficients 已由 calibration 模块 re-export，此处不重复导出
pub use acquisition::{
    AcquisitionConfig, ChannelConfig, ChannelStats, SampleBuffer, Trigger, TriggerDirection,
};

// product：产品模型与协议库
// 注意：ProtocolParser 已由此处导出（product::protocol::ProtocolParser），
// 与 protocol::custom::ProtocolParser 不同，后者需通过 miconlab_core::protocol::ProtocolParser 访问
pub use product::{
    CalibrationData, CalibrationPoint, CalibrationType, ChannelDef, ChannelType, ChecksumType,
    DataFormat, FrameFormat, ParsedData, Product, ProductCategory, ProductLibrary,
    ProtocolCommand, ProtocolConfig, ProtocolParser, ProtocolType, RegisterMapping,
    ResponseType, SensorType,
};

// protocol：底层协议实现
// 注意：ProtocolParser (custom) 未在此 re-export，与 product::ProtocolParser 冲突，
// 需通过 miconlab_core::protocol::ProtocolParser 路径访问
pub use protocol::{
    BridgeConfig, BridgeLogEntry, CanAnalyzer, CanFilter, CanFrame, CanIdStats, ExtractedFrame,
    FlashResult, FrameBuffer, FrameStrategy, I2cResult, I2cScanEntry, ModbusFrame, ModbusFunction,
    ModbusMaster, ModbusMasterError, ModbusMasterResult, ModbusParser, ModbusTcpFrame,
    ProtocolFrame, RtuExtractResult, ScopeConfig, ScopeStats, SerialScope, SimulatorConfig,
    SimulatorLogEntry, SimulatorMode, SimulatorState, SpiResult, modbus_t1_5, modbus_t3_5,
    start_bridge, start_simulator, update_coil, update_holding_register,
};

// insole_protocol：设备工作区协议解析（公开版不编译）
#[cfg(feature = "device-workspaces")]
pub use insole_protocol::{
    detect_insole_protocol, analyze_adc, ImuQuaternion, InsoleFrame, InsoleParser, InsoleStats,
    INSOLE_HEADER, PACKET_TYPE_1, PACKET_TYPE_2, COMPLETE_FRAME_LEN, ADC_DATA_LEN,
};

// crosstalk：串扰检测（识别 CH340 TX 自发自收）
pub use crosstalk::{CrosstalkDetector, CrosstalkReport, EchoPattern};

// web_bridge：WebSocket 桥（让 HTML 可视化软件连接 Rust 后端）
pub use web_bridge::{DEFAULT_WEB_BRIDGE_PORT, DiagReport, WebBridge, WebCommand, WebMessage};
