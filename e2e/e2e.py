"""Driver for the Linux/xvfb end-to-end checks of the real Muninn app.

Runs inside the test container next to the app (see run-e2e.sh). The app is
started with MUNINN_E2E_DIR set; this module drops command files there, reads
the results, calls the app's local HTTP API with the launch's token, and takes
screenshots of the X display.
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

E2E_DIR = os.environ.get("MUNINN_E2E_DIR", "/tmp/e2e")
DATA_DIR = os.environ.get("MUNINN_DATA", "/tmp/home/.local/share/io.github.iarnvidr.muninn")
SHOTS = os.environ.get("MUNINN_SHOTS", "/tmp/shots")
_seq = [0]


def _id():
    _seq[0] += 1
    return "c%05d" % _seq[0]


def send(cmd, timeout=30):
    cid = _id()
    cmd["id"] = cid
    tmp = os.path.join(E2E_DIR, cid + ".tmp")
    with open(tmp, "w") as f:
        json.dump(cmd, f)
    os.rename(tmp, os.path.join(E2E_DIR, cid + ".cmd.json"))
    res = os.path.join(E2E_DIR, cid + ".res.json")
    t = time.time()
    while time.time() - t < timeout:
        if os.path.exists(res):
            with open(res) as f:
                text = f.read()
            os.remove(res)
            try:
                return json.loads(text)
            except ValueError:
                return text
        time.sleep(0.1)
    raise TimeoutError("no result for %r" % cmd)


def call(name, **args):
    return send({"op": "call", "name": name, "args": args})


def state():
    return send({"op": "state"})


def evaljs(target, js, timeout=30):
    return send({"op": "eval", "target": target, "js": js}, timeout)


def evalsync(target, js, timeout=30):
    """Synchronous eval; works on pages without IPC (accounts.google.com)."""
    return send({"op": "evalsync", "target": target, "js": js}, timeout)


def service():
    with open(os.path.join(DATA_DIR, "service.json")) as f:
        port = json.load(f)["port"]
    with open(os.path.join(DATA_DIR, "token")) as f:
        token = f.read().strip()
    return "http://127.0.0.1:%d" % port, token


def api(path, method="GET", body=None, timeout=60):
    base, token = service()
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", "Bearer " + token)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        return {"http_error": e.code, "body": e.read().decode()[:300]}


def shot(name):
    os.makedirs(SHOTS, exist_ok=True)
    path = os.path.join(SHOTS, name + ".png")
    subprocess.run(["import", "-window", "root", path], check=True, env=dict(os.environ, DISPLAY=os.environ.get("DISPLAY", ":99")))
    return path


def wait(fn, timeout=30, every=0.5, what="condition"):
    t = time.time()
    last = None
    while time.time() - t < timeout:
        last = fn()
        if last:
            return last
        time.sleep(every)
    raise TimeoutError("timed out waiting for " + what + " (last=%r)" % (last,))


def sql(db, query):
    out = subprocess.run(["sqlite3", "-json", os.path.join(DATA_DIR, "db", db), query], capture_output=True, text=True)
    return json.loads(out.stdout) if out.stdout.strip() else []


if __name__ == "__main__":
    print(json.dumps(globals()[sys.argv[1]](*sys.argv[2:]), indent=1))


def rss_mb():
    """Resident memory (MB) of the web content/network processes and the app itself."""
    out = {"web": 0.0, "network": 0.0, "app": 0.0, "web_procs": 0}
    for pid in filter(str.isdigit, os.listdir("/proc")):
        try:
            with open("/proc/%s/cmdline" % pid, "rb") as f:
                cmd = f.read().split(b"\0")[0].decode()
            with open("/proc/%s/statm" % pid) as f:
                pages = int(f.read().split()[1])
        except (OSError, ValueError, IndexError):
            continue
        mb = pages * os.sysconf("SC_PAGE_SIZE") / 1048576
        if "WebKitWebProcess" in cmd:
            out["web"] += mb
            out["web_procs"] += 1
        elif "WebKitNetworkProcess" in cmd:
            out["network"] += mb
        elif cmd.endswith("/muninn"):
            out["app"] += mb
    return {k: round(v, 1) if isinstance(v, float) else v for k, v in out.items()}
