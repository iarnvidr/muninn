/* Muninn UI: pure logic (no DOM, no network). Loaded by index.html as a classic
 * script (window.MuninnLib) and require()-able from node for the unit tests. */
(function (root, factory) {
  'use strict';
  const lib = factory();
  if (typeof module === 'object' && module.exports) module.exports = lib;
  else root.MuninnLib = lib;
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  /* ------------------------------------------------------------------ formatting */

  /** m:ss or h:mm:ss. Invalid input reads as 0:00 so the control bar never shows NaN. */
  function fmtClock(sec) {
    if (sec == null || !isFinite(sec) || sec < 0) sec = 0;
    sec = Math.floor(sec);
    const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = sec % 60;
    const p = (x) => String(x).padStart(2, '0');
    return h ? `${h}:${p(m)}:${p(s)}` : `${m}:${p(s)}`;
  }

  /** Like fmtClock but empty for "unknown" (thumbnails without a duration). */
  function fmtDuration(sec) {
    return sec == null || !isFinite(sec) || sec <= 0 ? '' : fmtClock(sec);
  }

  function fmtBytes(n) {
    if (n == null || isNaN(n)) return '-';
    const u = ['B', 'KB', 'MB', 'GB', 'TB'];
    let v = Number(n), i = 0;
    while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
    const d = i === 0 ? 0 : (v >= 100 ? 0 : (v >= 10 ? 1 : 2));
    return v.toFixed(d) + ' ' + u[i];
  }

  /** 1234 -> "1.2K", 1500000 -> "1.5M". */
  function fmtCount(n) {
    if (n == null || isNaN(n)) return '';
    n = Number(n);
    if (n < 1000) return String(n);
    const f = (v, s) => (v >= 10 ? Math.round(v) : Math.round(v * 10) / 10) + s;
    if (n < 1e6) return f(n / 1e3, 'K');
    if (n < 1e9) return f(n / 1e6, 'M');
    return f(n / 1e9, 'B');
  }

  function relTime(ms, now) {
    if (ms == null) return '-';
    if (now == null) now = Date.now();
    const diff = ms - now;
    const a = Math.abs(diff);
    let txt;
    if (a < 45e3) return 'just now';
    if (a < 90 * 60e3) txt = Math.round(a / 60e3) + ' min';
    else if (a < 36 * 3600e3) txt = Math.round(a / 3600e3) + ' h';
    else if (a < 90 * 86400e3) txt = Math.round(a / 86400e3) + ' d';
    else txt = Math.round(a / (30 * 86400e3)) + ' mo';
    return diff < 0 ? txt + ' ago' : 'in ' + txt;
  }

  function pct(p) { return Math.max(0, Math.min(100, Math.round((Number(p) || 0) * 100))); }
  function qLabel(variant) { return variant === 'audio' ? 'audio' : (variant ? variant + 'p' : '-'); }
  function fraction(pos, dur) { return dur > 0 && isFinite(dur) ? Math.max(0, Math.min(1, (Number(pos) || 0) / dur)) : 0; }
  function asArray(x, key) { return Array.isArray(x) ? x : (x && Array.isArray(x[key]) ? x[key] : []); }

  /* ------------------------------------------------------------------ views and tabs */

  /** Which thing fills the content area. See the view rule in the task description. */
  function chooseView(internalPage, tab) {
    if (internalPage) return 'internal';
    if (!tab) return 'empty';
    if (tab.face === 'watch' && (tab.video_id || tab.playlist)) return 'watch';
    return tab.live ? 'native' : 'restore';
  }

  function tabTitle(tab) {
    if (!tab) return '';
    if (tab.title && String(tab.title).trim()) return String(tab.title).trim();
    if (tab.face === 'watch') return 'Video';
    if (tab.url) {
      try { return new URL(tab.url).hostname.replace(/^www\./, '') || tab.url; } catch (e) { return tab.url; }
    }
    return 'New tab';
  }

  /** What the address bar shows for a tab. */
  function addressText(tab) {
    if (!tab) return '';
    if (tab.face === 'watch' && tab.video_id) return 'https://www.youtube.com/watch?v=' + tab.video_id;
    return tab.url || '';
  }

  function channelUrl(channelId) { return 'https://www.youtube.com/channel/' + encodeURIComponent(channelId); }

  function extractVideoId(s) {
    s = (s || '').trim();
    const m = /[?&]v=([A-Za-z0-9_-]{11})/.exec(s) || /youtu\.be\/([A-Za-z0-9_-]{11})/.exec(s) || /(?:shorts|embed|live)\/([A-Za-z0-9_-]{11})/.exec(s);
    return m ? m[1] : '';
  }

  /** Whether the tab's page can be hibernated by hand (a sleeping or watch tab has nothing to free). */
  function canHibernate(tab) { return !!tab && tab.face === 'yt' && (tab.state === 'live' || tab.state === 'loading') && !!tab.live; }

  /** Window-level shortcut for a keydown event description ({key, metaKey, ctrlKey, altKey, shiftKey}). */
  function shortcutAction(e) {
    if (!(e.metaKey || e.ctrlKey) || e.altKey) return null;
    const k = String(e.key || '').toLowerCase();
    if (e.shiftKey && k !== '[' && k !== ']') return null;
    switch (k) {
      case 't': return 'new-tab';
      case 'w': return 'close-tab';
      case 'l': return 'focus-address';
      case 'r': return 'reload';
      case '[': return e.metaKey ? 'back' : null;
      case ']': return e.metaKey ? 'forward' : null;
      default: return null;
    }
  }

  /* ------------------------------------------------------------------ watch view */

  const SEEK_STEP = 5;
  const SEEK_MARGIN = 1;

  // A waiter may only clear the stop it awaited; later queued stops still own
  // the same entry and must finish before that entry can be reopened.
  function stopBarrier(stop) {
    const pending = new Map();
    const extras = new Map();
    // `extra` (the session generation) is captured when the stop is queued and travels with
    // every attempt, retries included, so a retried stop still cannot end a newer session of
    // the same entry, and a stop queued earlier never picks up a later stop's generation.
    function queue(id, extra) {
      if (arguments.length > 1) extras.set(id, extra);
      const own = extras.get(id);
      const next = (pending.get(id) || Promise.resolve()).catch(() => {}).then(() => stop(id, own));
      pending.set(id, next);
      return next;
    }
    async function wait(id) {
      while (pending.has(id)) {
        const current = pending.get(id);
        try { await current; }
        catch (error) { if (pending.get(id) === current) await queue(id); }
        if (pending.get(id) === current) pending.delete(id);
      }
    }
    async function waitVideo(id) {
      for (const entry of pending.keys()) if (entry.startsWith(id + '.')) await wait(entry);
    }
    return { queue, wait, waitVideo };
  }

  /* Pure helpers for the local-playback seek / restart / loading-frame state machine. */

  const SEEK_DEBOUNCE_MS = 250;
  const MAX_LOCAL_RESTARTS = 3;

  // A debounced window preparation fires only if it is still the wanted one: the same
  // viewing session is on screen and no newer seek (or other prepare) replaced its target.
  function seekDue(c, current, target) {
    return !!c && c === current && c.seekTarget === target;
  }

  // The backend prepared a window that does not cover the resume position: ask again, but
  // only once at a time and at most MAX_LOCAL_RESTARTS times per viewing session.
  function mayRestartLocal(c) {
    return !c.localRestarting && (c.localRestarts || 0) < MAX_LOCAL_RESTARTS;
  }

  // The loading feedback (awaitingFrame) ends once the source is attached, the resume seek is
  // done and the player shows a frame: not while seeking, nor before a frame is decodable.
  function frameSettled(c, v) {
    if (!c || !c.awaitingFrame || !c.sourceSet || !c.resumeDone) return false;
    return v.readyState >= 2 && !v.seeking && (v.videoWidth > 0 || v.readyState >= 3);
  }

  // Whether the loading spinner shows: buffering while playing, or until the first frame.
  function showSpinner(c, paused) {
    return !!c.buffering && (!paused || !!c.awaitingFrame);
  }

  function isTypingTarget(t) {
    if (!t) return false;
    if (t.editable) return true;
    const tag = String(t.tag || '').toUpperCase();
    if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
    return tag === 'INPUT' && !['range', 'checkbox', 'radio', 'button'].includes(String(t.type || 'text').toLowerCase());
  }

  /** Player shortcut for a keydown ({key, metaKey, ctrlKey, altKey}, target {tag,type,editable}). */
  function playerKeyAction(e, target) {
    if (e.isComposing || e.metaKey || e.ctrlKey || e.altKey || isTypingTarget(target)) return null;
    const tag = target && String(target.tag || '').toUpperCase();
    // Native controls and buttons own their activation/navigation keys.
    if (['BUTTON', 'SELECT', 'TEXTAREA'].includes(tag) || (tag === 'INPUT')) return null;
    const key = String(e.key || '');
    switch (key) {
      case ' ': case 'k': case 'K': return 'toggle';
      case 'j': case 'J': return { seek: -10 };
      case 'l': case 'L': return { seek: 10 };
      case 'ArrowLeft': return { seek: -5 };
      case 'ArrowRight': return { seek: 5 };
      case 'ArrowUp': return { volume: 5 };
      case 'ArrowDown': return { volume: -5 };
      case 'f': case 'F': return 'fullscreen';
      case 'm': case 'M': return 'mute';
      case 'i': case 'I': return 'pip';
      case ',': return 'frame-back';
      case '.': return 'frame-forward';
      case '<': return { speed: -1 };
      case '>': return { speed: 1 };
      case 'Home': return 'start';
      case 'End': return 'end';
      default:
        if (/^[0-9]$/.test(key)) return { percent: Number(key) * 10 };
        return null;
    }
  }

  /** The playable URL for a watch status's playback, or null while it is not ready. */
  function mediaUrl(playback, origin, token) {
    if (!playback || !playback.ready || !playback.url_path) return null;
    if (playback.kind === 'hls') return origin + playback.url_path;   // the token is part of the path
    return origin + playback.url_path + (playback.url_path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token || '');
  }

  function withToken(path, token) {
    if (!path) return '';
    return path + (path.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(token || '');
  }

  /** The duration the seek bar uses: what the server knows beats what the (growing) stream has so far. */
  function effectiveDuration(playback, meta, videoDuration) {
    const cands = [playback && playback.duration_seconds, meta && meta.duration_seconds, videoDuration];
    for (const c of cands) if (c != null && isFinite(c) && c > 0) return Number(c);
    return 0;
  }


  /* ------------------------------------------------------------------ local playback window
   * Local compatibility playback is prepared as a window of the original video.
   * Playlist time 0 is original time `offset_seconds` (see core/src/local_playback.rs), so
   * original time = offset + video.currentTime. `win` is {offset, until, complete}: the
   * original-time start and prepared end of the window, and whether it reaches the end. */

  const WINDOW_SEEK_MARGIN = 1;       // seconds before the prepared end that a seek may still land
  const WINDOW_REUSE_AHEAD = 12;      // the backend reuses a running window this far past its end

  function windowOffset(win) {
    return win && isFinite(win.offset) && win.offset > 0 ? Number(win.offset) : 0;
  }
  function toOriginal(win, windowTime) {
    return windowOffset(win) + (isFinite(windowTime) && windowTime > 0 ? Number(windowTime) : 0);
  }
  function toWindow(win, originalTime) {
    return Math.max(0, (isFinite(originalTime) ? Number(originalTime) : 0) - windowOffset(win));
  }
  /** The end of the playable original-time region of a window (0 when nothing is known). */
  function windowLimit(win, duration) {
    if (!win) return 0;
    if (win.complete && duration > 0) return duration;
    return isFinite(win.until) && win.until > 0 ? Number(win.until) : windowOffset(win);
  }
  /**
   * How to seek to original time `time`: inside the prepared window it is a plain
   * seek in window time; anywhere else the backend has to prepare a window near it.
   */
  function seekPlan(win, time, duration) {
    let t = Number(time);
    if (!isFinite(t) || t < 0) t = 0;
    if (duration > 0 && t > duration) t = duration;
    if (!win) return { kind: 'window', original: t, time: t };
    const inside = t >= windowOffset(win) - 0.001 && (win.complete || t <= windowLimit(win, duration) - WINDOW_SEEK_MARGIN);
    return inside ? { kind: 'window', original: t, time: toWindow(win, t) } : { kind: 'prepare', original: t, target: t };
  }
  /** Can the window described by a local status serve `target` without another prepare? */
  function localFits(pb, target) {
    if (!pb || !pb.local || pb.offset_seconds == null) return true;       // status from an older backend
    const offset = Number(pb.offset_seconds) || 0, t = Number(target) || 0;
    if (t + 0.001 < offset) return false;
    if (pb.window_complete || pb.mode === 'remux') return true;
    return t <= (Number(pb.prepared_until) || offset) + WINDOW_REUSE_AHEAD;
  }
  /** May the player attach a local status now? Requires the window to cover the target. */
  function localReady(pb, target) {
    if (!pb || !pb.ready) return false;
    if (pb.offset_seconds == null) {                                       // older backend: prefix from 0
      return !(target > 0 && pb.progress < 1 && (pb.prepared_seconds || 0) < target + 1);
    }
    const offset = Number(pb.offset_seconds) || 0, t = Number(target) || 0;
    return t + 0.001 >= offset && (!!pb.window_complete || (Number(pb.prepared_until) || 0) >= t);
  }

  // Reading native ranges before metadata can change WebKit's interpretation
  // of a finite file. Guard the getter itself, not just the returned ranges.
  function mediaRanges(video, name) {
    if (video.readyState < 1) return [];
    const ranges = video[name], result = [];
    for (let i = 0; i < Math.min(ranges.length, 8); i++) result.push([ranges.start(i), ranges.end(i)]);
    return result;
  }

  /** How far into the video playback can go right now. */
  function seekLimit({ kind, seekableEnd, duration, progress }) {
    if (kind !== 'hls') return duration > 0 ? duration : Infinity;
    if (progress != null && progress >= 0.999 && duration > 0) return duration;
    return Math.max(0, seekableEnd || 0);
  }

  /** Clamp a requested seek to the duration and to what is downloaded. */
  function clampSeek(target, { duration, limit }) {
    let t = Number(target);
    if (!isFinite(t) || t < 0) t = 0;
    if (duration > 0 && t > duration) t = duration;
    if (limit != null && isFinite(limit) && t > limit) {
      return { time: Math.max(0, limit - SEEK_MARGIN), clamped: true, limit };
    }
    return { time: t, clamped: false, limit: limit == null ? null : limit };
  }

  function clampNoticeText(limit) { return `Cached up to ${fmtClock(limit)}, the rest is still downloading`; }

  /** Resume position: only when it is past the first 5 s and not within 10 s of the end. */
  function resumePosition(pos, duration) {
    if (!(pos > 5)) return 0;
    if (duration > 0 && pos >= duration - 10) return 0;
    return pos;
  }

  /** The caching label next to the player ("" when there is nothing to say). */
  function streamLabel(playback) {
    if (!playback) return '';
    if(playback.local)return playback.state==='cached'?'From local copy':playback.ready?'Playing compatible local copy':'Preparing compatible local playback…';
    if (playback.state === 'cached') return playback.kind === 'file' ? 'From cache' : 'Cached';
    if (playback.kind === 'hls') {
      if (playback.state === 'resolving') return 'Preparing stream...';
      if (playback.state === 'streaming') return playback.progress >= 0.999 ? 'Cached' : `Playing while caching ${pct(playback.progress)}%`;
    }
    return '';
  }

  /** The message over the video while there is nothing to play yet. */
  function overlayMessage(status, retrying) {
    if (!status) return 'Starting...';
    const p = status.playback || {};
    if(p.local&&!p.ready&&p.state!=='error')return 'Preparing compatible local playback…';
    if (p.state === 'error') return '';
    if (p.ready) return '';
    if (p.state === 'resolving') return 'Finding the video stream...';
    if (p.state === 'streaming') return 'Starting the stream...';
    return retrying ? 'Retrying...' : 'Starting...';
  }

  /** What the player says when a video cannot start. Technical detail never reaches the screen. */
  const FAILURE_TITLE = "Couldn't start this video";
  const FAILURES = {
    unavailable: { detail: 'This video is unavailable. It may have been removed or made private.', retryable: false },
    auth_restricted: { detail: 'This video needs a signed-in account, or it is private, members-only or restricted by YouTube.', retryable: false },
    geo_age: { detail: 'This video is age-restricted or not available in your region.', retryable: false },
    live_stream: { detail: "Live streams can't be played here. Open it on youtube.com instead.", retryable: false },
    format_unavailable: { detail: 'YouTube offers no playable format for this video.', retryable: false },
    transport_retryable: { detail: 'The connection to YouTube was interrupted.', retryable: true },
    length_probe_failed: { detail: "Muninn couldn't read the video stream from YouTube.", retryable: true },
    no_first_bytes_timeout: { detail: "The video didn't start downloading in time.", retryable: true },
    resolve_timeout: { detail: 'YouTube took too long to answer.', retryable: true },
    helper_exit: { detail: 'The video helper stopped unexpectedly.', retryable: true },
    disk_space: { detail: 'There is not enough free disk space. Free some space or lower the cache size in Settings.', retryable: true },
    busy: { detail: 'Muninn is already preparing several videos. Try again in a moment.', retryable: true },
    tools_pending: { detail: 'Muninn is still setting up its video tools. Try again in a moment.', retryable: true },
    tools_missing: { detail: "This build of Muninn can't play videos that are not cached yet.", retryable: false },
    offline: { detail: 'Offline mode is on. Turn it off in Settings to play videos that are not cached.', retryable: false },
    cancelled: { detail: 'Playback was stopped.', retryable: true },
    start_stalled: { detail: 'The video is taking too long to start.', retryable: true },
    media_error: { detail: 'The video was found but could not be played.', retryable: true },
    engine_unsupported: { detail: 'This browser cannot play the stream.', retryable: true },
    unknown: { detail: 'Something went wrong while starting the video.', retryable: true },
  };

  /** `{title, detail, reason, retryable}` for a failure category. `retryable` (from the service) wins over the table. */
  function failureView(reason, retryable) {
    const known = FAILURES[reason] ? reason : 'unknown';
    const f = FAILURES[known];
    return { title: FAILURE_TITLE, detail: f.detail, reason: known, retryable: typeof retryable === 'boolean' ? retryable : f.retryable };
  }

  /** The failure to show for a watch status whose playback state is `error`, else null. */
  function playbackFailure(status) {
    const p = status && status.playback;
    if (!p || p.state !== 'error') return null;
    // A status without a category (local ffmpeg playback reports only text) is classified from its error text.
    return failureView(p.reason || classifyFailureText(p.error, !!p.local), p.retryable);
  }

  /** Best-effort category for helper text that arrived without one. Anything unrecognised from a local (ffmpeg) job is a helper exit. */
  function classifyFailureText(text, local) {
    const t = String(text || '').toLowerCase();
    if (/no space left|not enough disk|disk full/.test(t)) return 'disk_space';
    if (/timed out|timeout|connection (reset|refused|closed|failed)|network is unreachable|\bdns\b/.test(t)) return 'transport_retryable';
    if (/invalid data|moov atom|unsupported|decoder|codec|no such file|does not contain any stream|corrupt/.test(t)) return 'media_error';
    return local ? 'helper_exit' : 'unknown';
  }

  /** Bounded text without URLs, e-mail addresses, paths or long tokens, safe to write to the log. */
  function redactDetail(text, max) {
    const limit = max || 300;
    return String(text == null ? '' : text)
      .replace(/[^\s"'<>()]*:\/\/[^\s"'<>()]*/g, '[url]')
      .replace(/[^\s"'<>()]+@[^\s"'<>()]+/g, '[email]')
      .replace(/(^|[\s"'(])(?:[A-Za-z]:\\|~?\/|\\\\)[^\n"'<>]*?(?:\.[A-Za-z0-9]{2,5}\b|$)/gm, '$1[path]')
      .replace(/\b[A-Za-z0-9_\-.%+]{24,}\b/g, '[token]')
      .replace(/\b(token|key|secret|password|authorization|cookie|sig|signature)(\s*[:=]\s*)\S+/gi, '$1$2[redacted]')
      .slice(0, limit);
  }

  /** Backstop against an endless spinner: nothing playable and no error after `limitMs` of waiting. */
  // Above the service's worst case for one start (StreamTuning::worst_case_start with defaults, ~1118 s:
  // two resolves of 120 s, probes and first-bytes wait per attempt, two attempts), so Retry never just
  // rejoins a job that is still working. The service also reports its own budget as `start_budget_ms`.
  const START_WATCHDOG_MS = 1200000;
  const START_BUDGET_GRACE_MS = 30000;
  function startStalled(beganAt, now, status, limitMs) {
    if (!status || !beganAt) return false;
    const p = status.playback || {};
    const limit = limitMs || (p.start_budget_ms > 0 ? p.start_budget_ms + START_BUDGET_GRACE_MS : START_WATCHDOG_MS);
    if (now - beganAt <= limit) return false;
    if (p.local || p.ready || p.state === 'cached' || p.state === 'error') return false;
    return true;
  }

  function playbackError(status) {
    const p = status && status.playback;
    if (!p || p.state !== 'error') return null;
    return p.error || p.phase || 'Playback failed';
  }

  /** Whether the watch poll can stop: metadata settled and playback finished or failed. */
  function watchSettled(status) {
    if (!status) return false;
    const p = status.playback || {};
    const metaDone = status.meta_state === 'ready' || status.meta_state === 'error';
    const playDone = p.state === 'cached' || p.state === 'error' || (p.kind === 'hls' && p.progress >= 0.999);
    return metaDone && playDone && !status.comments_loading;
  }

  function pollDelay(status) {
    const p = status && status.playback;
    if (!p || !p.ready) return 700;
    return 1500;
  }

  /* ------------------------------------------------------------------ item / comment view models */

  function cacheBadge(cached) {
    if (!cached) return null;
    const done = cached.state === 'complete' || cached.state === 'protected' || cached.state === 'speculative';
    const full = cached.progress >= 1 || cached.state === 'protected' || cached.state === 'complete';
    if (full || (done && cached.progress == null)) {
      return { label: qLabel(cached.variant), cls: cached.state === 'protected' ? 'protected' : 'complete', progress: 1 };
    }
    if (cached.state === 'speculative') return { label: qLabel(cached.variant), cls: 'speculative', progress: cached.progress == null ? 1 : cached.progress };
    return { label: qLabel(cached.variant) + ' ' + pct(cached.progress) + '%', cls: 'partial', progress: cached.progress || 0 };
  }

  function itemView(it) {
    return {
      videoId: it.video_id,
      title: it.title || it.video_id,
      channel: it.channel || '',
      duration: fmtDuration(it.duration_seconds),
      meta: [it.view_text, it.published_text].filter(Boolean).join(' · '),
      badge: it.badge || '',
      cache: cacheBadge(it.cached),
      resume: fraction(it.position_seconds, it.duration_seconds),
      thumb: it.thumb_url || ('/thumb/' + it.video_id),
    };
  }

  function commentView(c) {
    return {
      id: c.id || null,
      author: c.author || 'Anonymous',
      text: c.text || '',
      likes: c.likes ? fmtCount(c.likes) : '',
      time: c.published_text || '',
      isReply: !!c.is_reply,
      replies: c.reply_count ? c.reply_count : 0,
    };
  }

  /** "1.2M views" etc. from the meta of /api/video. */
  function viewsLine(meta) {
    if (!meta) return '';
    const parts = [];
    if (meta.view_count != null) parts.push(Number(meta.view_count).toLocaleString('en-US') + ' views');
    if (meta.published_text) parts.push(meta.published_text);
    return parts.join(' · ');
  }

  /** Summary of the cache entries of a video for the cache panel. */
  function entryView(e) {
    const active = e.status === 'queued' || e.status === 'downloading';
    let status = e.state;
    if (e.status === 'downloading') status = 'downloading ' + pct(e.progress) + '%';
    else if (e.status === 'queued') status = 'queued';
    else if (e.status === 'failed') status = 'failed';
    else if (e.status === 'canceled') status = 'canceled';
    return {
      id: e.id,
      label: e.actual_height && e.variant !== 'audio' ? `${e.actual_height}p` + (String(e.actual_height) !== e.variant ? ` (requested up to ${e.variant}p)` : '') : qLabel(e.variant),
      status, active,
      failed: e.status === 'failed',
      complete: e.status === 'complete',
      protected: !!e.protected,
      size: e.size_bytes ? fmtBytes(e.size_bytes) : '',
      progress: pct(e.progress),
    };
  }

  /** The entry "Save to library" should promote: a finished, unprotected one (best quality first). */
  function promotableEntry(entries) {
    const RANK = { '1080': 4, '720': 3, '480': 2, '360': 1, audio: 0 };
    const list = (entries || []).filter((e) => e.status === 'complete' && !e.protected);
    list.sort((a, b) => (RANK[b.variant] || 0) - (RANK[a.variant] || 0));
    return list[0] || null;
  }

  function inLibrary(entries) { return (entries || []).some((e) => e.protected); }

  /** Align the marker with the integer position/duration sent by the reporter. */
  function cleanupPoint(policy, duration, position) {
    if (!policy || policy.action !== 'delete' || !Number.isFinite(duration) || Math.round(duration) <= 0 ||
        !Number.isFinite(policy.completion_percent) || policy.completion_percent < 50 || policy.completion_percent > 100) return null;
    const seconds = Math.ceil(Math.round(duration) * policy.completion_percent / 100);
    return { seconds: Math.min(duration, seconds), fraction: Math.min(1, seconds / duration),
      reached: Number.isFinite(position) && Math.floor(position) >= seconds };
  }

  /** Pick the snapshot the Restore view shows for a tab, from GET /api/feeds?kind=...(newest first). */
  function pickSnapshot(snapshots, tab) {
    const list = asArray(snapshots, 'feeds');
    if (!list.length) return null;
    if (tab && tab.kind === 'search' && tab.query) {
      const want = String(tab.query).trim().toLowerCase();
      const hit = list.find((s) => s.query && String(s.query).trim().toLowerCase() === want);
      if (hit) return hit;
    }
    return list[0];
  }

  /* ------------------------------------------------------------------ status area */

  /**
   * What the persistent part of the status area says. `setup` is `setup` from GET /api/setup,
   * `trust` is the result of the trust_status command. Returns {kind, text, progress, action}.
   */
  function statusBanner(setup, trust) {
    if (trust && trust.state === 'needed') {
      return { kind: 'trust', text: 'Muninn needs to trust its local certificate to read YouTube pages', progress: null, action: 'Allow' };
    }
    if (trust && trust.state === 'installing') return { kind: 'setup', text: 'Installing the local certificate...', progress: null, action: null };
    if (trust && trust.state === 'failed') {
      return { kind: 'error', text: 'Could not trust the local certificate' + (trust.detail ? ': ' + trust.detail : ''), progress: null, action: 'Retry' };
    }
    const phase = setup && setup.phase;
    if (phase === 'finding') return { kind: 'setup', text: 'Finding video downloader…', progress: null, action: null };
    if (phase === 'installing' || phase === 'updating' || phase === 'starting') {
      const p = setup.progress == null ? null : Math.max(0, Math.min(1, Number(setup.progress)));
      return { kind: 'setup', text: 'Setting up tools: ' + (setup.message || phase), progress: p, action: null };
    }
    if (phase === 'error') return { kind: 'error', text: setup.message || 'Tool setup failed', progress: null, action: 'settings' };
    return null;
  }

  /* ------------------------------------------------------------------ layout */

  function roundRect(r) {
    const q = (v) => Math.round(v * 2) / 2;
    return { x: q(r.x), y: q(r.y), w: q(r.width != null ? r.width : r.w), h: q(r.height != null ? r.height : r.h) };
  }
  function sameRect(a, b) { return !!a && !!b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h; }

  /* ------------------------------------------------------------------ rules and settings forms */

  const TRIGGERS = [['open_tab', 'Open tab'], ['first_play', 'First play'], ['new_upload', 'New upload'], ['manual', 'Manual only']];
  const QUALITIES = [['none', 'none'], ['audio', 'audio'], ['360', '360p'], ['480', '480p'], ['720', '720p'], ['1080','1080p']];

  function minutesStr(sec) { return sec == null ? '' : String(Math.round(sec / 60 * 100) / 100); }
  function splitList(s) { return String(s || '').split(',').map((x) => x.trim()).filter(Boolean); }
  function parseNum(s) { s = String(s == null ? '' : s); if (s.trim() === '') return null; const n = Number(s); return isFinite(n) ? n : NaN; }

  function ruleToModel(r) {
    const m = r.match || {};
    const mdl = {
      _orig: r,
      id: r.id,
      name: r.name || '',
      enabled: r.enabled !== false,
      channels: (m.channels || []).join(', '),
      channels_only: !!m.channels_only,
      keywords: (m.title_keywords || []).join(', '),
      min: minutesStr(m.min_duration_seconds),
      max: minutesStr(m.max_duration_seconds),
      trigger: r.trigger || 'manual',
      quality: r.quality || '360',
      retention: String(r.retention_days == null ? 0 : r.retention_days),
    };
    mdl._init = { min: mdl.min, max: mdl.max };
    return mdl;
  }

  function newRuleModel() {
    return ruleToModel({ name: 'New rule', enabled: true, match: { channels: [], channels_only: false, title_keywords: [], min_duration_seconds: null, max_duration_seconds: null }, trigger: 'first_play', quality: '360', retention_days: 0 });
  }

  function rulesSnapshot(models) {
    return JSON.stringify(models.map((m) => [m.id, m.name, m.enabled, m.channels, m.channels_only, m.keywords, m.min, m.max, m.trigger, m.quality, m.retention]));
  }

  /** Validates the rule models and builds the PUT /api/rules payload. */
  function rulesValidate(models) {
    const errors = [];
    const payload = [];
    models.forEach((m, i) => {
      const e = (t) => errors.push({ row: i, text: `Rule ${i + 1}: ${t}` });
      if (!m.name.trim()) e('name is required.');
      const mn = parseNum(m.min), mx = parseNum(m.max);
      let minS = null, maxS = null;
      if (Number.isNaN(mn) || (mn != null && mn < 0)) e('min duration must be a number of minutes >= 0.');
      else if (mn != null) minS = m.min === m._init.min && m._orig.match ? m._orig.match.min_duration_seconds : Math.round(mn * 60);
      if (Number.isNaN(mx) || (mx != null && mx < 0)) e('max duration must be a number of minutes >= 0.');
      else if (mx != null) maxS = m.max === m._init.max && m._orig.match ? m._orig.match.max_duration_seconds : Math.round(mx * 60);
      if (minS != null && maxS != null && minS > maxS) e('min duration is larger than max duration.');
      const ret = Number(m.retention);
      if (String(m.retention).trim() === '' || !Number.isInteger(ret) || ret < 0) e('retention days must be a whole number >= 0.');
      // "only these channels" with an empty list is valid (it matches nothing); the stock rules ship that way.
      const orig = m._orig || {};
      const rule = Object.assign({}, orig, {
        name: m.name.trim(), enabled: m.enabled,
        match: Object.assign({}, orig.match || {}, { channels: splitList(m.channels), channels_only: m.channels_only, title_keywords: splitList(m.keywords), min_duration_seconds: minS, max_duration_seconds: maxS }),
        trigger: m.trigger, quality: m.quality, retention_days: ret,
      });
      if (m.id == null || m.id === '') delete rule.id;
      payload.push(rule);
    });
    return { errors, payload };
  }

  /** kind: bytes (shown as decimal GB), int, bool, select. */
  const SETTINGS_FIELDS = [
    { group: 'Network', key: 'offline_mode', label: 'Offline mode', kind: 'bool', hint: 'Browse cached content and play local copies. Downloads pause and remote pages sleep until you reconnect.' },
    { group: 'Favourites', key: 'favourite_downloads', label: 'Download uploads from favourite groups', kind: 'bool', hint: 'Default for favourite groups. Individual channel overrides still apply.' },
    { group: 'Favourites', key: 'favourite_carousel', label: 'Feature favourite uploads on Browse', kind: 'bool' },
    { group: 'Favourites', key: 'favourite_hide_started', label: 'Remove featured videos after playback starts', kind: 'bool' },
    { group: 'Favourites', key: 'favourite_carousel_days', label: 'Featured upload lifetime', unit: 'days', kind: 'int', min: 1, max: 30, hint: 'Measured from first appearance in the carousel. Does not change media retention.' },
    { group: 'Appearance', key: 'theme', label: 'Theme', kind: 'choice', options: [['system', 'Muninn · system light/dark'], ['classified', 'Classified Terminal · dark'], ['classified_light', 'Classified Terminal · light']], hint: 'The movement / esbenthomsen.com visual style is optional' },
    { group: 'Appearance', key: 'accent', label: 'Accent color', kind: 'choice', options: [['blue', 'Blue'], ['red', 'Red'], ['amber', 'Amber · original Movement'], ['teal', 'Teal']] },
    { group: 'Playback', key: 'pip_mode', label: 'Picture in picture', kind: 'choice', options: [['manual', 'Click to open'], ['automatic', 'Automatic when leaving the player'], ['disabled', 'Disabled']], hint: 'Automatic PiP depends on web-engine permission; navigation gestures give it the best chance' },
    { group: 'Playback', key: 'background_playback', label: 'Continue playback without PiP', kind: 'bool', hint: 'Keep audio playing while browsing, including when automatic PiP is unavailable' },
    { group: 'Playback', key: 'pip_return_to_inline', label: 'Return PiP to the player when its tab is opened', kind: 'bool' },
    { group: 'Playback', key: 'pause_when_pip_closes', label: 'Pause when the floating player is closed', kind: 'bool' },
    { group: 'Disk', key: 'max_disk_bytes', label: 'Video cache limit', unit: 'GiB', kind: 'bytes', hint: 'Evictable cache; the library is never evicted' },
    { group: 'Disk', key: 'min_free_bytes', label: 'Keep free on disk', unit: 'GiB', kind: 'bytes', hint: 'Speculative data is evicted below this' },
    { group: 'Downloads', key: 'download_concurrency', label: 'Download concurrency', unit: 'jobs', kind: 'int', min: 1, max: 8 },
    { group: 'Downloads', key: 'prefetch_bandwidth_kbps', label: 'Prefetch bandwidth', unit: 'kbps', kind: 'int', min: 0, hint: '0 = unlimited' },
    { group: 'Downloads', key: 'auto_prefetch', label: 'Auto-prefetch by rules', kind: 'bool', hint: 'Cache videos in the background when a rule says so' },
    { group: 'Watching', key: 'watch_height', label: 'Watch quality', kind: 'select', options: [[360, '360p'], [480, '480p'], [720, '720p'], [1080, '1080p']], hint: 'Quality for videos that are not cached yet' },
    { group: 'Browsing', key: 'warm_tabs', label: 'Warm tabs', kind: 'int', min: 1, max: 12, hint: 'Tabs that keep a live page; others sleep' },
    { group: 'Browsing', key: 'hibernate_after_secs', label: 'Hibernate after', unit: 'seconds', kind: 'int', min: 5, hint: 'Background time before a tab may sleep' },
    { group: 'Freshness', key: 'feed_refresh_minutes', label: 'Feed refresh', unit: 'minutes', kind: 'int', min: 5 },
    { group: 'Freshness', key: 'channel_list_ttl_hours', label: 'Channel video lists', unit: 'hours', kind: 'int', min: 1, max: 720, hint: 'Stale lists stay visible while refreshing. Cached pages remain available offline.' },
    { group: 'Freshness', key: 'comments_ttl_hours', label: 'Comments kept', unit: 'hours', kind: 'int', min: 1 },
    { group: 'Freshness', key: 'search_ttl_hours', label: 'Searches kept', unit: 'hours', kind: 'int', min: 1 },
    { group: 'Freshness', key: 'metadata_ttl_days', label: 'Video info kept', unit: 'days', kind: 'int', min: 1 },
    { group: 'Developer', key: 'developer_mode', label: 'Developer mode', kind: 'bool', hint: 'Shows detailed diagnostics, raw technical status and logs, and lets you export a support package. Muninn keeps its short, redacted activity log either way.' },
  ];

  /** Pages that only exist while developer mode is on. */
  const DEVELOPER_PAGES = ['diagnostics'];
  function developerMode(settings) { return !!(settings && settings.developer_mode === true); }
  function pageAllowed(page, settings) { return !DEVELOPER_PAGES.includes(page) || developerMode(settings); }
  /** The page to show: the requested one, or Settings when it is developer-only and the mode is off. */
  function resolvePage(page, settings) { return page && !pageAllowed(page, settings) ? 'settings' : page; }
  /** Sidebar pages in order; developer pages are absent unless the mode is on. */
  function sidebarPages(settings) {
    const pages = ['browse', 'player', 'library', 'feeds', 'rules'];
    pages.push('settings');
    return pages;
  }
  /** Text for the result of the export command (or the error it failed with). */
  function exportOutcome(result, error) {
    if (error != null) {
      const text = typeof error === 'string' ? error : (error && error.message) || 'unknown error';
      return { kind: 'error', text: 'Could not export diagnostics: ' + text };
    }
    if (result && result.status === 'saved' && result.path) return { kind: 'ok', text: 'Saved diagnostics to ' + result.path };
    if (result && result.status === 'cancelled') return { kind: 'info', text: '' };
    return { kind: 'error', text: 'Could not export diagnostics: unexpected response' };
  }

  const GIB = 1073741824;   // the core's GB constant is binary
  function gbStr(bytes) { return bytes == null ? '' : String(Math.round(bytes / GIB * 1000) / 1000); }

  /** Binary sizes (GiB/MiB) for the disk limits, which the core counts in 1024^3. */
  function fmtBytesBin(n) {
    if (n == null || isNaN(n)) return '-';
    const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    let v = Number(n), i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return v.toFixed(i === 0 ? 0 : (v >= 100 ? 0 : (v >= 10 ? 1 : 2))) + ' ' + u[i];
  }

  /** The string an input shows for a setting value. */
  function settingToInput(f, v) {
    if (f.kind === 'bool') return !!v;
    if (f.kind === 'bytes') return gbStr(v);
    return v == null ? '' : String(v);
  }

  /**
   * Builds the PUT /api/settings patch from the form values (same keys as the settings).
   * Untouched fields are omitted so a stale form never overwrites newer values.
   */
  function settingsPatch(fields, orig, values) {
    const patch = {};
    for (const f of fields) {
      const cur = values[f.key];
      if (cur === settingToInput(f, orig[f.key])) continue;
      if (f.kind === 'bool') { patch[f.key] = !!cur; continue; }
      if (f.kind === 'choice') {
        if (!f.options.some(([value]) => value === cur)) return { error: `${f.label}: choose a supported option.`, key: f.key };
        patch[f.key] = cur; continue;
      }
      const n = Number(cur);
      const min = f.min == null ? 0 : f.min;
      if (String(cur).trim() === '' || !isFinite(n) || n < min) return { error: `${f.label}: enter a number >= ${min}.`, key: f.key };
      if (f.max != null && n > f.max) return { error: `${f.label}: at most ${f.max}.`, key: f.key };
      if ((f.kind === 'int' || f.kind === 'select') && !Number.isInteger(n)) return { error: `${f.label}: must be a whole number.`, key: f.key };
      patch[f.key] = f.kind === 'bytes' ? Math.round(n * GIB) : n;
    }
    return { patch };
  }

  /* ------------------------------------------------------------------ library */

  function isActiveEntry(e) { return e.status === 'queued' || e.status === 'downloading'; }

  /** All search words must match; prefer titles over channels and identifiers. */
  function librarySearchScore(e, text) {
    const query = String(text || '').trim().toLowerCase();
    if (!query) return 0;
    const title = String(e.title || '').toLowerCase(), channel = String(e.channel || '').toLowerCase();
    const identifiers = String(e.video_id || '') + ' ' + String(e.id || '');
    let score = title === query ? 80 : title.includes(query) ? 40 : 0;
    for (const word of query.split(/\s+/)) {
      if (title.includes(word)) score += 8;
      else if (channel.includes(word)) score += 4;
      else if (identifiers.toLowerCase().includes(word)) score += 1;
      else return -1;
    }
    return score;
  }

  /** Browsing shows one video, with a playable/kept copy preferred over a link. */
  function libraryVideos(entries) {
    const videos = new Map();
    const priority = e => e.status === 'complete' ? (e.protected ? 5 : 4) : isActiveEntry(e) ? 3 : e.state === 'link' ? 1 : 2;
    for (const entry of entries) {
      const key = entry.video_id || entry.id, previous = videos.get(key);
      const better = !previous || priority(entry) > priority(previous) ||
        (priority(entry) === priority(previous) && (entry.actual_height || 0) > (previous.actual_height || 0));
      videos.set(key, { ...(better ? entry : previous), copy_count: (previous?.copy_count || 0) + 1 });
    }
    return [...videos.values()];
  }

  function entryMatches(e, filter, text) {
    if (filter === 'active' && !isActiveEntry(e)) return false;
    if (filter === 'failed' && e.status !== 'failed') return false;
    if (['speculative', 'partial', 'complete', 'protected', 'link'].includes(filter) && e.state !== filter) return false;
    if (librarySearchScore(e, text) < 0) return false;
    return true;
  }

  function libraryCounts(entries) {
    const c = { all: 0, speculative: 0, partial: 0, complete: 0, protected: 0, active: 0, failed: 0, link: 0 };
    for (const e of entries) {
      c.all++;
      if (c[e.state] != null) c[e.state]++;
      if (isActiveEntry(e)) c.active++;
      if (e.status === 'failed') c.failed++;
    }
    return c;
  }

  /** The three disk meters of the Library page from GET /api/disk. */
  function diskMeters(d) {
    const cache = d.cache_bytes || 0, lib = d.library_bytes || 0, cap = d.max_disk_bytes || 0;
    const free = d.free_bytes, minFree = d.min_free_bytes || 0;
    const low = free != null && minFree > 0 && free < minFree;
    return [
      { key: 'cache', label: 'Video cache', fraction: cap > 0 ? Math.min(1, cache / cap) : 0, text: fmtBytesBin(cache) + (cap ? ' of ' + fmtBytesBin(cap) : ''), warn: cap > 0 && cache > cap * 0.95, hint: 'Evictable. The limit applies to this cache only.' },
      { key: 'library', label: 'Library', fraction: (cache + lib) > 0 ? lib / (cache + lib) : 0, text: fmtBytesBin(lib) + ' kept', warn: false, hint: 'Saved videos are never evicted automatically.' },
      { key: 'free', label: 'Free on disk', fraction: free != null && (free + cache + lib) > 0 ? Math.min(1, free / (free + cache + lib)) : 0, text: free == null ? '-' : fmtBytesBin(free) + (minFree ? ' (keep >= ' + fmtBytesBin(minFree) + ')' : ''), warn: low, hint: 'Speculative caching stops below the minimum.' },
    ];
  }

  /** Safe user-facing label from the live /api/channels/status account observation. */
  function observedAccountState(account) {
    const state = account && account.state;
    if (state === 'signed_in') return { state, label: 'Signed in to YouTube', canSignIn: false };
    if (state === 'signed_out') return { state, label: 'Signed out of YouTube', canSignIn: true };
    return { state: 'not_observed', label: 'YouTube sign-in has not been observed yet', canSignIn: true };
  }

  /** Keep independent readiness dimensions explicit; helper presence is not playback health. */
  function settingsReadiness(tools, trust, playback) {
    const t = tools || {}, y = t.ytdlp || {}, f = t.ffmpeg || {};
    return {
      downloader: y.installed ? 'available' : 'unavailable',
      ffmpeg: f.found ? 'available' : 'unavailable',
      trust: !trust ? 'unavailable' : (trust.state || 'unknown'),
      playback: playback && playback.state ? playback.state : 'not_observed',
    };
  }

  return {
    observedAccountState, settingsReadiness,
    fmtClock, fmtDuration, fmtBytes, fmtBytesBin, fmtCount, relTime, pct, qLabel, fraction, asArray,
    chooseView, tabTitle, addressText, channelUrl, extractVideoId, canHibernate, shortcutAction,
    SEEK_STEP, SEEK_MARGIN, SEEK_DEBOUNCE_MS, MAX_LOCAL_RESTARTS, seekDue, mayRestartLocal, frameSettled, showSpinner, stopBarrier, windowOffset, toOriginal, toWindow, windowLimit, seekPlan, localFits, localReady, isTypingTarget, playerKeyAction, mediaUrl, withToken, effectiveDuration, mediaRanges, seekLimit, clampSeek, clampNoticeText,
    resumePosition, streamLabel, overlayMessage, playbackError, failureView, playbackFailure, classifyFailureText, redactDetail, startStalled, START_WATCHDOG_MS, watchSettled, pollDelay,
    cacheBadge, itemView, commentView, viewsLine, entryView, promotableEntry, inLibrary, cleanupPoint, pickSnapshot,
    statusBanner, roundRect, sameRect,
    TRIGGERS, QUALITIES, splitList, ruleToModel, newRuleModel, rulesSnapshot, rulesValidate,
    SETTINGS_FIELDS, developerMode, pageAllowed, resolvePage, sidebarPages, exportOutcome, settingToInput, settingsPatch, gbStr,
    isActiveEntry, entryMatches, librarySearchScore, libraryVideos, libraryCounts, diskMeters,
  };
});
