"""Production Tauri/WebKitGTK player regression; run inside the isolated E2E app.

Uses actual import/watch APIs and physical X pointer clicks, never mock.js.
Does not establish native macOS HLS/AVFoundation behavior.
"""
import json
import os
import subprocess
import tempfile
import time
import e2e

VIDEO = "zzzzzzzzzzz"


def js(code):
    result = e2e.evaljs("chrome", code)
    assert not isinstance(result, dict) or "error" not in result, result
    return result


def click(selector):
    point = js("const b=document.querySelector(" + json.dumps(selector) + ").getBoundingClientRect(); return {x:screenX+b.x+b.width/2,y:screenY+b.y+b.height/2};")
    subprocess.run(["xdotool", "mousemove", "--sync", str(round(point["x"])), str(round(point["y"])), "click", "1"], check=True, env=dict(os.environ, DISPLAY=":99"))
    time.sleep(0.3)


def video():
    return js("const v=document.querySelector('#view-watch video');return {time:v.currentTime,ready:v.readyState,paused:v.paused,seeking:v.seeking,rate:v.playbackRate,width:v.videoWidth,height:v.videoHeight,frames:v.getVideoPlaybackQuality?.().totalVideoFrames??v.webkitDecodedFrameCount,source:v.currentSrc.replace(Muninn.token,'<token>'),error:v.error?.message};")


def advancing():
    start = video()
    time.sleep(1)
    end = video()
    # WebKitGTK's MSE path can expose an unimplemented counter fixed at zero.
    frames = not (start.get("frames") or end.get("frames")) or end["frames"] > start["frames"]
    return end if end["time"] > start["time"] + 0.2 and end["ready"]>=2 and end["width"]>0 and frames else None


def geometry():
    return js("const r=n=>n.getBoundingClientRect().toJSON(); return {stage:r(document.querySelector('.pv-stage')),pane:r(document.querySelector('#content')),bar:r(document.querySelector('.pc-bar')),pressed:document.querySelector('[aria-label=\"Theatre mode\"]').getAttribute('aria-pressed')};")


def main():
    e2e.api("/api/settings", "PUT", {"auto_prefetch": False, "min_free_bytes": 0})
    fixture=tempfile.mkdtemp(prefix="muninn-local-player-")
    source = os.path.join(fixture,"video.webm")
    subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30", "-t", "120", "-c:v", "libvpx-vp9", "-threads", "2", "-deadline", "realtime", "-cpu-used", "8", source], check=True)
    imported = e2e.api("/api/import/media", "POST", {"path": source, "video_id": VIDEO})
    mp4 = os.path.join(fixture,"video.mp4")
    subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i", source, "-c:v", "libx264", "-threads", "2", "-preset", "ultrafast", "-movflags", "+faststart", mp4], check=True)
    with open(os.path.join(fixture,"video.info.json"), "w") as f:
        json.dump({"id": VIDEO, "height": 180, "title": "Local playback regression", "duration": 120}, f)
    alternate = e2e.api("/api/import/media", "POST", {"path": mp4, "video_id": VIDEO})["entry"]["id"]
    e2e.api("/api/ingest/video", "POST", {"video_id": VIDEO, "title": "Local playback regression", "duration_seconds": 120})
    e2e.api("/api/ingest/comments", "POST", {"video_id": VIDEO, "comments": []})
    js("window.__heldEngine=null;const append=document.head.append.bind(document.head);document.head.append=function(...nodes){if(nodes.some(n=>n.tagName==='SCRIPT'&&n.getAttribute('src')==='/ui/hls.min.js')){window.__heldEngine=()=>append(...nodes);return;}return append(...nodes);};return true;")
    e2e.call("tab_new", url="https://www.youtube.com/watch?v=" + VIDEO)
    e2e.wait(lambda: js("return !document.querySelector('#view-watch').hidden;"), what="production watch view")
    e2e.wait(lambda: js("return !!window.__heldEngine;"), what="delayed first HLS engine load")

    def select(entry):
        js("const s=document.querySelector('[aria-label=\"Playback source\"]');s.value=" + json.dumps(entry) + ";s.dispatchEvent(new Event('change'));return true;")

    select(alternate)
    e2e.wait(advancing, what="MP4 selection while HLS engine is loading")
    js("window.__heldEngine();return true;")
    time.sleep(1)
    e2e.wait(lambda: js("return document.querySelector('.pv-overlay').hidden && document.querySelector('.player').dataset.engine==='file';"), 5, what="stale engine overlay cleared")
    resume_target=video()["time"]
    assert resume_target>0.2, "MP4 must advance before switching"
    js("const api=Muninn.api;window.__heldPreparation=null;Muninn.api=function(method,path,...args){if(method==='POST'&&path.endsWith('/playback'))return new Promise((resolve,reject)=>{window.__heldPreparation=()=>api(method,path,...args).then(resolve,reject);});return api(method,path,...args);};window.__restoreApi=()=>{Muninn.api=api;};return true;")
    select(imported["entry"]["id"])
    e2e.wait(lambda: js("return !!window.__heldPreparation;"), what="delayed local preparation")
    select(alternate)
    e2e.wait(advancing, what="switch back during preparation")
    assert js("const c=Muninn.watch.state();return c.resumeTarget>="+str(resume_target)+"&&c.playIntent!==false;"), "pending selection lost resume position/play intent"
    js("window.__restoreApi();window.__heldPreparation();return true;")
    time.sleep(1)
    assert "/media/"+alternate in video()["source"], "stale preparation replaced selected MP4"
    select(imported["entry"]["id"])
    e2e.wait(advancing, 45, what="local frames/time advancing")
    window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--name", "^Muninn$"], env=dict(os.environ, DISPLAY=":99"), text=True).splitlines()[0]
    sizes = []
    for width, height in [(1000, 700), (1360, 832), (1440, 880)]:
        subprocess.run(["xdotool", "windowsize", window, str(width), str(height)], check=True, env=dict(os.environ, DISPLAY=":99"))
        time.sleep(0.5)
        before = geometry()
        click('[aria-label="Theatre mode"]')
        during = geometry()
        assert during["pressed"] == "true", during
        target = during["pane"]["height"] - during["bar"]["height"]
        assert abs(during["stage"]["height"] - target) < 3, {"expected_height": target, "actual": during}
        click('[aria-label="Theatre mode"]')
        after = geometry()
        assert after["pressed"] == "false" and abs(after["stage"]["height"] - before["stage"]["height"]) < 2, after
        sizes.append({"size": [width, height], "before": before, "during": during, "after": after})
    js("const s=document.querySelector('[aria-label=\"Playback speed\"]');s.value='1.5';s.dispatchEvent(new Event('change'));return true;")
    e2e.wait(advancing, what="1.5x local playback")
    initial = video()
    assert initial["rate"] == 1.5, initial
    for _ in range(3):
        click('#nav-reload')
        e2e.wait(advancing, 30, what="immediate same-video reopen")
    js("const v=document.querySelector('#view-watch video');v.currentTime=20;return true;")
    e2e.wait(lambda: (lambda v: v if abs(v["time"]-20)<2 and not v["seeking"] else None)(video()), 5, what="local seek completion")
    e2e.wait(advancing, what="frames after local seek")
    logs = e2e.api("/api/logs")
    events = logs if isinstance(logs, list) else logs.get("events", [])
    assert any(e.get("event") == "ui.player.theatre" for e in events), "theatre event missing"
    assert any(e.get("event") == "hls.served" for e in events), "HLS request logs missing"
    print(json.dumps({"passed": True, "geometry": sizes, "playback": video(), "entry": imported["entry"]["id"]}, indent=2))


if __name__ == "__main__":
    main()
