//! Headless Muninn service: the same core the app embeds, for Linux checks and
//! scripting.
//!
//!   muninn-service [--data-dir DIR] [--port N]
//!
//! Prints the base URL and this launch's API token, then serves until killed. For Linux checks and
//! scripting only: the app embeds the same core and never exposes a token to the user.

use muninn_core::{CoreConfig, Service};
use std::path::PathBuf;

fn main() {
    let mut data_dir: Option<PathBuf> = std::env::var_os("MUNINN_DATA_DIR").map(PathBuf::from);
    let mut port: u16 = 0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = args.next().map(PathBuf::from),
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            "-h" | "--help" => {
                println!("muninn-service [--data-dir DIR] [--port N]");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let data_dir = data_dir.unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join(".local/share/io.github.iarnvidr.muninn")
    });
    let service = match Service::start(CoreConfig { data_dir: data_dir.clone(), ..CoreConfig::default() }, port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("muninn-service: could not start: {e}");
            std::process::exit(1);
        }
    };
    println!("muninn-service listening on {}", service.base_url());
    println!("data dir: {}", data_dir.display());
    println!("token: {}", service.core.token());
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
