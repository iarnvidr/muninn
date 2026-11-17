/* Additive browser UI: the existing PWA remains the only media player. */
(() => {
  'use strict';
  const node = (tag, text, cls) => {const n=document.createElement(tag);if(text)n.textContent=text;if(cls)n.className=cls;return n;};
  const panel=node('section',null,'panel remote-browser');panel.hidden=true;
  const tabs=document.querySelector('nav.tabs');
  const browse=node('button','YouTube browser');browse.type='button';browse.setAttribute('aria-pressed','false');tabs.append(browse);
  tabs.after(panel);
  let active=null, shown=false, busy=false, frameURL=null, lastState=null, actionTail=Promise.resolve(), pending=0;
  const heading=node('h2','YouTube · server browser');
  const explanation=node('p','Browse the real YouTube site on your server. Feeds and opened watch tabs are captured into Muninn. Video playback remains in On this device.','hint');
  const status=node('p','Start the browser to create a separate server-side YouTube session.','hint');status.setAttribute('role','status');
  const controls=node('div',null,'actions');
  const makeButton=(title,fn)=>{const b=node('button',title);b.type='button';b.addEventListener('click',()=>fn().catch(error=>{status.textContent=error.message;}));return b;};
  const tabBar=node('div',null,'tabs compact');tabBar.setAttribute('aria-label','Server browser tabs');
  const navigation=node('form',null,'input-row');
  const url=node('input');url.type='url';url.placeholder='https://www.youtube.com/';url.setAttribute('aria-label','YouTube URL');
  const go=node('button','Open');go.type='submit';navigation.append(url,go);
  const image=node('img');image.alt='Interactive server-side YouTube browser';image.tabIndex=0;image.width=1280;image.height=800;image.className='remote-frame';image.hidden=true;
  const input=node('form',null,'input-row');
  const text=node('input');text.type='password';text.autocomplete='off';text.placeholder='Type into the selected browser field';text.setAttribute('aria-label','Text sent to the server browser');
  const send=node('button','Type text');send.type='submit';input.append(text,send);
  const keyboard=node('div',null,'actions');
  for(const key of ['Tab','Enter','Backspace','Escape'])keyboard.append(makeButton(key,()=>action('key',{key})));
  keyboard.append(makeButton('Scroll up',()=>action('scroll',{dy:-600})),makeButton('Scroll down',()=>action('scroll',{dy:600})));
  const warning=node('p','Single-owner preview: signing into Google stores that session on this server, not on your Mac or phone. Everyone allowed into this preview and paired to Muninn can operate it. Reset removes the server browser profile.','hint');
  panel.append(heading,explanation,controls,tabBar,navigation,image,input,keyboard,status,warning);

  async function request(path, body) {
    const response=await fetch('/browser/'+path,{method:body?'POST':'GET',credentials:'same-origin',cache:'no-store',
      headers:{'X-Muninn-Client':'pwa',...(body?{'Content-Type':'application/json'}:{})},body:body?JSON.stringify(body):undefined});
    if(!response.ok)throw new Error(response.status===401?'Connect to Muninn first.':response.status===403?'Access refused. Reopen the preview through Authelia.':'Browser request failed ('+response.status+').');
    return response.json();
  }
  function render(state) {
    lastState=state;
    if(!state.tabs.some(t=>t.id===active))active=state.tabs.at(-1)?.id||null;
    tabBar.replaceChildren();
    for(const tab of state.tabs){
      const b=makeButton(tab.title||'New tab',async()=>{active=tab.id;render(state);await frame();});
      b.setAttribute('aria-pressed',String(tab.id===active));tabBar.append(b);
    }
    const tab=state.tabs.find(t=>t.id===active);
    if(document.activeElement!==url)url.value=tab?.url||'https://www.youtube.com/';
    status.textContent=state.running?`${state.captured} captures · ${state.dropped} skipped · ${state.failures} capture errors`:'Browser stopped. Start it to browse YouTube.';
    image.hidden=!tab;
  }
  function action(name, extra={}) {
    if(pending>=32)return Promise.reject(new Error('Input queue is full; wait for the browser to catch up.'));
    pending++;
    const task=actionTail.then(()=>performAction(name,extra));
    actionTail=task.catch(()=>{});
    return task.finally(()=>{pending--;});
  }
  async function performAction(name, extra) {
    busy=true;
    try{
      const previous=new Set(lastState?.tabs.map(t=>t.id)||[]);
      const state=await request('action',{action:name,tab:active,...extra});
      if(name==='new')active=state.tabs.find(t=>!previous.has(t.id))?.id||active;
      render(state);await frame();
    }finally{busy=false;}
  }
  async function frame(){
    if(!shown||!active||document.hidden||!navigator.onLine)return;
    const target=active;
    const response=await fetch('/browser/frame?tab='+encodeURIComponent(target),{credentials:'same-origin',cache:'no-store'});
    if(!response.ok)throw new Error('Browser frame unavailable; reconnect to Muninn.');
    const blob=await response.blob();
    if(target!==active||!shown)return;
    const next=URL.createObjectURL(blob);const old=frameURL;frameURL=next;image.src=next;if(old)URL.revokeObjectURL(old);
  }
  controls.append(makeButton('Start browser',()=>action('start')),makeButton('New tab',()=>action('new',{url:'https://www.youtube.com/'})),
    makeButton('Subscriptions',()=>action(active?'navigate':'new',{url:'https://www.youtube.com/feed/subscriptions'})),
    makeButton('Back',()=>action('back')),makeButton('Reload',()=>action('reload')),makeButton('Close tab',()=>action('close')),
    makeButton('Reset server browser',async()=>{if(confirm('Remove the server browser profile and Google sign-in? Downloaded videos are kept.'))await action('reset');}));
  navigation.addEventListener('submit',event=>{event.preventDefault();action(active?'navigate':'new',{url:url.value}).catch(e=>status.textContent=e.message);});
  input.addEventListener('submit',event=>{event.preventDefault();const value=text.value;text.value='';action('text',{text:value}).catch(e=>status.textContent=e.message);});
  image.addEventListener('click',event=>{const r=image.getBoundingClientRect();action('click',{x:(event.clientX-r.left)*1280/r.width,y:(event.clientY-r.top)*800/r.height}).catch(e=>status.textContent=e.message);image.focus();});
  image.addEventListener('keydown',event=>{
    if(event.key==='Tab'&&!event.shiftKey)return; // Tab exits the canvas; explicit Tab button controls remote focus.
    const named=['Enter','Backspace','Delete','Escape','ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End','PageUp','PageDown'];
    if(named.includes(event.key)){event.preventDefault();action('key',{key:event.key}).catch(e=>status.textContent=e.message);}
    else if(event.key.length===1&&!event.ctrlKey&&!event.metaKey&&!event.altKey){event.preventDefault();action('text',{text:event.key}).catch(e=>status.textContent=e.message);}
  });
  browse.addEventListener('click',async()=>{
    shown=!shown;panel.hidden=!shown;browse.setAttribute('aria-pressed',String(shown));
    if(!shown){image.removeAttribute('src');if(frameURL)URL.revokeObjectURL(frameURL);frameURL=null;return;}
    try{render(await request('state'));await frame();}catch(error){status.textContent=error.message;}
  });
  document.querySelector('#logout')?.addEventListener('click',()=>{shown=false;panel.hidden=true;image.removeAttribute('src');if(frameURL)URL.revokeObjectURL(frameURL);frameURL=null;});
  async function tick(){
    try{if(shown&&!busy&&!document.hidden&&navigator.onLine){render(await request('state'));await frame();}}
    catch(error){status.textContent=error.message;}
    finally{setTimeout(tick,1500);}
  }
  tick();
})();
