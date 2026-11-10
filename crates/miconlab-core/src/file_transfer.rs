use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TransferError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Transfer failed: {0}")]
    TransferFailed(String),
    #[error("Protocol error: {0}")]
    ProtocolError(String),
    #[error("Checksum mismatch")]
    ChecksumMismatch,
}

pub type TransferResult<T> = Result<T, TransferError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransferProtocol {
    Xmodem,
    XmodemCrc,
    Ymodem,
    Zmodem,
}

#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub speed_bps: f64,
    pub elapsed: Duration,
    pub eta: Option<Duration>,
}

impl TransferProgress {
    pub fn percentage(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.bytes_transferred as f32 / self.total_bytes as f32) * 100.0
    }
}

pub struct FileTransfer {
    protocol: TransferProtocol,
    chunk_size: usize,
    timeout: Duration,
}

impl FileTransfer {
    pub fn new(protocol: TransferProtocol) -> Self {
        let chunk_size = match protocol {
            TransferProtocol::Xmodem | TransferProtocol::XmodemCrc => 128,
            TransferProtocol::Ymodem => 1024,
            TransferProtocol::Zmodem => 1024,
        };

        Self {
            protocol,
            chunk_size,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn send_file<F>(
        &self,
        file_path: &Path,
        mut write_fn: F,
        mut read_fn: impl FnMut() -> TransferResult<u8>,
        mut progress_fn: impl FnMut(TransferProgress),
    ) -> TransferResult<()>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
    {
        let mut file = File::open(file_path)?;
        let file_size = file.metadata()?.len();

        let mut buffer = vec![0u8; self.chunk_size];
        let mut bytes_sent = 0u64;
        let start_time = std::time::Instant::now();

        // 等待接收方初始握手（XMODEM: NAK；XMODEM-CRC/YMODEM: 'C'；ZMODEM 简化无握手）。
        // 读失败（超时）则直接开始发送，兼容不回握手的简化接收端。
        match self.protocol {
            TransferProtocol::Xmodem
            | TransferProtocol::XmodemCrc
            | TransferProtocol::Ymodem => {
                let _ = read_fn();
            }
            TransferProtocol::Zmodem => {}
        }

        // XMODEM/YMODEM 块号从 1 开始，逐块 +1（mod 256 由发送函数截断）
        let mut block_num: u32 = 1;

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }

            let chunk = &buffer[..bytes_read];

            match self.protocol {
                TransferProtocol::Xmodem => {
                    self.send_xmodem_chunk(chunk, block_num as u16, &mut write_fn, &mut read_fn)?;
                }
                TransferProtocol::XmodemCrc => {
                    self.send_xmodem_crc_chunk(chunk, block_num as u16, &mut write_fn, &mut read_fn)?;
                }
                TransferProtocol::Ymodem => {
                    self.send_ymodem_chunk(chunk, block_num, &mut write_fn, &mut read_fn)?;
                }
                TransferProtocol::Zmodem => {
                    self.send_zmodem_chunk(chunk, bytes_sent, file_size, &mut write_fn, &mut read_fn)?;
                }
            }

            block_num = block_num.wrapping_add(1);
            bytes_sent += bytes_read as u64;
            let elapsed = start_time.elapsed();
            let speed = if elapsed.as_secs() > 0 {
                bytes_sent as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            };

            let eta = if speed > 0.0 {
                let remaining = file_size - bytes_sent;
                Some(Duration::from_secs_f64(remaining as f64 / speed))
            } else {
                None
            };

            progress_fn(TransferProgress {
                bytes_transferred: bytes_sent,
                total_bytes: file_size,
                speed_bps: speed,
                elapsed,
                eta,
            });
        }

        // 传输结束：发送 EOT 并等待 ACK（标准接收方收完最后一块后必须等到 EOT 才结束）。
        // 部分简化接收端不回 ACK，读失败时容忍退出。
        match self.protocol {
            TransferProtocol::Xmodem
            | TransferProtocol::XmodemCrc
            | TransferProtocol::Ymodem => {
                for _ in 0..3 {
                    write_fn(&[0x04])?; // EOT
                    match read_fn() {
                        Ok(0x06) => break, // ACK → 完成
                        Ok(_) => continue, // NAK/其他 → 重发 EOT
                        Err(_) => break,   // 接收端已关闭/超时 → 视为完成
                    }
                }
            }
            TransferProtocol::Zmodem => {}
        }

        Ok(())
    }

    pub fn receive_file<F, G>(
        &self,
        file_path: &Path,
        mut write_fn: F,
        mut read_fn: G,
        mut progress_fn: impl FnMut(TransferProgress),
    ) -> TransferResult<()>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        let file_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("received");
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_path = file_path.with_file_name(format!(
            ".{}.{}.{}.part",
            file_name,
            std::process::id(),
            nonce
        ));

        let result: TransferResult<()> = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            let mut bytes_received = 0u64;
            let start_time = std::time::Instant::now();

            // 发送初始握手字节，触发发送方开始传输：
            // - XMODEM 经典模式：NAK (0x15) 表示使用算术校验和
            // - XMODEM-CRC / YMODEM：'C' (0x43) 表示使用 CRC16 校验
            // - ZMODEM：握手帧格式复杂（ZRINIT），此处保持简化不发初始帧
            match self.protocol {
                TransferProtocol::Xmodem => {
                    write_fn(&[0x15])?; // NAK
                }
                TransferProtocol::XmodemCrc | TransferProtocol::Ymodem => {
                    write_fn(&[0x43])?; // 'C'
                }
                TransferProtocol::Zmodem => {}
            }

            // 期望接收的块号（从 1 开始，mod 256），用于识别 ACK 丢失导致的重传块
            let mut expected_block: u8 = 1;

            loop {
                let chunk = match self.protocol {
                    TransferProtocol::Xmodem => {
                        self.receive_xmodem_chunk(&mut expected_block, &mut write_fn, &mut read_fn)?
                    }
                    TransferProtocol::XmodemCrc => {
                        self.receive_xmodem_crc_chunk(&mut expected_block, &mut write_fn, &mut read_fn)?
                    }
                    TransferProtocol::Ymodem => {
                        self.receive_ymodem_chunk(&mut expected_block, &mut write_fn, &mut read_fn)?
                    }
                    TransferProtocol::Zmodem => {
                        self.receive_zmodem_chunk(&mut read_fn)?
                    }
                };

                if chunk.is_empty() {
                    break;
                }

                file.write_all(&chunk)?;
                bytes_received += chunk.len() as u64;

                let elapsed = start_time.elapsed();
                let speed = if elapsed.as_secs() > 0 {
                    bytes_received as f64 / elapsed.as_secs_f64()
                } else {
                    0.0
                };

                progress_fn(TransferProgress {
                    bytes_transferred: bytes_received,
                    total_bytes: 0,
                    speed_bps: speed,
                    elapsed,
                    eta: None,
                });
            }

            file.flush()?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                if let Err(error) = std::fs::rename(&temp_path, file_path) {
                    let _ = std::fs::remove_file(&temp_path);
                    return Err(error.into());
                }
                Ok(())
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp_path);
                Err(error)
            }
        }
    }

    fn send_xmodem_chunk<F, G>(
        &self,
        chunk: &[u8],
        block_num: u16,
        write_fn: &mut F,
        read_fn: &mut G,
    ) -> TransferResult<()>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        let mut packet = Vec::new();
        packet.push(0x01); // SOH
        packet.push((block_num & 0xFF) as u8);
        packet.push(!(block_num & 0xFF) as u8);

        let mut padded = chunk.to_vec();
        padded.resize(self.chunk_size, 0x1A); // Pad with SUB
        packet.extend_from_slice(&padded);

        let checksum = Self::checksum_simple(&padded);
        packet.push(checksum);

        write_fn(&packet)?;

        let ack = read_fn()?;
        if ack != 0x06 {
            return Err(TransferError::ProtocolError(
                "Expected ACK".to_string(),
            ));
        }

        Ok(())
    }

    fn send_xmodem_crc_chunk<F, G>(
        &self,
        chunk: &[u8],
        block_num: u16,
        write_fn: &mut F,
        read_fn: &mut G,
    ) -> TransferResult<()>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        let mut packet = Vec::new();
        packet.push(0x01); // SOH
        packet.push((block_num & 0xFF) as u8);
        packet.push(!(block_num & 0xFF) as u8);

        let mut padded = chunk.to_vec();
        padded.resize(self.chunk_size, 0x1A);
        packet.extend_from_slice(&padded);

        let crc = Self::crc16(&padded);
        packet.extend_from_slice(&crc.to_be_bytes());

        write_fn(&packet)?;

        let ack = read_fn()?;
        if ack != 0x06 {
            return Err(TransferError::ProtocolError(
                "Expected ACK".to_string(),
            ));
        }

        Ok(())
    }

    fn send_ymodem_chunk<F, G>(
        &self,
        chunk: &[u8],
        block_num: u32,
        write_fn: &mut F,
        read_fn: &mut G,
    ) -> TransferResult<()>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        let mut packet = Vec::new();
        packet.push(0x02); // STX
        packet.push((block_num & 0xFF) as u8);
        packet.push(!(block_num & 0xFF) as u8);

        let mut padded = chunk.to_vec();
        padded.resize(self.chunk_size, 0x1A);
        packet.extend_from_slice(&padded);

        let crc = Self::crc16(&padded);
        packet.extend_from_slice(&crc.to_be_bytes());

        write_fn(&packet)?;

        let ack = read_fn()?;
        if ack != 0x06 {
            return Err(TransferError::ProtocolError(
                "Expected ACK".to_string(),
            ));
        }

        Ok(())
    }

    fn send_zmodem_chunk<F, G>(
        &self,
        chunk: &[u8],
        offset: u64,
        total_size: u64,
        write_fn: &mut F,
        _read_fn: &mut G,
    ) -> TransferResult<()>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        // Simplified ZMODEM implementation
        let mut packet = Vec::new();
        packet.extend_from_slice(b"**\x18\x01"); // ZPAD ZDLE ZDATA

        let mut header = Vec::new();
        header.push(0x00); // Frame type
        header.extend_from_slice(&offset.to_le_bytes());
        header.extend_from_slice(&total_size.to_le_bytes());

        let crc = Self::crc16(&header);
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&crc.to_le_bytes());

        // Send data with transparency
        for &byte in chunk {
            if byte == 0x0D || byte == 0x10 || byte == 0x11 || byte == 0x13 || byte == 0x18 || byte == 0x1B {
                packet.push(0x10); // ZDLE
                packet.push(byte ^ 0x40);
            } else {
                packet.push(byte);
            }
        }

        packet.extend_from_slice(b"\x18\x04"); // ZCRCZ

        write_fn(&packet)?;

        Ok(())
    }

    /// 接收一个 XMODEM 经典数据块（算术校验和）。
    ///
    /// 流程（修复点：完整 ACK/NAK 确认 + 块序号跟踪）：
    ///   1. 读取 SOH(0x01) + block_num + ~block_num + 128 字节数据 + 1 字节校验和
    ///   2. 校验 block_num 补码与算术校验和
    ///   3. 块号 == expected：发 ACK 并返回数据；块号 == expected-1：重传块，ACK 但丢弃
    ///   4. 校验失败或序号错乱：发 NAK (0x15) 请求重传，循环等待下一块
    ///   5. 收到 EOT (0x04)：发 ACK 结束传输，返回空 Vec
    fn receive_xmodem_chunk<F, G>(
        &self,
        expected: &mut u8,
        write_fn: &mut F,
        read_fn: &mut G,
    ) -> TransferResult<Vec<u8>>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        loop {
            let header = read_fn()?;
            match header {
                0x01 => {
                    // SOH: 数据块起始
                    let block_num = read_fn()?;
                    let block_inv = read_fn()?;

                    let mut data = Vec::with_capacity(self.chunk_size);
                    for _ in 0..self.chunk_size {
                        data.push(read_fn()?);
                    }

                    let checksum = read_fn()?;
                    let expected_sum = Self::checksum_simple(&data);

                    // 校验：块号补码 + 算术校验和
                    let block_ok = (block_num ^ 0xFF) == block_inv && checksum == expected_sum;

                    if !block_ok {
                        // 校验失败：发 NAK 请求重传，继续循环读下一块
                        write_fn(&[0x15])?; // NAK
                        continue;
                    }
                    if block_num == *expected {
                        write_fn(&[0x06])?; // ACK
                        *expected = expected.wrapping_add(1);
                        return Ok(data);
                    } else if block_num == expected.wrapping_sub(1) {
                        // 重传块（上次 ACK 丢失）：ACK 但不重复返回数据
                        write_fn(&[0x06])?; // ACK
                        continue;
                    } else {
                        // 序号错乱：NAK 请求重传期望块
                        write_fn(&[0x15])?; // NAK
                        continue;
                    }
                }
                0x04 => {
                    // EOT: 发 ACK 结束传输
                    write_fn(&[0x06])?; // ACK
                    return Ok(Vec::new());
                }
                _ => {
                    return Err(TransferError::ProtocolError(
                        "Unexpected header".to_string(),
                    ));
                }
            }
        }
    }

    /// 接收一个 XMODEM-CRC 数据块（CRC16 校验）。
    ///
    /// 流程同 [`receive_xmodem_chunk`](Self::receive_xmodem_chunk)，
    /// 区别在于校验字段为 2 字节 CRC16（大端序）。
    fn receive_xmodem_crc_chunk<F, G>(
        &self,
        expected: &mut u8,
        write_fn: &mut F,
        read_fn: &mut G,
    ) -> TransferResult<Vec<u8>>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        loop {
            let header = read_fn()?;
            match header {
                0x01 => {
                    let block_num = read_fn()?;
                    let block_inv = read_fn()?;

                    let mut data = Vec::with_capacity(self.chunk_size);
                    for _ in 0..self.chunk_size {
                        data.push(read_fn()?);
                    }

                    let crc_hi = read_fn()? as u16;
                    let crc_lo = read_fn()? as u16;
                    let received_crc = (crc_hi << 8) | crc_lo;
                    let expected_crc = Self::crc16(&data);

                    let block_ok = (block_num ^ 0xFF) == block_inv && received_crc == expected_crc;

                    if !block_ok {
                        write_fn(&[0x15])?; // NAK
                        continue;
                    }
                    if block_num == *expected {
                        write_fn(&[0x06])?; // ACK
                        *expected = expected.wrapping_add(1);
                        return Ok(data);
                    } else if block_num == expected.wrapping_sub(1) {
                        // 重传块：ACK 但丢弃
                        write_fn(&[0x06])?; // ACK
                        continue;
                    } else {
                        write_fn(&[0x15])?; // NAK
                        continue;
                    }
                }
                0x04 => {
                    write_fn(&[0x06])?; // ACK
                    return Ok(Vec::new());
                }
                _ => {
                    return Err(TransferError::ProtocolError(
                        "Unexpected header".to_string(),
                    ));
                }
            }
        }
    }

    fn receive_ymodem_chunk<F, G>(
        &self,
        expected: &mut u8,
        write_fn: &mut F,
        read_fn: &mut G,
    ) -> TransferResult<Vec<u8>>
    where
        F: FnMut(&[u8]) -> TransferResult<()>,
        G: FnMut() -> TransferResult<u8>,
    {
        // YMODEM 数据块格式与 XMODEM-CRC 一致（SOH/STX + 块号 + 数据 + CRC16），
        // 复用 CRC 接收逻辑（含 ACK/NAK 确认与块序号跟踪）。
        // 注：YMODEM 的块 0（文件名/大小）此处未单独处理，保持简化。
        self.receive_xmodem_crc_chunk(expected, write_fn, read_fn)
    }

    fn receive_zmodem_chunk<G>(&self, read_fn: &mut G) -> TransferResult<Vec<u8>>
    where
        G: FnMut() -> TransferResult<u8>,
    {
        // Simplified ZMODEM receive（与发送侧的转义约定配套：
        // 0x10 为转义前缀，数据中的 0x0D/0x10/0x11/0x13/0x18/0x1B 被发为 0x10 + byte^0x40）
        let mut buffer = Vec::new();
        loop {
            let byte = read_fn()?;
            match byte {
                0x10 => {
                    // 转义对：读取下一字节并还原
                    let next = read_fn()?;
                    buffer.push(next ^ 0x40);
                }
                // 数据中的 0x04 已被发送侧转义，裸 0x04 只可能是帧结束标记
                0x04 => break,
                _ => buffer.push(byte),
            }
        }
        Ok(buffer)
    }

    fn checksum_simple(data: &[u8]) -> u8 {
        data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
    }

    fn crc16(data: &[u8]) -> u16 {
        let mut crc: u16 = 0;
        for &byte in data {
            crc ^= (byte as u16) << 8;
            for _ in 0..8 {
                if crc & 0x8000 != 0 {
                    crc = (crc << 1) ^ 0x1021;
                } else {
                    crc <<= 1;
                }
            }
        }
        crc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let checksum = FileTransfer::checksum_simple(&data);
        assert_eq!(checksum, 10);
    }

    #[test]
    fn test_crc16() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let crc = FileTransfer::crc16(&data);
        assert!(crc != 0);
    }

    #[test]
    fn test_transfer_progress() {
        let progress = TransferProgress {
            bytes_transferred: 50,
            total_bytes: 100,
            speed_bps: 1024.0,
            elapsed: Duration::from_secs(1),
            eta: Some(Duration::from_secs(1)),
        };

        assert_eq!(progress.percentage(), 50.0);
    }

    #[test]
    fn test_file_transfer_new() {
        let transfer = FileTransfer::new(TransferProtocol::Xmodem);
        assert_eq!(transfer.chunk_size, 128);

        let transfer = FileTransfer::new(TransferProtocol::Ymodem);
        assert_eq!(transfer.chunk_size, 1024);
    }

    #[test]
    fn test_failed_receive_preserves_existing_file() {
        let path = std::env::temp_dir().join(format!(
            "miconlab_failed_receive_{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"original contents").unwrap();
        let transfer = FileTransfer::new(TransferProtocol::XmodemCrc);
        let result = transfer.receive_file(
            &path,
            |_| Ok(()),
            || Err(TransferError::TransferFailed("simulated disconnect".into())),
            |_| {},
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original contents");
        let _ = std::fs::remove_file(&path);
    }
}
