// QA phase 1 (docs/spikes.md Task 12 Step 5, Task 13 Step 5, S1): the installed key router,
// driven through a fake `document` (no WebView): which keys run app commands, which reach the
// terminal, held keys, profile passthrough keys, and an open overlay.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { installKeyRouter, buildKeymap } from '../../src/keys.js';

const DEFAULTS = JSON.parse(readFileSync(new URL('../../src-tauri/defaults/keybindings.json', import.meta.url), 'utf8'));

function setup({ passthrough = {} } = {}) {
  const listeners = {};
  globalThis.document = {
    addEventListener: (t, f) => (listeners[t] ||= []).push(f),
    removeEventListener: (t, f) => (listeners[t] = (listeners[t] || []).filter((x) => x !== f)),
  };
  const runs = [];
  const router = installKeyRouter({
    keymap: buildKeymap(DEFAULTS),
    getPassthrough: (id) => passthrough[id] || [],
    focusedPane: () => 'p1',
    run: (id) => runs.push(id),
  });
  const fire = (type, e) => {
    for (const f of listeners[type] || []) f(e);
    return e;
  };
  const done = () => {
    router.dispose();
    delete globalThis.document;
  };
  return { router, runs, down: (e) => fire('keydown', e), up: (e) => fire('keyup', e), done };
}

/** A keydown on a terminal of pane `pane` (target inside `.pane[data-pane]`). */
const key = (code, mods = '', extra = {}) => {
  const e = {
    code,
    key: '?',
    ctrlKey: mods.includes('C'),
    altKey: mods.includes('A'),
    shiftKey: mods.includes('S'),
    metaKey: false,
    repeat: false,
    isComposing: false,
    target: { closest: () => ({ dataset: { pane: extra.pane || 'p1' } }) },
    defaultPrevented: false,
    stopped: false,
    ...extra,
  };
  e.preventDefault = () => {
    e.defaultPrevented = true;
  };
  e.stopPropagation = () => {
    e.stopped = true;
  };
  return e;
};

test('app shortcuts run their command and never reach xterm; other keys go to the terminal', () => {
  const t = setup();
  try {
    const e = t.down(key('KeyH', 'A'));
    assert.deepEqual(t.runs, ['pane.focusLeft']);
    assert.ok(e.defaultPrevented && e.stopped, 'the chord is not typed into the terminal');
    // spikes.md:223 — each later-task key is an app key for xterm (attachCustomKeyEventHandler)
    for (const [code, mods] of [['KeyQ', 'A'], ['KeyA', 'A'], ['KeyC', 'A'], ['KeyI', 'A'], ['KeyN', 'A'], ['KeyM', 'CA'], ['KeyG', 'CA'], ['Semicolon', 'A'], ['KeyV', 'CS']]) {
      assert.equal(t.router.termKeyFilter('p1', key(code, mods)), false, `${mods}+${code}`);
      assert.equal(t.router.isAppKey(key(code, mods)), true, `${mods}+${code}`);
    }
    // Claude Code's own keys and plain typing are left to the terminal (spikes.md:227)
    for (const [code, mods] of [['KeyR', 'C'], ['KeyP', 'C'], ['KeyP', 'A'], ['KeyV', 'A'], ['Tab', 'S'], ['KeyV', 'C'], ['KeyA', '']]) {
      assert.equal(t.router.termKeyFilter('p1', key(code, mods)), true, `${mods}+${code}`);
      const plain = t.down(key(code, mods));
      assert.equal(plain.defaultPrevented, false, `${mods}+${code}`);
    }
    assert.deepEqual(t.runs, ['pane.focusLeft']);
  } finally {
    t.done();
  }
});

test('a held movement / resize key repeats; a held one-shot key runs once and is still swallowed', () => {
  const t = setup();
  try {
    t.down(key('KeyH', 'AS', { repeat: true })); // spikes.md:213 "누르고 있으면 반복된다"
    t.down(key('KeyL', 'A', { repeat: true }));
    const held = t.down(key('KeyX', 'A', { repeat: true })); // a held Alt+x closes one pane only
    assert.deepEqual(t.runs, ['pane.resizeLeft', 'pane.focusRight']);
    assert.ok(held.defaultPrevented, 'the repeat is not typed either');
  } finally {
    t.done();
  }
});

test('passthroughKeys hand the chord to the program in that pane only', () => {
  const t = setup({ passthrough: { p2: ['Alt+h'] } }); // spikes.md:231 (profile passthroughKeys)
  try {
    const e = t.down(key('KeyH', 'A', { pane: 'p2' }));
    assert.equal(e.defaultPrevented, false);
    assert.equal(t.router.termKeyFilter('p2', key('KeyH', 'A')), true);
    assert.deepEqual(t.runs, []);
    t.down(key('KeyH', 'A', { pane: 'p1' })); // other panes keep the app shortcut
    assert.deepEqual(t.runs, ['pane.focusLeft']);
  } finally {
    t.done();
  }
});

test('while an overlay is open, app shortcuts do nothing and typing keeps its default', () => {
  const t = setup();
  try {
    // an overlay input (prompt / palette) answers false for keys it does not consume: typing
    const token = t.router.pushModal(() => false);
    for (const [code, mods] of [['KeyH', 'A'], ['KeyL', 'CA'], ['KeyX', 'A']]) {
      const e = t.down(key(code, mods)); // spikes.md:271 "입력 중 Alt+h, Ctrl+Alt+l을 눌러도"
      assert.equal(e.defaultPrevented, false, `${mods}+${code} types into the overlay input`);
    }
    assert.deepEqual(t.runs, []);
    // closing the overlay gives the keys back to the app (spikes.md:275, routing part)
    t.router.popModal(token);
    t.down(key('KeyH', 'A'));
    assert.deepEqual(t.runs, ['pane.focusLeft']);
    // copy mode answers null for app chords: they still work (spikes.md:294, routing part)
    const copy = t.router.pushModal((e) => (e.altKey ? null : true));
    t.down(key('KeyL', 'A'));
    assert.deepEqual(t.runs, ['pane.focusLeft', 'pane.focusRight']);
    const j = t.down(key('KeyJ'));
    assert.ok(j.defaultPrevented, 'a copy-mode key is consumed, never typed');
    t.router.popModal(copy);
  } finally {
    t.done();
  }
});

test('a lone Alt keyup and the keyup of a handled chord are swallowed (S1 prevention)', () => {
  const t = setup();
  try {
    assert.ok(t.up(key('AltLeft', '', { key: 'Alt' })).defaultPrevented);
    assert.ok(t.up(key('KeyH', 'A')).defaultPrevented);
    assert.equal(t.up(key('KeyA')).defaultPrevented, false);
  } finally {
    t.done();
  }
});
