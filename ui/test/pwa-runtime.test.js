'use strict';
const test=require('node:test'), assert=require('node:assert/strict'), vm=require('node:vm');
const fs=require('node:fs'),path=require('node:path');
const M=require('../pwa/model.js');
const source=fs.readFileSync(path.join(__dirname,'../pwa/app.js'),'utf8');
// Unit harness for the real application logic. This is not browser/PWA acceptance.
class Element {
  constructor(tag='div'){this.tag=tag;this.children=[];this.listeners={};this.attrs={};this.dataset={};this.hidden=false;this.value='';this.textContent='';this.classList={toggle(){}};this.currentTime=0;this.duration=12;}
  append(...nodes){this.children.push(...nodes);}
  replaceChildren(...nodes){this.children=nodes;}
  setAttribute(k,v){this.attrs[k]=String(v);}
  removeAttribute(k){delete this.attrs[k];if(k==='src')this.src=undefined;}
  addEventListener(name,fn){(this.listeners[name]??=[]).push(fn);}
  pause(){} load(){} focus(){} select(){}
  async play(){this.played=true;}
  async emit(name){for(const fn of this.listeners[name]||[])await fn({preventDefault(){}});}
}
const flatten=node=>[node,...node.children.flatMap(flatten)];
async function app({complete=true,truncate=false}={}){
  const nodes=new Map();
  const get=id=>{if(!nodes.has(id))nodes.set(id,new Element());return nodes.get(id);};
  const tabs=['server','device','feeds'].map(view=>{const b=new Element('button');b.dataset.view=view;return b;});
  const bytes=new Uint8Array(64).fill(5);
  const entry={id:'test-entry',video_id:'aBcD_efg-12',title:'A saved video',channel:'A channel',status:complete?'complete':'downloading',size_bytes:64};
  const stored=new Map(),requests=[],events={},copied=[],saved=[];
  let authed=true;
  const DB={
    list:async()=>[...stored.values()].map(x=>x.meta),get:async id=>stored.get(id)?.blob,
    save:async(e,blob)=>{saved.push(e.id);stored.set(e.id,{meta:{...e},blob});},
    clear:async()=>stored.clear(),remove:async id=>stored.delete(id),position:async()=>{}
  };
  const navigator={onLine:true,storage:{estimate:async()=>({quota:1000000000,usage:0}),persist:async()=>true},clipboard:{writeText:async url=>copied.push(url)}};
  const context={document:{hidden:false,querySelector:selector=>selector.startsWith('#')?get(selector):{value:'720'},querySelectorAll:()=>tabs,createElement:tag=>new Element(tag)},
    navigator,MuninnWebModel:M,MuninnWebStore:DB,Blob,AbortController,DOMException,
    URL:class extends URL{static createObjectURL(blob){assert.ok(blob instanceof Blob);return 'blob:device-copy';}static revokeObjectURL(){}},
    isSecureContext:true,confirm:()=>true,setInterval:()=>0,
    addEventListener:(type,fn)=>{events[type]=fn;},
    fetch:async(url,options={})=>{
      requests.push({url,options});
      if(!navigator.onLine)throw new Error('offline');
      if(url==='/api/session'){
        if(options.method==='DELETE')authed=false;
        if(options.method==='POST')authed=true;
        return new Response(JSON.stringify(authed||options.method==='DELETE'?{ok:true}:{error:'unauthorized'}),{status:authed||options.method==='DELETE'?200:401});
      }
      if(url==='/api/entries')return Response.json([entry]);
      if(url.startsWith('/api/feeds'))return Response.json([]);
      if(url==='/api/cache')return Response.json({entry:{...entry,status:'queued'}});
      if(url==='/media/test-entry')return new Response(truncate?bytes.subarray(0,20):bytes,{headers:{'content-length':'64','content-type':'video/mp4'}});
      throw new Error('Unexpected request '+url);
    }
  };
  context.window=context;
  vm.runInNewContext(source,context);
  const flush=async()=>{for(let i=0;i<5;i++)await new Promise(resolve=>setImmediate(resolve));};
  await flush();
  const find=text=>flatten(get('#content')).find(e=>e.tag==='button'&&e.textContent===text);
  return {get,tabs,requests,stored,events,navigator,copied,saved,entry,flush,find};
}
test('PWA saves a complete server file, then plays only its persisted device blob offline',async()=>{
  const h=await app();
  assert.ok(h.find('Save to device'));
  await h.find('Save to device').emit('click');await h.flush();
  assert.deepEqual(h.saved,['test-entry']);
  assert.equal(h.stored.get('test-entry').blob.size,64);
  assert.ok(h.find('Play local copy'));
  await h.find('Play local copy').emit('click');
  assert.equal(h.get('#player').src,'blob:device-copy');
  assert.equal(h.get('#player').played,true);
  h.navigator.onLine=false;h.events.offline();
  await h.tabs.find(t=>t.dataset.view==='device').emit('click');
  const before=h.requests.length;
  await h.find('Play local copy').emit('click');
  assert.equal(h.requests.length,before,'offline playback must not fetch any media or session');
  assert.equal(h.get('#player').src,'blob:device-copy');
});
test('PWA never offers partial server videos as playable or downloadable device copies',async()=>{
  const h=await app({complete:false});
  assert.equal(h.find('Save to device'),undefined);
  assert.equal(h.find('Play local copy'),undefined);
  assert.ok(h.find('Cancel server download'));
});
test('truncated device transfers do not create a playable record',async()=>{
  const h=await app({truncate:true});
  await h.find('Save to device').emit('click');await h.flush();
  assert.equal(h.stored.size,0);assert.deepEqual(h.saved,[]);
  assert.match(h.get('#notice').textContent,/Incomplete transfer/);
  assert.equal(h.find('Play local copy'),undefined);
});
test('URL queueing uses protected server API and logout clears downloaded device copies',async()=>{
  const h=await app();
  h.get('#url').value='https://youtu.be/aBcD_efg-12?si=tracking';
  await h.get('#queue-form').emit('submit');await h.flush();
  const req=h.requests.find(r=>r.url==='/api/cache');assert.ok(req);
  assert.equal(req.options.method,'POST');assert.equal(req.options.headers['X-Muninn-Client'],'pwa');
  assert.equal(JSON.parse(req.options.body).video_id,'aBcD_efg-12');
  assert.equal(JSON.parse(req.options.body).protect,true);
  await h.find('Copy video URL').emit('click');
  assert.deepEqual(h.copied,['https://www.youtube.com/watch?v=aBcD_efg-12']);
  await h.find('Save to device').emit('click');await h.flush();
  assert.equal(h.stored.size,1);
  await h.get('#logout').emit('click');await h.flush();
  assert.equal(h.stored.size,0);assert.equal(h.get('#login-panel').hidden,false);
  assert.ok(h.requests.some(r=>r.url==='/api/session'&&r.options.method==='DELETE'));
});
