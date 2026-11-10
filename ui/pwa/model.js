/* Shared pure model: also exercised by Node without a DOM. */
(function(root) {
  'use strict';
  const MAX_DEVICE_BYTES = 512 * 1024 * 1024;
  const validId = id => typeof id === 'string' && /^[A-Za-z0-9_-]{11}$/.test(id);
  function videoId(input) {
    const raw = String(input || '').trim();
    if (validId(raw)) return raw;
    let url; try { url = new URL(raw); } catch { return null; }
    if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password) return null;
    const host = url.hostname.toLowerCase();
    let id;
    if (host === 'youtu.be') id = url.pathname.split('/')[1];
    else if (['youtube.com','www.youtube.com','m.youtube.com','music.youtube.com'].includes(host)) {
      const parts = url.pathname.split('/').filter(Boolean);
      id = parts[0] === 'watch' ? url.searchParams.get('v') :
        ['shorts','live','embed'].includes(parts[0]) ? parts[1] : null;
    }
    return validId(id) ? id : null;
  }
  const youtubeURL = id => validId(id) ? 'https://www.youtube.com/watch?v=' + id : null;
  function sourceURL(input) {
    let url; try { url = new URL(input); } catch { return null; }
    if (!['https:','http:'].includes(url.protocol) || url.username || url.password || url.port) return null;
    if (!['youtube.com','www.youtube.com','m.youtube.com','music.youtube.com','youtu.be'].includes(url.hostname)) return null;
    const id = videoId(url.href);
    if (id) return youtubeURL(id);
    if (!/^\/(?:$|feed\/|channel\/|@|results)/.test(url.pathname)) return null;
    const query = url.pathname === '/results' ? url.searchParams.get('search_query') : null;
    return 'https://www.youtube.com' + url.pathname + (query ? '?search_query=' + encodeURIComponent(query) : '');
  }
  function feedTitle(feed) {
    const src = sourceURL(feed.source_url);
    const parent = src && videoId(src);
    const title = String(feed.source_title || feed.parent_title || '').trim();
    const query = String(feed.query || '').trim();
    if (feed.kind === 'related') return title || query ? 'Related to: ' + (title || query) :
      parent ? 'Related to video ' + parent + ' (title not recorded)' : 'Related — original video not recorded';
    if (feed.kind === 'search') return query ? 'Search: ' + query : 'Search — query not recorded';
    if (feed.kind === 'channel') {
      const handle = src && new URL(src).pathname.match(/^\/(@[^/]+)/)?.[1];
      return title || query || handle || 'Channel — name not recorded';
    }
    return ({home:'Home',subscriptions:'Subscriptions'})[feed.kind] || 'Captured feed';
  }
  function bytes(size) {
    if (!Number.isFinite(Number(size)) || Number(size) < 0) return 'Unknown size';
    const n = Number(size);
    return n >= 1024 ** 3 ? (n / 1024 ** 3).toFixed(1) + ' GB' : (n / 1024 ** 2).toFixed(1) + ' MB';
  }
  const complete = entry => entry?.status === 'complete';
  function validateCopy(expected, actual, type) {
    if (!Number.isSafeInteger(expected) || expected <= 0 || expected > MAX_DEVICE_BYTES)
      throw new Error('Choose a completed file smaller than 512 MB for this initial PWA.');
    if (expected !== actual) throw new Error('Incomplete transfer. Nothing was saved; try again.');
    if (!/^(video|audio)\//i.test(type)) throw new Error('The server did not return a media file.');
  }
  const api = {MAX_DEVICE_BYTES,validId,videoId,youtubeURL,sourceURL,feedTitle,bytes,complete,validateCopy};
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (root) root.MuninnWebModel = api;
})(typeof window === 'undefined' ? null : window);
