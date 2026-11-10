//! `miconlab record` — 录制脚本。

use anyhow::Result;
use miconlab_core::ScriptRecorder;
use std::io::{self, Write};
use std::path::Path;

use crate::util::open_port;

pub fn run(port_name: &str, output_path: &str, baud: u32) -> Result<()> {
    let mut port = open_port(port_name, baud)?;

    let mut recorder = ScriptRecorder::new("recorded", "Auto-recorded script");
    recorder.start();

    println!("Recording to {}", output_path);
    println!("Type commands to record, 'quit' to stop");
    println!();

    let mut input = String::new();
    loop {
        input.clear();
        print!("> ");
        io::stdout().flush()?;

        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }

        let data = input.trim();
        if data == "quit" {
            break;
        }

        if !data.is_empty() {
            recorder.record_send(data);
            port.write_string(data)?;
            port.write_string("\r\n")?;
        }
    }

    recorder.stop();
    recorder.save(Path::new(output_path))?;
    port.disconnect()?;
    println!("Recording saved to {}", output_path);
    Ok(())
}
