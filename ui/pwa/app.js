/* Standalone browser client. The native desktop UI and its lifecycle stay separate. */
(function(root) {
  'use strict';
  const ID=/^[A-Za-z0-9_-]{11}$/;
  const HOSTS=new Set(['youtube.com','www.youtube.com','m.youtube.com','music.youtube.com','youtu.be','www.youtu.be']);
  function videoId(input) {
    const text=String(input||'').trim();
    if(ID.test(text))return text;
    let url;try{url=new URL(text);}catch{return null;}
    if(!['http:','https:'].includes(url.protocol)||!HOSTS.has(url.hostname)||url.username||url.password||url.port)return null;
    const path=url.pathname.split('/').filter(Boolean);
    const id=url.hostname.endsWith('youtu.be')?path[0]:url.pathname==='/watch'?url.searchParams.get('v'):
      ['shorts','live','embed'].includes(path[0])?path[1]:null;
    return ID.test(id||'')?id:null;
  }
  function videoUrl(value){const id=videoId(value);return id?'https://www.youtube.com/watch?v='+id:null;}
  function sourceUrl(value) {
    if(!value)return null;
    let u;try{u=new URL(value);}catch{return null;}
    if(!['http:','https:'].includes(u.protocol)||!HOSTS.has(u.hostname)||u.username||u.password||u.port)return null;
    const id=videoId(value);if(id)return videoUrl(id);
    if(/^\/(?:@[^/]+|channel\/UC[A-Za-z0-9_-]{22})(?:\/(?:videos|featured|streams|shorts))?\/?$/.test(u.pathname))
      return 'https://www.youtube.com'+u.pathname;
    if(['/','/feed/subscriptions','/feed/history'].includes(u.pathname))return 'https://www.youtube.com'+u.pathname;
    if(u.pathname==='/results'&&u.searchParams.get('search_query'))return 'https://www.youtube.com/results?search_query='+encodeURIComponent(u.searchParams.get('search_query'));
    return null;
  }
  function mediaPath(value) {
    const p=String(value||'');
    if(/^\/media\/[A-Za-z0-9_-][A-Za-z0-9_.-]*$/.test(p))return p;
    if(/^\/stream\/[A-Za-z0-9_-][A-Za-z0-9_.-]*\/index\.m3u8$/.test(p))return p;
    return null;
  }
  function dateLabel(value) {
    if(value===null||value===undefined||value==='')return 'Capture time unavailable';
    const t=typeof value==='number'?value:new Date(value).valueOf();
    const d=new Date(t);return Number.isFinite(d.valueOf())?d.toLocaleString(undefined,{dateStyle:'medium',timeStyle:'short'}):'Capture time unavailable';
  }
  function feedLabel(feed) {
    const f=feed||{},kind=String(f.kind||'');
    const title=String(f.source_title||f.parent_video_title||f.source_video_title||f.parent_title||'').trim();
    if(kind==='related'){
      const id=videoId(f.source_url);
      return 'Related to '+(title||(id?'video '+id:'an unrecorded video'));
    }
    if(kind==='search')return f.query?'Search: '+f.query:'Search capture';
    if(kind==='channel'){
      if(title||f.channel_title||f.channel)return title||f.channel_title||f.channel;
      const src=sourceUrl(f.source_url);const handle=src?.match(/\/@([^/]+)/)?.[1];
      if(handle){try{return '@'+decodeURIComponent(handle);}catch{return '@'+handle;}}
      return 'Channel · source name not recorded';
    }
    return {home:'YouTube Home',subscriptions:'Subscriptions',other:'Saved feed'}[kind]||'Saved feed';
  }
  function duration(value) {
    const total=Math.max(0,Math.floor(Number(value)||0));
    if(!total)return '';
    return total>=3600?Math.floor(total/3600)+':'+String(Math.floor(total%3600/60)).padStart(2,'0')+':'+String(total%60).padStart(2,'0'):
      Math.floor(total/60)+':'+String(total%60).padStart(2,'0');
  }
  function humanBytes(value){const n=Number(value)||0;return n>=1e9?(n/1e9).toFixed(1)+' GB':n>=1e6?(n/1e6).toFixed(0)+' MB':n>0?Math.round(n/1024)+' KB':'';}
  function complete(row){return row?.status==='complete'||row?.streamable===true;}
  function cacheLabel(row) {
    if(row.protected||row.cached?.state==='protected')return 'Kept in library';
    if(complete(row)||['complete','speculative'].includes(row.cached?.state))return 'Ready to play';
    if(row.status==='downloading')return 'Downloading';
    if(row.status==='queued')return 'Queued';
    if(row.status==='failed')return 'Download failed';
    if(row.cached?.state==='partial')return 'Preparing on server';
    return row.cached?.id?'Copy on server':'';
  }
  function groupLibrary(rows) {
    const grouped=new Map();
    const score=r=>(complete(r)?8:0)+(r.protected?4:0)+(r.status==='downloading'?2:0)+(r.status==='queued'?1:0);
    for(const row of Array.isArray(rows)?rows:[]){
      if(!ID.test(row?.video_id||''))continue;
      const old=grouped.get(row.video_id);
      if(!old||score(row)>score(old))grouped.set(row.video_id,row);
    }
    return [...grouped.values()].sort((a,b)=>(Number(b.created_at)||0)-(Number(a.created_at)||0));
  }
  function filterLibrary(rows,filter,query) {
    const q=String(query||'').trim().toLowerCase();
    return groupLibrary(rows).filter(r=>(!q||[r.title,r.channel,r.video_id].some(x=>String(x||'').toLowerCase().includes(q)))&&
      (filter==='all'||filter==='ready'&&complete(r)||filter==='kept'&&r.protected||filter==='downloading'&&['queued','downloading'].includes(r.status)));
  }
  function requestOptions(method='GET',body) {
    const o={method,credentials:'same-origin',cache:'no-store',headers:{Accept:'application/json'}};
    if(body!==undefined){o.headers['Content-Type']='application/json';o.body=JSON.stringify(body);}
    return o;
  }
  function cacheRequest(row) {
    const requested=String(row.requested_quality||row.variant||'720').toLowerCase().replace(/p$/,'');
    const quality=['audio','360','480','720','1080'].includes(requested)?requested:'720';
    return {video_id:row.video_id,quality,protect:row.protected===true,title:row.title||null,channel:row.channel||null};
  }
  function pairingOptions(token) {
    const text=String(token||'').trim();
    if(!text||text.length>1024||/[\r\n]/.test(text))throw new Error('Enter the server pairing token.');
    const o=requestOptions('POST');o.headers.Authorization='Bearer '+text;return o;
  }
  async function startSession(token,send) {
    const options=pairingOptions(token);
    try{
      const response=await send('/app/session',options);
      const data=await response.json();
      if(!response.ok||!data.authenticated)throw new Error(data.error||'This pairing token was not accepted.');
      return data;
    }finally{delete options.headers.Authorization;}
  }
  async function loadWatchSnapshot(id,send) {
    // A cached file can emit loadedmetadata immediately. Resolve its stored
    // resume point before exposing any playable source to the video element.
    const [status,detail]=await Promise.allSettled([
      send('/api/watch/'+id,requestOptions('POST',{})),
      send('/api/video/'+id)
    ]);
    if(status.status==='rejected')throw status.reason;
    return {status:status.value,detail:detail.status==='fulfilled'?detail.value:null};
  }
  function playbackTimeline(playback,currentTime,mediaDuration) {
    const p=playback||{},local=p.local===true;
    const offset=local?Math.max(0,Number(p.offset_seconds)||0):0;
    const position=offset+Math.max(0,Number.isFinite(Number(currentTime))?Number(currentTime):0);
    const fullDuration=local?Number(p.duration_seconds):Number(mediaDuration);
    const total=Number.isFinite(fullDuration)&&fullDuration>0?fullDuration:null;
    return {position,total,atEnd:total!==null&&position>=total-2,continueWindow:local&&total!==null&&position<total-2};
  }
  const api={videoId,videoUrl,sourceUrl,mediaPath,feedLabel,dateLabel,duration,humanBytes,cacheLabel,groupLibrary,filterLibrary,requestOptions,cacheRequest,pairingOptions,startSession,loadWatchSnapshot,playbackTimeline};
  if(typeof module!=='undefined'&&module.exports)module.exports=api;
  if(root)root.MuninnPwa=api;
  if(!root?.document)return;

  const doc=root.document,$=s=>doc.querySelector(s);
  const dom={login:$('#sign-in'),workspace:$('#workspace'),results:$('#results'),filters:$('#filters'),context:$('#feed-context'),
    title:$('#section-title'),description:$('#section-description'),eyebrow:$('#section-eyebrow'),search:$('#search-input'),
    player:$('#player-section'),video:$('#video'),playerState:$('#player-state'),connection:$('#connection')};
  let authenticated=false,page='library',libraryFilter='all',feedKind='',library=[],feeds=[],entries=[],selectedFeed=null,
    searchResults=null,loadGeneration=0,watchGeneration=0,watch=null,hls=null,installPrompt=null,pollTimer=null,watchTimer=null,
    toastTimer=null,hlsPromise=null,lastProgress=0;
  function element(tag,klass,text){const n=doc.createElement(tag);if(klass)n.className=klass;if(text!==undefined)n.textContent=String(text);return n;}
  function button(text,fn,klass){const b=element('button',klass,text);b.type='button';b.addEventListener('click',fn);return b;}
  function toast(text,error=false){const t=$('#toast');t.textContent=text;t.dataset.error=String(error);t.hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>{t.hidden=true;},5000);}
  function connection(online){dom.connection.textContent=online?'Server connected':'Server unavailable';dom.connection.dataset.state=online?'connected':'offline';}
  function showLogin(message=''){
    authenticated=false;++loadGeneration;stopWatch();clearInterval(pollTimer);pollTimer=null;
    library=[];entries=[];feeds=[];searchResults=null;selectedFeed=null;
    dom.results.replaceChildren();dom.context.replaceChildren();dom.search.value='';
    dom.login.hidden=false;dom.workspace.hidden=true;$('#sign-out').hidden=true;
    $('#login-message').textContent=message;
    dom.connection.textContent='Not connected';dom.connection.dataset.state='disconnected';
  }
  async function jsonRequest(path,options=requestOptions()) {
    let response;try{response=await fetch(path,options);}catch{connection(false);throw new Error('Cannot reach your Muninn server. Check your connection and try again.');}
    let data;try{data=await response.json();}catch{throw new Error('The server returned an unreadable response. Try again.');}
    if(response.status===401){showLogin('Your session has ended. Connect with the server pairing token again.');throw new Error('Please connect to your server.');}
    if(!response.ok)throw new Error(data.error||'The request could not be completed.');
    connection(true);return data;
  }
  function empty(title,text,icon='▦'){
    const box=element('div','empty-state');box.append(element('span','empty-icon',icon),element('h2','',title),element('p','',text));return box;
  }
  function loading(){dom.results.className='results';dom.results.replaceChildren(element('div','loading-state'));dom.results.firstChild.append(element('p','','Loading from your server…'));}
  function showError(error){const box=element('div','error-state');box.append(element('h2','','Couldn’t load this view'),element('p','',error.message||String(error)),button('Try again',()=>loadPage()));dom.results.replaceChildren(box);}
  async function copy(text){
    if(!text){toast('This capture has no recorded source URL.',true);return;}
    try{await navigator.clipboard.writeText(text);toast('URL copied');}
    catch{
      const input=element('textarea','sr-only');input.value=text;input.setAttribute('readonly','');doc.body.append(input);input.select();
      const copied=doc.execCommand?.('copy');input.remove();
      if(copied)toast('URL copied');else toast('Copy unavailable. Open the source link to copy its address.',true);
    }
  }
  function videoTitle(row){return String(row.title||row.video_title||'Video '+(row.video_id||row.id||''));}
  function thumbnail(id){
    const img=element('img');let failures=0;img.alt='';img.loading='lazy';
    img.addEventListener('load',()=>{img.hidden=false;});
    img.addEventListener('error',()=>{
      img.hidden=true;
      // The first request asks the server to fetch a missing thumbnail. Give that
      // background job a bounded chance to finish, without contacting YouTube.
      if(failures>=3)return;
      const delay=[1500,4000,8000][failures++];
      setTimeout(()=>{if(authenticated&&img.isConnected)img.src='/thumb/'+id+'?retry='+failures;},delay);
    });
    img.src='/thumb/'+id;return img;
  }
  function card(row){
    const id=row.video_id||row.id;if(!ID.test(id||''))return null;
    const c=element('article','video-card');
    const thumb=button('',()=>openWatch({...row,video_id:id}),'video-thumb');thumb.setAttribute('aria-label','Play '+videoTitle(row));
    const img=thumbnail(id);
    thumb.append(img,element('span','thumb-mark','M'),element('span','play-mark','▶'));
    if(row.duration_seconds)thumb.append(element('span','duration',duration(row.duration_seconds)));
    const title=element('h3','',videoTitle(row));title.title=videoTitle(row);
    const meta=element('p','video-meta',row.channel||'Channel not recorded');meta.title=row.channel||'';
    const actions=element('div','card-actions');
    const label=cacheLabel(row);
    if(label)actions.append(element('span','state-label '+(row.status||''),label));
    else actions.append(button('↓ Download',e=>queueVideo({...row,video_id:id},e.currentTarget)));
    actions.append(button('Copy URL',()=>copy(videoUrl(id)),'copy'));
    c.append(thumb,title,meta,actions);return c;
  }
  function setFilters(options,current,onSelect){dom.filters.replaceChildren();for(const [value,label] of options){const b=button(label,()=>onSelect(value));b.setAttribute('aria-pressed',String(value===current));dom.filters.append(b);}}
  function drawCards(rows,emptyTitle,emptyText){dom.results.className='results';dom.results.replaceChildren();for(const row of rows){const c=card(row);if(c)dom.results.append(c);}if(!dom.results.children.length)dom.results.append(empty(emptyTitle,emptyText));}
  function renderLibrary(){
    dom.title.textContent=searchResults?'Search results':'Library';dom.eyebrow.textContent=searchResults?'FROM YOUTUBE':'ON YOUR SERVER';
    dom.description.textContent=searchResults?'Choose a video to play or download on your server.':groupLibrary(library).length+' videos in your server library';
    dom.context.hidden=true;
    if(searchResults){
      dom.filters.replaceChildren(button('← Back to library',()=>{searchResults=null;dom.search.value='';renderLibrary();}));
      drawCards(searchResults,'No videos found','Try another search or paste a YouTube video URL.');
    }else{
      setFilters([['all','All videos'],['ready','Ready to play'],['kept','Kept'],['downloading','Downloading']],libraryFilter,value=>{libraryFilter=value;renderLibrary();});
      const rows=filterLibrary(library,libraryFilter,dom.search.value);
      drawCards(rows,library.length?'No matching videos':'Your library starts here',library.length?'Try another filter or search.':'Paste a YouTube video URL above, or search for a video to download to your server.');
    }
  }
  function renderFeeds(){
    dom.title.textContent='Feeds';dom.eyebrow.textContent='SAVED DISCOVERY';dom.description.textContent='Explore feed snapshots stored on your server.';dom.context.hidden=true;
    setFilters([['','All captures'],['home','Home'],['subscriptions','Subscriptions'],['related','Related'],['channel','Channels'],['search','Search']],feedKind,value=>{feedKind=value;selectedFeed=null;renderFeeds();});
    dom.results.className='results';dom.results.replaceChildren();
    for(const f of feeds.filter(f=>!feedKind||f.kind===feedKind)){
      const box=element('article','feed-card');box.append(element('div','feed-kind',f.kind||'Feed'),element('h3','',feedLabel(f)),element('p','',dateLabel(f.captured_at)+' · '+(f.count||0)+' videos'));
      const actions=element('div','inline-actions');actions.append(button('Explore capture →',()=>loadFeed(f)));
      const src=sourceUrl(f.source_url);if(src)actions.append(button('Copy URL',()=>copy(src)));
      box.append(actions);dom.results.append(box);
    }
    if(!dom.results.children.length)dom.results.append(empty('No saved captures yet','The server shows feeds already saved in its data directory. Your browser cannot capture native YouTube tabs. You can still search and add videos above.','◉'));
  }
  async function loadFeed(f){
    const gen=++loadGeneration;loading();selectedFeed=f;
    try{
      const data=await jsonRequest('/api/feeds/'+encodeURIComponent(f.id));if(gen!==loadGeneration||!authenticated)return;
      const details={...f,...data};dom.title.textContent=feedLabel(details);dom.eyebrow.textContent='SAVED FEED';dom.description.textContent=dateLabel(details.captured_at)+' · '+(details.items?.length||0)+' videos';
      dom.filters.replaceChildren(button('← All captures',()=>{selectedFeed=null;renderFeeds();}));
      const src=sourceUrl(details.source_url);dom.context.replaceChildren();dom.context.hidden=false;
      dom.context.append(element('p','',src?'Source: '+src:details.kind==='related'?'The parent video was not recorded in this older capture.':'The source URL was not recorded for this capture.'));
      if(src){const actions=element('div','inline-actions'),a=element('a','','Open source ↗');a.href=src;a.target='_blank';a.rel='noopener noreferrer';actions.append(button('Copy URL',()=>copy(src)),a);dom.context.append(actions);}
      drawCards(details.items||[],'This capture is empty','No video items are stored in this snapshot.');
    }catch(error){if(gen===loadGeneration&&authenticated)showError(error);}
  }
  function renderDownloads(){
    dom.title.textContent='Downloads';dom.eyebrow.textContent='WORKING ON YOUR SERVER';dom.description.textContent='Downloads continue on your server when you close this app.';dom.filters.replaceChildren();dom.context.hidden=true;
    const rows=entries.filter(r=>['queued','downloading','failed'].includes(r.status));dom.results.className='results download-list';dom.results.replaceChildren();
    for(const r of rows){
      const row=element('article','download-row'),img=thumbnail(r.video_id);
      const info=element('div');info.append(element('h3','',videoTitle(r)));
      const percent=Math.max(0,Math.min(100,Math.round((Number(r.progress)||0)*100)));
      info.append(element('p','',r.status==='failed'?(r.error||'Download failed'):r.status==='queued'?'Queued on server':percent+'%'+(r.size_bytes?' · '+humanBytes(r.size_bytes):'')));
      const progress=element('progress');progress.max=100;progress.value=percent;progress.setAttribute('aria-label','Download progress');if(r.status==='downloading')info.append(progress);
      row.append(img,info,button(r.status==='failed'?'Retry':'Copy URL',e=>r.status==='failed'?queueVideo(r,e.currentTarget):copy(videoUrl(r.video_id))));dom.results.append(row);
    }
    if(!rows.length)dom.results.append(empty('All caught up','New downloads will appear here. Search or paste a video URL to add one to your server.','↓'));
  }
  function setPage(next){page=next;++loadGeneration;searchResults=null;selectedFeed=null;dom.search.value='';for(const b of doc.querySelectorAll('[data-page]')){if(b.dataset.page===page)b.setAttribute('aria-current','page');else b.removeAttribute('aria-current');}loadPage();}
  async function loadPage(){
    if(!authenticated)return;const gen=++loadGeneration;loading();
    try{
      const data=await jsonRequest(page==='library'?'/api/library':page==='feeds'?'/api/feeds?limit=100':'/api/entries');
      if(gen!==loadGeneration||!authenticated)return;
      if(page==='library')library=data;else if(page==='feeds')feeds=data;else entries=data;
      if(page==='library')renderLibrary();else if(page==='feeds')renderFeeds();else renderDownloads();
    }catch(error){if(gen===loadGeneration&&authenticated)showError(error);}
  }
  async function queueVideo(row,b){
    if(b)b.disabled=true;
    try{const result=await jsonRequest('/api/cache',requestOptions('POST',cacheRequest(row)));
      if(!result.entry)throw new Error('The server’s download policy did not queue this video.');
      toast('Queued on your server');await refreshQueue();if(page==='library'&&!searchResults)loadPage();
    }catch(error){toast(error.message,true);}finally{if(b)b.disabled=false;}
  }
  async function refreshQueue(){
    if(!authenticated||doc.hidden)return;
    try{const data=await jsonRequest('/api/entries');if(!authenticated)return;entries=data;const n=entries.filter(e=>['queued','downloading'].includes(e.status)).length;const badge=$('#download-count');badge.textContent=n;badge.hidden=!n;if(page==='downloads')renderDownloads();}catch{}
  }
  function clearMedia(){hls?.destroy();hls=null;dom.video.pause();dom.video.removeAttribute('src');dom.video.load();}
  function stopWatch(){
    ++watchGeneration;clearTimeout(watchTimer);watchTimer=null;
    watch=null;clearMedia();lastProgress=0;dom.player.hidden=true;$('#player-keep').hidden=true;$('#player-keep').textContent='Keep in library';$('#player-retry').hidden=true;
  }
  async function loadHls(){
    if(root.Hls)return root.Hls;
    if(!hlsPromise)hlsPromise=new Promise((resolve,reject)=>{const script=element('script');script.src='/app/hls.min.js';script.onload=()=>root.Hls?resolve(root.Hls):reject(new Error('HLS player unavailable'));script.onerror=()=>{hlsPromise=null;reject(new Error('The local player component could not be loaded.'));};doc.head.append(script);});
    return hlsPromise;
  }
  async function attachMedia(playback,gen){
    const path=mediaPath(playback.url_path);if(!path)throw new Error('The server did not return a valid cached media source.');
    if(gen!==watchGeneration)return;
    watch.entryId=playback.entry_id;watch.playback=playback;watch.ready=true;
    const restore=()=>{
      if(gen!==watchGeneration||!watch)return;
      const offset=playback.local?Math.max(0,Number(playback.offset_seconds)||0):0;
      const position=Math.max(0,watch.position-offset);
      if(position>0&&Number.isFinite(dom.video.duration)&&position<dom.video.duration-1)dom.video.currentTime=position;
      if(watch.continuePlaying){watch.continuePlaying=false;dom.video.play().catch(()=>{dom.playerState.textContent='The next part is ready. Press Play to continue.';});}
    };
    dom.video.addEventListener('loadedmetadata',restore,{once:true});
    const nativeHls=dom.video.canPlayType('application/vnd.apple.mpegurl');
    if(playback.kind==='hls'&&!nativeHls){
      const Hls=await loadHls();if(gen!==watchGeneration)return;
      if(!Hls.isSupported())throw new Error('This browser cannot play this cached format. Try Safari or a current Chromium browser.');
      hls=new Hls({enableWorker:false,xhrSetup:xhr=>{xhr.withCredentials=true;}});
      hls.on(Hls.Events.ERROR,(_,data)=>{if(data.fatal&&gen===watchGeneration){dom.playerState.textContent='Playback was interrupted. Check your connection, then retry.';$('#player-retry').hidden=false;}});
      hls.loadSource(path);hls.attachMedia(dom.video);
    }else dom.video.src=path;
    if(gen!==watchGeneration)return;
    $('#player-keep').hidden=!watch.entryId;dom.playerState.textContent='Ready. Press Play to watch from your server.';
  }
  async function inspectWatch(status,gen,attempt){
    if(gen!==watchGeneration||!watch)return;
    const playback=status?.playback||status||{};
    if(playback.ready){await attachMedia(playback,gen);return;}
    if(playback.state==='error'||playback.error)throw new Error(playback.error||'The server could not prepare this video.');
    if(attempt>=45)throw new Error('The server is still preparing this video. Check Downloads, then retry.');
    const pct=Math.round((Number(playback.progress)||0)*100);
    dom.playerState.textContent=(playback.state==='resolving'?'Finding a playable source on your server…':'Caching video on your server…')+(pct>0?' '+pct+'%':'');
    watchTimer=setTimeout(async()=>{
      if(gen!==watchGeneration||!watch)return;
      try{const next=await jsonRequest(watch.compatibility?'/api/entries/'+encodeURIComponent(watch.entryId)+'/playback':'/api/watch/'+watch.video_id);await inspectWatch(next,gen,attempt+1);}
      catch(error){if(gen===watchGeneration){dom.playerState.textContent=error.message;$('#player-retry').hidden=false;}}
    },2000);
  }
  async function openWatch(row){
    const id=videoId(row.video_id||row.id);if(!id){toast('This is not a supported YouTube video.',true);return;}
    if(watch?.ready){dom.video.pause();recordPlayback(false,false,true);}
    stopWatch();const gen=watchGeneration;watch={...row,video_id:id,title:videoTitle({...row,video_id:id}),position:Number(row.position_seconds)||0,ready:false};
    dom.player.hidden=false;$('#player-title').textContent=watch.title;$('#player-subtitle').textContent=row.channel||'Saved and served by Muninn';dom.playerState.textContent='Preparing video on your server…';
    dom.player.scrollIntoView({behavior:'smooth',block:'start'});
    try{
      const {status,detail}=await loadWatchSnapshot(id,jsonRequest);if(gen!==watchGeneration||!watch)return;
      const meta=detail?.meta||{};
      if(meta.title){watch.title=meta.title;$('#player-title').textContent=meta.title;}
      if(meta.channel)$('#player-subtitle').textContent=meta.channel;
      if(!watch.position)watch.position=Number(detail?.position_seconds)||0;
      await inspectWatch(status,gen,0);
    }catch(error){if(gen===watchGeneration){dom.playerState.textContent=error.message;$('#player-retry').hidden=false;}}
  }
  async function recordPlayback(started=false,ended=false,force=false){
    if(!watch?.ready||!authenticated)return;
    if(!force&&!started&&!ended&&Date.now()-lastProgress<10000)return;lastProgress=Date.now();
    const timeline=playbackTimeline(watch.playback,dom.video.currentTime,dom.video.duration);
    const data={video_id:watch.video_id,position_seconds:timeline.position,duration_seconds:timeline.total,
      paused:dom.video.paused,started,ended:ended&&timeline.atEnd};
    try{await jsonRequest('/api/ingest/playback',requestOptions('POST',data));}catch{}
  }
  async function prepareCompatible(position,continuePlaying=false){
    if(!watch?.entryId)return;
    const gen=++watchGeneration;clearTimeout(watchTimer);watchTimer=null;
    watch.ready=false;watch.position=Math.max(0,Number(position)||0);watch.compatibility=true;watch.continuePlaying=continuePlaying;
    clearMedia();$('#player-retry').hidden=true;dom.playerState.textContent='Preparing browser playback from your saved copy…';
    try{
      const status=await jsonRequest('/api/entries/'+encodeURIComponent(watch.entryId)+'/playback?position='+watch.position,requestOptions('POST'));
      await inspectWatch(status,gen,0);
    }catch(error){if(gen===watchGeneration){dom.playerState.textContent=error.message;$('#player-retry').hidden=false;}}
  }
  async function connect(){
    try{
      const response=await fetch('/app/session',requestOptions());const session=await response.json();
      if(!response.ok||!session.authenticated){showLogin();return;}
      authenticated=true;dom.login.hidden=true;dom.workspace.hidden=false;$('#sign-out').hidden=false;
      $('#server-version').textContent=session.version?'Muninn '+session.version:'';connection(true);await loadPage();refreshQueue();
      clearInterval(pollTimer);pollTimer=setInterval(refreshQueue,5000);
    }catch{showLogin('Cannot reach your Muninn server. Check the address and your connection.');connection(false);}
  }
  $('#login-form').addEventListener('submit',async event=>{
    event.preventDefault();const input=$('#pairing-token'),submit=event.currentTarget.querySelector('button');submit.disabled=true;$('#login-message').textContent='';
    const pairing=startSession(input.value,fetch);input.value='';
    try{await pairing;await connect();}
    catch(error){$('#login-message').textContent=error.message||'Could not connect to the server.';}
    finally{submit.disabled=false;}
  });
  $('#sign-out').addEventListener('click',async()=>{try{await jsonRequest('/app/session',requestOptions('DELETE'));showLogin('Signed out of this browser.');}catch(error){toast('Could not sign out from the server. Reconnect and try again.',true);}});
  doc.querySelectorAll('[data-page]').forEach(b=>b.addEventListener('click',()=>setPage(b.dataset.page)));
  $('#refresh').addEventListener('click',()=>selectedFeed?loadFeed(selectedFeed):loadPage());
  dom.search.addEventListener('input',()=>{if(page==='library'&&!searchResults)renderLibrary();});
  $('#search-form').addEventListener('submit',async event=>{
    event.preventDefault();const input=dom.search.value.trim();if(!input)return;
    const id=videoId(input);if(id){openWatch({video_id:id,title:'Video '+id});return;}
    if(/^https?:\/\//i.test(input)||input.includes('://')){toast('Paste a YouTube watch, Shorts or youtu.be video URL.',true);return;}
    if(input.length>200){toast('Use a search of 200 characters or fewer.',true);return;}
    page='library';selectedFeed=null;for(const b of doc.querySelectorAll('[data-page]')){if(b.dataset.page==='library')b.setAttribute('aria-current','page');else b.removeAttribute('aria-current');}
    const gen=++loadGeneration;loading();const submit=event.currentTarget.querySelector('button');submit.disabled=true;
    try{const found=await jsonRequest('/api/search',requestOptions('POST',{query:input,category:'videos'}));if(gen!==loadGeneration||!authenticated)return;searchResults=(found.items||[]).map(r=>({...r,video_id:r.id}));renderLibrary();}
    catch(error){if(gen===loadGeneration&&authenticated)showError(error);}finally{submit.disabled=false;}
  });
  $('#close-player').addEventListener('click',()=>{dom.video.pause();recordPlayback(false,false,true);stopWatch();});
  $('#player-copy').addEventListener('click',()=>copy(videoUrl(watch?.video_id)));
  $('#player-retry').addEventListener('click',()=>{
    if(!watch)return;
    if(watch.entryId&&(watch.playback?.kind==='file'||watch.playback?.local||watch.compatibility)){
      const position=watch.ready?playbackTimeline(watch.playback,dom.video.currentTime,dom.video.duration).position:watch.position;
      prepareCompatible(position);
    }else openWatch({...watch});
  });
  $('#player-keep').addEventListener('click',async e=>{if(!watch?.entryId)return;const b=e.currentTarget;b.disabled=true;try{await jsonRequest('/api/entries/'+encodeURIComponent(watch.entryId)+'/promote',requestOptions('POST'));b.textContent='Kept in library';toast('Video kept in your server library');}catch(error){toast(error.message,true);}finally{b.disabled=false;}});
  dom.video.addEventListener('playing',()=>{dom.playerState.textContent='Playing from your server’s cache';recordPlayback(true,false);});
  dom.video.addEventListener('timeupdate',()=>recordPlayback());dom.video.addEventListener('pause',()=>{lastProgress=0;recordPlayback();});
  dom.video.addEventListener('ended',()=>{
    if(!watch?.ready)return;
    const timeline=playbackTimeline(watch.playback,dom.video.currentTime,dom.video.duration);
    recordPlayback(false,true);
    if(timeline.continueWindow)prepareCompatible(timeline.position,true);
  });
  dom.video.addEventListener('error',()=>{if(watch?.ready){dom.playerState.textContent='This media could not play in your browser. Retry to prepare a compatible copy.';$('#player-retry').hidden=false;}});
  $('#server-address').textContent=location.origin;
  root.addEventListener('beforeinstallprompt',event=>{event.preventDefault();installPrompt=event;});
  $('#install').addEventListener('click',async()=>{if(installPrompt){installPrompt.prompt();await installPrompt.userChoice;installPrompt=null;}else $('#install-help').showModal();});
  $('#close-install').addEventListener('click',()=>$('#install-help').close());
  if(root.matchMedia?.('(display-mode: standalone)').matches||navigator.standalone)$('#install').hidden=true;
  function onlineState(){const offline=navigator.onLine===false;$('#offline-banner').hidden=!offline;if(offline)connection(false);else if(authenticated)refreshQueue();}
  root.addEventListener('online',onlineState);root.addEventListener('offline',onlineState);onlineState();
  doc.addEventListener('visibilitychange',()=>{if(!doc.hidden)refreshQueue();});
  if('serviceWorker'in navigator&&root.isSecureContext){navigator.serviceWorker.register('/app/sw.js',{scope:'/app/'}).catch(()=>{});}
  connect();
})(typeof window==='undefined'?null:window);
