"""Small, testable boundary for the single-owner YouTube browser."""
from __future__ import annotations
from urllib.parse import urlsplit, urlunsplit, parse_qsl, urlencode

NAV_HOSTS = frozenset({"www.youtube.com", "youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be", "accounts.google.com", "consent.google.com", "consent.youtube.com"})
RESOURCE_DOMAINS = ("youtube.com", "ytimg.com", "ggpht.com", "gstatic.com", "googleapis.com", "googleusercontent.com")
RESOURCE_HOSTS = NAV_HOSTS | {"www.google.com", "accounts.youtube.com", "play.google.com"}
MAX_CAPTURE = 4 * 1024 * 1024
MAX_TABS = 4
WIDTH, HEIGHT = 1280, 800


def parsed_https(value: str):
    if not isinstance(value, str) or len(value) > 4096 or any(ord(c) < 32 for c in value):
        raise ValueError("Invalid URL")
    p = urlsplit(value)
    if p.scheme != "https" or p.username or p.password or p.port not in (None, 443):
        raise ValueError("Only HTTPS provider URLs are allowed")
    return p


def navigation_url(value: str) -> str:
    p = parsed_https(value)
    if p.hostname not in NAV_HOSTS:
        raise ValueError("Only YouTube and its Google sign-in pages can be opened")
    return urlunsplit(("https", p.netloc, p.path or "/", p.query, ""))


def request_allowed(value: str, resource_type: str, navigation: bool) -> bool:
    try:
        p = parsed_https(value)
    except (ValueError, TypeError):
        return False
    if resource_type == "media" or p.hostname == "googlevideo.com" or (p.hostname or "").endswith(".googlevideo.com"):
        return False  # Never relay upstream video/audio through the browsing canvas.
    if navigation:
        return p.hostname in NAV_HOSTS
    return p.hostname in RESOURCE_HOSTS or any(p.hostname == d or (p.hostname or "").endswith("." + d) for d in RESOURCE_DOMAINS)


def capture_kind(value: str, content_type: str) -> str | None:
    try:
        p = parsed_https(value)
    except ValueError:
        return None
    if p.hostname not in {"www.youtube.com", "youtube.com", "m.youtube.com"}:
        return None
    if p.path in {"/youtubei/v1/browse", "/youtubei/v1/search", "/youtubei/v1/next", "/youtubei/v1/player"} and "json" in content_type:
        return "application/json"
    if "text/html" in content_type and not p.path.startswith(("/s/", "/embed")):
        return "text/html"
    return None


def public_url(value: str) -> str:
    """Never expose Google login query parameters or provider auth identifiers."""
    try:
        p = parsed_https(value)
    except ValueError:
        return ""
    if p.hostname not in NAV_HOSTS:
        return ""
    query = urlencode([(k, v) for k, v in parse_qsl(p.query) if k in {"v", "list", "index", "search_query"}]) if p.hostname.endswith("youtube.com") or p.hostname == "youtu.be" else ""
    return urlunsplit((p.scheme, p.netloc, p.path or "/", query, ""))


def video_id(value: str) -> str | None:
    import re
    try:
        p = parsed_https(value)
    except ValueError:
        return None
    if p.hostname not in {"youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be"}:
        return None
    query = dict(parse_qsl(p.query))
    parts = p.path.strip("/").split("/")
    candidate = parts[0] if p.hostname == "youtu.be" else query.get("v", "") if p.path == "/watch" else parts[1] if len(parts) > 1 and parts[0] in {"shorts", "live"} else ""
    return candidate if re.fullmatch(r"[A-Za-z0-9_-]{11}", candidate) else None


def safe_request_body(value: str | None) -> str | None:
    import json
    if not value or len(value) > 65536:
        return None
    try:
        body = json.loads(value)
    except (ValueError, TypeError):
        return None
    if not isinstance(body, dict):
        return None
    return json.dumps({k: body[k] for k in ("browseId", "query", "videoId", "continuation") if isinstance(body.get(k), str)}, separators=(",", ":"))


def key_allowed(key: str) -> bool:
    return key in {"Enter", "Tab", "Shift+Tab", "Backspace", "Delete", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Space", "Control+A", "Meta+A"}
