/* Muninn internal pages: Library, Feeds, Rules, Settings, and the developer-mode Diagnostics page.
 * Each page is { mount(el), unmount() } registered in Muninn.pages. */
(function () {
  'use strict';
  const M = window.Muninn;
  const L = M.L;
  const { h, $, $$, setText, setHidden, setCls, setAttr, icon } = M;

  /* ------------------------------------------------------------------ shared */

  /** A poll loop that runs while `alive()` and never overlaps itself. */
  function poller(fn, delayFn) {
    let t = null, busy = false, stopped = false;
    async function tick() {
      clearTimeout(t); t = null;
      if (stopped || busy) return;
      busy = true;
      try { await fn(); } catch (e) { /* surfaced by M.api (offline banner) */ }
      busy = false;
      if (!stopped) t = setTimeout(tick, delayFn());
    }
    return { kick: tick, stop() { stopped = true; clearTimeout(t); } };
  }

  /** A fixed-width message slot that clears itself. */
  function slot(cls) {
    const el = h('span', { class: 'slot ' + (cls || ''), role: 'status', 'aria-live': 'polite' });
    let t = null;
    el.say = (text, kind, ms) => {
      clearTimeout(t);
      el.textContent = text || '';
      el.title = text || '';
      if (kind) el.dataset.kind = kind; else delete el.dataset.kind;
      if (text && ms !== 0) t = setTimeout(() => { el.textContent = ''; el.title = ''; delete el.dataset.kind; }, ms || 5000);
    };
    return el;
  }

  function pageHead(title, sub, ...extra) {
    const sections={Library:'01 / Your archive',Feeds:'02 / Discovery',Rules:'03 / Automation',Diagnostics:'04 / System',Settings:'05 / Preferences'};
    return h('div', { class: 'page-head' }, h('div', null, h('p',{class:'design-eyebrow',text:sections[title]||'Muninn / Library'}),h('h1', { class: 'h1', text: title }), sub ? h('p', { class: 'muted page-sub', text: sub }) : null), ...extra);
  }

  function badge(text, cls) { return h('span', { class: 'badge ' + (cls || ''), text }); }

  const DASH = '-';
  const fmtTs = (ms) => (ms == null ? DASH : new Date(ms).toLocaleTimeString());
  const fmtDate = (ms) => (ms == null ? DASH : new Date(ms).toLocaleString());

  /* ================================================================== Library */

  function libraryPage() {
    let instance = null,host=null;
    const content = h('div', { class: 'library-section' });
    const nav = h('div', { class: 'library-nav', role: 'group', 'aria-label': 'Library sections' });
    const sections = [['videos','Videos'],['collections','Collections & tags'],['tabs','Tab groups'],['channels','Channels'],['cleanup','Playback & cleanup']];
    function select(key) {
      instance?.unmount(); content.replaceChildren(); M.librarySection = key;
      for (const button of nav.children) button.setAttribute('aria-pressed', String(button.dataset.section === key));
      instance = key === 'videos' ? videosPage() : key === 'channels' ? M.channelsPanel() : key === 'cleanup' ? M.cleanupPanel() : M.organizationPanel(key);
      instance.mount(content);
    }
    for (const [key,label] of sections) nav.append(h('button', { text:label,'data-section':key,onclick:()=>select(key) }));
    return {
      mount(el) {host=el;el.classList.add('library-workspace');el.append(h('div',{class:'library-topline'},pageHead('Library'),nav),content); select(M.librarySection || 'videos'); },
      unmount() { instance?.unmount();host?.classList.remove('library-workspace'); },
    };
  }

  function videosPage() {
    let savedMode = 'manage';
    try { savedMode = localStorage.getItem('muninn.library-mode') === 'browse' ? 'browse' : 'manage'; } catch (e) { /* storage may be disabled */ }
    const S = { entries: [], filter: 'all', text: '', mode: savedMode, limit: 36, rows: new Map(), cards: new Map() };
    const root = h('div', { class: 'library-videos' });
    const modes = h('div', { class: 'library-modes', role: 'group', 'aria-label': 'Library view' });
    for (const [key, label, glyph] of [['browse', 'Browse', 'library'], ['manage', 'Manage', 'feeds']]) {
      modes.append(h('button', { type: 'button', 'data-mode': key, 'aria-label': label + ' library', onclick: () => {
        S.mode = key; S.text = search.value.trim();
        try { localStorage.setItem('muninn.library-mode', key); } catch (e) { /* optional preference */ }
        renderRows();
      } }, icon(glyph), label));
    }
    const changed = () => { S.limit = 36; renderRows(); };
    const collection = h('select', { 'aria-label': 'Filter collection', onchange: changed }, h('option', { value: '', text: 'All collections' }));
    const tag = h('select', { 'aria-label': 'Filter tag', onchange: changed }, h('option', { value: '', text: 'All tags' }));
    const sort = h('select', { 'aria-label': 'Sort library', onchange: changed },
      ...[['recent', 'Recently added'], ['relevance', 'Relevance'], ['title', 'Title A–Z'], ['channel', 'Channel A–Z'], ['size', 'Largest first'], ['watched', 'Watch progress']].map(([value, text]) => h('option', { value, text })));
    let pol = null;
    const meters = h('div', { class: 'meters' });
    const meterEls = {};
    for (const key of ['cache', 'library', 'free']) {
      const fill = h('i');
      const m = { label: h('span', { class: 'meter-label' }), text: h('span', { class: 'meter-text' }), fill, bar: h('span', { class: 'meter-bar' }, fill), hint: h('span', { class: 'meter-hint muted' }) };
      meterEls[key] = m;
      meters.append(h('div', { class: 'meter', 'data-key': key }, h('div', { class: 'meter-top' }, m.label, m.text), m.bar, m.hint));
    }
    const chips = h('div', { class: 'chips', role: 'group', 'aria-label': 'Filter by state' });
    const chipDefs = [['all', 'All'], ['link', 'Links'], ['speculative', 'Speculative'], ['partial', 'Partial'], ['complete', 'Complete'], ['protected', 'Saved'], ['active', 'Active'], ['failed', 'Failed']];
    const chipEls = {};
    for (const [k, label] of chipDefs) {
      const count = h('span', { class: 'count', text: '0' });
      const b = h('button', { type: 'button', class: 'chip', 'aria-pressed': k === 'all' ? 'true' : 'false', onclick: () => { S.filter = k; for (const [kk, e] of Object.entries(chipEls)) e.b.setAttribute('aria-pressed', kk === k ? 'true' : 'false'); changed(); } }, label + ' ', count);
      chipEls[k] = { b, count };
      chips.append(b);
    }
    const search = h('input', { type: 'search', placeholder: 'Search your library', 'aria-label': 'Search library', autocomplete: 'off', oninput: () => {
      if (S.mode === 'manage' || !search.value.trim()) { S.text = search.value.trim(); changed(); }
    } });
    const searchForm = h('form', { class: 'library-search', role: 'search', 'aria-label': 'Search library', onsubmit: ev => {
      ev.preventDefault(); S.text = search.value.trim();
      if (S.text && sort.value === 'recent') sort.value = 'relevance';
      changed();
    } }, search, h('button', { type: 'submit', 'aria-label': 'Search library', title: 'Search library' }, icon('search')));
    const results = h('div', { class: 'library-results-head', role: 'status', 'aria-live': 'polite' });
    const cards = h('div', { class: 'library-cards' });
    const more = h('button', { type: 'button', class: 'library-show-more', text: 'Show more videos', onclick: () => { S.limit += 36; renderRows(); } });
    const body = h('tbody');
    const empty = h('p', { class: 'empty-note', hidden: true });
    const table = h('table', { class: 'grid lib' },
      h('colgroup', null, ...['c-thumb', 'c-title', 'c-quality', 'c-size', 'c-expiry', 'c-actions'].map((c) => h('col', { class: c }))),
      h('thead', null, h('tr', null, ...['', 'Video', 'Copy', 'Size', 'Retention', 'Actions'].map((t, i) => h('th', { scope: 'col' }, i === 0 ? h('span', { class: 'sr', text: 'Thumbnail' }) : t)))),
      body);
    const tableWrap = h('div', { class: 'tablewrap' }, table);

    function buildRow(browse = false) {
      const r = { entry: null, browse };
      r.img = M.thumbImg(null, 'row-thumb');
      r.title = h('div', { class: browse ? 'library-card-title' : 'ell strong' });
      r.sub = h('div', { class: 'ell line2' });
      r.collectionNames = h('div', { class: 'line2 muted' });
      r.collectionChoice = h('select', { 'aria-label': 'Organize video in collections' });
      r.collectionChoice.addEventListener('change', async () => {
        const name = r.collectionChoice.value; if (!name || !r.entry) return;
        const ids = M.organization.collections[name] || [];
        try { await M.organize({ kind: 'collection', action: ids.includes(r.entry.video_id) ? 'remove' : 'add', name, ids: [r.entry.video_id] }); await fetchAll(); }
        catch (e) { M.flashError(e); }
      });
      r.tagChoice = h('select', { 'aria-label': 'Edit video tags' });
      r.tagChoice.addEventListener('change', async () => {
        const name = r.tagChoice.value; if (!name || !r.entry) return;
        const ids = M.organization.tags[name] || [];
        try { await M.organize({ kind: 'tag', action: ids.includes(r.entry.video_id) ? 'remove' : 'add', name, ids: [r.entry.video_id] }); await fetchAll(); }
        catch (e) { M.flashError(e); }
      });
      r.q = h('span', { class: 'badge q' });
      r.qLine = h('div', { class: 'ell line2' });
      r.size = h('div', { class: 'ell' });
      r.sizeLine = h('div', { class: 'ell line2' });
      r.state = h('span', { class: 'badge' });
      r.status = h('div', { class: 'ell' });
      r.fill = h('i');
      r.bar = h('span', { class: 'meter-bar slim' }, r.fill);
      r.statusLine = h('div', { class: 'ell line2' });
      r.expiry = h('div', { class: 'ell' });
      const mk = (label, action, cls) => h('button', { type: 'button', class: 'small ' + (cls || ''), text: label, onclick: () => act(r, action) });
      r.btn = { play: mk('Play', 'play'), download: mk('Download 480p', 'download'), promote: mk('Keep', 'promote'), evict: mk('Remove', 'evict', 'danger'), retry: mk('Retry', 'retry'), cancel: mk('Cancel', 'cancel') };
      const edit = h('details', { class:'library-row-organize' },h('summary',{text:'Collections & tags'}),h('div',{class:'actions'},r.collectionChoice,r.tagChoice));
      if (browse) {
        r.duration = h('span', { class: 'library-duration' });
        r.watchedFill = h('i');
        r.watchedBar = h('span', { class: 'library-watch-progress' }, r.watchedFill);
        const play = () => act(r, 'play');
        r.play = h('button', { type: 'button', class: 'library-card-image', onclick: play }, r.img, r.duration, r.watchedBar);
        r.titleButton = h('button', { type: 'button', class: 'library-card-title-button', onclick: play }, r.title);
        const menu = h('details', { class: 'library-card-menu' },
          h('summary', { 'aria-label': 'Video actions', title: 'Video actions', text: '⋮' }),
          h('div', { class: 'library-card-actions' }, r.btn.download, r.btn.promote, r.btn.evict, r.btn.retry, r.btn.cancel, r.collectionChoice, r.tagChoice, r.expiry, r.size));
        r.tr = h('article', { class: 'library-video-card' }, r.play,
          h('div', { class: 'library-card-info' }, r.titleButton, menu, r.sub, r.collectionNames,
            h('div', { class: 'library-card-state' }, r.state, r.q, r.status, r.statusLine), r.bar));
      } else r.tr = h('tr', null,
        h('td', null, h('span', { class: 'thumb-box sm' }, r.img)),
        h('td', null, r.title, r.sub, r.collectionNames, edit),
        h('td', null, r.q, r.qLine, r.state, r.status, r.bar, r.statusLine),
        h('td', null, r.size, r.sizeLine),
        h('td', null, r.expiry),
        h('td', null, h('div', { class: 'rowbtns' }, r.btn.play, r.btn.download, r.btn.promote, r.btn.evict, r.btn.retry, r.btn.cancel)));
      return r;
    }
    function updateRow(r, e) {
      r.entry = e;
      const names = Object.keys(M.organization.collections);
      const memberships = names.filter((name) => M.organization.collections[name].includes(e.video_id));
      const tagNames = Object.keys(M.organization.tags || {});
      const tags = tagNames.filter((name) => M.organization.tags[name].includes(e.video_id));
      setText(r.collectionNames, memberships.concat(tags.map((name) => '#' + name)).join(' · '));
      const tagSignature = JSON.stringify([tagNames, tags]);
      if (r.tagSignature !== tagSignature) {
        r.tagSignature = tagSignature;
        r.tagChoice.replaceChildren(h('option', { value: '', text: tagNames.length ? 'Change tags…' : 'No tags yet' }), ...tagNames.map((name) => h('option', { value: name, text: (tags.includes(name) ? 'Remove #' : 'Add #') + name })));
      }
      r.tagChoice.value = ''; r.tagChoice.disabled = !tagNames.length;
      const signature = JSON.stringify([names, memberships]);
      if (r.collectionSignature !== signature) {
        r.collectionSignature = signature;
        r.collectionChoice.replaceChildren(h('option', { value: '', text: names.length ? 'Change collections…' : 'No collections yet' }), ...names.map((name) => h('option', { value: name, text: (memberships.includes(name) ? 'Remove from ' : 'Add to ') + name })));
      }
      r.collectionChoice.value = ''; r.collectionChoice.disabled = !names.length;
      if (r.vid !== e.video_id) { r.vid = e.video_id; r.img.classList.remove('missing'); r.img.src = M.url(e.thumb_url || ('/thumb/' + e.video_id)); }
      setText(r.title, e.title || e.video_id);
      setAttr(r.title, 'title', e.title || e.video_id);
      if (r.browse) {
        const name = e.title || e.video_id;
        setAttr(r.play, 'aria-label', 'Play ' + name);
        setAttr(r.titleButton, 'aria-label', 'Play ' + name);
        setText(r.duration, L.fmtDuration(e.duration_seconds));
        setHidden(r.duration, !e.duration_seconds);
        const progress = L.fraction(e.position_seconds, e.duration_seconds);
        r.watchedFill.style.width = Math.min(100, progress * 100) + '%';
        setHidden(r.watchedBar, !progress);
        setAttr(r.watchedBar, 'title', Math.round(progress * 100) + '% played');
      }
      const watched=e.position_seconds && e.duration_seconds ? Math.min(100,Math.round(e.position_seconds/e.duration_seconds*100))+'% played' : '';
      setText(r.sub, [e.channel, watched, r.browse && e.copy_count > 1 ? e.copy_count + ' copies' : ''].filter(Boolean).join(' · '));
      setText(r.q, L.qLabel(e.variant));
      setHidden(r.q, e.state==='link');
      setCls(r.q, 'badge ' + (e.variant === 'audio' ? 'audio' : 'q'));
      setText(r.qLine, e.actual_height ? 'actual ' + e.actual_height + 'p' : '');
      setText(r.size, e.state==='link' ? '—' : L.fmtBytes(e.size_bytes));
      setText(r.sizeLine, e.expected_bytes && e.expected_bytes !== e.size_bytes ? 'of ' + L.fmtBytes(e.expected_bytes) : '');
      setText(r.state, e.state==='link' ? 'Link only' : e.protected ? 'kept' : e.completed_at ? 'played' : e.status==='complete' ? 'ready' : e.state);
      setCls(r.state, 'badge ' + e.state);
      const p = L.pct(e.progress);
      let st = e.status, line = '';
      if (e.status === 'downloading') { st = 'downloading ' + p + '%'; line = e.speed_bps ? L.fmtBytes(e.speed_bps) + '/s' : ''; }
      else if (e.status === 'failed') line = e.error || 'unknown error';
      setText(r.status, st);
      setHidden(r.status, e.status==='complete'||e.state==='link');
      setText(r.statusLine, line);
      setAttr(r.statusLine, 'title', e.error || line || null);
      const showBar = L.isActiveEntry(e) || e.status === 'failed';
      setCls(r.bar, 'meter-bar slim' + (showBar ? '' : ' off') + (e.status === 'failed' ? ' failed' : ''));
      r.fill.style.width = p + '%';
      const due = e.cleanup_due_at || e.expires_at;
      setText(r.expiry, e.state==='link' ? 'Saved link' : e.protected ? 'Kept' : due ? 'Removes ' + L.relTime(due) : 'Until watched');
      setAttr(r.expiry, 'title', e.state==='link' ? 'Only a saved reference. No media has been downloaded.' : due ? fmtDate(due) : 'Kept until mostly played, then removed after the cleanup grace period. Disk limits still apply.');
      setHidden(r.btn.promote, e.protected || e.state === 'protected' || e.status !== 'complete');
      setHidden(r.btn.retry, e.status !== 'failed');
      setHidden(r.btn.cancel, !L.isActiveEntry(e));
      setHidden(r.btn.download, e.state!=='link');
    }
    async function act(r, action) {
      const e = r.entry;
      if (!e) return;
      if (action === 'play') { M.openVideo(e.video_id); return; }
      const btn = r.btn[action];
      const id = encodeURIComponent(e.id);
      const name = e.title || e.id;
      try {
        btn.disabled = true;
        if (e.state==='link' && action==='evict') { await M.api('POST', `/api/library/${e.video_id}/remove`); await M.refreshOrganization(); M.flash('info','Removed link: '+name); }
        else if (action==='download') { await M.api('POST','/api/cache',{video_id:e.video_id,title:e.title,quality:'480'}); M.flash('info','Download queued: '+name); }
        else if (action === 'promote') { await M.api('POST', `/api/entries/${id}/promote`); M.flash('info', 'Saved to library: ' + name); }
        else if (action === 'retry') { await M.api('POST', `/api/entries/${id}/retry`); M.flash('info', 'Retrying: ' + name); }
        else if (action === 'cancel') { await M.api('POST', `/api/entries/${id}/cancel`); M.flash('info', 'Canceled: ' + name); }
        else if (action === 'evict') {
          const prot = e.protected || e.state === 'protected';
          if (!(await M.confirm(`Delete the cached files of "${name}"?`, 'Delete'))) return;
          if (prot) {
            if (!(await M.confirm(`"${name}" is in your library. Really delete it permanently?`, 'Delete saved video'))) return;
            await M.api('POST', `/api/entries/${id}/evict?force=1`);
          } else {
            try { await M.api('POST', `/api/entries/${id}/evict`); }
            catch (err) {
              if (err instanceof M.ApiError && err.status === 409) {
                if (!(await M.confirm(`"${name}" is protected. Force delete?`, 'Force delete'))) return;
                await M.api('POST', `/api/entries/${id}/evict?force=1`);
              } else throw err;
            }
          }
          M.flash('info', 'Deleted: ' + name);
        }
        pol.kick();
      } catch (err) { M.flashError(err); }
      finally { btn.disabled = false; }
    }

    function renderRows() {
      const browsing = S.mode === 'browse';
      root.classList.toggle('is-browsing', browsing);
      for (const b of modes.children) b.setAttribute('aria-pressed', String(b.dataset.mode === S.mode));
      search.placeholder = browsing ? 'Search your library' : 'Filter by title, channel or video ID';
      setHidden(meters, browsing);
      setHidden(tableWrap, browsing);
      setHidden(cards, !browsing);
      setHidden(results, !browsing);
      setHidden(more, true);
      cards.classList.toggle('search-results', !!S.text);
      const counts = L.libraryCounts(S.entries);
      if (browsing) for (const [key] of chipDefs) counts[key] = L.libraryVideos(S.entries.filter(e => L.entryMatches(e, key, ''))).length;
      for (const [k, e] of Object.entries(chipEls)) setText(e.count, counts[k]);
      const matched = S.entries.filter((e) => L.entryMatches(e, S.filter, S.text) && (!collection.value || M.organization.collections[collection.value]?.includes(e.video_id)) && (!tag.value || M.organization.tags[tag.value]?.includes(e.video_id)));
      const shown = browsing ? L.libraryVideos(matched) : matched;
      shown.sort((a, b) => sort.value === 'relevance' && S.text ? L.librarySearchScore(b, S.text) - L.librarySearchScore(a, S.text) : sort.value === 'title' ? (a.title || '').localeCompare(b.title || '') : sort.value === 'channel' ? (a.channel || '').localeCompare(b.channel || '') : sort.value === 'size' ? b.size_bytes - a.size_bytes : sort.value === 'watched' ? L.fraction(b.position_seconds, b.duration_seconds) - L.fraction(a.position_seconds, a.duration_seconds) : b.created_at - a.created_at);
      setText(results, S.text ? `${shown.length} video${shown.length === 1 ? '' : 's'} for “${S.text}”` : `${shown.length} video${shown.length === 1 ? '' : 's'} in your library`);
      const map = browsing ? S.cards : S.rows, parent = browsing ? cards : body;
      const visible = browsing ? shown.slice(0, S.limit) : shown;
      setHidden(more, !browsing || visible.length >= shown.length);
      const keep = new Set();
      visible.forEach((e, i) => {
        const key = browsing ? e.video_id || e.id : e.id;
        keep.add(key);
        let r = map.get(key);
        if (!r) { r = buildRow(browsing); map.set(key, r); }
        updateRow(r, e);
        if (parent.children[i] !== r.tr) parent.insertBefore(r.tr, parent.children[i] || null);
      });
      for (const [id, r] of map) if (!keep.has(id)) { r.tr.remove(); map.delete(id); }
      setText(empty, S.entries.length ? 'No videos match. Try another search or change your filters.' : 'Your library is empty. Videos you watch, import or save appear here.');
      setHidden(empty, shown.length > 0);
      setHidden(table, shown.length === 0);
    }
    function renderMeters(d) {
      L.diskMeters(d).forEach((m) => {
        const e = meterEls[m.key];
        setText(e.label, m.label); setText(e.text, m.text); setText(e.hint, m.hint);
        e.fill.style.width = (m.fraction * 100).toFixed(1) + '%';
        setCls(e.bar, 'meter-bar' + (m.warn ? ' warn' : ''));
        setAttr(e.bar.parentNode, 'data-warn', m.warn ? '1' : null);
      });
    }
    async function fetchAll() {
      await M.refreshOrganization();
      const tags = Object.keys(M.organization.tags || {});
      if (tag.dataset.names !== JSON.stringify(tags)) {
        const value = tag.value; tag.dataset.names = JSON.stringify(tags);
        tag.replaceChildren(h('option', { value: '', text: 'All tags' }), ...tags.map((name) => h('option', { value: name, text: '#' + name })));
        tag.value = tags.includes(value) ? value : '';
      }
      const names = Object.keys(M.organization.collections);
      if (collection.dataset.names !== JSON.stringify(names)) {
        const value = collection.value; collection.dataset.names = JSON.stringify(names);
        collection.replaceChildren(h('option', { value: '', text: 'All collections' }), ...names.map((name) => h('option', { value: name, text: name })));
        collection.value = names.includes(value) ? value : '';
      }
      const [entries, disk] = await Promise.all([M.api('GET', '/api/library'), M.api('GET', '/api/disk').catch(() => null)]);
      S.entries = L.asArray(entries, 'entries');
      renderRows();
      if (disk) renderMeters(disk);
    }

    return {
      mount(el) {
        root.append(h('div', { class: 'library-viewbar' }, searchForm, modes), meters,
          h('div', { class: 'filters' }, chips, collection, tag, sort), results, cards, more, tableWrap, empty);
        el.append(root);
        renderRows();
        pol = poller(fetchAll, () => (S.entries.some(L.isActiveEntry) ? 2000 : 8000));
        pol.kick();
      },
      unmount() { if (pol) pol.stop(); },
    };
  }

  /* ================================================================== Feeds */

  function feedsPage() {
    const S = { kind: '', list: [], selected: null, data: null, listSig: '', sig: '', refreshing: null, attempted: new Set() };
    let pol = null;
    const kind = h('select', { 'aria-label': 'Feed kind', onchange: (ev) => { S.kind = ev.target.value; S.selected = null; S.listSig = ''; S.sig = ''; pol.kick(); } },
      [['', 'All kinds'], ['home', 'Home'], ['subscriptions', 'Subscriptions'], ['search', 'Search'], ['channel', 'Channel'], ['related', 'Related'], ['other', 'Other']].map(([v, t]) => h('option', { value: v, text: t })));
    const list = h('ul', { class: 'snaplist' });
    const listEmpty = h('p', { class: 'muted small-note', hidden: true, text: 'No snapshots yet. Browse YouTube and Muninn keeps what it sees.' });
    const title = h('h2', { class: 'h2', text: 'Feed' });
    const meta = h('span', { class: 'muted' });
    const stale = h('span', { class: 'badge warn', text: 'stale', hidden: true });
    const grid = h('div', { class: 'vgrid' });
    const note = h('p', { class: 'empty-note', hidden: true });

    const refreshNote = h('p', { class: 'muted small-note', role: 'status' });
    const refreshButton = h('button', { text: 'Refresh from YouTube', onclick: () => refreshFeed().catch(M.flashError) });
    async function refreshFeed() {
      if (S.refreshing) return;
      const feedKind = S.kind === 'home' ? 'home' : 'subscriptions';
      S.kind = feedKind; kind.value = feedKind; S.selected = null;
      S.attempted.add(feedKind); refreshButton.disabled = true;
      setText(refreshNote, 'Loading ' + feedKind + ' using your signed-in YouTube session…');
      try {
        const result = await M.api('POST', '/api/feeds/refresh', { kind: feedKind });
        S.refreshing = { kind: feedKind, since: result.requested_at, deadline: Date.now() + 45000 };
        pol.kick();
      } catch (e) {
        refreshButton.disabled = false;
        setText(refreshNote, 'Could not request refresh: ' + M.errText(e));
        throw e;
      }
    }

    function renderList() {
      const sig = JSON.stringify(S.list.map((s) => [s.id, s.count, s.captured_at])) + '|' + S.selected;
      if (sig === S.listSig) return;
      S.listSig = sig;
      list.replaceChildren(...S.list.map((s) => h('li', null, h('button', { type: 'button', 'aria-current': s.id === S.selected ? 'true' : 'false', onclick: () => { S.selected = s.id; S.sig = ''; renderList(); pol.kick(); } },
        h('span', { class: 'k', text: s.kind + (s.query ? ': ' + s.query : '') }),
        h('span', { class: 'n', text: s.count + ' videos' }),
        h('span', { class: 's', text: L.relTime(s.captured_at) })))));
      setHidden(listEmpty, S.list.length > 0);
    }
    function renderFeed() {
      const d = S.data;
      if (!d) { setText(title, 'Feed'); setText(meta, ''); setHidden(stale, true); grid.replaceChildren(); S.sig = ''; setText(note, 'Nothing to show yet.'); setHidden(note, false); return; }
      const items = d.items || [];
      setText(title, d.kind + (d.query ? ': ' + d.query : ''));
      setText(meta, `${items.length} videos, captured ${L.relTime(d.captured_at)}`);
      setHidden(stale, !d.stale);
      const sig = d.id + '|' + items.map((i) => i.video_id + ':' + (i.cached ? i.cached.state + i.cached.progress : '')).join(',');
      if (sig !== S.sig) { S.sig = sig; grid.replaceChildren(...items.map((it) => M.videoCard(it, (x) => M.openVideo(x.video_id)))); }
      setText(note, 'This snapshot has no videos.');
      setHidden(note, items.length > 0);
    }
    async function tick() {
      const q = new URLSearchParams({ limit: '60' });
      if (S.kind) q.set('kind', S.kind);
      S.list = L.asArray(await M.api('GET', '/api/feeds?' + q), 'feeds');
      if (S.refreshing) {
        const fresh = S.list.find((s) => s.kind === S.refreshing.kind && s.captured_at >= S.refreshing.since);
        if (fresh || Date.now() > S.refreshing.deadline) {
          S.refreshing = null; refreshButton.disabled = false;
          if (fresh) { S.selected = fresh.id; setText(refreshNote, 'Refreshed from your YouTube session.'); }
          else setText(refreshNote, 'No new feed was captured. Open the subscriptions tab in Muninn to check for a sign-in, consent or network prompt, then retry.');
        }
      }
      if (S.selected != null && !S.list.some((s) => s.id === S.selected)) S.selected = null;
      if (S.selected == null && S.list.length) S.selected = S.list[0].id;
      S.data = S.selected == null ? null : await M.api('GET', '/api/feeds/' + encodeURIComponent(S.selected)).catch(() => null);
      renderList(); renderFeed();
      if (S.kind === 'subscriptions' && !S.list.length && !S.refreshing && !S.attempted.has(S.kind)) {
        refreshFeed().catch(M.flashError);
      }
    }
    return {
      mount(el) {
        el.append(pageHead('Feeds', 'Snapshots of the pages you browsed, kept in the content cache. Click a video to watch it in Muninn.'),
          h('div', { class: 'split' },
            h('aside', { class: 'side-box', 'aria-label': 'Feed snapshots' }, h('label', { class: 'field' }, h('span', { text: 'Kind' }), kind), refreshButton, refreshNote, list, listEmpty),
            h('div', { class: 'feed-main' }, h('div', { class: 'feed-head' }, title, stale, meta), note, grid)));
        pol = poller(tick, () => S.refreshing ? 1000 : ((S.data && (S.data.items || []).some((i) => i.cached && i.cached.progress < 1)) ? 2500 : 15000));
        pol.kick();
      },
      unmount() { if (pol) pol.stop(); },
    };
  }

  /* ================================================================== Rules */

  function configEditor(title, path, help) {
    const text = h('textarea', { class: 'config-json', spellcheck: 'false', 'aria-label': title });
    const msg = slot('slot-xl');
    const load = async () => { text.value = JSON.stringify(await M.api('GET', path), null, 2); };
    const save = h('button', { text: 'Save ' + title, onclick: async () => {
      try { const result = await M.api('PUT', path, JSON.parse(text.value)); text.value = JSON.stringify(result, null, 2); msg.say('Saved', 'ok'); }
      catch (e) { msg.say(M.errText(e), 'error', 10000); }
    } });
    const root = h('section', { class: 'card' }, h('h2', { class: 'h2', text: title }), h('p', { class: 'muted', text: help }), text,
      h('div', { class: 'actions' }, save, h('button', { text: 'Reload', onclick: () => load().catch(M.flashError) }), msg));
    return { root, load };
  }

  function rulesPage() {
    const automation = configEditor('Advanced completion rules', '/api/automation', 'Default: mark mostly played at 95%, then remove after one day. Library → Playback & cleanup has the normal controls. Ordered completion_rules can override by channel, title or duration; action is delete or keep. Retention days 0 means until watched.');
    const S = { models: [], saved: '', saving: false };
    const body = h('tbody');
    const empty = h('p', { class: 'empty-note', hidden: true, text: 'No rules. Videos are only cached when you ask.' });
    const dirty = h('span', { class: 'slot slot-md muted' });
    const msg = slot('slot-xl');
    const save = h('button', { type: 'button', class: 'primary', text: 'Save rules' });
    const revert = h('button', { type: 'button', text: 'Revert' });
    const add = h('button', { type: 'button', text: 'Add rule' });
    const table = h('table', { class: 'grid rules' },
      h('colgroup', null, ...['r-order', 'r-on', 'r-name', 'r-channels', 'r-only', 'r-kw', 'r-min', 'r-max', 'r-trigger', 'r-quality', 'r-ret', 'r-del'].map((c) => h('col', { class: c }))),
      h('thead', null, h('tr', null, ...['Order', 'On', 'Name', 'Channels', 'Only these', 'Keywords', 'Min min', 'Max min', 'Trigger', 'Quality', 'Keep days', ''].map((t) => h('th', { scope: 'col', text: t })))),
      body);

    function markDirty() {
      const d = L.rulesSnapshot(S.models) !== S.saved;
      const { errors } = L.rulesValidate(S.models);
      setText(dirty, d ? 'Unsaved changes' : '');
      save.disabled = !d || S.saving;
      revert.disabled = !d || S.saving;
      const bad = new Set(errors.map((x) => x.row));
      $$('tr', body).forEach((tr, i) => tr.classList.toggle('invalid', bad.has(i)));
      return d;
    }
    function render(focus) {
      const n = S.models.length;
      body.replaceChildren(...S.models.map((m, i) => {
        const inp = (key, label, attrs) => h('input', Object.assign({ type: 'text', 'aria-label': `${label} (rule ${i + 1})`, value: m[key], spellcheck: 'false', oninput: (ev) => { m[key] = ev.target.value; markDirty(); } }, attrs || {}));
        const chk = (key, label) => h('input', { type: 'checkbox', 'aria-label': `${label} (rule ${i + 1})`, checked: m[key], onchange: (ev) => { m[key] = ev.target.checked; markDirty(); } });
        const sel = (key, opts, label) => {
          const s = h('select', { 'aria-label': `${label} (rule ${i + 1})`, onchange: (ev) => { m[key] = ev.target.value; markDirty(); } }, opts.map(([v, t]) => h('option', { value: v, text: t })));
          if (!opts.some(([v]) => v === m[key])) s.append(h('option', { value: m[key], text: m[key] }));
          s.value = m[key];
          return s;
        };
        const mv = (dir, label) => h('button', { type: 'button', class: 'small', text: dir < 0 ? '↑' : '↓', 'aria-label': `Move rule ${i + 1} ${label}`, disabled: dir < 0 ? i === 0 : i === n - 1, onclick: () => { const j = i + dir;[S.models[i], S.models[j]] = [S.models[j], S.models[i]]; render({ row: j, dir }); } });
        return h('tr', null,
          h('td', { class: 'nowrap' }, mv(-1, 'up'), ' ', mv(1, 'down')),
          h('td', { class: 'c' }, chk('enabled', 'Enabled')),
          h('td', null, inp('name', 'Name')),
          h('td', null, inp('channels', 'Channels', { placeholder: 'any channel' })),
          h('td', { class: 'c' }, chk('channels_only', 'Only listed channels')),
          h('td', null, inp('keywords', 'Title keywords', { placeholder: 'any title' })),
          h('td', null, inp('min', 'Min minutes', { placeholder: '-' })),
          h('td', null, inp('max', 'Max minutes', { placeholder: '-' })),
          h('td', null, sel('trigger', L.TRIGGERS, 'Trigger')),
          h('td', null, sel('quality', L.QUALITIES, 'Quality')),
          h('td', null, inp('retention', 'Retention days')),
          h('td', null, h('button', { type: 'button', class: 'small danger', text: 'Delete', 'aria-label': `Delete rule ${i + 1}`, onclick: () => { S.models.splice(i, 1); render({ row: Math.min(i, S.models.length - 1), name: true }); } })));
      }));
      setHidden(empty, n > 0);
      setHidden(table, n === 0);
      markDirty();
      if (focus && n) {
        const tr = body.children[focus.row];
        if (tr) { const b = focus.name ? $('input[type="text"]', tr) : ($$('button', tr).filter((x) => !x.disabled)[focus.dir < 0 ? 0 : 1] || $('button', tr)); if (b) b.focus(); }
      }
    }
    async function fetchRules() {
      S.models = L.asArray(await M.api('GET', '/api/rules'), 'rules').map(L.ruleToModel);
      S.saved = L.rulesSnapshot(S.models);
      render();
    }
    add.addEventListener('click', () => { S.models.push(L.newRuleModel()); render({ row: S.models.length - 1, name: true }); const i = $('tr:last-child input[type="text"]', body); if (i) i.select(); });
    revert.addEventListener('click', () => { fetchRules().then(() => msg.say('Reverted', 'ok')).catch(M.flashError); });
    save.addEventListener('click', async () => {
      const { errors, payload } = L.rulesValidate(S.models);
      if (errors.length) { msg.say(errors[0].text + (errors.length > 1 ? ` (+${errors.length - 1} more)` : ''), 'error', 8000); return; }
      S.saving = true; markDirty(); msg.say('Saving...', '', 0);
      try {
        const res = await M.api('PUT', '/api/rules', payload);
        const list = L.asArray(res, 'rules');
        S.models = list.map(L.ruleToModel); S.saved = L.rulesSnapshot(S.models); render();
        msg.say('Saved', 'ok');
      } catch (e) { msg.say(M.errText(e), 'error', 8000); }
      finally { S.saving = false; markDirty(); }
    });
    return {
      mount(el) {
        el.append(pageHead('Rules', 'The first enabled matching rule decides what is cached. Retention 0 means until watched; positive days add an age limit. Configure channel downloads and normal cleanup in Library.'),
          h('div', { class: 'tablewrap' }, table, empty),
          h('div', { class: 'actions' }, add, save, revert, dirty, msg), automation.root);
        render();
        fetchRules().catch(M.flashError);
        automation.load().catch(M.flashError);
      },
      unmount() { /* unsaved edits are dropped; the table re-reads on return */ },
    };
  }

  /* ================================================================== Settings */

  function settingsPage() {
    const S = { orig:null, inputs:{}, saving:false, revision:0, timer:null };
    const form=h('form',{class:'settings-form',autocomplete:'off'});
    const msg=h('span',{role:'status','aria-live':'polite',text:'Changes save automatically'});
    const retry=h('button',{type:'button',text:'Retry save',hidden:true,onclick:()=>flush()});
    const feedback=h('div',{class:'settings-feedback'},msg,retry);
    const signMsg=slot('slot-xl');
    const accountLine=h('p',{class:'muted',role:'status','aria-live':'polite'});
    let accountPoll=null, accountBusy=false;

    /* Setup and repair: the plain-language home of what used to live on Diagnostics. */
    const toolsMsg=slot('slot-xl'),trustMsg=slot('slot-xl');
    const toolsLine=h('p',{class:'setup-text'}),trustLine=h('p',{class:'trust-line'}),trustDetail=h('p',{class:'muted trust-detail'});
    const updateBtn=h('button',{type:'button',text:'Update video downloader',onclick:async()=>{
      updateBtn.disabled=true;toolsMsg.say('Updating…','',0);
      try{await M.api('POST','/api/tools/ytdlp/update');toolsMsg.say('The video downloader is up to date','ok');}
      catch(e){toolsMsg.say(M.errText(e),'error',9000);}
      finally{updateBtn.disabled=false;await refreshSetup();}
    }});
    const trustBtn=h('button',{type:'button',text:'Set up secure browsing',onclick:async()=>{
      trustBtn.disabled=true;
      try{await M.invoke('trust_install');}catch(e){trustMsg.say(M.errText(e),'error',9000);}
      await refreshSetup();
    }});
    function renderSetup(){
      const setup=M.setup,tools=M.tools||{};
      const y=tools.ytdlp||{},f=tools.ffmpeg||{};
      const parts=[];
      parts.push(y.installed?'Video downloader: ready'+(y.version?' ('+y.version+')':''):'Video downloader: not installed yet');
      parts.push(f.found?'FFmpeg video merging: available':'FFmpeg video merging: unavailable (higher qualities may be unavailable)');
      parts.push('Playback health: not inferred from helper availability; it is reported for an active playback only.');
      const phase=setup&&setup.phase&&setup.phase!=='ready'?' '+(setup.message||('Setup is '+setup.phase+'.')):'';
      setText(toolsLine,parts.join('. ')+'.'+phase);
      setHidden(updateBtn,!y.installed);
      const tr=M.trust;
      if(!tr){setText(trustLine,'Secure-browsing trust status is unavailable; it has not been checked.');setText(trustDetail,'');trustBtn.disabled=true;setHidden(trustBtn,true);return;}
      setHidden(trustBtn,false);
      const label={ok:'Secure browsing is set up.',needed:'Secure browsing is not set up yet. YouTube pages cannot be read until it is.',installing:'Setting up…',failed:'Setting up secure browsing failed. You can try again.'}[tr.state]||tr.state;
      setText(trustLine,label);setCls(trustLine,'trust-line '+(tr.state==='ok'?'ok':(tr.state==='needed'||tr.state==='failed'?'bad':'')));
      setText(trustDetail,tr.detail||'');
      trustBtn.disabled=tr.state==='ok'||tr.state==='installing';
      setCls(trustBtn,tr.state==='needed'||tr.state==='failed'?'primary':'');
    }
    async function refreshSetup(){try{await M.pollSystemNow();}catch(e){/* offline banner */}renderSetup();}
    const setupPoll=poller(async()=>{await M.pollSystemNow();renderSetup();},()=>5000);
    async function refreshAccount(){
      if(accountBusy)return; accountBusy=true;
      try { const res=await M.api('GET','/api/channels/status'); const observed=L.observedAccountState(res&&res.account);
        setText(accountLine, observed.label+'. Account identifiers are not shown.');
        signBtn.textContent=observed.state==='signed_in'?'Manage or switch account':'Sign in in a separate window';
        signBtn.hidden=false;
      } catch(e) { setText(accountLine,'YouTube sign-in status is temporarily unavailable.'); signBtn.textContent='Sign in in a separate window'; signBtn.hidden=false; }
      finally { accountBusy=false; }
    }
    accountPoll=poller(refreshAccount,()=>4000);

    /* Developer section: only present while developer mode is on. */
    const exportMsg=slot('slot-xl');
    const exportBtn=h('button',{type:'button',text:'Export diagnostics',onclick:async()=>{
      exportBtn.disabled=true;exportMsg.say('Choose where to save…','',0);
      let result=null,error=null;
      try{result=await M.invoke('export_diagnostics');}catch(e){error=e==null?'unknown error':e;}
      finally{exportBtn.disabled=false;}
      const out=L.exportOutcome(result,error);
      exportMsg.say(out.text,out.kind==='info'?'':out.kind,out.kind==='error'?12000:0);
      if(out.kind==='ok')M.flash('info',out.text);
      if(out.kind==='error')M.flash('error',out.text);
    }});
    const diagLink=h('button',{type:'button',text:'Open detailed diagnostics',onclick:()=>{M.page='diagnostics';M.render();}});
    const inApp=!!(window.__TAURI_INTERNALS__&&window.__TAURI_INTERNALS__.invoke);
    const devCard=h('section',{class:'card','aria-labelledby':'dev-h',hidden:true,'data-section':'developer'},
      h('h2',{class:'h2',id:'dev-h',text:'Developer tools'}),
      h('p',{class:'muted',text:'Detailed diagnostics show raw technical status, the capture proxy and application logs. The export is a ZIP with recent redacted activity for troubleshooting; review it before sharing.'}),
      h('div',{class:'actions'},diagLink,inApp?exportBtn:null,inApp?exportMsg:null),
      inApp?null:h('p',{class:'muted',text:'Exporting is available in the Muninn app, or with: muninn --cli diagnostics export FILE.zip'}));
    function syncDeveloper(){setHidden(devCard,!L.developerMode(M.settings));}
    function values() {const v={};for(const f of L.SETTINGS_FIELDS){const input=S.inputs[f.key];if(input)v[f.key]=f.kind==='bool'?input.checked:input.value;}return v;}
    function apply(settings) {const offline=M.settings?.offline_mode,developer=L.developerMode(M.settings);M.settings=settings;M.applyAppearance();M.watch?.applySettings();syncDeveloper();if(offline!==settings.offline_mode||developer!==L.developerMode(settings))M.render();}
    async function flush() {
      clearTimeout(S.timer);
      if(!S.orig||S.saving)return;
      S.saving=true;retry.hidden=true;
      try {
        while(true) {
          const result=L.settingsPatch(L.SETTINGS_FIELDS,S.orig,values());
          if(result.error){setText(msg,result.error);break;}
          if(!Object.keys(result.patch).length){setText(msg,'All changes saved');break;}
          const revision=S.revision;setText(msg,'Saving…');
          const saved=await M.api('PUT','/api/settings',result.patch);S.orig=saved;
          if(revision===S.revision){
            apply(saved);
            for(const f of L.SETTINGS_FIELDS){const input=S.inputs[f.key],value=L.settingToInput(f,saved[f.key]);if(f.kind==='bool')input.checked=value;else input.value=value;}
            setText(msg,'All changes saved');break;
          }
        }
      } catch(error) {apply(S.orig);setText(msg,'Could not save: '+M.errText(error));retry.hidden=false;M.flashError(error);}
      finally {S.saving=false;}
    }
    function change(immediate) {
      if(!S.orig)return;S.revision++;clearTimeout(S.timer);
      const result=L.settingsPatch(L.SETTINGS_FIELDS,S.orig,values());
      if(result.error){setText(msg,result.error);return;}
      // Apply on the input gesture, including PiP policy for an already-open player.
      apply({...M.settings,...result.patch});
      if(immediate)flush();else {setText(msg,'Saving shortly…');S.timer=setTimeout(flush,400);}
    }
    function render() {
      const groups=[];S.inputs={};
      for(const f of L.SETTINGS_FIELDS){let group=groups.find(g=>g.name===f.group);if(!group){group={name:f.group,fields:[]};groups.push(group);}group.fields.push(f);}
      form.replaceChildren(feedback,...groups.map(group=>h('fieldset',{class:'fgroup'},h('legend',{text:group.name}),h('div',{class:'fieldgrid'},...group.fields.map(f=>{
        const id='set-'+f.key,value=L.settingToInput(f,S.orig[f.key]);let input;
        if(f.kind==='bool')input=h('input',{type:'checkbox',id,checked:value,onchange:()=>change(true)});
        else if(f.kind==='select'||f.kind==='choice'){input=h('select',{id,onchange:()=>change(true)},...f.options.map(([value,text])=>h('option',{value:String(value),text})));input.value=value;}
        else input=h('input',{type:'number',id,value,min:String(f.min??0),max:f.max==null?null:String(f.max),step:f.kind==='bytes'?'any':'1',oninput:()=>change(false),onchange:()=>change(true)});
        S.inputs[f.key]=input;
        return h('div',{class:'field'},f.kind==='bool'?h('label',{class:'check',for:id},input,' '+f.label):h('label',{for:id,text:f.label+(f.unit?' ('+f.unit+')':'')}),f.kind==='bool'?null:input,h('span',{class:'hint',text:f.hint||' '}));
      })))));
    }
    async function fetchSettings(){S.orig=await M.api('GET','/api/settings');render();}
    form.addEventListener('submit',event=>{event.preventDefault();flush();});
    const signBtn = h('button', { type: 'button', text: 'Sign in in a separate window', onclick: async () => {
      signBtn.disabled = true;
      try { await M.invoke('signin_popup'); signMsg.say('Sign-in window opened', 'ok'); }
      catch (e) { signMsg.say(M.errText(e), 'error', 8000); }
      finally { signBtn.disabled = false; }
    } });
    return {
      mount(el) {
        el.append(pageHead('Settings', 'Limits, freshness and browsing behaviour.'), form,
          h('section', { class: 'card', 'aria-labelledby': 'setup-h' },
            h('h2', { class: 'h2', id: 'setup-h', text: 'Setup and repair' }),
            toolsLine, h('div', { class: 'actions' }, updateBtn, toolsMsg),
            trustLine, trustDetail, h('div', { class: 'actions' }, trustBtn, trustMsg)),
          h('section', { class: 'card', 'aria-labelledby': 'signin-h' },
            h('h2', { class: 'h2', id: 'signin-h', text: 'Google sign-in' }),
            accountLine,
            h('p', { class: 'muted', text: 'Sign-in status reflects the latest live YouTube page observation; opening a browser does not count as signing in. Google may reject some embedded-browser login flows; a separate window is available.' }),
            h('div', { class: 'actions' }, signBtn, signMsg)),
          devCard);
        renderSetup(); setupPoll.kick(); accountPoll.kick();
        fetchSettings().then(syncDeveloper).catch(M.flashError);
      },
      unmount() { clearTimeout(S.timer); setupPoll.stop(); accountPoll.stop(); flush(); },
    };
  }

  /* ================================================================== Diagnostics */

  function diagnosticsPage() {
    const S = { setup: null, proxy: null, captures: [], settings: null };
    let pol = null;
    const root = h('div', { class: 'diag' });
    const secs = {};
    function section(key, title, ...kids) {
      const body = h('div', { class: 'diag-body' }, ...kids);
      secs[key] = body;
      return h('section', { class: 'card', 'aria-label': title }, h('h2', { class: 'h2', text: title }), body);
    }
    const toolsMsg = slot('slot-lg');
    const updateBtn = h('button', { type: 'button', text: 'Update yt-dlp', onclick: async () => {
      updateBtn.disabled = true; toolsMsg.say('Updating...', '', 0);
      try { await M.api('POST', '/api/tools/ytdlp/update'); toolsMsg.say('yt-dlp is up to date', 'ok'); M.pollSystemNow(); pol.kick(); }
      catch (e) { toolsMsg.say(M.errText(e), 'error', 9000); }
      finally { updateBtn.disabled = false; }
    } });
    const trustBtn = h('button', { type: 'button', text: 'Install trust', onclick: async () => {
      trustBtn.disabled = true;
      try { await M.invoke('trust_install'); } catch (e) { M.flash('error', 'Trust: ' + M.errText(e)); }
      await M.pollSystemNow(); pol.kick();
    } });

    const setupLine = h('div', { class: 'setup-line' });
    const setupFill = h('i');
    const setupBar = h('span', { class: 'meter-bar' }, setupFill);
    const toolsList = h('dl', { class: 'facts' });
    const notes = h('ul', { class: 'notes muted' });
    const trustLine = h('p', { class: 'trust-line' });
    const trustDetail = h('p', { class: 'muted trust-detail' });
    const tiles = h('div', { class: 'tiles' });
    const hosts = h('p', { class: 'muted hosts' });
    const eventsBody = h('tbody');
    const capturesBody = h('tbody');
    const tabsBody = h('tbody');
    const tabsNote = h('p', { class: 'muted' });
    const logText = h('pre', { class: 'mono', style: 'white-space:pre-wrap;max-height:320px;overflow:auto' });
    const logPath = h('p', { class: 'muted' });
    const logBtn = h('button', { text: 'Refresh application logs', onclick: async () => {
      try { const logs = await M.api('GET', '/api/logs'); setText(logPath, logs.directory); setText(logText, logs.events.map((e) => JSON.stringify(e)).join('\n')); }
      catch (e) { M.flashError(e); }
    } });

    const mkTable = (cols, tbody, cls) => h('div', { class: 'tablewrap ' + (cls || '') }, h('table', { class: 'grid small' }, h('thead', null, h('tr', null, ...cols.map((c) => h('th', { scope: 'col', text: c })))), tbody));

    root.append(
      pageHead('Diagnostics', 'Developer mode. What Muninn is doing behind the page: tools, certificate trust, the capture proxy and your tabs.'),
      section('tools', 'Tools and setup', setupLine, setupBar, toolsList, notes, h('div', { class: 'actions' }, updateBtn, toolsMsg)),
      section('trust', 'Certificate trust', trustLine, trustDetail, h('div', { class: 'actions' }, trustBtn)),
      section('proxy', 'Capture proxy', tiles, hosts, h('h3', { class: 'h3', text: 'Recent proxy events' }), mkTable(['Time', 'Kind', 'Host', 'Detail', 'Status', 'Bytes'], eventsBody, 'tall')),
      section('captures', 'Recent captures', mkTable(['Time', 'What', 'Kind', 'Items', 'Comments', 'Video', 'Note'], capturesBody, 'tall')),
      section('tabs', 'Tabs', tabsNote, mkTable(['Title', 'State', 'Kind', 'Last active', ''], tabsBody)),
      section('logs', 'Application logs', logPath, logBtn, logText));

    function row(cells) { return h('tr', null, ...cells.map((c) => h('td', c instanceof Node ? null : { title: String(c == null ? '' : c) }, c instanceof Node ? c : (c == null ? DASH : String(c))))); }

    function renderTools() {
      const s = S.setup && S.setup.setup, t = S.setup && S.setup.tools;
      if (!s || !t) return;
      const phaseKind = s.phase === 'ready' ? 'ok' : (s.phase === 'error' ? 'error' : 'busy');
      setupLine.replaceChildren(badge(s.phase, phaseKind), h('span', { text: ' ' + (s.message || '') }));
      const busy = ['installing', 'updating', 'starting'].includes(s.phase);
      setCls(setupBar, 'meter-bar' + (busy ? '' : ' off') + (busy && s.progress == null ? ' indeterminate' : ''));
      setupFill.style.width = s.progress == null ? '' : Math.round(s.progress * 100) + '%';
      const fact = (k, label, kind, detail) => [h('dt', { text: k }), h('dd', null, badge(label, kind), h('span', { class: 'mono tool-detail', text: detail || '' }))];
      const y = t.ytdlp || {}, f = t.ffmpeg || {}, j = t.js_runtime || {};
      toolsList.replaceChildren(
        ...fact('yt-dlp', y.installed ? 'installed' : 'missing', y.installed ? 'ok' : 'error', [y.version, y.path, y.error].filter(Boolean).join('  ')),
        ...fact('ffmpeg', f.found ? 'found' : 'missing', f.found ? 'ok' : 'warn', [f.version, f.path].filter(Boolean).join('  ')),
        ...fact('JS runtime', j.found ? 'found' : 'missing', j.found ? 'ok' : 'warn', [j.name, j.path].filter(Boolean).join('  ')),
        ...fact('Merging 480p+', t.can_merge ? 'yes' : 'no', t.can_merge ? 'ok' : 'warn', t.can_merge ? '' : 'needs ffmpeg'));
      notes.replaceChildren(...(t.notes || []).map((n) => h('li', { text: n })));
      setHidden(notes, !(t.notes || []).length);
      setHidden(updateBtn, !y.installed);
    }
    function renderTrust() {
      const tr = M.trust;
      if (!tr) { setText(trustLine, 'Trust status is only available in the Muninn app.'); setText(trustDetail, ''); trustBtn.disabled = true; return; }
      const label = { ok: 'The local certificate is trusted.', needed: 'The local certificate is not trusted yet. YouTube pages cannot be read until it is.', installing: 'Installing...', failed: 'Trusting the certificate failed.' }[tr.state] || tr.state;
      setText(trustLine, label);
      setCls(trustLine, 'trust-line ' + (tr.state === 'ok' ? 'ok' : (tr.state === 'needed' || tr.state === 'failed' ? 'bad' : '')));
      setText(trustDetail, tr.detail || '');
      trustBtn.disabled = tr.state === 'ok' || tr.state === 'installing';
      setCls(trustBtn, tr.state === 'needed' || tr.state === 'failed' ? 'primary' : '');
    }
    function renderProxy() {
      const p = S.proxy;
      if (!p) return;
      const st = p.stats || {};
      const tile = (label, v, kind) => h('div', { class: 'tile' + (kind ? ' ' + kind : '') }, h('div', { class: 'tile-v', text: v }), h('div', { class: 'tile-l', text: label }));
      const media = st.media_bytes_fetched || 0;
      tiles.replaceChildren(
        tile('Connects', st.connects), tile('Inspected (MITM)', st.mitm_connects), tile('Tunnelled', st.tunnels), tile('Blocked', st.blocked),
        tile('TLS ok', st.tls_ok), tile('TLS failures', st.tls_failures, st.tls_failures ? 'warn' : ''), tile('Untrusted cert', st.tls_untrusted, st.tls_untrusted ? 'bad' : ''),
        tile('Captured', st.captured), tile('Captured data', L.fmtBytes(st.captured_bytes || 0)),
        tile('Media bytes fetched', L.fmtBytes(media), media ? 'bad' : 'good'));
      setText(hosts, (st.hosts || []).slice(0, 8).map((x) => `${x.host} (${x.count})`).join(', ') || (p.info ? '' : 'The capture proxy is not running.'));
      eventsBody.replaceChildren(...(p.events || []).slice().reverse().map((e) => row([fmtTs(e.ts), e.kind, e.host, e.detail, e.status, e.bytes == null ? DASH : L.fmtBytes(e.bytes)])));
      if (p.info && p.info.url) setAttr(hosts, 'title', 'Proxy: ' + p.info.url);
    }
    function renderCaptures() {
      capturesBody.replaceChildren(...(S.captures || []).map((c) => row([fmtTs(c.ts), c.what, c.kind, c.items, c.comments, c.video_id, c.note || c.url])));
    }
    function renderTabs() {
      const s = S.settings || {};
      setText(tabsNote, `Warm tabs: ${s.warm_tabs == null ? M.tabs.warm : s.warm_tabs}. A tab in the background for ${s.hibernate_after_secs == null ? '?' : s.hibernate_after_secs} seconds may go to sleep; sleeping tabs show their cached content until you wake them. Change this in Settings.`);
      tabsBody.replaceChildren(...M.tabs.list.map((t) => row([
        L.tabTitle(t) + (t.id === M.tabs.active ? '  (active)' : ''),
        badge(t.state === 'sleeping' ? 'sleeping' : (t.state === 'live' ? 'live' : t.state), t.state === 'live' ? 'ok' : (t.state === 'sleeping' ? '' : 'warn')),
        t.face === 'watch' ? 'watch' : t.kind, t.last_active_ms ? L.relTime(t.last_active_ms) : DASH,
        L.canHibernate(t) ? h('button', { type: 'button', class: 'small', text: 'Put to sleep', onclick: () => M.hibernateTab(t.id) }) : '',
      ])));
      if (!M.tabs.list.length) tabsBody.replaceChildren(row(['No tabs open', '', '', '', '']));
    }
    async function tick() {
      const [setup, proxy, caps, settings] = await Promise.all([
        M.api('GET', '/api/setup').catch(() => null), M.api('GET', '/api/proxy').catch(() => null),
        M.api('GET', '/api/captures?limit=30').catch(() => null), M.api('GET', '/api/settings').catch(() => null)]);
      S.setup = setup; S.proxy = proxy; S.captures = L.asArray(caps, 'captures'); S.settings = settings;
      try { M.trust = await M.invoke('trust_status'); } catch (e) { M.trust = null; }
      renderTools(); renderTrust(); renderProxy(); renderCaptures(); renderTabs();
    }
    return {
      mount(el) { el.append(root); renderTabs(); renderTrust(); pol = poller(tick, () => 2000); pol.kick(); },
      unmount() { if (pol) pol.stop(); },
    };
  }

  M.pages = {
    library: Object.assign({ title: 'Library' }, wrap(libraryPage)),
    feeds: Object.assign({ title: 'Feeds' }, wrap(feedsPage)),
    rules: Object.assign({ title: 'Rules' }, wrap(rulesPage)),
    diagnostics: Object.assign({ title: 'Diagnostics' }, wrap(diagnosticsPage)),
    settings: Object.assign({ title: 'Settings' }, wrap(settingsPage)),
  };

  /** Pages are rebuilt on each visit: no stale DOM, pollers are stopped on leave. */
  function wrap(factory) {
    let inst = null;
    return {
      mount(el) { inst = factory(); el.classList.add('page'); inst.mount(el); },
      unmount() { if (inst) { inst.unmount(); inst = null; } },
    };
  }
})();
