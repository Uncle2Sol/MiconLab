/// Serial oscilloscope for visualizing serial data as waveforms.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ScopeError {
    #[error("Scope error: {0}")]
    ScopeError(String),
}

pub type ScopeResult<T> = Result<T, ScopeError>;

/// A single sample point in the waveform.
#[derive(Debug, Clone)]
pub struct SamplePoint {
    /// Time offset in microseconds from the start of capture.
    pub time_us: f64,
    /// Logic level (true = high, false = low).
    pub level: bool,
}

/// Number of stop bits. 1.5 stop bits is valid only on UART receivers, never
/// transmitters — but the scope only decodes, so we support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    One,
    OnePointFive,
    Two,
}

impl Default for StopBits {
    fn default() -> Self {
        StopBits::One
    }
}

impl StopBits {
    /// Number of bit periods occupied by the stop bit(s).
    /// 1.5 stop bits is decoded at half-bit granularity via `has_half_stop`.
    pub fn bit_periods(&self) -> u32 {
        match self {
            StopBits::One => 1,
            StopBits::OnePointFive => 1, // integer part; half part handled separately
            StopBits::Two => 2,
        }
    }

    /// Whether the stop includes a half-bit period.
    pub fn has_half_bit(&self) -> bool {
        matches!(self, StopBits::OnePointFive)
    }
}

/// A captured byte with timing information.
#[derive(Debug, Clone)]
pub struct CapturedByte {
    /// The decoded byte value.
    pub value: u8,
    /// Start time in microseconds.
    pub start_us: f64,
    /// End time in microseconds.
    pub end_us: f64,
    /// Bit period used for decoding.
    pub bit_period_us: f64,
    /// Whether a framing error was detected on this byte (stop bit not high).
    pub framing_error: bool,
}

/// Configuration for the serial scope.
#[derive(Debug, Clone)]
pub struct ScopeConfig {
    /// Expected baud rate (used to calculate bit period).
    pub baud_rate: u32,
    /// Number of data bits (5-8).
    pub data_bits: u8,
    /// Number of stop bits. Replaces the previous `f32` field that truncated
    /// 1.5 stop bits to 1 via `as u32`.
    pub stop_bits: StopBits,
    /// Whether parity is enabled.
    pub parity: bool,
    /// Sample rate multiplier (1 = one sample per bit).
    pub sample_multiplier: u32,
}

impl Default for ScopeConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115200,
            data_bits: 8,
            stop_bits: StopBits::One,
            parity: false,
            sample_multiplier: 1,
        }
    }
}

/// Scope statistics for the captured data.
#[derive(Debug, Clone, Default)]
pub struct ScopeStats {
    /// Total number of bytes captured.
    pub total_bytes: usize,
    /// Average bit period in microseconds.
    pub avg_bit_period_us: f64,
    /// Minimum bit period in microseconds (estimated from byte durations).
    pub min_bit_period_us: f64,
    /// Maximum bit period in microseconds (estimated from byte durations).
    pub max_bit_period_us: f64,
    /// Number of framing errors detected (stop bit not high).
    pub framing_errors: usize,
    /// Total capture duration in microseconds.
    pub total_duration_us: f64,
}

/// Serial oscilloscope for analyzing serial data waveforms.
pub struct SerialScope {
    config: ScopeConfig,
    waveform: Vec<SamplePoint>,
    captured_bytes: Vec<CapturedByte>,
}

impl Default for SerialScope {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialScope {
    pub fn new() -> Self {
        Self {
            config: ScopeConfig::default(),
            waveform: Vec::new(),
            captured_bytes: Vec::new(),
        }
    }

    /// Create a scope with custom configuration.
    pub fn with_config(config: ScopeConfig) -> Self {
        Self {
            config,
            waveform: Vec::new(),
            captured_bytes: Vec::new(),
        }
    }

    /// Calculate the bit period in microseconds from the baud rate.
    pub fn bit_period_us(&self) -> f64 {
        1_000_000.0 / self.config.baud_rate as f64
    }

    /// Total number of bit periods per byte frame: start + data + parity + stop.
    fn bits_per_frame(&self) -> u32 {
        1 + self.config.data_bits as u32
            + if self.config.parity { 1 } else { 0 }
            + self.config.stop_bits.bit_periods()
    }

    /// Process raw serial data and convert to waveform sample points.
    ///
    /// Each byte is decoded into start bit + data bits + parity + stop bits.
    /// Framing errors (stop bit not high) are recorded per byte.
    pub fn process_raw_data(&mut self, data: &[u8], start_time_us: f64) {
        let bit_period = self.bit_period_us();
        let mut current_time = start_time_us;

        for &byte in data {
            let byte_start = current_time;
            let mut stop_level_seen = None;

            // Start bit (low)
            self.waveform.push(SamplePoint {
                time_us: current_time,
                level: false,
            });
            current_time += bit_period;

            // Data bits (LSB first)
            for i in 0..self.config.data_bits {
                let level = (byte >> i) & 1 == 1;
                self.waveform.push(SamplePoint {
                    time_us: current_time,
                    level,
                });
                current_time += bit_period;
            }

            // Parity bit (if enabled)
            if self.config.parity {
                let ones = byte.count_ones() as u8;
                // 奇校验：数据位 1 的个数为偶数时校验位补 1，使总 1 数为奇
                let parity_bit = ones % 2 == 0;
                self.waveform.push(SamplePoint {
                    time_us: current_time,
                    level: parity_bit,
                });
                current_time += bit_period;
            }

            // Stop bits (high) — 1.5 stop bits decoded as 1 full + 1 half
            for _ in 0..self.config.stop_bits.bit_periods() {
                self.waveform.push(SamplePoint {
                    time_us: current_time,
                    level: true,
                });
                stop_level_seen = Some(true);
                current_time += bit_period;
            }
            if self.config.stop_bits.has_half_bit() {
                // Half-bit stop — sample at half period; still expected high.
                self.waveform.push(SamplePoint {
                    time_us: current_time,
                    level: true,
                });
                stop_level_seen = Some(true);
                current_time += bit_period / 2.0;
            }

            let byte_end = current_time;
            // A framing error would manifest as the stop bit being low. Since
            // this scope synthesizes the waveform from already-decoded bytes,
            // we approximate: a byte whose last bit before stop is parity and
            // parity mismatches is not necessarily a framing error. We mark
            // framing_error=false here because the source bytes are already
            // decoded. Real framing-error detection requires sampling a live
            // RX line — the scope API exposes the field so future live mode
            // can populate it.
            let _ = stop_level_seen;

            self.captured_bytes.push(CapturedByte {
                value: byte,
                start_us: byte_start,
                end_us: byte_end,
                bit_period_us: bit_period,
                framing_error: false,
            });
        }
    }

    /// Mark a captured byte at the given index as having a framing error.
    /// Returns true if the index was in range.
    pub fn mark_framing_error(&mut self, index: usize) -> bool {
        if let Some(b) = self.captured_bytes.get_mut(index) {
            b.framing_error = true;
            true
        } else {
            false
        }
    }

    /// Convert the internal waveform to a simple list of (time, level) points.
    pub fn to_waveform_points(&self) -> Vec<(f64, bool)> {
        self.waveform
            .iter()
            .map(|p| (p.time_us, p.level))
            .collect()
    }

    /// Get statistics about the captured data.
    ///
    /// `min_bit_period_us` / `max_bit_period_us` are derived by dividing each
    /// byte frame duration by the number of bits per frame, giving a per-bit
    /// estimate. Previously this stored raw byte durations, mismatching the
    /// field name and `avg_bit_period_us`.
    pub fn stats(&self) -> ScopeStats {
        if self.captured_bytes.is_empty() {
            return ScopeStats::default();
        }

        let total_bytes = self.captured_bytes.len();
        let bits_per_frame = self.bits_per_frame() as f64;

        // Estimate per-bit periods from byte frame durations.
        let bit_periods: Vec<f64> = self
            .captured_bytes
            .iter()
            .map(|b| {
                let frame_duration = b.end_us - b.start_us;
                if bits_per_frame > 0.0 {
                    frame_duration / bits_per_frame
                } else {
                    b.bit_period_us
                }
            })
            .collect();

        let avg_bit_period = bit_periods.iter().sum::<f64>() / total_bytes as f64;
        let min_bit_period = bit_periods.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_bit_period = bit_periods.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        let total_duration_us = if let Some(last) = self.captured_bytes.last() {
            last.end_us
        } else {
            0.0
        };

        let framing_errors = self
            .captured_bytes
            .iter()
            .filter(|b| b.framing_error)
            .count();

        ScopeStats {
            total_bytes,
            avg_bit_period_us: avg_bit_period,
            min_bit_period_us: min_bit_period,
            max_bit_period_us: max_bit_period,
            framing_errors,
            total_duration_us,
        }
    }

    /// Clear all captured data.
    pub fn clear(&mut self) {
        self.waveform.clear();
        self.captured_bytes.clear();
    }

    /// Get the current configuration.
    pub fn config(&self) -> &ScopeConfig {
        &self.config
    }

    /// Get the number of captured bytes.
    pub fn byte_count(&self) -> usize {
        self.captured_bytes.len()
    }

    /// Get the number of waveform sample points.
    pub fn sample_count(&self) -> usize {
        self.waveform.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_config_default() {
        let config = ScopeConfig::default();
        assert_eq!(config.baud_rate, 115200);
        assert_eq!(config.data_bits, 8);
        assert_eq!(config.stop_bits, StopBits::One);
        assert!(!config.parity);
    }

    #[test]
    fn test_stop_bits_bit_periods() {
        assert_eq!(StopBits::One.bit_periods(), 1);
        assert_eq!(StopBits::OnePointFive.bit_periods(), 1);
        assert_eq!(StopBits::Two.bit_periods(), 2);
        assert!(StopBits::OnePointFive.has_half_bit());
        assert!(!StopBits::One.has_half_bit());
    }

    #[test]
    fn test_serial_scope_new() {
        let scope = SerialScope::new();
        assert_eq!(scope.byte_count(), 0);
        assert_eq!(scope.sample_count(), 0);
    }

    #[test]
    fn test_bit_period_calculation() {
        let scope = SerialScope::new();
        // 115200 baud => ~8.68 us per bit
        let period = scope.bit_period_us();
        assert!((period - 8.68).abs() < 0.1);

        let scope2 = SerialScope::with_config(ScopeConfig {
            baud_rate: 9600,
            ..Default::default()
        });
        let period2 = scope2.bit_period_us();
        assert!((period2 - 104.17).abs() < 0.1);
    }

    #[test]
    fn test_process_raw_data_0x55() {
        let mut scope = SerialScope::new();
        // 0x55 = 01010101 in binary, LSB first = 1,0,1,0,1,0,1,0
        scope.process_raw_data(&[0x55], 0.0);

        assert_eq!(scope.byte_count(), 1);
        assert!(scope.sample_count() > 0);

        let points = scope.to_waveform_points();
        // Start bit (low), then 8 data bits, then stop bit (high)
        assert!(!points[0].1); // Start bit is low
        assert!(points[9].1); // Stop bit is high (index 1 = start, 2-9 = data bits, 10 = stop)
    }

    #[test]
    fn test_process_raw_data_multi_byte() {
        let mut scope = SerialScope::new();
        scope.process_raw_data(&[0xAA, 0x55], 0.0);
        assert_eq!(scope.byte_count(), 2);
    }

    #[test]
    fn test_process_raw_data_with_parity() {
        let config = ScopeConfig {
            parity: true,
            ..Default::default()
        };
        let mut scope = SerialScope::with_config(config);
        scope.process_raw_data(&[0x55], 0.0);
        assert_eq!(scope.byte_count(), 1);
    }

    #[test]
    fn test_process_raw_data_two_stop_bits() {
        let config = ScopeConfig {
            stop_bits: StopBits::Two,
            ..Default::default()
        };
        let mut scope = SerialScope::with_config(config);
        scope.process_raw_data(&[0x00], 0.0);
        let points = scope.to_waveform_points();
        // start(1) + 8 data + 2 stop = 11 points
        assert_eq!(points.len(), 11);
        assert!(points[9].1); // first stop bit high
        assert!(points[10].1); // second stop bit high
    }

    #[test]
    fn test_process_raw_data_one_point_five_stop_bits() {
        // Regression: previously `stop_bits: 1.5f32` was truncated to 1 by
        // `as u32`. Now StopBits::OnePointFive emits 1.5 bit periods.
        let config = ScopeConfig {
            stop_bits: StopBits::OnePointFive,
            ..Default::default()
        };
        let mut scope = SerialScope::with_config(config);
        scope.process_raw_data(&[0x00], 0.0);
        let points = scope.to_waveform_points();
        // start(1) + 8 data + 1 full stop + 1 half stop = 11 points
        assert_eq!(points.len(), 11);
        // Half-stop point should be high
        assert!(points[10].1);
    }

    #[test]
    fn test_to_waveform_points() {
        let mut scope = SerialScope::new();
        scope.process_raw_data(&[0x00], 0.0); // All zeros, LSB first = 0,0,0,0,0,0,0,0
        let points = scope.to_waveform_points();
        // Start bit (low) + 8 data bits (all low) + stop bit (high) = 10 points
        assert_eq!(points.len(), 10);
        // All data bits should be low
        for i in 1..=8 {
            assert!(!points[i].1);
        }
        // Stop bit should be high
        assert!(points[9].1);
    }

    #[test]
    fn test_stats() {
        let mut scope = SerialScope::new();
        scope.process_raw_data(&[0x01, 0x02, 0x03], 0.0);

        let stats = scope.stats();
        assert_eq!(stats.total_bytes, 3);
        assert!(stats.avg_bit_period_us > 0.0);
        assert!(stats.total_duration_us > 0.0);
    }

    #[test]
    fn test_stats_bit_period_semantics() {
        // Regression: previously `min_bit_period_us` stored the byte frame
        // duration rather than a per-bit value, mismatching the field name.
        let mut scope = SerialScope::new();
        scope.process_raw_data(&[0x01, 0x02, 0x03], 0.0);

        let stats = scope.stats();
        // At 115200 baud the bit period is ~8.68us. Per-byte min/max should
        // be close to this value (not ~86.8us which would be the byte duration).
        assert!(stats.min_bit_period_us < 20.0, "min_bit_period_us={} should be per-bit", stats.min_bit_period_us);
        assert!(stats.max_bit_period_us < 20.0, "max_bit_period_us={} should be per-bit", stats.max_bit_period_us);
        assert!((stats.avg_bit_period_us - stats.min_bit_period_us).abs() < 0.01);
        assert!((stats.avg_bit_period_us - stats.max_bit_period_us).abs() < 0.01);
    }

    #[test]
    fn test_stats_empty() {
        let scope = SerialScope::new();
        let stats = scope.stats();
        assert_eq!(stats.total_bytes, 0);
    }

    #[test]
    fn test_framing_errors_counted() {
        let mut scope = SerialScope::new();
        scope.process_raw_data(&[0x01, 0x02], 0.0);
        scope.mark_framing_error(0);
        let stats = scope.stats();
        assert_eq!(stats.framing_errors, 1);
    }

    #[test]
    fn test_clear() {
        let mut scope = SerialScope::new();
        scope.process_raw_data(&[0x01], 0.0);
        assert_eq!(scope.byte_count(), 1);

        scope.clear();
        assert_eq!(scope.byte_count(), 0);
        assert_eq!(scope.sample_count(), 0);
    }

    #[test]
    fn test_config_access() {
        let config = ScopeConfig {
            baud_rate: 9600,
            data_bits: 7,
            stop_bits: StopBits::Two,
            parity: true,
            sample_multiplier: 4,
        };
        let scope = SerialScope::with_config(config);
        assert_eq!(scope.config().baud_rate, 9600);
        assert_eq!(scope.config().data_bits, 7);
        assert_eq!(scope.config().stop_bits, StopBits::Two);
    }
}
