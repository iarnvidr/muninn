'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

// Execute the production tabstrip keydown callback against a minimal DOM model.
function wiredHandler() {
  const source = fs.readFileSync(path.join(__dirname, '../app.js'), 'utf8');
  const marker = "$('#tabstrip').addEventListener('keydown', ";
  const start = source.indexOf(marker);
  assert.notEqual(start, -1, 'production keydown listener exists');
  const expressionStart = start + marker.length;
  const end = source.indexOf('\n  });', expressionStart);
  assert.notEqual(end, -1, 'production keydown listener closes');
  const expression = source.slice(expressionStart, end + 4).trim();
  let handler;
  const strip = { addEventListener(type, fn) { assert.equal(type, 'keydown'); handler = fn; } };
  const document = { activeElement: null };
  const tabs = [1, 2, 3].map(id => ({ parentNode: { hidden: false, dataset: { id: String(id) } }, focus() { document.activeElement = this; } }));
  const M = { tabs: { active: 2 }, activateTab(id) { this.activated = id; } };
  const $ = () => strip;
  const $$ = () => tabs;
  vm.runInNewContext(`(${expression})`, { $, $$, M, document, Number });
  // expression is the callback; register it exactly as app.js does.
  strip.addEventListener('keydown', vm.runInNewContext(`(${expression})`, { $, $$, M, document, Number }));
  return { handler, tabs, document, M };
}

test('tabstrip Ctrl+Tab cycles from active tab and focuses/activates production target', () => {
  const { handler, tabs, M, document } = wiredHandler();
  let prevented = false;
  handler({ key: 'Tab', ctrlKey: true, metaKey: false, altKey: false, shiftKey: false, preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  assert.equal(document.activeElement, tabs[2]);
  assert.equal(M.activated, 3);
});

test('Shift+Ctrl+Tab wraps backward; plain Tab is not intercepted', () => {
  const { handler, tabs, M } = wiredHandler();
  let prevented = false;
  handler({ key: 'Tab', ctrlKey: true, metaKey: false, altKey: false, shiftKey: true, preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  assert.equal(M.activated, 1);
  prevented = false;
  handler({ key: 'Tab', ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, preventDefault() { prevented = true; } });
  assert.equal(prevented, false);
});
