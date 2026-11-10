//! Opt-in server + PWA. This never replaces the desktop's embedded Service.
#[path = "../web/mod.rs"]
mod web;
use muninn_core::{Core, CoreConfig};
use std::{net::SocketAddr, path::PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("muninn-web: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut bind = std::env::var("MUNINN_WEB_BIND").unwrap_or_else(|_| "127.0.0.1:47872".into());
    let mut origin = std::env::var("MUNINN_WEB_ORIGIN").ok();
    let mut token_file = std::env::var_os("MUNINN_WEB_TOKEN_FILE").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let mut data_dir = std::env::var_os("MUNINN_DATA_DIR").map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/muninn-server"));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = |args: &mut std::iter::Skip<std::env::Args>| args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--bind" => bind = value(&mut args)?,
            "--origin" => origin = Some(value(&mut args)?),
            "--token-file" => token_file = Some(PathBuf::from(value(&mut args)?)),
            "--data-dir" => data_dir = PathBuf::from(value(&mut args)?),
            "--init-token" => {
                use std::io::Write;
                let path = PathBuf::from(value(&mut args)?);
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
                let mut file = options.open(&path)?;
                writeln!(file, "{}", muninn_core::util::random_token())?;
                file.sync_all()?;
                println!("Created token file {} (secret not printed).", path.display());
                return Ok(());
            }
            "--help" | "-h" => {
                println!("muninn-web --token-file FILE [--bind 127.0.0.1:47872] [--origin https://muninn.example] [--data-dir DIR]\nmuninn-web --init-token FILE\nUse a dedicated server data directory, not a running desktop app's directory.");
                return Ok(());
            }
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }
    let addr: SocketAddr = bind.parse()?;
    if addr.port() == 0 { return Err("choose an explicit port".into()); }
    let origin = match origin {
        Some(origin) => origin,
        None if addr.ip().is_loopback() => format!("http://{addr}"),
        None => return Err("--origin is required for non-loopback listeners".into()),
    };
    if !addr.ip().is_loopback() && !origin.starts_with("https://") {
        return Err("non-loopback listeners require an HTTPS public origin and a trusted TLS reverse proxy".into());
    }
    let path = token_file.ok_or("--token-file is required; create one with --init-token first")?;
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 1024 { return Err("invalid token file".into()); }
    let config = web::Config::new(&origin, &std::fs::read_to_string(path)?)?;
    // Bind before starting workers. A conflicting port must not open a second
    // core, and a running desktop must never silently be displaced.
    let socket = std::net::TcpListener::bind(addr)?;
    socket.set_nonblocking(true)?;
    let core = Core::open(CoreConfig { data_dir, spawn_workers: true, ..CoreConfig::default() })?;
    let app = web::router(core.clone(), config);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    println!("Muninn server PWA: {}/app/ (listener {addr})", origin.trim_end_matches('/'));
    let result = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(socket)?;
        axum::serve(listener, app).await
    });
    core.shutdown();
    result?;
    Ok(())
}
