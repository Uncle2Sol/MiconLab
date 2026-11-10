//! Ollama 本地模型客户端（feature `ollama`）
//!
//! 使用 Ollama 原生 API（而非 `/v1` OpenAI 兼容层），原因：
//! - `/api/chat` 支持 `format: <json-schema>`，可强制模型输出符合 schema 的 JSON，
//!   识别结果可靠机读；兼容层不支持 schema 约束
//! - 原生 API 支持 `keep_alive` 控制模型驻留内存，减少识别延迟抖动
//! - `/api/tags` 同时完成健康检查与模型发现
//!
//! 设计原则：
//! - 纯本地推理（默认 http://localhost:11434），无任何外部网络请求
//! - Ollama 不可用 → 返回 `OllamaError::NotRunning`，调用方静默降级规则引擎，绝不打扰用户
//! - 识别任务低温（temperature=0.1）求确定性，单次输出上限 512 token

use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::time::Duration;

/// 默认 Ollama 服务地址
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";
/// 默认识别模型（中文理解好，JSON schema 遵循度高）
pub const DEFAULT_MODEL: &str = "qwen2.5:7b-instruct";
/// 轻量备选模型
pub const LIGHT_MODEL: &str = "qwen2.5:3b-instruct";

/// Ollama 客户端配置
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// 服务地址（默认 http://localhost:11434）
    pub base_url: String,
    /// 识别用模型名
    pub model: String,
    /// 单次识别超时（秒），本地推理可能因冷启动较慢，默认 60s
    pub timeout_secs: u64,
    /// 模型驻留内存时长（如 "10m"），减少重复识别冷启动
    pub keep_alive: String,
    /// 最大输出 token 数（结构化结果很短，512 足够）
    pub max_tokens: u32,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            timeout_secs: 60,
            keep_alive: "10m".to_string(),
            max_tokens: 512,
        }
    }
}

/// Ollama 客户端错误
#[derive(Debug)]
pub enum OllamaError {
    /// Ollama 服务未运行（连接失败）——调用方应静默降级
    NotRunning,
    /// 配置的模型未拉取
    ModelMissing(String),
    /// 网络/HTTP 错误
    Network(String),
    /// API 返回错误
    ApiError(String),
    /// 输出解析失败（重试后仍失败）
    ParseError(String),
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OllamaError::NotRunning => write!(f, "Ollama service not running"),
            OllamaError::ModelMissing(m) => write!(f, "Model not pulled: {}", m),
            OllamaError::Network(e) => write!(f, "Network error: {}", e),
            OllamaError::ApiError(e) => write!(f, "API error: {}", e),
            OllamaError::ParseError(e) => write!(f, "Output parse failed: {}", e),
        }
    }
}

impl std::error::Error for OllamaError {}

/// Ollama 服务状态快照（供 GUI 状态指示：绿=就绪/灰=未运行/黄=缺模型）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaStatus {
    /// 服务未运行
    NotRunning,
    /// 服务运行中，但识别模型未拉取（内含已安装模型列表）
    ModelMissing(Vec<String>),
    /// 就绪（服务运行且识别模型可用）
    Ready(Vec<String>),
}

impl OllamaStatus {
    /// 是否可立即发起识别
    pub fn is_ready(&self) -> bool {
        matches!(self, OllamaStatus::Ready(_))
    }
}

/// /api/tags 响应中的模型条目
#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModel {
    /// 模型名（如 "qwen2.5:7b-instruct"）
    pub name: String,
    /// 模型文件大小（字节）
    #[serde(default)]
    pub size: u64,
}

/// Ollama 原生 API 客户端
///
/// 持有独立 current_thread 运行时，所有阻塞方法可在后台线程直接调用，
/// 与 egui 单线程帧驱动架构兼容（GUI 侧在后台线程调用 blocking 版本）。
pub struct OllamaClient {
    config: OllamaConfig,
    runtime: tokio::runtime::Runtime,
}

impl OllamaClient {
    /// 创建客户端（仅需配置，不要求服务在线）
    pub fn new(config: OllamaConfig) -> Result<Self, OllamaError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| OllamaError::Network(format!("Failed to create runtime: {}", e)))?;
        Ok(Self { config, runtime })
    }

    /// 当前配置
    pub fn config(&self) -> &OllamaConfig {
        &self.config
    }

    /// 更换识别模型
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.config.model = model.into();
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/api/{}", self.config.base_url.trim_end_matches('/'), path)
    }

    fn http_client(&self, timeout_secs: u64) -> Result<reqwest::Client, OllamaError> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| OllamaError::Network(e.to_string()))
    }

    /// 将 reqwest 错误归类：连接失败 = 服务未运行
    fn classify_send_error(e: reqwest::Error) -> OllamaError {
        if e.is_connect() {
            OllamaError::NotRunning
        } else {
            OllamaError::Network(e.to_string())
        }
    }

    // ============ 健康检查与模型发现 ============

    /// 健康检查：返回可用模型列表。连接失败返回 `NotRunning`（静默降级信号）
    pub async fn list_models(&self) -> Result<Vec<OllamaModel>, OllamaError> {
        let client = self.http_client(10)?;
        let resp = client
            .get(self.endpoint("tags"))
            .send()
            .await
            .map_err(Self::classify_send_error)?;
        if !resp.status().is_success() {
            return Err(OllamaError::ApiError(format!("HTTP {}", resp.status())));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| OllamaError::ParseError(e.to_string()))?;
        let models = json
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m.get("name")?.as_str()?.to_string();
                        let size = m.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                        Some(OllamaModel { name, size })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }

    /// 阻塞版健康检查（后台线程调用）
    pub fn list_models_blocking(&self) -> Result<Vec<OllamaModel>, OllamaError> {
        self.runtime.block_on(self.list_models())
    }

    /// 一次性状态检查：服务是否在跑 + 识别模型是否就绪
    pub fn status_blocking(&self) -> OllamaStatus {
        match self.list_models_blocking() {
            Err(_) => OllamaStatus::NotRunning,
            Ok(models) => {
                let names: Vec<String> = models.iter().map(|m| m.name.clone()).collect();
                // Ollama 模型名可能带 ":latest" 后缀，做前缀归一匹配
                let want = self.config.model.trim_end_matches(":latest");
                let found = names
                    .iter()
                    .any(|n| n.trim_end_matches(":latest") == want);
                if found {
                    OllamaStatus::Ready(names)
                } else {
                    OllamaStatus::ModelMissing(names)
                }
            }
        }
    }

    // ============ 结构化识别 ============

    /// 结构化识别：`format` 传 JSON Schema 强约束输出。
    ///
    /// 解析失败自动重试 1 次（user 消息附上次的解析错误，让模型自我纠正）。
    /// 模型未拉取返回 `ModelMissing`（Ollama 对缺模型返回 404）。
    pub async fn identify<T: DeserializeOwned>(
        &self,
        system: &str,
        evidence: &str,
        schema: &serde_json::Value,
    ) -> Result<T, OllamaError> {
        let mut last_err = String::new();
        for attempt in 0..2 {
            let user = if attempt == 0 {
                evidence.to_string()
            } else {
                format!(
                    "{}\n\n[System feedback] Previous output could not be parsed: {}.\nPlease output JSON strictly conforming to the given JSON Schema; do not output anything else.",
                    evidence, last_err
                )
            };
            match self.chat_once(system, &user, schema).await {
                Ok(content) => match parse_json_lenient::<T>(&content) {
                    Ok(v) => return Ok(v),
                    Err(e) => {
                        last_err = format!("{}; raw output: {}", e, truncate(&content, 200));
                        log::warn!("ollama identify parse failed on attempt {}: {}", attempt + 1, last_err);
                    }
                },
                Err(e) => return Err(e),
            }
        }
        Err(OllamaError::ParseError(last_err))
    }

    /// 阻塞版结构化识别（后台线程调用）
    pub fn identify_blocking<T: DeserializeOwned>(
        &self,
        system: &str,
        evidence: &str,
        schema: &serde_json::Value,
    ) -> Result<T, OllamaError> {
        self.runtime.block_on(self.identify(system, evidence, schema))
    }

    /// 单次 /api/chat 调用，返回 message.content 原文
    async fn chat_once(
        &self,
        system: &str,
        user: &str,
        schema: &serde_json::Value,
    ) -> Result<String, OllamaError> {
        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": false,
            "format": schema,
            "keep_alive": self.config.keep_alive,
            "options": {
                "temperature": 0.1,
                "num_predict": self.config.max_tokens,
            },
        });
        let client = self.http_client(self.config.timeout_secs)?;
        let resp = client
            .post(self.endpoint("chat"))
            .json(&body)
            .send()
            .await
            .map_err(Self::classify_send_error)?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| OllamaError::Network(format!("Failed to read response: {}", e)))?;
        if status.as_u16() == 404 {
            return Err(OllamaError::ModelMissing(self.config.model.clone()));
        }
        if !status.is_success() {
            return Err(OllamaError::ApiError(format!("HTTP {}: {}", status, text)));
        }
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| OllamaError::ParseError(format!("Response JSON parse failed: {}", e)))?;
        json.get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| OllamaError::ParseError("Response missing message.content".to_string()))
    }

    // ============ 模型拉取 ============

    /// 拉取识别模型（流式进度）。
    ///
    /// `progress(fraction, status_text)`：fraction ∈ [0.0, 1.0]，
    /// status_text 为 Ollama 的状态行（如 "pulling manifest" / "downloading ..."）。
    /// 大文件下载无总超时限制，仅有连接超时。
    pub async fn pull_model(
        &self,
        mut progress: impl FnMut(f32, String) + Send,
    ) -> Result<(), OllamaError> {
        use futures_util::StreamExt;

        let body = serde_json::json!({
            "name": self.config.model,
            "stream": true,
        });
        // 拉取可能达数 GB，不设总超时
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| OllamaError::Network(e.to_string()))?;
        let resp = client
            .post(self.endpoint("pull"))
            .json(&body)
            .send()
            .await
            .map_err(Self::classify_send_error)?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OllamaError::ApiError(format!("HTTP: {}", text)));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| OllamaError::Network(format!("Download interrupted: {}", e)))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // NDJSON：逐行解析进度
            while let Some(pos) = buf.find('\n') {
                let line: String = buf.drain(..=pos).collect();
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                    return Err(OllamaError::ApiError(err.to_string()));
                }
                let status = v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let fraction = match (v.get("completed"), v.get("total")) {
                    (Some(c), Some(t)) => {
                        let (c, t) = (c.as_u64().unwrap_or(0), t.as_u64().unwrap_or(0));
                        if t > 0 {
                            (c as f32 / t as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        }
                    }
                    _ if status == "success" => 1.0,
                    _ => 0.0,
                };
                progress(fraction, status.clone());
                if status == "success" {
                    return Ok(());
                }
            }
        }
        // 流结束但未收到 success：视为完成但告警
        log::warn!("ollama pull stream ended without success status");
        Ok(())
    }

    /// 阻塞版模型拉取（后台线程调用）
    pub fn pull_model_blocking(
        &self,
        progress: impl FnMut(f32, String) + Send,
    ) -> Result<(), OllamaError> {
        self.runtime.block_on(self.pull_model(progress))
    }
}

/// 宽松 JSON 解析：容忍模型在 JSON 前后包裹 ```json 代码块或多余文本
fn parse_json_lenient<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let trimmed = text.trim();
    // 直接解析
    if let Ok(v) = serde_json::from_str::<T>(trimmed) {
        return Ok(v);
    }
    // 提取第一个 { 到最后一个 } 之间的内容再解析
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            return serde_json::from_str::<T>(&trimmed[s..=e])
                .map_err(|e| format!("JSON parse failed: {}", e));
        }
    }
    Err("No JSON object found in output".to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// 启动一次性 mock HTTP 服务器：接收 1 个请求，回复固定响应。
    /// 返回 (base_url, 接收到的请求文本 channel)。
    fn mock_server_once(status: &str, body: &str) -> (String, mpsc::Receiver<String>) {
        mock_server_n(1, status, body).0
    }

    /// 支持连续 n 个请求的 mock 服务器（重试场景用）。
    /// 返回 ((base_url, 请求 channel), 第 2..n 次响应可后续推送——此处简化为每次同响应)。
    fn mock_server_n(
        n: usize,
        status: &str,
        body: &str,
    ) -> ((String, mpsc::Receiver<String>), std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let status = status.to_string();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            for _ in 0..n {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let req = read_request(&mut stream);
                let _ = tx.send(req);
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });
        ((format!("http://127.0.0.1:{}", port), rx), handle)
    }

    /// 读取完整 HTTP 请求（按 Content-Length 读 body）
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let mut content_length = 0usize;
        let mut header_done = false;
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if !header_done {
                if let Some(pos) = find_subslice(&data, b"\r\n\r\n") {
                    header_done = true;
                    let headers = String::from_utf8_lossy(&data[..pos]).to_lowercase();
                    for line in headers.lines() {
                        if let Some(v) = line.strip_prefix("content-length:") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
            if header_done {
                let pos = find_subslice(&data, b"\r\n\r\n").unwrap();
                if data.len() >= pos + 4 + content_length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|w| w == needle)
    }

    fn test_client(base_url: &str) -> OllamaClient {
        OllamaClient::new(OllamaConfig {
            base_url: base_url.to_string(),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn test_list_models_ok() {
        let body = r#"{"models":[{"name":"qwen2.5:7b-instruct","size":4700000000},{"name":"llama3:latest","size":100}]}"#;
        let (url, rx) = mock_server_once("200 OK", body);
        let client = test_client(&url);
        let models = client.list_models_blocking().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "qwen2.5:7b-instruct");
        assert_eq!(models[0].size, 4700000000);
        let req = rx.recv().unwrap();
        assert!(req.starts_with("GET /api/tags"));
    }

    #[test]
    fn test_status_ready_and_missing() {
        let body = r#"{"models":[{"name":"qwen2.5:7b-instruct","size":1}]}"#;
        let ((url, _rx), _h) = mock_server_n(2, "200 OK", body);
        let client = test_client(&url);
        assert!(matches!(client.status_blocking(), OllamaStatus::Ready(_)));

        // 换一个未安装的模型名 → ModelMissing
        let mut client2 = test_client(&url);
        client2.set_model("qwen2.5:3b-instruct");
        match client2.status_blocking() {
            OllamaStatus::ModelMissing(names) => {
                assert!(names.contains(&"qwen2.5:7b-instruct".to_string()))
            }
            other => panic!("期望 ModelMissing，实际 {:?}", other),
        }
    }

    #[test]
    fn test_status_not_running() {
        // 绑定后立即释放，得到几乎必然拒连的端口
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client = test_client(&format!("http://127.0.0.1:{}", port));
        assert_eq!(client.status_blocking(), OllamaStatus::NotRunning);
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct DummyResult {
        label: String,
        confidence: f32,
    }

    fn dummy_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "label": {"type": "string"},
                "confidence": {"type": "number"}
            },
            "required": ["label", "confidence"]
        })
    }

    #[test]
    fn test_identify_ok_and_request_shape() {
        let content = r#"{"label":"modbus-rtu","confidence":0.9}"#;
        let body = serde_json::json!({
            "message": {"role": "assistant", "content": content},
            "done": true
        })
        .to_string();
        let (url, rx) = mock_server_once("200 OK", &body);
        let client = test_client(&url);
        let r: DummyResult = client
            .identify_blocking("系统提示", "证据包文本", &dummy_schema())
            .unwrap();
        assert_eq!(r.label, "modbus-rtu");
        assert!((r.confidence - 0.9).abs() < 1e-6);

        // 校验请求体：原生 chat 端点 + format schema + 低温 + stream=false
        let req = rx.recv().unwrap();
        assert!(req.starts_with("POST /api/chat"));
        let body_start = req.find("\r\n\r\n").unwrap() + 4;
        let req_json: serde_json::Value =
            serde_json::from_str(&req[body_start..]).unwrap();
        assert_eq!(req_json["model"], "qwen2.5:7b-instruct");
        assert_eq!(req_json["stream"], false);
        assert_eq!(req_json["format"]["type"], "object");
        assert_eq!(req_json["options"]["temperature"], 0.1);
        assert_eq!(req_json["keep_alive"], "10m");
        assert_eq!(req_json["messages"][0]["role"], "system");
        assert_eq!(req_json["messages"][1]["content"], "证据包文本");
    }

    #[test]
    fn test_identify_lenient_parse_with_code_fence() {
        // 模型把 JSON 包在 ```json ... ``` 里 → 宽松解析应成功
        let content = "```json\n{\"label\":\"esp32\",\"confidence\":0.8}\n```";
        let body = serde_json::json!({"message": {"content": content}}).to_string();
        let (url, _rx) = mock_server_once("200 OK", &body);
        let client = test_client(&url);
        let r: DummyResult = client
            .identify_blocking("s", "e", &dummy_schema())
            .unwrap();
        assert_eq!(r.label, "esp32");
    }

    #[test]
    fn test_identify_retry_on_parse_failure() {
        // 第 1 次返回非 JSON，第 2 次返回合法 JSON → 重试成功
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let bodies = [
                serde_json::json!({"message": {"content": "这不是JSON"}}).to_string(),
                serde_json::json!({"message": {"content": "{\"label\":\"ch340\",\"confidence\":0.7}"}})
                    .to_string(),
            ];
            for body in bodies {
                let (mut stream, _) = listener.accept().unwrap();
                let req = read_request(&mut stream);
                tx.send(req).unwrap();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });
        let client = test_client(&format!("http://127.0.0.1:{}", port));
        let r: DummyResult = client
            .identify_blocking("s", "证据", &dummy_schema())
            .unwrap();
        assert_eq!(r.label, "ch340");
        // 第 2 次请求应包含上次的解析失败反馈
        let _first = rx.recv().unwrap();
        let second = rx.recv().unwrap();
        assert!(second.contains("Previous output could not be parsed"));
    }

    #[test]
    fn test_identify_model_missing_404() {
        let (url, _rx) = mock_server_once("404 Not Found", r#"{"error":"model 'x' not found"}"#);
        let client = test_client(&url);
        let r = client.identify_blocking::<DummyResult>("s", "e", &dummy_schema());
        assert!(matches!(r, Err(OllamaError::ModelMissing(_))));
    }

    #[test]
    fn test_pull_model_progress() {
        let lines = [
            r#"{"status":"pulling manifest"}"#,
            r#"{"status":"downloading","completed":50,"total":100}"#,
            r#"{"status":"downloading","completed":100,"total":100}"#,
            r#"{"status":"success"}"#,
        ];
        let body = lines.join("\n") + "\n";
        let (url, rx) = mock_server_once("200 OK", &body);
        let client = test_client(&url);
        let mut progress_log: Vec<(f32, String)> = Vec::new();
        client
            .pull_model_blocking(|f, s| progress_log.push((f, s)))
            .unwrap();
        let req = rx.recv().unwrap();
        assert!(req.starts_with("POST /api/pull"));
        assert!(progress_log.iter().any(|(f, _)| (*f - 0.5).abs() < 1e-6));
        assert!(progress_log.iter().any(|(f, s)| (*f - 1.0).abs() < 1e-6 && s == "success"));
    }

    #[test]
    fn test_pull_model_error_line() {
        let body = "{\"status\":\"pulling manifest\"}\n{\"error\":\"file not found\"}\n";
        let (url, _rx) = mock_server_once("200 OK", body);
        let client = test_client(&url);
        let r = client.pull_model_blocking(|_, _| {});
        match r {
            Err(OllamaError::ApiError(e)) => assert!(e.contains("file not found")),
            other => panic!("期望 ApiError，实际 {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn test_truncate_multibyte_safe() {
        let s = "你好世界abcdef";
        let t = truncate(s, 7); // 7 落在"世"的 UTF-8 中间
        assert!(t.ends_with('…'));
        assert!(t.len() <= 7 + '…'.len_utf8());
    }
}
