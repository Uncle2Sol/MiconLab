//! 三类自动识别任务：通讯芯片 / bin 固件 / 上位机协议（feature `ollama`）
//!
//! 每个任务 = 证据包采集（`EvidenceBundle`）+ Prompt 模板（`prompts.rs`）
//! + JSON Schema 约束（`ollama.rs` 的 `format` 字段）+ 结果交叉验证（`cross_check`）。
//!
//! 调用流程（全自动流水线）：
//! 1. 规则引擎先行（毫秒级，零成本）
//! 2. 置信度 < 0.7 且 Ollama 就绪 → 后台线程调用本模块的 `identify_*` 函数
//! 3. 结果带 `source = Ollama` 标记回写 UI；失败/超时静默保持"未知"
//!
//! 所有推理纯本地完成（localhost），无任何外部网络请求。

use crate::features::StreamFeatures;
use crate::ollama::{OllamaClient, OllamaError};
use crate::prompts;
use serde::{Deserialize, Deserializer, Serialize};

/// 识别目标类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyTarget {
    /// 通讯芯片（USB 桥接芯片 / MCU）
    CommChip,
    /// bin 固件（架构 / 基地址 / 芯片族）
    Firmware,
    /// 上位机协议（帧格式 / 波特率）
    HostProtocol,
}

impl IdentifyTarget {
    /// 目标中文名（用于证据包标题）
    pub fn label_zh(self) -> &'static str {
        match self {
            IdentifyTarget::CommChip => "Comm chip identification",
            IdentifyTarget::Firmware => "Firmware identification",
            IdentifyTarget::HostProtocol => "Host protocol identification",
        }
    }
}

/// 识别结果来源标记（UI 区分展示用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IdentifySource {
    /// 本地规则引擎
    #[default]
    Rule,
    /// Ollama 本地大模型
    Ollama,
}

/// 统一证据包：规则引擎的中间产物全部喂给 LLM，避免重复采集
#[derive(Debug, Clone, Default)]
pub struct EvidenceBundle {
    /// USB 桥接芯片 VID:PID
    pub vid_pid: Option<(u16, u16)>,
    /// 端口名（如 cu.usbserial-XXX / COM3）
    pub port_name: Option<String>,
    /// 最近 RX 样本 HEX（建议 ≤256 字节，空格分隔）
    pub sample_hex: String,
    /// 流统计特征（熵/可打印率/帧间隔等）
    pub features: Option<StreamFeatures>,
    /// 主动探测响应：(探测命令名, 响应 HEX)
    pub probe_responses: Vec<(String, String)>,
    /// 规则引擎初步结论 + 置信度
    pub rule_hint: Option<(String, f32)>,
    /// 补充说明（固件大小/架构摘要/提取的字符串等，每行一条）
    pub extra_notes: Vec<String>,
}

impl EvidenceBundle {
    pub fn new() -> Self {
        Self::default()
    }

    /// 序列化为 LLM 可读的证据文本（user 消息正文）
    pub fn render(&self, target: IdentifyTarget) -> String {
        let mut s = String::with_capacity(1024);
        s.push_str(&format!("[Evidence bundle: {}]\n", target.label_zh()));

        if let Some((vid, pid)) = self.vid_pid {
            s.push_str(&format!("- USB VID:PID = {:04X}:{:04X}\n", vid, pid));
        }
        if let Some(p) = &self.port_name {
            s.push_str(&format!("- Port name = {}\n", p));
        }
        for (name, resp) in &self.probe_responses {
            s.push_str(&format!(
                "- Probe response [{}] = {}\n",
                name,
                truncate_chars(resp, 256)
            ));
        }
        if let Some((label, conf)) = &self.rule_hint {
            s.push_str(&format!(
                "- Rule engine initial guess = {} (confidence {:.0}%). This is a reference; you may override it if evidence does not support it, but explain the reason in reasoning\n",
                label,
                conf * 100.0
            ));
        }
        for note in &self.extra_notes {
            s.push_str(&format!("- Note: {}\n", truncate_chars(note, 256)));
        }
        if let Some(f) = &self.features {
            s.push_str(&format!(
                "- Stream features: entropy={:.2} bits, printable ratio={:.2}, null ratio={:.2}, high-byte ratio={:.2}, control ratio={:.2}, data length={}\n",
                f.shannon_entropy,
                f.printable_ratio,
                f.null_ratio,
                f.high_byte_ratio,
                f.control_ratio,
                f.data_length
            ));
            if f.frame_interval_mean_ms > 0.0 {
                s.push_str(&format!(
                    "- Frame interval: mean={:.1}ms, coefficient of variation={:.2}\n",
                    f.frame_interval_mean_ms, f.frame_interval_cv
                ));
            }
            if !f.detected_headers.is_empty() {
                let headers: Vec<&str> =
                    f.detected_headers.iter().map(|h| h.label_zh()).collect();
                s.push_str(&format!("- Detected known headers: {}\n", headers.join(", ")));
            }
        }
        if !self.sample_hex.is_empty() {
            s.push_str(&format!(
                "- Data sample (HEX):\n{}\n",
                truncate_chars(&self.sample_hex, 1024)
            ));
        }
        s
    }
}

// ============ 识别结果结构 ============

/// 通讯芯片识别结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipIdentifyResult {
    /// 芯片型号（如 "CH340C" / "CP2102N" / "STM32F103C8"）
    pub chip: String,
    /// 厂商（如 "WCH" / "Silicon Labs" / "ST"）
    pub vendor: String,
    /// 芯片族（如 "usb-uart" / "cortex-m3"）
    pub family: String,
    /// 置信度 0.0-1.0
    pub confidence: f32,
    /// 中文推理解释（展示给用户）
    pub reasoning: String,
    /// 建议驱动（可选）
    pub suggested_driver: Option<String>,
    /// 结果来源（反序列化后为 Rule，identify_chip 成功时覆写为 Ollama）
    #[serde(skip)]
    pub source: IdentifySource,
}

/// bin 固件识别结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirmwareIdentifyResult {
    /// 架构（如 "cortex-m3" / "xtensa-lx6" / "riscv32"）
    pub arch: String,
    /// Flash 基地址（接受数字或 "0x..." 字符串）
    #[serde(deserialize_with = "de_u32_flexible")]
    pub base_addr: u32,
    /// 入口地址（可选）
    #[serde(default, deserialize_with = "de_opt_u32_flexible")]
    pub entry: Option<u32>,
    /// 芯片族（如 "stm32f1" / "esp32"）
    pub chip_family: Option<String>,
    /// 置信度 0.0-1.0
    pub confidence: f32,
    /// 中文推理解释
    pub reasoning: String,
    /// 结果来源
    #[serde(skip)]
    pub source: IdentifySource,
}

/// 上位机协议识别结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolIdentifyResult {
    /// 协议名（如 "modbus-rtu" / "at-command" / "自定义帧"）
    pub protocol: String,
    /// 帧格式描述（如 "AA 55 <len> <payload> <crc16>"）
    pub frame_format: Option<String>,
    /// 建议波特率
    pub suggested_baud: Option<u32>,
    /// 置信度 0.0-1.0
    pub confidence: f32,
    /// 中文推理解释
    pub reasoning: String,
    /// 结果来源
    #[serde(skip)]
    pub source: IdentifySource,
}

// ============ JSON Schema ============

/// 通讯芯片识别 schema
pub fn chip_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "chip": {"type": "string", "description": "Chip model, e.g. CH340C"},
            "vendor": {"type": "string", "description": "Vendor, e.g. WCH"},
            "family": {"type": "string", "description": "Chip family, e.g. usb-uart"},
            "confidence": {"type": "number", "description": "0.0-1.0"},
            "reasoning": {"type": "string", "description": "Reasoning explanation"},
            "suggested_driver": {"type": ["string", "null"]}
        },
        "required": ["chip", "vendor", "family", "confidence", "reasoning"]
    })
}

/// 固件识别 schema
pub fn firmware_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "arch": {"type": "string", "description": "Architecture, e.g. cortex-m3 / xtensa-lx6 / riscv32"},
            "base_addr": {"type": ["integer", "string"], "description": "Flash base address, integer or 0x-prefixed hex"},
            "entry": {"type": ["integer", "string", "null"], "description": "Entry address"},
            "chip_family": {"type": ["string", "null"], "description": "Chip family, e.g. stm32f1 / esp32"},
            "confidence": {"type": "number"},
            "reasoning": {"type": "string", "description": "Reasoning explanation"}
        },
        "required": ["arch", "base_addr", "confidence", "reasoning"]
    })
}

/// 协议识别 schema
pub fn protocol_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "protocol": {"type": "string", "description": "Protocol name, e.g. modbus-rtu / at-command / custom frame"},
            "frame_format": {"type": ["string", "null"], "description": "Frame format description"},
            "suggested_baud": {"type": ["integer", "null"]},
            "confidence": {"type": "number"},
            "reasoning": {"type": "string", "description": "Reasoning explanation"}
        },
        "required": ["protocol", "confidence", "reasoning"]
    })
}

// ============ 识别任务入口 ============

/// 通讯芯片识别（后台线程调用，阻塞式）
///
/// 成功时结果 `source` 自动标记为 `Ollama`。
pub fn identify_chip(
    client: &OllamaClient,
    evidence: &EvidenceBundle,
) -> Result<ChipIdentifyResult, OllamaError> {
    let mut r: ChipIdentifyResult = client.identify_blocking(
        &prompts::ollama_system_chip_identify(),
        &evidence.render(IdentifyTarget::CommChip),
        &chip_schema(),
    )?;
    r.source = IdentifySource::Ollama;
    Ok(r)
}

/// bin 固件识别（后台线程调用，阻塞式）
pub fn identify_firmware(
    client: &OllamaClient,
    evidence: &EvidenceBundle,
) -> Result<FirmwareIdentifyResult, OllamaError> {
    let mut r: FirmwareIdentifyResult = client.identify_blocking(
        &prompts::ollama_system_firmware_identify(),
        &evidence.render(IdentifyTarget::Firmware),
        &firmware_schema(),
    )?;
    r.source = IdentifySource::Ollama;
    Ok(r)
}

/// 上位机协议识别（后台线程调用，阻塞式）
pub fn identify_protocol(
    client: &OllamaClient,
    evidence: &EvidenceBundle,
) -> Result<ProtocolIdentifyResult, OllamaError> {
    let mut r: ProtocolIdentifyResult = client.identify_blocking(
        &prompts::ollama_system_protocol_identify(),
        &evidence.render(IdentifyTarget::HostProtocol),
        &protocol_schema(),
    )?;
    r.source = IdentifySource::Ollama;
    Ok(r)
}

// ============ 交叉验证 ============

/// 规则引擎 hint 与 LLM 结论的一致性
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossCheck {
    /// 一致（LLM 结论与规则 hint 指向同一对象）
    Agree,
    /// 冲突（UI 应并列展示两者供用户判断）
    Conflict,
    /// 无规则 hint（纯 LLM 结论）
    NoRuleHint,
}

/// 交叉验证：比较规则引擎初判与 LLM 结论是否指向同一对象。
///
/// 实现：双方小写化后做双向包含检查（容忍 "CH340" vs "CH340C" / "modbus-rtu" vs "Modbus RTU"）。
pub fn cross_check(rule_hint: &Option<(String, f32)>, llm_label: &str) -> CrossCheck {
    let Some((hint, _)) = rule_hint else {
        return CrossCheck::NoRuleHint;
    };
    let norm = |s: &str| -> String {
        s.to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect()
    };
    let h = norm(hint);
    let l = norm(llm_label);
    if h.is_empty() || l.is_empty() {
        return CrossCheck::NoRuleHint;
    }
    if h.contains(&l) || l.contains(&h) {
        CrossCheck::Agree
    } else {
        CrossCheck::Conflict
    }
}

// ============ 内部工具 ============

/// 宽松 u32 反序列化：接受 JSON 数字、"0x..." 十六进制字符串、十进制字符串
fn de_u32_flexible<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    parse_u32_value(&v).ok_or_else(|| serde::de::Error::custom(format!("Cannot parse as u32: {}", v)))
}

/// 宽松 Option<u32> 反序列化（null / 缺失 → None，无法解析也按 None 处理，不卡识别）
fn de_opt_u32_flexible<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    Ok(parse_u32_value(&v))
}

fn parse_u32_value(v: &serde_json::Value) -> Option<u32> {
    match v {
        serde_json::Value::Number(n) => n.as_u64().and_then(|x| u32::try_from(x).ok()),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u32::from_str_radix(hex, 16).ok()
            } else {
                s.parse::<u32>().ok()
            }
        }
        _ => None,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::KnownHeader;

    #[test]
    fn test_render_chip_evidence() {
        let mut ev = EvidenceBundle::new();
        ev.vid_pid = Some((0x1A86, 0x7523));
        ev.port_name = Some("cu.usbserial-1440".to_string());
        ev.rule_hint = Some(("CH340".to_string(), 0.85));
        ev.probe_responses
            .push(("AT".to_string(), "OK".to_string()));
        let text = ev.render(IdentifyTarget::CommChip);
        assert!(text.contains("Comm chip identification"));
        assert!(text.contains("1A86:7523"));
        assert!(text.contains("cu.usbserial-1440"));
        assert!(text.contains("CH340"));
        assert!(text.contains("85%"));
        assert!(text.contains("Probe response [AT]"));
    }

    #[test]
    fn test_render_with_features() {
        let mut ev = EvidenceBundle::new();
        let data = [0xAA, 0x55, 0x03, 0x99, 0x01, 0x02];
        ev.features = Some(StreamFeatures::extract(&data, Some(&[10.0, 12.0])));
        let text = ev.render(IdentifyTarget::HostProtocol);
        assert!(text.contains("entropy="));
        assert!(text.contains("Frame interval"));
        assert!(text.contains("Insole sensor header"));
    }

    #[test]
    fn test_render_empty_bundle() {
        let ev = EvidenceBundle::new();
        let text = ev.render(IdentifyTarget::Firmware);
        assert!(text.contains("Firmware identification"));
        assert!(!text.contains("VID:PID"));
        assert!(!text.contains("Stream features"));
    }

    #[test]
    fn test_flexible_u32_from_number() {
        let r: FirmwareIdentifyResult = serde_json::from_str(
            r#"{"arch":"cortex-m3","base_addr":134217728,"entry":null,"chip_family":"stm32f1","confidence":0.9,"reasoning":"向量表"}"#,
        )
        .unwrap();
        assert_eq!(r.base_addr, 0x0800_0000);
        assert_eq!(r.entry, None);
    }

    #[test]
    fn test_flexible_u32_from_hex_string() {
        let r: FirmwareIdentifyResult = serde_json::from_str(
            r#"{"arch":"xtensa-lx6","base_addr":"0x1000","entry":"0x40080400","chip_family":"esp32","confidence":0.8,"reasoning":"E9 魔数"}"#,
        )
        .unwrap();
        assert_eq!(r.base_addr, 0x1000);
        assert_eq!(r.entry, Some(0x4008_0400));
    }

    #[test]
    fn test_flexible_u32_invalid_entry_falls_back_none() {
        // entry 无法解析 → None，不阻断整体识别
        let r: FirmwareIdentifyResult = serde_json::from_str(
            r#"{"arch":"avr","base_addr":"0x0","entry":"unknown","chip_family":null,"confidence":0.5,"reasoning":"r"}"#,
        )
        .unwrap();
        assert_eq!(r.entry, None);
    }

    #[test]
    fn test_flexible_u32_base_addr_invalid() {
        let r = serde_json::from_str::<FirmwareIdentifyResult>(
            r#"{"arch":"avr","base_addr":"n/a","entry":null,"chip_family":null,"confidence":0.5,"reasoning":"r"}"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn test_schema_valid_json() {
        // schema 本身必须是合法 JSON 对象且含 required
        for schema in [chip_schema(), firmware_schema(), protocol_schema()] {
            assert_eq!(schema["type"], "object");
            assert!(schema["required"].is_array());
            assert!(schema["properties"].is_object());
        }
    }

    #[test]
    fn test_source_default_is_rule() {
        let r: ProtocolIdentifyResult = serde_json::from_str(
            r#"{"protocol":"modbus-rtu","frame_format":null,"suggested_baud":9600,"confidence":0.9,"reasoning":"CRC16"}"#,
        )
        .unwrap();
        assert_eq!(r.source, IdentifySource::Rule);
    }

    #[test]
    fn test_cross_check() {
        assert_eq!(
            cross_check(&Some(("CH340".to_string(), 0.8)), "CH340C"),
            CrossCheck::Agree
        );
        assert_eq!(
            cross_check(&Some(("Modbus RTU".to_string(), 0.6)), "modbus-rtu"),
            CrossCheck::Agree
        );
        assert_eq!(
            cross_check(&Some(("CH340".to_string(), 0.8)), "CP2102N"),
            CrossCheck::Conflict
        );
        assert_eq!(cross_check(&None, "CH340"), CrossCheck::NoRuleHint);
    }

    #[test]
    fn test_truncate_chars_multibyte() {
        let s = "固件识别测试abcdef";
        let t = truncate_chars(s, 10);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn test_known_header_render() {
        let h = KnownHeader::EspImageE9;
        assert_eq!(h.label_zh(), "ESP image magic (E9)");
    }
}
