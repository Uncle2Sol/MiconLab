// 面板状态管理

use super::chip_type::EditorChipType;
use super::memory::FirmwareMemory;

#[cfg(feature = "ollama")]
use miconlab_ai::{FirmwareIdentifyResult, OllamaConfig, OllamaStatus};
#[cfg(feature = "ollama")]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "ollama")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "ollama")]
use std::time::Instant;

/// 编辑模式
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditView {
    /// 十六进制编辑器
    Hex,
    /// 向量表
    VectorTable,
    /// 字符串查看
    Strings,
    /// 差分对比
    Diff,
    /// 外设映射（D.6 新增：DMA/IRQ/GPIO 可视化）
    PeriphMap,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Bin,
    Hex,
    Unknown,
}

#[derive(Clone)]
pub struct FirmwareEditorState {
    /// 加载的固件内存模型
    pub memory: Option<FirmwareMemory>,
    /// 文件路径
    pub file_path: String,
    /// 文件格式
    pub file_format: FileFormat,
    /// 选择的芯片类型
    pub chip_type: EditorChipType,
    /// 当前视图
    pub view: EditView,
    /// 滚动偏移（行号）
    pub scroll_row: usize,
    /// 每行字节数
    pub bytes_per_row: usize,
    /// 选中的地址（光标位置）
    pub cursor_addr: Option<u32>,
    /// 编辑缓冲区（十六进制输入）
    pub edit_buffer: String,
    /// 正在编辑的地址
    pub editing_addr: Option<u32>,
    /// 字符串搜索结果
    pub string_search: String,
    pub string_results: Vec<(u32, String)>,
    /// 状态消息
    pub status_msg: String,
    /// 自动检测的芯片类型
    pub detected_chip: EditorChipType,
    /// 跳转地址输入
    pub goto_addr_input: String,
    /// 待滚动到的地址（设置后下一帧滚动到该位置）
    pub scroll_to_addr: Option<u32>,
    /// 十六进制编辑器跨闭包传递待写入字节（避免借用冲突）
    pub pending_write: Option<(u32, u8)>,

    // ---- Ollama 本地模型固件识别（feature gate，Phase 3）----
    /// Ollama 配置（Phase 3 暂用默认值，后续设置页可编辑）
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
    pub ollama_status_slot: Arc<Mutex<Option<OllamaStatus>>>,
    /// 状态检测进行中
    #[cfg(feature = "ollama")]
    pub ollama_status_busy: bool,
    /// 固件识别后台任务结果槽
    #[cfg(feature = "ollama")]
    pub ollama_result_slot: Arc<Mutex<Option<Result<FirmwareIdentifyResult, String>>>>,
    /// 固件识别后台任务完成标志
    #[cfg(feature = "ollama")]
    pub ollama_done_flag: Arc<AtomicBool>,
    /// 固件识别进行中
    #[cfg(feature = "ollama")]
    pub ollama_busy: bool,
    /// 最近一次 LLM 固件识别结果（用于结果条展示）
    #[cfg(feature = "ollama")]
    pub ollama_result: Option<FirmwareIdentifyResult>,
    /// LLM 结果是否已自动应用到 detected_chip/base 地址
    #[cfg(feature = "ollama")]
    pub ollama_applied: bool,
    /// Ollama 配置签名（设置页联动检测，Phase 4）
    #[cfg(feature = "ollama")]
    pub ollama_cfg_sig: Option<String>,
}

impl Default for FirmwareEditorState {
    fn default() -> Self {
        Self {
            memory: None,
            file_path: String::new(),
            file_format: FileFormat::Unknown,
            chip_type: EditorChipType::Auto,
            view: EditView::Hex,
            scroll_row: 0,
            bytes_per_row: 16,
            cursor_addr: None,
            edit_buffer: String::new(),
            editing_addr: None,
            string_search: String::new(),
            string_results: Vec::new(),
            status_msg: String::new(),
            detected_chip: EditorChipType::Raw,
            goto_addr_input: String::new(),
            scroll_to_addr: None,
            pending_write: None,
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
            ollama_applied: false,
            #[cfg(feature = "ollama")]
            ollama_cfg_sig: None,
        }
    }
}

impl FirmwareEditorState {
    pub fn file_format_label(&self) -> &'static str {
        match self.file_format {
            FileFormat::Bin => "BIN",
            FileFormat::Hex => "HEX",
            FileFormat::Unknown => "?",
        }
    }

    pub fn effective_chip(&self) -> EditorChipType {
        if self.chip_type == EditorChipType::Auto {
            self.detected_chip
        } else {
            self.chip_type
        }
    }
}
