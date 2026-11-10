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
test('service worker caches static shell without authentication credentials',async()=>{
  const w=worker();let pending;w.events.install({waitUntil:p=>pending=p});await pending;
  const requests=w.calls.find(c=>c[0]==='addAll')[1];
  assert.ok(requests.length>=8);assert.ok(requests.every(r=>r.credentials==='omit'&&r.url.startsWith('https://muninn.test/app/')));
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
test('service worker supports offline shell but not authenticated response caching',async()=>{
  const w=worker();let pending;
  w.events.fetch({request:new Request('https://muninn.test/app/app.js'),respondWith:p=>pending=p});
  const response=await pending;assert.equal(await response.text(),'shell');
  assert.equal(w.calls.find(c=>c[0]==='fetch')[1].credentials,'omit');
});
test('device storage commits metadata and media together without secrets',()=>{
  const source=file('store.js');assert.match(source,/transaction\(\['metadata','media'\]/);assert.match(source,/tx.oncomplete/);
  assert.doesNotMatch(source,/localStorage|sessionStorage|authorization|document.cookie/i);
});
test('PWA is a separate client with a blob-only player and visible feed filters',()=>{
  const source=file('app.js'),html=file('index.html');
  assert.match(source,/player.src=state.playerURL/);assert.match(source,/URL.createObjectURL\(blob\)/);
  assert.doesNotMatch(source,/__TAURI|youtube\.com\/embed|player.src\s*=\s*['"]\/media|innerHTML|localStorage/);
  assert.doesNotMatch(html,/<select/);assert.match(html,/On this device/);assert.match(html,/playsinline/);
  for(const sourceName of ['app.js','model.js','store.js','sw.js'])assert.doesNotThrow(()=>new vm.Script(file(sourceName)),sourceName);
});
