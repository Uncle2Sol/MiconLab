//! 提示词模板库
//!
//! 参考 PROBES (kaiwu-probes.com) 的提示词模板，每个模板为纯函数
//! `fn template_zh(ctx: &TaskContext) -> String`，用于 Agent 7 步闭环诊断。
//!
//! 模板包含：任务目标、上下文快照、7 步执行计划、验证标准。

use crate::agent::TaskContext;

/// 模板标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptTemplate {
    /// 坐垫 CH340 串扰诊断
    CushionCrosstalk,
    /// 波特率不匹配诊断
    BaudMismatch,
    /// UART DMA 丢帧诊断
    UartDmaDrop,
    /// 软件逻辑分析仪
    LogicAnalyzerSim,
    /// 固件外设映射分析
    FirmwarePeriph,
    /// PID 调参（预留）
    PidTuning,
}

impl PromptTemplate {
    /// 所有模板列表
    pub fn all() -> &'static [PromptTemplate] {
        &[
            PromptTemplate::CushionCrosstalk,
            PromptTemplate::BaudMismatch,
            PromptTemplate::UartDmaDrop,
            PromptTemplate::LogicAnalyzerSim,
            PromptTemplate::FirmwarePeriph,
            PromptTemplate::PidTuning,
        ]
    }

    /// 模板中文名
    pub fn label_zh(self) -> &'static str {
        match self {
            PromptTemplate::CushionCrosstalk => "Cushion CH340 Crosstalk",
            PromptTemplate::BaudMismatch => "Baud Mismatch",
            PromptTemplate::UartDmaDrop => "UART DMA Drop",
            PromptTemplate::LogicAnalyzerSim => "Logic Analyzer Sim",
            PromptTemplate::FirmwarePeriph => "Firmware Peripheral Map",
            PromptTemplate::PidTuning => "PID Tuning (reserved)",
        }
    }

    /// 模板英文名
    pub fn label_en(self) -> &'static str {
        match self {
            PromptTemplate::CushionCrosstalk => "Cushion CH340 Crosstalk",
            PromptTemplate::BaudMismatch => "Baud Mismatch",
            PromptTemplate::UartDmaDrop => "UART DMA Drop",
            PromptTemplate::LogicAnalyzerSim => "Logic Analyzer Sim",
            PromptTemplate::FirmwarePeriph => "Firmware Peripheral Map",
            PromptTemplate::PidTuning => "PID Tuning (reserved)",
        }
    }

    /// 生成提示词（中文）
    pub fn render_zh(self, ctx: &TaskContext) -> String {
        match self {
            PromptTemplate::CushionCrosstalk => cushion_crosstalk_template(ctx),
            PromptTemplate::BaudMismatch => baud_mismatch_template(ctx),
            PromptTemplate::UartDmaDrop => uart_dma_drop_template(ctx),
            PromptTemplate::LogicAnalyzerSim => logic_analyzer_sim_template(ctx),
            PromptTemplate::FirmwarePeriph => firmware_periph_template(ctx),
            PromptTemplate::PidTuning => pid_tuning_template(ctx),
        }
    }
}

/// 格式化上下文快照（通用部分）
fn format_context(ctx: &TaskContext) -> String {
    let mut s = String::new();
    s.push_str(&format!("- Connection: {}\n", if ctx.is_connected { "Connected" } else { "Disconnected" }));
    s.push_str(&format!("- Port: {}\n", ctx.port_name.as_deref().unwrap_or("—")));
    s.push_str(&format!("- Baud rate: {}\n", ctx.baud_rate));
    s.push_str(&format!("- Chip: {}\n", ctx.chip_hint.as_deref().unwrap_or("Unknown")));
    if let Some((vid, pid)) = ctx.vid_pid {
        s.push_str(&format!("- VID/PID: {:04X}:{:04X}\n", vid, pid));
    }
    if !ctx.rx_sample.is_empty() {
        let hex: String = ctx.rx_sample.iter().take(32).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
        s.push_str(&format!("- RX sample ({} bytes): {}\n", ctx.rx_sample.len(), hex));
    }
    if !ctx.tx_sample.is_empty() {
        let hex: String = ctx.tx_sample.iter().take(32).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
        s.push_str(&format!("- TX sample ({} bytes): {}\n", ctx.tx_sample.len(), hex));
    }
    if let Some(ct) = &ctx.crosstalk {
        s.push_str(&format!("- Crosstalk: echo_ratio={:.0}%, rx_tx_ratio={:.2}, is_loopback={}, high_byte_ratio={:.0}%\n",
            ct.echo_ratio * 100.0, ct.rx_tx_ratio, ct.is_loopback, ct.high_byte_ratio * 100.0));
    }
    s
}

fn cushion_crosstalk_template(ctx: &TaskContext) -> String {
    format!(
        "# Cushion CH340 Crosstalk Diagnosis\n\n\
        ## Goal\n\
        Diagnose the root cause of no response after the cushion device is connected via CH340, and identify TX self-loopback crosstalk.\n\n\
        ## Context\n\
        {}\n\
        ## 7-Step Loop\n\
        1. Observe: check RX/TX byte statistics and the crosstalk report\n\
        2. Hypothesize: if echo_ratio > 50% and rx_tx_ratio ≈ 0.5, classify as TX crosstalk\n\
        3. Design command: construct a 528-byte trigger frame (0xA5 0x0A) to send\n\
        4. Send: send the trigger frame over the serial port\n\
        5. Verify: check whether RX receives response data that is not TX crosstalk\n\
        6. Summarize: still crosstalk → check wiring; new data → crosstalk resolved\n\
        7. Update knowledge: record CH340 crosstalk signature and compensation plan\n",
        format_context(ctx)
    )
}

fn baud_mismatch_template(ctx: &TaskContext) -> String {
    format!(
        "# Baud Mismatch Diagnosis\n\n\
        ## Goal\n\
        Diagnose whether garbled data (high-byte dominant) comes from a baud rate mismatch.\n\n\
        ## Context\n\
        {}\n\
        ## 7-Step Loop\n\
        1. Observe: check high_byte_ratio > 40% and low echo_ratio\n\
        2. Hypothesize: baud deviation causes bit misalignment\n\
        3. Design command: test 921600 / 460800 / 1843200 / 230400 in sequence\n\
        4. Send: send the trigger frame at each baud rate\n\
        5. Verify: check whether a valid header 0xAA55 is received\n\
        6. Summarize: the baud rate with a valid header is the correct one\n\
        7. Update knowledge: record the device baud fingerprint\n",
        format_context(ctx)
    )
}

fn uart_dma_drop_template(ctx: &TaskContext) -> String {
    format!(
        "# UART DMA Drop Diagnosis\n\n\
        ## Goal\n\
        Diagnose data loss or incomplete frames during UART DMA transfer.\n\n\
        ## Context\n\
        {}\n\
        ## 7-Step Loop\n\
        1. Observe: check whether frame intervals are abnormal (>100ms or jitter)\n\
        2. Hypothesize: DMA buffer overflow or interrupt priority conflict\n\
        3. Design command: send consecutive trigger frames and monitor frame intervals\n\
        4. Send: send 10 trigger commands at 25ms intervals\n\
        5. Verify: check whether received frame count == sent frame count\n\
        6. Summarize: drop rate > 10% → DMA configuration issue\n\
        7. Update knowledge: record DMA drop scenarios and thresholds\n",
        format_context(ctx)
    )
}

fn logic_analyzer_sim_template(ctx: &TaskContext) -> String {
    format!(
        "# Software Logic Analyzer\n\n\
        ## Goal\n\
        Analyze TX/RX timing relationships via software protocol decoding (no hardware logic analyzer).\n\n\
        ## Context\n\
        {}\n\
        ## 7-Step Loop\n\
        1. Observe: collect timestamped TX/RX byte streams\n\
        2. Hypothesize: RX frames with latency < 100ms from the latest TX frame are responses\n\
        3. Design command: send a known command and record the TX timestamp\n\
        4. Send: send the command and start timing capture\n\
        5. Verify: check the latency and content of the RX response frame\n\
        6. Summarize: build a command→response mapping table\n\
        7. Update knowledge: record the protocol command dictionary\n",
        format_context(ctx)
    )
}

fn firmware_periph_template(ctx: &TaskContext) -> String {
    format!(
        "# Firmware Peripheral Map Analysis\n\n\
        ## Goal\n\
        Extract DMA/IRQ/GPIO peripheral configuration from firmware binary analysis to feed the diagnosis engine.\n\n\
        ## Context\n\
        {}\n\
        ## 7-Step Loop\n\
        1. Observe: load the firmware bin and parse the vector table\n\
        2. Hypothesize: infer peripherals from APB1/APB2 register addresses\n\
        3. Design command: scan DMA CPAR/CMAR write sequences\n\
        4. Send: submit the firmware to the analysis engine\n\
        5. Verify: disassemble handler addresses to confirm peripheral access\n\
        6. Summarize: generate a DMA/IRQ/GPIO three-dimensional mapping table\n\
        7. Update knowledge: record the firmware peripheral fingerprint\n",
        format_context(ctx)
    )
}

fn pid_tuning_template(ctx: &TaskContext) -> String {
    format!(
        "# PID Tuning (reserved)\n\n\
        ## Goal\n\
        Reserved template for PID controller parameter tuning.\n\n\
        ## Context\n\
        {}\n\
        ## 7-Step Loop\n\
        1. Observe: collect system step response data\n\
        2. Hypothesize: infer parameter direction from overshoot/rise time\n\
        3. Design command: compute new P/I/D parameters\n\
        4. Send: write new parameters to the controller\n\
        5. Verify: run another step test and compare response curves\n\
        6. Summarize: keep if improved, otherwise roll back\n\
        7. Update knowledge: record the parameter→response mapping\n",
        format_context(ctx)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_list() {
        let templates = PromptTemplate::all();
        assert_eq!(templates.len(), 6);
    }

    #[test]
    fn test_cushion_template_render() {
        let ctx = TaskContext::default();
        let prompt = PromptTemplate::CushionCrosstalk.render_zh(&ctx);
        assert!(prompt.contains("Cushion"));
        assert!(prompt.contains("7-Step Loop"));
    }

    #[test]
    fn test_all_templates_render() {
        let ctx = TaskContext::default();
        for &t in PromptTemplate::all() {
            let prompt = t.render_zh(&ctx);
            assert!(!prompt.is_empty());
            assert!(prompt.contains("7-Step Loop"));
        }
    }

    #[test]
    fn test_ollama_identify_prompts_non_empty() {
        for p in [
            ollama_system_chip_identify(),
            ollama_system_firmware_identify(),
            ollama_system_protocol_identify(),
        ] {
            assert!(p.contains("JSON"));
            assert!(p.contains("reasoning"));
        }
    }
}

// ============ Ollama 识别任务系统提示词（feature `ollama` 使用） ============

/// Ollama 识别任务公共角色设定：嵌入式硬件识别专家，只输出 JSON
const OLLAMA_IDENTIFY_ROLE: &str = "You are an embedded hardware identification expert serving the miconlab serial debugging tool.\
You must output only one JSON object conforming to the user-provided JSON Schema; do not output markdown code blocks, explanatory prose, or any other content.\
The reasoning field must briefly describe the basis for the judgment (within 50 words).\
confidence ranges 0.0-1.0: strong evidence >=0.8, partial match 0.5-0.7, pure guess <=0.3; prefer low confidence over fabrication.\
The evidence bundle may contain the rule engine's initial judgment, which is a reference: adopt it if evidence supports it, or override it with justification and explain in reasoning.";

/// 通讯芯片识别系统提示词
pub fn ollama_system_chip_identify() -> String {
    format!(
        "{}\n\nCurrent task: identify the communication chip (USB-to-serial bridge chip or directly connected MCU).\n\
Evidence priority:\n\
1. USB VID:PID is the strongest evidence (e.g. 1A86:7523=WCH CH340, 10C4:EA60=Silicon Labs CP2102, \
0403:6001=FTDI FT232, 303A:xxxx=Espressif native USB, 1FC9/1FCB=NXP)\n\
2. Probe responses (e.g. STM32 Bootloader 0x7F reply, ESP sync response, AT reply)\n\
3. Port naming conventions (cu.usbserial=FTDI driver, cu.wchusbserial=WCH, cu.SLAB_USBtoUART=SiLabs)\n\
4. Data stream features\n\
The chip field must give the specific model (e.g. CH340C, CP2102N, STM32F103C8); vendor gives the vendor name; \
family gives the chip family (usb-uart / cortex-m0/m3/m4 / xtensa / riscv32 / native-usb).\
suggested_driver gives the recommended driver or flashing tool (e.g. wch-ch34x / silabs-cp210x / stm32flash / esptool); use null if unsure.",
        OLLAMA_IDENTIFY_ROLE
    )
}

/// bin 固件识别系统提示词
pub fn ollama_system_firmware_identify() -> String {
    format!(
        "{}\n\nCurrent task: identify the architecture, Flash base address, and chip family of the bin firmware.\n\
Evidence priority:\n\
1. Magic numbers: first byte 0xE9 = ESP image (ESP8266 base 0x0000, ESP32 base usually 0x1000); \
Intel HEX text starts with ':'\n\
2. ARM Cortex-M vector table: first 4 bytes are a valid stack pointer (0x2000xxxx), bytes 5-8 are the reset vector (pointing into Flash, LSB=1)\n\
3. String evidence (e.g. chip model strings, build info, SDK version)\n\
4. Instruction patterns (Thumb-2 density, Xtensa 24-bit instructions)\n\
The arch field must be one of cortex-m0/m0+/m3/m4/m7, xtensa-lx6/lx7, riscv32, avr, intel-hex, or the closest value;\
base_addr gives the Flash load base address (STM32 usually 0x08000000, ESP32 bootloader 0x1000), \
as a decimal integer or a \"0x\"-prefixed hex string; use null for entry if unsure; chip_family e.g. stm32f1/esp32; use null if unsure.",
        OLLAMA_IDENTIFY_ROLE
    )
}

/// 上位机协议识别系统提示词
pub fn ollama_system_protocol_identify() -> String {
    format!(
        "{}\n\nCurrent task: identify the host protocol used by the serial data stream.\n\
Evidence priority:\n\
1. Known headers/footers (AA 55, 0x7E HDLC, 0xE9, 0x7F, $GP NMEA, ':' Modbus ASCII)\n\
2. Frame structure (fixed header + length + payload + CRC16 = Modbus RTU style; AT prefix returning OK/ERROR = AT command set)\n\
3. Statistical features: high printable ratio + low entropy = text protocol; high entropy + high binary ratio = binary/encrypted; \
regular frame intervals (coefficient of variation < 0.2) = polled master-slave protocol\n\
4. Repeated patterns in data samples\n\
The protocol field must be the closest protocol name (modbus-rtu / modbus-ascii / at-command / nmea / hdlc / slcan / \
custom binary frame / custom text line, etc.); frame_format describes the frame layout (e.g. \"AA 55 <len> <payload> <crc16-le>\"); \
use null if it cannot be determined; suggested_baud is only given when data looks garbled and a different baud may help; otherwise null.",
        OLLAMA_IDENTIFY_ROLE
    )
}
