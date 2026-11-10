//! miconlab-cli 入口。
//!
//! 阶段三重构后，命令实现拆分到 [`commands`] 子模块，公共辅助位于 [`util`]。
//! 本文件仅保留 clap 枚举定义与命令调度。

mod commands;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "miconlab")]
#[command(about = "miconlab - Cross-platform serial port assistant for embedded developers")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List available serial ports
    List {
        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Connect to a serial port
    Connect {
        /// Port name (e.g., COM1, /dev/ttyUSB0)
        port: String,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,

        /// Data bits (5, 6, 7, 8)
        #[arg(short = 'd', long, default_value = "8")]
        data_bits: u8,

        /// Stop bits (1, 2)
        #[arg(short = 's', long, default_value = "1")]
        stop_bits: u8,

        /// Parity (none, odd, even)
        #[arg(short = 'p', long, default_value = "none")]
        parity: String,

        /// Flow control (none, software, hardware)
        #[arg(short = 'f', long, default_value = "none")]
        flow_control: String,
    },

    /// Send data to a serial port
    Send {
        /// Port name
        port: String,

        /// Data to send
        data: String,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,

        /// Send as hex
        #[arg(short = 'x', long)]
        hex: bool,
    },

    /// Monitor serial port data
    Monitor {
        /// Port name
        port: String,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,

        /// Log file path
        #[arg(short, long)]
        log: Option<String>,

        /// Show timestamps
        #[arg(short = 't', long)]
        timestamp: bool,

        /// Hex mode
        #[arg(short = 'x', long)]
        hex: bool,
    },

    /// Replay a script
    Replay {
        /// Port name
        port: String,

        /// Script file path
        script: String,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,
    },

    /// Record a script
    Record {
        /// Port name
        port: String,

        /// Output script file
        #[arg(short, long, default_value = "script.txt")]
        output: String,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,
    },

    /// Agent mode (JSON output for automation)
    Agent {
        /// Port name (optional, lists ports if not provided)
        port: Option<String>,

        /// Action to perform
        #[command(subcommand)]
        action: AgentAction,
    },

    /// Modbus RTU quick request
    Modbus {
        /// Port name
        port: String,

        /// Baud rate
        #[arg(short, long, default_value = "9600")]
        baud: u32,

        /// Slave ID (1-247)
        #[arg(short = 'i', long, default_value = "1")]
        slave_id: u8,

        /// Function code (1-4=read, 5-6=write single, 15-16=write multiple)
        #[arg(short = 'f', long)]
        function: u8,

        /// Start register address
        #[arg(short = 'a', long)]
        address: u16,

        /// Number of registers (for read) or value (for write)
        #[arg(short = 'v', long, default_value = "1")]
        value: u16,
    },

    /// Compute CRC/checksum for data
    Crc {
        /// Algorithm (crc16-modbus, crc16-ccitt, crc32, lrc, sum8, sum16)
        #[arg(short, long, default_value = "crc16-modbus")]
        algorithm: String,

        /// Data (hex string)
        data: String,
    },
}

#[derive(Subcommand)]
pub enum AgentAction {
    /// List all ports
    ListPorts,

    /// Get port info
    PortInfo {
        /// Port name
        name: String,
    },

    /// Send data
    Send {
        /// Data to send
        data: String,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,

        /// Send as hex
        #[arg(short = 'x', long)]
        hex: bool,
    },

    /// Send command and wait for response (write-read)
    SendCommand {
        /// Command to send
        command: String,

        /// Response timeout in ms
        #[arg(short, long, default_value = "1000")]
        timeout: u64,

        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,
    },

    /// Read data with timeout
    Read {
        /// Timeout in milliseconds
        #[arg(short, long, default_value = "1000")]
        timeout: u64,

        /// Maximum bytes to read
        #[arg(short, long, default_value = "1024")]
        max_bytes: usize,
    },

    /// Set DTR signal
    SetDtr {
        /// true or false
        #[arg(long)]
        value: bool,
    },

    /// Set RTS signal
    SetRts {
        /// true or false
        #[arg(long)]
        value: bool,
    },

    /// Change baud rate without disconnecting
    ChangeBaud {
        /// New baud rate
        baud: u32,
    },

    /// Run a script
    RunScript {
        /// Script file path
        script: String,
    },
}

fn main() -> Result<()> {
    // Windows 控制台默认 GBK 代码页，先切 UTF-8；macOS/Linux 为空操作
    miconlab_core::platform::prepare_console();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::List { format } => commands::list::run(&format),
        Commands::Connect {
            port,
            baud,
            data_bits,
            stop_bits,
            parity,
            flow_control,
        } => commands::connect::run(&port, baud, data_bits, stop_bits, &parity, &flow_control),
        Commands::Send {
            port,
            data,
            baud,
            hex,
        } => commands::send::run(&port, &data, baud, hex),
        Commands::Monitor {
            port,
            baud,
            log,
            timestamp,
            hex,
        } => commands::monitor::run(&port, baud, log.as_deref(), timestamp, hex),
        Commands::Replay {
            port,
            script,
            baud,
        } => commands::replay::run(&port, &script, baud),
        Commands::Record {
            port,
            output,
            baud,
        } => commands::record::run(&port, &output, baud),
        Commands::Agent { port, action } => commands::agent::run(port.as_deref(), action),
        Commands::Modbus {
            port,
            baud,
            slave_id,
            function,
            address,
            value,
        } => commands::modbus::run(&port, baud, slave_id, function, address, value),
        Commands::Crc { algorithm, data } => commands::crc::run(&algorithm, &data),
    }
}
