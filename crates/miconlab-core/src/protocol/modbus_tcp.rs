/// Modbus TCP frame handling.
///
/// Modbus TCP uses a 7-byte MBAP header instead of CRC:
///   Transaction ID (2 bytes) | Protocol ID (2 bytes, always 0x0000) | Length (2 bytes) | Unit ID (1 byte)
///   Followed by the PDU (function code + data)

use super::modbus::{ModbusFrame, ModbusFunction};

#[derive(Debug, Clone)]
pub struct ModbusTcpFrame {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub function: ModbusFunction,
    pub data: Vec<u8>,
}

impl ModbusTcpFrame {
    pub fn new(transaction_id: u16, unit_id: u8, function: ModbusFunction, data: Vec<u8>) -> Self {
        Self { transaction_id, unit_id, function, data }
    }

    /// Parse a Modbus TCP frame from raw bytes (including MBAP header).
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 7 {
            return Err("Frame too short for MBAP header".into());
        }
        let transaction_id = u16::from_be_bytes([bytes[0], bytes[1]]);
        let protocol_id = u16::from_be_bytes([bytes[2], bytes[3]]);
        if protocol_id != 0 {
            return Err(format!("Unexpected protocol ID: {}", protocol_id));
        }
        let length = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let unit_id = bytes[6];

        // Length field includes unit_id + PDU, so total frame = 6 + length
        if bytes.len() < 6 + length {
            return Err(format!("Frame truncated: expected {} bytes, got {}", 6 + length, bytes.len()));
        }
        if length < 2 {
            return Err("PDU too short".into());
        }

        let function_code = bytes[7];
        let data = bytes[8..6 + length].to_vec();

        Ok(Self {
            transaction_id,
            unit_id,
            function: ModbusFunction::from_code(function_code),
            data,
        })
    }

    /// Serialize to bytes (MBAP header + PDU).
    pub fn to_bytes(&self) -> Vec<u8> {
        let pdu_len = 1 + self.data.len(); // function code + data
        let length = (pdu_len + 1) as u16; // +1 for unit_id

        let mut bytes = Vec::with_capacity(7 + pdu_len);
        bytes.extend_from_slice(&self.transaction_id.to_be_bytes());
        bytes.extend_from_slice(&0x0000u16.to_be_bytes()); // protocol ID
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.push(self.unit_id);
        bytes.push(self.function.to_code());
        bytes.extend_from_slice(&self.data);
        bytes
    }

    /// Convert to RTU frame (strip MBAP header, add CRC).
    pub fn to_rtu_frame(&self) -> ModbusFrame {
        ModbusFrame::new(self.unit_id, self.function.clone(), self.data.clone())
    }

    /// Create from an RTU frame (strip CRC, add MBAP header).
    pub fn from_rtu_frame(rtu: &ModbusFrame, transaction_id: u16) -> Self {
        Self {
            transaction_id,
            unit_id: rtu.slave_id,
            function: rtu.function.clone(),
            data: rtu.data.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_frame_parse_and_serialize() {
        let original = ModbusTcpFrame::new(0x1234, 0x01, ModbusFunction::ReadHoldingRegisters, vec![0x00, 0x00, 0x00, 0x0A]);
        let bytes = original.to_bytes();
        let parsed = ModbusTcpFrame::parse(&bytes).unwrap();

        assert_eq!(parsed.transaction_id, 0x1234);
        assert_eq!(parsed.unit_id, 0x01);
        assert_eq!(parsed.function, ModbusFunction::ReadHoldingRegisters);
        assert_eq!(parsed.data, vec![0x00, 0x00, 0x00, 0x0A]);
    }

    #[test]
    fn test_tcp_to_rtu_conversion() {
        let tcp = ModbusTcpFrame::new(0x0001, 0x02, ModbusFunction::ReadCoils, vec![0x00, 0x00, 0x00, 0x08]);
        let rtu = tcp.to_rtu_frame();
        assert_eq!(rtu.slave_id, 0x02);
        assert_eq!(rtu.function, ModbusFunction::ReadCoils);
        assert_eq!(rtu.data, vec![0x00, 0x00, 0x00, 0x08]);
        // RTU frame should have CRC
        let rtu_bytes = rtu.to_bytes();
        assert!(rtu_bytes.len() > 6);
    }

    #[test]
    fn test_rtu_to_tcp_conversion() {
        let rtu = ModbusFrame::new(0x01, ModbusFunction::WriteSingleRegister, vec![0x00, 0x10, 0x12, 0x34]);
        let tcp = ModbusTcpFrame::from_rtu_frame(&rtu, 0xABCD);
        assert_eq!(tcp.transaction_id, 0xABCD);
        assert_eq!(tcp.unit_id, 0x01);
        assert_eq!(tcp.function, ModbusFunction::WriteSingleRegister);
    }

    #[test]
    fn test_tcp_frame_too_short() {
        assert!(ModbusTcpFrame::parse(&[0x00, 0x01]).is_err());
    }

    #[test]
    fn test_tcp_frame_bad_protocol_id() {
        let bytes = vec![0x00, 0x01, 0x00, 0x01, 0x00, 0x03, 0x01, 0x03, 0x00];
        assert!(ModbusTcpFrame::parse(&bytes).is_err());
    }
}
