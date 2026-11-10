//! 传感器智能分析模块
//!
//! 提供传感器数据流分析、类型识别、异常检测、质量评估和采集配置推荐能力。
//! 纯启发式分析，离线运行。

use serde::{Deserialize, Serialize};

/// 传感器类型（AI 推断）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensorType {
    /// 压力/力传感器（鞋垫压力、称重等）
    Pressure,
    /// 温度传感器
    Temperature,
    /// 加速度计
    Accelerometer,
    /// 陀螺仪
    Gyroscope,
    /// IMU（加速度+陀螺仪）
    Imu,
    /// 电压/电流传感器
    Voltage,
    /// 光照传感器
    Light,
    /// 距离/位移传感器
    Distance,
    /// 未知/通用
    Unknown,
}

impl SensorType {
    pub fn label_zh(&self) -> &'static str {
        match self {
            SensorType::Pressure => "Pressure Sensor",
            SensorType::Temperature => "Temperature Sensor",
            SensorType::Accelerometer => "Accelerometer",
            SensorType::Gyroscope => "Gyroscope",
            SensorType::Imu => "IMU (Accel+Gyro)",
            SensorType::Voltage => "Voltage/Current Sensor",
            SensorType::Light => "Light Sensor",
            SensorType::Distance => "Distance Sensor",
            SensorType::Unknown => "Unknown Sensor",
        }
    }

    pub fn label_en(&self) -> &'static str {
        match self {
            SensorType::Pressure => "Pressure Sensor",
            SensorType::Temperature => "Temperature Sensor",
            SensorType::Accelerometer => "Accelerometer",
            SensorType::Gyroscope => "Gyroscope",
            SensorType::Imu => "IMU (Accel+Gyro)",
            SensorType::Voltage => "Voltage/Current Sensor",
            SensorType::Light => "Light Sensor",
            SensorType::Distance => "Distance Sensor",
            SensorType::Unknown => "Unknown Sensor",
        }
    }

    /// 推荐单位
    pub fn recommended_unit(&self) -> &'static str {
        match self {
            SensorType::Pressure => "N/cm²",
            SensorType::Temperature => "°C",
            SensorType::Accelerometer => "m/s²",
            SensorType::Gyroscope => "°/s",
            SensorType::Imu => "m/s²",
            SensorType::Voltage => "V",
            SensorType::Light => "lux",
            SensorType::Distance => "mm",
            SensorType::Unknown => "—",
        }
    }

    /// 推荐量程范围 (min, max)
    pub fn recommended_range(&self) -> (f64, f64) {
        match self {
            SensorType::Pressure => (0.0, 500.0),
            SensorType::Temperature => (-40.0, 125.0),
            SensorType::Accelerometer => (-16.0, 16.0),
            SensorType::Gyroscope => (-2000.0, 2000.0),
            SensorType::Imu => (-16.0, 16.0),
            SensorType::Voltage => (0.0, 3.3),
            SensorType::Light => (0.0, 65535.0),
            SensorType::Distance => (0.0, 1000.0),
            SensorType::Unknown => (0.0, 4095.0),
        }
    }

    /// 推荐采样率
    pub fn recommended_sample_rate(&self) -> f64 {
        match self {
            SensorType::Pressure => 100.0,
            SensorType::Temperature => 10.0,
            SensorType::Accelerometer => 1000.0,
            SensorType::Gyroscope => 1000.0,
            SensorType::Imu => 1000.0,
            SensorType::Voltage => 50.0,
            SensorType::Light => 100.0,
            SensorType::Distance => 50.0,
            SensorType::Unknown => 100.0,
        }
    }
}

/// 传感器数据异常类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensorAnomaly {
    /// 数据突变（尖峰）
    Spike,
    /// 数据漂移（缓慢偏移）
    Drift,
    /// 数据卡死（持续不变）
    Stuck,
    /// 数据溢出（超出量程）
    Overflow,
    /// 通道丢失（某通道无数据）
    ChannelLoss,
    /// 高噪声
    HighNoise,
    /// 周期性干扰
    PeriodicInterference,
}

impl SensorAnomaly {
    pub fn label_zh(&self) -> &'static str {
        match self {
            SensorAnomaly::Spike => "Data spike",
            SensorAnomaly::Drift => "Data drift",
            SensorAnomaly::Stuck => "Data stuck",
            SensorAnomaly::Overflow => "Data overflow",
            SensorAnomaly::ChannelLoss => "Channel loss",
            SensorAnomaly::HighNoise => "High noise",
            SensorAnomaly::PeriodicInterference => "Periodic interference",
        }
    }
}

/// 传感器数据分析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorAnalysis {
    /// 推断的传感器类型
    pub sensor_type: SensorType,
    /// 类型推断置信度 (0.0-1.0)
    pub type_confidence: f32,
    /// 数据质量评分 (0.0-1.0)
    pub quality_score: f32,
    /// 检测到的异常列表
    pub anomalies: Vec<(SensorAnomaly, String)>,
    /// 信噪比估算 (dB)
    pub snr_db: f32,
    /// 数据稳定性 (0.0-1.0，基于变异系数)
    pub stability: f32,
    /// 推荐采样率
    pub recommended_sample_rate: f64,
    /// 每通道统计摘要
    pub channel_stats: Vec<ChannelStats>,
    /// 分析摘要（中文）
    pub summary_zh: String,
}

/// 单通道统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelStats {
    pub channel: usize,
    pub mean: f64,
    pub std: f64,
    pub min: f64,
    pub max: f64,
    /// 变异系数 (std/mean)
    pub cv: f64,
    /// 是否活跃（有变化）
    pub active: bool,
}

impl SensorAnalysis {
    /// 分析多通道传感器数据
    ///
    /// `channel_data` 为每通道的时间序列数据（f64 值）
    pub fn analyze(channel_data: &[&[f64]]) -> Self {
        if channel_data.is_empty() || channel_data[0].is_empty() {
            return Self {
                sensor_type: SensorType::Unknown,
                type_confidence: 0.0,
                quality_score: 0.0,
                anomalies: Vec::new(),
                snr_db: 0.0,
                stability: 0.0,
                recommended_sample_rate: 100.0,
                channel_stats: Vec::new(),
                summary_zh: "Insufficient data to analyze".to_string(),
            };
        }

        // 计算每通道统计
        let channel_stats: Vec<ChannelStats> = channel_data
            .iter()
            .enumerate()
            .map(|(i, data)| compute_channel_stats(i, data))
            .collect();

        // 推断传感器类型
        let (sensor_type, type_confidence) = infer_sensor_type(&channel_stats);

        // 计算信噪比
        let snr_db = compute_snr(&channel_stats);

        // 计算稳定性
        let stability = compute_stability(&channel_stats);

        // 检测异常
        let anomalies = detect_anomalies(channel_data, &channel_stats);

        // 计算质量评分
        let quality_score = compute_quality_score(snr_db, stability, &anomalies);

        // 推荐采样率
        let recommended_sample_rate = sensor_type.recommended_sample_rate();

        // 生成摘要
        let summary_zh = build_summary(
            &sensor_type,
            type_confidence,
            quality_score,
            snr_db,
            stability,
            &anomalies,
        );

        Self {
            sensor_type,
            type_confidence,
            quality_score,
            anomalies,
            snr_db,
            stability,
            recommended_sample_rate,
            channel_stats,
            summary_zh,
        }
    }
}

/// 计算单通道统计
fn compute_channel_stats(channel: usize, data: &[f64]) -> ChannelStats {
    let n = data.len() as f64;
    let mean = data.iter().sum::<f64>() / n;
    let variance = data.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    let std = variance.sqrt();
    let min = data.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = data.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let cv = if mean.abs() > 1e-9 { std / mean.abs() } else { 0.0 };

    // 判断是否活跃（标准差大于阈值的 5%）
    let range = (max - min).abs();
    let active = std > range * 0.01 || range > 0.001;

    ChannelStats {
        channel,
        mean,
        std,
        min,
        max,
        cv,
        active,
    }
}

/// 推断传感器类型
fn infer_sensor_type(stats: &[ChannelStats]) -> (SensorType, f32) {
    let active_count = stats.iter().filter(|s| s.active).count();
    let total = stats.len();

    // 所有通道的均值范围
    let all_min = stats.iter().map(|s| s.min).fold(f64::INFINITY, f64::min);
    let all_max = stats.iter().map(|s| s.max).fold(f64::NEG_INFINITY, f64::max);
    let range = all_max - all_min;

    // 通道数启发式
    match total {
        1 => {
            // 单通道
            // 温度: 范围小、变化慢、值在 -40~125
            if all_mean_in_range(stats, -40.0, 125.0) && range < 50.0 && all_cv_low(stats, 0.1) {
                return (SensorType::Temperature, 0.7);
            }
            // 电压: 0~3.3V 或 0~5V
            if all_mean_in_range(stats, 0.0, 5.0) && range < 5.0 {
                return (SensorType::Voltage, 0.6);
            }
            // 光照: 0~65535, 变化大
            if all_mean_in_range(stats, 0.0, 65535.0) && range > 100.0 {
                return (SensorType::Light, 0.5);
            }
            // 距离: 正值，范围有限
            if all_mean_in_range(stats, 0.0, 5000.0) && range > 10.0 {
                return (SensorType::Distance, 0.4);
            }
            // 压力: 非负，变化适中
            if all_mean_in_range(stats, 0.0, 1000.0) {
                return (SensorType::Pressure, 0.5);
            }
            (SensorType::Unknown, 0.3)
        }
        2 => {
            // 双通道：可能是差分压力或温度+湿度
            (SensorType::Pressure, 0.4)
        }
        3 => {
            // 3 轴：加速度计或陀螺仪
            // 加速度: 范围 ±2g~±16g (±19.6~±157 m/s²)，通常有一个轴接近重力(9.8)
            let has_gravity = stats.iter().any(|s| (s.mean.abs() - 9.8).abs() < 3.0);
            if has_gravity || all_mean_in_range(stats, -20.0, 20.0) {
                return (SensorType::Accelerometer, 0.8);
            }
            // 陀螺仪: 范围 ±2000°/s，静止时接近 0
            if all_mean_in_range(stats, -2000.0, 2000.0) && all_cv_low(stats, 0.05) {
                return (SensorType::Gyroscope, 0.6);
            }
            (SensorType::Unknown, 0.4)
        }
        6 => {
            // 6 轴 IMU（3 加速度 + 3 陀螺仪）
            let has_gravity = stats.iter().take(3).any(|s| (s.mean.abs() - 9.8).abs() < 3.0);
            if has_gravity {
                return (SensorType::Imu, 0.85);
            }
            (SensorType::Imu, 0.5)
        }
        n if n >= 8 => {
            // 多通道（8+）：压力阵列（鞋垫等）
            if all_mean_in_range(stats, 0.0, 5000.0) && active_count >= total / 2 {
                return (SensorType::Pressure, 0.75);
            }
            (SensorType::Unknown, 0.4)
        }
        _ => (SensorType::Unknown, 0.3),
    }
}

/// 检查所有通道均值是否在范围内
fn all_mean_in_range(stats: &[ChannelStats], min: f64, max: f64) -> bool {
    stats.iter().all(|s| s.mean >= min && s.mean <= max)
}

/// 检查所有通道变异系数是否低于阈值
fn all_cv_low(stats: &[ChannelStats], threshold: f64) -> bool {
    stats.iter().all(|s| s.cv.abs() < threshold)
}

/// 计算信噪比 (dB)
fn compute_snr(stats: &[ChannelStats]) -> f32 {
    let active: Vec<&ChannelStats> = stats.iter().filter(|s| s.active).collect();
    if active.is_empty() {
        return 0.0;
    }
    let avg_signal = active.iter().map(|s| s.mean.abs()).sum::<f64>() / active.len() as f64;
    let avg_noise = active.iter().map(|s| s.std).sum::<f64>() / active.len() as f64;
    if avg_noise < 1e-9 {
        return 60.0; // 几乎无噪声
    }
    let snr = avg_signal / avg_noise;
    (20.0 * snr.log10()) as f32
}

/// 计算稳定性 (0.0-1.0)
fn compute_stability(stats: &[ChannelStats]) -> f32 {
    let active: Vec<&ChannelStats> = stats.iter().filter(|s| s.active).collect();
    if active.is_empty() {
        return 0.0;
    }
    // 稳定性 = 1 - min(cv, 1)，cv 越小越稳定
    let avg_cv = active.iter().map(|s| s.cv.abs()).sum::<f64>() / active.len() as f64;
    (1.0 - avg_cv.min(1.0)) as f32
}

/// 检测传感器数据异常
fn detect_anomalies(channel_data: &[&[f64]], stats: &[ChannelStats]) -> Vec<(SensorAnomaly, String)> {
    let mut anomalies = Vec::new();

    for (ch_idx, data) in channel_data.iter().enumerate() {
        if data.len() < 10 {
            continue;
        }
        let s = &stats[ch_idx];

        // 1. 数据卡死检测：最后 N 个值的标准差接近 0
        let tail_len = data.len().min(20);
        let tail = &data[data.len() - tail_len..];
        let tail_std = {
            let mean = tail.iter().sum::<f64>() / tail_len as f64;
            (tail.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / tail_len as f64).sqrt()
        };
        if tail_std < 1e-6 && s.active {
            anomalies.push((
                SensorAnomaly::Stuck,
                format!("Channel {} data stuck (last {} samples unchanged)", ch_idx, tail_len),
            ));
        }

        // 2. 数据突变检测：存在超过 5 倍标准差的点
        if s.std > 1e-9 {
            let threshold = s.mean + 5.0 * s.std;
            let spike_count = data.iter().filter(|&&v| v.abs() > threshold.abs() && (v - s.mean).abs() > 5.0 * s.std).count();
            if spike_count > 0 {
                anomalies.push((
                    SensorAnomaly::Spike,
                    format!("Channel {} has {} spike(s) (>5σ)", ch_idx, spike_count),
                ));
            }
        }

        // 3. 数据漂移检测：前 1/4 和后 1/4 的均值差异显著
        if data.len() >= 40 {
            let quarter = data.len() / 4;
            let first_mean = data[..quarter].iter().sum::<f64>() / quarter as f64;
            let last_mean = data[data.len() - quarter..].iter().sum::<f64>() / quarter as f64;
            let drift = (last_mean - first_mean).abs();
            if drift > 3.0 * s.std && s.std > 1e-6 {
                anomalies.push((
                    SensorAnomaly::Drift,
                    format!("Channel {} data drift {:.2} (head-to-tail mean difference >3σ)", ch_idx, drift),
                ));
            }
        }

        // 4. 高噪声检测：变异系数过高
        if s.cv > 0.5 && s.active {
            anomalies.push((
                SensorAnomaly::HighNoise,
                format!("Channel {} high noise (coefficient of variation {:.2})", ch_idx, s.cv),
            ));
        }

        // 5. 通道丢失：所有值相同且为 0
        if s.max == 0.0 && s.min == 0.0 {
            anomalies.push((
                SensorAnomaly::ChannelLoss,
                format!("Channel {} has no valid data (all zeros)", ch_idx),
            ));
        }
    }

    // 6. 周期性干扰检测（通过自相关简化检测）
    if channel_data.iter().any(|d| d.len() >= 100) {
        for (ch_idx, data) in channel_data.iter().enumerate() {
            if data.len() < 100 {
                continue;
            }
            if let Some(period) = detect_periodicity(data) {
                anomalies.push((
                    SensorAnomaly::PeriodicInterference,
                    format!("Channel {} has periodic interference (period ≈ {} samples)", ch_idx, period),
                ));
            }
        }
    }

    anomalies
}

/// 简化的周期性检测（自相关峰值）
fn detect_periodicity(data: &[f64]) -> Option<usize> {
    let n = data.len();
    if n < 100 {
        return None;
    }

    let mean = data.iter().sum::<f64>() / n as f64;
    let centered: Vec<f64> = data.iter().map(|v| v - mean).collect();
    let variance: f64 = centered.iter().map(|v| v * v).sum::<f64>() / n as f64;
    if variance < 1e-9 {
        return None;
    }

    // 检查 lag 10..n/4 范围内的自相关
    let max_lag = n / 4;
    let min_lag = 10;
    let mut best_lag = 0usize;
    let mut best_corr = 0.0f64;

    for lag in min_lag..max_lag {
        let mut sum = 0.0;
        for i in 0..(n - lag) {
            sum += centered[i] * centered[i + lag];
        }
        let corr = sum / ((n - lag) as f64 * variance);
        if corr > best_corr {
            best_corr = corr;
            best_lag = lag;
        }
    }

    // 自相关峰值 > 0.5 认为有周期性
    if best_corr > 0.5 && best_lag > 0 {
        Some(best_lag)
    } else {
        None
    }
}

/// 计算质量评分
fn compute_quality_score(snr_db: f32, stability: f32, anomalies: &[(SensorAnomaly, String)]) -> f32 {
    let mut score = 1.0f32;

    // SNR 影响：SNR < 20dB 扣分
    if snr_db < 20.0 {
        score *= 0.5 + 0.5 * (snr_db / 20.0).max(0.0);
    }

    // 稳定性影响
    score *= 0.3 + 0.7 * stability;

    // 异常扣分
    for (kind, _) in anomalies {
        match kind {
            SensorAnomaly::Stuck => score *= 0.5,
            SensorAnomaly::ChannelLoss => score *= 0.3,
            SensorAnomaly::Spike => score *= 0.7,
            SensorAnomaly::HighNoise => score *= 0.6,
            SensorAnomaly::Drift => score *= 0.8,
            SensorAnomaly::PeriodicInterference => score *= 0.8,
            SensorAnomaly::Overflow => score *= 0.5,
        }
    }

    score.clamp(0.0, 1.0)
}

/// 构建分析摘要
fn build_summary(
    sensor_type: &SensorType,
    type_conf: f32,
    quality: f32,
    snr_db: f32,
    stability: f32,
    anomalies: &[(SensorAnomaly, String)],
) -> String {
    let quality_label = if quality >= 0.8 {
        "Excellent"
    } else if quality >= 0.6 {
        "Good"
    } else if quality >= 0.4 {
        "Fair"
    } else {
        "Poor"
    };

    let mut summary = format!(
        "Identified: {} ({:.0}%), quality: {} ({:.0}%), SNR: {:.1}dB, stability: {:.0}%",
        sensor_type.label_zh(),
        type_conf * 100.0,
        quality_label,
        quality * 100.0,
        snr_db,
        stability * 100.0,
    );

    if !anomalies.is_empty() {
        summary.push_str(&format!(", anomalies: {}", anomalies.len()));
    }

    summary
}

// ==================== 校准分析 ====================

/// 校准点质量评估结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationAssessment {
    /// 点分布均匀性 (0.0-1.0)
    pub uniformity: f32,
    /// 是否存在离群点
    pub outliers: Vec<usize>,
    /// 推荐校准模型
    pub recommended_model: CalibrationModel,
    /// 拟合残差 (RMSE)
    pub rmse: f64,
    /// 拟合 R²
    pub r_squared: f64,
    /// 建议补充校准点的区域
    pub gap_regions: Vec<(f64, f64)>,
    /// 评估摘要
    pub summary_zh: String,
}

/// 校准模型类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalibrationModel {
    /// 零点校准（1 点）
    ZeroOnly,
    /// 零点 + 满量程（2 点线性）
    ZeroSpan,
    /// 多点线性拟合
    Linear,
    /// 多项式拟合
    Polynomial,
}

impl CalibrationModel {
    pub fn label_zh(&self) -> &'static str {
        match self {
            CalibrationModel::ZeroOnly => "Zero-point calibration",
            CalibrationModel::ZeroSpan => "Zero + full span",
            CalibrationModel::Linear => "Multi-point linear fit",
            CalibrationModel::Polynomial => "Polynomial fit",
        }
    }
}

impl CalibrationAssessment {
    /// 评估校准点质量并推荐模型
    ///
    /// `points` 为 (raw_value, standard_value) 对
    pub fn assess(points: &[(f64, f64)]) -> Self {
        if points.is_empty() {
            return Self {
                uniformity: 0.0,
                outliers: Vec::new(),
                recommended_model: CalibrationModel::ZeroOnly,
                rmse: 0.0,
                r_squared: 0.0,
                gap_regions: Vec::new(),
                summary_zh: "No calibration points".to_string(),
            };
        }

        if points.len() == 1 {
            return Self {
                uniformity: 1.0,
                outliers: Vec::new(),
                recommended_model: CalibrationModel::ZeroOnly,
                rmse: 0.0,
                r_squared: 1.0,
                gap_regions: Vec::new(),
                summary_zh: "Single-point calibration (zero-point)".to_string(),
            };
        }

        if points.len() == 2 {
            return Self {
                uniformity: 1.0,
                outliers: Vec::new(),
                recommended_model: CalibrationModel::ZeroSpan,
                rmse: 0.0,
                r_squared: 1.0,
                gap_regions: Vec::new(),
                summary_zh: "Two-point calibration (zero + full span linear)".to_string(),
            };
        }

        // 多点评估
        let (uniformity, gap_regions) = assess_distribution(points);
        let outliers = detect_outliers(points);
        let (rmse, r_squared) = linear_fit_quality(points);

        // 推荐模型：如果线性拟合 R² > 0.99 用线性，否则用多项式
        let recommended_model = if r_squared >= 0.99 {
            CalibrationModel::Linear
        } else {
            CalibrationModel::Polynomial
        };

        let summary = format!(
            "{} calibration points, uniformity {:.0}%, outliers {}, R²={:.4}, recommended: {}",
            points.len(),
            uniformity * 100.0,
            outliers.len(),
            r_squared,
            recommended_model.label_zh(),
        );

        Self {
            uniformity,
            outliers,
            recommended_model,
            rmse,
            r_squared,
            gap_regions,
            summary_zh: summary,
        }
    }
}

/// 评估校准点分布均匀性
fn assess_distribution(points: &[(f64, f64)]) -> (f32, Vec<(f64, f64)>) {
    let n = points.len();
    if n < 2 {
        return (1.0, Vec::new());
    }

    // 按 raw 值排序
    let mut sorted: Vec<f64> = points.iter().map(|(r, _)| *r).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let range = sorted[n - 1] - sorted[0];
    if range < 1e-9 {
        return (0.0, Vec::new());
    }

    // 理想间隔
    let ideal_gap = range / (n - 1) as f64;
    let mut gap_regions = Vec::new();

    // 计算实际间隔与理想间隔的偏差
    let mut total_deviation = 0.0;
    for i in 1..n {
        let gap = sorted[i] - sorted[i - 1];
        let deviation = (gap - ideal_gap).abs() / ideal_gap;
        total_deviation += deviation;
        // 间隔超过理想间隔 2 倍，认为是间隙
        if gap > 2.0 * ideal_gap {
            gap_regions.push((sorted[i - 1], sorted[i]));
        }
    }

    let avg_deviation = total_deviation / (n - 1) as f64;
    let uniformity = (1.0 - avg_deviation.min(1.0)) as f32;

    (uniformity, gap_regions)
}

/// 检测离群点（基于线性拟合残差）
fn detect_outliers(points: &[(f64, f64)]) -> Vec<usize> {
    if points.len() < 4 {
        return Vec::new();
    }

    // 线性拟合
    let n = points.len() as f64;
    let sum_x: f64 = points.iter().map(|(x, _)| *x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
    let sum_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_xx: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < 1e-9 {
        return Vec::new();
    }

    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;

    // 计算残差
    let residuals: Vec<f64> = points
        .iter()
        .map(|(x, y)| y - (slope * x + intercept))
        .collect();

    // 残差的标准差
    let mean_residual = residuals.iter().sum::<f64>() / n;
    let std_residual = (residuals
        .iter()
        .map(|r| (r - mean_residual).powi(2))
        .sum::<f64>()
        / n)
        .sqrt();

    if std_residual < 1e-9 {
        return Vec::new();
    }

    // 残差超过 3 倍标准差的为离群点
    residuals
        .iter()
        .enumerate()
        .filter(|(_, &r)| (r - mean_residual).abs() > 3.0 * std_residual)
        .map(|(i, _)| i)
        .collect()
}

/// 线性拟合质量（RMSE 和 R²）
fn linear_fit_quality(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    if n < 2.0 {
        return (0.0, 1.0);
    }

    let sum_x: f64 = points.iter().map(|(x, _)| *x).sum();
    let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
    let sum_xy: f64 = points.iter().map(|(x, y)| x * y).sum();
    let sum_xx: f64 = points.iter().map(|(x, _)| x * x).sum();

    let denom = n * sum_xx - sum_x * sum_x;
    if denom.abs() < 1e-9 {
        return (0.0, 0.0);
    }

    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;

    // RMSE
    let sse: f64 = points
        .iter()
        .map(|(x, y)| {
            let pred = slope * x + intercept;
            (y - pred).powi(2)
        })
        .sum();
    let rmse = (sse / n).sqrt();

    // R²
    let mean_y = sum_y / n;
    let sst: f64 = points.iter().map(|(_, y)| (y - mean_y).powi(2)).sum();
    let r_squared = if sst < 1e-9 { 1.0 } else { 1.0 - sse / sst };

    (rmse, r_squared)
}

// ==================== 压力模式识别 ====================

/// 压力分布模式
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PressurePattern {
    /// 无接触
    NoContact,
    /// 单点按压
    SinglePoint,
    /// 多点按压
    MultiPoint,
    /// 面接触
    SurfaceContact,
    /// 抓握
    Grasp,
    /// 滑动
    Sliding,
    /// 均匀分布
    Uniform,
}

impl PressurePattern {
    pub fn label_zh(&self) -> &'static str {
        match self {
            PressurePattern::NoContact => "No contact",
            PressurePattern::SinglePoint => "Single-point press",
            PressurePattern::MultiPoint => "Multi-point press",
            PressurePattern::SurfaceContact => "Surface contact",
            PressurePattern::Grasp => "Grasp",
            PressurePattern::Sliding => "Sliding",
            PressurePattern::Uniform => "Uniform distribution",
        }
    }
}

/// 压力分布分析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PressureAnalysis {
    /// 识别的模式
    pub pattern: PressurePattern,
    /// 模式置信度
    pub confidence: f32,
    /// 压力中心 (x, y)，归一化到 0.0-1.0
    pub cop: (f32, f32),
    /// 总压力
    pub total_force: f64,
    /// 接触面积比例
    pub contact_ratio: f32,
    /// 有效通道数
    pub active_channels: usize,
    /// 摘要
    pub summary_zh: String,
}

impl PressureAnalysis {
    /// 分析压力分布
    ///
    /// `values` 为 rows×cols 的压力值矩阵（行优先）
    pub fn analyze(values: &[f64], rows: usize, cols: usize) -> Self {
        if values.is_empty() || rows == 0 || cols == 0 {
            return Self {
                pattern: PressurePattern::NoContact,
                confidence: 1.0,
                cop: (0.5, 0.5),
                total_force: 0.0,
                contact_ratio: 0.0,
                active_channels: 0,
                summary_zh: "No data".to_string(),
            };
        }

        let threshold = compute_adaptive_threshold(values);
        let active: Vec<bool> = values.iter().map(|&v| v > threshold).collect();
        let active_count = active.iter().filter(|&&a| a).count();
        let total_force: f64 = values.iter().sum();
        let contact_ratio = active_count as f32 / values.len() as f32;

        // 压力中心
        let (cop_x, cop_y) = compute_cop(values, &active, rows, cols);

        // 模式识别
        let (pattern, confidence) = classify_pattern(values, &active, rows, cols, active_count, contact_ratio, threshold);

        let summary = format!(
            "{} ({:.0}%), total force {:.1}, contact {:.0}%, CoP({:.2},{:.2})",
            pattern.label_zh(),
            confidence * 100.0,
            total_force,
            contact_ratio * 100.0,
            cop_x,
            cop_y,
        );

        Self {
            pattern,
            confidence,
            cop: (cop_x, cop_y),
            total_force,
            contact_ratio,
            active_channels: active_count,
            summary_zh: summary,
        }
    }
}

/// 自适应阈值：最大值的 10%
fn compute_adaptive_threshold(values: &[f64]) -> f64 {
    let max = values.iter().cloned().fold(0.0f64, f64::max);
    (max * 0.1).max(1.0)
}

/// 计算压力中心
fn compute_cop(values: &[f64], active: &[bool], rows: usize, cols: usize) -> (f32, f32) {
    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_w = 0.0f64;

    for (i, &v) in values.iter().enumerate() {
        if !active[i] || v <= 0.0 {
            continue;
        }
        let row = i / cols;
        let col = i % cols;
        sum_x += col as f64 * v;
        sum_y += row as f64 * v;
        sum_w += v;
    }

    if sum_w < 1e-9 {
        return (0.5, 0.5);
    }

    let cop_x = (sum_x / sum_w / (cols - 1).max(1) as f64) as f32;
    let cop_y = (sum_y / sum_w / (rows - 1).max(1) as f64) as f32;
    (cop_x.clamp(0.0, 1.0), cop_y.clamp(0.0, 1.0))
}

/// 分类压力模式
fn classify_pattern(
    values: &[f64],
    active: &[bool],
    rows: usize,
    cols: usize,
    active_count: usize,
    contact_ratio: f32,
    _threshold: f64,
) -> (PressurePattern, f32) {
    if active_count == 0 {
        return (PressurePattern::NoContact, 1.0);
    }

    // 计算活跃区域的连通性（简化：检查是否有多个独立的活跃群）
    let clusters = count_clusters(active, rows, cols);

    // 计算压力值的均匀性
    let active_values: Vec<f64> = values.iter().zip(active.iter()).filter(|(_, &a)| a).map(|(&v, _)| v).collect();
    let mean = active_values.iter().sum::<f64>() / active_values.len() as f64;
    let std = (active_values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / active_values.len() as f64).sqrt();
    let cv = if mean > 1e-9 { std / mean } else { 0.0 };

    // 判断逻辑
    if contact_ratio > 0.7 && cv < 0.2 {
        // 大面积接触且压力均匀
        return (PressurePattern::Uniform, 0.85);
    }

    if contact_ratio > 0.4 {
        // 大面积接触但压力不均匀
        return (PressurePattern::SurfaceContact, 0.7);
    }

    if clusters >= 3 {
        // 多个独立触点
        return (PressurePattern::MultiPoint, 0.75);
    }

    if clusters == 2 {
        // 两个触点：可能是抓握（拇趾+其他趾）或双点
        // 检查两个簇的距离
        return (PressurePattern::Grasp, 0.6);
    }

    if clusters == 1 {
        if active_count <= 3 {
            // 单点按压（1-3 个相邻单元）
            return (PressurePattern::SinglePoint, 0.8);
        }
        // 检查是否为滑动（活跃区域呈条带状）
        if let Some(aspect_ratio) = compute_aspect_ratio(active, rows, cols) {
            if aspect_ratio > 2.5 {
                return (PressurePattern::Sliding, 0.65);
            }
        }
        return (PressurePattern::SinglePoint, 0.5);
    }

    (PressurePattern::MultiPoint, 0.4)
}

/// 计算连通簇数量（4-连通）
fn count_clusters(active: &[bool], rows: usize, cols: usize) -> usize {
    let mut visited = vec![false; active.len()];
    let mut count = 0;

    for i in 0..active.len() {
        if active[i] && !visited[i] {
            count += 1;
            // BFS 标记连通区域
            let mut queue = vec![i];
            while let Some(idx) = queue.pop() {
                if visited[idx] {
                    continue;
                }
                visited[idx] = true;
                let row = idx / cols;
                let col = idx % cols;
                // 4-连通邻居
                if col > 0 {
                    let n = idx - 1;
                    if active[n] && !visited[n] {
                        queue.push(n);
                    }
                }
                if col + 1 < cols {
                    let n = idx + 1;
                    if active[n] && !visited[n] {
                        queue.push(n);
                    }
                }
                if row > 0 {
                    let n = idx - cols;
                    if active[n] && !visited[n] {
                        queue.push(n);
                    }
                }
                if row + 1 < rows {
                    let n = idx + cols;
                    if active[n] && !visited[n] {
                        queue.push(n);
                    }
                }
            }
        }
    }

    count
}

/// 计算活跃区域的长宽比
fn compute_aspect_ratio(active: &[bool], rows: usize, cols: usize) -> Option<f64> {
    let mut min_row = rows;
    let mut max_row = 0usize;
    let mut min_col = cols;
    let mut max_col = 0usize;
    let mut found = false;

    for (i, &a) in active.iter().enumerate() {
        if a {
            found = true;
            let row = i / cols;
            let col = i % cols;
            min_row = min_row.min(row);
            max_row = max_row.max(row);
            min_col = min_col.min(col);
            max_col = max_col.max(col);
        }
    }

    if !found {
        return None;
    }

    let height = (max_row - min_row + 1) as f64;
    let width = (max_col - min_col + 1) as f64;

    if height < 1.0 {
        None
    } else {
        Some(width / height)
    }
}
