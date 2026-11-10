//! 应用设置（磁盘持久化）
//!
//! 配置文件路径（由 miconlab_core::platform 统一定义）：
//! - macOS:   ~/Library/Application Support/miconlab/config.json
//! - Linux:   $XDG_CONFIG_HOME 或 ~/.config/miconlab/config.json
//! - Windows: %APPDATA%\miconlab\config.json
//!
//! 当前仅含 Ollama 本地模型配置；后续新增设置项直接在此结构体加字段（serde 默认值兼容旧文件）。
//! 保存失败仅记日志不报错（配置丢失不应影响使用，所有字段均有合理默认值）。

use serde::{Deserialize, Serialize};

/// Ollama 本地模型设置（三面板共享的唯一持久配置源）
#[cfg(feature = "ollama")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OllamaSettings {
    /// Ollama 服务地址
    pub base_url: String,
    /// 识别模型名
    pub model: String,
    /// 自动增强开关（规则引擎低置信时自动调用本地模型）
    pub auto_enabled: bool,
    /// 单次识别超时（秒）
    pub timeout_secs: u64,
}

#[cfg(feature = "ollama")]
impl Default for OllamaSettings {
    fn default() -> Self {
        Self {
            base_url: miconlab_ai::ollama::DEFAULT_BASE_URL.to_string(),
            model: miconlab_ai::ollama::DEFAULT_MODEL.to_string(),
            auto_enabled: true,
            timeout_secs: 60,
        }
    }
}

#[cfg(feature = "ollama")]
impl OllamaSettings {
    /// 转换为 miconlab-ai 客户端配置
    pub fn to_ollama_config(&self) -> miconlab_ai::OllamaConfig {
        miconlab_ai::OllamaConfig {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            timeout_secs: self.timeout_secs,
            ..Default::default()
        }
    }

    /// 配置签名（base_url + model），用于面板检测配置变化并强制重查状态
    pub fn signature(&self) -> String {
        format!("{}|{}", self.base_url, self.model)
    }
}

/// 面板配置联动（feature gate，Phase 4）：
/// 设置页是唯一可变源；各识别面板每帧调用，签名变化时刷新 config 并强制重查服务状态。
/// 面板需自持 `sig: Option<String>` 字段做跨帧比较。
#[cfg(feature = "ollama")]
pub fn sync_panel_config(
    cfg: &mut miconlab_ai::OllamaConfig,
    sig: &mut Option<String>,
    status: &mut Option<miconlab_ai::OllamaStatus>,
    checked_at: &mut Option<std::time::Instant>,
    settings: &OllamaSettings,
) {
    let s = settings.signature();
    if sig.as_deref() != Some(s.as_str()) {
        *sig = Some(s);
        *cfg = settings.to_ollama_config();
        *status = None;
        *checked_at = None;
    }
}

/// 鞋垫面板设置（持久化；默认值与 InsoleState::new() 一致）
#[cfg(feature = "device-workspaces")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsoleSettings {
    /// 触发阈值（ADC）
    pub trigger_threshold: u8,
    /// 插值密度 0-8（0=关闭）
    pub interp_density: u32,
    /// 色图颜色上限（同 HTML colorMax）
    pub color_max: f32,
    /// 高度缩放（同 HTML heightScale）
    pub height_scale: f32,
    /// 插值点大小（同 HTML pointSize，1-20）
    pub point_size: f32,
    /// 散射强度 0-1（插值场合成尖锐度，0=加权平均，1=强散射峰谷分明）
    pub scatter_sharpness: f32,
    /// 自动增益（色图上限跟随当前帧最大压力）
    pub auto_gain: bool,
    /// 数据左右对调（传感器内外镜像校准）
    pub swap_lr: bool,
    /// 上下翻转视图（脚趾朝上）
    pub flip_view: bool,
    /// 镜像演示（双脚同步显示）
    pub mirror_demo: bool,
    /// 显示传感器数值
    pub show_values: bool,
    /// 数据映射脚别（true=左脚）
    pub foot_is_left: bool,
}

#[cfg(feature = "device-workspaces")]
impl Default for InsoleSettings {
    fn default() -> Self {
        Self {
            trigger_threshold: 20, // insole::DEFAULT_TRIGGER_THRESHOLD
            interp_density: 4,
            color_max: 100.0,
            height_scale: 2.0,
            point_size: 3.0,
            scatter_sharpness: 0.7,
            auto_gain: true,
            swap_lr: false,
            flip_view: false,
            mirror_demo: false,
            show_values: false,
            foot_is_left: true,
        }
    }
}

#[cfg(feature = "device-workspaces")]
include!("settings_device.rs");

/// 应用设置根结构
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// Ollama 本地模型设置（feature 关闭时不存在，旧文件多余字段被忽略）
    #[cfg(feature = "ollama")]
    #[serde(default)]
    pub ollama: OllamaSettings,
    /// 鞋垫面板设置
    #[cfg(feature = "device-workspaces")]
    #[serde(default)]
    pub insole: InsoleSettings,
    /// 三台设备各自的串口配置（旧配置文件无此字段时用默认值）
    #[cfg(feature = "device-workspaces")]
    #[serde(default)]
    pub devices: DeviceProfiles,
    /// 界面语言（BCP-47，如 `zh-CN`）。`None` = 跟随系统语言。
    /// 旧配置文件没有该字段时自动视为跟随系统。
    #[serde(default)]
    pub language: Option<String>,
}

impl AppSettings {
    /// 配置文件路径（目录不存在时返回 None，save 时会创建）
    pub fn config_path() -> Option<std::path::PathBuf> {
        // 路径规则统一在 miconlab_core::platform 中（macOS/Linux/Windows 各自约定）
        miconlab_core::platform::config_file_path()
    }

    /// 从磁盘加载。文件不存在/解析失败 → 返回默认值（不报错，静默降级）。
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<AppSettings>(&text) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("{}", crate::i18n::tf("Config parse failed (using defaults): {error} @ {path}", &[("{error}", "{error}", &format!("{}", e)), ("{path}", "{path}", &format!("{:?}", path))]));
                    Self::default()
                }
            },
            Err(_) => Self::default(), // 首次运行无文件，正常
        }
    }

    /// 保存到磁盘（原子写：先写 tmp 再 rename）。失败仅记日志。
    /// （feature 关闭时暂无调用方，保留给后续设置项使用）
    #[cfg_attr(not(feature = "ollama"), allow(dead_code))]
    pub fn save(&self) {
        let Some(path) = Self::config_path() else {
            log::warn!("{}", crate::i18n::t("Cannot determine config directory; settings not persisted"));
            return;
        };
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("{}", crate::i18n::tf("Failed to create config directory: {error}", &[("{error}", "{error}", &format!("{}", e))]));
            return;
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                let tmp = path.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&tmp, text) {
                    log::warn!("{}", crate::i18n::tf("Failed to write config file: {error}", &[("{error}", "{error}", &format!("{}", e))]));
                    return;
                }
                if let Err(e) = std::fs::rename(&tmp, &path) {
                    log::warn!("{}", crate::i18n::tf("Failed to replace config file: {error}", &[("{error}", "{error}", &format!("{}", e))]));
                }
            }
            Err(e) => log::warn!("{}", crate::i18n::tf("Config serialization failed: {error}", &[("{error}", "{error}", &format!("{}", e))])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ollama")]
    #[test]
    fn test_ollama_default_matches_ai_crate() {
        let d = OllamaSettings::default();
        assert_eq!(d.base_url, miconlab_ai::ollama::DEFAULT_BASE_URL);
        assert_eq!(d.model, miconlab_ai::ollama::DEFAULT_MODEL);
        assert!(d.auto_enabled);
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_to_ollama_config() {
        let s = OllamaSettings {
            base_url: "http://192.168.1.10:11434".into(),
            model: "qwen2.5:3b-instruct".into(),
            auto_enabled: false,
            timeout_secs: 120,
        };
        let c = s.to_ollama_config();
        assert_eq!(c.base_url, "http://192.168.1.10:11434");
        assert_eq!(c.model, "qwen2.5:3b-instruct");
        assert_eq!(c.timeout_secs, 120);
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_settings_json_roundtrip() {
        let s = AppSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_settings_json_backward_compat() {
        // 旧版无 ollama 字段的配置文件 → serde default 填充
        let back: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(back.ollama, OllamaSettings::default());
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_signature_changes_only_on_key_fields() {
        let a = OllamaSettings::default();
        let mut b = a.clone();
        assert_eq!(a.signature(), b.signature());
        b.model = "other".into();
        assert_ne!(a.signature(), b.signature());
        // auto/timeout 不影响签名（不触发状态重查）
        let mut c = a.clone();
        c.auto_enabled = false;
        c.timeout_secs = 99;
        assert_eq!(a.signature(), c.signature());
    }

    #[test]
    fn test_config_path_is_absolute() {
        if let Some(p) = AppSettings::config_path() {
            assert!(p.is_absolute());
            assert!(p.to_string_lossy().contains("config.json"));
        }
    }

    #[cfg(feature = "device-workspaces")]
    #[test]
    fn test_device_profiles_backward_compat_and_isolation() {
        // 旧配置文件（无 devices 字段）→ 三台设备各取默认波特率
        let back: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            back.devices.get(crate::devices::DeviceId::Fa008).baud,
            crate::devices::DeviceId::Fa008.meta().default_baud
        );
        assert_eq!(
            back.devices.get(crate::devices::DeviceId::JqCare).baud,
            crate::devices::DeviceId::JqCare.meta().default_baud
        );
        assert_eq!(
            back.devices.get(crate::devices::DeviceId::Cushion).baud,
            crate::devices::DeviceId::Cushion.meta().default_baud
        );

        // 修改一台设备的配置不影响另外两台
        let mut s = AppSettings::default();
        s.devices.get_mut(crate::devices::DeviceId::JqCare).baud = 230400;
        s.devices.get_mut(crate::devices::DeviceId::JqCare).port = Some("COM7".into());
        assert_ne!(
            s.devices.get(crate::devices::DeviceId::Fa008).baud,
            230400
        );
        assert!(s.devices.get(crate::devices::DeviceId::Fa008).port.is_none());
        assert_ne!(
            s.devices.get(crate::devices::DeviceId::Cushion).baud,
            230400
        );

        // 序列化往返保持
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.devices.get(crate::devices::DeviceId::JqCare).baud, 230400);
        assert_eq!(
            back.devices.get(crate::devices::DeviceId::JqCare).port.as_deref(),
            Some("COM7")
        );
    }

    #[cfg(feature = "device-workspaces")]
    #[test]
    fn test_insole_settings_backward_compat() {
        // 旧版无 insole 字段的配置文件 → serde default 填充
        let back: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(back.insole, InsoleSettings::default());
        // 序列化往返
        let s = AppSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn test_save_and_reload_roundtrip() {
        // 临时 HOME 重定向避免污染真实配置（platform::config_dir 在 macOS 读 HOME）
        let tmp = std::env::temp_dir().join(format!("miconlab-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);
        // macOS config_dir 走 HOME；Linux 走 XDG_CONFIG_HOME 或 HOME/.config
        std::env::set_var("XDG_CONFIG_HOME", tmp.join(".config"));

        let mut s = AppSettings::default();
        s.ollama.model = "test-model:latest".into();
        s.save();
        let loaded = AppSettings::load();
        assert_eq!(loaded.ollama.model, "test-model:latest");

        // 恢复环境 + 清理
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
