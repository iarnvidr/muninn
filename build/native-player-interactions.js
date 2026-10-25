/*
 * Native WKWebView fixture, called through the existing evalsync chrome target:
 *   await window.__muninnNativePlayerInteractions({mediaBase64:'...', ...})
 * Supply a tiny locally generated playable MP4 as base64. No YouTube or
 * network fixture is used. Fixture API responses and the media transport are replaced; the
 * production watch mount, keyboard listeners, queue selection and native
 * tab_open_video command remain in control.
 */
(function installNativePlayerFixture(root) {
  'use strict';
  root.__muninnNativePlayerInteractions = async function runNativePlayerInteractions(options) {
    options = options || {};
    const M = root.Muninn;
    if (!M || !M.api || !M.watch) throw new Error('Muninn app/watch is not mounted');
    const ids = ['Sm0keVid001', 'Sm0keVid002', 'Sm0keVid003'];
    if (!options.playlistId || !options.mediaBase64) throw new Error('requires playlistId and local mediaBase64');
    const videoId = options.videoId || ids[0];
    if (!/^[A-Za-z0-9_-]{11}$/.test(videoId)) throw new Error('videoId must be an 11-character video id');
    const mediaBytes = Uint8Array.from(atob(options.mediaBase64), ch => ch.charCodeAt(0));
    const mediaUrl = URL.createObjectURL(new Blob([mediaBytes], {type:'video/mp4'}));
    const sourceProperty = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'src');
    const fixtureMediaPrefix = M.origin + '/media/native-smoke-';
    Object.defineProperty(HTMLMediaElement.prototype, 'src', {
      ...sourceProperty,
      set(value) { sourceProperty.set.call(this, String(value).startsWith(fixtureMediaPrefix) ? mediaUrl : value); },
    });
    const originalApi = M.api;
    const calls = [], routing = [];
    const originalInvoke = M.invoke;
    const queueIds = [videoId, 'Sm0keVid002', 'Sm0keVid003'];
    if (queueIds.some(id => !/^[A-Za-z0-9_-]{11}$/.test(id))) throw new Error('fixture queue IDs must each be 11 characters');
    const items = queueIds.map((id, i) => ({ id, title: 'Synthetic fixture ' + (i + 1), available: true }));
    M.api = async function fixtureApi(method, path, body, extra) {
      const activeId = routing.length ? routing[routing.length - 1].video_id : videoId;
      if (path === '/api/watch/' + activeId && (method === 'GET' || method === 'POST')) {
        calls.push({method, path});
        return {video_id:activeId,playback:{ready:true,state:'ready',kind:'file',ext:'mp4',url_path:'/media/native-smoke-'+activeId,duration_seconds:30},meta:{video_id:activeId,title:'Synthetic native player fixture',duration_seconds:30}};
      }
      if (path === '/api/video/' + activeId && method === 'GET') {
        calls.push({method,path});
        return {meta:{video_id:activeId,title:'Synthetic native player fixture',duration_seconds:30},entries:[],comments:[],related:[]};
      }
      if (path.startsWith('/api/playlist?') && method === 'GET') {
        const requestUrl = new URL(path, M.origin);
        const requestedVideo = requestUrl.searchParams.get('video');
        const matchedIndex = queueIds.indexOf(requestedVideo);
        const activeIndex = matchedIndex >= 0 ? matchedIndex : queueIds.indexOf(activeId);
        calls.push({method, path, requestedVideo, activeIndex});
        return { list_id: options.playlistId, items, active_index: activeIndex, truncated: false };
      }
      return originalApi(method, path, body, extra);
    };
    M.invoke = async function observeInvoke(command, args) {
      if (command === 'tab_open_video') routing.push({...args});
      return originalInvoke(command, args);
    };
    const results = [];
    const assert = (name, ok, detail) => { results.push({name, passed: !!ok, detail: detail ?? null}); if (!ok) throw new Error(name + ': ' + JSON.stringify(detail)); };
    const key = (value, target=document.body, composing=false, modifiers={}) => {
      const e = new KeyboardEvent('keydown', {key:value, bubbles:true, cancelable:true,...modifiers});
      if (composing) Object.defineProperty(e, 'isComposing', {value:true});
      target.dispatchEvent(e); return e;
    };
    const settle = () => new Promise(resolve => setTimeout(resolve, 120));
    try {
      // Open real app tab via its native command; then activate the tab to mount watch.
      const tabId = await M.newTab('https://www.youtube.com/watch?v='+videoId+'&list='+encodeURIComponent(options.playlistId));
      if (!tabId) throw new Error('could not create fixture player tab');
      await M.invoke('tab_open_video',{id:tabId,video_id:videoId,playlist:{list_id:options.playlistId,index:1}});
      // tab_open_video updates native context but does not guarantee this new tab is
      // active; the watch keyboard listener intentionally ignores sleeping tabs.
      await M.activateTab(tabId);
      await M.refreshTabs?.();
      await settle();
      const video = document.querySelector('video');
      assert('synthetic watch API fixture consumed', calls.some(x => x.path === '/api/watch/' + videoId && x.method==='POST') && calls.some(x => x.path === '/api/video/' + videoId && x.method==='GET'), calls);
      assert('production video element mounted', video instanceof HTMLVideoElement, !!video);
      if (!(video instanceof HTMLVideoElement)) return {passed:false, results, calls, tabId};
      await new Promise((resolve, reject) => {
        if (video.readyState >= 1) return resolve();
        const timer = setTimeout(() => reject(new Error('local fixture metadata timed out')), 8000);
        video.addEventListener('loadedmetadata', () => {clearTimeout(timer);resolve();}, {once:true});
        video.addEventListener('error', () => {clearTimeout(timer);reject(new Error('local fixture media failed'));}, {once:true});
      });
      video.currentTime = Math.min(12, Math.max(0, video.duration / 2)); await settle();
      const start = video.currentTime;
      key('J'); await settle(); assert('J seeks backward 10 seconds', Math.abs(video.currentTime-Math.max(0,start-10))<1.5, video.currentTime);
      const beforeL=video.currentTime; key('L'); await settle(); assert('L seeks forward 10 seconds', Math.abs(video.currentTime-Math.min(video.duration,beforeL+10))<1.5, video.currentTime);
      video.pause(); key('k'); await settle(); assert('K toggles playback through production handler', !video.paused, {paused:video.paused});
      video.pause();
      const theatre=document.querySelector('#view-watch');
      assert('theatre selector matches the production watch view root', !!theatre && theatre.id==='view-watch', theatre?.outerHTML?.slice(0,200));
      const theaterBefore=theatre.classList.contains('theatre'); key('T'); await settle();
      assert('T toggles theatre mode', theatre.classList.contains('theatre') !== theaterBefore, theatre.className);
      const guardedAt=video.currentTime, input=document.createElement('input'); document.body.append(input); input.focus();
      key('J', input); key('J',document.body,true); await settle();
      assert('focused editable and IME key events do not seek', Math.abs(video.currentTime-guardedAt)<0.2, video.currentTime);
      input.remove();
      const queueCallsBefore=calls.filter(x=>x.path.startsWith('/api/playlist?')).length;
      const initialQueueRequest=calls.find(x=>x.path.startsWith('/api/playlist?'));
      assert('production playlist read fixture served', queueCallsBefore>0, calls);
      assert('playlist fixture honors requested video and zero-based active index', initialQueueRequest?.requestedVideo===videoId && initialQueueRequest.activeIndex===queueIds.indexOf(videoId), initialQueueRequest);
      const queue=document.querySelector('.playlist-queue');
      assert('production queue rendered', !!queue && !queue.hidden, queue?.outerHTML);
      const currentPlayer = async expectedId => {
        for (let attempt=0; attempt<30; attempt++) {
          await settle();
          const current=document.querySelector('video');
          await M.refreshTabs();
          const tab=M.tabs.list.find(t=>t.id===tabId);
          const expectedIndex=queueIds.indexOf(expectedId);
          const request=calls.find(call=>call.path.startsWith('/api/playlist?')&&call.requestedVideo===expectedId);
          const currentIndex=[...document.querySelectorAll('.queue-item')].findIndex(item=>item.classList.contains('current'));
          if(current instanceof HTMLVideoElement && tab?.video_id===expectedId && request?.activeIndex===expectedIndex && currentIndex===expectedIndex) return current;
        }
        const tab=M.tabs.list.find(t=>t.id===tabId);
        const videos=[...document.querySelectorAll('video')].map(v=>({currentSrc:v.currentSrc,src:v.src,readyState:v.readyState,paused:v.paused,connected:v.isConnected}));
        throw new Error('current player/tab did not remount for '+expectedId+'; snapshot='+JSON.stringify({expectedId,tabId,activeTabId:M.tabs.active?.id??M.tabs.active,tab:tab&&{id:tab.id,video_id:tab.video_id,playlist:tab.playlist,url:tab.url},queue:{index:document.querySelectorAll('.queue-item.current').length?[...document.querySelectorAll('.queue-item')].findIndex(b=>b.classList.contains('current')):null,items:[...document.querySelectorAll('.queue-item')].map(b=>b.textContent)},routing:routing.slice(-8),apiCalls:calls.slice(-12),videos}));
      };
      const queueButton=queue?.querySelectorAll('button.queue-item')[1];
      if (queueButton) { queueButton.click(); await settle(); }
      // Native routing and the resulting playlist request complete asynchronously.
      // Wait for the matching request/render, rather than asserting after 120ms.
      let activeVideo=await currentPlayer(queueIds[1]);
      let opens=routing.filter(x=>x.id===tabId&&x.video_id===queueIds[1]&&x.playlist?.index===2);
      const secondQueueRequest=calls.filter(x=>x.path.startsWith('/api/playlist?')).find(x=>x.requestedVideo===queueIds[1]);
      assert('queue selection routes second valid id with playlist index 2', !!queueButton && opens.length>0, routing);
      assert('second-video queue request returns zero-based index 1', !!secondQueueRequest && secondQueueRequest.activeIndex===1, secondQueueRequest);
      assert('selection remounts current second player and native tab context', document.querySelector('video')===activeVideo, M.tabs.list.find(t=>t.id===tabId));
      key('N',document.body,false,{shiftKey:true}); await settle();
      activeVideo=await currentPlayer(queueIds[2]);
      opens=routing.filter(x=>x.id===tabId&&x.video_id===queueIds[2]&&x.playlist?.index===3);
      const thirdQueueRequest=calls.filter(x=>x.path.startsWith('/api/playlist?')).find(x=>x.requestedVideo===queueIds[2]);
      assert('next queue control routes third id and refreshes current tab/player', opens.length>0 && document.querySelector('video')===activeVideo, {opens,routing});
      assert('third-video queue request returns zero-based index 2', !!thirdQueueRequest && thirdQueueRequest.activeIndex===2, thirdQueueRequest);
      activeVideo.dispatchEvent(new Event('ended')); await settle();
      opens=routing.filter(x=>x.id===tabId&&x.video_id===queueIds[2]&&x.playlist?.index===3);
      await M.refreshTabs();
      assert('ended on current third mounted player does not route stale context', opens.length===1 && M.tabs.list.find(t=>t.id===tabId)?.video_id===queueIds[2], {opens,routing});
      key('P',document.body,false,{shiftKey:true}); await settle();
      activeVideo=await currentPlayer(queueIds[1]);
      opens=routing.filter(x=>x.id===tabId&&x.video_id===queueIds[1]&&x.playlist?.index===2);
      assert('previous queue control routes second id and refreshes current tab/player', opens.length>=2 && document.querySelector('video')===activeVideo, {opens,routing});
      return {passed:results.every(x=>x.passed), results, calls, routing, tabId};
    } finally {
      M.api = originalApi; M.invoke=originalInvoke;
      Object.defineProperty(HTMLMediaElement.prototype, 'src', sourceProperty);
      URL.revokeObjectURL(mediaUrl);
    }
  };
})(window);
