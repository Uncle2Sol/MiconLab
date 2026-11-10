/// Baud rate auto-detection by trying common rates and scoring results.

use crate::config::{DataBits, Parity, StopBits};
use crate::port::PortResult;
use crate::SerialConfig;

/// Common baud rates to try during detection.
pub const COMMON_BAUD_RATES: &[u32] = &[
    300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600,
];

/// CH340 晶振分频补偿表 (标称波特率, 补偿波特率)
///
/// CH340 芯片在高波特率下可能因时钟分频误差导致实际波特率偏差。
/// macOS 驱动尤其明显，配置 921600 实际可能只有 ~725000 bps。
/// 设置为 2 倍波特率可让 CH340 降频至目标值。
pub const CH340_COMPENSATED_RATES: &[(u32, u32)] = &[
    (921600, 1843200),  // 921600*2，CH340 降频至 921600
    (921600, 1000000),  // 圆整补偿
    (460800, 921600),   // 460800*2
    (230400, 460800),   // 230400*2
    (115200, 230400),   // 115200*2
];

/// Result of a baud rate detection attempt.
#[derive(Debug, Clone)]
pub struct BaudDetectResult {
    pub baud_rate: u32,
    pub bytes_read: usize,
    pub confidence: f64,
}

/// Attempt to auto-detect the baud rate of a serial port.
///
/// Opens the port at each common baud rate, reads available bytes,
/// and scores each rate based on how many valid bytes were received.
pub fn detect_baud_rate(
    port_name: &str,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
) -> PortResult<Vec<BaudDetectResult>> {
    let mut results = Vec::new();

    for &baud in COMMON_BAUD_RATES {
        let config = SerialConfig::new(port_name)
            .with_baud_rate(baud)
            .with_data_bits(data_bits.clone())
            .with_parity(parity.clone())
            .with_stop_bits(stop_bits.clone())
            .with_timeout(200);

        match score_baud_rate(&config) {
            Ok(result) => results.push(result),
            Err(_) => {
                results.push(BaudDetectResult {
                    baud_rate: baud,
                    bytes_read: 0,
                    confidence: 0.0,
                });
            }
        }
    }

    // Sort by confidence descending (NaN 安全：使用 unwrap_or 避免 panic)
    results.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

/// 根据芯片提示自动检测波特率，优先测试 CH340 补偿表
///
/// `chip_hint` 为 "CH340" / "CP210x" / None，影响补偿策略。
pub fn detect_baud_with_chip(
    port_name: &str,
    chip_hint: Option<&str>,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
) -> PortResult<Vec<BaudDetectResult>> {
    let mut results = Vec::new();

    // 如果是 CH340，优先测试补偿表
    if chip_hint.map_or(false, |c| c.contains("CH340") || c.contains("CH341")) {
        for &(_nominal, compensated) in CH340_COMPENSATED_RATES {
            let config = SerialConfig::new(port_name)
                .with_baud_rate(compensated)
                .with_data_bits(data_bits.clone())
                .with_parity(parity.clone())
                .with_stop_bits(stop_bits.clone())
                .with_timeout(200);

            match score_baud_rate(&config) {
                Ok(result) => results.push(result),
                Err(_) => {
                    results.push(BaudDetectResult {
                        baud_rate: compensated,
                        bytes_read: 0,
                        confidence: 0.0,
                    });
                }
            }
        }
    }

    // 再测试标准波特率
    for &baud in COMMON_BAUD_RATES {
        // 跳过已测试的补偿波特率
        if results.iter().any(|r| r.baud_rate == baud) {
            continue;
        }
        let config = SerialConfig::new(port_name)
            .with_baud_rate(baud)
            .with_data_bits(data_bits.clone())
            .with_parity(parity.clone())
            .with_stop_bits(stop_bits.clone())
            .with_timeout(200);

        match score_baud_rate(&config) {
            Ok(result) => results.push(result),
            Err(_) => {
                results.push(BaudDetectResult {
                    baud_rate: baud,
                    bytes_read: 0,
                    confidence: 0.0,
                });
            }
        }
    }

    // 按置信度降序排序
    results.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

/// 测试 base/2, base, base*2 三档波特率（倍频补偿检测）
pub fn detect_baud_double(
    port_name: &str,
    base_baud: u32,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
) -> PortResult<Vec<BaudDetectResult>> {
    let mut results = Vec::new();
    let test_rates = [base_baud / 2, base_baud, base_baud * 2];

    for &baud in &test_rates {
        if baud == 0 {
            continue;
        }
        let config = SerialConfig::new(port_name)
            .with_baud_rate(baud)
            .with_data_bits(data_bits.clone())
            .with_parity(parity.clone())
            .with_stop_bits(stop_bits.clone())
            .with_timeout(200);

        match score_baud_rate(&config) {
            Ok(result) => results.push(result),
            Err(_) => {
                results.push(BaudDetectResult {
                    baud_rate: baud,
                    bytes_read: 0,
                    confidence: 0.0,
                });
            }
        }
    }

    results.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

/// 在指定波特率附近微调测试（±tolerance_pct%）
///
/// 用于 CH340 时钟偏差的精细补偿。
pub fn try_baud_fine(
    port_name: &str,
    baud: u32,
    tolerance_pct: f32,
    data_bits: DataBits,
    parity: Parity,
    stop_bits: StopBits,
) -> PortResult<Vec<BaudDetectResult>> {
    let mut results = Vec::new();
    let delta = (baud as f32 * tolerance_pct / 100.0) as u32;
    let step = (delta / 4).max(1);

    let mut test_rates = vec![baud];
    let mut offset = step;
    while offset <= delta {
        test_rates.push(baud.saturating_add(offset));
        test_rates.push(baud.saturating_sub(offset));
        offset += step;
    }

    for &test_baud in &test_rates {
        let config = SerialConfig::new(port_name)
            .with_baud_rate(test_baud)
            .with_data_bits(data_bits.clone())
            .with_parity(parity.clone())
            .with_stop_bits(stop_bits.clone())
            .with_timeout(200);

        match score_baud_rate(&config) {
            Ok(result) => results.push(result),
            Err(_) => {
                results.push(BaudDetectResult {
                    baud_rate: test_baud,
                    bytes_read: 0,
                    confidence: 0.0,
                });
            }
        }
    }

    results.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

/// Score a single baud rate by opening the port and reading data.
fn score_baud_rate(config: &SerialConfig) -> PortResult<BaudDetectResult> {
    let mut port = crate::SerialPort::new(config.clone());
    port.connect()?;

    let mut buf = [0u8; 1024];
    let bytes_read = match port.read(&mut buf) {
        Ok(n) => n,
        Err(_) => 0,
    };

    let _ = port.disconnect();

    let confidence = compute_confidence(&buf[..bytes_read]);

    Ok(BaudDetectResult {
        baud_rate: config.baud_rate,
        bytes_read,
        confidence,
    })
}

/// Compute a confidence score (0.0 - 1.0) for the received data.
///
/// Higher scores indicate data that looks like valid serial communication:
/// printable ASCII, common control characters (CR, LF, TAB),
/// and reasonable byte distribution.
fn compute_confidence(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }

    let total = data.len() as f64;
    let mut printable = 0u32;
    let mut control = 0u32;
    let mut null_count = 0u32;

    for &byte in data {
        match byte {
            0 => null_count += 1,
            0x01..=0x1F if byte == 0x0A || byte == 0x0D || byte == 0x09 => control += 1,
            0x20..=0x7E => printable += 1,
            _ => {}
        }
    }

    let printable_ratio = printable as f64 / total;
    let control_ratio = control as f64 / total;
    let null_ratio = null_count as f64 / total;

    // High printable ratio with some control chars is typical of text protocols
    let mut score = printable_ratio * 0.6 + control_ratio * 0.3;

    // Penalize too many nulls (likely garbage data from wrong baud rate)
    score -= null_ratio * 0.2;

    // Bonus for having some data at all
    if data.len() > 10 {
        score += 0.1;
    }

    score.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_baud_rates() {
        assert!(COMMON_BAUD_RATES.contains(&9600));
        assert!(COMMON_BAUD_RATES.contains(&115200));
        assert!(COMMON_BAUD_RATES.len() >= 10);
    }

    #[test]
    fn test_baud_detect_result_struct() {
        let result = BaudDetectResult {
            baud_rate: 115200,
            bytes_read: 50,
            confidence: 0.85,
        };
        assert_eq!(result.baud_rate, 115200);
        assert_eq!(result.bytes_read, 50);
        assert!((result.confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_confidence_empty() {
        assert_eq!(compute_confidence(&[]), 0.0);
    }

    #[test]
    fn test_compute_confidence_printable() {
        let data = b"Hello, World! This is a test message.\r\n";
        let score = compute_confidence(data);
        assert!(score > 0.5);
    }

    #[test]
    fn test_compute_confidence_nulls() {
        let data = [0u8; 100];
        let score = compute_confidence(&data);
        assert!(score < 0.1);
    }

    #[test]
    fn test_compute_confidence_mixed() {
        let data = b"\r\nOK\r\n> ";
        let score = compute_confidence(data);
        assert!(score > 0.3);
    }
}
