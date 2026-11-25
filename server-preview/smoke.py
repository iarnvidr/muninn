"""Runs inside the exact Dagger image. Real Chromium + core; synthetic YouTube.

No personal Google account, live video or provider availability is required.
"""
import asyncio
import json
import os
import tempfile
from pathlib import Path
from urllib.parse import urlsplit
from aiohttp import ClientSession
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
                await browser.action({'action':'reset'})
                assert not (Path(d)/'profile').exists()
            finally:await browser.close()
        async with http.delete(base+'/api/session',headers=headers) as r:assert r.status==200
        async with http.get(base+'/browser/state',headers=headers) as r:assert r.status==401
    print('Image smoke passed: pairing, browser authorization, sandboxed Chromium, native feed capture, frame, tabs, private storage, reset and session revocation.')

asyncio.run(main())
