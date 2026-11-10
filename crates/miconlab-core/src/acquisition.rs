//! 采集配置和采样缓冲区模块。
//!
//! 提供 `AcquisitionConfig`（采集配置）、`ChannelConfig`（通道配置）、
//! `Trigger`（触发设置）以及 `SampleBuffer`（环形采样缓冲区）等核心类型，
//! 用于多通道数据采集、统计计算与 CSV 导出。

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// 采集配置
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AcquisitionConfig {
    /// 采样率 Hz
    pub sample_rate: f64,
    /// 通道配置
    pub channels: Vec<ChannelConfig>,
    /// 触发设置
    pub trigger: Option<Trigger>,
    /// 缓冲区大小（每个通道的样本数）
    pub buffer_size: usize,
    /// 是否自动记录到 CSV
    pub auto_record: bool,
    /// 记录文件路径
    pub record_path: Option<String>,
}

/// 通道配置
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChannelConfig {
    /// 通道名
    pub name: String,
    /// 通道索引
    pub channel_index: u8,
    /// 增益
    pub gain: f64,
    /// 偏移
    pub offset: f64,
    /// 单位
    pub unit: String,
    /// 校准系数
    pub calibration: Option<CalibrationCoefficients>,
    /// 是否启用
    pub enabled: bool,
    /// 波形颜色 RGB
    pub color: [u8; 3],
}

/// 触发设置
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Trigger {
    /// 触发通道
    pub channel: u8,
    /// 触发阈值
    pub level: f64,
    /// 触发方向
    pub direction: TriggerDirection,
    /// 触发前采样数
    pub pre_trigger_samples: usize,
}

/// 触发方向
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TriggerDirection {
    /// 上升沿
    Rising,
    /// 下降沿
    Falling,
    /// 双沿
    Both,
}

/// 校准系数
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CalibrationCoefficients {
    /// 斜率
    pub slope: f64,
    /// 截距
    pub intercept: f64,
}

/// 采样缓冲区（环形缓冲区）
#[derive(Clone)]
pub struct SampleBuffer {
    /// [channel][sample] 每通道的样本数据
    pub channels: Vec<Vec<f64>>,
    /// 时间戳序列
    pub timestamps: Vec<f64>,
    /// 最大样本数
    pub max_size: usize,
}

impl SampleBuffer {
    /// 创建一个新的采样缓冲区。
    ///
    /// - `channel_count`: 通道数量
    /// - `max_size`: 每个通道的最大样本数（超出后从头部丢弃旧样本）
    pub fn new(channel_count: usize, max_size: usize) -> Self {
        Self {
            channels: vec![Vec::with_capacity(max_size); channel_count],
            timestamps: Vec::with_capacity(max_size),
            max_size,
        }
    }

    /// 追加一个采样点。
    ///
    /// - `timestamp`: 当前样本的时间戳（秒）
    /// - `values`: 各通道的采样值，长度应与通道数一致（多余部分会被忽略，不足部分补 0.0）
    pub fn push(&mut self, timestamp: f64, values: &[f64]) {
        self.timestamps.push(timestamp);
        // 确保所有通道都推入值：不足的补 0.0，保持 timestamps 与 channels 长度同步
        for (i, ch) in self.channels.iter_mut().enumerate() {
            let v = values.get(i).copied().unwrap_or(0.0);
            ch.push(v);
        }
        // 超出最大容量时从头部丢弃
        if self.timestamps.len() > self.max_size {
            let overflow = self.timestamps.len() - self.max_size;
            self.timestamps.drain(0..overflow);
            for ch in &mut self.channels {
                let of = ch.len().saturating_sub(self.max_size);
                if of > 0 {
                    ch.drain(0..of);
                }
            }
        }
    }

    /// 清空所有通道与时间戳。
    pub fn clear(&mut self) {
        for ch in &mut self.channels {
            ch.clear();
        }
        self.timestamps.clear();
    }

    /// 获取指定通道的样本数据切片。
    pub fn channel_data(&self, ch: usize) -> &[f64] {
        self.channels
            .get(ch)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 获取指定通道的统计信息。
    pub fn stats(&self, ch: usize) -> ChannelStats {
        let data = self.channel_data(ch);
        if data.is_empty() {
            return ChannelStats::default();
        }
        let current = *data.last().unwrap();
        let min = data.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = data
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let sum: f64 = data.iter().sum();
        let avg = sum / data.len() as f64;
        let variance: f64 = data.iter().map(|v| (v - avg).powi(2)).sum::<f64>()
            / data.len() as f64;
        let std = variance.sqrt();
        ChannelStats {
            current,
            min,
            max,
            avg,
            std,
            sample_count: data.len(),
        }
    }

    /// 获取所有通道的统计信息。
    pub fn all_stats(&self) -> Vec<ChannelStats> {
        (0..self.channels.len()).map(|i| self.stats(i)).collect()
    }

    /// 当前样本数（以时间戳序列长度为准）。
    pub fn len(&self) -> usize {
        self.timestamps.len()
    }

    /// 缓冲区是否为空。
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// 导出为 CSV 文件。
    ///
    /// 表头为 `timestamp,ch0,ch1,...`，每行依次写入时间戳与各通道采样值。
    /// 文件使用 UTF-8 编码，写入失败时返回错误描述字符串。
    pub fn export_csv(&self, path: &Path) -> Result<(), String> {
        // 若父目录不存在则创建
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory: {}", e))?;
            }
        }

        let file = File::create(path).map_err(|e| format!("Failed to create file: {}", e))?;
        let mut writer = BufWriter::new(file);

        // 写表头：timestamp, ch0, ch1, ...
        let mut header = String::from("timestamp");
        for i in 0..self.channels.len() {
            header.push_str(&format!(",ch{}", i));
        }
        header.push('\n');
        writer
            .write_all(header.as_bytes())
            .map_err(|e| format!("Failed to write header: {}", e))?;

        // 写每行：时间戳 + 各通道值
        let n = self.timestamps.len();
        for row in 0..n {
            let mut line = format!("{}", self.timestamps[row]);
            for ch in &self.channels {
                let v = ch.get(row).copied().unwrap_or(f64::NAN);
                line.push(',');
                // NaN 输出为空字段，便于表格软件识别
                if v.is_nan() {
                    line.push_str("");
                } else {
                    line.push_str(&format_float(v));
                }
            }
            line.push('\n');
            writer
                .write_all(line.as_bytes())
                .map_err(|e| format!("Failed to write data: {}", e))?;
        }

        writer
            .flush()
            .map_err(|e| format!("Failed to flush buffer: {}", e))?;
        Ok(())
    }
}

/// 通道统计信息
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ChannelStats {
    /// 当前值
    pub current: f64,
    /// 最小值
    pub min: f64,
    /// 最大值
    pub max: f64,
    /// 平均值
    pub avg: f64,
    /// 标准差
    pub std: f64,
    /// 样本数
    pub sample_count: usize,
}

impl AcquisitionConfig {
    /// 返回一个默认的采集配置（单通道 CH0，100Hz，缓冲区 10000）。
    pub fn default_config() -> Self {
        Self {
            sample_rate: 100.0, // 100 Hz
            channels: vec![ChannelConfig {
                name: "CH0".to_string(),
                channel_index: 0,
                gain: 1.0,
                offset: 0.0,
                unit: "V".to_string(),
                calibration: None,
                enabled: true,
                color: [0x4F, 0xC3, 0xF7],
            }],
            trigger: None,
            buffer_size: 10000,
            auto_record: false,
            record_path: None,
        }
    }

    /// 应用校准到原始值。
    ///
    /// 若通道存在校准系数，先做线性校准 `slope*raw + intercept`，
    /// 再应用增益和偏移：`(calibrated) * gain + offset`。
    /// 若通道不存在则原样返回 `raw`。
    pub fn apply_calibration(&self, channel: usize, raw: f64) -> f64 {
        if let Some(ch) = self.channels.get(channel) {
            let calibrated = if let Some(cal) = &ch.calibration {
                cal.slope * raw + cal.intercept
            } else {
                raw
            };
            calibrated * ch.gain + ch.offset
        } else {
            raw
        }
    }
}

/// 将浮点数格式化为字符串，整数浮点数省略小数部分以提升可读性。
fn format_float(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e16 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时文件路径辅助函数
    fn temp_csv_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        dir.join(format!(
            "miconlab_acq_{}_{}.csv",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn test_default_config() {
        let cfg = AcquisitionConfig::default_config();
        assert!((cfg.sample_rate - 100.0).abs() < 1e-9);
        assert_eq!(cfg.channels.len(), 1);
        assert_eq!(cfg.channels[0].name, "CH0");
        assert_eq!(cfg.buffer_size, 10000);
        assert!(!cfg.auto_record);
        assert!(cfg.record_path.is_none());
        assert!(cfg.trigger.is_none());
    }

    #[test]
    fn test_apply_calibration_without_calibration() {
        let cfg = AcquisitionConfig::default_config();
        // 无校准系数，gain=1.0, offset=0.0，应原样返回
        let v = cfg.apply_calibration(0, 3.5);
        assert!((v - 3.5).abs() < 1e-9);
    }

    #[test]
    fn test_apply_calibration_with_gain_offset() {
        let mut cfg = AcquisitionConfig::default_config();
        cfg.channels[0].gain = 2.0;
        cfg.channels[0].offset = 1.0;
        // 无校准：raw * gain + offset = 5 * 2 + 1 = 11
        let v = cfg.apply_calibration(0, 5.0);
        assert!((v - 11.0).abs() < 1e-9);
    }

    #[test]
    fn test_apply_calibration_with_coefficients() {
        let mut cfg = AcquisitionConfig::default_config();
        cfg.channels[0].calibration = Some(CalibrationCoefficients {
            slope: 2.0,
            intercept: 1.0,
        });
        cfg.channels[0].gain = 3.0;
        cfg.channels[0].offset = 0.5;
        // 校准：2*10 + 1 = 21
        // 增益偏移：21 * 3 + 0.5 = 63.5
        let v = cfg.apply_calibration(0, 10.0);
        assert!((v - 63.5).abs() < 1e-9);
    }

    #[test]
    fn test_apply_calibration_out_of_range_channel() {
        let cfg = AcquisitionConfig::default_config();
        // 通道不存在，原样返回
        let v = cfg.apply_calibration(99, 7.7);
        assert!((v - 7.7).abs() < 1e-9);
    }

    #[test]
    fn test_sample_buffer_push_and_len() {
        let mut buf = SampleBuffer::new(2, 100);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        buf.push(0.0, &[1.0, 2.0]);
        buf.push(0.1, &[3.0, 4.0]);

        assert_eq!(buf.len(), 2);
        assert!(!buf.is_empty());
        assert_eq!(buf.channel_data(0), &[1.0, 3.0]);
        assert_eq!(buf.channel_data(1), &[2.0, 4.0]);
    }

    #[test]
    fn test_sample_buffer_ring_behavior() {
        // max_size = 3，超过后从头部丢弃
        let mut buf = SampleBuffer::new(1, 3);
        buf.push(0.0, &[10.0]);
        buf.push(1.0, &[20.0]);
        buf.push(2.0, &[30.0]);
        buf.push(3.0, &[40.0]); // 应丢弃 10.0

        assert_eq!(buf.len(), 3);
        assert_eq!(buf.channel_data(0), &[20.0, 30.0, 40.0]);
        assert_eq!(buf.timestamps, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_sample_buffer_push_more_values_than_channels() {
        let mut buf = SampleBuffer::new(1, 10);
        // 多余的值应被忽略
        buf.push(0.5, &[1.0, 2.0, 3.0]);
        assert_eq!(buf.channel_data(0), &[1.0]);
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn test_sample_buffer_clear() {
        let mut buf = SampleBuffer::new(2, 10);
        buf.push(0.0, &[1.0, 2.0]);
        buf.push(0.1, &[3.0, 4.0]);
        assert_eq!(buf.len(), 2);

        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.channel_data(0), &[] as &[f64]);
        assert_eq!(buf.channel_data(1), &[] as &[f64]);
    }

    #[test]
    fn test_channel_data_out_of_range() {
        let buf = SampleBuffer::new(1, 10);
        assert!(buf.channel_data(99).is_empty());
    }

    #[test]
    fn test_stats_empty() {
        let buf = SampleBuffer::new(1, 10);
        let s = buf.stats(0);
        assert_eq!(s.sample_count, 0);
    }

    #[test]
    fn test_stats_values() {
        let mut buf = SampleBuffer::new(1, 100);
        buf.push(0.0, &[1.0]);
        buf.push(0.1, &[2.0]);
        buf.push(0.2, &[3.0]);
        buf.push(0.3, &[4.0]);
        buf.push(0.4, &[5.0]);

        let s = buf.stats(0);
        assert_eq!(s.sample_count, 5);
        assert!((s.current - 5.0).abs() < 1e-9);
        assert!((s.min - 1.0).abs() < 1e-9);
        assert!((s.max - 5.0).abs() < 1e-9);
        assert!((s.avg - 3.0).abs() < 1e-9);
        // 标准差 = sqrt(2) ≈ 1.4142
        assert!((s.std - 2f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn test_all_stats() {
        let mut buf = SampleBuffer::new(2, 100);
        buf.push(0.0, &[1.0, 10.0]);
        buf.push(0.1, &[2.0, 20.0]);

        let all = buf.all_stats();
        assert_eq!(all.len(), 2);
        assert!((all[0].avg - 1.5).abs() < 1e-9);
        assert!((all[1].avg - 15.0).abs() < 1e-9);
    }

    #[test]
    fn test_export_csv_basic() {
        let path = temp_csv_path("basic");
        let mut buf = SampleBuffer::new(2, 100);
        buf.push(0.0, &[1.0, 2.0]);
        buf.push(0.1, &[3.0, 4.0]);

        buf.export_csv(&path).expect("导出 CSV 失败");

        let content = std::fs::read_to_string(&path).expect("读取 CSV 失败");
        // 表头
        assert!(content.starts_with("timestamp,ch0,ch1"));
        // 数据行
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3); // 1 表头 + 2 数据
        assert!(lines[1].contains("1"));
        assert!(lines[1].contains("2"));
        assert!(lines[2].contains("3"));
        assert!(lines[2].contains("4"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_export_csv_empty_buffer() {
        let path = temp_csv_path("empty");
        let buf = SampleBuffer::new(2, 100);

        buf.export_csv(&path).expect("导出空 CSV 失败");

        let content = std::fs::read_to_string(&path).expect("读取 CSV 失败");
        // 仅表头一行
        assert_eq!(content.lines().count(), 1);
        assert!(content.starts_with("timestamp,ch0,ch1"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_export_csv_missing_values() {
        // push 对缺失通道补 0.0（保证各通道与 timestamps 长度同步），CSV 导出为 0
        let path = temp_csv_path("missing");
        let mut buf = SampleBuffer::new(2, 100);
        // 仅第一通道有值
        buf.push(0.0, &[1.0]);

        buf.export_csv(&path).expect("导出 CSV 失败");

        let content = std::fs::read_to_string(&path).expect("读取 CSV 失败");
        let lines: Vec<&str> = content.lines().collect();
        // 第二列应为 1，第三列补 0
        let fields: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[1], "1");
        assert_eq!(fields[2], "0");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_trigger_direction_serialization() {
        let t = TriggerDirection::Rising;
        let json = serde_json::to_string(&t).expect("序列化失败");
        let back: TriggerDirection = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(t, back);
    }
}
