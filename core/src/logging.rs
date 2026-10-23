//! Small bounded diagnostic log. Callers supply event names and IDs, never credentials/URLs.
use crate::engine::Core;
use serde_json::{json, Value};
use std::io::Write;

impl Core {
    pub fn log_event(&self, level: &str, event: &str, data: Value) {
        let data = crate::diagnostics::scrub(data, &[self.token.read().unwrap_or_else(|e|e.into_inner()).clone()], &std::env::var("HOME").unwrap_or_default());
        let _guard = self.log_lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.paths.data_dir.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("muninn.jsonl");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() >= 2 * 1024 * 1024) {
            let _ = std::fs::rename(&path, dir.join("muninn.previous.jsonl"));
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if let Ok(mut file) = options.open(path) {
            let _ = writeln!(
                file,
                "{}",
                json!({"ts":self.now(),"level":level,"event":event,"data":data})
            );
        }
    }

    pub fn logs(&self, limit: usize) -> Value {
        let _guard = self.log_lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.paths.data_dir.join("logs");
        let mut lines = Vec::new();
        for name in ["muninn.previous.jsonl", "muninn.jsonl"] {
            if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
                lines.extend(
                    text.lines()
                        .filter_map(|s| serde_json::from_str::<Value>(s).ok()),
                );
            }
        }
        let skip = lines.len().saturating_sub(limit.min(1000));
        json!({"directory":dir,"events":lines.into_iter().skip(skip).collect::<Vec<_>>()})
    }
}
