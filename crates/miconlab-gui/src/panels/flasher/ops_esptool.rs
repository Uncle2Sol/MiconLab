//! flasher 子模块：自动拆分生成，请勿手动编辑文件边界。
//!
//! 所有子模块通过 `use super::*;` 访问 mod.rs 中的 pub(crate) 类型与函数。

use super::*;
pub(crate) fn read_flash_via_esptool(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    use std::sync::{Arc, Mutex};

    let port_name = match state.selected_port_name() {
        Some(p) => {
            // macOS: /dev/tty.* → /dev/cu.* (esptool 需要 cu 版本，tty 版本可能阻塞)
            let p = p.to_string();
            if p.starts_with("/dev/tty.") {
                let cu = format!("/dev/cu.{}", &p["/dev/tty.".len()..]);
                log_push(fs, crate::i18n::tf("   Port mapping: {} → {}",
                    &[("{}", "{}", &format!("{}", p)), ("{}", "{}", &format!("{}", cu))]));
                cu
            } else {
                p
            }
        }
        None => {
            log_push(fs, crate::i18n::t("<< Cannot get serial port name"));
            state.last_error = Some("No port name".to_string());
            return;
        }
    };

    let total = fs.read_len.max(1) as usize;
    let addr = fs.address;
    // esptool 用 115200 波特率（--no-stub 模式下 USB-Serial/JTAG 忽略波特率）
    let baud = 115200u32;
    let temp_file = std::env::temp_dir().join(format!(
        "esp32_flash_0x{:08X}_{}.bin",
        addr,
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ));
    let temp_path = temp_file.to_string_lossy().to_string();

    log_push(fs, crate::i18n::tf(
        ">> esptool.py fallback read: {} @ 0x{:08X} {} bytes (baud={})",
        &[("{}", "{}", &format!("{}", port_name)),
          ("{:08X}", "{:08X}", &format!("{:08X}", addr)),
          ("{}", "{}", &format!("{}", total)),
          ("{}", "{}", &format!("{}", baud))],
    ));

    // 策略：先启动 esptool 线程（Python 加载 ~200ms），再断开 miconlab
    // 这样端口空闲时间最短（~200ms），避免 USB-CDC 重新枚举导致芯片重启
    log_push(fs, crate::i18n::t("   esptool starting in background (loading Python first, then releasing the port)..."));

    // 创建共享槽
    let result_slot: Arc<Mutex<Option<Result<Vec<u8>, String>>>> = Arc::new(Mutex::new(None));
    let log_slot: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let port_released: Arc<std::sync::atomic::AtomicBool> =
        Arc::new(std::sync::atomic::AtomicBool::new(false));
    fs.esptool_result_slot = Some(result_slot.clone());
    fs.esptool_log_slot = Some(log_slot.clone());
    fs.esptool_temp_file = Some(temp_path.clone());
    fs.esptool_running = true;
    fs.esptool_last_log = Some(std::time::Instant::now());
    fs.esptool_started_at = Some(std::time::Instant::now());
    fs.progress = 0.0;
    // 关键：阻止 poll_rx 读取串口，否则会抢走 esptool 的响应数据
    state.flasher_read_busy = true;

    // 启动后台线程：等待端口释放后执行 esptool
    let temp_file_cleanup = temp_file.clone();
    let port_released_clone = port_released.clone();
    std::thread::spawn(move || {
        let push_log = |msg: &str| {
            if let Ok(mut buf) = log_slot.lock() {
                buf.push(msg.to_string());
            }
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // 等待 miconlab 释放端口（最多等 2 秒）
            let start = std::time::Instant::now();
            while !port_released_clone.load(std::sync::atomic::Ordering::SeqCst) {
                if start.elapsed() > std::time::Duration::from_secs(2) {
                    return Err(crate::i18n::t("Timed out waiting for the port to be released").to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            push_log(crate::i18n::t("Port released, starting esptool..."));

            // 用 sh -c 包装，2>&1 合并 stderr 到 stdout，python3 -u 无缓冲
            // --no-stub: ESP32-S3 USB-Serial/JTAG 模式下，stub 的波特率切换会导致失联
            let cmd = format!(
                "python3 -u -m esptool --port {} --baud {} --before default_reset --after hard_reset --no-stub read_flash {} {} {} 2>&1",
                port_name, baud, addr, total, temp_path
            );
            push_log(&format!("exec: {}", cmd));

            let mut child = match std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .stdout(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => return Err(crate::i18n::tf("Failed to start esptool: {}",
                    &[("{}", "{}", &format!("{}", e))])),
            };

            push_log(&format!("esptool PID={}", child.id()));

            // 实时读取 stdout（已合并 stderr）
            if let Some(stdout) = child.stdout.take() {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            push_log(&l);
                        }
                        Err(_) => break,
                    }
                }
            }

            // 等待进程结束
            let status = match child.wait() {
                Ok(s) => s,
                Err(e) => return Err(crate::i18n::tf("Failed while waiting for esptool: {}",
                    &[("{}", "{}", &format!("{}", e))])),
            };

            if status.success() {
                match std::fs::read(&temp_file) {
                    Ok(data) => Ok(data),
                    Err(e) => Err(crate::i18n::tf("Failed to read temp file: {}",
                        &[("{}", "{}", &format!("{}", e))])),
                }
            } else {
                Err(crate::i18n::tf("esptool exit code: {:?}",
                    &[("{:?}", "{:?}", &format!("{:?}", status.code()))]))
            }
        }));

        let final_result = match result {
            Ok(Ok(data)) => Ok(data),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(crate::i18n::t("esptool background thread panicked").to_string()),
        };

        // 清理临时文件
        let _ = std::fs::remove_file(&temp_file_cleanup);

        // 写入结果
        if let Ok(mut slot) = result_slot.lock() {
            *slot = Some(final_result);
        }
    });

    // 保存 port_released 标志到 FlasherState（poll 函数中不再需要，但保留引用避免提前 drop）
    fs.esptool_port_released = Some(port_released.clone());

    // 等 100ms 让 Python 开始加载，然后断开串口释放端口
    // 这段窗口期内 Python 在加载模块，不需要串口
    std::thread::sleep(std::time::Duration::from_millis(100));
    log_push(fs, crate::i18n::t("   Disconnecting serial port (releasing it for esptool)..."));
    state.disconnect();
    // 通知后台线程：端口已释放，可以启动 esptool
    port_released.store(true, std::sync::atomic::Ordering::SeqCst);

    // 立即返回，不阻塞 UI
}

/// 轮询 esptool 后台线程结果（在 update_flasher_read 中每帧调用）
pub(crate) fn poll_esptool_result(fs: &mut FlasherState, state: &mut AppState, _is_zh: bool) {
    if !fs.esptool_running {
        return;
    }

    // 读取后台线程的实时日志并转发到 UI（先收集再释放锁，避免借用冲突）
    let new_lines: Vec<String> = if let Some(ref log_slot) = fs.esptool_log_slot {
        if let Ok(mut buf) = log_slot.lock() {
            buf.drain(..).collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    for line in new_lines {
        log_push(fs, &format!("   | {}", line));
    }

    // 超时检测：5 分钟未完成则判定失败（4MB @ 921600 约需 45 秒，留足余量）
    if fs.esptool_running
        && fs.esptool_started_at.map_or(false, |t| t.elapsed() >= std::time::Duration::from_secs(300))
    {
        log_push(fs, crate::i18n::t("<< esptool timed out (not finished within 5 minutes), marking as failed"));
        fs.esptool_running = false;
        fs.esptool_result_slot = None;
        fs.esptool_log_slot = None;
        fs.esptool_temp_file = None;
        fs.esptool_started_at = None;
        fs.esptool_port_released = None;
        state.flasher_read_busy = false; // 恢复 poll_rx
        state.connect(); // 重新连接串口
        state.last_error = Some("esptool timeout".to_string());
        return;
    }

    // 检查结果是否就绪
    let result = fs.esptool_result_slot
        .as_ref()
        .and_then(|slot| {
            match slot.lock() {
                Ok(mut guard) => guard.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            }
        });

    if let Some(result) = result {
        // esptool 完成，重新连接串口
        log_push(fs, crate::i18n::t("   esptool finished, reconnecting serial port..."));
        state.connect();

        // 同步 session ID，防止 show 函数清空日志和数据
        fs.last_session_id = state.connection_session_id;

        fs.esptool_running = false;
        fs.esptool_result_slot = None;
        fs.esptool_log_slot = None;
        fs.esptool_temp_file = None;
        fs.esptool_started_at = None;
        fs.esptool_port_released = None;
        state.flasher_read_busy = false; // 恢复 poll_rx

        match result {
            Ok(data) => {
                fs.last_read_data = data;
                fs.read_chip = ChipType::Esp32;
                fs.progress = 1.0;
                log_push(fs, crate::i18n::tf(
                    "<< esptool read succeeded: {} bytes (you can now 'Save Backup' or 'Export Disasm')",
                    &[("{}", "{}", &format!("{}", fs.last_read_data.len()))],
                ));
                state.last_error = None;
            }
            Err(e) => {
                log_push(fs, crate::i18n::tf("<< esptool failed: {}",
                    &[("{}", "{}", &format!("{}", e))]));
                state.last_error = Some(e);
            }
        }
    }
}

// 从原始串口数据中提取并解码一个 SLIP 帧
//
// ESP32 SLIP 格式：`0xC0 <body> 0xC0`，body 中：
//   - `0xDB 0xDC` → `0xC0`
//   - `0xDB 0xDD` → `0xDB`
//
// 返回 `(解码后的 body, 消费的原始字节数)`，无完整帧则返回 `None`
// （slip_decode_frame 实现位于 ops_esp32.rs）


