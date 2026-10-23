"""Direct H.264/AAC playback in the real Tauri/WebKitGTK app, without mock.js.

Run inside an isolated Linux E2E container after run-app.sh. This does not
establish macOS/AVFoundation behavior. Fixtures are imported without processing.
"""
import importlib.util
import json
import os
import subprocess
import tempfile
import time
import urllib.request
import e2e

spec = importlib.util.spec_from_file_location("local_player", os.path.join(os.path.dirname(__file__), "local-player.py"))
player = importlib.util.module_from_spec(spec)
spec.loader.exec_module(player)


def main():
    e2e.api("/api/settings", "PUT", {"offline_mode": True, "auto_prefetch": False, "min_free_bytes": 0})
    directory = tempfile.mkdtemp(prefix="muninn-direct-player-")
    player.js("const api=Muninn.api;window.__preparations=[];Muninn.api=function(method,path,...args){if(method==='POST'&&/^\\/api\\/entries\\/[^/]+\\/playback$/.test(path))window.__preparations.push(path);return api(method,path,...args);};return true;")
    results = []
    tabs = []
    for index, faststart in enumerate([True, False]):
        video_id = ["ddddddddddd", "eeeeeeeeeee"][index]
        path = os.path.join(directory, video_id + ".mp4")
        args = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100", "-t", "90", "-c:v", "libx264", "-threads", "2", "-preset", "veryfast", "-profile:v", "high", "-c:a", "aac", "-ac", "2"]
        if faststart:
            args += ["-movflags", "+faststart"]
        subprocess.run(args + [path], check=True)
        imported = e2e.api("/api/import/media", "POST", {"path": path, "video_id": video_id})
        entry = imported["entry"]["id"]
        # Verify delivered bodies, not only headers, through the real HTTP server.
        with open(path, "rb") as file:
            original_bytes = file.read()
        base, token = e2e.service()
        for start, end in [(0, 1), (0, len(original_bytes)-1), (262000, 530000), (262144, 393215)]:
            request = urllib.request.Request(base + "/media/" + entry, headers={"Authorization": "Bearer " + token, "Range": f"bytes={start}-{end}"})
            with urllib.request.urlopen(request) as response:
                assert response.status == 206
                assert response.headers["Content-Range"] == f"bytes {start}-{end}/{len(original_bytes)}"
                assert response.read() == original_bytes[start:end+1]
        e2e.api("/api/ingest/video", "POST", {"video_id": video_id, "title": "Direct player fixture", "duration_seconds": 90})
        e2e.api("/api/ingest/comments", "POST", {"video_id": video_id, "comments": []})
        tabs.append(e2e.call("tab_new", url="https://www.youtube.com/watch?v=" + video_id)["id"])
        e2e.wait(player.advancing, 20, what="direct MP4 advancing")
        media = player.js('const v=document.querySelector("#view-watch video");return {duration:v.duration,ended:v.ended};')
        assert 89 < media["duration"] < 91 and not media["ended"], media
        original = player.video()["source"]
        assert "/media/" + entry in original, original
        player.js('document.querySelector("#view-watch .pc-controls-main button").click();return true;')
        assert player.video()["paused"], player.video()
        time.sleep(11.2)
        assert player.video()["paused"], player.video()
        player.js('document.querySelector("#view-watch .pc-controls-main button").click();return true;')
        e2e.wait(player.advancing, 5, what="direct MP4 advances after long pause")
        for _ in range(3):
            assert player.video()["source"] == original, player.video()
            assert player.js("return window.__preparations.length;") == 0
            time.sleep(0.5)
        player.js("const v=document.querySelector('#view-watch video');v.currentTime=30;return true;")
        e2e.wait(lambda: player.video()["time"] >= 30 and not player.video()["seeking"], 5, what="direct MP4 seek")
        e2e.wait(player.advancing, 5, what="direct MP4 advances after seek")
        assert player.video()["source"] == original
        assert player.js("return window.__preparations.length;") == 0
        results.append({"faststart": faststart, "state": player.video()})
    e2e.call("tab_activate", id=tabs[0])
    e2e.wait(player.advancing, 10, what="original MP4 advances on revisit")
    assert "/media/ddddddddddd." in player.video()["source"], player.video()
    assert player.js("return window.__preparations.length;") == 0
    print(json.dumps({"passed": True, "preparation_requests": 0, "results": results}))


if __name__ == "__main__":
    main()
