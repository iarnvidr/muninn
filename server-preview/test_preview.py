"""Offline policy/security tests. Live provider acceptance is a separate check."""
import asyncio
import json
import os
import ipaddress
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock
from aiohttp import web
from multidict import CIMultiDict
from browser import Browser
from gateway import validate_request, authenticated, trusted_owner, internal_identity_headers, upstream_headers
from policy import *


class PolicyTests(unittest.TestCase):
    def test_preview_mounts_browser_as_main_pwa_view(self):
        # The final OCI image contains /opt/muninn-preview only. The PWA
        # HTML is embedded into the Rust binary, and JS source tests cover it.
        # This image test must validate the actual packaged browser assets.
        preview = Path(__file__).resolve().parent
        browser = (preview / "client.js").read_text()
        gateway = (preview / "gateway.py").read_text()
        self.assertIn("querySelector('#browser-host')", browser)
        self.assertNotIn("querySelector('nav.tabs')", browser)
        self.assertIn('name="muninn-source-browser" content="enabled"', gateway)
        self.assertIn('muninn-pwa-shell-browser-v3', gateway)
        self.assertIn('https://www.tiktok.com/search?q=', browser)
        self.assertIn('https://www.youtube.com/results?search_query=', browser)

    def test_navigation_and_resource_boundary(self):
        for url in ['https://www.youtube.com/', 'https://accounts.google.com/ServiceLogin', 'https://youtu.be/aaaaaaaaaaa',
                    'https://www.tiktok.com/foryou', 'https://vm.tiktok.com/ZMexample/']:
            self.assertEqual(navigation_url(url), url)
        for url in ['http://youtube.com/', 'file:///etc/passwd','https://youtube.com.evil.test/',
                    'https://127.0.0.1/', 'https://youtube.com:8443/', 'https://u:p@youtube.com/']:
            with self.assertRaises(ValueError): navigation_url(url)
            self.assertFalse(request_allowed(url, 'document', True))
        self.assertTrue(request_allowed('https://i.ytimg.com/test.jpg','image',False))
        self.assertFalse(request_allowed('https://r1.googlevideo.com/videoplayback','fetch',False))
        self.assertFalse(request_allowed('https://www.youtube.com/video','media',False))
        self.assertFalse(request_allowed('https://i.ytimg.com/test','document',True))
        self.assertTrue(request_allowed('https://p16.tiktokcdn.com/cover.jpg','image',False))
        self.assertFalse(request_allowed('https://v16.tiktokv.com/tos/video','fetch',False))
        self.assertFalse(request_allowed('https://v16-webapp-prime.tiktok.com/bytes','fetch',False))
        self.assertFalse(request_allowed('https://www.tiktok.com/video/clip.mp4','fetch',False))
        with self.assertRaises(ValueError): navigation_url('https://tiktok.com.evil.test/@maker')

    def test_only_native_parser_inputs_are_captured(self):
        self.assertEqual(capture_kind('https://www.youtube.com/youtubei/v1/browse?key=secret','application/json'),'application/json')
        self.assertEqual(capture_kind('https://www.youtube.com/watch?v=aaaaaaaaaaa','text/html'),'text/html')
        self.assertEqual(capture_kind('https://www.tiktok.com/api/recommend/item_list/?msToken=private','application/json'),'application/json')
        self.assertEqual(capture_kind('https://www.tiktok.com/@creator/video/7668090902816017671','text/html'),'text/html')
        self.assertIsNone(capture_kind('https://www.tiktok.com/api/passport/account/login','application/json'))
        for url in ['https://accounts.google.com/', 'https://www.youtube.com/youtubei/v1/account', 'https://evil.test/']:
            self.assertIsNone(capture_kind(url,'application/json'))
        body=json.loads(safe_request_body('{"videoId":"aaaaaaaaaaa","context":{"user":"private"},"continuation":"next"}'))
        self.assertEqual(body, {'videoId':'aaaaaaaaaaa','continuation':'next'})
        self.assertIsNone(safe_request_body('not-json'))

    def test_public_urls_do_not_leak_login_queries(self):
        self.assertEqual(public_url('https://accounts.google.com/signin?email=private&token=secret'), 'https://accounts.google.com/signin')
        self.assertEqual(public_url('https://www.youtube.com/watch?v=aaaaaaaaaaa&token=secret'), 'https://www.youtube.com/watch?v=aaaaaaaaaaa')
        self.assertEqual(video_id('https://youtu.be/aaaaaaaaaaa'), 'aaaaaaaaaaa')
        self.assertIsNone(video_id('https://accounts.google.com/watch?v=aaaaaaaaaaa'))
        self.assertEqual(video_id('https://www.tiktok.com/@maker/video/7668090902816017671'), 'tt_7668090902816017671')
        self.assertIsNone(video_id('https://www.tiktok.com.evil.test/@maker/video/7668090902816017671'))
        self.assertEqual(public_url('https://www.tiktok.com/@maker/video/7668090902816017671?msToken=private'),
                         'https://www.tiktok.com/@maker/video/7668090902816017671')
        self.assertFalse(key_allowed('Control+Shift+J'))

    def test_exact_origin_and_csrf(self):
        def req(method='POST',host='preview.test',origin='https://preview.test',header='pwa'):
            h={}
            if origin is not None:h['Origin']=origin
            if header is not None:h['X-Muninn-Client']=header
            return SimpleNamespace(method=method,host=host,headers=h)
        validate_request(req(),'https://preview.test','preview.test')
        validate_request(req('GET',origin=None,header=None),'https://preview.test','preview.test')
        for r in [req(host='evil.test'),req(origin='https://evil.test'),req(origin=None),req(header=None)]:
            with self.assertRaises(web.HTTPForbidden):validate_request(r,'https://preview.test','preview.test')


class GatewayAutheliaTests(unittest.TestCase):
    def request(self, peer="172.29.0.2", user="esben", proto="https"):
        class Transport:
            def get_extra_info(self, name):
                return (peer, 55000) if name == "peername" else None
        h = CIMultiDict({"Host": "muninn-preview.example.test"})
        if user is not None: h.add("Remote-User", user)
        if proto is not None: h.add("X-Forwarded-Proto", proto)
        return SimpleNamespace(
            transport=Transport(),
            headers=h,
            app={"auth_mode": "authelia",
                 "authority": "muninn-preview.example.test",
                 "proxy_ips": {ipaddress.ip_address("172.29.0.2")},
                 "gateway_secret": "a" * 64},
        )

    def test_gateway_auth_is_not_a_spoofable_forwarded_header(self):
        valid = self.request()
        self.assertTrue(trusted_owner(valid))
        self.assertEqual(internal_identity_headers(valid)["X-Muninn-Gateway-Auth"], "a" * 64)
        for req in [self.request(peer="172.29.0.3"), self.request(peer="127.0.0.1"),
                    self.request(user=None), self.request(user="alice"),
                    self.request(proto="http"), self.request(proto=None)]:
            self.assertFalse(trusted_owner(req))
            with self.assertRaises(web.HTTPForbidden):
                internal_identity_headers(req)
        duplicate = self.request()
        duplicate.headers.add("Remote-User", "esben")
        self.assertFalse(trusted_owner(duplicate))
        duplicate = self.request()
        duplicate.headers.add("X-Forwarded-Proto", "https")
        self.assertFalse(trusted_owner(duplicate))
        forged = self.request()
        forged.headers["X-Muninn-Gateway-Auth"] = "a" * 64
        self.assertFalse(trusted_owner(forged))

    def test_authelia_healthcheck_remains_anonymous_without_opening_any_other_route(self):
        # Production Docker healthcheck originates in the container and has no
        # forward-auth identity. Rust exposes only this exact path anonymously.
        health = self.request(peer="127.0.0.1", user=None, proto=None)
        health.path = "/health"
        self.assertEqual(upstream_headers(health), {"Host": "muninn-preview.example.test"})
        for path in ("/health/", "/api/session", "/api/entries", "/app/", "/media/nonexistent"):
            health.path = path
            with self.assertRaises(web.HTTPForbidden):
                upstream_headers(health)
        owner = self.request()
        owner.path = "/api/session"
        headers = upstream_headers(owner)
        self.assertEqual(headers["Remote-User"], "esben")
        self.assertEqual(headers["X-Muninn-Gateway-Auth"], "a" * 64)
        self.assertNotIn("Cookie", headers)



class BrowserTests(unittest.IsolatedAsyncioTestCase):
    async def test_cookies_are_private_atomic_and_scoped(self):
        with tempfile.TemporaryDirectory() as d:
            browser=Browser(Path(d),None,'http://127.0.0.1:47873','test')
            browser.context=SimpleNamespace(cookies=AsyncMock(return_value=[
                {'domain':'.youtube.com','path':'/','name':'SID','value':'fixture-private','secure':True,'httpOnly':True,'expires':-1},
                {'domain':'accounts.google.com','path':'/','name':'SID','value':'other-private','secure':True,'httpOnly':True,'expires':-1}]))
            await asyncio.gather(browser.sync_cookies(),browser.sync_cookies())
            path=Path(d)/'youtube-cookies.json'
            self.assertEqual(path.stat().st_mode & 0o777,0o600)
            rows=json.loads(path.read_text());self.assertEqual(len(rows),1)
            self.assertEqual(rows[0]['domain'],'.youtube.com')
            tt=Path(d)/'tiktok-cookies.json'
            self.assertEqual(tt.stat().st_mode & 0o777,0o600)
            self.assertEqual(json.loads(tt.read_text()),[])
            self.assertNotIn('fixture-private',json.dumps(await browser.state()))

    async def test_capture_queue_is_bounded_and_tabs_use_native_open_encounter(self):
        with tempfile.TemporaryDirectory() as d:
            browser=Browser(Path(d),None,'http://127.0.0.1:47873','test')
            for _ in range(30):browser._queue_tab('id',SimpleNamespace(url='https://www.youtube.com/watch?v=aaaaaaaaaaa'))
            self.assertEqual(browser.queue.qsize(),16)
            self.assertEqual(browser.dropped,14)
            kind,payload,epoch=browser.queue.get_nowait()
            self.assertEqual((kind,payload['state'],payload['video_id']),('tab','open','aaaaaaaaaaa'))

    async def test_private_cookies_are_refreshed_before_tab_downloads(self):
        with tempfile.TemporaryDirectory() as d:
            browser=Browser(Path(d),None,'http://127.0.0.1:47873','test')
            order=[]
            async def sync():order.append('cookies')
            async def post(*args):order.append('tab')
            browser.sync_cookies=sync;browser._post=post
            task=asyncio.create_task(browser._consume())
            browser._queue_tab('id',SimpleNamespace(url='https://www.youtube.com/watch?v=aaaaaaaaaaa'))
            await browser.queue.join()
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):await task
            self.assertEqual(order,['cookies','tab'])

    async def test_browser_access_requires_real_muninn_session(self):
        class Response:
            status=401
            async def __aenter__(self):return self
            async def __aexit__(self,*args):pass
        calls=[]
        def get(url,**kw):calls.append((url,kw));return Response()
        request=SimpleNamespace(headers={'Cookie':'untrusted=1'},app={'authority':'preview.test','http':SimpleNamespace(get=get)})
        with self.assertRaises(web.HTTPUnauthorized):await authenticated(request)
        self.assertEqual(calls[0][0],'http://127.0.0.1:47872/api/session')
        self.assertFalse(calls[0][1]['allow_redirects'])

if __name__=='__main__':unittest.main()
