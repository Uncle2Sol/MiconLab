// AI 助手面板
//
// 五大标签页：
//   1. 协议识别 — 实时 KNN 分类 + 16 维特征向量可视化 + 已知帧头提示
//   2. 异常检测 — 滑动窗口告警流 + 灵敏度调节 + HEX 数据片段
//   3. 智能诊断 — 基于上下文的根因推理 + 操作建议
//   4. AI 对话  — OpenAI 兼容 LLM 配置 + 后台线程非阻塞对话（feature `llm-online`）
//   5. 知识库   — USB 设备指纹库 + JSON 导入/导出
//
// 状态管理：AiAssistantState 通过 egui IdTypeMap 持久化（Clone + Send + Sync）。
// 每帧从 AppState.rx_buffer 增量喂入 AiEngine，避免重复处理。

use crate::state::AppState;
use crate::theme;
use egui::{Color32, Id, RichText, Vec2};
use miconlab_ai::{
    AiEngine, AnomalyAlert, AnomalyKind, CrosstalkReport as AiCrosstalkReport,
    DeviceFingerprint, DiagnosisResult, DiagnosticContext, EchoPattern as AiEchoPattern,
    ProtocolClass, Severity, StreamFeatures,
};
#[cfg(any(feature = "llm-online", feature = "ollama"))]
use std::sync::atomic::AtomicBool;
#[cfg(any(feature = "llm-online", feature = "ollama"))]
use std::sync::atomic::Ordering;
#[cfg(any(feature = "llm-online", feature = "ollama"))]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "llm-online")]
use miconlab_ai::llm::{LlmClient, LlmConfig, LlmMessage};

#[cfg(feature = "ollama")]
use miconlab_ai::{
    CrossCheck, EvidenceBundle, OllamaClient, OllamaConfig, OllamaStatus, ProtocolIdentifyResult,
};

/// LLM 后台任务结果类型（Arc 包装便于跨线程 + egui 持久化）
#[cfg(any(feature = "llm-online", feature = "ollama"))]
type LlmResultSlot = Arc<Mutex<Option<Result<String, String>>>>;
/// LLM 后台任务完成标志
#[cfg(any(feature = "llm-online", feature = "ollama"))]
type LlmDoneFlag = Arc<AtomicBool>;

/// Ollama 协议识别后台任务结果槽
#[cfg(feature = "ollama")]
type OllamaResultSlot = Arc<Mutex<Option<Result<ProtocolIdentifyResult, String>>>>;
/// Ollama 状态检测后台任务结果槽
#[cfg(feature = "ollama")]
type OllamaStatusSlot = Arc<Mutex<Option<OllamaStatus>>>;

/// AI 助手面板状态（持久化到 egui IdTypeMap，必须 Clone + Send + Sync）
#[derive(Clone)]
pub struct AiAssistantState {
    /// 核心 AI 引擎
    pub engine: AiEngine,
    /// 最近告警（最多 100 条）
    pub anomalies: Vec<AnomalyAlert>,
    /// 最近诊断结果
    pub diagnoses: Vec<DiagnosisResult>,
    /// 异常检测灵敏度（0.0-1.0）
    pub sensitivity: f32,
    /// 已喂入 AI 引擎的条目序号（对应 AppState::rx_entry_seq，免疫 rx_buffer 头部丢弃）
    pub last_rx_seq: u64,
    /// 上次协议识别时刻（节流，每 500ms 最多一次）
    pub last_classify_at: Option<Instant>,
    /// 上次识别结果
    pub last_classification: Option<(ProtocolClass, f32)>,
    /// 上次特征向量（用于可视化）
    pub last_features: Option<StreamFeatures>,
    /// 上次识别的数据样本（HEX，最多 64 字节）
    pub last_sample_hex: String,
    /// 当前标签页
    pub tab: AiTab,
    /// 是否暂停 AI 处理
    pub paused: bool,
    /// 手动触发识别标志
    pub manual_classify_pending: bool,
    /// 已处理的连接会话 ID（用于检测重连后重置索引）
    pub last_session_id: u64,
    /// 最近检测到的异常（供 terminal 面板共享读取）
    pub last_shared_anomaly: Option<AnomalyAlert>,

    // ---- LLM 对话相关（feature gate）----
    #[cfg(feature = "llm-online")]
    pub llm_config: LlmConfig,
    #[cfg(feature = "llm-online")]
    pub llm_messages: Vec<LlmMessage>,
    /// LLM 后台任务结果槽
    #[cfg(feature = "llm-online")]
    pub llm_result_slot: LlmResultSlot,
    /// LLM 后台任务完成标志
    #[cfg(feature = "llm-online")]
    pub llm_done_flag: LlmDoneFlag,
    /// LLM 是否正在处理
    #[cfg(feature = "llm-online")]
    pub llm_busy: bool,
    /// LLM 输入框内容
    #[cfg(feature = "llm-online")]
    pub llm_input: String,
    /// LLM 配置编辑缓冲（用于 UI 编辑，保存时拷贝到 llm_config）
    #[cfg(feature = "llm-online")]
    pub llm_config_edit: LlmConfig,
    /// LLM 配置面板是否展开
    #[cfg(feature = "llm-online")]
    pub llm_config_expanded: bool,

    // ---- Agent 7 步闭环相关（D.3 新增）----
    /// 选中的任务模板索引
    pub agent_template_idx: usize,
    /// 自定义任务目标
    pub agent_goal_input: String,
    /// 上次运行的任务结果
    pub agent_result: Option<miconlab_ai::agent::TaskResult>,
    /// 历史任务记录
    pub agent_history: Vec<AgentHistoryEntry>,
    /// 是否已运行过
    pub agent_has_run: bool,

    // ---- Ollama 本地模型识别（feature gate，Phase 2）----
    /// Ollama 配置（Phase 3 设置页可编辑，当前用默认值）
    #[cfg(feature = "ollama")]
    pub ollama_config: OllamaConfig,
    /// Ollama 服务状态（后台线程检测结果，None = 尚未检测）
    #[cfg(feature = "ollama")]
    pub ollama_status: Option<OllamaStatus>,
    /// 状态最近检测时刻（30s 轮询节流）
    #[cfg(feature = "ollama")]
    pub ollama_status_checked_at: Option<Instant>,
    /// 状态检测后台任务结果槽
    #[cfg(feature = "ollama")]
    pub ollama_status_slot: OllamaStatusSlot,
    /// 状态检测进行中
    #[cfg(feature = "ollama")]
    pub ollama_status_busy: bool,
    /// 协议识别后台任务结果槽
    #[cfg(feature = "ollama")]
    pub ollama_result_slot: OllamaResultSlot,
    /// 协议识别后台任务完成标志
    #[cfg(feature = "ollama")]
    pub ollama_done_flag: LlmDoneFlag,
    /// 协议识别进行中
    #[cfg(feature = "ollama")]
    pub ollama_busy: bool,
    /// 最近一次 LLM 协议识别结果
    #[cfg(feature = "ollama")]
    pub ollama_result: Option<ProtocolIdentifyResult>,
    /// LLM 结论与规则引擎的交叉验证
    #[cfg(feature = "ollama")]
    pub ollama_cross: Option<CrossCheck>,
    /// 自动识别增强开关（默认开，规则置信度 <0.7 时自动触发）
    #[cfg(feature = "ollama")]
    pub ollama_auto_enabled: bool,
    /// Ollama 配置签名（设置页联动检测，Phase 4）
    #[cfg(feature = "ollama")]
    pub ollama_cfg_sig: Option<String>,
    /// 上次自动触发时刻（10s 节流，避免连续调用 LLM）
    #[cfg(feature = "ollama")]
    pub ollama_last_auto_at: Option<Instant>,
}

/// Agent 历史记录条目
#[derive(Clone)]
pub struct AgentHistoryEntry {
    pub goal: String,
    pub result: miconlab_ai::agent::TaskResult,
    pub timestamp: String,
}

impl Default for AiAssistantState {
    fn default() -> Self {
        let mut engine = AiEngine::new();
        engine.detector.set_sensitivity(0.7);
        Self {
            engine,
            anomalies: Vec::new(),
            diagnoses: Vec::new(),
            sensitivity: 0.7,
            last_rx_seq: 0,
            last_classify_at: None,
            last_classification: None,
            last_features: None,
            last_sample_hex: String::new(),
            tab: AiTab::Protocol,
            paused: false,
            manual_classify_pending: false,
            last_session_id: 0,
            last_shared_anomaly: None,
            #[cfg(feature = "llm-online")]
            llm_config: LlmConfig::default(),
            #[cfg(feature = "llm-online")]
            llm_messages: Vec::new(),
            #[cfg(feature = "llm-online")]
            llm_result_slot: Arc::new(Mutex::new(None)),
            #[cfg(feature = "llm-online")]
            llm_done_flag: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "llm-online")]
            llm_busy: false,
            #[cfg(feature = "llm-online")]
            llm_input: String::new(),
            #[cfg(feature = "llm-online")]
            llm_config_edit: LlmConfig::default(),
            #[cfg(feature = "llm-online")]
            llm_config_expanded: false,
            // D.3 Agent
            agent_template_idx: 0,
            agent_goal_input: String::new(),
            agent_result: None,
            agent_history: Vec::new(),
            agent_has_run: false,
            // Ollama 本地模型识别
            #[cfg(feature = "ollama")]
            ollama_config: OllamaConfig::default(),
            #[cfg(feature = "ollama")]
            ollama_status: None,
            #[cfg(feature = "ollama")]
            ollama_status_checked_at: None,
            #[cfg(feature = "ollama")]
            ollama_status_slot: Arc::new(Mutex::new(None)),
            #[cfg(feature = "ollama")]
            ollama_status_busy: false,
            #[cfg(feature = "ollama")]
            ollama_result_slot: Arc::new(Mutex::new(None)),
            #[cfg(feature = "ollama")]
            ollama_done_flag: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "ollama")]
            ollama_busy: false,
            #[cfg(feature = "ollama")]
            ollama_result: None,
            #[cfg(feature = "ollama")]
            ollama_cross: None,
            #[cfg(feature = "ollama")]
            ollama_auto_enabled: true,
            #[cfg(feature = "ollama")]
            ollama_cfg_sig: None,
            #[cfg(feature = "ollama")]
            ollama_last_auto_at: None,
        }
    }
}

/// AI 助手标签页
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTab {
    Protocol,
    Anomaly,
    Diagnosis,
    Agent,
    Chat,
    Knowledge,
}

impl AiTab {
    fn label(self) -> &'static str {
        match self {
            AiTab::Protocol => crate::i18n::t("Protocol"),
            AiTab::Anomaly => crate::i18n::t("Anomaly"),
            AiTab::Diagnosis => crate::i18n::t("Diagnosis"),
            AiTab::Agent => crate::i18n::t("Agent Loop"),
            AiTab::Chat => crate::i18n::t("AI Chat"),
            AiTab::Knowledge => crate::i18n::t("Knowledge"),
        }
    }
}

const STATE_ID: &str = "ai_assistant_state";
const SHARED_ANOMALY_ID: &str = "ai_shared_anomaly";
const ANOMALY_MAX: usize = 100;

/// 渲染 AI 助手面板
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let zh = state.language == crate::i18n::Language::Zh;

    // 取出持久化状态克隆（避免 CentralPanel 闭包内 ctx.data_mut() 死锁）
    let mut ai_state: AiAssistantState = ctx
        .data_mut(|d| d.get_temp::<AiAssistantState>(Id::new(STATE_ID)))
        .unwrap_or_default();

    // 检测连接会话变化 → 重置增量序号（重连后 rx_buffer 可能已被清空）
    if ai_state.last_session_id != state.connection_session_id {
        ai_state.last_session_id = state.connection_session_id;
        ai_state.last_rx_seq = state.rx_entry_seq;
    }

    // Ollama 配置联动：设置页是唯一可变源，签名变化时刷新配置并强制重查状态
    #[cfg(feature = "ollama")]
    {
        crate::settings::sync_panel_config(
            &mut ai_state.ollama_config,
            &mut ai_state.ollama_cfg_sig,
            &mut ai_state.ollama_status,
            &mut ai_state.ollama_status_checked_at,
            &state.settings.ollama,
        );
        ai_state.ollama_auto_enabled = state.settings.ollama.auto_enabled;
    }

    // 同步灵敏度
    if (ai_state.sensitivity - ai_state.engine.detector_sensitivity()).abs() > 0.01 {
        ai_state.engine.detector_set_sensitivity(ai_state.sensitivity);
    }

    // 增量喂入 rx_buffer 中未处理的数据到 AI 引擎
    if !ai_state.paused {
        feed_new_data(&mut ai_state, state);
    }

    // LLM 后台任务结果回收（feature gate）
    #[cfg(feature = "llm-online")]
    if ai_state.llm_busy && ai_state.llm_done_flag.load(Ordering::Relaxed) {
        let result = ai_state.llm_result_slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(res) = result {
            match res {
                Ok(content) => ai_state.llm_messages.push(LlmMessage::assistant(content)),
                Err(e) => ai_state.llm_messages.push(LlmMessage::assistant(format!("❌ {}", e))),
            }
        }
        ai_state.llm_busy = false;
        ai_state.llm_done_flag.store(false, Ordering::Relaxed);
    }

    // ---- Ollama 后台任务回收（feature gate）----
    #[cfg(feature = "ollama")]
    {
        // 1. 状态检测：到期（30s）且空闲 → 后台线程发起（连接超时 5s，绝不阻塞 UI）
        let status_due = ai_state
            .ollama_status_checked_at
            .map_or(true, |t| t.elapsed() > Duration::from_secs(30));
        if status_due && !ai_state.ollama_status_busy {
            ai_state.ollama_status_busy = true;
            let slot = ai_state.ollama_status_slot.clone();
            let cfg = ai_state.ollama_config.clone();
            std::thread::spawn(move || {
                let status = match OllamaClient::new(cfg) {
                    Ok(client) => client.status_blocking(),
                    Err(_) => OllamaStatus::NotRunning,
                };
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
            });
        }
        // 回收状态检测结果
        if ai_state.ollama_status_busy {
            let done = ai_state
                .ollama_status_slot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(status) = done {
                ai_state.ollama_status = Some(status);
                ai_state.ollama_status_checked_at = Some(Instant::now());
                ai_state.ollama_status_busy = false;
            }
        }
        // 2. 协议识别结果回收
        if ai_state.ollama_busy && ai_state.ollama_done_flag.load(Ordering::Relaxed) {
            let res = ai_state
                .ollama_result_slot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if let Some(res) = res {
                match res {
                    Ok(r) => {
                        // 交叉验证：规则引擎结论 vs LLM 结论（用英文标签，ASCII 归一化可比）
                        let rule_hint = ai_state
                            .last_classification
                            .map(|(c, cf)| (c.label_en().to_string(), cf));
                        ai_state.ollama_cross =
                            Some(miconlab_ai::identify::cross_check(&rule_hint, &r.protocol));
                        ai_state.ollama_result = Some(r);
                    }
                    Err(e) => {
                        // 静默降级：仅记日志，不打扰用户
                        log::warn!("{}", crate::i18n::tf("Ollama protocol identification failed (silent fallback): {error}", &[("{error}", "{error}", &format!("{}", e))]));
                    }
                }
            }
            ai_state.ollama_busy = false;
            ai_state.ollama_done_flag.store(false, Ordering::Relaxed);
        }
    }

    egui::CentralPanel::default()
        .frame(
            egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::same(16.0)),
        )
        .show(ctx, |ui| {
            // 标题
            ui.horizontal(|ui| {
                ui.label(RichText::new("🤖").size(20.0));
                ui.label(
                    RichText::new(crate::i18n::t("AI Assistant"))
                        .color(theme::TEXT_PRIMARY)
                        .strong()
                        .size(18.0),
                );
            });
            ui.label(
                RichText::new(crate::i18n::t("Native intelligent protocol recognition, anomaly detection, root cause diagnosis, optional online LLM"))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // 标签页栏
            ui.horizontal(|ui| {
                let tabs = [
                    AiTab::Protocol,
                    AiTab::Anomaly,
                    AiTab::Diagnosis,
                    AiTab::Agent,
                    AiTab::Chat,
                    AiTab::Knowledge,
                ];
                for &tab in &tabs {
                    let is_active = ai_state.tab == tab;
                    let label = tab.label();
                    let btn = egui::Button::new(
                        RichText::new(label).color(if is_active {
                            Color32::BLACK
                        } else {
                            theme::TEXT_PRIMARY
                        }),
                    )
                    .fill(if is_active { theme::ACCENT } else { theme::BG_ELEVATED })
                    .stroke(if is_active {
                        egui::Stroke::new(1.0_f32, theme::ACCENT)
                    } else {
                        egui::Stroke::new(1.0_f32, theme::BORDER)
                    })
                    .min_size(Vec2::new(90.0, 26.0));
                    if ui.add(btn).clicked() {
                        ai_state.tab = tab;
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let paused = ai_state.paused;
                    let (icon, color) = if paused { ("▶", theme::SUCCESS) } else { ("⏸", theme::WARNING) };
                    if ui
                        .button(RichText::new(icon).color(color).size(14.0))
                        .on_hover_text(if paused {
                            crate::i18n::t("Resume AI")
                        } else {
                            crate::i18n::t("Pause AI")
                        })
                        .clicked()
                    {
                        ai_state.paused = !ai_state.paused;
                        if !ai_state.paused {
                            // 恢复时同步序号到当前值，避免处理积压
                            ai_state.last_rx_seq = state.rx_entry_seq;
                        }
                    }
                });
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // 根据当前标签页渲染内容
            match ai_state.tab {
                AiTab::Protocol => render_protocol_tab(ui, &mut ai_state, state),
                AiTab::Anomaly => render_anomaly_tab(ui, &mut ai_state),
                AiTab::Diagnosis => render_diagnosis_tab(ui, &mut ai_state, state),
                AiTab::Agent => render_agent_tab(ui, &mut ai_state, state, zh),
                #[cfg(feature = "llm-online")]
                AiTab::Chat => render_chat_tab(ui, &mut ai_state, state, zh),
                #[cfg(not(feature = "llm-online"))]
                AiTab::Chat => render_chat_disabled(ui, zh),
                AiTab::Knowledge => render_knowledge_tab(ui, &mut ai_state, zh),
            }
        });

    // 写回持久化状态（先 take 出 last_shared_anomaly 避免重复写入共享 key）
    let shared_anomaly = ai_state.last_shared_anomaly.take();
    ctx.data_mut(|d| {
        *d.get_temp_mut_or_default::<AiAssistantState>(Id::new(STATE_ID)) = ai_state;
    });

    // 如果本帧检测到新异常，写入共享 key 供 terminal 面板读取
    if let Some(anomaly) = shared_anomaly {
        ctx.data_mut(|d| {
            *d.get_temp_mut_or_default::<Option<AnomalyAlert>>(Id::new(SHARED_ANOMALY_ID)) = Some(anomaly);
        });
    }
}

/// 增量喂入 rx_buffer 中未处理的数据到 AI 引擎
///
/// 同时更新异常检测基线，并节流触发协议识别（每 500ms 最多一次）。
/// 检测到异常时，写入共享 IdTypeMap key 供 terminal 面板读取显示。
fn feed_new_data(ai_state: &mut AiAssistantState, state: &AppState) {
    // 基于全局单调递增的条目序号计算新增条目数。
    // rx_buffer 满后会从头部 drain，索引方案会永久停滞或重复处理，序号方案天然免疫。
    let new_count = state
        .rx_entry_seq
        .saturating_sub(ai_state.last_rx_seq) as usize;
    ai_state.last_rx_seq = state.rx_entry_seq;
    if new_count == 0 {
        return;
    }

    // 从末尾向前取最近 new_count 条（drain 后可能不足 new_count 条，取现存即可）
    // 逆序遍历时各段按时间倒序收集，最后 reverse 恢复时间顺序
    let mut segments: Vec<&[u8]> = Vec::new();
    for entry in state.rx_buffer.iter().rev().take(new_count) {
        if entry.direction == crate::state::Direction::Rx {
            segments.push(&entry.data);
        }
    }
    segments.reverse();
    let new_bytes: Vec<u8> = segments.concat();

    if new_bytes.is_empty() {
        return;
    }

    // 喂入异常检测器
    if let Some(alert) = ai_state.engine.ingest(&new_bytes) {
        ai_state.anomalies.insert(0, alert.clone());
        if ai_state.anomalies.len() > ANOMALY_MAX {
            ai_state.anomalies.truncate(ANOMALY_MAX);
        }
        // 写入共享 key 供 terminal 面板读取
        // 注意：此处无法访问 ctx（feed_new_data 在 CentralPanel 闭包内调用），
        // 共享写入在 show() 函数末尾写回状态时统一处理。
        // 改为：通过 ai_state.last_shared_anomaly 字段传递。
        ai_state.last_shared_anomaly = Some(alert);
    }

    // 节流触发协议识别（每 500ms 最多一次，或手动触发）
    let now = Instant::now();
    let should_classify = ai_state.manual_classify_pending
        || ai_state
            .last_classify_at
            .map_or(true, |t| now.duration_since(t) > Duration::from_millis(500));

    if should_classify {
        ai_state.manual_classify_pending = false;
        ai_state.last_classify_at = Some(now);

        // 取最近 256 字节作为识别样本
        let sample_len = new_bytes.len().min(256);
        let sample = &new_bytes[new_bytes.len() - sample_len..];
        let (class, conf) = ai_state.engine.classify(sample, None);
        ai_state.last_classification = Some((class, conf));
        ai_state.last_features = Some(StreamFeatures::extract(sample, None));
        ai_state.last_sample_hex = format_hex_truncated(sample, 64);

        // Ollama 自动增强：规则置信度 < 0.7 且本地模型就绪 → 后台 LLM 识别
        #[cfg(feature = "ollama")]
        if conf < 0.7 && ai_state.ollama_auto_enabled {
            // 10s 节流（单次识别 2-5s，避免连续调用）
            let throttled = ai_state
                .ollama_last_auto_at
                .map_or(false, |t| t.elapsed() < Duration::from_secs(10));
            if !throttled {
                let ev = build_evidence(
                    state,
                    format_hex_truncated(sample, 256),
                    ai_state.last_features.clone(),
                    Some((class.label_en().to_string(), conf)),
                    state.config.baud_rate,
                );
                if spawn_ollama_identify(ai_state, ev) {
                    ai_state.ollama_last_auto_at = Some(Instant::now());
                }
            }
        }
    }
}

/// 构造协议识别证据包（Ollama）
#[cfg(feature = "ollama")]
fn build_evidence(
    state: &AppState,
    sample_hex: String,
    features: Option<StreamFeatures>,
    rule_hint: Option<(String, f32)>,
    baud: u32,
) -> EvidenceBundle {
    let mut ev = EvidenceBundle::new();
    if let Some(p) = state.port_list.get(state.selected_port_index) {
        ev.vid_pid = p.vid.zip(p.pid);
        ev.port_name = Some(p.name.clone());
    }
    ev.sample_hex = sample_hex;
    ev.features = features;
    ev.rule_hint = rule_hint;
    ev.extra_notes.push(crate::i18n::tf("Current baud rate: {baud}", &[("{baud}", "{baud}", &baud.to_string())]));
    ev
}

/// 启动 Ollama 协议识别后台线程。
///
/// 返回 false 表示未启动（服务未就绪/已有任务进行中），调用方据此决定是否更新节流时刻。
#[cfg(feature = "ollama")]
fn spawn_ollama_identify(ai_state: &mut AiAssistantState, ev: EvidenceBundle) -> bool {
    if ai_state.ollama_busy {
        return false;
    }
    if !matches!(ai_state.ollama_status, Some(OllamaStatus::Ready(_))) {
        return false;
    }
    ai_state.ollama_busy = true;
    ai_state.ollama_done_flag.store(false, Ordering::Relaxed);
    let slot = ai_state.ollama_result_slot.clone();
    let flag = ai_state.ollama_done_flag.clone();
    let cfg = ai_state.ollama_config.clone();
    std::thread::spawn(move || {
        let res = OllamaClient::new(cfg)
            .map_err(|e| e.to_string())
            .and_then(|client| {
                miconlab_ai::identify::identify_protocol(&client, &ev).map_err(|e| e.to_string())
            });
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
        flag.store(true, Ordering::Relaxed);
    });
    true
}

/// 格式化 HEX 字符串，最多 max_bytes 字节
fn format_hex_truncated(data: &[u8], max_bytes: usize) -> String {
    let truncated = if data.len() > max_bytes { &data[..max_bytes] } else { data };
    let hex: Vec<String> = truncated.iter().map(|b| format!("{:02X}", b)).collect();
    let mut s = hex.join(" ");
    if data.len() > max_bytes {
        s.push_str(&format!(" ... ({} bytes)", data.len()));
    }
    s
}

// ==================== 协议识别标签页 ====================

fn render_protocol_tab(
    ui: &mut egui::Ui,
    ai_state: &mut AiAssistantState,
    state: &AppState,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("Live Protocol Recognition"))
                .color(theme::CYAN)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(crate::i18n::t("Recognize Now"))
                .clicked()
            {
                ai_state.manual_classify_pending = true;
            }
            ui.label(
                RichText::new(if state.is_connected() {
                    crate::i18n::t("● Connected")
                } else {
                    crate::i18n::t("○ Disconnected")
                })
                .color(if state.is_connected() { theme::SUCCESS } else { theme::TEXT_DISABLED })
                .small(),
            );
        });
    });
    ui.add_space(8.0);

    // ---- Ollama 本地模型状态行（feature gate）----
    #[cfg(feature = "ollama")]
    {
        ui.horizontal(|ui| {
            // 状态指示：绿=就绪 / 黄=缺模型 / 灰=未运行或检测中
            let (dot, color, status_text) = match &ai_state.ollama_status {
                Some(OllamaStatus::Ready(_)) => (
                    "🟢",
                    theme::SUCCESS,
                    crate::i18n::t("Local model ready"),
                ),
                Some(OllamaStatus::ModelMissing(_)) => (
                    "🟡",
                    theme::WARNING,
                    crate::i18n::t("Running, model missing"),
                ),
                Some(OllamaStatus::NotRunning) => (
                    "⚪",
                    theme::TEXT_DISABLED,
                    crate::i18n::t("Local model not running"),
                ),
                None => (
                    "⚪",
                    theme::TEXT_DISABLED,
                    crate::i18n::t("Detecting local model…"),
                ),
            };
            ui.label(
                RichText::new(format!("{} {}", dot, status_text))
                    .color(color)
                    .small(),
            )
            .on_hover_text(&ai_state.ollama_config.base_url);
            ui.label(
                RichText::new(format!("({})", ai_state.ollama_config.model))
                    .color(theme::TEXT_DISABLED)
                    .small(),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 手动触发识别
                let can_identify = matches!(ai_state.ollama_status, Some(OllamaStatus::Ready(_)))
                    && !ai_state.ollama_busy
                    && ai_state.last_features.is_some();
                let btn_text = if ai_state.ollama_busy {
                    crate::i18n::t("Identifying…")
                } else { crate::i18n::t("Identify via Local Model") };
                if ui
                    .add_enabled(can_identify, egui::Button::new(btn_text).small())
                    .on_hover_text(crate::i18n::t("Identify protocol with local Ollama model (fully offline)"))
                    .clicked()
                {
                    let rule_hint = ai_state
                        .last_classification
                        .map(|(c, cf)| (c.label_en().to_string(), cf));
                    let ev = build_evidence(
                        state,
                        ai_state.last_sample_hex.clone(),
                        ai_state.last_features.clone(),
                        rule_hint,
                        state.config.baud_rate,
                    );
                    spawn_ollama_identify(ai_state, ev);
                }
                // 自动增强状态（只读，统一在「工具箱 → AI 本地模型」设置）
                ui.label(
                    RichText::new(if ai_state.ollama_auto_enabled {
                        crate::i18n::t("Auto-enhance: ON")
                    } else {
                        crate::i18n::t("Auto-enhance: OFF")
                    })
                    .color(if ai_state.ollama_auto_enabled {
                        theme::SUCCESS
                    } else {
                        theme::TEXT_DISABLED
                    })
                    .small(),
                )
                .on_hover_text(crate::i18n::t("Auto-invoke local model when rule confidence < 70%; configure in Tools panel (persisted)"));
            });
        });
        ui.add_space(6.0);
    }

    // 识别结果卡片
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            match ai_state.last_classification {
                None => {
                    ui.label(
                        RichText::new(crate::i18n::t("No recognition yet. Connect a device and exchange data — AI will recognize the protocol automatically."))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                    );
                }
                Some((class, conf)) => {
                    let label = crate::i18n::t_owned(class.label_en());
                    let conf_color = if conf >= 0.7 {
                        theme::SUCCESS
                    } else if conf >= 0.4 {
                        theme::WARNING
                    } else {
                        theme::ERROR
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::i18n::t("Protocol")).color(theme::TEXT_SECONDARY));
                        ui.label(RichText::new(label).color(theme::ACCENT).strong().size(16.0));
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::i18n::t("Confidence")).color(theme::TEXT_SECONDARY));
                        ui.label(RichText::new(format!("{:.1}%", conf * 100.0)).color(conf_color).strong());
                        // 置信度条
                        let bar_width = 200.0;
                        let bar_height = 8.0;
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(bar_width, bar_height), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 0.0, theme::BG_SURFACE);
                        let fill_width = bar_width * conf.clamp(0.0, 1.0);
                        let mut fill_rect = rect;
                        fill_rect.set_width(fill_width);
                        ui.painter().rect_filled(fill_rect, 0.0, conf_color);
                    });
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(crate::i18n::t_owned(class.description_zh()))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    if let Some(baud) = class.recommended_baud() {
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(crate::i18n::tf("💡 Recommended baud: {}", &[("{}", "{}", &format!("{}", baud))]))
                            .color(theme::CYAN)
                            .small(),
                        );
                    }
                }
            }
        });

    ui.add_space(10.0);

    // ---- Ollama 识别结果卡片（feature gate）----
    #[cfg(feature = "ollama")]
    if let Some(result) = &ai_state.ollama_result {
        egui::Frame::group(&ui.style())
            .fill(theme::BG_ELEVATED)
            .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::i18n::t("🧠 Local Model Result"))
                            .color(theme::ACCENT)
                            .strong(),
                    );
                    // 交叉验证徽章
                    match ai_state.ollama_cross {
                        Some(CrossCheck::Agree) => {
                            ui.label(
                                RichText::new(crate::i18n::t("✓ Agrees with rules"))
                                    .color(theme::SUCCESS)
                                    .small(),
                            );
                        }
                        Some(CrossCheck::Conflict) => {
                            let rule_label = ai_state
                                .last_classification
                                .map(|(c, _)| crate::i18n::t_owned(c.label_en()))
                                .unwrap_or_else(|| "-".to_string());
                            ui.label(
                                RichText::new(crate::i18n::tf("⚠ Conflicts with rules (rule: {})", &[("{}", "{}", &format!("{}", rule_label))]))
                                .color(theme::WARNING)
                                .small(),
                            );
                        }
                        _ => {}
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::i18n::t("Protocol")).color(theme::TEXT_SECONDARY));
                    ui.label(RichText::new(&result.protocol).color(theme::ACCENT).strong().size(16.0));
                    let conf_color = if result.confidence >= 0.7 {
                        theme::SUCCESS
                    } else if result.confidence >= 0.4 {
                        theme::WARNING
                    } else {
                        theme::ERROR
                    };
                    ui.label(
                        RichText::new(format!("{:.1}%", result.confidence * 100.0))
                            .color(conf_color)
                            .strong(),
                    );
                });
                if let Some(fmt) = &result.frame_format {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(crate::i18n::tf("Frame format: {}", &[("{}", "{}", &format!("{}", fmt))]))
                        .color(theme::TEXT_SECONDARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                    );
                }
                if let Some(baud) = result.suggested_baud {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(crate::i18n::tf("💡 Suggested baud: {}", &[("{}", "{}", &format!("{}", baud))]))
                        .color(theme::CYAN)
                        .small(),
                    );
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new(&result.reasoning)
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            });
        ui.add_space(10.0);
    }

    // 最近样本
    if !ai_state.last_sample_hex.is_empty() {
        ui.label(RichText::new(crate::i18n::t("Recent Sample")).color(theme::TEXT_SECONDARY).small());
        ui.add_space(2.0);
        egui::Frame::group(&ui.style())
            .fill(theme::BG_DEEP)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(&ai_state.last_sample_hex)
                        .color(theme::RX_COLOR)
                        .family(egui::FontFamily::Monospace)
                        .size(11.0),
                );
            });
    }

    ui.add_space(10.0);

    // 特征向量可视化
    if let Some(features) = &ai_state.last_features {
        ui.label(
            RichText::new(crate::i18n::t("Feature Vector (16-dim)"))
                .color(theme::CYAN)
                .strong(),
        );
        ui.add_space(4.0);
        render_features_visualization(ui, features);
    }
}

/// 渲染特征向量可视化
fn render_features_visualization(ui: &mut egui::Ui, features: &StreamFeatures) {
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            // 16 桶字节直方图
            ui.label(RichText::new(crate::i18n::t("Byte Histogram (16 buckets)")).color(theme::TEXT_SECONDARY).small());
            ui.add_space(4.0);
            let bar_width = 22.0;
            let bar_max_height = 60.0;
            let total_width = bar_width * 16.0 + 15.0 * 2.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(total_width, bar_max_height + 20.0), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let origin_x = rect.min.x;
            let origin_y = rect.max.y - 16.0;

            // 找最大桶值用于归一化
            let max_bucket = features.byte_histogram_16.iter().cloned().fold(0.0f32, f32::max).max(0.01);

            for (i, &val) in features.byte_histogram_16.iter().enumerate() {
                let h = (val / max_bucket * bar_max_height).max(1.0);
                let x = origin_x + i as f32 * (bar_width + 2.0);
                let bar_rect = egui::Rect::from_min_size(
                    egui::pos2(x, origin_y - h),
                    Vec2::new(bar_width, h),
                );
                // 颜色随桶索引渐变（青→蓝）
                let t = i as f32 / 15.0;
                let r = (0x4F as f32 * t).floor() as u8;
                let g = (0xB4 as f32 - (0xB4 as f32 - 0xC3 as f32) * t).floor() as u8;
                let b = (0xD8 as f32 + (0xF7 as f32 - 0xD8 as f32) * t).floor() as u8;
                painter.rect_filled(bar_rect, 2.0, Color32::from_rgb(r, g, b));
                // 桶索引标签
                let label = format!("{:02X}", i * 16);
                painter.text(
                    egui::pos2(x + bar_width / 2.0, origin_y + 2.0),
                    egui::Align2::CENTER_TOP,
                    &label,
                    egui::FontId::proportional(8.0),
                    theme::TEXT_DISABLED,
                );
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // 数值特征网格
            egui::Grid::new("ai_features_grid")
                .num_columns(2)
                .spacing([20.0, 4.0])
                .show(ui, |ui| {
                    feature_row(ui, crate::i18n::t("Shannon Entropy"), format!("{:.3} bits", features.shannon_entropy), 0.0, 8.0, features.shannon_entropy);
                    feature_row(ui, crate::i18n::t("Printable Ratio"), format!("{:.1}%", features.printable_ratio * 100.0), 0.0, 1.0, features.printable_ratio);
                    feature_row(ui, crate::i18n::t("Null Ratio"), format!("{:.1}%", features.null_ratio * 100.0), 0.0, 1.0, features.null_ratio);
                    feature_row(ui, crate::i18n::t("High Byte Ratio"), format!("{:.1}%", features.high_byte_ratio * 100.0), 0.0, 1.0, features.high_byte_ratio);
                    feature_row(ui, crate::i18n::t("Control Ratio"), format!("{:.1}%", features.control_ratio * 100.0), 0.0, 1.0, features.control_ratio);
                    feature_row(ui, crate::i18n::t("Avg Byte Value"), format!("{:.1}", features.avg_byte_value), 0.0, 255.0, features.avg_byte_value / 255.0);
                    feature_row(ui, crate::i18n::t("Byte Std Dev"), format!("{:.1}", features.byte_value_std), 0.0, 127.0, features.byte_value_std / 127.0);
                    feature_row(ui, crate::i18n::t("Max Run Length"), format!("{}", features.max_run_length), 0.0, 64.0, (features.max_run_length as f32).min(64.0) / 64.0);
                });

            if !features.detected_headers.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(crate::i18n::t("Detected Headers")).color(theme::TEXT_SECONDARY).small());
                ui.horizontal_wrapped(|ui| {
                    for &h in &features.detected_headers {
                        let label = crate::i18n::t_owned(h.label_en());
                        egui::Frame::group(&ui.style())
                            .fill(theme::BG_SURFACE)
                            .stroke(egui::Stroke::new(1.0_f32, theme::CYAN))
                            .inner_margin(egui::Margin::same(4.0))
                            .show(ui, |ui| {
                                ui.label(RichText::new(label).color(theme::CYAN).small().strong());
                            });
                    }
                });
            }
        });
}

/// 特征行（标签 + 值 + 进度条）
fn feature_row(ui: &mut egui::Ui, label: &str, value: String, _min: f32, _max: f32, ratio: f32) {
    ui.label(RichText::new(label).color(theme::TEXT_SECONDARY).small());
    ui.horizontal(|ui| {
        ui.label(RichText::new(value).color(theme::TEXT_PRIMARY).small().family(egui::FontFamily::Monospace));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(80.0, 6.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, theme::BG_SURFACE);
        let mut fill = rect;
        fill.set_width(rect.width() * ratio.clamp(0.0, 1.0));
        ui.painter().rect_filled(fill, 2.0, theme::ACCENT_DIM);
    });
    ui.end_row();
}

// ==================== 异常检测标签页 ====================

fn render_anomaly_tab(ui: &mut egui::Ui, ai_state: &mut AiAssistantState) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::t("Anomaly Alert Stream")).color(theme::CYAN).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(crate::i18n::t("Clear")).clicked() {
                ai_state.anomalies.clear();
            }
        });
    });

    ui.add_space(6.0);

    // 灵敏度调节
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::t("Sensitivity")).color(theme::TEXT_SECONDARY).small());
        let mut sens = ai_state.sensitivity;
        ui.add(egui::Slider::new(&mut sens, 0.0..=1.0).suffix("").fixed_decimals(2));
        ai_state.sensitivity = sens;
        ui.label(
            RichText::new(if sens >= 0.8 {
                crate::i18n::t("High")
            } else if sens >= 0.4 {
                crate::i18n::t("Medium")
            } else {
                crate::i18n::t("Low")
            })
            .color(theme::TEXT_DISABLED)
            .small(),
        );
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // 告警计数
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::tf("{} alerts", &[("{}", "{}", &format!("{}", ai_state.anomalies.len()))])).color(theme::TEXT_SECONDARY).small());
    });

    ui.add_space(4.0);

    // 告警列表（滚动）
    egui::ScrollArea::vertical()
        .max_height(ui.available_height() - 20.0)
        .show(ui, |ui| {
            if ai_state.anomalies.is_empty() {
                ui.label(
                    RichText::new(crate::i18n::t("No anomalies yet. Detection starts after enough data establishes a baseline."))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            } else {
                for alert in &ai_state.anomalies {
                    render_anomaly_card(ui, alert);
                    ui.add_space(4.0);
                }
            }
        });
}

/// 渲染单个异常告警卡片
fn render_anomaly_card(ui: &mut egui::Ui, alert: &AnomalyAlert) {
    let (icon, color) = match alert.kind {
        AnomalyKind::EntropySpike | AnomalyKind::EntropyDrop => ("📈", theme::WARNING),
        AnomalyKind::DistributionShift => ("🔀", theme::WARNING),
        AnomalyKind::TimingAnomaly => ("⏱", theme::WARNING),
        AnomalyKind::NullBurst => ("⭕", theme::ERROR),
        AnomalyKind::HighByteBurst => ("🔢", theme::WARNING),
    };
    let kind_label = crate::i18n::t_owned(alert.kind.label_en());
    let sev_color = if alert.severity >= 0.7 {
        theme::ERROR
    } else if alert.severity >= 0.4 {
        theme::WARNING
    } else {
        theme::TEXT_SECONDARY
    };

    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, sev_color))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(16.0));
                ui.label(RichText::new(kind_label).color(color).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0}%", alert.severity * 100.0)).color(sev_color).small().strong());
                });
            });
            ui.add_space(2.0);
            ui.label(RichText::new(&alert.message).color(theme::TEXT_SECONDARY).small());
            if !alert.data_hex.is_empty() {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(&alert.data_hex)
                        .color(theme::RX_COLOR)
                        .family(egui::FontFamily::Monospace)
                        .size(10.0),
                );
            }
        });
}

// ==================== 智能诊断标签页 ====================

fn render_diagnosis_tab(
    ui: &mut egui::Ui,
    ai_state: &mut AiAssistantState,
    state: &AppState,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::t("Smart Diagnosis")).color(theme::CYAN).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(crate::i18n::t("Re-diagnose")).clicked() {
                let ctx = build_diagnostic_context(state, ai_state);
                ai_state.diagnoses = ai_state.engine.diagnose(&ctx);
            }
        });
    });

    ui.add_space(6.0);
    ui.label(
        RichText::new(crate::i18n::t("Based on current connection state, errors and data features, AI infers root causes and suggests actions."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // 自动诊断（每帧根据状态变化触发，这里简化为有错误时自动诊断）
    if state.last_error.is_some() && ai_state.diagnoses.is_empty() {
        let ctx = build_diagnostic_context(state, ai_state);
        ai_state.diagnoses = ai_state.engine.diagnose(&ctx);
    }
    if state.last_error.is_none() && !ai_state.diagnoses.is_empty() {
        // 错误已清除，清空诊断（除非有非错误级别的诊断）
        ai_state.diagnoses.retain(|d| d.severity == Severity::Info);
    }

    if ai_state.diagnoses.is_empty() {
        ui.label(
            RichText::new(crate::i18n::t("✓ No diagnostic suggestions. System is running normally."))
            .color(theme::SUCCESS)
            .small(),
        );
    } else {
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 20.0)
            .show(ui, |ui| {
                for d in &ai_state.diagnoses {
                    render_diagnosis_card(ui, d);
                    ui.add_space(4.0);
                }
            });
    }
}

/// 构建诊断上下文
fn build_diagnostic_context(state: &AppState, ai_state: &AiAssistantState) -> DiagnosticContext {
    let mut ctx = DiagnosticContext::new();
    ctx.error_message = state.last_error.clone();
    ctx.is_connected = state.is_connected();
    ctx.baud_rate = state.config.baud_rate;
    ctx.port_name = state.selected_port_name().map(|s| s.to_string());
    ctx.protocol_class = ai_state.last_classification.map(|(c, _)| c);
    ctx.protocol_confidence = ai_state.last_classification.map(|(_, c)| c).unwrap_or(0.0);
    // 最近 256 字节接收数据
    ctx.last_rx_data = state
        .rx_buffer
        .iter()
        .rev()
        .filter(|e| e.direction == crate::state::Direction::Rx)
        .flat_map(|e| e.data.iter().copied())
        .take(256)
        .collect::<Vec<u8>>()
        .into_iter()
        .rev()
        .collect();
    // 填充 TX/RX 字节统计与最近发送数据
    ctx.tx_bytes = state.bytes_tx;
    ctx.rx_bytes = state.bytes_rx;
    ctx.last_tx_data = state
        .rx_buffer
        .iter()
        .rev()
        .filter(|e| e.direction == crate::state::Direction::Tx)
        .flat_map(|e| e.data.iter().copied())
        .take(256)
        .collect::<Vec<u8>>()
        .into_iter()
        .rev()
        .collect();
    // 填充串扰检测报告（从 core 类型转换为 ai 镜像类型）
    ctx.crosstalk = state.crosstalk_report.as_ref().map(convert_crosstalk_report);
    ctx
}

/// 将 miconlab_core::crosstalk::CrosstalkReport 转换为 miconlab_ai::diagnose::CrosstalkReport
///
/// 两个类型字段相同但是不同 crate 的镜像，需要手动转换。
fn convert_crosstalk_report(r: &miconlab_core::crosstalk::CrosstalkReport) -> AiCrosstalkReport {
    AiCrosstalkReport {
        echo_ratio: r.echo_ratio,
        rx_tx_ratio: r.rx_tx_ratio,
        is_loopback: r.is_loopback,
        latency_us: r.latency_us,
        pattern: match r.pattern {
            miconlab_core::crosstalk::EchoPattern::Exact => AiEchoPattern::Exact,
            miconlab_core::crosstalk::EchoPattern::BitShifted => AiEchoPattern::BitShifted,
            miconlab_core::crosstalk::EchoPattern::HighByteDominant => {
                AiEchoPattern::HighByteDominant
            }
            miconlab_core::crosstalk::EchoPattern::None => AiEchoPattern::None,
        },
        high_byte_ratio: r.high_byte_ratio,
        tx_total: r.tx_total,
        rx_total: r.rx_total,
    }
}

/// 渲染诊断结果卡片
fn render_diagnosis_card(ui: &mut egui::Ui, d: &DiagnosisResult) {
    let (icon, color) = match d.severity {
        Severity::Info => ("ℹ", theme::CYAN),
        Severity::Warning => ("⚠", theme::WARNING),
        Severity::Error => ("✗", theme::ERROR),
        Severity::Critical => ("☠", theme::ERROR),
    };
    let sev_label = crate::i18n::t_owned(d.severity.label_en());

    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, color))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).size(16.0).color(color));
                ui.label(RichText::new(format!("[{}] {}", sev_label, crate::i18n::t_owned(&d.diagnosis))).color(theme::TEXT_PRIMARY).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0}%", d.confidence * 100.0)).color(theme::TEXT_DISABLED).small());
                });
            });
            if !d.suggestions.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(crate::i18n::t("Suggestions")).color(theme::TEXT_SECONDARY).small());
                for (i, s) in d.suggestions.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}. ", i + 1)).color(theme::ACCENT).small());
                        ui.label(RichText::new(crate::i18n::t_owned(s)).color(theme::TEXT_PRIMARY).small());
                    });
                }
            }
        });
}

// ==================== AI 对话标签页 ====================

#[cfg(feature = "llm-online")]
fn render_chat_tab(ui: &mut egui::Ui, ai_state: &mut AiAssistantState, state: &AppState, zh: bool) {
    // 配置面板（可折叠）
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::t("AI Chat (Online LLM)")).color(theme::CYAN).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let cfg_label = if ai_state.llm_config_expanded {
                crate::i18n::t("Hide Config ▲")
            } else {
                crate::i18n::t("Show Config ▼")
            };
            if ui.button(cfg_label).clicked() {
                ai_state.llm_config_expanded = !ai_state.llm_config_expanded;
            }
        });
    });

    if ai_state.llm_config_expanded {
        ui.add_space(6.0);
        render_llm_config(ui, ai_state, zh);
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
    } else {
        // 显示配置状态摘要
        ui.add_space(4.0);
        let configured = ai_state.llm_config.is_configured();
        ui.label(
            RichText::new(if configured {
                crate::i18n::tf("✓ Configured: {} @ {}", &[("{}", "{}", &format!("{}", ai_state.llm_config.model)), ("{}", "{}", &format!("{}", ai_state.llm_config.base_url))])
            } else { crate::i18n::t("⚠ API key not configured, expand config panel to set up").to_string() })
            .color(if configured { theme::SUCCESS } else { theme::WARNING })
            .small(),
        );
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
    }

    // 对话历史
    let avail_height = ui.available_height();
    egui::ScrollArea::vertical()
        .max_height(avail_height - 60.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if ai_state.llm_messages.is_empty() {
                ui.label(
                    RichText::new(crate::i18n::t("No conversation yet. Type a question below, e.g.:\n• Analyze this HEX data protocol\n• How to read version after STM32 Bootloader 0x7F sync\n• How to calculate Modbus RTU CRC16"))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            } else {
                for msg in &ai_state.llm_messages {
                    render_chat_message(ui, msg, zh);
                    ui.add_space(4.0);
                }
            }
            if ai_state.llm_busy {
                ui.add_space(4.0);
                ui.label(RichText::new(crate::i18n::t("⏳ AI thinking...")).color(theme::ACCENT).small());
            }
        });

    ui.add_space(6.0);

    // 输入框 + 发送按钮
    ui.horizontal(|ui| {
        let send_enabled = !ai_state.llm_busy
            && ai_state.llm_config.is_configured()
            && !ai_state.llm_input.trim().is_empty();
        ui.add(
            egui::TextEdit::multiline(&mut ai_state.llm_input)
                .desired_width(f32::MAX)
                .desired_rows(1)
                .hint_text(crate::i18n::t("Type a question...")),
        );
        if ui
            .add_enabled(
                send_enabled,
                egui::Button::new(RichText::new(crate::i18n::t("Send")).color(Color32::BLACK).strong())
                    .fill(theme::ACCENT)
                    .min_size(Vec2::new(80.0, 30.0)),
            )
            .clicked()
        {
            send_llm_message(ai_state, state, zh);
        }
    });
}

#[cfg(feature = "llm-online")]
fn render_llm_config(ui: &mut egui::Ui, ai_state: &mut AiAssistantState, _zh: bool) {
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            egui::Grid::new("llm_cfg_grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(crate::i18n::t("API Key")).color(theme::TEXT_SECONDARY).small());
                    ui.add(
                        egui::TextEdit::singleline(&mut ai_state.llm_config_edit.api_key)
                            .password(true)
                            .desired_width(320.0)
                            .hint_text("sk-..."),
                    );
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Base URL")).color(theme::TEXT_SECONDARY).small());
                    ui.add(
                        egui::TextEdit::singleline(&mut ai_state.llm_config_edit.base_url)
                            .desired_width(320.0)
                            .hint_text("https://api.openai.com/v1"),
                    );
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Model")).color(theme::TEXT_SECONDARY).small());
                    ui.add(
                        egui::TextEdit::singleline(&mut ai_state.llm_config_edit.model)
                            .desired_width(320.0)
                            .hint_text("gpt-4o-mini"),
                    );
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Max Tokens")).color(theme::TEXT_SECONDARY).small());
                    ui.add(egui::DragValue::new(&mut ai_state.llm_config_edit.max_tokens).range(64..=8192));
                    ui.end_row();

                    ui.label(RichText::new(crate::i18n::t("Temperature")).color(theme::TEXT_SECONDARY).small());
                    ui.add(egui::Slider::new(&mut ai_state.llm_config_edit.temperature, 0.0..=2.0).fixed_decimals(2));
                    ui.end_row();
                });

            ui.add_space(6.0);
            // 预设提供商快捷按钮
            ui.label(RichText::new(crate::i18n::t("Quick Presets")).color(theme::TEXT_SECONDARY).small());
            ui.horizontal_wrapped(|ui| {
                for (name, url, model) in [
                    ("OpenAI", "https://api.openai.com/v1", "gpt-4o-mini"),
                    (crate::i18n::t("Qwen"), "https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen-plus"),
                    ("DeepSeek", "https://api.deepseek.com/v1", "deepseek-chat"),
                    ("Ollama", "http://localhost:11434/v1", "qwen2.5:7b"),
                ] {
                    if ui.button(name).clicked() {
                        ai_state.llm_config_edit.base_url = url.to_string();
                        ai_state.llm_config_edit.model = model.to_string();
                    }
                    ui.add_space(2.0);
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button(crate::i18n::t("Save Config"))
                    .clicked()
                {
                    ai_state.llm_config = ai_state.llm_config_edit.clone();
                }
                if ui
                    .button(crate::i18n::t("Clear Chat"))
                    .clicked()
                {
                    ai_state.llm_messages.clear();
                }
            });
        });
}

#[cfg(feature = "llm-online")]
fn render_chat_message(ui: &mut egui::Ui, msg: &LlmMessage, _zh: bool) {
    let (label, color, align_right) = match msg.role.as_str() {
        "user" => ("🧑", theme::TX_COLOR, true),
        "assistant" => ("🤖", theme::ACCENT, false),
        _ => ("💬", theme::TEXT_SECONDARY, false),
    };

    let frame = if align_right {
        egui::Frame::group(&ui.style())
            .fill(theme::BG_SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, color))
            .inner_margin(egui::Margin::same(8.0))
    } else {
        egui::Frame::group(&ui.style())
            .fill(theme::BG_ELEVATED)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .inner_margin(egui::Margin::same(8.0))
    };

    if align_right {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            ui.label(RichText::new(label).size(16.0));
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width().min(480.0), ui.available_height()),
                egui::Layout::top_down(egui::Align::RIGHT),
                |ui| {
                    frame.show(ui, |ui| {
                        ui.set_max_width(480.0);
                        ui.label(
                            RichText::new(&msg.content)
                                .color(theme::TEXT_PRIMARY)
                                .small(),
                        );
                    });
                },
            );
        });
    } else {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(16.0));
            frame.show(ui, |ui| {
                ui.set_max_width(560.0);
                ui.label(
                    RichText::new(&msg.content)
                        .color(theme::TEXT_PRIMARY)
                        .small(),
                );
            });
        });
    }
}

#[cfg(feature = "llm-online")]
fn send_llm_message(ai_state: &mut AiAssistantState, state: &AppState, zh: bool) {
    let input = ai_state.llm_input.trim().to_string();
    if input.is_empty() {
        return;
    }
    ai_state.llm_input.clear();

    // 构建带项目状态上下文的用户消息
    let chip_hint = state
        .port_list
        .get(state.selected_port_index)
        .and_then(|p| p.vid)
        .map(|v| match v {
            0x1A86 => "CH340",
            0x10C4 => "CP210x",
            0x0403 => "FTDI",
            _ => "Unknown",
        });
    let (echo_ratio, pattern_str) = state.crosstalk_report.as_ref().map_or((None, None), |r| {
        let p = match r.pattern {
            miconlab_core::crosstalk::EchoPattern::Exact => "Exact",
            miconlab_core::crosstalk::EchoPattern::BitShifted => "BitShifted",
            miconlab_core::crosstalk::EchoPattern::HighByteDominant => "HighByteDominant",
            miconlab_core::crosstalk::EchoPattern::None => "None",
        };
        (Some(r.echo_ratio), Some(p))
    });

    let ctx_msg = miconlab_ai::llm::build_context_message(
        state.is_connected(),
        state.config.baud_rate,
        chip_hint,
        echo_ratio,
        pattern_str,
        state.bytes_rx,
        state.bytes_tx,
        zh,
    );
    let full_input = format!("{}\n\n{}", ctx_msg, input);
    ai_state.llm_messages.push(LlmMessage::user(full_input));

    // 准备后台线程所需数据
    let config = ai_state.llm_config.clone();
    let messages = ai_state.llm_messages.clone();
    // 使用项目专项系统提示词（替代通用协议分析提示词）
    let system_prompt = miconlab_ai::llm::system_prompt_miconlab_expert(zh);
    let mut msgs_with_system = vec![LlmMessage::system(system_prompt)];
    msgs_with_system.extend(messages.iter().skip(1).cloned());

    // 重置共享状态
    {
        let mut slot = ai_state.llm_result_slot.lock().unwrap_or_else(|e| e.into_inner());
        *slot = None;
    }
    ai_state.llm_done_flag.store(false, std::sync::atomic::Ordering::Relaxed);
    ai_state.llm_busy = true;

    let result_slot = ai_state.llm_result_slot.clone();
    let done_flag = ai_state.llm_done_flag.clone();

    std::thread::spawn(move || {
        let result = match LlmClient::new(config) {
            Ok(client) => match client.chat_blocking(&msgs_with_system) {
                Ok(content) => Ok(content),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e.to_string()),
        };
        if let Ok(mut slot) = result_slot.lock() {
            *slot = Some(result);
        }
        done_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    });
}

#[cfg(not(feature = "llm-online"))]
fn render_chat_disabled(ui: &mut egui::Ui, zh: bool) {
    ui.label(
        RichText::new({ crate::i18n::t("AI chat is not enabled.\nThis feature requires reqwest + tokio. Build with `llm-online` feature.\n\nThe core AI engine (protocol recognition, anomaly detection, diagnosis) is always available offline.") })
        .color(theme::TEXT_SECONDARY),
    );
}

// ==================== 知识库标签页 ====================

fn render_knowledge_tab(ui: &mut egui::Ui, ai_state: &mut AiAssistantState, zh: bool) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("USB Device Fingerprint Library"))
                .color(theme::CYAN)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(crate::i18n::t("Export JSON")).clicked() {
                if let Ok(json) = ai_state.engine.knowledge.to_json() {
                    ui.output_mut(|o| o.copied_text = json);
                }
            }
        });
    });

    ui.add_space(4.0);
    ui.label(
        RichText::new(crate::i18n::tf("{} device fingerprints", &[("{}", "{}", &format!("{}", ai_state.engine.knowledge.len()))]))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    egui::ScrollArea::vertical()
        .max_height(ui.available_height() - 20.0)
        .show(ui, |ui| {
            for dev in &ai_state.engine.knowledge.devices {
                render_device_card(ui, dev, zh);
                ui.add_space(4.0);
            }
        });
}

/// 渲染设备指纹卡片
fn render_device_card(ui: &mut egui::Ui, dev: &DeviceFingerprint, _zh: bool) {
    egui::Frame::group(&ui.style())
        .fill(theme::BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&dev.chip_name).color(theme::ACCENT).strong());
                ui.label(
                    RichText::new(format!("VID: {:04X}  PID: {:04X}", dev.vid, dev.pid))
                        .color(theme::TEXT_SECONDARY)
                        .small()
                        .family(egui::FontFamily::Monospace),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(&dev.chip_type)
                            .color(theme::CYAN)
                            .small(),
                    );
                });
            });
            ui.add_space(2.0);
            ui.label(RichText::new(&dev.description).color(theme::TEXT_SECONDARY).small());
            if !dev.protocol_hints.is_empty() {
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(crate::i18n::t("Protocols: ")).color(theme::TEXT_DISABLED).small());
                    for h in &dev.protocol_hints {
                        ui.label(RichText::new(h).color(theme::TEXT_SECONDARY).small());
                    }
                });
            }
            if let Some(base) = dev.flash_base {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(crate::i18n::tf("Flash base: 0x{:08X}", &[("{:08X}", "{:08X}", &format!("{:08X}", base))]))
                    .color(theme::CYAN)
                    .small()
                    .family(egui::FontFamily::Monospace),
                );
            }
        });
}

// ==================== 辅助：暴露 detector 灵敏度访问 ====================
// AnomalyDetector 的 sensitivity 字段是私有的，通过 AiEngine 提供封装方法：
//   AiEngine::detector_sensitivity() / detector_set_sensitivity()

// ==================== D.3 Agent 7 步闭环标签页 ====================

/// Agent 任务模板预设
struct AgentTemplate {
    id: &'static str,
    name: &'static str,
    goal: &'static str,
}

const AGENT_TEMPLATES: &[AgentTemplate] = &[
    AgentTemplate {
        id: "cushion_crosstalk",
        name: "Cushion CH340 Crosstalk Diagnosis",
        goal: "Diagnose cushion device no response after CH340 connection, identify TX self-loopback crosstalk",
    },
    AgentTemplate {
        id: "baud_mismatch",
        name: "Baud Rate Mismatch Diagnosis",
        goal: "Diagnose serial garbled data, check baud rate configuration matches firmware",
    },
    AgentTemplate {
        id: "uart_dma_drop",
        name: "UART DMA Frame Drop Diagnosis",
        goal: "Diagnose intermittent data loss, check DMA buffer config and IRQ priority",
    },
    AgentTemplate {
        id: "no_data",
        name: "No Data After Connection Diagnosis",
        goal: "Diagnose successful connection but no data, check TX/RX wiring, power and start command",
    },
    AgentTemplate {
        id: "custom",
        name: "Custom Task",
        goal: "",
    },
];

/// 渲染 Agent 7 步闭环标签页
fn render_agent_tab(ui: &mut egui::Ui, ai_state: &mut AiAssistantState, state: &AppState, zh: bool) {
    ui.heading(
        RichText::new(crate::i18n::t("Agent 7-Step Loop Diagnosis"))
            .color(theme::CYAN)
            .strong(),
    );
    ui.label(
        RichText::new(crate::i18n::t("PROBES 7-step loop: Observe → Hypothesize → Design → Send → Verify → Summarize → Update.\nStructured diagnosis without online LLM."))
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // 7 步进度指示
    render_agent_steps(ui, ai_state);
    ui.add_space(8.0);

    // 任务配置面板
    egui::Frame::group(&ui.style())
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.label(RichText::new(crate::i18n::t("Task Config")).color(theme::ACCENT).strong());
            ui.add_space(4.0);

            // 模板选择
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Template:"));
                let selected_label =
                    crate::i18n::t(AGENT_TEMPLATES[ai_state.agent_template_idx].name);
                egui::ComboBox::from_id_salt("agent_template_combo")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for (i, t) in AGENT_TEMPLATES.iter().enumerate() {
                            let label = crate::i18n::t(t.name);
                            if ui.selectable_label(ai_state.agent_template_idx == i, label).clicked() {
                                ai_state.agent_template_idx = i;
                                // 模板选中时填充 goal（非自定义模板）
                                if t.id != "custom" {
                                    ai_state.agent_goal_input = crate::i18n::t(t.goal).to_string();
                                }
                            }
                        }
                    });
            });

            // 目标输入
            ui.add_space(4.0);
            ui.label(crate::i18n::t("Goal:"));
            ui.add_space(2.0);
            ui.add(
                egui::TextEdit::multiline(&mut ai_state.agent_goal_input)
                    .desired_width(f32::MAX)
                    .desired_rows(2)
                    .hint_text(crate::i18n::t("Describe diagnosis goal...")),
            );

            // 上下文摘要（自动从 state 提取）
            ui.add_space(6.0);
            render_agent_context_summary(ui, state, zh);

            // 运行按钮
            ui.add_space(8.0);
            let can_run = !ai_state.agent_goal_input.trim().is_empty();
            let run_label = crate::i18n::t("▶ Run 7-Step Loop");
            if ui
                .add_enabled(
                    can_run,
                    egui::Button::new(RichText::new(run_label).color(Color32::BLACK).strong())
                        .fill(theme::ACCENT)
                        .min_size(Vec2::new(160.0, 32.0)),
                )
                .clicked()
            {
                run_agent_task(ai_state, state, zh);
            }
        });

    ui.add_space(8.0);

    // 结果展示
    if ai_state.agent_has_run {
        if let Some(result) = &ai_state.agent_result.clone() {
            render_agent_result(ui, result, zh);
        }
    } else {
        ui.label(
            RichText::new(crate::i18n::t("Select a template or enter a goal, then click Run to start 7-step diagnosis"))
            .color(theme::TEXT_SECONDARY)
            .small(),
        );
    }

    // 历史记录
    if !ai_state.agent_history.is_empty() {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        render_agent_history(ui, ai_state, zh);
    }
}

/// 渲染 7 步进度指示
fn render_agent_steps(ui: &mut egui::Ui, ai_state: &AiAssistantState) {
    let steps = miconlab_ai::agent::TaskStep::all();
    ui.horizontal_wrapped(|ui| {
        for (i, step) in steps.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("→").color(theme::TEXT_SECONDARY).small());
            }
            let label = crate::i18n::t_owned(step.label_en());
            let is_done = ai_state.agent_has_run;
            let color = if is_done {
                theme::SUCCESS
            } else if i == 0 && !ai_state.agent_has_run {
                theme::ACCENT
            } else {
                theme::TEXT_SECONDARY
            };
            let text = if is_done {
                format!("✓ {}", label)
            } else {
                format!("{}. {}", i + 1, label)
            };
            ui.label(RichText::new(text).color(color).small());
        }
    });
}

/// 渲染上下文摘要
fn render_agent_context_summary(ui: &mut egui::Ui, state: &AppState, _zh: bool) {
    ui.label(RichText::new(crate::i18n::t("Auto-extracted context:")).color(theme::TEXT_SECONDARY).small());
    ui.add_space(2.0);

    let connected = state.is_connected();
    let baud = state.config.baud_rate;
    let rx_bytes = state.rx_buffer.len();
    let tx_bytes = state.bytes_tx;

    let mut lines = Vec::new();
    lines.push(crate::i18n::tf("• Connection: {status}", &[("{status}", "{status}", crate::i18n::t(if connected { "Connected" } else { "Disconnected" }))]));
    lines.push(crate::i18n::tf("• Baud rate: {baud}", &[("{baud}", "{baud}", &baud.to_string())]));
    lines.push(crate::i18n::tf("• RX buffer: {bytes} bytes", &[("{bytes}", "{bytes}", &rx_bytes.to_string())]));
    lines.push(crate::i18n::tf("• TX total: {bytes} bytes", &[("{bytes}", "{bytes}", &tx_bytes.to_string())]));

    if let Some(report) = &state.crosstalk_report {
        let ratio_pct = report.echo_ratio * 100.0;
        lines.push(crate::i18n::tf(
            "• Crosstalk: echo_ratio={ratio}%, pattern={pattern}",
            &[
                ("{ratio}", "{ratio}", &format!("{:.1}", ratio_pct)),
                ("{pattern}", "{pattern}", &format!("{:?}", report.pattern)),
            ],
        ));
    }

    for line in lines {
        ui.label(RichText::new(line).color(theme::TEXT_SECONDARY).small().monospace());
    }
}

/// 渲染 Agent 结果
fn render_agent_result(ui: &mut egui::Ui, result: &miconlab_ai::agent::TaskResult, _zh: bool) {
    egui::Frame::group(&ui.style())
        .stroke(egui::Stroke::new(1.0_f32, theme::ACCENT))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::i18n::t("Diagnosis")).color(theme::ACCENT).strong());
                ui.label(
                    RichText::new(crate::i18n::tf("({confidence}% confidence)", &[("{confidence}", "{confidence}", &format!("{:.0}", result.confidence * 100.0))]))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            });
            ui.add_space(4.0);
            ui.label(RichText::new(crate::i18n::t_owned(&result.diagnosis)).color(theme::TEXT_PRIMARY));

            // 建议动作
            if !result.suggested_actions.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new(crate::i18n::t("Suggested actions:")).color(theme::ACCENT).strong());
                ui.add_space(2.0);
                for (i, action) in result.suggested_actions.iter().enumerate() {
                    let desc = crate::i18n::t_owned(&action.describe_zh());
                    let text = format!("{}. {}", i + 1, desc);
                    ui.label(
                        RichText::new(text)
                            .color(if matches!(action, miconlab_ai::agent::AgentAction::SendBytes(_)) {
                                Color32::from_rgb(255, 200, 100)
                            } else {
                                theme::TEXT_PRIMARY
                            })
                            .monospace()
                            .small(),
                    );
                }
            }

            // 下一步提示
            if !result.next_step_hint.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new(crate::i18n::t("Next step:")).color(theme::ACCENT).small());
                ui.label(RichText::new(&result.next_step_hint).color(theme::TEXT_SECONDARY).small());
            }
        });
}

/// 渲染历史记录
fn render_agent_history(ui: &mut egui::Ui, ai_state: &mut AiAssistantState, zh: bool) {
    ui.label(RichText::new(crate::i18n::t("History")).color(theme::ACCENT).strong());
    ui.add_space(4.0);

    let mut show_idx: Option<usize> = None;
    egui::Frame::group(&ui.style())
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            for (i, entry) in ai_state.agent_history.iter().enumerate().rev() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&entry.timestamp).color(theme::TEXT_SECONDARY).small().monospace());
                    ui.label(
                        RichText::new(if entry.goal.len() > 50 {
                            format!("{}...", &entry.goal[..50])
                        } else {
                            entry.goal.clone()
                        })
                        .color(theme::TEXT_PRIMARY)
                        .small(),
                    );
                    ui.label(
                        RichText::new(format!("({:.0}%)", entry.result.confidence * 100.0))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );
                    if ui.small_button(crate::i18n::t("View")).clicked() {
                        show_idx = Some(i);
                    }
                });
            }
        });

    if let Some(idx) = show_idx {
        if let Some(entry) = ai_state.agent_history.get(idx) {
            ui.add_space(4.0);
            render_agent_result(ui, &entry.result, zh);
        }
    }
}

/// 运行 Agent 任务
fn run_agent_task(ai_state: &mut AiAssistantState, state: &AppState, zh: bool) {
    let goal = ai_state.agent_goal_input.trim().to_string();
    if goal.is_empty() {
        return;
    }

    // 构造 TaskContext
    let ctx = miconlab_ai::agent::TaskContext {
        is_connected: state.is_connected(),
        baud_rate: state.config.baud_rate,
        port_name: state.port_list.get(state.selected_port_index).map(|p| p.name.clone()),
        rx_sample: state.rx_buffer.iter().flat_map(|e| e.data.iter().copied()).take(256).collect(),
        tx_sample: Vec::new(),
        crosstalk: state.crosstalk_report.clone().map(|r| miconlab_ai::diagnose::CrosstalkReport {
            echo_ratio: r.echo_ratio,
            rx_tx_ratio: r.rx_tx_ratio,
            is_loopback: r.is_loopback,
            latency_us: r.latency_us,
            pattern: match r.pattern {
                miconlab_core::crosstalk::EchoPattern::Exact => miconlab_ai::diagnose::EchoPattern::Exact,
                miconlab_core::crosstalk::EchoPattern::BitShifted => miconlab_ai::diagnose::EchoPattern::BitShifted,
                miconlab_core::crosstalk::EchoPattern::HighByteDominant => miconlab_ai::diagnose::EchoPattern::HighByteDominant,
                miconlab_core::crosstalk::EchoPattern::None => miconlab_ai::diagnose::EchoPattern::None,
            },
            high_byte_ratio: r.high_byte_ratio,
            tx_total: r.tx_total,
            rx_total: r.rx_total,
        }),
        chip_hint: state
            .port_list
            .get(state.selected_port_index)
            .and_then(|p| p.vid)
            .map(|v| match v {
                0x1A86 => "CH340".to_string(),
                0x10C4 => "CP210x".to_string(),
                0x0403 => "FTDI".to_string(),
                _ => format!("VID_{:04X}", v),
            }),
        ..Default::default()
    };

    // 添加固件分析（如果已加载）
    // 这里用 state 中的固件分析（如果有）
    // 暂时跳过，因为 AppState 当前不直接持有 FirmwareAnalysis

    let mut task = miconlab_ai::agent::AgentTask::new(&goal, ctx);
    let engine = miconlab_ai::diagnose::DiagnosticEngine::with_builtin_rules();
    let result = task.run_offline(&engine);

    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
    ai_state.agent_history.push(AgentHistoryEntry {
        goal: goal.clone(),
        result: result.clone(),
        timestamp,
    });

    // 保留最近 20 条历史
    if ai_state.agent_history.len() > 20 {
        let drain_count = ai_state.agent_history.len() - 20;
        ai_state.agent_history.drain(0..drain_count);
    }

    ai_state.agent_result = Some(result);
    ai_state.agent_has_run = true;

    // 自动填充模板目标文本（如果是自定义模板）
    if AGENT_TEMPLATES[ai_state.agent_template_idx].id == "custom" && ai_state.agent_goal_input.is_empty() {
        let _ = zh; // zh unused for custom
    }
}
