"""Offline fixture for the production watch UI and real hls.js media engine.

Serve an existing 60-second fMP4 EVENT playlist directory; see VERIFY_RECORDED_START.md.
Only the native tab bridge and metadata/status APIs are synthetic.
"""
import argparse
import json
import mimetypes
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

parser = argparse.ArgumentParser()
parser.add_argument('--media-dir', type=Path, required=True)
args = parser.parse_args()
ui = Path(__file__).resolve().parents[1] / 'ui'
state = dict(position=0, finished=False)

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def send(self, value, mime='application/json', status=200):
        body = value if isinstance(value, bytes) else json.dumps(value).encode()
        self.send_response(status)
        for key, value in [('Content-Type', mime), ('Content-Length', str(len(body))), ('Cache-Control', 'no-store')]:
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))) or '{}')
        if urlsplit(self.path).path == '/fixture/resume': state.update(body)
        self.send({})

    def do_GET(self):
        path = urlsplit(self.path).path
        if path.startswith('/h/'):
            file = args.media_dir / path.split('/')[-1]
            if not file.is_file(): return self.send({}, status=404)
            data = file.read_bytes()
            if file.suffix == '.m3u8' and state['finished']: data += b'#EXT-X-ENDLIST\n'
            return self.send(data, 'application/vnd.apple.mpegurl' if file.suffix == '.m3u8' else 'video/mp4')
        if path.startswith('/api/video/'):
            return self.send(dict(meta=dict(title='Recorded startup fixture', duration_seconds=60), position_seconds=state['position'], entries=[], comments=[], related=[]))
        if path == '/api/settings': return self.send(dict(theme='system', pip_mode='manual'))
        if path == '/api/setup': return self.send(dict(setup=dict(phase='ready'), tools={}))
        if path == '/api/organization': return self.send(dict(collections={}, tags={}, tab_groups={}, tab_order=[]))
        if path == '/api/featured': return self.send(dict(items=[]))
        if path.startswith('/api/'): return self.send({})
        file = ui / (path.removeprefix('/ui/') or 'index.html')
        if path.endswith('.ttf'): file = ui / 'fonts' / path.split('/')[-1]
        if file.is_file() and file.resolve().is_relative_to(ui):
            return self.send(file.read_bytes(), mimetypes.guess_type(file)[0] or 'application/octet-stream')
        self.send({}, status=404)

ThreadingHTTPServer(('127.0.0.1', 47942), Handler).serve_forever()
