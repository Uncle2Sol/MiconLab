//! miconlab AI 引擎
//!
//! 为串口通信工具提供原生的智能识别与诊断能力，包括：
//! - **协议指纹识别**：基于字节分布、熵、帧间隔等多维特征，用 KNN 分类器识别协议类型
//! - **异常检测**：滑动窗口统计模型，实时检测数据流中的异常帧/协议偏离
//! - **诊断规则引擎**：基于上下文的根因推理，给出连接失败、波特率不匹配等问题的诊断建议
//! - **设备知识库**：可扩展的设备指纹库，支持 JSON 导入/导出
//! - **可选 LLM 对话**（feature `llm-online`）：用户配置 API key 后启用在线大模型对话增强
//!
//! 设计原则：
//! - 纯 Rust 实现，核心引擎无网络依赖，离线可用
//! - 与 egui 单线程帧驱动架构兼容：所有 API 同步、非阻塞、单次调用快速返回
//! - 增量学习：用户标注的样本可加入本地知识库
//! - panic = "abort" 安全：无 unwrap 于外部输入，所有错误用 Result

pub mod agent;
pub mod anomaly;
pub mod chip_profile;
pub mod diagnose;
pub mod features;
pub mod firmware;
pub mod knowledge;
pub mod protocol_fp;
pub mod prompts;
pub mod rebuild;
pub mod sensor;
pub mod thumb_disasm;
pub mod xtensa_disasm;

#[cfg(feature = "llm-online")]
pub mod llm;

#[cfg(feature = "ollama")]
pub mod identify;
#[cfg(feature = "ollama")]
pub mod ollama;

pub use anomaly::{AnomalyAlert, AnomalyDetector, AnomalyKind};
pub use agent::{AgentAction, AgentTask, TaskContext, TaskResult, TaskState, TaskStep, VerificationCriteria};
pub use diagnose::{
    CrosstalkReport, DiagnosisResult, DiagnosticContext, DiagnosticEngine, DiagnosticRule,
    EchoPattern, Severity,
};
pub use features::{KnownHeader, StreamFeatures};
pub use firmware::{
    DmaChannel, FirmwareAnalysis, FirmwareArch, GpioMode, GpioPin, IrqPeripheral,
};
pub use knowledge::{DeviceFingerprint, KnowledgeBase};
pub use prompts::PromptTemplate;
pub use protocol_fp::{ProtocolClass, ProtocolClassifier, ProtocolTemplate};
pub use sensor::{
    CalibrationAssessment, CalibrationModel, ChannelStats, PressureAnalysis, PressurePattern,
    SensorAnalysis, SensorAnomaly, SensorType,
};

#[cfg(feature = "ollama")]
pub use identify::{
    identify_chip, identify_firmware, identify_protocol, ChipIdentifyResult, CrossCheck,
    EvidenceBundle, FirmwareIdentifyResult, IdentifySource, IdentifyTarget,
    ProtocolIdentifyResult,
};
#[cfg(feature = "ollama")]
pub use ollama::{OllamaClient, OllamaConfig, OllamaError, OllamaModel, OllamaStatus};

/// AI 引擎：聚合所有智能能力的统一入口
///
/// 持有协议分类器、异常检测器、诊断引擎、知识库，
/// 提供 `ingest`（喂入数据）、`classify`（协议识别）、`check_anomaly`（异常检测）、
/// `diagnose`（诊断推理）等高层 API。
#[derive(Clone)]
pub struct AiEngine {
    /// 协议分类器（KNN + 模板匹配）
    pub classifier: ProtocolClassifier,
    /// 异常检测器（滑动窗口统计）
    pub detector: AnomalyDetector,
    /// 诊断规则引擎
    pub diagnostic: DiagnosticEngine,
    /// 设备知识库
    pub knowledge: KnowledgeBase,
    /// 最近一次协议识别结果（缓存）
    last_classification: Option<(ProtocolClass, f32)>,
}

impl Default for AiEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AiEngine {
    /// 创建引擎实例，预置所有默认模板、规则和设备知识
    pub fn new() -> Self {
        Self {
            classifier: ProtocolClassifier::with_builtin_templates(),
            detector: AnomalyDetector::new(256),
            diagnostic: DiagnosticEngine::with_builtin_rules(),
            knowledge: KnowledgeBase::with_builtin_devices(),
            last_classification: None,
        }
    }

    /// 喂入一段串口数据，更新异常检测基线
    ///
    /// 在 GUI 每帧 poll_rx 后调用，传入本帧接收的字节。
    /// 返回检测到的异常（如果有）。
    pub fn ingest(&mut self, data: &[u8]) -> Option<AnomalyAlert> {
        self.detector.update(data)
    }

    /// 喂入带时间戳的帧数据，同时更新异常检测和帧间隔统计
    ///
    /// `frame_intervals_ms` 是最近若干帧的到达间隔（毫秒），
    /// 用于协议指纹的时序特征。
    pub fn ingest_with_timing(
        &mut self,
        data: &[u8],
        frame_intervals_ms: &[f32],
    ) -> Option<AnomalyAlert> {
        let alert = self.detector.update(data);
        // 更新异常检测器的帧间隔基线
        self.detector.update_timing(frame_intervals_ms);
        alert
    }

    /// 对一段数据提取特征并识别协议类型
    ///
    /// `data` 应为具有代表性的数据片段（建议 >= 64 字节）。
    /// `frame_intervals_ms` 可选，提供时序特征可提升识别精度。
    ///
    /// 返回 (协议类型, 置信度 0.0-1.0)。
    pub fn classify(
        &mut self,
        data: &[u8],
        frame_intervals_ms: Option<&[f32]>,
    ) -> (ProtocolClass, f32) {
        let features = StreamFeatures::extract(data, frame_intervals_ms);
        let result = self.classifier.predict(&features);
        self.last_classification = Some(result);
        result
    }

    /// 获取最近一次识别结果
    pub fn last_classification(&self) -> Option<(ProtocolClass, f32)> {
        self.last_classification
    }

    /// 获取异常检测器当前灵敏度
    pub fn detector_sensitivity(&self) -> f32 {
        self.detector.sensitivity()
    }

    /// 设置异常检测器灵敏度
    pub fn detector_set_sensitivity(&mut self, sensitivity: f32) {
        self.detector.set_sensitivity(sensitivity);
    }

    /// 检查异常（不喂入新数据，基于当前基线状态）
    pub fn check_anomaly(&self, data: &[u8]) -> Option<AnomalyAlert> {
        self.detector.check(data)
    }

    /// 诊断推理：根据当前上下文给出诊断建议
    pub fn diagnose(&self, context: &diagnose::DiagnosticContext) -> Vec<DiagnosisResult> {
        self.diagnostic.diagnose(context)
    }

    /// 添加用户标注的协议样本（增量学习）
    ///
    /// `data` 为样本数据，`label` 为用户指定的协议类型。
    /// 样本加入分类器后，后续识别会参考该样本。
    pub fn add_protocol_sample(&mut self, data: &[u8], label: ProtocolClass) {
        let features = StreamFeatures::extract(data, None);
        self.classifier.add_sample(features, label);
    }
}
