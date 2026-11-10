//! Native and browser UI assets, embedded by their respective HTTP adapters.
//!
//! Keeping assets outside the application core prevents frontend edits from
//! invalidating the core and its independent policy, capture and storage tests.

pub const INDEX_HTML: &str = include_str!("../../../ui/index.html");
pub const APP_JS: &str = include_str!("../../../ui/app.js");
pub const LIB_JS: &str = include_str!("../../../ui/lib.js");
pub const QUEUE_JS: &str = include_str!("../../../ui/queue.js");
pub const WATCH_JS: &str = include_str!("../../../ui/watch.js");
pub const FEED_GRAPH_JS: &str = include_str!("../../../ui/feed-graph.js");
/// Offline graph engine shared with Chat Hub; loaded on demand by Feeds.
pub const CYTOSCAPE_MJS: &str = include_str!("../../../ui/vendor/cytoscape.esm.min.mjs");
pub const CYTOSCAPE_LICENSE: &str = include_str!("../../../ui/vendor/CYTOSCAPE-LICENSE");
pub const PAGES_JS: &str = include_str!("../../../ui/pages.js");
pub const BROWSE_CUSTOM_JS: &str = include_str!("../../../ui/browse-custom.js");
pub const UPDATES_JS: &str = include_str!("../../../ui/updates.js");
pub const ORGANIZE_JS: &str = include_str!("../../../ui/organize.js");
pub const CHANNELS_JS: &str = include_str!("../../../ui/channels.js");
pub const SEARCH_JS: &str = include_str!("../../../ui/search.js");
pub const BOOT_JS: &str = include_str!("../../../ui/boot.js");
/// Development mock of the Tauri side. Only ever loaded by `?mock=1` outside the app.
pub const MOCK_JS: &str = include_str!("../../../ui/mock.js");
/// hls.js, loaded on demand and only where the webview has no native HLS.
pub const HLS_JS: &str = include_str!("../../../ui/hls.min.js");
pub const STYLE_CSS: &str = include_str!("../../../ui/style.css");
pub const RAVEN_PNG: &[u8] = include_bytes!("../../../ui/raven.png");

const JS: &str = "text/javascript; charset=utf-8";

/// The additive browser client lives at `/app/` in explicit server mode only.
/// Keep a separate allowlist: the native shell, mocks and privileged Tauri
/// bridge are never part of the installable PWA's asset namespace.
pub fn pwa_asset(name: &str) -> Option<(&'static str, &'static [u8])> {
    match name {
        "" | "index.html" => Some(("text/html; charset=utf-8", include_bytes!("../../../ui/pwa/index.html"))),
        "app.js" => Some((JS, include_bytes!("../../../ui/pwa/app.js"))),
        "app.css" => Some(("text/css; charset=utf-8", include_bytes!("../../../ui/pwa/app.css"))),
        "manifest.webmanifest" => Some(("application/manifest+json", include_bytes!("../../../ui/pwa/manifest.webmanifest"))),
        "sw.js" => Some((JS, include_bytes!("../../../ui/pwa/sw.js"))),
        "icon-192.png" => Some(("image/png", include_bytes!("../../../ui/pwa/icon-192.png"))),
        "icon-512.png" => Some(("image/png", include_bytes!("../../../ui/pwa/icon-512.png"))),
        "raven.png" => Some(("image/png", RAVEN_PNG)),
        "hls.min.js" => Some((JS, HLS_JS.as_bytes())),
        _ => None,
    }
}

pub fn font(name: &str) -> Option<&'static [u8]> {
    match name {
        "Cinzel.ttf" => Some(include_bytes!("../../../ui/fonts/Cinzel.ttf")),
        "Inter.ttf" => Some(include_bytes!("../../../ui/fonts/Inter.ttf")),
        "IBMPlexMono.ttf" => Some(include_bytes!("../../../ui/fonts/IBMPlexMono.ttf")),
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
        "feed-graph.js" => Some((JS, FEED_GRAPH_JS)),
        "vendor/cytoscape.esm.min.mjs" => Some((JS, CYTOSCAPE_MJS)),
        "vendor/CYTOSCAPE-LICENSE" => Some(("text/plain; charset=utf-8", CYTOSCAPE_LICENSE)),
        "pages.js" => Some((JS, PAGES_JS)),
        "browse-custom.js" => Some((JS, BROWSE_CUSTOM_JS)),
        "updates.js" => Some((JS, UPDATES_JS)),
        "organize.js" => Some((JS, ORGANIZE_JS)),
        "channels.js" => Some((JS, CHANNELS_JS)),
        "search.js" => Some((JS, SEARCH_JS)),
        "boot.js" => Some((JS, BOOT_JS)),
        "mock.js" => Some((JS, MOCK_JS)),
        "hls.min.js" => Some((JS, HLS_JS)),
        "style.css" => Some(("text/css; charset=utf-8", STYLE_CSS)),
        "Cinzel-OFL.txt" => Some(("text/plain; charset=utf-8", include_str!("../../../ui/fonts/Cinzel-OFL.txt"))),
        "Inter-OFL.txt" => Some(("text/plain; charset=utf-8", include_str!("../../../ui/fonts/Inter-OFL.txt"))),
        "IBMPlexMono-OFL.txt" => Some(("text/plain; charset=utf-8", include_str!("../../../ui/fonts/IBMPlexMono-OFL.txt"))),
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
        for name in ["lib.js", "app.js", "queue.js", "watch.js", "feed-graph.js", "pages.js", "browse-custom.js", "updates.js", "boot.js", "mock.js", "hls.min.js", "style.css"] {
            assert!(!asset(name).unwrap().1.is_empty(), "{name} is empty");
        }
        assert!(asset("vendor/cytoscape.esm.min.mjs").unwrap().1.len() > 100_000);
        assert!(asset("vendor/CYTOSCAPE-LICENSE").unwrap().1.contains("MIT"));
        assert!(asset("nope.js").is_none());
    }

    #[test]
    fn the_mock_is_not_referenced_by_the_page() {
        // Only boot.js may load it, and only for ?mock=1 outside the app.
        let (_, html) = asset("").unwrap();
        assert!(!html.contains("mock.js"));
        assert!(!html.contains("hls.min.js"));
    }

    #[test]
    fn browser_assets_are_separate_and_cannot_escape_the_namespace() {
        for name in ["", "index.html", "app.js", "app.css", "manifest.webmanifest",
            "sw.js", "icon-192.png", "icon-512.png", "raven.png", "hls.min.js"] {
            assert!(!pwa_asset(name).unwrap().1.is_empty(), "empty PWA asset: {name}");
        }
        for name in ["../index.html", "../token", "mock.js", "boot.js", "watch.js",
            "/app.js", "app.js?token=secret", "%2e%2e/index.html"] {
            assert!(pwa_asset(name).is_none(), "unexpected PWA asset: {name}");
        }
        assert!(asset("sw.js").is_none(), "native UI must not register a PWA worker");
        assert_eq!(pwa_asset("manifest.webmanifest").unwrap().0, "application/manifest+json");
    }

    #[test]
    fn install_icons_have_their_advertised_png_dimensions() {
        for (name, size) in [("icon-192.png", 192u32), ("icon-512.png", 512u32)] {
            let png = pwa_asset(name).unwrap().1;
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), size);
            assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), size);
        }
    }
}
