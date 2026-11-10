/* Only this PWA's static shell is cached. No API, thumbnails, sessions, media,
 * cross-origin responses, mutations or authenticated requests enter CacheStorage.
 * Complete device videos are explicit IndexedDB objects managed by the app. */
'use strict';
const CACHE = 'muninn-pwa-shell-v1';
const SHELL = ['/app/','/app/index.html','/app/style.css','/app/model.js','/app/store.js',
  '/app/app.js','/app/manifest.webmanifest','/app/icon.svg','/app/icon-192.png','/app/icon-512.png'];
self.addEventListener('install', event => event.waitUntil((async()=>{
  const cache=await caches.open(CACHE);
  await cache.addAll(SHELL.map(url=>new Request(url,{credentials:'omit',cache:'reload'})));
  // Do not replace a worker beneath an active transfer; activate on next open.
})()));
self.addEventListener('activate', event => event.waitUntil((async()=>{
  for(const name of await caches.keys())if(name.startsWith('muninn-pwa-shell-')&&name!==CACHE)await caches.delete(name);
  await self.clients.claim();
})()));
self.addEventListener('fetch', event => {
  const request=event.request,url=new URL(request.url);
  if(request.method!=='GET'||url.origin!==self.location.origin||url.search||
      request.headers.has('authorization')||!SHELL.includes(url.pathname))return;
  event.respondWith((async()=>{
    const cache=await caches.open(CACHE);
    // Network first keeps deployment upgrades usable, cached shell permits
    // offline launch. Strip credentials because these assets are public.
    try{
      const response=await fetch(new Request(request,{credentials:'omit'}));
      if(response.ok&&!response.redirected){await cache.put(request,response.clone());return response;}
      return (await cache.match(url.pathname))||response;
    }catch{return (await cache.match(url.pathname))||Response.error();}
  })());
});
