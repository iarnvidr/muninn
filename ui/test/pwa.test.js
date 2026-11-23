'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const M=require('../pwa/model.js');
const dir=path.join(__dirname,'../pwa');
const file=name=>fs.readFileSync(path.join(dir,name),'utf8');
const ID='aBcD_efg-12';
test('PWA accepts known YouTube video URL forms and strips tracking',()=>{
  for(const value of [ID,'https://www.youtube.com/watch?v='+ID+'&list=x','https://youtu.be/'+ID+'?si=secret',
    'https://m.youtube.com/shorts/'+ID,'https://www.youtube.com/live/'+ID])assert.equal(M.videoId(value),ID);
  assert.equal(M.youtubeURL(ID),'https://www.youtube.com/watch?v='+ID);
});
test('TikTok is a native provider ID and never a YouTube URL',()=>{
  const id='tt_7668090902816017671';
  assert.equal(M.videoId('https://www.tiktok.com/@creator/video/7668090902816017671?msToken=private'),id);
  assert.equal(M.videoId('https://m.tiktok.com/@creator/video/7668090902816017671'),id);
  assert.equal(M.videoId(id),id);
  assert.equal(M.videoURL(id),'https://www.tiktok.com/@_/video/7668090902816017671');
  assert.equal(M.youtubeURL(id),null);
  assert.equal(M.sourceURL('https://www.tiktok.com/@creator/video/7668090902816017671?foo=private'),
    'https://www.tiktok.com/@creator/video/7668090902816017671');
  assert.equal(M.sourceURL('https://www.tiktok.com/foryou?msToken=private'),'https://www.tiktok.com/foryou');
  assert.equal(M.sourceURL('https://www.tiktok.com/search?q=Rust%20media&msToken=private'),
    'https://www.tiktok.com/search?q=Rust%20media');
  assert.equal(M.feedTitle({kind:'tiktok-search',source_url:'https://www.tiktok.com/search?q=Rust%20media'}),
    'TikTok · Search: Rust media');
  assert.equal(M.feedTitle({kind:'tiktok-profile',source_url:'https://www.tiktok.com/@creator'}),'TikTok · @creator');
  for(const url of ['https://tiktok.com.evil.test/@creator/video/7668090902816017671',
    'https://user:secret@tiktok.com/@creator/video/7668090902816017671',
    'https://tiktok.com:8080/@creator/video/7668090902816017671',
    'https://tiktok.com/@creator/video/1'])assert.equal(M.videoId(url),null,url);
});
test('PWA never turns arbitrary hosts, schemes or credentials into download requests',()=>{
  for(const input of ['javascript:alert(1)','file:///etc/passwd','https://evil.test/watch?v='+ID,
    'https://youtube.com.evil.test/watch?v='+ID,'https://user:secret@youtube.com/watch?v='+ID,
    'https://www.youtube.com/playlist?list=PLx','short'])assert.equal(M.videoId(input),null,input);
  assert.equal(M.youtubeURL('invalid'),null);
});
test('feed labels explain parent video and missing provenance honestly',()=>{
  assert.equal(M.feedTitle({kind:'related',source_url:'https://youtu.be/'+ID,source_title:'A real title'}),'Related to: A real title');
  assert.match(M.feedTitle({kind:'related',source_url:'https://youtu.be/'+ID}),/aBcD_efg-12/);
  assert.match(M.feedTitle({kind:'related'}),/not recorded/);
  assert.equal(M.feedTitle({kind:'search',query:'Rust media'}),'Search: Rust media');
  assert.equal(M.feedTitle({kind:'channel',source_url:'https://www.youtube.com/@creator/videos'}),'@creator');
});
test('copy source removes credentials, sensitive queries and unsupported hosts',()=>{
  assert.equal(M.sourceURL('https://www.youtube.com/watch?v='+ID+'&token=secret'),'https://www.youtube.com/watch?v='+ID);
  assert.equal(M.sourceURL('https://www.youtube.com/feed/subscriptions?auth=secret'),'https://www.youtube.com/feed/subscriptions');
  assert.equal(M.sourceURL('https://evil.test/'),null);
  assert.equal(M.sourceURL('https://user@youtube.com/'),null);
});
test('only completed entries and complete bounded media blobs can become device copies',()=>{
  assert.equal(M.complete({status:'complete'}),true);
  for(const status of ['queued','downloading','failed','canceled'])assert.equal(M.complete({status}),false);
  assert.doesNotThrow(()=>M.validateCopy(100,100,'video/mp4'));
  for(const args of [[100,99,'video/mp4'],[0,0,'video/mp4'],[M.MAX_DEVICE_BYTES+1,M.MAX_DEVICE_BYTES+1,'video/mp4'],[100,100,'text/html']])assert.throws(()=>M.validateCopy(...args));
});
test('manifest installs only the additive /app/ scope',()=>{
  const manifest=JSON.parse(file('manifest.webmanifest'));
  assert.equal(manifest.scope,'/app/');assert.equal(manifest.start_url,'/app/');assert.equal(manifest.display,'standalone');
  for(const icon of manifest.icons)assert.ok(fs.existsSync(path.join(dir,path.basename(icon.src))));
  assert.ok(manifest.icons.some(i=>i.sizes==='192x192'));
});
function worker(){
  const events={},calls=[];
  const cache={addAll:async requests=>calls.push(['addAll',requests]),put:async(...args)=>calls.push(['put',...args]),match:async()=>new Response('offline shell')};
  const ctx={self:{location:{origin:'https://muninn.test'},clients:{claim:async()=>{}},addEventListener:(name,fn)=>events[name]=fn},
    caches:{open:async()=>cache,keys:async()=>['unrelated-cache'],delete:async name=>calls.push(['delete',name])},
    fetch:async request=>{calls.push(['fetch',request]);return new Response('shell',{status:200});},
    Request:class extends Request{constructor(input,options){super(typeof input==='string'?new URL(input,'https://muninn.test'):input,options);}},Response,URL};
  vm.runInNewContext(file('sw.js'),ctx);
  return {events,calls};
}
test('service worker caches only owner-authorized static shell and refuses login redirects',async()=>{
  const w=worker();let pending;w.events.install({waitUntil:p=>pending=p});await pending;
  const requests=w.calls.find(c=>c[0]==='addAll')[1];
  assert.ok(requests.length>=8);
  assert.ok(requests.every(r=>r.credentials==='same-origin'&&r.redirect==='error'&&r.url.startsWith('https://muninn.test/app/')));
});
test('service worker never intercepts private APIs, media, sessions or foreign requests',()=>{
  const w=worker();
  for(const target of ['/api/session','/api/entries','/media/some-file','/thumb/'+ID,'/app/?token=secret','https://evil.test/app/']){
    let intercepted=false;
    w.events.fetch({request:new Request(new URL(target,'https://muninn.test')),respondWith:()=>intercepted=true});
    assert.equal(intercepted,false,target);
  }
  let hit=false;w.events.fetch({request:new Request('https://muninn.test/app/app.js',{headers:{Authorization:'Bearer secret'}}),respondWith:()=>hit=true});assert.equal(hit,false);
});
test('service worker supports offline shell without following owner-login redirects',async()=>{
  const w=worker();let pending;
  w.events.fetch({request:new Request('https://muninn.test/app/app.js'),respondWith:p=>pending=p});
  const response=await pending;assert.equal(await response.text(),'shell');
  assert.equal(w.calls.find(c=>c[0]==='fetch')[1].credentials,'same-origin');
  assert.equal(w.calls.find(c=>c[0]==='fetch')[1].redirect,'error');
});
test('device storage commits metadata and media together without secrets',()=>{
  const source=file('store.js');assert.match(source,/transaction\(\['metadata','media'\]/);assert.match(source,/tx.oncomplete/);
  assert.doesNotMatch(source,/localStorage|sessionStorage|authorization|document.cookie/i);
});
test('PWA is a separate client with a blob-only player and visible feed filters',()=>{
  const source=file('app.js'),html=file('index.html');
  assert.match(source,/player.src=state.playerURL/);assert.match(source,/URL.createObjectURL\(blob\)/);
  assert.doesNotMatch(source,/__TAURI|youtube\.com\/embed|innerHTML|localStorage/);
  assert.doesNotMatch(html,/<select/);assert.match(html,/On this device/);assert.match(html,/playsinline/);
  for(const sourceName of ['app.js','model.js','store.js','sw.js'])assert.doesNotThrow(()=>new vm.Script(file(sourceName)),sourceName);
});

test('PWA uses the desktop shell and authenticates completed server media',()=>{
  const html=file('index.html'),js=file('app.js'),css=file('style.css'),sw=file('sw.js');
  const adapter=fs.readFileSync(path.join(__dirname,'../../crates/server/src/web/mod.rs'),'utf8');
  assert.ok(html.includes('href="./desktop.css"'));
  assert.ok(html.includes('class="app"'));
  assert.ok(html.includes('class="muninn-brand"'));
  assert.ok(html.includes('class="library-cards"'));
  assert.ok(html.includes('id="side-toggle"'));
  assert.ok(css.includes('desktop.css'));
  assert.ok(sw.includes('/app/desktop.css'));
  assert.ok(adapter.includes('"desktop.css"'));
  assert.ok(adapter.includes('include_bytes!("../../../../ui/style.css")'));
  assert.ok(js.includes("player.src='/media/'+encodeURIComponent(record.id)"));
  assert.ok(js.includes('M.complete(record)'));
  assert.ok(js.includes('player.src=state.playerURL'));
  assert.doesNotMatch(js,/youtube\.com\/embed|tiktok\.com\/embed|__TAURI/);
});

test('PWA honors server-selected authentication without token fallback',()=>{
  const html=file('index.html'),js=file('app.js');
  assert.match(html,/name="muninn-auth-mode" content="__MUNINN_AUTH_MODE__"/);
  assert.match(js,/const tokenLogin = authMode === 'token'/);
  assert.match(js,/if\(!tokenLogin\) \{/);
  assert.match(js,/panel\.replaceChildren\(/);
  assert.match(js,/Reconnect through Authelia/);
  assert.match(js,/if\(tokenLogin\) \$\('#login-form'\)\.addEventListener/);
  assert.match(js,/!signedIn\|\|!tokenLogin/);
  assert.doesNotMatch(js,/localStorage|sessionStorage|document\.cookie/);
});

test('the guarded source browser has a first-class default Browse view instead of the legacy detached tab',()=>{
  const html=file('index.html'),app=file('app.js');
  const browser=fs.readFileSync(path.join(__dirname,'../../server-preview/client.js'),'utf8');
  const gateway=fs.readFileSync(path.join(__dirname,'../../server-preview/gateway.py'),'utf8');
  assert.match(html,/id="browser-host"/);
  assert.match(html,/data-view="browse"/);
  assert.match(app,/muninn-source-browser/);
  assert.match(app,/view:browserEnabled && navigator\.onLine \? 'browse'/);
  assert.match(app,/muninn:view-change/);
  assert.match(browser,/querySelector\('#browser-host'\)/);
  assert.doesNotMatch(browser,/querySelector\('nav\.tabs'\)/);
  for(const command of ['YouTube Home','Subscriptions','TikTok For You','TikTok Following','Search YouTube','Search TikTok','Save current video']) {
    assert.ok(browser.includes(command),command);
  }
  assert.match(gateway,/muninn-source-browser/);
  assert.match(gateway,/muninn-pwa-shell-browser-v3/);
  assert.doesNotMatch(browser,/youtube\.com\/embed|tiktok\.com\/embed|innerHTML|localStorage/);
});

test('the private Chromium browsing client mounts and starts against the real PWA host',async()=>{
  const browser=fs.readFileSync(path.join(__dirname,'../../server-preview/client.js'),'utf8');
  class Element{
    constructor(tag='div'){this.tag=tag;this.children=[];this.listeners={};this.hidden=false;this.value='';this.textContent='';this.className='';}
    append(...children){this.children.push(...children);}
    replaceChildren(...children){this.children=children;}
    setAttribute(){}removeAttribute(){}focus(){}
    addEventListener(name,callback){(this.listeners[name]??=[]).push(callback);}
  }
  const host=new Element(),events={},calls=[],page={id:'tab1',title:'Provider tab A',url:'https://www.youtube.com/',video_id:null};
  const other={id:'tab2',title:'Provider tab B',url:'https://www.tiktok.com/foryou',video_id:null};
  let holdNextAction=false, releaseAction;
  host.hidden=false;
  const document={
    hidden:false,activeElement:null,
    querySelector:selector=>selector==='#browser-host'?host:null,
    createElement:tag=>new Element(tag),
    addEventListener:(name,fn)=>events[name]=fn
  };
  const context={document,navigator:{onLine:true},confirm:()=>false,setTimeout:()=>0,
    URL:{createObjectURL:()=>'blob:frame',revokeObjectURL:()=>{}},
    fetch:async(url,options={})=>{
      calls.push({url,options});
      if(url==='/browser/state')return {ok:true,json:async()=>({running:false,tabs:[],captured:0,dropped:0,failures:0})};
      if(url==='/browser/action'){
        if(holdNextAction){holdNextAction=false;await new Promise(resolve=>{releaseAction=resolve;});}
        return {ok:true,json:async()=>({running:true,tabs:[page,other],captured:0,dropped:0,failures:0})};
      }
      if(url.startsWith('/browser/frame?tab='))return {ok:true,blob:async()=>new Blob(['jpeg'])};
      throw new Error('Unexpected browser request '+url);
    }
  };
  vm.runInNewContext(browser,context);
  const flush=async()=>{for(let i=0;i<6;i++)await new Promise(resolve=>setImmediate(resolve));};
  await flush();
  assert.equal(host.children.length,1,'real Browse container should receive the browser canvas');
  assert.ok(calls.some(call=>call.url==='/browser/state'));
  assert.ok(calls.some(call=>call.url==='/browser/action'&&JSON.parse(call.options.body).action==='start'));
  assert.ok(calls.some(call=>call.url.startsWith('/browser/frame?tab=')));
  const all=node=>[node,...node.children.flatMap(all)];
  const search=all(host).find(node=>node.className==='input-row remote-search');
  assert.ok(search,'both providers need an accessible search form');
  const viewport=all(host).find(node=>node.className==='remote-frame-viewport');
  assert.ok(viewport,'mobile provider screenshots must be horizontally pannable');
  const remote=viewport.children[0];
  search.children[0].value='Rust engineering';
  search.listeners.submit[0]({preventDefault(){},submitter:{value:'tiktok'}});
  await flush();
  assert.ok(calls.some(call=>call.url==='/browser/action' &&
    JSON.parse(call.options.body).url==='https://www.tiktok.com/search?q=Rust%20engineering'));
  search.listeners.submit[0]({preventDefault(){},submitter:{value:'youtube'}});
  await flush();
  assert.ok(calls.some(call=>call.url==='/browser/action' &&
    JSON.parse(call.options.body).url==='https://www.youtube.com/results?search_query=Rust%20engineering'));
  const before=()=>calls.filter(call=>call.url==='/browser/action' && JSON.parse(call.options.body).action==='scroll').length;
  let prevented=false;
  const beforePan=before();
  remote.listeners.touchstart[0]({touches:[{clientX:160,clientY:100}]});
  remote.listeners.touchmove[0]({touches:[{clientX:90,clientY:98}],preventDefault(){prevented=true;}});
  await flush();
  assert.equal(prevented,false,'horizontal mobile pan must remain native');
  assert.equal(before(),beforePan,'horizontal pan must not queue remote scrolling');
  remote.listeners.touchstart[0]({touches:[{clientX:160,clientY:200}]});
  remote.listeners.touchmove[0]({touches:[{clientX:159,clientY:160}],preventDefault(){prevented=true;}});
  await flush();
  assert.equal(prevented,true,'vertical swipe must scroll remote page instead of PWA');
  assert.ok(before()>beforePan,'vertical swipe must send a remote scroll action');
  // A slow action can leave scrolls/text in the client queue while the user
  // switches tabs. No queued input may be redirected to the new provider.
  const tabButton=title=>all(host).find(node=>node.tag==='button'&&node.textContent===title);
  tabButton('Provider tab A').listeners.click[0]();await flush();
  const offset=calls.length;
  holdNextAction=true;
  remote.listeners.wheel[0]({deltaY:40,preventDefault(){}});await flush();
  assert.equal(typeof releaseAction,'function');
  remote.listeners.keydown[0]({key:'x',preventDefault(){}});
  tabButton('Provider tab B').listeners.click[0]();await flush();
  releaseAction();await flush();
  const queued=calls.slice(offset).filter(call=>call.url==='/browser/action').map(call=>JSON.parse(call.options.body));
  assert.deepEqual(queued.map(x=>[x.action,x.tab]),[['scroll','tab1'],['text','tab1']],
    'queued input must target the original tab, not whichever tab is selected at delivery');
  events['muninn:view-change']({detail:{visible:false}});
  await flush();
  assert.ok(events['muninn:view-change'],'browser responds to PWA navigation changes');
});


test('mobile server-browser canvas remains readable and can pan horizontally',()=>{
  const css=fs.readFileSync(path.join(__dirname,'../../server-preview/client.css'),'utf8');
  assert.match(css,/remote-frame-viewport\{[^}]*overflow-x:auto/);
  assert.match(css,/touch-action:pan-x;width:min\(100%,112vh\)/);
  assert.match(css,/max-height:none/);
  assert.match(css,/@media\(max-width:600px\)\{[\s\S]*?remote-frame\{width:960px/);
});
