'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('playlist-only watch bootstraps from playlist API without issuing video API calls for an empty id', () => {
  const source = fs.readFileSync(path.join(__dirname, '../watch.js'), 'utf8');
  const begin = source.slice(source.indexOf('async function loadQueue'), source.indexOf('function renderQueue'));
  assert.match(begin, /new URLSearchParams\(\{list:p\.list_id\}\)/);
  assert.match(begin, /if\(c\.videoId\)qs\.set\('video',c\.videoId\)/);
  assert.match(begin, /if\(!c\.videoId\)\{/);
  assert.match(begin, /M\.invoke\('tab_open_video',\{id:c\.tabId,video_id:selected\.id,playlist:\{list_id:p\.list_id,index:items\.indexOf\(selected\)\+1\}\}\)/);
  assert.match(source, /if\(c\.videoId\)\{fetchDetail\(c\);startWatch\(c\);\}/);
  assert.match(begin, /item=>item\?\.available!==false&&item\?\.id/);
  const rendering = source.slice(source.indexOf('function renderQueue'), source.indexOf('async function selectQueue'));
  assert.match(rendering, /Could not load playlist queue/);
  assert.match(rendering, /text:'Retry',onclick:\(\)=>loadQueue\(c\)/);
});
