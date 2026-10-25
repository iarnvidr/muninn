//! Native browser credentials, never serialized into IPC, settings or diagnostics.
use crate::{Core, Result};
use std::{io::Write, path::{Path, PathBuf}};

// Deliberately neither Debug nor Serialize: values are credentials.
pub struct YoutubeCookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<i64>,
}

pub fn youtube_domain(domain: &str) -> bool {
    let domain = domain.trim_start_matches('.');
    domain == "youtube.com" || domain.ends_with(".youtube.com")
}

fn cookie_line(cookie: &YoutubeCookie) -> Option<String> {
    if !youtube_domain(&cookie.domain) || cookie.name.is_empty() || !cookie.path.starts_with('/')
        || [&cookie.domain, &cookie.path, &cookie.name, &cookie.value].iter()
            .any(|s| s.chars().any(char::is_control)) {
        return None;
    }
    Some(format!("{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        if cookie.http_only { "#HttpOnly_" } else { "" }, cookie.domain,
        if cookie.domain.starts_with('.') { "TRUE" } else { "FALSE" }, cookie.path,
        if cookie.secure { "TRUE" } else { "FALSE" }, cookie.expires.unwrap_or(0),
        cookie.name, cookie.value))
}

/// A separate private jar for each helper. yt-dlp can rewrite its jar on exit;
/// sharing one between concurrent jobs would race. Drop removes it on all exits.
pub(crate) struct CookieFile { directory: PathBuf, path: PathBuf }
impl CookieFile {
    pub(crate) fn path(&self) -> &Path { &self.path }
    pub(crate) fn args(&self) -> [String; 2] {
        ["--cookies".into(), self.path.display().to_string()]
    }
}
impl Drop for CookieFile {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.directory); }
}

impl Core {
    /// Only native code calls this; a delayed cookie read cannot restore a
    /// signed-out or replaced account's credentials.
    pub fn observe_youtube_cookies(&self, key: &str, changed_at: i64, cookies: Vec<YoutubeCookie>) -> bool {
        let mut account = self.account.write().unwrap_or_else(|e| e.into_inner());
        if account.signed_in != Some(true) || account.key.as_deref() != Some(key)
            || account.changed_at != changed_at { return false; }
        account.cookies = cookies.into_iter().filter(|c| cookie_line(c).is_some()).collect();
        true
    }

    pub(crate) fn youtube_cookie_file(&self) -> Result<Option<CookieFile>> {
        let text = {
            let account = self.account.read().unwrap_or_else(|e| e.into_inner());
            if account.signed_in != Some(true) || account.cookies.is_empty() { return Ok(None); }
            account.cookies.iter().filter_map(cookie_line).collect::<String>()
        };
        let directory = std::env::temp_dir().join(format!("muninn-session-{}", uuid::Uuid::new_v4()));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)] {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        let jar = CookieFile { path: directory.join("cookies.txt"), directory };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(jar.path())?;
        file.write_all(b"# Netscape HTTP Cookie File\n")?;
        file.write_all(text.as_bytes())?;
        Ok(Some(jar))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    fn cookie(domain: &str) -> YoutubeCookie {
        YoutubeCookie { domain: domain.into(), path: "/".into(), name: "SID".into(),
            value: "synthetic-secret".into(), secure: true, http_only: true, expires: None }
    }
    #[test]
    fn native_session_is_private_scoped_and_removed_after_helper_exits() {
        let (_d, core) = test_core(None, None);
        core.set_now_for_tests(Some(1000));
        let key = "a".repeat(64);
        core.observe_youtube_account(true, Some(&key), 100).unwrap();
        let mut injected = cookie(".youtube.com"); injected.value = "bad\n.youtube.com\tTRUE".into();
        assert!(core.observe_youtube_cookies(&key, 100, vec![cookie(".youtube.com"), cookie("youtube.com.attacker.test"), cookie("accounts.google.com"), injected]));
        let jar = core.youtube_cookie_file().unwrap().unwrap();
        let path = jar.path().to_owned();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Netscape HTTP Cookie File\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsynthetic-secret\n");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let other = core.youtube_cookie_file().unwrap().unwrap();
        assert_ne!(jar.path(), other.path());
        assert!(!core.youtube_account().to_string().contains("synthetic-secret"));
        drop(jar); assert!(!path.exists());
        core.observe_youtube_account(false, None, 200).unwrap();
        assert!(!core.observe_youtube_cookies(&key, 100, vec![cookie(".youtube.com")]));
        assert!(core.youtube_cookie_file().unwrap().is_none());
        core.observe_youtube_account(true, Some(&key), 300).unwrap();
        core.observe_youtube_cookies(&key, 300, vec![cookie(".youtube.com")]);
        core.observe_youtube_account(true, Some(&"b".repeat(64)), 400).unwrap();
        assert!(core.youtube_cookie_file().unwrap().is_none());
        assert!(!core.observe_youtube_cookies(&key, 300, vec![cookie(".youtube.com")]));
        core.invalidate_youtube_account();
        assert!(core.youtube_cookie_file().unwrap().is_none());
    }
}
