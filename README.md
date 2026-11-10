<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="crates/miconlab-gui/assets/icons/miconlab-512.png">
  <img src="crates/miconlab-gui/assets/icons/miconlab-512.png" alt="miconlab" width="64" height="64">
</picture>

# miconlab

**mcu serial workbench · 单片机实验室**

**Cross-platform serial debugging, protocol analysis, data recording and quality-check toolkit for embedded developers.**

[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-PolyForm%20Noncommercial%201.0.0-blue)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)]()

</div>

---

> **License in one line:** free for personal, study, teaching and non-commercial research; commercial use requires a paid license. See [License](#license).
>
> This repository is **source-available, not open source**.

## What it is

miconlab is a Rust toolkit for talking to serial devices: open a port, exchange bytes, parse frames, detect baud rates, verify checksums, record a session, replay it, and check the captured data for quality problems. It runs on Windows, macOS and Linux, and ships as a CLI plus an MCP server that AI assistants can drive.

## What you can build with it

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

## Core capabilities

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

## Platform support

| Platform | Architecture | Build | Verified on real hardware |
|----------|--------------|-------|---------------------------|
| macOS | Apple Silicon (aarch64), Intel (x86_64) | Implemented | macOS verified |
| Windows | x86_64 | Implemented (cross-built) | Not yet verified on real hardware |
| Linux | x86_64 | Implemented | Not yet verified on real hardware |
| Linux | aarch64 | Implemented (cross-built) | Not yet verified on real hardware |

"Implemented" means the code compiles and the test suite passes for that target. "Not yet verified on real hardware" means the serial stack has not been exercised against physical adapters on that platform.

## Quick start

### Build

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

### CLI

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

### MCP server

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

## Files and paths

miconlab does not create a hidden global configuration directory. Everything is explicit:

| Data | Location |
|------|----------|
| Recorded session script | Path passed to `record -o` (default `script.txt` in the current directory) |
| Replay input | Path passed to `replay` |
| Monitor log | Path passed to `monitor -l` |
| CSV data log | Path chosen by the embedding application |
| Ports | Enumerated at runtime by the OS; nothing is cached on disk |

If you embed `miconlab-core` in your own host application, you decide where configuration and logs live.

## Testing

```bash
cargo test --workspace
```

Tests cover checksums, frame parsing, baud detection, protocol codecs, recorder/replayer and the plugin ABI. Tests that require physical hardware are marked ignored and are expected to be run manually.

## FAQ

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

## Commercial use

Commercial use — including internal company use, paid products or services, and building a commercial host application on top of this code — requires a written commercial license.

Commercial engagements can cover: commercial licensing, host-application development, device protocol integration, private deployment and support.

The public repository includes the **AI Assistant** panel and general protocol analysis. The commercial edition additionally covers **device-specific AI/agent-assisted hardware bring-up**: an agent can drive a particular device, send its proprietary frames, wait for specific responses, judge pass/fail automatically, and run recorded sessions as regression tests. Those device-specific workflows, configurations and prompt templates are part of the commercial product and are not published here.

**Contact:** <!-- TODO: 联系方式 -->

See [COMMERCIAL.md](COMMERCIAL.md) for scope and terms.

## License

miconlab is **source-available** under the [PolyForm Noncommercial License 1.0.0](LICENSE), which is **not** an open-source license.

| Use | Terms |
|-----|-------|
| Personal, study, teaching, non-commercial research | Free of charge |
| Charitable, educational, public research, public safety/health, environmental, government use | Free of charge |
| Commercial use (companies, paid products or services) | Requires a commercial license |
| Dedicated device workspaces, device-specific communication implementations, firmware | Not in this repository; commercial product (GUI shows a placeholder entry) |

Third-party components keep their own licenses — see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). In particular, the bundled Noto Sans SC subset is distributed under the SIL Open Font License 1.1 and is **not** relicensed by this project.

> This is the project's licensing statement, not legal advice. Have counsel review it before any public release.
