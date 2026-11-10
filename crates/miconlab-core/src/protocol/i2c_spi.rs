/// I2C and SPI debug tools for embedded development.
///
/// The CH341A command bytes follow the WCH CH341DLL v1.9 protocol:
/// - `0xAA` = I2C stream command (per-USB-packet prefix)
/// - `0x74` = I2C START condition
/// - `0x75` = I2C STOP condition
/// - `0x80 | count` = OUT (write) `count` bytes (1-30), followed by `count` data bytes
/// - `0xC0 | count` = IN (read) `count` bytes (1-32), all ACK'd
/// - `0xC0`          = IN 1 byte with NAK (end of read)
/// - `0x60 | speed`  = set I2C speed (0=20k, 1=100k, 2=400k, 3=750k)
/// - `0xAB` = SPI stream command (data bytes clocked out, MISO read back)

use thiserror::Error;

#[derive(Error, Debug)]
pub enum BusError {
    #[error("I2C error: {0}")]
    I2cError(String),
    #[error("SPI error: {0}")]
    SpiError(String),
    #[error("NAK received")]
    Nack,
    #[error("Timeout")]
    Timeout,
}

pub type I2cResult<T> = Result<T, BusError>;
pub type SpiResult<T> = Result<T, BusError>;

// ---- CH341A command constants ----

/// I2C stream command — prefixes each USB bulk-out packet.
const CH341_CMD_I2C_STREAM: u8 = 0xAA;
/// SPI stream command — following bytes are SPI data.
const CH341_CMD_SPI_STREAM: u8 = 0xAB;
/// I2C START condition sub-command.
const CH341_I2C_STM_STA: u8 = 0x74;
/// I2C STOP condition sub-command.
const CH341_I2C_STM_STO: u8 = 0x75;
/// I2C OUT (write) base: `0x80 | count` followed by `count` data bytes.
const CH341_I2C_STM_OUT: u8 = 0x80;
/// I2C IN (read) base: `0xC0 | count` reads `count` bytes (all ACK'd).
/// `0xC0` alone (count=0) reads 1 byte with NAK (end of read).
const CH341_I2C_STM_IN: u8 = 0xC0;
/// I2C speed set base: `0x60 | speed` (speed: 0=20k, 1=100k, 2=400k, 3=750k).
const CH341_I2C_STM_SET: u8 = 0x60;
/// End-of-packet marker (sub-command stream terminator).
///
/// 保留为协议文档常量：当前 `ch341_packetize` 实现依赖 USB bulk 传输的
/// 短包语义（变长包即结束），无需显式 END 填充。若未来迁移到必须以
/// END 终止的传输模式（如某些固件版本），可在此处启用。
#[allow(dead_code)]
const CH341_I2C_STM_END: u8 = 0x00;

/// Maximum data bytes per OUT sub-command (fits in a 32-byte USB packet:
/// 1 byte 0xAA + 1 byte OUT cmd + 30 bytes data = 32).
const CH341_I2C_MAX_OUT_CHUNK: usize = 30;
/// Maximum bytes per IN sub-command (IN cmd is 1 byte, no data follows).
const CH341_I2C_MAX_IN_CHUNK: usize = 32;
/// USB bulk-out packet size.
const CH341_PACKET_SIZE: usize = 32;

// ---- I2C speed codes ----

/// I2C bus speed codes for the CH341A SET command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum I2cSpeed {
    /// 20 kHz
    Low = 0,
    /// 100 kHz (default)
    Standard = 1,
    /// 400 kHz
    Fast = 2,
    /// 750 kHz
    High = 3,
}

impl I2cSpeed {
    /// Get the frequency in Hz.
    pub fn hz(self) -> u32 {
        match self {
            Self::Low => 20_000,
            Self::Standard => 100_000,
            Self::Fast => 400_000,
            Self::High => 750_000,
        }
    }
}

// ---- Result types ----

/// Result of an I2C bus scan.
#[derive(Debug, Clone)]
pub struct I2cScanEntry {
    pub address: u8,
    pub name: Option<String>,
}

/// Known I2C device names by address.
pub fn i2c_device_name(address: u8) -> Option<&'static str> {
    match address {
        0x50..=0x57 => Some("EEPROM (AT24Cxx)"),
        0x68 => Some("RTC (DS3231) / IMU (MPU6050)"),
        0x76 | 0x77 => Some("Pressure/Humidity (BME280/BMP280)"),
        0x3C | 0x3D => Some("OLED Display (SSD1306)"),
        0x20..=0x27 => Some("GPIO Expander (PCF8574)"),
        0x48..=0x4F => Some("ADC (ADS1115) / Temperature (LM75)"),
        0x29 => Some("Distance Sensor (VL53L0X)"),
        0x60..=0x63 => Some("Clock Generator (SI5351)"),
        0x40..=0x47 => Some("Current Sensor (INA219/INA226)"),
        _ => None,
    }
}

// ---- CH341A I2C command builders ----
//
// These functions return the I2C sub-command stream (without the 0xAA packet
// prefix). Use [`ch341_packetize`] to split the stream into 0xAA-prefixed USB
// bulk-out packets before sending to the device.

/// Build an I2C write command stream for the CH341A.
///
/// Produces: `STA | OUT(addr) | OUT(data...) | STO`
/// The `address` is a 7-bit I2C address (will be shifted left).
pub fn ch341_i2c_write(address: u8, data: &[u8]) -> Vec<u8> {
    let mut cmd = Vec::new();
    cmd.push(CH341_I2C_STM_STA);
    // Write address byte (7-bit addr << 1, R/W = 0)
    cmd.push(CH341_I2C_STM_OUT | 1);
    cmd.push((address << 1) & 0xFE);
    // Write data in chunks of up to MAX_OUT_CHUNK bytes
    for chunk in data.chunks(CH341_I2C_MAX_OUT_CHUNK) {
        cmd.push(CH341_I2C_STM_OUT | (chunk.len() as u8));
        cmd.extend_from_slice(chunk);
    }
    cmd.push(CH341_I2C_STM_STO);
    cmd
}

/// Build an I2C read command stream for the CH341A.
///
/// Produces: `STA | OUT(addr|R) | IN(len bytes) | STO`
/// The last byte is read with NAK to signal end-of-read per I2C spec.
pub fn ch341_i2c_read(address: u8, len: usize) -> Vec<u8> {
    let mut cmd = Vec::new();
    cmd.push(CH341_I2C_STM_STA);
    cmd.push(CH341_I2C_STM_OUT | 1);
    cmd.push((address << 1) | 0x01); // Read address
    append_i2c_in_commands(&mut cmd, len);
    cmd.push(CH341_I2C_STM_STO);
    cmd
}

/// Build an I2C write-then-read command stream for the CH341A.
///
/// This is the standard pattern for reading from a register: write the
/// register address, then repeated-start and read data.
///
/// Produces: `STA | OUT(addr|W + write_data) | STA | OUT(addr|R) | IN(read_len) | STO`
pub fn ch341_i2c_write_read(address: u8, write_data: &[u8], read_len: usize) -> Vec<u8> {
    let mut cmd = Vec::new();
    cmd.push(CH341_I2C_STM_STA);
    // Write phase: address + write_data
    if !write_data.is_empty() {
        let total = 1 + write_data.len(); // addr byte + data
        // Chunk: first chunk includes the address byte
        let first_chunk_size = total.min(CH341_I2C_MAX_OUT_CHUNK);
        cmd.push(CH341_I2C_STM_OUT | (first_chunk_size as u8));
        cmd.push((address << 1) & 0xFE); // write address
        let first_data = write_data.len().min(first_chunk_size - 1);
        cmd.extend_from_slice(&write_data[..first_data]);
        // Remaining write data in additional OUT chunks
        let mut offset = first_data;
        while offset < write_data.len() {
            let chunk_size = (write_data.len() - offset).min(CH341_I2C_MAX_OUT_CHUNK);
            cmd.push(CH341_I2C_STM_OUT | (chunk_size as u8));
            cmd.extend_from_slice(&write_data[offset..offset + chunk_size]);
            offset += chunk_size;
        }
    }
    // Repeated START for read phase
    cmd.push(CH341_I2C_STM_STA);
    cmd.push(CH341_I2C_STM_OUT | 1);
    cmd.push((address << 1) | 0x01); // read address
    append_i2c_in_commands(&mut cmd, read_len);
    cmd.push(CH341_I2C_STM_STO);
    cmd
}

/// Build I2C scan commands for all valid addresses (0x08..=0x77).
///
/// Each entry is `(address, sub_command_stream)` where the stream probes
/// the address with a write (START + addr + STOP). A device that ACKs
/// the address byte is present.
pub fn ch341_i2c_scan_commands() -> Vec<(u8, Vec<u8>)> {
    let mut commands = Vec::new();
    for addr in 0x08..=0x77 {
        let mut cmd = Vec::new();
        cmd.push(CH341_I2C_STM_STA);
        cmd.push(CH341_I2C_STM_OUT | 1);
        cmd.push((addr << 1) & 0xFE); // write address
        cmd.push(CH341_I2C_STM_STO);
        commands.push((addr, cmd));
    }
    commands
}

/// Build a CH341A I2C speed-set command stream.
///
/// `speed` is a speed code (0=20k, 1=100k, 2=400k, 3=750k).
pub fn ch341_set_speed(speed: I2cSpeed) -> Vec<u8> {
    vec![CH341_I2C_STM_SET | (speed as u8)]
}

/// Append I2C IN (read) sub-commands for reading `len` bytes.
///
/// All bytes except the last are read with ACK; the final byte is read
/// with NAK (0xC0) to signal end-of-read per I2C spec.
fn append_i2c_in_commands(cmd: &mut Vec<u8>, len: usize) {
    if len == 0 {
        return;
    }
    let mut remaining = len;
    // Full chunks of MAX_IN_CHUNK bytes, all ACK'd
    while remaining > CH341_I2C_MAX_IN_CHUNK {
        cmd.push(CH341_I2C_STM_IN | (CH341_I2C_MAX_IN_CHUNK as u8));
        remaining -= CH341_I2C_MAX_IN_CHUNK;
    }
    // Second-to-last chunk: (remaining - 1) bytes, all ACK'd
    if remaining > 1 {
        cmd.push(CH341_I2C_STM_IN | ((remaining - 1) as u8));
    }
    // Last byte: NAK'd (0xC0 = IN | 0)
    cmd.push(CH341_I2C_STM_IN);
}

// ---- CH341A packetization ----

/// Split an I2C sub-command stream into 0xAA-prefixed USB bulk-out packets.
///
/// Each packet is at most 32 bytes: `[0xAA, sub_cmds..., (padding)]`.
/// Sub-commands are never split across packets — an OUT sub-command and its
/// data bytes always stay together.
pub fn ch341_packetize(sub_cmds: &[u8]) -> Vec<Vec<u8>> {
    if sub_cmds.is_empty() {
        return Vec::new();
    }

    let mut packets = Vec::new();
    let mut current = vec![CH341_CMD_I2C_STREAM];
    let mut i = 0;

    while i < sub_cmds.len() {
        let cmd_byte = sub_cmds[i];
        // Determine sub-command size: OUT has data following, others are 1 byte
        let sub_size = if (cmd_byte & 0xC0) == CH341_I2C_STM_OUT {
            // OUT: 1 + (count) bytes
            1 + (cmd_byte & 0x3F) as usize
        } else {
            // STA, STO, IN, SET, END: 1 byte
            1
        };

        let end = (i + sub_size).min(sub_cmds.len());
        let actual_size = end - i;

        if current.len() + actual_size > CH341_PACKET_SIZE {
            // Flush current packet
            packets.push(std::mem::take(&mut current));
            current = vec![CH341_CMD_I2C_STREAM];
        }
        current.extend_from_slice(&sub_cmds[i..end]);
        i = end;
    }

    if current.len() > 1 {
        packets.push(current);
    }
    packets
}

// ---- SPI ----

/// Build an SPI transfer command stream for the CH341A.
///
/// The CH341A SPI interface uses the `0xAB` (SPI_STREAM) command: each
/// following byte is clocked out via MOSI, and MISO data is returned
/// simultaneously (full-duplex).
///
/// **Note:** CS (chip select) control requires separate GPIO commands
/// (`0xA1`) before and after the SPI stream. The `cs_pin` and `mode`
/// parameters are kept for API compatibility but are not encoded in the
/// stream — SPI mode is set via `CH341SetStream` at the API level.
pub fn spi_transfer(tx_data: &[u8], _cs_pin: u8, _mode: u8) -> Vec<u8> {
    let mut cmd = Vec::with_capacity(1 + tx_data.len());
    cmd.push(CH341_CMD_SPI_STREAM);
    cmd.extend_from_slice(tx_data);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ch341_i2c_write tests ----

    #[test]
    fn test_ch341_i2c_write_basic() {
        let cmd = ch341_i2c_write(0x50, &[0x00, 0x42]);
        // STA, OUT 1 (addr), 0xA0, OUT 2 (data), 0x00, 0x42, STO
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[2], 0xA0); // 0x50 << 1 = 0xA0
        assert_eq!(cmd[3], CH341_I2C_STM_OUT | 2);
        assert_eq!(cmd[4], 0x00);
        assert_eq!(cmd[5], 0x42);
        assert_eq!(cmd[6], CH341_I2C_STM_STO);
    }

    #[test]
    fn test_ch341_i2c_write_empty_data() {
        let cmd = ch341_i2c_write(0x50, &[]);
        // STA, OUT 1 (addr), 0xA0, STO — no data OUT
        assert_eq!(cmd.len(), 4);
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[2], 0xA0);
        assert_eq!(cmd[3], CH341_I2C_STM_STO);
    }

    #[test]
    fn test_ch341_i2c_write_chunking() {
        // Data exceeding MAX_OUT_CHUNK should be split into multiple OUTs
        let data = vec![0xAA; 35]; // > 30
        let cmd = ch341_i2c_write(0x50, &data);
        // STA, OUT 1, addr, OUT 30, data[0..30], OUT 5, data[30..35], STO
        let mut idx = 0;
        assert_eq!(cmd[idx], CH341_I2C_STM_STA);
        idx += 1;
        assert_eq!(cmd[idx], CH341_I2C_STM_OUT | 1);
        idx += 1;
        assert_eq!(cmd[idx], 0xA0);
        idx += 1;
        // First chunk: 30 bytes
        assert_eq!(cmd[idx], CH341_I2C_STM_OUT | 30);
        idx += 1;
        idx += 30; // skip 30 data bytes
        // Second chunk: 5 bytes
        assert_eq!(cmd[idx], CH341_I2C_STM_OUT | 5);
        idx += 1;
        idx += 5;
        assert_eq!(cmd[idx], CH341_I2C_STM_STO);
    }

    #[test]
    fn test_ch341_i2c_write_address_mask() {
        // Ensure 7-bit address is properly shifted and R/W bit is 0
        let cmd = ch341_i2c_write(0x68, &[]);
        assert_eq!(cmd[2], 0xD0); // 0x68 << 1 = 0xD0
    }

    // ---- ch341_i2c_read tests ----

    #[test]
    fn test_ch341_i2c_read_basic() {
        let cmd = ch341_i2c_read(0x68, 3);
        // STA, OUT 1, 0xD1, IN 2 (ACK), IN 0 (NAK last), STO
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[2], 0xD1); // (0x68 << 1) | 1
        assert_eq!(cmd[3], CH341_I2C_STM_IN | 2); // read 2 bytes ACK
        assert_eq!(cmd[4], CH341_I2C_STM_IN); // read 1 byte NAK (0xC0)
        assert_eq!(cmd[5], CH341_I2C_STM_STO);
    }

    #[test]
    fn test_ch341_i2c_read_single_byte() {
        let cmd = ch341_i2c_read(0x50, 1);
        // STA, OUT 1, 0xA1, IN 0 (1 byte NAK), STO
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[2], 0xA1);
        assert_eq!(cmd[3], CH341_I2C_STM_IN); // 0xC0 = 1 byte NAK
        assert_eq!(cmd[4], CH341_I2C_STM_STO);
    }

    #[test]
    fn test_ch341_i2c_read_large() {
        let cmd = ch341_i2c_read(0x50, 65);
        // 65 bytes: IN 32 (ACK) + IN 32 (ACK) + IN 0 (1 byte NAK) = 65
        let in_cmds: Vec<u8> = cmd.iter()
            .filter(|&&b| (b & 0xC0) == CH341_I2C_STM_IN)
            .copied()
            .collect();
        // 3 IN commands: 2 full (32 each) + 1 NAK (1 byte) = 65 bytes
        assert_eq!(in_cmds.len(), 3);
        assert_eq!(in_cmds[0], CH341_I2C_STM_IN | 32);
        assert_eq!(in_cmds[1], CH341_I2C_STM_IN | 32);
        assert_eq!(in_cmds[2], CH341_I2C_STM_IN); // 0xC0 = last byte NAK
    }

    #[test]
    fn test_ch341_i2c_read_zero() {
        let cmd = ch341_i2c_read(0x50, 0);
        // STA, OUT 1, addr|R, STO — no IN commands
        assert_eq!(cmd.len(), 4);
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[3], CH341_I2C_STM_STO);
    }

    // ---- ch341_i2c_write_read tests ----

    #[test]
    fn test_ch341_i2c_write_read_basic() {
        let cmd = ch341_i2c_write_read(0x50, &[0x00], 2);
        // STA, OUT 2 (addr+reg), 0xA0, 0x00, STA (repeated), OUT 1, 0xA1, IN 1 (ACK), IN 0 (NAK), STO
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_OUT | 2); // OUT 2 bytes: addr + register
        assert_eq!(cmd[2], 0xA0);
        assert_eq!(cmd[3], 0x00);
        assert_eq!(cmd[4], CH341_I2C_STM_STA); // repeated START
        assert_eq!(cmd[5], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[6], 0xA1); // read address
        assert_eq!(cmd[7], CH341_I2C_STM_IN | 1); // 1 byte ACK
        assert_eq!(cmd[8], CH341_I2C_STM_IN); // 1 byte NAK
        assert_eq!(cmd[9], CH341_I2C_STM_STO);
    }

    #[test]
    fn test_ch341_i2c_write_read_no_write() {
        let cmd = ch341_i2c_write_read(0x50, &[], 1);
        // STA, STA (repeated), OUT 1, addr|R, IN 0 (NAK), STO
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_STA); // repeated START
        assert_eq!(cmd[2], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[3], 0xA1);
        assert_eq!(cmd[4], CH341_I2C_STM_IN);
        assert_eq!(cmd[5], CH341_I2C_STM_STO);
    }

    // ---- scan tests ----

    #[test]
    fn test_ch341_i2c_scan_commands() {
        let commands = ch341_i2c_scan_commands();
        assert_eq!(commands.len(), 0x70); // 0x08 to 0x77 = 112 addresses

        // First address
        assert_eq!(commands[0].0, 0x08);
        let cmd = &commands[0].1;
        assert_eq!(cmd[0], CH341_I2C_STM_STA);
        assert_eq!(cmd[1], CH341_I2C_STM_OUT | 1);
        assert_eq!(cmd[2], 0x10); // 0x08 << 1 = 0x10
        assert_eq!(cmd[3], CH341_I2C_STM_STO);

        // Last address
        assert_eq!(commands.last().unwrap().0, 0x77);
        assert_eq!(commands.last().unwrap().1[2], 0xEE); // 0x77 << 1 = 0xEE
    }

    // ---- speed tests ----

    #[test]
    fn test_ch341_set_speed() {
        assert_eq!(ch341_set_speed(I2cSpeed::Low), vec![0x60]);
        assert_eq!(ch341_set_speed(I2cSpeed::Standard), vec![0x61]);
        assert_eq!(ch341_set_speed(I2cSpeed::Fast), vec![0x62]);
        assert_eq!(ch341_set_speed(I2cSpeed::High), vec![0x63]);
    }

    #[test]
    fn test_i2c_speed_hz() {
        assert_eq!(I2cSpeed::Low.hz(), 20_000);
        assert_eq!(I2cSpeed::Standard.hz(), 100_000);
        assert_eq!(I2cSpeed::Fast.hz(), 400_000);
        assert_eq!(I2cSpeed::High.hz(), 750_000);
    }

    // ---- packetize tests ----

    #[test]
    fn test_ch341_packetize_single() {
        let sub = ch341_i2c_write(0x50, &[0x01]);
        let packets = ch341_packetize(&sub);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0][0], CH341_CMD_I2C_STREAM);
        // 0xAA + STA + OUT1 + addr + OUT1 + data + STO = 7 bytes
        assert_eq!(packets[0].len(), 7);
    }

    #[test]
    fn test_ch341_packetize_empty() {
        let packets = ch341_packetize(&[]);
        assert!(packets.is_empty());
    }

    #[test]
    fn test_ch341_packetize_multi() {
        // Build a long sub-command stream that exceeds 32 bytes
        let data = vec![0xAA; 50];
        let sub = ch341_i2c_write(0x50, &data);
        let packets = ch341_packetize(&sub);
        assert!(packets.len() > 1);
        // Each packet starts with 0xAA
        for pkt in &packets {
            assert_eq!(pkt[0], CH341_CMD_I2C_STREAM);
            assert!(pkt.len() <= CH341_PACKET_SIZE);
        }
    }

    #[test]
    fn test_ch341_packetize_no_split_out() {
        // An OUT sub-command must not be split across packets
        let data = vec![0xBB; 30]; // exactly MAX_OUT_CHUNK
        let sub = ch341_i2c_write(0x50, &data);
        let packets = ch341_packetize(&sub);
        // Verify no packet has an OUT cmd at the end without its data
        for pkt in &packets {
            let mut i = 1; // skip 0xAA
            while i < pkt.len() {
                let b = pkt[i];
                if (b & 0xC0) == CH341_I2C_STM_OUT {
                    let count = (b & 0x3F) as usize;
                    // Ensure all data bytes are in this packet
                    assert!(i + 1 + count <= pkt.len(),
                        "OUT sub-command split: i={}, count={}, pkt_len={}",
                        i, count, pkt.len());
                    i += 1 + count;
                } else {
                    i += 1;
                }
            }
        }
    }

    // ---- SPI tests ----

    #[test]
    fn test_spi_transfer_basic() {
        let cmd = spi_transfer(&[0xAA, 0xBB], 0, 0);
        assert_eq!(cmd[0], CH341_CMD_SPI_STREAM);
        assert_eq!(cmd[1], 0xAA);
        assert_eq!(cmd[2], 0xBB);
    }

    #[test]
    fn test_spi_transfer_empty() {
        let cmd = spi_transfer(&[], 0, 0);
        assert_eq!(cmd, vec![CH341_CMD_SPI_STREAM]);
    }

    // ---- device name tests ----

    #[test]
    fn test_i2c_device_name_known() {
        assert_eq!(i2c_device_name(0x50), Some("EEPROM (AT24Cxx)"));
        assert_eq!(i2c_device_name(0x68), Some("RTC (DS3231) / IMU (MPU6050)"));
        assert_eq!(i2c_device_name(0x3C), Some("OLED Display (SSD1306)"));
        assert_eq!(i2c_device_name(0x76), Some("Pressure/Humidity (BME280/BMP280)"));
    }

    #[test]
    fn test_i2c_device_name_unknown() {
        assert!(i2c_device_name(0x01).is_none());
        assert!(i2c_device_name(0x7F).is_none());
    }

    // ---- append_i2c_in_commands unit tests ----

    #[test]
    fn test_append_in_commands_zero() {
        let mut cmd = Vec::new();
        append_i2c_in_commands(&mut cmd, 0);
        assert!(cmd.is_empty());
    }

    #[test]
    fn test_append_in_commands_one() {
        let mut cmd = Vec::new();
        append_i2c_in_commands(&mut cmd, 1);
        assert_eq!(cmd, vec![CH341_I2C_STM_IN]); // 0xC0 = 1 byte NAK
    }

    #[test]
    fn test_append_in_commands_exact_chunk() {
        let mut cmd = Vec::new();
        append_i2c_in_commands(&mut cmd, 32);
        // 32 bytes: IN 31 (ACK) + IN 0 (NAK) = 32
        assert_eq!(cmd.len(), 2);
        assert_eq!(cmd[0], CH341_I2C_STM_IN | 31);
        assert_eq!(cmd[1], CH341_I2C_STM_IN);
    }

    #[test]
    fn test_append_in_commands_two_chunks() {
        let mut cmd = Vec::new();
        append_i2c_in_commands(&mut cmd, 64);
        // 64 bytes: IN 32 (ACK) + IN 31 (ACK) + IN 0 (NAK) = 64
        assert_eq!(cmd.len(), 3);
        assert_eq!(cmd[0], CH341_I2C_STM_IN | 32);
        assert_eq!(cmd[1], CH341_I2C_STM_IN | 31);
        assert_eq!(cmd[2], CH341_I2C_STM_IN);
    }
}
