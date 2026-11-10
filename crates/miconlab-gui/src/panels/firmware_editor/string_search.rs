// 字符串搜索功能

use super::memory::FirmwareMemory;

/// 在固件内存中搜索可打印字符串
pub fn search_strings(mem: &FirmwareMemory, min_len: usize) -> Vec<(u32, String)> {
    let mut results = Vec::new();
    let mut current = String::new();
    let mut start_addr: u32 = 0;
    let mut addrs: Vec<u32> = mem.cells.keys().copied().collect();
    addrs.sort();

    let mut prev_addr: Option<u32> = None;
    for &addr in &addrs {
        let b = mem.cells[&addr].value;
        // 检查地址连续性
        let is_contiguous = prev_addr.map_or(false, |p| addr == p + 1);

        if !is_contiguous {
            if current.len() >= min_len {
                results.push((start_addr, current.clone()));
            }
            current.clear();
        }

        if current.is_empty() {
            start_addr = addr;
        }

        if b >= 0x20 && b < 0x7f {
            current.push(b as char);
        } else {
            if current.len() >= min_len {
                results.push((start_addr, current.clone()));
            }
            current.clear();
        }

        prev_addr = Some(addr);
    }
    if current.len() >= min_len {
        results.push((start_addr, current));
    }

    // 限制结果数量
    results.truncate(500);
    results
}
