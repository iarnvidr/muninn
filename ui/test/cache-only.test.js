'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const L = require('../lib.js');

const ORIGIN = 'http://127.0.0.1:47871';
const file = url_path => ({ ready: true, kind: 'file', url_path });
const hls = url_path => ({ ready: true, kind: 'hls', url_path });

test('only device-cache routes are playable, including a still-growing disk-backed HLS cache', () => {
  assert.equal(L.mediaUrl(file('/media/abcdefghijk.720'), ORIGIN, 'secret token'),
    ORIGIN + '/media/abcdefghijk.720?token=secret%20token');
  assert.equal(L.mediaUrl({ ...hls('/h/TOKEN/abcdefghijk.720/index.m3u8'), progress: 0.1, complete: false }, ORIGIN, 'TOKEN'),
    ORIGIN + '/h/TOKEN/abcdefghijk.720/index.m3u8');
  assert.equal(L.mediaUrl(hls('/h/TOKEN/abcdefghijk.720.local.42/index.m3u8'), ORIGIN, 'TOKEN'),
    ORIGIN + '/h/TOKEN/abcdefghijk.720.local.42/index.m3u8');
  assert.equal(L.mediaUrl({ ...file('/media/abcdefghijk.720'), ready: false }, ORIGIN, 'TOKEN'), null);
  assert.equal(L.mediaUrl(null, ORIGIN, 'TOKEN'), null);
});

test('remote origins cannot turn cache metadata into remote media playback', () => {
  for (const origin of ['https://server.example', 'http://192.168.1.2:47871', 'http://100.117.89.46:47871',
    'https://www.youtube.com', 'http://127.0.0.1.evil.test', 'http://user:pass@127.0.0.1:47871',
    'file://', 'data:text/plain,', '', undefined]) {
    assert.equal(L.mediaUrl(file('/media/abcdefghijk.720'), origin, 'TOKEN'), null, String(origin));
    assert.equal(L.mediaUrl(hls('/h/TOKEN/abcdefghijk.720/index.m3u8'), origin, 'TOKEN'), null, String(origin));
  }
  for (const origin of ['http://127.0.0.1:47871', 'http://localhost:47871', 'http://[::1]:47871']) {
    assert.equal(L.mediaUrl(file('/media/abcdefghijk.720'), origin, 'TOKEN'),
      origin + '/media/abcdefghijk.720?token=TOKEN');
  }
});

test('external URLs, upstream feeders, traversal, and arbitrary routes never become player sources', () => {
  const rejected = [
    'https://rr1.googlevideo.com/videoplayback', '//server.example/media/abc',
    'blob:http://127.0.0.1:47871/id', 'data:video/mp4;base64,AAAA',
    '/s/TOKEN/upstream', '/api/watch/abcdefghijk', '/thumb/abcdefghijk',
    '/media/', '/media/..', '/media/.', '/media/../s/upstream',
    '/media/%2e%2e', '/media/%2e%2e%2fs%2fupstream', '/media/abc%2fdef',
    '/media/abc\\def', '/media/abc\n', '/media/abc\r',
    '/media/abc?redirect=https://server.example', '/media/abc#fragment',
  ];
  for (const url of rejected) assert.equal(L.mediaUrl(file(url), ORIGIN, 'TOKEN'), null, url);
  for (const url of ['/h/TOKEN/job/../index.m3u8', '/h/TOKEN/../index.m3u8',
    '/h/TOKEN/%2e%2e/index.m3u8', '/h/TOKEN/job/video.ts', '/h/TOKEN/job/index.m3u8?remote=1',
    '/h/TOKEN/job/index.m3u8#fragment', '/media/abcdefghijk.720']) {
    assert.equal(L.mediaUrl(hls(url), ORIGIN, 'TOKEN'), null, url);
  }
  assert.equal(L.mediaUrl(file('/h/TOKEN/job/index.m3u8'), ORIGIN, 'TOKEN'), null);
  for (const kind of ['remote', 'stream', 'youtube', '', undefined]) {
    assert.equal(L.mediaUrl({ ready: true, kind, url_path: '/media/abcdefghijk.720' }, ORIGIN, 'TOKEN'), null);
  }
});

// Run the complete production watch module, not a copied Keep implementation or
// a source-pattern assertion. This DOM/media model proves JS behavior only; real
// WebKit playback, focus, native tabs, and packaging remain macOS smoke gates.
function watchHarness(promote = () => Promise.resolve({}), options = {}) {
  const elements = [];
  class Element {
    constructor(tag, props = {}) {
      this.tagName = tag.toUpperCase();
      this.props = props;
      this.children = [];
      this.dataset = {};
      this.attributes = new Map();
      this.listeners = new Map();
      this.style = { removeProperty(name) { delete this[name]; }, setProperty(name, value) { this[name] = value; } };
      const classes = new Set((props.class || '').split(' ').filter(Boolean));
      this.classList = {
        add: (...names) => names.forEach(name => classes.add(name)),
        remove: (...names) => names.forEach(name => classes.delete(name)),
        contains: name => classes.has(name),
        toggle(name, force) { const enabled = force === undefined ? !classes.has(name) : force; enabled ? classes.add(name) : classes.delete(name); return enabled; },
      };
      this.textContent = props.text || '';
      this.hidden = !!props.hidden;
      this.value = props.value || '';
      this.paused = true;
      this.ended = false;
      this.readyState = 0;
      this.currentTime = 0;
      this.duration = NaN;
      this.currentSrc = '';
      this.src = '';
      this.volume = 1;
      this.playbackRate = 1;
      this.seekable = this.buffered = { length: 0 };
      this.mediaOperations = [];
      elements.push(this);
    }
    append(...children) { for (const child of children.flat()) if (child != null) { this.children.push(child); if (typeof child === 'object') child.parentElement = this; } }
    replaceChildren(...children) { this.children = []; this.append(...children); }
    insertBefore(child) { this.append(child); }
    querySelector() { return null; }
    addEventListener(type, callback) { if (!this.listeners.has(type)) this.listeners.set(type, []); this.listeners.get(type).push(callback); }
    emit(type) { for (const callback of this.listeners.get(type) || []) callback({ type, target: this }); }
    setAttribute(name, value) { this.attributes.set(name, String(value)); }
    removeAttribute(name) { this.attributes.delete(name); }
    getBoundingClientRect() { return { x: 0, y: 0, width: 100, height: 100, toJSON: () => ({}) }; }
    play() { this.mediaOperations.push('play'); this.paused = false; return Promise.resolve(); }
    pause() { this.mediaOperations.push('pause'); this.paused = true; }
    load() { this.mediaOperations.push('load'); }
    canPlayType() { return options.nativeHls ? 'probably' : ''; }
  }
  const nodes = new Map();
  const select = selector => { if (!nodes.has(selector)) nodes.set(selector, new Element('div')); return nodes.get(selector); };
  const calls = [];
  const tab = { id: 1, video_id: 'abcdefghijk', face: 'watch' };
  const M = {
    L, origin: ORIGIN, token: 'TOKEN', settings: {}, tabs: { active: 1, list: [tab] },
    h(tag, props, ...children) { const element = new Element(tag, props); element.append(...children); return element; },
    $: select,
    icon: name => new Element('span', { text: name }),
    setText(element, text) { element.textContent = text; },
    setHidden(element, hidden) { element.hidden = !!hidden; },
    setCls(element, name, enabled) { element.classList.toggle(name, enabled); },
    setAttr(element, name, value) { value == null ? element.removeAttribute(name) : element.setAttribute(name, value); },
    api(method, url, body) {
      calls.push({ method, url, body });
      if (method === 'POST' && url.endsWith('/promote')) return promote(url);
      if (url.startsWith('/api/watch/') && options.status) return Promise.resolve(options.status);
      // Hold unrelated detail/watch refreshes instead of inventing remote data.
      return new Promise(() => {});
    },
    flashError() {},
    errText: error => error?.message || String(error),
  };
  const document = { querySelector: select, body: new Element('body'), addEventListener() {} };
  const window = { Muninn: M, addEventListener() {} };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../watch.js'), 'utf8'), {
    window, document, URL, URLSearchParams, console,
    localStorage: { getItem: () => null, setItem() {} },
    setTimeout: () => 1, clearTimeout() {}, setInterval: () => 1, clearInterval() {},
    requestAnimationFrame: () => 1,
  }, { filename: 'watch.js' });
  M.watch.sync(tab);
  const state = M.watch.state();
  const video = M.watch.video;
  if (options.seed !== false) Object.assign(state, {
    sourceSet: true, resumeDone: true, kind: 'file', playbackEntry: 'abcdefghijk.720',
    completionPolicy: { action: 'delete', completion_percent: 95, cleanup_delay_seconds: 86400 },
    status: { playback: { ready: true, kind: 'file', duration_seconds: 600, url_path: '/media/abcdefghijk.720' } },
    detail: { entries: [{ id: 'abcdefghijk.360', protected: true }, { id: 'abcdefghijk.720', protected: false }] },
  });
  if (options.seed !== false) Object.assign(video, { paused: false, currentTime: 570, duration: 600, readyState: 4,
    src: ORIGIN + '/media/abcdefghijk.720?token=TOKEN', currentSrc: ORIGIN + '/media/abcdefghijk.720?token=TOKEN' });
  const keep = elements.find(element => element.props.text === 'Keep' && typeof element.props.onclick === 'function');
  const prompt = elements.find(element => element.props['aria-label'] === 'Keep this video');
  const marker = elements.find(element => element.props.id === 'player-cleanup-point');
  assert.ok(keep && prompt && marker);
  return { M, calls, state, video, keep, prompt, marker };
}

test('the actual player never attaches or starts rejected remote, feeder, or unready sources', async () => {
  for (const playback of [
    file('https://rr1.googlevideo.com/videoplayback'),
    hls('//server.example/h/TOKEN/job/index.m3u8'),
    hls('/s/TOKEN/upstream'),
    { ...file('/media/abcdefghijk.720'), ready: false },
  ]) {
    const h = watchHarness(undefined, { seed: false, status: { playback } });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(h.state.sourceSet, false, playback.url_path);
    assert.equal(h.video.src, '', playback.url_path);
    assert.equal(h.video.mediaOperations.includes('play'), false, playback.url_path);
    assert.equal(h.calls.some(call => call.url.startsWith('http') || call.url === '/api/cache'), false);
  }
});

test('the actual player attaches a growing HLS cache through the local route only', async () => {
  const url_path = '/h/TOKEN/abcdefghijk.720/index.m3u8';
  const h = watchHarness(undefined, { seed: false, nativeHls: true,
    status: { playback: { ...hls(url_path), state: 'streaming', phase: 'caching', progress: 0.1, segments: 3 } } });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(h.state.sourceSet, true);
  assert.equal(h.video.src, ORIGIN + url_path);
  assert.equal(h.video.mediaOperations.includes('play'), true);
});

test('the existing Keep prompt appears at 95% without interrupting playback and protects this exact copy', async () => {
  const h = watchHarness();
  const before = { src: h.video.src, time: h.video.currentTime, operations: [...h.video.mediaOperations] };
  h.video.currentTime = 569;
  h.video.emit('timeupdate');
  assert.equal(h.prompt.hidden, true);
  h.video.currentTime = 570;
  h.video.emit('timeupdate');
  assert.equal(h.prompt.hidden, false);
  assert.equal(h.marker.style.left, '95%');
  await h.keep.props.onclick();
  assert.deepEqual(h.calls.filter(call => call.url.endsWith('/promote')),
    [{ method: 'POST', url: '/api/entries/abcdefghijk.720/promote', body: undefined }]);
  assert.equal(h.state.keptEntries.has('abcdefghijk.720'), true);
  assert.equal(h.state.keptEntries.has('abcdefghijk.360'), false);
  assert.equal(h.prompt.hidden, true);
  assert.equal(h.video.src, before.src);
  assert.equal(h.video.currentTime, before.time);
  assert.equal(h.video.paused, false);
  assert.deepEqual(h.video.mediaOperations, before.operations);
  assert.equal(h.calls.some(call => call.url === '/api/cache'), false, 'Keep must not download another quality');
});

test('double-clicking Keep shares the in-flight action instead of requesting another copy', async () => {
  let finish;
  const h = watchHarness(() => new Promise(resolve => { finish = resolve; }));
  const first = h.keep.props.onclick();
  await h.keep.props.onclick();
  assert.equal(h.calls.filter(call => call.url.endsWith('/promote')).length, 1);
  assert.equal(h.keep.disabled, true);
  finish({});
  await first;
  assert.equal(h.keep.disabled, false);
  assert.equal(h.video.paused, false);
});

test('a delayed Keep acknowledgement still protects the copy that was playing when clicked', async () => {
  let finish;
  const h = watchHarness(() => new Promise(resolve => { finish = resolve; }));
  const keeping = h.keep.props.onclick();
  h.state.playbackEntry = 'abcdefghijk.audio';
  finish({});
  await keeping;
  assert.equal(h.state.keptEntries.has('abcdefghijk.720'), true);
  assert.equal(h.state.keptEntries.has('abcdefghijk.audio'), false);
  assert.deepEqual(h.calls.filter(call => call.url.endsWith('/promote')).map(call => call.url),
    ['/api/entries/abcdefghijk.720/promote']);
  assert.equal(h.video.paused, false);
});

test('the Keep threshold follows an existing custom policy rather than imposing new defaults', () => {
  const h = watchHarness();
  h.state.completionPolicy.completion_percent = 80;
  h.state.completionPolicy.cleanup_delay_seconds = 7200;
  h.video.currentTime = 479;
  h.video.emit('timeupdate');
  assert.equal(h.prompt.hidden, true);
  h.video.currentTime = 480;
  h.video.emit('timeupdate');
  assert.equal(h.prompt.hidden, false);
  assert.equal(h.marker.style.left, '80%');
  assert.equal(h.state.completionPolicy.cleanup_delay_seconds, 7200);
  h.state.completionPolicy.action = 'keep';
  h.video.emit('timeupdate');
  assert.equal(h.prompt.hidden, true);
  assert.equal(h.video.paused, false);
});

test('a failed Keep remains retryable and never falsely marks a copy as protected', async () => {
  let attempts = 0;
  const h = watchHarness(() => ++attempts === 1 ? Promise.reject(new Error('temporary write failure')) : Promise.resolve({}));
  await h.keep.props.onclick();
  assert.equal(h.state.keptEntries?.has('abcdefghijk.720') || false, false);
  assert.equal(h.prompt.hidden, false);
  assert.equal(h.keep.disabled, false);
  assert.match(h.state.keepPointError, /Try again/);
  assert.equal(h.video.paused, false);
  await h.keep.props.onclick();
  assert.equal(h.state.keptEntries.has('abcdefghijk.720'), true);
  assert.equal(attempts, 2);
});
