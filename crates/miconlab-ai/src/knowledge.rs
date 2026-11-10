//! 设备知识库
//!
//! 可扩展的 USB 设备指纹库，存储 VID/PID 到芯片信息的映射。
//! 支持 JSON 序列化，用户可导入/导出知识库。

use serde::{Deserialize, Serialize};

/// 设备指纹
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    /// USB Vendor ID
    pub vid: u16,
    /// USB Product ID
    pub pid: u16,
    /// 芯片/桥接器名称
    pub chip_name: String,
    /// 芯片类型（CH340/FT232/CP210x 等）
    pub chip_type: String,
    /// 协议提示（该设备可能使用的协议）
    pub protocol_hints: Vec<String>,
    /// Flash 起始地址（若已知）
    pub flash_base: Option<u32>,
    /// 描述
    pub description: String,
}

/// 知识库
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBase {
    /// 设备指纹列表
    pub devices: Vec<DeviceFingerprint>,
}

impl Default for KnowledgeBase {
    fn default() -> Self {
        Self::with_builtin_devices()
    }
}

impl KnowledgeBase {
    /// 创建空知识库
    pub fn new() -> Self {
        Self { devices: Vec::new() }
    }

    /// 创建预置知识库
    pub fn with_builtin_devices() -> Self {
        Self {
            devices: builtin_devices(),
        }
    }

    /// 根据 VID/PID 查找设备
    pub fn lookup(&self, vid: u16, pid: u16) -> Option<&DeviceFingerprint> {
        self.devices.iter().find(|d| d.vid == vid && d.pid == pid)
    }

    /// 添加设备指纹
    pub fn add(&mut self, device: DeviceFingerprint) {
        // 如果已存在相同 VID/PID，替换
        if let Some(existing) = self
            .devices
            .iter_mut()
            .find(|d| d.vid == device.vid && d.pid == device.pid)
        {
            *existing = device;
        } else {
            self.devices.push(device);
        }
    }

    /// 序列化为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// 从 JSON 字符串反序列化
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 设备数量
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
}

/// 预置设备指纹库
fn builtin_devices() -> Vec<DeviceFingerprint> {
    vec![
        DeviceFingerprint {
            vid: 0x1A86,
            pid: 0x7523,
            chip_name: "CH340".to_string(),
            chip_type: "USB-UART".to_string(),
            protocol_hints: vec!["Generic serial".to_string(), "AT commands".to_string(), "STM32 flashing".to_string()],
            flash_base: None,
            description: "knowledge.ch340.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x1A86,
            pid: 0x5523,
            chip_name: "CH341".to_string(),
            chip_type: "USB-UART/I2C/SPI".to_string(),
            protocol_hints: vec!["Generic serial".to_string(), "I2C".to_string(), "SPI".to_string()],
            flash_base: None,
            description: "knowledge.ch341.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x1A86,
            pid: 0x5512,
            chip_name: "CH341A".to_string(),
            chip_type: "SPI-Flash-Programmer".to_string(),
            protocol_hints: vec!["SPI Flash".to_string()],
            flash_base: Some(0x000000),
            description: "knowledge.ch341a.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x0403,
            pid: 0x6001,
            chip_name: "FT232R".to_string(),
            chip_type: "USB-UART".to_string(),
            protocol_hints: vec!["Generic serial".to_string()],
            flash_base: None,
            description: "knowledge.ft232r.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x0403,
            pid: 0x6014,
            chip_name: "FT232H".to_string(),
            chip_type: "USB-FIFO/SPI/I2C/JTAG".to_string(),
            protocol_hints: vec!["SPI Flash".to_string(), "I2C".to_string(), "JTAG".to_string()],
            flash_base: Some(0x000000),
            description: "knowledge.ft232h.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x10C4,
            pid: 0xEA60,
            chip_name: "CP210x".to_string(),
            chip_type: "USB-UART".to_string(),
            protocol_hints: vec!["Generic serial".to_string()],
            flash_base: None,
            description: "knowledge.cp210x.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x303A,
            pid: 0x0001,
            chip_name: "ESP32 USB-CDC".to_string(),
            chip_type: "ESP32-Native-USB".to_string(),
            protocol_hints: vec!["ESP image".to_string(), "USB-CDC serial".to_string()],
            flash_base: Some(0x000000),
            description: "knowledge.esp32_usb_cdc.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x2341,
            pid: 0x0043,
            chip_name: "Arduino Uno".to_string(),
            chip_type: "AVR-USB".to_string(),
            protocol_hints: vec!["STK500".to_string(), "Arduino".to_string()],
            flash_base: Some(0x000000),
            description: "knowledge.arduino_uno.desc".to_string(),
        },
        DeviceFingerprint {
            vid: 0x2341,
            pid: 0x0001,
            chip_name: "Arduino Mega".to_string(),
            chip_type: "AVR-USB".to_string(),
            protocol_hints: vec!["STK500".to_string(), "Arduino".to_string()],
            flash_base: Some(0x000000),
            description: "knowledge.arduino_mega.desc".to_string(),
        },
    ]
}
