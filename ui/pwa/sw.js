/* Only the public app shell is cached. APIs, cookies, thumbnails and media are
 * deliberately outside this allowlist, including while the device is offline. */
'use strict';
const SHELL_CACHE='muninn-pwa-shell-v1';
const SHELL_PATHS=Object.freeze([
  '/app/', '/app/index.html', '/app/app.js', '/app/app.css',
  '/app/manifest.webmanifest', '/app/raven.png', '/app/icon-192.png', '/app/icon-512.png'
]);
function cacheableRequest(request,origin) {
  if(request.method!=='GET'||request.headers.has('Authorization'))return false;
  let url;try{url=new URL(request.url);}catch{return false;}
  return url.origin===origin&&!url.search&&!url.hash&&SHELL_PATHS.includes(url.pathname);
}
if(typeof module!=='undefined'&&module.exports)module.exports={SHELL_CACHE,SHELL_PATHS,cacheableRequest};
if(typeof self!=='undefined'&&self.addEventListener){
  self.addEventListener('install',event=>{
    event.waitUntil(caches.open(SHELL_CACHE).then(cache=>cache.addAll(SHELL_PATHS.map(path=>new Request(path,{credentials:'omit',cache:'reload'})))));
  });
  self.addEventListener('activate',event=>{
    event.waitUntil(caches.keys().then(keys=>Promise.all(keys.filter(key=>key.startsWith('muninn-pwa-shell-')&&key!==SHELL_CACHE).map(key=>caches.delete(key)))).then(()=>self.clients.claim()));
  });
  self.addEventListener('fetch',event=>{
    if(!cacheableRequest(event.request,self.location.origin))return;
    event.respondWith((async()=>{
      const cache=await caches.open(SHELL_CACHE);
      try{
        // Public shell fetching never carries the user's cookie into a cache fill.
        const response=await fetch(new Request(event.request,{credentials:'omit',cache:'no-cache'}));
        if(response.ok&&response.type==='basic')await cache.put(event.request,response.clone());
        return response;
      }catch(error){
        const saved=await cache.match(event.request);
        if(saved)return saved;
        throw error;
      }
    })());
  });
}
