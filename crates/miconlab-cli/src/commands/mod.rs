//! CLI 子命令实现。
//!
//! 每个命令对应一个文件，统一通过 [`crate::util`] 中的辅助函数打开串口、
//! 解析参数、输出 agent JSON，避免样板代码。

pub mod agent;
pub mod connect;
pub mod crc;
pub mod list;
pub mod modbus;
pub mod monitor;
pub mod record;
pub mod replay;
pub mod send;
