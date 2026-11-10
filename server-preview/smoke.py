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
                await browser.sync_cookies()
                assert (Path(d)/'youtube-cookies.json').stat().st_mode & 0o077 == 0
                async with http.get(base+'/api/feeds',headers=headers) as r:
                    assert r.status==200;assert len(await r.json())>=1
                await browser.action({'action':'reset'})
                assert not (Path(d)/'profile').exists()
            finally:await browser.close()
        async with http.delete(base+'/api/session',headers=headers) as r:assert r.status==200
        async with http.get(base+'/browser/state',headers=headers) as r:assert r.status==401
    print('Image smoke passed: pairing, browser authorization, sandboxed Chromium, native feed capture, frame, tabs, private storage, reset and session revocation.')

asyncio.run(main())
