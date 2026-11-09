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

// The Chat Hub layout is shared rather than a fabricated four-column diagram.
test('network contains one capture hub and bounded actual channel nodes by default',()=>{
 const items=Array.from({length:102},(_,i)=>({
   video_id:String(i).padStart(11,'0'),title:'Video '+i,
   channel_id:'UC'+String(i%12).padStart(22,'x'),channel:'Creator '+(i%12)
 }));
 const g=G.build([{id:21,kind:'subscriptions',items,count:102}]);
 const network=G.prepareNetwork(g,{snapshotId:'snapshot:21'});
 assert.equal(network.totalVideos,102);
 assert.equal(network.totalChannels,12);
 assert.equal(network.visibleChannels,12);
 assert.equal(network.nodes.filter(n=>n.kind==='video').length,0,
   'individual video labels must not flood the overview');
 assert.equal(network.nodes.filter(n=>n.kind==='snapshot').length,1,
   'source/snapshot duplicate nodes obscure the real relationships');
 assert.equal(network.nodes.filter(n=>n.kind==='channel').length,12);
 assert.equal(network.edges.filter(e=>e.basis==='captured_in').length,12);
 assert.equal(network.edges.reduce((sum,e)=>sum+e.count,0),102);
 assert.ok(network.nodes.some(n=>n.landmark));
 assert.ok(network.nodes.every(n=>Number.isFinite(n.degree)));
});

test('one selected channel expands only its observed videos, with bounded satellites',()=>{
 const id='UC'+String(0).padStart(22,'x');
 const items=Array.from({length:102},(_,i)=>({
   video_id:String(i).padStart(11,'0'),title:'Video '+i,
   channel_id:'UC'+String(i%3).padStart(22,'x'),channel:'Creator '+i%3
 }));
 const g=G.build([{id:21,kind:'subscriptions',items,count:102}]);
 const focused=G.prepareNetwork(g,{snapshotId:'snapshot:21',focusId:'channel:'+id});
 assert.equal(focused.expandedVideos,24);
 assert.equal(focused.nodes.filter(n=>n.kind==='video').length,24);
 assert.ok(focused.edges.filter(e=>e.basis==='published').length>=24);
 for(const e of focused.edges) {
   assert.ok(focused.nodes.some(n=>n.id===e.source));
   assert.ok(focused.nodes.some(n=>n.id===e.target));
 }
});

test('search highlights matching channels and does not invent shared identity',()=>{
 const a={id:1,kind:'subscriptions',items:[{video_id:A,title:'Creative review',channel:'Studio'},
  {video_id:B,title:'Computer tips',channel:'Workroom'}]};
 const b={id:2,kind:'home',items:[{video_id:'ccccccccccc',title:'Other',channel:'Studio'}]};
 const graph=G.build([a,b]);
 const n=G.prepareNetwork(graph,{scope:'all',query:'Creative'});
 assert.equal(n.totalChannels,1);
 assert.equal(n.nodes.filter(x=>x.kind==='channel').length,1);
 assert.equal(n.edges.filter(e=>e.basis==='captured_in').length,1);
 assert.equal(G.prepareNetwork(graph,{scope:'all'}).totalChannels,3,
   'same display name without channel ID must stay capture-scoped');
});

test('search and more-nodes controls cap graph complexity without dropping stored data',()=>{
 const items=Array.from({length:102},(_,i)=>({
   video_id:String(i).padStart(11,'0'),title:'Unique video '+i,
   channel_id:'UC'+String(i).padStart(22,'x'),channel:'Channel '+i
 }));
 const g=G.build([{id:9,kind:'subscriptions',items}]);
 const n=G.prepareNetwork(g,{snapshotId:'snapshot:9'});
 assert.equal(n.totalVideos,102);
 assert.equal(n.totalChannels,102);
 assert.equal(n.visibleChannels,42);
 assert.equal(n.hiddenChannels,60);
 const larger=G.prepareNetwork(g,{snapshotId:'snapshot:9',maxChannels:70});
 assert.equal(larger.visibleChannels,70);
 assert.equal(larger.hiddenChannels,32);
 assert.equal(G.prepareNetwork(g,{snapshotId:'snapshot:9',query:'Channel 99'}).visibleChannels,1);
});

test('actual Chat Hub Cytoscape engine spreads 60 nodes with finite coordinates',async()=>{
 const {default:cytoscape}=await import('../vendor/cytoscape.esm.min.mjs');
 const elements=[];
 for(let i=0;i<60;i++)elements.push({data:{id:'node'+i}});
 for(let i=1;i<60;i++)elements.push({data:{id:'edge'+i,source:'node'+i,target:'node'+Math.floor((i-1)/2)}});
 const cy=cytoscape({headless:true,elements});
 try{
   let stopped=false;
   const layout=cy.layout({name:'cose',animate:false,randomize:false,padding:45,
      nodeRepulsion:()=>11000,idealEdgeLength:()=>110,gravity:.7,numIter:250});
   layout.one('layoutstop',()=>{stopped=true;});layout.run();
   const points=cy.nodes().map(n=>n.position());
   assert.equal(stopped,true);
   assert.equal(points.length,60);
   assert.ok(points.every(p=>Number.isFinite(p.x)&&Number.isFinite(p.y)));
   assert.ok(Math.max(...points.map(p=>p.x))-Math.min(...points.map(p=>p.x))>100);
   assert.ok(Math.max(...points.map(p=>p.y))-Math.min(...points.map(p=>p.y))>100);
 } finally {cy.destroy();}
});
