//! CLI client for the running app. No second database owner, service or browser.
use crate::{Error, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

const HELP: &str = "Muninn CLI (the app must be running)
  muninn --cli [--data-dir DIR] status|logs|entries|jobs|tabs
  muninn --cli [--data-dir DIR] diagnostics export FILE.zip  (also works with app closed)
  muninn --cli settings|rules|automation|subscriptions get
  muninn --cli settings|rules|automation|subscriptions set FILE.json|-  (stdin)
  muninn --cli cleanup preview|run
  muninn --cli subscriptions poll
  muninn --cli subscriptions effective
  muninn --cli channel-groups get
  muninn --cli channel-groups set FILE.json|-
  muninn --cli channel-videos CHANNEL_ID library|uploads
  muninn --cli feeds refresh home|subscriptions  (uses the signed-in desktop session)
  muninn --cli channels   (discovered channels and automatic-download status)
  muninn --cli feeds refresh channels  (discover signed-in subscriptions)
  muninn --cli organization get
  muninn --cli organization set FILE.json|-
  muninn --cli group create|delete NAME
  muninn --cli group assign|remove NAME TAB_ID [TAB_ID ...]
  muninn --cli collection create|delete NAME
  muninn --cli collection add|remove NAME VIDEO_ID [VIDEO_ID ...]
  muninn --cli tag create|delete NAME
  muninn --cli tag add|remove NAME VIDEO_ID [VIDEO_ID ...]
  muninn --cli group|collection|tag rename NAME NEW_NAME
  muninn --cli group collapse|expand NAME
  muninn --cli keep|delete|retry|cancel ENTRY_ID
  muninn --cli release ENTRY_ID DAYS   (return a kept copy to the temporary cache)
  muninn --cli cache VIDEO_ID [360|480|720|1080|audio]
  muninn --cli open URL [URL ...]       (adds sleeping tabs)
  muninn --cli import tabs FILE|-      (URL lines or JSON array of URLs)
  muninn --cli import links FILE|- [--collection NAME] [--dry-run]
  muninn --cli library                 (media and saved links, no downloads)
  muninn --cli library remove VIDEO_ID  (remove a saved link, keep local media)
  muninn --cli import media FILE [--video-id ID] [--metadata FILE.info.json]
      [--move] [--enrich] [--dry-run]
  muninn --cli api GET|POST|PUT PATH [/api/...] [FILE.json|-]
All successful output is JSON; errors exit nonzero. Local files are copied and
protected by default. --move removes the source only after a successful copy.
";

pub fn default_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| ".".into());
    #[cfg(target_os = "macos")]
    let root = home.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    root.join("io.github.iarnvidr.muninn")
}
fn read_text(path: &str) -> Result<String> {
    if path == "-" {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        Ok(text)
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}
fn input(path: &str) -> Result<Value> {
    Ok(serde_json::from_str(&read_text(path)?)?)
}
fn abs(path: &str) -> Result<PathBuf> {
    Ok(std::fs::canonicalize(path)?)
}

pub fn run(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        println!("{HELP}");
        return Ok(());
    }
    let mut dir = std::env::var_os("MUNINN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    if args[0] == "--data-dir" {
        if args.len() < 3 {
            return Err(Error::bad("--data-dir needs a directory and command"));
        }
        dir = PathBuf::from(args.remove(1));
        args.remove(0);
    }
    if let [name, action, output] = args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        if *name == "diagnostics" && *action == "export" {
            let result = crate::diagnostics::export(&dir, std::path::Path::new(output))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
    }
    let (method, path, body) = command(&args)?;
    let service: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("service.json")).map_err(
            |_| Error::other("start Muninn first, or use --data-dir for its data directory"),
        )?)?;
    let port = service["port"]
        .as_u64()
        .filter(|p| *p > 0 && *p <= 65535)
        .ok_or_else(|| Error::other("invalid service.json"))?;
    let token = std::fs::read_to_string(dir.join("token"))?;
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(300))
        .build();
    let request = agent
        .request(&method, &format!("http://127.0.0.1:{port}{path}"))
        .set("Authorization", &format!("Bearer {}", token.trim()));
    let response = match body {
        Some(b) => request
            .set("Content-Type", "application/json")
            .send_string(&b.to_string()),
        None => request.call(),
    };
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            return Err(Error::other(format!(
                "HTTP {code}: {}",
                r.into_string().unwrap_or_default()
            )))
        }
        Err(e) => return Err(Error::other(e.to_string())),
    };
    let value: Value = serde_json::from_str(
        &response
            .into_string()
            .map_err(|e| Error::other(e.to_string()))?,
    )?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn command(a: &[String]) -> Result<(String, String, Option<Value>)> {
    let get = |path: &str| Ok(("GET".into(), path.into(), None));
    let post = |path: String, body: Value| Ok(("POST".into(), path, Some(body)));
    match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["status"] => get("/api/status"),
        ["logs"] => get("/api/logs?limit=200"),
        ["feeds", "refresh", kind] => post("/api/feeds/refresh".into(), json!({"kind":kind})),
        ["entries"] => get("/api/entries"),
        ["library"] => get("/api/library"),
        ["library", "remove", id] if crate::util::valid_video_id(id) => {
            post(format!("/api/library/{id}/remove"), json!({}))
        }
        ["jobs"] => get("/api/jobs"),
        ["channels"] => get("/api/channels"),
        ["channel-groups", "get"] => get("/api/channel-groups"),
        ["channel-groups", "set", file] => Ok((
            "PUT".into(),
            "/api/channel-groups".into(),
            Some(input(file)?),
        )),
        ["subscriptions", "effective"] => get("/api/subscriptions/effective"),
        ["channel-videos", id, source] if matches!(*source, "library" | "uploads") => {
            get(&format!("/api/channels/{id}/videos?source={source}"))
        }
        ["tabs"] => get("/api/tabs"),
        ["cleanup", "preview"] => get("/api/cleanup"),
        ["cleanup", "run"] => post("/api/cleanup".into(), json!({})),
        ["subscriptions", "poll"] => post("/api/subscriptions/poll".into(), json!({})),
        [name, "get"]
            if matches!(
                *name,
                "settings" | "rules" | "automation" | "subscriptions" | "organization"
            ) =>
        {
            get(&format!("/api/{name}"))
        }
        [name, "set", file]
            if matches!(
                *name,
                "settings" | "rules" | "automation" | "subscriptions" | "organization"
            ) =>
        {
            Ok(("PUT".into(), format!("/api/{name}"), Some(input(file)?)))
        }
        ["group", action, name] if matches!(*action, "collapse" | "expand") => post(
            "/api/organization".into(),
            json!({"kind":"tab_group","action":"collapse","name":name,"collapsed":*action=="collapse"}),
        ),
        [kind, "rename", name, new_name] if matches!(*kind, "group" | "collection" | "tag") => {
            post(
                "/api/organization".into(),
                json!({"kind":if *kind=="group" {"tab_group"} else {*kind},"action":"rename","name":name,"new_name":new_name}),
            )
        }
        [kind, action, name, ids @ ..]
            if matches!(*kind, "group" | "collection" | "tag")
                && matches!(*action, "create" | "delete" | "assign" | "add" | "remove") =>
        {
            post(
                "/api/organization".into(),
                json!({"kind":if *kind=="group" {"tab_group"} else {*kind},"action":action,"name":name,"ids":ids}),
            )
        }
        ["release", id, days] => {
            if crate::videodb::split_entry_id(id).is_none() {
                return Err(Error::bad("invalid entry id"));
            }
            let days: u32 = days
                .parse()
                .map_err(|_| Error::bad("DAYS must be a whole number"))?;
            post(
                format!("/api/entries/{id}/retention"),
                json!({"protect":false,"retention_days":days}),
            )
        }
        [action, id] if matches!(*action, "keep" | "delete" | "retry" | "cancel") => {
            if crate::videodb::split_entry_id(id).is_none() {
                return Err(Error::bad("invalid entry id"));
            }
            let action = match *action {
                "keep" => "promote",
                "delete" => "evict",
                x => x,
            };
            post(format!("/api/entries/{id}/{action}"), json!({}))
        }
        ["cache", id] => post("/api/cache".into(), json!({"video_id":id,"quality":"480"})),
        ["cache", id, q] => post("/api/cache".into(), json!({"video_id":id,"quality":q})),
        ["open", urls @ ..] if !urls.is_empty() => {
            post("/api/import/tabs".into(), json!({"urls":urls}))
        }
        ["import", "tabs", file] => {
            let text = read_text(file)?;
            let urls: Vec<String> = if text.trim_start().starts_with('[') {
                serde_json::from_str(&text)?
            } else {
                text.lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            post("/api/import/tabs".into(), json!({"urls":urls}))
        }
        ["import", "links", file, flags @ ..] => {
            let text = read_text(file)?;
            let items: Value = if text.trim_start().starts_with('[') {
                serde_json::from_str(&text)?
            } else {
                json!(text
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>())
            };
            let mut collection = "Imported links";
            let mut dry_run = false;
            let mut i = 0;
            while i < flags.len() {
                match flags[i] {
                    "--collection" if i + 1 < flags.len() => {
                        i += 1;
                        collection = flags[i];
                    }
                    "--dry-run" => dry_run = true,
                    _ => return Err(Error::bad("use --collection NAME and/or --dry-run")),
                }
                i += 1;
            }
            post(
                "/api/import/links".into(),
                json!({"items":items,"collection":collection,"dry_run":dry_run}),
            )
        }
        ["import", "media", file, flags @ ..] => {
            let mut b = json!({"path":abs(file)?});
            let mut i = 0;
            while i < flags.len() {
                match flags[i] {
                    "--move" => b["move_file"] = json!(true),
                    "--enrich" => b["enrich"] = json!(true),
                    "--dry-run" => b["dry_run"] = json!(true),
                    "--video-id" | "--metadata" => {
                        let flag = flags[i];
                        i += 1;
                        let v = flags
                            .get(i)
                            .ok_or_else(|| Error::bad("missing option value"))?;
                        if flag == "--video-id" {
                            b["video_id"] = json!(v);
                        } else {
                            b["metadata"] = json!(abs(v)?);
                        }
                    }
                    x => return Err(Error::bad(format!("unknown option {x}"))),
                }
                i += 1;
            }
            post("/api/import/media".into(), b)
        }
        ["api", method, path, rest @ ..]
            if matches!(*method, "GET" | "POST" | "PUT")
                && path.starts_with("/api/")
                && rest.len() <= 1 =>
        {
            Ok((
                method.to_string(),
                path.to_string(),
                rest.first().map(|f| input(f)).transpose()?,
            ))
        }
        _ => Err(Error::bad(format!("unknown command or arguments\n{HELP}"))),
    }
}
