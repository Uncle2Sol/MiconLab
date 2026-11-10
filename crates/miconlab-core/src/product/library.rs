//! 产品协议库。
//!
//! 管理已知产品定义，支持增删查、JSON 持久化以及单产品导入导出。

use super::model::*;
use std::collections::HashMap;
use std::path::Path;

/// 产品库
#[derive(Clone)]
pub struct ProductLibrary {
    /// 产品集合，按产品 id 索引
    pub products: HashMap<String, Product>,
}

impl ProductLibrary {
    /// 创建空产品库。
    pub fn new() -> Self {
        Self {
            products: HashMap::new(),
        }
    }

    /// 内置常见柔性传感器产品。
    ///
    /// 预置 5 个典型产品：
    /// 1. ESP32-ADC-FlexSensor —— ESP32 + 电阻分压法柔性传感器
    /// 2. STM32-ADC-8CH —— STM32F103 8 通道 ADC 采集器
    /// 3. I2C-Pressure-0x28 —— I2C 数字压力传感器
    /// 4. Modbus-Pressure-XM —— Modbus RTU 压力变送器
    /// 5. Array-8x8-Pressure —— 8x8 阵列式压力传感器
    pub fn with_builtin() -> Self {
        let mut lib = Self::new();
        lib.add(builtin_esp32_adc_flex());
        lib.add(builtin_stm32_adc_8ch());
        lib.add(builtin_i2c_pressure_0x28());
        lib.add(builtin_modbus_pressure_xm());
        lib.add(builtin_array_8x8_pressure());
        lib
    }

    /// 添加产品。若 id 已存在则覆盖。
    pub fn add(&mut self, product: Product) {
        self.products.insert(product.id.clone(), product);
    }

    /// 移除指定 id 的产品。
    pub fn remove(&mut self, id: &str) -> Option<Product> {
        self.products.remove(id)
    }

    /// 获取指定 id 的产品引用。
    pub fn get(&self, id: &str) -> Option<&Product> {
        self.products.get(id)
    }

    /// 列出所有产品。
    pub fn list(&self) -> Vec<&Product> {
        let mut v: Vec<&Product> = self.products.values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// 保存产品库到 JSON 文件。
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let products: Vec<&Product> = self.list();
        let json = serde_json::to_string_pretty(&products)
            .map_err(|e| format!("Serialization failed: {}", e))?;
        std::fs::write(path, json).map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(())
    }

    /// 从 JSON 文件加载产品库。
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;
        let products: Vec<Product> =
            serde_json::from_str(&content).map_err(|e| format!("Deserialization failed: {}", e))?;
        let mut lib = Self::new();
        for p in products {
            lib.add(p);
        }
        Ok(lib)
    }

    /// 导出单个产品到 JSON 文件。
    pub fn export_product(&self, id: &str, path: &Path) -> Result<(), String> {
        let product = self
            .products
            .get(id)
            .ok_or_else(|| format!("Product not found: {}", id))?;
        let json = serde_json::to_string_pretty(product)
            .map_err(|e| format!("Serialization failed: {}", e))?;
        std::fs::write(path, json).map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(())
    }

    /// 从 JSON 文件导入单个产品。
    pub fn import_product(&self, path: &Path) -> Result<Product, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;
        let product: Product =
            serde_json::from_str(&content).map_err(|e| format!("Deserialization failed: {}", e))?;
        Ok(product)
    }
}

impl Default for ProductLibrary {
    fn default() -> Self {
        Self::new()
    }
}

// ========== 内置产品构造 ==========

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 1. ESP32-ADC-FlexSensor —— ESP32 + 电阻分压法柔性传感器
fn builtin_esp32_adc_flex() -> Product {
    Product {
        id: "ESP32-ADC-FlexSensor".to_string(),
        name: "ESP32 ADC flex sensor".to_string(),
        manufacturer: "Espressif".to_string(),
        model: "ESP32-Flex01".to_string(),
        category: ProductCategory::FlexibleSensor,
        sensor_type: SensorType::Resistive,
        range_min: 0.0,
        range_max: 100.0,
        sensitivity: 1.0,
        unit: "N".to_string(),
        description: "ESP32 + resistive divider flex sensor, 12-bit ADC, 3.3V reference, single-channel force measurement on GPIO34".to_string(),
        protocol: ProtocolConfig {
            protocol_type: ProtocolType::AdcDirect,
            baud_rate: 0,
            slave_id: None,
            frame_format: None,
            commands: Vec::new(),
            register_map: Vec::new(),
            i2c_address: None,
            adc_resolution: Some(12),
            adc_ref_voltage: Some(3.3),
        },
        channels: vec![ChannelDef {
            name: "CH0".to_string(),
            channel_type: ChannelType::Adc,
            pin: "GPIO34".to_string(),
            channel_index: 0,
            gain: 30.5,
            offset: 0.0,
            unit: "N".to_string(),
            enabled: true,
        }],
        calibration: Some(CalibrationData {
            calibration_type: CalibrationType::Linear,
            coefficients: vec![30.5, 0.0],
            points: vec![
                CalibrationPoint {
                    raw_value: 0.0,
                    standard_value: 0.0,
                    description: Some("Zero".to_string()),
                },
                CalibrationPoint {
                    raw_value: 3.3,
                    standard_value: 100.0,
                    description: Some("Full scale".to_string()),
                },
            ],
            calibrated_at: Some(now_iso()),
        }),
        created_at: now_iso(),
        notes: "Resistive divider: Vout = Vref * Rflex / (Rflex + Rfixed)".to_string(),
    }
}

/// 2. STM32-ADC-8CH —— STM32F103 8 通道 ADC 采集器
fn builtin_stm32_adc_8ch() -> Product {
    let pins = ["PA0", "PA1", "PA2", "PA3", "PA4", "PA5", "PA6", "PA7"];
    let channels: Vec<ChannelDef> = pins
        .iter()
        .enumerate()
        .map(|(i, pin)| ChannelDef {
            name: format!("CH{}", i),
            channel_type: ChannelType::Adc,
            pin: pin.to_string(),
            channel_index: i as u8,
            gain: 1.0,
            offset: 0.0,
            unit: "V".to_string(),
            enabled: true,
        })
        .collect();

    Product {
        id: "STM32-ADC-8CH".to_string(),
        name: "STM32 8-channel ADC collector".to_string(),
        manufacturer: "STMicroelectronics".to_string(),
        model: "STM32F103C8T6".to_string(),
        category: ProductCategory::AcquisitionModule,
        sensor_type: SensorType::Resistive,
        range_min: 0.0,
        range_max: 3.3,
        sensitivity: 1.0,
        unit: "V".to_string(),
        description: "STM32F103 + 8-channel ADC collector, 12-bit, 3.3V reference, PA0-PA7".to_string(),
        protocol: ProtocolConfig {
            protocol_type: ProtocolType::AdcDirect,
            baud_rate: 0,
            slave_id: None,
            frame_format: None,
            commands: Vec::new(),
            register_map: Vec::new(),
            i2c_address: None,
            adc_resolution: Some(12),
            adc_ref_voltage: Some(3.3),
        },
        channels,
        calibration: Some(CalibrationData {
            calibration_type: CalibrationType::Zero,
            coefficients: vec![1.0, 0.0],
            points: Vec::new(),
            calibrated_at: Some(now_iso()),
        }),
        created_at: now_iso(),
        notes: "ADC1 8-channel scan mode, DMA transfer".to_string(),
    }
}

/// 3. I2C-Pressure-0x28 —— I2C 数字压力传感器
fn builtin_i2c_pressure_0x28() -> Product {
    Product {
        id: "I2C-Pressure-0x28".to_string(),
        name: "I2C digital pressure sensor".to_string(),
        manufacturer: "Generic".to_string(),
        model: "I2C-PS-0x28".to_string(),
        category: ProductCategory::Custom("PressureSensor".to_string()),
        sensor_type: SensorType::Piezoelectric,
        range_min: 0.0,
        range_max: 500.0,
        sensitivity: 1.0,
        unit: "kPa".to_string(),
        description: "I2C digital pressure sensor, address 0x28, 16-bit data, 0-500kPa range".to_string(),
        protocol: ProtocolConfig {
            protocol_type: ProtocolType::I2c,
            baud_rate: 0,
            slave_id: None,
            frame_format: None,
            commands: vec![ProtocolCommand {
                name: "READ_PRESSURE".to_string(),
                command_bytes: vec![0x00],
                expected_response_length: Some(2),
                description: "Read 16-bit raw pressure value (big-endian)".to_string(),
                response_parser: Some(ResponseType::ChannelData),
            }],
            register_map: Vec::new(),
            i2c_address: Some(0x28),
            adc_resolution: None,
            adc_ref_voltage: None,
        },
        channels: vec![ChannelDef {
            name: "Pressure".to_string(),
            channel_type: ChannelType::I2c,
            pin: "0x28".to_string(),
            channel_index: 0,
            gain: 500.0 / 65535.0,
            offset: 0.0,
            unit: "kPa".to_string(),
            enabled: true,
        }],
        calibration: Some(CalibrationData {
            calibration_type: CalibrationType::Linear,
            coefficients: vec![500.0 / 65535.0, 0.0],
            points: vec![
                CalibrationPoint {
                    raw_value: 0.0,
                    standard_value: 0.0,
                    description: Some("Zero pressure".to_string()),
                },
                CalibrationPoint {
                    raw_value: 65535.0,
                    standard_value: 500.0,
                    description: Some("Full scale".to_string()),
                },
            ],
            calibrated_at: Some(now_iso()),
        }),
        created_at: now_iso(),
        notes: "Raw value 0-65535 maps to 0-500kPa".to_string(),
    }
}

/// 4. Modbus-Pressure-XM —— Modbus RTU 压力变送器
fn builtin_modbus_pressure_xm() -> Product {
    Product {
        id: "Modbus-Pressure-XM".to_string(),
        name: "Modbus RTU pressure transmitter".to_string(),
        manufacturer: "XM".to_string(),
        model: "XM-PT-100".to_string(),
        category: ProductCategory::Custom("PressureTransmitter".to_string()),
        sensor_type: SensorType::StrainGauge,
        range_min: 0.0,
        range_max: 1.6,
        sensitivity: 1.0,
        unit: "MPa".to_string(),
        description: "Modbus RTU pressure transmitter, slave ID=1, register 0x0000, function code 4, 0-1.6MPa range".to_string(),
        protocol: ProtocolConfig {
            protocol_type: ProtocolType::ModbusRtu,
            baud_rate: 9600,
            slave_id: Some(1),
            frame_format: None,
            commands: Vec::new(),
            register_map: vec![RegisterMapping {
                register_addr: 0x0000,
                channel_name: "Pressure".to_string(),
                quantity: 1,
                scale: 0.0001,
                offset: 0.0,
                unit: "MPa".to_string(),
                function_code: 4,
            }],
            i2c_address: None,
            adc_resolution: None,
            adc_ref_voltage: None,
        },
        channels: vec![ChannelDef {
            name: "Pressure".to_string(),
            channel_type: ChannelType::Modbus,
            pin: "0x0000".to_string(),
            channel_index: 0,
            gain: 0.0001,
            offset: 0.0,
            unit: "MPa".to_string(),
            enabled: true,
        }],
        calibration: Some(CalibrationData {
            calibration_type: CalibrationType::Linear,
            coefficients: vec![0.0001, 0.0],
            points: vec![
                CalibrationPoint {
                    raw_value: 0.0,
                    standard_value: 0.0,
                    description: Some("Zero".to_string()),
                },
                CalibrationPoint {
                    raw_value: 16000.0,
                    standard_value: 1.6,
                    description: Some("Full scale 1.6MPa".to_string()),
                },
            ],
            calibrated_at: Some(now_iso()),
        }),
        created_at: now_iso(),
        notes: "Register value 0-16000 maps to 0-1.6MPa, scale factor 0.0001".to_string(),
    }
}

/// 5. Array-8x8-Pressure —— 8x8 阵列式压力传感器
fn builtin_array_8x8_pressure() -> Product {
    let channels: Vec<ChannelDef> = (0..64)
        .map(|i| {
            let row = i / 8;
            let col = i % 8;
            ChannelDef {
                name: format!("R{}C{}", row, col),
                channel_type: ChannelType::Serial,
                pin: format!("cell[{}][{}]", row, col),
                channel_index: i as u8,
                gain: 500.0 / 65535.0,
                offset: 0.0,
                unit: "kPa".to_string(),
                enabled: true,
            }
        })
        .collect();

    Product {
        id: "Array-8x8-Pressure".to_string(),
        name: "8x8 array pressure sensor".to_string(),
        manufacturer: "Generic".to_string(),
        model: "Array-8x8".to_string(),
        category: ProductCategory::PressureArray,
        sensor_type: SensorType::Resistive,
        range_min: 0.0,
        range_max: 500.0,
        sensitivity: 1.0,
        unit: "kPa".to_string(),
        description: "8x8 array pressure sensor, serial protocol frame (header 0xAA55, 64 channels, u16LE)".to_string(),
        protocol: ProtocolConfig {
            protocol_type: ProtocolType::SerialFrame,
            baud_rate: 115200,
            slave_id: None,
            frame_format: Some(FrameFormat {
                header: vec![0xAA, 0x55],
                footer: vec![0x55, 0xAA],
                length_offset: None,
                length_bytes: 0,
                data_offset: 0,
                checksum_type: ChecksumType::Sum8,
                checksum_offset: None,
                channel_count: 64,
                data_format: DataFormat::U16LE,
            }),
            commands: vec![ProtocolCommand {
                name: "START_STREAM".to_string(),
                command_bytes: vec![0xAA, 0x01, 0x55],
                expected_response_length: None,
                description: "Start continuous data stream".to_string(),
                response_parser: Some(ResponseType::ChannelData),
            }],
            register_map: Vec::new(),
            i2c_address: None,
            adc_resolution: None,
            adc_ref_voltage: None,
        },
        channels,
        calibration: Some(CalibrationData {
            calibration_type: CalibrationType::Linear,
            coefficients: vec![500.0 / 65535.0, 0.0],
            points: vec![
                CalibrationPoint {
                    raw_value: 0.0,
                    standard_value: 0.0,
                    description: Some("Zero".to_string()),
                },
                CalibrationPoint {
                    raw_value: 65535.0,
                    standard_value: 500.0,
                    description: Some("Full scale".to_string()),
                },
            ],
            calibrated_at: Some(now_iso()),
        }),
        created_at: now_iso(),
        notes: "Frame structure: AA 55 + [64×u16LE] + Sum8 + 55 AA".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_count() {
        let lib = ProductLibrary::with_builtin();
        assert_eq!(lib.products.len(), 5);
        assert!(lib.get("ESP32-ADC-FlexSensor").is_some());
        assert!(lib.get("STM32-ADC-8CH").is_some());
        assert!(lib.get("I2C-Pressure-0x28").is_some());
        assert!(lib.get("Modbus-Pressure-XM").is_some());
        assert!(lib.get("Array-8x8-Pressure").is_some());
    }

    #[test]
    fn test_add_remove_get() {
        let mut lib = ProductLibrary::new();
        let p = builtin_esp32_adc_flex();
        lib.add(p.clone());
        assert!(lib.get(&p.id).is_some());
        assert!(lib.remove(&p.id).is_some());
        assert!(lib.get(&p.id).is_none());
    }

    #[test]
    fn test_array_8x8_channels() {
        let lib = ProductLibrary::with_builtin();
        let p = lib.get("Array-8x8-Pressure").unwrap();
        assert_eq!(p.channels.len(), 64);
        assert_eq!(p.protocol.frame_format.as_ref().unwrap().channel_count, 64);
    }

    #[test]
    fn test_modbus_register_map() {
        let lib = ProductLibrary::with_builtin();
        let p = lib.get("Modbus-Pressure-XM").unwrap();
        assert_eq!(p.protocol.register_map.len(), 1);
        assert_eq!(p.protocol.register_map[0].function_code, 4);
        assert_eq!(p.protocol.slave_id, Some(1));
    }

    #[test]
    fn test_list_sorted() {
        let lib = ProductLibrary::with_builtin();
        let list = lib.list();
        assert_eq!(list.len(), 5);
        // 列表按 id 排序
        for w in list.windows(2) {
            assert!(w[0].id <= w[1].id);
        }
    }

    #[test]
    fn test_json_roundtrip_single() {
        let lib = ProductLibrary::with_builtin();
        let dir = std::env::temp_dir();
        let path = dir.join("miconlab_product_export_test.json");
        let id = "ESP32-ADC-FlexSensor";
        lib.export_product(id, &path).unwrap();
        let imported = lib.import_product(&path).unwrap();
        assert_eq!(imported.id, id);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_json_roundtrip_library() {
        let lib = ProductLibrary::with_builtin();
        let dir = std::env::temp_dir();
        let path = dir.join("miconlab_library_test.json");
        lib.save_to_file(&path).unwrap();
        let loaded = ProductLibrary::load_from_file(&path).unwrap();
        assert_eq!(loaded.products.len(), 5);
        let _ = std::fs::remove_file(&path);
    }
}
