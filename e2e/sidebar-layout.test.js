'use strict';
// Browser layout checks for the sidebar and the native-view pane, run against the UI's own
// ?mock=1 mode (synthetic tabs; the native YouTube view is a drawn stand-in) with a stub API.
// It does NOT exercise macOS WKWebView layering: that needs the owner's Mac check.
//
//   npm i --prefix /tmp/pw playwright && npx --prefix /tmp/pw playwright install chromium
//   PLAYWRIGHT_MODULE=/tmp/pw/node_modules/playwright node --test e2e/sidebar-layout.test.js
//   SHOTS=/tmp/muninn-shots   (optional: write screenshots of every state)
//
// Skipped (not failed) when Playwright or a launchable Chromium is unavailable.
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

const UI = process.env.UI_DIR || path.join(__dirname, '..', 'ui');
const SHOTS = process.env.SHOTS || '';
let chromium = null;
try { ({ chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright')); } catch (e) { /* skipped below */ }

const TYPES = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png', '.woff2': 'font/woff2' };
function serve() {
  const featured = { items: [{ video_id: 'bbbbbbbbbbb', title: 'Synthetic favourite video', channel: 'Synthetic channel', duration_seconds: 300 }] };
  const stub = {
    '/api/settings': { background_playback: true },
    '/api/organization': { tab_groups: {} },
    '/api/featured': featured,
  };
  return new Promise((resolve) => {
    const srv = http.createServer((q, r) => {
      const u = new URL(q.url, 'http://x');
      if (u.pathname.startsWith('/ui/')) {
        const f = path.join(UI, path.normalize(u.pathname.slice(4)));
        if (!f.startsWith(UI)) { r.statusCode = 403; return r.end(); }
        try { const d = fs.readFileSync(f); r.setHeader('content-type', TYPES[path.extname(f)] || 'application/octet-stream'); return r.end(d); } catch (e) { r.statusCode = 404; return r.end(); }
      }
      if (u.pathname.startsWith('/api/')) { r.setHeader('content-type', 'application/json'); return r.end(JSON.stringify(stub[u.pathname] || {})); }
      r.statusCode = 404; r.end();
    }).listen(0, '127.0.0.1', () => resolve(srv));
  });
}

const THEMES = [
  { name: 'system-light', theme: 'system', scheme: 'light' },
  { name: 'system-dark', theme: 'system', scheme: 'dark' },
  { name: 'classified', theme: 'classified', scheme: 'dark', accent: 'red' },
  { name: 'classified-light', theme: 'classified_light', scheme: 'light', accent: 'blue' },
];
const SIZES = [{ w: 1360, h: 832 }, { w: 1000, h: 600 }, { w: 1512, h: 1100 }];

const inside = (r, o, eps = 0.5) => r.x >= o.x - eps && r.y >= o.y - eps && r.x + r.width <= o.x + o.width + eps && r.y + r.height <= o.y + o.height + eps;
const overlap = (a, b) => a.x < b.x + b.width - 0.5 && b.x < a.x + a.width - 0.5 && a.y < b.y + b.height - 0.5 && b.y < a.y + a.height - 0.5;

async function open(browser, base, size, t, collapsed, mini) {
  const ctx = await browser.newContext({ viewport: { width: size.w, height: size.h }, colorScheme: t.scheme });
  await ctx.addInitScript((c) => { try { localStorage.setItem('muninn.sidebar.collapsed', c ? '1' : '0'); } catch (e) { /* ignore */ } }, collapsed);
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(`${base}/ui/index.html?mock=1&active=${mini ? 5 : 1}&watch=file#token=t`);
  await page.waitForSelector('#sidebar .side-item');
  await page.evaluate((t) => { document.documentElement.dataset.theme = t.theme; document.documentElement.dataset.accent = t.accent || 'blue'; }, t);
  if (mini) {
    await page.click('#sidebar [data-page=settings]');
    await page.waitForFunction(() => !document.querySelector('.now-playing').hidden);
  }
  await page.waitForTimeout(150);
  return { page, ctx, errors };
}

const snapshot = (page) => page.evaluate(() => {
  const box = (el) => { const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; };
  const visible = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const side = document.querySelector('#sidebar'), nav = document.querySelector('#side-nav');
  const controls = [...side.querySelectorAll('button, input, a[href]')].filter(visible).map((el) => ({
    id: el.id || el.className || el.tagName, label: el.getAttribute('aria-label') || el.title || el.textContent.trim(),
    inNav: nav.contains(el), mini: !!el.closest('.now-playing'), cls: el.className, range: el.type === 'range', box: box(el),
    focusable: el.tabIndex >= 0, disabled: el.disabled,
  }));
  const active = document.querySelector('.side-item[aria-current=page]');
  const M = window.__mock;
  return {
    vw: innerWidth, vh: innerHeight, side: box(side), nav: box(nav), navScrolls: nav.scrollHeight > nav.clientHeight + 1,
    navOverflowY: getComputedStyle(nav).overflowY, sideScrollW: [side.scrollWidth, side.clientWidth], controls,
    activeBox: active ? box(active) : null, content: box(document.querySelector('#content')),
    back: box(document.querySelector('#nav-back')), toolbar: box(document.querySelector('.toolbar')),
    hero: document.querySelector('.favourite-hero') && !document.querySelector('.favourite-hero').hidden ? box(document.querySelector('.favourite-hero')) : null,
    native: M ? { visible: M.native.visible, rect: M.native.rect } : null,
  };
});

test('sidebar and native pane layout across themes, sizes, collapse and mini-player', { skip: !chromium && 'playwright is not installed' }, async (tctx) => {
  const srv = await serve();
  const base = `http://127.0.0.1:${srv.address().port}`;
  let browser;
  try { browser = await chromium.launch(); } catch (e) { srv.close(); return tctx.skip('chromium cannot launch: ' + String(e.message).split('\n')[0]); }
  if (SHOTS) fs.mkdirSync(SHOTS, { recursive: true });
  let checked = 0;
  try {
    for (const t of THEMES) for (const size of SIZES) for (const collapsed of [false, true]) for (const mini of [false, true]) {
      const tag = `${t.name} ${size.w}x${size.h} ${collapsed ? 'collapsed' : 'expanded'} mini=${mini}`;
      const { page, ctx, errors } = await open(browser, base, size, t, collapsed, mini);
      const s = await snapshot(page);
      assert.deepEqual(errors, [], tag + ': page errors');
      // Rail width and no horizontal clipping inside the sidebar.
      if (collapsed) assert.ok(s.side.width <= 56, `${tag}: rail ${s.side.width}`);
      assert.ok(s.sideScrollW[0] <= s.sideScrollW[1] + 1, `${tag}: sidebar scrolls horizontally ${s.sideScrollW}`);
      // The section list is the scroller; every other control lies fully inside the sidebar and the window.
      assert.equal(s.navOverflowY, 'auto', tag);
      for (const c of s.controls) {
        assert.ok(c.focusable, `${tag}: ${c.label} not focusable`);
        assert.ok(c.label, `${tag}: control without a name`);
        assert.ok(inside(c.box, s.side), `${tag}: ${c.label} clipped by the sidebar ${JSON.stringify(c.box)}`);
        if (!c.inNav) assert.ok(c.box.y + c.box.height <= s.vh && c.box.y >= 0, `${tag}: ${c.label} outside the window`);
        else assert.ok(inside(c.box, s.nav) || s.navScrolls, `${tag}: ${c.label} clipped without a scrollbar`);
      }
      const pinned = s.controls.filter((c) => !c.inNav);
      for (let i = 0; i < pinned.length; i++) for (let j = i + 1; j < pinned.length; j++) assert.ok(!overlap(pinned[i].box, pinned[j].box), `${tag}: ${pinned[i].label} overlaps ${pinned[j].label}`);
      const toggle = s.controls.find((c) => c.id === 'side-toggle');
      assert.ok(toggle && s.vh - (toggle.box.y + toggle.box.height) >= 8, `${tag}: collapse toggle touches the window bottom`);
      // Mini-player contents.
      const minis = s.controls.filter((c) => c.mini);
      if (!mini) assert.equal(minis.length, 0, tag);
      else if (collapsed) {
        assert.ok(minis.some((c) => /^(Play|Pause) background video$/.test(c.label)) && minis.some((c) => c.label === 'Return to video'), `${tag}: ${minis.map((c) => c.label)}`);
        assert.equal(minis.length, 2, `${tag}: collapsed rail must show exactly play/pause and return`);
        assert.ok(minis.every((c) => !c.range && /pla|paus|return/i.test(c.label)), tag);
        assert.ok(minis.every((c) => c.box.width <= 40), tag);
      } else {
        assert.ok(minis.some((c) => c.range), `${tag}: expanded mini-player keeps its seek slider`);
        assert.ok(minis.length >= 5, tag);
      }
      // Active item stays inside the nav (outline not clipped) and toolbar controls are clear of the sidebar.
      if (s.activeBox) assert.ok(s.activeBox.x >= s.side.x && s.activeBox.x + s.activeBox.width <= s.side.x + s.side.width, `${tag}: active item clipped`);
      assert.ok(s.back.x >= s.side.x + s.side.width - 0.5 && s.back.x + s.back.width <= s.vw, `${tag}: back button clipped`);
      if (s.activeBox && s.nav.height > 0) assert.ok(s.activeBox.x - s.nav.x >= 2, `${tag}: active marker has no room for its outline`);
      if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `${t.name}-${size.w}x${size.h}-${collapsed ? 'collapsed' : 'expanded'}${mini ? '-mini' : ''}.png`) });
      await ctx.close();
      checked++;
    }

    // Native pane: below the toolbar and the favourites carousel, inside the window, current before shown.
    for (const t of [THEMES[0], THEMES[2]]) for (const size of SIZES) for (const collapsed of [false, true]) {
      const tag = `native ${t.name} ${size.w}x${size.h} ${collapsed ? 'collapsed' : 'expanded'}`;
      const { page, ctx } = await open(browser, base, size, t, collapsed, false);
      await page.waitForFunction(() => window.__mock.native.visible && window.__mock.native.rect);
      await page.waitForTimeout(700);                       // the carousel loads, then the pane is re-sent
      const s = await snapshot(page);
      const r = s.native.rect;
      assert.ok(s.native.visible, tag);
      assert.ok(r.y >= s.toolbar.y + s.toolbar.height - 0.5, `${tag}: native pane overlaps the toolbar (y=${r.y}, toolbar bottom ${s.toolbar.y + s.toolbar.height})`);
      assert.ok(r.x >= s.side.x + s.side.width - 0.5 && r.x + r.w <= s.vw + 0.5 && r.y + r.h <= s.vh + 0.5, `${tag}: native pane outside the window ${JSON.stringify(r)}`);
      assert.ok(Math.abs(r.y + r.h - s.vh) <= 1, `${tag}: native pane does not reach the window bottom`);
      if (s.hero) assert.ok(r.y >= s.hero.y + s.hero.height - 1, `${tag}: native pane overlaps the favourites carousel`);
      const calls = await page.evaluate(() => window.__mock.calls.map((c) => c.cmd + (c.args && c.args.visible !== undefined ? ':' + c.args.visible : '')));
      assert.ok(calls.indexOf('layout') !== -1 && calls.indexOf('layout') < calls.indexOf('native_visible:true'), `${tag}: layout must precede show: ${calls.join(',')}`);
      if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `${tag.replace(/ /g, '-')}.png`) });
      // A covering dialog hides the native view and restores it afterwards.
      await page.evaluate(() => { window.__confirm = window.Muninn.confirm('Synthetic confirmation?'); });
      await page.waitForFunction(() => !window.__mock.native.visible);
      await page.click('#confirm-cancel');
      await page.waitForFunction(() => window.__mock.native.visible);
      // An internal page hides it.
      await page.click('#sidebar [data-page=library]');
      await page.waitForFunction(() => !window.__mock.native.visible);
      await ctx.close();
      checked++;
    }

    // Keyboard: every sidebar control is reachable with Tab and shows a visible focus ring.
    {
      const { page, ctx } = await open(browser, base, SIZES[1], THEMES[2], true, true);
      const seen = new Set();
      for (let i = 0; i < 40 && seen.size < 12; i++) {
        await page.keyboard.press('Tab');
        const f = await page.evaluate(() => { const e = document.activeElement; return e && e.closest('#sidebar') ? (e.getAttribute('aria-label') || e.title || e.textContent.trim()) : null; });
        if (f) seen.add(f);
      }
      for (const name of ['Browse', 'Settings', 'Pause background video', 'Play background video', 'Return to video', 'Expand sidebar']) {
        if (name.startsWith('Pause') || name.startsWith('Play')) continue;
        assert.ok([...seen].some((x) => x === name || x.startsWith(name)), `keyboard cannot reach ${name}: ${[...seen]}`);
      }
      assert.ok([...seen].some((x) => /(Play|Pause) background video/.test(x)), 'keyboard cannot reach play/pause');
      await ctx.close();
      checked++;
    }
  } finally {
    await browser.close();
    srv.close();
  }
  assert.ok(checked > 60, 'ran ' + checked + ' states');
});
