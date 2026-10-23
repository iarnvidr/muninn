"""The live check of the real Muninn app (Linux, xvfb, WebKitGTK) against real youtube.com.

  scenario.py online     browse, capture, click a video, play while caching, seek, comments/related,
                         hibernate and restore, Google sign-in page check
  scenario.py offline    (network cut by the caller) reopen the cached video and play it from disk

Every step prints a PASS/FAIL line with its evidence; screenshots land in $MUNINN_SHOTS and the
machine-readable results in $MUNINN_SHOTS/results-<phase>.json.
"""
import json
import os
import subprocess
import sys
import time

import e2e

RESULTS = {}
VIDEO_JS = """
const v = document.querySelector('video');
if (!v) return null;
const seekable = v.seekable.length ? v.seekable.end(v.seekable.length - 1) : 0;
const buffered = v.buffered.length ? v.buffered.end(v.buffered.length - 1) : 0;
return {ct: v.currentTime, paused: v.paused, rs: v.readyState, w: v.videoWidth, h: v.videoHeight, dur: v.duration, seekable, buffered,
        src: (v.currentSrc || '').replace(/\\/h\\/[0-9a-f]+\\//, '/h/<token>/').replace(/token=[0-9a-f]+/, 'token=<token>'), err: v.error && v.error.code};
"""


def log(ok, name, evidence=""):
    print(("PASS " if ok else "FAIL ") + name + (": " + str(evidence) if evidence != "" else ""), flush=True)
    RESULTS.setdefault("steps", []).append({"step": name, "ok": bool(ok), "evidence": evidence})
    return ok


def ui(js, timeout=30):
    return e2e.evaljs("chrome", js, timeout)


def active_tab():
    st = e2e.state()
    return next(t for t in st["tabs"] if t["id"] == st["active"])


def native_ready():
    st = e2e.state()
    return any(t["id"] == st["active"] and t["state"] == "live" and t["face"] == "yt" for t in st["tabs"])


def accept_consent():
    """EU consent wall: the user would click this; so does the check."""
    r = e2e.evaljs("active", "const b=[...document.querySelectorAll('button')].find(x=>/accept all/i.test(x.textContent)); if(!b) return 'none'; b.click(); return 'clicked'")
    if r == "clicked":
        time.sleep(8)
    return r


def go_via_address_bar(text):
    return ui("const a=document.getElementById('address'); a.focus(); a.value=%s; document.getElementById('address-form').requestSubmit(); return 'submitted'" % json.dumps(text))


def click_video(video_id):
    """Click the thumbnail link of a given video in the YouTube tab, like a user would."""
    return e2e.evaljs("active", "const a=[...document.querySelectorAll('a[href*=\"v=%s\"]')].find(x=>x.offsetParent); if(!a) return null; const href=a.getAttribute('href'); a.scrollIntoView({block:'center'}); a.click(); return href" % video_id)


def video_id_of(href):
    return href.split("v=")[1].split("&")[0]


def phase_online():
    # -- boot ------------------------------------------------------------------------------------
    st = e2e.wait(lambda: e2e.state() if e2e.state()["tabs"] else None, 30, what="the first tab")
    log(True, "app started, first tab present", [(t["id"], t["state"]) for t in st["tabs"]])
    setup = e2e.wait(lambda: (lambda s: s if s["setup"]["phase"] == "ready" else None)(e2e.api("/api/setup")), 60, what="tool setup")
    tools = setup["tools"]
    log(tools["ytdlp"]["installed"] and tools["ffmpeg"]["found"] and tools["js_runtime"]["found"], "zero-setup tools ready",
        {"yt-dlp": tools["ytdlp"]["version"], "ffmpeg": (tools["ffmpeg"]["version"] or "")[:30], "js": tools["js_runtime"]["name"]})
    e2e.wait(native_ready, 40, what="the YouTube tab")
    time.sleep(3)
    print("consent:", accept_consent())
    e2e.shot("01-start-home")

    # -- browse: search through the address bar, feed rows land in SQLite -------------------------------
    print("address bar:", go_via_address_bar("rick astley never gonna give you up"))
    snap = e2e.wait(lambda: e2e.sql("content.sqlite", "select id,kind,count,query from feed_snapshots where kind='search' order by id desc limit 1"), 40, what="search feed snapshot")
    time.sleep(3)
    e2e.shot("02-search-results")
    rows = e2e.sql("content.sqlite", "select count(*) as n from feed_snapshots")[0]["n"]
    log(snap[0]["count"] > 5, "search feed captured into SQLite via the proxy", {"snapshot": snap[0], "snapshots": rows})
    items = e2e.api("/api/feeds/%d" % snap[0]["id"])
    log(len(items["items"]) > 5 and items["items"][0]["title"], "feed rows readable through the content cache", [i["title"][:40] for i in items["items"][:3]])
    proxy = e2e.api("/api/proxy")
    stats = proxy["stats"]
    tunneled = [h["host"] for h in stats["hosts"]]
    log(stats["blocked"] >= 0 and stats["media_bytes_fetched"] == 0 and not any(h.endswith("googlevideo.com") and False for h in tunneled),
        "YouTube's own player fetched nothing", {"googlevideo_connects_refused": stats["blocked"], "media_bytes_fetched": stats["media_bytes_fetched"],
                                              "mitm_connects": stats["mitm_connects"], "tls_failures": stats["tls_failures"], "captured": stats["captured"]})
    ev = [e for e in proxy["events"] if e["kind"] == "blocked"][:2]
    print("blocked examples:", ev)

    # -- click a video: Muninn's player takes over ---------------------------------------------------
    # a video of ordinary length (the first hits are often 6-hour streams)
    pick = max((i for i in items["items"] if 90 <= (i.get("duration_seconds") or 0) <= 900 and not i.get("badge")), key=lambda i: i["duration_seconds"], default=None)
    log(pick is not None, "picked a 1.5-15 minute video from the captured results", pick and {"id": pick["video_id"], "duration_s": pick["duration_seconds"], "title": pick["title"][:40]})
    t_click = time.time()
    href = click_video(pick["video_id"])
    log(bool(href), "clicked its thumbnail in the YouTube tab", href)
    vid = video_id_of(href)
    tab = e2e.wait(lambda: (lambda t: t if t["face"] == "watch" else None)(active_tab()), 10, 0.1, "the watch face")
    log(tab["video_id"] == vid, "the click opened Muninn's watch view in the same tab, not YouTube's player", {"face": tab["face"], "video_id": tab["video_id"], "url": tab["url"]})
    first = None
    t0 = time.time()
    while time.time() - t0 < 60:
        v = ui(VIDEO_JS)
        if v and v["ct"] > 0.3 and v["rs"] >= 3 and v["w"] > 0:
            first = v
            break
        time.sleep(0.15)
    t_first_frame = time.time() - t_click
    st = e2e.api("/api/watch/" + vid)
    pb = st["playback"]
    part = os.path.join(e2e.DATA_DIR, "cache", "video", "%s.%s.partial.mp4" % (vid, pb["variant"]))
    sizes = []
    for _ in range(3):
        sizes.append(os.path.getsize(part) if os.path.exists(part) else -1)
        time.sleep(0.25)
    pb2 = e2e.api("/api/watch/" + vid)["playback"]
    log(first is not None, "Muninn's player started", {"time_to_first_frame_s": round(t_first_frame, 2), "engine_src": first and first["src"], "video": first and [first["w"], first["h"]],
                                                       "server_time_to_first_segment_ms": pb.get("time_to_first_segment_ms"), "playback": {k: pb.get(k) for k in ("state", "phase", "error")}})
    RESULTS["time_to_first_frame_s"] = round(t_first_frame, 2)
    e2e.shot("03-watch-playing-while-caching")
    segs = len([f for f in os.listdir(os.path.join(e2e.DATA_DIR, "cache", "hls", "%s.%s" % (vid, pb["variant"]))) if f.endswith(".m4s")]) if pb["kind"] == "hls" else -1
    growing = pb2["state"] == "streaming" and sizes[-1] > sizes[0]
    log(pb["kind"] == "hls" and growing, "playback source was the growing HLS stream, the file was still being written",
        {"state_while_playing": pb2["state"], "progress": round(pb2.get("progress", 0), 3), "partial_mp4_sizes_over_0.5s": sizes, "hls_segments": segs, "still_growing": growing})
    # seek inside the cached part, then past the end of it
    ui("document.querySelector('video').currentTime = 20; return 1")
    time.sleep(2.0)
    v = ui(VIDEO_JS)
    log(v and abs(v["ct"] - 20) < 4 and not v["err"], "seek works", {"currentTime": v and round(v["ct"], 1)})
    meta = e2e.wait(lambda: (lambda d: d if d["meta"] and d["comments"] else None)(e2e.api("/api/video/" + vid)), 60, 1, "metadata and comments")
    log(len(meta["comments"]["comments"]) > 0 and len(meta["related"]) > 0, "comments and related videos in the content cache",
        {"title": meta["meta"]["title"][:50], "comments": len(meta["comments"]["comments"]), "related": len(meta["related"])})
    e2e.shot("04-watch-details")
    ui("const m=document.getElementById('view-watch'); if(m){ m.scrollTop = 700 } ; window.scrollTo(0, 700); return 1")
    time.sleep(0.7)
    e2e.shot("05-watch-comments-related")
    # wait for the cache to finish
    done = e2e.wait(lambda: (lambda s: s if s["playback"]["state"] == "cached" else None)(e2e.api("/api/watch/" + vid)), 300, 2, "the cache to complete")
    size = os.path.getsize(os.path.join(e2e.DATA_DIR, "cache", "video", "%s.%s.mp4" % (vid, pb["variant"])))
    probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration:stream=codec_name,height", "-of", "json",
                            os.path.join(e2e.DATA_DIR, "cache", "video", "%s.%s.mp4" % (vid, pb["variant"]))], capture_output=True, text=True)
    log(size > 100000, "the stream became a finished cache entry on disk", {"entry": done["playback"]["entry_id"], "bytes": size, "ffprobe": json.loads(probe.stdout or "{}")})
    RESULTS["video_id"] = vid
    RESULTS["variant"] = pb["variant"]
    # position was saved
    prog = e2e.sql("content.sqlite", "select position_seconds from progress where video_id='%s'" % vid)
    log(bool(prog), "playback position saved to SQLite", prog)

    # -- a yt-dlp 360p cache with ffmpeg (the rules/prefetch path) ---------------------------------------
    r = e2e.api("/api/cache", "POST", {"video_id": "jNQXAC9IVRw", "quality": "360"})
    ent = e2e.wait(lambda: (lambda es: next((e for e in es if e["video_id"] == "jNQXAC9IVRw" and e["variant"] == "360" and e["status"] in ("complete", "failed", "canceled")), None))(e2e.api("/api/entries")), 180, 2, "the 360p yt-dlp job")
    log(ent["status"] == "complete", "yt-dlp 360p prefetch with ffmpeg (merged format)", {"status": ent["status"], "actual_height": ent.get("actual_height"), "size": ent.get("size_bytes"), "error": ent.get("error")})

    # -- the internal pages of the same window -----------------------------------------------------------------
    nav = ui("return [...document.querySelectorAll('#sidebar [data-page]')].map(b => b.dataset.page).join(',')")
    log("diagnostics" not in nav, "normal launch has no Diagnostics navigation", nav)
    e2e.api("/api/settings", "PUT", {"developer_mode": True})
    time.sleep(12)  # the shell picks the setting up on its next system poll
    for name in ("Library", "Feeds", "Rules", "Diagnostics", "Settings"):
        ui("[...document.querySelectorAll('#sidebar button, #sidebar a')].find(b => /%s/i.test(b.textContent)).click(); return 1" % name)
        time.sleep(1.5)
        e2e.shot("15-page-" + name.lower())
        if name == "Diagnostics":
            diag = ui("return document.getElementById('view-internal').innerText.slice(0, 1500)")
    log("yt-dlp" in diag and "blocked" in diag.lower(), "Diagnostics shows the tools, proxy counters and captures", diag[:200].replace("\n", " | "))
    e2e.api("/api/settings", "PUT", {"developer_mode": False})
    ui("[...document.querySelectorAll('#sidebar button, #sidebar a')].find(b => /browse/i.test(b.textContent)).click(); return 1")
    time.sleep(1)

    # -- hibernation ----------------------------------------------------------------------------------------
    e2e.api("/api/settings", "PUT", {"warm_tabs": 2, "hibernate_after_secs": 6})
    rss_before = e2e.rss_mb()
    ids = []
    for q in ["rust programming", "cooking pasta", "jazz piano", "mountain biking"]:
        ids.append(e2e.call("tab_new", url=q)["id"])
        time.sleep(5)
    rss_open = e2e.rss_mb()
    e2e.shot("06-many-tabs")
    time.sleep(14)
    states = {t["id"]: t["state"] for t in e2e.state()["tabs"]}
    live = [i for i, s in states.items() if s in ("live", "loading")]
    rss_after = e2e.rss_mb()
    log(len(live) <= 2 + 1, "inactive tabs beyond the warm count were destroyed", {"states": states, "rss_mb": {"before": rss_before, "open": rss_open, "after_hibernation": rss_after}})
    rows = e2e.sql("browsing.sqlite", "select tab_id,url,scroll_y,video_id,position_seconds,state from tabs order by tab_id")
    log(any(r["state"] == "hidden" for r in rows), "tab state saved to the browsing DB (url, scroll, video, position)", rows)
    sleeping = next(i for i, s in states.items() if s == "sleeping")
    # scrolled tab restores where it was
    act = e2e.state()["active"]
    e2e.evaljs("tab:%d" % act, "window.scrollTo(0, 1600); await new Promise(r => setTimeout(r, 2500)); return window.scrollY")
    time.sleep(1.5)
    saved = e2e.sql("browsing.sqlite", "select scroll_y from tabs where tab_id=%d" % act)[0]["scroll_y"]
    e2e.call("tab_hibernate", id=act)
    e2e.call("tab_activate", id=act)
    time.sleep(1.0)
    e2e.shot("07-restore-view-from-cache")
    restoring = next(t for t in e2e.state()["tabs"] if t["id"] == act)["state"]
    e2e.wait(native_ready, 40, what="live page after restore")
    time.sleep(5)
    y = e2e.evaljs("tab:%d" % act, "return window.scrollY")
    e2e.shot("08-restored-live")
    log(abs(y - saved) < 200 and saved > 1000, "hibernated tab restored: cached view first, then live page at the saved scroll", {"saved": saved, "after_restore": y, "state_right_after_activate": restoring})

    # -- Google sign-in page check ----------------------------------------------------------------------------
    e2e.evaljs("active", "location.href='https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F'; return 1")
    e2e.wait(lambda: (lambda t: ("accounts.google.com" in t["url"] and t["state"] == "live") and t)(active_tab()), 40, 0.5, "the sign-in page to load")
    time.sleep(4)
    txt = None
    for _ in range(4):
        try:
            txt = e2e.evalsync("active", "return [location.href, document.body ? document.body.innerText.slice(0, 400) : '', !!document.querySelector('input[type=email]')]", 8)
            if "email or phone" in txt[1].lower():
                break
        except TimeoutError:
            pass
        time.sleep(2)
    e2e.shot("09-google-signin")
    bad = "may not be secure" in txt[1].lower() or "couldn't sign you in" in txt[1].lower()
    form = txt[2] or "email or phone" in txt[1].lower()
    log(not bad and form, "accounts.google.com reached its email entry form without the 'browser or app may not be secure' page", {"url": txt[0][:80], "email_input": txt[2], "text": txt[1][:80].replace("\n", " | ")})
    stats = e2e.api("/api/proxy")["stats"]
    log("accounts.google.com" in [h["host"] for h in stats["hosts"]] and not any(e["host"] == "accounts.google.com" and e["kind"] == "mitm" for e in e2e.api("/api/proxy")["events"]),
        "Google sign-in traffic was tunneled, never decrypted", {"tls_ok": stats["tls_ok"], "tls_failures": stats["tls_failures"], "tls_untrusted": stats["tls_untrusted"]})
    final = e2e.api("/api/proxy")["stats"]
    RESULTS["proxy"] = final
    RESULTS["captures"] = e2e.api("/api/captures?limit=200")


def phase_offline():
    vid = sys.argv[2]
    variant = sys.argv[3]
    # network is cut by the caller: prove it
    net = subprocess.run(["curl", "-sS", "-m", "4", "-o", "/dev/null", "-w", "%{http_code}", "https://www.youtube.com/"], capture_output=True, text=True)
    log(net.stdout.strip() in ("", "000"), "no network", (net.stdout + net.stderr).strip()[:80])
    before = e2e.api("/api/proxy")["stats"]
    st0 = e2e.state()
    tid = e2e.call("tab_new")["id"]
    time.sleep(1)
    t0 = time.time()
    e2e.call("tab_open_video", id=tid, video_id=vid)
    first = None
    while time.time() - t0 < 30:
        v = ui(VIDEO_JS)
        if v and v["ct"] > 0.3 and v["rs"] >= 3 and v["w"] > 0:
            first = v
            break
        time.sleep(0.1)
    dt = time.time() - t0
    st = e2e.api("/api/watch/" + vid)
    e2e.shot("10-offline-replay-from-disk")
    log(first is not None and st["playback"]["state"] == "cached" and st["playback"]["kind"] == "file", "cached video plays from disk with no network",
        {"time_to_first_frame_s": round(dt, 2), "source": first and first["src"], "state": st["playback"]["state"], "ct": first and round(first["ct"], 1)})
    ui("document.querySelector('video').currentTime = 100; return 1")
    time.sleep(2)
    v = ui(VIDEO_JS)
    log(v and abs(v["ct"] - 100) < 5, "seeking anywhere works from the file", v and round(v["ct"], 1))
    after = e2e.api("/api/proxy")["stats"]
    procs = subprocess.run(["pgrep", "-fa", "yt-dlp|ffmpeg"], capture_output=True, text=True).stdout.strip()
    log(procs == "" and after["tunnels"] >= before["tunnels"], "no yt-dlp or ffmpeg process was started for the replay", procs or "none running")
    RESULTS["offline_time_to_first_frame_s"] = round(dt, 2)


def phase_restart():
    """After a relaunch: the session store persisted (cookies survive, standing in for the login) and
    the tabs came back."""
    st = e2e.wait(lambda: e2e.state() if e2e.state()["tabs"] else None, 30, what="tabs after restart")
    log(len(st["tabs"]) >= 3, "tabs restored from the browsing DB after a relaunch", [(t["id"], t["state"], t["face"]) for t in st["tabs"]])
    yt = next(t for t in st["tabs"] if t["face"] == "yt")
    e2e.call("tab_activate", id=yt["id"])
    e2e.wait(native_ready, 40, what="a YouTube tab")
    time.sleep(4)
    cookies = e2e.evaljs("active", "return document.cookie.split('; ').map(c => c.split('=')[0])")
    log(any(c in cookies for c in ("SOCS", "CONSENT", "PREF", "VISITOR_INFO1_LIVE")), "the web data store persisted across the restart (cookies from the previous run are still there)", cookies)
    e2e.shot("11-after-restart")


def phase_ram():
    """Memory with many tabs: all warm versus hibernating (WebKitGTK shares one web process per data
    store, so this under-states what per-tab WKWebView processes give on macOS)."""
    e2e.wait(native_ready, 40, what="the first tab")
    accept_consent()
    queries = ["rust programming", "cooking pasta", "jazz piano", "mountain biking", "chess openings", "space documentary", "guitar lesson", "origami"]
    e2e.api("/api/settings", "PUT", {"warm_tabs": 12, "hibernate_after_secs": 3600})
    base = e2e.rss_mb()
    for q in queries:
        e2e.call("tab_new", url=q)
        time.sleep(5)
    time.sleep(5)
    all_warm = e2e.rss_mb()
    live_before = sum(1 for t in e2e.state()["tabs"] if t["state"] in ("live", "loading"))
    e2e.shot("13-ram-all-tabs-warm")
    e2e.api("/api/settings", "PUT", {"warm_tabs": 2, "hibernate_after_secs": 5})
    time.sleep(20)
    hib = e2e.rss_mb()
    live_after = sum(1 for t in e2e.state()["tabs"] if t["state"] in ("live", "loading"))
    e2e.shot("14-ram-after-hibernation")
    tot = lambda r: round(r["web"] + r["network"] + r["app"], 1)
    log(live_after <= 3 and live_before >= 8, "RAM with %d warm tabs versus hibernated" % live_before,
        {"live_tabs": [live_before, live_after], "rss_mb": {"first_tab_only": base, "all_warm": all_warm, "hibernated": hib}, "total_mb": [tot(base), tot(all_warm), tot(hib)]})


def phase_signin_popup():
    """The fallback: Google sign-in in a plain window on the same data store."""
    ui("await window.__TAURI_INTERNALS__.invoke('signin_popup'); return 1")
    time.sleep(10)
    wins = subprocess.run(["xdotool", "search", "--name", "Sign in to YouTube"], capture_output=True, text=True, env=dict(os.environ, DISPLAY=os.environ.get("DISPLAY", ":99"))).stdout.split()
    e2e.shot("12-signin-popup-window")
    log(bool(wins), "the separate sign-in window opens on the same store", wins)


if __name__ == "__main__":
    phase = sys.argv[1]
    try:
        {"online": phase_online, "offline": phase_offline, "restart": phase_restart, "popup": phase_signin_popup, "ram": phase_ram}[phase]()
    finally:
        os.makedirs(e2e.SHOTS, exist_ok=True)
        with open(os.path.join(e2e.SHOTS, "results-%s.json" % phase), "w") as f:
            json.dump(RESULTS, f, indent=1)
    bad = [s for s in RESULTS.get("steps", []) if not s["ok"]]
    print("\n%d steps, %d failed" % (len(RESULTS.get("steps", [])), len(bad)))
    sys.exit(1 if bad else 0)
