'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const model=require('../pwa/model.js');
const source=fs.readFileSync(path.join(__dirname,'../../server-preview/discovery.js'),'utf8');
const ID='aBcD_efg-12',TT='tt_7668090902816017671';

class Element {
  constructor(tag='div') {
    this.tag=tag;this.children=[];this.events={};this.dataset={};
    this.hidden=false;this.value='';this.textContent='';this.className='';
    this.attributes={};this.parent=null;
  }
  append(...nodes){for(const n of nodes){if(n===null||n===undefined)continue;this.children.push(n);n.parent=this;}}
  prepend(...nodes){for(const n of nodes.reverse()){this.children.unshift(n);n.parent=this;}}
  replaceChildren(...nodes){this.children=[];this.append(...nodes);}
  addEventListener(event,fn){(this.events[event]??=[]).push(fn);}
  setAttribute(name,value){this.attributes[name]=String(value);}
  removeAttribute(name){delete this.attributes[name];}
  querySelector(selector){
    const stack=[...this.children];
    while(stack.length){
      const e=stack.shift();
      if(selector==='video'&&e.tag==='video')return e;
      if(selector.startsWith('.')&&e.className.split(/\s+/).includes(selector.slice(1)))return e;
      stack.unshift(...e.children);
    }
    return null;
  }
  querySelectorAll(){return [];}
  click(){for(const fn of this.events.click||[])fn({preventDefault(){}});}
  pause(){this.paused=true;}
  load(){}
  play(){return Promise.resolve();}
}
const all=node=>[node,...node.children.flatMap(all)];
const textIn=node=>all(node).map(e=>e.textContent).filter(Boolean);
const flush=async()=>{for(let i=0;i<12;i++)await new Promise(resolve=>setImmediate(resolve));};

test('real provider captures are rendered as native Muninn cards, while site controls stay opt-in',async()=>{
  const refs={};for(const id of ['browser-host','pwa-discover-menu','app','pwa-nav-backdrop',
    'pwa-menu-toggle','pwa-advanced-toggle','pwa-discover-search','pwa-discover-query'])refs['#'+id]=new Element();
  refs['#browser-host'].hidden=false;
  refs['#pwa-discover-menu'].hidden=true;
  const sources=['youtube-home','youtube-subscriptions','tiktok-foryou','tiktok-following']
    .map(key=>Object.assign(new Element('button'),{dataset:{browseSource:key}}));
  refs['#pwa-discover-menu'].querySelectorAll=selector=>selector==='[data-browse-source]'?sources:[];
  const browseButton=new Element('button');
  const events={},requests=[],dispatched=[],navigator={onLine:true,clipboard:{writeText:async()=>{}}};
  const now=Date.now();
  const snapshots=[
    {id:1,kind:'home',captured_at:now,source_url:'https://www.youtube.com/'},
    {id:2,kind:'tiktok-for-you',captured_at:now,source_url:'https://www.tiktok.com/foryou'},
  ];
  const feeds={
    1:{id:1,kind:'home',captured_at:now,items:[{video_id:ID,title:'Actual captured YouTube title',channel:'Engineering channel',duration_seconds:105,view_text:'19K views'}]},
    2:{id:2,kind:'tiktok-for-you',captured_at:now,items:[{video_id:TT,title:'Actual captured TikTok title',channel:'TikTok channel',duration_seconds:27}]}
  };
  const document={
    hidden:false,
    querySelector:sel=>sel==='meta[name="muninn-source-browser"]'?{content:'enabled'}:
      sel==='[data-view="browse"]'?browseButton:refs[sel]||null,
    createElement:tag=>new Element(tag),
    addEventListener:(name,fn)=>{(events[name]??=[]).push(fn);},
    dispatchEvent:event=>{dispatched.push(event.type);for(const fn of events[event.type]||[])fn(event);}
  };
  const fetch=async(url,options={})=>{
    requests.push({url,options});
    let data;
    if(url.startsWith('/api/feeds?kind='))data=snapshots;
    else if(url==='/api/feeds/1')data=feeds[1];
    else if(url==='/api/feeds/2')data=feeds[2];
    else if(url==='/api/entries')data=[];
    else if(url==='/browser/state')data={running:true,tabs:[{id:'a',url:'https://www.youtube.com/'}]};
    else if(url==='/browser/action')data={running:true,tabs:[]};
    else if(url==='/api/cache')data={ok:true,entry:{id:'queued'}};
    else throw new Error('Unexpected remote call '+url);
    return {ok:true,status:200,json:async()=>data};
  };
  const context={document,navigator,window:{MuninnWebModel:model},fetch,
    setInterval:()=>0,CustomEvent:class CustomEvent{constructor(type,options){this.type=type;this.detail=options?.detail;}},
    Date,Promise,console};
  vm.runInNewContext(source,context);
  await flush();
  assert.equal(refs['#pwa-discover-menu'].hidden,false);
  assert.ok(textIn(refs['#browser-host']).includes('Actual captured YouTube title'));
  assert.ok(all(refs['#browser-host']).some(e=>e.tag==='img'&&e.src==='/thumb/'+ID));
  assert.equal(requests.filter(x=>x.url.startsWith('/browser/frame')).length,0,'native view never requests remote screenshots');
  assert.equal(requests.filter(x=>x.url==='/browser/action').length,0,'already captured content does not start unnecessary Chromium navigation');
  assert.ok(!textIn(refs['#browser-host']).some(x=>x.includes('Start the private browser')));

  refs['#pwa-menu-toggle'].click();
  assert.equal(refs['#app'].dataset.menu,'open');
  sources[2].click();
  await flush();
  assert.equal(refs['#app'].dataset.menu,'closed');
  assert.ok(textIn(refs['#browser-host']).includes('Actual captured TikTok title'));
  assert.ok(requests.some(x=>x.url==='/browser/action'&&
    JSON.parse(x.options.body).url==='https://www.tiktok.com/foryou'),
    'source navigation asks Chromium for a new capture rather than displaying its pixels');
  assert.ok(requests.every(x=>!x.url.startsWith('/browser/frame')));
  const card=all(refs['#browser-host']).find(e=>e.className==='muninn-discover-card');
  assert.ok(card,'captured feed item should be an interactive Muninn card');
  const open=all(card).find(e=>e.className==='muninn-discover-art-button');
  open.click();await flush();
  assert.ok(textIn(refs['#browser-host']).includes('Actual captured TikTok title'));
  const save=all(refs['#browser-host']).find(e=>e.tag==='button'&&e.textContent==='Save video to Muninn');
  save.click();await flush();
  assert.ok(requests.some(x=>x.url==='/api/cache'&&
    JSON.parse(x.options.body).video_id===TT&&JSON.parse(x.options.body).protect===true));
  assert.ok(dispatched.includes('muninn:video-queued'));

  refs['#pwa-advanced-toggle'].click();
  assert.ok(dispatched.includes('muninn:advanced-browser'),'explicitly opt in to screenshot/sign-in tool');
  assert.ok(requests.every(x=>!x.url.startsWith('/browser/frame')));
});

test('PWA mobile keeps provider navigation in a drawer, not a remote screenshot toolbar',()=>{
  const html=fs.readFileSync(path.join(__dirname,'../pwa/index.html'),'utf8');
  const css=fs.readFileSync(path.join(__dirname,'../pwa/style.css'),'utf8');
  const gateway=fs.readFileSync(path.join(__dirname,'../../server-preview/gateway.py'),'utf8');
  assert.match(html,/id="pwa-menu-toggle"/);
  assert.match(html,/id="pwa-nav-backdrop"/);
  assert.match(html,/data-browse-source="youtube-home"/);
  assert.match(html,/data-browse-source="tiktok-foryou"/);
  assert.match(css,/\.muninn-discover-grid/);
  assert.match(css,/#app\[data-client="pwa"\]\[data-menu="open"\] \.sidebar/);
  assert.match(css,/#app\[data-client="pwa"\] \.tabrow \{ display:none;/);
  assert.match(gateway,/app\/discovery.js/);
  assert.match(gateway,/muninn-pwa-shell-browser-v5/);
  assert.doesNotMatch(source,/innerHTML|eval\(|document\.cookie|localStorage/);
});
