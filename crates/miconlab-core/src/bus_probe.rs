// 总线深度探测模块
// 提供多种总线和协议的设备扫描与识别功能：
// - I2C 设备扫描与识别（通过 CH341/FT232H USB 直连或 UART 代理）
// - SPI Flash JEDEC ID 读取与识别
// - JTAG IDCODE 扫描与芯片识别
// - 1-Wire 设备搜索与家族码识别
// - Modbus 从站扫描与寄存器扫描
// - UART 自动波特率检测与协议特征探测

use std::time::Duration;

// ============================================================================
// I2C 设备识别表
// ============================================================================

/// I2C 设备识别信息
#[derive(Debug, Clone)]
pub struct I2cDeviceInfo {
    pub address: u8,
    pub chip_name: &'static str,
    pub description: &'static str,
    pub category: I2cDeviceCategory,
}

/// I2C 设备分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cDeviceCategory {
    Sensor,
    Display,
    Eeprom,
    IoExpander,
    Adc,
    Rtc,
    Imu,
    Other,
}

/// Fallback display name for an unknown manufacturer or chip (language-independent identity).
pub const UNKNOWN_NAME: &str = "Unknown";

/// Fallback model name when the part number cannot be determined (language-independent identity).
pub const UNKNOWN_MODEL: &str = "Unknown model";

/// 常见 I2C 设备地址表（7-bit 地址）
const I2C_DEVICE_TABLE: &[(u8, &str, &str, I2cDeviceCategory)] = &[
    // I/O 扩展
    (0x20, "PCF8574", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x21, "PCF8574", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x22, "PCF8574", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x23, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x24, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x25, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x26, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x27, "PCF8574A", "8-bit I/O expander (often used for LCD backpacks)", I2cDeviceCategory::IoExpander),
    (0x38, "PCF8574", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x39, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x3A, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x3B, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x3C, "PCF8574A", "8-bit I/O expander (or SSD1306 OLED)", I2cDeviceCategory::IoExpander),
    (0x3D, "PCF8574A", "8-bit I/O expander (or SSD1306 OLED)", I2cDeviceCategory::IoExpander),
    (0x3E, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    (0x3F, "PCF8574A", "8-bit I/O expander", I2cDeviceCategory::IoExpander),
    // 显示屏
    (0x3C, "SSD1306", "0.96/1.3-inch OLED display", I2cDeviceCategory::Display),
    (0x3D, "SSD1306", "0.96/1.3-inch OLED display", I2cDeviceCategory::Display),
    (0x27, "LCD1602", "1602 LCD character display (PCF8574 backpack)", I2cDeviceCategory::Display),
    (0x3F, "LCD1602", "1602 LCD character display (PCF8574A backpack)", I2cDeviceCategory::Display),
    // EEPROM
    (0x50, "AT24C02", "2Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x51, "AT24C04", "4Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x52, "AT24C08", "8Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x53, "AT24C16", "16Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x54, "AT24C32", "32Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x55, "AT24C64", "64Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x56, "AT24C128", "128Kbit EEPROM", I2cDeviceCategory::Eeprom),
    (0x57, "AT24C256", "256Kbit EEPROM (or DS3231 RTC)", I2cDeviceCategory::Eeprom),
    // RTC
    (0x57, "DS3231", "High-precision RTC (same address as AT24C32)", I2cDeviceCategory::Rtc),
    (0x68, "DS1307", "RTC clock", I2cDeviceCategory::Rtc),
    (0x68, "DS3231", "High-precision RTC (or MPU6050)", I2cDeviceCategory::Rtc),
    (0x69, "DS3231", "High-precision RTC", I2cDeviceCategory::Rtc),
    // ADC/DAC
    (0x48, "PCF8591", "8-bit ADC/DAC", I2cDeviceCategory::Adc),
    (0x49, "PCF8591", "8-bit ADC/DAC", I2cDeviceCategory::Adc),
    (0x4A, "PCF8591", "8-bit ADC/DAC", I2cDeviceCategory::Adc),
    (0x4B, "PCF8591", "8-bit ADC/DAC", I2cDeviceCategory::Adc),
    (0x48, "ADS1115", "16-bit ADC", I2cDeviceCategory::Adc),
    (0x49, "ADS1115", "16-bit ADC", I2cDeviceCategory::Adc),
    (0x4A, "ADS1115", "16-bit ADC", I2cDeviceCategory::Adc),
    (0x4B, "ADS1115", "16-bit ADC", I2cDeviceCategory::Adc),
    // IMU / 加速度计 / 陀螺仪
    (0x53, "ADXL345", "3-axis accelerometer", I2cDeviceCategory::Imu),
    (0x68, "MPU6050", "6-axis IMU (accelerometer + gyroscope)", I2cDeviceCategory::Imu),
    (0x69, "MPU6050", "6-axis IMU (accelerometer + gyroscope)", I2cDeviceCategory::Imu),
    (0x6A, "LSM6DS3", "6-axis IMU", I2cDeviceCategory::Imu),
    (0x6B, "LSM6DS3", "6-axis IMU", I2cDeviceCategory::Imu),
    (0x1E, "HMC5883L", "3-axis magnetometer", I2cDeviceCategory::Sensor),
    (0x0D, "QMC5883L", "3-axis magnetometer", I2cDeviceCategory::Sensor),
    // 温湿度 / 气压
    (0x76, "BME280", "Temperature, humidity and pressure sensor", I2cDeviceCategory::Sensor),
    (0x77, "BME280/BMP280", "Temperature, humidity and pressure sensor", I2cDeviceCategory::Sensor),
    (0x77, "BMP180", "Pressure sensor", I2cDeviceCategory::Sensor),
    (0x40, "SHT3x", "Temperature and humidity sensor (or PCA9685)", I2cDeviceCategory::Sensor),
    (0x44, "SHT3x", "Temperature and humidity sensor", I2cDeviceCategory::Sensor),
    (0x45, "SHT3x", "Temperature and humidity sensor", I2cDeviceCategory::Sensor),
    (0x5C, "AM2320", "Temperature and humidity sensor", I2cDeviceCategory::Sensor),
    (0x38, "AHT10/AHT20", "Temperature and humidity sensor", I2cDeviceCategory::Sensor),
    (0x39, "TSL2561", "Ambient light sensor", I2cDeviceCategory::Sensor),
    (0x29, "TSL2591", "Ambient light sensor", I2cDeviceCategory::Sensor),
    (0x44, "SHT31", "Temperature and humidity sensor", I2cDeviceCategory::Sensor),
    // PWM / LED 控制
    (0x40, "PCA9685", "16-channel 12-bit PWM", I2cDeviceCategory::Other),
    (0x70, "PCA9685", "16-channel PWM (responds on all addresses)", I2cDeviceCategory::Other),
    // 触摸 / 手势
    (0x38, "FT6206", "Capacitive touchscreen controller", I2cDeviceCategory::Sensor),
    (0x5A, "MPR121", "12-channel capacitive touch", I2cDeviceCategory::Sensor),
    (0x15, "GSL1680", "Capacitive touch controller", I2cDeviceCategory::Sensor),
    // 其他
    (0x1D, "ADXL345", "3-axis accelerometer (alternate address)", I2cDeviceCategory::Imu),
    (0x2A, "CAP1188", "8-channel capacitive touch", I2cDeviceCategory::Sensor),
    (0x5B, "CAP1188", "8-channel capacitive touch", I2cDeviceCategory::Sensor),
    (0x6F, "MCP7940", "RTC clock", I2cDeviceCategory::Rtc),
];

/// 根据 I2C 地址查询设备信息
pub fn lookup_i2c_device(addr: u8) -> Vec<I2cDeviceInfo> {
    I2C_DEVICE_TABLE
        .iter()
        .filter(|(a, _, _, _)| *a == addr)
        .map(|(addr, name, desc, cat)| I2cDeviceInfo {
            address: *addr,
            chip_name: name,
            description: desc,
            category: *cat,
        })
        .collect()
}

/// I2C 扫描结果
#[derive(Debug, Clone)]
pub struct I2cScanResult {
    pub address: u8,
    pub devices: Vec<I2cDeviceInfo>,
}

// ============================================================================
// SPI Flash JEDEC ID 识别表
// ============================================================================

/// SPI Flash 工作电压
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiFlashVoltage {
    /// 3.3V (2.7V~3.6V)，最常见
    V3_3,
    /// 1.8V (1.65V~1.95V)，低电压系列（如 MX25U、W25Q_EW）
    V1_8,
    /// 宽电压 1.65V~3.6V（兼容 1.8V 和 3.3V）
    Wide,
}

impl SpiFlashVoltage {
    /// 返回典型工作电压（单位：伏特）
    pub fn typical(&self) -> f32 {
        match self {
            SpiFlashVoltage::V3_3 => 3.3,
            SpiFlashVoltage::V1_8 => 1.8,
            SpiFlashVoltage::Wide => 3.3,
        }
    }

    /// 返回电压范围字符串
    pub fn range_str(&self) -> &'static str {
        match self {
            SpiFlashVoltage::V3_3 => "2.7V~3.6V",
            SpiFlashVoltage::V1_8 => "1.65V~1.95V",
            SpiFlashVoltage::Wide => "1.65V~3.6V",
        }
    }

    /// 英文描述（界面层用 i18n 翻译，源语言为英文）
    pub fn name(&self) -> &'static str {
        match self {
            SpiFlashVoltage::V3_3 => "3.3V",
            SpiFlashVoltage::V1_8 => "1.8V (low voltage)",
            SpiFlashVoltage::Wide => "Wide voltage (1.65V~3.6V)",
        }
    }
}

/// SPI Flash 识别信息
#[derive(Debug, Clone)]
pub struct SpiFlashInfo {
    pub manufacturer: &'static str,
    pub part_number: &'static str,
    pub capacity_mbit: u32,
    pub jedec_id: u32,
    /// 是否支持 4-byte 地址模式（≥32MB 大容量芯片通常支持）
    pub supports_4byte_addr: bool,
    /// 工作电压（决定是否需要电平转换）
    pub voltage: SpiFlashVoltage,
}

/// SPI Flash JEDEC ID 表
/// (厂商 ID, 容量 ID, 厂商, 型号, 容量 Mbit, 支持 4-byte 地址模式, 工作电压)
const SPI_FLASH_TABLE_V2: &[(u8, u16, &str, &str, u32, bool, SpiFlashVoltage)] = &[
    // ===== Winbond (0xEF) =====
    // W25Q BV 系列（3.3V，老版本）
    (0xEF, 0x3013, "Winbond", "W25Q80BV", 8, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x3014, "Winbond", "W25Q16BV", 16, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x3015, "Winbond", "W25Q32BV", 32, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x4016, "Winbond", "W25Q64FV", 64, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x4017, "Winbond", "W25Q128FV", 128, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x4018, "Winbond", "W25Q256FV", 256, true, SpiFlashVoltage::V3_3),
    // W25Q JV 系列（3.3V，新版本，更常见）
    (0xEF, 0x6014, "Winbond", "W25Q16JV", 16, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x6015, "Winbond", "W25Q32JV", 32, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x6016, "Winbond", "W25Q64JV", 64, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x6017, "Winbond", "W25Q128JV", 128, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x6018, "Winbond", "W25Q256JV", 256, true, SpiFlashVoltage::V3_3),
    (0xEF, 0x6019, "Winbond", "W25Q512JV", 512, true, SpiFlashVoltage::V3_3),
    // W25Q EW 系列（1.8V，汽车级）
    (0xEF, 0x7018, "Winbond", "W25Q256JW", 256, true, SpiFlashVoltage::V1_8),
    (0xEF, 0x7019, "Winbond", "W25Q512JW", 512, true, SpiFlashVoltage::V1_8),
    // W25X 系列（老式）
    (0xEF, 0x3010, "Winbond", "W25X10", 1, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x3011, "Winbond", "W25X20", 2, false, SpiFlashVoltage::V3_3),
    (0xEF, 0x3012, "Winbond", "W25X40", 4, false, SpiFlashVoltage::V3_3),
    // ===== Macronix (0xC2) =====
    // MX25L 系列（3.3V）
    (0xC2, 0x2013, "Macronix", "MX25L8006", 8, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2014, "Macronix", "MX25L1606", 16, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2015, "Macronix", "MX25L3206", 32, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2016, "Macronix", "MX25L6406", 64, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2017, "Macronix", "MX25L12806", 128, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2018, "Macronix", "MX25L25635", 256, true, SpiFlashVoltage::V3_3),
    (0xC2, 0x2019, "Macronix", "MX25L51245G", 512, true, SpiFlashVoltage::V3_3),
    // MX25L 新版本（3.3V，E 后缀）
    (0xC2, 0x2516, "Macronix", "MX25L6433E", 64, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2517, "Macronix", "MX25L12833E", 128, false, SpiFlashVoltage::V3_3),
    (0xC2, 0x2518, "Macronix", "MX25L25635E", 256, true, SpiFlashVoltage::V3_3),
    (0xC2, 0x2519, "Macronix", "MX25L51245G", 512, true, SpiFlashVoltage::V3_3),
    // MX25U 系列（1.8V 低电压）— 用户芯片 MX DZ-U010001 可能属于此系列
    (0xC2, 0x2533, "Macronix", "MX25U2033E", 2, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x2534, "Macronix", "MX25U4033E", 4, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x2535, "Macronix", "MX25U8033E", 8, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x2536, "Macronix", "MX25U1633E", 16, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x2537, "Macronix", "MX25U3233E", 32, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x2538, "Macronix", "MX25U6433E", 64, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x2539, "Macronix", "MX25U12833E", 128, false, SpiFlashVoltage::V1_8),
    (0xC2, 0x253A, "Macronix", "MX25U25635E", 256, true, SpiFlashVoltage::V1_8),
    (0xC2, 0x253B, "Macronix", "MX25U51245G", 512, true, SpiFlashVoltage::V1_8),
    // MX25R 系列（宽电压 1.65V~3.6V）
    (0xC2, 0x2816, "Macronix", "MX25R6435F", 64, false, SpiFlashVoltage::Wide),
    (0xC2, 0x2817, "Macronix", "MX25R12835F", 128, false, SpiFlashVoltage::Wide),
    // ===== Micron/Numonyx (0x20) =====
    (0x20, 0x2014, "Micron", "M25P16", 16, false, SpiFlashVoltage::V3_3),
    (0x20, 0x2015, "Micron", "M25P32", 32, false, SpiFlashVoltage::V3_3),
    (0x20, 0x2016, "Micron", "M25P64", 64, false, SpiFlashVoltage::V3_3),
    (0x20, 0x2017, "Micron", "M25P128", 128, false, SpiFlashVoltage::V3_3),
    (0x20, 0xBA18, "Micron", "N25Q128", 128, false, SpiFlashVoltage::V3_3),
    (0x20, 0xBA19, "Micron", "N25Q256", 256, true, SpiFlashVoltage::V3_3),
    (0x20, 0xBA20, "Micron", "N25Q512", 512, true, SpiFlashVoltage::V3_3),
    (0x20, 0xBA21, "Micron", "N25Q1G", 1024, true, SpiFlashVoltage::V3_3),
    // MT25Q 系列（新版）
    (0x20, 0xBB18, "Micron", "MT25QU128", 128, false, SpiFlashVoltage::V3_3),
    (0x20, 0xBB19, "Micron", "MT25QU256", 256, true, SpiFlashVoltage::V3_3),
    (0x20, 0xBB20, "Micron", "MT25QU512", 512, true, SpiFlashVoltage::V3_3),
    (0x20, 0xBA1A, "Micron", "MT25QL256", 256, true, SpiFlashVoltage::V3_3),
    (0x20, 0xBA1B, "Micron", "MT25QL512", 512, true, SpiFlashVoltage::V3_3),
    // ===== Spansion/Cypress/Infineon (0x01) =====
    (0x01, 0x4014, "Spansion", "S25FL016K", 16, false, SpiFlashVoltage::V3_3),
    (0x01, 0x4015, "Spansion", "S25FL032K", 32, false, SpiFlashVoltage::V3_3),
    (0x01, 0x4016, "Spansion", "S25FL064K", 64, false, SpiFlashVoltage::V3_3),
    (0x01, 0x4017, "Spansion", "S25FL128K", 128, false, SpiFlashVoltage::V3_3),
    (0x01, 0x4018, "Spansion", "S25FL256K", 256, true, SpiFlashVoltage::V3_3),
    (0x01, 0x4019, "Spansion", "S25FL512S", 512, true, SpiFlashVoltage::V3_3),
    (0x01, 0x6015, "Spansion", "S25FL132K", 32, false, SpiFlashVoltage::V3_3),
    (0x01, 0x6016, "Spansion", "S25FL164K", 64, false, SpiFlashVoltage::V3_3),
    // ===== GigaDevice (0xC8) =====
    // GD25Q 系列（3.3V）
    (0xC8, 0x4013, "GigaDevice", "GD25Q80", 8, false, SpiFlashVoltage::V3_3),
    (0xC8, 0x4014, "GigaDevice", "GD25Q16", 16, false, SpiFlashVoltage::V3_3),
    (0xC8, 0x4015, "GigaDevice", "GD25Q32", 32, false, SpiFlashVoltage::V3_3),
    (0xC8, 0x4016, "GigaDevice", "GD25Q64", 64, false, SpiFlashVoltage::V3_3),
    (0xC8, 0x4017, "GigaDevice", "GD25Q128", 128, false, SpiFlashVoltage::V3_3),
    (0xC8, 0x4018, "GigaDevice", "GD25Q256", 256, true, SpiFlashVoltage::V3_3),
    (0xC8, 0x4019, "GigaDevice", "GD25Q512", 512, true, SpiFlashVoltage::V3_3),
    // GD25B 系列（1.8V）
    (0xC8, 0x6014, "GigaDevice", "GD25B16", 16, false, SpiFlashVoltage::V1_8),
    (0xC8, 0x6016, "GigaDevice", "GD25B64", 64, false, SpiFlashVoltage::V1_8),
    (0xC8, 0x6017, "GigaDevice", "GD25B128", 128, false, SpiFlashVoltage::V1_8),
    (0xC8, 0x6018, "GigaDevice", "GD25B256", 256, true, SpiFlashVoltage::V1_8),
    // GD25LQ 系列（宽电压）
    (0xC8, 0x6015, "GigaDevice", "GD25LQ32", 32, false, SpiFlashVoltage::Wide),
    // ===== ISSI (0x9D) =====
    // IS25LP 系列（3.3V）
    (0x9D, 0x4014, "ISSI", "IS25LP016", 16, false, SpiFlashVoltage::V3_3),
    (0x9D, 0x4015, "ISSI", "IS25LP032", 32, false, SpiFlashVoltage::V3_3),
    (0x9D, 0x4016, "ISSI", "IS25LP064", 64, false, SpiFlashVoltage::V3_3),
    (0x9D, 0x6017, "ISSI", "IS25LP128", 128, false, SpiFlashVoltage::V3_3),
    (0x9D, 0x6018, "ISSI", "IS25LP256", 256, true, SpiFlashVoltage::V3_3),
    // IS25WP 系列（宽电压 1.65V~3.6V）
    (0x9D, 0x7014, "ISSI", "IS25WP016", 16, false, SpiFlashVoltage::Wide),
    (0x9D, 0x7015, "ISSI", "IS25WP032", 32, false, SpiFlashVoltage::Wide),
    (0x9D, 0x7016, "ISSI", "IS25WP064", 64, false, SpiFlashVoltage::Wide),
    (0x9D, 0x7017, "ISSI", "IS25WP128", 128, false, SpiFlashVoltage::Wide),
    // IS25WQ 系列（1.8V）
    (0x9D, 0x5116, "ISSI", "IS25WQ064", 64, false, SpiFlashVoltage::V1_8),
    // ===== Adesto/Atmel/Microchip (0x1F) =====
    (0x1F, 0x2400, "Adesto", "AT25FS010", 1, false, SpiFlashVoltage::V3_3),
    (0x1F, 0x4500, "Adesto", "AT25DF041A", 4, false, SpiFlashVoltage::V3_3),
    (0x1F, 0x4600, "Adesto", "AT25DF081A", 8, false, SpiFlashVoltage::V3_3),
    (0x1F, 0x4700, "Adesto", "AT25DF161", 16, false, SpiFlashVoltage::V3_3),
    (0x1F, 0x4800, "Adesto", "AT25DF321", 32, false, SpiFlashVoltage::V3_3),
    (0x1F, 0x4900, "Adesto", "AT25DF641", 64, false, SpiFlashVoltage::V3_3),
    // ===== EON (0x1C) =====
    (0x1C, 0x3013, "EON", "EN25Q80", 8, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x3014, "EON", "EN25Q16", 16, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x3015, "EON", "EN25Q32", 32, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x3016, "EON", "EN25Q64", 64, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x3017, "EON", "EN25Q128", 128, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x7015, "EON", "EN25QH32", 32, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x7016, "EON", "EN25QH64", 64, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x7017, "EON", "EN25QH128", 128, false, SpiFlashVoltage::V3_3),
    (0x1C, 0x7018, "EON", "EN25QH256", 256, true, SpiFlashVoltage::V3_3),
    // ===== Puya (0x85) =====
    (0x85, 0x6013, "Puya", "P25Q80H", 8, false, SpiFlashVoltage::Wide),
    (0x85, 0x6014, "Puya", "P25Q16H", 16, false, SpiFlashVoltage::Wide),
    (0x85, 0x6015, "Puya", "P25Q32H", 32, false, SpiFlashVoltage::Wide),
    (0x85, 0x6016, "Puya", "P25Q64H", 64, false, SpiFlashVoltage::Wide),
    (0x85, 0x6017, "Puya", "P25Q128H", 128, false, SpiFlashVoltage::Wide),
    // ===== ESMT (0x8C) =====
    (0x8C, 0x4015, "ESMT", "EN25S32", 32, false, SpiFlashVoltage::V3_3),
    (0x8C, 0x4016, "ESMT", "EN25S64", 64, false, SpiFlashVoltage::V3_3),
    (0x8C, 0x4017, "ESMT", "EN25S128", 128, false, SpiFlashVoltage::V3_3),
    (0x8C, 0x4018, "ESMT", "EN25S256", 256, true, SpiFlashVoltage::V3_3),
    // ===== XTX (0x0B) =====
    (0x0B, 0x4015, "XTX", "XT25Q32D", 32, false, SpiFlashVoltage::Wide),
    (0x0B, 0x4016, "XTX", "XT25Q64D", 64, false, SpiFlashVoltage::Wide),
    (0x0B, 0x4017, "XTX", "XT25Q128D", 128, false, SpiFlashVoltage::Wide),
    (0x0B, 0x4018, "XTX", "XT25Q256D", 256, true, SpiFlashVoltage::Wide),
    // ===== Fremont Micro (0x62) =====
    (0x62, 0x0614, "Fremont", "P25Q16L", 16, false, SpiFlashVoltage::Wide),
    (0x62, 0x0615, "Fremont", "P25Q32L", 32, false, SpiFlashVoltage::Wide),
    (0x62, 0x0616, "Fremont", "P25Q64L", 64, false, SpiFlashVoltage::Wide),
    (0x62, 0x0617, "Fremont", "P25Q128L", 128, false, SpiFlashVoltage::Wide),
    // ===== Boya (0x68) =====
    (0x68, 0x4014, "Boya", "BY25Q16AS", 16, false, SpiFlashVoltage::Wide),
    (0x68, 0x4015, "Boya", "BY25Q32AS", 32, false, SpiFlashVoltage::Wide),
    (0x68, 0x4016, "Boya", "BY25Q64AS", 64, false, SpiFlashVoltage::Wide),
    (0x68, 0x4017, "Boya", "BY25Q128AS", 128, false, SpiFlashVoltage::Wide),
    // ===== DOFULY (0xE0) =====
    (0xE0, 0x4015, "Dofuly", "DF25Q32", 32, false, SpiFlashVoltage::Wide),
    (0xE0, 0x4016, "Dofuly", "DF25Q64", 64, false, SpiFlashVoltage::Wide),
    (0xE0, 0x4017, "Dofuly", "DF25Q128", 128, false, SpiFlashVoltage::Wide),
    // ===== Fudan Micro (0xA1) =====
    (0xA1, 0x4014, "FudanMicro", "FM25Q16", 16, false, SpiFlashVoltage::Wide),
    (0xA1, 0x4015, "FudanMicro", "FM25Q32", 32, false, SpiFlashVoltage::Wide),
    (0xA1, 0x4016, "FudanMicro", "FM25Q64", 64, false, SpiFlashVoltage::Wide),
    (0xA1, 0x4017, "FudanMicro", "FM25Q128", 128, false, SpiFlashVoltage::Wide),
    // ===== Paragon (0x61) =====
    (0x61, 0x4014, "Paragon", "PN25F16", 16, false, SpiFlashVoltage::V3_3),
    (0x61, 0x4015, "Paragon", "PN25F32", 32, false, SpiFlashVoltage::V3_3),
    // ===== SST/Microchip (0xBF) =====
    (0xBF, 0x2541, "SST", "SST25VF016B", 16, false, SpiFlashVoltage::V3_3),
    (0xBF, 0x258D, "SST", "SST25VF032B", 32, false, SpiFlashVoltage::V3_3),
    (0xBF, 0x4A01, "SST", "SST26VF016", 16, false, SpiFlashVoltage::V3_3),
    (0xBF, 0x4A02, "SST", "SST26VF032", 32, false, SpiFlashVoltage::V3_3),
    (0xBF, 0x4A43, "SST", "SST26VF064", 64, false, SpiFlashVoltage::V3_3),
];

/// 根据 JEDEC ID 查询 SPI Flash 信息
/// jedec_id = (manufacturer_id << 16) | capacity_id
pub fn lookup_spi_flash(manufacturer_id: u8, capacity_id: u16) -> Option<SpiFlashInfo> {
    SPI_FLASH_TABLE_V2
        .iter()
        .find(|(mfg, cap, _, _, _, _, _)| *mfg == manufacturer_id && *cap == capacity_id)
        .map(|(mfg, cap, name, part, mbit, supports_4byte, voltage)| SpiFlashInfo {
            manufacturer: name,
            part_number: part,
            capacity_mbit: *mbit,
            jedec_id: ((*mfg as u32) << 16) | (*cap as u32),
            supports_4byte_addr: *supports_4byte,
            voltage: *voltage,
        })
}

/// SPI Flash 厂商名查询
pub fn spi_flash_vendor_name(mfg_id: u8) -> &'static str {
    match mfg_id {
        0xEF => "Winbond",
        0xC2 => "Macronix",
        0x20 => "Micron/Numonyx",
        0x01 => "Spansion/Cypress/Infineon",
        0xC8 => "GigaDevice",
        0x9D => "ISSI",
        0x1F => "Adesto/Atmel/Microchip",
        0x1C => "EON",
        0x85 => "Puya",
        0x8C => "ESMT",
        0x0B => "XTX",
        0x62 => "Fremont Micro",
        0x68 => "Boya",
        0xE0 => "Dofuly",
        0xA1 => "Fudan Micro",
        0x61 => "Paragon",
        0xBF => "SST/Microchip",
        0xD8 => "AMIC",
        0x37 => "AMIC",
        0xC9 => "JHC",
        0x52 => "GD Silicon Solutions",
        0x9C => "Fidelix",
        0x15 => "Phase Memory",
        0xB0 => "Crystalonic",
        0xB3 => "XMC",
        0xF8 => "Boya",
        0x4A => "Fairchild",
        _ => UNKNOWN_NAME,
    }
}

// ============================================================================
// JTAG IDCODE 识别表
// ============================================================================

/// JTAG 芯片识别信息
#[derive(Debug, Clone)]
pub struct JtagChipInfo {
    pub idcode: u32,
    pub manufacturer: &'static str,
    pub part_number: &'static str,
    pub core: &'static str,
}

/// JTAG IDCODE 表
/// IDCODE 格式: [31:28] Version | [27:12] Part Number | [11:1] Manufacturer | [0] 1
const JTAG_IDCODE_TABLE: &[(u32, &str, &str, &str)] = &[
    // ARM Cortex-M (JTAG-DP)
    (0x0BA9542F, "ARM", "Cortex-M3", "JTAG-DP"),
    (0x0BA01477, "ARM", "Cortex-M0+", "JTAG-DP"),
    (0x0BA02477, "ARM", "Cortex-M0", "JTAG-DP"),
    (0x0BA04477, "ARM", "Cortex-M4", "JTAG-DP"),
    (0x0BA07477, "ARM", "Cortex-M7", "JTAG-DP"),
    (0x0BA05477, "ARM", "Cortex-M3 r2p1", "JTAG-DP"),
    // STM32
    (0x06412041, "STMicro", "STM32F103xB", "Cortex-M3"),
    (0x06410041, "STMicro", "STM32F103x6", "Cortex-M3"),
    (0x06414041, "STMicro", "STM32F103xE", "Cortex-M3"),
    (0x06418041, "STMicro", "STM32F103xG", "Cortex-M3"),
    (0x06411041, "STMicro", "STM32F100xB", "Cortex-M3"),
    (0x06412041, "STMicro", "STM32F100xE", "Cortex-M3"),
    // Xilinx FPGA
    (0x0362D093, "Xilinx", "XC3S200", "Spartan-3"),
    (0x0362C093, "Xilinx", "XC3S400", "Spartan-3"),
    (0x03632093, "Xilinx", "XC3S1000", "Spartan-3"),
    (0x0364D093, "Xilinx", "XC3S1400A", "Spartan-3A"),
    (0x037C2093, "Xilinx", "XC3S700A", "Spartan-3AN"),
    (0x03724093, "Xilinx", "XC3S1400AN", "Spartan-3AN"),
    (0x44002093, "Xilinx", "XC6SLX9", "Spartan-6"),
    (0x44012093, "Xilinx", "XC6SLX16", "Spartan-6"),
    (0x4401E093, "Xilinx", "XC6SLX25", "Spartan-6"),
    // Altera/Intel FPGA
    (0x020820DD, "Altera", "EP2C8", "Cyclone II"),
    (0x020810DD, "Altera", "EP2C5", "Cyclone II"),
    (0x020A20DD, "Altera", "EP2C20", "Cyclone II"),
    (0x020B30DD, "Altera", "EP2C35", "Cyclone II"),
    (0x020F40DD, "Altera", "EP2C70", "Cyclone II"),
    (0x020F50DD, "Altera", "EP3C5", "Cyclone III"),
    (0x021020DD, "Altera", "EP3C16", "Cyclone III"),
    (0x021120DD, "Altera", "EP3C25", "Cyclone III"),
    (0x024120DD, "Altera", "EP4CE6", "Cyclone IV"),
    (0x024220DD, "Altera", "EP4CE10", "Cyclone IV"),
    (0x024320DD, "Altera", "EP4CE15", "Cyclone IV"),
    // ESP32
    (0x00005C25, "Espressif", "ESP32", "Tensilica LX6"),
    (0x00007C25, "Espressif", "ESP32-S2", "Tensilica LX7"),
    (0x00008C25, "Espressif", "ESP32-C3", "RISC-V"),
    // NXP
    (0x0BA01477, "NXP", "LPC11xx", "Cortex-M0"),
    (0x0BA01F47, "NXP", "LPC1768", "Cortex-M3"),
    // TI
    (0x0B95402F, "TI", "TMS470", "ARM7"),
    (0x0B95502F, "TI", "TM4C123", "Cortex-M4F"),
    (0x0B97A02F, "TI", "CC26xx", "Cortex-M3"),
    // Microchip
    (0x0372703F, "Microchip", "ATSAMD21", "Cortex-M0+"),
    (0x0371C03F, "Microchip", "ATSAM3X", "Cortex-M3"),
];

/// 根据 IDCODE 查询 JTAG 芯片
pub fn lookup_jtag_chip(idcode: u32) -> Option<JtagChipInfo> {
    // 精确匹配
    if let Some((_, mfg, part, core)) = JTAG_IDCODE_TABLE
        .iter()
        .find(|(id, _, _, _)| *id == idcode)
    {
        return Some(JtagChipInfo {
            idcode,
            manufacturer: mfg,
            part_number: part,
            core,
        });
    }

    // 尝试厂商匹配（通过 Manufacturer ID 字段 [11:1]）
    let mfg_id = ((idcode >> 1) & 0x7FF) as u16;
    let mfg_name = jtag_manufacturer_name(mfg_id);
    if mfg_name != UNKNOWN_NAME {
        return Some(JtagChipInfo {
            idcode,
            manufacturer: mfg_name,
            part_number: UNKNOWN_MODEL,
            core: UNKNOWN_NAME,
        });
    }

    None
}

/// JTAG 厂商 ID 查询
pub fn jtag_manufacturer_name(mfg_id: u16) -> &'static str {
    // JEP106 厂商码（部分常见）
    match mfg_id {
        0x017 => "ARM",
        0x020 => "STMicroelectronics",
        0x049 => "Xilinx",
        0x06E => "Altera/Intel",
        0x037 => "Microchip",
        0x021 => "NXP",
        0x02A => "TI",
        0x062 => "Espressif",
        0x0C1 => "Cypress",
        0x015 => "ADI",
        0x024 => "Maxim",
        _ => UNKNOWN_NAME,
    }
}

// ============================================================================
// 1-Wire 设备识别表
// ============================================================================

/// 1-Wire 设备识别信息
#[derive(Debug, Clone)]
pub struct OnewireDeviceInfo {
    pub family_code: u8,
    pub chip_name: &'static str,
    pub description: &'static str,
}

/// 1-Wire 家族码表
const ONEWIRE_FAMILY_TABLE: &[(u8, &str, &str)] = &[
    (0x01, "DS2401/DS1990A", "64-bit silicon serial number"),
    (0x02, "DS1991", "Multifunction iButton"),
    (0x04, "DS1994", "4K NVRAM with clock"),
    (0x05, "DS2405", "Single-channel address switch"),
    (0x06, "DS1993", "4K NVRAM"),
    (0x08, "DS1992", "1K NVRAM"),
    (0x09, "DS2502", "1K EPROM"),
    (0x0A, "DS1995", "16K NVRAM"),
    (0x0B, "DS2505", "16K EPROM"),
    (0x0C, "DS1996", "64K NVRAM"),
    (0x0F, "DS2506", "64K EPROM"),
    (0x10, "DS18S20", "Temperature sensor (1-Wire)"),
    (0x12, "DS2406/DS2407", "Dual-channel address switch"),
    (0x14, "DS2430A", "256-bit EEPROM"),
    (0x1A, "DS2431", "1024-bit EEPROM"),
    (0x1D, "DS2423", "4K RAM with counter"),
    (0x1F, "DS2409", "Dual-channel coupler"),
    (0x21, "DS1921", "Temperature logger iButton"),
    (0x22, "DS1822", "Temperature sensor"),
    (0x23, "DS2433", "4K EEPROM"),
    (0x24, "DS2434", "Battery ID chip with clock"),
    (0x26, "DS2438", "Smart battery monitor"),
    (0x27, "DS2417", "RTC with clock"),
    (0x28, "DS18B20", "Digital temperature sensor"),
    (0x29, "DS2408", "8-channel addressable switch"),
    (0x2C, "DS2890", "Single-channel digital potentiometer"),
    (0x2D, "DS2431", "1K EEPROM"),
    (0x2E, "DS2432", "1K EEPROM with SHA-1"),
    (0x2F, "DS28E04-100", "4K EEPROM with PIO"),
    (0x30, "DS2760", "High-precision lithium battery monitor"),
    (0x31, "DS2761", "High-precision lithium battery monitor"),
    (0x33, "DS2433", "4K EEPROM"),
    (0x3A, "DS2413", "Dual-channel addressable switch"),
    (0x3B, "DS18B20-PAR", "Digital temperature sensor (parasitic power)"),
    (0x3D, "DS2780", "Fuel gauge"),
    (0x41, "DS1923", "Temperature and humidity logger iButton"),
    (0x42, "DS28EA00", "Temperature sensor with PIO"),
    (0x43, "DS28EC20", "20K EEPROM"),
];

/// 根据家族码查询 1-Wire 设备
pub fn lookup_onewire_device(family_code: u8) -> Option<OnewireDeviceInfo> {
    ONEWIRE_FAMILY_TABLE
        .iter()
        .find(|(code, _, _)| *code == family_code)
        .map(|(code, name, desc)| OnewireDeviceInfo {
            family_code: *code,
            chip_name: name,
            description: desc,
        })
}

// ============================================================================
// Modbus 从站扫描
// ============================================================================

/// Modbus 扫描结果
#[derive(Debug, Clone)]
pub struct ModbusScanResult {
    pub slave_id: u8,
    pub registers: Vec<u16>,
    pub raw_response: Vec<u8>,
}

/// 扫描 Modbus 从站
/// 逐个地址发送读保持寄存器命令（FC 0x03），地址 0，数量 1
/// 返回有响应的从站列表
pub fn scan_modbus_slaves(
    port: &mut crate::SerialPort,
    start_id: u8,
    end_id: u8,
) -> Vec<ModbusScanResult> {
    let mut results = Vec::new();
    let master = crate::protocol::modbus_master::ModbusMaster::new(0)
        .with_timeout(Duration::from_millis(200));

    for slave_id in start_id..=end_id {
        let frame = crate::protocol::modbus_common::build_read_frame(
            slave_id,
            crate::protocol::modbus::ModbusFunction::ReadHoldingRegisters,
            0x0000,
            1,
        );
        match master.execute(port, &frame) {
            Ok(response) => {
                if !response.is_empty() {
                    let regs = master.parse_response(&response).unwrap_or_default();
                    results.push(ModbusScanResult {
                        slave_id,
                        registers: regs,
                        raw_response: response,
                    });
                }
            }
            Err(_) => continue,
        }
    }

    results
}

/// 扫描 Modbus 寄存器
/// 在指定地址范围内逐个读取保持寄存器
pub fn scan_modbus_registers(
    port: &mut crate::SerialPort,
    slave_id: u8,
    start_addr: u16,
    end_addr: u16,
) -> Vec<(u16, Option<u16>)> {
    let mut results = Vec::new();
    let master = crate::protocol::modbus_master::ModbusMaster::new(slave_id)
        .with_timeout(Duration::from_millis(200));

    for addr in start_addr..=end_addr {
        match master.read_holding_registers(port, addr, 1) {
            Ok(regs) => {
                if let Some(val) = regs.first() {
                    results.push((addr, Some(*val)));
                } else {
                    results.push((addr, None));
                }
            }
            Err(_) => {
                results.push((addr, None));
            }
        }
    }

    results
}

// ============================================================================
// UART 协议特征探测
// ============================================================================

/// UART 探测结果
#[derive(Debug, Clone)]
pub struct UartProbeResult {
    pub protocol_name: String,
    pub description: String,
    pub confidence: f32,
    pub evidence: String,
}

/// 分析 UART 接收数据，识别可能的协议
pub fn analyze_uart_data(data: &[u8]) -> Vec<UartProbeResult> {
    let mut results = Vec::new();
    if data.is_empty() {
        return results;
    }

    // NMEA 0183 检测：以 $ 或 ! 开头，以 \r\n 结尾
    if (data[0] == b'$' || data[0] == b'!') && data.contains(&b'\n') {
        let text = String::from_utf8_lossy(data);
        let nmea_count = text.lines().take(5).filter(|s| {
            s.starts_with('$') && (s.contains("GGA") || s.contains("RMC") || s.contains("GSV") ||
                s.contains("GLL") || s.contains("VTG") || s.contains("GSA"))
        }).count();
        if nmea_count > 0 {
            let sample: Vec<&str> = text.lines().filter(|s| s.starts_with('$')).take(2).collect();
            results.push(UartProbeResult {
                protocol_name: "NMEA 0183".to_string(),
                description: format!("GPS navigation data ({} valid sentences)", nmea_count),
                confidence: 0.95,
                evidence: format!("Detected NMEA sentences: {}", sample.join("; ")),
            });
        }
    }

    // Modbus RTU 检测：帧长度 >= 4，CRC 校验
    if data.len() >= 4 {
        if let Ok(frame) = crate::protocol::modbus::ModbusFrame::parse(data) {
            results.push(UartProbeResult {
                protocol_name: "Modbus RTU".to_string(),
                description: format!("Slave {} function code 0x{:02X}", frame.slave_id, frame.function.to_code()),
                confidence: 0.95,
                evidence: format!("CRC check passed, slave ID={}, function={}", frame.slave_id, frame.function.name()),
            });
        }
    }

    // SLCAN 检测：以 t/T/r/b/d/O/C 开头
    if let Ok(text) = std::str::from_utf8(data) {
        let first_char = text.chars().next().unwrap_or(' ');
        if matches!(first_char, 't' | 'T' | 'r' | 'R' | 'b' | 'B' | 'd' | 'D' | 'O' | 'C' | 'F' | 'V' | 'N' | 'S' | 's' | 'W' | 'M' | 'm' | 'A' | 'L' | 'Z' | 'z') {
            results.push(UartProbeResult {
                protocol_name: "SLCAN".to_string(),
                description: "CAN bus ASCII protocol (Lawicel)".to_string(),
                confidence: 0.75,
                evidence: format!("Start character '{}' matches the SLCAN command set", first_char),
            });
        }
    }

    // JSON 检测
    if data[0] == b'{' || data[0] == b'[' {
        if let Ok(text) = std::str::from_utf8(data) {
            if text.trim_end().ends_with('}') || text.trim_end().ends_with(']') {
                results.push(UartProbeResult {
                    protocol_name: "JSON".to_string(),
                    description: "JSON data stream".to_string(),
                    confidence: 0.85,
                    evidence: format!("Valid JSON structure ({} bytes)", data.len()),
                });
            }
        }
    }

    // AT 命令响应检测
    if let Ok(text) = std::str::from_utf8(data) {
        let upper = text.to_uppercase();
        if upper.contains("OK\r\n") || upper.contains("ERROR\r\n") {
            results.push(UartProbeResult {
                protocol_name: "AT commands".to_string(),
                description: "AT command device response".to_string(),
                confidence: 0.8,
                evidence: "Detected OK/ERROR response".to_string(),
            });
        }
    }

    // SCPI 检测
    if let Ok(text) = std::str::from_utf8(data) {
        if text.contains(',') && text.contains('\n') {
            let fields: Vec<&str> = text.split(',').collect();
            if fields.len() >= 2 && !fields[0].is_empty() && fields[0].len() < 40 {
                results.push(UartProbeResult {
                    protocol_name: "SCPI".to_string(),
                    description: "SCPI instrument response (comma-separated fields)".to_string(),
                    confidence: 0.6,
                    evidence: format!("Multi-field response: {}", fields.iter().take(3).cloned().collect::<Vec<_>>().join(",")),
                });
            }
        }
    }

    // 二进制帧头检测
    if data.len() >= 2 {
        let header = [data[0], data[1]];
        match header {
            [0xAA, 0x55] => {
                // 优先匹配设备专用协议（仅设备工作区构建）
                #[cfg(feature = "device-workspaces")]
                let insole_result = probe_insole_protocol(data);
                #[cfg(not(feature = "device-workspaces"))]
                let insole_result: Option<UartProbeResult> = None;
                if let Some(r) = insole_result {
                    results.push(r);
                } else {
                    results.push(UartProbeResult {
                        protocol_name: "Custom binary".to_string(),
                        description: "Header AA 55 (common industrial protocol)".to_string(),
                        confidence: 0.6,
                        evidence: "Detected 0xAA 0x55 header".to_string(),
                    });
                }
            }
            [0x55, 0xAA] => {
                results.push(UartProbeResult {
                    protocol_name: "Custom binary".to_string(),
                    description: "Header 55 AA (common industrial protocol)".to_string(),
                    confidence: 0.6,
                    evidence: "Detected 0x55 0xAA header".to_string(),
                });
            }
            [0x7E, _] => {
                results.push(UartProbeResult {
                    protocol_name: "HDLC/AX.25".to_string(),
                    description: "Header 7E (HDLC flag)".to_string(),
                    confidence: 0.7,
                    evidence: "Detected 0x7E HDLC flag byte".to_string(),
                });
            }
            _ => {}
        }
    }

    // 设备专用协议扫描（仅设备工作区构建）
    #[cfg(feature = "device-workspaces")]
    {
        if !(data.len() >= 2 && data[0] == 0xAA && data[1] == 0x55) {
            if let Some(insole_result) = probe_insole_protocol(data) {
                results.push(insole_result);
            }
        }
    }

    // MIDI 检测：状态字节 0x80-0xEF
    if data[0] >= 0x80 && data[0] <= 0xEF && data.len() >= 3 {
        let status = data[0] & 0xF0;
        if matches!(status, 0x80 | 0x90 | 0xA0 | 0xB0 | 0xC0 | 0xD0 | 0xE0) {
            results.push(UartProbeResult {
                protocol_name: "MIDI".to_string(),
                description: "MIDI message (Musical Instrument Digital Interface)".to_string(),
                confidence: 0.65,
                evidence: format!("Status byte 0x{:02X} ({})", data[0], match status {
                    0x80 => "Note Off",
                    0x90 => "Note On",
                    0xA0 => "Poly Pressure",
                    0xB0 => "Control Change",
                    0xC0 => "Program Change",
                    0xD0 => "Channel Pressure",
                    0xE0 => "Pitch Bend",
                    _ => "Unknown",
                }),
            });
        }
    }

    // DMX512 检测：起始码 0x00，帧间隔 Break
    if data[0] == 0x00 && data.len() >= 24 {
        results.push(UartProbeResult {
            protocol_name: "DMX512".to_string(),
            description: "DMX512 stage lighting control".to_string(),
            confidence: 0.5,
            evidence: "Start code 0x00 + long frame characteristics".to_string(),
        });
    }

    // ASCII 文本检测
    let printable_count = data.iter().filter(|&&b| b >= 0x20 && b < 0x7F || b == b'\r' || b == b'\n' || b == b'\t').count();
    let printable_ratio = printable_count as f32 / data.len() as f32;
    if printable_ratio > 0.9 && results.is_empty() {
        results.push(UartProbeResult {
            protocol_name: "ASCII text".to_string(),
            description: format!("Plain text data ({:.0}% printable)", printable_ratio * 100.0),
            confidence: 0.4,
            evidence: format!("First 40 bytes: {}", String::from_utf8_lossy(&data[..data.len().min(40)])),
        });
    }

    results
}

#[cfg(feature = "device-workspaces")]
include!("bus_probe_device.rs");

// ============================================================================
// CH341 USB 直连命令协议
// ============================================================================

/// CH341 USB 命令码
pub mod ch341 {
    /// CH341 功能码
    pub const CH341_VERSION: u8 = 0x5A;
    pub const CH341_ECHO: u8 = 0x5B;
    pub const CH341_GET_STATUS: u8 = 0x52;
    pub const CH341_WRITE_REG: u8 = 0x53;
    pub const CH341_READ_REG: u8 = 0x54;
    pub const CH341_I2C_COMMAND: u8 = 0xAA;
    pub const CH341_SPI_COMMAND: u8 = 0xAB;
    pub const CH341_PARA_COMMAND: u8 = 0xAC;

    /// I2C 流命令子码
    pub const I2C_STM_START: u8 = 0x74;
    pub const I2C_STM_STOP: u8 = 0x75;
    pub const I2C_STM_OUT: u8 = 0x80; // 输出数据，bit0=0 表示 ACK
    pub const I2C_STM_IN: u8 = 0xC0;  // 读入数据
    pub const I2C_STM_MAX: u8 = 0x3F; // 单次最大 63 字节
    pub const I2C_STM_US: u8 = 0x40;  // 微秒延时
    pub const I2C_STM_MS: u8 = 0x50;  // 毫秒延时
    pub const I2C_STM_DLY: u8 = 0x0F; // 延时参数
    pub const I2C_STM_END: u8 = 0x00; // 流结束

    /// CH341 I2C 速度设置
    pub const I2C_SPEED_LOW: u8 = 0;    // 20kHz
    pub const I2C_SPEED_NORMAL: u8 = 1; // 100kHz
    pub const I2C_SPEED_FAST: u8 = 2;   // 400kHz
    pub const I2C_SPEED_HIGH: u8 = 3;   // 750kHz

    /// 构建 CH341 I2C 扫描命令
    /// 对指定地址发送 START + 写地址 + STOP，根据 ACK 判断设备存在
    pub fn build_i2c_scan_cmd(addr: u8) -> Vec<u8> {
        vec![
            I2C_STM_START,
            I2C_STM_OUT, // 输出方向
            (addr << 1) & 0xFE, // 7-bit 地址左移1位，写模式
            I2C_STM_STOP,
            I2C_STM_END,
        ]
    }

    /// 构建 CH341 I2C 读命令
    pub fn build_i2c_read_cmd(addr: u8, len: u8) -> Vec<u8> {
        let len = len.min(I2C_STM_MAX);
        vec![
            I2C_STM_START,
            I2C_STM_OUT,
            (addr << 1) | 0x01, // 读模式
            I2C_STM_IN | len,
            I2C_STM_STOP,
            I2C_STM_END,
        ]
    }

    /// 构建 CH341 I2C 写命令
    pub fn build_i2c_write_cmd(addr: u8, data: &[u8]) -> Vec<u8> {
        let mut cmd = vec![I2C_STM_START, I2C_STM_OUT, (addr << 1) & 0xFE];
        for &b in data.iter().take(I2C_STM_MAX as usize) {
            cmd.push(b);
        }
        cmd.push(I2C_STM_STOP);
        cmd.push(I2C_STM_END);
        cmd
    }

    /// 构建 CH341 SPI 命令
    pub fn build_spi_cmd(data_out: &[u8]) -> Vec<u8> {
        let mut cmd = Vec::with_capacity(data_out.len() + 2);
        cmd.push(0x80); // SPI 数据输出
        cmd.extend_from_slice(data_out);
        cmd.push(0x00); // 结束
        cmd
    }

    /// 构建 SPI Flash JEDEC ID 读取命令 (0x9F)
    pub fn build_spi_jedec_cmd() -> Vec<u8> {
        build_spi_cmd(&[0x9F, 0x00, 0x00, 0x00, 0x00, 0x00])
    }

    /// CH341 速度码 → 实际频率
    pub fn i2c_speed_hz(speed: u8) -> u32 {
        match speed {
            I2C_SPEED_LOW => 20_000,
            I2C_SPEED_NORMAL => 100_000,
            I2C_SPEED_FAST => 400_000,
            I2C_SPEED_HIGH => 750_000,
            _ => 100_000,
        }
    }
}

// ============================================================================
// FT232H MPSSE 命令协议
// ============================================================================

/// FT232H MPSSE 命令码
pub mod mpsse {
    /// MPSSE 命令
    pub const SET_BITS_LOW: u8 = 0x80;  // 设置低字节 GPIO
    pub const SET_BITS_HIGH: u8 = 0x82; // 设置高字节 GPIO
    pub const GET_BITS_LOW: u8 = 0x81;  // 读取低字节 GPIO
    pub const GET_BITS_HIGH: u8 = 0x83; // 读取高字节 GPIO
    pub const SET_TCK_DIVISOR: u8 = 0x86; // 设置时钟分频
    pub const SEND_IMMEDIATE: u8 = 0x87; // 立即返回数据
    pub const LOOPBACK_START: u8 = 0x84; // 启用回环
    pub const LOOPBACK_END: u8 = 0x85;   // 禁用回环
    pub const CLOCK_DATA_OUT: u8 = 0x11; // 时钟输出（负沿）
    pub const CLOCK_DATA_IN: u8 = 0x24;  // 时钟输入（负沿）
    pub const CLOCK_DATA_RDWR: u8 = 0x31; // 时钟读写（负沿）

    /// I2C GPIO 引脚定义（FT232H）
    /// SCL = ADBUS0 (Pin 0)
    /// SDA_OUT = ADBUS1 (Pin 1)
    /// SDA_IN = ADBUS2 (Pin 2)
    pub const SCL_BIT: u8 = 0x01;
    pub const SDA_OUT_BIT: u8 = 0x02;
    pub const SDA_IN_BIT: u8 = 0x04;

    /// 构建 MPSSE 设置时钟分频命令
    /// 频率 = 6MHz / ((1 + divisor) * 2) 或 30MHz / ((1 + divisor) * 2)（X 系列）
    pub fn build_set_clock_cmd(divisor: u16) -> Vec<u8> {
        vec![SET_TCK_DIVISOR, (divisor & 0xFF) as u8, (divisor >> 8) as u8]
    }

    /// 构建 MPSSE 设置 GPIO 命令
    pub fn build_set_gpio_cmd(direction: u8, value: u8) -> Vec<u8> {
        vec![SET_BITS_LOW, value, direction]
    }

    /// 构建 MPSSE 读取 GPIO 命令
    pub fn build_read_gpio_cmd() -> Vec<u8> {
        vec![GET_BITS_LOW, SEND_IMMEDIATE]
    }

    /// 构建 MPSSE SPI 输出命令
    pub fn build_spi_write_cmd(data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }
        let len = (data.len() - 1) as u16;
        vec![
            CLOCK_DATA_OUT,
            (len & 0xFF) as u8,
            (len >> 8) as u8,
        ]
        .into_iter()
        .chain(data.iter().copied())
        .collect()
    }

    /// 构建 MPSSE SPI 读写命令（全双工）
    pub fn build_spi_xfer_cmd(data_out: &[u8]) -> Vec<u8> {
        if data_out.is_empty() {
            return Vec::new();
        }
        let len = (data_out.len() - 1) as u16;
        vec![
            CLOCK_DATA_RDWR,
            (len & 0xFF) as u8,
            (len >> 8) as u8,
        ]
        .into_iter()
        .chain(data_out.iter().copied())
        .collect()
    }

    /// 构建 MPSSE SPI 读取命令（输出 0x00 作为时钟源）
    pub fn build_spi_read_cmd(len: usize) -> Vec<u8> {
        if len == 0 {
            return Vec::new();
        }
        let count = (len - 1) as u16;
        vec![
            CLOCK_DATA_IN,
            (count & 0xFF) as u8,
            (count >> 8) as u8,
            SEND_IMMEDIATE,
        ]
    }

    /// 构建 SPI Flash JEDEC ID 读取命令序列
    /// 发送 0x9F，然后读取 3 字节（厂商 ID + 容量 ID + 类型）
    pub fn build_spi_jedec_cmds() -> Vec<Vec<u8>> {
        vec![
            build_spi_write_cmd(&[0x9F]),
            build_spi_read_cmd(3),
        ]
    }

    /// MPSSE 分频值 → 实际频率
    pub fn clock_hz(divisor: u16) -> u32 {
        6_000_000 / ((1 + divisor as u32) * 2)
    }

    /// 根据目标频率计算分频值
    pub fn divisor_for_hz(hz: u32) -> u16 {
        if hz >= 3_000_000 {
            return 0;
        }
        // hz=0 会除零 panic；极小频率时除法结果截断为 u16 会产生错误分频
        if hz == 0 {
            return 0xFFFF;
        }
        ((6_000_000 / hz) / 2).saturating_sub(1).min(0xFFFF) as u16
    }
}

// ============================================================================
// SWD (Serial Wire Debug) 命令
// ============================================================================

/// SWD 命令
pub mod swd {
    /// SWD-AP IDCODE 寄存器地址
    pub const DP_IDCODE: u8 = 0x00;

    /// 构建 SWD 读 IDCODE 命令序列
    /// SWD 协议：线复位（>= 50 个高电平时钟）+ JTAG→SWD 切换序列 + 读 IDCODE
    pub fn build_read_idcode_cmds() -> Vec<Vec<u8>> {
        // 线复位：>= 50 个 1
        let reset: Vec<u8> = vec![0xFF; 8]; // 64 个 1
        // JTAG→SWD 切换序列：0111 1001 1110 0111 (0x79E7)
        let switch: Vec<u8> = vec![0x79, 0xE7];
        // 线复位：>= 50 个 1
        let reset2: Vec<u8> = vec![0xFF; 8];
        // 读 DPIDR：0xA5 (START=1, APnDP=0, RnW=1, A=00, Parity=0, Stop=0, Park=1)
        let read_id: Vec<u8> = vec![0xA5];
        vec![reset, switch, reset2, read_id]
    }

    /// SWD IDCODE 厂商识别
    pub fn identify_swd_idcode(idcode: u32) -> Option<(&'static str, &'static str)> {
        // SWD IDCODE 格式与 JTAG 相同
        // IDCODE: [31:28] Version | [27:12] Part Number | [11:1] Manufacturer | [0] 1
        // ARM Cortex-M 的 IDCODE 中 part_no 包含 0xBAxx，制造商 ID 为 0x23B (0x476 的 JEP106 ARM 编码)
        let part_no = (idcode >> 12) & 0xFFFF;
        let bit0 = idcode & 1;

        // 无效值
        if idcode == 0 || idcode == 0xFFFFFFFF || bit0 != 1 {
            return None;
        }

        // ARM Cortex-M 系列的 Part Number 以 0xBA 开头
        if (part_no & 0xFF00) == 0xBA00 {
            let core = match part_no & 0xFF {
                0x95 => "Cortex-M3 SW-DP",
                0x01 => "Cortex-M0+ SW-DP",
                0x02 => "Cortex-M0 SW-DP",
                0x04 => "Cortex-M4 SW-DP",
                0x07 => "Cortex-M7 SW-DP",
                _ => "Cortex-M SW-DP (unknown model)",
            };
            return Some(("ARM", core));
        }

        // 尝试通过 JTAG 表匹配
        if let Some(info) = crate::bus_probe::lookup_jtag_chip(idcode) {
            return Some((info.manufacturer, info.part_number));
        }

        Some(("Unknown vendor", "SW-DP device"))
    }
}

// ============================================================================
// 1-Wire 协议命令
// ============================================================================

/// 1-Wire 命令
pub mod onewire {
    /// 1-Wire ROM 命令
    pub const SEARCH_ROM: u8 = 0xF0;
    pub const READ_ROM: u8 = 0x33;
    pub const MATCH_ROM: u8 = 0x55;
    pub const SKIP_ROM: u8 = 0xCC;
    pub const ALARM_SEARCH: u8 = 0xEC;

    /// 1-Wire 函数命令（DS18B20 为例）
    pub const CONVERT_T: u8 = 0x44;      // 启动温度转换
    pub const READ_SCRATCHPAD: u8 = 0xBE; // 读取暂存器
    pub const WRITE_SCRATCHPAD: u8 = 0x4E;
    pub const COPY_SCRATCHPAD: u8 = 0x48;
    pub const READ_POWER: u8 = 0xB4;

    /// 构建 1-Wire 读 ROM 命令（用于单设备总线）
    pub fn build_read_rom_cmd() -> Vec<u8> {
        vec![READ_ROM]
    }

    /// 构建 1-Wire 搜索 ROM 命令（用于多设备总线）
    pub fn build_search_rom_cmd() -> Vec<u8> {
        vec![SEARCH_ROM]
    }

    /// 解析 1-Wire ROM 编号
    /// ROM = [8字节] = [家族码1] + [序列号6] + [CRC1]
    pub fn parse_rom(rom: &[u8; 8]) -> (u8, u64, u8) {
        let family = rom[0];
        let serial: u64 = rom[1..7].iter()
            .enumerate()
            .map(|(i, &b)| (b as u64) << (i * 8))
            .sum();
        let crc = rom[7];
        (family, serial, crc)
    }

    /// 1-Wire CRC8 计算（多项式 0x31）
    pub fn crc8(data: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &byte in data {
            crc ^= byte;
            for _ in 0..8 {
                if crc & 0x01 != 0 {
                    crc = (crc >> 1) ^ 0x8C; // 反向多项式 0x31 → 0x8C
                } else {
                    crc >>= 1;
                }
            }
        }
        crc
    }
}

// ============================================================================
// 深度探测结果汇总
// ============================================================================

/// 深度探测结果
#[derive(Debug, Clone)]
pub struct DeepProbeResult {
    /// 探测类型
    pub probe_type: ProbeType,
    /// 探测的名称
    pub name: String,
    /// 结果描述
    pub description: String,
    /// 置信度 0.0-1.0
    pub confidence: f32,
    /// 详细信息（JSON 或文本）
    pub details: String,
    /// 原始数据
    pub raw_data: Vec<u8>,
}

/// 探测类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeType {
    UsbBridge,
    I2cScan,
    SpiFlash,
    JtagScan,
    OnewireScan,
    SwdScan,
    ModbusScan,
    UartProbe,
    ChipBootloader,
}

impl ProbeType {
    pub fn label(&self) -> &'static str {
        match self {
            ProbeType::UsbBridge => "USB bridge chip",
            ProbeType::I2cScan => "I2C bus scan",
            ProbeType::SpiFlash => "SPI Flash identification",
            ProbeType::JtagScan => "JTAG scan",
            ProbeType::OnewireScan => "1-Wire search",
            ProbeType::SwdScan => "SWD identification",
            ProbeType::ModbusScan => "Modbus slave scan",
            ProbeType::UartProbe => "UART protocol analysis",
            ProbeType::ChipBootloader => "Chip bootloader probe",
        }
    }
}

// ============================================================================
// IDCODE 字段详细解析
// ============================================================================

/// IDCODE 字段解析结果
#[derive(Debug, Clone)]
pub struct IdcodeFields {
    /// 原始 IDCODE 值
    pub raw: u32,
    /// [31:28] 版本号
    pub version: u8,
    /// [27:12] 部件编号（16 bit）
    pub part_number: u16,
    /// [11:1] 制造商 ID（11 bit JEP106）
    pub manufacturer_id: u16,
    /// bit[0] 固定值（必须为 1）
    pub bit0: u8,
    /// 制造商名称
    pub manufacturer_name: &'static str,
}

/// 解析 32 位 IDCODE 的各字段
pub fn parse_idcode(idcode: u32) -> IdcodeFields {
    let mfg_id = ((idcode >> 1) & 0x7FF) as u16;
    IdcodeFields {
        raw: idcode,
        version: ((idcode >> 28) & 0xF) as u8,
        part_number: ((idcode >> 12) & 0xFFFF) as u16,
        manufacturer_id: mfg_id,
        bit0: (idcode & 1) as u8,
        manufacturer_name: jtag_manufacturer_name(mfg_id),
    }
}

// ============================================================================
// 芯片详细技术信息表
// ============================================================================

/// 芯片详细技术信息
#[derive(Debug, Clone)]
pub struct ChipDetailInfo {
    /// 芯片型号
    pub chip_name: &'static str,
    /// 制造商
    pub manufacturer: &'static str,
    /// CPU 架构
    pub architecture: &'static str,
    /// 调试接口类型
    pub debug_interface: &'static str,
    /// Flash 起始地址
    pub flash_base: u32,
    /// Flash 容量（KB），0 表示未知
    pub flash_size_kb: u32,
    /// RAM 起始地址
    pub ram_base: u32,
    /// RAM 容量（KB），0 表示未知
    pub ram_size_kb: u32,
    /// 支持的烧录协议
    pub flash_protocol: &'static str,
    /// Bootloader 类型
    pub bootloader: &'static str,
    /// 芯片特性描述
    pub features: &'static str,
    /// 对应的烧录器面板类型
    pub flasher_target: FlasherTarget,
}

/// 烧录器面板支持的目标类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlasherTarget {
    /// STM32 系统存储 Bootloader (UART)
    Stm32Bootloader,
    /// ESP32 ROM Bootloader (UART SLIP)
    Esp32RomBootloader,
    /// ESP32 AT 固件
    Esp32AtFirmware,
    /// Arduino STK500 (AVR)
    ArduinoStk500,
    /// 通用 UART 烧录
    GenericUart,
    /// JTAG 直接烧录（需 JTAG 适配器）
    JtagDirect,
    /// SWD 直接烧录（需 SWD 适配器）
    SwdDirect,
    /// ESP8266 ROM Bootloader (UART SLIP, 与 ESP32 协议相近但部分命令不同)
    Esp8266RomBootloader,
    /// STM8 UART Bootloader (UM0560)
    Stm8Bootloader,
    /// NXP LPC ISP UART Bootloader (UM10295, ASCII 命令)
    NxpLpcIsp,
    /// GD32 UART Bootloader (兼容 STM32 协议)
    Gd32Bootloader,
    /// CH32 WCH 自定义 ISP 协议 (CH32F103/CH32V103/CH552/CH573/CH582)
    Ch32Isp,
    /// Microchip PIC ICSP 协议 (需 PicKit/ICD 硬件)
    PicIcsp,
    /// 开发板 SD 卡镜像烧录 (树莓派/Jetson Nano/Orange Pi 等)
    SdCardImage,
    /// 不支持直接烧录
    NotSupported,
}

/// 常见芯片的详细技术信息表
const CHIP_DETAIL_TABLE: &[ChipDetailInfo] = &[
    ChipDetailInfo {
        chip_name: "STM32F103xB",
        manufacturer: "STMicroelectronics",
        architecture: "ARM Cortex-M3 @ 72MHz",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 128,
        ram_base: 0x20000000,
        ram_size_kb: 20,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader",
        features: "Medium density, USB 2.0 FS, CAN, 3x ADC, 4x Timer, widely used in industrial control",
        flasher_target: FlasherTarget::Stm32Bootloader,
    },
    ChipDetailInfo {
        chip_name: "STM32F103x6",
        manufacturer: "STMicroelectronics",
        architecture: "ARM Cortex-M3 @ 72MHz",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 32,
        ram_base: 0x20000000,
        ram_size_kb: 10,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader",
        features: "Small density, low-cost solution, suitable for simple control",
        flasher_target: FlasherTarget::Stm32Bootloader,
    },
    ChipDetailInfo {
        chip_name: "STM32F103xE",
        manufacturer: "STMicroelectronics",
        architecture: "ARM Cortex-M3 @ 72MHz",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 512,
        ram_base: 0x20000000,
        ram_size_kb: 64,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader",
        features: "High density, USB 2.0 FS, SDIO, FSMC, suitable for complex applications",
        flasher_target: FlasherTarget::Stm32Bootloader,
    },
    ChipDetailInfo {
        chip_name: "STM32F407",
        manufacturer: "STMicroelectronics",
        architecture: "ARM Cortex-M4F @ 168MHz",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 1024,
        ram_base: 0x20000000,
        ram_size_kb: 192,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader",
        features: "High-performance F4 series, FPU, USB OTG, Ethernet MAC, 3x ADC",
        flasher_target: FlasherTarget::Stm32Bootloader,
    },
    ChipDetailInfo {
        chip_name: "ESP32",
        manufacturer: "Espressif",
        architecture: "Xtensa LX6 dual-core @ 240MHz",
        debug_interface: "JTAG (built-in) + UART",
        flash_base: 0x00000000,
        flash_size_kb: 0, // 外部 SPI Flash，容量不固定
        ram_base: 0x3FFAE000,
        ram_size_kb: 520,
        flash_protocol: "ROM Bootloader (SLIP, SYNC 07 07 12 20)",
        bootloader: "ROM Bootloader (fixed, non-modifiable)",
        features: "WiFi + BLE, dual-core 240MHz, 16MB external Flash, rich peripherals, mainstream IoT solution",
        flasher_target: FlasherTarget::Esp32RomBootloader,
    },
    ChipDetailInfo {
        chip_name: "ESP32-S2",
        manufacturer: "Espressif",
        architecture: "Xtensa LX7 @ 240MHz",
        debug_interface: "JTAG + USB-OTG + UART",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0x3FFB0000,
        ram_size_kb: 320,
        flash_protocol: "ROM Bootloader (SLIP)",
        bootloader: "ROM Bootloader",
        features: "Native USB-OTG support, secure boot, suitable for USB peripheral development",
        flasher_target: FlasherTarget::Esp32RomBootloader,
    },
    ChipDetailInfo {
        chip_name: "ESP32-C3",
        manufacturer: "Espressif",
        architecture: "RISC-V @ 160MHz",
        debug_interface: "JTAG + UART",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0x3FC80000,
        ram_size_kb: 400,
        flash_protocol: "ROM Bootloader (SLIP)",
        bootloader: "ROM Bootloader",
        features: "RISC-V architecture, WiFi 4 + BLE 5, low power, native USB-CDC support",
        flasher_target: FlasherTarget::Esp32RomBootloader,
    },
    ChipDetailInfo {
        chip_name: "Arduino Uno (ATmega328P)",
        manufacturer: "Atmel/Microchip",
        architecture: "AVR 8-bit @ 16MHz",
        debug_interface: "STK500 (UART)",
        flash_base: 0x00000000,
        flash_size_kb: 32,
        ram_base: 0x01000000,
        ram_size_kb: 2,
        flash_protocol: "STK500 v1 (0x30 0x20 → 0x14 0x10)",
        bootloader: "Optiboot (512B boot area)",
        features: "Classic Arduino platform, 5V logic, SPI/I2C/UART, rich open-source ecosystem",
        flasher_target: FlasherTarget::ArduinoStk500,
    },
    ChipDetailInfo {
        chip_name: "Arduino Mega (ATmega2560)",
        manufacturer: "Atmel/Microchip",
        architecture: "AVR 8-bit @ 16MHz",
        debug_interface: "STK500 v2 (UART)",
        flash_base: 0x00000000,
        flash_size_kb: 256,
        ram_base: 0x02000000,
        ram_size_kb: 8,
        flash_protocol: "STK500 v1/v2",
        bootloader: "STK500 v2 Bootloader",
        features: "4x UART, 16MHz, abundant GPIO, suitable for 3D printers/multi-serial applications",
        flasher_target: FlasherTarget::ArduinoStk500,
    },
    ChipDetailInfo {
        chip_name: "Cortex-M3 (generic)",
        manufacturer: "ARM",
        architecture: "ARM Cortex-M3",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 0,
        ram_base: 0x20000000,
        ram_size_kb: 0,
        flash_protocol: "SWD/JTAG direct Flash read/write",
        bootloader: "Depends on the chip vendor implementation",
        features: "Generic ARM Cortex-M3 core, specific parameters depend on the chip vendor implementation",
        flasher_target: FlasherTarget::SwdDirect,
    },
    ChipDetailInfo {
        chip_name: "Cortex-M4 (generic)",
        manufacturer: "ARM",
        architecture: "ARM Cortex-M4F",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 0,
        ram_base: 0x20000000,
        ram_size_kb: 0,
        flash_protocol: "SWD/JTAG direct Flash read/write",
        bootloader: "Depends on the chip vendor implementation",
        features: "Cortex-M4 with single-precision FPU, suitable for DSP applications",
        flasher_target: FlasherTarget::SwdDirect,
    },
    ChipDetailInfo {
        chip_name: "Cortex-M0+ (generic)",
        manufacturer: "ARM",
        architecture: "ARM Cortex-M0+",
        debug_interface: "SWD (JTAG optional)",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0x20000000,
        ram_size_kb: 0,
        flash_protocol: "SWD direct read/write",
        bootloader: "Depends on the chip vendor implementation",
        features: "Ultra-low power, smallest ARM core, suitable for IoT end nodes",
        flasher_target: FlasherTarget::SwdDirect,
    },
    ChipDetailInfo {
        chip_name: "Spartan-3 FPGA",
        manufacturer: "Xilinx",
        architecture: "FPGA (programmable logic)",
        debug_interface: "JTAG",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0,
        ram_size_kb: 0,
        flash_protocol: "JTAG Direct (bitstream download)",
        bootloader: "None (FPGA loads bitstream directly)",
        features: "Programmable logic device, configuration stored in external SPI Flash",
        flasher_target: FlasherTarget::JtagDirect,
    },
    ChipDetailInfo {
        chip_name: "Cyclone IV FPGA",
        manufacturer: "Altera/Intel",
        architecture: "FPGA (programmable logic)",
        debug_interface: "JTAG",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0,
        ram_size_kb: 0,
        flash_protocol: "JTAG Direct (.sof/.pof file)",
        bootloader: "None",
        features: "Altera Cyclone IV, uses EPCQ Flash to store configuration",
        flasher_target: FlasherTarget::JtagDirect,
    },
    // ---- ESP8266 ----
    ChipDetailInfo {
        chip_name: "ESP8266",
        manufacturer: "Espressif",
        architecture: "Tensilica L106 32-bit @ 80MHz",
        debug_interface: "UART (no JTAG)",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0x3FFE8000,
        ram_size_kb: 80,
        flash_protocol: "ROM Bootloader (SLIP, 0x08 SYNC)",
        bootloader: "ROM Bootloader",
        features: "WiFi 4, single-core 80MHz, external SPI Flash, classic IoT solution",
        flasher_target: FlasherTarget::Esp8266RomBootloader,
    },
    ChipDetailInfo {
        chip_name: "ESP8266EX",
        manufacturer: "Espressif",
        architecture: "Tensilica L106 32-bit @ 80MHz",
        debug_interface: "UART",
        flash_base: 0x00000000,
        flash_size_kb: 0,
        ram_base: 0x3FFE8000,
        ram_size_kb: 80,
        flash_protocol: "ROM Bootloader (SLIP)",
        bootloader: "ROM Bootloader",
        features: "Core of ESP-12/ESP-01 modules, ultra-low-cost WiFi solution",
        flasher_target: FlasherTarget::Esp8266RomBootloader,
    },
    // ---- STM8 ----
    ChipDetailInfo {
        chip_name: "STM8S103",
        manufacturer: "STMicroelectronics",
        architecture: "STM8 8-bit @ 16MHz",
        debug_interface: "SWIM + UART",
        flash_base: 0x008000,
        flash_size_kb: 8,
        ram_base: 0x000000,
        ram_size_kb: 1,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader (UM0560)",
        features: "Low-cost 8-bit MCU, UART/SPI/I2C, 5V tolerant",
        flasher_target: FlasherTarget::Stm8Bootloader,
    },
    ChipDetailInfo {
        chip_name: "STM8S207",
        manufacturer: "STMicroelectronics",
        architecture: "STM8 8-bit @ 16MHz",
        debug_interface: "SWIM + UART",
        flash_base: 0x008000,
        flash_size_kb: 128,
        ram_base: 0x000000,
        ram_size_kb: 6,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader (UM0560)",
        features: "High-performance STM8, multiple UARTs, CAN 2.0B, 10-bit ADC",
        flasher_target: FlasherTarget::Stm8Bootloader,
    },
    ChipDetailInfo {
        chip_name: "STM8L051",
        manufacturer: "STMicroelectronics",
        architecture: "STM8L 8-bit @ 16MHz",
        debug_interface: "SWIM + UART",
        flash_base: 0x008000,
        flash_size_kb: 8,
        ram_base: 0x000000,
        ram_size_kb: 1,
        flash_protocol: "UART Bootloader (0x7F → 0x79)",
        bootloader: "System Memory Bootloader (UM0560)",
        features: "Ultra-low-power STM8L series, suitable for battery-powered scenarios",
        flasher_target: FlasherTarget::Stm8Bootloader,
    },
    // ---- NXP LPC ----
    ChipDetailInfo {
        chip_name: "LPC1768",
        manufacturer: "NXP",
        architecture: "ARM Cortex-M3 @ 96MHz",
        debug_interface: "JTAG + SWD + ISP(UART)",
        flash_base: 0x00000000,
        flash_size_kb: 512,
        ram_base: 0x10000000,
        ram_size_kb: 64,
        flash_protocol: "ISP UART (UM10295, '?' Synchronized → ASCII commands)",
        bootloader: "ROM ISP Bootloader",
        features: "Ethernet MAC, USB 2.0, CAN, 3x ADC, classic mbed platform",
        flasher_target: FlasherTarget::NxpLpcIsp,
    },
    ChipDetailInfo {
        chip_name: "LPC2148",
        manufacturer: "NXP",
        architecture: "ARM7TDMI-S @ 60MHz",
        debug_interface: "JTAG + ISP(UART)",
        flash_base: 0x00000000,
        flash_size_kb: 512,
        ram_base: 0x40000000,
        ram_size_kb: 40,
        flash_protocol: "ISP UART (UM10295)",
        bootloader: "ROM ISP Bootloader",
        features: "Classic ARM7 chip, USB 2.0 Device, dual ADC",
        flasher_target: FlasherTarget::NxpLpcIsp,
    },
    ChipDetailInfo {
        chip_name: "LPC546xx",
        manufacturer: "NXP",
        // 修正：LPC546xx 主频应为 220MHz（参考 NXP DS11590 数据手册），原 180MHz 为同系列 LPC540xx 主频
        architecture: "ARM Cortex-M4F @ 220MHz",
        debug_interface: "JTAG + SWD + ISP(UART)",
        flash_base: 0x00000000,
        flash_size_kb: 512,
        ram_base: 0x20000000,
        ram_size_kb: 192,
        flash_protocol: "ISP UART (UM10295)",
        bootloader: "ROM ISP Bootloader",
        features: "FPU, Ethernet, USB 2.0 HS/FS, LCD controller",
        flasher_target: FlasherTarget::NxpLpcIsp,
    },
    // ---- GD32 (国产 STM32 兼容) ----
    ChipDetailInfo {
        chip_name: "GD32F103",
        manufacturer: "GigaDevice",
        architecture: "ARM Cortex-M3 @ 108MHz",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 64,
        ram_base: 0x20000000,
        ram_size_kb: 20,
        flash_protocol: "UART Bootloader (STM32-compatible, 0x7F)",
        bootloader: "System Memory Bootloader",
        features: "Pin-compatible domestic alternative to STM32F103, higher clock (108MHz)",
        flasher_target: FlasherTarget::Gd32Bootloader,
    },
    ChipDetailInfo {
        chip_name: "GD32F4xx",
        manufacturer: "GigaDevice",
        architecture: "ARM Cortex-M4F @ 168MHz",
        debug_interface: "JTAG + SWD",
        flash_base: 0x08000000,
        flash_size_kb: 1024,
        ram_base: 0x20000000,
        ram_size_kb: 192,
        flash_protocol: "UART Bootloader (STM32-compatible, 0x7F)",
        bootloader: "System Memory Bootloader",
        features: "STM32F4-compatible domestic alternative, FPU, USB OTG",
        flasher_target: FlasherTarget::Gd32Bootloader,
    },
    // ---- CH32 (WCH 国产芯片) ----
    ChipDetailInfo {
        chip_name: "CH32F103",
        manufacturer: "WCH (Qinheng)",
        architecture: "ARM Cortex-M3 @ 72MHz",
        debug_interface: "SWD + WCH-Link",
        flash_base: 0x08000000,
        flash_size_kb: 64,
        ram_base: 0x20000000,
        ram_size_kb: 20,
        flash_protocol: "WCH ISP (0x57 0xAB frame, USB/UART)",
        bootloader: "WCH Bootloader (built-in ROM)",
        features: "STM32F103-compatible domestic alternative, supports USB device/host",
        flasher_target: FlasherTarget::Ch32Isp,
    },
    ChipDetailInfo {
        chip_name: "CH32V103",
        manufacturer: "WCH (Qinheng)",
        architecture: "RISC-V (Qingke V4) @ 72MHz",
        debug_interface: "WCH-Link (RISC-V Debug)",
        flash_base: 0x08000000,
        flash_size_kb: 64,
        ram_base: 0x20000000,
        ram_size_kb: 20,
        flash_protocol: "WCH ISP (0x57 0xAB frame)",
        bootloader: "WCH Bootloader",
        features: "RISC-V domestic MCU, STM32 pin-compatible, ultra-low cost",
        flasher_target: FlasherTarget::Ch32Isp,
    },
    ChipDetailInfo {
        chip_name: "CH552",
        manufacturer: "WCH (Qinheng)",
        architecture: "E8051 @ 24MHz",
        debug_interface: "USB (built-in Bootloader)",
        flash_base: 0x000000,
        flash_size_kb: 16,
        ram_base: 0x000000,
        ram_size_kb: 1,
        flash_protocol: "WCH ISP (USB)",
        bootloader: "USB Bootloader (built-in)",
        features: "8051 core + USB 2.0 FS, ultra-low-cost USB device solution",
        flasher_target: FlasherTarget::Ch32Isp,
    },
    ChipDetailInfo {
        chip_name: "CH582",
        manufacturer: "WCH (Qinheng)",
        architecture: "RISC-V (Qingke V4) @ 40MHz",
        debug_interface: "WCH-Link + USB",
        flash_base: 0x00000000,
        flash_size_kb: 448,
        ram_base: 0x20000000,
        ram_size_kb: 32,
        flash_protocol: "WCH ISP (USB/UART)",
        bootloader: "WCH Bootloader",
        features: "RISC-V + BLE 5.3, USB 2.0 FS, low power",
        flasher_target: FlasherTarget::Ch32Isp,
    },
    // ---- PIC ----
    ChipDetailInfo {
        chip_name: "PIC18F4550",
        manufacturer: "Microchip",
        architecture: "PIC18 8-bit @ 48MHz",
        debug_interface: "ICSP (PGC/PGD) + USB",
        flash_base: 0x000000,
        flash_size_kb: 32,
        ram_base: 0x000000,
        ram_size_kb: 2,
        flash_protocol: "ICSP (6-bit commands, PicKit/ICD)",
        bootloader: "HID Bootloader (optional)",
        features: "USB 2.0 Device, self-programming, classic 8-bit USB MCU",
        flasher_target: FlasherTarget::PicIcsp,
    },
    ChipDetailInfo {
        chip_name: "PIC18F26K80",
        manufacturer: "Microchip",
        architecture: "PIC18 8-bit @ 64MHz",
        debug_interface: "ICSP",
        flash_base: 0x000000,
        flash_size_kb: 64,
        ram_base: 0x000000,
        // 修正：PIC18F26K80 实际 RAM 为 3648 bytes ≈ 3.6KB（参考 Microchip DS39977C），原标注 4KB 不准确
        ram_size_kb: 3,
        flash_protocol: "ICSP (PicKit 3/4)",
        bootloader: "None (requires ICSP programming)",
        features: "CAN 2.0B, LIN 2.1, EEPROM, commonly used in automotive electronics",
        flasher_target: FlasherTarget::PicIcsp,
    },
    ChipDetailInfo {
        chip_name: "PIC32MX",
        manufacturer: "Microchip",
        architecture: "MIPS32 M4K @ 80MHz",
        debug_interface: "ICSP (JTAG optional)",
        flash_base: 0x9D000000,
        flash_size_kb: 512,
        ram_base: 0xA0000000,
        ram_size_kb: 128,
        flash_protocol: "ICSP (PicKit 3/4, SNP)",
        bootloader: "Tiny Bootloader (optional)",
        features: "32-bit PIC, MIPS core, USB Device/Host",
        flasher_target: FlasherTarget::PicIcsp,
    },
    // ---- 开发板（SD 卡镜像烧录） ----
    ChipDetailInfo {
        chip_name: "Raspberry Pi 4B",
        manufacturer: "Raspberry Pi Foundation",
        architecture: "Quad-core ARM Cortex-A72 @ 1.5GHz",
        debug_interface: "UART (GPIO 14/15) + SD",
        flash_base: 0,
        flash_size_kb: 0,
        ram_base: 0,
        ram_size_kb: 0,
        flash_protocol: "SD card image flashing (img/iso → dd)",
        bootloader: "Bootloader on EEPROM (updatable)",
        features: "Linux SBC, 4GB RAM, WiFi/BT, Gigabit Ethernet, 2x USB 3.0",
        flasher_target: FlasherTarget::SdCardImage,
    },
    ChipDetailInfo {
        chip_name: "Raspberry Pi Zero W",
        manufacturer: "Raspberry Pi Foundation",
        architecture: "ARM11 @ 1GHz",
        debug_interface: "UART + SD",
        flash_base: 0,
        flash_size_kb: 0,
        ram_base: 0,
        ram_size_kb: 0,
        flash_protocol: "SD card image flashing",
        bootloader: "Bootloader on EEPROM",
        features: "Ultra-compact Linux SBC, 512MB RAM, WiFi/BT",
        flasher_target: FlasherTarget::SdCardImage,
    },
    ChipDetailInfo {
        chip_name: "Jetson Nano",
        manufacturer: "NVIDIA",
        architecture: "Quad-core ARM Cortex-A57 @ 1.43GHz + Maxwell GPU with 128 CUDA cores",
        debug_interface: "UART + USB Recovery",
        flash_base: 0,
        flash_size_kb: 0,
        ram_base: 0,
        ram_size_kb: 0,
        flash_protocol: "SD card image flashing / USB Recovery mode flashing",
        bootloader: "U-Boot on QSPI",
        features: "Edge AI platform, 4GB LPDDR4, 4K video decoding, CUDA acceleration",
        flasher_target: FlasherTarget::SdCardImage,
    },
    ChipDetailInfo {
        chip_name: "Orange Pi 3B",
        manufacturer: "Shenzhen Xunlong",
        architecture: "Quad-core ARM Cortex-A55 @ 2.0GHz (RK3566)",
        debug_interface: "UART + SD",
        flash_base: 0,
        flash_size_kb: 0,
        ram_base: 0,
        ram_size_kb: 0,
        flash_protocol: "SD card image flashing",
        bootloader: "U-Boot (SPL)",
        features: "RK3566 platform, 4GB RAM, Gigabit Ethernet, WiFi 6, NVMe SSD",
        flasher_target: FlasherTarget::SdCardImage,
    },
];

/// 根据芯片名称（部分匹配）查询详细技术信息
pub fn lookup_chip_detail(name: &str) -> Option<&'static ChipDetailInfo> {
    // 空名称在部分匹配中会命中任何条目（contains("") 恒为 true），直接拒绝
    if name.trim().is_empty() {
        return None;
    }
    // 精确匹配
    if let Some(info) = CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == name) {
        return Some(info);
    }
    // 部分匹配
    if let Some(info) = CHIP_DETAIL_TABLE.iter().find(|c| {
        c.chip_name.to_lowercase().contains(&name.to_lowercase())
            || name.to_lowercase().contains(&c.chip_name.to_lowercase())
    }) {
        return Some(info);
    }
    None
}

/// 根据制造商名称推断通用芯片信息
pub fn generic_chip_info(mfg: &str) -> Option<&'static ChipDetailInfo> {
    let mfg_lower = mfg.to_lowercase();
    if mfg_lower.contains("arm") || mfg_lower.contains("cortex") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name.contains("Cortex-M3 (generic)"))
    } else if mfg_lower.contains("stmicro") || mfg_lower.contains("stmicroelectronics") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name.starts_with("STM32F103xE"))
    } else if mfg_lower.contains("esp32") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "ESP32")
    } else if mfg_lower.contains("esp8266") || mfg_lower == "esp" {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "ESP8266")
    } else if mfg_lower.contains("espressif") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "ESP32")
    } else if mfg_lower.contains("atmel") || mfg_lower.contains("arduino") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name.contains("Arduino Uno"))
    } else if mfg_lower.contains("microchip") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "PIC18F4550")
    } else if mfg_lower.contains("nxp") || mfg_lower.contains("lpc") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "LPC1768")
    } else if mfg_lower.contains("gigadevice") || mfg_lower.contains("gd32") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "GD32F103")
    } else if mfg_lower.contains("wch") || mfg_lower.contains("ch32") || mfg_lower.contains("ch55") || mfg_lower.contains("ch58") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "CH32F103")
    } else if mfg_lower.contains("stm8") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "STM8S103")
    } else if mfg_lower.contains("raspberry") || mfg_lower.contains("raspberry pi") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "Raspberry Pi 4B")
    } else if mfg_lower.contains("nvidia") || mfg_lower.contains("jetson") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "Jetson Nano")
    } else if mfg_lower.contains("xunlong") || mfg_lower.contains("orange pi") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name == "Orange Pi 3B")
    } else if mfg_lower.contains("xilinx") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name.contains("Spartan-3"))
    } else if mfg_lower.contains("altera") {
        CHIP_DETAIL_TABLE.iter().find(|c| c.chip_name.contains("Cyclone IV"))
    } else {
        None
    }
}

// ============================================================================
// JEP106 制造商完整表（前 127 条）
// ============================================================================

/// JEP106 制造商码 → 名称（完整版）
pub fn jep106_manufacturer(code: u16) -> &'static str {
    match code {
        0x001 => "Altera/Intel",
        0x002 => "AMD",
        0x003 => "Analog Devices",
        0x006 => "Intel",
        0x009 => "Atmel/Microchip",
        0x00A => "Renesas",
        0x00E => "Motorola/Freescale/NXP",
        0x010 => "Philips/NXP",
        0x011 => "Xilinx",
        0x013 => "Qualcomm",
        0x015 => "Analog Devices",
        0x017 => "ARM",
        0x01F => "STMicroelectronics",
        0x020 => "STMicroelectronics",
        0x021 => "NXP (Philips)",
        0x024 => "Maxim Integrated",
        0x02A => "Texas Instruments",
        0x031 => "Cypress/Infineon",
        0x037 => "Microchip",
        0x049 => "Xilinx",
        0x062 => "Espressif",
        0x06E => "Altera/Intel",
        0x0A6 => "Realtek",
        0x0C1 => "Cypress/Infineon",
        0x0FF => "Gowin Semiconductor",
        _ => "Unknown (not in the common JEP106 table)",
    }
}

// ============================================================================
// MCU Boot ROM 探测（用于鞋垫/床垫等设备的主控芯片识别）
// ============================================================================

/// MCU Boot 探测结果
#[derive(Debug, Clone)]
pub struct McuBootProbeResult {
    /// 芯片厂商
    pub vendor: &'static str,
    /// 芯片系列
    pub chip_family: &'static str,
    /// 芯片描述
    pub description: String,
    /// 置信度 0.0-1.0
    pub confidence: f32,
    /// 证据
    pub evidence: String,
}

/// MCU Boot 探测项（各厂商 Boot ROM 同步字节）
///
/// 用于在检测到鞋垫/床垫等持续推送协议后，
/// 自动尝试各厂商 Boot ROM 同步字节识别主控 MCU 芯片类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McuBootProbeKind {
    /// STM32 USART Bootloader (0x7F → 0x79 ACK)
    Stm32,
    /// ESP32 ROM Bootloader (SLIP SYNC)
    Esp32,
    /// ESP8266 ROM Bootloader (SLIP SYNC)
    Esp8266,
    /// CH32 WCH ISP IDENTIFY (0x57 0xAB 0xA1)
    Ch32,
    /// STM8 UART Bootloader (0x7F → 0x79 ACK)
    Stm8,
    /// NXP LPC ISP (???????????? → Synchronized)
    Lpc,
    /// Arduino STK500 (0x30 0x20 → 0x14 0x10)
    Arduino,
    /// GD32 兼容 STM32 协议 (0x7F → 0x79 ACK)
    Gd32,
}

impl McuBootProbeKind {
    /// 探测序列（按鞋垫/床垫设备中常见度排序）
    pub const PROBE_SEQUENCE: &'static [McuBootProbeKind] = &[
        McuBootProbeKind::Stm32,
        McuBootProbeKind::Esp32,
        McuBootProbeKind::Ch32,
        McuBootProbeKind::Gd32,
        McuBootProbeKind::Esp8266,
        McuBootProbeKind::Stm8,
        McuBootProbeKind::Lpc,
        McuBootProbeKind::Arduino,
    ];

    /// 探测项显示名称
    pub fn name(&self) -> &'static str {
        match self {
            McuBootProbeKind::Stm32 => "STM32 USART Bootloader",
            McuBootProbeKind::Esp32 => "ESP32 ROM Bootloader",
            McuBootProbeKind::Esp8266 => "ESP8266 ROM Bootloader",
            McuBootProbeKind::Ch32 => "CH32 WCH ISP",
            McuBootProbeKind::Stm8 => "STM8 UART Bootloader",
            McuBootProbeKind::Lpc => "NXP LPC ISP",
            McuBootProbeKind::Arduino => "Arduino STK500",
            McuBootProbeKind::Gd32 => "GD32 USART Bootloader",
        }
    }

    /// 发送的同步字节序列
    pub fn command(&self) -> Vec<u8> {
        use crate::protocol::flasher::{
            Ch32Flasher, Esp32Flasher, Esp8266Flasher, LpcIspFlasher, Stk500Flasher,
            Stm32Flasher, Stm8Flasher,
        };
        match self {
            McuBootProbeKind::Stm32 | McuBootProbeKind::Gd32 => Stm32Flasher::new().get_command(),
            McuBootProbeKind::Esp32 => Esp32Flasher::new().sync(),
            McuBootProbeKind::Esp8266 => Esp8266Flasher::new().sync(),
            McuBootProbeKind::Ch32 => Ch32Flasher::new().identify(),
            McuBootProbeKind::Stm8 => Stm8Flasher::new().sync(),
            McuBootProbeKind::Lpc => LpcIspFlasher::new().sync(),
            McuBootProbeKind::Arduino => Stk500Flasher::new().get_sync(),
        }
    }

    /// 读取应答超时（毫秒）
    pub fn timeout_ms(&self) -> u64 {
        match self {
            McuBootProbeKind::Stm32
            | McuBootProbeKind::Gd32
            | McuBootProbeKind::Ch32
            | McuBootProbeKind::Stm8
            | McuBootProbeKind::Arduino => 500,
            McuBootProbeKind::Esp32 | McuBootProbeKind::Esp8266 => 200, // ESP 单次短超时
            McuBootProbeKind::Lpc => 1000, // LPC ISP 响应较慢
        }
    }

    /// 校验应答，返回识别结果
    ///
    /// 复用 flasher.rs 中各芯片的校验逻辑
    pub fn validate(&self, response: &[u8]) -> Option<McuBootProbeResult> {
        use crate::protocol::flasher::{Ch32Flasher, Stm32Flasher, Stm8Flasher};
        match self {
            McuBootProbeKind::Stm32 => {
                // STM32: 首字节 0x79 (ACK) 或 0x1F (NACK)
                if response.is_empty() {
                    return None;
                }
                let b = response[0];
                if Stm32Flasher::is_ack(b) {
                    Some(McuBootProbeResult {
                        vendor: "STMicroelectronics",
                        chip_family: "STM32",
                        description: "STM32 series (USART Bootloader active)".to_string(),
                        confidence: 0.95,
                        evidence: format!("Received ACK (0x79), STM32 ROM Bootloader is ready ({} bytes in response)", response.len()),
                    })
                } else if Stm32Flasher::is_nak(b) {
                    Some(McuBootProbeResult {
                        vendor: "STMicroelectronics",
                        chip_family: "STM32",
                        description: "STM32 series (Bootloader returned NACK, user firmware may be running)".to_string(),
                        confidence: 0.70,
                        evidence: "Received NACK (0x1F), STM32 ROM Bootloader detected but not ready".to_string(),
                    })
                } else {
                    None
                }
            }
            McuBootProbeKind::Gd32 => {
                // GD32 兼容 STM32 协议
                if response.is_empty() {
                    return None;
                }
                let b = response[0];
                if Stm32Flasher::is_ack(b) {
                    Some(McuBootProbeResult {
                        vendor: "GigaDevice",
                        chip_family: "GD32",
                        description: "GD32 series (STM32-compatible USART Bootloader protocol)".to_string(),
                        confidence: 0.85,
                        evidence: "Received ACK (0x79), GD32 reuses the STM32 protocol (cannot distinguish STM32/GD32 without reading the chip ID)".to_string(),
                    })
                } else {
                    None
                }
            }
            McuBootProbeKind::Esp32 => {
                // ESP32: 响应包含 0x55 0x00 0x07 0x07 0x12 0x20 或部分特征
                let sync_resp: [u8; 6] = [0x55, 0x00, 0x07, 0x07, 0x12, 0x20];
                if response.windows(6).any(|w| w == sync_resp) {
                    Some(McuBootProbeResult {
                        vendor: "Espressif",
                        chip_family: "ESP32",
                        description: "ESP32 series (ROM Bootloader SYNC succeeded)".to_string(),
                        confidence: 0.95,
                        evidence: "Received complete SYNC response 55 00 07 07 12 20".to_string(),
                    })
                } else if response.windows(4).any(|w| w == [0x07, 0x07, 0x12, 0x20]) {
                    Some(McuBootProbeResult {
                        vendor: "Espressif",
                        chip_family: "ESP32",
                        description: "ESP32 series (partial ROM Bootloader response)".to_string(),
                        confidence: 0.70,
                        evidence: "Received partial SYNC response pattern 07 07 12 20".to_string(),
                    })
                } else if !response.is_empty() {
                    // ESP32 SYNC 可能返回非空但非标准响应（设备在运行固件，未进 Boot）
                    None
                } else {
                    None
                }
            }
            McuBootProbeKind::Esp8266 => {
                // ESP8266: 响应特征与 ESP32 类似
                let sync_resp: [u8; 6] = [0x55, 0x00, 0x07, 0x07, 0x12, 0x20];
                if response.windows(6).any(|w| w == sync_resp) {
                    Some(McuBootProbeResult {
                        vendor: "Espressif",
                        chip_family: "ESP8266",
                        description: "ESP8266 series (ROM Bootloader SYNC succeeded)".to_string(),
                        confidence: 0.90,
                        evidence: "Received SYNC response (ESP8266 and ESP32 share the same response format; read the MAC to distinguish)".to_string(),
                    })
                } else {
                    None
                }
            }
            McuBootProbeKind::Ch32 => {
                // CH32: 响应以 0x57 0xAB 0xA1 开头且校验和通过
                if Ch32Flasher::validate_packet(response) {
                    Some(McuBootProbeResult {
                        vendor: "WCH (Nanjing Qinheng)",
                        chip_family: "CH32",
                        description: "CH32 series (WCH ISP IDENTIFY checksum passed)".to_string(),
                        confidence: 0.95,
                        evidence: format!(
                            "Received valid WCH ISP response packet ({} bytes, checksum passed)",
                            response.len()
                        ),
                    })
                } else if response.len() >= 3 && response[0] == 0x57 && response[1] == 0xAB {
                    // 帧头匹配但校验失败
                    Some(McuBootProbeResult {
                        vendor: "WCH (Nanjing Qinheng)",
                        chip_family: "CH32",
                        description: "CH32 series (WCH ISP header matched, but checksum failed)".to_string(),
                        confidence: 0.60,
                        evidence: "Response starts with 0x57 0xAB but the packet checksum failed".to_string(),
                    })
                } else {
                    None
                }
            }
            McuBootProbeKind::Stm8 => {
                // STM8: 同样使用 0x7F 唤醒，0x79 ACK
                if response.is_empty() {
                    return None;
                }
                let b = response[0];
                if Stm8Flasher::is_ack(b) {
                    Some(McuBootProbeResult {
                        vendor: "STMicroelectronics",
                        chip_family: "STM8",
                        description: "STM8 series (UART Bootloader active)".to_string(),
                        confidence: 0.85,
                        evidence: "Received ACK (0x79), STM8 Bootloader is ready (shares the 0x7F wake-up with STM32)".to_string(),
                    })
                } else {
                    None
                }
            }
            McuBootProbeKind::Lpc => {
                // LPC: 响应文本包含 "Synchronized"
                let text = String::from_utf8_lossy(response);
                if text.contains("Synchronized") {
                    Some(McuBootProbeResult {
                        vendor: "NXP",
                        chip_family: "LPC",
                        description: "LPC series (ISP synchronization succeeded)".to_string(),
                        confidence: 0.95,
                        evidence: "Received 'Synchronized' response, LPC ISP is ready".to_string(),
                    })
                } else {
                    None
                }
            }
            McuBootProbeKind::Arduino => {
                // Arduino STK500: 响应包含 0x14 (STK_INSYNC) 和 0x10 (STK_OK)
                if response.iter().any(|&b| b == 0x14) && response.iter().any(|&b| b == 0x10) {
                    Some(McuBootProbeResult {
                        vendor: "Microchip (Atmel)",
                        chip_family: "AVR (Arduino)",
                        description: "AVR series (STK500 protocol synchronized)".to_string(),
                        confidence: 0.90,
                        evidence: "Received STK_INSYNC (0x14) + STK_OK (0x10), Arduino Bootloader is ready".to_string(),
                    })
                } else {
                    None
                }
            }
        }
    }

    /// 探测前的注意事项提示
    pub fn note(&self) -> &'static str {
        match self {
            McuBootProbeKind::Stm32 | McuBootProbeKind::Gd32 => {
                "Requires BOOT0=1 + reset to enter Boot mode (DTR/RTS can trigger automatically)"
            }
            McuBootProbeKind::Esp32 | McuBootProbeKind::Esp8266 => {
                "Requires GPIO0=low + reset to enter Boot mode (hold the BOOT button while resetting)"
            }
            McuBootProbeKind::Ch32 => {
                "Requires BOOT0=1 + reset (CH32 uses the WCH-specific ISP protocol)"
            }
            McuBootProbeKind::Stm8 => "Requires the SWIM pin or UART Boot mode",
            McuBootProbeKind::Lpc => "Requires pulling the ISP pin low + reset",
            McuBootProbeKind::Arduino => "Requires DTR auto-reset (standard Arduino IDE method)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i2c_lookup() {
        let devices = lookup_i2c_device(0x68);
        assert!(!devices.is_empty());
        assert!(devices.iter().any(|d| d.chip_name.contains("MPU6050") || d.chip_name.contains("DS1307") || d.chip_name.contains("DS3231")));
    }

    #[test]
    fn test_spi_flash_lookup() {
        let info = lookup_spi_flash(0xEF, 0x4017).unwrap();
        assert_eq!(info.manufacturer, "Winbond");
        assert_eq!(info.part_number, "W25Q128FV");
        assert_eq!(info.capacity_mbit, 128);
    }

    #[test]
    fn test_spi_flash_vendor_name() {
        assert_eq!(spi_flash_vendor_name(0xEF), "Winbond");
        assert_eq!(spi_flash_vendor_name(0xC2), "Macronix");
        assert_eq!(spi_flash_vendor_name(0xFF), UNKNOWN_NAME);
    }

    #[test]
    fn test_jtag_lookup() {
        // Cortex-M3 IDCODE
        let info = lookup_jtag_chip(0x0BA9542F).unwrap();
        assert_eq!(info.manufacturer, "ARM");
        assert_eq!(info.part_number, "Cortex-M3");
    }

    #[test]
    fn test_onewire_lookup() {
        let info = lookup_onewire_device(0x28).unwrap();
        assert_eq!(info.chip_name, "DS18B20");
    }

    #[test]
    fn test_onewire_crc8() {
        // DS18B20 ROM: 28-FF-12-34-56-78-9A-XX
        let rom = [0x28, 0xFF, 0x12, 0x34, 0x56, 0x78, 0x9A, 0x00];
        let crc = onewire::crc8(&rom[..7]);
        let _ = crc; // 只验证不 panic
    }

    #[test]
    fn test_uart_probe_nmea() {
        let data = b"$GPGGA,092750.000,5321.6802,N,00630.3372,W,1,8,1.03,61.7,M,55.2,M,,*76\r\n";
        let results = analyze_uart_data(data);
        assert!(results.iter().any(|r| r.protocol_name == "NMEA 0183"));
    }

    #[test]
    fn test_uart_probe_at() {
        let data = b"AT\rOK\r\n";
        let results = analyze_uart_data(data);
        assert!(results.iter().any(|r| r.protocol_name == "AT commands"));
    }

    #[test]
    fn test_uart_probe_modbus() {
        // Modbus RTU: slave=1, FC=3, byte_count=4, data=[0x00,0x0A,0x00,0x14], CRC
        let mut frame = vec![0x01, 0x03, 0x04, 0x00, 0x0A, 0x00, 0x14];
        let crc = crate::protocol::modbus::ModbusFrame::calculate_crc(&frame);
        frame.extend_from_slice(&crc.to_le_bytes());
        let results = analyze_uart_data(&frame);
        assert!(results.iter().any(|r| r.protocol_name == "Modbus RTU"));
    }

    #[test]
    fn test_uart_probe_json() {
        let data = b"{\"temp\":25.5,\"humidity\":60}";
        let results = analyze_uart_data(data);
        assert!(results.iter().any(|r| r.protocol_name == "JSON"));
    }

    #[test]
    fn test_uart_probe_binary_header() {
        let data = [0xAA, 0x55, 0x01, 0x02, 0x03];
        let results = analyze_uart_data(&data);
        assert!(results.iter().any(|r| r.protocol_name == "Custom binary"));
    }

    #[test]
    fn test_ch341_i2c_scan_cmd() {
        let cmd = ch341::build_i2c_scan_cmd(0x68);
        assert_eq!(cmd[0], ch341::I2C_STM_START);
        assert_eq!(cmd[2], (0x68 << 1) & 0xFE);
        assert_eq!(cmd[3], ch341::I2C_STM_STOP);
    }

    #[test]
    fn test_ch341_spi_jedec_cmd() {
        let cmd = ch341::build_spi_jedec_cmd();
        assert!(!cmd.is_empty());
        assert_eq!(cmd[0], 0x80); // SPI 数据输出
    }

    #[test]
    fn test_mpsse_clock_divisor() {
        let divisor = mpsse::divisor_for_hz(100_000);
        assert!(divisor > 0);
        let hz = mpsse::clock_hz(divisor);
        assert!(hz <= 100_000);
    }

    #[test]
    fn test_swd_identify() {
        let result = swd::identify_swd_idcode(0x0BA9542F);
        assert!(result.is_some());
        let (mfg, _) = result.unwrap();
        assert_eq!(mfg, "ARM");
    }

    #[test]
    fn test_onewire_parse_rom() {
        let rom = [0x28, 0xFF, 0x12, 0x34, 0x56, 0x78, 0x9A, 0x42];
        let (family, serial, crc) = onewire::parse_rom(&rom);
        assert_eq!(family, 0x28);
        assert_eq!(crc, 0x42);
        assert!(serial > 0);
    }

        #[cfg(feature = "device-workspaces")]
    include!("bus_probe_device_tests.rs");

    // ===== MCU Boot ROM 探测测试 =====

    #[test]
    fn test_mcu_boot_stm32_ack() {
        let kind = McuBootProbeKind::Stm32;
        // STM32 ACK: 0x79
        let result = kind.validate(&[0x79]).expect("STM32 ACK 应识别成功");
        assert_eq!(result.chip_family, "STM32");
        assert_eq!(result.vendor, "STMicroelectronics");
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_mcu_boot_stm32_nak() {
        let kind = McuBootProbeKind::Stm32;
        // STM32 NACK: 0x1F
        let result = kind.validate(&[0x1F]).expect("STM32 NAK 应识别成功");
        assert_eq!(result.chip_family, "STM32");
        assert!(result.confidence < 0.9);
        assert!(result.confidence >= 0.6);
    }

    #[test]
    fn test_mcu_boot_stm32_no_response() {
        let kind = McuBootProbeKind::Stm32;
        // 空响应
        assert!(kind.validate(&[]).is_none());
        // 非匹配响应（鞋垫数据帧头）
        assert!(kind.validate(&[0xAA, 0x55, 0x03, 0x99]).is_none());
    }

    #[test]
    fn test_mcu_boot_esp32_sync() {
        let kind = McuBootProbeKind::Esp32;
        // ESP32 完整 SYNC 响应
        let resp = [0x55, 0x00, 0x07, 0x07, 0x12, 0x20];
        let result = kind.validate(&resp).expect("ESP32 SYNC 应识别成功");
        assert_eq!(result.chip_family, "ESP32");
        assert_eq!(result.vendor, "Espressif");
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_mcu_boot_esp32_partial() {
        let kind = McuBootProbeKind::Esp32;
        // 部分响应（只有 07 07 12 20）
        let resp = [0x07, 0x07, 0x12, 0x20];
        let result = kind.validate(&resp).expect("ESP32 部分响应应识别");
        assert!(result.confidence < 0.9);
        assert!(result.confidence >= 0.6);
    }

    #[test]
    fn test_mcu_boot_ch32_valid_packet() {
        let kind = McuBootProbeKind::Ch32;
        // 构造有效的 CH32 响应包: 帧头 57 AB + cmd A1 + len 00 00 + sum A1
        let resp = [0x57, 0xAB, 0xA1, 0x00, 0x00, 0xA1];
        let result = kind.validate(&resp).expect("CH32 有效包应识别成功");
        assert_eq!(result.chip_family, "CH32");
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_mcu_boot_ch32_header_only() {
        let kind = McuBootProbeKind::Ch32;
        // 帧头匹配但校验失败
        let resp = [0x57, 0xAB, 0xA1, 0x00, 0x00, 0x00]; // sum 错误
        let result = kind.validate(&resp).expect("CH32 帧头匹配应部分识别");
        assert!(result.confidence < 0.9);
        assert!(result.confidence >= 0.5);
    }

    #[test]
    fn test_mcu_boot_lpc_synchronized() {
        let kind = McuBootProbeKind::Lpc;
        let resp = b"Synchronized\r\n";
        let result = kind.validate(resp).expect("LPC Synchronized 应识别成功");
        assert_eq!(result.chip_family, "LPC");
        assert_eq!(result.vendor, "NXP");
        assert!(result.confidence >= 0.9);
    }

    #[test]
    fn test_mcu_boot_arduino_stk500() {
        let kind = McuBootProbeKind::Arduino;
        // STK500 响应: 0x14 (STK_INSYNC) + 0x10 (STK_OK)
        let resp = [0x14, 0x10];
        let result = kind.validate(&resp).expect("Arduino STK500 应识别成功");
        assert_eq!(result.chip_family, "AVR (Arduino)");
        assert!(result.confidence >= 0.85);
    }

    #[test]
    fn test_mcu_boot_probe_sequence_order() {
        // 验证探测序列按优先级排序
        let seq = McuBootProbeKind::PROBE_SEQUENCE;
        assert!(!seq.is_empty());
        assert_eq!(seq[0], McuBootProbeKind::Stm32); // STM32 最常见
        // 确保每种类型都出现
        assert!(seq.contains(&McuBootProbeKind::Esp32));
        assert!(seq.contains(&McuBootProbeKind::Ch32));
        assert!(seq.contains(&McuBootProbeKind::Gd32));
        assert!(seq.contains(&McuBootProbeKind::Lpc));
        assert!(seq.contains(&McuBootProbeKind::Arduino));
    }

    #[test]
    fn test_mcu_boot_command_not_empty() {
        // 验证每种探测项的命令不为空
        for kind in McuBootProbeKind::PROBE_SEQUENCE {
            let cmd = kind.command();
            assert!(!cmd.is_empty(), "{} 命令不应为空", kind.name());
        }
    }

    #[test]
    fn test_mcu_boot_timeout_reasonable() {
        // 验证超时在合理范围
        for kind in McuBootProbeKind::PROBE_SEQUENCE {
            let timeout = kind.timeout_ms();
            assert!(timeout >= 100 && timeout <= 2000, "{} 超时 {} 不合理", kind.name(), timeout);
        }
    }
}
