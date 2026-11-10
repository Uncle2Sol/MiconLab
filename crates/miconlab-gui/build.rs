//! build.rs
//!
//! 1. 扫描 `locales/*.toml`，生成编译期嵌入的语言包索引。
//!    语言包分两层：
//!      - `<code>.toml`        通用文案（两版共用）
//!      - `<code>.device.toml` 设备工作区文案（仅 device-workspaces 特性开启时合并）
//!    公开版不开启该特性，设备文案从编译入口就排除，不依赖事后扫描。
//! 2. 设备固件一致性测试的 C 侧编译（仅设备工作区构建，且本机存在固件工程时启用）。
//!
//! 固件目录检测：
//!   1. 环境变量 DEVICE_FW_DIR（显式指定）
//!   2. 本机私有配置文件 `device-fw-dir.txt`（不随公开版导出）
//!
//! 找到固件 → 编译固件 protocol.c + ffi/device_shim.c 为静态库，
//! 并启用 `fw_conformance` cfg（protocol.rs 中的一致性测试随之启用）。
//! 固件缺失（其他机器/CI）→ 静默跳过，普通构建完全不受影响。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() {
    generate_locale_index();
    build_firmware_conformance();
}

fn device_workspaces_enabled() -> bool {
    std::env::var_os("CARGO_FEATURE_DEVICE_WORKSPACES").is_some()
}

fn build_firmware_conformance() {
    // 公开版关闭 device-workspaces：不编译设备固件一致性测试的 C 侧代码
    if !device_workspaces_enabled() {
        return;
    }

    // 交叉编译时跳过：一致性测试只能在目标平台上运行，且交叉产物需要
    // 目标平台的 C 编译器（本机 macOS 上默认没有 Linux/Windows 的 cc）。
    let host = std::env::var("HOST").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_default();
    if !host.is_empty() && !target.is_empty() && host != target {
        return;
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    println!("cargo:rerun-if-env-changed=DEVICE_FW_DIR");
    let fw_dir = match std::env::var("DEVICE_FW_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => {
            let cfg = manifest.join("device-fw-dir.txt");
            println!("cargo:rerun-if-changed={}", cfg.display());
            match std::fs::read_to_string(&cfg) {
                Ok(s) if !s.trim().is_empty() => PathBuf::from(s.trim()),
                _ => return,
            }
        }
    };

    let proto_c = fw_dir.join("main").join("protocol.c");
    if !proto_c.exists() {
        // 固件源码不在本机：跳过一致性测试编译
        return;
    }

    println!("cargo:rerun-if-changed={}", proto_c.display());
    println!("cargo:rerun-if-changed=ffi/device_shim.c");
    // 声明自定义 cfg，避免 Rust 1.80+ 的 unexpected_cfgs 警告
    println!("cargo:rustc-check-cfg=cfg(fw_conformance)");
    println!("cargo:rustc-cfg=fw_conformance");

    cc::Build::new()
        .file("ffi/device_shim.c")
        .file(&proto_c)
        .include(fw_dir.join("main"))
        .compile("device_conformance");
}

/// 生成 `locales_generated.rs`：把每个语言包以 `include_str!` 嵌入二进制。
///
/// 设备文案位于 `<code>.device.toml`，只在 `device-workspaces` 特性开启时合并，
/// 因此公开版二进制里不会出现任何设备工作区字符串。
fn generate_locale_index() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let locale_dir = manifest.join("locales");
    println!("cargo:rerun-if-changed={}", locale_dir.display());

    let include_device = device_workspaces_enabled();

    // 按语言代码聚合：`zh-CN.toml` 与 `zh-CN.device.toml` 都归入 `zh-CN`
    let mut by_code: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    if let Ok(read_dir) = std::fs::read_dir(&locale_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let code = stem.split('.').next().unwrap_or(stem).to_string();
            by_code.entry(code).or_default().push(path);
        }
    }

    if by_code.is_empty() {
        panic!("locales/ 目录为空：至少需要 en.toml");
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let merged_dir = out_dir.join("locales");
    std::fs::create_dir_all(&merged_dir).unwrap_or_else(|e| panic!("创建 {} 失败: {e}", merged_dir.display()));

    let mut out = String::new();
    out.push_str("// 由 build.rs 生成，请勿手工编辑\n");
    out.push_str("pub const LOCALE_FILES: &[(&str, &str)] = &[\n");

    for (code, paths) in by_code {
        let mut merged = String::new();
        for path in paths {
            let is_device = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.contains(".device"))
                .unwrap_or(false);
            println!("cargo:rerun-if-changed={}", path.display());
            if is_device && !include_device {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", path.display()));
            merged.push_str(&text);
            if !text.ends_with('\n') {
                merged.push('\n');
            }
        }

        let dest = merged_dir.join(format!("{code}.toml"));
        std::fs::write(&dest, merged).unwrap_or_else(|e| panic!("写入 {} 失败: {e}", dest.display()));
        out.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            code,
            dest.to_string_lossy()
        ));
    }

    out.push_str("];\n");

    let dest = out_dir.join("locales_generated.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("写入 {} 失败: {e}", dest.display()));
}
