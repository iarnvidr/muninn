/* Muninn: page script for the YouTube tabs (runs at document start in the main frame).
 *
 * Everything about the network (feeds, metadata, comments, blocking video bytes)
 * happens in Muninn's proxy. This script only does what lives in the DOM:
 *   - a video link opens Muninn's own player instead of YouTube's (click, middle
 *     click, keyboard, and YouTube's own single-page navigation);
 *   - it reports the page URL, title and scroll position so a hibernated tab can
 *     be restored where it was.
 * It talks to Rust through two narrow commands (page_open_video, page_state);
 * see src-tauri/capabilities/youtube.json. It never throws into the page.
 */
(function (root, factory) {
  'use strict';
  var api = factory(root);
  if (typeof module === 'object' && module && module.exports) {
    module.exports = api;
  }
})(typeof globalThis !== 'undefined' ? globalThis : this, function (root) {
  'use strict';

  var ID_RE = /^[A-Za-z0-9_-]{11}$/;
  // Structural ad containers, independent of the user's YouTube language.
  // CSS also covers cards inserted by SPA navigation and infinite scrolling.
  var AD_CONTAINERS = 'ytd-ad-slot-renderer,yt-ad-slot-renderer,ytd-in-feed-ad-layout-renderer,ytd-display-ad-renderer,ytd-promoted-video-renderer,ytd-promoted-sparkles-web-renderer,ytd-promoted-sparkles-text-search-renderer,ytd-search-pyv-renderer,ytd-action-companion-ad-renderer,ytd-banner-promo-renderer';
  var SPONSORED_CSS = AD_CONTAINERS + ', :is(ytd-rich-item-renderer,ytd-video-renderer,ytd-grid-video-renderer,yt-lockup-view-model):has(:is(' + AD_CONTAINERS + ')) { display:none!important; }';

  function blockSponsoredCards(doc) {
    if (!doc.createElement || doc.getElementById('muninn-sponsored-filter')) return;
    var parent = doc.head || doc.documentElement;
    if (!parent) return;
    var style = doc.createElement('style');
    style.id = 'muninn-sponsored-filter';
    style.textContent = SPONSORED_CSS;
    parent.appendChild(style);
  }

  /** The video id a YouTube link opens, or null when it is not a video link. */
  function videoIdFromHref(href, base) {
    try {
      var u = new URL(href, base || 'https://www.youtube.com/');
      var host = u.hostname.toLowerCase();
      var id = null;
      if (host === 'youtu.be') {
        id = u.pathname.slice(1).split('/')[0];
      } else if (host === 'youtube.com' || host.slice(-12) === '.youtube.com') {
        var p = u.pathname;
        if (p === '/watch') {
          id = u.searchParams.get('v');
        } else {
          var m = /^\/(shorts|live|embed|v)\/([^/?#]+)/.exec(p);
          if (m) id = m[2];
        }
      }
      return id && ID_RE.test(id) ? id : null;
    } catch (e) {
      return null;
    }
  }

  /** Throttle: run fn at most once per ms, trailing edge included. */
  function throttle(fn, ms) {
    var last = 0;
    var timer = null;
    return function () {
      var now = Date.now();
      if (now - last >= ms) {
        last = now;
        fn();
      } else if (!timer) {
        timer = setTimeout(function () {
          timer = null;
          last = Date.now();
          fn();
        }, ms - (now - last));
      }
    };
  }

  function channelCandidates(data) {
    var found = {}, seen = new Set(), visited = 0;
    function text(value) {
      if (typeof value === 'string') return value;
      if (!value) return '';
      if (typeof value.content === 'string') return value.content;
      if (typeof value.simpleText === 'string') return value.simpleText;
      return Array.isArray(value.runs) ? value.runs.map(function(r){return r.text || '';}).join('') : '';
    }
    function walk(node,depth) {
      if (!node || typeof node !== 'object' || depth > 45 || visited++ > 100000 || seen.has(node)) return;
      seen.add(node);
      var endpoint = node.navigationEndpoint || node.endpoint || {};
      var id = node.channelId || node.contentId || (endpoint.browseEndpoint || {}).browseId;
      var metadata = (node.metadata || {}).lockupMetadataViewModel || {};
      var name = text(node.title || node.channelName || metadata.title);
      if (typeof id === 'string' && /^UC[A-Za-z0-9_-]{22}$/.test(id) && name.trim()) found[id] = {channel_id:id,name:name.trim().slice(0,200)};
      Object.keys(node).forEach(function(key){walk(node[key],depth+1);});
    }
    walk(data,0);return Object.keys(found).slice(0,1000).map(function(id){return found[id];});
  }

  function accountSeed(w) {
    var cfg=w.ytcfg;
    if(!cfg||typeof cfg.get!=='function'||cfg.get('LOGGED_IN')!==true)return null;
    var context=cfg.get('INNERTUBE_CONTEXT')||{};
    var identity=cfg.get('DATASYNC_ID')||cfg.get('DELEGATED_SESSION_ID')||(context.user||{}).onBehalfOfUser;
    if(typeof identity!=='string'||!identity||identity.length>4096)return null;
    return String(cfg.get('SESSION_INDEX')||0)+'|'+identity;
  }
  async function accountKey(w) {
    var seed=accountSeed(w);
    if(!seed||!w.crypto||!w.crypto.subtle)return null;
    var digest=await w.crypto.subtle.digest('SHA-256',new TextEncoder().encode(seed));
    return Array.from(new Uint8Array(digest),function(v){return v.toString(16).padStart(2,'0');}).join('');
  }

  function install() {
    var w = root;
    if (!w || !w.document || w.__muninnInstalled) return;
    w.__muninnAccountKey=function(){return accountKey(w);};
    w.__muninnInstalled = true;
    blockSponsoredCards(w.document);
    w.document.addEventListener('DOMContentLoaded', function () { blockSponsoredCards(w.document); });
    var internals = function () {
      return w.__TAURI_INTERNALS__;
    };
    function send(cmd, args) {
      try {
        var t = internals();
        if (t && typeof t.invoke === 'function') {
          var p = t.invoke(cmd, args || {});
          if (p && typeof p.catch === 'function') p.catch(function () {});
        }
      } catch (e) {
        /* the page must never see our errors */
      }
    }

    function openVideo(id, background) {
      send('page_open_video', { video_id: id, background: !!background });
    }

    function linkOf(target) {
      var el = target;
      while (el && el !== w.document) {
        if (el.tagName === 'A' && el.href) return el;
        el = el.parentNode || el.host;
      }
      return null;
    }

    function onClick(e) {
      if (e.defaultPrevented && e.type === 'click') return;
      var a = linkOf(e.target);
      if (!a) return;
      var id = videoIdFromHref(a.href, w.location.href);
      if (!id) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      openVideo(id, e.metaKey || e.ctrlKey || e.button === 1);
    }
    w.document.addEventListener('click', onClick, true);
    w.document.addEventListener('auxclick', function (e) {
      if (e.button === 1) onClick(e);
    }, true);

    // YouTube's own navigation (autoplay, programmatic): cannot be cancelled, so go
    // back to where we were and open the video in Muninn instead.
    function onNavigateStart(e) {
      var url = e && e.detail && (e.detail.url || (e.detail.endpoint && e.detail.endpoint.commandMetadata && e.detail.endpoint.commandMetadata.webCommandMetadata && e.detail.endpoint.commandMetadata.webCommandMetadata.url));
      var id = url ? videoIdFromHref(url, w.location.href) : null;
      if (id) {
        openVideo(id, false);
        if (e.stopImmediatePropagation) e.stopImmediatePropagation();
      }
    }
    w.addEventListener('yt-navigate-start', onNavigateStart, true);
    w.document.addEventListener('yt-navigate-start', onNavigateStart, true);

    // Keep any <video> from playing: the proxy blocks the bytes, this stops the spinner too.
    function quiet() {
      try {
        var vs = w.document.getElementsByTagName('video');
        for (var i = 0; i < vs.length; i++) {
          if (!vs[i].paused) vs[i].pause();
        }
      } catch (e) {
        /* ignore */
      }
    }
    w.addEventListener('play', quiet, true);

    var last = '', channelScan = null, authSignature=null, authSeed=null, authSignedIn=null, authKey=null, authPending=null, authChangedAt=0, authContextChanged=false, documentStartedAt=Date.now();
    function report(force) {
      var url = w.location.href;
      var title = w.document.title || '';
      var scroll = Math.round(w.scrollY || 0);
      var key = url + '|' + title + '|' + Math.round(scroll / 40);
      var args = { url: url, title: title, scroll_y: scroll };
      var signedIn=w.ytcfg&&typeof w.ytcfg.get==='function'?w.ytcfg.get('LOGGED_IN'):null;
      if(typeof signedIn==='boolean'){
        var seed=accountSeed(w),signature=String(signedIn)+'|'+(seed||'');
        if(signature!==authSignature){
          var changed=authSignature!==null&&((authSeed&&seed!==authSeed)||signedIn!==authSignedIn);
          if(changed){authContextChanged=true;authChangedAt=Date.now();}else if(authSignature===null)authChangedAt=documentStartedAt;
          authSignature=signature;authSeed=seed;authSignedIn=signedIn;authKey=null;authPending=null;channelScan=null;
        }
        if(signedIn&&seed&&!authKey&&authPending!==signature){
          authPending=signature;
          (function(expected){accountKey(w).then(function(value){if(authSignature===expected&&value){authKey=value;report(true);}},function(){if(authSignature===expected)authPending=null;});})(signature);
        }
        args.signed_in=signedIn;args.account_key=authKey;args.account_changed_at=authChangedAt;
        key+='|'+signature+'|'+(authKey||'pending');
      }
      if (w.location.pathname === '/feed/channels') {
        if (!channelScan) channelScan = {start:Date.now(),channels:{},height:0,stable:0,last:0,done:false};
        var samples = [w.ytInitialData];
        if (w.document.querySelector) {var app = w.document.querySelector('ytd-app');if(app)samples.push(app.data);}
        if (w.document.querySelectorAll) {
          Array.from(w.document.querySelectorAll('ytd-channel-renderer, yt-lockup-view-model, ytd-guide-entry-renderer')).forEach(function(node){samples.push(node.data);});
          Array.from(w.document.querySelectorAll('a[href*="/channel/UC"]')).forEach(function(anchor){var match=/\/channel\/(UC[A-Za-z0-9_-]{22})/.exec(anchor.getAttribute('href')||'');var name=(anchor.textContent||'').trim();if(match&&name)samples.push({channelId:match[1],title:name});});
        }
        samples.forEach(function(sample){channelCandidates(sample).forEach(function(channel){channelScan.channels[channel.channel_id]=channel;});});
        var list = Object.keys(channelScan.channels).slice(0,1000).map(function(id){return channelScan.channels[id];});
        var loggedOut = w.ytcfg && typeof w.ytcfg.get === 'function' && w.ytcfg.get('LOGGED_IN') === false;
        if (!channelScan.done && Date.now()-channelScan.last > 1500) {
          var height=(w.document.documentElement||{}).scrollHeight||0;
          channelScan.stable=height===channelScan.height?channelScan.stable+1:0;channelScan.height=height;channelScan.last=Date.now();
          channelScan.done=loggedOut || list.length>=1000 || (list.length>0&&channelScan.stable>=3) || Date.now()-channelScan.start>45000;
          if(!channelScan.done && w.scrollTo)w.scrollTo(0,height);
        }
        args.channels=authContextChanged?[]:list;
        args.channel_status=loggedOut?'needs_sign_in':authContextChanged?'error':channelScan.done?(list.length?'ready':'empty'):'loading';
        key+='|'+args.channel_status+'|'+list.map(function(c){return c.channel_id;}).join(',');
      } else {channelScan=null;}
      if (!force && key === last) return;
      last = key;
      send('page_state', args);
    }
    var soon = throttle(function () {
      report(false);
    }, 1200);
    w.addEventListener('scroll', soon, { passive: true });
    w.addEventListener('yt-navigate-finish', function () {
      report(true);
    }, true);
    w.addEventListener('popstate', soon);
    w.addEventListener('load', function () {
      report(true);
    });
    w.document.addEventListener('DOMContentLoaded', function () {
      report(true);
    });
    w.document.addEventListener('visibilitychange', soon);
    setInterval(soon, 4000);
    // Title changes after SPA navigation.
    try {
      new w.MutationObserver(soon).observe(w.document.documentElement, { childList: true, subtree: false });
    } catch (e) {
      /* ignore */
    }
  }

  try {
    install();
  } catch (e) {
    /* ignore */
  }

  return { videoIdFromHref: videoIdFromHref, throttle: throttle, channelCandidates:channelCandidates, accountKey:accountKey, sponsoredCSS:SPONSORED_CSS };
});
