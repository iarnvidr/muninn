# Muninn 0.2.8: playback lifecycle and theatre regression

The reported Mac spinner is not established as fixed. The available Mac archive is
from 0.2.6; it proves direct AV1 WebM stalls and subsequent local H.264/AAC HLS
playback, but contains no 0.2.7 run. A fresh Mac trace is still required. Two
independent reviewers agreed to retain the existing local media format and correct
demonstrated lifecycle bugs, rather than redesign playback around an unproven cause.

Same-entry stop/start is now serialized. The stop endpoint captures the worker,
requests cancellation and acknowledges only after that worker exits, waiting outside
the job-map lock in a blocking worker thread. Preparation waits for a stopping worker
instead of returning the former transient error. A bounded timeout remains an error.
Frontend stop barriers survive watch-session replacement; stale preparation and poll
callbacks cannot stop or replace a newer same-entry selection. Retry keeps position
and play/pause intent. Explicit copy selections retain their playback source. A waiter
drains subsequently queued stops rather than erasing their barrier. Source replacement
clears the old loading/buffering state. A resume is acknowledged only after the
engine reaches the target, rather than immediately after assigning currentTime.

Local playback now polls fresh full watch status and overlays selected local playback,
so metadata/comments completion is no longer frozen at the old status. Working HLS
stays attached when the backend reports a completed cache file; polling does not
automatically promote it to a different source.

Theatre previously changed the real WebKit stage from its normal size to only
463.5 px high in a 1000×700 window, despite 567 px being available. The physical
toggle was received but the aspect ratio kept it short. Theatre now uses the content
pane height minus its measured control bar, removes the aspect-ratio constraint, and
restores normal layout on exit. Entering theatre also exits the full-window fallback.

The previous theatre diagnostic request was rejected by the client-log allowlist.
The allowlist now accepts theatre and playback-state events. Theatre records before/
after geometry. Playback records loading, waiting, seeking, readiness, rate and source,
with the token redacted. HLS requests record file, status, MIME type, range and length
without token-bearing paths or authorization headers. Regression tests cover these
events reaching the export log and stop acknowledgement before same-copy reopen.

Production-app regression: e2e/local-player.py runs the actual Tauri player under
WebKitGTK/Xvfb, imports a real WebM fixture and clicks controls through the X pointer.
No mock native tabs or standalone replacement video element is used. The unmodified
0.2.7 app fails the pane-height assertion. The changed app passes:

| Window | Theatre stage | Pane minus controls |
| --- | ---: | ---: |
| 1000×700 | 567 px | 567 px |
| 1360×832 | 699 px | 699 px |
| 1440×880 | 747 px | 747 px |

The test also advances local playback at 1.5×, reopens the same video three times,
seeks to 20 seconds, and ends with readyState 4, seeking false and time advancing.
It verifies theatre toggle-back and diagnostic event delivery. The fixture uses
WebKitGTK's hls.js path; macOS uses native AVFoundation HLS and remains unverified.
The test also delays the first hls.js load, selects MP4, releases the obsolete
load and checks that the MP4 source and clear overlay remain. It delays a local
preparation response, changes copies and checks preserved resume/play intent and
rejection of the obsolete response. Decoded dimensions and playback time must
advance; frame counters are checked where the engine supplies a nonzero counter.
WebKitGTK's MSE counter can remain zero despite playback.
Full Rust, JavaScript and packaging checks run before the Apple Silicon test bundle.

Run inside an isolated Linux E2E container after building the app:

```bash
cd /build/e2e
./run-app.sh /cache/target/debug/muninn /usr/bin
python3 local-player.py
```

The launcher resets its isolated test home. Do not point it at user application data.
