'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const G=require('../feed-graph.js');
const L=require('../lib.js');
const A='aaaaaaaaaaa',B='bbbbbbbbbbb',CHANNEL='UC'+'x'.repeat(22);
const captures=[
 {id:11,kind:'home',captured_at:123456,count:2,source_url:'https://www.youtube.com/',
  items:[{video_id:A,title:'First video',channel_id:CHANNEL,channel:'Studio'},
         {video_id:B,title:'Second video',channel_id:CHANNEL,channel:'Studio'}]},
 {id:12,kind:'related',captured_at:123457,count:1,source_url:null,
  items:[{video_id:A,title:'First video',channel_id:CHANNEL,channel:'Studio'}]}
];

test('all graph links are backed by a captured snapshot/video/channel relationship',()=>{
 const g=G.build(captures);
 assert.equal(g.snapshots,2);
 assert.equal(g.nodes.filter(n=>n.kind==='video').length,2);
 assert.equal(g.nodes.filter(n=>n.kind==='snapshot').length,2);
 assert.equal(g.nodes.filter(n=>n.kind==='channel').length,1);
 assert.ok(g.edges.some(e=>e.source==='source:home'&&e.target==='snapshot:11'&&e.relation==='capture'));
 assert.ok(g.edges.some(e=>e.source==='snapshot:11'&&e.target==='video:'+A));
 assert.ok(g.edges.some(e=>e.source==='snapshot:12'&&e.target==='video:'+A));
 assert.ok(g.edges.some(e=>e.source==='channel:'+CHANNEL&&e.target==='video:'+B&&e.relation==='published'));
 assert.equal(g.edges.filter(e=>e.source==='snapshot:12'&&e.relation==='contains').length,1);
 assert.equal(G.positions(g,1000,640).size,g.nodes.length);
});

test('captures with over one hundred videos are summarized, not clipped at 24 or drawn as a hairball',()=>{
 const chan=i=>'UC'+String(i).padStart(22,'x');
 const items=Array.from({length:102},(_,i)=>({
  video_id:String(i).padStart(11,'0'),title:'Video '+i,
  channel_id:chan(i%12),channel:'Creator '+(i%12)
 }));
 const g=G.build([{id:21,kind:'subscriptions',captured_at:123,count:102,items}]);
 assert.equal(g.nodes.filter(n=>n.kind==='video').length,102);
 assert.equal(g.nodes.filter(n=>n.kind==='channel').length,12);
 assert.equal(g.truncated,false);
 const first=G.overview(g,{snapshotId:'snapshot:21'});
 assert.equal(first.videoCount,102);
 assert.equal(first.totalGroups,12);
 assert.equal(first.visible.length,8);
 assert.equal(first.end,8);
 assert.equal(first.more,true);
 assert.equal(first.groups.reduce((sum,c)=>sum+c.count,0),102);
 const second=G.overview(g,{snapshotId:'snapshot:21',offset:8});
 assert.equal(second.visible.length,4);
 assert.equal(second.start,8);
 assert.equal(second.previous,true);
 assert.equal(second.more,false);
 assert.equal(new Set([...first.visible,...second.visible].map(c=>c.id)).size,12);
});

test('channel-first model links videos shared across captures without inventing other videos',()=>{
 const g=G.build(captures);
 const one=G.overview(g,{snapshotId:'snapshot:11'});
 assert.equal(one.videoCount,2);
 assert.equal(one.totalGroups,1);
 assert.equal(one.visible[0].count,2);
 assert.equal(one.visible[0].sharedCaptures,2);
 const two=G.overview(g,{snapshotId:'snapshot:12'});
 assert.equal(two.videoCount,1);
 assert.equal(two.visible[0].count,1);
 const all=G.overview(g,{scope:'all'});
 assert.equal(all.videoCount,2);
 assert.equal(all.visible[0].count,2);
 const indexed=G.relationships(g);
 assert.deepEqual([...indexed.videoCaptures.get('video:'+A)].sort(),['snapshot:11','snapshot:12']);
 assert.deepEqual([...indexed.videoCaptures.get('video:'+B)],['snapshot:11']);
 assert.equal(indexed.channelVideos.get('channel:'+CHANNEL).size,2);
});

test('filter matches channel names or video titles and pagination is bounded',()=>{
 const g=G.build(captures);
 assert.equal(G.overview(g,{query:'Studio'}).visible.length,1);
 assert.equal(G.overview(g,{query:'Second video'}).visible.length,1);
 assert.equal(G.overview(g,{query:'No such thing'}).visible.length,0);
 assert.equal(G.overview(g,{offset:999}).visible.length,1);
 assert.equal(G.overview(g,{limit:999}).visible.length,1);
});

test('name-only channels remain capture-scoped and never imply shared identity',()=>{
 const one={id:3,kind:'subscriptions',items:[{video_id:A,title:'A',channel:'Common name'}]};
 const two={id:4,kind:'home',items:[{video_id:B,title:'B',channel:'Common name'}]};
 const g=G.build([one,two]);
 const all=G.overview(g,{scope:'all'});
 assert.equal(all.totalGroups,2);
 assert.equal(all.visible[0].node.verified_id,false);
 assert.notEqual(all.visible[0].id,all.visible[1].id);
 assert.equal(all.visible[0].node.channel_id,null);
});

test('snapshots, videos, and channel inputs have finite safe bounds',()=>{
 const videos=Array.from({length:500},(_,i)=>({video_id:String(i).padStart(11,'0'),title:'Video '+i}));
 const rows=Array.from({length:40},(_,id)=>({id:id+1,kind:'home',items:videos}));
 const g=G.build(rows);
 assert.equal(g.snapshots,20);
 assert.equal(g.nodes.filter(n=>n.kind==='video').length,160);
 assert.equal(g.truncated,true);
 for(const e of g.edges)assert.ok(g.nodes.some(n=>n.id===e.source)&&g.nodes.some(n=>n.id===e.target));
 assert.equal(G.overview(g,{scope:'all'}).visible.length,8);
});

test('empty or invalid captures do not produce fake links',()=>{
 const g=G.build([{id:0,kind:'home',items:[{video_id:A}]},{id:1,kind:'home',items:[{video_id:'bad'}]}]);
 assert.equal(g.snapshots,1);
 assert.equal(G.overview(g).totalGroups,0);
 assert.equal(G.build(null).nodes.length,0);
});

test('feed list explains real provenance and missing origins without fabricating a URL',()=>{
 assert.deepEqual(L.feedProvenance({kind:'search',query:'rust',source_url:'https://www.youtube.com/results?search_query=rust'}),
  {label:'Search: rust',origin:'www.youtube.com/results · query rust',hasSource:true});
 const related=L.feedProvenance({kind:'related',source_url:null});
 assert.equal(related.label,'Related videos');
 assert.match(related.origin,/specific origin not recorded/);
 assert.equal(L.feedProvenance({kind:'home',source_url:'https://example.com/secret'}).hasSource,false);
 assert.equal(L.feedProvenance({kind:'home'}).origin,'Origin URL not recorded');
});

// A small DOM stand-in exercises the *real* renderer's default complexity and
// prevents reverting to the unbounded node-label display, even without Chromium.
test('mounted explorer draws at most eight legible channel cards plus a summary root',()=>{
 class FakeElement {
  constructor(name){this.name=name;this.children=[];this.attrs={};this.listeners={};this.style={};this.textContent='';}
  setAttribute(k,v){this.attrs[k]=String(v);}
  addEventListener(k,fn){this.listeners[k]=fn;}
  append(...nodes){this.children.push(...nodes);}
  replaceChildren(...nodes){this.children=nodes;}
  setPointerCapture(){}
 }
 const document={
  createElement:(name)=>new FakeElement(name),
  createElementNS:(_,name)=>new FakeElement(name)
 };
 const host=new FakeElement('host');host.ownerDocument=document;
 const items=Array.from({length:102},(_,i)=>({
  video_id:String(i).padStart(11,'0'),title:'Real test video '+i,
  channel_id:'UC'+String(i%12).padStart(22,'x'),channel:'Creator '+(i%12)
 }));
 const graph=G.build([{id:21,kind:'subscriptions',count:102,items}]);
 const explorer=G.mount(host);
 explorer.render(graph,'snapshot:21');
 const flat=(node)=>[node,...node.children.flatMap(flat)];
 let elements=flat(host);
 assert.equal(elements.filter(n=>n.name==='g'&&n.attrs.class?.includes('feed-explorer-node')).length,9);
 const buttons=elements.filter(n=>n.name==='button');
 assert.ok(buttons.some(n=>n.textContent==='Next'));
 assert.ok(elements.some(n=>n.name==='aside'));
 assert.ok(elements.some(n=>n.name==='text'&&n.textContent.includes('Creator')));
 // A programmatic refresh/selection of the same capture keeps the focused graph.
 explorer.select('snapshot:21');
 elements=flat(host);
 assert.equal(elements.filter(n=>n.name==='g'&&n.attrs.class?.includes('feed-explorer-node')).length,9);
 explorer.destroy();
 assert.equal(host.children.length,0);
});
