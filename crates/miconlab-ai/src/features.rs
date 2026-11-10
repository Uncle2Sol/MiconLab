//! 数据流特征提取
//!
//! 从一段串口字节流中提取多维统计特征，作为协议指纹识别和异常检测的基础。
//!
//! 特征维度设计原则：
//! - 区分性强：不同协议的特征向量应有明显差异
//! - 鲁棒性高：对数据长度和内容变化不敏感
//! - 计算低开销：可在每帧 GUI 刷新中实时计算

use serde::{Deserialize, Serialize};

/// 已知的常见帧头模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KnownHeader {
    /// Modbus RTU / 通用 0xAA 0x55 帧头
    Aa55,
    /// 鞋垫压力传感器专用帧头 AA 55 03 99
    InsoleAa550399,
    /// HDLC / PPP 帧标志 0x7E
    Hdlc7e,
    /// Modbus TCP MBAP 事务标识（高字节变化）
    ModbusMbap,
    /// SLCCAN 命令起始（t/T/r/R/v/V 等字母开头）
    SlcanAscii,
    /// ESP image 魔数 0xE9
    EspImageE9,
    /// STM32 Bootloader 同步字节 0x7F
    Stm32Bootloader7f,
    /// NMEA 语句起始 $GP/$GN/$GL
    NmeaGp,
}

impl KnownHeader {
    /// 帧头的中文名称
    pub fn label_zh(self) -> &'static str {
        match self {
            KnownHeader::Aa55 => "AA 55 generic header",
            KnownHeader::InsoleAa550399 => "Insole sensor header (AA 55 03 99)",
            KnownHeader::Hdlc7e => "HDLC/PPP flag (7E)",
            KnownHeader::ModbusMbap => "Modbus TCP MBAP",
            KnownHeader::SlcanAscii => "SLCAN ASCII command",
            KnownHeader::EspImageE9 => "ESP image magic (E9)",
            KnownHeader::Stm32Bootloader7f => "STM32 Bootloader sync (7F)",
            KnownHeader::NmeaGp => "NMEA sentence ($GP/$GN)",
        }
    }

    /// 帧头的英文名称
    pub fn label_en(self) -> &'static str {
        match self {
            KnownHeader::Aa55 => "AA 55 generic header",
            KnownHeader::InsoleAa550399 => "Insole sensor header (AA 55 03 99)",
            KnownHeader::Hdlc7e => "HDLC/PPP flag (7E)",
            KnownHeader::ModbusMbap => "Modbus TCP MBAP",
            KnownHeader::SlcanAscii => "SLCAN ASCII command",
            KnownHeader::EspImageE9 => "ESP image magic (E9)",
            KnownHeader::Stm32Bootloader7f => "STM32 Bootloader sync (7F)",
            KnownHeader::NmeaGp => "NMEA sentence ($GP/$GN)",
        }
    }
}

/// 从数据流中提取的统计特征向量
///
/// 所有比例特征取值范围 [0.0, 1.0]，熵取值范围 [0.0, 8.0]，
/// 帧间隔特征单位为毫秒。特征向量可直接用于 KNN 距离计算。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamFeatures {
    /// 字节直方图（16 桶，每桶覆盖 16 个字节值），归一化到 [0,1]
    pub byte_histogram_16: [f32; 16],
    /// 香农熵（0.0-8.0 bits）
    pub shannon_entropy: f32,
    /// 可打印 ASCII 字符比例 (0x20-0x7E)
    pub printable_ratio: f32,
    /// 空字节 (0x00) 比例
    pub null_ratio: f32,
    /// 高字节 (>=0x80) 比例
    pub high_byte_ratio: f32,
    /// 控制字符 (0x00-0x1F，不含 \r\n\t) 比例
    pub control_ratio: f32,
    /// 平均字节值 (0-255)
    pub avg_byte_value: f32,
    /// 字节值标准差 (0-~127)
    pub byte_value_std: f32,
    /// 帧间隔均值 (ms)，无时序数据时为 0
    pub frame_interval_mean_ms: f32,
    /// 帧间隔标准差 (ms)
    pub frame_interval_std_ms: f32,
    /// 帧间隔变异系数 (std/mean)，衡量时序规律性，0 表示无数据
    pub frame_interval_cv: f32,
    /// 检测到的已知帧头
    pub detected_headers: Vec<KnownHeader>,
    /// 最长连续重复字节长度（run-length）
    pub max_run_length: u32,
    /// 数据长度
    pub data_length: usize,
}

impl StreamFeatures {
    /// 从原始字节流提取特征
    ///
    /// `data` 建议长度 >= 32 字节以保证统计有效性。
    /// `frame_intervals_ms` 可选，提供时序特征。
    pub fn extract(data: &[u8], frame_intervals_ms: Option<&[f32]>) -> Self {
        let data_length = data.len();
        if data_length == 0 {
            return Self::empty();
        }

        // ---- 字节直方图（256 桶 → 压缩为 16 桶）----
        let mut hist256 = [0u32; 256];
        for &b in data {
            hist256[b as usize] += 1;
        }
        let mut byte_histogram_16 = [0.0f32; 16];
        for i in 0..16 {
            let bucket_sum: u32 = hist256[i * 16..(i + 1) * 16].iter().sum();
            byte_histogram_16[i] = bucket_sum as f32 / data_length as f32;
        }

        // ---- 香农熵 ----
        let shannon_entropy = compute_shannon_entropy(&hist256, data_length);

        // ---- 字符分类比例 ----
        let mut printable = 0u32;
        let mut null_count = 0u32;
        let mut high_count = 0u32;
        let mut control_count = 0u32;
        let mut sum: u64 = 0;
        for &b in data {
            sum += b as u64;
            if b == 0 {
                null_count += 1;
            } else if b >= 0x80 {
                high_count += 1;
            }
            if (0x20..=0x7E).contains(&b) {
                printable += 1;
            }
            if b < 0x20 && b != b'\r' && b != b'\n' && b != b'\t' {
                control_count += 1;
            }
        }
        let n = data_length as f32;
        let printable_ratio = printable as f32 / n;
        let null_ratio = null_count as f32 / n;
        let high_byte_ratio = high_count as f32 / n;
        let control_ratio = control_count as f32 / n;
        let avg_byte_value = sum as f32 / n;

        // ---- 字节值标准差 ----
        let variance: f32 = data
            .iter()
            .map(|&b| {
                let d = b as f32 - avg_byte_value;
                d * d
            })
            .sum::<f32>()
            / n;
        let byte_value_std = variance.sqrt();

        // ---- 最长连续重复字节 ----
        let max_run_length = compute_max_run_length(data);

        // ---- 帧间隔统计 ----
        let (frame_interval_mean_ms, frame_interval_std_ms, frame_interval_cv) =
            compute_interval_stats(frame_intervals_ms);

        // ---- 已知帧头检测 ----
        let detected_headers = detect_known_headers(data);

        Self {
            byte_histogram_16,
            shannon_entropy,
            printable_ratio,
            null_ratio,
            high_byte_ratio,
            control_ratio,
            avg_byte_value,
            byte_value_std,
            frame_interval_mean_ms,
            frame_interval_std_ms,
            frame_interval_cv,
            detected_headers,
            max_run_length,
            data_length,
        }
    }

    /// 空特征（无数据时）
    pub fn empty() -> Self {
        Self {
            byte_histogram_16: [0.0; 16],
            shannon_entropy: 0.0,
            printable_ratio: 0.0,
            null_ratio: 0.0,
            high_byte_ratio: 0.0,
            control_ratio: 0.0,
            avg_byte_value: 0.0,
            byte_value_std: 0.0,
            frame_interval_mean_ms: 0.0,
            frame_interval_std_ms: 0.0,
            frame_interval_cv: 0.0,
            detected_headers: Vec::new(),
            max_run_length: 0,
            data_length: 0,
        }
    }

    /// 计算两个特征向量的欧氏距离（仅数值维度，不含 detected_headers）
    ///
    /// 各维度已归一化或取值范围一致，可直接相加。
    /// 帧间隔特征在有数据时才参与计算。
    pub fn distance(&self, other: &Self) -> f32 {
        let mut sum_sq = 0.0f32;

        // 字节直方图（16 维，每维 0-1）
        for i in 0..16 {
            let d = self.byte_histogram_16[i] - other.byte_histogram_16[i];
            sum_sq += d * d;
        }

        // 熵（0-8，归一化到 0-1）
        let d = (self.shannon_entropy - other.shannon_entropy) / 8.0;
        sum_sq += d * d;

        // 比例特征（0-1）
        let ratios = [
            self.printable_ratio - other.printable_ratio,
            self.null_ratio - other.null_ratio,
            self.high_byte_ratio - other.high_byte_ratio,
            self.control_ratio - other.control_ratio,
        ];
        for d in ratios {
            sum_sq += d * d;
        }

        // 平均字节值（0-255，归一化）
        let d = (self.avg_byte_value - other.avg_byte_value) / 255.0;
        sum_sq += d * d;

        // 标准差（0-~127，归一化）
        let d = (self.byte_value_std - other.byte_value_std) / 128.0;
        sum_sq += d * d;

        // 帧间隔特征（仅当双方都有数据时参与）
        if self.frame_interval_mean_ms > 0.0 && other.frame_interval_mean_ms > 0.0 {
            // 归一化：除以 100ms（常见帧间隔范围）
            let d = (self.frame_interval_mean_ms - other.frame_interval_mean_ms) / 100.0;
            sum_sq += d * d;
            let d = (self.frame_interval_cv - other.frame_interval_cv).abs();
            sum_sq += d * d;
        }

        sum_sq.sqrt()
    }

    /// 帧头匹配加分：若双方检测到相同帧头，返回匹配数量
    pub fn header_match_count(&self, other: &Self) -> usize {
        self.detected_headers
            .iter()
            .filter(|h| other.detected_headers.contains(h))
            .count()
    }
}

/// 计算香农熵（以 2 为底）
fn compute_shannon_entropy(hist256: &[u32; 256], total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let n = total as f32;
    let mut entropy = 0.0f32;
    for &count in hist256.iter() {
        if count > 0 {
            let p = count as f32 / n;
            entropy -= p * p.log2();
        }
    }
    entropy
}

/// 计算最长连续重复字节长度
fn compute_max_run_length(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let mut max_len = 1u32;
    let mut cur_len = 1u32;
    for i in 1..data.len() {
        if data[i] == data[i - 1] {
            cur_len += 1;
            if cur_len > max_len {
                max_len = cur_len;
            }
        } else {
            cur_len = 1;
        }
    }
    max_len
}

/// 计算帧间隔统计量 (mean, std, cv)
fn compute_interval_stats(intervals: Option<&[f32]>) -> (f32, f32, f32) {
    match intervals {
        Some(v) if v.len() >= 2 => {
            let n = v.len() as f32;
            let mean = v.iter().sum::<f32>() / n;
            let variance = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
            let std = variance.sqrt();
            let cv = if mean > 0.0 { std / mean } else { 0.0 };
            (mean, std, cv)
        }
        _ => (0.0, 0.0, 0.0),
    }
}

/// 检测数据中是否包含已知帧头模式
fn detect_known_headers(data: &[u8]) -> Vec<KnownHeader> {
    let mut headers = Vec::new();

    // 数据太短无法检测
    if data.len() < 2 {
        return headers;
    }

    // 鞋垫协议帧头 AA 55 03 99（优先于通用 AA 55）
    if data.windows(4).any(|w| w == [0xAA, 0x55, 0x03, 0x99]) {
        headers.push(KnownHeader::InsoleAa550399);
    } else if data.windows(2).any(|w| w == [0xAA, 0x55]) {
        headers.push(KnownHeader::Aa55);
    }

    // HDLC/PPP 0x7E 标志
    if data.iter().any(|&b| b == 0x7E) {
        headers.push(KnownHeader::Hdlc7e);
    }

    // ESP image 魔数 0xE9（通常在首字节）
    if data.first() == Some(&0xE9) {
        headers.push(KnownHeader::EspImageE9);
    }

    // STM32 Bootloader 同步 0x7F
    if data.iter().any(|&b| b == 0x7F) {
        headers.push(KnownHeader::Stm32Bootloader7f);
    }

    // NMEA 语句 $GP / $GN / $GL
    if data.windows(3).any(|w| w == b"$GP" || w == b"$GN" || w == b"$GL") {
        headers.push(KnownHeader::NmeaGp);
    }

    // SLCAN ASCII 命令（t/T/r/R/v/V 开头 + 数字）
    if let Some(&first) = data.first() {
        if matches!(first, b't' | b'T' | b'r' | b'R' | b'v' | b'V')
            && data.iter().take(8).all(|&b| b.is_ascii())
        {
            headers.push(KnownHeader::SlcanAscii);
        }
    }

    // Modbus TCP MBAP：前两字节为事务标识，第 3-4 字节为 0x0000，第 5-6 字节为长度
    if data.len() >= 8 && data[2] == 0x00 && data[3] == 0x00 && data[4] == 0x00 {
        headers.push(KnownHeader::ModbusMbap);
    }

    headers
}
