"""Exercise outside-open handling in the production Tauri app, in an isolated HOME.

Start run-app.sh with a cold app link first; see docs/APP_LINKS.md.
Linux verifies argv, single-instance delivery and chrome focus, not Launch Services.
"""
import json
import os
from pathlib import Path
import subprocess
import time
from urllib.parse import urlencode

import e2e as E

COLD = "https://www.youtube.com/results?search_query=muninn+handoff+cold"
WARM = "https://www.youtube.com/results?search_query=muninn+handoff+running"


def link(target):
    return "muninn://open?" + urlencode({"url": target})


def has_active_url(target):
    state = E.state()
    return state if any(t["id"] == state["active"] and t["url"] == target for t in state["tabs"]) else None


def secondary_instance(target, direct=False):
    # Reuse only the test app's private bus address, never dump its environment.
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if (proc / "comm").read_text().strip() != "muninn":
                continue
            address = next(item.split(b"=", 1)[1].decode() for item in
                           (proc / "environ").read_bytes().split(b"\0")
                           if item.startswith(b"DBUS_SESSION_BUS_ADDRESS="))
            binary = os.readlink(proc / "exe")
            env = dict(os.environ, HOME="/tmp/home", DISPLAY=":99", DBUS_SESSION_BUS_ADDRESS=address)
            subprocess.run([binary, target if direct else link(target)], env=env, check=True, timeout=15,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return
        except (FileNotFoundError, PermissionError, StopIteration):
            continue
    raise RuntimeError("isolated primary app was not found")


def main():
    cold = E.wait(lambda: has_active_url(COLD), what="cold link tab")
    E.wait(lambda: E.evaljs("chrome", "return Boolean(window.Muninn?.boot)"), what="chrome startup")
    E.evaljs("chrome", "document.querySelector('[data-page=library]').click(); return true")
    E.wait(lambda: E.evaljs("chrome", "return window.Muninn.page==='library'"), what="library page")
    count = len(E.state()["tabs"])
    E.call("app_open_link", url=link("https://example.invalid/"))
    time.sleep(1)
    assert len(E.state()["tabs"]) == count, "rejected link created a tab"
    assert E.evaljs("chrome", "return window.Muninn.page==='library'"), "rejected link changed pages"

    # The normal secondary process must open the link and leave Library behind.
    secondary_instance(WARM)
    warm = E.wait(lambda: has_active_url(WARM), what="single-instance delivery")
    E.wait(lambda: E.evaljs("chrome", "return window.Muninn.page===null"), what="handoff leaves library")
    assert len(warm["tabs"]) == count + 1, "handoff duplicated or lost the tab"

    # Subsequent state polls must not repeatedly pull the user out of Library.
    E.evaljs("chrome", "document.querySelector('[data-page=library]').click(); return true")
    time.sleep(1.2)
    assert E.evaljs("chrome", "return window.Muninn.page==='library'"), "handoff focus request repeated"

    E.call("window_hide")
    E.wait(lambda: not E.state()["window_visible"], what="hidden app window")
    hidden_target = "https://www.youtube.com/results?search_query=muninn+handoff+hidden"
    secondary_instance(hidden_target, direct=True)
    E.wait(lambda: has_active_url(hidden_target), what="hidden-app delivery")
    E.wait(lambda: E.state()["window_visible"], what="window restored")
    E.wait(lambda: E.evaljs("chrome", "return window.Muninn.page===null"), what="hidden-app focus")
    print(json.dumps({"passed": True, "cold_start": True, "single_instance": True,
                      "hidden_window": True, "leaves_internal_page": True,
                      "rejected_link_no_tab": True, "focus_request_consumed_once": True,
                      "platform": "Linux Tauri/WebKitGTK; not macOS Launch Services"}, indent=2))


if __name__ == "__main__":
    main()
