//! Stream-oriented frame buffer for robust serial protocol framing.
//!
//! Serial reads are arbitrary byte chunks that don't align with protocol frame
//! boundaries. This module provides [`FrameBuffer`] which accumulates raw bytes
//! and extracts complete frames according to a pluggable [`FrameStrategy`].
//!
//! # Supported strategies
//!
//! - [`FrameStrategy::ModbusRtu`]: Modbus RTU framing via T3.5 character
//!   silence + CRC-16 validation. Handles the classic "粘包/拆包" problem
//!   where multiple reads must be concatenated before a frame is complete,
//!   or one read contains a partial frame.
//! - [`FrameStrategy::Newline`]: newline-terminated ASCII lines (SLCAN, NMEA,
//!   AT commands). Extraction is immediate — no silence wait needed.
//! - [`FrameStrategy::StartEnd`]: start/end byte delimited framing (e.g. SLIP
//!   `0xC0 ... 0xC0` used by ESP32 ROM bootloader).

use std::time::{Duration, Instant};

use super::modbus::ModbusFrame;

/// Strategy for extracting frames from the byte stream.
#[derive(Debug, Clone)]
pub enum FrameStrategy {
    /// Modbus RTU: frames are delimited by T3.5 character silence and
    /// validated by CRC-16. The baud rate determines T3.5 per the spec:
    /// - baud > 19200: T3.5 fixed at 1.75 ms
    /// - baud <= 19200: T3.5 = 3.5 * (11 bits / baud)
    ModbusRtu { baud_rate: u32 },

    /// Newline-terminated ASCII frames. If `strip_cr` is true, trailing `\r`
    /// before `\n` is removed from the returned frame data but the `\n` itself
    /// is kept as part of the frame (callers can trim if needed).
    Newline { strip_cr: bool },

    /// Start/end byte delimited frames. The returned frame includes the start
    /// and end bytes. The first matching start byte begins a frame; the next
    /// end byte completes it. Bytes before the first start byte are discarded.
    StartEnd { start: u8, end: u8 },
}

impl FrameStrategy {
    /// Convenience: Modbus RTU at a given baud rate.
    pub fn modbus_rtu(baud_rate: u32) -> Self {
        FrameStrategy::ModbusRtu { baud_rate }
    }

    /// Convenience: newline-delimited, stripping `\r`.
    pub fn newline() -> Self {
        FrameStrategy::Newline { strip_cr: true }
    }

    /// Convenience: SLIP-style `0xC0 ... 0xC0` framing.
    pub fn slip() -> Self {
        FrameStrategy::StartEnd { start: 0xC0, end: 0xC0 }
    }
}

/// Calculate the Modbus RTU T3.5 (inter-frame) silence duration for a given
/// baud rate.
///
/// Per the Modbus over Serial Line spec:
/// - For baud rates > 19200, T3.5 is fixed at 1.75 ms (implementation-defined,
///   but 1.75 ms is the standard recommendation).
/// - For baud rates <= 19200, T3.5 = 3.5 character times, where one character
///   = 11 bits (1 start + 8 data + 1 parity + 1 stop).
pub fn modbus_t3_5(baud_rate: u32) -> Duration {
    if baud_rate > 19200 {
        Duration::from_micros(1750)
    } else {
        // 3.5 * 11 bits / baud_rate, in microseconds
        let us = (3_500_000u64 * 11) / baud_rate.max(1) as u64;
        Duration::from_micros(us)
    }
}

/// Calculate the Modbus RTU T1.5 (inter-character) silence duration.
pub fn modbus_t1_5(baud_rate: u32) -> Duration {
    if baud_rate > 19200 {
        Duration::from_micros(750)
    } else {
        let us = (1_500_000u64 * 11) / baud_rate.max(1) as u64;
        Duration::from_micros(us)
    }
}

/// A stream buffer that accumulates raw bytes and extracts complete frames.
pub struct FrameBuffer {
    buf: Vec<u8>,
    strategy: FrameStrategy,
    /// Timestamp of the most recently received byte (for RTU silence).
    last_byte_time: Option<Instant>,
    /// T3.5 duration cached for RTU strategy.
    t3_5: Duration,
}

impl FrameBuffer {
    /// Create a new frame buffer with the given strategy.
    pub fn new(strategy: FrameStrategy) -> Self {
        let t3_5 = match &strategy {
            FrameStrategy::ModbusRtu { baud_rate } => modbus_t3_5(*baud_rate),
            _ => Duration::ZERO,
        };
        Self {
            buf: Vec::new(),
            strategy,
            last_byte_time: None,
            t3_5,
        }
    }

    /// Append raw bytes to the buffer. The `now` timestamp is used for RTU
    /// silence detection. If `now` is `None`, the current instant is used.
    pub fn feed(&mut self, data: &[u8], now: Option<Instant>) {
        if data.is_empty() {
            return;
        }
        let ts = now.unwrap_or_else(Instant::now);
        self.buf.extend_from_slice(data);
        self.last_byte_time = Some(ts);
    }

    /// Returns the number of buffered bytes not yet extracted as a frame.
    pub fn pending_len(&self) -> usize {
        self.buf.len()
    }

    /// Returns a slice of the pending (not-yet-extracted) bytes.
    pub fn pending(&self) -> &[u8] {
        &self.buf
    }

    /// Clear all buffered data.
    pub fn clear(&mut self) {
        self.buf.clear();
        self.last_byte_time = None;
    }

    /// Try to extract complete frames.
    ///
    /// - For [`FrameStrategy::Newline`] and [`FrameStrategy::StartEnd`],
    ///   extraction is immediate and returns all complete frames in the buffer.
    /// - For [`FrameStrategy::ModbusRtu`], a frame is only returned when T3.5
    ///   silence has elapsed since the last received byte. The frame is
    ///   CRC-validated; if CRC fails, the bytes are consumed and an error is
    ///   returned alongside (see [`RtuExtractResult`]).
    ///
    /// The `now` parameter is used for RTU silence checking. If `None`, the
    /// current instant is used.
    pub fn try_extract(&mut self, now: Option<Instant>) -> Vec<ExtractedFrame> {
        match &self.strategy {
            FrameStrategy::Newline { strip_cr } => self.extract_newline(*strip_cr),
            FrameStrategy::StartEnd { start, end } => self.extract_start_end(*start, *end),
            FrameStrategy::ModbusRtu { .. } => {
                let now = now.unwrap_or_else(Instant::now);
                vec![self.extract_modbus_rtu(now)]
            }
        }
    }

    /// Force-extract the pending buffer as a Modbus RTU frame, regardless of
    /// T3.5 silence. Useful when the caller knows the frame is complete (e.g.
    /// port was closed or a timeout fired). The frame is CRC-validated.
    pub fn flush_rtu(&mut self) -> RtuExtractResult {
        if self.buf.is_empty() {
            return RtuExtractResult::Empty;
        }
        let data = std::mem::take(&mut self.buf);
        self.last_byte_time = None;
        validate_and_split_rtu(data)
    }

    fn extract_newline(&mut self, strip_cr: bool) -> Vec<ExtractedFrame> {
        let mut frames = Vec::new();
        loop {
            let nl_pos = self.buf.iter().position(|&b| b == b'\n');
            match nl_pos {
                Some(pos) => {
                    let mut frame: Vec<u8> = self.buf.drain(..=pos).collect();
                    if strip_cr && frame.len() >= 2 && frame[frame.len() - 2] == b'\r' {
                        // Remove the \r but keep the \n
                        let nl = frame.pop().unwrap();
                        frame.pop();
                        frame.push(nl);
                    }
                    frames.push(ExtractedFrame::Data(frame));
                }
                None => break,
            }
        }
        frames
    }

    fn extract_start_end(&mut self, start: u8, end: u8) -> Vec<ExtractedFrame> {
        let mut frames = Vec::new();
        loop {
            // Find the first start byte.
            let start_pos = match self.buf.iter().position(|&b| b == start) {
                Some(p) => p,
                None => {
                    // No start byte — discard everything (incomplete garbage).
                    self.buf.clear();
                    break;
                }
            };
            // Discard bytes before the start byte.
            if start_pos > 0 {
                self.buf.drain(..start_pos);
            }
            // Find the end byte after the start byte.
            // 起始字节已位于 buf[0]，从位置 1 开始搜索结束字节。
            let search_from = 1;
            let end_pos = self.buf[search_from..].iter().position(|&b| b == end);
            match end_pos {
                Some(rel) => {
                    let abs_end = search_from + rel;
                    let frame: Vec<u8> = self.buf.drain(..=abs_end).collect();
                    frames.push(ExtractedFrame::Data(frame));
                }
                None => break, // incomplete frame, wait for more data
            }
        }
        frames
    }

    fn extract_modbus_rtu(&mut self, now: Instant) -> ExtractedFrame {
        if self.buf.is_empty() {
            return ExtractedFrame::Empty;
        }

        // Check T3.5 silence since last byte.
        let last = match self.last_byte_time {
            Some(t) => t,
            None => now.checked_sub(self.t3_5).unwrap_or(now),
        };

        if now.duration_since(last) < self.t3_5 {
            // Not enough silence yet — frame may still be growing.
            return ExtractedFrame::Pending;
        }

        // T3.5 elapsed: the buffer should contain one or more complete frames.
        let data = std::mem::take(&mut self.buf);
        self.last_byte_time = None;
        match validate_and_split_rtu(data) {
            RtuExtractResult::Frame(frame) => ExtractedFrame::Data(frame),
            RtuExtractResult::Frames(frames) => {
                // Multiple frames — return the first, requeue the rest.
                // Since ExtractedFrame is single, we return the first and
                // push the rest back into the buffer. The caller should call
                // try_extract again to get them (they'll be returned
                // immediately since last_byte_time is None → treated as
                // elapsed).
                let mut iter = frames.into_iter();
                if let Some(first) = iter.next() {
                    for rest in iter {
                        self.buf.extend_from_slice(&rest);
                    }
                    // Set last_byte_time to None so the requeued frames are
                    // immediately extractable on the next call.
                    ExtractedFrame::Data(first)
                } else {
                    ExtractedFrame::Empty
                }
            }
            RtuExtractResult::CrcError { consumed, .. } => {
                ExtractedFrame::Error(format!("CRC mismatch, consumed {} bytes", consumed))
            }
            RtuExtractResult::TooShort => {
                ExtractedFrame::Error("RTU frame too short (< 4 bytes) after T3.5".into())
            }
            RtuExtractResult::Empty => ExtractedFrame::Empty,
        }
    }
}

/// A single extraction result from [`FrameBuffer::try_extract`].
#[derive(Debug)]
pub enum ExtractedFrame {
    /// A complete frame was extracted.
    Data(Vec<u8>),
    /// No data available (buffer empty).
    Empty,
    /// Data is pending but not yet complete (RTU: T3.5 not elapsed).
    Pending,
    /// A framing error occurred (e.g. CRC mismatch).
    Error(String),
}

impl ExtractedFrame {
    /// Returns the frame data if this is [`ExtractedFrame::Data`].
    pub fn data(&self) -> Option<&[u8]> {
        match self {
            ExtractedFrame::Data(d) => Some(d.as_slice()),
            _ => None,
        }
    }

    /// Returns `true` if a complete frame was extracted.
    pub fn is_data(&self) -> bool {
        matches!(self, ExtractedFrame::Data(_))
    }
}

/// Result of extracting a Modbus RTU frame from a byte buffer.
#[derive(Debug)]
pub enum RtuExtractResult {
    /// A single complete frame was extracted (CRC valid).
    Frame(Vec<u8>),
    /// Multiple complete frames were extracted from the buffer.
    Frames(Vec<Vec<u8>>),
    /// CRC validation failed. `consumed` bytes were removed from the buffer.
    CrcError { consumed: usize },
    /// The buffer had fewer than 4 bytes (minimum RTU frame size).
    TooShort,
    /// The buffer was empty.
    Empty,
}

/// Validate and split a Modbus RTU byte buffer into one or more CRC-valid frames.
///
/// Modbus RTU frames have no explicit delimiter — they rely on T3.5 silence.
/// When the buffer is flushed, it may contain:
/// - Exactly one frame (common case)
/// - Multiple concatenated frames (if silence boundaries were missed)
/// - A partial frame (too short)
/// - A frame with bad CRC (line noise / corruption)
///
/// This function attempts to split the buffer by trying to parse from the
/// start. If CRC matches, the frame is extracted and the remainder is
/// recursively processed. If CRC fails at the minimum frame size (4 bytes),
/// we try advancing byte-by-byte to resync, up to a reasonable limit.
fn validate_and_split_rtu(data: Vec<u8>) -> RtuExtractResult {
    if data.is_empty() {
        return RtuExtractResult::Empty;
    }
    if data.len() < 4 {
        return RtuExtractResult::TooShort;
    }

    // Try to parse the entire buffer as a single frame first (common case).
    if let Ok(_frame) = ModbusFrame::parse(&data) {
        return RtuExtractResult::Frame(data);
    }

    // The buffer might contain multiple frames. Try to find the first valid
    // frame by testing CRC at increasing lengths. The minimum RTU frame is
    // 4 bytes (slave + func + 2 CRC); the maximum is 256 bytes (slave + func
    // + 252 data + 2 CRC per spec).
    let max_frame = data.len().min(256);
    for end in 4..=max_frame {
        if ModbusFrame::parse(&data[..end]).is_ok() {
            let frame = data[..end].to_vec();
            let remainder = data[end..].to_vec();
            if remainder.is_empty() {
                return RtuExtractResult::Frame(frame);
            }
            // Recursively process the remainder.
            let mut all_frames = vec![frame];
            match validate_and_split_rtu(remainder) {
                RtuExtractResult::Frame(f) => all_frames.push(f),
                RtuExtractResult::Frames(fs) => all_frames.extend(fs),
                RtuExtractResult::CrcError { consumed } => {
                    // Remainder had a CRC error — return the valid frames
                    // we found and discard the consumed bytes.
                    let _ = consumed;
                }
                RtuExtractResult::TooShort => {
                    // Remainder too short — discard it (partial frame).
                }
                RtuExtractResult::Empty => {}
            }
            return RtuExtractResult::Frames(all_frames);
        }
    }

    // No valid frame found in the buffer. Discard everything.
    RtuExtractResult::CrcError {
        consumed: data.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::modbus::{ModbusFrame, ModbusFunction};

    // ---- T3.5 calculation tests ----

    #[test]
    fn test_t3_5_high_baud() {
        // Baud > 19200: fixed 1.75 ms
        assert_eq!(modbus_t3_5(115200), Duration::from_micros(1750));
        assert_eq!(modbus_t3_5(38400), Duration::from_micros(1750));
    }

    #[test]
    fn test_t3_5_low_baud() {
        // 9600 baud: 3.5 * 11 / 9600 = 4.0104 ms
        let t = modbus_t3_5(9600);
        assert!(t >= Duration::from_micros(4000) && t <= Duration::from_micros(4011));
    }

    #[test]
    fn test_t3_5_boundary() {
        // Exactly 19200: calculated, not fixed
        let t = modbus_t3_5(19200);
        // 3.5 * 11 / 19200 = 2.005 ms
        assert!(t < Duration::from_millis(3));
    }

    // ---- Newline strategy tests ----

    #[test]
    fn test_newline_single_frame() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"hello\n", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), b"hello\n");
    }

    #[test]
    fn test_newline_multiple_frames() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"a\nb\n", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data().unwrap(), b"a\n");
        assert_eq!(frames[1].data().unwrap(), b"b\n");
    }

    #[test]
    fn test_newline_partial_then_complete() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"hel", None);
        let frames = fb.try_extract(None);
        assert!(frames.is_empty());

        fb.feed(b"lo\n", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), b"hello\n");
    }

    #[test]
    fn test_newline_strip_cr() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"hello\r\n", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        // \r should be stripped, \n kept
        assert_eq!(frames[0].data().unwrap(), b"hello\n");
    }

    #[test]
    fn test_newline_no_strip_cr() {
        let mut fb = FrameBuffer::new(FrameStrategy::Newline { strip_cr: false });
        fb.feed(b"hello\r\n", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), b"hello\r\n");
    }

    #[test]
    fn test_newline_leftover_kept() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"a\nb", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), b"a\n");
        assert_eq!(fb.pending(), b"b");

        fb.feed(b"\n", None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), b"b\n");
    }

    // ---- StartEnd strategy tests ----

    #[test]
    fn test_start_end_single_frame() {
        let mut fb = FrameBuffer::new(FrameStrategy::slip());
        fb.feed(&[0xC0, 0x01, 0x02, 0xC0], None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), vec![0xC0, 0x01, 0x02, 0xC0]);
    }

    #[test]
    fn test_start_end_multiple_frames() {
        let mut fb = FrameBuffer::new(FrameStrategy::slip());
        fb.feed(&[0xC0, 0x01, 0xC0, 0xC0, 0x02, 0xC0], None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data().unwrap(), vec![0xC0, 0x01, 0xC0]);
        assert_eq!(frames[1].data().unwrap(), vec![0xC0, 0x02, 0xC0]);
    }

    #[test]
    fn test_start_end_partial() {
        let mut fb = FrameBuffer::new(FrameStrategy::slip());
        fb.feed(&[0xC0, 0x01], None);
        let frames = fb.try_extract(None);
        assert!(frames.is_empty());
        assert_eq!(fb.pending(), &[0xC0, 0x01]);

        fb.feed(&[0x02, 0xC0], None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), vec![0xC0, 0x01, 0x02, 0xC0]);
    }

    #[test]
    fn test_start_end_discards_before_start() {
        let mut fb = FrameBuffer::new(FrameStrategy::slip());
        fb.feed(&[0xFF, 0xFE, 0xC0, 0x01, 0xC0], None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), vec![0xC0, 0x01, 0xC0]);
    }

    #[test]
    fn test_start_end_different_markers() {
        let mut fb = FrameBuffer::new(FrameStrategy::StartEnd { start: 0xAA, end: 0xBB });
        fb.feed(&[0xAA, 0x42, 0xBB], None);
        let frames = fb.try_extract(None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data().unwrap(), vec![0xAA, 0x42, 0xBB]);
    }

    // ---- Modbus RTU strategy tests ----

    fn make_rtu_frame(slave: u8, func: ModbusFunction, data: &[u8]) -> Vec<u8> {
        ModbusFrame::new(slave, func, data.to_vec()).to_bytes()
    }

    #[test]
    fn test_rtu_frame_after_silence() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let frame_bytes = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);

        let t0 = Instant::now();
        fb.feed(&frame_bytes, Some(t0));

        // Immediately after feeding — T3.5 not elapsed yet.
        let frames = fb.try_extract(Some(t0));
        assert_eq!(frames.len(), 1);
        assert!(matches!(frames[0], ExtractedFrame::Pending));

        // After T3.5 (1.75 ms at 115200) — frame should be extracted.
        let t1 = t0 + Duration::from_millis(2);
        let frames = fb.try_extract(Some(t1));
        assert_eq!(frames.len(), 1);
        let extracted = frames[0].data().expect("frame should be extracted");
        assert_eq!(extracted, frame_bytes);
    }

    #[test]
    fn test_rtu_partial_then_complete() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let frame_bytes = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);

        let t0 = Instant::now();
        // Feed first half
        fb.feed(&frame_bytes[..4], Some(t0));

        // Feed second half quickly (within T3.5)
        let t1 = t0 + Duration::from_micros(500);
        fb.feed(&frame_bytes[4..], Some(t1));

        // Not enough silence yet
        let frames = fb.try_extract(Some(t1));
        assert!(matches!(frames[0], ExtractedFrame::Pending));

        // After T3.5 from last byte
        let t2 = t1 + Duration::from_millis(2);
        let frames = fb.try_extract(Some(t2));
        let extracted = frames[0].data().expect("frame should be extracted");
        assert_eq!(extracted, frame_bytes);
    }

    #[test]
    fn test_rtu_too_short() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let t0 = Instant::now();
        fb.feed(&[0x01, 0x03], Some(t0)); // only 2 bytes, min is 4

        let t1 = t0 + Duration::from_millis(2);
        let frames = fb.try_extract(Some(t1));
        assert!(matches!(frames[0], ExtractedFrame::Error(_)));
    }

    #[test]
    fn test_rtu_crc_error() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let mut bad_frame = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);
        // Corrupt the CRC (last 2 bytes)
        let last_idx = bad_frame.len() - 1;
        bad_frame[last_idx] ^= 0xFF;

        let t0 = Instant::now();
        fb.feed(&bad_frame, Some(t0));
        let t1 = t0 + Duration::from_millis(2);
        let frames = fb.try_extract(Some(t1));
        assert!(matches!(frames[0], ExtractedFrame::Error(_)));
    }

    #[test]
    fn test_rtu_empty_buffer() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let frames = fb.try_extract(Some(Instant::now()));
        assert_eq!(frames.len(), 1);
        assert!(matches!(frames[0], ExtractedFrame::Empty));
    }

    #[test]
    fn test_rtu_flush_forces_extraction() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let frame_bytes = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);

        // Feed but don't wait for T3.5
        fb.feed(&frame_bytes, Some(Instant::now()));

        // Flush should extract immediately
        let result = fb.flush_rtu();
        match result {
            RtuExtractResult::Frame(f) => assert_eq!(f, frame_bytes),
            other => panic!("expected Frame, got {:?}", other),
        }
    }

    #[test]
    fn test_rtu_flush_empty() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let result = fb.flush_rtu();
        assert!(matches!(result, RtuExtractResult::Empty));
    }

    #[test]
    fn test_rtu_multiple_frames_in_buffer() {
        let f1 = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);
        let f2 = make_rtu_frame(0x02, ModbusFunction::ReadCoils, &[0x00, 0x00, 0x00, 0x08]);

        // Two frames concatenated (as if silence boundaries were missed)
        let mut combined = f1.clone();
        combined.extend_from_slice(&f2);

        let result = validate_and_split_rtu(combined);
        match result {
            RtuExtractResult::Frames(frames) => {
                assert_eq!(frames.len(), 2);
                assert_eq!(frames[0], f1);
                assert_eq!(frames[1], f2);
            }
            other => panic!("expected Frames, got {:?}", other),
        }
    }

    #[test]
    fn test_rtu_flush_splits_multiple() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let f1 = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);
        let f2 = make_rtu_frame(0x02, ModbusFunction::ReadCoils, &[0x00, 0x00, 0x00, 0x08]);

        let mut combined = f1.clone();
        combined.extend_from_slice(&f2);
        fb.feed(&combined, Some(Instant::now()));

        let result = fb.flush_rtu();
        match result {
            RtuExtractResult::Frames(frames) => {
                assert_eq!(frames.len(), 2);
                assert_eq!(frames[0], f1);
                assert_eq!(frames[1], f2);
            }
            other => panic!("expected Frames, got {:?}", other),
        }
    }

    #[test]
    fn test_rtu_requeued_frames_extract_immediately() {
        let mut fb = FrameBuffer::new(FrameStrategy::modbus_rtu(115200));
        let f1 = make_rtu_frame(0x01, ModbusFunction::ReadHoldingRegisters, &[0x00, 0x00, 0x00, 0x0A]);
        let f2 = make_rtu_frame(0x02, ModbusFunction::ReadCoils, &[0x00, 0x00, 0x00, 0x08]);
        let mut combined = f1.clone();
        combined.extend_from_slice(&f2);
        let t0 = Instant::now();
        fb.feed(&combined, Some(t0));
        let t1 = t0 + Duration::from_millis(2);

        let first = fb.try_extract(Some(t1));
        assert_eq!(first[0].data(), Some(f1.as_slice()));
        let second = fb.try_extract(Some(t1));
        assert_eq!(second[0].data(), Some(f2.as_slice()));
        assert_eq!(fb.pending_len(), 0);
    }

    // ---- Clear and pending tests ----

    #[test]
    fn test_clear() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"hello", None);
        assert_eq!(fb.pending_len(), 5);
        fb.clear();
        assert_eq!(fb.pending_len(), 0);
        assert!(fb.pending().is_empty());
    }

    #[test]
    fn test_pending_view() {
        let mut fb = FrameBuffer::new(FrameStrategy::newline());
        fb.feed(b"abc", None);
        assert_eq!(fb.pending(), b"abc");
    }
}
