# Local compatibility playback: windows and the timeline contract

MKV/WebM copies are not played directly. ffmpeg writes a temporary fragmented-MP4
HLS stream under the cache directory; the original file is never modified and no
remote download is started for a complete local copy.

## Symptom this addresses

A long cached WebM resumed late in the video took about 40 seconds to start. The
conversion always began at 0 s and the player waited until the prefix up to the
resume point was encoded before attaching the source.

## Timeline contract

HLS output is **window relative**. The input is seeked (`-ss` before `-i`, frame
accurate) to a window start a segment boundary a little before the requested
position, and everything from there to the end of the source is prepared. Playlist
time 0 is original time `offset_seconds`:

    original time = offset_seconds + video.currentTime

`GET/POST /api/entries/{id}/playback` (and `playback` inside the watch status)
report, all in original seconds unless stated:

| field | meaning |
| --- | --- |
| `offset_seconds` | original time of playlist time 0 (window start) |
| `prepared_seconds` | window-relative length written so far |
| `prepared_until` | `offset_seconds + prepared_seconds` |
| `window_complete` | the window reaches the end of the source |
| `target_seconds` | position the latest caller asked for |
| `duration_seconds` | duration of the **original** (entry metadata or `ffmpeg -i`) |
| `ready` | `prepared_until` is past `target_seconds`, or the window is complete |
| `mode` | `encode` or `remux` |
| `job_id`, `generation` | ffmpeg job and latest session using it |

The UI keeps duration, the seek bar, saved progress, completion percentage, the
cleanup point, the mini player and resume in original time and converts only at
`video.currentTime`. Window-relative time was chosen over `-output_ts_offset`
so the player never depends on how a given engine (native HLS or hls.js) exposes a
non-zero media start time as `currentTime`.

## Seeking, reuse and cancellation

* A seek inside `[offset_seconds, prepared_until]` is a plain seek.
* Any other seek asks for a window near the new target (`POST ...?position=`),
  debounced in the UI. One ffmpeg job runs at a time; the previous job is cancelled.
* A running or finished window that covers the target is reused: reopening or
  seeking inside it starts nothing. Finished windows are kept (at most two, within a
  quarter of the cache budget) and the least recently used is evicted.
* `POST .../playback/stop?generation=N` only stops the job if `N` is still its
  latest session, so a late stop from a replaced session cannot end the current one.
* `POST ...?position=&client=TOKEN&seq=N`: the UI numbers its prepares per page. A prepare
  with a `seq` lower than the newest already seen for that entry and `client` is refused
  (HTTP 400, nothing changes), so one handled late cannot hand the UI a stale generation.
* The running job is bounded by `max_disk_bytes / 4` and `min_free_bytes`.

## Codec-dependent path

The streams reported by `ffmpeg -i` decide the path, not the file name. H.264
(8-bit 4:2:0) with AAC or no audio is remuxed with `-c copy`. Stream copy can only
cut on source keyframes, so a remux window always starts at 0 to keep the timeline
exact; it is I/O bound. AV1, VP9, Opus and anything else are encoded (VideoToolbox
on macOS, x264 on development hosts).

## Verification

`core/tests/local_playback.rs` uses synthetic ffmpeg media and a spawn-counting
ffmpeg wrapper; `ui/test/lib.test.js` covers the offset math. These do not
establish native macOS behaviour: long-resume cold and warm time to first frame,
an immediate local MP4, and AVFoundation seeking inside an offset window need a
check on a Mac.
