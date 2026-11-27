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
  const state = {visible:!host.hidden, source:'youtube-home', query:'', channelLabel:'', creatorHandle:'', selected:null,
    snapshot:null, entries:[], pending:false, loading:false, readEpoch:0, routeEpoch:0,
    advanced:false, lastCapture:null, lastError:null};
  let navigationTail = Promise.resolve();
  let renderedGridKey = null;
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
  const siteToggle = button('Browse YouTube / TikTok websites',()=>setAdvanced(!state.advanced),'muninn-discover-refresh');
  siteToggle.id='pwa-site-browser-toggle';
  const siteActions=el('div',undefined,'muninn-discover-head');
  siteActions.append(siteToggle);
  host.prepend(siteActions,surface);
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
  const channelId = id => typeof id==='string' && /^UC[A-Za-z0-9_-]{22}$/.test(id);
  const tiktokHandle = h => typeof h==='string' && /^[A-Za-z0-9_.-]{1,30}$/.test(h);
  const tiktokId = id => typeof id==='string' && /^tt_\d{15,22}$/.test(id);
  const tiktokWatchUrl = (id, handle) => tiktokId(id) && tiktokHandle(handle) ? 'https://www.tiktok.com/@'+handle+'/video/'+id.slice(3) : null;
  const kind = () => state.source==='tiktok-related' ? 'tiktok-related' : state.source==='tiktok-profile' ? 'tiktok-profile' : state.source==='youtube-related' ? 'related' : state.source==='youtube-channel' ? 'channel' : state.source==='search' ? 'search'
    : state.source==='tiktok-search' ? 'tiktok-search' : sources[state.source]?.kind || 'home';
  const sourceUrl = () => state.source==='tiktok-related' ? tiktokWatchUrl(state.query,state.creatorHandle) : state.source==='tiktok-profile' ? 'https://www.tiktok.com/@'+state.query : state.source==='youtube-related' ? M.youtubeURL(state.query) : state.source==='youtube-channel' ? 'https://www.youtube.com/channel/'+state.query+'/videos' : state.source==='search' ? 'https://www.youtube.com/results?search_query='+encodeURIComponent(state.query)
    : state.source==='tiktok-search' ? 'https://www.tiktok.com/search?q='+encodeURIComponent(state.query)
    : sources[state.source]?.url || sources['youtube-home'].url;
  const sourceLabel = () => state.source==='youtube-related'||state.source==='tiktok-related' ? 'Related to '+(state.channelLabel||'selected video') : state.source==='tiktok-profile' ? state.channelLabel||'TikTok creator' : state.source==='youtube-channel' ? state.channelLabel || 'YouTube channel' : state.source==='search' ? 'YouTube · '+state.query
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
    if (state.source==='youtube-channel') {
      // Match the exact creator, never a newer snapshot from another channel.
      const base='https://www.youtube.com/channel/'+state.query;
      return [base,base+'/',base+'/videos',base+'/videos/'].includes(String(meta.source_url||'').split('?')[0]);
    }
    if (state.source==='tiktok-related') {
      const canonical=M.sourceURL(meta.source_url);
      return !!canonical && canonical.startsWith('https://www.tiktok.com/') && M.videoId(canonical)===state.query;
    }
    if (state.source==='tiktok-profile') {
      const canonical=M.sourceURL(meta.source_url);
      return canonical===sourceUrl() || canonical===sourceUrl()+'/';
    }
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
    siteToggle.setAttribute('aria-expanded',String(state.advanced));
    siteToggle.textContent=state.advanced?'Return to captured videos':'Browse YouTube / TikTok websites';
    surface.hidden=state.advanced;
  }
  function showCollection() {
    const player=watch.querySelector('video');
    if(player){player.pause();player.removeAttribute('src');player.load();}
    watch.hidden=true; grid.hidden=false; heading.hidden=false; message.hidden=false;
    state.selected=null;
    draw();
  }
  function browseView() {
    const b=document.querySelector('[data-view="browse"]');
    if(b)b.click();
  }
  async function switchSource(source, query='', channelLabel='', creatorHandle='') {
    if (!sources[source] && !['search','tiktok-search','youtube-channel','youtube-related','tiktok-profile','tiktok-related'].includes(source)) return;
    if(source==='youtube-channel'&&!channelId(query))return;
    if(source==='youtube-related'&&!M.youtubeURL(query))return;
    if(source==='tiktok-profile'&&!tiktokHandle(query))return;
    if(source==='tiktok-related'&&!tiktokWatchUrl(query,creatorHandle))return;
    if (!navigator.onLine) { setMessage('Connect to your Muninn server to browse. Your offline library is still available.'); return; }
    state.source=source;state.query=query;state.channelLabel=channelLabel;state.creatorHandle=creatorHandle;state.snapshot=null;state.lastError=null;
    state.pending=false;state.loading=false;
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
  function setAdvanced(open) {
    state.advanced=open;
    if(open){
      // Native capture and the interactive website share the same remote tabs.
      // Invalidate queued/background feed navigation before handing them to
      // the owner; a delayed state read must not navigate the active tab.
      state.routeEpoch++;state.pending=false;
    }
    renderNav();
    openMenu(false);
    if(!state.visible)browseView();
    document.dispatchEvent(new CustomEvent('muninn:advanced-browser',{detail:{open:state.advanced}}));
    if(!open && state.visible && !state.snapshot){
      loadSnapshot().then(()=>{
        if(state.visible&&!state.advanced&&!state.snapshot)navigateSource(false);
      }).catch(showError);
    }
  }
  $('#pwa-advanced-toggle').addEventListener('click',()=>setAdvanced(!state.advanced));
  function creatorButton(item) {
    if(tiktokId(item.video_id))return tiktokHandle(item.creator_handle) ? button('Creator · @'+item.creator_handle,
      ()=>switchSource('tiktok-profile',item.creator_handle,item.channel||'@'+item.creator_handle)) : null;
    if(!channelId(item.channel_id))return null;
    return button('Channel'+(item.channel?' · '+item.channel:''),
      ()=>switchSource('youtube-channel',item.channel_id,item.channel||'YouTube channel'));
  }
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
  function entriesFor(item) {
    return state.entries.filter(x=>x.video_id===item.video_id ||
      (item.cached?.id && x.id===item.cached.id));
  }
  function cachedEntry(item) {
    return entriesFor(item).find(x=>x.status==='complete') || null;
  }
  function downloadPending(item) {
    return entriesFor(item).some(x=>['queued','downloading'].includes(x.status));
  }
  function updateWatchAction() {
    const action=watch.querySelector('.muninn-watch-primary');
    if(!action || !state.selected)return;
    const ready=cachedEntry(state.selected), pending=!ready&&downloadPending(state.selected);
    action.textContent=ready?'Play in Muninn':pending?'Download in progress':'Save video to Muninn';
    action.disabled=pending;
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
    await loadSnapshot();
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
    actions.append(button('Save video to Muninn',()=>{
      const entry=cachedEntry(item);
      if(entry)return play(item,entry);
      if(!downloadPending(item))return save(item);
    },'muninn-watch-primary'));
    const creator=creatorButton(item);if(creator)actions.append(creator);
    if(M.youtubeURL(item.video_id))actions.append(button('Related videos',()=>switchSource('youtube-related',item.video_id,item.title)));
    else if(tiktokWatchUrl(item.video_id,item.creator_handle))actions.append(button('Related videos',()=>switchSource('tiktok-related',item.video_id,item.title,item.creator_handle)));
    actions.append(button('Copy source URL',async()=>{
      const url=tiktokWatchUrl(item.video_id,item.creator_handle)||M.videoURL(item.video_id);
      if(!url)return;
      await navigator.clipboard.writeText(url);
      setMessage('Video URL copied.');
    }));
    watch.append(actions,el('p',undefined,'muninn-watch-feedback'));
    updateWatchAction();
  }
  function draw() {
    if(!state.visible)return;
    title.textContent=sourceLabel();
    subtitle.textContent=state.source==='search'||state.source==='tiktok-search'
      ? 'Captured search results, presented in Muninn'
      : 'Videos captured by your private '+(state.source.startsWith('tiktok')?'TikTok':'YouTube')+' browser';
    renderNav();
    if(state.selected){updateWatchAction();return;}
    const items=(state.snapshot?.items||[]).filter(isValidItem).slice(0,100);
    const emptyText=state.loading ? 'Reading captured videos from your server…'
      : state.pending ? 'Opening '+sourceLabel()+' in the private browser and collecting videos…'
        : state.lastError || 'No captured videos yet. Refresh to load this provider; private account sign-in is under Site controls.';
    // Unchanged poll results must not replace focused/touched card controls,
    // restart thumbnail loading or disrupt a Safari scroll gesture.
    const gridKey=JSON.stringify([state.source,state.query,items.length
      ? items.map(item=>[item,cachedEntry(item)?.id||null,downloadPending(item)])
      : emptyText]);
    if(gridKey!==renderedGridKey){
      renderedGridKey=gridKey;
      grid.replaceChildren();
      if(!items.length){
      const empty=el('p',emptyText,'muninn-discover-empty');
      grid.append(empty);
    }else {
      for (const item of items) {
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
        else {
          const pending=downloadPending(item);
          const action=button(pending?'Downloading…':'Save',()=>save(item));
          action.disabled=pending;actions.append(action);
        }
        const creator=creatorButton(item);if(creator)actions.append(creator);
         copy.append(actions);card.append(open,copy);grid.append(card);
      }
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
      // The global related feed is not scoped to its parent. Read the selected
      // video's own metadata instead, and never substitute another video's list.
      if(state.source==='youtube-related'){
        const parent=state.query;
        const [detail,entries]=await Promise.all([request('/api/video/'+encodeURIComponent(parent)),request('/api/entries')]);
        if(epoch!==state.readEpoch)return;
        if(detail.video_id!==parent || !Array.isArray(detail.related) || !Array.isArray(entries))
          throw new Error('Invalid related video response.');
        state.entries=entries;
        const items=detail.related.filter(item=>isValidItem(item)&&M.youtubeURL(item.video_id)&&item.video_id!==parent);
        state.snapshot=items.length?{items,captured_at:detail.meta?.captured_at}:null;
        state.lastError=null;
        return;
      }
      const [snapshots,entries]=await Promise.all([request('/api/feeds?kind='+encodeURIComponent(kind())+'&limit=25'),request('/api/entries')]);
      if(epoch!==state.readEpoch)return;
      if(!Array.isArray(snapshots)||!Array.isArray(entries))throw new Error('Invalid Muninn feed response.');
      state.entries=entries;
      const selected=snapshots.filter(matchingSnapshot).sort((a,b)=>Number(b.captured_at)-Number(a.captured_at)||Number(b.id)-Number(a.id))[0];
      if(selected&&Number.isSafeInteger(selected.id)){
        const detail=await request('/api/feeds/'+selected.id);
        if(epoch!==state.readEpoch)return;
        if(!Array.isArray(detail.items))throw new Error('Invalid captured feed.');
        if(state.source==='tiktok-related'||state.source==='tiktok-profile') {
          // A response may contain mixed items or an unrelated snapshot. Validate both the parent and provider again.
          if(!matchingSnapshot(detail))throw new Error('Captured TikTok feed does not match this selection.');
          detail.items=detail.items.filter(item=>isValidItem(item)&&tiktokId(item.video_id)&&item.video_id!==state.query);
        }
        state.snapshot=detail;state.lastError=null;
      }else state.snapshot=null;
    }catch(error){if(epoch===state.readEpoch)state.lastError=error.message;}
    finally {if(epoch===state.readEpoch){state.loading=false;draw();}}
  }
  // Actual remote Chromium navigation is used to *produce* provider captures,
  // never as the primary screen or Safari iframe.
  async function navigateSource(force=false) {
    if(!state.visible||state.advanced||!navigator.onLine)return;
    const route=++state.routeEpoch,target=sourceUrl();
    const execute=async()=>{
      if(route!==state.routeEpoch || !state.visible || state.advanced)return;
      state.pending=true;draw();
      try{
        const remote=await request('/browser/state');
        if(route!==state.routeEpoch||!state.visible||state.advanced)return;
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
      showCollection();state.advanced=false;renderNav();
      document.dispatchEvent(new CustomEvent('muninn:advanced-browser',{detail:{open:false}}));
      openMenu(false);
    }else {
      draw();
      loadSnapshot().then(()=>{
        if(state.visible&&!state.advanced&&!state.snapshot)navigateSource(false);
      }).catch(showError);
    }
  }
  document.addEventListener('muninn:view-change',event=>onView(event.detail?.visible===true));
  draw();
  onView(state.visible);
  setInterval(()=>{if(state.visible&&navigator.onLine&&!document.hidden&&!state.loading)loadSnapshot();},10000);
})();