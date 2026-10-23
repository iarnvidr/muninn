//! The UI, embedded and served at `/ui/` by the localhost server.

pub const INDEX_HTML: &str = include_str!("../../ui/index.html");
pub const APP_JS: &str = include_str!("../../ui/app.js");
pub const LIB_JS: &str = include_str!("../../ui/lib.js");
pub const QUEUE_JS: &str = include_str!("../../ui/queue.js");
pub const WATCH_JS: &str = include_str!("../../ui/watch.js");
pub const PAGES_JS: &str = include_str!("../../ui/pages.js");
pub const ORGANIZE_JS: &str = include_str!("../../ui/organize.js");
pub const CHANNELS_JS: &str = include_str!("../../ui/channels.js");
pub const BOOT_JS: &str = include_str!("../../ui/boot.js");
/// Development mock of the Tauri side. Only ever loaded by `?mock=1` outside the app.
pub const MOCK_JS: &str = include_str!("../../ui/mock.js");
/// hls.js, loaded on demand and only where the webview has no native HLS.
pub const HLS_JS: &str = include_str!("../../ui/hls.min.js");
pub const STYLE_CSS: &str = include_str!("../../ui/style.css");
pub const RAVEN_PNG: &[u8] = include_bytes!("../../ui/raven.png");

const JS: &str = "text/javascript; charset=utf-8";

pub fn font(name: &str) -> Option<&'static [u8]> {
    match name {
        "Cinzel.ttf" => Some(include_bytes!("../../ui/fonts/Cinzel.ttf")),
        "Inter.ttf" => Some(include_bytes!("../../ui/fonts/Inter.ttf")),
        "IBMPlexMono.ttf" => Some(include_bytes!("../../ui/fonts/IBMPlexMono.ttf")),
        _ => None,
    }
}

pub fn asset(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "" | "index.html" => Some(("text/html; charset=utf-8", INDEX_HTML)),
        "app.js" => Some((JS, APP_JS)),
        "lib.js" => Some((JS, LIB_JS)),
        "queue.js" => Some((JS, QUEUE_JS)),
        "watch.js" => Some((JS, WATCH_JS)),
        "pages.js" => Some((JS, PAGES_JS)),
        "organize.js" => Some((JS, ORGANIZE_JS)),
        "channels.js" => Some((JS, CHANNELS_JS)),
        "boot.js" => Some((JS, BOOT_JS)),
        "mock.js" => Some((JS, MOCK_JS)),
        "hls.min.js" => Some((JS, HLS_JS)),
        "style.css" => Some(("text/css; charset=utf-8", STYLE_CSS)),
        "Cinzel-OFL.txt" => Some(("text/plain; charset=utf-8", include_str!("../../ui/fonts/Cinzel-OFL.txt"))),
        "Inter-OFL.txt" => Some(("text/plain; charset=utf-8", include_str!("../../ui/fonts/Inter-OFL.txt"))),
        "IBMPlexMono-OFL.txt" => Some(("text/plain; charset=utf-8", include_str!("../../ui/fonts/IBMPlexMono-OFL.txt"))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_the_page_loads_is_embedded() {
        let (_, html) = asset("").unwrap();
        for src in html.split("src=\"/ui/").skip(1).chain(html.split("href=\"/ui/").skip(1)) {
            let name = src.split('"').next().unwrap();
            assert!(asset(name).is_some() || name == "raven.png", "index.html loads /ui/{name} but it is not embedded");
        }
        for name in ["lib.js", "app.js", "queue.js", "watch.js", "pages.js", "boot.js", "mock.js", "hls.min.js", "style.css"] {
            assert!(!asset(name).unwrap().1.is_empty(), "{name} is empty");
        }
        assert!(asset("nope.js").is_none());
    }

    #[test]
    fn the_mock_is_not_referenced_by_the_page() {
        // Only boot.js may load it, and only for ?mock=1 outside the app.
        let (_, html) = asset("").unwrap();
        assert!(!html.contains("mock.js"));
        assert!(!html.contains("hls.min.js"));
    }
}
