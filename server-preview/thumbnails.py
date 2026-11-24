"""Serve only validated captured TikTok cover art through Muninn's owner-only gateway.

The browser receives no signed TikTok CDN URLs. Only images which have already
appeared in an authenticated Core feed snapshot may be fetched. No provider
cookies, redirects, private DNS destinations, arbitrary hostnames or HTML.
"""
from __future__ import annotations

import asyncio
from collections import OrderedDict
import ipaddress
import re
import socket
from urllib.parse import urlsplit

from aiohttp.abc import AbstractResolver

MAX_COVER_BYTES = 1024 * 1024
MAX_INDEX = 1500
CDN_DOMAINS = (
    "tiktokcdn.com", "tiktokcdn-us.com", "tiktokcdn-eu.com",
    "byteimg.com", "ibyteimg.com", "ibytedtos.com",
)
TIKTOK_ID = re.compile(r"tt_[0-9]{15,22}\Z")


def safe_cover_url(value):
    if not isinstance(value, str) or len(value) > 2048 or any(ord(c) < 32 for c in value):
        return None
    try:
        url = urlsplit(value)
        if (url.scheme != "https" or not url.hostname or url.username or url.password
                or url.port not in (None, 443) or url.fragment
                or not url.hostname.isascii() or url.hostname.endswith(".")):
            return None
        host = url.hostname.lower()
    except ValueError:
        return None
    if not any(host == domain or host.endswith("." + domain) for domain in CDN_DOMAINS):
        return None
    return value


def remember_covers(index: OrderedDict, feed):
    """A bounded image-origin index derived only from the authenticated feed API."""
    if not isinstance(feed, dict) or not isinstance(feed.get("items"), list):
        return
    for item in feed["items"][:500]:
        if not isinstance(item, dict):
            continue
        ident = item.get("video_id")
        cover = safe_cover_url(item.get("thumbnail"))
        if not isinstance(ident, str) or not TIKTOK_ID.fullmatch(ident) or cover is None:
            continue
        index.pop(ident, None)
        index[ident] = cover
        while len(index) > MAX_INDEX:
            index.popitem(last=False)


class PublicResolver(AbstractResolver):
    """Pin TLS requests to public DNS results so CDN-host SSRF cannot reach LAN."""
    async def resolve(self, host, port=0, family=socket.AF_INET):
        if not any(host == domain or host.endswith("." + domain) for domain in CDN_DOMAINS):
            raise OSError("Unapproved cover host")
        loop = asyncio.get_running_loop()
        results = await loop.getaddrinfo(host, port, family=family, type=socket.SOCK_STREAM)
        if not results:
            raise OSError("Cover DNS returned no addresses")
        addresses = []
        for result in results:
            address = result[4][0]
            if not ipaddress.ip_address(address).is_global:
                raise OSError("Cover DNS returned a non-public address")
            addresses.append({"hostname": host, "host": address, "port": port,
                              "family": result[0], "proto": result[2], "flags": 0})
        return addresses

    async def close(self):
        pass


def image_type(body, declared):
    """Refuse HTML, SVG and bogus content types even when a CDN labels them images."""
    declared = declared.split(";", 1)[0].strip().lower()
    if declared == "image/jpeg" and body.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if declared == "image/png" and body.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if declared == "image/webp" and len(body) >= 12 and body[:4] == b"RIFF" and body[8:12] == b"WEBP":
        return "image/webp"
    return None
