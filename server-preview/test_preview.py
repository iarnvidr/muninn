"""Offline policy/security tests. Live provider acceptance is a separate check."""
import asyncio
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock
from aiohttp import web
from browser import Browser
from gateway import validate_request, authenticated
from policy import *


class PolicyTests(unittest.TestCase):
    def test_navigation_and_resource_boundary(self):
        for url in ['https://www.youtube.com/', 'https://accounts.google.com/ServiceLogin', 'https://youtu.be/aaaaaaaaaaa']:
            self.assertEqual(navigation_url(url), url)
        for url in ['http://youtube.com/', 'file:///etc/passwd','https://youtube.com.evil.test/',
                    'https://127.0.0.1/', 'https://youtube.com:8443/', 'https://u:p@youtube.com/']:
            with self.assertRaises(ValueError): navigation_url(url)
            self.assertFalse(request_allowed(url, 'document', True))
        self.assertTrue(request_allowed('https://i.ytimg.com/test.jpg','image',False))
        self.assertFalse(request_allowed('https://r1.googlevideo.com/videoplayback','fetch',False))
        self.assertFalse(request_allowed('https://www.youtube.com/video','media',False))
        self.assertFalse(request_allowed('https://i.ytimg.com/test','document',True))

    def test_only_native_parser_inputs_are_captured(self):
        self.assertEqual(capture_kind('https://www.youtube.com/youtubei/v1/browse?key=secret','application/json'),'application/json')
        self.assertEqual(capture_kind('https://www.youtube.com/watch?v=aaaaaaaaaaa','text/html'),'text/html')
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
