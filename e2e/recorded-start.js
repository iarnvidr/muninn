// Run through playwright-cli run-code --filename, against recorded-start-server.py.
async (page) => {
  await page.addInitScript(() => {
    const original = HTMLMediaElement.prototype.canPlayType;
    HTMLMediaElement.prototype.canPlayType = function(type) { return /mpegurl/i.test(type) ? '' : original.call(this,type); };
  });
  const results=[];
  for(const scenario of [{position:0,finished:false,start:0},{position:20,finished:false,start:20},{position:55,finished:true,start:0}]){
    await page.request.post('http://127.0.0.1:47942/fixture/resume',{data:scenario});
    await page.goto('http://127.0.0.1:47942/ui/index.html?mock=1&hlsjs=1&hlsdir=aaaaaaaaaaa.360&active=5&dur=60&v='+Date.now()+'#token=fixture');
    await page.locator('#sidebar [data-page="player"]').click();
    await page.waitForFunction(start=>{const v=document.querySelector('#view-watch video');return v?.readyState>=2&&!v.seeking&&v.currentTime>=start&&v.currentTime<start+5;},scenario.start);
    const result=await page.evaluate(async()=>{
      const v=document.querySelector('#view-watch video');await v.play();await new Promise(resolve=>setTimeout(resolve,1200));
      return {time:v.currentTime,readyState:v.readyState,engine:document.querySelector('[data-engine]')?.dataset.engine};
    });
    if(result.engine!=='hls.js'||result.time<scenario.start||result.time>=scenario.start+5)throw Error(JSON.stringify({scenario,result}));
    results.push({scenario,...result});
  }
  return {result:'passed',results};
}
