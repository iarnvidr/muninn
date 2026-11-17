import json
import os
import urllib.request
from urllib.parse import urlsplit
request = urllib.request.Request('http://127.0.0.1:8080/health', headers={'Host': urlsplit(os.environ['MUNINN_WEB_ORIGIN']).netloc})
with urllib.request.urlopen(request, timeout=4) as response:
    if response.status != 200 or json.load(response).get('ok') is not True:
        raise SystemExit(1)
