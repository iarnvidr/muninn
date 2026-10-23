// Plays an HLS URL in a <video> with hls.js (when native HLS is missing) and reports timing.
const url = location.origin + __P__;
let v = document.getElementById("muninn-test-video");
if (v) v.remove();
v = document.createElement("video"); v.id = "muninn-test-video";
v.style.cssText = "position:fixed;left:100px;top:100px;width:800px;height:450px;z-index:99999;background:#000"; v.controls = true; document.body.appendChild(v);
v.muted = true;
const events = []; const t0 = performance.now();
for (const n of ["loadedmetadata", "canplay", "playing", "error", "waiting", "seeked", "ended"]) v.addEventListener(n, () => events.push(n + ":" + v.currentTime.toFixed(1) + "@" + Math.round(performance.now() - t0)));
let engine = "native";
if (!v.canPlayType("application/vnd.apple.mpegurl")) {
  if (!window.Hls) { (0, eval)(__HLS__); }
  engine = "hls.js";
  const hls = new Hls({ maxBufferLength: 30, backBufferLength: 30 });
  window.__hls = hls;
  hls.on(Hls.Events.ERROR, (e, d) => events.push("hlserr:" + d.details + (d.fatal ? ":fatal" : "")));
  hls.loadSource(url); hls.attachMedia(v);
} else { v.src = url; }
try { await v.play(); } catch (e) { events.push("playerr:" + e); }
await new Promise(r => setTimeout(r, __WAIT__));
const buffered = v.buffered.length ? [v.buffered.start(0), v.buffered.end(v.buffered.length - 1)] : null;
const seekable = v.seekable.length ? [v.seekable.start(0), v.seekable.end(v.seekable.length - 1)] : null;
return { engine, ct: v.currentTime, rs: v.readyState, err: v.error && [v.error.code, v.error.message], dur: v.duration, w: v.videoWidth, h: v.videoHeight, buffered, seekable, events };
