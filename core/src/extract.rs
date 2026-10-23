//! Pure, defensive parsers for YouTube JSON and page HTML.
//!
//! Port of `extension/extract.js`. Everything works on `serde_json::Value`;
//! nothing here panics or errors on bad input, bad input yields empty results.
//! The proxy calls, per captured response:
//!
//! * [`classify_request`] / [`classify_request_bytes`] / [`classify`]: what a
//!   `/youtubei/v1/<endpoint>` request is (home, search, related, ...), from the
//!   URL, the JSON request body and the page URL ([`page_context`]).
//! * [`extract_feed`]: every video in a youtubei/page JSON document, page order,
//!   deduplicated, as [`Item`]s (`include_shorts` keeps Shorts).
//! * [`extract_video_meta`]: a [`VideoIngest`] from a `player` response
//!   (`videoDetails`) and/or a `next` response (primary/secondary info plus
//!   related videos). `captured_at` is left unset.
//! * [`extract_formats`]: the itag/size table of a player response
//!   ([`Format`]), fed to `playback_url::verify_attribution`.
//! * [`extract_comments`]: [`ExtractedComments`] from comment threads, replies
//!   and the newer entity-batch (`commentEntityPayload`) shapes.
//! * [`extract_comment_continuations`] / [`comments_section_token`]: the
//!   continuation tokens that load (more) comments, from a watch page's
//!   `ytInitialData`, a `next` response or a comments response.
//! * [`initial_data_from_html`]: finds `ytInitialData` and
//!   `ytInitialPlayerResponse` in watch/home/search page HTML.
//! * [`parse_json_text`] (XSSI prefix tolerant) and [`parse_request_body`]
//!   (size capped, object only) decode raw bodies.
//! * Helpers: [`walk`] (iterative, node/depth capped), [`text`],
//!   [`parse_duration`], [`parse_count`], [`pick_thumbnail`], [`clip`],
//!   [`video_id_from_url`], [`comment_key`], [`parse_url`], [`parse_query`].
//!
//! Note: object key order is whatever `serde_json::Map` iterates. Page order of
//! items inside arrays is always preserved; for keys of one object it is
//! preserved only when serde_json's `preserve_order` feature is enabled.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Comment, Item, VideoIngest};
use crate::util::valid_video_id;

/// Default node cap of [`walk`].
pub const MAX_NODES: usize = 300_000;
/// Default depth cap of [`walk`].
pub const MAX_DEPTH: usize = 400;
/// Largest request body accepted by [`parse_request_body`].
pub const MAX_REQUEST_BODY: usize = 262_144;
/// Largest `ytInitial*` object [`initial_data_from_html`] will scan.
pub const MAX_HTML_OBJECT: usize = 16 * 1024 * 1024;
/// How many `ytInitial*` assignment candidates are tried per name.
const MAX_HTML_CANDIDATES: usize = 8;
/// How many raw occurrences of a name are inspected per page.
const MAX_HTML_OCCURRENCES: usize = 2_000;

// ---- small helpers ----------------------------------------------------------

/// JS `\s`: whitespace plus BOM.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

fn is_obj(v: &Value) -> bool {
    v.is_object() || v.is_array()
}

/// JS truthiness of an optional JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn ne(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Collapse whitespace runs to one space, trim, and cut to `n` characters.
pub fn clip(s: &str, n: usize) -> String {
    let mut out = String::with_capacity(s.len().min(n.saturating_mul(4)));
    let mut pending_space = false;
    let mut count = 0usize;
    for c in s.chars() {
        if is_js_space(c) {
            pending_space = count > 0;
            continue;
        }
        if pending_space {
            if count >= n {
                break;
            }
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        if count >= n {
            break;
        }
        out.push(c);
        count += 1;
    }
    out
}

/// Safe nested getter: `g(v, &["a", "0", "b"])`. Numeric segments index arrays.
fn g<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path {
        cur = match cur {
            Value::Object(m) => m.get(*seg)?,
            Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn g_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    g(v, path).and_then(Value::as_str)
}

fn valid_id(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| valid_video_id(s))
}

/// JS `String(number)` for the cases that matter here.
fn js_number_string(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    match n.as_f64() {
        Some(f) if f.is_finite() => format!("{f}"),
        _ => String::new(),
    }
}

/// Text of a YouTube text object: `simpleText`, `content`, `runs[]` or a plain
/// string/number. Anything else is the empty string.
pub fn text(x: Option<&Value>) -> String {
    match x {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => js_number_string(n),
        Some(Value::Object(m)) => {
            if let Some(Value::String(s)) = m.get("simpleText") {
                return s.clone();
            }
            if let Some(Value::String(s)) = m.get("content") {
                return s.clone();
            }
            if let Some(Value::Array(runs)) = m.get("runs") {
                let mut out = String::new();
                for r in runs.iter().take(2000) {
                    if let Some(Value::String(t)) = r.get("text") {
                        out.push_str(t);
                    }
                }
                return out;
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// `text(a)` or, when empty, `text(b)`.
fn text_or(a: Option<&Value>, b: Option<&Value>) -> String {
    let t = text(a);
    if t.is_empty() {
        text(b)
    } else {
        t
    }
}

// ---- walk -------------------------------------------------------------------

/// What a [`walk`] visitor wants next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visit {
    /// Descend into the node's children.
    Continue,
    /// Do not descend into this node.
    SkipChildren,
    /// End the whole walk.
    Stop,
}

/// Iterative pre-order walk over every object/array node with the default caps
/// ([`MAX_NODES`], [`MAX_DEPTH`]). Returns the number of nodes visited.
pub fn walk<'a>(root: &'a Value, visit: impl FnMut(&'a Value) -> Visit) -> usize {
    walk_with(root, visit, MAX_NODES, MAX_DEPTH)
}

/// [`walk`] with explicit caps. A node at `max_depth` is visited but its
/// children are not.
pub fn walk_with<'a>(root: &'a Value, mut visit: impl FnMut(&'a Value) -> Visit, max_nodes: usize, max_depth: usize) -> usize {
    if !is_obj(root) {
        return 0;
    }
    let mut count = 0usize;
    let mut stack: Vec<(&'a Value, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if count > max_nodes {
            break;
        }
        match visit(node) {
            Visit::Stop => break,
            Visit::SkipChildren => continue,
            Visit::Continue => {}
        }
        if depth >= max_depth {
            continue;
        }
        match node {
            Value::Array(a) => {
                for c in a.iter().rev() {
                    if is_obj(c) {
                        stack.push((c, depth + 1));
                    }
                }
            }
            Value::Object(m) => {
                for c in m.values().rev() {
                    if is_obj(c) {
                        stack.push((c, depth + 1));
                    }
                }
            }
            _ => {}
        }
    }
    count
}

/// String values stored under any of `keys` anywhere in a small subtree.
fn collect_strings(root: &Value, keys: &[&str], cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk_with(
        root,
        |n| {
            if let Value::Object(m) = n {
                for k in keys {
                    if let Some(Value::String(s)) = m.get(*k) {
                        out.push(s.clone());
                    }
                }
                if out.len() >= cap {
                    return Visit::Stop;
                }
            }
            Visit::Continue
        },
        2000,
        12,
    );
    out
}

// ---- durations and counts -----------------------------------------------------

/// `"1:02:03"` -> 3723, `"12:34"` -> 754, `"12345"` -> 12345, otherwise `None`.
pub fn parse_duration_str(s: &str) -> Option<u32> {
    let t: String = s.chars().filter(|c| !is_js_space(*c) && *c != '\u{200e}' && *c != '\u{200f}').collect();
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if digits(&t) && t.len() < 9 {
        return t.parse().ok();
    }
    let parts: Vec<&str> = t.split(':').collect();
    let ok = |p: &str, max: usize| digits(p) && p.len() <= max;
    match parts.as_slice() {
        [a, b] if ok(a, 3) && ok(b, 2) => Some(a.parse::<u32>().ok()? * 60 + b.parse::<u32>().ok()?),
        [a, b, c] if ok(a, 3) && ok(b, 2) && ok(c, 2) => Some(a.parse::<u32>().ok()? * 3600 + b.parse::<u32>().ok()? * 60 + c.parse::<u32>().ok()?),
        _ => None,
    }
}

/// [`parse_duration_str`] for strings; non-negative finite numbers pass through.
pub fn parse_duration(v: &Value) -> Option<u32> {
    match v {
        Value::Number(n) => {
            let f = n.as_f64()?;
            if f.is_finite() && f >= 0.0 {
                u32::try_from(f.floor() as u64).ok()
            } else {
                None
            }
        }
        Value::String(s) => parse_duration_str(s),
        _ => None,
    }
}

/// JS `parseFloat` restricted to the `\d*(\.\d*)?` prefix.
fn parse_float_prefix(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut end = 0;
    let mut seen = false;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
        seen = true;
    }
    if end < b.len() && b[end] == b'.' {
        end += 1;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
            seen = true;
        }
    }
    if !seen {
        return None;
    }
    s[..end].parse::<f64>().ok().filter(|f| f.is_finite())
}

/// `"1,234"` -> 1234, `"1.2K"` -> 1200, `"3.4M views"` -> 3400000, `"No views"`
/// -> 0, `""` -> `None`. Mirrors the JS regex including its backtracking.
pub fn parse_count_str(s: &str) -> Option<u64> {
    let t = s.trim_matches(is_js_space);
    if t.is_empty() {
        return None;
    }
    let ch: Vec<char> = t.chars().collect();
    let in_class = |c: char| c.is_ascii_digit() || c == '.' || c == ',' || is_js_space(c);
    let run_end = ch.iter().position(|&c| !in_class(c)).unwrap_or(ch.len());
    // /^([\d.,\s]*\d)\s*([KkMmBb])?(?![A-Za-z])/ with backtracking
    let mut found: Option<(usize, Option<char>)> = None;
    'outer: for e in (0..run_end).rev() {
        if !ch[e].is_ascii_digit() {
            continue;
        }
        let pos = e + 1;
        let mut ws_end = pos;
        while ws_end < ch.len() && is_js_space(ch[ws_end]) {
            ws_end += 1;
        }
        for q in (pos..=ws_end).rev() {
            let next = ch.get(q).copied();
            if let Some(c) = next {
                if "KkMmBb".contains(c) {
                    let after = ch.get(q + 1).copied();
                    if !after.is_some_and(|a| a.is_ascii_alphabetic()) {
                        found = Some((e, Some(c)));
                        break 'outer;
                    }
                }
            }
            if !next.is_some_and(|c| c.is_ascii_alphabetic()) {
                found = Some((e, None));
                break 'outer;
            }
        }
    }
    let Some((e, suffix)) = found else {
        let lower: Vec<char> = ch.iter().take(3).map(|c| c.to_ascii_lowercase()).collect();
        return if lower.len() == 3 && lower[0] == 'n' && lower[1] == 'o' && is_js_space(lower[2]) { Some(0) } else { None };
    };
    let group = &ch[..=e];
    if let Some(sfx) = suffix {
        let cleaned: String = group.iter().filter(|c| !is_js_space(**c)).collect();
        let cleaned = cleaned.replacen(',', ".", 1);
        let n = parse_float_prefix(&cleaned)?;
        let mult = match sfx.to_ascii_uppercase() {
            'K' => 1e3,
            'M' => 1e6,
            _ => 1e9,
        };
        return Some((n * mult).round() as u64);
    }
    let digits: String = group.iter().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 15 {
        return None;
    }
    digits.parse().ok()
}

/// [`parse_count_str`] for strings; non-negative finite numbers pass through.
pub fn parse_count(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => {
            let f = n.as_f64()?;
            if f.is_finite() && f >= 0.0 {
                Some(f.floor() as u64)
            } else {
                None
            }
        }
        Value::String(s) => parse_count_str(s),
        _ => None,
    }
}

/// Non-negative integer from a number or a string of 1-15 digits.
fn to_int(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                return Some(u);
            }
            let f = n.as_f64()?;
            if f.is_finite() && f >= 0.0 {
                Some(f.floor() as u64)
            } else {
                None
            }
        }
        Value::String(s) if !s.is_empty() && s.len() <= 15 && s.bytes().all(|b| b.is_ascii_digit()) => s.parse().ok(),
        _ => None,
    }
}

fn to_u32(v: Option<&Value>) -> Option<u32> {
    to_int(v).and_then(|n| u32::try_from(n).ok())
}

// ---- thumbnails ---------------------------------------------------------------

/// `https://i.ytimg.com/vi/<id>/mqdefault.jpg`
pub fn thumb_url(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
}

/// Smallest thumbnail at least 320px wide, else the largest; falls back to the
/// built mqdefault url when `id` is known.
pub fn pick_thumbnail(list: Option<&Value>, id: Option<&str>) -> Option<String> {
    if let Some(Value::Array(list)) = list {
        let mut best: Option<(&str, f64)> = None;
        let mut big: Option<(&str, f64)> = None;
        for t in list.iter().take(50) {
            let u = if truthy(t.get("url")) { t.get("url") } else { t.get("src") };
            let Some(u) = u.and_then(Value::as_str).filter(|u| !u.is_empty()) else { continue };
            let w = t.get("width").and_then(Value::as_f64).unwrap_or(0.0);
            if w >= 320.0 && best.is_none_or(|(_, bw)| w < bw) {
                best = Some((u, w));
            }
            if big.is_none_or(|(_, bw)| w >= bw) {
                big = Some((u, w));
            }
        }
        if let Some((pick, _)) = best.or(big) {
            let pick = if pick.starts_with("//") { format!("https:{pick}") } else { pick.to_string() };
            let lower = pick.get(..8).map(str::to_ascii_lowercase).unwrap_or_default();
            if lower.starts_with("https://") || lower.starts_with("http://") {
                return Some(pick);
            }
        }
    }
    id.filter(|i| !i.is_empty()).map(thumb_url)
}

// ---- item builders --------------------------------------------------------------

fn browse_id_of(text_obj: Option<&Value>) -> Option<String> {
    text_obj.and_then(|t| g_str(t, &["runs", "0", "navigationEndpoint", "browseEndpoint", "browseId"])).filter(|s| !s.is_empty()).map(str::to_string)
}

fn item_from_video_renderer(r: &Value, allow_shorts: bool) -> Option<Item> {
    if !is_obj(r) {
        return None;
    }
    let id = valid_id(r.get("videoId"))?;
    if r.get("isPlayable") == Some(&Value::Bool(false)) {
        return None;
    }
    let mut title = clip(&text_or(r.get("title"), r.get("headline")), 500);
    if title.is_empty() {
        title = clip(g_str(r, &["title", "accessibility", "accessibilityData", "label"]).unwrap_or(""), 500);
    }
    let byline = ["ownerText", "longBylineText", "shortBylineText", "channelName"].iter().map(|k| r.get(*k)).find(|v| truthy(*v)).flatten();
    let channel = clip(&text(byline), 200);
    let channel_id = browse_id_of(r.get("ownerText")).or_else(|| browse_id_of(r.get("longBylineText"))).or_else(|| browse_id_of(r.get("shortBylineText")));

    let mut duration: Option<u32> = None;
    let mut live = false;
    let mut shorts = false;
    if let Some(Value::Array(overlays)) = r.get("thumbnailOverlays") {
        for o in overlays.iter().take(20) {
            let Some(ts) = o.get("thumbnailOverlayTimeStatusRenderer").filter(|t| truthy(Some(t))) else { continue };
            let style = ts.get("style").and_then(Value::as_str).unwrap_or("").to_uppercase();
            if style == "LIVE" {
                live = true;
            }
            if style == "SHORTS" {
                shorts = true;
            }
            if duration.is_none() {
                duration = parse_duration_str(&text(ts.get("text")));
            }
        }
    }
    let length_text = text(r.get("lengthText"));
    if duration.is_none() && !length_text.is_empty() {
        duration = parse_duration_str(&length_text);
    }
    if duration.is_none() {
        if let Some(ls) = r.get("lengthSeconds") {
            let s = match ls {
                Value::Number(n) => js_number_string(n),
                Value::String(s) => s.clone(),
                _ => String::new(),
            };
            duration = parse_duration_str(&s);
        }
    }
    if shorts && !allow_shorts {
        return None;
    }

    let badge = if live || r.get("isLive") == Some(&Value::Bool(true)) {
        Some("LIVE".to_string())
    } else if r.get("upcomingEventData").is_some_and(is_obj) {
        Some("UPCOMING".to_string())
    } else if shorts {
        Some("SHORT".to_string())
    } else {
        let mut b = None;
        if let Some(Value::Array(badges)) = r.get("badges") {
            for badge in badges.iter().take(10) {
                if let Some(label) = g_str(badge, &["metadataBadgeRenderer", "label"]).filter(|l| !l.is_empty()) {
                    b = ne(clip(label, 40));
                    break;
                }
            }
        }
        b
    };

    Some(Item {
        video_id: id.to_string(),
        title,
        channel: ne(channel),
        channel_id,
        duration_seconds: duration,
        thumbnail: pick_thumbnail(g(r, &["thumbnail", "thumbnails"]), Some(id)),
        published_text: ne(clip(&text(r.get("publishedTimeText")), 100)),
        view_text: ne(clip(&text_or(r.get("viewCountText"), r.get("shortViewCountText")), 100)),
        badge,
    })
}

fn item_from_reel(r: &Value, allow_shorts: bool) -> Option<Item> {
    if !allow_shorts || !is_obj(r) {
        return None;
    }
    let id = valid_id(r.get("videoId"))?;
    Some(Item {
        video_id: id.to_string(),
        title: clip(&text_or(r.get("headline"), r.get("title")), 500),
        thumbnail: pick_thumbnail(g(r, &["thumbnail", "thumbnails"]), Some(id)),
        view_text: ne(clip(&text(r.get("viewCountText")), 100)),
        badge: Some("SHORT".to_string()),
        ..Item::default()
    })
}

/// `/^UC[\w-]{10,}$/`
fn is_channel_id(s: &str) -> bool {
    s.len() >= 12 && s.starts_with("UC") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn item_from_lockup(r: &Value) -> Option<Item> {
    if !is_obj(r) {
        return None;
    }
    if r.get("contentType").and_then(Value::as_str) != Some("LOCKUP_CONTENT_TYPE_VIDEO") {
        return None;
    }
    let id = valid_id(r.get("contentId"))?;
    let meta = g(r, &["metadata", "lockupMetadataViewModel"]);
    let title = clip(&text(meta.and_then(|m| m.get("title"))), 500);

    let mut parsed_rows: Vec<Vec<String>> = Vec::new();
    if let Some(Value::Array(rows)) = meta.and_then(|m| g(m, &["metadata", "contentMetadataViewModel", "metadataRows"])) {
        for row in rows.iter().take(6) {
            let mut texts = Vec::new();
            if let Some(Value::Array(parts)) = row.get("metadataParts") {
                for part in parts.iter().take(6) {
                    let tx = clip(&text(part.get("text")), 200);
                    if !tx.is_empty() {
                        texts.push(tx);
                    }
                }
            }
            if !texts.is_empty() {
                parsed_rows.push(texts);
            }
        }
    }
    let mut channel: Option<String> = None;
    let mut stats_row: Option<&Vec<String>> = None;
    if parsed_rows.len() >= 2 {
        channel = parsed_rows[0].first().cloned();
        stats_row = Some(&parsed_rows[1]);
    } else if parsed_rows.len() == 1 {
        stats_row = Some(&parsed_rows[0]);
    }
    let view_text = stats_row.and_then(|s| s.first().cloned());
    let published = stats_row.and_then(|s| s.get(1).cloned());

    let mut channel_id: Option<String> = None;
    if let Some(meta) = meta {
        walk_with(
            meta,
            |n| {
                if let Some(Value::String(b)) = n.get("browseId") {
                    if is_channel_id(b) {
                        channel_id = Some(b.clone());
                        return Visit::Stop;
                    }
                }
                Visit::Continue
            },
            3000,
            25,
        );
    }

    let mut duration: Option<u32> = None;
    let mut badge: Option<String> = None;
    if let Some(overlays @ Value::Array(_)) = g(r, &["contentImage", "thumbnailViewModel", "overlays"]) {
        for s in collect_strings(overlays, &["text", "simpleText"], 20) {
            if duration.is_none() {
                duration = parse_duration_str(&s);
            }
            let t = s.trim_matches(is_js_space);
            if badge.is_none() && ["LIVE", "PREMIERE", "UPCOMING"].iter().any(|k| t.eq_ignore_ascii_case(k)) {
                badge = Some(t.to_ascii_uppercase());
            }
        }
    }

    Some(Item {
        video_id: id.to_string(),
        title,
        channel,
        channel_id,
        duration_seconds: duration,
        thumbnail: pick_thumbnail(g(r, &["contentImage", "thumbnailViewModel", "image", "sources"]), Some(id)),
        published_text: published,
        view_text,
        badge,
    })
}

const RENDERER_KEYS: [&str; 6] = ["videoRenderer", "compactVideoRenderer", "gridVideoRenderer", "playlistVideoRenderer", "reelItemRenderer", "lockupViewModel"];

/// Extract every video from a YouTube JSON document, in page order,
/// deduplicated by video id (first occurrence wins).
pub fn extract_feed(json: &Value, include_shorts: bool) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    walk(json, |node| {
        let Value::Object(m) = node else { return Visit::Continue };
        // Never turn nested advertised videos into cached recommendations or
        // rule-driven downloads. Skip the ad subtree before video deduplication.
        if ["adSlotRenderer", "inFeedAdLayoutRenderer", "displayAdRenderer",
            "promotedVideoRenderer", "promotedSparklesWebRenderer",
            "promotedSparklesTextSearchRenderer", "searchPyvRenderer",
            "actionCompanionAdRenderer", "bannerPromoRenderer", "adPlacementRenderer"]
            .iter().any(|key| m.contains_key(*key)) {
            return Visit::SkipChildren;
        }
        let mut matched = false;
        for key in RENDERER_KEYS {
            let Some(v) = m.get(key).filter(|v| is_obj(v)) else { continue };
            matched = true;
            let it = match key {
                "lockupViewModel" => item_from_lockup(v),
                "reelItemRenderer" => item_from_reel(v, include_shorts),
                _ => item_from_video_renderer(v, include_shorts),
            };
            if let Some(it) = it {
                if seen.insert(it.video_id.clone()) {
                    items.push(it);
                }
            }
        }
        // a matched renderer is fully consumed; do not descend into it
        if matched {
            Visit::SkipChildren
        } else {
            Visit::Continue
        }
    });
    items
}

// ---- video metadata ---------------------------------------------------------------

fn description_of(secondary: &Value) -> String {
    let d = text_or(secondary.get("attributedDescription"), secondary.get("description"));
    take_chars(&d, 5000)
}

/// Accepts a player response (`videoDetails`) and/or a `next` response
/// (`videoPrimaryInfoRenderer` / `videoSecondaryInfoRenderer`). Returns a
/// [`VideoIngest`] (without `captured_at`) or `None` when there is no video id
/// or title. A `next` response also yields `related`.
pub fn extract_video_meta(json: &Value) -> Option<VideoIngest> {
    if !is_obj(json) {
        return None;
    }
    let mut video_id: Option<String> = None;
    let mut title = String::new();
    let mut channel = String::new();
    let mut channel_id = String::new();
    let mut duration: Option<u32> = None;
    let mut description = String::new();
    let mut thumbnail: Option<String> = None;
    let mut view_count: Option<u64> = None;
    let mut published_text = String::new();
    let mut is_live: Option<bool> = None;

    let vd = json.get("videoDetails").filter(|v| is_obj(v));
    if let Some(vd) = vd {
        let mf = g(json, &["microformat", "playerMicroformatRenderer"]);
        video_id = valid_id(vd.get("videoId")).map(str::to_string);
        title = clip(vd.get("title").and_then(Value::as_str).unwrap_or(""), 500);
        channel = clip(vd.get("author").and_then(Value::as_str).unwrap_or(""), 200);
        channel_id = vd.get("channelId").and_then(Value::as_str).unwrap_or("").to_string();
        duration = to_u32(vd.get("lengthSeconds"));
        description = take_chars(vd.get("shortDescription").and_then(Value::as_str).unwrap_or(""), 5000);
        thumbnail = pick_thumbnail(g(vd, &["thumbnail", "thumbnails"]), video_id.as_deref());
        view_count = to_int(vd.get("viewCount"));
        if let Some(mf) = mf.filter(|m| is_obj(m)) {
            published_text = clip(mf.get("publishDate").and_then(Value::as_str).unwrap_or(""), 100);
        }
        if let Some(Value::Bool(b)) = vd.get("isLive") {
            is_live = Some(*b);
        }
    }

    let mut primary: Option<&Value> = None;
    let mut secondary: Option<&Value> = None;
    let mut current: Option<&Value> = None;
    walk(json, |n| {
        if let Value::Object(m) = n {
            if primary.is_none() {
                primary = m.get("videoPrimaryInfoRenderer").filter(|v| is_obj(v));
            }
            if secondary.is_none() {
                secondary = m.get("videoSecondaryInfoRenderer").filter(|v| is_obj(v));
            }
            if current.is_none() {
                current = g(n, &["currentVideoEndpoint", "watchEndpoint"]).filter(|v| is_obj(v));
            }
        }
        if primary.is_some() && secondary.is_some() && current.is_some() {
            Visit::Stop
        } else {
            Visit::Continue
        }
    });

    if video_id.is_none() {
        if let Some(c) = current {
            video_id = valid_id(c.get("videoId")).map(str::to_string);
        }
    }
    if let Some(p) = primary {
        if title.is_empty() {
            title = clip(&text(p.get("title")), 500);
        }
        if view_count.is_none() {
            view_count = parse_count_str(&text(g(p, &["viewCount", "videoViewCountRenderer", "viewCount"])));
        }
        if published_text.is_empty() {
            published_text = clip(&text(p.get("dateText")), 100);
        }
        if is_live.is_none() {
            if let Some(vcr) = g(p, &["viewCount", "videoViewCountRenderer"]).filter(|v| is_obj(v)) {
                if vcr.get("isLive") == Some(&Value::Bool(true)) {
                    is_live = Some(true);
                }
            }
        }
    }
    if let Some(s) = secondary {
        let owner = g(s, &["owner", "videoOwnerRenderer"]);
        if channel.is_empty() {
            channel = clip(&text(owner.and_then(|o| o.get("title"))), 200);
        }
        if channel_id.is_empty() {
            if let Some(cid) = owner.and_then(|o| g_str(o, &["title", "runs", "0", "navigationEndpoint", "browseEndpoint", "browseId"])) {
                channel_id = cid.to_string();
            }
        }
        if description.is_empty() {
            description = description_of(s);
        }
    }

    let video_id = video_id.filter(|v| !v.is_empty())?;
    if title.is_empty() {
        return None;
    }
    let thumbnail = thumbnail.or_else(|| Some(thumb_url(&video_id)));
    let related = if vd.is_none() { extract_feed(json, false).into_iter().filter(|it| it.video_id != video_id).collect() } else { Vec::new() };

    Some(VideoIngest {
        video_id,
        title,
        channel: ne(channel),
        channel_id: ne(channel_id),
        duration_seconds: duration,
        description: ne(description),
        thumbnail,
        view_count,
        published_text: ne(published_text),
        is_live,
        related,
        captured_at: None,
    })
}

// ---- formats ---------------------------------------------------------------------------

/// One entry of a player response's `streamingData`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Format {
    pub itag: i64,
    /// Mime type without the codecs parameter, e.g. `video/mp4`.
    pub mime: Option<String>,
    /// `"1080p"`, or `"audio:medium"` for audio-only formats.
    pub quality_label: Option<String>,
    pub height: Option<u32>,
    pub bitrate: Option<u64>,
    pub content_length: Option<u64>,
}

/// The itag table of a player response (`formats` then `adaptiveFormats`).
pub fn extract_formats(json: &Value) -> Vec<Format> {
    let mut out = Vec::new();
    let Some(sd) = json.get("streamingData").filter(|v| is_obj(v)) else { return out };
    for key in ["formats", "adaptiveFormats"] {
        let Some(Value::Array(list)) = sd.get(key) else { continue };
        for f in list.iter().take(100) {
            if !is_obj(f) {
                continue;
            }
            let Some(itag) = to_int(f.get("itag")).and_then(|i| i64::try_from(i).ok()) else { continue };
            let mut label = f.get("qualityLabel").and_then(Value::as_str).unwrap_or("").to_string();
            if label.is_empty() {
                if let Some(aq) = f.get("audioQuality").and_then(Value::as_str) {
                    label = format!("audio:{}", aq.strip_prefix("AUDIO_QUALITY_").unwrap_or(aq).to_lowercase());
                }
            }
            out.push(Format {
                itag,
                mime: f.get("mimeType").and_then(Value::as_str).and_then(|m| ne(m.split(';').next().unwrap_or("").trim_matches(is_js_space).to_string())),
                quality_label: ne(label),
                height: to_u32(f.get("height")),
                bitrate: to_int(f.get("bitrate")),
                content_length: to_int(f.get("contentLength")),
            });
        }
    }
    out
}

// ---- comments --------------------------------------------------------------------------

/// Comments of one response and the total count when the response carried it.
#[derive(Debug, Clone, Default)]
pub struct ExtractedComments {
    pub comments: Vec<Comment>,
    pub total: Option<u64>,
}

/// De-duplication key: the comment id, else author plus the first 120 chars.
pub fn comment_key(c: &Comment) -> String {
    match c.id.as_deref().filter(|i| !i.is_empty()) {
        Some(id) => format!("i:{id}"),
        None => format!("t:{}\u{0}{}", c.author, take_chars(&c.text, 120)),
    }
}

fn comment_from_renderer(c: &Value, is_reply: bool) -> Option<Comment> {
    if !is_obj(c) {
        return None;
    }
    let body = text(c.get("contentText"));
    let author = clip(&text(c.get("authorText")), 200);
    if body.is_empty() && author.is_empty() {
        return None;
    }
    let votes = text(c.get("voteCount"));
    let likes = if votes.is_empty() { Some(0) } else { parse_count_str(&votes) };
    Some(Comment {
        id: c.get("commentId").and_then(Value::as_str).and_then(|s| ne(s.to_string())),
        author,
        text: take_chars(&body, 4000),
        likes,
        published_text: ne(clip(&text(c.get("publishedTimeText")), 100)),
        reply_count: to_int(c.get("replyCount")),
        is_reply: is_reply.then_some(true),
    })
}

fn comment_from_entity(p: &Value) -> Option<Comment> {
    if !is_obj(p) {
        return None;
    }
    let props = p.get("properties");
    let prop = |k: &str| props.and_then(|x| x.get(k));
    let body = text(prop("content"));
    let author = clip(&text(g(p, &["author", "displayName"])), 200);
    if body.is_empty() && author.is_empty() {
        return None;
    }
    let tb = p.get("toolbar");
    let likes = match tb.and_then(|t| t.get("likeCountNotliked")) {
        Some(Value::String(s)) if s.is_empty() => Some(0),
        Some(Value::String(s)) => parse_count_str(s),
        _ => None,
    };
    let level = prop("replyLevel").and_then(|v| if v.is_number() { v.as_f64() } else { None }).unwrap_or(0.0);
    Some(Comment {
        id: prop("commentId").and_then(Value::as_str).and_then(|s| ne(s.to_string())),
        author,
        text: take_chars(&body, 4000),
        likes,
        published_text: ne(clip(&text(prop("publishedTime")), 100)),
        reply_count: tb.and_then(|t| t.get("replyCount")).and_then(Value::as_str).and_then(parse_count_str),
        is_reply: (level > 0.0).then_some(true),
    })
}

/// Handles `commentThreadRenderer` -> `comment.commentRenderer`, stand-alone
/// `commentRenderer` (replies) and the newer entity-batch payloads
/// (`commentViewModel` order + `commentEntityPayload` mutations).
pub fn extract_comments(json: &Value) -> ExtractedComments {
    let mut list: Vec<Comment> = Vec::new();
    let mut total: Option<u64> = None;
    let mut handled: Vec<&Value> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut entities: HashMap<String, Comment> = HashMap::new();
    let mut entity_order: Vec<String> = Vec::new();

    walk(json, |n| {
        let Value::Object(m) = n else { return Visit::Continue };
        if let Some(thread) = m.get("commentThreadRenderer").filter(|t| is_obj(t)) {
            if let Some(cr) = g(thread, &["comment", "commentRenderer"]).filter(|c| is_obj(c)) {
                handled.push(cr);
                if let Some(it) = comment_from_renderer(cr, false) {
                    list.push(it);
                }
            }
        }
        if let Some(cr2) = m.get("commentRenderer").filter(|c| is_obj(c)) {
            if !handled.iter().any(|h| std::ptr::eq(*h, cr2)) {
                if let Some(rep) = comment_from_renderer(cr2, true) {
                    list.push(rep);
                }
            }
        }
        if let Some(vm) = m.get("commentViewModel").filter(|v| is_obj(v)) {
            let inner = vm.get("commentViewModel").filter(|v| is_obj(v)).unwrap_or(vm);
            if let Some(Value::String(k)) = inner.get("commentKey") {
                order.push(k.clone());
            }
        }
        if let Some(payload) = g(n, &["payload", "commentEntityPayload"]).filter(|p| is_obj(p)) {
            let ekey = m.get("entityKey").and_then(Value::as_str).or_else(|| payload.get("key").and_then(Value::as_str)).map(str::to_string);
            if let Some(ent) = comment_from_entity(payload) {
                let ekey = ekey.unwrap_or_else(|| format!("anon:{}", entity_order.len()));
                if !entities.contains_key(&ekey) {
                    entities.insert(ekey.clone(), ent);
                    entity_order.push(ekey);
                }
            }
        }
        if let Some(header) = m.get("commentsHeaderRenderer").filter(|h| is_obj(h)) {
            if total.is_none() {
                total = parse_count_str(&text_or(header.get("countText"), header.get("commentsCount")));
            }
        }
        if total.is_none() {
            if let Some(ep) = m.get("commentsEntryPointHeaderRenderer").filter(|h| is_obj(h)) {
                total = parse_count_str(&text(ep.get("commentCount")));
            }
        }
        Visit::Continue
    });

    let mut used: HashSet<&str> = HashSet::new();
    for k in &order {
        if let Some(c) = entities.get(k) {
            if used.insert(k.as_str()) {
                list.push(c.clone());
            }
        }
    }
    for k in &entity_order {
        if !used.contains(k.as_str()) {
            if let Some(c) = entities.get(k) {
                list.push(c.clone());
            }
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let comments = list.into_iter().filter(|c| seen.insert(comment_key(c))).collect();
    ExtractedComments { comments, total }
}

// ---- URL helpers ----------------------------------------------------------------------------------

/// A parsed absolute URL (`scheme://[userinfo@]host[:port]/path?query#fragment`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// Lowercased scheme.
    pub scheme: String,
    /// Lowercased host without userinfo and port.
    pub host: String,
    pub port: Option<u16>,
    /// Path, `"/"` when empty; still percent-encoded.
    pub path: String,
    /// Raw query without the leading `?`.
    pub query: String,
}

/// Parse an absolute `scheme://host...` URL. `None` for anything else.
pub fn parse_url(input: &str) -> Option<Url> {
    let s = input.trim_matches(|c: char| c <= ' ');
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    if scheme.is_empty() || !scheme.as_bytes()[0].is_ascii_alphabetic() || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
        return None;
    }
    let rest = s[colon + 1..].strip_prefix("//")?;
    let rest = rest.split('#').next().unwrap_or("");
    let (before_query, query) = match rest.find('?') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let auth_end = before_query.find('/').unwrap_or(before_query.len());
    let (authority, path) = before_query.split_at(auth_end);
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let (host, port_str) = if hostport.starts_with('[') {
        let end = hostport.find(']')?;
        (&hostport[..=end], hostport[end + 1..].strip_prefix(':').unwrap_or(""))
    } else {
        match hostport.rfind(':') {
            Some(i) => (&hostport[..i], &hostport[i + 1..]),
            None => (hostport, ""),
        }
    };
    if host.is_empty() {
        return None;
    }
    let port = if port_str.is_empty() {
        None
    } else if port_str.bytes().all(|b| b.is_ascii_digit()) {
        Some(port_str.parse::<u16>().ok()?)
    } else {
        return None;
    };
    Some(Url {
        scheme: scheme.to_ascii_lowercase(),
        host: host.to_ascii_lowercase(),
        port,
        path: if path.is_empty() { "/".to_string() } else { path.to_string() },
        query: query.to_string(),
    })
}

fn hex_val(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Percent-decode (lossy UTF-8); malformed `%` sequences stay literal.
pub fn percent_decode(s: &str, plus_as_space: bool) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() && hex_val(b[i + 1]).is_some() && hex_val(b[i + 2]).is_some() => {
                out.push(hex_val(b[i + 1]).unwrap_or(0) * 16 + hex_val(b[i + 2]).unwrap_or(0));
                i += 3;
                continue;
            }
            b'+' if plus_as_space => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `application/x-www-form-urlencoded` query parsing (like `URLSearchParams`):
/// `+` is a space, empty segments are skipped, a key without `=` has value `""`.
pub fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let (k, v) = seg.split_once('=').unwrap_or((seg, ""));
            (percent_decode(k, true), percent_decode(v, true))
        })
        .collect()
}

// ---- request / page classification -------------------------------------------------------

/// What kind of page or feed a request belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Home,
    Subscriptions,
    Search,
    Related,
    Channel,
    Other,
    /// A `player` call (not a feed).
    Video,
}

impl Kind {
    /// The `kind` string used by `FeedIngest` (`"video"` for [`Kind::Video`]).
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Home => "home",
            Kind::Subscriptions => "subscriptions",
            Kind::Search => "search",
            Kind::Related => "related",
            Kind::Channel => "channel",
            Kind::Other => "other",
            Kind::Video => "video",
        }
    }
}

/// A page URL reduced to what feed classification needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageContext {
    pub kind: Kind,
    pub query: Option<String>,
    pub video_id: Option<String>,
}

fn is_youtube_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "youtube.com" || h.ends_with(".youtube.com")
}

/// Page URL -> kind, search query and video id. Non-YouTube or unparseable
/// URLs are `Kind::Other`.
pub fn page_context(page_url: &str) -> PageContext {
    let mut res = PageContext { kind: Kind::Other, query: None, video_id: None };
    let Some(u) = parse_url(page_url) else { return res };
    if !is_youtube_host(&u.host) {
        return res;
    }
    let p = u.path.as_str();
    let q = parse_query(&u.query);
    let param = |k: &str| q.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let live_or_embed_id = || -> Option<&str> {
        let rest = p.strip_prefix("/live/").or_else(|| p.strip_prefix("/embed/"))?;
        valid_video_id(rest).then_some(rest)
    };
    if p == "/" || p.is_empty() {
        res.kind = Kind::Home;
    } else if p == "/feed/subscriptions" {
        res.kind = Kind::Subscriptions;
    } else if p == "/results" {
        res.kind = Kind::Search;
        res.query = param("search_query").filter(|s| !s.is_empty()).map(str::to_string);
    } else if p == "/watch" {
        res.kind = Kind::Related;
        res.video_id = param("v").filter(|v| valid_video_id(v)).map(str::to_string);
    } else if let Some(id) = live_or_embed_id() {
        res.kind = Kind::Related;
        res.video_id = Some(id.to_string());
    } else if p.starts_with("/@") || p.starts_with("/channel/") || p.starts_with("/c/") || p.starts_with("/user/") {
        res.kind = Kind::Channel;
    }
    res
}

/// The video id of a `/watch?v=`, `/live/<id>` or `/embed/<id>` page URL.
pub fn video_id_from_url(page_url: &str) -> Option<String> {
    page_context(page_url).video_id
}

/// Result of [`classify_request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    /// `browse`, `search`, `next` or `player`; `None` when the URL is not an
    /// interesting youtubei call.
    pub endpoint: Option<String>,
    pub kind: Option<Kind>,
    pub browse_id: Option<String>,
    pub query: Option<String>,
    pub video_id: Option<String>,
    pub continuation: bool,
}

/// Find `/youtubei/v1/<name>` followed by `?`, `/`, `#` or the end.
fn youtubei_endpoint(url: &str) -> Option<&str> {
    const PREFIX: &str = "/youtubei/v1/";
    let mut from = 0;
    while let Some(rel) = url[from..].find(PREFIX) {
        let start = from + rel + PREFIX.len();
        let name_len = url[start..].bytes().take_while(|b| b.is_ascii_lowercase() || *b == b'_').count();
        let end = start + name_len;
        if name_len > 0 && url[end..].bytes().next().is_none_or(|b| matches!(b, b'?' | b'/' | b'#')) {
            return Some(&url[start..end]);
        }
        from += rel + 1;
    }
    None
}

/// Full classification of a youtubei request. `body` is the parsed JSON
/// request body (anything that is not an object contributes nothing).
pub fn classify_request(url: &str, body: &Value, page_url: Option<&str>) -> Classified {
    let mut out = Classified { endpoint: None, kind: None, browse_id: None, query: None, video_id: None, continuation: false };
    let Some(ep) = youtubei_endpoint(url) else { return out };
    out.endpoint = Some(ep.to_string());
    out.browse_id = body.get("browseId").and_then(Value::as_str).map(str::to_string);
    out.query = body.get("query").and_then(Value::as_str).map(str::to_string);
    out.video_id = valid_id(body.get("videoId")).map(str::to_string);
    out.continuation = body.get("continuation").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
    let page = page_url.map(page_context).unwrap_or(PageContext { kind: Kind::Other, query: None, video_id: None });
    match ep {
        "browse" => {
            let id = out.browse_id.as_deref().filter(|s| !s.is_empty());
            out.kind = Some(match id {
                Some("FEwhat_to_watch") => Kind::Home,
                Some("FEsubscriptions") => Kind::Subscriptions,
                Some(id) if is_channel_id(id) || id.starts_with("channel") => Kind::Channel,
                None if out.continuation => page.kind,
                _ => Kind::Other,
            });
        }
        "search" => {
            out.kind = Some(Kind::Search);
            if out.query.as_deref().is_none_or(str::is_empty) && page.kind == Kind::Search {
                out.query = page.query;
            }
        }
        "next" => {
            out.kind = Some(Kind::Related);
            if out.video_id.is_none() {
                out.video_id = page.video_id;
            }
        }
        "player" => out.kind = Some(Kind::Video),
        _ => out.endpoint = None,
    }
    out
}

/// [`classify_request`] with the raw request body bytes (JSON). Unparseable or
/// oversized bodies count as empty.
pub fn classify_request_bytes(url: &str, body: Option<&[u8]>, page_url: Option<&str>) -> Classified {
    let parsed = body.and_then(parse_request_body).unwrap_or(Value::Null);
    classify_request(url, &parsed, page_url)
}

/// Feed kind of a request (`None` when it is not an interesting youtubei call).
pub fn classify(url: &str, body: &Value, page_url: Option<&str>) -> Option<Kind> {
    classify_request(url, body, page_url).kind
}

// ---- comment continuations -----------------------------------------------------------------------

/// What a comment continuation token loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContinuationKind {
    /// Found in a watch page / `next` response: loads the first page of comments.
    Section,
    /// Found next to loaded comments: loads the next page (of comments or replies).
    Page,
    /// "N replies" of one comment thread.
    Replies,
    /// A sort-order choice (top / newest) from the comments header.
    Sort,
}

/// One continuation token, to be sent as `{"continuation": token}` to `/youtubei/v1/next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentContinuation {
    pub token: String,
    pub kind: ContinuationKind,
}

/// First continuation token anywhere in a small subtree
/// (`continuationCommand.token` or `nextContinuationData.continuation`).
fn continuation_token(v: &Value) -> Option<String> {
    let mut tok: Option<String> = None;
    walk_with(
        v,
        |n| {
            let t = g_str(n, &["continuationCommand", "token"]).or_else(|| g_str(n, &["nextContinuationData", "continuation"]));
            if let Some(t) = t.filter(|t| !t.is_empty()) {
                tok = Some(t.to_string());
                return Visit::Stop;
            }
            Visit::Continue
        },
        500,
        14,
    );
    tok
}

fn is_comment_item(item: &Value) -> bool {
    ["commentThreadRenderer", "commentViewModel", "commentRenderer"].iter().any(|k| item.get(*k).is_some())
}

/// Continuation tokens for the comments of a video: from a watch page's
/// `ytInitialData` or a `next` response (the first-page [`ContinuationKind::Section`]
/// token), and from a comments response (next-page, reply and sort tokens).
/// Tokens are de-duplicated and returned in page order.
pub fn extract_comment_continuations(json: &Value) -> Vec<CommentContinuation> {
    let mut out: Vec<CommentContinuation> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |token: Option<String>, kind: ContinuationKind| {
        if let Some(token) = token {
            if out.len() < 200 && seen.insert(token.clone()) {
                out.push(CommentContinuation { token, kind });
            }
        }
    };
    walk(json, |n| {
        let Value::Object(m) = n else { return Visit::Continue };
        // watch page: <itemSectionRenderer sectionIdentifier="comment-item-section">
        if let Some(sec) = m.get("itemSectionRenderer").filter(|v| is_obj(v)) {
            let is_comments = sec.get("sectionIdentifier").and_then(Value::as_str) == Some("comment-item-section") || sec.get("targetId").and_then(Value::as_str) == Some("comments-section");
            if is_comments {
                if let Some(Value::Array(contents)) = sec.get("contents") {
                    for item in contents.iter().take(50) {
                        if let Some(cir) = item.get("continuationItemRenderer") {
                            add(continuation_token(cir), ContinuationKind::Section);
                        }
                    }
                }
            }
        }
        // engagement panel holding the comments
        if let Some(panel) = m.get("engagementPanelSectionListRenderer").filter(|v| is_obj(v)) {
            let id = panel.get("panelIdentifier").and_then(Value::as_str).unwrap_or("");
            if id.contains("comments") {
                let mut cirs: Vec<&Value> = Vec::new();
                walk_with(
                    panel,
                    |x| {
                        if let Some(c) = x.get("continuationItemRenderer").filter(|c| is_obj(c)) {
                            cirs.push(c);
                        }
                        Visit::Continue
                    },
                    3000,
                    24,
                );
                for c in cirs.into_iter().take(10) {
                    add(continuation_token(c), ContinuationKind::Section);
                }
            }
        }
        // comments response: continuationItems next to loaded comments
        if let Some(Value::Array(items)) = m.get("continuationItems") {
            let target = m.get("targetId").and_then(Value::as_str).unwrap_or("");
            let for_comments = target == "comments-section" || target == "engagement-panel-comments-section" || items.iter().take(200).any(is_comment_item);
            if for_comments {
                for item in items.iter().take(200) {
                    if let Some(cir) = item.get("continuationItemRenderer") {
                        add(continuation_token(cir), ContinuationKind::Page);
                    }
                }
            }
        }
        // "show N replies" / "show more replies" of one thread
        if let Some(rep) = m.get("commentRepliesRenderer").filter(|v| is_obj(v)) {
            if let Some(Value::Array(contents)) = rep.get("contents") {
                for item in contents.iter().take(50) {
                    if let Some(cir) = item.get("continuationItemRenderer") {
                        add(continuation_token(cir), ContinuationKind::Replies);
                    }
                }
            }
            if let Some(Value::Array(conts)) = rep.get("continuations") {
                for c in conts.iter().take(10) {
                    add(continuation_token(c), ContinuationKind::Replies);
                }
            }
        }
        // sort menu of the comments header
        if let Some(header) = m.get("commentsHeaderRenderer").filter(|v| is_obj(v)) {
            if let Some(Value::Array(opts)) = g(header, &["sortMenu", "sortFilterSubMenuRenderer", "subMenuItems"]) {
                for o in opts.iter().take(10) {
                    add(o.get("serviceEndpoint").and_then(continuation_token), ContinuationKind::Sort);
                }
            }
        }
        Visit::Continue
    });
    out
}

/// The first-page comments token of a watch page's `ytInitialData` (or a
/// `next` response), if the comments section is present.
pub fn comments_section_token(json: &Value) -> Option<String> {
    extract_comment_continuations(json).into_iter().find(|c| c.kind == ContinuationKind::Section).map(|c| c.token)
}

/// Associate each reply continuation with its parent, including entity-backed comments.
pub fn comment_reply_tokens(json: &Value) -> Vec<(String, String)> {
    let mut entities = HashMap::new();
    walk(json, |n| {
        if let Some(p) = g(n, &["payload", "commentEntityPayload"]) {
            if let (Some(key), Some(id)) = (n.get("entityKey").and_then(Value::as_str), g_str(p, &["properties", "commentId"])) {
                entities.insert(key.to_string(), id.to_string());
            }
        }
        Visit::Continue
    });
    let mut result = Vec::new();
    walk(json, |n| {
        if let Some(thread) = n.get("commentThreadRenderer") {
            let mut parent = None;
            if let Some(comment) = thread.get("comment").or_else(|| thread.get("commentViewModel")) {
                walk(comment, |c| {
                    if parent.is_none() {
                        parent = c.get("commentId").and_then(Value::as_str).map(str::to_string)
                            .or_else(||c.get("commentKey").and_then(Value::as_str).and_then(|k|entities.get(k).cloned()));
                    }
                    Visit::Continue
                });
            }
            if let (Some(id), Some(replies)) = (parent, thread.get("replies")) {
                if let Some(token) = continuation_token(replies) { result.push((id, token)); }
            }
        }
        Visit::Continue
    });
    result
}

// ---- raw bodies ----------------------------------------------------------------------------

/// A raw request body -> JSON object. `None` for non-objects, invalid UTF-8 or
/// JSON, and bodies over [`MAX_REQUEST_BODY`].
pub fn parse_request_body(bytes: &[u8]) -> Option<Value> {
    if bytes.is_empty() || bytes.len() > MAX_REQUEST_BODY {
        return None;
    }
    let s = std::str::from_utf8(bytes).ok()?;
    let v: Value = serde_json::from_str(s).ok()?;
    v.is_object().then_some(v)
}

/// Strip an XSSI prefix (`)]}'`) and parse JSON; `None` on failure.
pub fn parse_json_text(s: &str) -> Option<Value> {
    let s = match s.strip_prefix(")]}") {
        Some(rest) => rest.strip_prefix('\'').unwrap_or(rest).trim_start_matches(is_js_space),
        None => s,
    };
    serde_json::from_str(s).ok()
}

// ---- page HTML ---------------------------------------------------------------------------------

/// The two embedded JSON blobs of a YouTube page.
#[derive(Debug, Clone, Default)]
pub struct InitialData {
    /// `ytInitialData`: the browse/search/next-shaped page data.
    pub yt_initial_data: Option<Value>,
    /// `ytInitialPlayerResponse`: the player response of a watch page.
    pub yt_initial_player_response: Option<Value>,
}

/// Find `var ytInitialData = {...};`, `window["ytInitialData"] = ...` and
/// `ytInitialPlayerResponse = {...};` in page HTML by string/escape-aware brace
/// matching. Also accepts the `= JSON.parse('...')` form. Work is bounded
/// ([`MAX_HTML_OBJECT`], a handful of candidates); never panics.
pub fn initial_data_from_html(html: &str) -> InitialData {
    InitialData { yt_initial_data: find_assigned_json(html, "ytInitialData"), yt_initial_player_response: find_assigned_json(html, "ytInitialPlayerResponse") }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// Exclusive end of the `{...}` starting at `start`, or `None`.
fn match_braces(b: &[u8], start: usize) -> Option<usize> {
    let end = b.len().min(start.saturating_add(MAX_HTML_OBJECT));
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in b.iter().enumerate().take(end).skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Decode the body of a JS single/double quoted string literal.
fn unescape_js_string(s: &str) -> Option<String> {
    let mut units: Vec<u16> = Vec::with_capacity(s.len());
    let mut it = s.chars();
    let hex = |it: &mut std::str::Chars<'_>, n: usize| -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = v * 16 + it.next()?.to_digit(16)?;
        }
        Some(v)
    };
    while let Some(c) = it.next() {
        if c != '\\' {
            let mut buf = [0u16; 2];
            units.extend_from_slice(c.encode_utf16(&mut buf));
            continue;
        }
        let e = it.next()?;
        let unit = match e {
            'n' => 10,
            't' => 9,
            'r' => 13,
            'b' => 8,
            'f' => 12,
            'v' => 11,
            '0' => 0,
            'x' => hex(&mut it, 2)? as u16,
            'u' => hex(&mut it, 4)? as u16,
            other => {
                let mut buf = [0u16; 2];
                units.extend_from_slice(other.encode_utf16(&mut buf));
                continue;
            }
        };
        units.push(unit);
    }
    String::from_utf16(&units).ok()
}

fn parse_object(s: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(s).ok()?;
    v.is_object().then_some(v)
}

fn find_assigned_json(html: &str, name: &str) -> Option<Value> {
    let b = html.as_bytes();
    let mut from = 0usize;
    let mut tried = 0usize;
    let mut seen = 0usize;
    while tried < MAX_HTML_CANDIDATES && seen < MAX_HTML_OCCURRENCES {
        let rel = html.get(from..)?.find(name)?;
        seen += 1;
        let start = from + rel;
        let mut i = start + name.len();
        from = i;
        if start > 0 && is_ident_byte(b[start - 1]) {
            continue;
        }
        if i < b.len() && is_ident_byte(b[i]) {
            continue;
        }
        // window["ytInitialData"] / window['ytInitialData']
        if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
            i += 1;
            if i < b.len() && b[i] == b']' {
                i += 1;
            } else {
                continue;
            }
        }
        i = skip_ws(b, i);
        if i >= b.len() || b[i] != b'=' || b.get(i + 1) == Some(&b'=') {
            continue;
        }
        i = skip_ws(b, i + 1);
        tried += 1;
        if b.get(i) == Some(&b'{') {
            if let Some(end) = match_braces(b, i) {
                if let Some(v) = html.get(i..end).and_then(parse_object) {
                    return Some(v);
                }
            }
        } else if html[i..].starts_with("JSON.parse(") {
            let q = skip_ws(b, i + "JSON.parse(".len());
            let Some(&quote) = b.get(q).filter(|c| **c == b'\'' || **c == b'"') else { continue };
            let mut j = q + 1;
            let limit = b.len().min(q.saturating_add(MAX_HTML_OBJECT));
            let mut esc = false;
            while j < limit {
                if esc {
                    esc = false;
                } else if b[j] == b'\\' {
                    esc = true;
                } else if b[j] == quote {
                    break;
                }
                j += 1;
            }
            if j < limit {
                if let Some(v) = html.get(q + 1..j).and_then(unescape_js_string).as_deref().and_then(parse_object) {
                    return Some(v);
                }
            }
        }
    }
    None
}

// ---- tests ------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID1: &str = "dQw4w9WgXcQ";
    const ID2: &str = "abcdefghijk";
    const ID3: &str = "A_b-C1d2E3f";
    const ID4: &str = "ZZZZZZZZZZZ";

    fn thumbs() -> Value {
        json!([
            { "url": "https://i.ytimg.com/vi/x/hqdefault.jpg?sqp=a", "width": 168, "height": 94 },
            { "url": "https://i.ytimg.com/vi/x/hq2.jpg?sqp=b", "width": 336, "height": 188 },
            { "url": "https://i.ytimg.com/vi/x/big.jpg", "width": 720, "height": 404 }
        ])
    }

    /// Object.assign(base, extra): `null` in `extra` models JS `undefined`.
    fn video_renderer(id: &str, extra: Value) -> Value {
        let mut v = json!({
            "videoId": id,
            "title": { "runs": [{ "text": "Title " }, { "text": id }] },
            "longBylineText": { "runs": [{ "text": "Some Channel", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCabcdefghijklmnopqrstuv" } } }] },
            "lengthText": { "simpleText": "1:02:03" },
            "publishedTimeText": { "simpleText": "3 days ago" },
            "viewCountText": { "simpleText": "1,234 views" },
            "thumbnail": { "thumbnails": thumbs() }
        });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    }

    fn lockup(id: &str, ty: &str) -> Value {
        json!({
            "lockupViewModel": {
                "contentId": id,
                "contentType": ty,
                "contentImage": {
                    "thumbnailViewModel": {
                        "image": { "sources": [{ "url": format!("https://i.ytimg.com/vi/{id}/hq720.jpg"), "width": 480, "height": 270 }] },
                        "overlays": [{
                            "thumbnailBottomOverlayViewModel": {
                                "badges": [{ "thumbnailBadgeViewModel": { "text": "12:34", "badgeStyle": "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT" } }]
                            }
                        }]
                    }
                },
                "metadata": {
                    "lockupMetadataViewModel": {
                        "title": { "content": format!("Lockup {id}") },
                        "image": {
                            "decoratedAvatarViewModel": {
                                "rendererContext": {
                                    "commandContext": { "onTap": { "innertubeCommand": { "browseEndpoint": { "browseId": "UCzyxwvutsrqponmlkjihgfe" } } } }
                                }
                            }
                        },
                        "metadata": {
                            "contentMetadataViewModel": {
                                "metadataRows": [
                                    { "metadataParts": [{ "text": { "content": "Lockup Channel" } }] },
                                    { "metadataParts": [{ "text": { "content": "5.6K views" } }, { "text": { "content": "2 weeks ago" } }] }
                                ]
                            }
                        }
                    }
                }
            }
        })
    }

    fn video_lockup(id: &str) -> Value {
        lockup(id, "LOCKUP_CONTENT_TYPE_VIDEO")
    }

    /// `{ key: inner }` without the deep clone `json!` would make.
    fn wrap(key: &str, inner: Value) -> Value {
        let mut m = serde_json::Map::new();
        m.insert(key.to_string(), inner);
        Value::Object(m)
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.video_id.as_str()).collect()
    }

    fn feed(v: &Value) -> Vec<Item> {
        extract_feed(v, false)
    }

    #[test]
    fn duration_parsing() {
        assert_eq!(parse_duration(&json!("1:02:03")), Some(3723));
        assert_eq!(parse_duration(&json!("12:34")), Some(754));
        assert_eq!(parse_duration(&json!("0:07")), Some(7));
        assert_eq!(parse_duration(&json!(" 3:00 ")), Some(180));
        assert_eq!(parse_duration(&json!("LIVE")), None);
        assert_eq!(parse_duration(&json!("")), None);
        assert_eq!(parse_duration(&Value::Null), None);
        assert_eq!(parse_duration(&json!("12345")), Some(12345));
        assert_eq!(parse_duration(&json!({})), None);
        // invisible marks and nbsp are stripped, numbers pass through
        assert_eq!(parse_duration(&json!("\u{200e}1:05\u{a0}")), Some(65));
        assert_eq!(parse_duration(&json!(42.9)), Some(42));
        assert_eq!(parse_duration(&json!(-1)), None);
        assert_eq!(parse_duration(&json!("1:2:3:4")), None);
        assert_eq!(parse_duration(&json!("1234:00")), None);
        assert_eq!(parse_duration(&json!("123456789")), None);
    }

    #[test]
    fn count_parsing() {
        assert_eq!(parse_count(&json!("1,234")), Some(1234));
        assert_eq!(parse_count(&json!("1.2K")), Some(1200));
        assert_eq!(parse_count(&json!("3.4M views")), Some(3_400_000));
        assert_eq!(parse_count(&json!("1,234,567 views")), Some(1_234_567));
        assert_eq!(parse_count(&json!("1.234.567 visninger")), Some(1_234_567));
        assert_eq!(parse_count(&json!("")), None);
        assert_eq!(parse_count(&json!("abc")), None);
        assert_eq!(parse_count(&json!("No views")), Some(0));
        assert_eq!(parse_count(&json!(42)), Some(42));
        // regex quirks kept from the JS: a suffix followed by a letter is not a suffix
        assert_eq!(parse_count(&json!("2B")), Some(2_000_000_000));
        assert_eq!(parse_count(&json!("1,5 Mio")), Some(15));
        assert_eq!(parse_count(&json!("1,5 M")), Some(1_500_000));
        assert_eq!(parse_count(&Value::Null), None);
        assert_eq!(parse_count(&json!("1234567890123456789")), None);
    }

    #[test]
    fn video_id_validation() {
        assert!(valid_id(Some(&json!(ID1))).is_some());
        assert!(valid_id(Some(&json!(ID3))).is_some());
        assert!(valid_id(Some(&json!("short"))).is_none());
        assert!(valid_id(Some(&json!("toolongvideoid1"))).is_none());
        assert!(valid_id(Some(&json!("abc def ghi"))).is_none());
        assert!(valid_id(Some(&Value::Null)).is_none());
        assert!(valid_id(Some(&json!(12345678901u64))).is_none());
    }

    #[test]
    fn video_renderer_inside_rich_item_renderer_home_feed() {
        let json = json!({
            "contents": { "twoColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": { "content": { "richGridRenderer": { "contents": [
                { "richItemRenderer": { "content": { "videoRenderer": video_renderer(ID1, json!({})) } } },
                { "richItemRenderer": { "content": { "videoRenderer": video_renderer(ID2, json!({})) } } },
                { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "x" } } } }
            ] } } } }] } }
        });
        let items = feed(&json);
        assert_eq!(ids(&items), [ID1, ID2]);
        let it = &items[0];
        assert_eq!(it.title, format!("Title {ID1}"));
        assert_eq!(it.channel.as_deref(), Some("Some Channel"));
        assert_eq!(it.channel_id.as_deref(), Some("UCabcdefghijklmnopqrstuv"));
        assert_eq!(it.duration_seconds, Some(3723));
        assert_eq!(it.published_text.as_deref(), Some("3 days ago"));
        assert_eq!(it.view_text.as_deref(), Some("1,234 views"));
        assert_eq!(it.thumbnail.as_deref(), Some("https://i.ytimg.com/vi/x/hq2.jpg?sqp=b")); // smallest >= 320 wide
    }

    #[test]
    fn compact_grid_video_and_playlist_renderers() {
        let json = json!({ "a": [
            { "compactVideoRenderer": video_renderer(ID1, json!({ "shortBylineText": null })) },
            { "gridVideoRenderer": video_renderer(ID2, json!({ "shortViewCountText": { "simpleText": "1.2K views" }, "viewCountText": null })) },
            { "videoRenderer": video_renderer(ID3, json!({})) },
            { "playlistVideoRenderer": {
                "videoId": ID4,
                "title": { "runs": [{ "text": "PL item" }] },
                "shortBylineText": { "runs": [{ "text": "PL Channel" }] },
                "lengthSeconds": "754",
                "lengthText": { "simpleText": "12:34" },
                "thumbnail": { "thumbnails": thumbs() },
                "isPlayable": true
            } }
        ] });
        let items = feed(&json);
        assert_eq!(ids(&items), [ID1, ID2, ID3, ID4]);
        assert_eq!(items[1].view_text.as_deref(), Some("1.2K views"));
        assert_eq!(items[3].duration_seconds, Some(754));
        assert_eq!(items[3].channel.as_deref(), Some("PL Channel"));
    }

    #[test]
    fn unplayable_playlist_items_are_skipped() {
        let items = feed(&json!({ "playlistVideoRenderer": { "videoId": ID1, "title": { "simpleText": "x" }, "isPlayable": false } }));
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn lockup_view_model_video_other_lockup_types_skipped() {
        let json = json!({ "contents": [video_lockup(ID1), lockup("PLabcdefghi", "LOCKUP_CONTENT_TYPE_PLAYLIST"), video_lockup(ID2)] });
        let items = feed(&json);
        assert_eq!(ids(&items), [ID1, ID2]);
        let it = &items[0];
        assert_eq!(it.title, format!("Lockup {ID1}"));
        assert_eq!(it.channel.as_deref(), Some("Lockup Channel"));
        assert_eq!(it.channel_id.as_deref(), Some("UCzyxwvutsrqponmlkjihgfe"));
        assert_eq!(it.duration_seconds, Some(754));
        assert_eq!(it.view_text.as_deref(), Some("5.6K views"));
        assert_eq!(it.published_text.as_deref(), Some("2 weeks ago"));
        assert_eq!(it.thumbnail.as_deref(), Some(format!("https://i.ytimg.com/vi/{ID1}/hq720.jpg").as_str()));
    }

    #[test]
    fn lockup_live_badge_has_no_duration() {
        let mut l = video_lockup(ID1);
        *l.pointer_mut("/lockupViewModel/contentImage/thumbnailViewModel/overlays/0/thumbnailBottomOverlayViewModel/badges/0/thumbnailBadgeViewModel/text").unwrap() = json!("LIVE");
        let items = feed(&l);
        assert_eq!(items[0].badge.as_deref(), Some("LIVE"));
        assert_eq!(items[0].duration_seconds, None);
    }

    #[test]
    fn live_and_upcoming_badges_on_video_renderer() {
        let live = video_renderer(ID1, json!({ "lengthText": null, "thumbnailOverlays": [{ "thumbnailOverlayTimeStatusRenderer": { "style": "LIVE", "text": { "simpleText": "LIVE" } } }] }));
        let up = video_renderer(ID2, json!({ "upcomingEventData": { "startTime": "1" }, "lengthText": null }));
        let items = feed(&json!([{ "videoRenderer": live }, { "videoRenderer": up }]));
        assert_eq!(items[0].badge.as_deref(), Some("LIVE"));
        assert_eq!(items[0].duration_seconds, None);
        assert_eq!(items[1].badge.as_deref(), Some("UPCOMING"));
    }

    #[test]
    fn metadata_badge_label_is_used() {
        let r = video_renderer(ID1, json!({ "badges": [{ "metadataBadgeRenderer": { "label": "  New  " } }] }));
        assert_eq!(feed(&json!({ "videoRenderer": r }))[0].badge.as_deref(), Some("New"));
    }

    #[test]
    fn duration_from_overlay_when_length_text_missing() {
        let r = video_renderer(ID1, json!({ "lengthText": null, "thumbnailOverlays": [{ "thumbnailOverlayTimeStatusRenderer": { "style": "DEFAULT", "text": { "simpleText": "4:05" } } }] }));
        assert_eq!(feed(&json!({ "videoRenderer": r }))[0].duration_seconds, Some(245));
    }

    #[test]
    fn shorts_are_skipped_by_default_marked_when_asked() {
        let json = json!({ "items": [
            { "reelItemRenderer": { "videoId": ID1, "headline": { "simpleText": "Short one" }, "thumbnail": { "thumbnails": thumbs() } } },
            { "videoRenderer": video_renderer(ID2, json!({ "thumbnailOverlays": [{ "thumbnailOverlayTimeStatusRenderer": { "style": "SHORTS", "text": { "simpleText": "Shorts" } } }] })) },
            { "videoRenderer": video_renderer(ID3, json!({})) }
        ] });
        assert_eq!(ids(&extract_feed(&json, false)), [ID3]);
        let with_shorts = extract_feed(&json, true);
        assert_eq!(ids(&with_shorts), [ID1, ID2, ID3]);
        assert_eq!(with_shorts[0].badge.as_deref(), Some("SHORT"));
        assert_eq!(with_shorts[0].title, "Short one");
        assert_eq!(with_shorts[1].badge.as_deref(), Some("SHORT"));
    }

    #[test]
    fn dedupes_within_a_feed_keeping_first_occurrence_and_page_order() {
        let a = video_renderer(ID1, json!({}));
        let b = video_renderer(ID1, json!({ "title": { "simpleText": "second copy" } }));
        let items = feed(&json!({ "x": [{ "videoRenderer": a }, { "videoRenderer": video_renderer(ID2, json!({})) }, { "videoRenderer": b }] }));
        assert_eq!(ids(&items), [ID1, ID2]);
        assert_eq!(items[0].title, format!("Title {ID1}"));
    }

    #[test]
    fn rejects_invalid_video_ids() {
        let mut numeric = video_renderer(ID1, json!({}));
        numeric["videoId"] = json!(12345);
        let items = feed(&json!({ "a": [
            { "videoRenderer": video_renderer("short", json!({})) },
            { "videoRenderer": video_renderer("has space!!", json!({})) },
            { "videoRenderer": numeric },
            { "videoRenderer": video_renderer(ID1, json!({})) }
        ] }));
        assert_eq!(ids(&items), [ID1]);
    }

    #[test]
    fn thumbnail_falls_back_to_built_mqdefault_url_protocol_relative_fixed() {
        let no_thumb = &feed(&json!({ "videoRenderer": video_renderer(ID1, json!({ "thumbnail": null })) }))[0];
        assert_eq!(no_thumb.thumbnail.as_deref(), Some(format!("https://i.ytimg.com/vi/{ID1}/mqdefault.jpg").as_str()));
        let rel = &feed(&json!({ "videoRenderer": video_renderer(ID1, json!({ "thumbnail": { "thumbnails": [{ "url": "//i.ytimg.com/vi/x/a.jpg", "width": 400 }] } })) }))[0];
        assert_eq!(rel.thumbnail.as_deref(), Some("https://i.ytimg.com/vi/x/a.jpg"));
        let small = &feed(&json!({ "videoRenderer": video_renderer(ID1, json!({ "thumbnail": { "thumbnails": [{ "url": "https://a/1.jpg", "width": 100 }, { "url": "https://a/2.jpg", "width": 200 }] } })) }))[0];
        assert_eq!(small.thumbnail.as_deref(), Some("https://a/2.jpg"));
        // a non-http url is rejected and the built url used instead
        let odd = pick_thumbnail(Some(&json!([{ "url": "data:image/png;base64,AA", "width": 400 }])), Some(ID1));
        assert_eq!(odd.as_deref(), Some(thumb_url(ID1).as_str()));
        assert_eq!(pick_thumbnail(None, None), None);
    }

    #[test]
    fn items_have_no_empty_fields() {
        let it = &feed(&json!({ "videoRenderer": { "videoId": ID1, "title": { "simpleText": "T" } } }))[0];
        assert!(!it.title.is_empty());
        for s in [&it.channel, &it.channel_id, &it.thumbnail, &it.published_text, &it.view_text, &it.badge].into_iter().flatten() {
            assert!(!s.is_empty());
        }
        assert!(it.channel.is_none() && it.published_text.is_none() && it.view_text.is_none() && it.badge.is_none());
    }

    // ---- video meta ----

    #[test]
    fn video_meta_from_a_player_response() {
        let pr = json!({
            "videoDetails": {
                "videoId": ID1, "title": "Never Gonna", "lengthSeconds": "212", "author": "Rick",
                "channelId": "UCuAXFkgsw1L7xaCfnd5JJOw", "shortDescription": "desc", "viewCount": "1500000000",
                "isLive": false, "thumbnail": { "thumbnails": thumbs() }
            },
            "microformat": { "playerMicroformatRenderer": { "publishDate": "2009-10-25" } }
        });
        let m = extract_video_meta(&pr).unwrap();
        assert_eq!(m.video_id, ID1);
        assert_eq!(m.title, "Never Gonna");
        assert_eq!(m.duration_seconds, Some(212));
        assert_eq!(m.channel.as_deref(), Some("Rick"));
        assert_eq!(m.channel_id.as_deref(), Some("UCuAXFkgsw1L7xaCfnd5JJOw"));
        assert_eq!(m.description.as_deref(), Some("desc"));
        assert_eq!(m.view_count, Some(1_500_000_000));
        assert_eq!(m.published_text.as_deref(), Some("2009-10-25"));
        assert_eq!(m.is_live, Some(false));
        assert!(m.related.is_empty());
        assert!(m.captured_at.is_none());
    }

    #[test]
    fn video_meta_from_a_next_response_with_related_videos() {
        let next = json!({
            "currentVideoEndpoint": { "watchEndpoint": { "videoId": ID1 } },
            "contents": { "twoColumnWatchNextResults": {
                "results": { "results": { "contents": [
                    { "videoPrimaryInfoRenderer": {
                        "title": { "runs": [{ "text": "Next Title" }] },
                        "viewCount": { "videoViewCountRenderer": { "viewCount": { "simpleText": "1,234,567 views" } } },
                        "dateText": { "simpleText": "Oct 25, 2009" }
                    } },
                    { "videoSecondaryInfoRenderer": {
                        "owner": { "videoOwnerRenderer": { "title": { "runs": [{ "text": "Owner", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCowner123456789012345" } } }] } } },
                        "attributedDescription": { "content": "Attributed desc" }
                    } }
                ] } },
                "secondaryResults": { "secondaryResults": { "results": [
                    { "compactVideoRenderer": video_renderer(ID2, json!({})) },
                    video_lockup(ID3),
                    { "compactVideoRenderer": video_renderer(ID1, json!({})) }
                ] } }
            } }
        });
        let m = extract_video_meta(&next).unwrap();
        assert_eq!(m.video_id, ID1);
        assert_eq!(m.title, "Next Title");
        assert_eq!(m.view_count, Some(1_234_567));
        assert_eq!(m.channel.as_deref(), Some("Owner"));
        assert_eq!(m.channel_id.as_deref(), Some("UCowner123456789012345"));
        assert_eq!(m.description.as_deref(), Some("Attributed desc"));
        assert_eq!(m.published_text.as_deref(), Some("Oct 25, 2009"));
        assert_eq!(ids(&m.related), [ID2, ID3]); // own id removed
        assert_eq!(m.thumbnail.as_deref(), Some(thumb_url(ID1).as_str()));
    }

    #[test]
    fn video_meta_is_none_without_id_or_title() {
        assert!(extract_video_meta(&json!({})).is_none());
        assert!(extract_video_meta(&json!({ "videoDetails": { "videoId": "bad", "title": "x" } })).is_none());
        assert!(extract_video_meta(&json!({ "videoDetails": { "videoId": ID1 } })).is_none());
        assert!(extract_video_meta(&Value::Null).is_none());
        assert!(extract_video_meta(&json!("str")).is_none());
    }

    #[test]
    fn formats() {
        let f = extract_formats(&json!({ "streamingData": {
            "formats": [{ "itag": 18, "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"", "qualityLabel": "360p", "height": 360, "contentLength": "1000" }],
            "adaptiveFormats": [
                { "itag": 137, "mimeType": "video/mp4; codecs=\"avc1\"", "qualityLabel": "1080p", "bitrate": 4000000, "contentLength": "5000" },
                { "itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a\"", "audioQuality": "AUDIO_QUALITY_MEDIUM", "contentLength": "300" },
                { "nonsense": true }
            ]
        } }));
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], Format { itag: 18, mime: Some("video/mp4".into()), quality_label: Some("360p".into()), height: Some(360), bitrate: None, content_length: Some(1000) });
        assert_eq!(f[1].bitrate, Some(4_000_000));
        assert_eq!(f[2].quality_label.as_deref(), Some("audio:medium"));
        assert!(extract_formats(&Value::Null).is_empty());
        assert!(extract_formats(&json!({ "streamingData": 5 })).is_empty());
    }

    // ---- comments ----

    #[test]
    fn comments_from_comment_thread_renderer() {
        let json = json!({ "onResponseReceivedEndpoints": [{ "reloadContinuationItemsCommand": { "continuationItems": [
            { "commentsHeaderRenderer": { "countText": { "runs": [{ "text": "1,234" }, { "text": " Comments" }] } } },
            { "commentThreadRenderer": {
                "comment": { "commentRenderer": {
                    "commentId": "Ugx1", "authorText": { "simpleText": "@alice" },
                    "contentText": { "runs": [{ "text": "hello " }, { "text": "world" }] },
                    "voteCount": { "simpleText": "1.2K" }, "publishedTimeText": { "runs": [{ "text": "3 days ago" }] }, "replyCount": 4
                } },
                "replies": { "commentRepliesRenderer": { "contents": [
                    { "commentRenderer": { "commentId": "Ugx1.r1", "authorText": { "simpleText": "@bob" }, "contentText": { "runs": [{ "text": "a reply" }] } } }
                ] } }
            } },
            { "commentThreadRenderer": { "comment": { "commentRenderer": { "commentId": "Ugx2", "authorText": { "simpleText": "@carol" }, "contentText": { "simpleText": "second" } } } } }
        ] } }] });
        let r = extract_comments(&json);
        assert_eq!(r.total, Some(1234));
        let cid: Vec<_> = r.comments.iter().map(|c| c.id.as_deref().unwrap()).collect();
        assert_eq!(cid, ["Ugx1", "Ugx1.r1", "Ugx2"]);
        assert_eq!(r.comments[0].text, "hello world");
        assert_eq!(r.comments[0].likes, Some(1200));
        assert_eq!(r.comments[0].reply_count, Some(4));
        assert_eq!(r.comments[0].published_text.as_deref(), Some("3 days ago"));
        assert_eq!(r.comments[0].is_reply, None);
        assert_eq!(r.comments[1].is_reply, Some(true));
        assert_eq!(r.comments[2].likes, Some(0));
    }

    #[test]
    fn comments_from_framework_updates_entity_payloads_in_page_order() {
        let entity = |key: &str, id: &str, text: &str, level: u32| {
            json!({ "entityKey": key, "payload": { "commentEntityPayload": {
                "key": key,
                "properties": { "commentId": id, "content": { "content": text }, "publishedTime": "2 days ago", "replyLevel": level },
                "author": { "displayName": format!("@u_{id}"), "channelId": "UCx" },
                "toolbar": { "likeCountNotliked": if id == "c2" { "" } else { "12" }, "replyCount": "3" }
            } } })
        };
        let json = json!({
            "onResponseReceivedEndpoints": [{ "reloadContinuationItemsCommand": { "continuationItems": [
                { "commentThreadRenderer": { "commentViewModel": { "commentViewModel": { "commentKey": "k2" } } } },
                { "commentThreadRenderer": { "commentViewModel": { "commentViewModel": { "commentKey": "k1" } } } }
            ] } }],
            "frameworkUpdates": { "entityBatchUpdate": { "mutations": [
                entity("k1", "c1", "first in mutations", 0),
                entity("k2", "c2", "second in mutations", 0),
                entity("k3", "c3", "unreferenced reply", 1)
            ] } }
        });
        let r = extract_comments(&json);
        let cid: Vec<_> = r.comments.iter().map(|c| c.id.as_deref().unwrap()).collect();
        assert_eq!(cid, ["c2", "c1", "c3"]); // page order, then leftovers
        assert_eq!(r.comments[0].likes, Some(0));
        assert_eq!(r.comments[1].likes, Some(12));
        assert_eq!(r.comments[1].reply_count, Some(3));
        assert_eq!(r.comments[1].text, "first in mutations");
        assert_eq!(r.comments[1].author, "@u_c1");
        assert_eq!(r.comments[2].is_reply, Some(true));
        assert_eq!(r.comments[0].is_reply, None);
    }

    #[test]
    fn comments_are_deduplicated() {
        let c = json!({ "commentThreadRenderer": { "comment": { "commentRenderer": { "commentId": "dup", "authorText": { "simpleText": "a" }, "contentText": { "simpleText": "x" } } } } });
        assert_eq!(extract_comments(&json!([c.clone(), c.clone(), c])).comments.len(), 1);
        // without ids the author + text prefix is the key
        let anon = |t: &str| json!({ "commentRenderer": { "authorText": { "simpleText": "a" }, "contentText": { "simpleText": t } } });
        assert_eq!(extract_comments(&json!([anon("x"), anon("x"), anon("y")])).comments.len(), 2);
    }

    #[test]
    fn comment_count_from_entry_point_header() {
        let r = extract_comments(&json!({ "commentsEntryPointHeaderRenderer": { "commentCount": { "simpleText": "2.5K" } } }));
        assert_eq!(r.total, Some(2500));
        assert!(r.comments.is_empty());
    }

    // ---- classify ----

    fn yt(p: &str) -> String {
        format!("https://www.youtube.com/youtubei/v1/{p}?prettyPrint=false")
    }

    #[test]
    fn classify_browse_search_next_player() {
        let c = |p: &str, body: Value| classify(&yt(p), &body, None).map(Kind::as_str);
        assert_eq!(c("browse", json!({ "browseId": "FEwhat_to_watch" })), Some("home"));
        assert_eq!(c("browse", json!({ "browseId": "FEsubscriptions" })), Some("subscriptions"));
        assert_eq!(c("browse", json!({ "browseId": "UCabcdefghijklmnopqrstuv" })), Some("channel"));
        assert_eq!(c("browse", json!({ "browseId": "FEhistory" })), Some("other"));
        assert_eq!(c("search", json!({ "query": "cats" })), Some("search"));
        assert_eq!(c("next", json!({ "videoId": ID1 })), Some("related"));
        assert_eq!(c("player", json!({ "videoId": ID1 })), Some("video"));
        assert_eq!(classify("https://www.youtube.com/youtubei/v1/log_event", &json!({}), None), None);
        assert_eq!(classify("https://example.com/", &json!({}), None), None);
        assert_eq!(classify("https://www.youtube.com/youtubei/v1/browsefoo.bar", &json!({}), None), None);
        assert_eq!(classify("https://www.youtube.com/youtubei/v1/", &json!({}), None), None);
    }

    #[test]
    fn classify_uses_the_page_url_for_continuations() {
        let url = "https://www.youtube.com/youtubei/v1/browse";
        let body = json!({ "continuation": "TOKEN" });
        let k = |page: &str| classify(url, &body, Some(page)).map(Kind::as_str);
        assert_eq!(k("https://www.youtube.com/"), Some("home"));
        assert_eq!(k("https://www.youtube.com/feed/subscriptions"), Some("subscriptions"));
        assert_eq!(k("https://www.youtube.com/@someone/videos"), Some("channel"));
        assert_eq!(k("https://www.youtube.com/feed/history"), Some("other"));
    }

    #[test]
    fn classify_request_details_and_raw_bodies() {
        let c = classify_request_bytes("https://www.youtube.com/youtubei/v1/search", Some(br#"{"query":"q1"}"#), None);
        assert_eq!(c.kind, Some(Kind::Search));
        assert_eq!(c.endpoint.as_deref(), Some("search"));
        assert_eq!(c.query.as_deref(), Some("q1"));
        let n = classify_request("https://www.youtube.com/youtubei/v1/next", &json!({ "continuation": "T" }), Some(&format!("https://www.youtube.com/watch?v={ID1}")));
        assert_eq!(n.video_id.as_deref(), Some(ID1));
        assert!(n.continuation);
        let s = classify_request("https://www.youtube.com/youtubei/v1/search", &json!({ "continuation": "T" }), Some("https://www.youtube.com/results?search_query=hello+world"));
        assert_eq!(s.query.as_deref(), Some("hello world"));
        assert_eq!(classify_request_bytes("https://www.youtube.com/youtubei/v1/browse", Some(b"{not json"), None).kind, Some(Kind::Other));
        assert_eq!(classify_request_bytes("https://www.youtube.com/youtubei/v1/browse", None, None).kind, Some(Kind::Other));
        let bid = classify_request_bytes("https://www.youtube.com/youtubei/v1/browse", Some(br#"{"context":{"client":{"clientName":"WEB"}},"browseId":"FEwhat_to_watch"}"#), None);
        assert_eq!((bid.kind, bid.browse_id.as_deref()), (Some(Kind::Home), Some("FEwhat_to_watch")));
        // unknown endpoints are not interesting
        let lg = classify_request("https://www.youtube.com/youtubei/v1/log_event", &json!({}), None);
        assert_eq!((lg.endpoint, lg.kind), (None, None));
    }

    #[test]
    fn page_context_and_video_id_from_url() {
        assert_eq!(page_context(&format!("https://www.youtube.com/watch?v={ID1}&t=5")), PageContext { kind: Kind::Related, query: None, video_id: Some(ID1.into()) });
        assert_eq!(video_id_from_url("https://www.youtube.com/watch?v=bad"), None);
        assert_eq!(video_id_from_url("https://www.youtube.com/"), None);
        assert_eq!(video_id_from_url(&format!("https://evil.example/watch?v={ID1}")), None);
        assert_eq!(video_id_from_url(&format!("https://youtube.com.evil.example/watch?v={ID1}")), None);
        assert_eq!(video_id_from_url(&format!("https://youtube.com@evil.example/watch?v={ID1}")), None);
        assert_eq!(video_id_from_url(&format!("https://m.youtube.com/watch?v={ID1}")), Some(ID1.into()));
        assert_eq!(video_id_from_url(&format!("https://www.youtube.com/live/{ID1}")), Some(ID1.into()));
        assert_eq!(video_id_from_url(&format!("https://www.youtube.com/embed/{ID1}?x=1")), Some(ID1.into()));
        assert_eq!(page_context("garbage").kind, Kind::Other);
        assert_eq!(page_context("").kind, Kind::Other);
        assert_eq!(page_context("https://www.youtube.com/results?search_query=a%20b").query.as_deref(), Some("a b"));
        assert_eq!(page_context("https://www.youtube.com/channel/UCabc").kind, Kind::Channel);
        assert_eq!(page_context("https://www.youtube.com").kind, Kind::Home);
    }

    // ---- URL helpers ----

    #[test]
    fn url_parsing() {
        let u = parse_url("HTTPS://user:pw@WWW.YouTube.com:8443/watch?v=abc&x=1#frag?no").unwrap();
        assert_eq!((u.scheme.as_str(), u.host.as_str(), u.port, u.path.as_str(), u.query.as_str()), ("https", "www.youtube.com", Some(8443), "/watch", "v=abc&x=1"));
        let u = parse_url("https://a.example").unwrap();
        assert_eq!((u.path.as_str(), u.query.as_str(), u.port), ("/", "", None));
        let u = parse_url("https://a.example?q=1").unwrap();
        assert_eq!((u.path.as_str(), u.query.as_str()), ("/", "q=1"));
        assert_eq!(parse_url("https://[::1]:80/x").unwrap().host, "[::1]");
        for bad in ["", "garbage", "javascript:alert(1)", "https://", "https:///x", "://x", "1http://x", "https://h:99999/", "https://h:ab/", "//x/y"] {
            assert!(parse_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn query_parsing() {
        let q = parse_query("a=1&b=hello+world&c=%E2%82%AC&&d&e=%zz&f=%4");
        assert_eq!(q[0], ("a".into(), "1".into()));
        assert_eq!(q[1], ("b".into(), "hello world".into()));
        assert_eq!(q[2], ("c".into(), "\u{20ac}".into()));
        assert_eq!(q[3], ("d".into(), "".into()));
        assert_eq!(q[4], ("e".into(), "%zz".into()));
        assert_eq!(q[5], ("f".into(), "%4".into()));
        assert_eq!(q.len(), 6);
        assert!(parse_query("").is_empty());
        assert_eq!(percent_decode("%41%2B+", false), "A++");
        assert_eq!(percent_decode("%ff", false), "\u{fffd}");
    }

    // ---- comment continuations ----

    fn cir(token: &str) -> Value {
        json!({ "continuationItemRenderer": { "trigger": "CONTINUATION_TRIGGER_ON_ITEM_SHOWN", "continuationEndpoint": { "continuationCommand": { "token": token, "request": "CONTINUATION_REQUEST_TYPE_WATCH_NEXT" } } } })
    }

    fn toks(v: &Value) -> Vec<(String, ContinuationKind)> {
        extract_comment_continuations(v).into_iter().map(|c| (c.token, c.kind)).collect()
    }

    #[test]
    fn comment_section_token_from_watch_page_initial_data() {
        let data = json!({ "contents": { "twoColumnWatchNextResults": { "results": { "results": { "contents": [
            { "videoPrimaryInfoRenderer": { "title": { "runs": [{ "text": "T" }] } } },
            { "itemSectionRenderer": { "sectionIdentifier": "other", "contents": [cir("NOT_COMMENTS")] } },
            { "itemSectionRenderer": { "sectionIdentifier": "comment-item-section", "contents": [cir("SECTION_TOKEN")] } }
        ] } } } } });
        assert_eq!(toks(&data), [("SECTION_TOKEN".to_string(), ContinuationKind::Section)]);
        assert_eq!(comments_section_token(&data).as_deref(), Some("SECTION_TOKEN"));
    }

    #[test]
    fn comment_section_token_from_engagement_panel_and_target_id() {
        let data = json!({ "engagementPanels": [
            { "engagementPanelSectionListRenderer": { "panelIdentifier": "engagement-panel-structured-description", "content": { "x": cir("DESC") } } },
            { "engagementPanelSectionListRenderer": { "panelIdentifier": "engagement-panel-comments-section", "content": { "sectionListRenderer": { "contents": [
                { "itemSectionRenderer": { "contents": [cir("PANEL_TOKEN")] } }
            ] } } } }
        ] });
        assert_eq!(toks(&data), [("PANEL_TOKEN".to_string(), ContinuationKind::Section)]);
        let old = json!({ "itemSectionRenderer": { "targetId": "comments-section", "contents": [cir("OLD")] } });
        assert_eq!(comments_section_token(&old).as_deref(), Some("OLD"));
        // the same token found twice is reported once
        let both = json!([data, { "itemSectionRenderer": { "targetId": "comments-section", "contents": [cir("PANEL_TOKEN")] } }]);
        assert_eq!(toks(&both).len(), 1);
    }

    #[test]
    fn comment_page_reply_and_sort_tokens_from_a_comments_response() {
        let json = json!({ "onResponseReceivedEndpoints": [
            { "reloadContinuationItemsCommand": { "slot": "RELOAD_CONTINUATION_SLOT_HEADER", "continuationItems": [
                { "commentsHeaderRenderer": { "countText": { "runs": [{ "text": "12" }] }, "sortMenu": { "sortFilterSubMenuRenderer": { "subMenuItems": [
                    { "title": "Top comments", "serviceEndpoint": { "continuationCommand": { "token": "SORT_TOP" } } },
                    { "title": "Newest first", "serviceEndpoint": { "continuationCommand": { "token": "SORT_NEW" } } }
                ] } } } }
            ] } },
            { "reloadContinuationItemsCommand": { "slot": "RELOAD_CONTINUATION_SLOT_BODY", "continuationItems": [
                { "commentThreadRenderer": {
                    "comment": { "commentRenderer": { "commentId": "a", "authorText": { "simpleText": "x" }, "contentText": { "simpleText": "y" } } },
                    "replies": { "commentRepliesRenderer": { "contents": [
                        { "continuationItemRenderer": { "button": { "buttonRenderer": { "command": { "continuationCommand": { "token": "REPLIES_1" } } } } } }
                    ] } }
                } },
                { "commentThreadRenderer": {
                    "comment": { "commentRenderer": { "commentId": "b", "authorText": { "simpleText": "x" }, "contentText": { "simpleText": "y" } } },
                    "replies": { "commentRepliesRenderer": { "continuations": [{ "nextContinuationData": { "continuation": "REPLIES_2" } }] } }
                } },
                cir("NEXT_PAGE")
            ] } }
        ] });
        let t = toks(&json);
        let has = |tok: &str, k: ContinuationKind| t.iter().any(|(a, b)| a == tok && *b == k);
        assert!(has("SORT_TOP", ContinuationKind::Sort));
        assert!(has("SORT_NEW", ContinuationKind::Sort));
        assert!(has("REPLIES_1", ContinuationKind::Replies));
        assert!(has("REPLIES_2", ContinuationKind::Replies));
        assert!(has("NEXT_PAGE", ContinuationKind::Page));
        assert_eq!(t.len(), 5);
        // comments and tokens come from the same response
        assert_eq!(extract_comments(&json).comments.len(), 2);
    }

    #[test]
    fn continuation_items_of_other_features_are_not_comment_tokens() {
        let related = json!({ "onResponseReceivedEndpoints": [{ "appendContinuationItemsAction": { "continuationItems": [
            { "compactVideoRenderer": video_renderer(ID1, json!({})) }, cir("RELATED_MORE")
        ] } }] });
        assert!(toks(&related).is_empty());
        // an append action targeted at the comments section with only a token still counts
        let target = json!({ "appendContinuationItemsAction": { "targetId": "comments-section", "continuationItems": [cir("T")] } });
        assert_eq!(toks(&target), [("T".to_string(), ContinuationKind::Page)]);
        // replies page: stand-alone comments plus a "more replies" token
        let replies = json!({ "appendContinuationItemsAction": { "continuationItems": [
            { "commentViewModel": { "commentViewModel": { "commentKey": "k" } } }, cir("MORE_REPLIES")
        ] } });
        assert_eq!(toks(&replies), [("MORE_REPLIES".to_string(), ContinuationKind::Page)]);
    }

    #[test]
    fn continuation_extraction_survives_garbage() {
        for g in [Value::Null, json!(1), json!("s"), json!([]), json!({}), json!({ "itemSectionRenderer": 5 }), json!({ "continuationItems": [null, 1, "x", []] }), json!({ "commentRepliesRenderer": { "contents": [5], "continuations": [null] } }), json!({ "engagementPanelSectionListRenderer": [] })] {
            assert!(toks(&g).is_empty(), "{g}");
            assert!(comments_section_token(&g).is_none());
        }
        let empty_token = json!({ "itemSectionRenderer": { "sectionIdentifier": "comment-item-section", "contents": [cir("")] } });
        assert!(toks(&empty_token).is_empty());
    }

    // ---- raw bodies ----

    #[test]
    fn request_body_parsing() {
        let rb = parse_request_body(br#"{"context":{"client":{"clientName":"WEB"}},"browseId":"FEwhat_to_watch"}"#).unwrap();
        assert_eq!(rb["browseId"], "FEwhat_to_watch");
        assert_eq!(parse_request_body(br#"{"query":"cats"}"#).unwrap()["query"], "cats");
        assert!(parse_request_body(&[0xff, 0xfe, 1]).is_none());
        assert!(parse_request_body(b"[1,2]").is_none());
        assert!(parse_request_body(b"").is_none());
        assert!(parse_request_body(b"5").is_none());
        assert!(parse_request_body(&vec![b' '; 300_000]).is_none());
    }

    #[test]
    fn json_text_strips_an_xssi_prefix() {
        assert_eq!(parse_json_text(")]}'\n{\"a\":1}"), Some(json!({ "a": 1 })));
        assert_eq!(parse_json_text("{\"a\":2}"), Some(json!({ "a": 2 })));
        assert_eq!(parse_json_text(")]}{\"a\":3}"), Some(json!({ "a": 3 })));
        assert_eq!(parse_json_text("nope"), None);
        assert_eq!(parse_json_text(""), None);
    }

    #[test]
    fn sponsored_feed_subtrees_are_skipped_without_hiding_organic_videos() {
        let data = json!({"contents":[
            {"adSlotRenderer":{"inFeedAdLayoutRenderer":{"videoRenderer":video_renderer(ID1,json!({"title":{"simpleText":"Advertisement"}}))}}},
            {"promotedVideoRenderer":{"videoRenderer":video_renderer(ID2,json!({}))}},
            {"videoRenderer":video_renderer(ID1,json!({"title":{"simpleText":"Organic copy"}}))},
            {"videoRenderer":video_renderer(ID3,json!({"title":{"simpleText":"How sponsored videos work"}}))}
        ]});
        let items = extract_feed(&data, false);
        assert_eq!(ids(&items), [ID1, ID3]);
        assert_eq!(items[0].title, "Organic copy");
        assert_eq!(items[1].title, "How sponsored videos work");
    }

    // ---- robustness ----

    #[test]
    fn garbage_and_empty_inputs_never_panic_and_return_empty_results() {
        let garbage = [
            Value::Null,
            json!(0),
            json!(1),
            json!(""),
            json!("str"),
            json!(true),
            json!([]),
            json!({}),
            json!([[]]),
            json!([null, null]),
            json!({ "a": null }),
            json!({ "videoRenderer": null }),
            json!({ "videoRenderer": "x" }),
            json!({ "videoRenderer": [] }),
            json!({ "lockupViewModel": [] }),
            json!({ "commentThreadRenderer": 5 }),
            json!({ "videoDetails": [] }),
            json!({ "streamingData": { "formats": 5, "adaptiveFormats": [null, 1, "x", []] } }),
        ];
        for g in &garbage {
            let s = g.as_str().unwrap_or("");
            let _ = classify(s, g, Some(s));
            assert!(extract_feed(g, false).is_empty(), "{g}");
            assert!(extract_feed(g, true).is_empty(), "{g}");
            let c = extract_comments(g);
            assert!(c.comments.is_empty() && c.total.is_none(), "{g}");
            assert!(extract_video_meta(g).is_none(), "{g}");
            assert!(extract_formats(g).is_empty(), "{g}");
            let _ = initial_data_from_html(s);
        }
    }

    #[test]
    fn huge_nesting_does_not_overflow_the_stack() {
        let deep = json!({ "videoRenderer": video_renderer(ID1, json!({})) });
        // a 200000-deep chain hanging off a node that is not consumed by the feed walker
        let mut chain = json!({});
        for _ in 0..200_000 {
            chain = wrap("x", chain);
        }
        let mut root = serde_json::Map::new();
        root.insert("a".into(), deep);
        root.insert("b".into(), chain);
        let root = Value::Object(root);
        assert_eq!(extract_feed(&root, false).len(), 1);
        assert!(extract_comments(&root).comments.is_empty());
        assert!(extract_video_meta(&root).is_none());
        assert!(extract_formats(&root).is_empty());
        // dropping a 200000-deep Value would itself recurse; leak it on purpose
        std::mem::forget(root);
    }

    #[test]
    fn a_video_buried_deeper_than_the_depth_cap_is_not_found() {
        let mut o = json!({ "videoRenderer": video_renderer(ID1, json!({})) });
        for _ in 0..1000 {
            o = wrap("w", o);
        }
        assert!(extract_feed(&o, false).is_empty());
        std::mem::forget(o);
        // but a moderately deep one is
        let mut o = json!({ "videoRenderer": video_renderer(ID1, json!({})) });
        for _ in 0..300 {
            o = wrap("w", o);
        }
        assert_eq!(extract_feed(&o, false).len(), 1);
    }

    #[test]
    fn node_cap_bounds_work_on_huge_flat_inputs() {
        let mut big: Vec<Value> = (0..400_000).map(|i| json!({ "n": i })).collect();
        big.push(json!({ "videoRenderer": video_renderer(ID1, json!({})) }));
        let big = Value::Array(big);
        let t0 = std::time::Instant::now();
        let r = extract_feed(&big, false);
        assert!(t0.elapsed().as_secs() < 5);
        assert!(r.is_empty()); // past the cap
        assert_eq!(walk(&big, |_| Visit::Continue), MAX_NODES + 1);
    }

    #[test]
    fn walk_skip_and_stop() {
        let v = json!({ "a": { "b": { "c": {} } }, "d": [ { "e": 1 }, { "f": 2 } ] });
        // root, a, a.b, a.b.c, d, d[0], d[1]
        assert_eq!(walk(&v, |_| Visit::Continue), 7);
        // `a` holds the key "b": it is visited but its subtree is skipped
        assert_eq!(walk(&v, |n| if n.get("b").is_some() { Visit::SkipChildren } else { Visit::Continue }), 5);
        assert_eq!(walk(&v, |_| Visit::Stop), 1);
        assert_eq!(walk(&json!(5), |_| Visit::Continue), 0);
        // depth cap 1: root plus its two children
        assert_eq!(walk_with(&v, |_| Visit::Continue, 100, 1), 3);
        // node cap
        assert_eq!(walk_with(&v, |_| Visit::Continue, 4, 100), 5);
    }

    #[test]
    fn oversized_strings_are_clipped() {
        let big = "x".repeat(100_000);
        let it = &feed(&json!({ "videoRenderer": video_renderer(ID1, json!({ "title": { "simpleText": big.clone() } })) }))[0];
        assert!(it.title.chars().count() <= 500);
        let m = extract_video_meta(&json!({ "videoDetails": { "videoId": ID1, "title": "t", "shortDescription": big.clone() } })).unwrap();
        assert!(m.description.unwrap().chars().count() <= 5000);
        let c = extract_comments(&json!({ "commentRenderer": { "authorText": { "simpleText": "a" }, "contentText": { "simpleText": big } } }));
        assert_eq!(c.comments[0].text.chars().count(), 4000);
    }

    #[test]
    fn clip_collapses_whitespace_and_counts_characters() {
        assert_eq!(clip("  a \n\t b\u{a0}\u{a0}c  ", 100), "a b c");
        assert_eq!(clip("héllo wörld", 5), "héllo");
        assert_eq!(clip("", 5), "");
        assert_eq!(clip("abc", 0), "");
    }

    #[test]
    fn text_shapes() {
        assert_eq!(text(Some(&json!({ "simpleText": "s", "content": "c" }))), "s");
        assert_eq!(text(Some(&json!({ "content": "c" }))), "c");
        assert_eq!(text(Some(&json!({ "runs": [{ "text": "a" }, { "nope": 1 }, { "text": "b" }] }))), "ab");
        assert_eq!(text(Some(&json!("plain"))), "plain");
        assert_eq!(text(Some(&json!(12))), "12");
        assert_eq!(text(Some(&json!(true))), "");
        assert_eq!(text(Some(&json!([1]))), "");
        assert_eq!(text(None), "");
    }

    // ---- HTML ----

    fn player_response_json() -> String {
        // braces and quotes inside strings must not confuse the matcher
        r#"{"videoDetails":{"videoId":"dQw4w9WgXcQ","title":"Weird } title { with \"quotes\" and \\ backslash","lengthSeconds":"212","author":"Rick åäö 😀","shortDescription":"line1\nline2 };var x = {"},"streamingData":{"formats":[{"itag":18,"mimeType":"video/mp4; codecs=\"avc1\"","qualityLabel":"360p","contentLength":"1000"}]}}"#.to_string()
    }

    fn initial_data_json() -> String {
        json!({ "contents": { "twoColumnWatchNextResults": { "secondaryResults": { "secondaryResults": { "results": [
            { "compactVideoRenderer": video_renderer(ID2, json!({})) }
        ] } } } } })
        .to_string()
    }

    fn watch_page() -> String {
        format!(
            "<!DOCTYPE html><html><head><title>x</title></head><body>\
             <script nonce=\"abc\">var ytInitialPlayerResponse = {};var meta = document.createElement('meta');</script>\
             <script nonce=\"abc\">if (window.ytInitialData) {{ console.log(1); }}</script>\
             <script nonce=\"abc\">var ytInitialData = {};</script>\
             <script>window.ytcfg.set({{\"a\":1}});</script></body></html>",
            player_response_json(),
            initial_data_json()
        )
    }

    #[test]
    fn html_watch_page_has_both_blobs() {
        let d = initial_data_from_html(&watch_page());
        let pr = d.yt_initial_player_response.expect("player response");
        assert_eq!(pr["videoDetails"]["videoId"], ID1);
        assert_eq!(pr["videoDetails"]["title"], "Weird } title { with \"quotes\" and \\ backslash");
        assert_eq!(pr["videoDetails"]["author"], "Rick \u{e5}\u{e4}\u{f6} \u{1f600}");
        assert_eq!(pr["videoDetails"]["shortDescription"], "line1\nline2 };var x = {");
        let m = extract_video_meta(&pr).unwrap();
        assert_eq!(m.video_id, ID1);
        assert_eq!(extract_formats(&pr)[0].itag, 18);
        let data = d.yt_initial_data.expect("initial data");
        assert_eq!(ids(&extract_feed(&data, false)), [ID2]);
    }

    #[test]
    fn html_window_bracket_assignment_and_home_page() {
        let home = format!(
            "<html><body><script>window[\"ytInitialData\"] = {};\n</script><script>window['ytInitialPlayerResponse'] = {};</script></body></html>",
            json!({ "contents": { "richGridRenderer": { "contents": [{ "richItemRenderer": { "content": { "videoRenderer": video_renderer(ID1, json!({})) } } }] } } }),
            json!({ "a": 1 })
        );
        let d = initial_data_from_html(&home);
        assert_eq!(ids(&extract_feed(&d.yt_initial_data.unwrap(), false)), [ID1]);
        assert_eq!(d.yt_initial_player_response.unwrap()["a"], 1);
    }

    #[test]
    fn html_search_page_without_a_player_response() {
        let page = format!("<script>var ytInitialData  =\n   {};</script>", json!({ "contents": { "a": [{ "videoRenderer": video_renderer(ID3, json!({})) }] } }));
        let d = initial_data_from_html(&page);
        assert!(d.yt_initial_player_response.is_none());
        assert_eq!(ids(&extract_feed(&d.yt_initial_data.unwrap(), false)), [ID3]);
    }

    #[test]
    fn html_skips_references_without_an_assignment_and_uses_a_later_real_one() {
        let page = concat!(
            "<script>var a = \"ytInitialData\"; foo.ytInitialData; xytInitialData = {\"bad\":1}; ytInitialDataX = {\"bad\":2}; ",
            "ytInitialData == 5; {\"ytInitialData\": {\"bad\":3}}</script>",
            "<script>var ytInitialData = {\"good\":true};</script>"
        );
        let d = initial_data_from_html(page);
        assert_eq!(d.yt_initial_data.unwrap(), json!({ "good": true }));
    }

    #[test]
    fn html_json_parse_form() {
        let page = r#"<script>var ytInitialData = JSON.parse('\x7b\x22a\x22:\x22b\\\x22c\x22,\x22u\x22:\x22å\x22\x7d');</script>"#;
        let d = initial_data_from_html(page);
        assert_eq!(d.yt_initial_data.unwrap(), json!({ "a": "b\"c", "u": "\u{e5}" }));
        let bad = r#"<script>var ytInitialData = JSON.parse('\x7b\x22a');</script>"#;
        assert!(initial_data_from_html(bad).yt_initial_data.is_none());
    }

    #[test]
    fn html_truncated_malformed_and_hostile_pages_yield_none() {
        assert!(initial_data_from_html("").yt_initial_data.is_none());
        assert!(initial_data_from_html("var ytInitialData = ").yt_initial_data.is_none());
        assert!(initial_data_from_html("var ytInitialData = {").yt_initial_data.is_none());
        assert!(initial_data_from_html("var ytInitialData = {\"a\": \"unterminated").yt_initial_data.is_none());
        assert!(initial_data_from_html("var ytInitialData = {\"a\":}").yt_initial_data.is_none());
        assert!(initial_data_from_html("var ytInitialData = [1,2];").yt_initial_data.is_none());
        assert!(initial_data_from_html("var ytInitialData = }}}{").yt_initial_data.is_none());
        assert!(initial_data_from_html("ytInitialData\u{e5}\u{e5} = {}\u{1f600}").yt_initial_data.is_none());
        // a truncated first assignment does not hide a good second one
        let page = "var ytInitialData = {\"a\": [1,2; var ytInitialData = {\"ok\":1};";
        assert_eq!(initial_data_from_html(page).yt_initial_data, Some(json!({ "ok": 1 })));
        // unbalanced brace spam is rejected quickly
        let spam = format!("var ytInitialData = {};", "{\"a\":".repeat(50_000));
        let t0 = std::time::Instant::now();
        assert!(initial_data_from_html(&spam).yt_initial_data.is_none());
        assert!(t0.elapsed().as_secs() < 5);
        // thousands of non-assignment mentions stay bounded
        let many = "ytInitialData ".repeat(100_000);
        let t0 = std::time::Instant::now();
        assert!(initial_data_from_html(&many).yt_initial_data.is_none());
        assert!(t0.elapsed().as_secs() < 5);
        // deeper than serde_json's recursion limit: rejected, not a crash
        let deep = format!("var ytInitialData = {}{};", "{\"a\":".repeat(500), format!("1{}", "}".repeat(500)));
        assert!(initial_data_from_html(&deep).yt_initial_data.is_none());
    }

    #[test]
    fn html_large_realistic_page_is_found_quickly() {
        let rows: Vec<Value> = (0..3000).map(|i| json!({ "videoRenderer": video_renderer(&format!("vid{:08}", i), json!({})) })).collect();
        let data = json!({ "contents": { "rows": rows } }).to_string();
        let pad = "<div class=\"x\">padding {not json}</div>".repeat(5000);
        let page = format!("<html>{pad}<script>var ytInitialData = {data};</script>{pad}</html>");
        let t0 = std::time::Instant::now();
        let d = initial_data_from_html(&page);
        assert!(t0.elapsed().as_secs() < 5);
        assert_eq!(extract_feed(&d.yt_initial_data.unwrap(), false).len(), 3000);
    }
}
