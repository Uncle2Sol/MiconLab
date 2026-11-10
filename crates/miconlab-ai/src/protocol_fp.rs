//! 协议指纹识别引擎
//!
//! 基于 KNN (K-Nearest Neighbors) 分类器 + 模板匹配 + 帧头强匹配的混合识别策略。
//!
//! 识别流程：
//! 1. **帧头强匹配**：若检测到唯一性帧头（如 AA 55 03 99、0xE9、0x7F），直接给出高置信度判断
//! 2. **KNN 分类**：计算特征向量与所有模板/样本的欧氏距离，取 K 个最近邻投票
//! 3. **置信度融合**：帧头匹配加分 + KNN 票数 + 距离加权
//!
//! 增量学习：用户标注的样本通过 `add_sample` 加入分类器，后续识别会参考。

use crate::features::{KnownHeader, StreamFeatures};
use serde::{Deserialize, Serialize};

/// 识别出的协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProtocolClass {
    /// 未知协议
    Unknown,
    /// Modbus RTU（二进制，CRC16 校验）
    ModbusRtu,
    /// Modbus ASCII（冒号开头，LRC 校验）
    ModbusAscii,
    /// 纯 ASCII 文本
    PlainText,
    /// HDLC / PPP（0x7E 标志）
    Hdlc,
    /// 鞋垫压力传感器协议（AA 55 03 99）
    InsoleSensor,
    /// SLCAN CAN 总线 ASCII 命令
    Slcan,
    /// STM32 Bootloader 通信
    Stm32Bootloader,
    /// ESP 镜像数据
    EspImage,
    /// NMEA GPS 语句
    Nmea,
    /// 未知二进制协议
    BinaryRaw,
}

impl ProtocolClass {
    /// 协议中文名称
    pub fn label_zh(self) -> &'static str {
        match self {
            ProtocolClass::Unknown => "Unknown protocol",
            ProtocolClass::ModbusRtu => "Modbus RTU",
            ProtocolClass::ModbusAscii => "Modbus ASCII",
            ProtocolClass::PlainText => "Plain ASCII text",
            ProtocolClass::Hdlc => "HDLC/PPP",
            ProtocolClass::InsoleSensor => "Insole pressure sensor",
            ProtocolClass::Slcan => "SLCAN (CAN bus)",
            ProtocolClass::Stm32Bootloader => "STM32 Bootloader",
            ProtocolClass::EspImage => "ESP image",
            ProtocolClass::Nmea => "NMEA (GPS)",
            ProtocolClass::BinaryRaw => "Unknown binary protocol",
        }
    }

    /// 协议英文名称
    pub fn label_en(self) -> &'static str {
        match self {
            ProtocolClass::Unknown => "Unknown protocol",
            ProtocolClass::ModbusRtu => "Modbus RTU",
            ProtocolClass::ModbusAscii => "Modbus ASCII",
            ProtocolClass::PlainText => "Plain ASCII text",
            ProtocolClass::Hdlc => "HDLC/PPP",
            ProtocolClass::InsoleSensor => "Insole pressure sensor",
            ProtocolClass::Slcan => "SLCAN (CAN bus)",
            ProtocolClass::Stm32Bootloader => "STM32 Bootloader",
            ProtocolClass::EspImage => "ESP image",
            ProtocolClass::Nmea => "NMEA (GPS)",
            ProtocolClass::BinaryRaw => "Unknown binary protocol",
        }
    }

    /// 协议简述（中文）
    pub fn description_zh(self) -> &'static str {
        match self {
            ProtocolClass::Unknown => "protocol.unknown.desc",
            ProtocolClass::ModbusRtu => "protocol.modbus_rtu.desc",
            ProtocolClass::ModbusAscii => "protocol.modbus_ascii.desc",
            ProtocolClass::PlainText => "protocol.plain_text.desc",
            ProtocolClass::Hdlc => "protocol.hdlc.desc",
            ProtocolClass::InsoleSensor => "protocol.insole_sensor.desc",
            ProtocolClass::Slcan => "protocol.slcan.desc",
            ProtocolClass::Stm32Bootloader => "protocol.stm32_bootloader.desc",
            ProtocolClass::EspImage => "protocol.esp_image.desc",
            ProtocolClass::Nmea => "protocol.nmea.desc",
            ProtocolClass::BinaryRaw => "protocol.binary_raw.desc",
        }
    }

    /// 协议简述（英文）
    pub fn description_en(self) -> &'static str {
        match self {
            ProtocolClass::Unknown => "Unrecognized protocol; capture more data and retry",
            ProtocolClass::ModbusRtu => "Industry-standard Modbus RTU: binary frames + CRC16, common for PLCs and meters",
            ProtocolClass::ModbusAscii => "Modbus ASCII mode: colon-prefixed + LRC, readable but less efficient",
            ProtocolClass::PlainText => "Plain ASCII text protocol, common for debug output, AT commands, etc.",
            ProtocolClass::Hdlc => "HDLC/PPP protocol with 0x7E frame flags for high-level data link control",
            ProtocolClass::InsoleSensor => "Insole/mattress pressure sensor protocol: AA 55 03 99 header, ADC pressure matrix + IMU data",
            ProtocolClass::Slcan => "SLCAN protocol: ASCII-command CAN bus, t/T transmit, r/R receive",
            ProtocolClass::Stm32Bootloader => "STM32 system bootloader protocol: 0x7F sync + 0x79 ACK, for firmware flashing",
            ProtocolClass::EspImage => "ESP32/ESP8266 firmware image starting with 0xE9 magic",
            ProtocolClass::Nmea => "NMEA 0183 GPS sentences starting with $GP/$GN, position/time info",
            ProtocolClass::BinaryRaw => "Binary data stream detected, but no known protocol template matched",
        }
    }

    /// 推荐的波特率（若已知）
    pub fn recommended_baud(self) -> Option<u32> {
        match self {
            ProtocolClass::ModbusRtu | ProtocolClass::ModbusAscii => Some(9600),
            ProtocolClass::InsoleSensor => Some(921600),
            ProtocolClass::Stm32Bootloader => Some(115200),
            ProtocolClass::EspImage => Some(115200),
            ProtocolClass::Nmea => Some(4800),
            ProtocolClass::Slcan => Some(500000),
            _ => None,
        }
    }
}

/// 协议模板：预定义的协议特征参考
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolTemplate {
    /// 协议类型
    pub class: ProtocolClass,
    /// 参考特征向量
    pub features: StreamFeatures,
    /// 强匹配帧头（检测到此帧头时直接判定）
    pub strong_headers: Vec<KnownHeader>,
}

/// KNN 分类器
#[derive(Clone)]
pub struct ProtocolClassifier {
    /// 预置协议模板
    templates: Vec<ProtocolTemplate>,
    /// 用户标注样本（增量学习）
    samples: Vec<(StreamFeatures, ProtocolClass)>,
    /// K 值（最近邻数量）
    k: usize,
}

impl ProtocolClassifier {
    /// 创建分类器，预置所有协议模板
    pub fn with_builtin_templates() -> Self {
        Self {
            templates: builtin_templates(),
            samples: Vec::new(),
            k: 5,
        }
    }

    /// 添加用户标注样本
    pub fn add_sample(&mut self, features: StreamFeatures, label: ProtocolClass) {
        self.samples.push((features, label));
        // 限制样本数量，避免无限增长
        if self.samples.len() > 500 {
            self.samples.remove(0);
        }
    }

    /// 清空用户样本
    pub fn clear_samples(&mut self) {
        self.samples.clear();
    }

    /// 对特征向量进行协议识别
    ///
    /// 返回 (协议类型, 置信度 0.0-1.0)。
    /// 识别策略：帧头强匹配优先，其次 KNN 投票。
    pub fn predict(&self, features: &StreamFeatures) -> (ProtocolClass, f32) {
        // 数据太短，直接返回未知
        if features.data_length < 4 {
            return (ProtocolClass::Unknown, 0.0);
        }

        // 1. 帧头强匹配
        if let Some((class, conf)) = self.strong_header_match(features) {
            return (class, conf);
        }

        // 2. KNN 分类（模板 + 样本）
        self.knn_classify(features)
    }

    /// 帧头强匹配：检测到唯一性帧头时直接判定
    fn strong_header_match(&self, features: &StreamFeatures) -> Option<(ProtocolClass, f32)> {
        for template in &self.templates {
            if !template.strong_headers.is_empty() {
                let matched = template
                    .strong_headers
                    .iter()
                    .filter(|h| features.detected_headers.contains(h))
                    .count();
                if matched > 0 {
                    // 帧头匹配置信度 0.85-0.95，根据数据长度微调
                    let len_bonus = if features.data_length >= 64 {
                        0.10
                    } else if features.data_length >= 32 {
                        0.05
                    } else {
                        0.0
                    };
                    return Some((template.class, 0.85 + len_bonus));
                }
            }
        }
        None
    }

    /// KNN 分类：计算与所有模板和样本的距离，取 K 个最近邻投票
    fn knn_classify(&self, features: &StreamFeatures) -> (ProtocolClass, f32) {
        // 收集所有参考点（模板 + 样本）
        let mut references: Vec<(f32, ProtocolClass)> = Vec::new();

        for template in &self.templates {
            let dist = features.distance(&template.features);
            // 帧头匹配减距离（相当于拉近）
            let header_bonus = features.header_match_count(&template.features) as f32 * 0.1;
            let adjusted_dist = (dist - header_bonus).max(0.01);
            references.push((adjusted_dist, template.class));
        }

        for (sample_features, label) in &self.samples {
            let dist = features.distance(sample_features);
            references.push((dist, *label));
        }

        if references.is_empty() {
            return (ProtocolClass::Unknown, 0.0);
        }

        // 按距离排序，取 K 个最近邻
        references.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let k = self.k.min(references.len());
        let neighbors = &references[..k];

        // 加权投票：距离越近权重越高（1/dist）
        let mut votes: std::collections::HashMap<ProtocolClass, f32> =
            std::collections::HashMap::new();
        for (dist, class) in neighbors {
            let weight = 1.0 / dist;
            *votes.entry(*class).or_insert(0.0) += weight;
        }

        // 找出票数最高的类别
        let (best_class, best_weight) = votes
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(c, w)| (*c, *w))
            .unwrap_or((ProtocolClass::Unknown, 0.0));

        // 总权重
        let total_weight: f32 = votes.values().sum();

        // 置信度 = 最高票数权重 / 总权重 * 距离因子
        let vote_ratio = if total_weight > 0.0 {
            best_weight / total_weight
        } else {
            0.0
        };

        // 距离因子：最近邻距离越小，置信度越高
        let nearest_dist = neighbors.first().map(|(d, _)| *d).unwrap_or(1.0);
        let distance_factor = 1.0 / (1.0 + nearest_dist * 2.0);

        let confidence = (vote_ratio * distance_factor).clamp(0.0, 0.85);

        // 如果置信度过低，返回 Unknown
        if confidence < 0.15 {
            // 但如果数据明显是二进制，返回 BinaryRaw
            if features.shannon_entropy > 5.0 && features.printable_ratio < 0.3 {
                return (ProtocolClass::BinaryRaw, confidence.max(0.3));
            }
            return (ProtocolClass::Unknown, confidence);
        }

        (best_class, confidence)
    }

    /// 获取所有模板（用于 UI 展示知识库）
    pub fn templates(&self) -> &[ProtocolTemplate] {
        &self.templates
    }

    /// 获取用户样本数量
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

/// 构建预置协议模板库
///
/// 每个模板基于协议的已知特性构造参考特征向量。
fn builtin_templates() -> Vec<ProtocolTemplate> {
    vec![
        // Modbus RTU：二进制，中等熵，AA 55 帧头可能存在
        ProtocolTemplate {
            class: ProtocolClass::ModbusRtu,
            features: StreamFeatures {
                byte_histogram_16: modbus_rtu_histogram(),
                shannon_entropy: 4.5,
                printable_ratio: 0.3,
                null_ratio: 0.05,
                high_byte_ratio: 0.4,
                control_ratio: 0.1,
                avg_byte_value: 90.0,
                byte_value_std: 70.0,
                frame_interval_mean_ms: 50.0,
                frame_interval_std_ms: 10.0,
                frame_interval_cv: 0.2,
                detected_headers: vec![KnownHeader::Aa55],
                max_run_length: 4,
                data_length: 64,
            },
            strong_headers: vec![],
        },
        // Modbus ASCII：高可打印比例，冒号开头
        ProtocolTemplate {
            class: ProtocolClass::ModbusAscii,
            features: StreamFeatures {
                byte_histogram_16: modbus_ascii_histogram(),
                shannon_entropy: 3.5,
                printable_ratio: 0.95,
                null_ratio: 0.0,
                high_byte_ratio: 0.0,
                control_ratio: 0.05,
                avg_byte_value: 60.0,
                byte_value_std: 25.0,
                frame_interval_mean_ms: 100.0,
                frame_interval_std_ms: 20.0,
                frame_interval_cv: 0.2,
                detected_headers: vec![],
                max_run_length: 3,
                data_length: 64,
            },
            strong_headers: vec![],
        },
        // 纯 ASCII 文本：极高可打印比例
        ProtocolTemplate {
            class: ProtocolClass::PlainText,
            features: StreamFeatures {
                byte_histogram_16: plain_text_histogram(),
                shannon_entropy: 4.5,
                printable_ratio: 0.98,
                null_ratio: 0.0,
                high_byte_ratio: 0.0,
                control_ratio: 0.02,
                avg_byte_value: 80.0,
                byte_value_std: 35.0,
                frame_interval_mean_ms: 0.0,
                frame_interval_std_ms: 0.0,
                frame_interval_cv: 0.0,
                detected_headers: vec![],
                max_run_length: 5,
                data_length: 64,
            },
            strong_headers: vec![],
        },
        // HDLC：0x7E 标志
        ProtocolTemplate {
            class: ProtocolClass::Hdlc,
            features: StreamFeatures {
                byte_histogram_16: [0.0; 16],
                shannon_entropy: 6.0,
                printable_ratio: 0.4,
                null_ratio: 0.05,
                high_byte_ratio: 0.3,
                control_ratio: 0.1,
                avg_byte_value: 80.0,
                byte_value_std: 75.0,
                frame_interval_mean_ms: 0.0,
                frame_interval_std_ms: 0.0,
                frame_interval_cv: 0.0,
                detected_headers: vec![KnownHeader::Hdlc7e],
                max_run_length: 3,
                data_length: 64,
            },
            strong_headers: vec![KnownHeader::Hdlc7e],
        },
        // 鞋垫传感器：AA 55 03 99 强匹配
        ProtocolTemplate {
            class: ProtocolClass::InsoleSensor,
            features: StreamFeatures {
                byte_histogram_16: [0.0; 16],
                shannon_entropy: 7.0,
                printable_ratio: 0.1,
                null_ratio: 0.05,
                high_byte_ratio: 0.5,
                control_ratio: 0.1,
                avg_byte_value: 120.0,
                byte_value_std: 80.0,
                frame_interval_mean_ms: 10.0,
                frame_interval_std_ms: 1.0,
                frame_interval_cv: 0.1,
                detected_headers: vec![KnownHeader::InsoleAa550399],
                max_run_length: 4,
                data_length: 272,
            },
            strong_headers: vec![KnownHeader::InsoleAa550399],
        },
        // SLCAN：ASCII 命令
        ProtocolTemplate {
            class: ProtocolClass::Slcan,
            features: StreamFeatures {
                byte_histogram_16: plain_text_histogram(),
                shannon_entropy: 3.8,
                printable_ratio: 0.95,
                null_ratio: 0.0,
                high_byte_ratio: 0.0,
                control_ratio: 0.05,
                avg_byte_value: 70.0,
                byte_value_std: 30.0,
                frame_interval_mean_ms: 0.0,
                frame_interval_std_ms: 0.0,
                frame_interval_cv: 0.0,
                detected_headers: vec![KnownHeader::SlcanAscii],
                max_run_length: 4,
                data_length: 32,
            },
            strong_headers: vec![KnownHeader::SlcanAscii],
        },
        // STM32 Bootloader：0x7F 同步
        ProtocolTemplate {
            class: ProtocolClass::Stm32Bootloader,
            features: StreamFeatures {
                byte_histogram_16: [0.0; 16],
                shannon_entropy: 2.0,
                printable_ratio: 0.0,
                null_ratio: 0.0,
                high_byte_ratio: 0.5,
                control_ratio: 0.5,
                avg_byte_value: 120.0,
                byte_value_std: 50.0,
                frame_interval_mean_ms: 0.0,
                frame_interval_std_ms: 0.0,
                frame_interval_cv: 0.0,
                detected_headers: vec![KnownHeader::Stm32Bootloader7f],
                max_run_length: 2,
                data_length: 8,
            },
            strong_headers: vec![KnownHeader::Stm32Bootloader7f],
        },
        // ESP Image：0xE9 魔数
        ProtocolTemplate {
            class: ProtocolClass::EspImage,
            features: StreamFeatures {
                byte_histogram_16: [0.0; 16],
                shannon_entropy: 6.5,
                printable_ratio: 0.1,
                null_ratio: 0.1,
                high_byte_ratio: 0.4,
                control_ratio: 0.1,
                avg_byte_value: 100.0,
                byte_value_std: 78.0,
                frame_interval_mean_ms: 0.0,
                frame_interval_std_ms: 0.0,
                frame_interval_cv: 0.0,
                detected_headers: vec![KnownHeader::EspImageE9],
                max_run_length: 4,
                data_length: 64,
            },
            strong_headers: vec![KnownHeader::EspImageE9],
        },
        // NMEA：$GP/$GN 开头
        ProtocolTemplate {
            class: ProtocolClass::Nmea,
            features: StreamFeatures {
                byte_histogram_16: plain_text_histogram(),
                shannon_entropy: 4.0,
                printable_ratio: 0.95,
                null_ratio: 0.0,
                high_byte_ratio: 0.0,
                control_ratio: 0.05,
                avg_byte_value: 65.0,
                byte_value_std: 28.0,
                frame_interval_mean_ms: 1000.0,
                frame_interval_std_ms: 10.0,
                frame_interval_cv: 0.01,
                detected_headers: vec![KnownHeader::NmeaGp],
                max_run_length: 5,
                data_length: 80,
            },
            strong_headers: vec![KnownHeader::NmeaGp],
        },
        // 未知二进制：高熵，低可打印
        ProtocolTemplate {
            class: ProtocolClass::BinaryRaw,
            features: StreamFeatures {
                byte_histogram_16: uniform_histogram(),
                shannon_entropy: 7.5,
                printable_ratio: 0.15,
                null_ratio: 0.05,
                high_byte_ratio: 0.5,
                control_ratio: 0.15,
                avg_byte_value: 127.0,
                byte_value_std: 80.0,
                frame_interval_mean_ms: 0.0,
                frame_interval_std_ms: 0.0,
                frame_interval_cv: 0.0,
                detected_headers: vec![],
                max_run_length: 3,
                data_length: 64,
            },
            strong_headers: vec![],
        },
    ]
}

// ---- 参考直方图构造 ----

fn modbus_rtu_histogram() -> [f32; 16] {
    let mut h = [0.0f32; 16];
    h[0] = 0.15;
    h[1] = 0.08;
    h[3] = 0.12;
    h[5] = 0.10;
    h[7] = 0.08;
    h[8] = 0.07;
    h[10] = 0.06;
    h[12] = 0.08;
    h[14] = 0.10;
    h[15] = 0.08;
    h
}

fn modbus_ascii_histogram() -> [f32; 16] {
    let mut h = [0.0f32; 16];
    // Modbus ASCII 集中在 0x30-0x46 (0-9, A-F) 和 0x3A (冒号), 0x0D 0x0A
    h[3] = 0.65;
    h[4] = 0.20;
    h[0] = 0.10;
    h[2] = 0.05;
    h
}

fn plain_text_histogram() -> [f32; 16] {
    let mut h = [0.0f32; 16];
    // ASCII 文本集中在 0x20-0x7E
    h[2] = 0.05;
    h[3] = 0.20;
    h[4] = 0.25;
    h[5] = 0.15;
    h[6] = 0.15;
    h[7] = 0.15;
    h[0] = 0.05;
    h
}

fn uniform_histogram() -> [f32; 16] {
    [0.0625; 16]
}
