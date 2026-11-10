// Real standalone server + Chromium smoke test, with synthetic media and yt-dlp.
// Build first: cargo build --locked -p muninn-server --bin muninn-service
// Run: node --test e2e/pwa-server.test.js
// Optional: CHROMIUM_PATH, MUNINN_SERVICE, PWA_SCREENSHOTS (output directory).
// Requires Playwright, FFmpeg and Python 3; never uses a personal data directory.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn, spawnSync } = require('node:child_process');
const { chromium } = require('playwright');

const ROOT = path.resolve(__dirname, '..');
const BINARY = process.env.MUNINN_SERVICE || path.join(ROOT, 'target/debug/muninn-service');
const SHOTS = process.env.PWA_SCREENSHOTS;
const ID = 'aaaaaaaaaaa';
const OTHER = 'bbbbbbbbbbb';
const SEARCH = 'ccccccccccc';
const HLS_ID = 'ddddddddddd';
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
function run(command, args) {
  const r = spawnSync(command, args, { encoding: 'utf8' });
  assert.equal(r.status, 0, `${command}: ${r.error || r.stderr}`);
  return r.stdout;
}
async function eventually(check, label, timeout = 15000) {
  const end = Date.now() + timeout;
  let error;
  while (Date.now() < end) {
    try { const result = await check(); if (result) return result; } catch (e) { error = e; }
    await delay(100);
  }
  throw new Error(`${label} timed out${error ? ': ' + error.message : ''}`);
}
async function screenshot(page, name) {
  if (SHOTS) { fs.mkdirSync(SHOTS, { recursive: true }); await page.screenshot({ path: path.join(SHOTS, name + '.png'), fullPage: true }); }
}

test('PWA pairs, browses, plays, seeks, copies links and caches only its shell', { timeout: 120000 }, async () => {
  assert.ok(fs.existsSync(BINARY), 'build muninn-service before running the smoke test');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'muninn-pwa-e2e-'));
  const data = path.join(dir, 'data');
  const bin = path.join(data, 'bin');
  fs.mkdirSync(bin, { recursive: true });
  // A local stand-in for the acquisition tool keeps this test independent of YouTube.
  // The HTTP adapter, databases, media Range responses, PWA and browser are real.
  fs.writeFileSync(path.join(bin, 'yt-dlp'), `#!/usr/bin/env python3
import json,sys
if any('search_query=' in arg for arg in sys.argv):
 print(json.dumps({'entries':[{'id':'${SEARCH}','title':'Synthetic search result','channel':'Test channel','duration':20}]}))
else:
 print('2026.11.10')
`, { mode: 0o755 });
  const media = path.join(dir, 'sample.mp4');
  run('ffmpeg', ['-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=24', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000', '-t', '20', '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-movflags', '+faststart', media]);
  const info = path.join(dir, 'sample.info.json');
  fs.writeFileSync(info, JSON.stringify({ id: ID, title: 'A quiet afternoon — browser playback test', channel: 'Muninn test library', height: 360, duration: 20 }));
  const compatible = path.join(dir, 'compatible.mkv');
  run('ffmpeg', ['-v', 'error', '-i', media, '-c', 'copy', compatible]);
  const compatibleInfo = path.join(dir, 'compatible.info.json');
  fs.writeFileSync(compatibleInfo, JSON.stringify({ id: HLS_ID, title: 'A local compatibility copy', channel: 'Muninn test library', height: 360, duration: 20 }));
  const thumbnail = path.join(dir, 'sample.png');
  run('ffmpeg', ['-v', 'error', '-i', media, '-frames:v', '1', '-threads', '1', thumbnail]);
  let logs = '', server, browser;
  try {
    server = spawn(BINARY, ['--server', '--bind', '127.0.0.1:0', '--data-dir', data], { stdio: ['ignore', 'pipe', 'pipe'] });
    server.stdout.on('data', chunk => { logs += chunk; });
    server.stderr.on('data', chunk => { logs += chunk; });
    const base = await eventually(() => logs.match(/Browser app: (http:\/\/127\.0\.0\.1:\d+)\/app\//)?.[1], 'server startup');
    const token = fs.readFileSync(path.join(data, 'token'), 'utf8').trim();
    const internal = 'http://127.0.0.1:' + JSON.parse(fs.readFileSync(path.join(data, 'service.json'))).port;
    assert.notEqual(base, internal, 'private helper listener must be separate');
    assert.ok(!logs.includes(token), 'server must not print the pairing token');
    const local = async (route, method = 'GET', body) => {
      const response = await fetch(internal + route, { method, headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
      const value = await response.json();
      assert.ok(response.ok, `fixture ${route}: ${JSON.stringify(value)}`);
      return value;
    };
    await local('/api/settings', 'PUT', { offline_mode: true, auto_prefetch: false, min_free_bytes: 0 });
    await local('/api/import/media', 'POST', { path: media, video_id: ID, metadata: info });
    await local('/api/import/media', 'POST', { path: compatible, video_id: HLS_ID, metadata: compatibleInfo });
    run('python3', ['-c', `import sqlite3,sys,time
db=sqlite3.connect(sys.argv[1]); data=open(sys.argv[2],'rb').read()
for video in sys.argv[3:]:
 db.execute('INSERT OR REPLACE INTO content(kind,key,data,compressed,fetched_at,expires_at,size) VALUES(?,?,?,0,?,NULL,?)',('thumb',video,data,int(time.time()*1000),len(data)))
db.commit()
`, path.join(data, 'db/content.sqlite'), thumbnail, ID, HLS_ID]);
    await local('/api/ingest/feed', 'POST', { kind: 'related', source_url: 'https://www.youtube.com/watch?v=' + ID, video_id: ID, items: [
      { video_id: ID, title: 'A quiet afternoon — browser playback test', channel: 'Muninn test library', duration_seconds: 20 },
      { video_id: OTHER, title: 'A second saved discovery', channel: 'A different channel', duration_seconds: 260 },
    ] });
    await local('/api/ingest/feed', 'POST', { kind: 'search', query: 'Slow afternoons', source_url: 'https://www.youtube.com/results?search_query=Slow%20afternoons', items: [{ video_id: OTHER, title: 'A second saved discovery', channel: 'A different channel' }] });
    assert.equal((await fetch(base + '/api/library')).status, 401);
    assert.equal((await fetch(base + '/api/library?token=' + token)).status, 401);
    assert.equal((await fetch(base + '/api/settings')).status, 401);

    browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined, args: ['--no-sandbox'] });
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, permissions: ['clipboard-read', 'clipboard-write'] });
    const page = await context.newPage();
    const errors = [], requests = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => requests.push(request.url()));
    await page.goto(base + '/app/');
    await page.locator('#sign-in').waitFor({ state: 'visible' });
    await screenshot(page, 'pwa-connect');
    await page.locator('#pairing-token').fill('incorrect-token');
    await page.locator('#login-form button').click();
    await eventually(async () => (await page.locator('#login-message').textContent()).includes('invalid pairing token'), 'wrong-token error');
    await page.locator('#pairing-token').fill(token);
    await page.locator('#login-form button').click();
    await page.locator('.video-card').first().waitFor({ state: 'visible' });
    assert.equal(await page.locator('#pairing-token').inputValue(), '');
    const cookie = (await context.cookies()).find(c => c.name === 'muninn_session');
    assert.ok(cookie?.httpOnly && cookie.sameSite === 'Strict');
    assert.equal((await context.request.get(base + '/api/settings')).status(), 404);
    assert.equal(await page.evaluate(() => document.cookie), '');
    assert.equal(await page.evaluate(() => localStorage.length + sessionStorage.length), 0);
    await screenshot(page, 'pwa-library-desktop');
    const directCard = page.locator('.video-card').filter({ hasText: 'A quiet afternoon' });
    await directCard.getByRole('button', { name: 'Copy URL', exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'https://www.youtube.com/watch?v=' + ID);
    await directCard.locator('.video-thumb').click();
    await page.waitForFunction(() => document.querySelector('#video').readyState >= 1);
    assert.ok((await page.locator('#video').getAttribute('src')).startsWith('/media/'));
    await page.locator('#video').evaluate(video => { video.muted = true; return video.play(); });
    await page.waitForFunction(() => document.querySelector('#video').currentTime > 0.2);
    await page.locator('#video').evaluate(video => { video.currentTime = 8; });
    await page.waitForFunction(() => document.querySelector('#video').currentTime >= 8);
    await page.locator('#video').evaluate(video => video.pause());
    await eventually(async () => (await local('/api/video/' + ID)).position_seconds >= 8, 'saved playback position');
    await screenshot(page, 'pwa-player-desktop');
    await page.locator('#close-player').click();
    await page.getByRole('button', { name: 'Play A local compatibility copy', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('#video').readyState >= 1);
    assert.ok(requests.some(url => url.includes('/stream/') && url.endsWith('/index.m3u8')), 'compatibility playback uses cookie-authenticated HLS');
    await page.locator('#video').evaluate(video => { video.muted = true; return video.play(); });
    await page.waitForFunction(() => document.querySelector('#video').currentTime > 0.2);
    await page.locator('#close-player').click();
    await page.locator('#search-input').fill('https://www.youtube.com/watch?v=' + ID);
    await page.locator('#search-form button').click();
    await page.waitForFunction(() => document.querySelector('#video').readyState >= 1 && document.querySelector('#video').currentTime >= 8);
    await page.locator('#close-player').click();
    await page.locator('[data-page="feeds"]').click();
    await page.locator('.feed-card').first().waitFor({ state: 'visible' });
    assert.equal(await page.locator('select').count(), 0);
    await page.getByRole('button', { name: 'Related', exact: true }).click();
    assert.equal(await page.locator('.feed-card').count(), 1);
    await page.getByRole('button', { name: 'Explore capture →' }).click();
    await page.locator('#feed-context').waitFor({ state: 'visible' });
    assert.equal(await page.locator('#section-title').textContent(), 'Related to A quiet afternoon — browser playback test');
    assert.ok((await page.locator('#feed-context').textContent()).includes(ID));
    await screenshot(page, 'pwa-related-feed-desktop');
    await page.locator('.video-card').filter({ hasText: 'A second saved discovery' }).getByRole('button', { name: '↓ Download', exact: true }).click();
    await page.locator('[data-page="downloads"]').click();
    await page.locator('.download-row').waitFor({ state: 'visible' });
    assert.match(await page.locator('.download-row').textContent(), /A second saved discovery/);
    await page.setViewportSize({ width: 390, height: 844 });
    await screenshot(page, 'pwa-downloads-mobile');
    await page.locator('[data-page="library"]').click();
    await page.locator('.video-card').first().waitFor({ state: 'visible' });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'mobile must not overflow horizontally');
    await screenshot(page, 'pwa-library-mobile');

    // Exercise the real server search adapter with our deterministic extractor.
    await local('/api/settings', 'PUT', { offline_mode: false, auto_prefetch: false, min_free_bytes: 0 });
    await page.locator('#search-input').fill('Synthetic search');
    await page.locator('#search-form button').click();
    await page.getByRole('heading', { name: 'Synthetic search result', exact: true }).waitFor();
    await local('/api/settings', 'PUT', { offline_mode: true, auto_prefetch: false, min_free_bytes: 0 });
    await page.evaluate(() => navigator.serviceWorker.ready);
    const cached = await page.evaluate(async () => (await Promise.all((await caches.keys()).map(async key => (await (await caches.open(key)).keys()).map(request => new URL(request.url).pathname)))).flat());
    assert.ok(cached.includes('/app/app.js'));
    assert.ok(cached.every(p => p.startsWith('/app/') && p !== '/app/session'), 'only public app-shell URLs may be cached');
    assert.ok(requests.every(url => !url.includes(token) && !url.includes('/h/')), 'no token-bearing player URLs');
    assert.deepEqual(errors, [], 'browser JavaScript errors');

    await page.locator('#sign-out').click();
    await page.locator('#sign-in').waitFor({ state: 'visible' });
    assert.equal((await context.request.get(base + '/api/library')).status(), 401);
    await context.setOffline(true);
    await page.reload();
    await page.locator('#offline-banner').waitFor({ state: 'visible' });
    await page.locator('#sign-in').waitFor({ state: 'visible' });
    assert.equal(await page.locator('.video-card').count(), 0, 'no private library persists in the offline shell');
    await screenshot(page, 'pwa-offline-mobile');
    await context.close();
  } finally {
    await browser?.close();
    if (server && server.exitCode === null) {
      server.kill('SIGTERM');
      await eventually(() => server.exitCode !== null || server.signalCode !== null, 'server shutdown', 10000);
    }
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
