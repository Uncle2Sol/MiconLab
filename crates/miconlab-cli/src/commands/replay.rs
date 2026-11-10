//! `miconlab replay` — 回放脚本。

use anyhow::Result;
use miconlab_core::recorder::Action;
use miconlab_core::ScriptReplayer;
use std::path::Path;
use std::time::Duration;

use crate::util::open_port;

pub fn run(port_name: &str, script_path: &str, baud: u32) -> Result<()> {
    let mut port = open_port(port_name, baud)?;

    let mut replayer = ScriptReplayer::load(Path::new(script_path))?;
    replayer.start();

    println!("Replaying script: {}", replayer.script().name);

    while let Some(cmd) = replayer.next_command() {
        match cmd.action {
            Action::Send => {
                if let Some(ref data) = cmd.data {
                    println!("Sending: {}", data);
                    port.write_string(data)?;
                    port.write_string("\r\n")?;
                }
            }
            Action::Wait => {
                std::thread::sleep(Duration::from_millis(cmd.delay_ms));
            }
            Action::Read => {
                let mut buf = [0u8; 1024];
                if let Ok(n) = port.read(&mut buf) {
                    if n > 0 {
                        println!("Received: {}", String::from_utf8_lossy(&buf[..n]));
                    }
                }
            }
            Action::Comment => {
                if let Some(ref comment) = cmd.data {
                    println!("# {}", comment);
                }
            }
        }
        std::thread::sleep(replayer.get_delay());
    }

    port.disconnect()?;
    println!("Script replay complete");
    Ok(())
}
