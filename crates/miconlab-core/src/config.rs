use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub stop_bits: StopBits,
    pub parity: Parity,
    pub flow_control: FlowControl,
    pub timeout_ms: u64,
}

/// 数据位数。derive `Copy` 以避免不必要的克隆。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataBits {
    Five,
    Six,
    Seven,
    Eight,
}

/// 停止位数。derive `Copy` 以避免不必要的克隆。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum StopBits {
    One,
    Two,
}

/// 校验位。derive `Copy` 以避免不必要的克隆。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Parity {
    None,
    Odd,
    Even,
}

/// 流控方式。derive `Copy` 以避免不必要的克隆。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FlowControl {
    None,
    Software,
    Hardware,
}

impl DataBits {
    /// 返回数据位数值（5/6/7/8）。
    pub fn bits(self) -> u8 {
        match self {
            Self::Five => 5,
            Self::Six => 6,
            Self::Seven => 7,
            Self::Eight => 8,
        }
    }
}

impl StopBits {
    /// 返回停止位数值（1 或 2）。
    pub fn bits(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

impl FromStr for DataBits {
    type Err = ConfigParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "5" | "five" => Ok(Self::Five),
            "6" | "six" => Ok(Self::Six),
            "7" | "seven" => Ok(Self::Seven),
            "8" | "eight" => Ok(Self::Eight),
            other => Err(ConfigParseError::invalid("data_bits", other)),
        }
    }
}

impl FromStr for StopBits {
    type Err = ConfigParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "one" => Ok(Self::One),
            "2" | "two" => Ok(Self::Two),
            other => Err(ConfigParseError::invalid("stop_bits", other)),
        }
    }
}

impl FromStr for Parity {
    type Err = ConfigParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "n" | "none" => Ok(Self::None),
            "o" | "odd" => Ok(Self::Odd),
            "e" | "even" => Ok(Self::Even),
            other => Err(ConfigParseError::invalid("parity", other)),
        }
    }
}

impl FromStr for FlowControl {
    type Err = ConfigParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "n" | "none" => Ok(Self::None),
            "x" | "sw" | "software" | "xonxoff" => Ok(Self::Software),
            "h" | "hw" | "hardware" | "rtscts" => Ok(Self::Hardware),
            other => Err(ConfigParseError::invalid("flow_control", other)),
        }
    }
}

/// 配置项字符串解析错误。
#[derive(Debug, thiserror::Error)]
#[error("invalid {field}: {value} (expected one of the documented values)")]
pub struct ConfigParseError {
    pub field: &'static str,
    pub value: String,
}

impl ConfigParseError {
    fn new(field: &'static str, value: impl Into<String>) -> Self {
        Self {
            field,
            value: value.into(),
        }
    }

    /// 便捷构造：以原始字符串形式记录无效值。
    pub fn invalid(field: &'static str, value: &str) -> Self {
        Self::new(field, value)
    }
}

/// 配置文件 IO / 序列化错误。
///
/// 统一 `SerialConfig::to_toml` / `from_toml` / `AppConfig::load` / `save`
/// 的错误类型，替代原先的 `anyhow::Result`，让调用方可以按错误类型分支处理
/// （例如区分"文件不存在"与"格式错误"）。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("TOML deserialization error: {0}")]
    Deserialize(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub serial: SerialConfig,
    pub log_dir: PathBuf,
    pub auto_reconnect: bool,
    pub hex_mode: bool,
    pub timestamp_logs: bool,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115200,
            data_bits: DataBits::Eight,
            stop_bits: StopBits::One,
            parity: Parity::None,
            flow_control: FlowControl::None,
            timeout_ms: 1000,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            serial: SerialConfig::default(),
            log_dir: PathBuf::from("logs"),
            auto_reconnect: true,
            hex_mode: false,
            timestamp_logs: true,
        }
    }
}

impl SerialConfig {
    pub fn new(port_name: impl Into<String>) -> Self {
        Self {
            port_name: port_name.into(),
            ..Default::default()
        }
    }

    pub fn with_baud_rate(mut self, baud_rate: u32) -> Self {
        self.baud_rate = baud_rate;
        self
    }

    pub fn with_data_bits(mut self, data_bits: DataBits) -> Self {
        self.data_bits = data_bits;
        self
    }

    pub fn with_stop_bits(mut self, stop_bits: StopBits) -> Self {
        self.stop_bits = stop_bits;
        self
    }

    pub fn with_parity(mut self, parity: Parity) -> Self {
        self.parity = parity;
        self
    }

    pub fn with_flow_control(mut self, flow_control: FlowControl) -> Self {
        self.flow_control = flow_control;
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(s)?)
    }
}

impl AppConfig {
    pub fn load(path: &PathBuf) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save(&self, path: &PathBuf) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_serial_config() {
        let config = SerialConfig::default();
        assert_eq!(config.baud_rate, 115200);
        assert_eq!(config.data_bits, DataBits::Eight);
        assert_eq!(config.stop_bits, StopBits::One);
        assert_eq!(config.parity, Parity::None);
        assert_eq!(config.flow_control, FlowControl::None);
    }

    #[test]
    fn test_serial_config_builder() {
        let config = SerialConfig::new("COM1")
            .with_baud_rate(9600)
            .with_data_bits(DataBits::Seven)
            .with_stop_bits(StopBits::Two)
            .with_parity(Parity::Even)
            .with_flow_control(FlowControl::Hardware);

        assert_eq!(config.port_name, "COM1");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.data_bits, DataBits::Seven);
        assert_eq!(config.stop_bits, StopBits::Two);
        assert_eq!(config.parity, Parity::Even);
        assert_eq!(config.flow_control, FlowControl::Hardware);
    }

    #[test]
    fn test_serial_config_toml_roundtrip() {
        let config = SerialConfig::new("/dev/ttyUSB0")
            .with_baud_rate(115200)
            .with_parity(Parity::Odd);

        let toml_str = config.to_toml().unwrap();
        let restored = SerialConfig::from_toml(&toml_str).unwrap();

        assert_eq!(config.port_name, restored.port_name);
        assert_eq!(config.baud_rate, restored.baud_rate);
        assert_eq!(config.parity, restored.parity);
    }

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert_eq!(config.serial.baud_rate, 115200);
        assert!(config.auto_reconnect);
        assert!(!config.hex_mode);
        assert!(config.timestamp_logs);
    }

    // ---- FromStr / Copy 测试 ----

    #[test]
    fn test_data_bits_from_str_variants() {
        assert_eq!(DataBits::from_str("5").unwrap(), DataBits::Five);
        assert_eq!(DataBits::from_str("6").unwrap(), DataBits::Six);
        assert_eq!(DataBits::from_str("7").unwrap(), DataBits::Seven);
        assert_eq!(DataBits::from_str("8").unwrap(), DataBits::Eight);
        // 大小写不敏感 + 空格容忍
        assert_eq!(DataBits::from_str("  Eight  ").unwrap(), DataBits::Eight);
        assert_eq!(DataBits::from_str("FIVE").unwrap(), DataBits::Five);
    }

    #[test]
    fn test_data_bits_from_str_invalid() {
        assert!(DataBits::from_str("9").is_err());
        assert!(DataBits::from_str("abc").is_err());
        assert!(DataBits::from_str("").is_err());
    }

    #[test]
    fn test_stop_bits_from_str_variants() {
        assert_eq!(StopBits::from_str("1").unwrap(), StopBits::One);
        assert_eq!(StopBits::from_str("2").unwrap(), StopBits::Two);
        assert_eq!(StopBits::from_str("one").unwrap(), StopBits::One);
        assert_eq!(StopBits::from_str("TWO").unwrap(), StopBits::Two);
    }

    #[test]
    fn test_stop_bits_from_str_invalid() {
        assert!(StopBits::from_str("1.5").is_err());
        assert!(StopBits::from_str("3").is_err());
    }

    #[test]
    fn test_parity_from_str_variants() {
        assert_eq!(Parity::from_str("n").unwrap(), Parity::None);
        assert_eq!(Parity::from_str("none").unwrap(), Parity::None);
        assert_eq!(Parity::from_str("o").unwrap(), Parity::Odd);
        assert_eq!(Parity::from_str("odd").unwrap(), Parity::Odd);
        assert_eq!(Parity::from_str("e").unwrap(), Parity::Even);
        assert_eq!(Parity::from_str("even").unwrap(), Parity::Even);
        assert_eq!(Parity::from_str("  NONE  ").unwrap(), Parity::None);
    }

    #[test]
    fn test_parity_from_str_invalid() {
        assert!(Parity::from_str("mark").is_err());
        assert!(Parity::from_str("space").is_err());
        assert!(Parity::from_str("xyz").is_err());
    }

    #[test]
    fn test_flow_control_from_str_variants() {
        assert_eq!(FlowControl::from_str("n").unwrap(), FlowControl::None);
        assert_eq!(
            FlowControl::from_str("none").unwrap(),
            FlowControl::None
        );
        assert_eq!(
            FlowControl::from_str("x").unwrap(),
            FlowControl::Software
        );
        assert_eq!(
            FlowControl::from_str("software").unwrap(),
            FlowControl::Software
        );
        assert_eq!(
            FlowControl::from_str("xonxoff").unwrap(),
            FlowControl::Software
        );
        assert_eq!(
            FlowControl::from_str("h").unwrap(),
            FlowControl::Hardware
        );
        assert_eq!(
            FlowControl::from_str("hardware").unwrap(),
            FlowControl::Hardware
        );
        assert_eq!(
            FlowControl::from_str("rtscts").unwrap(),
            FlowControl::Hardware
        );
    }

    #[test]
    fn test_flow_control_from_str_invalid() {
        assert!(FlowControl::from_str("auto").is_err());
        assert!(FlowControl::from_str("").is_err());
    }

    #[test]
    fn test_data_bits_bits() {
        assert_eq!(DataBits::Five.bits(), 5);
        assert_eq!(DataBits::Six.bits(), 6);
        assert_eq!(DataBits::Seven.bits(), 7);
        assert_eq!(DataBits::Eight.bits(), 8);
    }

    #[test]
    fn test_stop_bits_bits() {
        assert_eq!(StopBits::One.bits(), 1);
        assert_eq!(StopBits::Two.bits(), 2);
    }

    #[test]
    fn test_enum_copy_semantics() {
        // Copy 语义：赋值无需 clone
        let a = DataBits::Eight;
        let b = a;
        assert_eq!(a, b); // a 仍可用，证明 Copy

        let p = Parity::Odd;
        let q = p;
        assert_eq!(p, q);

        let s = StopBits::Two;
        let t = s;
        assert_eq!(s, t);

        let f = FlowControl::Hardware;
        let g = f;
        assert_eq!(f, g);
    }

    #[test]
    fn test_config_parse_error_message() {
        let err = DataBits::from_str("invalid").unwrap_err();
        assert_eq!(err.field, "data_bits");
        assert_eq!(err.value, "invalid");
        let msg = err.to_string();
        assert!(msg.contains("data_bits"));
        assert!(msg.contains("invalid"));
    }
}
