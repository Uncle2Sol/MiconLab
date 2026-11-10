//! Agent 任务模型（PROBES 7 步闭环）
//!
//! 参考 kaiwu-probes.com 的任务四要素 + UART 7 步闭环模型，
//! 为串口通信问题提供结构化诊断建议。
//!
//! 7 步闭环：观察 → 假设 → 设计命令 → 发送 → 验证 → 归纳 → 更新知识库
//!
//! Agent 只输出 `AgentAction` 建议（不自动执行），GUI 层显示"建议执行"按钮，
//! 用户确认后调用现有 `state.rs` API。

use crate::diagnose::{CrosstalkReport, DiagnosticEngine};
use crate::firmware::FirmwareAnalysis;
use serde::{Deserialize, Serialize};

/// Agent 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// 观察阶段
    Observing,
    /// 假设阶段
    Hypothesizing,
    /// 设计命令阶段
    DesigningCommand,
    /// 已发送，等待验证
    Verifying,
    /// 归纳阶段
    Summarizing,
    /// 已完成（更新知识库）
    Completed,
    /// 失败
    Failed,
}

impl Default for TaskState {
    fn default() -> Self {
        TaskState::Observing
    }
}

/// 7 步闭环的步骤定义
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStep {
    /// 1. 观察当前状态
    Observe,
    /// 2. 提出假设
    Hypothesize,
    /// 3. 设计验证命令
    DesignCommand,
    /// 4. 发送命令
    Send,
    /// 5. 验证响应
    Verify,
    /// 6. 归纳结论
    Summarize,
    /// 7. 更新知识库
    UpdateKnowledge,
}

impl TaskStep {
    /// 所有步骤（有序）
    pub fn all() -> &'static [TaskStep] {
        &[
            TaskStep::Observe,
            TaskStep::Hypothesize,
            TaskStep::DesignCommand,
            TaskStep::Send,
            TaskStep::Verify,
            TaskStep::Summarize,
            TaskStep::UpdateKnowledge,
        ]
    }

    /// 步骤中文名
    pub fn label_zh(self) -> &'static str {
        match self {
            TaskStep::Observe => "Observe",
            TaskStep::Hypothesize => "Hypothesize",
            TaskStep::DesignCommand => "Design Command",
            TaskStep::Send => "Send",
            TaskStep::Verify => "Verify",
            TaskStep::Summarize => "Summarize",
            TaskStep::UpdateKnowledge => "Update Knowledge",
        }
    }

    /// 步骤英文名
    pub fn label_en(self) -> &'static str {
        match self {
            TaskStep::Observe => "Observe",
            TaskStep::Hypothesize => "Hypothesize",
            TaskStep::DesignCommand => "Design Command",
            TaskStep::Send => "Send",
            TaskStep::Verify => "Verify",
            TaskStep::Summarize => "Summarize",
            TaskStep::UpdateKnowledge => "Update Knowledge",
        }
    }
}

/// Agent 允许执行的动作（只输出建议，不自动执行）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentAction {
    /// 发送字节数据
    SendBytes(Vec<u8>),
    /// 设置波特率
    SetBaud(u32),
    /// 切换 DTR 信号
    ToggleDtr(bool),
    /// 切换 RTS 信号
    ToggleRts(bool),
    /// 等待毫秒
    WaitMs(u32),
    /// 运行诊断
    RunDiagnose,
    /// 扫描波特率
    ScanBaud,
    /// 扫描串扰
    ScanCrosstalk,
}

impl AgentAction {
    /// 中文描述
    pub fn describe_zh(&self) -> String {
        match self {
            AgentAction::SendBytes(data) => {
                let hex: String = data.iter().take(16).map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                let suffix = if data.len() > 16 { " ..." } else { "" };
                format!("Send {} bytes: {}", data.len(), hex) + suffix
            }
            AgentAction::SetBaud(baud) => format!("Set baud rate → {}", baud),
            AgentAction::ToggleDtr(on) => format!("DTR → {}", if *on { "ON" } else { "OFF" }),
            AgentAction::ToggleRts(on) => format!("RTS → {}", if *on { "ON" } else { "OFF" }),
            AgentAction::WaitMs(ms) => format!("Wait {} ms", ms),
            AgentAction::RunDiagnose => "Run diagnosis".to_string(),
            AgentAction::ScanBaud => "Scan baud rates".to_string(),
            AgentAction::ScanCrosstalk => "Scan crosstalk".to_string(),
        }
    }
}

/// 任务上下文（本地信息快照）
#[derive(Debug, Clone, Default)]
pub struct TaskContext {
    /// USB VID/PID
    pub vid_pid: Option<(u16, u16)>,
    /// 当前波特率
    pub baud_rate: u32,
    /// 端口名
    pub port_name: Option<String>,
    /// 最近接收数据样本（最多 256 字节）
    pub rx_sample: Vec<u8>,
    /// 最近发送数据样本（最多 256 字节）
    pub tx_sample: Vec<u8>,
    /// 串扰检测报告
    pub crosstalk: Option<CrosstalkReport>,
    /// 固件分析提示
    pub firmware_hint: Option<FirmwareAnalysis>,
    /// 芯片提示（CH340 / CP210x 等）
    pub chip_hint: Option<String>,
    /// 是否已连接
    pub is_connected: bool,
}

/// 验证标准
#[derive(Debug, Clone)]
pub struct VerificationCriteria {
    /// 期望的响应模式（前缀匹配）
    pub expected_pattern: Option<Vec<u8>>,
    /// 最少接收字节数
    pub min_rx_bytes: usize,
    /// 超时（毫秒）
    pub timeout_ms: u32,
    /// 最大允许串扰比
    pub max_crosstalk_ratio: f32,
}

impl Default for VerificationCriteria {
    fn default() -> Self {
        Self {
            expected_pattern: None,
            min_rx_bytes: 1,
            timeout_ms: 3000,
            max_crosstalk_ratio: 0.1,
        }
    }
}

/// Agent 任务
#[derive(Debug, Clone)]
pub struct AgentTask {
    /// 工程目标
    pub goal: String,
    /// 本地上下文
    pub context: TaskContext,
    /// 允许的动作
    pub allowed_actions: Vec<AgentAction>,
    /// 验证标准
    pub verification: VerificationCriteria,
    /// 当前步骤索引（0-6）
    pub current_step: usize,
    /// 任务状态
    pub state: TaskState,
    /// 已输出的建议动作
    pub suggested_actions: Vec<AgentAction>,
    /// 诊断结论
    pub diagnosis: String,
    /// 下一步提示
    pub next_step_hint: String,
    /// 置信度
    pub confidence: f32,
}

impl AgentTask {
    /// 创建新任务
    pub fn new(goal: &str, context: TaskContext) -> Self {
        Self {
            goal: goal.to_string(),
            context,
            allowed_actions: Vec::new(),
            verification: VerificationCriteria::default(),
            current_step: 0,
            state: TaskState::Observing,
            suggested_actions: Vec::new(),
            diagnosis: String::new(),
            next_step_hint: String::new(),
            confidence: 0.0,
        }
    }

    /// 运行任务（纯规则，离线）
    ///
    /// 基于 PROBES 7 步闭环推进状态机，输出诊断建议。
    pub fn run_offline(&mut self, engine: &DiagnosticEngine) -> TaskResult {
        // 步骤 1: 观察
        self.current_step = 0;
        self.state = TaskState::Observing;

        // 步骤 2: 假设
        self.current_step = 1;
        self.state = TaskState::Hypothesizing;
        let hypothesis = self.form_hypothesis();

        // 步骤 3: 设计命令
        self.current_step = 2;
        self.state = TaskState::DesigningCommand;
        self.suggested_actions = self.design_actions(&hypothesis);

        // 步骤 4-5: 发送与验证（建议动作，不自动执行）
        self.current_step = 4;
        self.state = TaskState::Verifying;

        // 步骤 6: 归纳
        self.current_step = 5;
        self.state = TaskState::Summarizing;
        self.diagnosis = self.summarize(&hypothesis, engine);
        self.next_step_hint = if self.suggested_actions.is_empty() {
            "No further action needed; diagnosis complete".to_string()
        } else {
            format!(
                "Recommend executing the following {} action(s) to verify",
                self.suggested_actions.len()
            )
        };

        // 步骤 7: 更新知识库
        self.current_step = 6;
        self.state = TaskState::Completed;

        TaskResult {
            diagnosis: self.diagnosis.clone(),
            suggested_actions: self.suggested_actions.clone(),
            next_step_hint: self.next_step_hint.clone(),
            confidence: self.confidence,
        }
    }

    /// 形成假设
    fn form_hypothesis(&self) -> String {
        if let Some(ct) = &self.context.crosstalk {
            if ct.is_loopback {
                return "CH340 TX loopback: RX is receiving TX crosstalk; device TX is not connected to CH340 RX".to_string();
            }
            if ct.is_half_echo() {
                return format!(
                    "~50% crosstalk (echo ratio {:.0}%): typical TX impedance reflection; device TX may be miswired",
                    ct.echo_ratio * 100.0
                );
            }
            if ct.high_byte_ratio > 0.4 {
                return "High-byte dominant: baud mismatch or CH340 clock drift; try over-sampling compensation".to_string();
            }
        }

        if !self.context.is_connected {
            return "Device not connected: select a port and connect first".to_string();
        }

        if self.context.rx_sample.is_empty() {
            return "No data received: device may be powered off, TX not connected, or wrong baud rate".to_string();
        }

        // 检查固件波特率提示（通过 summary_zh 中的波特率信息）
        if let Some(fw) = &self.context.firmware_hint {
            if fw.summary_zh.contains("921600") && self.context.baud_rate != 921600 {
                return format!(
                    "Firmware analysis suggests 921600 baud, but current configuration is {} baud (possible mismatch)",
                    self.context.baud_rate
                );
            }
        }

        "Data stream normal, no obvious anomaly".to_string()
    }

    /// 设计验证动作
    fn design_actions(&self, hypothesis: &str) -> Vec<AgentAction> {
        let mut actions = Vec::new();

        if hypothesis.contains("loopback") || hypothesis.contains("crosstalk") {
            // 串扰问题：建议检查接线和尝试倍频补偿
            if let Some(ct) = &self.context.crosstalk {
                if ct.high_byte_ratio > 0.4 {
                    // 高字节主导：建议尝试倍频
                    actions.push(AgentAction::SetBaud(self.context.baud_rate * 2));
                    actions.push(AgentAction::SendBytes(vec![0xA5, 0x0A]));
                    actions.push(AgentAction::WaitMs(1500));
                    actions.push(AgentAction::ScanCrosstalk);
                }
            }
            // 串扰但非高字节：建议检查接线（无自动动作）
            actions.push(AgentAction::RunDiagnose);
        } else if hypothesis.contains("baud") {
            // 波特率不匹配：建议扫描
            actions.push(AgentAction::ScanBaud);
        } else if hypothesis.contains("No data") || hypothesis.contains("not connected") {
            // 无数据：建议发送触发命令
            actions.push(AgentAction::SendBytes(vec![0xA5, 0x0A]));
            actions.push(AgentAction::WaitMs(2000));
            actions.push(AgentAction::RunDiagnose);
        }

        actions
    }

    /// 归纳结论
    fn summarize(&mut self, hypothesis: &str, _engine: &DiagnosticEngine) -> String {
        self.confidence = if hypothesis.contains("loopback") {
            0.9
        } else if hypothesis.contains("crosstalk") || hypothesis.contains("baud") {
            0.75
        } else if hypothesis.contains("No data") || hypothesis.contains("not connected") {
            0.6
        } else {
            0.5
        };

        let mut summary = format!("Hypothesis: {}\n", hypothesis);
        if !self.suggested_actions.is_empty() {
            summary.push_str("Suggested actions:\n");
            for (i, action) in self.suggested_actions.iter().enumerate() {
                summary.push_str(&format!("  {}. {}\n", i + 1, action.describe_zh()));
            }
        }
        summary.push_str(&format!("Confidence: {:.0}%", self.confidence * 100.0));
        summary
    }

    /// 获取建议动作列表（供 GUI 显示）
    pub fn suggested_actions(&self) -> &[AgentAction] {
        &self.suggested_actions
    }
}

/// 任务运行结果
#[derive(Debug, Clone)]
pub struct TaskResult {
    /// 诊断结论
    pub diagnosis: String,
    /// 建议动作列表
    pub suggested_actions: Vec<AgentAction>,
    /// 下一步提示
    pub next_step_hint: String,
    /// 置信度 0.0-1.0
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_task_crosstalk() {
        let mut ctx = TaskContext::default();
        ctx.is_connected = true;
        ctx.baud_rate = 921600;
        ctx.crosstalk = Some(CrosstalkReport {
            echo_ratio: 0.85,
            rx_tx_ratio: 0.5,
            is_loopback: true,
            latency_us: Some(120),
            pattern: crate::diagnose::EchoPattern::Exact,
            high_byte_ratio: 0.9,
            tx_total: 528,
            rx_total: 264,
        });

        let mut task = AgentTask::new("诊断坐垫按压无响应", ctx);
        let engine = DiagnosticEngine::with_builtin_rules();
        let result = task.run_offline(&engine);

        assert!(result.diagnosis.contains("loopback"));
        assert!(!result.suggested_actions.is_empty());
        assert!(result.confidence > 0.8);
    }

    #[test]
    fn test_agent_task_no_data() {
        let ctx = TaskContext::default();
        let mut task = AgentTask::new("诊断无数据", ctx);
        let engine = DiagnosticEngine::with_builtin_rules();
        let result = task.run_offline(&engine);

        assert!(result.diagnosis.contains("not connected"));
    }

    #[test]
    fn test_task_step_order() {
        let steps = TaskStep::all();
        assert_eq!(steps.len(), 7);
        assert_eq!(steps[0], TaskStep::Observe);
        assert_eq!(steps[6], TaskStep::UpdateKnowledge);
    }
}
