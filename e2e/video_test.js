const p = __P__;
let v = document.getElementById("muninn-test-video");
if (v) v.remove();
v=document.createElement("video"); v.id="muninn-test-video"; v.style.cssText="position:fixed;left:100px;top:100px;width:800px;height:450px;z-index:99999;background:#000"; v.controls=true; document.body.appendChild(v);
v.muted=true; v.src=p;
const events=[]; for(const n of ["loadedmetadata","canplay","playing","error","stalled","waiting","seeked"]) v.addEventListener(n,()=>events.push(n+":"+v.currentTime.toFixed(1)));
const t0=performance.now();
try{ await v.play(); }catch(e){ events.push("playerr:"+e) }
await new Promise(r=>setTimeout(r,__WAIT__));
return {ct:v.currentTime, rs:v.readyState, ns:v.networkState, err: v.error && [v.error.code, v.error.message], dur:v.duration, w:v.videoWidth, h:v.videoHeight, events, firstframe_ms: window.__ff||null};
