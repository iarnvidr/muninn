/* Explicit per-origin device copies. Credentials and API responses never enter
 * this database. A copy is visible only once its metadata + blob commit together. */
(function(root) {
  'use strict';
  const NAME = 'muninn-pwa-device-v1';
  let opening;
  function open() {
    if (!root.indexedDB) return Promise.reject(new Error('Device storage is unavailable in this browser.'));
    return opening ||= new Promise((resolve, reject) => {
      const request = root.indexedDB.open(NAME, 1);
      request.onupgradeneeded = () => {
        request.result.createObjectStore('metadata', {keyPath:'id'});
        request.result.createObjectStore('media');
      };
      request.onsuccess = () => {
        const db = request.result;
        db.onversionchange = () => { db.close(); opening = null; };
        resolve(db);
      };
      request.onerror = () => { opening = null; reject(request.error); };
      request.onblocked = () => reject(new Error('Close other Muninn web windows to update device storage.'));
    });
  }
  async function transaction(mode, fn) {
    const db = await open();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(['metadata','media'], mode);
      let value;
      tx.oncomplete = () => resolve(value);
      tx.onabort = tx.onerror = () => reject(tx.error || new Error('Device storage failed; free space and retry.'));
      try { fn(tx, result => { value = result; }); } catch (error) { tx.abort(); reject(error); }
    });
  }
  root.MuninnWebStore = {
    list: () => transaction('readonly', (tx, done) => {
      const request = tx.objectStore('metadata').getAll(); request.onsuccess = () => done(request.result);
    }),
    get: id => transaction('readonly', (tx, done) => {
      const request = tx.objectStore('media').get(id); request.onsuccess = () => done(request.result);
    }),
    save: (entry, blob) => transaction('readwrite', tx => {
      const metadata = {id:entry.id,video_id:entry.video_id,title:entry.title || entry.video_id,
        channel:entry.channel || '',size_bytes:blob.size,type:blob.type,variant:entry.variant,
        saved_at:Date.now(),position_seconds:0};
      tx.objectStore('metadata').put(metadata); tx.objectStore('media').put(blob, entry.id);
    }),
    remove: id => transaction('readwrite', tx => { tx.objectStore('metadata').delete(id); tx.objectStore('media').delete(id); }),
    clear: () => transaction('readwrite', tx => { tx.objectStore('metadata').clear(); tx.objectStore('media').clear(); }),
    position: (id, seconds) => transaction('readwrite', tx => {
      const table = tx.objectStore('metadata'), request = table.get(id);
      request.onsuccess = () => { if (request.result) table.put({...request.result,position_seconds:Math.max(0,Number(seconds)||0)}); };
    }),
  };
})(window);
