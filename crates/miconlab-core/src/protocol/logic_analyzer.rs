//! 软件逻辑分析仪
//!
//! 通过软件协议解码（无硬件逻辑分析仪），分析 TX/RX 时序关系。
//! 参考 PROBES 功率仪逆向案例：
//! - RX 帧与最近 TX 帧 latency ∈ (0, 100ms) 视为响应
//! - 建立命令→响应映射表
//! - 多次采样归纳命令字、长度、参数、CRC 结构

use std::collections::VecDeque;

/// 数据方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Tx,
    Rx,
}

/// 带时间戳的字节
#[derive(Debug, Clone, Copy)]
pub struct TimedByte {
    pub byte: u8,
    pub timestamp_us: u64,
    pub direction: Direction,
}

/// 解码后的协议帧
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// 帧方向
    pub direction: Direction,
    /// 帧起始时间戳（微秒）
    pub start_us: u64,
    /// 帧结束时间戳（微秒）
    pub end_us: u64,
    /// 帧原始字节
    pub data: Vec<u8>,
    /// 协议类型标签
    pub protocol: &'static str,
    /// 解码摘要（如 "Modbus RTU, addr=1, func=3"）
    pub summary: String,
}

/// 推断的命令
#[derive(Debug, Clone)]
pub struct InferredCommand {
    pub direction: Direction,
    pub frame: DecodedFrame,
    /// 与关联响应的延迟（毫秒）
    pub latency_ms: f32,
    /// 关联的响应帧索引
    pub correlated_response: Option<usize>,
}

/// 命令-响应对
#[derive(Debug, Clone)]
pub struct CommandResponse {
    pub command: DecodedFrame,
    pub response: Option<DecodedFrame>,
    pub latency_ms: f32,
}

/// 协议扫描器 trait
pub trait ProtocolScanner: Send + Sync {
    /// 扫描时间线，返回解码帧列表
    fn scan(&self, timeline: &[TimedByte]) -> Vec<DecodedFrame>;
    /// 协议名称
    fn name(&self) -> &'static str;
}

#[cfg(feature = "device-workspaces")]
include!("logic_analyzer_device.rs");

/// 自定义帧头扫描器
pub struct CustomHeaderScanner {
    pub header: Vec<u8>,
    pub min_len: usize,
    pub max_len: usize,
    pub name: String,
}

impl ProtocolScanner for CustomHeaderScanner {
    fn scan(&self, timeline: &[TimedByte]) -> Vec<DecodedFrame> {
        let mut frames = Vec::new();
        if self.header.is_empty() {
            return frames;
        }
        let mut i = 0;
        while i + self.header.len() < timeline.len() {
            let matches = self.header
                .iter()
                .enumerate()
                .all(|(j, &h)| timeline[i + j].byte == h);
            if matches {
                // 查找帧结束（方向变化或超时）
                let start_dir = timeline[i].direction;
                let start_ts = timeline[i].timestamp_us;
                let mut end_idx = i;
                for j in i..timeline.len().min(i + self.max_len) {
                    if timeline[j].direction != start_dir {
                        break;
                    }
                    end_idx = j;
                    if end_idx - i + 1 >= self.min_len {
                        break;
                    }
                }
                let data: Vec<u8> = timeline[i..=end_idx].iter().map(|tb| tb.byte).collect();
                if data.len() >= self.min_len {
                    frames.push(DecodedFrame {
                        direction: start_dir,
                        start_us: start_ts,
                        end_us: timeline[end_idx].timestamp_us,
                        data,
                        protocol: "Custom",
                        summary: format!("Custom header {:02X?} ({}B)", self.header, end_idx - i + 1),
                    });
                    i = end_idx + 1;
                    continue;
                }
                // 不满足 min_len：可能是噪声，i 前进 1 继续搜索（避免跳过中间数据）
            }
            i += 1;
        }
        frames
    }

    fn name(&self) -> &'static str {
        "Custom"
    }
}

/// 软件逻辑分析仪
pub struct SoftwareLogicAnalyzer {
    scanners: Vec<Box<dyn ProtocolScanner>>,
    timeline: VecDeque<TimedByte>,
    commands: Vec<InferredCommand>,
    /// 时间线最大长度（防止无限增长）
    max_timeline: usize,
}

impl SoftwareLogicAnalyzer {
    /// 创建逻辑分析仪，预置默认扫描器
    pub fn new() -> Self {
        Self {
            scanners: {
                #[cfg(feature = "device-workspaces")]
                {
                    vec![Box::new(CushionFrameScanner)]
                }
                #[cfg(not(feature = "device-workspaces"))]
                {
                    Vec::new()
                }
            },
            timeline: VecDeque::with_capacity(4096),
            commands: Vec::new(),
            max_timeline: 8192,
        }
    }

    /// 添加自定义扫描器
    pub fn add_scanner(&mut self, scanner: Box<dyn ProtocolScanner>) {
        self.scanners.push(scanner);
    }

    /// 喂入一个字节
    pub fn ingest(&mut self, byte: u8, direction: Direction, ts: u64) {
        self.timeline.push_back(TimedByte {
            byte,
            timestamp_us: ts,
            direction,
        });
        // 限制时间线长度
        while self.timeline.len() > self.max_timeline {
            self.timeline.pop_front();
        }
    }

    /// 喂入字节序列
    pub fn ingest_slice(&mut self, data: &[u8], direction: Direction, ts: u64) {
        for (i, &b) in data.iter().enumerate() {
            self.ingest(b, direction, ts + i as u64 * 10); // 每字节 10μs 估算
        }
    }

    /// 扫描当前时间线，返回所有解码帧
    pub fn scan_all(&self) -> Vec<DecodedFrame> {
        let timeline: Vec<TimedByte> = self.timeline.iter().copied().collect();
        let mut all_frames = Vec::new();
        for scanner in &self.scanners {
            let frames = scanner.scan(&timeline);
            all_frames.extend(frames);
        }
        // 按时间戳排序
        all_frames.sort_by_key(|f| f.start_us);
        all_frames
    }

    /// 关联 TX 命令与 RX 响应
    ///
    /// 对每个 TX 帧，在时间窗口内（前向 100ms + 后向 100ms）查找最近的 RX 帧作为响应。
    /// 双向查找避免漏配对：先查后向（TX 之后），再查前向（TX 之前，处理 RX 先到的场景）。
    pub fn correlate(&mut self) -> Vec<CommandResponse> {
        let frames = self.scan_all();
        let mut pairs = Vec::new();

        for (i, frame) in frames.iter().enumerate() {
            if frame.direction != Direction::Tx {
                continue;
            }

            let mut best_response = None;
            let mut best_latency_ms = f32::MAX;

            // 后向查找：TX 之后的 RX（标准场景）
            for resp in frames.iter().skip(i + 1) {
                if resp.direction != Direction::Rx {
                    continue;
                }
                let lat = (resp.start_us as f64 - frame.end_us as f64) / 1000.0;
                if lat < 0.0 {
                    continue;
                }
                if lat >= 100.0 {
                    break; // 已按时间排序，后续帧更远
                }
                if (lat as f32) < best_latency_ms {
                    best_latency_ms = lat as f32;
                    best_response = Some(resp.clone());
                }
            }

            // 前向查找：TX 之前的 RX（RX 先到的异常场景，如设备主动上报）
            if best_response.is_none() {
                for resp in frames.iter().take(i).rev() {
                    if resp.direction != Direction::Rx {
                        continue;
                    }
                    let lat = (frame.start_us as f64 - resp.end_us as f64) / 1000.0;
                    if lat < 0.0 || lat >= 100.0 {
                        continue;
                    }
                    if (lat as f32) < best_latency_ms {
                        best_latency_ms = lat as f32;
                        best_response = Some(resp.clone());
                    }
                }
            }

            let latency_ms = if best_response.is_some() {
                best_latency_ms
            } else {
                0.0
            };

            pairs.push(CommandResponse {
                command: frame.clone(),
                response: best_response,
                latency_ms,
            });
        }

        // 更新 commands 列表
        self.commands = pairs
            .iter()
            .map(|p| InferredCommand {
                direction: p.command.direction,
                frame: p.command.clone(),
                latency_ms: p.latency_ms,
                correlated_response: None, // 简化：不追踪索引
            })
            .collect();

        pairs
    }

    /// 获取已注册的扫描器名称
    pub fn scanner_names(&self) -> Vec<&'static str> {
        self.scanners.iter().map(|s| s.name()).collect()
    }

    /// 清空时间线
    pub fn clear(&mut self) {
        self.timeline.clear();
        self.commands.clear();
    }
}

impl Default for SoftwareLogicAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

        #[cfg(feature = "device-workspaces")]
    include!("logic_analyzer_device_tests.rs");

    #[test]
    fn test_correlate() {
        let mut analyzer = SoftwareLogicAnalyzer::new();
        // 注册自定义帧头扫描器（4 字节最小长度）
        analyzer.add_scanner(Box::new(CustomHeaderScanner {
            header: vec![0xA5, 0x0A],
            min_len: 4,
            max_len: 256,
            name: "TestA5".to_string(),
        }));

        // TX 帧：0xA5 0x0A + 2 字节数据（满足 min_len=4）
        analyzer.ingest_slice(&[0xA5, 0x0A, 0x01, 0x02], Direction::Tx, 1000);
        // 50ms 后 RX 响应：同样帧头 + 不同数据
        analyzer.ingest_slice(&[0xA5, 0x0A, 0x03, 0x04], Direction::Rx, 51000);
        let pairs = analyzer.correlate();
        assert!(!pairs.is_empty(), "应至少配对到一对 TX→RX");
        assert!(pairs[0].latency_ms > 0.0, "延迟应为正值");
    }

    #[test]
    fn test_custom_header_scan() {
        let scanner = CustomHeaderScanner {
            header: vec![0xAA, 0x55],
            min_len: 4,
            max_len: 256,
            name: "Test".to_string(),
        };
        let mut timeline = Vec::new();
        for (i, &b) in [0xAA, 0x55, 0x01, 0x02].iter().enumerate() {
            timeline.push(TimedByte {
                byte: b,
                timestamp_us: i as u64,
                direction: Direction::Rx,
            });
        }
        let frames = scanner.scan(&timeline);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data.len(), 4);
    }

    // ==================== D.9 边界条件测试 ====================

    #[test]
    fn test_correlate_rx_before_tx() {
        // Bug 修复验证：RX 在 TX 之前到达（设备主动上报），原 correlate 只向后搜索会漏配对
        let mut analyzer = SoftwareLogicAnalyzer::new();
        analyzer.add_scanner(Box::new(CustomHeaderScanner {
            header: vec![0xA5, 0x0A],
            min_len: 4,
            max_len: 256,
            name: "TestA5".to_string(),
        }));

        // RX 先到（设备主动上报）
        analyzer.ingest_slice(&[0xA5, 0x0A, 0x01, 0x02], Direction::Rx, 1000);
        // TX 后到（用户发送命令）
        analyzer.ingest_slice(&[0xA5, 0x0A, 0x03, 0x04], Direction::Tx, 5000);
        let pairs = analyzer.correlate();
        assert!(!pairs.is_empty(), "RX 先到场景应能配对");
        assert!(pairs[0].response.is_some(), "应找到 RX 响应");
    }

    #[test]
    fn test_correlate_multiple_tx_single_rx() {
        // 多个 TX 命令对应一个 RX 响应
        let mut analyzer = SoftwareLogicAnalyzer::new();
        analyzer.add_scanner(Box::new(CustomHeaderScanner {
            header: vec![0xA5, 0x0A],
            min_len: 4,
            max_len: 256,
            name: "TestA5".to_string(),
        }));

        analyzer.ingest_slice(&[0xA5, 0x0A, 0x01, 0x01], Direction::Tx, 1000);
        analyzer.ingest_slice(&[0xA5, 0x0A, 0x02, 0x02], Direction::Tx, 2000);
        analyzer.ingest_slice(&[0xA5, 0x0A, 0x03, 0x03], Direction::Rx, 3000);
        let pairs = analyzer.correlate();
        // 至少应有一个 TX 配对到 RX
        assert!(pairs.iter().any(|p| p.response.is_some()));
    }

    #[test]
    fn test_correlate_no_rx() {
        // 只有 TX 无 RX
        let mut analyzer = SoftwareLogicAnalyzer::new();
        analyzer.add_scanner(Box::new(CustomHeaderScanner {
            header: vec![0xA5, 0x0A],
            min_len: 4,
            max_len: 256,
            name: "TestA5".to_string(),
        }));

        analyzer.ingest_slice(&[0xA5, 0x0A, 0x01, 0x02], Direction::Tx, 1000);
        let pairs = analyzer.correlate();
        assert!(!pairs.is_empty());
        assert!(pairs[0].response.is_none());
        assert_eq!(pairs[0].latency_ms, 0.0);
    }

    #[test]
    fn test_custom_header_noise_before_frame() {
        // 噪声数据不满足 min_len，不应跳过中间的有效帧
        let scanner = CustomHeaderScanner {
            header: vec![0xA5, 0x0A],
            min_len: 4,
            max_len: 256,
            name: "TestA5".to_string(),
        };
        let mut timeline = Vec::new();
        // 噪声：只有帧头但长度不足
        timeline.push(TimedByte { byte: 0xA5, timestamp_us: 0, direction: Direction::Tx });
        timeline.push(TimedByte { byte: 0x0A, timestamp_us: 1, direction: Direction::Tx });
        // 方向变化，噪声帧结束（只有 2 字节 < min_len=4）
        timeline.push(TimedByte { byte: 0x00, timestamp_us: 2, direction: Direction::Rx });
        // 有效帧
        timeline.push(TimedByte { byte: 0xA5, timestamp_us: 3, direction: Direction::Tx });
        timeline.push(TimedByte { byte: 0x0A, timestamp_us: 4, direction: Direction::Tx });
        timeline.push(TimedByte { byte: 0x01, timestamp_us: 5, direction: Direction::Tx });
        timeline.push(TimedByte { byte: 0x02, timestamp_us: 6, direction: Direction::Tx });

        let frames = scanner.scan(&timeline);
        assert_eq!(frames.len(), 1, "噪声不应阻止后续有效帧的检测");
        assert_eq!(frames[0].data.len(), 4);
    }

    #[test]
    fn test_timeline_max_capacity() {
        // 时间线容量限制：超过 max_timeline 后旧数据被丢弃
        let mut analyzer = SoftwareLogicAnalyzer::new();
        for i in 0..10000 {
            analyzer.ingest(0x00, Direction::Tx, i as u64);
        }
        // timeline 应被限制在 max_timeline (8192) 以内
        assert!(analyzer.timeline.len() <= 8192);
    }
}
