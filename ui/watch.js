/* Muninn's own watch view: player with its own control bar, caching status,
 * title/channel/description, cache panel, comments and related videos.
 * Exactly one <video> element exists; leaving the view detaches it. */
(function () {
  'use strict';
  const M = window.Muninn;
  const L = M.L;
  const { h, $, setText, setHidden, setCls, setAttr, icon } = M;
  const root = $('#view-watch');

  /* ------------------------------------------------------------------ skeleton (built once) */

  const video = h('video', { class: 'pv-video', preload: 'auto' });
  video.setAttribute('playsinline', '');
  video.setAttribute('aria-label', 'Video');
  video.disablePictureInPicture = false;

  const ov = {
    spinner: h('span', { class: 'spinner big' }),
    text: h('p', { class: 'pv-ov-text' }),
    detail: h('p', { class: 'pv-ov-detail' }),
    bigPlay: h('button', { type: 'button', class: 'pv-bigplay', 'aria-label': 'Play' }),
    retry: h('button', { type: 'button', class: 'primary', text: 'Retry' }),
    ext: h('button', { type: 'button', text: 'Open on youtube.com' }),
  };
  ov.bigPlay.append(icon('play'));
  ov.actions = h('div', { class: 'pv-ov-actions' }, ov.retry, ov.ext);
  ov.el = h('div', { class: 'pv-overlay', hidden: true }, ov.spinner, ov.text, ov.detail, ov.bigPlay, ov.actions);

  const ctl = {
    play: h('button', { type: 'button', class: 'pc-btn', title: 'Play (space)', 'aria-label': 'Play' }),
    time: h('span', { class: 'pc-time', text: '0:00 / --:--' }),
    avail: h('span', { class: 'pc-avail' }),
    played: h('span', { class: 'pc-played' }),
    seek: h('input', { type: 'range', class: 'pc-seek-input', min: '0', max: '1', step: 'any', value: '0', 'aria-label': 'Seek', disabled: true }),
    mute: h('button', { type: 'button', class: 'pc-btn', title: 'Mute (m)', 'aria-label': 'Mute' }),
    vol: h('input', { type: 'range', class: 'pc-vol', min: '0', max: '1', step: '0.05', value: '1', 'aria-label': 'Volume' }),
    speed: h('select', { class: 'pc-speed', 'aria-label': 'Playback speed', title: 'Playback speed' },
      [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4].map((s) => h('option', { value: String(s), text: s + 'x' }))),
    fs: h('button', { type: 'button', class: 'pc-btn', title: 'Fullscreen (f)', 'aria-label': 'Fullscreen' }),
  };
  ctl.play.append(icon('play'));
  ctl.mute.append(icon('volume'));
  ctl.fs.append(icon('fullscreen'));
  ctl.cleanupPoint = h('span', { id: 'player-cleanup-point', class: 'pc-cleanup-point', role: 'img', hidden: true });
  ctl.seekWrap = h('div', { class: 'pc-seek' }, h('span', { class: 'pc-track' }, ctl.avail, ctl.played), ctl.cleanupPoint, ctl.seek);
  ctl.bar = h('div', { class: 'pc-bar' }, ctl.play, ctl.time, ctl.seekWrap, ctl.mute, ctl.vol, ctl.speed, ctl.fs);

  const keepPrompt = {
    text: h('p', { class: 'pv-keep-text', role: 'status', 'aria-live': 'polite' }),
    keep: h('button', { type: 'button', class: 'primary', text: 'Keep', onclick: () => keepCurrentCopy() }),
    dismiss: h('button', { type: 'button', class: 'pc-btn', 'aria-label': 'Dismiss keep reminder', title: 'Dismiss keep reminder', onclick: () => {
      if (cur) cur.keepPromptDismissed = true;
      updateKeepPoint();
    } }, icon('close')),
  };
  keepPrompt.el = h('div', { class: 'pv-keep-prompt', hidden: true, 'aria-label': 'Keep this video' },
    keepPrompt.text, keepPrompt.keep, keepPrompt.dismiss);
  const player = h('div', { class: 'player', tabindex: '-1' }, h('div', { class: 'pv-stage' }, video, ov.el, keepPrompt.el), ctl.bar);
  const sourceChoice = h('select', { 'aria-label': 'Playback source' });
  const actionIcon=(name,iconName,action)=>h('button',{type:'button',class:'pc-btn',title:name,'aria-label':name,onclick:action},icon(iconName));
  const fit=actionIcon('Fill player','fit',()=>{fit.value=fit.value==='contain'?'cover':'contain';video.style.objectFit=fit.value;setAttr(fit,'aria-pressed',String(fit.value==='cover'));setAttr(fit,'title',fit.value==='cover'?'Fit whole video':'Fill player');});fit.value='contain';fit.setAttribute('aria-pressed','false');
  const loop=actionIcon('Loop video','loop');loop.setAttribute('aria-pressed','false');
  const pip=actionIcon('Picture in picture','pip');
  const theatre=actionIcon('Theatre mode','theatre');theatre.setAttribute('aria-pressed','false');
  ctl.bar.replaceChildren(ctl.seekWrap,h('div',{class:'pc-control-row'},
    h('div',{class:'pc-controls-main'},ctl.play,actionIcon('Back 10 seconds','skipBack',()=>seekBy(-10)),actionIcon('Forward 10 seconds','skipForward',()=>seekBy(10)),ctl.mute,ctl.vol,ctl.time),
    h('div',{class:'pc-controls-options'},sourceChoice,ctl.speed,fit,loop,pip,theatre,ctl.fs)));
  const notes = {
    notice: h('span', { class: 'pn-notice', role: 'status', 'aria-live': 'polite' }),
    engine: h('span', { class: 'pn-engine', title: 'Playback engine' }),
    stream: h('span', { class: 'pn-stream' }),
  };
  const nowTitle=h('span',{class:'now-playing-title'});
  const nowChannel=h('span',{class:'mini-channel'});
  const nowTime=h('span',{class:'mini-time'});
  const nowPreview=h('button',{type:'button',class:'mini-preview','aria-label':'Return to full player',onclick:()=>returnToPlayer()},icon('fullscreen'));
  const miniButton=(name,iconName,action,essential)=>h('button',{type:'button',class:'mini-control '+(essential?'mini-essential':'mini-secondary'),'aria-label':name,title:name,onclick:action},icon(iconName));
  const nowToggle=miniButton('Pause background video','pause',()=>togglePlay(),true);
  const nowPip=miniButton('Picture in picture','fullscreen',()=>togglePip());
  const nowReturn=miniButton('Return to video','expand',()=>returnToPlayer(),true);
  const nowStop=miniButton('Stop playback','close',()=>leave());
  const nowSeek=h('input',{type:'range',class:'mini-seek',min:'0',max:'1',step:'any',value:'0','aria-label':'Seek background video',onchange:()=>commitSeek(Number(nowSeek.value))});
  const nowPlaying=h('section',{class:'now-playing',hidden:true,'aria-label':'Background player'},
    h('div',{class:'mini-head'},nowPreview,h('div',{class:'mini-copy'},nowTitle,nowChannel)),
    nowSeek,h('div',{class:'mini-bottom'},nowTime,h('div',{class:'mini-controls'},nowToggle,nowPip,nowReturn,nowStop)));
  const sidebar=document.querySelector('#sidebar');sidebar.insertBefore(nowPlaying,sidebar.querySelector('.sidebar-note'));
  function returnToPlayer(){if(cur)M.activateTab(cur.tabId);}
  function placeMiniVideo(){
    if(cur?.background){
      const rect=nowPreview.getBoundingClientRect();
      Object.assign(video.style,{position:'fixed',left:rect.left+'px',top:rect.top+'px',width:rect.width+'px',height:rect.height+'px',zIndex:'20',visibility:'visible',pointerEvents:'none',objectFit:'cover',borderRadius:'3px'});
    } else {
      for(const property of ['position','left','top','width','height','z-index','visibility','pointer-events','border-radius'])video.style.removeProperty(property);
      video.style.objectFit=fit.value;
    }
  }
  window.addEventListener('resize',placeMiniVideo);
  if(window.ResizeObserver)new ResizeObserver(placeMiniVideo).observe(nowPreview);

  const W = {
    title: h('h1', { class: 'w-title' }),
    queue: h('section', { class: 'playlist-queue', hidden: true, 'aria-label': 'Playlist queue' }),
    channel: h('button', { type: 'button', class: 'link w-channel' }),
    views: h('span', { class: 'w-views muted' }),
    metaNote: h('p', { class: 'w-note', hidden: true }),
    desc: h('div', { class: 'w-desc' }),
    descToggle: h('button', { type: 'button', class: 'link w-desc-toggle', text: 'more', hidden: true }),
    cacheRows: h('ul', { class: 'cache-rows' }),
    cacheMsg: h('span', { class: 'slot cache-msg', role: 'status', 'aria-live': 'polite' }),
    commentsHead: h('h2', { class: 'h3' }),
    comments: h('div', { class: 'comments' }),
    commentsMore: h('button', { type: 'button', class: 'comments-more', text: 'Load more comments' }),
    commentsRefresh: h('button', { type: 'button', text: 'Refresh comments' }),
    commentsMsg: h('span', { class: 'slot comments-msg', role: 'status', 'aria-live': 'polite' }),
    related: h('div', { class: 'related' }),
  };
  const cacheButtons = [];
  function cacheBtn(label, onclick, cls) {
    const b = h('button', { type: 'button', class: cls || 'cache-btn', text: label, onclick });
    cacheButtons.push(b);
    return b;
  }
  W.save = cacheBtn('Keep video', () => saveToLibrary(), 'cache-btn save primary');
  const cacheBar = h('div', { class: 'cache-actions' },
    cacheBtn('Cache 360p', () => cacheQuality('360')),
    cacheBtn('Cache 480p', () => cacheQuality('480')),
    cacheBtn('Cache 720p', () => cacheQuality('720')),
    cacheBtn('Cache audio', () => cacheQuality('audio')),
    W.save);

  root.replaceChildren(
    h('div', { class: 'watch-grid' },
      h('div', { class: 'watch-main' },
        W.queue,
        player,
        h('div', { class: 'player-notes' }, notes.notice, notes.engine, notes.stream),
        W.title,
        h('div', { class: 'w-line' }, W.channel, W.views),
        W.metaNote,
        h('div', { class: 'w-descbox' }, W.desc, W.descToggle),
        h('section', { class: 'panel cache-panel', 'aria-label': 'Offline copy' },
          h('div', { class: 'panel-head' }, h('h2', { class: 'h3', text: 'Offline copy' }), W.cacheMsg),
          W.cacheRows, cacheBar, h('p', { class: 'muted', text: 'Temporary copies stay until mostly played, then follow your cleanup policy. Keep protects a copy.' })),
        h('section', { class: 'comments-sec', 'aria-label': 'Comments' },
          W.commentsHead, W.comments,
          h('div', { class: 'comments-foot' }, W.commentsMore, W.commentsRefresh, W.commentsMsg))),
      h('aside', { class: 'watch-side', 'aria-label': 'Related videos' }, h('h2', { class: 'h3', text: 'Related' }), W.related)));

  /* ------------------------------------------------------------------ state */

  let cur = null;            // the video currently shown
  let gen = 0;
  let uiTimer = null, reportTimer = null;
  const pendingStops = L.stopBarrier((entryId,generation)=>M.api('POST','/api/entries/'+encodeURIComponent(entryId)+'/playback/stop'+(generation!=null?'?generation='+encodeURIComponent(generation):''),{}));
  // `generation` names the viewing session being stopped: the backend ignores a stop for a
  // session that a newer prepare has since replaced or reused, so it cannot end the new one.
  function stopLocalPlayback(entryId,generation) {
    const stop=pendingStops.queue(entryId,generation);
    stop.catch(e=>M.flashError(e));
    return stop;
  }
  async function awaitLocalStop(entryId) {
    await pendingStops.wait(entryId);
  }
  const prefs = { volume: 1, muted: false, speed: 1 };
  try {
    const v = parseFloat(localStorage.getItem('muninn.volume')); if (isFinite(v)) prefs.volume = Math.max(0, Math.min(1, v));
    prefs.muted = localStorage.getItem('muninn.muted') === '1';
    const s = parseFloat(localStorage.getItem('muninn.speed')); if (isFinite(s) && s > 0) prefs.speed = s;
  } catch (e) { /* ignore */ }
  video.volume = prefs.volume; video.muted = prefs.muted;
  ctl.vol.value = String(prefs.volume);
  ctl.speed.value = String(prefs.speed);
  function savePref(k, v) { try { localStorage.setItem('muninn.' + k, String(v)); } catch (e) { /* ignore */ } }

  /* ------------------------------------------------------------------ small helpers */

  let noticeTimer = null;
  function notice(text, ms) {
    clearTimeout(noticeTimer);
    setText(notes.notice, text);
    if (text) noticeTimer = setTimeout(() => setText(notes.notice, ''), ms || 4500);
  }
  let cacheMsgTimer = null;
  function cacheMsg(text, kind) {
    clearTimeout(cacheMsgTimer);
    setText(W.cacheMsg, text);
    setAttr(W.cacheMsg, 'data-kind', kind || null);
    if (text) cacheMsgTimer = setTimeout(() => setText(W.cacheMsg, ''), 5000);
  }

  // Local compatibility playback is a window of the original video: playlist time 0 is
  // original time c.win.offset. Everything the user sees or saves (position, duration,
  // seek bar, progress, resume, cleanup point) is original time; only video.currentTime
  // is window time. c.win is null for every other kind of source.
  function mediaPos() { return L.toOriginal(cur && cur.win, video.currentTime); }
  function setMediaPos(t) { video.currentTime = L.toWindow(cur && cur.win, t); }
  function duration() {
    const c = cur;
    const videoDuration = video.readyState >= 1 && Number.isFinite(video.duration) ? L.toOriginal(c && c.win, video.duration) : null;
    return L.effectiveDuration(c && c.status && c.status.playback, c && c.detail && c.detail.meta, videoDuration);
  }
  function seekableEnd() {
    // Never touch video.seekable before the metadata is in: WebKitGTK's GStreamer player then
    // decides the stream is live and never learns the real duration of a finished file.
    if (video.readyState < 1) return 0;
    const s = video.seekable;
    return s && s.length ? s.end(s.length - 1) : 0;
  }
  function limit() {
    const c = cur;
    if (c && c.win) return L.windowLimit(c.win, duration());
    return L.seekLimit({ kind: c && c.kind, seekableEnd: c?.kind === 'hls' ? seekableEnd() : 0, duration: duration(), progress: c && c.status && c.status.playback ? c.status.playback.progress : null });
  }

  /* ------------------------------------------------------------------ overlay */

  function updateOverlay() {
    const c = cur;
    let mode = null, text = '', detail = '', failure = null;
    if (!c) mode = null;
    else if (c.error || c.mediaError) { mode = 'error'; failure = c.error || c.mediaError; text = failure.title; detail = failure.detail; }
    else if (!c.sourceSet) { mode = 'message'; text = L.overlayMessage(c.status, c.retrying); }
    else if (c.loadingEngine) { mode = 'message'; text = 'Loading the video engine...'; }
    else if (c.needsClick && video.paused) mode = 'play';
    else if (L.showSpinner(c, video.paused)) mode = 'spin';
    setHidden(ov.el, !mode);
    setAttr(ov.el, 'data-mode', mode);
    setHidden(ov.spinner, !(mode === 'message' || mode === 'spin'));
    setHidden(ov.text, !(mode === 'message' || mode === 'error'));
    setText(ov.text, text);
    setHidden(ov.detail, mode !== 'error');
    setText(ov.detail, detail);
    setHidden(ov.retry, !!failure && !failure.retryable);
    setHidden(ov.bigPlay, mode !== 'play');
    setHidden(ov.actions, mode !== 'error');
  }

  /* ------------------------------------------------------------------ control bar */

  // The loading feedback outlives attaching the source: it ends once the resume seek is
  // done and a frame is available, not when the player merely accepted the URL.
  function settleFrame(c) {
    if (!c || !c.awaitingFrame || !c.sourceSet || !c.resumeDone) return;
    if (L.frameSettled(c, video)) { c.awaitingFrame = false; c.buffering = false; updateOverlay(); }
  }

  let dragging = false;
  function updateControls() {
    updateNowPlaying();
    const c = cur;
    if(c&&!c.resumeDone&&c.sourceSet)maybeResume();
    if(c?.sourceSet&&!video.paused){
      const position=video.currentTime||0;
      if(c.lastProgressPosition!==position){
        c.lastProgressPosition=position;c.lastProgressAt=Date.now();
        if(c.buffering&&!c.awaitingFrame&&video.readyState>=2&&!video.seeking){c.buffering=false;updateOverlay();}
      }
      else if(!video.ended&&Date.now()-(c.lastProgressAt||Date.now())>10000&&!c.stallReported){
        c.stallReported=true;
        logPlaybackState(c,'stall');
        if(c.kind==='file'&&!c.localAttempted&&c.playbackEntry)prepareLocalPlayback(c,c.playbackEntry);
      }
    }
    const has = !!(c && c.sourceSet);
    const dur = duration();
    settleFrame(c);
    const pos = dragging ? Number(ctl.seek.value) : has ? (c.seekTarget != null ? c.seekTarget : mediaPos()) : (c && c.localEntry && c.resumeTarget) || 0;
    ctl.seek.disabled = !(has && dur > 0);
    ctl.seek.max = String(dur > 0 ? dur : 1);
    if (!dragging) ctl.seek.value = String(Math.min(pos, dur > 0 ? dur : 1));
    const lim = has ? limit() : 0;
    ctl.played.style.width = (L.fraction(pos, dur) * 100).toFixed(2) + '%';
    ctl.avail.style.width = (dur > 0 ? Math.min(1, (isFinite(lim) ? lim : dur) / dur) * 100 : 0).toFixed(2) + '%';
    setText(ctl.time, `${L.fmtClock(pos)} / ${dur > 0 ? L.fmtClock(dur) : '--:--'}`);
    const paused = video.paused || video.ended;
    const wantPlayIcon = paused ? 'play' : 'pause';
    if (ctl.play.dataset.icon !== wantPlayIcon) {
      ctl.play.dataset.icon = wantPlayIcon;
      ctl.play.replaceChildren(icon(wantPlayIcon));
      setAttr(ctl.play, 'aria-label', paused ? 'Play' : 'Pause');
      setAttr(ctl.play, 'title', paused ? 'Play (space)' : 'Pause (space)');
    }
    const muted = video.muted || video.volume === 0;
    if (ctl.mute.dataset.icon !== (muted ? 'mute' : 'volume')) {
      ctl.mute.dataset.icon = muted ? 'mute' : 'volume';
      ctl.mute.replaceChildren(icon(muted ? 'mute' : 'volume'));
      setAttr(ctl.mute, 'aria-label', muted ? 'Unmute' : 'Mute');
    }
    ctl.play.disabled = !has;
    setText(notes.stream, c ? L.streamLabel(c.status && c.status.playback) : '');
    updateKeepPoint();
  }

  function updateKeepPoint() {
    const c = cur;
    const entry = c?.detail?.entries?.find(e => e.id === c.playbackEntry);
    const point = c?.sourceSet && c.resumeDone && entry && !entry.protected && !c.keptEntries?.has(entry.id)
      ? L.cleanupPoint(c.completionPolicy, duration(), mediaPos()) : null;
    setHidden(ctl.cleanupPoint, !point);
    setAttr(ctl.seek, 'aria-describedby', point ? 'player-cleanup-point' : null);
    if (point) {
      const percent = c.completionPolicy.completion_percent;
      const delay = c.completionPolicy.cleanup_delay_seconds || 0;
      const timing = delay > 0 ? 'Cleanup ' + (delay < 60 ? `in ${delay} s` : L.relTime(Date.now() + delay * 1000)) : 'Cleanup after playback';
      const label = `Watched threshold: ${percent}% at ${L.fmtClock(point.seconds)}. ${timing}. Keep protects this copy.`;
      ctl.cleanupPoint.style.left = (point.fraction * 100) + '%';
      setAttr(ctl.cleanupPoint, 'aria-label', label);
      setAttr(ctl.seekWrap, 'title', label);
      if (c.keepPointSeconds !== point.seconds) { c.keepPointSeconds = point.seconds; c.keepPointReached = false; }
      if (point.reached) c.keepPointReached = true;
      setText(keepPrompt.text, c.keepPointError || `Watched threshold reached · ${timing.toLowerCase()}.`);
    } else setAttr(ctl.seekWrap, 'title', null);
    setHidden(keepPrompt.el, !point || !c.keepPointReached || c.keepPromptDismissed || c.background || c.error || c.mediaError);
    keepPrompt.keep.disabled = !!c?.keepPointBusy;
    setText(keepPrompt.keep, c?.keepPointBusy ? 'Keeping…' : 'Keep');
  }

  async function keepCurrentCopy() {
    const c = cur, id = c?.playbackEntry;
    if (!c || !id || c.keepPointBusy) return;
    c.keepPointBusy = true; c.keepPointError = null; updateKeepPoint();
    try {
      await M.api('POST', '/api/entries/' + encodeURIComponent(id) + '/promote');
      if (!c.keptEntries) c.keptEntries = new Set();
      c.keptEntries.add(id);
      if (c === cur) { notice('Copy kept in your library'); fetchDetail(c); }
    } catch (e) {
      c.keepPointError = 'Could not keep this copy. Try again.';
    } finally {
      c.keepPointBusy = false;
      if (c === cur) updateKeepPoint();
    }
  }

  function togglePlay() {
    if (!cur || !cur.sourceSet) return;
    if (video.paused || video.ended) {
      cur.playIntent=true;
      if (video.ended && cur.win && L.windowOffset(cur.win) > 0) { commitSeek(0); return; }   // replay needs a window at 0
      if (video.ended) video.currentTime = 0;
      cur.needsClick = false;
      const p = video.play();
      if (p && p.catch) p.catch(() => { cur.needsClick = true; updateOverlay(); });
    } else {cur.playIntent=false;video.pause();}
  }
  function commitSeek(t) {
    if (!cur || !cur.sourceSet) return;
    cur.resumeDone = true;     // a manual seek beats a pending resume
    cur.resumeTarget=null;
    clearTimeout(cur.seekTimer);cur.seekTarget=null;
    if (cur.win) {
      // Local window: a seek inside the prepared window is a plain seek. Anywhere else a
      // window near the target is prepared (debounced; the backend runs one job at a time).
      const plan = L.seekPlan(cur.win, t, duration());
      if (plan.kind === 'window') { try { video.currentTime = plan.time; } catch (e) { /* not seekable yet */ } }
      else {
        const c = cur;
        c.seekTarget = plan.target; c.buffering = true; c.awaitingFrame = true; updateOverlay();
        c.seekTimer = setTimeout(() => { if (L.seekDue(c, cur, plan.target)) prepareLocalPlayback(c, c.localEntry, plan.target); }, L.SEEK_DEBOUNCE_MS);
      }
      updateControls();
      return;
    }
    const r = L.clampSeek(t, { duration: duration(), limit: limit() });
    try { video.currentTime = r.time; } catch (e) { /* not seekable yet */ }
    if (r.clamped) notice(L.clampNoticeText(r.limit));
    updateControls();
  }
  function seekBy(d) { commitSeek(mediaPos() + d); }
  function toggleMute() { video.muted = !video.muted; prefs.muted = video.muted; savePref('muted', video.muted ? '1' : '0'); updateControls(); }
  loop.addEventListener('click', () => { video.loop = !video.loop; setAttr(loop, 'aria-pressed', String(video.loop)); });
  function sizeTheatre() {
    if(root.classList.contains('theatre'))root.style.setProperty('--theatre-stage-height',Math.max(200,document.querySelector('#content').clientHeight-ctl.bar.offsetHeight)+'px');
  }
  window.addEventListener('resize',sizeTheatre);
  if(window.ResizeObserver){const observer=new ResizeObserver(sizeTheatre);observer.observe(document.querySelector('#content'));observer.observe(ctl.bar);}
  async function toggleTheatre() {
    const before=player.getBoundingClientRect().toJSON();
    const wasFullscreen=!!fsEl()||player.classList.contains('expanded');
    if(fsEl()) {try{await (document.exitFullscreen||document.webkitExitFullscreen).call(document);}catch(e){M.flashError(e);return;}}
    player.classList.remove('expanded');
    const on = root.classList.toggle('theatre');
    sizeTheatre();
    setAttr(theatre, 'aria-pressed', String(on));
    setAttr(theatre, 'title', on ? 'Exit theatre mode' : 'Theatre mode');
    requestAnimationFrame(()=>M.api('POST','/api/logs',{event:'ui.player.theatre',message:JSON.stringify({enabled:on,before,after:player.getBoundingClientRect().toJSON(),stage:video.parentElement.getBoundingClientRect().toJSON(),was_fullscreen:wasFullscreen,background:!!cur?.background,width:innerWidth,height:innerHeight,version:M.boot?.version})}).catch(()=>{}));
  }
  theatre.addEventListener('click', toggleTheatre);
  const inPip = () => document.pictureInPictureElement === video || video.webkitPresentationMode === 'picture-in-picture';
  async function exitPip() {
    try {
      if (document.pictureInPictureElement === video) await document.exitPictureInPicture();
      else if (video.webkitPresentationMode === 'picture-in-picture') video.webkitSetPresentationMode('inline');
    } catch (e) { /* already closed by the system */ }
  }
  function updateNowPlaying() {
    nowPlaying.hidden=!cur||!cur.background;
    nowPreview.classList.toggle('in-pip',inPip());
    document.body.classList.toggle('has-playback-mini',!!cur?.background);
    if(cur){
      const title=cur.detail?.meta?.title||cur.videoId;
      setText(nowTitle,title);setAttr(nowPlaying,'title',(video.paused?'Paused: ':'Playing: ')+title);
      setText(nowChannel,cur.detail?.meta?.channel||'');
      const durationValue=duration();
      nowSeek.disabled=!(durationValue>0);nowSeek.max=String(durationValue||1);
      if(document.activeElement!==nowSeek)nowSeek.value=String(mediaPos());
      setText(nowTime,L.fmtClock(mediaPos())+' / '+L.fmtClock(durationValue));
      nowToggle.replaceChildren(icon(video.paused?'play':'pause'));
      setAttr(nowToggle,'aria-label',video.paused?'Play background video':'Pause background video');
      setAttr(nowPreview,'title','Return to '+title);
    }
    pip.disabled=nowPip.disabled=M.settings.pip_mode==='disabled'||!cur?.sourceSet||video.readyState===0;
    setAttr(pip,'aria-pressed',String(inPip()));
    placeMiniVideo();requestAnimationFrame(placeMiniVideo);
  }
  async function enterPip(automatic) {
    const c = cur;
    if (!c || !c.sourceSet || c.pipPending || M.settings.pip_mode === 'disabled') return;
    c.pipPending = true;
    try {
      if (video.requestPictureInPicture && document.pictureInPictureEnabled) await video.requestPictureInPicture();
      else if (video.webkitSetPresentationMode && (!video.webkitSupportsPresentationMode || video.webkitSupportsPresentationMode('picture-in-picture'))) video.webkitSetPresentationMode('picture-in-picture');
      else throw new Error('Picture in picture is unavailable in this web engine.');
      if (c !== cur) { await exitPip(); return; }
      c.pipSessionStarted = true;
      if (automatic && !c.background && M.settings.pip_return_to_inline !== false) await exitPip();
    } catch (e) {
      if (c === cur) {
        const fallback = c.background && M.settings.background_playback ? ' Continuing audio in the background.' : (c.background ? ' Playback paused.' : '');
        M.flash('info', M.errText(e) + fallback, 7000);
        if (c.background && !M.settings.background_playback) video.pause();
      }
    } finally { c.pipPending = false; updateNowPlaying(); }
  }
  function togglePip() { if (inPip()) exitPip(); else enterPip(false); }
  function prepareBackground(destinationTab) {
    const c = cur;
    if (!c || c.tabId === destinationTab || inPip() || c.pipPending || video.paused || video.ended || !c.sourceSet) return;
    if (M.settings.pip_mode === 'automatic') {
      c.background = true;
      // Call the browser API before yielding away the navigation click's user activation.
      enterPip(true);
    }
  }
  function pipChanged() {
    if (cur && inPip()) cur.pipSessionStarted = true;
    else if (cur?.background && !cur.pipPending && M.settings.pause_when_pip_closes !== false) video.pause();
    updateNowPlaying();
  }
  video.addEventListener('enterpictureinpicture', pipChanged);
  video.addEventListener('leavepictureinpicture', pipChanged);
  video.addEventListener('webkitpresentationmodechanged', pipChanged);
  pip.addEventListener('click', togglePip);
  function detachSource(c) {
    report(); c.sourceSet = false;
    c.loadingEngine=false;c.buffering=false;c.needsClick=false;c.resumePending=null;
    video.pause(); destroyHls(); video.removeAttribute('src'); video.load();
  }
  function playCopy(entry) {
    const c = cur; if (!c) return;
    if(c.playbackEntry===entry.id&&c.sourceSet){startPlayback(c);return;}
    const attached=c.sourceSet&&!!video.currentSrc;
    const position = !c.resumeDone&&c.resumeTarget!=null ? c.resumeTarget : attached ? mediaPos() : c.resumeTarget ?? c.detail?.position_seconds ?? 0;
    if(c.localEntry)stopLocalPlayback(c.localEntry,c.localGeneration);
    c.localEntry=null;c.localAttempted=false;c.localGeneration=null;
    c.sourceGeneration=(c.sourceGeneration||0)+1;
    clearTimeout(c.pollTimer);clearTimeout(c.seekTimer);c.seekTarget=null;
    if(attached)c.playIntent=!video.paused;
    c.resumeTarget=position;
    detachSource(c); c.win = null; c.resumeDone = false; c.error = c.mediaError = null;
    c.detail = Object.assign({}, c.detail, { position_seconds: position });
    c.status = Object.assign({}, c.status, { playback: { entry_id: entry.id, kind: 'file', state: 'cached', ready: true, height: entry.actual_height, variant: entry.variant, progress: 1, url_path: entry.media_url } });
    c.explicitPlayback=c.status.playback;
    setSource(c, c.status); updateControls();
    if(!c.localEntry)pollLoop(c);
  }
  sourceChoice.addEventListener('change', () => {
    const e = cur?.detail?.entries?.find((e) => e.id === sourceChoice.value);
    if (e?.status === 'complete') playCopy(e);
    else if (sourceChoice.value.startsWith('download:')) cacheQuality(sourceChoice.value.slice(9));
  });

  const fsEl = () => document.fullscreenElement || document.webkitFullscreenElement || null;
  function toggleFullscreen() {
    if (fsEl()) { (document.exitFullscreen || document.webkitExitFullscreen).call(document); return; }
    if (player.classList.contains('expanded')) { player.classList.remove('expanded'); M.scheduleLayout(); return; }
    const req = player.requestFullscreen || player.webkitRequestFullscreen;
    if (req) {
      try { const p = req.call(player); if (p && p.catch) p.catch(() => expand()); } catch (e) { expand(); return; }
      // WKWebView may ignore element fullscreen without telling us: fill the window instead.
      setTimeout(() => { if (!fsEl() && cur) expand(); }, 350);
    } else expand();
  }
  function expand() { player.classList.add('expanded'); }
  document.addEventListener('keydown', (ev) => { if (ev.key === 'Escape' && player.classList.contains('expanded')) { player.classList.remove('expanded'); } });

  ctl.play.addEventListener('click', togglePlay);
  ov.bigPlay.addEventListener('click', togglePlay);
  ctl.mute.addEventListener('click', toggleMute);
  ctl.fs.addEventListener('click', toggleFullscreen);
  video.addEventListener('click', togglePlay);
  video.addEventListener('dblclick', toggleFullscreen);
  ctl.vol.addEventListener('input', () => { video.volume = Number(ctl.vol.value); video.muted = false; prefs.volume = video.volume; prefs.muted = false; savePref('volume', video.volume); savePref('muted', '0'); updateControls(); });
  ctl.speed.addEventListener('change', () => { prefs.speed = Number(ctl.speed.value); video.playbackRate = prefs.speed; video.defaultPlaybackRate = prefs.speed; savePref('speed', prefs.speed); });
  ctl.seek.addEventListener('pointerdown', () => { dragging = true; });
  const endDrag = () => { dragging = false; };
  window.addEventListener('pointerup', endDrag);
  ctl.seek.addEventListener('input', () => { setText(ctl.time, `${L.fmtClock(Number(ctl.seek.value))} / ${duration() > 0 ? L.fmtClock(duration()) : '--:--'}`); });
  ctl.seek.addEventListener('change', () => { commitSeek(Number(ctl.seek.value)); dragging = false; });
  ov.retry.addEventListener('click', () => retry());
  ov.ext.addEventListener('click', () => { if (cur) M.invoke('open_external', { url: 'https://www.youtube.com/watch?v=' + cur.videoId }).catch((e) => M.flash('error', M.errText(e))); });

  document.addEventListener('fullscreenchange', () => { updateControls(); M.scheduleLayout(); });

  // Player keyboard shortcuts: only while the watch view is up and no dialog is open.
  window.addEventListener('keydown', (ev) => {
    if (!cur || cur.background || document.querySelector('dialog[open]') || ev.isComposing) return;
    const t = ev.target || {};
    const act = L.playerKeyAction(ev, { tag: t.tagName, type: t.type, editable: !!t.isContentEditable });
    if (!act) return;
    if((act==='queue-next'||act==='queue-previous')&&cur.queue){ev.preventDefault();advanceQueue(act==='queue-next'?1:-1,false);return;}
    ev.preventDefault();
    if (act === 'toggle') togglePlay();
    else if (act === 'fullscreen') toggleFullscreen();
    else if (act === 'mute') toggleMute();
    else if (act === 'theatre') toggleTheatre();
    else if (act.seek) seekBy(act.seek);
    else if (act.percent != null && duration() > 0) commitSeek(duration() * act.percent / 100);
    else if (act.start) commitSeek(0);
    else if (act.end && duration() > 0) commitSeek(duration());
    else if (act.volume) { video.volume = Math.max(0, Math.min(1, video.volume + act.volume / 100)); video.muted = false; updateControls(); }
    else if (act.speed) { const rates = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2]; const i = rates.indexOf(video.playbackRate); const next = rates[Math.max(0, Math.min(rates.length - 1, i + act.speed))]; video.playbackRate = next; video.defaultPlaybackRate = next; ctl.speed.value = String(next); savePref('speed', next); }
    else if (act === 'pip') togglePip();
    else if (act === 'frame-back' || act === 'frame-forward') { if (video.paused) seekBy(act === 'frame-back' ? -1 / 30 : 1 / 30); }
  });

  /* ------------------------------------------------------------------ playback reporting */

  function report(extra) {
    const c = cur;
    if (!c || !c.sourceSet || !c.resumeDone || video.readyState === 0) return;
    const dur = duration();
    const pb = c.status && c.status.playback;
    const body = {
      video_id: c.videoId,
      started: !!c.started,
      position_seconds: Math.floor(mediaPos()),
      duration_seconds: dur > 0 ? Math.round(dur) : null,
      watch_height: c.playbackHeight || video.videoHeight || (pb && pb.height) || null,
      paused: video.paused,
    };
    if (extra) Object.assign(body, extra);
    M.api('POST', '/api/ingest/playback', body, { keepalive: true }).then(result => {
      if(c.started&&!c.featuredReported){c.featuredReported=true;M.refreshFeatured?.(true);}
      if (c === cur && result.completion_policy) { c.completionPolicy = result.completion_policy; updateKeepPoint(); }
    }).catch(() => {});
  }
  function startReporter() { if (!reportTimer) reportTimer = setInterval(() => report(), 5000); }
  function stopReporter() { clearInterval(reportTimer); reportTimer = null; }

  video.addEventListener('play', () => {
    if (cur) {
      cur.needsClick = false;
      // A pause is not a decoder stall. Start a fresh progress window before
      // updateControls can inspect the clock, rather than waiting for `playing`.
      cur.lastProgressAt = Date.now();
      cur.lastProgressPosition = video.currentTime || 0;
      cur.stallReported = false;
    }
    startReporter(); updateControls(); updateOverlay();
  });
  video.addEventListener('playing', () => { if (cur) { cur.started=true; report(); if(!cur.awaitingFrame)cur.buffering = false; cur.mediaError = null; cur.lastProgressAt=Date.now();cur.stallReported=false;logPlaybackState(cur,'playing'); } startReporter(); updateControls(); updateOverlay(); });
  video.addEventListener('pause', () => { stopReporter(); report(); updateControls(); updateOverlay(); });
  video.addEventListener('ended', () => { if(cur)cur.buffering=false;stopReporter(); report({ ended: true }); if(cur) advanceQueue(1, true); updateControls();updateOverlay(); });
  video.addEventListener('seeked', () => { maybeResume();report(); settleFrame(cur); updateControls(); updateOverlay(); });
  video.addEventListener('waiting', () => { if (cur && cur.sourceSet) { cur.buffering = true; updateOverlay(); } });
  video.addEventListener('canplay', () => { if (cur) { if(!cur.awaitingFrame)cur.buffering = false; maybeResume(); settleFrame(cur); updateOverlay(); } });
  video.addEventListener('timeupdate', updateControls);
  video.addEventListener('durationchange', updateControls);
  video.addEventListener('volumechange', updateControls);
  video.addEventListener('loadedmetadata', () => {
    video.playbackRate = prefs.speed;
    maybeResume();
    updateControls();
  });
  video.addEventListener('error', () => {
    const c = cur;
    if (!c || !c.sourceSet) return;
    const err = video.error;
    const code = err ? err.code : 0;
    logPlaybackState(c,'error');
    if(c.kind==='file' && (code===3||code===4) && !c.localAttempted && c.playbackEntry){prepareLocalPlayback(c,c.playbackEntry);return;}
    c.mediaError = c.kind === 'hls' && !c.hlsOk
      ? L.failureView('engine_unsupported')
      : L.failureView('media_error');
    logFailureDetail(c, 'media_element', c.mediaError, 'media error ' + code + ' ' + (err && err.message || ''));
    c.buffering = false;
    updateOverlay();
  });
  window.addEventListener('pagehide', () => report());

  /** Breadcrumb for a failure the player replaces with a generic message: category plus redacted, bounded detail. */
  function logFailureDetail(c, source, failure, detail) {
    if (!c) return;
    const text = L.redactDetail(detail, 300);
    const key = source + '|' + (failure && failure.reason) + '|' + text;
    if (c.loggedFailure === key) return;
    c.loggedFailure = key;
    M.api('POST', '/api/logs', { event: 'ui.playback.error', message: JSON.stringify({ video_id: c.videoId, failure: source, reason: failure && failure.reason, detail: text, version: M.boot?.version }) }).catch(() => {});
  }

  function logPlaybackState(c,event,reason) {
    const haveMetadata = video.readyState >= 1;
    const entry=c.detail?.entries?.find(e=>e.id===c.playbackEntry);
    M.api('POST','/api/logs',{event:'ui.playback.'+event,message:JSON.stringify({video_id:c.videoId,entry_id:c.playbackEntry,container:entry?.ext,kind:c.kind,
      code:video.error?.code,message:video.error?.message||'',time:video.currentTime,window_offset:c.win?c.win.offset:0,original_time:mediaPos(),duration:haveMetadata&&Number.isFinite(video.duration)?video.duration:null,
      ready_state:video.readyState,network_state:video.networkState,paused:video.paused,seeking:video.seeking,width:video.videoWidth,height:video.videoHeight,
      decoded_frames:haveMetadata?(video.getVideoPlaybackQuality?.().totalVideoFrames??video.webkitDecodedFrameCount):null,
      buffered:L.mediaRanges(video,'buffered'),seekable:L.mediaRanges(video,'seekable'),local_compatibility:!!c.localEntry,
      reason,version:M.boot?.version,playback_rate:video.playbackRate,source:video.currentSrc.replaceAll(M.token||'__none__','[redacted]'),
      backend_state:c.status?.playback?.state,prepared_seconds:c.status?.playback?.prepared_seconds,prepared_until:c.status?.playback?.prepared_until,job_id:c.status?.playback?.job_id,backend_ready:c.status?.playback?.ready,overlay:ov.el.dataset.mode,
      resume_target:c.resumeTarget,resume_pending:c.resumePending?.target,resume_done:c.resumeDone,
      user_agent:navigator.userAgent,mp4_support:video.canPlayType('video/mp4; codecs="avc1.42E01E,mp4a.40.2"'),webm_support:video.canPlayType('video/webm; codecs="vp9,opus"')})}).catch(()=>{});
  }
  for(const event of ['play','pause','loadstart','loadedmetadata','waiting','stalled','seeking','seeked','canplay','ratechange'])video.addEventListener(event,()=>{
    const c=cur;if(!c?.sourceSet)return;
    c.stateEvents=c.stateEvents||{};
    if(Date.now()-(c.stateEvents[event]||0)<1000)return;
    c.stateEvents[event]=Date.now();logPlaybackState(c,'state',event);
  });

  // Prepares (or reuses) a local window near `target` (original seconds; default: the
  // resume position). The window is attached by applyStatus once the backend reports it ready.
  // Each prepare carries this page's token and a counter so the backend can refuse one that
  // is handled after a newer one (it would otherwise hand back, and act on, a stale session).
  const prepareClient=Date.now().toString(36)+Math.random().toString(36).slice(2,8);
  let prepareSeq=0;
  async function prepareLocalPlayback(c,entryId,target) {
    const seq=++prepareSeq;
    const selection=c.sourceGeneration=(c.sourceGeneration||0)+1;
    c.localAttempted=true;c.localEntry=entryId;
    clearTimeout(c.seekTimer);c.seekTarget=null;
    const attached=c.sourceSet&&!!video.currentSrc;
    const position=target!=null ? target : !c.resumeDone&&c.resumeTarget!=null ? c.resumeTarget : attached ? mediaPos() : c.resumeTarget ?? c.detail?.position_seconds ?? 0;
    if(attached&&!video.ended)c.playIntent=!video.paused;
    c.resumeTarget=position;
    detachSource(c);c.win=null;c.mediaError=null;c.error=null;c.resumeDone=false;
    c.awaitingFrame=true;c.buffering=true;
    c.detail=Object.assign({},c.detail,{position_seconds:position});
    c.status=Object.assign({},c.status,{playback:{local:true,state:'resolving',ready:false}});updateOverlay();
    clearTimeout(c.pollTimer);
    try {
      await awaitLocalStop(entryId);
      if(c!==cur||c.sourceGeneration!==selection)return;
      const playback=await M.api('POST','/api/entries/'+encodeURIComponent(entryId)+'/playback?position='+encodeURIComponent(Math.max(0,Math.floor(position*1000)/1000))+'&client='+prepareClient+'&seq='+seq,{});
      if(c!==cur||c.sourceGeneration!==selection){stopLocalPlayback(entryId,playback.generation);return;}
      c.localGeneration=playback.generation;
      applyStatus(c,Object.assign({},c.status,{playback}));pollLoop(c);
    }catch(error){if(c===cur&&c.sourceGeneration===selection){c.error=L.failureView('media_error');logFailureDetail(c,'local_prepare',c.error,M.errText(error));updateOverlay();}}
  }

  /* ------------------------------------------------------------------ resume */

  function maybeResume() {
    const c = cur;
    if (!c || !c.sourceSet || c.resumeDone) return;
    if (!c.detail && !c.detailFailed) return;            // wait for the saved position
    if (video.readyState < 1) return;                     // metadata not there yet
    const dur = duration();
    const pos = c.resumeTarget!=null ? Math.max(0,Math.min(c.resumeTarget,dur||Infinity)) : L.resumePosition(c.detail?.position_seconds, dur);    // original time
    // Zero is an intentional start too. Growing EVENT HLS may choose its live
    // edge; metadata alone does not prove the media has started at the beginning.
    if(pos===0&&video.readyState<2)return;
    if(!video.seeking&&Math.abs(mediaPos()-pos)<1){c.resumeDone=true;c.resumePending=null;if(pos>0)notice('Resumed at '+L.fmtClock(pos));return;}
    if(c.resumePending&&(video.seeking||Date.now()-c.resumePending.at<3000))return;
    if(pos>limit())return;
    const windowPos=L.toWindow(c.win,pos), end=seekableEnd();
    if(end>0&&windowPos>end)return;
    c.resumePending={target:pos,at:Date.now()};
    try { video.currentTime = windowPos; } catch (e) { c.resumePending=null; }
  }

  /* ------------------------------------------------------------------ source + status */

  let hlsLoad = null;
  /** Loads the vendored hls.js on demand (only when the browser has no native HLS). */
  function loadHlsJs() {
    if (window.Hls) return Promise.resolve(window.Hls);
    if (!hlsLoad) {
      hlsLoad = new Promise((resolve, reject) => {
        const s = document.createElement('script');
        s.src = '/ui/hls.min.js';
        s.onload = () => (window.Hls ? resolve(window.Hls) : reject(new Error('hls.js did not initialise')));
        s.onerror = () => { hlsLoad = null; reject(new Error('could not load hls.js')); };
        document.head.append(s);
      });
    }
    return hlsLoad;
  }
  function destroyHls() {
    if (hls) { try { hls.destroy(); } catch (e) { /* ignore */ } hls = null; }
  }
  let hls = null;

  function setEngine(c, name) {
    c.engine = name;
    setAttr(player, 'data-engine', name || null);
    setText(notes.engine, '');
  }

  function startPlayback(c) {
    if(c.playIntent===false){video.pause();return;}
    const selection=c.sourceGeneration||0, source=c.sourceKey;
    const p = video.play();
    if (p && p.catch) p.catch((error) => { if (c === cur && (c.sourceGeneration||0)===selection && c.sourceKey===source) { logPlaybackState(c,'state','play-rejected:'+(error?.name||'unknown'));c.needsClick = true; updateOverlay(); } });
  }

  function setSource(c, st) {
    const pb = st.playback;
    const ext = pb.ext || c.detail?.entries?.find(e => e.id === pb.entry_id)?.ext;
    if (pb.kind === 'file' && ['mkv','webm'].includes(ext) && !c.localAttempted) {
      c.playbackEntry = pb.entry_id;
      prepareLocalPlayback(c, pb.entry_id);
      return;
    }
    const url = L.mediaUrl(pb, M.origin, M.token);
    if (!url) return;
    c.kind = pb.kind;
    c.lastProgressPosition=video.currentTime||0;c.lastProgressAt=Date.now();c.stallReported=false;
    c.sourceSet = true;
    c.sourceKey = pb.kind + ':' + pb.url_path;
    const selection=c.sourceGeneration||0, source=c.sourceKey;
    const ownsSource=()=>c===cur && (c.sourceGeneration||0)===selection && c.sourceKey===source;
    c.playbackEntry = pb.entry_id;
    sourceChoice.value=pb.entry_id||'';
    if(pb.local){
      c.localEntry=pb.entry_id;c.localAttempted=true;c.localGeneration=pb.generation;
      c.win={job:pb.job_id,offset:pb.offset_seconds||0,until:pb.prepared_until,complete:!!pb.window_complete};
      // Attaching is not playing: keep the loading feedback until a frame is decoded.
      c.awaitingFrame=true;c.buffering=true;updateOverlay();
    } else {c.win=null;c.awaitingFrame=false;}
    c.seekTarget=null;
    c.playbackHeight = pb.height;
    video.defaultPlaybackRate = prefs.speed;
    const native = !!(video.canPlayType && video.canPlayType('application/vnd.apple.mpegurl'));
    if (pb.kind !== 'hls') { setEngine(c, 'file'); video.src = url; startPlayback(c); return; }
    if (native || M.isMock && !M.mockHlsJs) {
      c.hlsOk = true;
      setEngine(c, M.isMock && !native ? 'mock' : 'native HLS');
      video.src = url; startPlayback(c); return;
    }
    // No native HLS (WebKitGTK, Chromium): fall back to the vendored hls.js, only now.
    c.hlsOk = false;
    c.loadingEngine = true;
    updateOverlay();
    loadHlsJs().then((Hls) => {
      if (!ownsSource()) return;
      c.loadingEngine = false;
      if (!Hls.isSupported()) throw new Error('this browser supports neither native HLS nor Media Source');
      destroyHls();
      hls = new Hls({ startPosition: 0, maxBufferLength: 30, maxMaxBufferLength: 60, backBufferLength: 30, lowLatencyMode: false, enableWorker: false });
      hls.on(Hls.Events.ERROR, (_e, data) => {
        if (!ownsSource() || !data || !data.fatal) return;
        c.mediaError = L.failureView('media_error');
        logFailureDetail(c, 'hls', c.mediaError, [data.type, data.details].filter(Boolean).join(' '));
        c.buffering = false;
        updateOverlay();
      });
      c.hlsOk = true;
      setEngine(c, 'hls.js');
      hls.loadSource(url);
      hls.attachMedia(video);
      startPlayback(c);
    }).catch((e) => {
      if (!ownsSource()) return;
      c.loadingEngine = false;
      c.mediaError = L.failureView('engine_unsupported');
      logFailureDetail(c, 'engine', c.mediaError, M.errText(e));
      updateOverlay();
    });
  }

  function applyStatus(c, st) {
    if (c !== cur || !st) return;
    const prev = c.status;
    c.status = st;
    if (st.completion_policy) c.completionPolicy = st.completion_policy;
    if (st.meta_state === 'ready' && (!prev || prev.meta_state !== 'ready' || prev.comments_count !== st.comments_count || prev.comments_loading && !st.comments_loading)) fetchDetail(c);
    renderMetaState(c);
    if (!c.commentsBusy) setHidden(W.commentsMore, !st.has_more_comments);
    const failure = L.playbackFailure(st);
    if (failure) {
      if (c.sourceSet) { video.pause(); }
      c.error = failure;
      logFailureDetail(c, st.playback.local ? 'local_status' : 'status', failure, st.playback.error || st.playback.phase);
    } else {
      c.error = null;
      const pb = st.playback || {};
      if(pb.local && c.win && pb.job_id===c.win.job){          // keep the attached window's extent current
        c.win.until=pb.prepared_until;c.win.complete=!!pb.window_complete;c.localGeneration=pb.generation;
      }
      if(pb.local && !c.sourceSet && pb.state!=='error'){
        const saved=c.resumeTarget!=null ? c.resumeTarget : L.resumePosition(c.detail?.position_seconds,duration());
        // The window is chosen around the resume position, so it must be known before attaching.
        if(c.resumeTarget==null && !c.detail && !c.detailFailed)pb.ready=false;
        else if(!L.localFits(pb,saved) && pb.entry_id && L.mayRestartLocal(c)){
          c.localRestarts=(c.localRestarts||0)+1;
          // The backend prepared another window (for example from the saved position of an
          // earlier session): ask for one that covers where this session resumes.
          c.localRestarting=true;prepareLocalPlayback(c,pb.entry_id,saved).finally(()=>{c.localRestarting=false;});return;
        }
        if(pb.ready && !L.localReady(pb,saved))pb.ready=false;
      }
      if (pb.state === 'none') {
        c.noneSince = c.noneSince || Date.now();
        if (Date.now() - c.noneSince > 6000) c.error = L.failureView('start_stalled');
      } else c.noneSince = 0;
      if (!c.error && !c.sourceSet && L.startStalled(c.beganAt, Date.now(), st)) c.error = L.failureView('start_stalled');
      if (pb.ready && !c.sourceSet) setSource(c, st);
    }
    updateOverlay();
    updateControls();
  }

  function pollLoop(c) {
    if (c !== cur) return;
    c.pollTimer = setTimeout(async () => {
      if (c !== cur) return;
      const selection=c.sourceGeneration||0;
      try {
        const st = await M.api('GET', '/api/watch/' + c.videoId);
        if(c.localEntry)st.playback=await M.api('GET','/api/entries/'+encodeURIComponent(c.localEntry)+'/playback');
        else if(c.explicitPlayback)st.playback=c.explicitPlayback;
        if (c !== cur || (c.sourceGeneration||0)!==selection) return;
        applyStatus(c, st);
        const active = c.detail && (c.detail.entries || []).some((e) => L.isActiveEntry(e));
        if (c.detail && active && Date.now() - c.detailAt > 3000) fetchDetail(c);
        if (L.watchSettled(st) && !active) { c.pollTimer = null; return; }
        c.pollDelayMs = L.pollDelay(st);
      } catch (e) { if(c!==cur||(c.sourceGeneration||0)!==selection)return;c.pollDelayMs = 2000; }
      pollLoop(c);
    }, c.pollDelayMs || 700);
  }

  async function startWatch(c) {
    const selection=c.sourceGeneration||0;
    try {
      await pendingStops.waitVideo(c.videoId);
      if(c!==cur||(c.sourceGeneration||0)!==selection)return;
      const st = await M.api('POST', '/api/watch/' + c.videoId);
      if (c !== cur || (c.sourceGeneration||0)!==selection) return;
      applyStatus(c, st);
      if (!(L.watchSettled(st))) pollLoop(c);
    } catch (e) {
      if (c !== cur || (c.sourceGeneration||0)!==selection) return;
      c.error = L.failureView('unknown');
      logFailureDetail(c, 'status_poll', c.error, M.errText(e));
      updateOverlay();
    }
  }

  async function fetchDetail(c) {
    try {
      const d = await M.api('GET', '/api/video/' + c.videoId);
      if (c !== cur) return;
      c.detail = d; c.detailAt = Date.now();
      renderDetail(c);
      maybeResume();
    } catch (e) {
      if (c !== cur) return;
      c.detailAt = Date.now();
      if (!c.detail) { c.detailFailed = true; maybeResume(); }
    }
  }

  /* ------------------------------------------------------------------ detail rendering */

  function renderMetaState(c) {
    const st = c.status;
    const meta = c.detail && c.detail.meta;
    let note = '';
    if (st && st.meta_state === 'error') note = 'Could not load video info: ' + (st.meta_error || 'unknown error');
    else if (!meta && (!st || st.meta_state === 'loading')) note = 'Loading video info...';
    setText(W.metaNote, note);
    setHidden(W.metaNote, !note);
    setAttr(W.metaNote, 'data-kind', st && st.meta_state === 'error' ? 'error' : null);
  }

  function renderDetail(c) {
    const d = c.detail;
    if (!d) return;
    const meta = d.meta || {};
    const entries = d.entries || [];
    const e0 = entries[0] || {};
    setText(W.title, meta.title || e0.title || (c.status && c.status.meta_state === 'error' ? c.videoId : 'Loading...'));
    const chName = meta.channel || e0.channel || '';
    setText(W.channel, chName);
    setHidden(W.channel, !chName);
    W.channel.disabled = !meta.channel_id;
    setAttr(W.channel, 'title', meta.channel_id ? 'Open channel' : null);
    setText(W.views, L.viewsLine(meta));
    // description
    const desc = meta.description || '';
    if (W.desc.textContent !== desc) {
      W.desc.textContent = desc;
      W.desc.classList.add('collapsed');
      setText(W.descToggle, 'more');
      requestAnimationFrame(() => setHidden(W.descToggle, !(W.desc.scrollHeight > W.desc.clientHeight + 2)));
    }
    renderMetaState(c);
    renderCache(c);
    renderComments(c);
    renderRelated(c);
    updateControls();
  }

  W.descToggle.addEventListener('click', () => {
    const col = W.desc.classList.toggle('collapsed');
    setText(W.descToggle, col ? 'more' : 'less');
  });
  W.channel.addEventListener('click', () => { const m = cur && cur.detail && cur.detail.meta; if (m && m.channel_id) M.openChannel(m.channel_id); });

  let cacheSig = '';
  function renderCache(c) {
    const entries = (c.detail && c.detail.entries) || [];
    const views = entries.map(L.entryView);
    const sig = JSON.stringify(views.map((v) => [v.id, v.status, v.size, v.progress, v.protected]));
    if (sig !== cacheSig) {
      cacheSig = sig;
      W.cacheRows.replaceChildren(...(views.length ? views.map((v) => h('li', { class: 'cache-row' },
        h('span', { class: 'cbadge ' + (v.protected ? 'protected' : (v.complete ? 'complete' : (v.failed ? 'failed' : 'partial'))), text: v.label }),
        h('span', { class: 'cache-status', text: (v.protected ? 'Kept' : v.complete ? 'Ready · temporary' : v.status) }),
        v.active ? h('span', { class: 'cache-prog' }, (() => { const i = h('i'); i.style.width = v.progress + '%'; return i; })()) : h('span', { class: 'cache-prog off' }),
        h('span', { class: 'cache-size muted', text: v.size }),
        !v.protected && v.complete ? h('button',{text:'Keep',class:'small primary',onclick:()=>cacheRequest('Copy kept',null,()=>M.api('POST','/api/entries/'+encodeURIComponent(v.id)+'/promote'))}) : null,
        savedActions(entries.find((e) => e.id === v.id), c))) : [h('li', { class: 'cache-empty muted', text: 'Nothing cached for this video yet.' })]));
    }
    const lib = L.inLibrary(entries);
    setText(W.save, lib ? 'Kept in library' : 'Keep video');
    updateCacheButtons();
    setHidden(cacheBar,entries.some(e=>e.status==='complete'));
    const signature = entries.filter((e) => e.status === 'complete').map((e) => e.id + ':' + e.actual_height).join('|');
    if (sourceChoice.dataset.signature !== signature) {
      sourceChoice.dataset.signature = signature;
      sourceChoice.replaceChildren(h('option', { value: '', text: 'Playback source' }),
        ...entries.filter((e) => e.status === 'complete').map((e) => h('option', { value: e.id, text: (e.variant==='audio'?'Audio':(e.actual_height||e.variant)+'p') + ' · on disk' })),
        ...['360', '480', '720', '1080', 'audio'].map((q) => h('option', { value: 'download:' + q, text: 'Download ' + L.qLabel(q) + ' copy…' })));
    }
    sourceChoice.value = c.playbackEntry || '';
  }
  function savedActions(e, c) {
    const actions = h('div', { class: 'saved-actions' });
    const action = (text, fn) => h('button', { text, onclick: () => cacheRequest(text, null, fn) });
    if (e.status === 'complete') {
      actions.append(h('button', { text: 'Play this copy', onclick: () => playCopy(e) }));
      const retention = h('select', { 'aria-label': 'Retention for ' + e.id },
        h('option', { value: '', text: e.protected ? 'Kept indefinitely' : 'Change retention…' }),
        h('option', { value: '0', text: 'Until watched · then cleanup' }),
        ...[1, 3, 7, 30, 90].map((n) => h('option', { value: String(n), text: 'Temporary cache · up to ' + n + ' days' })),
        h('option', { value: 'keep', text: 'Keep indefinitely' }));
      retention.addEventListener('change', () => {
        if (!retention.value) return;
        const value = retention.value;
        cacheRequest('Retention updated', null, () => M.api('POST', '/api/entries/' + encodeURIComponent(e.id) + '/retention', { protect: value === 'keep', retention_days: value === 'keep' ? null : Number(value) }));
      });
      actions.append(retention);
      const quality = h('select', { 'aria-label': 'Change quality of ' + e.id }, h('option', { value: '', text: 'Create another quality…' }),
        ...['360', '480', '720', '1080', 'audio'].filter((q) => q !== e.variant).map((q) => h('option', { value: q, text: L.qLabel(q) })));
      quality.addEventListener('change', () => {
        if (!quality.value) return;
        const q = quality.value;
        cacheRequest('New ' + L.qLabel(q) + ' copy queued; delete the old copy after it finishes.', null, () => M.api('POST', '/api/cache', cacheBody(c.detail?.meta || {}, { quality: q, protect: !!e.protected })));
      });
      actions.append(quality);
    }
    if (!e.protected && e.status!=='complete') actions.append(action('Keep', () => M.api('POST', '/api/entries/' + encodeURIComponent(e.id) + '/promote')));
    if (e.status === 'failed' || e.status === 'canceled') actions.append(action('Retry', () => M.api('POST', '/api/entries/' + encodeURIComponent(e.id) + '/retry')));
    if (e.status === 'downloading' || e.status === 'queued') { actions.append(action('Cancel', () => M.api('POST', '/api/entries/' + encodeURIComponent(e.id) + '/cancel'))); return actions; }
    actions.append(h('button', { text: 'Delete copy', class: 'danger', onclick: async () => {
      if (!(await M.confirm('Delete this ' + L.entryView(e).label + ' copy' + (e.protected ? ' from your library' : '') + '? Other copies and metadata remain.', 'Delete copy', true))) return;
      if (c !== cur) return;
      cacheRequest('Copy deleted', null, async () => {
        const playing = c.playbackEntry === e.id;
        if (playing) detachSource(c);
        try { await M.api('POST', '/api/entries/' + encodeURIComponent(e.id) + '/evict' + (e.protected ? '?force=1' : '')); }
        catch (error) { if (playing && e.status === 'complete') playCopy(e); throw error; }
        if (playing) { c.error = { title: 'This copy was deleted', detail: 'Select another saved copy or download a new one.', reason: 'deleted', retryable: true }; updateOverlay(); }
      });
    } }));
    return h('details',{class:'copy-manage'},h('summary',{text:'Manage copy'}),actions);
  }
  function updateCacheButtons() {
    const busy = !!(cur && cur.cacheBusy);
    const entries = (cur && cur.detail && cur.detail.entries) || [];
    for (const b of cacheButtons) b.disabled = busy || !cur;
    if (L.inLibrary(entries)) W.save.disabled = true;
  }

  async function cacheRequest(label, body, after) {
    const c = cur;
    if (!c || c.cacheBusy) return;
    c.cacheBusy = true; updateCacheButtons();
    const meta = (c.detail && c.detail.meta) || {};
    try {
      await after(body, meta);
      if (c === cur) cacheMsg(label);
    } catch (e) {
      if (c === cur) cacheMsg(M.errText(e), 'error');
    } finally {
      c.cacheBusy = false;
      if (c === cur) { updateCacheButtons(); fetchDetail(c); pollKick(c); }
    }
  }
  function cacheBody(meta, extra) {
    return Object.assign({ video_id: cur.videoId, title: meta.title || null, channel: meta.channel || null, channel_id: meta.channel_id || null, duration_seconds: meta.duration_seconds || null }, extra);
  }
  function cacheQuality(q) {
    return cacheRequest(q === 'audio' ? 'Queued audio' : `Queued ${q}p`, null, (_b, meta) => M.api('POST', '/api/cache', cacheBody(meta, { quality: q })));
  }
  function saveToLibrary() {
    const c = cur;
    if (!c) return;
    const entries = (c.detail && c.detail.entries) || [];
    const ent = L.promotableEntry(entries) || entries.find((e) => !e.protected && e.status === 'downloading');
    if (ent) return cacheRequest('Saved to library', null, () => M.api('POST', '/api/entries/' + encodeURIComponent(ent.id) + '/promote'));
    return cacheRequest('Saving to library (480p)', null, (_b, meta) => M.api('POST', '/api/cache', cacheBody(meta, { quality: '480', protect: true })));
  }
  function pollKick(c) { if (c === cur && !c.pollTimer && c.status && !L.watchSettled(c.status)) { c.pollDelayMs = 700; pollLoop(c); } }

  let commentsSig = '';
  function commentBody(cm) {
    if (cm.text.length <= 280) return h('p', { class: 'comment-text', text: cm.text });
    return h('details', { class: 'comment-expand' }, h('summary', { text: cm.text.slice(0, 250) + '… Show more' }), h('p', { class: 'comment-text', text: cm.text }));
  }
  function commentNode(cm, c, withReplies) {
    const article = h('article', { class: 'comment' + (withReplies ? '' : ' reply') },
      h('header', { class: 'comment-head' }, h('b', { text: cm.author }), cm.time ? h('span', { class: 'muted', text: cm.time }) : null), commentBody(cm),
      cm.likes ? h('footer', { class: 'comment-foot muted', text: cm.likes + ' likes' }) : null);
    if (withReplies && cm.replies && cm.id) {
      const replies = h('div', { hidden: true });
      const message = h('span', { class: 'muted', role: 'status' });
      const toggle = h('button', { class: 'link', text: cm.replies + ' replies', 'aria-expanded': 'false' });
      const more = h('button', { text: 'Load more replies', hidden: true });
      let loaded = false, hasMore = false;
      async function load(next) {
        toggle.disabled = more.disabled = true; setText(message, 'Loading replies…');
        try {
          const result = await M.api('POST', '/api/video/' + c.videoId + '/comments/' + encodeURIComponent(cm.id) + '/replies' + (next ? '?more=1' : ''));
          if (c !== cur) return;
          replies.replaceChildren(...(result.comments || []).map((r) => commentNode(L.commentView(r), c, false)));
          loaded = true; setHidden(replies, false); setAttr(toggle, 'aria-expanded', 'true');
          hasMore = !!result.has_more;
          setText(toggle, 'Hide replies'); setHidden(more, !hasMore);
          setText(message, result.comments?.length ? '' : 'No replies returned.');
        } catch (e) { if (c === cur) setText(message, M.errText(e)); }
        finally { toggle.disabled = more.disabled = false; }
      }
      toggle.addEventListener('click', () => {
        if (!loaded) { load(false); return; }
        replies.hidden = !replies.hidden;
        setAttr(toggle, 'aria-expanded', String(!replies.hidden));
        setText(toggle, replies.hidden ? cm.replies + ' replies' : 'Hide replies');
        more.hidden = replies.hidden || !hasMore;
      });
      more.addEventListener('click', () => load(true));
      article.append(h('div', { class: 'comment-reply-actions' }, toggle, message), replies, more);
    }
    return article;
  }
  function renderComments(c) {
    const cd = (c.detail && c.detail.comments) || {};
    const list = (cd.comments || []).filter((cm) => !cm.is_reply).map(L.commentView);
    const total = cd.total;
    setText(W.commentsHead, 'Comments' + (total ? ' (' + total.toLocaleString('en-US') + ')' : (list.length ? ' (' + list.length + ' loaded)' : '')));
    const sig = JSON.stringify(list);
    if (sig !== commentsSig) {
      commentsSig = sig;
      c.commentNodes = c.commentNodes || new Map();
      W.comments.replaceChildren(...(list.length ? list.map((cm) => {
        const key = cm.id || cm.author + ':' + cm.text;
        if (!c.commentNodes.has(key)) c.commentNodes.set(key, commentNode(cm, c, true));
        return c.commentNodes.get(key);
      })
        : [h('p', { class: 'muted comment-empty', text: c.status && c.status.meta_state === 'loading' ? 'Loading comments...' : 'No comments cached for this video.' })]));
    }
    if (c.detail && !c.commentsBusy) setHidden(W.commentsMore, !c.detail.has_more_comments);
  }

  async function loadComments(refresh) {
    const c = cur;
    if (!c) return;
    if (c.commentsBusy) return;
    c.commentsBusy = true;
    W.commentsMore.disabled = W.commentsRefresh.disabled = true;
    setText(W.commentsMsg, 'Loading...');
    try {
      const before = c.detail?.comments?.comments?.length || 0;
      const r = await M.api('POST', '/api/video/' + c.videoId + '/comments/more' + (refresh ? '?refresh=1' : ''));
      if (c !== cur) return;
      setText(W.commentsMsg, r && r.stored > before ? `${r.stored - before} more loaded` : (r?.has_more ? 'Page refreshed. Load more for the next page.' : 'No more comments'));
      if (c.status) c.status.has_more_comments = !!(r && r.has_more);
      setHidden(W.commentsMore, !(r && r.has_more));
      await fetchDetail(c);
    } catch (e) {
      if (c === cur) setText(W.commentsMsg, M.errText(e));
    } finally {
      c.commentsBusy = false;
      W.commentsMore.disabled = W.commentsRefresh.disabled = false;
    }
  }
  W.commentsMore.addEventListener('click', () => loadComments(false));
  W.commentsRefresh.addEventListener('click', () => loadComments(true));

  let relatedSig = '';
  function renderRelated(c) {
    const list = ((c.detail && c.detail.related) || []).map(L.itemView);
    const sig = list.map((v) => v.videoId + (v.cache ? v.cache.label : '')).join(',');
    if (sig === relatedSig) return;
    relatedSig = sig;
    W.related.replaceChildren(...(list.length ? list.map((v) => { const link = h('a', { href: 'https://www.youtube.com/watch?v=' + v.videoId, class: 'rel', title: v.title },
      h('span', { class: 'rel-thumb' }, M.thumbImg(v.thumb), v.duration ? h('span', { class: 'vdur', text: v.duration }) : null),
      h('span', { class: 'rel-text' },
        h('span', { class: 'rel-title', text: v.title }),
        h('span', { class: 'rel-chan', text: v.channel || ' ' }),
        h('span', { class: 'rel-meta' }, v.cache ? h('span', { class: 'cbadge ' + v.cache.cls, text: v.cache.label }) : null, v.meta ? h('span', { class: 'muted', text: v.meta }) : null)));
      M.bindVideoLink(link, v.videoId, () => openRelated(v.videoId)); return link; })
      : [h('p', { class: 'muted', text: 'No related videos cached yet.' })]));
  }
  function openRelated(videoId) {
    if (!cur) return;
    M.invoke('tab_open_video', { id: cur.tabId, video_id: videoId }).then(() => M.refreshTabs()).catch((e) => M.flash('error', M.errText(e)));
  }

  /* ------------------------------------------------------------------ lifecycle */

  function resetUi() {
    commentsSig = ''; relatedSig = ''; cacheSig = '';
    setText(W.title, 'Loading...');
    setText(W.channel, ''); setHidden(W.channel, true);
    setText(W.views, '');
    W.desc.textContent = ''; setHidden(W.descToggle, true);
    W.cacheRows.replaceChildren(h('li', { class: 'cache-empty muted', text: 'Nothing cached for this video yet.' }));
    W.comments.replaceChildren(h('p', { class: 'muted comment-empty', text: 'Loading comments...' }));
    setText(W.commentsHead, 'Comments');
    W.related.replaceChildren();
    setHidden(W.commentsMore, true);
    setText(W.commentsMsg, ''); setText(W.cacheMsg, '');
    notice('');
    ctl.seek.value = '0';
    setHidden(keepPrompt.el, true); setHidden(ctl.cleanupPoint, true);
    root.scrollTop = 0;
  }

  async function loadQueue(c) {
    const tab=M.tabs.list.find(t=>t.id===c.tabId);
    let p=tab&&tab.playlist;
    if(!p&&tab?.url){const u=new URL(tab.url);const list=u.searchParams.get('list');if(list)p={list_id:list,index:u.searchParams.has('index')?Number(u.searchParams.get('index')):null};}
    c.queue=null; c.queueError=null; W.queue.hidden=true;
    if(!p||!p.list_id)return;
    try {
      const qs=new URLSearchParams({list:p.list_id});
      if(c.videoId)qs.set('video',c.videoId);
      if(p.index!=null)qs.set('index',String(p.index));
      const data=await M.api('GET','/api/playlist?'+qs.toString());
      if(c!==cur)return;
      c.queue={...data,index:Number.isInteger(data.active_index)?data.active_index:-1};
      renderQueue(c);
      // Playlist URLs can open a watch tab before any video has been selected. Resolve
      // that tab from the playlist response; never start watch/detail calls with an empty ID.
      if(!c.videoId){
        const items=Array.isArray(data.items)?data.items:[];
        const preferred=Number.isInteger(p.index)?p.index-1:-1;
        const active=Number.isInteger(data.active_index)&&data.active_index>=0?data.active_index:preferred;
        const selected=(items[active]?.available!==false&&items[active]?.id ? items[active] : null)||items.find(item=>item?.available!==false&&item?.id);
        if(!selected){c.queueEmpty=true;renderQueue(c);return;}
        try { await M.invoke('tab_open_video',{id:c.tabId,video_id:selected.id,playlist:{list_id:p.list_id,index:items.indexOf(selected)+1}}); await M.refreshTabs(); }
        catch(e){if(c===cur){c.queueError=e;renderQueue(c);}}
      }
    }
    catch(e){if(c===cur){c.queueError=e;renderQueue(c);}}
  }
  function renderQueue(c){const q=c.queue; W.queue.hidden=!q&&!c.queueError&&!c.queueEmpty; if(W.queue.hidden)return; W.queue.replaceChildren(h('div',{class:'queue-head'},h('strong',{text:q?'Playlist':c.queueEmpty?'No available videos':'Playlist unavailable'}),h('span',{class:'muted',text:q?`${q.index+1} / ${q.items.length}${q.truncated?' · more items not loaded':''}`:c.queueEmpty?'This playlist has no available videos.':('Could not load playlist queue'+(c.queueError?': '+M.errText(c.queueError):''))})));if(q)W.queue.append(...q.items.map((it,i)=>h('button',{type:'button',class:'queue-item'+(i===q.index?' current':''),disabled:it.available===false,text:it.title||it.id,onclick:()=>selectQueue(i)},it.available===false?' · unavailable':'')));if(c.queueError||c.queueEmpty)W.queue.append(h('button',{type:'button',class:'primary',text:'Retry',onclick:()=>loadQueue(c)}));}
  async function selectQueue(index){const c=cur,q=c&&c.queue,item=q&&q.items[index];if(!c||!item||item.available===false)return;try{await M.invoke('tab_open_video',{id:c.tabId,video_id:item.id,playlist:{list_id:q.list_id,index:index+1}});await M.refreshTabs();}catch(e){notice('Could not open playlist video: '+M.errText(e));}}
  function advanceQueue(direction, ended){const c=cur,q=c&&c.queue;if(!c||!q)return false;const i=window.MuninnQueue.next(q.items,q.index,direction);if(i==null){notice(ended?'End of available playlist items':'No more available playlist items');return false;}selectQueue(i);return true;}

  function begin(tab) {
    const c = {
      key: tab.id + ':' + tab.video_id, tabId: tab.id, videoId: tab.video_id || null, playlistOnly: !tab.video_id, gen: ++gen,
      status: null, detail: null, detailAt: 0, detailFailed: false, error: null, mediaError: null,
      sourceSet: false, sourceKey: '', kind: null, hlsOk: true, resumeDone: false, needsClick: false, buffering: false,
      pollTimer: null, pollDelayMs: 700, noneSince: 0, beganAt: Date.now(), retrying: false, cacheBusy: false,
    };
    cur = c;
    resetUi();
    updateOverlay();
    updateControls();
    clearInterval(uiTimer);
    uiTimer = setInterval(updateControls, 500);
    if(c.videoId){fetchDetail(c);startWatch(c);}
    else {setText(W.title,'Loading playlist…');setText(W.commentsHead,'Playlist');setHidden(W.commentsMore,true);}
    loadQueue(c);
  }

  function leave() {
    const c = cur;
    if (!c) return;
    if(c.localEntry)stopLocalPlayback(c.localEntry,c.localGeneration);
    clearTimeout(c.seekTimer);
    stopReporter();
    if (c.sourceSet) report();           // while the position is still there
    cur = null;
    exitPip(); updateNowPlaying(); root.classList.remove('in-background');
    if(M.page || M.tabs.list.find(tab=>tab.id===M.tabs.active)?.face!=='watch')root.hidden=true;
    clearTimeout(c.pollTimer);
    clearInterval(uiTimer); uiTimer = null;
    if (fsEl()) { try { (document.exitFullscreen || document.webkitExitFullscreen).call(document); } catch (e) { /* ignore */ } }
    player.classList.remove('expanded');
    // Detach: only one player may exist, RAM stays bounded.
    try { video.pause(); } catch (e) { /* ignore */ }
    destroyHls();
    video.removeAttribute('src');
    video.load();
    dragging = false;
    setAttr(player, 'data-engine', null);
    setText(notes.engine, '');
    ctl.played.style.width = '0%'; ctl.avail.style.width = '0%';
    ctl.seek.value = '0'; ctl.seek.disabled = true;
    setHidden(keepPrompt.el, true); setHidden(ctl.cleanupPoint, true);
    setText(ctl.time, '0:00 / --:--');
    setText(notes.stream, '');
    notice('');
    updateOverlay();
  }

  function retry() {
    const c = cur;
    if (!c) return;
    const tab = { id: c.tabId, video_id: c.videoId };
    const position=!c.resumeDone&&c.resumeTarget!=null ? c.resumeTarget : mediaPos();
    const playing=c.sourceSet&&video.currentSrc ? !video.paused : c.playIntent!==false;
    leave();
    begin(tab);
    cur.resumeTarget=position;cur.playIntent=playing;
    cur.retrying = true;
    updateOverlay();
  }

  let appliedPipMode=null;
  M.watch = {
    sync(tab) {
      if (cur && !M.tabs.list.some((t) => t.id === cur.tabId)) leave();
      if (!tab) {
        if (!cur) return;
        prepareBackground();
        if (cur.sourceSet || inPip() || cur.pipPending || cur.background || M.settings.background_playback) {
          cur.background = true; updateNowPlaying(); return;
        }
        leave(); return;
      }
      const key = tab.id + ':' + tab.video_id;
      if (cur && cur.videoId === tab.video_id) {
        const returning = cur.background;
        cur.tabId = tab.id; cur.key = key; cur.background = false;
        if (returning && M.settings.pip_return_to_inline !== false) exitPip();
        updateNowPlaying(); return;
      }
      if (cur) leave();
      begin(tab);
    },
    prepareBackground,
    closeTab(id) { if (cur?.tabId === id) leave(); },
    applySettings() {
      const changed=appliedPipMode!==M.settings.pip_mode;appliedPipMode=M.settings.pip_mode;
      if(changed && appliedPipMode==='automatic' && cur?.background && !video.paused && !inPip())enterPip(true);
      if (M.settings.pip_mode === 'disabled' && inPip()) {
        exitPip();
        if (cur?.background && !M.settings.background_playback) video.pause();
      }
      updateNowPlaying();
    },
    leave,
    retry,
    video,                      // for tests
    state: () => cur,
  };
})();
