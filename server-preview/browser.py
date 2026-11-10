"""Persistent server Chromium; only bounded captures and rendered UI cross out.

No arbitrary script evaluation, CDP endpoint, filesystem upload, or download is
exposed to clients. Google credentials remain in this dedicated server profile.
"""
from __future__ import annotations
import asyncio
import contextlib
import json
import os
import time
import uuid
from pathlib import Path
from policy import (MAX_CAPTURE, MAX_TABS, WIDTH, HEIGHT, navigation_url, request_allowed,
                    capture_kind, public_url, video_id, safe_request_body, key_allowed)

STOP_MEDIA = """(() => {
 const stop = e => { if(e instanceof HTMLMediaElement) { e.pause(); e.muted=true; } };
 document.addEventListener('play', e => stop(e.target), true);
 setInterval(() => document.querySelectorAll('video,audio').forEach(stop), 500);
})()"""


class Browser:
    def __init__(self, root: Path, http, capture_origin: str, token: str):
        self.root, self.http, self.capture_origin, self.token = root, http, capture_origin, token
        self.context = self.driver = None
        self.pages = {}
        self.lock = asyncio.Lock()
        self.cookie_lock = asyncio.Lock()
        self.queue = asyncio.Queue(maxsize=16)
        self.consumer = None
        self.cookie_task = None
        self.captured = self.dropped = self.failures = 0
        self.last_capture = None
        self.epoch = 0
        self.last_activity = time.monotonic()
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.root, 0o700)

    async def start(self):
        if self.context:
            return
        from patchright.async_api import async_playwright
        self.driver = await async_playwright().start()
        try:
            self.context = await self.driver.chromium.launch_persistent_context(
                str(self.root / "profile"), headless=True, viewport={"width": WIDTH, "height": HEIGHT},
                accept_downloads=False, service_workers="block", chromium_sandbox=True,
                args=["--disable-dev-shm-usage", "--mute-audio", "--disable-background-networking"])
            await self.context.route("**/*", self._route)
            await self.context.add_init_script(STOP_MEDIA)
            self.context.on("response", self._response)
            self.context.on("page", lambda page: asyncio.create_task(self._register(page)))
            self.consumer = asyncio.create_task(self._consume())
            self.cookie_task = asyncio.create_task(self._cookie_loop())
            for page in list(self.context.pages):
                await self._register(page)
        except BaseException:
            await self.close()
            raise

    async def _route(self, route, request):
        if request_allowed(request.url, request.resource_type, request.is_navigation_request()):
            await route.continue_()
        else:
            await route.abort("blockedbyclient")

    async def _register(self, page):
        if page in self.pages.values():
            return
        if len(self.pages) >= MAX_TABS:
            await page.close()
            return
        ident = uuid.uuid4().hex
        self.pages[ident] = page
        page.on("download", lambda download: asyncio.create_task(download.cancel()))
        page.on("close", lambda: self._closed(ident, page))
        page.on("framenavigated", lambda frame: self._queue_tab(ident, frame) if frame == page.main_frame else None)
        if page.url not in {"about:blank", ""}:
            try:
                navigation_url(page.url)
            except ValueError:
                await page.close()

    def _closed(self, ident, page):
        self.pages.pop(ident, None)
        url = public_url(page.url)
        if url.startswith(("https://www.youtube.com/", "https://youtube.com/", "https://m.youtube.com/")):
            try:
                self.queue.put_nowait(("tab", {"tab_id": "server-" + ident, "url": url,
                    "video_id": video_id(url), "state": "closed"}, self.epoch))
            except asyncio.QueueFull:
                self.dropped += 1

    def _queue_tab(self, ident, frame):
        # SPA navigations also fire framenavigated; background watch tabs retain
        # state=open, so the existing Core OpenTab encounter fires normally.
        url = public_url(frame.url)
        if not url or "youtube.com" not in url and "youtu.be" not in url:
            return
        item = ("tab", {"tab_id": "server-" + ident, "url": url, "video_id": video_id(url), "state": "open"}, self.epoch)
        try:
            self.queue.put_nowait(item)
        except asyncio.QueueFull:
            self.dropped += 1

    def _response(self, response):
        kind = capture_kind(response.url, response.headers.get("content-type", ""))
        if not kind:
            return
        try:
            size = int(response.headers.get("content-length", "0"))
        except ValueError:
            size = MAX_CAPTURE + 1
        if size > MAX_CAPTURE:
            self.dropped += 1
            return
        request = response.request
        value = {"url": public_url(response.url), "referer": public_url(request.headers.get("referer", "")) or None,
                 "request_body": safe_request_body(request.post_data), "content_type": kind}
        try:
            self.queue.put_nowait(("capture", (response, value), self.epoch))
        except asyncio.QueueFull:
            self.dropped += 1

    async def _post(self, endpoint, payload):
        async with self.http.post(self.capture_origin + "/" + endpoint, json=payload,
                                  headers={"Authorization": "Bearer " + self.token}) as result:
            if result.status != 200:
                raise RuntimeError("Capture delivery failed")
            return await result.json()

    async def _consume(self):
        while True:
            kind, item, epoch = await self.queue.get()
            try:
                if epoch != self.epoch:
                    continue
                if kind == "capture":
                    response, value = item
                    body = await asyncio.wait_for(response.body(), timeout=8)
                    if len(body) > MAX_CAPTURE:
                        self.dropped += 1
                        continue
                    value["body"] = body.decode("utf-8", errors="replace")
                    result = await self._post("capture", value)
                    self.captured += 1
                    # Only metadata counts, never raw response, cookies or provider errors.
                    self.last_capture = {k: result.get(k) for k in ("kind", "items", "comments", "snapshot_id")}
                else:
                    await self.sync_cookies()
                    await self._post("tab", item)
            except asyncio.CancelledError:
                raise
            except Exception:
                self.failures += 1  # Capture failure never prevents page browsing.
            finally:
                self.queue.task_done()

    async def sync_cookies(self):
        async with self.cookie_lock:
            await self._sync_cookies()

    async def _sync_cookies(self):
        if not self.context:
            return
        rows = await self.context.cookies(["https://www.youtube.com", "https://youtube.com"])
        cookies = [{"domain": c["domain"], "path": c["path"], "name": c["name"], "value": c["value"],
                    "secure": c["secure"], "http_only": c["httpOnly"],
                    "expires": int(c["expires"]) if c.get("expires", -1) > 0 else None}
                   for c in rows if c["domain"].lstrip(".") == "youtube.com" or c["domain"].endswith(".youtube.com")]
        target = self.root / "youtube-cookies.json"
        temp = self.root / ".youtube-cookies.tmp"
        fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as file:
            json.dump(cookies, file)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temp, target)

    async def _cookie_loop(self):
        while True:
            try:
                await self.sync_cookies()
            except Exception:
                self.failures += 1
            await asyncio.sleep(10)

    async def state(self):
        tabs = []
        for ident, page in list(self.pages.items()):
            if page.is_closed():
                continue
            # Sign-in page titles can contain identity; keep them generic.
            url = public_url(page.url)
            title = "Google sign-in" if "google.com" in url else (await page.title())[:200]
            tabs.append({"id": ident, "url": url, "title": title or "New tab", "video_id": video_id(page.url)})
        return {"running": bool(self.context), "tabs": tabs, "width": WIDTH, "height": HEIGHT,
                "captured": self.captured, "dropped": self.dropped, "failures": self.failures, "last_capture": self.last_capture}

    def page(self, ident):
        if ident not in self.pages or self.pages[ident].is_closed():
            raise ValueError("Browser tab no longer exists")
        return self.pages[ident]

    async def action(self, command):
        async with self.lock:
            self.last_activity = time.monotonic()
            kind = command.get("action")
            if kind == "start":
                await self.start()
                if not self.pages:
                    await self._register(await self.context.new_page())
                page = next(iter(self.pages.values()))
                if page.url == "about:blank":
                    await page.goto("https://www.youtube.com/", wait_until="domcontentloaded")
            elif kind == "new":
                url = navigation_url(command.get("url", "https://www.youtube.com/"))
                await self.start()
                if len(self.pages) >= MAX_TABS:
                    raise ValueError("Close a tab before opening another (four-tab limit)")
                page = await self.context.new_page()
                await self._register(page)
                await page.goto(url, wait_until="domcontentloaded")
            elif kind == "reset":
                await self.close()
                import shutil
                shutil.rmtree(self.root / "profile", ignore_errors=False) if (self.root / "profile").exists() else None
                (self.root / "youtube-cookies.json").unlink(missing_ok=True)
            else:
                page = self.page(command.get("tab"))
                if kind == "navigate":
                    await page.goto(navigation_url(command.get("url", "")), wait_until="domcontentloaded")
                elif kind == "back":
                    await page.go_back(wait_until="domcontentloaded")
                elif kind == "reload":
                    await page.reload(wait_until="domcontentloaded")
                elif kind == "close":
                    await page.close()
                elif kind == "click":
                    x, y = float(command.get("x", -1)), float(command.get("y", -1))
                    if not (0 <= x < WIDTH and 0 <= y < HEIGHT):
                        raise ValueError("Click outside browser viewport")
                    await page.mouse.click(x, y)
                elif kind == "scroll":
                    dy = max(-1600, min(1600, int(command.get("dy", 0))))
                    await page.mouse.wheel(0, dy)
                elif kind == "text":
                    text = command.get("text")
                    if not isinstance(text, str) or len(text) > 2048:
                        raise ValueError("Text is too long")
                    await page.keyboard.insert_text(text)
                elif kind == "key":
                    if not key_allowed(command.get("key")):
                        raise ValueError("Unsupported key")
                    await page.keyboard.press(command["key"])
                else:
                    raise ValueError("Unknown browser action")
            if self.context:
                await self.sync_cookies()
            return await self.state()

    async def frame(self, ident):
        async with self.lock:
            return await self.page(ident).screenshot(type="jpeg", quality=70, timeout=8000)

    async def close(self):
        self.epoch += 1
        for task in (self.consumer, self.cookie_task):
            if task:
                task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await task
        if self.context:
            await self.context.close()
        if self.driver:
            await self.driver.stop()
        self.context = self.driver = self.consumer = self.cookie_task = None
        self.pages.clear()
        self.queue = asyncio.Queue(maxsize=16)
