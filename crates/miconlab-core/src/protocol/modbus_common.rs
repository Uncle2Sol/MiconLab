//! Shared Modbus PDU builders and constants.
//!
//! This module centralizes Protocol Data Unit (PDU) construction logic that
//! was previously duplicated between [`super::modbus::ModbusParser`] and
//! [`super::modbus_master::ModbusMaster`]. All multi-byte fields use
//! big-endian encoding per the Modbus specification.

use super::modbus::{ModbusFrame, ModbusFunction};

/// Modbus exception codes (from section 7 of the Modbus specification).
pub mod exception {
    pub const ILLEGAL_FUNCTION: u8 = 0x01;
    pub const ILLEGAL_DATA_ADDRESS: u8 = 0x02;
    pub const ILLEGAL_DATA_VALUE: u8 = 0x03;
    pub const SLAVE_DEVICE_FAILURE: u8 = 0x04;
    pub const ACKNOWLEDGE: u8 = 0x05;
    pub const SLAVE_DEVICE_BUSY: u8 = 0x06;
    pub const MEMORY_PARITY_ERROR: u8 = 0x08;
    pub const GATEWAY_PATH_UNAVAILABLE: u8 = 0x0A;
    pub const GATEWAY_TARGET_NO_RESPONSE: u8 = 0x0B;
}

/// Maximum quantity limits per the Modbus specification.
pub mod limits {
    /// Max coils/discrete inputs per read request (FC 0x01/0x02).
    pub const MAX_READ_BITS: u16 = 2000;
    /// Max registers per read request (FC 0x03/0x04).
    pub const MAX_READ_REGISTERS: u16 = 125;
    /// Max coils per write multiple request (FC 0x0F).
    pub const MAX_WRITE_MULTIPLE_COILS: u16 = 1968;
    /// Max registers per write multiple request (FC 0x10).
    pub const MAX_WRITE_MULTIPLE_REGISTERS: u16 = 123;
}

// ---- PDU builders ----

/// Build a read PDU (FC 0x01-0x04). The PDU contains the function code,
/// starting address (2 bytes BE), and quantity (2 bytes BE).
///
/// Returns only the PDU data (without slave ID and CRC). The caller wraps
/// it in a [`ModbusFrame`] with the appropriate slave ID.
pub fn build_read_pdu(start_addr: u16, quantity: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(4);
    data.extend_from_slice(&start_addr.to_be_bytes());
    data.extend_from_slice(&quantity.to_be_bytes());
    data
}

/// Build a write single register PDU (FC 0x06). PDU = address (2 BE) +
/// value (2 BE).
pub fn build_write_single_register_pdu(addr: u16, value: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(4);
    data.extend_from_slice(&addr.to_be_bytes());
    data.extend_from_slice(&value.to_be_bytes());
    data
}

/// Build a write single coil PDU (FC 0x05). PDU = address (2 BE) +
/// value (2 BE: 0xFF00 for ON, 0x0000 for OFF).
pub fn build_write_single_coil_pdu(addr: u16, on: bool) -> Vec<u8> {
    let mut data = Vec::with_capacity(4);
    data.extend_from_slice(&addr.to_be_bytes());
    let value: u16 = if on { 0xFF00 } else { 0x0000 };
    data.extend_from_slice(&value.to_be_bytes());
    data
}

/// Build a write multiple registers PDU (FC 0x10). PDU = starting address
/// (2 BE) + quantity (2 BE) + byte count (1) + values (N * 2 BE).
///
/// Returns `Err(message)` if `values` exceeds [`limits::MAX_WRITE_MULTIPLE_REGISTERS`].
pub fn build_write_multiple_registers_pdu(
    start_addr: u16,
    values: &[u16],
) -> Result<Vec<u8>, &'static str> {
    if values.len() > limits::MAX_WRITE_MULTIPLE_REGISTERS as usize {
        return Err("quantity exceeds max write multiple registers");
    }
    let n = values.len();
    let mut data = Vec::with_capacity(5 + n * 2);
    data.extend_from_slice(&start_addr.to_be_bytes());
    data.extend_from_slice(&(n as u16).to_be_bytes());
    data.push((n * 2) as u8); // byte count
    for &val in values {
        data.extend_from_slice(&val.to_be_bytes());
    }
    Ok(data)
}

/// Build a write multiple coils PDU (FC 0x0F). PDU = starting address
/// (2 BE) + quantity (2 BE) + byte count (1) + packed coil bytes.
///
/// Returns `Err(message)` if `coils` exceeds [`limits::MAX_WRITE_MULTIPLE_COILS`].
pub fn build_write_multiple_coils_pdu(
    start_addr: u16,
    coils: &[bool],
) -> Result<Vec<u8>, &'static str> {
    if coils.len() > limits::MAX_WRITE_MULTIPLE_COILS as usize {
        return Err("quantity exceeds max write multiple coils");
    }
    let n = coils.len();
    let byte_count = (n + 7) / 8;
    let mut packed = vec![0u8; byte_count];
    for (i, &on) in coils.iter().enumerate() {
        if on {
            packed[i / 8] |= 1 << (i % 8);
        }
    }
    let mut data = Vec::with_capacity(5 + byte_count);
    data.extend_from_slice(&start_addr.to_be_bytes());
    data.extend_from_slice(&(n as u16).to_be_bytes());
    data.push(byte_count as u8);
    data.extend_from_slice(&packed);
    Ok(data)
}

// ---- Convenience: build complete ModbusFrame ----

/// Build a read request frame (FC 0x01-0x04).
pub fn build_read_frame(
    slave_id: u8,
    function: ModbusFunction,
    start_addr: u16,
    quantity: u16,
) -> ModbusFrame {
    ModbusFrame::new(slave_id, function, build_read_pdu(start_addr, quantity))
}

/// Build a write single register frame (FC 0x06).
pub fn build_write_single_register_frame(
    slave_id: u8,
    addr: u16,
    value: u16,
) -> ModbusFrame {
    ModbusFrame::new(
        slave_id,
        ModbusFunction::WriteSingleRegister,
        build_write_single_register_pdu(addr, value),
    )
}

/// Build a write single coil frame (FC 0x05).
pub fn build_write_single_coil_frame(slave_id: u8, addr: u16, on: bool) -> ModbusFrame {
    ModbusFrame::new(
        slave_id,
        ModbusFunction::WriteSingleCoil,
        build_write_single_coil_pdu(addr, on),
    )
}

/// Build a write multiple registers frame (FC 0x10).
pub fn build_write_multiple_registers_frame(
    slave_id: u8,
    start_addr: u16,
    values: &[u16],
) -> Result<ModbusFrame, &'static str> {
    Ok(ModbusFrame::new(
        slave_id,
        ModbusFunction::WriteMultipleRegisters,
        build_write_multiple_registers_pdu(start_addr, values)?,
    ))
}

/// Build a write multiple coils frame (FC 0x0F).
pub fn build_write_multiple_coils_frame(
    slave_id: u8,
    start_addr: u16,
    coils: &[bool],
) -> Result<ModbusFrame, &'static str> {
    Ok(ModbusFrame::new(
        slave_id,
        ModbusFunction::WriteMultipleCoils,
        build_write_multiple_coils_pdu(start_addr, coils)?,
    ))
}

// ---- Response parsing helpers ----

/// Extract register values from a read registers response (FC 0x03/0x04).
/// Response PDU = function(1) + byte_count(1) + data(N*2).
pub fn parse_read_registers_response(pdu: &[u8]) -> Result<Vec<u16>, &'static str> {
    if pdu.len() < 2 {
        return Err("response too short");
    }
    let byte_count = pdu[1] as usize;
    if pdu.len() < 2 + byte_count {
        return Err("response data truncated");
    }
    if byte_count % 2 != 0 {
        return Err("odd byte count in registers response");
    }
    let mut registers = Vec::with_capacity(byte_count / 2);
    for i in 0..byte_count / 2 {
        let offset = 2 + i * 2;
        registers.push(u16::from_be_bytes([pdu[offset], pdu[offset + 1]]));
    }
    Ok(registers)
}

/// Extract coil/discrete input values from a read bits response (FC 0x01/0x02).
/// Response PDU = function(1) + byte_count(1) + packed bits(N).
/// Returns exactly `expected_count` booleans.
pub fn parse_read_bits_response(pdu: &[u8], expected_count: usize) -> Result<Vec<bool>, &'static str> {
    if pdu.len() < 2 {
        return Err("response too short");
    }
    let byte_count = pdu[1] as usize;
    if pdu.len() < 2 + byte_count {
        return Err("response data truncated");
    }
    let mut bits = Vec::with_capacity(expected_count);
    for i in 0..expected_count {
        let byte_idx = 2 + i / 8;
        let bit_idx = i % 8;
        if byte_idx < pdu.len() {
            bits.push((pdu[byte_idx] >> bit_idx) & 1 == 1);
        } else {
            bits.push(false);
        }
    }
    Ok(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_read_pdu() {
        let pdu = build_read_pdu(0x0000, 0x000A);
        assert_eq!(pdu, vec![0x00, 0x00, 0x00, 0x0A]);
    }

    #[test]
    fn test_build_write_single_register_pdu() {
        let pdu = build_write_single_register_pdu(0x0010, 0x1234);
        assert_eq!(pdu, vec![0x00, 0x10, 0x12, 0x34]);
    }

    #[test]
    fn test_build_write_single_coil_pdu_on() {
        let pdu = build_write_single_coil_pdu(0x0001, true);
        assert_eq!(pdu, vec![0x00, 0x01, 0xFF, 0x00]);
    }

    #[test]
    fn test_build_write_single_coil_pdu_off() {
        let pdu = build_write_single_coil_pdu(0x0001, false);
        assert_eq!(pdu, vec![0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn test_build_write_multiple_registers_pdu() {
        let pdu = build_write_multiple_registers_pdu(0x0000, &[0x1234, 0x5678]).unwrap();
        // start(2) + quantity(2) + byte_count(1) + values(4) = 9 bytes
        assert_eq!(pdu.len(), 9);
        assert_eq!(&pdu[0..2], &[0x00, 0x00]); // start addr
        assert_eq!(&pdu[2..4], &[0x00, 0x02]); // quantity = 2
        assert_eq!(pdu[4], 4); // byte count = 4
        assert_eq!(&pdu[5..7], &[0x12, 0x34]); // first value
        assert_eq!(&pdu[7..9], &[0x56, 0x78]); // second value
    }

    #[test]
    fn test_build_write_multiple_registers_pdu_exceeds_limit() {
        let values = vec![0u16; 124];
        let result = build_write_multiple_registers_pdu(0x0000, &values);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_write_multiple_coils_pdu() {
        let pdu = build_write_multiple_coils_pdu(0x0000, &[true, false, true, true, false, false, false, true]).unwrap();
        // start(2) + quantity(2) + byte_count(1) + data(1) = 6 bytes
        assert_eq!(pdu.len(), 6);
        assert_eq!(&pdu[0..2], &[0x00, 0x00]); // start addr
        assert_eq!(&pdu[2..4], &[0x00, 0x08]); // quantity = 8
        assert_eq!(pdu[4], 1); // byte count = 1
        // bits: 1,0,1,1,0,0,0,1 (LSB first) = 0b10001101 = 0x8D
        assert_eq!(pdu[5], 0x8D);
    }

    #[test]
    fn test_build_write_multiple_coils_pdu_partial_byte() {
        let pdu = build_write_multiple_coils_pdu(0x0000, &[true, true, true]).unwrap();
        // 3 coils → 1 byte
        assert_eq!(pdu[4], 1); // byte count
        // bits: 1,1,1,0,0,0,0,0 = 0b00000111 = 0x07
        assert_eq!(pdu[5], 0x07);
    }

    #[test]
    fn test_build_read_frame() {
        let frame = build_read_frame(0x01, ModbusFunction::ReadHoldingRegisters, 0x0000, 0x000A);
        assert_eq!(frame.slave_id, 0x01);
        assert_eq!(frame.function, ModbusFunction::ReadHoldingRegisters);
        assert_eq!(frame.data, vec![0x00, 0x00, 0x00, 0x0A]);
    }

    #[test]
    fn test_parse_read_registers_response() {
        // FC=0x03, byte_count=4, values=[0x1234, 0x5678]
        let pdu = vec![0x03, 0x04, 0x12, 0x34, 0x56, 0x78];
        let regs = parse_read_registers_response(&pdu).unwrap();
        assert_eq!(regs, vec![0x1234, 0x5678]);
    }

    #[test]
    fn test_parse_read_bits_response() {
        // FC=0x01, byte_count=1, data=0x0B (bits: 1,1,0,1,0,0,0,0)
        let pdu = vec![0x01, 0x01, 0x0B];
        let bits = parse_read_bits_response(&pdu, 4).unwrap();
        assert_eq!(bits, vec![true, true, false, true]);
    }

    #[test]
    fn test_parse_read_registers_response_truncated() {
        let pdu = vec![0x03, 0x04, 0x12, 0x34]; // says 4 bytes but only 2
        assert!(parse_read_registers_response(&pdu).is_err());
    }

    #[test]
    fn test_parse_read_registers_response_odd_byte_count() {
        let pdu = vec![0x03, 0x03, 0x12, 0x34, 0x56];
        assert!(parse_read_registers_response(&pdu).is_err());
    }

    #[test]
    fn test_exception_constants() {
        assert_eq!(exception::ILLEGAL_FUNCTION, 0x01);
        assert_eq!(exception::ILLEGAL_DATA_ADDRESS, 0x02);
        assert_eq!(exception::ILLEGAL_DATA_VALUE, 0x03);
    }

    #[test]
    fn test_limits() {
        assert_eq!(limits::MAX_READ_BITS, 2000);
        assert_eq!(limits::MAX_READ_REGISTERS, 125);
        assert_eq!(limits::MAX_WRITE_MULTIPLE_COILS, 1968);
        assert_eq!(limits::MAX_WRITE_MULTIPLE_REGISTERS, 123);
    }
}
