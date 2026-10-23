# Recorded HLS startup

Before the fix, the production watch UI with real hls.js started a never-played
60-second EVENT recording at **52.21 seconds**. Its playlist had twenty 3-second
segments and no ENDLIST, as during download/conversion. hls.js used its default
live start, and the resume path accepted that offset when saved progress was zero.

Recorded hls.js playback now starts explicitly at zero. The resume path also checks
zero against the media position instead of immediately marking it done. Native
HLS receives `EXT-X-START:TIME-OFFSET=0,PRECISE=YES` in the served playlist; ffmpeg's
file is not edited. Saved resume points still use the existing resume policy.

The same real-media browser reproducer reached **1.21 seconds after 1.2 seconds**
of playback with the fix. Retained browser scenarios check a new recording,
resuming at 20 seconds, and restarting a near-completed recording. HTTP tests
check the hint for download/local-conversion playlists, HEAD lengths, unchanged
segment references, no-store and no mutation of ffmpeg's file.

Run the offline fixture on Linux (ffmpeg and Playwright CLI required):

```sh
mkdir -p /tmp/muninn-recorded-start
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=320x180:rate=15 \
  -t 60 -c:v libx264 -preset ultrafast -g 45 -keyint_min 45 -sc_threshold 0 \
  -hls_time 3 -hls_playlist_type event -hls_flags omit_endlist \
  -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 \
  -hls_segment_filename /tmp/muninn-recorded-start/seg_%05d.m4s \
  /tmp/muninn-recorded-start/index.m3u8
python3 e2e/recorded-start-server.py --media-dir /tmp/muninn-recorded-start
```

In another terminal, open a dedicated Playwright CLI session on localhost:47942
and run `e2e/recorded-start.js` with `run-code --filename`. The fixture uses the
production video element and bundled hls.js, with synthetic native tab/metadata
APIs. It does not use a YouTube account. It deliberately serves raw EVENT HLS;
the native start hint is covered separately by the production HTTP route tests.

This reproduces and verifies the hls.js path. Actual macOS WKWebView/native HLS
startup remains unverified; cross-compilation is not a native runtime check.

References: [hls.js startPosition](https://github.com/video-dev/hls.js/blob/master/docs/API.md#startposition),
[HLS EXT-X-START](https://www.rfc-editor.org/rfc/rfc8216.html#section-4.3.5.2).
