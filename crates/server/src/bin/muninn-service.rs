//! Existing localhost adapter plus an explicit, additive server/PWA mode.
use muninn_core::{CoreConfig, Error, Result};
use muninn_server::{
    remote::{default_server_data_dir, ServerConfig, ServerService},
    Service,
};
use std::path::PathBuf;

const HELP: &str = "muninn-service [--data-dir DIR] [--port N]
muninn-service --server [--data-dir DIR] [--bind ADDRESS:PORT] [--public-origin ORIGIN]

Without --server: existing localhost desktop-compatible service.
With --server: optional PWA at /app/, default 127.0.0.1:47872 and separate
~/.local/share/muninn-server data. Remote access needs an HTTPS public origin
and a reverse proxy preserving Host. The pairing token is in DATA_DIR/token;
server mode never prints it. Sessions expire when the server restarts.";

#[derive(Debug)]
struct Args {
    server: bool,
    data_dir: Option<PathBuf>,
    port: u16,
    network: ServerConfig,
    help: bool,
}
fn parse(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut out = Args {
        server: false,
        data_dir: None,
        port: 0,
        network: ServerConfig::default(),
        help: false,
    };
    let mut args = args.into_iter();
    let mut has_port = false;
    let mut has_network = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" => out.server = true,
            "--data-dir" => {
                out.data_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| Error::bad("--data-dir needs a path"))?,
                ))
            }
            "--port" => {
                has_port = true;
                out.port = args
                    .next()
                    .ok_or_else(|| Error::bad("--port needs a number"))?
                    .parse()
                    .map_err(|_| Error::bad("invalid --port"))?;
            }
            "--bind" => {
                has_network = true;
                out.network.bind = args
                    .next()
                    .ok_or_else(|| Error::bad("--bind needs ADDRESS:PORT"))?
                    .parse()
                    .map_err(|_| Error::bad("--bind must be a numeric IP address and port"))?;
            }
            "--public-origin" => {
                has_network = true;
                out.network.public_origin = Some(
                    args.next()
                        .ok_or_else(|| Error::bad("--public-origin needs an HTTP(S) origin"))?,
                );
            }
            "-h" | "--help" => out.help = true,
            _ => return Err(Error::bad(format!("unknown argument {arg}"))),
        }
    }
    if !out.server && has_network {
        return Err(Error::bad("--bind and --public-origin require --server"));
    }
    if out.server && has_port {
        return Err(Error::bad(
            "use --bind ADDRESS:PORT with --server, not --port",
        ));
    }
    Ok(out)
}
fn run() -> Result<()> {
    let args = parse(std::env::args().skip(1))?;
    if args.help {
        println!("{HELP}");
        return Ok(());
    }
    if args.server {
        let data_dir = args
            .data_dir
            .or_else(|| std::env::var_os("MUNINN_SERVER_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(default_server_data_dir);
        let service = ServerService::start(
            CoreConfig {
                data_dir: data_dir.clone(),
                ..Default::default()
            },
            args.network,
        )?;
        println!("Muninn server listening on {}", service.addr);
        println!("Browser app: {}/app/", service.public_origin);
        println!("Data directory: {}", data_dir.display());
        println!("Pairing token file: {}", data_dir.join("token").display());
        service.wait_for_shutdown()
    } else {
        let data_dir = args
            .data_dir
            .or_else(|| std::env::var_os("MUNINN_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                home.join(".local/share/io.github.iarnvidr.muninn")
            });
        let service = Service::start(
            CoreConfig {
                data_dir: data_dir.clone(),
                ..Default::default()
            },
            args.port,
        )?;
        println!("muninn-service listening on {}", service.base_url());
        println!("data dir: {}", data_dir.display());
        println!("token: {}", service.core.token());
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("muninn-service: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Result<Args> {
        parse(values.iter().map(|s| s.to_string()))
    }
    #[test]
    fn desktop_options_remain_loopback_only() {
        let a = args(&["--port", "47871", "--data-dir", "/tmp/cache"]).unwrap();
        assert!(!a.server);
        assert_eq!(a.port, 47871);
        assert!(args(&["--bind", "0.0.0.0:47872"]).is_err());
    }
    #[test]
    fn server_is_explicit_and_rejects_bad_flags() {
        let a = args(&[
            "--server",
            "--bind",
            "0.0.0.0:47872",
            "--public-origin",
            "https://muninn.example.com",
        ])
        .unwrap();
        assert!(a.server);
        assert!(a.network.bind.ip().is_unspecified());
        assert_eq!(
            a.network.public_origin.as_deref(),
            Some("https://muninn.example.com")
        );
        for input in [
            &["--server", "--port", "2"][..],
            &["--bind"],
            &["--port", "bad"],
            &["--data-dir"],
        ] {
            assert!(args(input).is_err(), "{input:?}");
        }
    }
}
