'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const P=require('../pwa/app.js');
const SW=require('../pwa/sw.js');
const origin='https://muninn.example';
const id='dQw4w9WgXcQ';

test('PWA canonicalizes supported video links without retaining query credentials or tracking',()=>{
  for(const input of [id,`https://www.youtube.com/watch?v=${id}&t=30&token=secret`,`https://youtu.be/${id}?si=tracking`,`https://m.youtube.com/shorts/${id}`,`https://music.youtube.com/watch?v=${id}`,`https://youtube.com/live/${id}`]){
    assert.equal(P.videoId(input),id);
    assert.equal(P.videoUrl(input),`https://www.youtube.com/watch?v=${id}`);
  }
  for(const input of [`https://youtube.com.evil.example/watch?v=${id}`,`https://youtube.com@evil.example/watch?v=${id}`,`https://user:secret@youtube.com/watch?v=${id}`,`https://youtube.com:9443/watch?v=${id}`,`javascript:alert('${id}')`,`file:///watch?v=${id}`,`https://example.com/${id}`,`https://youtube.com/watch?v=too-short`])assert.equal(P.videoId(input),null,input);
});

test('PWA source URLs describe supported YouTube origins without exporting arbitrary links',()=>{
  assert.equal(P.sourceUrl('https://www.youtube.com/@Raven/videos?session=private'),'https://www.youtube.com/@Raven/videos');
  assert.equal(P.sourceUrl('https://www.youtube.com/results?search_query=raven%20calls&token=private'),'https://www.youtube.com/results?search_query=raven%20calls');
  assert.equal(P.sourceUrl('https://www.youtube.com/feed/subscriptions?token=private'),'https://www.youtube.com/feed/subscriptions');
  for(const input of ['https://evil.example/library','javascript:alert(1)','https://youtube.com/redirect?q=evil','https://user:secret@youtube.com/','/api/library'])assert.equal(P.sourceUrl(input),null,input);
});

test('PWA playback only accepts same-origin media and rewritten HLS paths',()=>{
  for(const input of ['/media/video_720','/media/video-720.mp4','/stream/video.local.4/index.m3u8'])assert.equal(P.mediaPath(input),input);
  for(const input of [`${origin}/media/video_720`,'//evil.example/media/video','https://evil.example/movie.mp4','/api/library','/h/master-token/job/index.m3u8','/stream/job/../secret','/stream/job/index.m3u8?token=master','/media/%2e%2e','/media/id?session=secret','javascript:alert(1)','blob:https://example.com/id'])assert.equal(P.mediaPath(input),null,input);
});

test('feed names keep the relationship to the source video explicit, including older incomplete captures',()=>{
  assert.equal(P.feedLabel({kind:'related',source_title:'Building a nest',source_url:`https://youtube.com/watch?v=${id}` }),'Related to Building a nest');
  assert.equal(P.feedLabel({kind:'related',source_url:`https://youtu.be/${id}`}),`Related to video ${id}`);
  assert.equal(P.feedLabel({kind:'related'}),'Related to an unrecorded video');
  assert.equal(P.feedLabel({kind:'search',query:'raven calls'}),'Search: raven calls');
  assert.equal(P.feedLabel({kind:'home'}),'YouTube Home');
  assert.equal(P.feedLabel({kind:'channel',source_url:'https://youtube.com/@Raven/videos'}),'@Raven');
  assert.equal(P.feedLabel({kind:'channel',source_url:'https://youtube.com/@bad%ZZ'}),'@bad%ZZ');
  assert.equal(P.feedLabel({kind:'channel'}),'Channel · source name not recorded');
});

test('library prefers a playable copy over an unfinished protected duplicate',()=>{
  const rows=[
    {id:'pending',video_id:id,title:'Raven calls',protected:true,status:'downloading'},
    {id:'ready',video_id:id,title:'Raven calls',status:'complete'},
    {id:'keep',video_id:'aaaaaaaaaaa',title:'Forest walk',channel:'Alice',protected:true,status:'complete'},
    {id:'queue',video_id:'bbbbbbbbbbb',title:'A nest',status:'queued'},
    {id:'invalid',video_id:'bad',status:'complete'}
  ];
  assert.deepEqual(P.groupLibrary(rows).map(r=>r.id),['ready','keep','queue']);
  assert.deepEqual(P.filterLibrary(rows,'ready','').map(r=>r.id),['ready','keep']);
  assert.deepEqual(P.filterLibrary(rows,'kept','ALICE').map(r=>r.id),['keep']);
  assert.deepEqual(P.filterLibrary(rows,'downloading','nest').map(r=>r.id),['queue']);
  assert.deepEqual(P.filterLibrary(rows,'ready','missing'),[]);
});

test('failed download retries preserve requested quality and kept status',()=>{
  for(const quality of ['audio','360','480','720','1080']){
    const body=P.cacheRequest({video_id:id,requested_quality:quality,variant:'720',status:'failed',protected:true,title:'Raven'});
    assert.equal(body.quality,quality);assert.equal(body.protect,true);assert.equal(body.video_id,id);
  }
  assert.equal(P.cacheRequest({video_id:id,variant:'audio'}).quality,'audio');
  assert.equal(P.cacheRequest({video_id:id}).quality,'720');
  assert.equal(P.cacheRequest({video_id:id}).protect,false);
});

test('feed cards recognize the server’s nested cached copy without offering a duplicate download',()=>{
  assert.equal(P.cacheLabel({cached:{id:'a.720',state:'protected',progress:1}}),'Kept in library');
  assert.equal(P.cacheLabel({cached:{id:'a.720',state:'complete',progress:1}}),'Ready to play');
  assert.equal(P.cacheLabel({cached:{id:'a.720',state:'speculative',progress:1}}),'Ready to play');
  assert.equal(P.cacheLabel({cached:{id:'a.720',state:'partial',progress:.5}}),'Preparing on server');
  assert.equal(P.cacheLabel({cached:{id:'a.720'}}),'Copy on server');
  assert.equal(P.cacheLabel({cached:null}),'');
});

test('API requests use same-origin cookies and bypass the browser cache',()=>{
  assert.deepEqual(P.requestOptions(),{method:'GET',credentials:'same-origin',cache:'no-store',headers:{Accept:'application/json'}});
  const post=P.requestOptions('POST',{video_id:id});
  assert.equal(post.credentials,'same-origin');assert.equal(post.cache,'no-store');
  assert.equal(post.headers['Content-Type'],'application/json');
  assert.deepEqual(JSON.parse(post.body),{video_id:id});
  assert.equal(Object.hasOwn(post.headers,'Authorization'),false);
  assert.equal(P.requestOptions('DELETE').body,undefined);
});

test('pairing exchanges the token only in the request header, then drops it',async()=>{
  const token='pairing-token-do-not-persist';let sent;
  const data=await P.startSession(token,async(url,options)=>{
    sent=options;
    assert.equal(url,'/app/session');assert.equal(options.method,'POST');
    assert.equal(options.headers.Authorization,'Bearer '+token);
    assert.equal(options.body,undefined);assert.equal(options.credentials,'same-origin');assert.equal(options.cache,'no-store');
    return {ok:true,json:async()=>({authenticated:true,version:'0.2.28'})};
  });
  assert.equal(data.authenticated,true);
  assert.equal(Object.hasOwn(sent.headers,'Authorization'),false);
  assert.equal(JSON.stringify(sent).includes(token),false);
});

test('failed pairing removes transient headers on rejection, network failure, and invalid JSON',async()=>{
  for(const response of [async()=>({ok:false,json:async()=>({error:'Invalid pairing token'})}),async()=>{throw new Error('offline');},async()=>({ok:true,json:async()=>{throw new Error('invalid json');}})]){
    let sent;await assert.rejects(P.startSession('secret',async(url,options)=>{sent=options;return response();}));
    assert.equal(Object.hasOwn(sent.headers,'Authorization'),false);
  }
  for(const value of ['', '  ', 'abc\r\nAuthorization: evil', 'x'.repeat(1025)])assert.throws(()=>P.pairingOptions(value));
});

test('local HLS windows record the original timeline and never report a window boundary as video completion',()=>{
  const playback={local:true,offset_seconds:300,duration_seconds:1800};
  assert.deepEqual(P.playbackTimeline(playback,120,120),{position:420,total:1800,atEnd:false,continueWindow:true});
  assert.deepEqual(P.playbackTimeline({...playback,offset_seconds:1680},120,120),{position:1800,total:1800,atEnd:true,continueWindow:false});
  assert.deepEqual(P.playbackTimeline({local:true,offset_seconds:300},120,120),{position:420,total:null,atEnd:false,continueWindow:false});
  assert.deepEqual(P.playbackTimeline({kind:'file'},60,180),{position:60,total:180,atEnd:false,continueWindow:false});
  assert.deepEqual(P.playbackTimeline({kind:'file'},180,180),{position:180,total:180,atEnd:true,continueWindow:false});
  assert.deepEqual(P.playbackTimeline({},0,Infinity),{position:0,total:null,atEnd:false,continueWindow:false});
});

test('fast cached watch waits for stored resume metadata before returning a playable snapshot',async()=>{
  let resolveDetail,finished=false;
  const detail=new Promise(resolve=>{resolveDetail=resolve;});
  const ready={playback:{ready:true,kind:'file',url_path:'/media/example'}};
  const pending=P.loadWatchSnapshot(id,async url=>url.startsWith('/api/watch/')?ready:detail).then(value=>{finished=true;return value;});
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(finished,false,'playable source must wait for the resume record');
  resolveDetail({position_seconds:42,meta:{title:'Resume me',channel:'Raven'}});
  const result=await pending;
  assert.equal(result.status,ready);
  assert.equal(result.detail.position_seconds,42);
  assert.equal(result.detail.meta.title,'Resume me');
});

test('missing optional watch metadata does not prevent playback, but failed watch preparation does',async()=>{
  const ready={playback:{ready:true}};
  const result=await P.loadWatchSnapshot(id,async url=>{if(url.startsWith('/api/video/'))throw new Error('metadata missing');return ready;});
  assert.equal(result.status,ready);assert.equal(result.detail,null);
  await assert.rejects(P.loadWatchSnapshot(id,async url=>{if(url.startsWith('/api/watch/'))throw new Error('preparation failed');return {position_seconds:12};}),/preparation failed/);
});

test('service worker allowlist excludes every private surface, credential query, method and foreign origin',()=>{
  for(const pathname of SW.SHELL_PATHS)assert.equal(SW.cacheableRequest(new Request(origin+pathname),origin),true,pathname);
  for(const pathname of ['/app/session','/api/library','/api/feeds?limit=100','/media/video','/thumb/video','/stream/job/index.m3u8','/app/hls.min.js','/app/?token=secret','/app/index.html?pairing=secret','/app/unknown.js'])assert.equal(SW.cacheableRequest(new Request(origin+pathname),origin),false,pathname);
  assert.equal(SW.cacheableRequest(new Request('https://foreign.example/app/app.js'),origin),false);
  assert.equal(SW.cacheableRequest(new Request(origin+'/app/',{method:'POST'}),origin),false);
  assert.equal(SW.cacheableRequest(new Request(origin+'/app/',{headers:{authorization:'Bearer secret'}}),origin),false);
});

function workerHarness(network){
  const listeners={},added=[],written=[],removed=[],saved=new Map();let claimed=false;
  class BrowserRequest extends Request {constructor(input,options){super(typeof input==='string'?new URL(input,origin):input,options);}}
  const cache={addAll:async requests=>{added.push(...requests);},put:async(request,response)=>{written.push(request.url);saved.set(request.url,response);},match:async request=>saved.get(request.url)};
  const context={URL,Request:BrowserRequest,fetch:network,caches:{open:async()=>cache,keys:async()=>['unrelated-app-cache','muninn-pwa-shell-old',SW.SHELL_CACHE],delete:async key=>{removed.push(key);return true;}},self:{location:{origin},clients:{claim:async()=>{claimed=true;}},addEventListener:(type,fn)=>{listeners[type]=fn;}}};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname,'../pwa/sw.js'),'utf8'),context);
  return {listeners,added,written,removed,saved,get claimed(){return claimed;}};
}

test('service worker installation fetches public shell without credentials and leaves unrelated caches alone',async()=>{
  const worker=workerHarness(()=>assert.fail('unexpected network'));
  let work;worker.listeners.install({waitUntil:p=>{work=p;}});await work;
  assert.equal(worker.added.length,SW.SHELL_PATHS.length);
  assert.equal(worker.added.every(request=>request.credentials==='omit'&&request.cache==='reload'),true);
  worker.listeners.activate({waitUntil:p=>{work=p;}});await work;
  assert.deepEqual(worker.removed,['muninn-pwa-shell-old']);assert.equal(worker.claimed,true);
});

test('service worker serves the public shell offline but never intercepts authenticated content',async()=>{
  let offline=false;const requests=[];
  const response={ok:true,type:'basic',clone(){return this;}};
  const worker=workerHarness(async request=>{requests.push(request);if(offline)throw new Error('offline');return response;});
  const shell=new Request(origin+'/app/app.js',{credentials:'include'});
  let work;worker.listeners.fetch({request:shell,respondWith:p=>{work=p;}});assert.equal(await work,response);
  assert.equal(requests[0].credentials,'omit');assert.equal(requests[0].cache,'no-cache');
  assert.deepEqual(worker.written,[origin+'/app/app.js']);
  offline=true;worker.listeners.fetch({request:shell,respondWith:p=>{work=p;}});assert.equal(await work,response);
  for(const pathname of ['/app/session','/api/library','/media/video','/thumb/video','/stream/job/index.m3u8'])worker.listeners.fetch({request:new Request(origin+pathname),respondWith:()=>assert.fail('private request intercepted: '+pathname)});
  assert.equal(requests.length,2);
});

test('install manifest stays inside /app and declares real icon dimensions',()=>{
  const manifest=JSON.parse(fs.readFileSync(path.join(__dirname,'../pwa/manifest.webmanifest'),'utf8'));
  assert.equal(manifest.id,'/app/');assert.equal(manifest.start_url,'/app/');assert.equal(manifest.scope,'/app/');assert.equal(manifest.display,'standalone');
  for(const icon of manifest.icons){
    const data=fs.readFileSync(path.join(__dirname,'../pwa',path.basename(icon.src)));
    assert.equal(data.subarray(1,4).toString(),'PNG');
    assert.equal(icon.sizes,`${data.readUInt32BE(16)}x${data.readUInt32BE(20)}`);
  }
});
