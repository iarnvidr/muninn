//! Public app links open the same tabs as the address bar, with no CLI bridge.
use url::Url;

/// Accept a direct HTTP(S) YouTube URL or one explicit app-link open action.
/// In particular, never turn a malformed app link into an external-browser launch.
pub fn youtube_target(link: &str) -> Option<Url> {
    if link.len() > 16_384 {
        return None;
    }
    let link = Url::parse(link).ok()?;
    let target = if link.scheme() == "muninn" {
        if link.host_str() != Some("open")
            || !matches!(link.path(), "" | "/")
            || !link.username().is_empty()
            || link.password().is_some()
            || link.port().is_some()
            || link.fragment().is_some()
        {
            return None;
        }
        let mut parameters = link.query_pairs();
        let (key, value) = parameters.next()?;
        if key != "url" || parameters.next().is_some() {
            return None;
        }
        Url::parse(&value).ok()?
    } else {
        link
    };
    let host = target.host_str()?;
    if !matches!(target.scheme(), "http" | "https")
        || !target.username().is_empty()
        || target.password().is_some()
        || target.port().is_some()
        || !(host == "youtu.be" || muninn_core::tabs::is_youtube_host(host))
    {
        return None;
    }
    // The normal tab router sends non-video short links back to the browser.
    // Reject those here rather than create a handoff loop with an external caller.
    if host == "youtu.be" && muninn_core::tabs::video_id_of_url(target.as_str()).is_none() {
        return None;
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_link(target: &str) -> String {
        let mut link = Url::parse("muninn://open").unwrap();
        link.query_pairs_mut().append_pair("url", target);
        link.into()
    }

    #[test]
    fn accepts_youtube_pages_and_preserves_the_target_query() {
        for target in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=35&list=PLtest",
            "https://youtu.be/dQw4w9WgXcQ?t=35",
            "https://m.youtube.com/shorts/dQw4w9WgXcQ",
            "https://music.youtube.com/",
            "https://www.youtube.com/@channel",
            "https://www.youtube.com/results?search_query=a%26b+exercise",
        ] {
            assert_eq!(youtube_target(&app_link(target)), Some(Url::parse(target).unwrap()));
            assert_eq!(youtube_target(target), Some(Url::parse(target).unwrap()));
        }
    }

    #[test]
    fn rejects_external_targets_credentials_and_non_web_schemes() {
        for target in [
            "https://example.com/",
            "https://youtube.com.evil.example/",
            "https://youtube.com@evil.example/",
            "https://user:secret@youtube.com/",
            "https://youtube.com:8443/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "muninn://open",
            "https://youtu.be/",
            "https://youtu.be/short",
        ] {
            assert!(youtube_target(&app_link(target)).is_none(), "{target}");
            assert!(youtube_target(target).is_none(), "{target}");
        }
    }

    #[test]
    fn rejects_ambiguous_or_unsupported_actions() {
        for link in [
            "https://open?url=https%3A%2F%2Fyoutube.com",
            "muninn://cache?url=https%3A%2F%2Fyoutube.com",
            "muninn://open/extra?url=https%3A%2F%2Fyoutube.com",
            "muninn://user@open?url=https%3A%2F%2Fyoutube.com",
            "muninn://open:123?url=https%3A%2F%2Fyoutube.com",
            "muninn://open?url=https%3A%2F%2Fyoutube.com#extra",
            "muninn://open?url=https%3A%2F%2Fyoutube.com&url=https%3A%2F%2Fyoutu.be",
            "muninn://open?url=https%3A%2F%2Fyoutube.com&download=true",
            "muninn://open?url=%ZZ",
            "muninn://open",
        ] {
            assert!(youtube_target(link).is_none(), "{link}");
        }
        assert!(youtube_target(&"x".repeat(16_385)).is_none());
    }

    #[test]
    fn accepted_targets_never_route_back_to_an_external_browser() {
        for target in [
            "https://youtu.be/dQw4w9WgXcQ?t=35",
            "https://www.youtube.com:443/watch?v=dQw4w9WgXcQ",
            "http://youtube.com/playlist?list=PLexample",
            "https://www.youtube.com/results?search_query=rust",
        ] {
            let target = youtube_target(&app_link(target)).unwrap();
            assert!(!matches!(muninn_core::tabs::classify_input(target.as_str()), muninn_core::tabs::Input::External(_)));
        }
    }
}
