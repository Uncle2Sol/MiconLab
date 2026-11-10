/// CAN bus analyzer for parsing and analyzing CAN frames.
///
/// Supports classic CAN (2.0A/2.0B) and CAN FD frames, SLCAN command
/// generation/parsing, and a bounded ring buffer for frame storage.

use std::collections::HashMap;
use std::time::Instant;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CanError {
    #[error("Invalid frame format: {0}")]
    InvalidFrame(String),
    #[error("Parse error: {0}")]
    ParseError(String),
}

pub type CanResult<T> = Result<T, CanError>;

/// Maximum data bytes in a CAN FD frame (DLC 15 → 64 bytes).
pub const FD_MAX_DATA_LEN: usize = 64;

/// Default maximum number of frames stored in the analyzer ring buffer.
pub const DEFAULT_MAX_FRAMES: usize = 10_000;

/// Map a CAN FD DLC code (0-15) to the actual data length in bytes.
///
/// DLC 0-8 map directly to 0-8 bytes. DLC 9-15 map to 12/16/20/24/32/48/64
/// bytes per the CAN FD specification.
pub fn fd_dlc_to_len(dlc: u8) -> usize {
    match dlc {
        0..=8 => dlc as usize,
        9 => 12,
        10 => 16,
        11 => 20,
        12 => 24,
        13 => 32,
        14 => 48,
        _ => 64, // 15
    }
}

/// Map a data length in bytes to the nearest valid CAN FD DLC code.
pub fn fd_len_to_dlc(len: usize) -> u8 {
    match len {
        0..=8 => len as u8,
        9..=12 => 9,
        13..=16 => 10,
        17..=20 => 11,
        21..=24 => 12,
        25..=32 => 13,
        33..=48 => 14,
        _ => 15,
    }
}

/// A CAN bus frame. Supports both classic CAN (up to 8 data bytes) and
/// CAN FD (up to 64 data bytes with BRS and ESI flags).
#[derive(Debug, Clone)]
pub struct CanFrame {
    pub id: u32,
    pub is_extended: bool,
    pub is_rtr: bool,
    /// Data Length Code (0-15). For classic CAN, equals data.len().
    /// For CAN FD, DLC 9-15 map to 12/16/20/24/32/48/64 bytes.
    pub dlc: u8,
    pub data: Vec<u8>,
    pub timestamp: Option<Instant>,
    pub channel: u8,
    /// True if this is a CAN FD frame.
    pub is_fd: bool,
    /// Bit Rate Switch (FD only): true = use higher bit rate for data phase.
    pub brs: bool,
    /// Error State Indicator (FD only): true = transmitting node is in
    /// error-passive state.
    pub esi: bool,
}

impl CanFrame {
    /// Create a new standard (11-bit) classic CAN frame.
    pub fn new_standard(id: u32, data: Vec<u8>) -> Self {
        let dlc = data.len().min(8) as u8;
        let mut truncated_data = data;
        truncated_data.truncate(8);

        Self {
            id: id & 0x7FF,
            is_extended: false,
            is_rtr: false,
            dlc,
            data: truncated_data,
            timestamp: Some(Instant::now()),
            channel: 0,
            is_fd: false,
            brs: false,
            esi: false,
        }
    }

    /// Create a new extended (29-bit) classic CAN frame.
    pub fn new_extended(id: u32, data: Vec<u8>) -> Self {
        let dlc = data.len().min(8) as u8;
        let mut truncated_data = data;
        truncated_data.truncate(8);

        Self {
            id: id & 0x1FFFFFFF,
            is_extended: true,
            is_rtr: false,
            dlc,
            data: truncated_data,
            timestamp: Some(Instant::now()),
            channel: 0,
            is_fd: false,
            brs: false,
            esi: false,
        }
    }

    /// Create a new RTR (Remote Transmission Request) frame.
    pub fn new_rtr(id: u32, is_extended: bool, dlc: u8) -> Self {
        Self {
            id: if is_extended { id & 0x1FFFFFFF } else { id & 0x7FF },
            is_extended,
            is_rtr: true,
            dlc: dlc.min(8),
            data: Vec::new(),
            timestamp: Some(Instant::now()),
            channel: 0,
            is_fd: false,
            brs: false,
            esi: false,
        }
    }

    /// Create a new CAN FD frame (standard or extended ID).
    ///
    /// Data is truncated to the maximum allowed by the DLC code. If `brs`
    /// is true, the frame uses a higher bit rate for the data phase.
    pub fn new_fd(id: u32, is_extended: bool, data: Vec<u8>, brs: bool) -> Self {
        let max_len = FD_MAX_DATA_LEN;
        let mut data = data;
        data.truncate(max_len);
        let dlc = fd_len_to_dlc(data.len());

        Self {
            id: if is_extended { id & 0x1FFFFFFF } else { id & 0x7FF },
            is_extended,
            is_rtr: false,
            dlc,
            data,
            timestamp: Some(Instant::now()),
            channel: 0,
            is_fd: true,
            brs,
            esi: false,
        }
    }

    /// Get the actual data length in bytes (may differ from DLC in FD mode).
    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    /// Get the data bytes as a hex string.
    pub fn data_hex(&self) -> String {
        crate::hex::format_hex_spaced(&self.data)
    }

    /// Get the ID as a formatted string.
    pub fn id_str(&self) -> String {
        if self.is_extended {
            format!("0x{:08X}", self.id)
        } else {
            format!("0x{:03X}", self.id)
        }
    }

    /// Serialize this frame to an SLCAN command string (without trailing CR).
    ///
    /// - Classic standard data: `t<id3><dlc1><data>`
    /// - Classic extended data: `T<id8><dlc1><data>`
    /// - Classic standard RTR: `r<id3><dlc1>`
    /// - Classic extended RTR: `R<id8><dlc1>`
    /// - FD standard no BRS: `d<id3><dlc1><data>`
    /// - FD extended no BRS: `D<id8><dlc1><data>`
    /// - FD standard BRS: `b<id3><dlc1><data>`
    /// - FD extended BRS: `B<id8><dlc1><data>`
    pub fn to_slcan(&self) -> String {
        let id_str = if self.is_extended {
            format!("{:08X}", self.id)
        } else {
            format!("{:03X}", self.id)
        };

        let data_hex: String = crate::hex::format_hex_upper(&self.data);

        if self.is_fd {
            let prefix = match (self.is_extended, self.brs) {
                (false, false) => 'd',
                (true, false) => 'D',
                (false, true) => 'b',
                (true, true) => 'B',
            };
            if self.is_rtr {
                // FD RTR — reuse same prefix, no data
                format!("{}{}{:X}", prefix, id_str, self.dlc)
            } else {
                format!("{}{}{:X}{}", prefix, id_str, self.dlc, data_hex)
            }
        } else {
            if self.is_rtr {
                let prefix = if self.is_extended { 'R' } else { 'r' };
                format!("{}{}{:X}", prefix, id_str, self.dlc)
            } else {
                let prefix = if self.is_extended { 'T' } else { 't' };
                format!("{}{}{:X}{}", prefix, id_str, self.dlc, data_hex)
            }
        }
    }
}

/// Filter for accepting/rejecting CAN frames.
#[derive(Debug, Clone)]
pub struct CanFilter {
    pub min_id: u32,
    pub max_id: u32,
    pub extended_only: bool,
    pub standard_only: bool,
    pub exclude_rtr: bool,
    pub fd_only: bool,
    pub classic_only: bool,
}

impl Default for CanFilter {
    fn default() -> Self {
        Self {
            min_id: 0,
            max_id: 0x1FFFFFFF,
            extended_only: false,
            standard_only: false,
            exclude_rtr: false,
            fd_only: false,
            classic_only: false,
        }
    }
}

impl CanFilter {
    /// Check if a frame passes this filter.
    pub fn accept(&self, frame: &CanFrame) -> bool {
        if frame.id < self.min_id || frame.id > self.max_id {
            return false;
        }

        if self.extended_only && !frame.is_extended {
            return false;
        }

        if self.standard_only && frame.is_extended {
            return false;
        }

        if self.exclude_rtr && frame.is_rtr {
            return false;
        }

        if self.fd_only && !frame.is_fd {
            return false;
        }

        if self.classic_only && frame.is_fd {
            return false;
        }

        true
    }
}

/// Statistics for a specific CAN ID.
#[derive(Debug, Clone)]
pub struct CanIdStats {
    pub count: usize,
    pub last_data: Vec<u8>,
    pub last_seen: Option<Instant>,
    pub avg_interval_ms: f64,
}

impl Default for CanIdStats {
    fn default() -> Self {
        Self {
            count: 0,
            last_data: Vec::new(),
            last_seen: None,
            avg_interval_ms: 0.0,
        }
    }
}

/// CAN bus analyzer that collects frames and maintains statistics.
/// Uses a bounded ring buffer to prevent unbounded memory growth.
pub struct CanAnalyzer {
    frames: Vec<CanFrame>,
    stats: HashMap<u32, CanIdStats>,
    filter: CanFilter,
    last_timestamps: HashMap<u32, Instant>,
    max_frames: usize,
}

impl Default for CanAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl CanAnalyzer {
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            stats: HashMap::new(),
            filter: CanFilter::default(),
            last_timestamps: HashMap::new(),
            max_frames: DEFAULT_MAX_FRAMES,
        }
    }

    /// Create an analyzer with a custom ring buffer capacity.
    pub fn with_capacity(max_frames: usize) -> Self {
        let mut analyzer = Self::new();
        analyzer.max_frames = max_frames;
        analyzer
    }

    /// Set the maximum number of frames to retain. Older frames are discarded
    /// when the limit is exceeded.
    pub fn set_max_frames(&mut self, max: usize) {
        self.max_frames = max;
        self.evict_excess();
    }

    /// Get the current ring buffer capacity.
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Remove oldest frames until within the capacity limit.
    fn evict_excess(&mut self) {
        while self.frames.len() > self.max_frames {
            self.frames.remove(0);
        }
    }

    /// Set the filter for this analyzer.
    pub fn set_filter(&mut self, filter: CanFilter) {
        self.filter = filter;
    }

    /// Parse an SLCAN-format line (e.g., "t1232AABB\r").
    ///
    /// Supports classic CAN (t/T/r/R) and CAN FD (d/D/b/B) frames.
    /// Returns the parsed frame or an error. This does NOT add the frame
    /// to the analyzer — call [`add_frame`](Self::add_frame) separately.
    pub fn parse_slcan(&self, line: &str) -> CanResult<CanFrame> {
        let line = line.trim();

        if line.is_empty() {
            return Err(CanError::ParseError("Empty line".to_string()));
        }

        // SLCAN 是 ASCII 协议；含多字节 UTF-8 字符的垃圾数据会导致后续按字节位置切片 panic
        if !line.is_ascii() {
            return Err(CanError::ParseError("Non-ASCII SLCAN line".to_string()));
        }

        let first_char = line.chars().next().ok_or_else(|| {
            CanError::ParseError("Empty line".to_string())
        })?;

        match first_char {
            // ---- Classic CAN data frames ----
            't' => self.parse_slcan_data(line, false, false),
            'T' => self.parse_slcan_data(line, true, false),
            // ---- Classic CAN RTR frames ----
            'r' => self.parse_slcan_rtr(line, false),
            'R' => self.parse_slcan_rtr(line, true),
            // ---- CAN FD data frames ----
            'd' => self.parse_slcan_fd_data(line, false, false),
            'D' => self.parse_slcan_fd_data(line, true, false),
            'b' => self.parse_slcan_fd_data(line, false, true),
            'B' => self.parse_slcan_fd_data(line, true, true),
            _ => Err(CanError::ParseError(format!(
                "Unknown frame type: {}",
                first_char
            ))),
        }
    }

    /// Parse a classic CAN data frame: `t<id3><dlc1><data>` or `T<id8><dlc1><data>`.
    fn parse_slcan_data(&self, line: &str, is_extended: bool, _is_fd: bool) -> CanResult<CanFrame> {
        let id_len = if is_extended { 8 } else { 3 };
        let min_len = 1 + id_len + 1; // prefix + id + dlc

        if line.len() < min_len {
            return Err(CanError::ParseError(
                "Data frame too short".to_string(),
            ));
        }

        let id = u32::from_str_radix(&line[1..1 + id_len], 16)
            .map_err(|e| CanError::ParseError(format!("Invalid ID: {}", e)))?;

        let dlc = line[1 + id_len..2 + id_len]
            .parse::<usize>()
            .map_err(|e| CanError::ParseError(format!("Invalid DLC: {}", e)))?;

        let data_str = &line[2 + id_len..];
        let data = parse_hex_data(data_str)?;
        let mut data = data;
        data.truncate(dlc.min(8));

        if is_extended {
            Ok(CanFrame::new_extended(id, data))
        } else {
            Ok(CanFrame::new_standard(id, data))
        }
    }

    /// Parse a classic CAN RTR frame: `r<id3><dlc1>` or `R<id8><dlc1>`.
    fn parse_slcan_rtr(&self, line: &str, is_extended: bool) -> CanResult<CanFrame> {
        let id_len = if is_extended { 8 } else { 3 };
        let min_len = 1 + id_len + 1;

        if line.len() < min_len {
            return Err(CanError::ParseError(
                "RTR frame too short".to_string(),
            ));
        }

        let id = u32::from_str_radix(&line[1..1 + id_len], 16)
            .map_err(|e| CanError::ParseError(format!("Invalid ID: {}", e)))?;

        let dlc = line[1 + id_len..2 + id_len]
            .parse::<u8>()
            .map_err(|e| CanError::ParseError(format!("Invalid DLC: {}", e)))?;

        Ok(CanFrame::new_rtr(id, is_extended, dlc))
    }

    /// Parse a CAN FD data frame: `d/D/b/B<id><dlc1><data>`.
    fn parse_slcan_fd_data(
        &self,
        line: &str,
        is_extended: bool,
        brs: bool,
    ) -> CanResult<CanFrame> {
        let id_len = if is_extended { 8 } else { 3 };
        let min_len = 1 + id_len + 1;

        if line.len() < min_len {
            return Err(CanError::ParseError(
                "FD frame too short".to_string(),
            ));
        }

        let id = u32::from_str_radix(&line[1..1 + id_len], 16)
            .map_err(|e| CanError::ParseError(format!("Invalid ID: {}", e)))?;

        let dlc_byte = line.as_bytes()[1 + id_len];
        let dlc = (dlc_byte as char).to_digit(16)
            .map(|d| d as u8)
            .ok_or_else(|| CanError::ParseError("Invalid FD DLC".into()))?;

        let data_str = &line[2 + id_len..];
        let data = parse_hex_data(data_str)?;
        // Truncate to the FD data length for this DLC
        let max_len = fd_dlc_to_len(dlc);
        let mut data = data;
        data.truncate(max_len);

        let mut frame = CanFrame::new_fd(id, is_extended, data, brs);
        frame.dlc = dlc;
        Ok(frame)
    }

    /// Parse a raw CAN frame from bytes (CAN ID + data).
    /// Format: [id_high, id_low, dlc, data...]
    pub fn parse_raw(&self, data: &[u8]) -> CanResult<CanFrame> {
        if data.len() < 3 {
            return Err(CanError::ParseError(
                "Raw frame too short".to_string(),
            ));
        }

        let id = ((data[0] as u32) << 8) | (data[1] as u32);
        let dlc = data[2].min(8);
        let frame_data = if data.len() > 3 {
            data[3..].to_vec()
        } else {
            Vec::new()
        };

        let mut frame = CanFrame::new_standard(id, frame_data);
        frame.dlc = dlc;
        Ok(frame)
    }

    /// Add a CAN frame to the analyzer and update statistics.
    pub fn add_frame(&mut self, frame: CanFrame) {
        // Update stats
        let stats = self.stats.entry(frame.id).or_default();
        stats.count += 1;
        stats.last_data = frame.data.clone();
        stats.last_seen = frame.timestamp;

        // Calculate average interval
        if let Some(last_ts) = self.last_timestamps.get(&frame.id) {
            if let Some(current_ts) = frame.timestamp {
                let interval = current_ts.duration_since(*last_ts).as_secs_f64() * 1000.0;
                if stats.count > 1 {
                    stats.avg_interval_ms =
                        (stats.avg_interval_ms * (stats.count as f64 - 1.0) + interval)
                            / stats.count as f64;
                } else {
                    stats.avg_interval_ms = interval;
                }
            }
        }

        if let Some(ts) = frame.timestamp {
            self.last_timestamps.insert(frame.id, ts);
        }

        self.frames.push(frame);
        self.evict_excess();
    }

    /// Get frames that pass the current filter.
    pub fn filtered_frames(&self) -> Vec<&CanFrame> {
        self.frames
            .iter()
            .filter(|f| self.filter.accept(f))
            .collect()
    }

    /// Get statistics for a specific CAN ID.
    pub fn get_stats(&self, id: u32) -> Option<&CanIdStats> {
        self.stats.get(&id)
    }

    /// Get all ID statistics.
    pub fn all_stats(&self) -> &HashMap<u32, CanIdStats> {
        &self.stats
    }

    /// Format a CAN frame for display.
    pub fn format_frame(frame: &CanFrame) -> String {
        let rtr = if frame.is_rtr { " RTR" } else { "" };
        let ext = if frame.is_extended { " EXT" } else { "" };
        let fd = if frame.is_fd {
            let brs = if frame.brs { " BRS" } else { "" };
            let esi = if frame.esi { " ESI" } else { "" };
            format!(" FD{}{}", brs, esi)
        } else {
            String::new()
        };

        format!(
            "CH{} {:>5}{}{}{} [{}] {}",
            frame.channel,
            frame.id_str(),
            rtr,
            ext,
            fd,
            frame.dlc,
            frame.data_hex(),
        )
    }

    /// Clear all stored frames and statistics.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.stats.clear();
        self.last_timestamps.clear();
    }

    /// Get total number of stored frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Get unique CAN IDs seen.
    pub fn unique_ids(&self) -> Vec<u32> {
        self.stats.keys().copied().collect()
    }
}

/// Parse a hex data string (e.g. "AABBCC") into bytes.
///
/// 委托到 [`crate::hex::parse_hex`] 统一实现。修复点：
///   - 移除冗余循环条件 `i + 1 < bytes.len() + 1 && i + 1 <= bytes.len()`
///   - 奇数长度不再静默截断（原行为会丢失最后一个奇字节，导致 DLC 与数据不匹配时无法察觉）
fn parse_hex_data(s: &str) -> CanResult<Vec<u8>> {
    crate::hex::parse_hex(s)
        .map_err(|e| CanError::ParseError(format!("Invalid hex data: {}", e)))
}

// ---- SLCAN command builders ----
//
// These functions build SLCAN commands (strings) to send TO a CAN adapter.
// Each returns a string without the trailing CR; the caller appends `\r`.

/// Predefined CAN bit rates for the SLCAN `S` command.
pub fn slcan_bit_rate_command(speed_index: u8) -> CanResult<String> {
    match speed_index {
        0 => Ok("S0".into()), // 10 kbps
        1 => Ok("S1".into()), // 20 kbps
        2 => Ok("S2".into()), // 50 kbps
        3 => Ok("S3".into()), // 100 kbps
        4 => Ok("S4".into()), // 125 kbps
        5 => Ok("S5".into()), // 250 kbps
        6 => Ok("S6".into()), // 500 kbps
        7 => Ok("S7".into()), // 800 kbps
        8 => Ok("S8".into()), // 1 Mbps
        _ => Err(CanError::InvalidFrame(format!(
            "Invalid speed index {} (must be 0-8)",
            speed_index
        ))),
    }
}

/// Get the human-readable bit rate for a speed index.
pub fn slcan_bit_rate_label(speed_index: u8) -> Option<&'static str> {
    match speed_index {
        0 => Some("10 kbps"),
        1 => Some("20 kbps"),
        2 => Some("50 kbps"),
        3 => Some("100 kbps"),
        4 => Some("125 kbps"),
        5 => Some("250 kbps"),
        6 => Some("500 kbps"),
        7 => Some("800 kbps"),
        8 => Some("1 Mbps"),
        _ => None,
    }
}

/// Predefined UART baud rates for the SLCAN `U` command.
pub fn slcan_uart_baud_command(uart_index: u8) -> CanResult<String> {
    match uart_index {
        0 => Ok("U0".into()), // 230400
        1 => Ok("U1".into()), // 115200
        2 => Ok("U2".into()), // 57600
        3 => Ok("U3".into()), // 38400
        4 => Ok("U4".into()), // 19200
        5 => Ok("U5".into()), // 9600
        6 => Ok("U6".into()), // 4800
        7 => Ok("U7".into()), // 2400
        8 => Ok("U8".into()), // 1200
        9 => Ok("U9".into()), // 600
        _ => Err(CanError::InvalidFrame(format!(
            "Invalid UART index {} (must be 0-9)",
            uart_index
        ))),
    }
}

/// Get the UART baud rate for a UART index.
pub fn slcan_uart_baud_label(uart_index: u8) -> Option<u32> {
    match uart_index {
        0 => Some(230_400),
        1 => Some(115_200),
        2 => Some(57_600),
        3 => Some(38_400),
        4 => Some(19_200),
        5 => Some(9_600),
        6 => Some(4_800),
        7 => Some(2_400),
        8 => Some(1_200),
        9 => Some(600),
        _ => None,
    }
}

/// Build a set of common SLCAN setup commands for opening a CAN channel.
///
/// Returns the commands in order:
/// 1. `C` — close (ensure clean state)
/// 2. `S<n>` — set bit rate
/// 3. `O` — open
pub fn slcan_open(speed_index: u8) -> CanResult<Vec<String>> {
    let s_cmd = slcan_bit_rate_command(speed_index)?;
    Ok(vec!["C".into(), s_cmd, "O".into()])
}

/// SLCAN close command.
pub fn slcan_close() -> String {
    "C".into()
}

/// SLCAN open command.
pub fn slcan_open_channel() -> String {
    "O".into()
}

/// SLCAN set acceptance filter: `M<mask>` (11-bit mask, hex).
pub fn slcan_set_filter_mask(mask: u16) -> String {
    format!("M{:03X}", mask & 0x7FF)
}

/// SLCAN set acceptance code: `m<code>` (11-bit code, hex).
pub fn slcan_set_filter_code(code: u16) -> String {
    format!("m{:03X}", code & 0x7FF)
}

/// SLCAN get version: `V`.
pub fn slcan_get_version() -> String {
    "V".into()
}

/// SLCAN get serial number: `N`.
pub fn slcan_get_serial() -> String {
    "N".into()
}

/// SLCAN get status flags: `F`.
pub fn slcan_get_status() -> String {
    "F".into()
}

/// SLCAN enable/disable timestamp: `Z0` or `Z1`.
pub fn slcan_set_timestamp(enable: bool) -> String {
    if enable {
        "Z1".into()
    } else {
        "Z0".into()
    }
}

/// SLCAN auto-initialization: `i`.
pub fn slcan_auto_init() -> String {
    "i".into()
}

/// SLCAN set custom bit timing register: `sxxxx` (4 hex digits).
pub fn slcan_set_custom_btr(btr_value: u16) -> String {
    format!("s{:04X}", btr_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_frame_standard() {
        let frame = CanFrame::new_standard(0x123, vec![0x01, 0x02, 0x03]);
        assert_eq!(frame.id, 0x123);
        assert!(!frame.is_extended);
        assert!(!frame.is_rtr);
        assert_eq!(frame.dlc, 3);
        assert_eq!(frame.data, vec![0x01, 0x02, 0x03]);
        assert!(!frame.is_fd);
    }

    #[test]
    fn test_can_frame_extended() {
        let frame = CanFrame::new_extended(0x12345678, vec![0xAA, 0xBB]);
        assert!(frame.is_extended);
        assert_eq!(frame.id, 0x12345678);
        assert_eq!(frame.dlc, 2);
    }

    #[test]
    fn test_can_frame_truncate() {
        let frame = CanFrame::new_standard(0x100, vec![0; 10]);
        assert_eq!(frame.dlc, 8);
        assert_eq!(frame.data.len(), 8);
    }

    #[test]
    fn test_can_frame_rtr() {
        let frame = CanFrame::new_rtr(0x200, false, 4);
        assert!(frame.is_rtr);
        assert_eq!(frame.dlc, 4);
        assert!(frame.data.is_empty());
    }

    #[test]
    fn test_can_frame_hex() {
        let frame = CanFrame::new_standard(0x100, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(frame.data_hex(), "DE AD BE EF");
    }

    #[test]
    fn test_can_frame_id_str() {
        let std = CanFrame::new_standard(0x123, vec![0]);
        assert_eq!(std.id_str(), "0x123");

        let ext = CanFrame::new_extended(0x12345678, vec![0]);
        assert_eq!(ext.id_str(), "0x12345678");
    }

    #[test]
    fn test_can_fd_frame() {
        let data = vec![0xAA; 64];
        let frame = CanFrame::new_fd(0x100, false, data.clone(), true);
        assert!(frame.is_fd);
        assert!(frame.brs);
        assert_eq!(frame.data_len(), 64);
        assert_eq!(frame.dlc, 15); // 64 bytes → DLC 15
    }

    #[test]
    fn test_can_fd_frame_truncate() {
        let data = vec![0xFF; 100];
        let frame = CanFrame::new_fd(0x100, false, data, false);
        assert_eq!(frame.data_len(), 64); // truncated to FD max
    }

    #[test]
    fn test_fd_dlc_to_len() {
        assert_eq!(fd_dlc_to_len(0), 0);
        assert_eq!(fd_dlc_to_len(8), 8);
        assert_eq!(fd_dlc_to_len(9), 12);
        assert_eq!(fd_dlc_to_len(12), 24);
        assert_eq!(fd_dlc_to_len(15), 64);
    }

    #[test]
    fn test_fd_len_to_dlc() {
        assert_eq!(fd_len_to_dlc(0), 0);
        assert_eq!(fd_len_to_dlc(8), 8);
        assert_eq!(fd_len_to_dlc(12), 9);
        assert_eq!(fd_len_to_dlc(16), 10);
        assert_eq!(fd_len_to_dlc(64), 15);
    }

    #[test]
    fn test_can_filter_default() {
        let filter = CanFilter::default();
        let frame = CanFrame::new_standard(0x100, vec![1]);
        assert!(filter.accept(&frame));
    }

    #[test]
    fn test_can_filter_id_range() {
        let filter = CanFilter {
            min_id: 0x100,
            max_id: 0x200,
            ..Default::default()
        };

        let frame_low = CanFrame::new_standard(0x050, vec![1]);
        let frame_in = CanFrame::new_standard(0x150, vec![1]);
        let frame_high = CanFrame::new_standard(0x300, vec![1]);

        assert!(!filter.accept(&frame_low));
        assert!(filter.accept(&frame_in));
        assert!(!filter.accept(&frame_high));
    }

    #[test]
    fn test_can_filter_extended_only() {
        let filter = CanFilter {
            extended_only: true,
            ..Default::default()
        };

        let std = CanFrame::new_standard(0x100, vec![1]);
        let ext = CanFrame::new_extended(0x100, vec![1]);
        assert!(!filter.accept(&std));
        assert!(filter.accept(&ext));
    }

    #[test]
    fn test_can_filter_standard_only() {
        let filter = CanFilter {
            standard_only: true,
            ..Default::default()
        };

        let std = CanFrame::new_standard(0x100, vec![1]);
        let ext = CanFrame::new_extended(0x100, vec![1]);
        assert!(filter.accept(&std));
        assert!(!filter.accept(&ext));
    }

    #[test]
    fn test_can_filter_exclude_rtr() {
        let filter = CanFilter {
            exclude_rtr: true,
            ..Default::default()
        };

        let data = CanFrame::new_standard(0x100, vec![1]);
        let rtr = CanFrame::new_rtr(0x100, false, 4);
        assert!(filter.accept(&data));
        assert!(!filter.accept(&rtr));
    }

    #[test]
    fn test_can_filter_fd_only() {
        let filter = CanFilter {
            fd_only: true,
            ..Default::default()
        };

        let classic = CanFrame::new_standard(0x100, vec![1]);
        let fd = CanFrame::new_fd(0x100, false, vec![1], false);
        assert!(!filter.accept(&classic));
        assert!(filter.accept(&fd));
    }

    #[test]
    fn test_can_filter_classic_only() {
        let filter = CanFilter {
            classic_only: true,
            ..Default::default()
        };

        let classic = CanFrame::new_standard(0x100, vec![1]);
        let fd = CanFrame::new_fd(0x100, false, vec![1], false);
        assert!(filter.accept(&classic));
        assert!(!filter.accept(&fd));
    }

    // ---- to_slcan serialization tests ----

    #[test]
    fn test_to_slcan_standard() {
        let frame = CanFrame::new_standard(0x123, vec![0xAA, 0xBB]);
        let s = frame.to_slcan();
        assert_eq!(s, "t1232AABB");
    }

    #[test]
    fn test_to_slcan_extended() {
        let frame = CanFrame::new_extended(0x12345678, vec![0x01, 0x02]);
        let s = frame.to_slcan();
        assert_eq!(s, "T1234567820102");
    }

    #[test]
    fn test_to_slcan_rtr_standard() {
        let frame = CanFrame::new_rtr(0x123, false, 4);
        let s = frame.to_slcan();
        assert_eq!(s, "r1234");
    }

    #[test]
    fn test_to_slcan_rtr_extended() {
        let frame = CanFrame::new_rtr(0x12345678, true, 8);
        let s = frame.to_slcan();
        assert_eq!(s, "R123456788");
    }

    #[test]
    fn test_to_slcan_fd_standard_no_brs() {
        let frame = CanFrame::new_fd(0x123, false, vec![0xAA, 0xBB], false);
        let s = frame.to_slcan();
        assert_eq!(s, "d1232AABB");
    }

    #[test]
    fn test_to_slcan_fd_extended_no_brs() {
        let frame = CanFrame::new_fd(0x12345678, true, vec![0x01], false);
        let s = frame.to_slcan();
        assert_eq!(s, "D12345678101");
    }

    #[test]
    fn test_to_slcan_fd_standard_brs() {
        let frame = CanFrame::new_fd(0x123, false, vec![0xAA], true);
        let s = frame.to_slcan();
        assert_eq!(s, "b1231AA");
    }

    #[test]
    fn test_to_slcan_fd_extended_brs() {
        let frame = CanFrame::new_fd(0x12345678, true, vec![0x01, 0x02], true);
        let s = frame.to_slcan();
        assert_eq!(s, "B1234567820102");
    }

    // ---- parse_slcan tests ----

    #[test]
    fn test_parse_slcan_standard() {
        let analyzer = CanAnalyzer::new();
        let frame = analyzer.parse_slcan("t1232AABB").unwrap();
        assert!(!frame.is_extended);
        assert!(!frame.is_rtr);
        assert!(!frame.is_fd);
        assert_eq!(frame.id, 0x123);
        assert_eq!(frame.dlc, 2);
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_parse_slcan_extended() {
        let analyzer = CanAnalyzer::new();
        let frame = analyzer.parse_slcan("T123456782AABB").unwrap();
        assert!(frame.is_extended);
        assert_eq!(frame.id, 0x12345678);
        assert_eq!(frame.dlc, 2);
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_parse_slcan_rtr_standard() {
        let analyzer = CanAnalyzer::new();
        let frame = analyzer.parse_slcan("r1234").unwrap();
        assert!(!frame.is_extended);
        assert!(frame.is_rtr);
        assert_eq!(frame.id, 0x123);
        assert_eq!(frame.dlc, 4);
    }

    #[test]
    fn test_parse_slcan_rtr_extended() {
        let analyzer = CanAnalyzer::new();
        let frame = analyzer.parse_slcan("R123456788").unwrap();
        assert!(frame.is_extended);
        assert!(frame.is_rtr);
        assert_eq!(frame.id, 0x12345678);
        assert_eq!(frame.dlc, 8);
    }

    #[test]
    fn test_parse_slcan_fd_standard() {
        let analyzer = CanAnalyzer::new();
        let frame = analyzer.parse_slcan("d1232AABB").unwrap();
        assert!(frame.is_fd);
        assert!(!frame.brs);
        assert!(!frame.is_extended);
        assert_eq!(frame.id, 0x123);
        assert_eq!(frame.dlc, 2);
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_parse_slcan_fd_extended_brs() {
        let analyzer = CanAnalyzer::new();
        let frame = analyzer.parse_slcan("B1234567820102").unwrap();
        assert!(frame.is_fd);
        assert!(frame.brs);
        assert!(frame.is_extended);
        assert_eq!(frame.id, 0x12345678);
        assert_eq!(frame.dlc, 2);
        assert_eq!(frame.data, vec![0x01, 0x02]);
    }

    #[test]
    fn test_parse_slcan_fd_high_dlc() {
        // DLC F (15) → up to 64 bytes
        let analyzer = CanAnalyzer::new();
        let data_hex: String = "AB".repeat(64);
        let line = format!("d123F{}", data_hex);
        let frame = analyzer.parse_slcan(&line).unwrap();
        assert!(frame.is_fd);
        assert_eq!(frame.dlc, 15);
        assert_eq!(frame.data_len(), 64);
    }

    #[test]
    fn test_parse_slcan_unknown() {
        let analyzer = CanAnalyzer::new();
        assert!(analyzer.parse_slcan("x123").is_err());
    }

    #[test]
    fn test_parse_slcan_empty() {
        let analyzer = CanAnalyzer::new();
        assert!(analyzer.parse_slcan("").is_err());
    }

    #[test]
    fn test_parse_slcan_too_short() {
        let analyzer = CanAnalyzer::new();
        assert!(analyzer.parse_slcan("t1").is_err());
    }

    // ---- SLCAN command builder tests ----

    #[test]
    fn test_slcan_bit_rate_command() {
        assert_eq!(slcan_bit_rate_command(0).unwrap(), "S0");
        assert_eq!(slcan_bit_rate_command(6).unwrap(), "S6");
        assert_eq!(slcan_bit_rate_command(8).unwrap(), "S8");
        assert!(slcan_bit_rate_command(9).is_err());
    }

    #[test]
    fn test_slcan_bit_rate_label() {
        assert_eq!(slcan_bit_rate_label(6), Some("500 kbps"));
        assert_eq!(slcan_bit_rate_label(9), None);
    }

    #[test]
    fn test_slcan_uart_baud_command() {
        assert_eq!(slcan_uart_baud_command(1).unwrap(), "U1");
        assert_eq!(slcan_uart_baud_command(9).unwrap(), "U9");
        assert!(slcan_uart_baud_command(10).is_err());
    }

    #[test]
    fn test_slcan_uart_baud_label() {
        assert_eq!(slcan_uart_baud_label(1), Some(115_200));
        assert_eq!(slcan_uart_baud_label(10), None);
    }

    #[test]
    fn test_slcan_open() {
        let cmds = slcan_open(6).unwrap();
        assert_eq!(cmds, vec!["C", "S6", "O"]);
    }

    #[test]
    fn test_slcan_close() {
        assert_eq!(slcan_close(), "C");
    }

    #[test]
    fn test_slcan_open_channel() {
        assert_eq!(slcan_open_channel(), "O");
    }

    #[test]
    fn test_slcan_set_filter_mask() {
        assert_eq!(slcan_set_filter_mask(0x7FF), "M7FF");
        assert_eq!(slcan_set_filter_mask(0x123), "M123");
    }

    #[test]
    fn test_slcan_set_filter_code() {
        assert_eq!(slcan_set_filter_code(0x7FF), "m7FF");
        assert_eq!(slcan_set_filter_code(0x100), "m100");
    }

    #[test]
    fn test_slcan_get_commands() {
        assert_eq!(slcan_get_version(), "V");
        assert_eq!(slcan_get_serial(), "N");
        assert_eq!(slcan_get_status(), "F");
    }

    #[test]
    fn test_slcan_set_timestamp() {
        assert_eq!(slcan_set_timestamp(true), "Z1");
        assert_eq!(slcan_set_timestamp(false), "Z0");
    }

    #[test]
    fn test_slcan_auto_init() {
        assert_eq!(slcan_auto_init(), "i");
    }

    #[test]
    fn test_slcan_set_custom_btr() {
        assert_eq!(slcan_set_custom_btr(0x1234), "s1234");
    }

    // ---- Ring buffer / analyzer tests ----

    #[test]
    fn test_ring_buffer_eviction() {
        let mut analyzer = CanAnalyzer::with_capacity(5);
        for i in 0..10u32 {
            analyzer.add_frame(CanFrame::new_standard(i, vec![1]));
        }
        assert_eq!(analyzer.frame_count(), 5);
        // Oldest 5 frames evicted; remaining frames have IDs 5..10.
        // Stats are cumulative and retain all IDs ever seen.
        let retained_ids: Vec<u32> =
            analyzer.filtered_frames().iter().map(|f| f.id).collect();
        for id in 0..5u32 {
            assert!(
                !retained_ids.contains(&id),
                "id {} should have been evicted from frames",
                id
            );
        }
        for id in 5..10u32 {
            assert!(
                retained_ids.contains(&id),
                "id {} should be retained in frames",
                id
            );
        }
        // Stats still track all 10 IDs (cumulative)
        assert_eq!(analyzer.unique_ids().len(), 10);
    }

    #[test]
    fn test_set_max_frames() {
        let mut analyzer = CanAnalyzer::new();
        assert_eq!(analyzer.max_frames(), DEFAULT_MAX_FRAMES);

        for i in 0..20u32 {
            analyzer.add_frame(CanFrame::new_standard(i, vec![1]));
        }
        assert_eq!(analyzer.frame_count(), 20);

        analyzer.set_max_frames(10);
        assert_eq!(analyzer.max_frames(), 10);
        assert_eq!(analyzer.frame_count(), 10);
    }

    #[test]
    fn test_with_capacity() {
        let analyzer = CanAnalyzer::with_capacity(100);
        assert_eq!(analyzer.max_frames(), 100);
    }

    #[test]
    fn test_analyzer_add_frame_and_stats() {
        let mut analyzer = CanAnalyzer::new();
        let frame = CanFrame::new_standard(0x100, vec![0xAA, 0xBB]);
        analyzer.add_frame(frame);

        assert_eq!(analyzer.frame_count(), 1);
        let stats = analyzer.get_stats(0x100).unwrap();
        assert_eq!(stats.count, 1);
        assert_eq!(stats.last_data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_analyzer_unique_ids() {
        let mut analyzer = CanAnalyzer::new();
        analyzer.add_frame(CanFrame::new_standard(0x100, vec![1]));
        analyzer.add_frame(CanFrame::new_standard(0x200, vec![1]));
        analyzer.add_frame(CanFrame::new_standard(0x100, vec![2]));

        let ids = analyzer.unique_ids();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_analyzer_clear() {
        let mut analyzer = CanAnalyzer::new();
        analyzer.add_frame(CanFrame::new_standard(0x100, vec![1]));
        analyzer.add_frame(CanFrame::new_standard(0x200, vec![1]));

        analyzer.clear();
        assert_eq!(analyzer.frame_count(), 0);
        assert!(analyzer.unique_ids().is_empty());
    }

    #[test]
    fn test_analyzer_filtered_frames() {
        let mut analyzer = CanAnalyzer::new();
        analyzer.set_filter(CanFilter {
            min_id: 0x100,
            max_id: 0x1FF,
            ..Default::default()
        });
        analyzer.add_frame(CanFrame::new_standard(0x050, vec![1]));
        analyzer.add_frame(CanFrame::new_standard(0x150, vec![1]));
        analyzer.add_frame(CanFrame::new_standard(0x300, vec![1]));

        let filtered = analyzer.filtered_frames();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, 0x150);
    }

    #[test]
    fn test_format_frame() {
        let frame = CanFrame::new_standard(0x123, vec![0xAA, 0xBB]);
        let s = CanAnalyzer::format_frame(&frame);
        assert!(s.contains("0x123"));
        assert!(s.contains("AA BB"));
    }

    #[test]
    fn test_format_frame_fd() {
        let frame = CanFrame::new_fd(0x123, false, vec![0xAA], true);
        let s = CanAnalyzer::format_frame(&frame);
        assert!(s.contains("FD"));
        assert!(s.contains("BRS"));
    }

    #[test]
    fn test_parse_raw() {
        let analyzer = CanAnalyzer::new();
        // id_high=0x01, id_low=0x23, dlc=2, data=AA BB
        let frame = analyzer.parse_raw(&[0x01, 0x23, 0x02, 0xAA, 0xBB]).unwrap();
        assert_eq!(frame.id, 0x0123);
        assert_eq!(frame.dlc, 2);
        assert_eq!(frame.data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_parse_raw_too_short() {
        let analyzer = CanAnalyzer::new();
        assert!(analyzer.parse_raw(&[0x01, 0x02]).is_err());
    }

    #[test]
    fn test_parse_hex_data() {
        assert_eq!(parse_hex_data("AABB").unwrap(), vec![0xAA, 0xBB]);
        assert_eq!(parse_hex_data("").unwrap(), Vec::<u8>::new());
        assert_eq!(parse_hex_data("0102").unwrap(), vec![0x01, 0x02]);
        assert!(parse_hex_data("GG").is_err());
    }
}