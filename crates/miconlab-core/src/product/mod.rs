//! 产品模型与协议库模块。
//!
//! 包含产品数据模型、产品协议库以及协议解析器。

pub mod library;
pub mod model;
pub mod protocol;

pub use library::ProductLibrary;
pub use model::*;
pub use protocol::*;
