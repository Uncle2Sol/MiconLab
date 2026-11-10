//! 协议解析器。
//!
//! 用于解析产品通讯数据，支持 ADC 直读、I2C、SPI、串口自定义帧以及 Modbus RTU 协议。

use super::model::*;

/// 协议解析结果
#[derive(Clone, Debug)]
pub struct ParsedData {
    /// 各通道值
    pub channel_values: Vec<f64>,
    /// 原始字节
    pub raw_bytes: Vec<u8>,
    /// 时间戳（秒）
    pub timestamp: f64,
    /// 是否有效
    pub valid: bool,
    /// 错误信息
    pub error: Option<String>,
}

/// 协议解析器
pub struct ProtocolParser {
    /// 协议配置
    pub config: ProtocolConfig,
    /// 接收缓冲区
    pub buffer: Vec<u8>,
}

impl ProtocolParser {
    /// 创建解析器。
    pub fn new(config: ProtocolConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
        }
    }

    /// 喂入接收到的字节，尝试解析出所有完整帧。
    ///
    /// 仅 `SerialFrame` 协议支持流式帧解析；其他协议类型直接返回空。
    pub fn feed(&mut self, data: &[u8]) -> Vec<ParsedData> {
        self.buffer.extend_from_slice(data);

        // 仅串口自定义帧协议支持流式帧解析
        if self.config.protocol_type != ProtocolType::SerialFrame {
            return Vec::new();
        }

        let frame_format = match &self.config.frame_format {
            Some(ff) => ff.clone(),
            None => return Vec::new(),
        };

        let mut results = Vec::new();
        loop {
            // 1. 查找帧头
            let header_pos = if frame_format.header.is_empty() {
                0
            } else {
                match self
                    .buffer
                    .windows(frame_format.header.len())
                    .position(|w| w == frame_format.header.as_slice())
                {
                    Some(p) => p,
                    None => {
                        // 未找到帧头，保留尾部可能的不完整帧头
                        let keep = frame_format.header.len().saturating_sub(1);
                        if self.buffer.len() > keep {
                            self.buffer.drain(..self.buffer.len() - keep);
                        }
                        break;
                    }
                }
            };
            // 丢弃帧头之前的字节
            if header_pos > 0 {
                self.buffer.drain(..header_pos);
            }

            // 2. 查找帧尾或按长度确定完整帧
            let frame_end = match self.find_frame_end(&frame_format) {
                Some(end) => end,
                None => break, // 不完整，等待更多数据
            };

            // 3. 提取并解析帧
            let frame: Vec<u8> = self.buffer.drain(..frame_end).collect();
            let parsed = match self.parse_frame(&frame) {
                Ok(p) => p,
                Err(e) => ParsedData {
                    channel_values: Vec::new(),
                    raw_bytes: frame,
                    timestamp: current_timestamp(),
                    valid: false,
                    error: Some(e),
                },
            };
            results.push(parsed);
        }

        results
    }

    /// 在缓冲区（以帧头起始）中查找完整帧的结束位置（不含）。
    fn find_frame_end(&self, ff: &FrameFormat) -> Option<usize> {
        let buf = &self.buffer;
        let after_header = if ff.header.is_empty() { 0 } else { ff.header.len() };

        if !ff.footer.is_empty() {
            // 依据帧尾定位
            if buf.len() < after_header + ff.footer.len() {
                return None;
            }
            let footer_pos = buf[after_header..]
                .windows(ff.footer.len())
                .position(|w| w == ff.footer.as_slice())?;
            Some(after_header + footer_pos + ff.footer.len())
        } else if let Some(len_off) = ff.length_offset {
            // 依据长度字段定位
            let len_start = after_header + len_off;
            if buf.len() < len_start + ff.length_bytes {
                return None;
            }
            let total_len = read_length(&buf[len_start..len_start + ff.length_bytes])? as usize;
            if buf.len() < total_len {
                return None;
            }
            Some(total_len)
        } else {
            // 固定长度：帧头 + 通道数据 + 校验
            let data_size = data_format_size(&ff.data_format);
            let checksum_size = checksum_size(&ff.checksum_type);
            let total = after_header + ff.channel_count * data_size + checksum_size;
            if buf.len() < total {
                None
            } else {
                Some(total)
            }
        }
    }

    /// 解析单个完整帧。
    pub fn parse_frame(&self, frame: &[u8]) -> Result<ParsedData, String> {
        if frame.is_empty() {
            return Err("Empty frame".to_string());
        }

        // 校验完整性
        if !self.validate_checksum(frame) {
            return Err(format!("Checksum failed: {}", frame_to_hex(frame)));
        }

        let channel_values = self.extract_channels(frame);
        Ok(ParsedData {
            channel_values,
            raw_bytes: frame.to_vec(),
            timestamp: current_timestamp(),
            valid: true,
            error: None,
        })
    }

    /// 构建读取请求。
    ///
    /// - Modbus RTU：根据寄存器映射构建读寄存器请求帧。
    /// - SerialFrame：返回第一条命令的字节。
    /// - I2C：构造 I2C 读请求（设备地址 + 第一条命令字节）。
    /// - SPI：返回第一条命令字节（CS 由底层驱动管理）。
    /// - AdcDirect：返回空（ADC 直读无需发送命令）。
    pub fn build_read_request(&self) -> Vec<u8> {
        match self.config.protocol_type {
            ProtocolType::ModbusRtu => {
                if let Some(rm) = self.config.register_map.first() {
                    let slave_id = self.config.slave_id.unwrap_or(1);
                    let mut buf = Vec::with_capacity(8);
                    buf.push(slave_id);
                    buf.push(rm.function_code);
                    buf.extend_from_slice(&rm.register_addr.to_be_bytes());
                    buf.extend_from_slice(&rm.quantity.to_be_bytes());
                    let crc = crate::checksum::crc16_modbus(&buf);
                    buf.extend_from_slice(&crc.to_le_bytes());
                    buf
                } else {
                    Vec::new()
                }
            }
            ProtocolType::SerialFrame => self
                .config
                .commands
                .first()
                .map(|c| c.command_bytes.clone())
                .unwrap_or_default(),
            ProtocolType::I2c => {
                // I2C 读请求：设备地址 + 命令字节（如有）
                let mut buf = Vec::new();
                if let Some(addr) = self.config.i2c_address {
                    buf.push(addr << 1); // 7 位地址左移 1 位，R/W=0（写）
                }
                if let Some(cmd) = self.config.commands.first() {
                    buf.extend_from_slice(&cmd.command_bytes);
                }
                buf
            }
            ProtocolType::Spi => {
                // SPI 读请求：返回第一条命令字节（CS 由底层驱动管理）
                self.config
                    .commands
                    .first()
                    .map(|c| c.command_bytes.clone())
                    .unwrap_or_default()
            }
            ProtocolType::AdcDirect => {
                // ADC 直读无需发送命令
                Vec::new()
            }
        }
    }

    /// 验证帧完整性（校验和）。
    pub fn validate_checksum(&self, frame: &[u8]) -> bool {
        let ff = match &self.config.frame_format {
            Some(ff) => ff,
            None => return true, // 无帧格式定义，跳过校验
        };

        match ff.checksum_type {
            ChecksumType::None => true,
            ChecksumType::Sum8 | ChecksumType::Xor => {
                let (data, expected) = match split_frame(frame, ff, 1) {
                    Some(v) => v,
                    None => return false,
                };
                let computed = match ff.checksum_type {
                    ChecksumType::Sum8 => data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)),
                    ChecksumType::Xor => data.iter().fold(0u8, |acc, &b| acc ^ b),
                    // None 和 Crc16 已由外层 match 分支处理，此处不可达
                    _ => unreachable!(),
                };
                expected == vec![computed]
            }
            ChecksumType::Crc16 => {
                let (data, expected) = match split_frame(frame, ff, 2) {
                    Some(v) => v,
                    None => return false,
                };
                let computed = crate::checksum::crc16_modbus(&data);
                expected == computed.to_le_bytes().to_vec()
            }
        }
    }

    /// 计算校验和，返回校验字段字节序列。
    ///
    /// - `None`：返回空 Vec
    /// - `Sum8`/`Xor`：返回单字节
    /// - `Crc16`：返回 2 字节（小端，与 Modbus CRC16 一致）
    pub fn calculate_checksum(&self, data: &[u8]) -> Vec<u8> {
        let ff = match &self.config.frame_format {
            Some(ff) => ff,
            None => return Vec::new(),
        };
        match ff.checksum_type {
            ChecksumType::None => Vec::new(),
            ChecksumType::Sum8 => vec![data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))],
            ChecksumType::Xor => vec![data.iter().fold(0u8, |acc, &b| acc ^ b)],
            ChecksumType::Crc16 => {
                let crc = crate::checksum::crc16_modbus(data);
                crc.to_le_bytes().to_vec()
            }
        }
    }

    /// 从帧中提取通道数据。
    pub fn extract_channels(&self, frame: &[u8]) -> Vec<f64> {
        let ff = match &self.config.frame_format {
            Some(ff) => ff,
            None => return Vec::new(),
        };

        let after_header = if ff.header.is_empty() { 0 } else { ff.header.len() };
        let data_size = data_format_size(&ff.data_format);
        // 数据起始偏移（相对于帧头之后）
        let start = after_header + ff.data_offset;
        let checksum_size = checksum_size(&ff.checksum_type);
        // 数据区末尾：去掉帧尾与校验
        let footer_len = ff.footer.len();
        let data_end = frame.len().saturating_sub(footer_len + checksum_size);

        let mut values = Vec::with_capacity(ff.channel_count);
        for i in 0..ff.channel_count {
            let off = start + i * data_size;
            if off + data_size > data_end {
                break;
            }
            if let Ok(v) = parse_data_bytes(&frame[off..off + data_size], &ff.data_format) {
                values.push(v);
            } else {
                break;
            }
        }
        values
    }

    /// 应用缩放和偏移：`value * scale + offset`。
    pub fn apply_scaling(value: f64, scale: f64, offset: f64) -> f64 {
        value * scale + offset
    }
}

/// 将帧拆分为「校验数据区」与「校验字段字节」。
///
/// `checksum_size` 为校验字段字节数。校验字段位于帧尾之前（帧尾之后无校验）。
/// 返回 `(参与校验的数据, 校验字段字节)`。
fn split_frame<'a>(frame: &'a [u8], ff: &'a FrameFormat, checksum_size: usize) -> Option<(&'a [u8], Vec<u8>)> {
    if frame.len() < checksum_size {
        return None;
    }

    // 校验字段位置：若提供 checksum_offset（从帧尾算），则校验字段起始为
    // frame.len() - offset；否则默认位于帧尾与帧尾标记之间。
    let (cs_start, cs_end) = if let Some(off) = ff.checksum_offset {
        let end = frame.len().checked_sub(off)?;
        let start = end.checked_sub(checksum_size)?;
        (start, end)
    } else {
        let footer_len = ff.footer.len();
        let end = frame.len().checked_sub(footer_len)?;
        let start = end.checked_sub(checksum_size)?;
        (start, end)
    };

    if cs_start >= cs_end || cs_end > frame.len() {
        return None;
    }

    // 参与校验的数据为帧头之后到校验字段之前的所有字节
    let header_len = if ff.header.is_empty() { 0 } else { ff.header.len() };
    if cs_start < header_len {
        return None;
    }
    let data = &frame[header_len..cs_start];
    let checksum = frame[cs_start..cs_end].to_vec();
    Some((data, checksum))
}

/// 读取长度字段（按大端解析）。
fn read_length(bytes: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    for &b in bytes {
        v = (v << 8) | (b as u64);
    }
    Some(v)
}

/// 数据格式对应的字节数。
fn data_format_size(format: &DataFormat) -> usize {
    match format {
        DataFormat::U8 => 1,
        DataFormat::U16BE | DataFormat::U16LE => 2,
        DataFormat::U32BE | DataFormat::U32LE => 4,
        DataFormat::F32BE | DataFormat::F32LE => 4,
    }
}

/// 校验类型对应的字节数。
fn checksum_size(t: &ChecksumType) -> usize {
    match t {
        ChecksumType::None => 0,
        ChecksumType::Sum8 | ChecksumType::Xor => 1,
        ChecksumType::Crc16 => 2,
    }
}

/// 当前时间戳（秒）。
fn current_timestamp() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64 / 1000.0
}

/// 将字节序列格式化为十六进制字符串。
fn frame_to_hex(frame: &[u8]) -> String {
    let mut s = String::with_capacity(frame.len() * 3);
    for (i, &b) in frame.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{:02X}", b));
    }
    s
}

/// 将原始 ADC 值转换为电压。
pub fn adc_to_voltage(raw: u16, resolution: u16, ref_voltage: f64) -> f64 {
    let max = (1u32 << resolution) - 1;
    (raw as f64 / max as f64) * ref_voltage
}

/// 将电压转换为物理量（通过校准系数）。
pub fn voltage_to_physical(voltage: f64, slope: f64, intercept: f64) -> f64 {
    slope * voltage + intercept
}

/// 从字节解析数据。
pub fn parse_data_bytes(data: &[u8], format: &DataFormat) -> Result<f64, String> {
    match format {
        DataFormat::U8 => Ok(data.first().map(|b| *b as f64).unwrap_or(0.0)),
        DataFormat::U16BE => data
            .get(..2)
            .map(|d| u16::from_be_bytes([d[0], d[1]]) as f64)
            .ok_or_else(|| "Insufficient bytes".to_string()),
        DataFormat::U16LE => data
            .get(..2)
            .map(|d| u16::from_le_bytes([d[0], d[1]]) as f64)
            .ok_or_else(|| "Insufficient bytes".to_string()),
        DataFormat::U32BE => data
            .get(..4)
            .map(|d| u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as f64)
            .ok_or_else(|| "Insufficient bytes".to_string()),
        DataFormat::U32LE => data
            .get(..4)
            .map(|d| u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64)
            .ok_or_else(|| "Insufficient bytes".to_string()),
        DataFormat::F32BE => data
            .get(..4)
            .map(|d| f32::from_be_bytes([d[0], d[1], d[2], d[3]]) as f64)
            .ok_or_else(|| "Insufficient bytes".to_string()),
        DataFormat::F32LE => data
            .get(..4)
            .map(|d| f32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64)
            .ok_or_else(|| "Insufficient bytes".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn array_parser() -> ProtocolParser {
        let config = ProtocolConfig {
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
                channel_count: 2,
                data_format: DataFormat::U16LE,
            }),
            commands: Vec::new(),
            register_map: Vec::new(),
            i2c_address: None,
            adc_resolution: None,
            adc_ref_voltage: None,
        };
        ProtocolParser::new(config)
    }

    #[test]
    fn test_parse_data_bytes_u16le() {
        let v = parse_data_bytes(&[0x10, 0x20], &DataFormat::U16LE).unwrap();
        assert_eq!(v, 0x2010 as f64);
    }

    #[test]
    fn test_parse_data_bytes_insufficient() {
        assert!(parse_data_bytes(&[0x10], &DataFormat::U16BE).is_err());
    }

    #[test]
    fn test_adc_to_voltage() {
        let v = adc_to_voltage(2048, 12, 3.3);
        assert!((v - 1.65).abs() < 0.01);
    }

    #[test]
    fn test_voltage_to_physical() {
        assert_eq!(voltage_to_physical(2.0, 10.0, 1.0), 21.0);
    }

    #[test]
    fn test_apply_scaling() {
        assert_eq!(ProtocolParser::apply_scaling(2.0, 3.0, 1.0), 7.0);
    }

    #[test]
    fn test_feed_extracts_frame() {
        let mut parser = array_parser();
        // 帧头(2) + 数据(4: 0x0100,0x0200) + 校验(1) + 帧尾(2)
        let mut frame = vec![0xAA, 0x55, 0x00, 0x01, 0x00, 0x02];
        let cs: u8 = frame[2..].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        frame.push(cs);
        frame.push(0x55);
        frame.push(0xAA);

        let results = parser.feed(&frame);
        assert_eq!(results.len(), 1);
        assert!(results[0].valid);
        assert_eq!(results[0].channel_values, vec![256.0, 512.0]);
    }

    #[test]
    fn test_build_read_request_modbus() {
        let config = ProtocolConfig {
            protocol_type: ProtocolType::ModbusRtu,
            baud_rate: 9600,
            slave_id: Some(1),
            frame_format: None,
            commands: Vec::new(),
            register_map: vec![RegisterMapping {
                register_addr: 0x0000,
                channel_name: "Pressure".to_string(),
                quantity: 1,
                scale: 1.0,
                offset: 0.0,
                unit: "MPa".to_string(),
                function_code: 4,
            }],
            i2c_address: None,
            adc_resolution: None,
            adc_ref_voltage: None,
        };
        let parser = ProtocolParser::new(config);
        let req = parser.build_read_request();
        // 01 04 0000 0001 + CRC(LE)
        assert_eq!(req.len(), 8);
        assert_eq!(&req[0..6], &[0x01, 0x04, 0x00, 0x00, 0x00, 0x01]);
    }
}
