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

    /// Logs at most one event per `key` per minute; the next one reports how many were skipped.
    fn log_coalesced(&self, level: &str, event: &str, key: String, mut data: Value) {
        const WINDOW_MS: i64 = 60_000;
        let now = self.now();
        let skipped = {
            let mut seen = self.log_coalesce.lock().unwrap_or_else(|e| e.into_inner());
            if seen.len() > 256 {
                seen.clear();
            }
            let slot = seen.entry(key).or_insert((i64::MIN / 2, 0));
            if now - slot.0 < WINDOW_MS {
                slot.1 += 1;
                return;
            }
            let skipped = slot.1;
            *slot = (now, 0);
            skipped
        };
        if skipped > 0 {
            data["suppressed_since_last"] = json!(skipped);
        }
        self.log_event(level, event, data);
    }

    /// A failed HTTP request to the local API. A missing thumbnail is routine (not cached yet) and
    /// never an error; every repeated failure is coalesced so the bounded log is not flooded.
    pub fn log_api_failure(&self, method: &str, route: &str, status: u16) {
        if status == 404 && route == "/thumb/{id}" {
            self.log_coalesced("debug", "thumb.missing", "thumb.missing".into(), json!({"status":status}));
        } else {
            self.log_coalesced("error", "api.failed", format!("{method} {route} {status}"), json!({"method":method,"route":route,"status":status}));
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

#[cfg(test)]
mod tests {
    use crate::engine::testutil::test_core;

    fn events(core: &crate::Core, name: &str) -> Vec<serde_json::Value> {
        core.logs(1000)["events"].as_array().unwrap().iter().filter(|e| e["event"] == name).cloned().collect()
    }

    #[test]
    fn missing_thumbnails_are_debug_noise_and_never_api_failures() {
        let (_d, core) = test_core(None, None);
        core.set_now_for_tests(Some(1_000_000));
        for _ in 0..50 {
            core.log_api_failure("GET", "/thumb/{id}", 404);
        }
        assert!(events(&core, "api.failed").is_empty());
        let missing = events(&core, "thumb.missing");
        assert_eq!((missing.len(), missing[0]["level"].as_str()), (1, Some("debug")), "50 misses coalesce into one line");
        core.set_now_for_tests(Some(1_000_000 + 61_000));
        core.log_api_failure("GET", "/thumb/{id}", 404);
        let missing = events(&core, "thumb.missing");
        assert_eq!((missing.len(), missing[1]["data"]["suppressed_since_last"].as_u64()), (2, Some(49)));
    }

    #[test]
    fn other_api_failures_are_still_logged_but_bounded() {
        let (_d, core) = test_core(None, None);
        core.set_now_for_tests(Some(5_000_000));
        for _ in 0..20 {
            core.log_api_failure("GET", "/api/entries", 500);
        }
        core.log_api_failure("POST", "/api/watch/{id}", 500);
        core.log_api_failure("GET", "/thumb/{id}", 500);
        let failed = events(&core, "api.failed");
        assert_eq!(failed.len(), 3, "{failed:?}");
        assert_eq!(failed[0]["level"], "error");
    }
}
