/* Development mock of the Tauri side and of the playback source, for exercising
 * the UI in a plain browser against a real muninn-service. boot.js loads this
 * file ONLY for ?mock=1 outside the app; production never executes it.
 *
 * Query options:  trust=needed|ok|failed   watch=hls|file|error|none|slow
 *                 entry=<entry id of a seeded complete mp4>   tabs=empty
 * Test hooks: window.__mock = { calls, tabs(), native, trust }.                */
(function () {
  'use strict';
  if (window.__TAURI_INTERNALS__) return;
  const M = window.Muninn;
  M.isMock = true;
  const q = new URLSearchParams(location.search);
  const scenario = q.get('watch') || 'hls';
  const seededEntry = q.get('entry') || 'aaaaaaaaaaa.360';
  M.mockHlsJs = q.get('hlsjs') === '1';          // test the real hls.js path against real HLS files
  const hlsDir = q.get('hlsdir');                // <video_id>.<variant> dir under cache/hls
  const MOCK_DURATION = Number(q.get('dur')) || 60;

  const calls = [];
  const native = { visible: false, rect: null, layouts: 0 };
  let trust = { state: q.get('trust') || 'ok', detail: q.get('trust') === 'failed' ? 'security: SecTrustSettingsSetTrustSettings failed' : undefined };
  let rev = 1, nextId = 20, active = 1;

  const now = Date.now();
  const mk = (id, title, url, extra) => Object.assign({
    id, title, url, face: 'yt', video_id: null, state: 'live', kind: 'home', query: null,
    hist: [{ url, title, face: 'yt', video_id: null, kind: 'home', query: null }], hi: 0, last_active_ms: now - id * 1000,
  }, extra || {});
  let tabs = q.get('tabs') === 'empty' ? [] : [
    mk(1, 'YouTube', 'https://www.youtube.com/'),
    mk(2, 'Subscriptions - YouTube', 'https://www.youtube.com/feed/subscriptions', { kind: 'subscriptions' }),
    mk(3, 'lofi hip hop - YouTube', 'https://www.youtube.com/results?search_query=lofi+hip+hop', { kind: 'search', query: 'lofi hip hop', state: 'sleeping', last_active_ms: now - 900000 }),
    mk(4, 'Rust Programming', 'https://www.youtube.com/@rustlang', { kind: 'channel', state: 'sleeping', last_active_ms: now - 1800000 }),
    mk(5, 'Watching: Seeded video', 'https://www.youtube.com/watch?v=aaaaaaaaaaa', { face: 'watch', video_id: 'aaaaaaaaaaa', state: 'live', kind: 'other' }),
  ];
  if (q.get('active')) active = Number(q.get('active'));
  if (!tabs.length) active = null;

  function bump() { rev++; }
  function tabView(t) {
    return {
      id: t.id, title: t.title, url: t.url, face: t.face, video_id: t.video_id, state: t.state,
      live: t.state === 'live' || t.state === 'loading', kind: t.kind, query: t.query,
      can_back: t.hi > 0, can_forward: t.hi < t.hist.length - 1, last_active_ms: t.last_active_ms,
    };
  }
  const find = (id) => tabs.find((t) => t.id === id);
  function applyEntry(t, e) { Object.assign(t, { url: e.url, title: e.title, face: e.face, video_id: e.video_id, kind: e.kind, query: e.query }); }
  function navigate(t, e, instant) {
    t.hist = t.hist.slice(0, t.hi + 1); t.hist.push(e); t.hi = t.hist.length - 1;
    applyEntry(t, e);
    if (e.face === 'yt' && !instant) { t.state = 'loading'; setTimeout(() => { if (t.state === 'loading') { t.state = 'live'; bump(); } }, 900); }
    bump();
  }
  function classify(input) {
    input = String(input || '').trim();
    let m = /[?&]v=([A-Za-z0-9_-]{11})/.exec(input) || /youtu\.be\/([A-Za-z0-9_-]{11})/.exec(input);
    if (m) return { url: 'https://www.youtube.com/watch?v=' + m[1], title: 'Video ' + m[1], face: 'watch', video_id: m[1], kind: 'other', query: null };
    if (/^https?:\/\//.test(input) || /^[\w-]+(\.[\w-]+)+(\/|$)/.test(input)) {
      const url = /^https?:\/\//.test(input) ? input : 'https://' + input;
      const kind = /\/feed\/subscriptions/.test(url) ? 'subscriptions' : (/\/(channel|@)/.test(url) ? 'channel' : (/results\?/.test(url) ? 'search' : (/youtube\.com\/?$/.test(url) ? 'home' : 'other')));
      return { url, title: new URL(url).hostname, face: 'yt', video_id: null, kind, query: null };
    }
    return { url: 'https://www.youtube.com/results?search_query=' + encodeURIComponent(input), title: input + ' - YouTube', face: 'yt', video_id: null, kind: 'search', query: input };
  }
  function enforceWarm() {
    const warm = 3;
    const live = tabs.filter((t) => t.state === 'live' || t.state === 'loading');
    if (live.length <= warm) return;
    const victim = live.filter((t) => t.id !== active && t.face === 'yt').sort((a, b) => a.last_active_ms - b.last_active_ms)[0];
    if (victim) setTimeout(() => { victim.state = 'sleeping'; bump(); }, 1200);
  }

  const handlers = {
    app_boot() { const m = /(?:^#|&)token=([^&]*)/.exec(location.hash); let tok = m ? decodeURIComponent(m[1]) : null; try { tok = tok || sessionStorage.getItem('muninn.token'); } catch (e) { /* ignore */ } return { token: tok, version: 'mock', platform: 'mock', data_dir: '/tmp/mock' }; },
    tabs_state() { return { rev, active, warm_tabs: 3, tabs: tabs.map(tabView) }; },
    tab_new(a) {
      const url = (a && a.url) || 'https://www.youtube.com/';
      const id = nextId++;
      const t = mk(id, 'New tab', url, { state: 'loading', last_active_ms: Date.now() });
      tabs.push(t); active = id;
      const videoId=window.MuninnLib.extractVideoId(url);
      if(videoId){Object.assign(t,{face:'watch',video_id:videoId,state:'live',kind:'other',title:'Video '+videoId});t.hist=[{url,title:t.title,face:'watch',video_id:videoId,kind:'other',query:null}];bump();return id;}
      setTimeout(() => { t.title = 'YouTube'; if (t.state === 'loading') t.state = 'live'; bump(); enforceWarm(); }, 900);
      bump();
      return id;
    },
    tab_activate(a) {
      const t = find(a.id); if (!t) throw 'no such tab';
      active = t.id; t.last_active_ms = Date.now();
      if (t.state === 'sleeping') {
        t.state = 'restoring';
        setTimeout(() => { t.state = 'loading'; bump(); setTimeout(() => { t.state = 'live'; bump(); enforceWarm(); }, 700); }, Number(q.get('wake')) || 1500);
      }
      bump();
    },
    tab_close(a) {
      const i = tabs.findIndex((t) => t.id === a.id); if (i < 0) return;
      tabs.splice(i, 1);
      if (active === a.id) active = tabs.length ? tabs[Math.min(i, tabs.length - 1)].id : null;
      bump();
    },
    tab_go(a) { const t = find(a.id); if (!t) throw 'no such tab'; navigate(t, classify(a.input)); },
    tab_back(a) { const t = find(a.id); if (t && t.hi > 0) { t.hi--; applyEntry(t, t.hist[t.hi]); bump(); } },
    tab_forward(a) { const t = find(a.id); if (t && t.hi < t.hist.length - 1) { t.hi++; applyEntry(t, t.hist[t.hi]); bump(); } },
    tab_reload(a) { const t = find(a.id); if (t && t.face === 'yt') { t.state = 'loading'; bump(); setTimeout(() => { t.state = 'live'; bump(); }, 800); } },
    tab_open_video(a) {
      if (a.background) {
        const id = nextId++;
        tabs.push(mk(id, 'Video ' + a.video_id, 'https://www.youtube.com/watch?v=' + a.video_id, { face: 'watch', video_id: a.video_id, state: 'sleeping', last_active_ms: Date.now() }));
        bump(); return;
      }
      const t = find(a.id); if (!t) throw 'no such tab';
      const e = { url: 'https://www.youtube.com/watch?v=' + a.video_id, title: 'Video ' + a.video_id, face: 'watch', video_id: a.video_id, kind: 'other', query: null };
      navigate(t, e, true);
      fetch('/api/video/' + a.video_id, { headers: { Authorization: 'Bearer ' + handlers.app_boot().token } }).then((r) => r.json()).then((d) => {
        if (d && d.meta && d.meta.title) { e.title = d.meta.title; if (t.video_id === a.video_id) t.title = d.meta.title; bump(); }
      }).catch(() => {});
    },
    tab_hibernate(a) { const t = find(a.id); if (t && t.face === 'yt') { t.state = 'sleeping'; bump(); } },
    layout(r) { native.rect = r; native.layouts++; paintNative(); },
    native_visible(a) { native.visible = !!a.visible; paintNative(); },
    signin_popup() { return null; },
    open_external() { return null; },
    export_diagnostics() { return { status: 'saved', path: 'Downloads/muninn-diagnostics-2026-01-01.zip' }; },
    trust_status() { return trust; },
    trust_install() { trust = { state: 'installing' }; setTimeout(() => { trust = { state: 'ok' }; }, 1500); },
  };

  window.__TAURI_INTERNALS__ = {
    invoke(cmd, args) {
      if (cmd !== 'tabs_state') calls.push({ cmd, args, t: Date.now() });
      const f = handlers[cmd];
      if (!f) return Promise.reject('unknown command ' + cmd);
      try { return Promise.resolve(f(args || {})); } catch (e) { return Promise.reject(String(e)); }
    },
  };
  window.__mock = { calls, watchPosts: [], native, tabs: () => tabs.map(tabView), get trust() { return trust; }, set trust(v) { trust = v; } };

  /* A stand-in for the native YouTube web view, drawn exactly where Rust would put it. */
  let nativeEl = null;
  function paintNative() {
    if (!nativeEl) {
      nativeEl = document.createElement('div');
      nativeEl.id = 'mock-native';
      // CSSOM only: the page's CSP forbids <style> elements and style attributes.
      Object.assign(nativeEl.style, { position: 'fixed', zIndex: '99999', pointerEvents: 'none', display: 'flex', alignItems: 'center', justifyContent: 'center', font: '600 14px system-ui', color: '#fff', opacity: '.92', background: 'repeating-linear-gradient(45deg,#b3261e,#b3261e 14px,#8c1d18 14px,#8c1d18 28px)' });
      document.body.append(nativeEl);
    }
    const r = native.rect;
    const t = tabs.find((x) => x.id === active);
    nativeEl.style.display = native.visible && r ? 'flex' : 'none';
    if (r) { nativeEl.style.left = r.x + 'px'; nativeEl.style.top = r.y + 'px'; nativeEl.style.width = r.w + 'px'; nativeEl.style.height = r.h + 'px'; }
    nativeEl.textContent = 'Native YouTube web view' + (t ? ': ' + t.url : '');
  }
  setInterval(paintNative, 500);

  /* ---- the watch status progression (the real service needs yt-dlp for this) ---- */
  const started = new Map();
  const realFetch = window.fetch.bind(window);
  function statusFor(id) {
    const t0 = started.get(id) || Date.now();
    const t = (Date.now() - t0) / 1000;
    const base = { video_id: id, has_more_comments: true };
    if (scenario === 'none') return Object.assign(base, { meta_state: 'loading', meta_error: null, playback: { state: 'none', ready: false } });
    const meta_state = t < 2 ? 'loading' : (scenario === 'error' ? 'error' : 'ready');
    const meta_error = meta_state === 'error' ? 'could not fetch the watch page' : null;
    if (scenario === 'error') {
      if (t < 1.2) return Object.assign(base, { meta_state, meta_error, playback: { state: 'resolving', phase: 'resolving', kind: 'hls', ready: false, progress: 0 } });
      return Object.assign(base, { meta_state, meta_error, playback: { state: 'error', phase: 'failed', kind: 'hls', ready: false, progress: 0, error: "ERROR: [youtube] " + id + ": Sign in to confirm you're not a bot. Use --cookies-from-browser or --cookies for the authentication." } });
    }
    const slow = scenario === 'slow' ? 4 : 1;
    if (t < 1.2 * slow) return Object.assign(base, { meta_state, meta_error, playback: { state: 'resolving', phase: 'resolving', kind: 'hls', ready: false, progress: 0, entry_id: id + '.720', variant: '720', height: 720 } });
    if (scenario === 'file') {
      return Object.assign(base, { meta_state, meta_error, playback: { state: 'cached', kind: 'file', entry_id: seededEntry, variant: '360', height: 360, ready: true, url_path: '/media/' + seededEntry, progress: 1 } });
    }
    const ready = t >= 2.5 * slow;
    const prog = ready ? Math.min(1, 0.05 + (t - 2.5 * slow) / (Number(q.get('grow')) || 40)) : 0.01;
    if (prog >= 1) return Object.assign(base, { meta_state, meta_error, playback: { state: 'cached', kind: 'file', entry_id: seededEntry, variant: '360', height: 360, ready: true, url_path: '/media/' + seededEntry, progress: 1 } });
    return Object.assign(base, {
      meta_state, meta_error,
      playback: {
        state: 'streaming', phase: 'caching', kind: 'hls', entry_id: seededEntry, variant: '360', height: 360, ready, progress: prog, error: null, segments: ready ? 3 : 1,
        duration_seconds: MOCK_DURATION, audio_only: false,
        // Chromium cannot play HLS: hand it the seeded mp4 (token inside the path, like the real HLS URLs).
        url_path: hlsDir ? '/h/' + handlers.app_boot().token + '/' + hlsDir + '/index.m3u8' : '/media/' + seededEntry + '?token=' + encodeURIComponent(handlers.app_boot().token || ''),
      },
    });
  }
  window.fetch = function (input, init) {
    const url = typeof input === 'string' ? input : input.url;
    const m = /^\/api\/watch\/([A-Za-z0-9_-]{11})$/.exec(url.split('?')[0]);
    if (m) {
      const method = ((init && init.method) || 'GET').toUpperCase();
      if (method === 'POST') { started.set(m[1], Date.now()); window.__mock.watchPosts.push(m[1]); }
      return Promise.resolve(new Response(JSON.stringify(statusFor(m[1])), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    }
    if (q.get('diag') && /^\/api\/proxy$/.test(url.split('?')[0])) {
      const t = Date.now();
      return Promise.resolve(new Response(JSON.stringify({
        info: { port: 47872, url: 'http://127.0.0.1:47872', ca_pem_path: '/tmp/ca.pem' },
        stats: { connects: 128, mitm_connects: 41, tunnels: 87, blocked: 12, requests: 300, captured: 23, captured_bytes: 4812000, tls_ok: 40, tls_failures: 1, tls_untrusted: 0, media_bytes_fetched: 0, hosts: [{ host: 'www.youtube.com', count: 90 }, { host: 'i.ytimg.com', count: 31 }, { host: 'accounts.google.com', count: 6 }] },
        events: [{ ts: t - 3000, kind: 'mitm', host: 'www.youtube.com', detail: 'GET /youtubei/v1/browse', status: 200, bytes: 182000 }, { ts: t - 2000, kind: 'blocked', host: 'rr3---sn-abc.googlevideo.com', detail: 'videoplayback blocked', status: 403, bytes: null }, { ts: t - 1000, kind: 'tunnel', host: 'accounts.google.com', detail: 'CONNECT', status: null, bytes: null }],
        ca_pem: null }), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    }
    if (q.get('diag') && /^\/api\/captures$/.test(url.split('?')[0])) {
      const t = Date.now();
      return Promise.resolve(new Response(JSON.stringify([{ ts: t - 5000, url: 'https://www.youtube.com/youtubei/v1/browse', what: 'browse', kind: 'home', items: 24, comments: 0, video_id: null, snapshot_id: 3, note: null }, { ts: t - 9000, url: 'https://www.youtube.com/youtubei/v1/next', what: 'next', kind: null, items: 12, comments: 20, video_id: 'aaaaaaaaaaa', snapshot_id: null, note: 'related + comments' }]), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    }
    if (q.get('setup') && /^\/api\/setup$/.test(url.split('?')[0])) {
      const setup = q.get('setup') === 'error' ? { phase: 'error', message: 'Could not download yt-dlp: no network', progress: null } : { phase: 'installing', message: 'Downloading yt-dlp', progress: 0.4 };
      return realFetch(input, init).then((r) => r.json()).then((d) => new Response(JSON.stringify(Object.assign(d, { setup })), { status: 200, headers: { 'Content-Type': 'application/json' } }));
    }
    return realFetch(input, init);
  };

  // With the mock's fake "HLS", only the part that has "downloaded" is seekable.
  if ((scenario === 'hls' || scenario === 'slow') && !M.mockHlsJs) {
    Object.defineProperty(HTMLMediaElement.prototype, 'seekable', {
      configurable: true,
      get() {
        const st = M.watch && M.watch.state && M.watch.state();
        const p = st && st.status && st.status.playback;
        const real = this.duration;
        if (!p || !isFinite(real)) return { length: 0, start() { return 0; }, end() { return 0; } };
        const end = p.progress >= 1 ? real : Math.min(real, MOCK_DURATION * p.progress);
        return { length: 1, start() { return 0; }, end() { return end; } };
      },
    });
  }
})();
