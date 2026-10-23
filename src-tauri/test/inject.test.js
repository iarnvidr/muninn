'use strict';
const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const SOURCE = fs.readFileSync(path.join(__dirname, '..', 'inject.js'), 'utf8');
const lib = require('../inject.js');

test('channel discovery reads both legacy and current channel renderers without credentials',()=>{
  const data={contents:[{channelRenderer:{channelId:'UCabcdefghijklmnopqrstuv',title:{simpleText:'First'}}},{lockupViewModel:{contentId:'UCzyxwvutsrqponmlkjihgfe',metadata:{lockupMetadataViewModel:{title:{content:'Second'}}}}}]};
  const channels=lib.channelCandidates(data);
  assert.deepEqual(channels.map(c=>c.name),['First','Second']);
  data.self=data;
  assert.equal(lib.channelCandidates(data).length,2,'cyclic page data is bounded');
});

test('video links are recognised in every shape YouTube uses', () => {
  const id = 'dQw4w9WgXcQ';
  assert.strictEqual(lib.videoIdFromHref('/watch?v=' + id + '&pp=abc', 'https://www.youtube.com/'), id);
  assert.strictEqual(lib.videoIdFromHref('https://www.youtube.com/watch?v=' + id + '&t=30s'), id);
  assert.strictEqual(lib.videoIdFromHref('https://m.youtube.com/watch?v=' + id), id);
  assert.strictEqual(lib.videoIdFromHref('/shorts/' + id), id);
  assert.strictEqual(lib.videoIdFromHref('/live/' + id + '?feature=share'), id);
  assert.strictEqual(lib.videoIdFromHref('https://youtu.be/' + id + '?si=x'), id);
  assert.strictEqual(lib.videoIdFromHref('https://www.youtube.com/embed/' + id), id);
});

test('everything else is left alone', () => {
  assert.strictEqual(lib.videoIdFromHref('/results?search_query=x'), null);
  assert.strictEqual(lib.videoIdFromHref('/@somechannel'), null);
  assert.strictEqual(lib.videoIdFromHref('/watch?v=short'), null, 'not an 11 character id');
  assert.strictEqual(lib.videoIdFromHref('/watch'), null);
  assert.strictEqual(lib.videoIdFromHref('https://example.org/watch?v=dQw4w9WgXcQ'), null, 'only YouTube hosts');
  assert.strictEqual(lib.videoIdFromHref('https://evilyoutube.com/watch?v=dQw4w9WgXcQ'), null);
  assert.strictEqual(lib.videoIdFromHref('not a url at all', 'not a base'), null);
  assert.strictEqual(lib.videoIdFromHref(undefined), null);
});

// A page just detailed enough for install(): records listeners and IPC calls.
function fakePage(config) {
  const calls = [];
  const docListeners = {};
  const winListeners = {};
  const sandbox = {
    __TAURI_INTERNALS__: { invoke: (cmd, args) => { calls.push([cmd, args]); return Promise.resolve(); } },
    location: { href: 'https://www.youtube.com/' },
    scrollY: 0,
    setInterval: () => 0,
    setTimeout: () => 0,
    MutationObserver: class { observe() {} },
    URL,
    Date,
    Math,
  };
  sandbox.document = {
    title: 'YouTube',
    documentElement: {},
    getElementsByTagName: () => [],
    addEventListener: (n, f) => { (docListeners[n] = docListeners[n] || []).push(f); },
  };
  sandbox.addEventListener = (n, f) => { (winListeners[n] = winListeners[n] || []).push(f); };
  sandbox.globalThis = sandbox;
  if(config){sandbox.ytcfg={get:key=>config[key]};sandbox.crypto=require('node:crypto').webcrypto;sandbox.TextEncoder=TextEncoder;}
  vm.runInNewContext(SOURCE, sandbox);
  return { calls, docListeners, winListeners, sandbox };
}

function clickOn(page, href, extra = {}) {
  const a = { tagName: 'A', href, parentNode: null };
  const ev = { type: 'click', target: a, defaultPrevented: false, prevented: false, stopped: false, metaKey: false, ctrlKey: false, button: 0, ...extra };
  ev.preventDefault = () => { ev.prevented = true; };
  ev.stopImmediatePropagation = () => { ev.stopped = true; };
  page.docListeners.click.forEach((f) => f(ev));
  return ev;
}

test('a click on a video link goes to Muninn and never reaches YouTube', () => {
  const page = fakePage();
  const ev = clickOn(page, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ&pp=x');
  assert.ok(ev.prevented && ev.stopped);
  assert.strictEqual(JSON.stringify(page.calls.filter((c) => c[0] === 'page_open_video')), JSON.stringify([['page_open_video', { video_id: 'dQw4w9WgXcQ', background: false }]]));
});

test('ctrl/cmd/middle click opens the video in a background tab', () => {
  const page = fakePage();
  clickOn(page, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ', { metaKey: true });
  clickOn(page, 'https://www.youtube.com/shorts/aaaaaaaaaaa', { button: 1 });
  const opens = page.calls.filter((c) => c[0] === 'page_open_video').map((c) => c[1].background);
  assert.strictEqual(JSON.stringify(opens), JSON.stringify([true, true]));
});

test('other links and clicks on non-links are untouched', () => {
  const page = fakePage();
  const ev = clickOn(page, 'https://www.youtube.com/@chan');
  assert.ok(!ev.prevented);
  const plain = { type: 'click', target: { tagName: 'DIV', parentNode: null }, preventDefault() { throw new Error('no'); } };
  page.docListeners.click.forEach((f) => f(plain));
  assert.strictEqual(page.calls.filter((c) => c[0] === 'page_open_video').length, 0);
});

test("YouTube's own navigation to a video is handed over too", () => {
  const page = fakePage();
  page.winListeners['yt-navigate-start'].forEach((f) => f({ detail: { url: '/watch?v=dQw4w9WgXcQ' }, stopImmediatePropagation() {} }));
  assert.strictEqual(page.calls.filter((c) => c[0] === 'page_open_video').length, 1);
  page.calls.length = 0;
  page.winListeners['yt-navigate-start'].forEach((f) => f({ detail: { url: '/feed/subscriptions' } }));
  assert.strictEqual(page.calls.length, 0);
});

test('page state reports the url, title and scroll position; bursts are throttled', () => {
  const page = fakePage();
  page.winListeners.load.forEach((f) => f());
  page.sandbox.scrollY = 1234.4;
  page.winListeners['yt-navigate-finish'].forEach((f) => f());
  const states = () => page.calls.filter((c) => c[0] === 'page_state');
  assert.strictEqual(states().length, 2);
  assert.strictEqual(JSON.stringify(states()[1][1]), JSON.stringify({ url: 'https://www.youtube.com/', title: 'YouTube', scroll_y: 1234 }));
  // Scrolling fires constantly: only the first event in a window reports.
  page.sandbox.scrollY = 5000;
  for (let i = 0; i < 20; i++) page.winListeners.scroll.forEach((f) => f());
  assert.strictEqual(states().length, 3);
});

test('the script never throws into the page when IPC is missing', () => {
  const page = fakePage();
  delete page.sandbox.__TAURI_INTERNALS__;
  assert.doesNotThrow(() => clickOn(page, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ'));
});

test('account identity is opaque, changes with the selected account and is not guessed when unavailable',async()=>{
  const config={LOGGED_IN:true,DATASYNC_ID:'fixture-account-identity',SESSION_INDEX:0};const p=fakePage(config);
  const first=await lib.accountKey(p.sandbox);assert.match(first,/^[a-f0-9]{64}$/);assert.ok(!first.includes(config.DATASYNC_ID));
  config.SESSION_INDEX=1;assert.notEqual(await lib.accountKey(p.sandbox),first);
  config.LOGGED_IN=false;assert.equal(await lib.accountKey(p.sandbox),null);
  config.LOGGED_IN=true;delete config.DATASYNC_ID;assert.equal(await lib.accountKey(p.sandbox),null);
});
test('page observations distinguish unknown, signed in and signed out without sending raw identity',async()=>{
  const config={LOGGED_IN:true,DATASYNC_ID:'fixture-private-identity',SESSION_INDEX:0};const p=fakePage(config);
  p.winListeners.load.forEach(f=>f());
  for(let i=0;i<30&&!p.calls.some(c=>c[1].account_key);i++)await new Promise(resolve=>setImmediate(resolve));
  const states=()=>p.calls.filter(c=>c[0]==='page_state').map(c=>c[1]);
  assert.equal(states().at(-1).signed_in,true);assert.match(states().at(-1).account_key,/^[a-f0-9]{64}$/);assert.ok(!JSON.stringify(p.calls).includes(config.DATASYNC_ID));
  config.LOGGED_IN=false;p.winListeners['yt-navigate-finish'].forEach(f=>f());
  assert.equal(states().at(-1).signed_in,false);assert.equal(states().at(-1).account_key,null);
  const unknown=fakePage();unknown.winListeners.load.forEach(f=>f());assert.equal(unknown.calls[0][1].signed_in,undefined);
});
test('account switch in the same document cannot relabel the previous account DOM as new subscriptions',async()=>{
  const config={LOGGED_IN:true,DATASYNC_ID:'account-a',SESSION_INDEX:0},p=fakePage(config);
  p.sandbox.location.pathname='/feed/channels';p.sandbox.ytInitialData={channelRenderer:{channelId:'UCabcdefghijklmnopqrstuv',title:{simpleText:'Old account channel'}}};
  p.winListeners.load.forEach(f=>f());
  for(let i=0;i<30&&!p.calls.some(c=>c[1].account_key);i++)await new Promise(resolve=>setImmediate(resolve));
  assert.equal(p.calls.at(-1)[1].channels.length,1);
  config.DATASYNC_ID='account-b';p.winListeners['yt-navigate-finish'].forEach(f=>f());
  const latest=p.calls.at(-1)[1];assert.equal(latest.channel_status,'error');assert.equal(latest.channels.length,0,'fresh document required before binding old DOM to a changed account');
});

test('install is idempotent', () => {
  const page = fakePage();
  vm.runInNewContext(SOURCE, page.sandbox);
  assert.strictEqual(page.docListeners.click.length, 1);
});

test('throttle runs at most once per window and keeps the last call', () => {
  let n = 0;
  const f = lib.throttle(() => { n++; }, 50);
  f(); f(); f();
  assert.strictEqual(n, 1);
});
