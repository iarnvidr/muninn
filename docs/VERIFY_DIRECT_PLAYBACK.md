# Direct playback and cancelled recovery

The supplied macOS trace identifies an H.264/AAC MP4, with media buffered and
no decoder error, whose playback clock is not advancing. It does not establish
an unsupported codec or justify re-encoding or repackaging that file.

The production player was querying native media ranges from its diagnostic logger
during `play`/`loadstart`, before metadata existed. This bypassed the readiness
guard already used by the seek control. The reproduction showed a 30-second MP4
reporting a duration near elapsed playback time, `ended=true` and readyState 2.
A bare video element, in the same native webview and using the same unmodified
file through the same HTTP endpoint, correctly reported duration 30 and readyState
4. Adding an early range query to that bare element also damaged its duration.

Range diagnostics now guard the getter itself until metadata exists. Early
duration and decoded-frame inspection are guarded too. Complete file controls no
longer query native seek ranges unnecessarily. The real production player now
plays direct MP4 correctly in the reproducer, without media processing.

Two additional lifecycle defects are addressed:

- The stall watchdog included time spent paused. The `play` handler evaluated
  it before `playing` reset its clock, so resuming after a long pause could
  immediately replace direct playback with compatibility preparation. Start a
  fresh progress window on `play`, before evaluating controls.
- Cancelling local recovery left its stopped job ahead of the original complete
  file in watch status. Cancelled jobs no longer advertise a playback source;
  reopening a supported MP4 selects its original `/media/` endpoint again.
  Unrelated preparation failures and healthy completed jobs retain their status.

No download, encoding or repackaging policy is changed. Diagnostic player events
now include play/pause transitions and play rejection names. Media probes retain
the duration/start timing line alongside the codec information.

`core/tests/local_playback.rs` covers repeated cancellation/reopening of MP4 and
the existing cancellation acknowledgement and MKV/WebM recovery paths.
`e2e/direct-player.py` uses the actual Tauri application and its media HTTP server,
with real H.264/AAC MP4 files both with and without faststart. It requires advancing
playback, pause longer than ten seconds, resume, and seeking while retaining the
original file source and issuing zero compatibility-preparation requests. Run
inside an isolated Linux E2E container after `e2e/run-app.sh`; no mock.js or account
is used. It imports synthetic files without modifying them. Controls are invoked
through their actual DOM click handlers, not physical pointer coordinates.

Validation: 204 core unit tests and 29 integration tests pass, including the new
repeated MP4 cancellation/reopen regression; 48 JavaScript checks include a
native getter that throws if diagnostics touch it before metadata. The real
Tauri/WebKitGTK regression now passes: correct 90-second duration, frame/time
advancement, pause longer than ten seconds, resume, seek and revisit, original
`/media/` sources retained and zero preparation requests. It covers both MP4
layouts. The closer H.264 High/AAC stereo fixture also compares complete and
overlapping real HTTP range response bodies against the unchanged original.

Earlier audio-backend warnings were not sufficient to attribute the failed test
to the environment. The paired bare-element test localized a production-player
defect, and guarding premature inspection corrected it in the same environment.

These checks exercise Linux WebKitGTK. They do not establish native Mac behavior
or prove the reported Mac playback stall is resolved. The existing server logs
record response headers, not complete body delivery. If a fresh trace still shows
the direct stall, compare the unchanged file through the same endpoint in a bare
native-controls video, then Safari and QuickTime, and verify delivered range
bodies against the original. Those comparisons separate player lifecycle from
the native media stack and HTTP/file issues without changing formats.
