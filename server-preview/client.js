/* Additive browser UI: the existing PWA remains the only media player. */
(() => {
  'use strict';
  const node = (tag, text, cls) => {const n=document.createElement(tag);if(text)n.textContent=text;if(cls)n.className=cls;return n;};
  // The desktop-parity PWA has a sidebar + tabstrip, not the obsolete
  // nav.tabs placeholder. Mount in its dedicated first-class Browse view.
  const host=document.querySelector('#browser-host');
  if(!host)return;
  const panel=node('section',null,'pwa-panel remote-browser');
  host.append(panel);
  let active=null, shown=false, busy=false, frameURL=null, lastState=null, actionTail=Promise.resolve(), pending=0, frameTab=null, frameSequence=0;
  const heading=node('h2','YouTube and TikTok');
  const explanation=node('p','Browse the real provider pages through your private server browser. Muninn captures feeds and video links, but upstream video playback is blocked in the browsing canvas; downloaded videos play in Muninn.','hint');
  const status=node('p','Start the private browser to capture feeds and opened videos.','hint');status.setAttribute('role','status');
  const controls=node('div',null,'actions');
  const makeButton=(title,fn)=>{const b=node('button',title);b.type='button';b.addEventListener('click',()=>fn().catch(error=>{status.textContent=error.message;}));return b;};
  const tabBar=node('div',null,'tabs compact');tabBar.setAttribute('aria-label','Server browser tabs');
  const navigation=node('form',null,'input-row');
  const url=node('input');url.type='url';url.placeholder='YouTube or TikTok page URL';url.setAttribute('aria-label','YouTube or TikTok URL');
  const go=node('button','Go');go.type='submit';navigation.append(url,go);
  const search=node('form',null,'input-row remote-search');
  const query=node('input');query.type='search';query.maxLength=120;
  query.placeholder='Search YouTube or TikTok';query.setAttribute('aria-label','Search videos and creators');
  const ytSearch=node('button','Search YouTube');ytSearch.type='submit';ytSearch.value='youtube';ytSearch.name='provider';
  const ttSearch=node('button','Search TikTok');ttSearch.type='submit';ttSearch.value='tiktok';ttSearch.name='provider';
  search.append(query,ytSearch,ttSearch);
  const image=node('img');image.alt='Interactive server-side provider browser';image.tabIndex=0;image.width=1280;image.height=800;image.className='remote-frame';image.hidden=true;
  const viewport=node('div',null,'remote-frame-viewport');viewport.append(image);
  const mobileHint=node('p','On a phone, drag sideways to pan across the page, swipe vertically to scroll YouTube or TikTok, and tap to open a video.','hint remote-touch-hint');
  const input=node('form',null,'input-row');
  const text=node('input');text.type='password';text.autocomplete='off';text.placeholder='Type into the selected browser field';text.setAttribute('aria-label','Text sent to the server browser');
  const send=node('button','Type text');send.type='submit';input.append(text,send);
  const keyboard=node('div',null,'actions');
  for(const key of ['Tab','Enter','Backspace','Escape'])keyboard.append(makeButton(key,()=>action('key',{key})));
  keyboard.append(makeButton('Scroll up',()=>action('scroll',{dy:-600})),makeButton('Scroll down',()=>action('scroll',{dy:600})));
  const warning=node('p','Single-owner preview: signing into Google or TikTok stores those sessions on this server, not on your Mac or phone. Anyone allowed into this preview and paired to Muninn can operate it. Reset removes both sessions.','hint');
  panel.append(heading,explanation,controls,search,tabBar,navigation,viewport,mobileHint,input,keyboard,status,warning);

  async function request(path, body) {
    const response=await fetch('/browser/'+path,{method:body?'POST':'GET',credentials:'same-origin',cache:'no-store',
      headers:{'X-Muninn-Client':'pwa',...(body?{'Content-Type':'application/json'}:{})},body:body?JSON.stringify(body):undefined});
    if(!response.ok)throw new Error(response.status===401?'Connect to Muninn first.':response.status===403?'Access refused. Reopen the preview through Authelia.':'Browser request failed ('+response.status+').');
    return response.json();
  }
  function clearFrame() {
    frameSequence++;frameTab=null;image.hidden=true;image.removeAttribute('src');
    if(touchX!==null){touchMoved=true;touchX=null;touchY=null;touchAxis=null;}
    if(frameURL)URL.revokeObjectURL(frameURL);
    frameURL=null;
  }
  function selectTab(target) {
    if(target!==active){clearFrame();active=target;}
  }
  function canvasReady(){return shown&&active!==null&&frameTab===active;}
  function render(state) {
    lastState=state;
    if(!state.tabs.some(t=>t.id===active))selectTab(state.tabs.at(-1)?.id||null);
    tabBar.replaceChildren();
    for(const tab of state.tabs){
      const b=makeButton(tab.title||'New tab',async()=>{selectTab(tab.id);render(state);await frame();});
      b.setAttribute('aria-pressed',String(tab.id===active));tabBar.append(b);
    }
    const tab=state.tabs.find(t=>t.id===active);
    if(document.activeElement!==url)url.value=tab?.url||'https://www.youtube.com/';
    status.textContent=state.running?`${state.captured} captures · ${state.dropped} skipped · ${state.failures} capture errors`:'Browser stopped. Start it to browse YouTube or TikTok.';
    image.hidden=!tab||!canvasReady();
  }
  function action(name, extra={}) {
    if(pending>=32)return Promise.reject(new Error('Input queue is full; wait for the browser to catch up.'));
    pending++;
    // Bind queued input to the tab the user was looking at when it occurred.
    // Changing tabs while a prior request is slow must not retarget a click,
    // scroll or keystroke (including text entered into a provider sign-in form).
    const target=active;
    const task=actionTail.then(()=>performAction(name,extra,target));
    actionTail=task.catch(()=>{});
    return task.finally(()=>{pending--;});
  }
  async function performAction(name, extra, target) {
    busy=true;
    try{
      const previous=new Set(lastState?.tabs.map(t=>t.id)||[]);
      const state=await request('action',{action:name,...extra,tab:target});
      if(name==='new')selectTab(state.tabs.find(t=>!previous.has(t.id))?.id||active);
      render(state);await frame();
    }finally{busy=false;}
  }
  async function frame(){
    if(!shown||!active||document.hidden||!navigator.onLine)return;
    const target=active, sequence=++frameSequence;
    const response=await fetch('/browser/frame?tab='+encodeURIComponent(target),{credentials:'same-origin',cache:'no-store'});
    if(sequence!==frameSequence||target!==active||!shown)return;
    if(!response.ok)throw new Error('Browser frame unavailable; reconnect to Muninn.');
    const blob=await response.blob();
    if(sequence!==frameSequence||target!==active||!shown)return;
    const next=URL.createObjectURL(blob);const old=frameURL;frameURL=next;image.src=next;if(old)URL.revokeObjectURL(old);
    // Safari may still be decoding the new image after src is assigned. Keep
    // a switched tab inert until its own screenshot is actually ready.
    try{if(typeof image.decode==='function')await image.decode();}
    catch(error){if(sequence===frameSequence)throw error;return;}
    if(sequence!==frameSequence||target!==active||!shown)return;
    frameTab=target;image.hidden=false;
  }
  const goTo=url=>action(active?'navigate':'new',{url});
  controls.append(makeButton('YouTube Home',()=>goTo('https://www.youtube.com/')),
    makeButton('Subscriptions',()=>goTo('https://www.youtube.com/feed/subscriptions')),
    makeButton('TikTok For You',()=>goTo('https://www.tiktok.com/foryou')),
    makeButton('TikTok Following',()=>goTo('https://www.tiktok.com/following')),
    makeButton('New tab',()=>action('new',{url:'https://www.youtube.com/'})),
    makeButton('Back',()=>action('back')),makeButton('Reload',()=>action('reload')),
    makeButton('Save current video',async()=>{
      const tab=lastState?.tabs.find(t=>t.id===active);
      if(!tab?.video_id)throw new Error('Open a YouTube or TikTok video before saving it.');
      const response=await fetch('/api/cache',{method:'POST',credentials:'same-origin',cache:'no-store',
        headers:{'X-Muninn-Client':'pwa','Content-Type':'application/json'},
        body:JSON.stringify({video_id:tab.video_id,quality:'720',title:tab.title,protect:true})});
      if(!response.ok)throw new Error('Unable to queue video ('+response.status+').');
      document.dispatchEvent(new CustomEvent('muninn:video-queued',{detail:{id:tab.video_id}}));
      status.textContent='Video queued. Open Library to monitor and play the downloaded copy.';
    }),
    makeButton('Close tab',()=>action('close')),
    makeButton('Reset browser login',async()=>{
      if(confirm('Remove server browser sessions for YouTube and TikTok? Downloaded videos are kept.'))await action('reset');
    }));
  search.addEventListener('submit',event=>{
    event.preventDefault();
    const term=query.value.trim();
    if(!term)return;
    const provider=event.submitter?.value==='tiktok'?'tiktok':'youtube';
    const target=provider==='tiktok'?'https://www.tiktok.com/search?q='+encodeURIComponent(term):
      'https://www.youtube.com/results?search_query='+encodeURIComponent(term);
    goTo(target).catch(e=>status.textContent=e.message);
  });
  navigation.addEventListener('submit',event=>{event.preventDefault();action(active?'navigate':'new',{url:url.value}).catch(e=>status.textContent=e.message);});
  input.addEventListener('submit',event=>{event.preventDefault();const value=text.value;text.value='';action('text',{text:value}).catch(e=>status.textContent=e.message);});
  // Wheel/trackpad and mobile swipes scroll the remote provider page, not
  // the PWA document. Native upstream video is still blocked by policy.py.
  image.addEventListener('wheel',event=>{
    if(!canvasReady())return;
    event.preventDefault();
    action('scroll',{dy:Math.round(Math.max(-1600,Math.min(1600,event.deltaY)))})
      .catch(e=>status.textContent=e.message);
  },{passive:false});
  // The screenshot is a 1280px Chromium viewport. A phone needs horizontal
  // panning to read it, while vertical gestures must scroll the remote site.
  // Lock each gesture's axis so a sideways pan cannot queue remote scrolls.
  let touchX=null,touchY=null,touchAxis=null,touchMoved=false;
  image.addEventListener('touchstart',event=>{
    const touch=canvasReady()&&event.touches.length===1?event.touches[0]:null;
    touchX=touch?.clientX??null;touchY=touch?.clientY??null;
    touchAxis=null;touchMoved=false;
  },{passive:true});
  image.addEventListener('touchmove',event=>{
    if(!canvasReady()||touchX===null||touchY===null||event.touches.length!==1)return;
    const nextX=event.touches[0].clientX,nextY=event.touches[0].clientY;
    const dx=touchX-nextX,dy=touchY-nextY;
    if(!touchAxis){
      if(Math.max(Math.abs(dx),Math.abs(dy))<7)return;
      touchAxis=Math.abs(dx)>Math.abs(dy)?'horizontal':'vertical';
    }
    touchMoved=true;
    if(touchAxis==='horizontal')return; // Native scrolling inside the image viewport.
    event.preventDefault();touchX=nextX;touchY=nextY;
    action('scroll',{dy:Math.round(Math.max(-1600,Math.min(1600,dy*3)))})
      .catch(e=>status.textContent=e.message);
  },{passive:false});
  image.addEventListener('touchend',event=>{
    if(touchMoved&&touchAxis==='vertical')event.preventDefault();
    touchX=null;touchY=null;touchAxis=null;
  },{passive:false});
  image.addEventListener('click',event=>{
    if(touchMoved){touchMoved=false;return;}
    if(!canvasReady())return;
    const r=image.getBoundingClientRect();
    action('click',{x:(event.clientX-r.left)*1280/r.width,y:(event.clientY-r.top)*800/r.height})
      .catch(e=>status.textContent=e.message);
    image.focus();
  });
  image.addEventListener('keydown',event=>{
    if(!canvasReady())return;
    if(event.key==='Tab'&&!event.shiftKey)return; // Tab exits the canvas; explicit Tab button controls remote focus.
    const named=['Enter','Backspace','Delete','Escape','ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End','PageUp','PageDown'];
    if(named.includes(event.key)){event.preventDefault();action('key',{key:event.key}).catch(e=>status.textContent=e.message);}
    else if(event.key.length===1&&!event.ctrlKey&&!event.metaKey&&!event.altKey){event.preventDefault();action('text',{text:event.key}).catch(e=>status.textContent=e.message);}
  });
  async function setVisible(visible){
    if(visible===shown)return;
    shown=visible;
    if(!shown){
      clearFrame();return;
    }
    try{
      const state=await request('state');render(state);
      if(!state.tabs.length)await action('start');
      else await frame();
    }catch(error){status.textContent=error.message;}
  }
  document.addEventListener('muninn:view-change',event=>{setVisible(event.detail?.visible===true);});
  setVisible(!host.hidden);
  document.querySelector('#logout')?.addEventListener('click',()=>{shown=false;panel.hidden=true;clearFrame();});
  async function tick(){
    try{if(shown&&!busy&&!document.hidden&&navigator.onLine){render(await request('state'));await frame();}}
    catch(error){status.textContent=error.message;}
    finally{setTimeout(tick,1500);}
  }
  tick();
})();
