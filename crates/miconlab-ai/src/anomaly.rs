//! 数据流异常检测
//!
//! 基于滑动窗口统计模型的实时异常检测，识别数据流中的异常帧、协议偏离、
//! 突发噪声等异常情况。
//!
//! 检测维度：
//! - **熵突增/骤降**：数据熵的 z-score 超过阈值（协议切换或数据损坏）
//! - **分布偏移**：字节直方图与基线的卡方距离超过阈值（协议变化）
//! - **时序异常**：帧间隔 z-score 超过阈值（设备卡顿或丢包）
//! - **空字节爆发**：null 比例突然升高（设备无响应或线路故障）
//!
//! 工作流程：
//! 1. 前 N 次更新建立基线（学习期）
//! 2. 后续更新同时检测异常和自适应更新基线

use serde::{Deserialize, Serialize};

/// 异常类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnomalyKind {
    /// 熵突增（数据变得过于随机，可能是噪声或协议切换）
    EntropySpike,
    /// 熵骤降（数据变得过于规律，可能是重复数据或设备卡死）
    EntropyDrop,
    /// 字节分布偏移（协议可能发生变化）
    DistributionShift,
    /// 帧间隔异常（时序不规律，可能丢包或卡顿）
    TimingAnomaly,
    /// 空字节爆发（线路故障或设备无响应）
    NullBurst,
    /// 高字节爆发（数据流突然变为高字节主导）
    HighByteBurst,
}

impl AnomalyKind {
    pub fn label_zh(&self) -> &'static str {
        match self {
            AnomalyKind::EntropySpike => "Entropy spike",
            AnomalyKind::EntropyDrop => "Entropy drop",
            AnomalyKind::DistributionShift => "Distribution shift",
            AnomalyKind::TimingAnomaly => "Timing anomaly",
            AnomalyKind::NullBurst => "Null byte burst",
            AnomalyKind::HighByteBurst => "High byte burst",
        }
    }

    pub fn label_en(&self) -> &'static str {
        match self {
            AnomalyKind::EntropySpike => "Entropy spike",
            AnomalyKind::EntropyDrop => "Entropy drop",
            AnomalyKind::DistributionShift => "Distribution shift",
            AnomalyKind::TimingAnomaly => "Timing anomaly",
            AnomalyKind::NullBurst => "Null byte burst",
            AnomalyKind::HighByteBurst => "High byte burst",
        }
    }

    pub fn description_zh(&self) -> &'static str {
        match self {
            AnomalyKind::EntropySpike => "Entropy suddenly increased; possible protocol switch, noise injection, or data corruption",
            AnomalyKind::EntropyDrop => "Entropy suddenly decreased; possible repeated data, device hang, or transmission error",
            AnomalyKind::DistributionShift => "Byte distribution differs markedly from baseline; protocol may have changed",
            AnomalyKind::TimingAnomaly => "Abnormal frame arrival interval; possible packet loss, device stall, or buffer overflow",
            AnomalyKind::NullBurst => "Null-byte ratio suddenly increased; possible line fault, device unresponsive, or misconfiguration",
            AnomalyKind::HighByteBurst => "High-byte (>=0x80) ratio suddenly increased; possible binary data mixing or encoding error",
        }
    }
}

/// 异常告警
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyAlert {
    /// 异常类型
    pub kind: AnomalyKind,
    /// 严重程度 0.0-1.0
    pub severity: f32,
    /// 异常描述
    pub message: String,
    /// 检测时间戳（Unix 毫秒）
    pub timestamp_ms: i64,
    /// 异常数据片段（HEX 格式，最多 32 字节）
    pub data_hex: String,
}

/// 异常检测器
///
/// 维护滑动窗口的统计基线，对每帧新数据进行异常检测。
/// 前 `warmup_samples` 次更新为学习期，仅建立基线不产生告警。
#[derive(Clone)]
pub struct AnomalyDetector {
    /// 滑动窗口大小（预留，用于未来扩展真正的滑动窗口缓冲）
    #[allow(dead_code)]
    window_size: usize,
    /// 基线字节频率（256 桶）
    baseline_hist: [f32; 256],
    /// 基线熵
    baseline_entropy: f32,
    /// 基线熵的移动平均和方差（用于 z-score）
    entropy_mean: f32,
    entropy_var: f32,
    /// 基线帧间隔统计
    interval_mean: f32,
    interval_var: f32,
    /// 基线 null 比例
    baseline_null_ratio: f32,
    /// 基线高字节比例
    baseline_high_ratio: f32,
    /// 已处理的样本数（用于判断是否完成 warmup）
    sample_count: usize,
    /// warmup 样本数（达到后开始检测）
    warmup_samples: usize,
    /// 检测灵敏度（0.0-1.0，越高越敏感）
    sensitivity: f32,
}

impl AnomalyDetector {
    /// 创建检测器
    ///
    /// `window_size` 滑动窗口大小（建议 128-512）
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size: window_size.max(32),
            baseline_hist: [0.0; 256],
            baseline_entropy: 0.0,
            entropy_mean: 0.0,
            entropy_var: 0.0,
            interval_mean: 0.0,
            interval_var: 0.0,
            baseline_null_ratio: 0.0,
            baseline_high_ratio: 0.0,
            sample_count: 0,
            warmup_samples: 10,
            sensitivity: 0.7,
        }
    }

    /// 设置检测灵敏度（0.0-1.0）
    pub fn set_sensitivity(&mut self, sensitivity: f32) {
        self.sensitivity = sensitivity.clamp(0.0, 1.0);
    }

    /// 获取当前检测灵敏度
    pub fn sensitivity(&self) -> f32 {
        self.sensitivity
    }

    /// 更新基线并检测异常
    ///
    /// 在 GUI 每帧 poll_rx 后调用。返回检测到的异常（如果有）。
    /// 学习期（前 warmup_samples 次）不产生告警。
    pub fn update(&mut self, data: &[u8]) -> Option<AnomalyAlert> {
        if data.is_empty() {
            return None;
        }

        // 计算当前数据的特征
        let mut hist = [0u32; 256];
        for &b in data {
            hist[b as usize] += 1;
        }
        let n = data.len() as f32;
        let mut hist_f = [0.0f32; 256];
        for i in 0..256 {
            hist_f[i] = hist[i] as f32 / n;
        }

        let current_entropy = compute_entropy(&hist, data.len());
        let current_null_ratio = hist[0] as f32 / n;
        let current_high_ratio: f32 = hist[128..].iter().sum::<u32>() as f32 / n;

        let alert = if self.sample_count >= self.warmup_samples {
            // 检测期
            self.detect_anomaly(
                data,
                &hist_f,
                current_entropy,
                current_null_ratio,
                current_high_ratio,
            )
        } else {
            None
        };

        // 自适应更新基线（指数移动平均，慢更新）
        let alpha = 0.1;
        if self.sample_count == 0 {
            // 首次直接初始化
            self.baseline_hist = hist_f;
            self.baseline_entropy = current_entropy;
            self.entropy_mean = current_entropy;
            self.entropy_var = 0.5;
            self.baseline_null_ratio = current_null_ratio;
            self.baseline_high_ratio = current_high_ratio;
        } else {
            for i in 0..256 {
                self.baseline_hist[i] = self.baseline_hist[i] * (1.0 - alpha) + hist_f[i] * alpha;
            }
            // 熵统计量更新
            let delta = current_entropy - self.entropy_mean;
            self.entropy_mean += alpha * delta;
            self.entropy_var = (1.0 - alpha) * self.entropy_var + alpha * delta * delta;
            self.baseline_entropy =
                self.baseline_entropy * (1.0 - alpha) + current_entropy * alpha;
            self.baseline_null_ratio =
                self.baseline_null_ratio * (1.0 - alpha) + current_null_ratio * alpha;
            self.baseline_high_ratio =
                self.baseline_high_ratio * (1.0 - alpha) + current_high_ratio * alpha;
        }

        self.sample_count += 1;
        alert
    }

    /// 更新时序基线
    pub fn update_timing(&mut self, intervals_ms: &[f32]) {
        if intervals_ms.len() < 2 {
            return;
        }
        let mean = intervals_ms.iter().sum::<f32>() / intervals_ms.len() as f32;
        let variance = intervals_ms
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f32>()
            / intervals_ms.len() as f32;

        if self.sample_count < self.warmup_samples {
            self.interval_mean = mean;
            self.interval_var = variance;
        } else {
            let alpha = 0.1;
            self.interval_mean = self.interval_mean * (1.0 - alpha) + mean * alpha;
            self.interval_var = self.interval_var * (1.0 - alpha) + variance * alpha;
        }
    }

    /// 仅检测异常，不更新基线
    pub fn check(&self, data: &[u8]) -> Option<AnomalyAlert> {
        if data.is_empty() || self.sample_count < self.warmup_samples {
            return None;
        }

        let mut hist = [0u32; 256];
        for &b in data {
            hist[b as usize] += 1;
        }
        let n = data.len() as f32;
        let mut hist_f = [0.0f32; 256];
        for i in 0..256 {
            hist_f[i] = hist[i] as f32 / n;
        }
        let current_entropy = compute_entropy(&hist, data.len());
        let current_null_ratio = hist[0] as f32 / n;
        let current_high_ratio: f32 = hist[128..].iter().sum::<u32>() as f32 / n;

        self.detect_anomaly(
            data,
            &hist_f,
            current_entropy,
            current_null_ratio,
            current_high_ratio,
        )
    }

    /// 是否已完成 warmup（基线建立完成）
    pub fn is_warmed_up(&self) -> bool {
        self.sample_count >= self.warmup_samples
    }

    /// 获取当前基线熵
    pub fn baseline_entropy(&self) -> f32 {
        self.baseline_entropy
    }

    /// 获取样本计数
    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// 执行异常检测
    fn detect_anomaly(
        &self,
        data: &[u8],
        hist_f: &[f32; 256],
        current_entropy: f32,
        current_null_ratio: f32,
        current_high_ratio: f32,
    ) -> Option<AnomalyAlert> {
        // 灵敏度转阈值：灵敏度越高，阈值越低
        let z_threshold = 3.0 - self.sensitivity * 1.5; // 1.5 - 3.0
        let ratio_threshold = 0.3 + (1.0 - self.sensitivity) * 0.2; // 0.3 - 0.5

        let now_ms = chrono::Local::now().timestamp_millis();
        let data_hex = data
            .iter()
            .take(32)
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ");

        // 1. 熵异常（z-score）
        let entropy_std = self.entropy_var.sqrt().max(0.1);
        let entropy_z = (current_entropy - self.entropy_mean).abs() / entropy_std;
        if entropy_z > z_threshold {
            if current_entropy > self.entropy_mean {
                return Some(AnomalyAlert {
                    kind: AnomalyKind::EntropySpike,
                    severity: ((entropy_z - z_threshold) / 3.0).clamp(0.0, 1.0),
                    message: format!(
                        "Entropy spiked to {:.2} bits (baseline {:.2}, z-score {:.1})",
                        current_entropy, self.entropy_mean, entropy_z
                    ),
                    timestamp_ms: now_ms,
                    data_hex,
                });
            } else {
                return Some(AnomalyAlert {
                    kind: AnomalyKind::EntropyDrop,
                    severity: ((entropy_z - z_threshold) / 3.0).clamp(0.0, 1.0),
                    message: format!(
                        "Entropy dropped to {:.2} bits (baseline {:.2}, z-score {:.1})",
                        current_entropy, self.entropy_mean, entropy_z
                    ),
                    timestamp_ms: now_ms,
                    data_hex,
                });
            }
        }

        // 2. 分布偏移（卡方距离）
        let chi_sq: f32 = (0..256)
            .map(|i| {
                let expected = self.baseline_hist[i].max(0.001);
                let observed = hist_f[i];
                (observed - expected).powi(2) / expected
            })
            .sum();
        // 卡方距离归一化（除以自由度 255）
        let chi_norm = chi_sq / 255.0;
        if chi_norm > 0.5 + (1.0 - self.sensitivity) * 0.5 {
            return Some(AnomalyAlert {
                kind: AnomalyKind::DistributionShift,
                severity: ((chi_norm - 0.5) / 1.5).clamp(0.0, 1.0),
                message: format!(
                    "Byte distribution shifted (chi-square distance {:.3}, above threshold)",
                    chi_norm
                ),
                timestamp_ms: now_ms,
                data_hex,
            });
        }

        // 3. 空字节爆发
        if current_null_ratio > self.baseline_null_ratio + ratio_threshold
            && current_null_ratio > 0.2
        {
            return Some(AnomalyAlert {
                kind: AnomalyKind::NullBurst,
                severity: ((current_null_ratio - self.baseline_null_ratio) / 0.5).clamp(0.0, 1.0),
                message: format!(
                    "Null-byte ratio spiked to {:.0}% (baseline {:.0}%)",
                    current_null_ratio * 100.0,
                    self.baseline_null_ratio * 100.0
                ),
                timestamp_ms: now_ms,
                data_hex,
            });
        }

        // 4. 高字节爆发
        if current_high_ratio > self.baseline_high_ratio + ratio_threshold
            && current_high_ratio > 0.5
        {
            return Some(AnomalyAlert {
                kind: AnomalyKind::HighByteBurst,
                severity: ((current_high_ratio - self.baseline_high_ratio) / 0.5).clamp(0.0, 1.0),
                message: format!(
                    "High-byte ratio spiked to {:.0}% (baseline {:.0}%)",
                    current_high_ratio * 100.0,
                    self.baseline_high_ratio * 100.0
                ),
                timestamp_ms: now_ms,
                data_hex,
            });
        }

        None
    }

    /// 检测时序异常（帧间隔）
    pub fn check_timing_anomaly(&self, interval_ms: f32) -> Option<AnomalyAlert> {
        if self.sample_count < self.warmup_samples || self.interval_var < 0.01 {
            return None;
        }
        let std = self.interval_var.sqrt();
        let z = (interval_ms - self.interval_mean).abs() / std;
        let z_threshold = 3.0 - self.sensitivity * 1.5;
        if z > z_threshold {
            let now_ms = chrono::Local::now().timestamp_millis();
            return Some(AnomalyAlert {
                kind: AnomalyKind::TimingAnomaly,
                severity: ((z - z_threshold) / 3.0).clamp(0.0, 1.0),
                message: format!(
                    "Frame interval anomaly: {:.1}ms (baseline mean {:.1}ms, z-score {:.1})",
                    interval_ms, self.interval_mean, z
                ),
                timestamp_ms: now_ms,
                data_hex: String::new(),
            });
        }
        None
    }
}

/// 计算香农熵
fn compute_entropy(hist: &[u32; 256], total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let n = total as f32;
    let mut entropy = 0.0f32;
    for &count in hist.iter() {
        if count > 0 {
            let p = count as f32 / n;
            entropy -= p * p.log2();
        }
    }
    entropy
}
