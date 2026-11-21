(() => {
  'use strict';
  const M = window.MuninnWebModel, DB = window.MuninnWebStore, $ = s => document.querySelector(s);
  const state = {view:'server',signedIn:false,entries:[],device:[],feeds:[],feed:null,kind:'',
    transfer:null,playerURL:null,playing:null,epoch:0,refreshing:false};
  let installPrompt, progressWrite = 0;
  const notice = (text, bad=false) => { $('#notice').textContent=text; $('#notice').classList.toggle('error',bad); };
  const el = (tag, text, className) => { const n=document.createElement(tag); if(text!==undefined)n.textContent=text; if(className)n.className=className; return n; };
  const button = (text, action) => { const n=el('button',text); n.type='button'; n.addEventListener('click',()=>Promise.resolve().then(action).catch(e=>notice(e.message,true))); return n; };
  async function api(path, {method='GET',body,signal}={}) {
    const response=await fetch(path,{method,credentials:'same-origin',cache:'no-store',signal,
      headers:{'X-Muninn-Client':'pwa',...(body?{'Content-Type':'application/json'}:{})},
      body:body?JSON.stringify(body):undefined});
    const value=await response.json();
    if(!response.ok){if(response.status===401)setConnection(false);throw new Error(value.error||'Server request failed.');}
    return value;
  }
  function setConnection(signedIn) {
    state.signedIn=signedIn;
    $('#connection').textContent=!navigator.onLine?'Offline · device copies available':signedIn?'Connected to server':'Server not connected';
    $('#login-panel').hidden=signedIn||!navigator.onLine;
    $('#logout').hidden=!signedIn;
    $('#queue-panel').hidden=!signedIn||state.view!=='server';
  }
  function stopPlayer() {
    const player=$('#player');player.pause();player.removeAttribute('src');player.load();
    if(state.playerURL)URL.revokeObjectURL(state.playerURL);
    state.playerURL=null;state.playing=null;$('#player-panel').hidden=true;
  }
  async function play(record) {
    const epoch=state.epoch,blob=await DB.get(record.id);
    if(epoch!==state.epoch)return;
    if(!blob){await loadDevice();throw new Error('This device copy is no longer available. Save it again from the server.');}
    stopPlayer();state.playerURL=URL.createObjectURL(blob);state.playing=record.id;
    const player=$('#player');player.src=state.playerURL;$('#playing-title').textContent=record.title||record.video_id;
    $('#player-panel').hidden=false;
    player.onloadedmetadata=()=>{if(record.position_seconds>0&&record.position_seconds<player.duration)player.currentTime=record.position_seconds;};
    try {await player.play();} catch {notice('Copy ready. Press Play in the player; this browser may require a direct tap.');}
  }
  // The server's authenticated /media route supports byte ranges. A completed
  // video can play immediately from Muninn without storing a second local copy.
  // Offline playback remains the explicit IndexedDB path in play().
  async function playServer(record) {
    if(!state.signedIn||!M.complete(record)||!record.id)
      throw new Error('A completed server video and an active session are required.');
    stopPlayer();
    const player=$('#player');
    player.onloadedmetadata=null;
    player.src='/media/'+encodeURIComponent(record.id);
    state.playing=null;
    $('#playing-title').textContent=record.title||record.video_id;
    $('#player-panel').hidden=false;
    try{await player.play();}catch{notice('Video ready. Press Play to start streaming from your server.');}
  }
  async function loadDevice() {
    try{state.device=await DB.list();$('#device-count').textContent=state.device.length;}
    catch(error){state.device=[];notice('Device storage: '+error.message,true);}
  }
  async function copy(url) {
    if(!url)throw new Error('The source URL was not recorded.');
    try{await navigator.clipboard.writeText(url);notice('URL copied.');}
    catch{$('#copy-value').value=url;$('#copy-fallback').hidden=false;$('#copy-value').focus();$('#copy-value').select();}
  }
  // The PWA uses the same library-card structure and CSS classes as the desktop.
  // Only PWA-safe actions are exposed; no native shell or provider iframe.
  function card(record, device=false) {
    const article=el('article',undefined,'library-video-card');
    const cover=el('div',undefined,'library-card-image');
    if(!device&&M.validId(record.video_id)) {
      const img=el('img');img.src='/thumb/'+record.video_id;img.alt='';
      img.loading='lazy';img.addEventListener('error',()=>img.replaceWith(el('span','Video','pwa-empty-art')));
      cover.append(img);
    } else {
      cover.append(el('span',device?'Offline copy':'Video','pwa-empty-art'));
    }
    article.append(cover);
    const info=el('div',undefined,'library-card-info');
    const heading=el('h3',undefined,'pwa-card-title');
    heading.append(el('span',record.title||'Video '+record.video_id,'library-card-title'));
    info.append(heading);
    info.append(el('p',record.channel||'Channel not recorded','pwa-card-info'));
    info.append(el('p',device?'Saved on this device · '+M.bytes(record.size_bytes):
      (M.complete(record)?'Ready on server':record.status||'Not downloaded')+' · '+M.bytes(record.size_bytes),'pwa-card-status'));
    if(record.error)info.append(el('p',record.error,'error'));
    const actions=el('div',undefined,'pwa-card-actions');
    const saved=state.device.find(d=>d.id===record.id);
    if(!device&&M.complete(record))actions.append(button('Play from server',()=>playServer(record)));
    if(device||saved)actions.append(button('Play offline',()=>play(device?record:saved)));
    else if(M.complete(record))actions.append(button('Save to device',()=>saveDevice(record)));
    if(device)actions.append(button('Remove copy',async()=>{
      if(state.playing===record.id)stopPlayer();await DB.remove(record.id);await loadDevice();render();
    }));
    if(!device&&['failed','canceled'].includes(record.status))actions.append(button('Retry download',async()=>{
      await api('/api/entries/'+encodeURIComponent(record.id)+'/retry',{method:'POST'});await refresh();
    }));
    if(!device&&['queued','downloading'].includes(record.status))actions.append(button('Cancel download',async()=>{
      await api('/api/entries/'+encodeURIComponent(record.id)+'/cancel',{method:'POST'});await refresh();
    }));
    if(!device&&!record.id)actions.append(button('Download on server',()=>queue(record.video_id,record.title)));
    actions.append(button('Copy URL',()=>copy(M.sourceURL(record.source_url)||M.videoURL(record.video_id))));
    info.append(actions);article.append(info);return article;
  }
  function render() {
    for(const tab of document.querySelectorAll('[data-view]')){
      const selected=tab.dataset.view===state.view;
      tab.setAttribute('aria-pressed',String(selected));
      if(typeof tab.className==='string'&&tab.className.split(/\s+/).includes('side-item')){
        if(selected)tab.setAttribute('aria-current','page');else tab.removeAttribute('aria-current');
      }
    }
    const device=state.view==='device',feeds=state.view==='feeds';
    $('#view-title').textContent=device?'On this device':feeds?'Feeds':'Library';
    $('#view-hint').textContent=device?'Your complete offline video copies, with playback positions saved locally.':feeds?
      'Explore captured sources from YouTube and TikTok.':
      'Downloads, saved videos and provider captures from your Muninn server.';
    $('#view-meta').textContent=device?state.device.length+' device copies':feeds?state.feeds.length+' captured sources':state.entries.length+' server videos';
    $('#persist').hidden=!device;$('#feed-list').hidden=!feeds;$('#feed-filters').hidden=!feeds;
    $('#queue-panel').hidden=!state.signedIn||state.view!=='server';
    $('#server-count').textContent=state.entries.length;
    const content=$('#content');content.replaceChildren();
    if(feeds){renderFeeds();return;}
    const records=device?state.device:state.entries;
    for(const item of records)content.append(card(item,device));
    if(!records.length)content.append(el('p',device?'No device copies yet. Save a complete video from the server library.':state.signedIn?'No server downloads yet. Add a YouTube or TikTok video above.':'Connect to browse the server, or open On this device for saved videos.','pwa-empty'));
  }
  function renderFeeds() {
    const filters=$('#feed-filters');filters.replaceChildren();
    for(const [kind,label] of [['','All'],['home','YouTube · Home'],['subscriptions','YouTube · Subscriptions'],['search','YouTube · Search'],['channel','YouTube · Channels'],['related','YouTube · Related'],
      ['tiktok-for-you','TikTok · For You'],['tiktok-following','TikTok · Following'],['tiktok-profile','TikTok · Creators'],['tiktok-search','TikTok · Search'],['tiktok-related','TikTok · Related']]){
      const b=button(label,()=>{state.kind=kind;state.feed=null;render();});b.setAttribute('aria-pressed',String(state.kind===kind));filters.append(b);
    }
    const list=$('#feed-list');list.replaceChildren();
    for(const feed of state.feeds.filter(f=>!state.kind||state.kind===f.kind)){
      const b=button(M.feedTitle(feed)+' · '+new Date(feed.captured_at).toLocaleString(),async()=>{
        state.feed=await api('/api/feeds/'+feed.id);render();
      });b.className='feed-row';b.setAttribute('aria-pressed',String(state.feed?.id===feed.id));list.append(b);
    }
    const content=$('#content');
    if(state.feed){
      const info=el('section',undefined,'feed-context');info.append(el('h3',M.feedTitle(state.feed)));
      const url=M.sourceURL(state.feed.source_url);
      if(url){info.append(el('p',url,'source-url'),button('Copy source URL',()=>copy(url)));}
      else info.append(el('p','Original source URL was not recorded.'));
      content.append(info);
      for(const item of state.feed.items||[]){
        const entry=state.entries.find(e=>e.id===item.cached?.id);
        content.append(card(entry||{...item,id:null,status:'Not downloaded'}));
      }
    }else content.append(el('p',state.feeds.length?'Choose a captured source above.':'No feed captures on this server yet. You can still add videos by URL.','empty'));
  }
  async function queue(id,title) {
    if(!M.validId(id))throw new Error('Enter a YouTube or TikTok video URL (or a recognized source ID).');
    const quality=document.querySelector('input[name=quality]:checked').value;
    const result=await api('/api/cache',{method:'POST',body:{video_id:id,quality,title:title||undefined,protect:true}});
    notice(result.entry?'Queued on the server. It will keep downloading when you close this page.':'The server did not queue this video. Check its download rules.');
    await refresh();
  }
  async function refresh() {
    if(state.refreshing)return;state.refreshing=true;
    const epoch=state.epoch;
    try{
      await loadDevice();
      if(navigator.onLine){
        await api('/api/session');
        const [entries,feeds]=await Promise.all([api('/api/entries'),api('/api/feeds?limit=100')]);
        if(epoch!==state.epoch)return;
        state.entries=entries;state.feeds=feeds;setConnection(true);
      }else setConnection(false);
    }catch(error){if(epoch===state.epoch){setConnection(false);notice('Server unavailable: '+error.message+' Saved device copies remain playable.',true);}}
    finally{state.refreshing=false;if(epoch===state.epoch)render();}
  }
  async function saveDevice(entry) {
    if(state.transfer)throw new Error('Finish or cancel the current device transfer first.');
    if(!M.complete(entry))throw new Error('Wait for the server download to finish first.');
    const expected=Number(entry.size_bytes);
    if(!Number.isSafeInteger(expected)||expected<=0||expected>M.MAX_DEVICE_BYTES)throw new Error('The initial PWA supports device copies up to 512 MB. Try a lower-quality server download.');
    const controller=new AbortController(),epoch=state.epoch;state.transfer=controller;
    $('#transfer-panel').hidden=false;$('#transfer-progress').value=0;$('#transfer-status').textContent='Connecting…';
    try{
      const estimate=await navigator.storage?.estimate?.();
      if(estimate?.quota&&estimate.quota-(estimate.usage||0)<expected+10*1024*1024)throw new Error('Not enough browser storage. Remove some device copies first.');
      if(controller.signal.aborted||epoch!==state.epoch)throw new DOMException('Cancelled','AbortError');
      const response=await fetch('/media/'+encodeURIComponent(entry.id),{credentials:'same-origin',cache:'no-store',signal:controller.signal});
      if(response.status!==200)throw new Error(response.status===401?'Session expired. Connect again.':'Server file is not ready for a full transfer.');
      const type=response.headers.get('content-type')||'';
      const length=Number(response.headers.get('content-length'));
      if(length!==expected)throw new Error('The server file changed. Refresh the library and retry.');
      const chunks=[];let received=0;
      if(!response.body)throw new Error('This browser does not support device transfers.');
      const reader=response.body.getReader();
      while(true){const {done,value}=await reader.read();if(done)break;received+=value.byteLength;
        if(received>expected||received>M.MAX_DEVICE_BYTES){controller.abort();throw new Error('Transfer exceeded the expected size.');}
        chunks.push(value);$('#transfer-progress').value=received/expected;
        $('#transfer-status').textContent=M.bytes(received)+' / '+M.bytes(expected);
      }
      M.validateCopy(expected,received,type);
      if(controller.signal.aborted||epoch!==state.epoch)throw new DOMException('Cancelled','AbortError');
      await DB.save(entry,new Blob(chunks,{type}));
      if(epoch!==state.epoch){await DB.remove(entry.id);return;}
      await loadDevice();notice('Complete copy saved on this device. It is ready for offline playback.');render();
    }catch(error){notice(error.name==='AbortError'?'Device transfer cancelled; no partial copy retained.':error.message,true);}
    finally{state.transfer=null;$('#transfer-panel').hidden=true;}
  }
  $('#login-form').addEventListener('submit',async event=>{
    event.preventDefault();const field=$('#token'),token=field.value;field.value='';
    try{await api('/api/session',{method:'POST',body:{token}});notice('Connected.');await refresh();}catch(e){notice(e.message,true);}
  });
  $('#queue-form').addEventListener('submit',event=>{event.preventDefault();queue(M.videoId($('#url').value)).catch(e=>notice(e.message,true));});
  for(const b of document.querySelectorAll('[data-view]'))b.addEventListener('click',()=>{state.view=b.dataset.view;render();});
  // Mobile keeps the native rail visible; desktop can collapse the sidebar.
  $('#side-toggle').addEventListener('click',()=>{
    const shell=$('#app'),collapsed=shell.dataset.sidebar==='collapsed';
    shell.dataset.sidebar=collapsed?'expanded':'collapsed';
    $('#side-toggle').setAttribute('aria-expanded',String(collapsed));
    const label=$('#side-toggle .side-label');if(label)label.textContent=collapsed?'Collapse sidebar':'Expand sidebar';
  });
  $('#refresh').addEventListener('click',refresh);
  $('#close-player').addEventListener('click',stopPlayer);
  $('#cancel-transfer').addEventListener('click',()=>state.transfer?.abort());
  $('#close-copy').addEventListener('click',()=>{$('#copy-fallback').hidden=true;});
  $('#persist').addEventListener('click',async()=>notice(await navigator.storage?.persist?.()?'Persistent device storage granted.':'The browser did not grant persistent storage. Keep server originals.'));
  $('#logout').addEventListener('click',async()=>{
    if(!confirm('Sign out and remove all saved videos from this device? Server originals are kept.'))return;
    state.epoch++;state.transfer?.abort();stopPlayer();
    try{await DB.clear();await api('/api/session',{method:'DELETE'});state.entries=[];state.feeds=[];state.feed=null;state.device=[];$('#device-count').textContent='0';setConnection(false);render();notice('Signed out; device copies removed.');}
    catch(error){notice('Could not finish signing out: '+error.message+'. Retry when the server is reachable.',true);}
  });
  $('#player').addEventListener('error',()=>notice('This browser cannot play this codec or the video is unavailable. Try another quality or download a device copy.',true));
  $('#player').addEventListener('timeupdate',()=>{if(state.playing&&Date.now()-progressWrite>5000){progressWrite=Date.now();const saved=state.device.find(d=>d.id===state.playing);if(saved)saved.position_seconds=$('#player').currentTime;DB.position(state.playing,$('#player').currentTime).catch(()=>{});}});
  addEventListener('offline',()=>{setConnection(false);notice('Offline. Device copies remain available.');});
  addEventListener('online',refresh);
  addEventListener('beforeinstallprompt',event=>{event.preventDefault();installPrompt=event;$('#install').hidden=false;});
  $('#install').addEventListener('click',async()=>{await installPrompt?.prompt();installPrompt=null;$('#install').hidden=true;});
  if('serviceWorker' in navigator&&window.isSecureContext)navigator.serviceWorker.register('./sw.js',{scope:'./'}).catch(e=>notice('Offline app shell unavailable: '+e.message,true));
  if(!window.isSecureContext)notice('Use HTTPS to install this PWA and enable reliable offline storage.',true);
  refresh();
  setInterval(()=>{if(!document.hidden&&navigator.onLine&&state.signedIn)refresh();},15000);
})();
