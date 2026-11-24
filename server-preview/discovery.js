/* Muninn-native discovery. Chromium supplies captures; it is not the PWA UI.
 * No provider embed, cookies, arbitrary proxy or screenshot in the primary view. */
(() => {
  'use strict';
  const host = document.querySelector('#browser-host');
  const menu = document.querySelector('#pwa-discover-menu');
  if (!host || !menu || document.querySelector('meta[name="muninn-source-browser"]')?.content !== 'enabled') return;
  const M = window.MuninnWebModel;
  const sources = {
    'youtube-home': {label:'YouTube Home',kind:'home',url:'https://www.youtube.com/'},
    'youtube-subscriptions': {label:'Subscriptions',kind:'subscriptions',url:'https://www.youtube.com/feed/subscriptions'},
    'tiktok-foryou': {label:'TikTok For You',kind:'tiktok-for-you',url:'https://www.tiktok.com/foryou'},
    'tiktok-following': {label:'Following',kind:'tiktok-following',url:'https://www.tiktok.com/following'}
  };
  const $ = selector => document.querySelector(selector);
  const el = (tag, text, cls) => {
    const n = document.createElement(tag);
    if (text !== undefined && text !== null) n.textContent = String(text);
    if (cls) n.className = cls;
    return n;
  };
  const button = (text, action, cls) => {
    const n = el('button',text,cls);
    n.type = 'button'; n.addEventListener('click', () => Promise.resolve().then(action).catch(showError));
    return n;
  };
  const shell = $('#app'), backdrop = $('#pwa-nav-backdrop'), toggle = $('#pwa-menu-toggle');
  const state = {visible:!host.hidden, source:'youtube-home', query:'', selected:null,
    snapshot:null, entries:[], pending:false, loading:false, readEpoch:0, routeEpoch:0,
    advanced:false, lastCapture:null, lastError:null};
  let navigationTail = Promise.resolve();
  menu.hidden = false;
  const surface = el('section',undefined,'muninn-discover');
  const heading = el('div',undefined,'muninn-discover-head');
  const headingText = el('div');
  const title = el('h2','YouTube Home'), subtitle = el('p','From your private captured feed','muninn-discover-subtitle');
  headingText.append(title,subtitle);
  const refresh = button('Refresh feed',()=>navigateSource(true),'muninn-discover-refresh');
  heading.append(headingText,refresh);
  const message = el('p','Connecting to your captured library…','muninn-discover-state');
  message.setAttribute('role','status');
  const grid = el('div',undefined,'muninn-discover-grid');
  const watch = el('section',undefined,'muninn-discover-watch');
  watch.hidden = true;
  surface.append(heading,message,grid,watch);
  host.prepend(surface);
  function openMenu(open) {
    shell.dataset.menu = open ? 'open' : 'closed';
    backdrop.hidden = !open;
    toggle.setAttribute('aria-expanded',String(open));
    toggle.setAttribute('aria-label',open?'Close navigation menu':'Open navigation menu');
  }
  toggle.addEventListener('click',()=>openMenu(shell.dataset.menu!=='open'));
  backdrop.addEventListener('click',()=>openMenu(false));
  document.addEventListener('keydown',event=>{if(event.key==='Escape'&&shell.dataset.menu==='open')openMenu(false);});
  function showError(error) {
    state.lastError = error.message || 'The captured feed could not be loaded.';
    message.textContent = state.lastError;
  }
  async function request(path, data) {
    const response = await fetch(path,{method:data?'POST':'GET',credentials:'same-origin',cache:'no-store',
      headers:{'X-Muninn-Client':'pwa',...(data?{'Content-Type':'application/json'}:{})},
      body:data?JSON.stringify(data):undefined});
    if (!response.ok) {
      if (response.status===401||response.status===403) throw new Error('Reopen Muninn through Authelia.');
      throw new Error('Muninn server returned HTTP '+response.status+'.');
    }
    return response.json();
  }
  const kind = () => state.source==='search' ? 'search'
    : state.source==='tiktok-search' ? 'tiktok-search' : sources[state.source]?.kind || 'home';
  const sourceUrl = () => state.source==='search' ? 'https://www.youtube.com/results?search_query='+encodeURIComponent(state.query)
    : state.source==='tiktok-search' ? 'https://www.tiktok.com/search?q='+encodeURIComponent(state.query)
    : sources[state.source]?.url || sources['youtube-home'].url;
  const sourceLabel = () => state.source==='search' ? 'YouTube · '+state.query
    : state.source==='tiktok-search' ? 'TikTok · '+state.query
    : sources[state.source]?.label || 'Discover';
  const isValidItem = item => item && M.validId(item.video_id) && typeof item.title==='string';
  const minutes = seconds => {
    const value=Math.max(0,Math.trunc(Number(seconds)||0));
    return value>=3600 ? Math.floor(value/3600)+':'+String(Math.floor(value%3600/60)).padStart(2,'0')+':'+String(value%60).padStart(2,'0')
      : Math.floor(value/60)+':'+String(value%60).padStart(2,'0');
  };
  function matchingSnapshot(meta) {
    if (meta.kind!==kind()) return false;
    if (state.source!=='search' && state.source!=='tiktok-search') return true;
    const text=state.query.trim().toLowerCase();
    // Search snapshots include a query or an exact source URL.
    return String(meta.query||'').trim().toLowerCase()===text ||
      meta.source_url===sourceUrl();
  }
  function setMessage(text) { message.textContent=text; }
  function renderNav() {
    for (const b of menu.querySelectorAll('[data-browse-source]')) {
      const active=b.dataset.browseSource===state.source;
      b.setAttribute('aria-pressed',String(active));
      if(active)b.setAttribute('aria-current','page');else b.removeAttribute('aria-current');
    }
    const advanced = $('#pwa-advanced-toggle');
    advanced.setAttribute('aria-expanded',String(state.advanced));
  }
  function showCollection() {
    const player=watch.querySelector('video');
    if(player){player.pause();player.removeAttribute('src');player.load();}
    watch.hidden=true; grid.hidden=false; heading.hidden=false; message.hidden=false;
    state.selected=null;
  }
  function browseView() {
    const b=document.querySelector('[data-view="browse"]');
    if(b)b.click();
  }
  async function switchSource(source, query='') {
    if (!sources[source] && !['search','tiktok-search'].includes(source)) return;
    if (!navigator.onLine) { setMessage('Connect to your Muninn server to browse. Your offline library is still available.'); return; }
    state.source=source;state.query=query;state.snapshot=null;state.lastError=null;
    state.advanced=false;
    document.dispatchEvent(new CustomEvent('muninn:advanced-browser',{detail:{open:false}}));
    state.routeEpoch++;state.readEpoch++;
    const wasVisible=state.visible;
    renderNav();showCollection();openMenu(false);browseView();draw();
    // Entering Browse triggers its own initial capture; switching providers
    // inside an already visible view issues exactly one navigation.
    if(wasVisible) await Promise.allSettled([loadSnapshot(),navigateSource(false)]);
  }
  for (const b of menu.querySelectorAll('[data-browse-source]')) {
    b.addEventListener('click',()=>switchSource(b.dataset.browseSource).catch(showError));
  }
  $('#pwa-discover-search').addEventListener('submit',event=>{
    event.preventDefault();
    const query=$('#pwa-discover-query').value.trim();
    if(!query)return;
    const provider=event.submitter?.value==='tiktok'?'tiktok-search':'search';
    switchSource(provider,query).catch(showError);
  });
  $('#pwa-advanced-toggle').addEventListener('click',()=>{
    state.advanced=!state.advanced;
    renderNav();
    openMenu(false);
    browseView();
    document.dispatchEvent(new CustomEvent('muninn:advanced-browser',{detail:{open:state.advanced}}));
  });
  function captureMeta(item) {
    const bits=[];
    if(item.channel)bits.push(item.channel);
    if(item.view_text)bits.push(item.view_text);
    if(item.published_text)bits.push(item.published_text);
    return bits.join(' · ') || (item.video_id.startsWith('tt_')?'TikTok':'YouTube');
  }
  function art(item,large=false) {
    const artwork=el('div',item.video_id.startsWith('tt_')?'TikTok':'YouTube','muninn-discover-art');
    if(large)artwork.className='muninn-watch-art';
    // Core owns and serves its own cached images. Never leak a private
    // account to a third-party CDN from Safari or the installed PWA.
    const img=el('img');
    img.alt='';
    img.loading='lazy';
    img.src=(item.video_id.startsWith('tt_')?'/discover/thumb/':'/thumb/')+
      encodeURIComponent(item.video_id);
    img.addEventListener('error',()=>img.remove());
    artwork.append(img);
    if(!large && Number(item.duration_seconds)>0)artwork.append(el('span',minutes(item.duration_seconds),'muninn-discover-duration'));
    return artwork;
  }
  function cachedEntry(item) {
    if (!item.cached?.id) return null;
    const entry=state.entries.find(x=>x.id===item.cached.id);
    return entry?.status==='complete' ? entry : null;
  }
  async function save(item) {
    if(!M.validId(item.video_id))throw new Error('This video has no valid provider ID.');
    await request('/api/cache',{video_id:item.video_id,quality:'720',title:item.title,
      channel:item.channel||undefined,protect:true});
    setMessage('Saved to the Muninn download queue. Library will show progress.');
    if(!watch.hidden && state.selected?.video_id===item.video_id) {
      const status=watch.querySelector('.muninn-watch-feedback');
      if(status)status.textContent='Added to your Muninn library. The download continues in the background.';
    }
    document.dispatchEvent(new CustomEvent('muninn:video-queued',{detail:{id:item.video_id}}));
  }
  function play(item,entry) {
    showDetail(item);
    let video=watch.querySelector('video');
    if(!video) {
      video=el('video',undefined,'muninn-watch-player');
      video.controls=true;video.playsInline=true;video.preload='metadata';
      watch.append(video);
    }
    video.src='/media/'+encodeURIComponent(entry.id);
    video.play().catch(()=>{setMessage('Tap Play to start the saved video.');});
  }
  function showDetail(item) {
    const current=watch.querySelector('video');
    if(current){current.pause();current.removeAttribute('src');current.load();}
    watch.replaceChildren();
    heading.hidden=true;message.hidden=true;grid.hidden=true;watch.hidden=false;
    state.selected=item;
    watch.append(button('← Back to '+sourceLabel(),showCollection,'muninn-watch-back'));
    watch.append(art(item,true));
    watch.append(el('h2',item.title||'Untitled video'));
    watch.append(el('p',captureMeta(item),'muninn-watch-meta'));
    const actions=el('div',undefined,'muninn-watch-actions');
    const entry=cachedEntry(item);
    if(entry)actions.append(button('Play in Muninn',()=>play(item,entry)));
    else actions.append(button(item.cached?.id?'Download in progress':'Save video to Muninn',()=>save(item)));
    actions.append(button('Copy source URL',async()=>{
      const url=M.videoURL(item.video_id);
      if(!url)return;
      await navigator.clipboard.writeText(url);
      setMessage('Video URL copied.');
    }));
    watch.append(actions,el('p',undefined,'muninn-watch-feedback'));
  }
  function draw() {
    if(!state.visible)return;
    title.textContent=sourceLabel();
    subtitle.textContent=state.source==='search'||state.source==='tiktok-search'
      ? 'Captured search results, presented in Muninn'
      : 'Videos captured by your private '+(state.source.startsWith('tiktok')?'TikTok':'YouTube')+' browser';
    renderNav();
    if(state.selected)return;
    grid.replaceChildren();
    const items=(state.snapshot?.items||[]).filter(isValidItem);
    if(!items.length){
      const empty=el('p',state.loading ? 'Reading captured videos from your server…'
        : state.pending ? 'Opening '+sourceLabel()+' in the private browser and collecting videos…'
          : state.lastError || 'No captured videos yet. Refresh to load this provider; private account sign-in is under Site controls.',
        'muninn-discover-empty');
      grid.append(empty);
    }else {
      for (const item of items.slice(0,100)) {
        const card=el('article',undefined,'muninn-discover-card');
        const open=button(undefined,()=>showDetail(item),'muninn-discover-art-button');
        open.setAttribute('aria-label','Open '+(item.title||'video'));
        open.append(art(item));
        const copy=el('div',undefined,'muninn-discover-copy');
        copy.append(el('h3',item.title,'muninn-discover-title'));
        copy.append(el('p',captureMeta(item),'muninn-discover-byline'));
        const actions=el('div',undefined,'muninn-discover-actions');
        const entry=cachedEntry(item);
        if(entry)actions.append(button('Play',()=>play(item,entry)));
        else actions.append(button(item.cached?.id?'Downloading…':'Save',()=>save(item)));
        copy.append(actions);card.append(open,copy);grid.append(card);
      }
    }
    const captured=state.snapshot?.captured_at;
    const time=Number(captured);
    const info=Number.isFinite(time)&&time>0?new Date(time).toLocaleString():'';
    setMessage(items.length ? items.length+' captured videos'+(info?' · Last capture '+info:'')+
      (state.pending?' · Updating…':'') : (state.lastError|| (state.pending?'Collecting fresh videos…':'Browse by choosing a provider.')));
  }
  async function loadSnapshot() {
    if(!state.visible || !navigator.onLine)return;
    const epoch=++state.readEpoch;
    state.loading=true;
    try {
      const [snapshots,entries]=await Promise.all([request('/api/feeds?kind='+encodeURIComponent(kind())+'&limit=25'),request('/api/entries')]);
      if(epoch!==state.readEpoch)return;
      if(!Array.isArray(snapshots)||!Array.isArray(entries))throw new Error('Invalid Muninn feed response.');
      state.entries=entries;
      const selected=snapshots.filter(matchingSnapshot).sort((a,b)=>Number(b.captured_at)-Number(a.captured_at)||Number(b.id)-Number(a.id))[0];
      if(selected&&Number.isSafeInteger(selected.id)){
        const detail=await request('/api/feeds/'+selected.id);
        if(epoch!==state.readEpoch)return;
        if(!Array.isArray(detail.items))throw new Error('Invalid captured feed.');
        state.snapshot=detail;state.lastError=null;
      }else state.snapshot=null;
    }catch(error){if(epoch===state.readEpoch)state.lastError=error.message;}
    finally {if(epoch===state.readEpoch){state.loading=false;draw();}}
  }
  // Actual remote Chromium navigation is used to *produce* provider captures,
  // never as the primary screen or Safari iframe.
  async function navigateSource(force=false) {
    if(!state.visible||!navigator.onLine)return;
    const route=++state.routeEpoch,target=sourceUrl();
    const execute=async()=>{
      if(route!==state.routeEpoch || !state.visible)return;
      state.pending=true;draw();
      try{
        const remote=await request('/browser/state');
        if(route!==state.routeEpoch||!state.visible)return;
        const tabs=Array.isArray(remote.tabs)?remote.tabs:[];
        const match=tabs.find(tab=>tab.url===target);
        if(match) {
          // An existing provider tab is not proof of a usable capture. A restarted
          // capture pipeline or failed earlier acquisition may leave it empty.
          if(force || !state.snapshot)await request('/browser/action',{action:'reload',tab:match.id});
        }else if(tabs.length<4)await request('/browser/action',{action:'new',url:target});
        else {
          // At the four-tab limit recycle only the oldest tab in this
          // single-owner browsing session; never reset cookies or downloads.
          await request('/browser/action',{action:'navigate',tab:tabs[0].id,url:target});
        }
        if(route===state.routeEpoch)state.lastError=null;
      }catch(error){if(route===state.routeEpoch)state.lastError=error.message;}
      finally{if(route===state.routeEpoch){state.pending=false;draw();await loadSnapshot();}}
    };
    // Serialize provider changes so late responses cannot retarget navigation.
    navigationTail=navigationTail.catch(()=>{}).then(execute);
    return navigationTail;
  }
  // The Refresh button is wired by button() above.
  function onView(visible) {
    state.visible=visible;
    if(!visible) {
      state.readEpoch++;state.routeEpoch++;
      state.selected=null;state.advanced=false;
      document.dispatchEvent(new CustomEvent('muninn:advanced-browser',{detail:{open:false}}));
      openMenu(false);
    }else {
      draw();
      loadSnapshot().then(()=>{
        if(state.visible&&!state.snapshot)navigateSource(false);
      }).catch(showError);
    }
  }
  document.addEventListener('muninn:view-change',event=>onView(event.detail?.visible===true));
  draw();
  onView(state.visible);
  setInterval(()=>{if(state.visible&&navigator.onLine&&!document.hidden&&!state.loading)loadSnapshot();},10000);
})();