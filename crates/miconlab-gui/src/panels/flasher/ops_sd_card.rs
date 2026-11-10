//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
// =====================================================================
// SD 卡镜像烧录实现（调用系统命令 diskutil/dd）
// =====================================================================

/// 扫描系统磁盘列表
///
/// macOS: diskutil list
/// Linux: lsblk
/// Windows: wmic diskdrive
pub(crate) fn do_sd_scan_disks(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    // 使用 Command::new 直接调用，避免 sh -c（虽 cmd_str 为固定字符串无注入风险，
    // 但保持与 do_sd_unmount/eject/flash_image 一致的安全风格）
    let output = if cfg!(target_os = "macos") {
        log_push(fs, ">> Scan Disks: diskutil list");
        std::process::Command::new("diskutil").arg("list").output()
    } else if cfg!(target_os = "linux") {
        log_push(fs, ">> Scan Disks: lsblk -d -o NAME,SIZE,TYPE,MODEL");
        std::process::Command::new("lsblk")
            .arg("-d")
            .arg("-o")
            .arg("NAME,SIZE,TYPE,MODEL")
            .output()
    } else {
        log_push(fs, ">> Scan Disks: wmic diskdrive get Model,Size,DeviceID");
        std::process::Command::new("wmic")
            .arg("diskdrive")
            .arg("get")
            .arg("Model,Size,DeviceID")
            .output()
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if !stdout.is_empty() {
                fs.sd_card_disk_list = stdout.clone();
                log_push(fs, format!("<< Found {} disks", stdout.lines().count().saturating_sub(1)));
                state.last_error = None;
            } else if !stderr.is_empty() {
                log_push(fs, format!("<< stderr: {}", stderr.trim()));
                state.last_error = Some(stderr.to_string());
            } else {
                fs.sd_card_disk_list = String::new();
                log_push(fs, "<< No disks found");
                state.last_error = None;
            }
        }
        Err(e) => {
            let msg = format!("Failed to scan disks: {}", e);
            log_push(fs, format!("<< {}", msg));
            state.last_error = Some(msg);
        }
    }
}

/// 检查 SD 卡设备名是否安全（防止误写系统盘）
/// 返回 Some(错误消息) 如果不安全，None 表示通过
pub(crate) fn validate_sd_disk(disk: &str, _is_zh: bool) -> Option<String> {
    // 系统盘黑名单
    const SYSTEM_DISKS: &[&str] = &[
        "/dev/disk0", "/dev/rdisk0",
        "/dev/sda", "/dev/sda1", "/dev/sda2",
        "/dev/nvme0n1", "/dev/nvme0n1p1", "/dev/nvme0n1p2",
        "/dev/mmcblk0", "/dev/mmcblk0p1", "/dev/mmcblk0p2",
    ];
    if SYSTEM_DISKS.iter().any(|&d| disk == d) {
        return Some(crate::i18n::tf("⚠ Safety check failed: {} looks like system disk, blocked", &[("{}", "{}", &format!("{}", disk))]));
    }
    // 白名单：仅允许符合 SD 卡设备名格式的路径
    let valid = if cfg!(target_os = "macos") {
        disk.starts_with("/dev/disk") || disk.starts_with("/dev/rdisk")
    } else if cfg!(target_os = "linux") {
        disk.starts_with("/dev/sd") || disk.starts_with("/dev/mmcblk")
    } else {
        true // Windows 不检查
    };
    if !valid {
        return Some(crate::i18n::tf("⚠ Safety check failed: {} does not match SD card device pattern", &[("{}", "{}", &format!("{}", disk))]));
    }
    None
}

/// 卸载 SD 卡所有分区
pub(crate) fn do_sd_unmount(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    let disk = fs.sd_card_disk.trim().to_string();
    if disk.is_empty() {
        state.last_error = Some(crate::i18n::t("SD card device path is empty").to_string());
        return;
    }
    if let Some(err) = validate_sd_disk(&disk, is_zh) {
        state.last_error = Some(err);
        return;
    }

    // 使用 Command::new 直接调用，避免 shell 注入
    let result = if cfg!(target_os = "macos") {
        log_push(fs, format!(">> Unmount: diskutil unmountDisk {}", disk));
        std::process::Command::new("diskutil")
            .arg("unmountDisk")
            .arg(&disk)
            .output()
    } else if cfg!(target_os = "linux") {
        let pattern = format!("{}?*", disk);
        log_push(fs, format!(">> Unmount: umount {}", pattern));
        if fs.sd_card_need_sudo {
            std::process::Command::new("sudo")
                .arg("umount")
                .arg(&pattern)
                .output()
        } else {
            std::process::Command::new("umount")
                .arg(&pattern)
                .output()
        }
    } else {
        log_push(fs, format!(">> Unmount: mountvol {} /d", disk));
        std::process::Command::new("mountvol")
            .arg(&disk)
            .arg("/d")
            .output()
    };

    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if out.status.success() {
                log_push(fs, format!("<< Unmount OK: {}", stdout.trim()));
                state.last_error = None;
            } else {
                let msg = format!("Unmount failed: {}", if stderr.is_empty() { stdout.trim() } else { stderr.trim() });
                log_push(fs, format!("<< {}", msg));
                state.last_error = Some(msg);
            }
        }
        Err(e) => {
            let msg = format!("Failed to run unmount: {}", e);
            log_push(fs, format!("<< {}", msg));
            state.last_error = Some(msg);
        }
    }
}

/// 烧录镜像到 SD 卡（dd 命令）
pub(crate) fn do_sd_flash_image(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    let disk = fs.sd_card_disk.trim().to_string();
    let image = fs.sd_card_image_path.trim().to_string();

    if disk.is_empty() {
        state.last_error = Some(crate::i18n::t("SD card device path is empty").to_string());
        return;
    }
    if image.is_empty() {
        state.last_error = Some(crate::i18n::t("Image file path is empty").to_string());
        return;
    }
    if let Some(err) = validate_sd_disk(&disk, is_zh) {
        state.last_error = Some(err);
        return;
    }

    if cfg!(target_os = "windows") {
        log_push(fs, crate::i18n::t("<< Windows: use Rufus/balenaEtcher, dd not supported"));
        state.last_error = Some("Windows SD flashing not supported, use Rufus".to_string());
        return;
    }

    // macOS 上需要用 /dev/rdiskN 提速
    let disk_arg = if cfg!(target_os = "macos") && disk.starts_with("/dev/disk") {
        disk.replacen("/dev/disk", "/dev/rdisk", 1)
    } else {
        disk.clone()
    };

    log_push(fs, format!(">> Flash Image: dd if={} of={} bs=4M conv=fsync", image, disk_arg));
    log_push(fs, crate::i18n::t("  Flashing (please wait, no progress)..."));

    // 使用 Command::new 直接调用 dd，避免 shell 注入
    let cmd = if fs.sd_card_need_sudo {
        std::process::Command::new("sudo")
            .arg("dd")
            .arg(format!("if={}", image))
            .arg(format!("of={}", disk_arg))
            .arg("bs=4M")
            .arg("conv=fsync")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    } else {
        std::process::Command::new("dd")
            .arg(format!("if={}", image))
            .arg(format!("of={}", disk_arg))
            .arg("bs=4M")
            .arg("conv=fsync")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    };

    match cmd {
        Ok(child) => {
            match child.wait_with_output() {
                Ok(out) => {
                    if out.status.success() {
                        log_push(fs, "<< Flash complete");
                        fs.progress = 1.0;
                        state.last_error = None;
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                        let msg = format!("Flash failed: {}", stderr.trim());
                        log_push(fs, format!("<< {}", msg));
                        state.last_error = Some(msg);
                    }
                }
                Err(e) => {
                    let msg = format!("Wait failed: {}", e);
                    log_push(fs, format!("<< {}", msg));
                    state.last_error = Some(msg);
                }
            }
        }
        Err(e) => {
            let msg = format!("Failed to spawn dd: {}", e);
            log_push(fs, format!("<< {}", msg));
            state.last_error = Some(msg);
        }
    }
}

/// 弹出 SD 卡
pub(crate) fn do_sd_eject(fs: &mut FlasherState, state: &mut AppState, is_zh: bool) {
    let disk = fs.sd_card_disk.trim().to_string();
    if disk.is_empty() {
        state.last_error = Some(crate::i18n::t("SD card device path is empty").to_string());
        return;
    }
    if let Some(err) = validate_sd_disk(&disk, is_zh) {
        state.last_error = Some(err);
        return;
    }

    // 使用 Command::new 直接调用，避免 shell 注入
    let result = if cfg!(target_os = "macos") {
        log_push(fs, format!(">> Eject: diskutil eject {}", disk));
        std::process::Command::new("diskutil")
            .arg("eject")
            .arg(&disk)
            .output()
    } else if cfg!(target_os = "linux") {
        log_push(fs, format!(">> Eject: eject {}", disk));
        std::process::Command::new("eject")
            .arg(&disk)
            .output()
    } else {
        log_push(fs, crate::i18n::t("<< Windows needs no eject, just unplug the card"));
        state.last_error = None;
        return;
    };

    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if out.status.success() {
                log_push(fs, format!("<< Eject OK: {}", stdout.trim()));
                state.last_error = None;
            } else {
                let msg = format!("Eject failed: {}", if stderr.is_empty() { stdout.trim() } else { stderr.trim() });
                log_push(fs, format!("<< {}", msg));
                state.last_error = Some(msg);
            }
        }
        Err(e) => {
            let msg = format!("Failed to eject: {}", e);
            log_push(fs, format!("<< {}", msg));
            state.last_error = Some(msg);
        }
    }
}

// =====================================================================

