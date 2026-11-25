"""Runs inside the exact Dagger image. Real Chromium + core; synthetic YouTube.

No personal Google account, live video or provider availability is required.
"""
import asyncio
import json
import os
import re
import tempfile
from pathlib import Path
from urllib.parse import urlsplit
from aiohttp import ClientSession, web
from browser import Browser

async def mobile_browse_smoke(browser, http, base, origin, headers):
    """Exercise shipped PWA HTML/CSS/JS in a real, fresh touch-enabled Chromium.

    The isolated test origin is mapped to the actual gateway's HTTP listener;
    authentication uses the synthetic pairing session. Only provider-browser
    control is redirected to the already-fixtured Browser, so there is no
    external provider traffic, owner profile or live download. This is not
    installed Safari, service-worker lifecycle or live Authelia acceptance.
    """
    frontend = await browser.driver.chromium.launch(
        headless=True, chromium_sandbox=True,
        args=["--disable-dev-shm-usage", "--disable-background-networking"])
    try:
        context = await frontend.new_context(
            viewport={"width":390,"height":844}, is_mobile=True, has_touch=True,
            device_scale_factor=2, service_workers="block")
        navigations = []
        async def serve(route):
            req = route.request
            url = urlsplit(req.url)
            if (url.scheme + "://" + url.netloc) != origin:
                await route.abort()
                return
            path = url.path + ("?" + url.query if url.query else "")
            if url.path == "/browser/state" and req.method == "GET":
                await route.fulfill(status=200, json=await browser.state())
            elif url.path == "/browser/action" and req.method == "POST":
                action = json.loads(req.post_data or "{}")
                if action.get("action") not in {"new", "navigate", "reload"}:
                    await route.fulfill(status=400, json={"error":"Unexpected fixture action"})
                    return
                if action.get("action") != "reload" and action.get("url") not in {
                    "https://www.youtube.com/", "https://www.tiktok.com/foryou"
                }:
                    await route.fulfill(status=400, json={"error":"Unexpected fixture destination"})
                    return
                navigations.append(action)
                await route.fulfill(status=200, json=await browser.action(action))
            elif req.method in {"GET", "HEAD"}:
                async with http.request(req.method, base + path, headers=headers,
                                        allow_redirects=False) as response:
                    # Keep the actual gateway status, bytes and security policy.
                    response_headers = {k:v for k,v in response.headers.items()
                        if k.lower() in {"content-type", "cache-control",
                                        "content-security-policy", "x-content-type-options",
                                        "referrer-policy", "location"}}
                    await route.fulfill(status=response.status, headers=response_headers,
                                        body=await response.read())
            else:
                await route.fulfill(status=405)
        await context.route("**/*", serve)
        page = await context.new_page()
        page.set_default_timeout(15000)
        await page.goto(origin + "/app/?utm_source=chatgpt.com", wait_until="domcontentloaded")
        youtube = page.locator(".muninn-discover-card").filter(has_text="Server browser fixture").first
        await youtube.wait_for(state="visible")
        await youtube.locator(".muninn-discover-art-button").tap()
        await page.locator(".muninn-discover-watch h2").filter(has_text="Server browser fixture").wait_for(state="visible")
        await page.locator(".muninn-watch-back").tap()
        await page.locator("#pwa-menu-toggle").tap()
        await page.locator('[data-browse-source="tiktok-foryou"]').tap()
        tiktok = page.locator(".muninn-discover-card").filter(has_text="TikTok browser fixture").first
        await tiktok.wait_for(state="visible")
        await tiktok.locator(".muninn-discover-art-button").tap()
        await page.locator(".muninn-discover-watch h2").filter(has_text="TikTok browser fixture").wait_for(state="visible")
        await page.locator(".muninn-watch-back").tap()
        # Library and Browse must remain accessible from the same mobile menu.
        await page.locator("#pwa-menu-toggle").tap()
        await page.locator('#sidebar [data-view="server"]').last.tap()
        await page.locator("#browser-host").wait_for(state="hidden")
        await page.locator("#pwa-menu-toggle").tap()
        await page.locator('[data-browse-source="youtube-home"]').tap()
        await youtube.wait_for(state="visible")
        assert await page.locator("#login-panel").is_hidden()
        print("Mobile Chromium DOM smoke passed: tracked URL, real gateway assets and captured feeds, touch cards, YouTube/TikTok menu and library return.")
    finally:
        await frontend.close()


async def installed_worker_smoke(browser, http, base, headers):
    """Run the actual gateway worker through install, waiting and activation.

    A loopback-only fixture origin is a browser secure context. It forwards
    only the fixed static shell paths to the paired test gateway, never
    provider traffic, media, private APIs or owner credentials. Unlike
    Playwright route.fulfill, this also serves worker-initiated requests.
    """
    async with http.get(base + "/app/sw.js", headers=headers) as response:
        assert response.status == 200
        current = await response.text()
    match = re.search(r"const CACHE = '(muninn-pwa-shell-browser-v5-[0-9a-f]{64})';", current)
    assert match, "Gateway must serve content-versioned worker"
    current_cache = match.group(1)
    old_cache = "muninn-pwa-shell-browser-fixture-old"
    browse_assets = ("/app/discovery.js", "/app/browser.js", "/app/browser.css")
    old = current.replace(current_cache, old_cache).replace(
        "'/app/discovery.js','/app/browser.js','/app/browser.css',", "")
    assert old != current and all(p not in old for p in browse_assets)
    shell_paths = set(re.findall(r"'/app/[^']*'", current))
    shell_paths = {p[1:-1] for p in shell_paths}
    assert set(browse_assets) <= shell_paths
    generation = {"worker": old}

    async def serve(request):
        path = request.path
        if path in {"/worker-harness", "/app/worker-harness"}:
            return web.Response(text="<html><title>Worker test</title></html>",
                                content_type="text/html", headers={"Cache-Control":"no-store"})
        if path == "/app/sw.js":
            return web.Response(text=generation["worker"], content_type="text/javascript",
                                headers={"Cache-Control":"no-store"})
        if request.method != "GET" or path not in shell_paths:
            raise web.HTTPNotFound()
        async with http.get(base + path, headers=headers, allow_redirects=False) as response:
            assert response.status == 200, (path, response.status)
            return web.Response(body=await response.read(), status=response.status,
                                headers={"Content-Type":response.headers["Content-Type"],
                                         "Cache-Control":"no-store"})

    app = web.Application()
    app.router.add_get("/{path:.*}", serve)
    runner = web.AppRunner(app)
    await runner.setup()
    frontend = None
    try:
        site = web.TCPSite(runner, "127.0.0.1", 0)
        await site.start()
        address = "http://127.0.0.1:" + str(runner.addresses[0][1])
        frontend = await browser.driver.chromium.launch(
            headless=True, chromium_sandbox=True,
            args=["--disable-dev-shm-usage", "--disable-background-networking"])
        context = await frontend.new_context(service_workers="allow")
        page = await context.new_page()
        await page.goto(address + "/app/worker-harness")
        await page.evaluate("navigator.serviceWorker.register('/app/sw.js', {scope:'/app/'})")
        await page.wait_for_function("!!navigator.serviceWorker.controller", timeout=30000)
        assert old_cache in await page.evaluate("caches.keys()")
        assert not await page.evaluate("caches.match('/app/discovery.js').then(Boolean)")
        await page.evaluate("""async () => {
            await caches.open('unrelated-fixture-cache');
            await new Promise((resolve, reject) => {
                const request = indexedDB.open('offline-device-fixture', 1);
                request.onupgradeneeded = () => request.result.createObjectStore('videos');
                request.onerror = () => reject(request.error);
                request.onsuccess = () => {
                    const db = request.result;
                    const tx = db.transaction('videos', 'readwrite');
                    tx.objectStore('videos').put('retained', 'video');
                    tx.oncomplete = () => { db.close(); resolve(); };
                    tx.onerror = () => reject(tx.error);
                };
            });
        }""")
        generation["worker"] = current
        await page.evaluate("navigator.serviceWorker.getRegistration().then(r => r.update())")
        await page.wait_for_function(
            "navigator.serviceWorker.getRegistration().then(r => !!r.waiting)", timeout=30000)
        # A new version must not take over a page with an active transfer.
        assert old_cache in await page.evaluate("caches.keys()")
        assert current_cache in await page.evaluate("caches.keys()")
        observer = await context.new_page()
        await observer.goto(address + "/worker-harness")
        await page.close()
        await observer.wait_for_function(
            "(old) => caches.keys().then(keys => !keys.includes(old))",
            arg=old_cache, timeout=30000)
        assert "unrelated-fixture-cache" in await observer.evaluate("caches.keys()")
        retained = await observer.evaluate("""() => new Promise((resolve, reject) => {
            const request = indexedDB.open('offline-device-fixture', 1);
            request.onerror = () => reject(request.error);
            request.onsuccess = () => {
                const db = request.result;
                const get = db.transaction('videos').objectStore('videos').get('video');
                get.onsuccess = () => { db.close(); resolve(get.result); };
                get.onerror = () => reject(get.error);
            };
        })""")
        assert retained == "retained"
        await observer.goto(address + "/app/worker-harness")
        await observer.wait_for_function("!!navigator.serviceWorker.controller", timeout=30000)
        await context.set_offline(True)
        for path in browse_assets:
            result = await observer.evaluate(
                "p => fetch(p).then(async r => [r.ok, (await r.text()).length])", path)
            assert result[0] and result[1] > 0, (path, result)
        assert not await observer.evaluate("caches.match('/api/session').then(Boolean)")
        await context.set_offline(False)
        print("Installed-worker Chromium smoke passed: stale generation waits for old client, activates after close, retains unrelated cache and IndexedDB, serves Browse assets offline.")
    finally:
        if frontend is not None:
            await frontend.close()
        await runner.cleanup()


async def main():
    origin=os.environ['MUNINN_WEB_ORIGIN'];authority=urlsplit(origin).netloc
    async with ClientSession() as http:
        headers={'Host':authority,'Origin':origin,'X-Muninn-Client':'pwa'}
        base='http://127.0.0.1:8080'
        async with http.get(base+'/browser/state',headers=headers) as r:assert r.status==401
        async with http.post(base+'/api/session',json={'token':Path(os.environ['MUNINN_WEB_TOKEN_FILE']).read_text().strip()},headers=headers) as r:
            assert r.status==200, r.status
            cookie=r.headers['Set-Cookie'].split(';')[0]
        headers['Cookie']=cookie
        async with http.get(base+'/browser/state',headers=headers) as r:
            assert r.status==200;assert (await r.json())['running'] is False
        bad={**headers,'Origin':'https://evil.test'}
        async with http.post(base+'/browser/action',json={'action':'start'},headers=bad) as r:assert r.status==403
        for route in ['/capture','/tab','/api/settings']:
            async with http.get(base+route,headers=headers) as r:assert r.status==404
        async with http.get(base+'/app/',headers=headers) as r:assert 'browser.js' in await r.text()
        async with http.get(base+'/app/sw.js',headers=headers) as r:assert '/app/browser.js' in await r.text()
        with tempfile.TemporaryDirectory() as d:
            browser=Browser(Path(d),http,'http://127.0.0.1:47873',(Path(os.environ['MUNINN_DATA_DIR'])/'token').read_text().strip())
            try:
                await browser.start()
                fixture={'contents':{'items':[{'videoRenderer':{'videoId':'aaaaaaaaaaa','title':{'simpleText':'Server browser fixture'}}}]}}
                html='<html><title>Fixture home</title><script>var ytInitialData = '+json.dumps(fixture)+';</script><h1>Fixture home</h1></html>'
                async def fulfill(route):await route.fulfill(status=200,content_type='text/html',body=html)
                await browser.context.route('https://www.youtube.com/**',fulfill)
                state=await browser.action({'action':'start'})
                await asyncio.wait_for(browser.queue.join(),10)
                assert browser.captured>=1, await browser.state()
                image=await browser.frame(state['tabs'][0]['id']);assert image[:2]==b'\xff\xd8'
                await browser.action({'action':'new','url':'https://www.youtube.com/feed/trending'})
                assert len((await browser.state())['tabs'])==2
                # TikTok enters the same private Chromium -> native Core ->
                # feed snapshot pipeline, never an embedded remote player.
                tiktok={'id':'7668090902816017671','desc':'TikTok browser fixture',
                        'author':{'uniqueId':'fixture'},'video':{'duration':15},'stats':{'playCount':3}}
                tt_html='<html><title>TikTok fixture</title><script id="__UNIVERSAL_DATA_FOR_REHYDRATION__" type="application/json">'+json.dumps({'__DEFAULT_SCOPE__':{'webapp.feed':{'itemList':[tiktok]}}})+'</script></html>'
                async def fulfill_tiktok(route):await route.fulfill(status=200,content_type='text/html',body=tt_html)
                await browser.context.route('https://www.tiktok.com/**',fulfill_tiktok)
                await browser.action({'action':'new','url':'https://www.tiktok.com/foryou'})
                await asyncio.wait_for(browser.queue.join(),10)
                assert browser.captured>=2, await browser.state()
                await browser.sync_cookies()
                assert (Path(d)/'youtube-cookies.json').stat().st_mode & 0o077 == 0
                assert (Path(d)/'tiktok-cookies.json').stat().st_mode & 0o077 == 0
                async with http.get(base+'/api/feeds',headers=headers) as r:
                    assert r.status==200
                    feeds=await r.json()
                    assert any(f['kind']=='tiktok-for-you' for f in feeds),feeds
                await mobile_browse_smoke(browser,http,base,origin,headers)
                await installed_worker_smoke(browser,http,base,headers)
                await browser.action({'action':'reset'})
                assert not (Path(d)/'profile').exists()
            finally:await browser.close()
        async with http.delete(base+'/api/session',headers=headers) as r:assert r.status==200
        async with http.get(base+'/browser/state',headers=headers) as r:assert r.status==401
    print('Image smoke passed: pairing, browser authorization, sandboxed Chromium, native feed capture, frame, tabs, private storage, reset and session revocation.')

asyncio.run(main())
