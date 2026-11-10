// 固件内存模型（稀疏地址空间）
// 支持 BIN 和 HEX 统一表示，提供读写、修改追踪和导出功能

use std::collections::BTreeMap;

/// 单字节内存单元（带修改标记）
#[derive(Clone)]
pub struct MemoryCell {
    pub value: u8,
    pub modified: bool,
}

/// 固件内存模型：稀疏地址空间，支持 BIN 和 HEX 统一表示
#[derive(Clone)]
pub struct FirmwareMemory {
    /// 地址 -> 字节（BTreeMap 保持地址有序）
    pub cells: BTreeMap<u32, MemoryCell>,
    /// 最低地址
    pub base_addr: u32,
    /// 最高地址+1
    pub end_addr: u32,
    /// 原始数据备份（用于差分对比）
    pub original: BTreeMap<u32, u8>,
}

impl FirmwareMemory {
    pub fn new() -> Self {
        Self {
            cells: BTreeMap::new(),
            base_addr: 0,
            end_addr: 0,
            original: BTreeMap::new(),
        }
    }

    /// 从二进制数据加载（BIN 格式，从指定基地址开始）
    pub fn from_binary(data: &[u8], base_addr: u32) -> Self {
        let mut mem = Self::new();
        for (i, &b) in data.iter().enumerate() {
            let addr = base_addr.wrapping_add(i as u32);
            mem.cells.insert(
                addr,
                MemoryCell {
                    value: b,
                    modified: false,
                },
            );
            mem.original.insert(addr, b);
        }
        if !data.is_empty() {
            mem.base_addr = base_addr;
            mem.end_addr = base_addr.wrapping_add(data.len() as u32);
        }
        mem
    }

    /// 重新设置基地址（平移所有 cells 和 original 的键）
    /// 用于 BIN 文件加载后，根据自动检测的芯片类型调整地址
    pub fn rebase(&mut self, new_base: u32) {
        if self.base_addr == new_base || self.cells.is_empty() {
            return;
        }
        let offset = new_base.wrapping_sub(self.base_addr);
        // 平移 cells
        let old_cells = std::mem::take(&mut self.cells);
        for (addr, cell) in old_cells {
            self.cells.insert(addr.wrapping_add(offset), cell);
        }
        // 平移 original
        let old_original = std::mem::take(&mut self.original);
        for (addr, val) in old_original {
            self.original.insert(addr.wrapping_add(offset), val);
        }
        self.base_addr = new_base;
        self.end_addr = self.end_addr.wrapping_add(offset);
    }

    /// 从 Intel HEX 字符串加载
    pub fn from_hex(content: &str) -> Result<Self, String> {
        let mut mem = Self::new();
        let mut current_base: u32 = 0; // 扩展线性地址
        let mut ext_seg_base: u32 = 0; // 扩展段地址
        let mut has_data = false;

        for (line_no, line) in content.lines().enumerate() {
            let line = line.trim();
            if !line.starts_with(':') {
                continue;
            }
            if line.len() < 11 {
                return Err(format!("Line {}: too short", line_no + 1));
            }

            let byte_count = u8::from_str_radix(&line[1..3], 16)
                .map_err(|e| format!("Line {}: byte_count parse error: {}", line_no + 1, e))?;
            let addr_lo = u16::from_str_radix(&line[3..7], 16)
                .map_err(|e| format!("Line {}: addr parse error: {}", line_no + 1, e))? as u32;
            let rec_type = u8::from_str_radix(&line[7..9], 16)
                .map_err(|e| format!("Line {}: rec_type parse error: {}", line_no + 1, e))?;

            let data_start = 9;
            let data_end = data_start + byte_count as usize * 2;
            if line.len() < data_end + 2 {
                return Err(format!("Line {}: data too short", line_no + 1));
            }

            match rec_type {
                0x00 => {
                    // 数据记录
                    let abs_addr = current_base.wrapping_add(ext_seg_base).wrapping_add(addr_lo);
                    for i in 0..byte_count as usize {
                        let hex_pair = &line[data_start + i * 2..data_start + i * 2 + 2];
                        let b = u8::from_str_radix(hex_pair, 16)
                            .map_err(|e| format!("Line {}: data parse error: {}", line_no + 1, e))?;
                        mem.cells.insert(
                            abs_addr + i as u32,
                            MemoryCell {
                                value: b,
                                modified: false,
                            },
                        );
                        mem.original.insert(abs_addr + i as u32, b);
                    }
                    if !has_data {
                        mem.base_addr = abs_addr;
                    }
                    mem.end_addr = mem.end_addr.max(abs_addr + byte_count as u32);
                    has_data = true;
                }
                0x04 => {
                    // 扩展线性地址记录（固定 2 字节数据，需要 line.len() >= 13；
                    // 前面的长度检查对 byte_count=0 的畸形行只保证 len >= 11）
                    if line.len() < 13 {
                        return Err(format!("Line {}: ext addr record too short", line_no + 1));
                    }
                    let upper = u16::from_str_radix(&line[9..13], 16)
                        .map_err(|e| format!("Line {}: ext addr parse error: {}", line_no + 1, e))?
                        as u32;
                    current_base = upper << 16;
                    ext_seg_base = 0;
                }
                0x02 => {
                    // 扩展段地址记录（同 0x04，需要 line.len() >= 13）
                    if line.len() < 13 {
                        return Err(format!("Line {}: seg addr record too short", line_no + 1));
                    }
                    let seg = u16::from_str_radix(&line[9..13], 16)
                        .map_err(|e| format!("Line {}: seg addr parse error: {}", line_no + 1, e))?
                        as u32;
                    ext_seg_base = seg << 4;
                    current_base = 0;
                }
                0x01 => {
                    // EOF
                    break;
                }
                0x03 | 0x05 => {
                    // 起始地址记录，忽略
                }
                _ => {}
            }
        }

        if !has_data {
            return Err(crate::i18n::t("HEX file has no data records").to_string());
        }

        Ok(mem)
    }

    /// 导出为二进制数据（连续地址空间，空洞填充 0xFF）
    pub fn to_binary(&self) -> Vec<u8> {
        if self.cells.is_empty() {
            return Vec::new();
        }
        let size = (self.end_addr - self.base_addr) as usize;
        let mut data = vec![0xFFu8; size];
        for (&addr, cell) in &self.cells {
            if addr >= self.base_addr && addr < self.end_addr {
                let off = (addr - self.base_addr) as usize;
                data[off] = cell.value;
            }
        }
        data
    }

    /// 导出为 Intel HEX 字符串
    pub fn to_hex(&self) -> String {
        let mut output = String::new();
        let mut current_upper: u32 = 0xFFFF_FFFF; // 不存在的标记

        // 按地址排序遍历
        let mut addrs: Vec<u32> = self.cells.keys().copied().collect();
        addrs.sort();

        let mut i = 0;
        while i < addrs.len() {
            let addr = addrs[i];
            let upper = addr & 0xFFFF_0000;

            // 如果高位地址变化，输出扩展线性地址记录
            if upper != current_upper {
                current_upper = upper;
                let upper_val = (upper >> 16) as u16;
                let checksum = (0x02u8)
                    .wrapping_add(0u8)
                    .wrapping_add(0u8)
                    .wrapping_add(0x04u8)
                    .wrapping_add((upper_val >> 8) as u8)
                    .wrapping_add(upper_val as u8);
                output.push_str(&format!(
                    ":02000004{:04X}{:02X}\n",
                    upper_val,
                    (!checksum).wrapping_add(1)
                ));
            }

            // 收集连续地址的数据（最多 16 字节一行）
            let mut row_data: Vec<u8> = Vec::new();
            let row_start = addr;
            while i < addrs.len()
                && row_data.len() < 16
                && addrs[i] == row_start + row_data.len() as u32
            {
                row_data.push(self.cells[&addrs[i]].value);
                i += 1;
            }

            // 输出数据记录
            let addr_lo = (row_start & 0xFFFF) as u16;
            let byte_count = row_data.len() as u8;
            let mut checksum = byte_count
                .wrapping_add((addr_lo >> 8) as u8)
                .wrapping_add(addr_lo as u8)
                .wrapping_add(0x00);
            for &b in &row_data {
                checksum = checksum.wrapping_add(b);
            }
            let hex_data: String = row_data.iter().map(|b| format!("{:02X}", b)).collect();
            output.push_str(&format!(
                ":{:02X}{:04X}00{}{:02X}\n",
                byte_count,
                addr_lo,
                hex_data,
                (!checksum).wrapping_add(1)
            ));
        }

        // EOF 记录
        output.push_str(":00000001FF\n");
        output
    }

    /// 读取一个 u32（小端）
    pub fn read_u32_le(&self, addr: u32) -> Option<u32> {
        let b0 = self.cells.get(&addr)?.value;
        let b1 = self.cells.get(&(addr + 1))?.value;
        let b2 = self.cells.get(&(addr + 2))?.value;
        let b3 = self.cells.get(&(addr + 3))?.value;
        Some(u32::from_le_bytes([b0, b1, b2, b3]))
    }

    /// 写入一个 u32（小端）
    pub fn write_u32_le(&mut self, addr: u32, val: u32) {
        let bytes = val.to_le_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            let a = addr + i as u32;
            let modified = self.original.get(&a).map_or(true, |&orig| orig != b);
            self.cells.insert(a, MemoryCell { value: b, modified });
        }
    }

    /// 写入一个字节
    pub fn write_byte(&mut self, addr: u32, val: u8) {
        let modified = self
            .original
            .get(&addr)
            .map_or(true, |&orig| orig != val);
        self.cells.insert(addr, MemoryCell {
            value: val,
            modified,
        });
    }

    /// 统计修改字节数
    pub fn modified_count(&self) -> usize {
        self.cells.values().filter(|c| c.modified).count()
    }

    /// 总字节数
    pub fn total_bytes(&self) -> usize {
        self.cells.len()
    }

    /// 重置所有修改（恢复原始值）
    pub fn revert_all(&mut self) {
        for (addr, cell) in &mut self.cells {
            if let Some(&orig) = self.original.get(addr) {
                cell.value = orig;
                cell.modified = false;
            }
        }
    }
}
