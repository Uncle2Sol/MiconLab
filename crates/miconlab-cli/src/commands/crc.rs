//! `miconlab crc` — 计算 CRC/校验和。

use anyhow::Result;
use miconlab_core::checksum;

use crate::util::parse_hex;

pub fn run(algorithm: &str, data_str: &str) -> Result<()> {
    let data = parse_hex(data_str)?;
    let result = match algorithm.to_lowercase().as_str() {
        "crc16-modbus" | "crc16modbus" => {
            let crc = checksum::crc16_modbus(&data);
            format!(
                "CRC16/MODBUS: {:04X} (LE: {:02X} {:02X})",
                crc,
                crc as u8,
                (crc >> 8) as u8
            )
        }
        "crc16-ccitt" | "crc16ccitt" => {
            let crc = checksum::crc16_ccitt(&data);
            format!("CRC16/CCITT: {:04X}", crc)
        }
        "crc16-xmodem" | "crc16xmodem" => {
            let crc = checksum::crc16_xmodem(&data);
            format!("CRC16/XMODEM: {:04X}", crc)
        }
        "crc32" => {
            let crc = checksum::crc32(&data);
            format!("CRC32: {:08X}", crc)
        }
        "lrc" => {
            let lrc = checksum::lrc(&data);
            format!("LRC: {:02X}", lrc)
        }
        "sum8" | "checksum8" => {
            let sum = checksum::checksum8(&data);
            format!("SUM8: {:02X}", sum)
        }
        "sum16" | "checksum16" => {
            let sum = checksum::checksum16(&data);
            format!("SUM16: {:04X}", sum)
        }
        _ => {
            eprintln!(
                "Unknown algorithm: {}. Use crc16-modbus, crc16-ccitt, crc32, lrc, sum8, sum16",
                algorithm
            );
            return Ok(());
        }
    };

    let hex_data = miconlab_core::hex::format_hex_spaced(&data);
    println!("Data:   {}", hex_data);
    println!("Length: {} bytes", data.len());
    println!("{}", result);
    Ok(())
}
