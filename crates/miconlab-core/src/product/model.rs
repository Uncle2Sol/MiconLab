//! 产品数据模型。
//!
//! 定义柔性传感器及相关配件的产品数据结构，包括产品类别、传感器类型、
//! 通道定义、协议配置、校准数据等。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 产品类别
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ProductCategory {
    /// 柔性传感器
    FlexibleSensor,
    /// 压力阵列
    PressureArray,
    /// 采集模块
    AcquisitionModule,
    /// 配件
    Accessory,
    /// 自定义类别
    Custom(String),
}

impl fmt::Display for ProductCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProductCategory::FlexibleSensor => write!(f, "Flexible sensor"),
            ProductCategory::PressureArray => write!(f, "Pressure array"),
            ProductCategory::AcquisitionModule => write!(f, "Acquisition module"),
            ProductCategory::Accessory => write!(f, "Accessory"),
            ProductCategory::Custom(s) => write!(f, "Custom ({})", s),
        }
    }
}

/// 传感器类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SensorType {
    /// 电阻式
    Resistive,
    /// 电容式
    Capacitive,
    /// 压电式
    Piezoelectric,
    /// 光学式
    Optical,
    /// 应变片
    StrainGauge,
    /// 其他类型
    Other(String),
}

impl fmt::Display for SensorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SensorType::Resistive => write!(f, "Resistive"),
            SensorType::Capacitive => write!(f, "Capacitive"),
            SensorType::Piezoelectric => write!(f, "Piezoelectric"),
            SensorType::Optical => write!(f, "Optical"),
            SensorType::StrainGauge => write!(f, "Strain gauge"),
            SensorType::Other(s) => write!(f, "Other ({})", s),
        }
    }
}

/// 通道类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ChannelType {
    /// ADC 模拟量
    Adc,
    /// I2C 数字
    I2c,
    /// SPI 数字
    Spi,
    /// 串口协议帧
    Serial,
    /// Modbus 寄存器
    Modbus,
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelType::Adc => write!(f, "ADC analog"),
            ChannelType::I2c => write!(f, "I2C digital"),
            ChannelType::Spi => write!(f, "SPI digital"),
            ChannelType::Serial => write!(f, "Serial protocol frame"),
            ChannelType::Modbus => write!(f, "Modbus register"),
        }
    }
}

/// 协议类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ProtocolType {
    /// ADC 直读
    AdcDirect,
    /// I2C
    I2c,
    /// SPI
    Spi,
    /// 串口自定义帧
    SerialFrame,
    /// Modbus RTU
    ModbusRtu,
}

impl fmt::Display for ProtocolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolType::AdcDirect => write!(f, "ADC direct"),
            ProtocolType::I2c => write!(f, "I2C"),
            ProtocolType::Spi => write!(f, "SPI"),
            ProtocolType::SerialFrame => write!(f, "Serial custom frame"),
            ProtocolType::ModbusRtu => write!(f, "Modbus RTU"),
        }
    }
}

/// 校验类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ChecksumType {
    /// 无校验
    None,
    /// 8 位累加和
    Sum8,
    /// CRC16
    Crc16,
    /// 异或
    Xor,
}

impl fmt::Display for ChecksumType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChecksumType::None => write!(f, "No checksum"),
            ChecksumType::Sum8 => write!(f, "8-bit sum"),
            ChecksumType::Crc16 => write!(f, "CRC16"),
            ChecksumType::Xor => write!(f, "XOR"),
        }
    }
}

/// 校准类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum CalibrationType {
    /// 无校准
    None,
    /// 零点校准
    Zero,
    /// 零点+满量程
    ZeroSpan,
    /// 线性拟合
    Linear,
    /// 多项式拟合（阶数）
    Polynomial(u8),
}

impl fmt::Display for CalibrationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalibrationType::None => write!(f, "No calibration"),
            CalibrationType::Zero => write!(f, "Zero calibration"),
            CalibrationType::ZeroSpan => write!(f, "Zero + full span"),
            CalibrationType::Linear => write!(f, "Linear fit"),
            CalibrationType::Polynomial(n) => write!(f, "Polynomial fit (order {})", n),
        }
    }
}

/// 数据格式
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum DataFormat {
    /// 单字节
    U8,
    /// 大端 u16
    U16BE,
    /// 小端 u16
    U16LE,
    /// 大端 u32
    U32BE,
    /// 小端 u32
    U32LE,
    /// 大端 f32
    F32BE,
    /// 小端 f32
    F32LE,
}

impl fmt::Display for DataFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataFormat::U8 => write!(f, "U8"),
            DataFormat::U16BE => write!(f, "U16 big-endian"),
            DataFormat::U16LE => write!(f, "U16 little-endian"),
            DataFormat::U32BE => write!(f, "U32 big-endian"),
            DataFormat::U32LE => write!(f, "U32 little-endian"),
            DataFormat::F32BE => write!(f, "F32 big-endian"),
            DataFormat::F32LE => write!(f, "F32 little-endian"),
        }
    }
}

/// 响应类型
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ResponseType {
    /// ACK/NAK
    Ack,
    /// 原始数据
    Raw,
    /// 通道数据
    ChannelData,
    /// 文本
    Text,
}

impl fmt::Display for ResponseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResponseType::Ack => write!(f, "ACK/NAK"),
            ResponseType::Raw => write!(f, "Raw data"),
            ResponseType::ChannelData => write!(f, "Channel data"),
            ResponseType::Text => write!(f, "Text"),
        }
    }
}

/// 产品定义
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Product {
    /// 唯一标识
    pub id: String,
    /// 产品名称
    pub name: String,
    /// 厂家
    pub manufacturer: String,
    /// 型号
    pub model: String,
    /// 类别
    pub category: ProductCategory,
    /// 传感器类型
    pub sensor_type: SensorType,
    /// 量程下限
    pub range_min: f64,
    /// 量程上限
    pub range_max: f64,
    /// 灵敏度
    pub sensitivity: f64,
    /// 单位（如 N, kPa, Ω）
    pub unit: String,
    /// 描述
    pub description: String,
    /// 通讯协议
    pub protocol: ProtocolConfig,
    /// 通道定义
    pub channels: Vec<ChannelDef>,
    /// 校准数据
    pub calibration: Option<CalibrationData>,
    /// 创建时间
    pub created_at: String,
    /// 备注
    pub notes: String,
}

impl Default for Product {
    /// 返回一个空产品。
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            manufacturer: String::new(),
            model: String::new(),
            category: ProductCategory::Custom(String::new()),
            sensor_type: SensorType::Other(String::new()),
            range_min: 0.0,
            range_max: 0.0,
            sensitivity: 0.0,
            unit: String::new(),
            description: String::new(),
            protocol: ProtocolConfig {
                protocol_type: ProtocolType::AdcDirect,
                baud_rate: 0,
                slave_id: None,
                frame_format: None,
                commands: Vec::new(),
                register_map: Vec::new(),
                i2c_address: None,
                adc_resolution: None,
                adc_ref_voltage: None,
            },
            channels: Vec::new(),
            calibration: None,
            created_at: String::new(),
            notes: String::new(),
        }
    }
}

/// 通道定义
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChannelDef {
    /// 通道名（如 "CH0", "Force_X"）
    pub name: String,
    /// 通道类型
    pub channel_type: ChannelType,
    /// 引脚/地址（如 "GPIO34", "0x48"）
    pub pin: String,
    /// 通道索引
    pub channel_index: u8,
    /// 增益
    pub gain: f64,
    /// 偏移
    pub offset: f64,
    /// 单位
    pub unit: String,
    /// 是否启用
    pub enabled: bool,
}

impl Default for ChannelDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            channel_type: ChannelType::Adc,
            pin: String::new(),
            channel_index: 0,
            gain: 1.0,
            offset: 0.0,
            unit: String::new(),
            enabled: true,
        }
    }
}

/// 协议配置
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProtocolConfig {
    /// 协议类型
    pub protocol_type: ProtocolType,
    /// 波特率
    pub baud_rate: u32,
    /// Modbus 从站ID
    pub slave_id: Option<u8>,
    /// 帧格式（串口协议用）
    pub frame_format: Option<FrameFormat>,
    /// 命令列表
    pub commands: Vec<ProtocolCommand>,
    /// 寄存器映射（Modbus用）
    pub register_map: Vec<RegisterMapping>,
    /// I2C 设备地址
    pub i2c_address: Option<u8>,
    /// ADC 分辨率（位）
    pub adc_resolution: Option<u16>,
    /// ADC 参考电压
    pub adc_ref_voltage: Option<f64>,
}

/// 帧格式定义
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FrameFormat {
    /// 帧头
    pub header: Vec<u8>,
    /// 帧尾
    pub footer: Vec<u8>,
    /// 长度字段偏移
    pub length_offset: Option<usize>,
    /// 长度字段字节数
    pub length_bytes: usize,
    /// 数据起始偏移
    pub data_offset: usize,
    /// 校验类型
    pub checksum_type: ChecksumType,
    /// 校验字段偏移（从帧尾算）
    pub checksum_offset: Option<usize>,
    /// 通道数
    pub channel_count: usize,
    /// 数据格式
    pub data_format: DataFormat,
}

/// 协议命令
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProtocolCommand {
    /// 命令名
    pub name: String,
    /// 命令字节
    pub command_bytes: Vec<u8>,
    /// 期望响应长度
    pub expected_response_length: Option<usize>,
    /// 描述
    pub description: String,
    /// 响应解析类型
    pub response_parser: Option<ResponseType>,
}

/// 寄存器映射
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RegisterMapping {
    /// 寄存器地址
    pub register_addr: u16,
    /// 对应通道名
    pub channel_name: String,
    /// 寄存器数量
    pub quantity: u16,
    /// 缩放系数
    pub scale: f64,
    /// 偏移
    pub offset: f64,
    /// 单位
    pub unit: String,
    /// Modbus 功能码（3=读保持，4=读输入）
    pub function_code: u8,
}

/// 校准数据
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CalibrationData {
    /// 校准类型
    pub calibration_type: CalibrationType,
    /// 校准系数
    pub coefficients: Vec<f64>,
    /// 校准点
    pub points: Vec<CalibrationPoint>,
    /// 校准时间
    pub calibrated_at: Option<String>,
}

/// 校准点
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CalibrationPoint {
    /// 原始值
    pub raw_value: f64,
    /// 标准值
    pub standard_value: f64,
    /// 描述
    pub description: Option<String>,
}
