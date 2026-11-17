"""Container entry point and authenticated gateway; never exposes Chromium/CDP."""
from __future__ import annotations
import asyncio
import contextlib
import json
import os
from pathlib import Path
from urllib.parse import urlsplit
from aiohttp import web, ClientSession, ClientTimeout
from browser import Browser

HOP = {"connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade", "content-length"}
ROOT = Path(__file__).resolve().parent


def config():
    origin = os.environ["MUNINN_WEB_ORIGIN"].rstrip("/")
    p = urlsplit(origin)
    if p.scheme not in {"https", "http"} or not p.netloc or p.path or p.query or p.fragment or p.username or p.password:
        raise RuntimeError("MUNINN_WEB_ORIGIN must be an exact origin")
    if p.scheme != "https" and p.hostname not in {"localhost", "127.0.0.1", "::1"}:
        raise RuntimeError("Remote access requires HTTPS")
    return origin, p.netloc.lower()


def validate_request(request, origin: str, authority: str):
    if request.host.lower() != authority:
        raise web.HTTPForbidden(text="Unexpected host")
    request_origin = request.headers.get("Origin")
    if request_origin is not None and request_origin != origin:
        raise web.HTTPForbidden(text="Unexpected origin")
    if request.method not in {"GET", "HEAD"} and (
        request_origin != origin or request.headers.get("X-Muninn-Client") != "pwa"
    ):
        raise web.HTTPForbidden(text="Browser action requires same-origin PWA request")


@web.middleware
async def boundary(request, handler):
    validate_request(request, request.app["origin"], request.app["authority"])
    try:
        response = await handler(request)
    except web.HTTPException:
        raise
    except ValueError as error:
        response = web.json_response({"error": str(error)}, status=400)
    except Exception:
        # Provider errors, action text and URLs may contain private account data.
        response = web.json_response({"error": "Server browser operation failed. Reload the tab or reset the browser session."}, status=502)
    if not response.prepared:
        response.headers.update({"Cache-Control": "no-store", "X-Content-Type-Options": "nosniff", "Referrer-Policy": "no-referrer"})
    return response


async def authenticated(request):
    headers = {"Host": request.app["authority"], "Cookie": request.headers.get("Cookie", "")}
    async with request.app["http"].get("http://127.0.0.1:47872/api/session", headers=headers, allow_redirects=False) as response:
        if response.status != 200:
            raise web.HTTPUnauthorized(text="Connect to Muninn before using the server browser")


async def browser_state(request):
    await authenticated(request)
    return web.json_response(await request.app["browser"].state())


async def browser_action(request):
    await authenticated(request)
    value = await request.json()
    if not isinstance(value, dict):
        raise ValueError("Expected an action object")
    return web.json_response(await request.app["browser"].action(value))


async def browser_frame(request):
    await authenticated(request)
    image = await request.app["browser"].frame(request.query.get("tab", ""))
    return web.Response(body=image, content_type="image/jpeg")


async def asset(request):
    files = {"browser.js": ("client.js", "text/javascript"), "browser.css": ("client.css", "text/css")}
    name, mime = files[request.match_info["asset"]]
    return web.Response(body=(ROOT / name).read_bytes(), content_type=mime)


async def proxy(request):
    # The Rust remote adapter still owns its existing operation allowlist, session
    # lifecycle, CSRF checks and media transfer semantics. Private capture port is
    # never a forwarding target, and arbitrary URLs/headers are not accepted.
    headers = {key: value for key, value in request.headers.items()
               if key.lower() in {"host", "origin", "cookie", "content-type", "range", "if-range", "x-muninn-client", "accept"}}
    body = await request.read()
    url = "http://127.0.0.1:47872" + request.raw_path
    async with request.app["http"].request(request.method, url, headers=headers, data=body or None, allow_redirects=False) as upstream:
        copied = [(key, value) for key, value in upstream.headers.items() if key.lower() not in HOP]
        if request.path in {"/app/", "/app/index.html"} and upstream.status == 200:
            text = await upstream.text()
            text = text.replace("</head>", '<link rel="stylesheet" href="/app/browser.css"><script src="/app/browser.js" defer></script></head>')
            return web.Response(body=text.encode(), status=upstream.status, headers=copied)
        if request.path == "/app/sw.js" and upstream.status == 200:
            text = await upstream.text()
            text = text.replace("muninn-pwa-shell-v1", "muninn-pwa-shell-browser-v1").replace("const SHELL = [", "const SHELL = ['/app/browser.js','/app/browser.css',")
            return web.Response(body=text.encode(), headers=copied)
        response = web.StreamResponse(status=upstream.status, headers=copied)
        if "Content-Length" in upstream.headers:
            response.headers["Content-Length"] = upstream.headers["Content-Length"]
        await response.prepare(request)
        if request.method != "HEAD":
            async for chunk in upstream.content.iter_chunked(256 * 1024):
                await response.write(chunk)
        await response.write_eof()
        return response


async def lifecycle(app):
    data = Path(os.environ.get("MUNINN_DATA_DIR", "/var/lib/muninn"))
    private = data / "browser"
    private.mkdir(parents=True, exist_ok=True, mode=0o700)
    # Token comes from an operator-managed file; no generated default password.
    token_file = Path(os.environ["MUNINN_WEB_TOKEN_FILE"])
    if not token_file.is_file():
        raise RuntimeError("Missing Muninn access-token file")
    env = {**os.environ, "MUNINN_WEB_BIND": "127.0.0.1:47872", "MUNINN_BROWSER_CAPTURE": "1",
           "MUNINN_BROWSER_COOKIE_FILE": str(private / "youtube-cookies.json")}
    child = await asyncio.create_subprocess_exec("muninn-web", env=env)
    http = ClientSession(timeout=ClientTimeout(total=None, connect=10, sock_read=60), auto_decompress=False)
    app["http"] = http
    browser = None
    try:
        for _ in range(100):
            if child.returncode is not None:
                raise RuntimeError("Muninn core exited during startup")
            try:
                async with http.get("http://127.0.0.1:47872/health", headers={"Host": app["authority"]}) as response:
                    if response.status == 200:
                        break
            except OSError:
                pass
            await asyncio.sleep(.2)
        else:
            raise RuntimeError("Muninn core did not become healthy")
        core_token = (data / "token").read_text().strip()
        browser = Browser(private, http, "http://127.0.0.1:47873", core_token)
        app["browser"] = browser
        async def watch_core():
            await child.wait()
            # PID 1 exits nonzero and Compose restarts the complete unit.
            os._exit(1)
        watcher = asyncio.create_task(watch_core())
        try:
            yield
        finally:
            watcher.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await watcher
    finally:
        if browser:
            await browser.close()
        await http.close()
        if child.returncode is None:
            child.terminate()
            try:
                await asyncio.wait_for(child.wait(), 10)
            except asyncio.TimeoutError:
                child.kill()
                await child.wait()


def create_app():
    origin, authority = config()
    app = web.Application(middlewares=[boundary], client_max_size=16 * 1024)
    app["origin"], app["authority"] = origin, authority
    app.cleanup_ctx.append(lifecycle)
    app.router.add_get("/browser/state", browser_state)
    app.router.add_post("/browser/action", browser_action)
    app.router.add_get("/browser/frame", browser_frame)
    app.router.add_get(r"/app/{asset:browser\.js|browser\.css}", asset)
    app.router.add_route("*", "/{tail:.*}", proxy)
    return app


if __name__ == "__main__":
    web.run_app(create_app(), host="0.0.0.0", port=8080, access_log=None, print=None, shutdown_timeout=15)
