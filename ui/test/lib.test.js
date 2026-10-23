'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const L = require('../lib.js');

test('playback diagnostics do not query native ranges before metadata', () => {
  const video = { readyState: 0 };
  for (const name of ['buffered', 'seekable']) {
    Object.defineProperty(video, name, { configurable: true, get() { throw Error('native range queried too early'); } });
    assert.deepEqual(L.mediaRanges(video, name), []);
    Object.defineProperty(video, name, { value: { length: 1, start: () => 0, end: () => 90 } });
  }
  video.readyState = 1;
  assert.deepEqual(L.mediaRanges(video, 'seekable'), [[0, 90]]);
});

test('a stop waiter drains later stops before permitting the same entry to reopen', async () => {
  const resolvers = [];
  const stops = L.stopBarrier(() => new Promise(resolve => resolvers.push(resolve)));
  const tick = () => new Promise(resolve => setImmediate(resolve));
  stops.queue('video.720');
  await tick();
  let finished = false;
  const waiter = stops.wait('video.720').then(() => { finished = true; });
  stops.queue('video.720');
  resolvers[0]();
  await tick();
  assert.equal(resolvers.length, 2);
  assert.equal(finished, false);
  const secondWaiter = stops.waitVideo('video');
  await tick();
  assert.equal(finished, false);
  resolvers[1]();
  await Promise.all([waiter, secondWaiter]);
  assert.equal(finished, true);
});

test('a failed stop is retried, and a second failure blocks reopening', async () => {
  let calls = 0;
  const stops = L.stopBarrier(() => { if (++calls === 1) return Promise.reject(new Error('temporary')); return Promise.resolve(); });
  stops.queue('video.720').catch(() => {});
  await stops.waitVideo('video');
  assert.equal(calls, 2);
  const broken = L.stopBarrier(() => Promise.reject(new Error('still stopping')));
  broken.queue('video.720').catch(() => {});
  await assert.rejects(broken.waitVideo('video'), /still stopping/);
});

test('library search matches words across title/channel and ranks title relevance', () => {
  const e = { id: 'copy', video_id: 'abcdefghijk', title: 'Rust async explained', channel: 'Code Studio' };
  assert.equal(L.entryMatches(e, 'all', 'STUDIO rust'), true);
  assert.equal(L.entryMatches(e, 'all', 'rust piano'), false);
  assert.equal(L.entryMatches(e, 'all', 'abcdefghijk'), true);
  assert.ok(L.librarySearchScore(e, 'rust') > L.librarySearchScore({ title: 'Other', channel: 'Rust' }, 'rust'));
  assert.equal(L.librarySearchScore(e, '   '), 0);
});

test('library browsing groups copies without losing the management entries', () => {
  const entries = [
    { id: 'link:v', video_id: 'v', state: 'link' },
    { id: 'v.720', video_id: 'v', status: 'complete', actual_height: 720 },
    { id: 'v.audio', video_id: 'v', status: 'complete', protected: true },
    { id: 'w.360', video_id: 'w', status: 'complete', actual_height: 360 },
    { id: 'w.720', video_id: 'w', status: 'complete', actual_height: 720 },
  ];
  const videos = L.libraryVideos(entries);
  assert.equal(videos.length, 2);
  assert.equal(videos[0].id, 'v.audio');
  assert.equal(videos[0].copy_count, 3);
  assert.equal(videos[1].id, 'w.720');
  assert.equal(videos[1].copy_count, 2);
  assert.equal(entries.length, 5);
  assert.equal(entries[0].copy_count, undefined);
});

test('fmtClock formats m:ss and h:mm:ss and survives garbage', () => {
  assert.equal(L.fmtClock(0), '0:00');
  assert.equal(L.fmtClock(65.9), '1:05');
  assert.equal(L.fmtClock(3600), '1:00:00');
  assert.equal(L.fmtClock(3725), '1:02:05');
  for (const bad of [NaN, null, undefined, -5, Infinity]) assert.equal(L.fmtClock(bad), '0:00');
  assert.equal(L.fmtDuration(null), '');
  assert.equal(L.fmtDuration(0), '');
  assert.equal(L.fmtDuration(754), '12:34');
});

test('cleanup point follows the configured percentage and playback-report rounding', () => {
  const policy = { action: 'delete', completion_percent: 95 };
  assert.deepEqual(L.cleanupPoint(policy, 600, 569.9), { seconds: 570, fraction: .95, reached: false });
  assert.equal(L.cleanupPoint(policy, 600, 570).reached, true);
  assert.equal(L.cleanupPoint({ ...policy, completion_percent: 80 }, 600, 480).seconds, 480);
  // Backend receives floor(position) / round(duration), not the displayed fraction.
  assert.equal(L.cleanupPoint(policy, 359.46, 341.9).reached, false);
  assert.equal(L.cleanupPoint(policy, 359.46, 342).reached, true);
  assert.equal(L.cleanupPoint({ ...policy, completion_percent: 100 }, 600, 600).reached, true);
});

test('no cleanup marker for disabled/keep policies or an unknown timeline', () => {
  for (const action of [null, 'keep']) assert.equal(L.cleanupPoint({ action, completion_percent: 95 }, 600, 580), null);
  for (const duration of [0, .1, NaN, Infinity, null]) assert.equal(L.cleanupPoint({ action: 'delete', completion_percent: 95 }, duration, 580), null);
  assert.equal(L.cleanupPoint(null, 600, 580), null);
  assert.equal(L.cleanupPoint({ action: 'delete', completion_percent: 101 }, 600, 580), null);
});

test('byte, count and relative time formatting', () => {
  assert.equal(L.fmtBytes(0), '0 B');
  assert.equal(L.fmtBytes(1500), '1.50 KB');
  assert.equal(L.fmtBytes(20e9), '20.0 GB');
  assert.equal(L.fmtBytes(null), '-');
  assert.equal(L.fmtBytesBin(1073741824 * 20), '20.0 GiB');
  assert.equal(L.fmtCount(999), '999');
  assert.equal(L.fmtCount(1204), '1.2K');
  assert.equal(L.fmtCount(15300), '15K');
  assert.equal(L.fmtCount(2500000), '2.5M');
  assert.equal(L.relTime(1000, 1000 + 10e3), 'just now');
  assert.equal(L.relTime(0, 3 * 3600e3), '3 h ago');
  assert.equal(L.relTime(3 * 3600e3, 0), 'in 3 h');
  assert.equal(L.pct(0.426), 43);
  assert.equal(L.pct(7), 100);
  assert.equal(L.qLabel('audio'), 'audio');
  assert.equal(L.qLabel('480'), '480p');
});

test('chooseView implements the content area rule', () => {
  const yt = { face: 'yt', live: true };
  assert.equal(L.chooseView(null, null), 'empty');
  assert.equal(L.chooseView(null, yt), 'native');
  assert.equal(L.chooseView(null, { face: 'yt', live: false }), 'restore');
  assert.equal(L.chooseView(null, { face: 'watch', video_id: 'aaaaaaaaaaa', live: false }), 'watch');
  assert.equal(L.chooseView(null, { face: 'watch', video_id: 'aaaaaaaaaaa', live: true }), 'watch');
  assert.equal(L.chooseView('library', yt), 'internal');
  assert.equal(L.chooseView('settings', null), 'internal');
  // A watch tab without a video id is not a watch view.
  assert.equal(L.chooseView(null, { face: 'watch', video_id: null, live: false }), 'restore');
});

test('address text and tab titles', () => {
  assert.equal(L.addressText(null), '');
  assert.equal(L.addressText({ face: 'watch', video_id: 'abcdefghijk', url: 'x' }), 'https://www.youtube.com/watch?v=abcdefghijk');
  assert.equal(L.addressText({ face: 'yt', url: 'https://www.youtube.com/' }), 'https://www.youtube.com/');
  assert.equal(L.tabTitle({ title: '  Home  ' }), 'Home');
  assert.equal(L.tabTitle({ face: 'yt', url: 'https://www.youtube.com/feed' }), 'youtube.com');
  assert.equal(L.tabTitle({ face: 'watch' }), 'Video');
  assert.equal(L.tabTitle({}), 'New tab');
  assert.equal(L.channelUrl('UC123'), 'https://www.youtube.com/channel/UC123');
});

test('extractVideoId', () => {
  assert.equal(L.extractVideoId('https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=3'), 'dQw4w9WgXcQ');
  assert.equal(L.extractVideoId('https://youtu.be/dQw4w9WgXcQ'), 'dQw4w9WgXcQ');
  assert.equal(L.extractVideoId('https://www.youtube.com/shorts/dQw4w9WgXcQ'), 'dQw4w9WgXcQ');
  assert.equal(L.extractVideoId('lofi hip hop'), '');
});

test('canHibernate only for live YouTube pages', () => {
  assert.equal(L.canHibernate({ face: 'yt', state: 'live', live: true }), true);
  assert.equal(L.canHibernate({ face: 'yt', state: 'sleeping', live: false }), false);
  assert.equal(L.canHibernate({ face: 'yt', state: 'restoring', live: false }), false);
  assert.equal(L.canHibernate({ face: 'watch', state: 'live', live: true }), false);
  assert.equal(L.canHibernate(null), false);
});

test('window shortcuts', () => {
  const k = (key, o) => L.shortcutAction(Object.assign({ key }, o));
  assert.equal(k('t', { metaKey: true }), 'new-tab');
  assert.equal(k('T', { ctrlKey: true }), 'new-tab');
  assert.equal(k('w', { metaKey: true }), 'close-tab');
  assert.equal(k('l', { ctrlKey: true }), 'focus-address');
  assert.equal(k('r', { metaKey: true }), 'reload');
  assert.equal(k('[', { metaKey: true }), 'back');
  assert.equal(k(']', { metaKey: true }), 'forward');
  assert.equal(k('[', { ctrlKey: true }), null);
  assert.equal(k('t', {}), null);
  assert.equal(k('t', { metaKey: true, altKey: true }), null);
  assert.equal(k('t', { metaKey: true, shiftKey: true }), null);
});

test('player keys: space, arrows, f, m, and never while typing', () => {
  const div = { tag: 'DIV' };
  assert.equal(L.playerKeyAction({ key: ' ' }, div), 'toggle');
  assert.deepEqual(L.playerKeyAction({ key: 'ArrowLeft' }, div), { seek: -5 });
  assert.deepEqual(L.playerKeyAction({ key: 'ArrowRight' }, div), { seek: 5 });
  assert.equal(L.playerKeyAction({ key: 'f' }, div), 'fullscreen');
  assert.equal(L.playerKeyAction({ key: 'M' }, div), 'mute');
  assert.equal(L.playerKeyAction({ key: 'x' }, div), null);
  assert.equal(L.playerKeyAction({ key: ' ' }, { tag: 'INPUT', type: 'text' }), null);
  assert.equal(L.playerKeyAction({ key: 'm' }, { tag: 'INPUT', type: 'search' }), null);
  assert.equal(L.playerKeyAction({ key: 'f' }, { tag: 'SELECT' }), null);
  assert.equal(L.playerKeyAction({ key: 'f' }, { tag: 'DIV', editable: true }), null);
  // the seek bar is a range input: arrows must still work there
  assert.deepEqual(L.playerKeyAction({ key: 'ArrowRight' }, { tag: 'INPUT', type: 'range' }), { seek: 5 });
  // a focused button handles space itself
  assert.equal(L.playerKeyAction({ key: ' ' }, { tag: 'BUTTON' }), null);
  assert.equal(L.playerKeyAction({ key: 'f', metaKey: true }, div), null);
});

test('mediaUrl: files carry the token, HLS already has it in the path', () => {
  const o = 'http://127.0.0.1:47871';
  assert.equal(L.mediaUrl({ ready: true, kind: 'file', url_path: '/media/abc.360' }, o, 'tok en'), o + '/media/abc.360?token=tok%20en');
  assert.equal(L.mediaUrl({ ready: true, kind: 'hls', url_path: '/h/TOK/abc.360/index.m3u8' }, o, 'TOK'), o + '/h/TOK/abc.360/index.m3u8');
  assert.equal(L.mediaUrl({ ready: false, kind: 'hls', url_path: '/h/x' }, o, 't'), null);
  assert.equal(L.mediaUrl(null, o, 't'), null);
  assert.equal(L.withToken('/thumb/a', 't'), '/thumb/a?token=t');
  assert.equal(L.withToken('/thumb/a?x=1', 't'), '/thumb/a?x=1&token=t');
});

test('effectiveDuration prefers the server over the growing stream', () => {
  assert.equal(L.effectiveDuration({ duration_seconds: 600 }, { duration_seconds: 590 }, 12), 600);
  assert.equal(L.effectiveDuration({}, { duration_seconds: 590 }, 12), 590);
  assert.equal(L.effectiveDuration(null, null, 12), 12);
  assert.equal(L.effectiveDuration(null, null, Infinity), 0);
  assert.equal(L.effectiveDuration(null, null, NaN), 0);
});

test('seek limit and clamping while the stream is still caching', () => {
  const hls = { kind: 'hls', seekableEnd: 100, duration: 600, progress: 0.17 };
  assert.equal(L.seekLimit(hls), 100);
  assert.equal(L.seekLimit({ kind: 'file', seekableEnd: 0, duration: 600 }), 600);
  assert.equal(L.seekLimit(Object.assign({}, hls, { progress: 1 })), 600);
  assert.equal(L.seekLimit({ kind: 'hls', seekableEnd: 0, duration: 600, progress: 0 }), 0);
  // within what is downloaded
  assert.deepEqual(L.clampSeek(50, { duration: 600, limit: 100 }), { time: 50, clamped: false, limit: 100 });
  // beyond: clamp just before the end of what exists
  const r = L.clampSeek(400, { duration: 600, limit: 100 });
  assert.equal(r.clamped, true);
  assert.equal(r.time, 99);
  assert.equal(r.limit, 100);
  // beyond the duration for a file: clamped to the duration, no "still downloading" notice
  assert.deepEqual(L.clampSeek(900, { duration: 600, limit: 600 }), { time: 600, clamped: false, limit: 600 });
  assert.equal(L.clampSeek(-3, { duration: 600, limit: Infinity }).time, 0);
  assert.equal(L.clampSeek(NaN, { duration: 600, limit: 10 }).time, 0);
  assert.equal(L.clampNoticeText(192), 'Cached up to 3:12, the rest is still downloading');
});

test('resume position rule: > 5 s and not within 10 s of the end', () => {
  assert.equal(L.resumePosition(5, 600), 0);
  assert.equal(L.resumePosition(5.5, 600), 5.5);
  assert.equal(L.resumePosition(300, 600), 300);
  assert.equal(L.resumePosition(595, 600), 0);
  assert.equal(L.resumePosition(589, 600), 589);
  assert.equal(L.resumePosition(300, 0), 300);
  assert.equal(L.resumePosition(null, 600), 0);
});

test('stream label and overlay messages', () => {
  assert.equal(L.streamLabel({ state: 'streaming', kind: 'hls', progress: 0.426 }), 'Playing while caching 43%');
  assert.equal(L.streamLabel({ state: 'streaming', kind: 'hls', progress: 1 }), 'Cached');
  assert.equal(L.streamLabel({ state: 'cached', kind: 'file' }), 'From cache');
  assert.equal(L.streamLabel({ state: 'resolving', kind: 'hls' }), 'Preparing stream...');
  assert.equal(L.streamLabel({ state: 'none' }), '');
  assert.equal(L.streamLabel(null), '');
  assert.equal(L.overlayMessage({ playback: { state: 'resolving' } }), 'Finding the video stream...');
  assert.equal(L.overlayMessage({ playback: { state: 'streaming', ready: false } }), 'Starting the stream...');
  assert.equal(L.overlayMessage({ playback: { state: 'streaming', ready: true } }), '');
});

test('playbackError and watchSettled', () => {
  assert.equal(L.playbackError({ playback: { state: 'error', error: 'boom' } }), 'boom');
  assert.equal(L.playbackError({ playback: { state: 'error' } }), 'Playback failed');
  assert.equal(L.playbackError({ playback: { state: 'streaming' } }), null);
  assert.equal(L.watchSettled({ meta_state: 'loading', playback: { state: 'cached' } }), false);
  assert.equal(L.watchSettled({ meta_state: 'ready', playback: { state: 'streaming', kind: 'hls', progress: 0.5 } }), false);
  assert.equal(L.watchSettled({ meta_state: 'ready', playback: { state: 'cached' } }), true);
  assert.equal(L.watchSettled({ meta_state: 'error', playback: { state: 'error' } }), true);
  assert.equal(L.pollDelay({ playback: { ready: false } }), 700);
  assert.equal(L.pollDelay({ playback: { ready: true } }), 1500);
});

test('item, comment, views and cache view models', () => {
  const v = L.itemView({ video_id: 'a', title: 'T', channel: 'C', duration_seconds: 125, view_text: '1K views', published_text: '2 d ago', position_seconds: 50,
    cached: { state: 'partial', variant: '480', progress: 0.5 } });
  assert.equal(v.duration, '2:05');
  assert.equal(v.meta, '1K views · 2 d ago');
  assert.equal(v.resume, 50 / 125);
  assert.deepEqual(v.cache, { label: '480p 50%', cls: 'partial', progress: 0.5 });
  assert.equal(v.thumb, '/thumb/a');
  assert.equal(L.itemView({ video_id: 'a' }).title, 'a');
  assert.equal(L.cacheBadge(null), null);
  assert.equal(L.cacheBadge({ state: 'protected', variant: '720', progress: 1 }).cls, 'protected');
  assert.equal(L.cacheBadge({ state: 'complete', variant: 'audio', progress: 1 }).label, 'audio');
  assert.equal(L.cacheBadge({ state: 'speculative', variant: '360', progress: 1 }).cls, 'complete');
  const c = L.commentView({ author: '', text: 'hi', likes: 1500, is_reply: true, reply_count: 2 });
  assert.deepEqual(c, { id: null, author: 'Anonymous', text: 'hi', likes: '1.5K', time: '', isReply: true, replies: 2 });
  assert.equal(L.viewsLine({ view_count: 1234567, published_text: '3 days ago' }), '1,234,567 views · 3 days ago');
  assert.equal(L.viewsLine(null), '');
});

test('entry view and the entry "Save to library" promotes', () => {
  const e = L.entryView({ id: 'a.480', variant: '480', actual_height: 478, status: 'downloading', progress: 0.3, state: 'partial', size_bytes: 1000 });
  assert.equal(e.label, '478p (requested up to 480p)');
  assert.equal(e.status, 'downloading 30%');
  assert.equal(e.active, true);
  const list = [
    { id: 'a.360', variant: '360', status: 'complete', protected: false },
    { id: 'a.720', variant: '720', status: 'complete', protected: false },
    { id: 'a.480', variant: '480', status: 'downloading', protected: false },
    { id: 'a.audio', variant: 'audio', status: 'complete', protected: true },
  ];
  assert.equal(L.promotableEntry(list).id, 'a.720');
  assert.equal(L.promotableEntry([list[2]]), null);
  assert.equal(L.inLibrary(list), true);
  assert.equal(L.inLibrary(list.slice(0, 3)), false);
});

test('cached playback waits for background comments before stopping its poll', () => {
  const status = { meta_state: 'ready', playback: { state: 'cached' }, comments_loading: true };
  assert.equal(L.watchSettled(status), false);
  status.comments_loading = false;
  assert.equal(L.watchSettled(status), true);
});

test('Restore view picks the snapshot whose query matches the search tab', () => {
  const snaps = [{ id: 3, query: 'cats' }, { id: 2, query: 'Lofi Hip Hop ' }, { id: 1, query: null }];
  assert.equal(L.pickSnapshot(snaps, { kind: 'search', query: 'lofi hip hop' }).id, 2);
  assert.equal(L.pickSnapshot(snaps, { kind: 'search', query: 'unknown' }).id, 3);
  assert.equal(L.pickSnapshot(snaps, { kind: 'home' }).id, 3);
  assert.equal(L.pickSnapshot([], { kind: 'search', query: 'x' }), null);
});

test('status banner: trust first, then setup progress, then errors', () => {
  assert.equal(L.statusBanner(null, null), null);
  assert.equal(L.statusBanner({ phase: 'ready' }, { state: 'ok' }), null);
  const t = L.statusBanner({ phase: 'installing', message: 'x' }, { state: 'needed' });
  assert.equal(t.kind, 'trust');
  assert.equal(t.action, 'Allow');
  assert.match(t.text, /trust its local certificate/);
  const s = L.statusBanner({ phase: 'installing', message: 'Downloading yt-dlp', progress: 0.4 }, { state: 'ok' });
  assert.deepEqual(s, { kind: 'setup', text: 'Setting up tools: Downloading yt-dlp', progress: 0.4, action: null });
  assert.equal(L.statusBanner({ phase: 'starting', message: 'Checking tools' }, null).progress, null);
  assert.equal(L.statusBanner({ phase: 'error', message: 'No network' }, null).kind, 'error');
  assert.equal(L.statusBanner(null, { state: 'failed', detail: 'denied' }).text, 'Could not trust the local certificate: denied');
  assert.equal(L.statusBanner(null, { state: 'installing' }).action, null);
});

test('layout rect rounding and comparison', () => {
  const r = L.roundRect({ x: 176.26, y: 85.74, width: 800.4, height: 600.9 });
  assert.deepEqual(r, { x: 176.5, y: 85.5, w: 800.5, h: 601 });
  assert.equal(L.sameRect(r, { x: 176.5, y: 85.5, w: 800.5, h: 601 }), true);
  assert.equal(L.sameRect(r, null), false);
  assert.equal(L.sameRect(r, Object.assign({}, r, { w: 700 })), false);
});

test('rules: model round trip keeps untouched durations exact and validates', () => {
  const orig = { id: 'r1', name: 'Docs', enabled: true, match: { channels: ['A'], channels_only: true, title_keywords: ['x'], min_duration_seconds: 125, max_duration_seconds: null }, trigger: 'first_play', quality: '480', retention_days: 7 };
  const m = L.ruleToModel(orig);
  assert.equal(m.min, '2.08');
  let { errors, payload } = L.rulesValidate([m]);
  assert.equal(errors.length, 0);
  assert.equal(payload[0].match.min_duration_seconds, 125, 'unchanged minutes keep the original seconds');
  assert.equal(payload[0].id, 'r1');
  m.min = '3';
  assert.equal(L.rulesValidate([m]).payload[0].match.min_duration_seconds, 180);
  m.max = '1';
  assert.match(L.rulesValidate([m]).errors[0].text, /min duration is larger/);
  const n = L.newRuleModel();
  n.name = '  ';
  n.retention = '-1';
  const v = L.rulesValidate([n]);
  assert.equal(v.errors.length, 2);
  n.name = 'ok'; n.retention = '4';
  assert.equal('id' in L.rulesValidate([n]).payload[0], false);
  const before = L.rulesSnapshot([m]);
  m.name = 'renamed';
  assert.notEqual(L.rulesSnapshot([m]), before);
  m.channels = ''; m.min = '3'; m.max = '';
  assert.equal(L.rulesValidate([m]).errors.length, 0, 'an empty channel list with only-listed is valid (matches nothing)');
});

test('settings patch only carries what changed and converts GB', () => {
  const F = L.SETTINGS_FIELDS;
  const orig = { max_disk_bytes: 20 * 1073741824, min_free_bytes: 10 * 1073741824, download_concurrency: 2, prefetch_bandwidth_kbps: 0, auto_prefetch: true, watch_height: 720, warm_tabs: 3, hibernate_after_secs: 120, feed_refresh_minutes: 45, comments_ttl_hours: 24, search_ttl_hours: 2, metadata_ttl_days: 21 };
  const values = {};
  for (const f of F) values[f.key] = L.settingToInput(f, orig[f.key]);
  assert.equal(values.max_disk_bytes, '20');
  assert.equal(values.watch_height, '720');
  assert.equal(values.auto_prefetch, true);
  assert.deepEqual(L.settingsPatch(F, orig, values), { patch: {} });
  values.max_disk_bytes = '35.5'; values.warm_tabs = '5'; values.auto_prefetch = false; values.watch_height = '1080';
  assert.deepEqual(L.settingsPatch(F, orig, values), { patch: { max_disk_bytes: Math.round(35.5 * 1073741824), warm_tabs: 5, auto_prefetch: false, watch_height: 1080 } });
  values.warm_tabs = '99';
  assert.match(L.settingsPatch(F, orig, values).error, /at most 12/);
  values.warm_tabs = '2.5';
  assert.match(L.settingsPatch(F, orig, values).error, /whole number/);
  values.warm_tabs = '';
  assert.equal(L.settingsPatch(F, orig, values).key, 'warm_tabs');
  values.warm_tabs = '3'; values.hibernate_after_secs = '1';
  assert.match(L.settingsPatch(F, orig, values).error, />= 5/);
  assert.deepEqual(F.filter((f) => ['watch_height', 'warm_tabs', 'hibernate_after_secs', 'auto_prefetch'].includes(f.key)).length, 4);
});

test('library helpers: filter, counts and disk meters', () => {
  const es = [
    { id: 'a.360', title: 'Rust talk', channel: 'Labs', state: 'complete', status: 'complete' },
    { id: 'b.480', title: 'Piano', channel: 'Hours', state: 'partial', status: 'downloading' },
    { id: 'c.360', title: 'X', state: 'speculative', status: 'failed' },
  ];
  assert.equal(L.entryMatches(es[0], 'all', 'rust'), true);
  assert.equal(L.entryMatches(es[0], 'all', 'piano'), false);
  assert.equal(L.entryMatches(es[1], 'active', ''), true);
  assert.equal(L.entryMatches(es[2], 'failed', ''), true);
  assert.equal(L.entryMatches(es[2], 'complete', ''), false);
  assert.deepEqual(L.libraryCounts(es), { all: 3, speculative: 1, partial: 1, complete: 1, protected: 0, active: 1, failed: 1, link: 0 });
  const G = 1073741824;
  const m = L.diskMeters({ cache_bytes: 10 * G, library_bytes: 5 * G, max_disk_bytes: 20 * G, free_bytes: 5 * G, min_free_bytes: 10 * G });
  assert.equal(m[0].text, '10.0 GiB of 20.0 GiB');
  assert.equal(m[0].fraction, 0.5);
  assert.equal(m[0].warn, false);
  assert.equal(m[2].warn, true, 'free space below the minimum is flagged');
  assert.equal(L.diskMeters({ cache_bytes: 0, library_bytes: 0, max_disk_bytes: 0, free_bytes: null })[2].text, '-');
});

test('failure view: one title, a reason sentence, and retry only where it can help', () => {
  const dead = ['unavailable', 'auth_restricted', 'geo_age', 'live_stream', 'format_unavailable', 'tools_missing', 'offline'];
  for (const reason of dead) {
    const v = L.failureView(reason);
    assert.equal(v.title, "Couldn't start this video");
    assert.equal(v.retryable, false, reason);
    assert.ok(v.detail.length > 10);
  }
  for (const reason of ['transport_retryable', 'length_probe_failed', 'no_first_bytes_timeout', 'helper_exit', 'disk_space', 'busy', 'cancelled', 'start_stalled', 'unknown']) {
    assert.equal(L.failureView(reason).retryable, true, reason);
  }
  assert.notEqual(L.failureView('unavailable').detail, L.failureView('auth_restricted').detail);
  // Unknown categories degrade to the generic retryable message; the service flag wins over the table.
  assert.equal(L.failureView('from-the-future').reason, 'unknown');
  assert.equal(L.failureView('transport_retryable', false).retryable, false);
});

test('playbackFailure maps an error status and never shows technical advice or raw helper text', () => {
  const status = { playback: { state: 'error', reason: 'unavailable', retryable: false, error: 'ERROR: x https://rr1.example.com/videoplayback' } };
  const f = L.playbackFailure(status);
  assert.deepEqual([f.reason, f.retryable], ['unavailable', false]);
  const all = ['unavailable', 'auth_restricted', 'geo_age', 'live_stream', 'format_unavailable', 'transport_retryable', 'length_probe_failed', 'no_first_bytes_timeout',
      'resolve_timeout', 'helper_exit', 'disk_space', 'busy', 'tools_pending', 'tools_missing', 'offline', 'cancelled', 'start_stalled', 'media_error', 'engine_unsupported', 'unknown'];
  for (const reason of all) {
    const text = JSON.stringify(L.failureView(reason));
    assert.ok(!/diagnostic|yt-dlp|ffmpeg|https?:/i.test(text), `${reason}: ${text}`);
  }
  assert.equal(L.playbackFailure({ playback: { state: 'streaming' } }), null);
  assert.equal(L.playbackFailure(null), null);
  // A failure without a reason from an older service still yields a retryable generic message.
  assert.deepEqual([L.playbackFailure({ playback: { state: 'error' } }).reason, L.playbackFailure({ playback: { state: 'error' } }).retryable], ['unknown', true]);
});

test('the start watchdog ends an endless spinner but leaves cached, errored and local playback alone', () => {
  const now = 1_000_000;
  const waiting = { playback: { state: 'resolving', ready: false } };
  assert.equal(L.startStalled(now - 1000, now, waiting), false);
  assert.equal(L.startStalled(now - L.START_WATCHDOG_MS - 1, now, waiting), true);
  assert.equal(L.startStalled(now - 5000, now, waiting, 4000), true);
  for (const playback of [{ state: 'cached' }, { state: 'error' }, { state: 'streaming', ready: true }, { local: true, state: 'resolving' }]) {
    assert.equal(L.startStalled(now - L.START_WATCHDOG_MS - 1, now, { playback }), false, JSON.stringify(playback));
  }
  assert.equal(L.startStalled(0, now, waiting), false);
});
