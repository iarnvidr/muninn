/* Runs only in the existing YouTube origin. Credentials never leave this page.
 * Narrow operation: one explicitly requested subscription removal.
 */
(function (root) {
  'use strict';
  async function unsubscribe(w, channelId, expectedAccount) {
    if (!/^UC[A-Za-z0-9_-]{22}$/.test(channelId)) throw new Error('Invalid channel');
    if (w.location.origin !== 'https://www.youtube.com') throw new Error('A signed-in YouTube page is required');
    const cfg=w.ytcfg;
    if (!cfg?.get('LOGGED_IN')) throw new Error('Sign in to YouTube in Browse, then try again.');
    if(!expectedAccount||typeof w.__muninnAccountKey!=='function'||await w.__muninnAccountKey()!==expectedAccount)throw new Error('YouTube account changed. Load subscriptions for the current account before unsubscribing.');
    const context=cfg.get('INNERTUBE_CONTEXT');
    if (!context?.client) throw new Error('YouTube is still loading. Try again shortly.');
    const cookie=w.document.cookie.split(';').map(s=>s.trim()).find(s=>s.startsWith('SAPISID='));
    if (!cookie) throw new Error('YouTube sign-in is unavailable. Open Browse and sign in again.');
    const timestamp=Math.floor(Date.now()/1000);
    const digest=await w.crypto.subtle.digest('SHA-1',new TextEncoder().encode(timestamp+' '+cookie.slice(8)+' '+w.location.origin));
    const hash=Array.from(new Uint8Array(digest),v=>v.toString(16).padStart(2,'0')).join('');
    const headers={'Content-Type':'application/json','Authorization':'SAPISIDHASH '+timestamp+'_'+hash,
      'X-Goog-AuthUser':String(cfg.get('SESSION_INDEX')||0),'X-Youtube-Client-Name':String(cfg.get('INNERTUBE_CONTEXT_CLIENT_NAME')||1),
      'X-Youtube-Client-Version':context.client.clientVersion};
    if(context.user?.onBehalfOfUser)headers['X-Goog-PageId']=context.user.onBehalfOfUser;
    const controller=new AbortController(),timer=setTimeout(()=>controller.abort(),20000);
    try {
      if(await w.__muninnAccountKey()!==expectedAccount)throw new Error('YouTube account changed before unsubscribe.');
      const response=await w.fetch('/youtubei/v1/subscription/unsubscribe?prettyPrint=false',{
        method:'POST',credentials:'include',redirect:'error',headers,signal:controller.signal,
        body:JSON.stringify({context,channelIds:[channelId],params:'CgIIAhgA'})
      });
      if(!response.ok)throw new Error('YouTube rejected the request ('+response.status+').');
      const data=await response.json();
      if(data.error||data.responseContext?.mainAppWebResponseContext?.loggedOut===true)throw new Error('YouTube did not accept the subscription change.');
      // Require positive acknowledgement, not merely HTTP 200 from a signed-out page.
      let confirmed=false;
      function walk(value,depth=0){
        if(!value||typeof value!=='object'||depth>25)return;
        const action=value.updateSubscribeButtonAction;
        if(action?.channelId===channelId&&action.subscribed===false)confirmed=true;
        const state=value.subscriptionStateEntity;
        if(state?.channelId===channelId&&state.subscribed===false)confirmed=true;
        for(const child of Object.values(value))walk(child,depth+1);
      }
      walk(data);
      if(!confirmed)throw new Error('YouTube did not confirm the new subscription state. Refresh subscriptions before retrying.');
      return {ok:true};
    } finally {clearTimeout(timer);}
  }
  if(typeof module==='object'&&module.exports)module.exports={unsubscribe};
  else root.__muninnUnsubscribe=unsubscribe;
})(typeof globalThis==='undefined'?this:globalThis);
