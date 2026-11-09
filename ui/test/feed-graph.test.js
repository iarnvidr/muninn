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

test('graph edges express only actual stored source, snapshot, video and channel relations',()=>{
 const g=G.build(captures);
 assert.equal(g.snapshots,2);
 assert.equal(g.nodes.filter(n=>n.kind==='video').length,2);
 assert.equal(g.nodes.filter(n=>n.kind==='snapshot').length,2);
 assert.equal(g.nodes.filter(n=>n.kind==='channel').length,1);
 assert.ok(g.edges.some(e=>e.source==='source:home'&&e.target==='snapshot:11'&&e.relation==='capture'));
 assert.ok(g.edges.some(e=>e.source==='snapshot:11'&&e.target==='video:'+A));
 assert.ok(g.edges.some(e=>e.source==='snapshot:12'&&e.target==='video:'+A));
 assert.ok(g.edges.some(e=>e.source==='channel:'+CHANNEL&&e.target==='video:'+B&&e.relation==='published'));
 assert.equal(g.edges.filter(e=>e.source==='snapshot:12').length,1);
 assert.equal(G.positions(g,1000,640).size,g.nodes.length);
});

test('graph caps large histories and refuses malformed links',()=>{
 const videos=Array.from({length:90},(_,i)=>({video_id:String(i).padStart(11,'0'),title:'Video '+i}));
 const rows=Array.from({length:40},(_,id)=>({id:id+1,kind:'home',source_url:null,items:videos}));
 const g=G.build(rows);
 assert.equal(g.snapshots,20);
 assert.equal(g.nodes.filter(n=>n.kind==='video').length,64);
 assert.equal(g.truncated,true);
 for(const e of g.edges)assert.ok(g.nodes.some(n=>n.id===e.source)&&g.nodes.some(n=>n.id===e.target));
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
