'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const U = require('../updates.js');
test('all updater lifecycle states have safe actionable view', () => {
  for (const state of ['idle','checking','current','available','downloading','installing','unavailable','failed','ready_to_install','installed']) {
    const v = U.view({ state, available_version: state === 'available' ? '2.0' : undefined });
    assert.equal(v.state, state); assert.ok(v.label); assert.equal(v.canInstall, state === 'ready_to_install');
  }
});
test('missing status and missing first release are not reported current', () => {
  assert.equal(U.view(null).state, 'unavailable');
  assert.equal(U.view({ state: 'available' }).state, 'unavailable');
});
test('progress is bounded and normalized; notes remain plain text', () => {
  assert.equal(U.normalize({ state: 'downloading', progress: 150 }).progress, 100);
  assert.equal(U.normalize({ state: 'downloading', progress: -2 }).progress, 0);
  assert.equal(U.view({ state: 'available', available_version: '2', release_notes: '<script>x</script>' }).notes, '<script>x</script>');
});
test('stable channel semantics, cumulative progress, cancellation boundaries and actual version', () => {
  const v = U.view({ state: 'downloading', downloaded: 6, content_length: 10, channel: 'stable', meta: { version: '1.2.3' } });
  assert.equal(v.progress, 60); assert.equal(v.downloaded, 6); assert.equal(v.channel, 'stable'); assert.equal(v.installedVersion, '1.2.3');
  assert.equal(v.canCancel, true); assert.equal(U.view({ state: 'checking' }).canCancel, true);
  assert.equal(U.view({ state: 'installing' }).canCancel, false); assert.equal(U.view({ state: 'ready_to_install' }).canInstall, true);
  assert.match(U.view({ state: 'current', channel: 'stable' }).label, /up to date/);
});
test('errors and offline unavailable do not expose unspecified fields', () => {
  const v = U.view({ state: 'failed', error: 'offline', token: 'secret' });
  assert.equal(v.label, 'offline'); assert.equal('token' in v, false);
});
