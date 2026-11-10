/// Plugin loader using libloading for dynamic library loading.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PluginError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Library load error: {0}")]
    LoadError(String),
    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),
    #[error("Plugin error: {0}")]
    PluginError(String),
    #[error("Plugin panicked: {0}")]
    PluginPanic(String),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}

pub type PluginResult<T> = Result<T, PluginError>;

/// Information about a loaded plugin.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
}

/// A command exposed by a plugin.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
    pub parameters: Vec<PluginParameter>,
}

/// A parameter for a plugin command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginParameter {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub param_type: String,
}

/// Result of executing a plugin command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginResultData {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

// FFI function signatures
type FnGetInfo = unsafe extern "C" fn() -> *mut std::os::raw::c_char;
type FnGetCommands = unsafe extern "C" fn() -> *mut std::os::raw::c_char;
type FnExecute =
    unsafe extern "C" fn(*const std::os::raw::c_char, *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
type FnFreeString = unsafe extern "C" fn(*mut std::os::raw::c_char);
// 获取插件 API 版本字符串（指向插件内静态存储，调用方无需释放）
type FnGetApiVersion = unsafe extern "C" fn() -> *const std::os::raw::c_char;

/// A loaded plugin from a dynamic library.
pub struct LoadedPlugin {
    path: PathBuf,
    #[allow(dead_code)]
    library: libloading::Library,
    info: PluginInfo,
    commands: Vec<PluginCommand>,
    is_enabled: bool,
    fn_execute: FnExecute,
    fn_free_string: FnFreeString,
}

// Library 可跨线程移动（Send），但不声明 Sync：插件是否可重入未知，
// 多线程同时调用同一 C 函数不安全。如需跨线程共享请用 Mutex 包裹。
unsafe impl Send for LoadedPlugin {}

impl LoadedPlugin {
    /// Load a plugin from a dynamic library file.
    pub fn load(path: &Path) -> PluginResult<Self> {
        unsafe {
            let library = libloading::Library::new(path).map_err(|e| {
                PluginError::LoadError(format!("Failed to load {}: {}", path.display(), e))
            })?;

            let fn_get_info: FnGetInfo = *library
                .get(b"plugin_get_info")
                .map_err(|e| PluginError::SymbolNotFound(format!("plugin_get_info: {}", e)))?;

            let fn_get_commands: FnGetCommands = *library
                .get(b"plugin_get_commands")
                .map_err(|e| PluginError::SymbolNotFound(format!("plugin_get_commands: {}", e)))?;

            let fn_execute: FnExecute = *library
                .get(b"plugin_execute")
                .map_err(|e| PluginError::SymbolNotFound(format!("plugin_execute: {}", e)))?;

            let fn_free_string: FnFreeString = *library
                .get(b"plugin_free_string")
                .map_err(|e| PluginError::SymbolNotFound(format!("plugin_free_string: {}", e)))?;

            // 加载插件 API 版本符号并校验，防止 ABI 不兼容导致 UB
            let fn_get_api_version: FnGetApiVersion = *library
                .get(b"plugin_api_version")
                .map_err(|e| {
                    PluginError::LoadError(format!("plugin_api_version symbol not found: {}", e))
                })?;

            let version_ptr = fn_get_api_version();
            if version_ptr.is_null() {
                return Err(PluginError::LoadError(
                    "plugin_api_version returned null".to_string(),
                ));
            }
            // 版本字符串指向插件内的静态存储，无需调用 plugin_free_string 释放
            let found_version = CStr::from_ptr(version_ptr).to_string_lossy().to_string();
            if found_version != miconlab_plugin_api::PLUGIN_API_VERSION {
                return Err(PluginError::LoadError(format!(
                    "API version mismatch: expected {}, found {}",
                    miconlab_plugin_api::PLUGIN_API_VERSION, found_version
                )));
            }

            // Get info
            let info_ptr = fn_get_info();
            if info_ptr.is_null() {
                return Err(PluginError::PluginError(
                    "plugin_get_info returned null".to_string(),
                ));
            }
            let info_str = CStr::from_ptr(info_ptr).to_string_lossy().to_string();
            fn_free_string(info_ptr);

            let info: PluginInfo = serde_json::from_str(&info_str)?;

            // Get commands
            let commands_ptr = fn_get_commands();
            if commands_ptr.is_null() {
                return Err(PluginError::PluginError(
                    "plugin_get_commands returned null".to_string(),
                ));
            }
            let commands_str = CStr::from_ptr(commands_ptr).to_string_lossy().to_string();
            fn_free_string(commands_ptr);

            let commands: Vec<PluginCommand> = serde_json::from_str(&commands_str)?;

            Ok(Self {
                path: path.to_path_buf(),
                library,
                info,
                commands,
                is_enabled: true,
                fn_execute,
                fn_free_string,
            })
        }
    }

    /// Execute a command on this plugin.
    pub fn execute_command(&self, command: &str, params: &str) -> PluginResult<PluginResultData> {
        if !self.is_enabled {
            return Err(PluginError::PluginError(
                "Plugin is disabled".to_string(),
            ));
        }

        let cmd_c = CString::new(command).map_err(|e| {
            PluginError::PluginError(format!("Invalid command string: {}", e))
        })?;

        let params_c = CString::new(params).map_err(|e| {
            PluginError::PluginError(format!("Invalid params string: {}", e))
        })?;

        // 使用 catch_unwind 包裹 FFI 调用，防止插件 panic 跨 FFI 边界导致 UB
        let result = catch_unwind(AssertUnwindSafe(|| {
            unsafe {
                let result_ptr = (self.fn_execute)(cmd_c.as_ptr(), params_c.as_ptr());
                if result_ptr.is_null() {
                    return Ok(PluginResultData {
                        success: false,
                        result: None,
                        error: Some("Plugin returned null".to_string()),
                    });
                }

                let result_str = CStr::from_ptr(result_ptr).to_string_lossy().to_string();
                (self.fn_free_string)(result_ptr);

                let value: serde_json::Value = serde_json::from_str(&result_str)?;

                Ok(PluginResultData {
                    success: value.get("success").and_then(|v| v.as_bool()).unwrap_or(false),
                    result: value.get("result").cloned(),
                    error: value.get("error").and_then(|v| v.as_str()).map(String::from),
                })
            }
        }));

        match result {
            Ok(inner) => inner,
            Err(payload) => {
                // 从 panic 载荷中提取可读的错误信息
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().map(|s| s.clone()))
                    .unwrap_or_else(|| "Unknown panic from plugin".to_string());
                Err(PluginError::PluginPanic(msg))
            }
        }
    }

    /// Get plugin info.
    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// Get the list of commands this plugin provides.
    pub fn commands(&self) -> &[PluginCommand] {
        &self.commands
    }

    /// Get the path to the plugin library.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Check if the plugin is enabled.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled
    }

    /// Enable or disable the plugin.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.is_enabled = enabled;
    }
}

/// Manages discovery, loading, and execution of plugins.
pub struct PluginManager {
    plugin_dirs: Vec<PathBuf>,
    plugins: HashMap<String, LoadedPlugin>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugin_dirs: Vec::new(),
            plugins: HashMap::new(),
        }
    }

    /// Add a directory to search for plugins.
    pub fn add_plugin_dir(&mut self, dir: PathBuf) {
        if !self.plugin_dirs.contains(&dir) {
            self.plugin_dirs.push(dir);
        }
    }

    /// Discover and load all plugins from registered directories.
    pub fn discover(&mut self) -> PluginResult<()> {
        let dirs: Vec<PathBuf> = self.plugin_dirs.clone();

        for dir in &dirs {
            if !dir.exists() {
                continue;
            }

            let entries = std::fs::read_dir(dir)?;

            for entry in entries.flatten() {
                let path = entry.path();

                let is_plugin = path.extension().map_or(false, |ext| {
                    ext == "dylib" || ext == "so" || ext == "dll"
                });

                if is_plugin {
                    match LoadedPlugin::load(&path) {
                        Ok(plugin) => {
                            let name = plugin.info().name.clone();
                            self.plugins.insert(name, plugin);
                        }
                        Err(e) => {
                            log::warn!("Failed to load plugin {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get all loaded plugins.
    pub fn plugins(&self) -> &HashMap<String, LoadedPlugin> {
        &self.plugins
    }

    /// Get a plugin by name.
    pub fn get_plugin(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(name)
    }

    /// Execute a command on a specific plugin.
    pub fn execute(
        &self,
        plugin_name: &str,
        command: &str,
        params: &str,
    ) -> PluginResult<PluginResultData> {
        let plugin = self
            .plugins
            .get(plugin_name)
            .ok_or_else(|| PluginError::PluginError(format!("Plugin '{}' not found", plugin_name)))?;

        plugin.execute_command(command, params)
    }

    /// Get all commands across all enabled plugins.
    pub fn all_commands(&self) -> Vec<(&str, &PluginCommand)> {
        let mut commands = Vec::new();
        for (name, plugin) in &self.plugins {
            if plugin.is_enabled() {
                for cmd in plugin.commands() {
                    commands.push((name.as_str(), cmd));
                }
            }
        }
        commands
    }

    /// Remove a plugin by name.
    pub fn remove_plugin(&mut self, name: &str) -> Option<LoadedPlugin> {
        self.plugins.remove(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manager_new() {
        let manager = PluginManager::new();
        assert!(manager.plugins().is_empty());
    }

    #[test]
    fn test_plugin_manager_default() {
        let manager = PluginManager::default();
        assert!(manager.plugins().is_empty());
    }

    #[test]
    fn test_add_plugin_dir() {
        let mut manager = PluginManager::new();
        manager.add_plugin_dir(PathBuf::from("/tmp/plugins"));
        manager.add_plugin_dir(PathBuf::from("/tmp/plugins")); // duplicate
        assert_eq!(manager.plugin_dirs.len(), 1);
    }

    #[test]
    fn test_discover_nonexistent_dir() {
        let mut manager = PluginManager::new();
        manager.add_plugin_dir(PathBuf::from("/nonexistent/dir"));
        // Should not error
        manager.discover().unwrap();
        assert!(manager.plugins().is_empty());
    }

    #[test]
    fn test_get_plugin_not_found() {
        let manager = PluginManager::new();
        assert!(manager.get_plugin("nonexistent").is_none());
    }

    #[test]
    fn test_execute_not_found() {
        let manager = PluginManager::new();
        let result = manager.execute("nonexistent", "cmd", "{}");
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_plugin() {
        let mut manager = PluginManager::new();
        assert!(manager.remove_plugin("nonexistent").is_none());
    }

    #[test]
    fn test_plugin_info_serde() {
        let info = PluginInfo {
            name: "test".to_string(),
            version: "1.0".to_string(),
            description: "test plugin".to_string(),
            author: "test".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: PluginInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
    }

    #[test]
    fn test_plugin_result_data_serde() {
        let result = PluginResultData {
            success: true,
            result: Some(serde_json::json!({"value": 42})),
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: PluginResultData = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
    }
}
