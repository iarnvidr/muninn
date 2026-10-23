/* Muninn UI shell: tab strip, toolbar, sidebar, the content-area view switch,
 * native web view layout reporting, status area, Restore view.
 * Plain classic scripts, no build step. Other files extend window.Muninn:
 *   watch.js (Watch view), pages.js (internal pages), boot.js (start-up). */
(function () {
  'use strict';
  const L = window.MuninnLib;
  const M = window.Muninn = {
    L,
    token: null,
    origin: location.origin,
    isMock: false,
    page: null,                       // internal page name, or null = Browse
    tabs: { rev: -1, active: null, warm: 3, list: [] },
    setup: null, tools: null, trust: null, offline: false,
    pages: {},                        // filled by pages.js: name -> { title, mount(el), unmount() }
    organization: { tab_groups: {}, collections: {}, tags: {} },
    settings: {},
  };
  M.applyAppearance = () => {
    const theme=M.settings.theme || 'system', accent=M.settings.accent || 'blue';
    document.documentElement.dataset.theme=theme;
    document.documentElement.dataset.accent=accent;
    try { localStorage.setItem('muninn.appearance',JSON.stringify({theme,accent})); } catch(e) { /* private storage may be disabled */ }
  };
  try { const saved=JSON.parse(localStorage.getItem('muninn.appearance')||'null'); if(saved){M.settings={...M.settings,...saved};M.applyAppearance();} } catch(e) { /* ignore stale preferences */ }

  /* ------------------------------------------------------------------ DOM helpers */

  const $ = (sel, root) => (root || document).querySelector(sel);
  const $$ = (sel, root) => Array.from((root || document).querySelectorAll(sel));
  const PROPS = new Set(['value', 'checked', 'disabled', 'hidden', 'selected', 'readOnly', 'type', 'min', 'max', 'step']);

  function h(tag, props, ...kids) {
    const e = document.createElement(tag);
    if (props) {
      for (const k of Object.keys(props)) {
        const v = props[k];
        if (v == null || v === false) continue;
        if (k === 'class') e.className = v;
        else if (k === 'text') e.textContent = v;
        else if (k.startsWith('on') && typeof v === 'function') e.addEventListener(k.slice(2), v);
        else if (PROPS.has(k)) e[k] = v;
        else e.setAttribute(k, v === true ? '' : String(v));
      }
    }
    for (const kid of kids.flat()) {
      if (kid == null || kid === false) continue;
      e.append(kid instanceof Node ? kid : String(kid));
    }
    return e;
  }
  function setText(node, s) { s = s == null ? '' : String(s); if (node.textContent !== s) node.textContent = s; }
  function setCls(node, cls) { if (node.className !== cls) node.className = cls; }
  function setHidden(node, hidden) { if (node.hidden !== !!hidden) node.hidden = !!hidden; }
  function setAttr(node, name, value) { if (value == null || value === false) { if (node.hasAttribute(name)) node.removeAttribute(name); } else { const v = value === true ? '' : String(value); if (node.getAttribute(name) !== v) node.setAttribute(name, v); } }
  Object.assign(M, { $, $$, h, setText, setCls, setHidden, setAttr });

  /* ------------------------------------------------------------------ icons */

  const SVG = (inner, fill) => `<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" focusable="false" fill="${fill ? 'currentColor' : 'none'}" stroke="${fill ? 'none' : 'currentColor'}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${inner}</svg>`;
  const ICONS = {
    plus: SVG('<path d="M12 5v14M5 12h14"/>'),
    back: SVG('<path d="M15 18l-6-6 6-6"/>'),
    forward: SVG('<path d="M9 18l6-6-6-6"/>'),
    reload: SVG('<path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/>'),
    close: SVG('<path d="M7 7l10 10M17 7L7 17"/>'),
    browse: SVG('<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3.2 3.4 3.2 14.6 0 18M12 3c-3.2 3.4-3.2 14.6 0 18"/>'),
    library: SVG('<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M10 9.5v5l4.5-2.5z"/>'),
    feeds: SVG('<path d="M4 6h16M4 12h16M4 18h10"/>'),
    rules: SVG('<path d="M3 5h18l-7 8v6l-4 2v-8z"/>'),
    pulse: SVG('<path d="M3 12h4l3-8 4 16 3-8h4"/>'),
    settings: SVG('<path d="M4 7h9M19 7h1M4 17h1M11 17h9"/><circle cx="16" cy="7" r="2.2"/><circle cx="8" cy="17" r="2.2"/>'),
    collapse: SVG('<path d="M15 18l-6-6 6-6"/>'),
    expand: SVG('<path d="M9 18l6-6-6-6"/>'),
    moon: SVG('<path d="M20 13.5A8.5 8.5 0 1 1 10.5 4a6.7 6.7 0 0 0 9.5 9.5z"/>'),
    play: SVG('<path d="M7 4.5v15l12.5-7.5z"/>', true),
    pause: SVG('<path d="M6.5 4.5h4v15h-4zM13.5 4.5h4v15h-4z"/>', true),
    volume: SVG('<path d="M4 9.5v5h4l5 4v-13l-5 4z" fill="currentColor"/><path d="M16.5 8.5a5 5 0 0 1 0 7"/>'),
    mute: SVG('<path d="M4 9.5v5h4l5 4v-13l-5 4z" fill="currentColor"/><path d="M17 9.5l5 5M22 9.5l-5 5"/>'),
    fullscreen: SVG('<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>'),
    pip: SVG('<rect x="3" y="4" width="18" height="16" rx="2"/><rect x="12" y="12" width="7" height="6" rx="1"/>'),
    loop: SVG('<path d="M4 8h13l-3-3M20 16H7l3 3M20 8v4M4 16v-4"/>'),
    theatre: SVG('<rect x="2" y="6" width="20" height="12" rx="1"/>'),
    fit: SVG('<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5M8 12h8"/>'),
    compatibility: SVG('<path d="M5 3h10l4 4v14H5zM15 3v5h4M8 14l2 2 5-5"/>'),
    skipBack: SVG('<path d="M5 8a8 8 0 1 1-1 8M5 3v5h5"/><text x="8" y="16" font-size="8" stroke="none" fill="currentColor">10</text>'),
    skipForward: SVG('<path d="M19 8a8 8 0 1 0 1 8M19 3v5h-5"/><text x="8" y="16" font-size="8" stroke="none" fill="currentColor">10</text>'),
    search: SVG('<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/>'),
    dot: SVG('<circle cx="12" cy="12" r="4"/>', true),
    channel: SVG('<circle cx="12" cy="8.5" r="3.5"/><path d="M5 20c0-4 3-6 7-6s7 2 7 6"/>'),
  };
  function icon(name) { const s = document.createElement('span'); s.className = 'ico'; s.innerHTML = ICONS[name] || ''; return s; }
  function fillIcons(root) {
    for (const el of $$('[data-icon]', root)) {
      if (el.dataset.iconSet !== el.dataset.icon) { el.innerHTML = ICONS[el.dataset.icon] || ''; el.dataset.iconSet = el.dataset.icon; }
    }
  }
  Object.assign(M, { icon, fillIcons, ICONS });

  /* ------------------------------------------------------------------ Tauri IPC and the service API */

  function invoke(cmd, args) {
    const t = window.__TAURI_INTERNALS__;
    if (!t || !t.invoke) return Promise.reject('Not running inside the Muninn app');
    try { return Promise.resolve(t.invoke(cmd, args)); } catch (e) { return Promise.reject(e); }
  }
  M.invoke = invoke;

  function errText(e) {
    if (e == null) return 'Unknown error';
    if (typeof e === 'string') return e;
    return e.message || String(e);
  }
  M.errText = errText;

  class ApiError extends Error {
    constructor(message, status, data) { super(message); this.status = status; this.data = data; }
  }
  M.ApiError = ApiError;

  async function api(method, path, body, opts) {
    opts = opts || {};
    const headers = { Authorization: 'Bearer ' + (M.token || '') };
    const init = { method, headers };
    if (body !== undefined) { headers['Content-Type'] = 'application/json'; init.body = JSON.stringify(body); }
    if (opts.keepalive) init.keepalive = true;
    let res;
    try { res = await fetch(path, init); } catch (e) { setOffline(true); throw new ApiError('Muninn service not reachable', 0); }
    setOffline(false);
    let data = null;
    const text = await res.text();
    try { data = text ? JSON.parse(text) : null; } catch (e) { data = null; }
    if (!res.ok || (data && !Array.isArray(data) && data.ok === false)) {
      throw new ApiError((data && data.error) || res.statusText || ('HTTP ' + res.status), res.status, data);
    }
    return data;
  }
  M.api = api;
  M.url = (path) => L.withToken(path, M.token);

  function setOffline(v) { if (M.offline !== v) { M.offline = v; renderStatus(); } }

  /* ------------------------------------------------------------------ confirm dialog */

  M.confirm = function (message, okLabel, danger) {
    return new Promise((resolve) => {
      const d = $('#confirm');
      $('#confirm-msg').textContent = message;
      const ok = $('#confirm-ok');
      ok.textContent = okLabel || 'OK';
      ok.className = danger === false ? 'primary' : 'danger';
      d.returnValue = '';
      const onClose = () => { d.removeEventListener('close', onClose); resolve(d.returnValue === 'ok'); };
      d.addEventListener('close', onClose);
      d.showModal();
      $('#confirm-cancel').focus();
    });
  };

  /* ------------------------------------------------------------------ status area (fixed size) */

  const status = { kind: null, text: '', timer: null, action: null };

  /** A transient message in the toolbar status area. kind: info | error */
  M.flash = function (kind, text, ms) {
    clearTimeout(status.timer);
    status.kind = kind; status.text = text;
    renderStatus();
    status.timer = setTimeout(() => { status.kind = null; status.text = ''; renderStatus(); }, ms || (kind === 'error' ? 7000 : 3500));
  };
  M.flashError = (e) => {
    if (e instanceof ApiError && e.status === 0) return;   // the offline banner says it
    M.api('POST', '/api/logs', { event: 'ui.error', message: errText(e) }).catch(() => {});
    M.flash('error', errText(e));
  };
  window.addEventListener('error', (event) => {
    M.api('POST', '/api/logs', { event: 'ui.unhandled', message: event.message || 'Unhandled script error' }).catch(() => {});
  });
  window.addEventListener('unhandledrejection', (event) => {
    M.api('POST', '/api/logs', { event: 'ui.unhandled', message: errText(event.reason) }).catch(() => {});
  });

  function renderStatus() {
    const el = $('#status');
    let kind = 'none', text = '', action = null, progress = null;
    const banner = L.statusBanner(M.setup, M.trust);
    if (status.kind) { kind = status.kind; text = status.text; }
    else if (M.offline) { kind = 'error'; text = 'Cannot reach the Muninn service. Retrying...'; }
    else if (M.settings?.offline_mode) { kind = 'info'; text = 'Offline mode · cached content only'; action = 'Reconnect'; }
    else if (banner) { kind = banner.kind; text = banner.text; action = banner.action; progress = banner.progress; }
    setAttr(el, 'data-kind', kind);
    setText($('#status-text'), text);
    setAttr($('#status-text'), 'title', text || null);
    const btn = $('#status-action');
    setHidden(btn, !action);
    if (action) setText(btn, action);
    status.action = action ? (M.settings?.offline_mode ? 'reconnect' : banner?.kind) : null;
    const fill = $('#status-fill');
    const bar = fill.parentNode;
    const showBar = kind === 'setup' && !status.kind;
    bar.dataset.on = showBar ? '1' : '0';
    bar.dataset.indeterminate = showBar && progress == null ? '1' : '0';
    fill.style.width = progress == null ? '' : Math.round(progress * 100) + '%';
  }

  $('#status-action').addEventListener('click', async () => {
    if (status.action === 'reconnect') {
      try { M.settings = await api('PUT', '/api/settings', {offline_mode:false}); render(); renderStatus(); }
      catch (e) { M.flash('error',errText(e)); }
      return;
    }
    if (status.action !== 'trust' && status.action !== 'error') return;
    M.trust = { state: 'installing' };
    renderStatus();
    try { await invoke('trust_install'); } catch (e) { M.flash('error', 'Could not install trust: ' + errText(e)); }
    pollSystemNow();
  });

  /* ------------------------------------------------------------------ system poll (setup + trust) */

  let sysTimer = null;
  async function pollSystem() {
    clearTimeout(sysTimer);
    try { const r = await api('GET', '/api/setup'); M.setup = r && r.setup; M.tools = r && r.tools; } catch (e) { /* offline banner */ }
    try { M.trust = await invoke('trust_status'); } catch (e) { M.trust = null; }
    try { const offline=M.settings?.offline_mode,developer=L.developerMode(M.settings); M.settings = await api('GET', '/api/settings'); M.applyAppearance(); M.watch?.applySettings(); if(offline!==M.settings.offline_mode||developer!==L.developerMode(M.settings))render(); } catch (e) { /* keep the last playback policy */ }
    await M.refreshOrganization().catch(() => {});
    M.refreshFeatured?.();
    renderStatus();
    const busy = (M.setup && ['installing', 'updating', 'starting'].includes(M.setup.phase)) || (M.trust && M.trust.state === 'installing');
    sysTimer = setTimeout(pollSystem, busy ? 1000 : (M.trust && M.trust.state === 'needed' ? 4000 : 10000));
  }
  function pollSystemNow() { clearTimeout(sysTimer); return pollSystem(); }
  M.pollSystemNow = pollSystemNow;

  /* ------------------------------------------------------------------ tabs */

  function activeTab() { return M.tabs.list.find((t) => t.id === M.tabs.active) || null; }
  M.activeTab = activeTab;

  let tabPollTimer = null, tabPolling = false, lastHandoffRev = 0;
  async function pollTabs() {
    if (tabPolling) return;
    tabPolling = true;
    clearTimeout(tabPollTimer);
    try {
      const s = await invoke('tabs_state');
      if (s && s.rev !== M.tabs.rev) {
        if ((s.handoff_rev || 0) > lastHandoffRev) {
          lastHandoffRev = s.handoff_rev;
          M.watch?.prepareBackground(s.active);
          M.page = null;
        }
        M.tabs.rev = s.rev;
        M.tabs.active = s.active == null ? null : s.active;
        M.tabs.warm = s.warm_tabs || M.tabs.warm;
        M.tabs.list = s.tabs || [];
        render();
      }
    } catch (e) { /* the shell is starting or gone; keep trying */ }
    tabPolling = false;
    tabPollTimer = setTimeout(pollTabs, document.hidden ? 2000 : 500);
  }
  function refreshTabs() { clearTimeout(tabPollTimer); tabPolling = false; return pollTabs(); }
  M.refreshTabs = refreshTabs;

  async function cmd(name, args) {
    try { const r = await invoke(name, args); await refreshTabs(); return r; }
    catch (e) { M.flash('error', errText(e)); refreshTabs(); return undefined; }
  }

  M.activateTab = async function (id) {
    M.watch?.prepareBackground(id);
    M.page = null;
    if (id !== M.tabs.active) { M.tabs.active = id; render(); await cmd('tab_activate', { id }); }
    else render();
  };
  M.closeTab = async function (id) {
    M.watch?.closeTab(id);
    const list = M.tabs.list.filter((t) => t.id !== id);
    if (id === M.tabs.active) M.tabs.active = (list[list.length - 1] || {}).id == null ? null : list[list.length - 1].id;
    M.tabs.list = list;
    render();
    await cmd('tab_close', { id });
  };
  M.newTab = async function (url) {
    M.watch?.prepareBackground();
    M.page = null;
    const args = url ? { url } : {};
    const id = await cmd('tab_new', args);
    render();
    if (!url) { const a = $('#address'); a.focus(); a.select(); }
    return id;
  };
  M.hibernateTab = async function (id) {
    const t = M.tabs.list.find((x) => x.id === id);
    if (!t) return;
    if (!L.canHibernate(t)) { M.flash('info', t.face === 'watch' ? 'A video tab has nothing to put to sleep' : (t.state === 'restoring' ? 'That tab is waking up' : 'That tab is already asleep')); return; }
    await cmd('tab_hibernate', { id });
    M.flash('info', 'Put to sleep: ' + L.tabTitle(t));
  };
  M.go = async function (input) {
    input = String(input || '').trim();
    if (!input) return;
    M.page = null;
    let tab = activeTab();
    let id = tab && tab.id;
    if (!tab) { id = await invoke('tab_new', {}).catch((e) => { M.flash('error', errText(e)); return null; }); if (id == null) return; }
    addrPending = { tabId: id, url: tab ? tab.url : '', until: Date.now() + 4000 };
    render();
    M.watch?.prepareBackground();
    await cmd('tab_go', { id, input });
  };
  /** Local cards preserve the browsing tab and open a dedicated player tab. */
  M.openVideo = async function (videoId, background) {
    if (!videoId) return;
    if (!background) {const previous={page:M.page,tab:M.tabs.active};const id=await M.newTab('https://www.youtube.com/watch?v='+encodeURIComponent(videoId));M.watchReturns=M.watchReturns||new Map();M.watchReturns.set(id,previous);renderToolbar();return id;}
    let tab = activeTab();
    let id = tab && tab.id;
    if (!tab) { id = await invoke('tab_new', {}).catch((e) => { M.flash('error', errText(e)); return null; }); if (id == null) return; }
    render();
    await cmd('tab_open_video', { id, video_id: videoId, background: !!background });
  };
  M.openChannel = (channelId) => { if (channelId) M.newTab(L.channelUrl(channelId)); };

  /* ---- rendering the tab strip (keyed, so hover/focus survive polls) ---- */

  const tabEls = new Map();
  const groupEls = new Map();
  M.refreshOrganization = async () => {
    const org = await api('GET', '/api/organization');
    if (JSON.stringify(org) !== JSON.stringify(M.organization)) { M.organization = org; renderTabs(); }
    return org;
  };
  M.organize = async (change) => { M.organization = await api('POST', '/api/organization', change); renderTabs(); M.refreshLibraryGroups?.(); return M.organization; };
  M.tabGroup = id => Object.keys(M.organization.tab_groups).find(name=>M.organization.tab_groups[name].tab_ids.includes(String(id))) || '';
  M.orderedTabs = () => {
    const order=M.organization.tab_order || [];
    return M.tabs.list.slice().sort((a,b)=>{const ga=M.tabGroup(a.id),gb=M.tabGroup(b.id);if(ga!==gb)return ga.localeCompare(gb);const ids=ga?M.organization.tab_groups[ga].tab_ids:order;const rank=id=>{const i=ids.indexOf(String(id));return i<0?10000+M.tabs.list.findIndex(t=>t.id===id):i;};return rank(a.id)-rank(b.id);});
  };
  M.dragTab = (el,id) => {
    el.draggable=true;
    el.addEventListener('dragstart',event=>{M.draggedTab=String(id);event.dataTransfer.setData('application/x-muninn-tab',String(id));event.dataTransfer.setData('text/plain',String(id));event.dataTransfer.effectAllowed='move';el.classList.add('dragging');document.body.classList.add('tab-dragging');});
    el.addEventListener('dragend',()=>{M.draggedTab=null;el.classList.remove('dragging');document.body.classList.remove('tab-dragging');document.querySelectorAll('.tab-drop-target').forEach(n=>n.classList.remove('tab-drop-target'));});
  };
  M.dropTabs = (el,group,before,done) => {
    el.addEventListener('dragover',event=>{if(!M.draggedTab)return;event.preventDefault();event.stopPropagation();event.dataTransfer.dropEffect='move';el.classList.add('tab-drop-target');});
    el.addEventListener('dragleave',event=>{if(!el.contains(event.relatedTarget))el.classList.remove('tab-drop-target');});
    el.addEventListener('drop',async event=>{const id=M.draggedTab;if(!id)return;event.preventDefault();event.stopPropagation();el.classList.remove('tab-drop-target');M.draggedTab=null;if(String(before?.())===id)return;try{await M.organize({kind:'tab',action:'move',name:typeof group==='function'?group():group,ids:[id],before_id:before?.() || null});done?.();}catch(e){M.flashError(e);}});
  };
  M.dropTabs($('#tab-ungroup'),'');

  function buildTab(id) {
    const lead = h('span', { class: 'tab-lead' });
    const title = h('span', { class: 'tab-title' });
    const main = h('button', { type: 'button', class: 'tab-main', role: 'tab', onclick: () => M.activateTab(id), onauxclick: (ev) => { if (ev.button === 1) { ev.preventDefault(); M.closeTab(id); } } }, lead, title);
    const zzz = h('button', { type: 'button', class: 'tab-zzz', tabindex: '-1', onclick: (ev) => { ev.stopPropagation(); M.hibernateTab(id); } });
    zzz.append(icon('moon'));
    const close = h('button', { type: 'button', class: 'tab-close', tabindex: '-1', onclick: (ev) => { ev.stopPropagation(); M.closeTab(id); } });
    close.append(icon('close'));
    const el = h('div', { class: 'tab', 'data-id': id, oncontextmenu: (ev) => { ev.preventDefault(); M.hibernateTab(id); } }, main, zzz, close);
    M.dragTab(el,id); M.dropTabs(el,()=>M.tabGroup(id),()=>String(id));
    return { id, el, main, lead, title, zzz, close, leadKind: '' };
  }

  function leadKind(tab) {
    if (tab.state === 'loading' || tab.state === 'restoring') return 'spin';
    if (tab.state === 'sleeping') return 'moon';
    if (tab.face === 'watch') return 'play';
    if (tab.kind === 'search') return 'search';
    if (tab.kind === 'channel') return 'channel';
    return 'dot';
  }

  function renderTabs() {
    const strip = $('#tabstrip');
    const keep = new Set();
    const grouped = new Map();
    for (const [name, group] of Object.entries(M.organization.tab_groups)) for (const id of group.tab_ids) grouped.set(String(id), name);
    const list = M.orderedTabs();
    const nodes = [], added = new Set();
    list.forEach((tab) => {
      const name = grouped.get(String(tab.id));
      const group = name && M.organization.tab_groups[name];
      if (name && !added.has(name)) {
        added.add(name);
        let label = groupEls.get(name);
        if (!label) {
          label = h('button', { class: 'tab-group-label', onclick: () => M.organize({ kind: 'tab_group', action: 'collapse', name, collapsed: !M.organization.tab_groups[name].collapsed }).catch(M.flashError) });
          groupEls.set(name, label);
          M.dropTabs(label,name);
        }
        setText(label, (group.collapsed ? '▸ ' : '▾ ') + name + ' · ' + list.filter((t) => grouped.get(String(t.id)) === name).length);
        setAttr(label, 'aria-expanded', String(!group.collapsed)); nodes.push(label);
      }
      keep.add(tab.id);
      let t = tabEls.get(tab.id);
      if (!t) { t = buildTab(tab.id); tabEls.set(tab.id, t); }
      const active = tab.id === M.tabs.active;
      const title = L.tabTitle(tab);
      setText(t.title, title);
      const cls = 'tab' + (active ? ' active' : '') + (tab.state === 'sleeping' ? ' sleeping' : '') + (tab.face === 'watch' ? ' watch' : '') + ((tab.state === 'loading' || tab.state === 'restoring') ? ' busy' : '');
      setCls(t.el, cls);
      const stateWord = tab.state === 'sleeping' ? ' (sleeping)' : (tab.state === 'restoring' ? ' (waking)' : '');
      setAttr(t.el, 'title', title + stateWord);
      setAttr(t.main, 'aria-selected', active ? 'true' : 'false');
      setAttr(t.main, 'tabindex', active ? '0' : '-1');
      setAttr(t.main, 'aria-label', title + stateWord);
      const lk = leadKind(tab);
      if (lk !== t.leadKind) {
        t.leadKind = lk;
        t.lead.replaceChildren(lk === 'spin' ? h('span', { class: 'spinner' }) : icon(lk));
      }
      setHidden(t.zzz, !L.canHibernate(tab));
      setAttr(t.zzz, 'aria-label', 'Put "' + title + '" to sleep');
      setAttr(t.zzz, 'title', 'Put to sleep');
      setAttr(t.close, 'aria-label', 'Close "' + title + '"');
      setAttr(t.close, 'title', 'Close tab');
      setHidden(t.el, !!(group?.collapsed && !active));
      nodes.push(t.el);
    });
    for (const [id, t] of tabEls) if (!keep.has(id)) { t.el.remove(); tabEls.delete(id); }
    if (nodes.length !== strip.children.length || nodes.some((n, i) => strip.children[i] !== n)) strip.replaceChildren(...nodes);
    if (!M.tabs.list.length) strip.dataset.empty = '1'; else delete strip.dataset.empty;
    if (strip.dataset.active !== String(M.tabs.active)) {
      strip.dataset.active = String(M.tabs.active);
      tabEls.get(M.tabs.active)?.el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }

  $('#tabstrip').addEventListener('keydown', (ev) => {
    const mains = $$('.tab-main', $('#tabstrip')).filter((n) => !n.parentNode.hidden);
    const i = mains.indexOf(document.activeElement);
    if (i < 0) return;
    let n = -1;
    if (ev.key === 'ArrowRight') n = (i + 1) % mains.length;
    else if (ev.key === 'ArrowLeft') n = (i - 1 + mains.length) % mains.length;
    else if (ev.key === 'Home') n = 0;
    else if (ev.key === 'End') n = mains.length - 1;
    else if (ev.key === 'Delete' || ev.key === 'Backspace') { ev.preventDefault(); M.closeTab(Number(mains[i].parentNode.dataset.id)); return; }
    if (n >= 0) { ev.preventDefault(); mains[n].focus(); M.activateTab(Number(mains[n].parentNode.dataset.id)); }
  });
  $('#tab-new').addEventListener('click', () => M.newTab());
  $('#empty-new').addEventListener('click', () => M.newTab());

  /* ------------------------------------------------------------------ toolbar and address bar */

  const addr = $('#address');
  let addrDirty = false, addrPending = null;

  function syncAddress(force) {
    if (!force && document.activeElement === addr && addrDirty) return;
    const tab = activeTab();
    if (addrPending) {
      if (tab && tab.id === addrPending.tabId && tab.url === addrPending.url && Date.now() < addrPending.until) return;
      addrPending = null;
    }
    const text = L.addressText(tab);
    if (addr.value !== text) addr.value = text;
  }
  addr.addEventListener('input', () => { addrDirty = true; });
  addr.addEventListener('focus', () => { addrDirty = false; setTimeout(() => addr.select(), 0); });
  addr.addEventListener('blur', () => { addrDirty = false; syncAddress(); });
  addr.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { addrDirty = false; syncAddress(true); addr.select(); }
  });
  $('#address-form').addEventListener('submit', (ev) => {
    ev.preventDefault();
    const v = addr.value.trim();
    if (!v) return;
    addrDirty = false;
    addr.blur();
    M.go(v);
  });

  function navAction(name) {
    const tab = activeTab();
    if (!tab) return;
    if(M.page){
      if(name==='tab_reload'){M.pages[mountedPage]?.unmount();mountedPage=null;views.internal.replaceChildren();renderView();}
      else if(name==='tab_back')M.activateTab(tab.id);
      return;
    }
    if(tab.face==='watch'&&name==='tab_back'){
      const previous=M.watchReturns?.get(tab.id);
      if(previous?.page){M.watch.prepareBackground();M.page=previous.page;render();return;}
      const browse=M.tabs.list.find(t=>t.id===(previous?.tab||M.lastBrowseTab)&&t.face!=='watch')||M.tabs.list.find(t=>t.face!=='watch');
      if(browse)M.activateTab(browse.id);else M.newTab();return;
    }
    M.page = null;
    if (name === 'tab_reload' && tab.face === 'watch') { render(); if (M.watch) M.watch.retry(); return; }
    render();
    if (name === 'tab_back' || name === 'tab_forward') M.watch?.prepareBackground();
    cmd(name, { id: tab.id });
  }
  $('#nav-back').addEventListener('click', () => navAction('tab_back'));
  $('#nav-forward').addEventListener('click', () => navAction('tab_forward'));
  $('#nav-reload').addEventListener('click', () => navAction('tab_reload'));

  function renderToolbar() {
    const tab = activeTab();
    const browsing=!M.page&&tab?.face!=='watch',player=!M.page&&tab?.face==='watch';
    $('#address-form').hidden=!browsing;$('#view-context').hidden=browsing;
    setText($('#view-context'),M.page?M.pages[M.page]?.title||M.page:'Player · '+(tab?.title||''));
    $('#nav-back').disabled=!tab||(browsing&&!tab.can_back);
    const backTitle=browsing?'Back (Cmd+[)':player?'Return to previous view':'Return to tab';
    setAttr($('#nav-back'),'title',backTitle);setAttr($('#nav-back'),'aria-label',backTitle);
    $('#nav-forward').hidden=!browsing;
    $('#nav-forward').disabled = !tab || !tab.can_forward;
    $('#nav-reload').disabled = !tab;
    const reloadTitle=browsing?'Reload (Cmd+R)':player?'Restart playback':'Refresh '+(M.pages[M.page]?.title||'page');
    setAttr($('#nav-reload'),'title',reloadTitle);setAttr($('#nav-reload'),'aria-label',reloadTitle);
    addr.disabled = false;
    syncAddress();
  }

  /* ------------------------------------------------------------------ sidebar */

  const collapsedKey = 'muninn.sidebar.collapsed';
  function setSidebar(collapsed, persist) {
    const app = $('#app');
    app.dataset.sidebar = collapsed ? 'collapsed' : 'expanded';
    const t = $('#side-toggle');
    t.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
    const label = collapsed ? 'Expand sidebar' : 'Collapse sidebar';
    t.title = label; t.setAttribute('aria-label', label);
    setText($('.side-label', t), collapsed ? 'Expand' : 'Collapse');
    const ico = $('.side-icon', t);
    ico.dataset.icon = collapsed ? 'expand' : 'collapse';
    fillIcons(t);
    if (persist) { try { localStorage.setItem(collapsedKey, collapsed ? '1' : '0'); } catch (e) { /* ignore */ } }
    scheduleLayout();
  }
  $('#side-toggle').addEventListener('click', () => setSidebar($('#app').dataset.sidebar !== 'collapsed', true));
  function onSidebarClick(b) {
    M.watch?.prepareBackground();
    const p = b.dataset.page;
    if (p==='browse') {
      const tab=M.tabs.list.find(t=>t.id===M.lastBrowseTab && t.face!=='watch') || M.tabs.list.find(t=>t.face!=='watch');
      if(tab) M.activateTab(tab.id); else M.newTab('https://www.youtube.com/');
      return;
    }
    if(p==='player') {
      const tab=M.tabs.list.find(t=>t.id===M.watch?.state()?.tabId) || M.tabs.list.find(t=>t.face==='watch');
      if(tab) M.activateTab(tab.id); else M.flash('info','Open a video from Library or YouTube to start playing.');
      return;
    }
    M.page = L.resolvePage(p === 'browse' ? null : p, M.settings);
    render();
  }
  for (const b of $$('#sidebar [data-page]')) b.addEventListener('click', () => onSidebarClick(b));
  /* Diagnostics is a developer-mode page: its button exists only while the mode is on. */
  function syncDeveloperNav() {
    const existing = $('#sidebar [data-page="diagnostics"]');
    if (!L.developerMode(M.settings)) { existing?.remove(); return; }
    if (existing) return;
    const b = h('button', { type: 'button', class: 'side-item', 'data-page': 'diagnostics', title: 'Diagnostics' }, h('span', { class: 'side-icon', 'data-icon': 'pulse' }), h('span', { class: 'side-label', text: 'Diagnostics' }));
    b.addEventListener('click', () => onSidebarClick(b));
    $('#sidebar [data-page="settings"]').before(b);
    fillIcons(b);
  }
  M.syncDeveloperNav = syncDeveloperNav;
  function renderSidebar() {
    syncDeveloperNav();
    const tab=activeTab();
    if(tab?.face!=='watch' && tab) M.lastBrowseTab=tab.id;
    for (const b of $$('#sidebar [data-page]')) {
      const on = (M.page || (tab?.face==='watch'?'player':'browse')) === b.dataset.page;
      setAttr(b, 'aria-current', on ? 'page' : null);
    }
    $('#app').dataset.pageOpen = M.page ? '1' : '0';
  }

  /* ------------------------------------------------------------------ layout + native view visibility */

  const content = $('#content');
  const hero=h('section',{class:'favourite-hero',hidden:true,'aria-label':'New from your favourites'});
  const heroCards=h('div',{class:'favourite-carousel'});
  const moveHero=direction=>heroCards.scrollBy({left:direction*Math.max(280,heroCards.clientWidth*0.8),behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'instant':'smooth'});
  hero.append(h('header',null,h('div',null,h('h2',{text:'New from your favourites'}),h('span',{class:'muted',text:'Picked from your favourite channel groups'})),h('div',{class:'actions'},h('button',{'aria-label':'Previous favourite videos',onclick:()=>moveHero(-1)},M.icon('back')),h('button',{'aria-label':'Next favourite videos',onclick:()=>moveHero(1)},M.icon('forward')))),heroCards);
  content.prepend(hero);
  let featuredItems=[],featuredSignature='',featuredLoading=false,featuredAt=0;
  function renderFeatured(){
    const tab=activeTab();
    const visible=!M.page&&tab?.face!=='watch'&&!!tab&&(tab.kind==='home'||/^https:\/\/(www\.)?youtube\.com\/?(?:\?.*)?$/.test(tab.url||''))&&M.settings?.favourite_carousel!==false&&featuredItems.length>0;
    setHidden(hero,!visible);
    const height=visible?hero.getBoundingClientRect().height:0;
    views.restore.style.top=height+'px';
    scheduleLayout();
  }
  M.refreshFeatured=async function(force=false){
    if(featuredLoading||(!force&&Date.now()-featuredAt<15000))return;
    featuredLoading=true;
    try{
      const result=await api('GET','/api/featured'),items=result.items||[],signature=JSON.stringify(items);
      featuredItems=items;featuredAt=Date.now();
      if(signature!==featuredSignature){featuredSignature=signature;heroCards.replaceChildren(...items.map(item=>M.videoCard(item,v=>M.openVideo(v.video_id))));}
      renderFeatured();
    }catch(e){/* keep cached featured cards if the local service restarts */}
    finally{featuredLoading=false;}
  };
  let lastRect = null, lastVisible = null, layoutRaf = 0;

  function sendLayout(force) {
    const r = L.roundRect(content.getBoundingClientRect());
    if(!hero.hidden){const height=Math.ceil(hero.getBoundingClientRect().height);r.y+=height;r.h=Math.max(1,r.h-height);}
    if (!force && L.sameRect(r, lastRect)) return;
    lastRect = r;
    M.layoutCalls = (M.layoutCalls || 0) + 1;
    invoke('layout', {...r,viewport_width:window.innerWidth,viewport_height:window.innerHeight}).catch(() => {});
  }
  function scheduleLayout() {
    if (layoutRaf) return;
    layoutRaf = requestAnimationFrame(() => { layoutRaf = 0; sendLayout(false); });
  }
  M.scheduleLayout = scheduleLayout;

  function setNativeVisible(v) {
    if (v === lastVisible) return;
    if (v) sendLayout(true);           // the rect is current before the view appears
    lastVisible = v;
    invoke('native_visible', { visible: v }).catch(() => {});
  }

  if (window.ResizeObserver) new ResizeObserver(scheduleLayout).observe(content);
  window.addEventListener('resize', scheduleLayout);

  /* ------------------------------------------------------------------ view switching */

  const views = { empty: $('#view-empty'), restore: $('#view-restore'), watch: $('#view-watch'), internal: $('#view-internal') };
  let mountedPage = null, lastView = null;

  function render() {
    // Developer-only pages close when the mode goes off (the page's pollers stop on unmount).
    const allowed = L.resolvePage(M.page, M.settings);
    if (allowed !== M.page) M.page = allowed;
    renderStatus();
    renderTabs();
    renderToolbar();
    renderSidebar();
    renderView();
  }
  M.render = render;

  function renderView() {
    const tab = activeTab();
    let view = L.chooseView(M.page, tab);
    if(M.settings?.offline_mode && view==='native')view='restore';
    renderFeatured();
    if(!M.page&&tab?.face!=='watch')M.refreshFeatured();
    // Hide the native layer BEFORE showing anything of ours, show it AFTER everything else is hidden.
    if (view !== 'native') setNativeVisible(false);
    if (M.watch) M.watch.sync(view === 'watch' ? tab : null);
    const mini=!!M.watch?.state()?.background;
    for (const k of Object.keys(views)) setHidden(views[k], k !== view && !(k==='watch' && mini));
    views.watch.classList.toggle('in-background',mini);

    if (mountedPage !== (view === 'internal' ? M.page : null)) {
      if (mountedPage && M.pages[mountedPage]) M.pages[mountedPage].unmount();
      mountedPage = null;
      if (view === 'internal') {
        const p = M.pages[M.page];
        views.internal.replaceChildren();
        views.internal.scrollTop = 0;
        if (p) { p.mount(views.internal); mountedPage = M.page; fillIcons(views.internal); }
      }
    }
    if (view === 'restore') syncRestore(tab); else resetRestore();
    if (view === 'native') setNativeVisible(true);
    if (view !== lastView) { lastView = view; scheduleLayout(); }
  }

  /* ------------------------------------------------------------------ shared building blocks */

  M.thumbImg = function (path, cls) {
    const img = h('img', { class: cls || '', alt: '', decoding: 'async', draggable: 'false' });
    img.addEventListener('error', () => img.classList.add('missing'));
    img.addEventListener('load', () => img.classList.remove('missing'));
    if (path) img.src = M.url(path);
    return img;
  };

  /** A video card (Restore, Feeds). `item` is an /api item (video_id, title, channel, cached...). */
  M.videoCard = function (item, onOpen) {
    const v = L.itemView(item);
    const dur = v.duration ? h('span', { class: 'vdur', text: v.duration }) : null;
    const badge = v.cache ? h('span', { class: 'cbadge ' + v.cache.cls, text: v.cache.label }) : null;
    const resume = v.resume > 0.01 ? h('span', { class: 'vresume' }, h('i', { style: null })) : null;
    if (resume) resume.firstChild.style.width = Math.round(v.resume * 100) + '%';
    const btn = h('a', { href: 'https://www.youtube.com/watch?v=' + v.videoId, class: 'vcard', 'aria-label': 'Play ' + v.title + (v.channel ? ' by ' + v.channel : ''), title: v.title },
      h('span', { class: 'vthumb' }, M.thumbImg(v.thumb), dur, badge, resume),
      h('span', { class: 'vtitle', text: v.title }),
      h('span', { class: 'vchan', text: v.channel || ' ' }),
      h('span', { class: 'vmeta', text: v.meta || ' ' }));
    M.bindVideoLink(btn, v.videoId, () => onOpen(item));
    return btn;
  };

  M.bindVideoLink = function (el, videoId, open) {
    el.addEventListener('click', (ev) => {
      ev.preventDefault();
      if (ev.metaKey || ev.ctrlKey) M.openVideo(videoId, true);
      else if (ev.shiftKey) M.newTab('https://www.youtube.com/watch?v=' + videoId);
      else open();
    });
    el.addEventListener('auxclick', (ev) => { if (ev.button === 1) { ev.preventDefault(); M.openVideo(videoId, true); } });
    el.addEventListener('contextmenu', (ev) => {
      ev.preventDefault();
      document.querySelector('.video-menu')?.remove();
      const close = () => menu.remove();
      const menu = h('div', { class: 'video-menu', role: 'menu' },
        h('button', { text: 'Open video', role: 'menuitem', onclick: () => { close(); open(); } }),
        h('button', { text: 'Open in new tab', role: 'menuitem', onclick: () => { close(); M.openVideo(videoId, true); } }),
        h('button', { text: 'Copy video URL', role: 'menuitem', onclick: () => { close(); navigator.clipboard.writeText('https://www.youtube.com/watch?v=' + videoId).catch(M.flashError); } }));
      document.body.append(menu);
      menu.style.left = Math.max(0, Math.min(ev.clientX, innerWidth - menu.offsetWidth)) + 'px';
      menu.style.top = Math.max(0, Math.min(ev.clientY, innerHeight - menu.offsetHeight)) + 'px';
      menu.firstChild.focus();
      const dismiss = (e) => { if (!menu.contains(e.target)) close(); };
      setTimeout(() => document.addEventListener('pointerdown', dismiss, { once: true }), 0);
      menu.addEventListener('keydown', (e) => { if (e.key === 'Escape') { close(); el.focus(); } });
    });
  };

  /* ------------------------------------------------------------------ Restore view */

  const restore = { key: '', data: null, state: 'idle', error: '', sig: '', els: null };

  function buildRestore() {
    const el = views.restore;
    const title = h('h1', { class: 'h1 restore-title' });
    const sub = h('p', { class: 'muted restore-sub' });
    const load = h('button', { type: 'button', class: 'primary restore-load', text: 'Load live page' });
    const wake = h('span', { class: 'slot restore-wake', role: 'status', 'aria-live': 'polite' });
    const grid = h('div', { class: 'vgrid' });
    const note = h('p', { class: 'empty-note' });
    el.replaceChildren(
      h('div', { class: 'restore-head' }, h('div', { class: 'restore-titles' }, title, sub), h('div', { class: 'restore-actions' }, wake, load)),
      note, grid);
    restore.els = { title, sub, load, wake, grid, note };
    load.addEventListener('click', () => { const t = activeTab(); if (t) cmd('tab_activate', { id: t.id }); });
  }

  function resetRestore() { if (restore.key) { restore.key = ''; restore.sig = ''; restore.state = 'idle'; } }

  function restoreKey(tab) { return [tab.id, tab.kind, tab.query || '', tab.url].join('|'); }

  async function loadRestore(tab, key) {
    restore.state = 'loading';
    try {
      let data = null;
      if (tab.kind === 'search') {
        const list = await api('GET', '/api/feeds?' + new URLSearchParams({ kind: 'search', limit: '40' }));
        const snap = L.pickSnapshot(list, tab);
        if (snap) data = await api('GET', '/api/feeds/' + encodeURIComponent(snap.id));
      } else {
        data = await api('GET', '/api/feeds/latest?kind=' + encodeURIComponent(tab.kind || 'home'));
      }
      if (restore.key !== key) return;
      restore.data = data; restore.state = 'ready'; restore.error = '';
    } catch (e) {
      if (restore.key !== key) return;
      restore.data = null;
      restore.state = e instanceof ApiError && e.status === 404 ? 'empty' : 'error';
      restore.error = errText(e);
    }
    const t = activeTab();
    if (t && (L.chooseView(M.page, t) === 'restore' || (M.settings?.offline_mode&&!M.page&&t.face!=='watch'))) syncRestore(t);
  }

  function syncRestore(tab) {
    if (!restore.els) buildRestore();
    const key = restoreKey(tab);
    if (restore.key !== key) {
      restore.key = key; restore.data = null; restore.sig = ''; restore.state = 'loading';
      restore.els.grid.replaceChildren();
      loadRestore(tab, key);
    }
    const E = restore.els;
    const waking = tab.state === 'restoring' || tab.state === 'loading';
    setText(E.title, L.tabTitle(tab) + ' - restored from cache');
    const d = restore.data;
    setText(E.sub, d ? `${(d.items || []).length} videos, captured ${L.relTime(d.captured_at)}${d.stale ? ' (stale)' : ''}` : (restore.state === 'loading' ? 'Reading the cache...' : ''));
    E.load.disabled = waking || !!M.settings?.offline_mode;
    setText(E.wake, waking ? 'Loading live page...' : '');
    E.wake.dataset.on = waking ? '1' : '0';
    let note = '';
    if (restore.state === 'empty') note = 'Nothing from this page is cached yet. Load the live page to capture it.';
    else if (restore.state === 'error') note = 'Could not read the cache: ' + restore.error;
    else if (d && !(d.items || []).length) note = 'The cached snapshot has no videos.';
    setText(E.note, note);
    setHidden(E.note, !note);
    if (d) {
      const sig = d.id + '|' + (d.items || []).map((i) => i.video_id + ':' + (i.cached ? i.cached.progress : '')).join(',');
      if (sig !== restore.sig) {
        restore.sig = sig;
        E.grid.replaceChildren(...(d.items || []).map((it) => M.videoCard(it, (x) => M.openVideo(x.video_id))));
      }
    }
  }

  /* ------------------------------------------------------------------ keyboard shortcuts */

  let shortcutLast = { act: '', t: 0 };
  /** Called by the native menu (Cmd+L). */
  window.muninnFocusAddress = function () { addr.focus(); addr.select(); };
  window.addEventListener('keydown', (ev) => {
    const act = L.shortcutAction(ev);
    if (!act) return;
    const tab = activeTab();
    ev.preventDefault();
    // The macOS menu owns these natively; this is the fallback. Never fire twice for one press.
    const nowMs = Date.now();
    if (shortcutLast.act === act && nowMs - shortcutLast.t < 80) return;
    shortcutLast = { act, t: nowMs };
    switch (act) {
      case 'new-tab': M.newTab(); break;
      case 'close-tab': if (tab) M.closeTab(tab.id); break;
      case 'focus-address': if(M.page||tab?.face==='watch')M.newTab();else {addr.focus();addr.select();} break;
      case 'reload': navAction('tab_reload'); break;
      case 'back': if (tab && (M.page||tab.face==='watch'||tab.can_back)) navAction('tab_back'); break;
      case 'forward': if (tab && tab.can_forward) navAction('tab_forward'); break;
    }
  }, true);

  /* ------------------------------------------------------------------ start */

  M.start = async function () {
    fillIcons(document);
    try { setSidebar(localStorage.getItem(collapsedKey) === '1', false); } catch (e) { setSidebar(false, false); }
    render();
    pollSystem();
    await pollTabs();
    scheduleLayout();
    sendLayout(true);
  };
})();
