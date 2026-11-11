<div align="center">

<img src="assets/brand/logo/miconlab-lockup-horizontal-primary-on-dark.svg" alt="MiconLab" height="56">

**mcu serial workbench · 单片机实验室**

**Cross-platform serial debugging, protocol analysis, data recording and quality-check toolkit for embedded developers.**

[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-PolyForm%20Noncommercial%201.0.0-blue)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)]()

**[English](#english)** | **[中文](#中文)**

</div>

---

> **License in one line:** free for personal, study, teaching and non-commercial research; commercial use requires a paid license. See [License](#license).
>
> This repository is **source-available, not open source**.

## English

### What it is

miconlab is a Rust toolkit for talking to serial devices: open a port, exchange bytes, parse frames, detect baud rates, verify checksums, record a session, replay it, and check the captured data for quality problems. It runs on Windows, macOS and Linux, and ships as a CLI plus an MCP server that AI assistants can drive.

### What you can build with it

The framework is deliberately split into reusable layers, so it can serve as the foundation of a **host-side application (上位机)** for your own hardware:

| Layer | What it gives you | Where |
|-------|-------------------|-------|
| Device access abstraction | Port discovery, open/close, baud/data/parity/flow-control, buffered read/write, clear-buffer primitives | `miconlab-core` |
| Protocol analysis | Modbus RTU/TCP, SLCAN (CAN), I2C/SPI register helpers, custom frame parsers, frame extraction | `miconlab-core::protocol` |
| Baud detection | Auto-detect baud rate, including CH340-specific compensated rates | `miconlab-core` |
| Data recording & replay | Timestamped script recorder/replayer, CSV data logger | `miconlab-core` |
| Quality checks | CRC/checksum families, LRC, frame integrity, crosstalk (TX echo) detection, byte statistics | `miconlab-core` |
| Automation surface | CLI commands and a JSON-RPC MCP server over stdio | `miconlab-cli`, `miconlab-mcp` |
| Extensibility | C-ABI plugin API with a working example plugin | `miconlab-plugin-api` |

**Commercial scope.** The public repository includes the general desktop GUI (terminal, protocol analysis, AI assistant and the other general panels). The three dedicated device workspaces, device-specific communication implementations, firmware and device-side tooling are a commercial product: the GUI keeps a **Device Host** entry that shows “This feature is available in the Commercial Edition only.” These parts are not covered by the PolyForm Noncommercial license. See [Commercial use](#commercial-use).

### Core capabilities

- **Terminal-level serial I/O** — text or HEX, configurable baud/data/stop/parity/flow control.
- **Protocol analysis** — Modbus RTU and Modbus TCP parsing, SLCAN/CAN frames, I2C and SPI register operations, custom frame definitions.
- **Frame extraction** — time-gap based (t1.5 / t3.5) and delimiter/strategy based splitting.
- **Baud detection** — probe common rates and fine-tune, with compensation for CH340 bridges.
- **Integrity checks** — CRC-16 (Modbus, CCITT, XMODEM), CRC-32/CRC-32C, checksum8/16, LRC.
- **Crosstalk detection** — recognize TX echo caused by certain USB-serial bridges.
- **Recording & replay** — record a session with timing, replay it against a device.
- **File transfer** — XMODEM / YMODEM / ZMODEM.
- **Data logging** — timestamped CSV output.
- **CLI + MCP** — scriptable from a shell, or callable by an AI assistant through JSON-RPC.
- **Plugins** — load native plugins through a stable C ABI.

### Platform support

| Platform | Architecture | Build | Verified on real hardware |
|----------|--------------|-------|---------------------------|
| macOS | Apple Silicon (aarch64), Intel (x86_64) | Implemented | macOS verified |
| Windows | x86_64 | Implemented (cross-built) | Not yet verified on real hardware |
| Linux | x86_64 | Implemented | Not yet verified on real hardware |
| Linux | aarch64 | Implemented (cross-built) | Not yet verified on real hardware |

"Implemented" means the code compiles and the test suite passes for that target. "Not yet verified on real hardware" means the serial stack has not been exercised against physical adapters on that platform.

### Quick start

#### Build

```bash
git clone <this-repository>
cd miconlab
cargo build --release
```

Binaries:

| Binary | Purpose |
|--------|---------|
| `target/release/miconlab-cli` | CLI (`list`, `connect`, `send`, `monitor`, `record`, `replay`) |
| `target/release/miconlab-mcp` | MCP server (JSON-RPC over stdio) |
| `target/release/miconlab-launcher` | Small launcher that starts the CLI/MCP entry points |

Platform notes:

- **Windows**: use the MSVC toolchain (`x86_64-pc-windows-msvc`); no extra system libraries are required.
- **macOS**: Xcode command line tools are enough; `cargo build --release` produces a plain binary.
- **Linux**: install a C toolchain and `pkg-config`; `libudev` is recommended for hot-plug port enumeration.

#### CLI

```bash
# List ports (text or JSON)
miconlab-cli list
miconlab-cli list --format json

# Send and monitor
miconlab-cli send /dev/ttyUSB0 "AT+RST" -b 115200
miconlab-cli monitor /dev/ttyUSB0 -b 115200 -t -x

# Record a session, then replay it
miconlab-cli record /dev/ttyUSB0 -o session.txt
miconlab-cli replay /dev/ttyUSB0 session.txt
```

On Windows use `COM3`-style port names; on Linux `/dev/ttyUSB0` or `/dev/ttyACM0`; on macOS `/dev/cu.usbserial-*`.

#### MCP server

`miconlab-mcp` speaks JSON-RPC 2.0 over stdio (`Content-Length` framing is not used; each request/response is newline-delimited JSON as implemented by the server). Point your MCP client at the binary:

```json
{
  "mcpServers": {
    "miconlab": {
      "command": "/absolute/path/to/miconlab-mcp"
    }
  }
}
```

Available tools:

| Tool | Description |
|------|-------------|
| `list_ports` | List serial ports |
| `connect` | Open a port (baud, data bits, stop bits, parity, flow control) |
| `disconnect` | Close the current port |
| `send` | Send text or hex data |
| `read` | Read buffered RX data (non-blocking) |
| `send_command` | Send and read the response |
| `modbus_read` | Read Modbus holding registers |
| `modbus_write` | Write a Modbus holding register |
| `plc_read` | Read a preset PLC register block |
| `plc_write` | Write a PLC register |

### Files and paths

miconlab does not create a hidden global configuration directory. Everything is explicit:

| Data | Location |
|------|----------|
| Recorded session script | Path passed to `record -o` (default `script.txt` in the current directory) |
| Replay input | Path passed to `replay` |
| Monitor log | Path passed to `monitor -l` |
| CSV data log | Path chosen by the embedding application |
| Ports | Enumerated at runtime by the OS; nothing is cached on disk |

If you embed `miconlab-core` in your own host application, you decide where configuration and logs live.

### Testing

```bash
cargo test --workspace
```

Tests cover checksums, frame parsing, baud detection, protocol codecs, recorder/replayer and the plugin ABI. Tests that require physical hardware are marked ignored and are expected to be run manually.

### FAQ

**Does this repository contain the GUI or the device host application?**
Yes for the general GUI: it contains the reusable core, CLI, MCP server, plugin API, example plugin and the general desktop GUI. The three dedicated device workspaces are a commercial product; the GUI only shows a **Device Host** placeholder for them.

**Can I use it at work?**
Not without a commercial license. Personal, academic and non-commercial use is free. See [COMMERCIAL.md](COMMERCIAL.md).

**Can I use it with my own device?**
Yes. The generic protocol and recording layers are device-agnostic; you add your own framing/parsing on top. Device-specific implementations are not included.

**Why are some platform targets marked "not verified on real hardware"?**
The code compiles and tests pass, but we have not yet run them against physical serial adapters on that platform. Treat them as build-verified, not field-verified.

**Where do I report a bug?**
<!-- TODO: issue tracker / contact -->

### Commercial use

Commercial use — including internal company use, paid products or services, and building a commercial host application on top of this code — requires a written commercial license.

Commercial engagements can cover: commercial licensing, host-application development, device protocol integration, private deployment and support.

The public repository includes the **AI Assistant** panel and general protocol analysis. The commercial edition additionally covers **device-specific AI/agent-assisted hardware bring-up**: an agent can drive a particular device, send its proprietary frames, wait for specific responses, judge pass/fail automatically, and run recorded sessions as regression tests. Those device-specific workflows, configurations and prompt templates are part of the commercial product and are not published here.

**Contact:** <!-- TODO: 联系方式 -->

See [COMMERCIAL.md](COMMERCIAL.md) for scope and terms.

### License

miconlab is **source-available** under the [PolyForm Noncommercial License 1.0.0](LICENSE), which is **not** an open-source license.

| Use | Terms |
|-----|-------|
| Personal, study, teaching, non-commercial research | Free of charge |
| Charitable, educational, public research, public safety/health, environmental, government use | Free of charge |
| Commercial use (companies, paid products or services) | Requires a commercial license |
| Dedicated device workspaces, device-specific communication implementations, firmware | Not in this repository; commercial product (GUI shows a placeholder entry) |

Third-party components keep their own licenses — see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). In particular, the bundled Noto Sans SC subset is distributed under the SIL Open Font License 1.1 and is **not** relicensed by this project.

> This is the project's licensing statement, not legal advice. Have counsel review it before any public release.

---

## 中文

### 这是什么

miconlab 是一套用来和串口设备打交道的 Rust 工具集：打开端口、收发字节、解析帧、检测波特率、校验校验和、记录会话、回放会话，并对采集到的数据做质量检查。它可在 Windows、macOS、Linux 上运行，提供 CLI 与可被 AI 助手驱动的 MCP 服务器。

### 可以用它做什么

框架刻意拆成可复用的分层，因此可以直接当作自研硬件**上位机**的底座：

| 层次 | 提供什么 | 位置 |
|------|----------|------|
| 设备访问抽象 | 端口发现、打开/关闭、波特率/数据位/校验位/流控、带缓冲的读写、清空缓冲等原语 | `miconlab-core` |
| 协议分析 | Modbus RTU/TCP、SLCAN（CAN）、I2C/SPI 寄存器读写、自定义帧解析、帧提取 | `miconlab-core::protocol` |
| 波特率检测 | 自动探测波特率，含 CH340 专用补偿档位 | `miconlab-core` |
| 数据记录与回放 | 带时间戳的脚本录制/回放、CSV 数据记录 | `miconlab-core` |
| 质量检查 | CRC/校验和系列、LRC、帧完整性、串扰（TX 回显）检测、字节统计 | `miconlab-core` |
| 自动化接口 | CLI 命令，以及基于 stdio 的 JSON-RPC MCP 服务器 | `miconlab-cli`、`miconlab-mcp` |
| 可扩展性 | C ABI 插件接口，附带可运行的示例插件 | `miconlab-plugin-api` |

**商业范围。** 公开仓库包含通用桌面 GUI（终端、协议分析、AI 助手以及其他通用面板）。三套专用设备工作区、设备专用的通讯实现、固件与设备侧工具属于商业产品：GUI 中保留 **设备上位机（Device Host）** 入口，并提示“该功能仅在商业版提供”。这些内容不在 PolyForm Noncommercial 许可范围内。见[商业使用](#商业使用)。

### 核心能力

- **终端级串口收发** —— 文本或 HEX，可配置波特率/数据位/停止位/校验位/流控。
- **协议分析** —— Modbus RTU 与 Modbus TCP 解析、SLCAN/CAN 帧、I2C 与 SPI 寄存器操作、自定义帧定义。
- **帧提取** —— 基于时间间隔（t1.5 / t3.5）的切分，以及基于分隔符/策略的切分。
- **波特率检测** —— 探测常用档位并微调，对 CH340 桥接芯片做补偿。
- **完整性校验** —— CRC-16（Modbus、CCITT、XMODEM）、CRC-32/CRC-32C、checksum8/16、LRC。
- **串扰检测** —— 识别某些 USB 转串口芯片造成的 TX 回显。
- **录制与回放** —— 带时序录制会话，再对设备回放。
- **文件传输** —— XMODEM / YMODEM / ZMODEM。
- **数据记录** —— 带时间戳的 CSV 输出。
- **CLI + MCP** —— 可在 Shell 里脚本化，也可由 AI 助手通过 JSON-RPC 调用。
- **插件** —— 通过稳定的 C ABI 加载原生插件。

### 平台支持

| 平台 | 架构 | 构建 | 真机验证 |
|------|------|------|----------|
| macOS | Apple Silicon (aarch64)、Intel (x86_64) | 已实现 | macOS 已验证 |
| Windows | x86_64 | 已实现（交叉编译） | 尚未在真机验证 |
| Linux | x86_64 | 已实现 | 尚未在真机验证 |
| Linux | aarch64 | 已实现（交叉编译） | 尚未在真机验证 |

“已实现”指该目标平台能编译通过且测试套件全部通过。“尚未在真机验证”指串口栈还没有在该平台的实体适配器上跑过。

### 快速开始

#### 构建

```bash
git clone <this-repository>
cd miconlab
cargo build --release
```

产物：

| 可执行文件 | 用途 |
|------------|------|
| `target/release/miconlab-cli` | CLI（`list`、`connect`、`send`、`monitor`、`record`、`replay`） |
| `target/release/miconlab-mcp` | MCP 服务器（基于 stdio 的 JSON-RPC） |
| `target/release/miconlab-launcher` | 启动 CLI/MCP 入口的小型启动器 |

平台备注：

- **Windows**：使用 MSVC 工具链（`x86_64-pc-windows-msvc`），无需额外的系统库。
- **macOS**：有 Xcode 命令行工具即可，`cargo build --release` 直接产出可执行文件。
- **Linux**：安装 C 工具链与 `pkg-config`；热插拔端口枚举建议安装 `libudev`。

#### CLI

```bash
# 列出端口（文本或 JSON）
miconlab-cli list
miconlab-cli list --format json

# 发送与监听
miconlab-cli send /dev/ttyUSB0 "AT+RST" -b 115200
miconlab-cli monitor /dev/ttyUSB0 -b 115200 -t -x

# 录制会话，再回放
miconlab-cli record /dev/ttyUSB0 -o session.txt
miconlab-cli replay /dev/ttyUSB0 session.txt
```

Windows 下使用 `COM3` 这类端口名；Linux 下是 `/dev/ttyUSB0` 或 `/dev/ttyACM0`；macOS 下是 `/dev/cu.usbserial-*`。

#### MCP 服务器

`miconlab-mcp` 通过 stdio 传输 JSON-RPC 2.0（不使用 `Content-Length` 分帧；该服务器的每个请求/响应都是一行 JSON）。把 MCP 客户端指向这个可执行文件即可：

```json
{
  "mcpServers": {
    "miconlab": {
      "command": "/absolute/path/to/miconlab-mcp"
    }
  }
}
```

可用工具：

| 工具 | 说明 |
|------|------|
| `list_ports` | 列出串口 |
| `connect` | 打开端口（波特率、数据位、停止位、校验位、流控） |
| `disconnect` | 关闭当前端口 |
| `send` | 发送文本或十六进制数据 |
| `read` | 读取 RX 缓冲区（非阻塞） |
| `send_command` | 发送并读取响应 |
| `modbus_read` | 读取 Modbus 保持寄存器 |
| `modbus_write` | 写入 Modbus 保持寄存器 |
| `plc_read` | 读取预设的 PLC 寄存器块 |
| `plc_write` | 写入 PLC 寄存器 |

### 文件与路径

miconlab 不会创建隐藏的全局配置目录，所有位置都显式指定：

| 数据 | 位置 |
|------|------|
| 录制的会话脚本 | `record -o` 传入的路径（默认当前目录下的 `script.txt`） |
| 回放输入 | `replay` 传入的路径 |
| 监听日志 | `monitor -l` 传入的路径 |
| CSV 数据记录 | 由调用方（宿主应用）自行决定 |
| 端口 | 运行时由系统枚举，不落盘缓存 |

如果把 `miconlab-core` 嵌进自己的宿主应用，配置和日志放在哪里由你决定。

### 测试

```bash
cargo test --workspace
```

测试覆盖校验和、帧解析、波特率检测、协议编解码、录制/回放以及插件 ABI。需要真实硬件的测试标记为 ignored，需手动执行。

### 常见问题

**这个仓库包含 GUI 或设备上位机吗？**
通用 GUI 是包含的：仓库里有可复用的核心库、CLI、MCP 服务器、插件接口、示例插件和通用桌面 GUI。三套专用设备工作区属于商业产品；GUI 里只为它们保留一个 **设备上位机（Device Host）** 占位入口。

**可以在公司里用吗？**
不可以，除非取得商业授权。个人、学术和非商业用途免费。见 [COMMERCIAL.md](COMMERCIAL.md)。

**能接我自己的设备吗？**
可以。通用协议层与记录层不绑定具体设备，你在上面加自己的分帧/解析即可。设备专用实现不在本仓库内。

**为什么有些平台标注“尚未在真机验证”？**
代码能编译、测试也能通过，但我们还没有在该平台的实体串口适配器上跑过。请把它们当作“构建已验证”，而不是“现场已验证”。

**在哪里反馈问题？**
<!-- TODO: issue tracker / contact -->

### 商业使用

商业使用——包括公司内部使用、付费产品或服务，以及基于这套代码构建商业上位机——都需要取得书面商业授权。

商业合作可覆盖：商业授权、上位机开发、设备协议接入、私有化部署与技术支持。

公开仓库包含 **AI 助手** 面板与通用协议分析。商业版额外提供**面向具体设备的 AI/Agent 辅助硬件调试**：Agent 可以驱动指定设备、发送其专有帧、等待特定响应、自动判定通过/失败，并把录制的会话当作回归测试跑。这些设备专用的工作流、配置与提示词模板属于商业产品，不在此公开。

**联系方式：** <!-- TODO: 联系方式 -->

范围与条款见 [COMMERCIAL.md](COMMERCIAL.md)。

### 许可证

miconlab 以 [PolyForm Noncommercial License 1.0.0](LICENSE) 授权，属于**源码可见（source-available）**，**不是**开源项目。

| 使用方式 | 条款 |
|----------|------|
| 个人学习、教学、非商业研究 | 免费 |
| 慈善、教育、公共研究、公共安全/健康、环保、政府使用 | 免费 |
| 商业使用（公司、付费产品或服务） | 需取得商业授权 |
| 专用设备工作区、设备专用通讯实现、固件 | 不在本仓库，属商业产品（GUI 仅显示占位入口） |

第三方组件保留各自的许可，见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。其中内置的 Noto Sans SC 子集按 SIL Open Font License 1.1 分发，本项目不会对其重新授权。

> 以上为项目方的授权声明，不构成法律意见。正式对外发布前建议由律师确认。
