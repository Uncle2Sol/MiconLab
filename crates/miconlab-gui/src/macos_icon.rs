//! macOS 运行时 Dock 图标设置。
//!
//! 根因：Dock 中"运行状态"的图标来自进程向 Launch Services 的注册信息，
//! 而 Finder/启动台的静态图标才读 bundle 内的 miconlab.icns。eframe/winit
//! 应用不会主动设置运行时图标，当进程与 bundle 的关联失效时
//! （`make install` 反复覆盖 /Applications、Dock/LS 缓存陈旧、
//! 直接运行二进制等），Dock 就退回通用可执行占位图标（"e"）。
//!
//! 修复：启动时显式调用 `NSApplication.setApplicationIconImage`——
//! 优先加载 bundle Resources/miconlab.icns，非 bundle 启动（cargo run）回退
//! 编译期内嵌 PNG。无论何种启动方式或缓存状态，运行时图标必定正确。

use objc2::MainThreadMarker;
use objc2::AnyThread;
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{NSData, NSString};

/// 启动时设置 Dock 图标（必须在主线程调用，main() 开头满足）。
pub fn set() {
    let Some(mtm) = MainThreadMarker::new() else {
        return; // 非主线程：winit 也不会在此场景运行，直接跳过
    };
    let app = NSApplication::sharedApplication(mtm);

    // 1) bundle 路径：<exe>/../Resources/miconlab.icns（正常安装场景，矢量多分辨率）
    if let Ok(exe) = std::env::current_exe() {
        let icns = exe
            .parent() // <bundle>/Contents/MacOS
            .and_then(|macos| macos.parent()) // <bundle>/Contents
            .map(|contents| contents.join("Resources").join("miconlab.icns"));
        if let Some(path) = icns {
            if path.exists() {
                let ns_path = NSString::from_str(&path.to_string_lossy());
                if let Some(img) = NSImage::initWithContentsOfFile(NSImage::alloc(), &ns_path) {
                    unsafe { app.setApplicationIconImage(Some(&img)) };
                    return;
                }
            }
        }
    }

    // 2) 回退：编译期内嵌 PNG（cargo run / 裸二进制启动）
    let data = NSData::with_bytes(include_bytes!("../icon_embedded.png"));
    if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
        unsafe { app.setApplicationIconImage(Some(&img)) };
    }
}
