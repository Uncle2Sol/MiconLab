/// Modbus master functionality - re-exports types from modbus module.

use crate::protocol::modbus::{ModbusFrame, ModbusFunction};
use crate::protocol::modbus_common;
use std::time::Duration;

/// Errors that can occur during Modbus master operations.
#[derive(Debug)]
pub enum ModbusMasterError {
    PortError(String),
    Timeout,
    Exception(u8),
}

impl std::fmt::Display for ModbusMasterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortError(e) => write!(f, "Port error: {}", e),
            Self::Timeout => write!(f, "Timeout"),
            Self::Exception(code) => write!(f, "Exception: {}", code),
        }
    }
}

impl std::error::Error for ModbusMasterError {}

pub type ModbusMasterResult<T> = Result<T, ModbusMasterError>;

/// A Modbus master for sending requests and receiving responses.
///
/// 提供帧构建、响应解析和通过 SerialPort 实际收发的完整功能。
pub struct ModbusMaster {
    slave_id: u8,
    timeout: Duration,
}

impl ModbusMaster {
    /// Create a new ModbusMaster with the given slave ID.
    pub fn new(slave_id: u8) -> Self {
        Self {
            slave_id,
            timeout: Duration::from_secs(1),
        }
    }

    /// Set the timeout for Modbus transactions.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get the current slave ID.
    pub fn slave_id(&self) -> u8 {
        self.slave_id
    }

    /// Get the current timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// 通过 SerialPort 发送 Modbus 请求帧并读取响应。
    ///
    /// 发送后会临时设置长超时读取响应，读取完成后恢复 10ms 短超时。
    /// 返回完整的响应字节（含 CRC）。
    pub fn execute(
        &self,
        port: &mut crate::SerialPort,
        frame: &ModbusFrame,
    ) -> ModbusMasterResult<Vec<u8>> {
        if !port.is_connected() {
            return Err(ModbusMasterError::PortError("Not connected".to_string()));
        }

        let frame_bytes = frame.to_bytes();
        port.write(&frame_bytes)
            .map_err(|e| ModbusMasterError::PortError(e.to_string()))?;

        // 临时设长超时
        let _ = port.set_timeout(self.timeout);
        let mut buf = [0u8; 256];
        let mut response = Vec::new();

        // 循环读取直到收到完整帧或超时
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            if std::time::Instant::now() >= deadline {
                break;
            }
            match port.read(&mut buf) {
                Ok(n) if n > 0 => {
                    response.extend_from_slice(&buf[..n]);
                    // 尝试解析，如果成功则说明已收到完整帧
                    if ModbusFrame::parse(&response).is_ok() {
                        break;
                    }
                }
                _ => break,
            }
        }
        // 恢复短超时
        let _ = port.set_timeout(Duration::from_millis(10));

        if response.is_empty() {
            return Err(ModbusMasterError::Timeout);
        }

        Ok(response)
    }

    /// 解析 Modbus RTU 响应，返回寄存器值列表。
    ///
    /// 对于读寄存器响应（FC 0x03/0x04），返回每个寄存器的 u16 值（大端）。
    /// 对于异常响应（FC | 0x80），返回 `Exception` 错误。
    /// 对于其他功能码响应，返回空 Vec。
    pub fn parse_response(&self, data: &[u8]) -> ModbusMasterResult<Vec<u16>> {
        let frame = ModbusFrame::parse(data).map_err(|e| {
            ModbusMasterError::PortError(format!("Parse error: {}", e))
        })?;

        // 检查是否为异常响应
        let func_code = frame.function.to_code();
        if func_code & 0x80 != 0 {
            // 异常响应，data[0] 为异常码
            let exception_code = frame.data.first().copied().unwrap_or(0);
            return Err(ModbusMasterError::Exception(exception_code));
        }

        // 检查 slave_id 是否匹配
        if frame.slave_id != self.slave_id {
            return Err(ModbusMasterError::PortError(format!(
                "Slave ID mismatch: expected {}, got {}",
                self.slave_id, frame.slave_id
            )));
        }

        // 对于读寄存器响应（FC 0x03/0x04），解析寄存器值
        match func_code {
            0x03 | 0x04 => {
                // data 格式: [byte_count][reg_value_hi][reg_value_lo]...
                if frame.data.is_empty() {
                    return Ok(Vec::new());
                }
                let byte_count = frame.data[0] as usize;
                let reg_data = &frame.data[1..1 + byte_count.min(frame.data.len() - 1)];
                let mut values = Vec::with_capacity(byte_count / 2);
                for chunk in reg_data.chunks(2) {
                    if chunk.len() == 2 {
                        values.push(u16::from_be_bytes([chunk[0], chunk[1]]));
                    }
                }
                Ok(values)
            }
            // 其他功能码响应（写操作等），返回空
            _ => Ok(Vec::new()),
        }
    }

    /// 便捷方法：发送读保持寄存器请求并返回解析后的寄存器值。
    pub fn read_holding_registers(
        &self,
        port: &mut crate::SerialPort,
        start_addr: u16,
        quantity: u16,
    ) -> ModbusMasterResult<Vec<u16>> {
        let frame = self.build_read_holding_registers(start_addr, quantity);
        let response = self.execute(port, &frame)?;
        self.parse_response(&response)
    }

    /// 便捷方法：发送读输入寄存器请求并返回解析后的寄存器值。
    pub fn read_input_registers(
        &self,
        port: &mut crate::SerialPort,
        start_addr: u16,
        quantity: u16,
    ) -> ModbusMasterResult<Vec<u16>> {
        let frame = self.build_read_input_registers(start_addr, quantity);
        let response = self.execute(port, &frame)?;
        self.parse_response(&response)
    }

    /// 便捷方法：发送写单个寄存器请求并验证响应。
    pub fn write_single_register(
        &self,
        port: &mut crate::SerialPort,
        addr: u16,
        value: u16,
    ) -> ModbusMasterResult<()> {
        let frame = self.build_write_single_register(addr, value);
        let response = self.execute(port, &frame)?;
        self.parse_response(&response)?;
        Ok(())
    }

    /// Build a read coils request (FC 0x01).
    pub fn build_read_coils(&self, start_addr: u16, quantity: u16) -> ModbusFrame {
        modbus_common::build_read_frame(
            self.slave_id,
            ModbusFunction::ReadCoils,
            start_addr,
            quantity,
        )
    }

    /// Build a read discrete inputs request (FC 0x02).
    pub fn build_read_discrete_inputs(&self, start_addr: u16, quantity: u16) -> ModbusFrame {
        modbus_common::build_read_frame(
            self.slave_id,
            ModbusFunction::ReadDiscreteInputs,
            start_addr,
            quantity,
        )
    }

    /// Build a read holding registers request (FC 0x03).
    pub fn build_read_holding_registers(
        &self,
        start_addr: u16,
        quantity: u16,
    ) -> ModbusFrame {
        modbus_common::build_read_frame(
            self.slave_id,
            ModbusFunction::ReadHoldingRegisters,
            start_addr,
            quantity,
        )
    }

    /// Build a read input registers request (FC 0x04).
    pub fn build_read_input_registers(
        &self,
        start_addr: u16,
        quantity: u16,
    ) -> ModbusFrame {
        modbus_common::build_read_frame(
            self.slave_id,
            ModbusFunction::ReadInputRegisters,
            start_addr,
            quantity,
        )
    }

    /// Build a write single coil request (FC 0x05).
    pub fn build_write_single_coil(&self, addr: u16, on: bool) -> ModbusFrame {
        modbus_common::build_write_single_coil_frame(self.slave_id, addr, on)
    }

    /// Build a write single register request (FC 0x06).
    pub fn build_write_single_register(&self, addr: u16, value: u16) -> ModbusFrame {
        modbus_common::build_write_single_register_frame(self.slave_id, addr, value)
    }

    /// Build a write multiple coils request (FC 0x0F).
    pub fn build_write_multiple_coils(
        &self,
        start_addr: u16,
        coils: &[bool],
    ) -> Result<ModbusFrame, &'static str> {
        modbus_common::build_write_multiple_coils_frame(self.slave_id, start_addr, coils)
    }

    /// Build a write multiple registers request (FC 0x10).
    ///
    /// Returns `Err` if `values` exceeds the Modbus spec limit of 123 registers.
    pub fn build_write_multiple_registers(
        &self,
        start_addr: u16,
        values: &[u16],
    ) -> Result<ModbusFrame, &'static str> {
        modbus_common::build_write_multiple_registers_frame(self.slave_id, start_addr, values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modbus_master_new() {
        let master = ModbusMaster::new(0x01);
        assert_eq!(master.slave_id(), 0x01);
        assert_eq!(master.timeout(), Duration::from_secs(1));
    }

    #[test]
    fn test_modbus_master_timeout() {
        let master = ModbusMaster::new(0x01).with_timeout(Duration::from_secs(5));
        assert_eq!(master.timeout(), Duration::from_secs(5));
    }

    #[test]
    fn test_build_read_holding_registers() {
        let master = ModbusMaster::new(0x01);
        let frame = master.build_read_holding_registers(0x0000, 0x000A);

        assert_eq!(frame.slave_id, 0x01);
        assert_eq!(frame.function, ModbusFunction::ReadHoldingRegisters);
        assert_eq!(frame.data, vec![0x00, 0x00, 0x00, 0x0A]);
    }

    #[test]
    fn test_build_read_input_registers() {
        let master = ModbusMaster::new(0x02);
        let frame = master.build_read_input_registers(0x0100, 0x0005);

        assert_eq!(frame.slave_id, 0x02);
        assert_eq!(frame.function, ModbusFunction::ReadInputRegisters);
        assert_eq!(frame.data, vec![0x01, 0x00, 0x00, 0x05]);
    }

    #[test]
    fn test_build_read_coils() {
        let master = ModbusMaster::new(0x01);
        let frame = master.build_read_coils(0x00C0, 0x0008);
        assert_eq!(frame.function, ModbusFunction::ReadCoils);
        assert_eq!(frame.data, vec![0x00, 0xC0, 0x00, 0x08]);
    }

    #[test]
    fn test_build_read_discrete_inputs() {
        let master = ModbusMaster::new(0x01);
        let frame = master.build_read_discrete_inputs(0x0000, 0x0001);
        assert_eq!(frame.function, ModbusFunction::ReadDiscreteInputs);
    }

    #[test]
    fn test_build_write_single_register() {
        let master = ModbusMaster::new(0x01);
        let frame = master.build_write_single_register(0x0010, 0x1234);

        assert_eq!(frame.slave_id, 0x01);
        assert_eq!(frame.function, ModbusFunction::WriteSingleRegister);
        assert_eq!(frame.data, vec![0x00, 0x10, 0x12, 0x34]);
    }

    #[test]
    fn test_build_write_single_coil() {
        let master = ModbusMaster::new(0x01);
        let frame_on = master.build_write_single_coil(0x0001, true);
        assert_eq!(frame_on.function, ModbusFunction::WriteSingleCoil);
        assert_eq!(frame_on.data, vec![0x00, 0x01, 0xFF, 0x00]);

        let frame_off = master.build_write_single_coil(0x0001, false);
        assert_eq!(frame_off.data, vec![0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn test_build_write_multiple_registers() {
        let master = ModbusMaster::new(0x01);
        let frame = master.build_write_multiple_registers(0x0000, &[0x1234, 0x5678]).unwrap();

        assert_eq!(frame.slave_id, 0x01);
        assert_eq!(frame.function, ModbusFunction::WriteMultipleRegisters);
        // start_addr(2) + quantity(2) + byte_count(1) + values(4) = 9 bytes
        // Previously this was 7 bytes due to a bug: quantity was 1 byte and
        // byte_count was missing.
        assert_eq!(frame.data.len(), 9);
        assert_eq!(&frame.data[0..2], &[0x00, 0x00]); // start addr
        assert_eq!(&frame.data[2..4], &[0x00, 0x02]); // quantity = 2
        assert_eq!(frame.data[4], 4); // byte count
    }

    #[test]
    fn test_build_write_multiple_registers_exceeds_limit() {
        let master = ModbusMaster::new(0x01);
        let values = vec![0u16; 124];
        assert!(master.build_write_multiple_registers(0x0000, &values).is_err());
    }

    #[test]
    fn test_build_write_multiple_coils() {
        let master = ModbusMaster::new(0x01);
        let frame = master
            .build_write_multiple_coils(0x0000, &[true, false, true, true])
            .unwrap();
        assert_eq!(frame.function, ModbusFunction::WriteMultipleCoils);
        // start(2) + quantity(2) + byte_count(1) + data(1) = 6 bytes
        assert_eq!(frame.data.len(), 6);
        assert_eq!(frame.data[4], 1); // byte count
        // bits: 1,0,1,1,0,0,0,0 = 0x0D
        assert_eq!(frame.data[5], 0x0D);
    }

    #[test]
    fn test_error_display() {
        let e = ModbusMasterError::PortError("test".to_string());
        assert!(format!("{}", e).contains("test"));

        let e = ModbusMasterError::Timeout;
        assert_eq!(format!("{}", e), "Timeout");

        let e = ModbusMasterError::Exception(0x02);
        assert!(format!("{}", e).contains("2"));
    }
}
