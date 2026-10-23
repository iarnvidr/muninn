/* Muninn UI start-up: pick the token (app_boot, URL hash or session), load the
 * development mock only for ?mock=1 outside the app, then start the shell. */
(function () {
  'use strict';
  const M = window.Muninn;

  function loadScript(src) {
    return new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = resolve;
      s.onerror = () => reject(new Error('could not load ' + src));
      document.head.append(s);
    });
  }

  function hashToken() {
    const m = /(?:^#|&)token=([^&]*)/.exec(location.hash);
    return m ? decodeURIComponent(m[1]) : null;
  }

  (async function main() {
    const fromHash = hashToken();
    if (fromHash) {
      try { sessionStorage.setItem('muninn.token', fromHash); } catch (e) { /* ignore */ }
      history.replaceState(null, '', location.pathname + location.search);   // the token does not stay in the URL
    }
    // The mock exists only for plain-browser testing; the real app never loads it.
    if (!window.__TAURI_INTERNALS__ && /[?&]mock=1(&|$)/.test(location.search)) {
      try { await loadScript('/ui/mock.js'); } catch (e) { console.error(e); }
    }
    let boot = null;
    try { boot = await M.invoke('app_boot'); } catch (e) { /* not in the app */ }
    let stored = null;
    try { stored = sessionStorage.getItem('muninn.token'); } catch (e) { /* ignore */ }
    M.token = (boot && boot.token) || fromHash || stored;
    M.boot = boot;
    if (!M.token) M.flash('error', 'No access token. Open Muninn from the app.', 60000);
    await M.start();
  })();
})();
