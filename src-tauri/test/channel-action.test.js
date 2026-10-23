const {test}=require('node:test');
const assert=require('node:assert/strict');
const {webcrypto}=require('node:crypto');
const {unsubscribe}=require('../channel-action.js');
const expectedAccount='a'.repeat(64);
const request=(w,id)=>unsubscribe(w,id,expectedAccount);
function page(data={actions:[{updateSubscribeButtonAction:{channelId:'UCabcdefghijklmnopqrstuv',subscribed:false}}]}) {
  const requests=[];
  const config={LOGGED_IN:true,INNERTUBE_CONTEXT:{client:{clientVersion:'fixture'}},SESSION_INDEX:2};
  const w={location:{origin:'https://www.youtube.com'},ytcfg:{get:key=>config[key]},document:{cookie:'SAPISID=fixture-secret'},crypto:webcrypto,__muninnAccountKey:async()=>expectedAccount,
    fetch:async(url,options)=>{requests.push({url,options});return {ok:true,json:async()=>data};}};
  return {w,requests,config};
}
test('unsubscribe uses only the current YouTube session and requires a positive acknowledgement',async()=>{
  const {w,requests}=page();assert.deepEqual(await request(w,'UCabcdefghijklmnopqrstuv'),{ok:true});
  assert.equal(requests.length,1);assert.equal(requests[0].url,'/youtubei/v1/subscription/unsubscribe?prettyPrint=false');
  const options=requests[0].options;assert.equal(options.credentials,'include');
  assert.deepEqual(JSON.parse(options.body).channelIds,['UCabcdefghijklmnopqrstuv']);assert.equal(options.headers['X-Goog-AuthUser'],'2');
  assert.match(options.headers.Authorization,/^SAPISIDHASH \d+_[a-f0-9]{40}$/);assert.ok(!options.body.includes('fixture-secret'));
});
test('signed out, invalid channel and wrong origin never perform an account action',async()=>{
  for(const change of [p=>p.config.LOGGED_IN=false,p=>p.w.location.origin='https://example.com']){
    const p=page();change(p);await assert.rejects(request(p.w,'UCabcdefghijklmnopqrstuv'));assert.equal(p.requests.length,0);
  }
  const p=page();await assert.rejects(request(p.w,'bad'));assert.equal(p.requests.length,0);
});
test('HTTP success without acknowledgement, logout and rejection remain errors',async()=>{
  for(const data of [{},{error:{code:403}},{responseContext:{mainAppWebResponseContext:{loggedOut:true}}},{actions:[{updateSubscribeButtonAction:{channelId:'UCzyxwvutsrqponmlkjihgfe',subscribed:false}}]}]){
    await assert.rejects(request(page(data).w,'UCabcdefghijklmnopqrstuv'));
  }
  const {w}=page();w.fetch=async()=>({ok:false,status:403});await assert.rejects(request(w,'UCabcdefghijklmnopqrstuv'),/403/);
});
test('account switch before or during request preparation never sends unsubscribe',async()=>{
  const first=page();first.w.__muninnAccountKey=async()=> 'b'.repeat(64);
  await assert.rejects(request(first.w,'UCabcdefghijklmnopqrstuv'),/account changed/);assert.equal(first.requests.length,0);
  const second=page();let calls=0;second.w.__muninnAccountKey=async()=>++calls===1?expectedAccount:'b'.repeat(64);
  await assert.rejects(request(second.w,'UCabcdefghijklmnopqrstuv'),/account changed/);assert.equal(second.requests.length,0);
});
