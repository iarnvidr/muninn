'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const L = require('../lib.js');

test('Diagnostics is not in the static navigation', () => {
  const html = fs.readFileSync(path.join(__dirname, '../index.html'), 'utf8');
  assert.ok(html.includes('data-page="settings"'));
  assert.ok(!html.includes('data-page="diagnostics"'));
});

test('developer mode defaults off and gates the sidebar', () => {
  for (const settings of [undefined, null, {}, { developer_mode: false }, { developer_mode: 'yes' }]) {
    assert.equal(L.developerMode(settings), false);
    assert.ok(!L.sidebarPages(settings).includes('diagnostics'));
  }
  const on = L.sidebarPages({ developer_mode: true });
  assert.deepEqual(on, ['browse', 'player', 'library', 'feeds', 'rules', 'diagnostics', 'settings']);
});

test('turning developer mode off while Diagnostics is open falls back to Settings', () => {
  assert.equal(L.resolvePage('diagnostics', { developer_mode: true }), 'diagnostics');
  assert.equal(L.resolvePage('diagnostics', { developer_mode: false }), 'settings');
  assert.equal(L.resolvePage('diagnostics', undefined), 'settings');
  assert.equal(L.resolvePage('library', { developer_mode: false }), 'library');
  assert.equal(L.resolvePage(null, {}), null);
  assert.equal(L.pageAllowed('settings', {}), true);
});

test('the Developer switch is a persisted settings field and patches by itself', () => {
  const field = L.SETTINGS_FIELDS.find(f => f.key === 'developer_mode');
  assert.equal(field.kind, 'bool');
  assert.equal(field.group, 'Developer');
  const orig = { developer_mode: false };
  const result = L.settingsPatch([field], orig, { developer_mode: true });
  assert.deepEqual(result.patch, { developer_mode: true });
});

test('export outcomes: saved path, harmless cancel, clear errors', () => {
  assert.deepEqual(L.exportOutcome({ status: 'saved', path: '/tmp/x.zip' }), { kind: 'ok', text: 'Saved diagnostics to /tmp/x.zip' });
  assert.deepEqual(L.exportOutcome({ status: 'cancelled' }), { kind: 'info', text: '' });
  assert.equal(L.exportOutcome(null, 'File exists').kind, 'error');
  assert.match(L.exportOutcome(null, new Error('disk full')).text, /disk full/);
  assert.equal(L.exportOutcome(undefined).kind, 'error');
});
