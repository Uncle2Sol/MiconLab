//! 在线 LLM 对话客户端（feature `llm-online`）
//!
//! 提供 OpenAI 兼容 API 的对话能力，支持协议逆向辅助、固件分析、自然语言问答。
//!
//! 使用方式：
//! 1. 用户配置 `LlmConfig`（API key、base URL、模型名）
//! 2. 创建 `LlmClient`
//! 3. 调用 `chat_blocking` 发送对话（同步阻塞，建议在后台线程调用）
//!
//! 兼容的 API 提供商：
//! - OpenAI (https://api.openai.com/v1)
//! - 通义千问 (https://dashscope.aliyuncs.com/compatible-mode/v1)
//! - DeepSeek (https://api.deepseek.com/v1)
//! - 本地 Ollama (http://localhost:11434/v1)
//! - 任何 OpenAI 兼容端点

use serde::{Deserialize, Serialize};

/// LLM 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// API Key
    pub api_key: String,
    /// Base URL（如 https://api.openai.com/v1）
    pub base_url: String,
    /// 模型名（如 gpt-4o-mini / qwen-plus / deepseek-chat）
    pub model: String,
    /// 最大输出 token 数
    pub max_tokens: u32,
    /// 温度（0.0-2.0，越高越随机）
    pub temperature: f32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
            max_tokens: 1024,
            temperature: 0.7,
        }
    }
}

impl LlmConfig {
    /// 是否已配置（有 API key）
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty() && !self.base_url.trim().is_empty()
    }
}

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    /// 角色：system / user / assistant
    pub role: String,
    /// 消息内容
    pub content: String,
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// LLM 客户端错误
#[derive(Debug)]
pub enum LlmError {
    /// 配置错误（未配置 API key 等）
    NotConfigured,
    /// 网络错误
    Network(String),
    /// API 返回错误
    ApiError(String),
    /// 响应解析错误
    ParseError(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::NotConfigured => write!(f, "LLM not configured (missing API key)"),
            LlmError::Network(e) => write!(f, "Network error: {}", e),
            LlmError::ApiError(e) => write!(f, "API error: {}", e),
            LlmError::ParseError(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl std::error::Error for LlmError {}

/// LLM 客户端
pub struct LlmClient {
    config: LlmConfig,
    runtime: tokio::runtime::Runtime,
}

impl LlmClient {
    /// 创建客户端
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        if !config.is_configured() {
            return Err(LlmError::NotConfigured);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| LlmError::Network(format!("Failed to create runtime: {}", e)))?;
        Ok(Self { config, runtime })
    }

    /// 同步阻塞式对话
    ///
    /// 在后台线程调用此方法，避免阻塞 GUI 主线程。
    pub fn chat_blocking(&self, messages: &[LlmMessage]) -> Result<String, LlmError> {
        self.runtime.block_on(self.chat_async(messages))
    }

    /// 异步对话
    pub async fn chat_async(&self, messages: &[LlmMessage]) -> Result<String, LlmError> {
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
        });

        // 默认 Client 无超时：端点可连但不响应时会永久阻塞后台线程
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| LlmError::Network(e.to_string()))?;
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| LlmError::Network(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            return Err(LlmError::ApiError(format!("HTTP {}: {}", status, text)));
        }

        // 解析 OpenAI 格式响应
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| LlmError::ParseError(format!("JSON parse failed: {}", e)))?;

        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| LlmError::ParseError("Invalid response format".to_string()))?
            .to_string();

        Ok(content)
    }
}

/// 构建串口协议分析专用的系统提示词
pub fn system_prompt_protocol_analysis(is_zh: bool) -> String {
    if is_zh {
        "You are the AI protocol analysis assistant for miconlab serial tool. The user will provide HEX data captured from serial port, \
         help analyze possible protocol type, frame structure, and field meanings. \
         Analysis points:\n\
         1. Identify header, footer, length field, checksum field\n\
         2. Guess protocol type (Modbus/custom binary/ASCII/HDLC etc.)\n\
         3. Parse possible meanings of data fields (sensor data, status, commands, etc.)\n\
         4. Give protocol reverse engineering suggestions\n\
         Be concise and focused.".to_string()
    } else {
        "You are the AI protocol analysis assistant for miconlab serial tool. \
         The user will provide HEX data captured from serial port. \
         Help analyze possible protocol type, frame structure, and field meanings. \
         Analysis points:\n\
         1. Identify header, footer, length field, checksum field\n\
         2. Guess protocol type (Modbus/custom binary/ASCII/HDLC etc.)\n\
         3. Parse possible meanings of data fields\n\
         4. Give protocol reverse engineering suggestions\n\
         Be concise and focused.".to_string()
    }
}

/// 构建固件分析专用的系统提示词
pub fn system_prompt_firmware_analysis(is_zh: bool) -> String {
    if is_zh {
        "You are the AI firmware analysis assistant for miconlab. The user will provide firmware HEX dump data, \
         help analyze:\n\
         1. Chip type inference (ARM Cortex-M vector table, ESP image header, AVR etc.)\n\
         2. Memory layout (code, data, vector table)\n\
         3. Identifiable strings and symbols\n\
         4. Security features (encryption, checksums)\n\
         Be concise.".to_string()
    } else {
        "You are the AI firmware analysis assistant for miconlab. \
         The user will provide firmware HEX dump data. Help analyze:\n\
         1. Chip type inference (ARM Cortex-M vector table, ESP image header, AVR etc.)\n\
         2. Memory layout (code, data, vector table)\n\
         3. Identifiable strings and symbols\n\
         4. Security features (encryption, checksums)\n\
         Be concise.".to_string()
    }
}

/// 构建 miconlab 项目专项系统提示词（D.3 调优）
///
/// 让 AI 了解当前项目的核心功能模块，回答时能结合项目特性给出针对性建议：
/// - CH340 串扰检测（TX 自发自收识别）
/// - 坐垫 16×16 压力矩阵可视化
/// - STM32F303 固件外设映射反哺诊断
/// - 软件逻辑分析仪（替代 DSLogic）
/// - Agent 7 步闭环诊断
pub fn system_prompt_miconlab_expert(is_zh: bool) -> String {
    if is_zh {
        "You are the AI assistant for miconlab. \
         You understand the project's core modules and should give targeted advice based on project features.\n\n\
         ## Project Core Features\n\
         1. **CH340 Crosstalk Detection**: Identify TX self-loopback in CH340 USB-serial chips, \
            using rolling hash and sliding window to calculate echo_ratio, >50% = crosstalk\n\
         2. **Cushion 16x16 Pressure Matrix**: Square cushion pressure data acquisition and heatmap, \
            frame format: 0xA5 0x0A header + 528 bytes data\n\
         3. **STM32F303 Firmware Analysis**: Dual-mode sampling (ADC1 multi-channel + TIM3/TIM4 input capture), \
            921600 baud, USART1/USART3 DMA transfer\n\
         4. **Software Logic Analyzer**: Protocol decoding for TX/RX timing analysis, replaces DSLogic hardware\n\
         5. **Agent 7-Step Loop**: Observe → Hypothesize → Design → Send → Verify → Summarize → Update\n\
         6. **Firmware Peripheral Map**: Extract DMA/IRQ/GPIO config from bin, \
            feed back to diagnosis engine for mismatch/conflict/baud detection\n\n\
         ## Response Guidelines\n\
         - Reference project features in advice (e.g. \"check Scope protocol decode tab\")\n\
         - For CH340 issues, suggest crosstalk report and baud compensation\n\
         - For cushion data, mention 0xA5 0x0A + 528B frame format\n\
         - For firmware issues, suggest Periph Map tab\n\
         - Be concise and focused".to_string()
    } else {
        "You are the AI assistant for miconlab. \
         You understand the project's core modules and should give targeted advice based on project features.\n\n\
         ## Project Core Features\n\
         1. **CH340 Crosstalk Detection**: Identify TX self-loopback in CH340 USB-serial chips, \
            using rolling hash and sliding window to calculate echo_ratio, >50% = crosstalk\n\
         2. **Cushion 16x16 Pressure Matrix**: Square cushion pressure data acquisition and heatmap, \
            frame format: 0xA5 0x0A header + 528 bytes data\n\
         3. **STM32F303 Firmware Analysis**: Dual-mode sampling (ADC1 multi-channel + TIM3/TIM4 input capture), \
            921600 baud, USART1/USART3 DMA transfer\n\
         4. **Software Logic Analyzer**: Protocol decoding for TX/RX timing analysis, \
            replaces DSLogic hardware\n\
         5. **Agent 7-Step Loop**: Observe → Hypothesize → Design → Send → Verify → Summarize → Update\n\
         6. **Firmware Peripheral Map**: Extract DMA/IRQ/GPIO config from bin, \
            feed back to diagnosis engine for mismatch/conflict/baud detection\n\n\
         ## Response Guidelines\n\
         - Reference project features in advice (e.g., \"check Scope protocol decode tab\")\n\
         - For CH340 issues, suggest crosstalk report and baud compensation\n\
         - For cushion data, mention 0xA5 0x0A + 528B frame format\n\
         - For firmware issues, suggest Periph Map tab\n\
         - Be concise and focused".to_string()
    }
}

/// 构建当前状态上下文消息（注入到 LLM 对话中）
///
/// 把当前连接状态、波特率、串扰报告等关键信息打包成 user 消息前缀，
/// 让 LLM 能基于实时状态给出针对性回答
pub fn build_context_message(
    is_connected: bool,
    baud_rate: u32,
    chip_hint: Option<&str>,
    crosstalk_echo_ratio: Option<f32>,
    crosstalk_pattern: Option<&str>,
    rx_bytes: u64,
    tx_bytes: u64,
    is_zh: bool,
) -> String {
    let mut ctx = String::new();
    if is_zh {
        ctx.push_str("[Current Project State]\n");
        ctx.push_str(&format!("- Connection: {}\n", if is_connected { "Connected" } else { "Disconnected" }));
        ctx.push_str(&format!("- Baud rate: {}\n", baud_rate));
        if let Some(chip) = chip_hint {
            ctx.push_str(&format!("- USB bridge chip: {}\n", chip));
        }
        if let Some(ratio) = crosstalk_echo_ratio {
            ctx.push_str(&format!("- Crosstalk echo_ratio: {:.1}%\n", ratio * 100.0));
        }
        if let Some(pattern) = crosstalk_pattern {
            ctx.push_str(&format!("- Crosstalk pattern: {}\n", pattern));
        }
        ctx.push_str(&format!("- RX total: {} bytes\n", rx_bytes));
        ctx.push_str(&format!("- TX total: {} bytes\n", tx_bytes));
    } else {
        ctx.push_str("[Current Project State]\n");
        ctx.push_str(&format!("- Connection: {}\n", if is_connected { "Connected" } else { "Disconnected" }));
        ctx.push_str(&format!("- Baud rate: {}\n", baud_rate));
        if let Some(chip) = chip_hint {
            ctx.push_str(&format!("- USB bridge chip: {}\n", chip));
        }
        if let Some(ratio) = crosstalk_echo_ratio {
            ctx.push_str(&format!("- Crosstalk echo_ratio: {:.1}%\n", ratio * 100.0));
        }
        if let Some(pattern) = crosstalk_pattern {
            ctx.push_str(&format!("- Crosstalk pattern: {}\n", pattern));
        }
        ctx.push_str(&format!("- RX total: {} bytes\n", rx_bytes));
        ctx.push_str(&format!("- TX total: {} bytes\n", tx_bytes));
    }
    ctx
}
