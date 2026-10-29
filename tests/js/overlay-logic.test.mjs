import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  PICK_TIMEOUT_MS,
  TOAST_MS,
  checkDirName,
  dirChoices,
  templateOrder,
  listMove,
  pickerKey,
  focusCommands,
  pickCommands,
  commandLabel,
  paletteItems,
  newTabFlow,
  groupMissing,
  missingDirsFlow,
  closeFlow,
  confirmKey,
  envStatusFlag,
  envItemLabel,
} from '../../src/overlay-logic.js';

const leaf = (id) => ({ leaf: id });
const split = (dir, sizes, children) => ({ split: dir, sizes, children });

// g1 (left, focused): t1 dispatcher [p1], t2 active [p2 | p3/p4]; g2 (right): t3 [p5]
function fixture() {
  return {
    groupLayout: split('horizontal', [50, 50], [leaf('g1'), leaf('g2')]),
    groups: {
      g1: { id: 'g1', name: 'control', tabIds: ['t1', 't2'], activeTabId: 't2' },
      g2: { id: 'g2', name: 'work', tabIds: ['t3'], activeTabId: 't3' },
    },
    tabs: {
      t1: { id: 't1', name: 'dispatcher', isDispatcher: true, layout: leaf('p1'), lastFocusedPaneId: 'p1', maximizedPaneId: null },
      t2: {
        id: 't2',
        name: 'api',
        isDispatcher: false,
        layout: split('horizontal', [60, 40], [leaf('p2'), split('vertical', [50, 50], [leaf('p3'), leaf('p4')])]),
        lastFocusedPaneId: 'p2',
        maximizedPaneId: null,
      },
      t3: { id: 't3', name: 'ha', isDispatcher: false, layout: leaf('p5'), lastFocusedPaneId: 'p5', maximizedPaneId: null },
    },
    panes: {
      p1: { id: 'p1', tab: 't1', name: 'main' },
      p2: { id: 'p2', tab: 't2', name: 'lead' },
      p3: { id: 'p3', tab: 't2', name: 'pane-1' },
      p4: { id: 'p4', tab: 't2', name: 'pane-2' },
      p5: { id: 'p5', tab: 't3', name: 'main' },
    },
    focus: { groupId: 'g1' },
    lastLocation: null,
    groupMaximized: null,
  };
}

const r = (id, left, top, right, bottom) => ({ id, left, top, right, bottom });
const geo = {
  paneRects: [r('p2', 0, 30, 360, 700), r('p3', 364, 30, 600, 400), r('p4', 364, 404, 600, 700), r('p5', 604, 30, 1000, 700)],
  groupRects: [r('g1', 0, 0, 600, 700), r('g2', 604, 0, 1000, 700)],
};

test('constants follow the global constraints', () => {
  assert.equal(PICK_TIMEOUT_MS, 2000);
  assert.equal(TOAST_MS, 3000);
});

test('checkDirName mirrors the backend rules', () => {
  for (const bad of ['', '  ', '..', '.', 'a/b', 'a\\b', 'c:']) assert.equal(typeof checkDirName(bad), 'string', bad);
  for (const ok of ['api', 'my-project', '한글']) assert.equal(checkDirName(ok), null, ok);
});

test('dirChoices: empty query lists every dir in order', () => {
  const dirs = ['api', 'mdviewer', 'hoohootv'];
  assert.deepEqual(dirChoices(dirs, ''), dirs.map((dir) => ({ dir, create: false })));
});

test('dirChoices: an exact (case-insensitive) dir name comes first, no create entry (R18)', () => {
  // 'api-old' scores higher for 'api'? it must not matter: the exact name is first
  assert.deepEqual(dirChoices(['api-old', 'xapi', 'api'], 'api'), [
    { dir: 'api', create: false },
    { dir: 'api-old', create: false },
    { dir: 'xapi', create: false },
  ]);
  assert.deepEqual(dirChoices(['mdviewer', 'api'], 'API'), [{ dir: 'api', create: false }]);
});

test('dirChoices: a new name is first (preselected) even when it fuzzy-matches others (R18)', () => {
  assert.deepEqual(dirChoices(['api-old', 'mdviewer'], 'api'), [
    { dir: 'api', create: true },
    { dir: 'api-old', create: false },
  ]);
  assert.deepEqual(dirChoices(['api', 'mdviewer', 'hoohootv'], 'mv'), [
    { dir: 'mv', create: true },
    { dir: 'mdviewer', create: false },
  ]);
  assert.deepEqual(dirChoices(['api'], ' newproj '), [{ dir: 'newproj', create: true }]);
  // an invalid name never becomes the preselected entry
  assert.deepEqual(dirChoices(['v1.2.3'], '..'), [
    { dir: 'v1.2.3', create: false },
    { dir: '..', create: true },
  ]);
  assert.deepEqual(dirChoices(['api'], 'a/b'), [{ dir: 'a/b', create: true }]);
});

test('templateOrder puts the default first', () => {
  assert.deepEqual(templateOrder(['dispatcher', 'solo', 'team-3'], 'solo'), ['solo', 'dispatcher', 'team-3']);
  assert.deepEqual(templateOrder(['a', 'b'], 'zzz'), ['a', 'b']);
  assert.deepEqual(templateOrder(['a', 'b'], null), ['a', 'b']);
});

test('listMove wraps', () => {
  assert.equal(listMove(0, 3, 1), 1);
  assert.equal(listMove(2, 3, 1), 0);
  assert.equal(listMove(0, 3, -1), 2);
  assert.equal(listMove(0, 0, 1), 0);
});

test('pickerKey maps digits, hjkl, extra keys and Esc', () => {
  const k = (code, extra = {}) => ({ code, ...extra });
  const all = { allowHjkl: true, extraKeys: ['\\', '-'] };
  assert.deepEqual(pickerKey(k('Digit3'), 5, all), { index: 2 });
  assert.deepEqual(pickerKey(k('Numpad2'), 5, all), { index: 1 });
  assert.equal(pickerKey(k('Digit7'), 5, all), null);
  assert.equal(pickerKey(k('Digit0'), 5, all), null);
  assert.deepEqual(pickerKey(k('KeyH'), 5, all), { dir: 'left' });
  assert.deepEqual(pickerKey(k('KeyJ'), 5, all), { dir: 'down' });
  assert.deepEqual(pickerKey(k('KeyK'), 5, all), { dir: 'up' });
  assert.deepEqual(pickerKey(k('KeyL', { altKey: true, ctrlKey: true }), 5, all), { dir: 'right' });
  assert.equal(pickerKey(k('KeyH'), 5, { allowHjkl: false }), null);
  assert.deepEqual(pickerKey(k('Backslash'), 5, all), { key: '\\' });
  assert.deepEqual(pickerKey(k('Minus'), 5, all), { key: '-' });
  assert.deepEqual(pickerKey(k('NumpadSubtract'), 5, all), { key: '-' });
  assert.equal(pickerKey(k('Minus'), 5, { allowHjkl: true }), null);
  assert.deepEqual(pickerKey(k('Escape'), 5, all), { cancel: true });
  assert.equal(pickerKey(k('ControlLeft'), 5, all), null);
  assert.equal(pickerKey(k('KeyX'), 5, all), null);
});

test('focusCommands un-maximizes what would hide the target', () => {
  const ws = fixture();
  assert.deepEqual(focusCommands(ws, 'g2', 't3', null), [{ type: 'setFocus', group: 'g2', tab: 't3', pane: null }]);
  ws.groupMaximized = 'g1';
  ws.tabs.t2.maximizedPaneId = 'p2';
  assert.deepEqual(focusCommands(ws, 'g1', 't2', 'p3'), [
    { type: 'setMaximized', tab: 't2', pane: null },
    { type: 'setFocus', group: 'g1', tab: 't2', pane: 'p3' },
  ]);
  assert.deepEqual(focusCommands(ws, 'g2', 't3', 'p5'), [
    { type: 'setGroupMaximized', group: null },
    { type: 'setFocus', group: 'g2', tab: 't3', pane: 'p5' },
  ]);
});

test('pickCommands: pane.pick → SetFocus by id or direction', () => {
  const ws = fixture();
  assert.deepEqual(pickCommands('pane', ws, { id: 'p5' }, geo), [{ type: 'setFocus', group: 'g2', tab: 't3', pane: 'p5' }]);
  assert.deepEqual(pickCommands('pane', ws, { dir: 'right' }, geo), [{ type: 'setFocus', group: 'g1', tab: 't2', pane: 'p3' }]);
  assert.deepEqual(pickCommands('pane', ws, { dir: 'left' }, geo), []);
  assert.deepEqual(pickCommands('pane', ws, { id: 'p2' }, geo), []); // already focused
  assert.deepEqual(pickCommands('pane', ws, { id: 'nope' }, geo), []);
});

test('pickCommands: tab.moveTo → MoveTab (number/hjkl) or SplitGroupWith (\\ -)', () => {
  const ws = fixture();
  assert.deepEqual(pickCommands('moveTo', ws, { id: 'g2' }, geo), [{ type: 'moveTab', tab: 't2', group: 'g2', index: 1 }]);
  assert.deepEqual(pickCommands('moveTo', ws, { dir: 'right' }, geo), [{ type: 'moveTab', tab: 't2', group: 'g2', index: 1 }]);
  assert.deepEqual(pickCommands('moveTo', ws, { id: 'g1' }, geo), []); // own group
  assert.deepEqual(pickCommands('moveTo', ws, { dir: 'left' }, geo), []);
  assert.deepEqual(pickCommands('moveTo', ws, { key: '\\' }, geo), [
    { type: 'splitGroupWith', tab: 't2', group: 'g1', dir: 'horizontal', before: false },
  ]);
  assert.deepEqual(pickCommands('moveTo', ws, { key: '-' }, geo), [
    { type: 'splitGroupWith', tab: 't2', group: 'g1', dir: 'vertical', before: false },
  ]);
});

test('pickCommands: group.mergeInto → MergeGroup{src: focused, dst}', () => {
  const ws = fixture();
  assert.deepEqual(pickCommands('merge', ws, { id: 'g2' }, geo), [{ type: 'mergeGroup', src: 'g1', dst: 'g2' }]);
  assert.deepEqual(pickCommands('merge', ws, { dir: 'right' }, geo), [{ type: 'mergeGroup', src: 'g1', dst: 'g2' }]);
  assert.deepEqual(pickCommands('merge', ws, { id: 'g1' }, geo), []);
});

test('every default keybinding has a Korean label', () => {
  const kb = JSON.parse(readFileSync(new URL('../../src-tauri/defaults/keybindings.json', import.meta.url), 'utf8'));
  for (const id of Object.keys(kb)) {
    const label = commandLabel(id);
    assert.notEqual(label, id, id);
    assert.match(label, /[가-힣]/, id);
  }
});

test('paletteItems: commands with keys, navigation, moves, merges, specials', () => {
  const ws = fixture();
  const bindings = { 'pane.splitRight': 'Alt+\\', 'app.palette': 'Alt+;', 'pane.copyMode': 'Alt+c', 'pane.breakToTab': null };
  const has = (id) => id !== 'pane.copyMode';
  const items = paletteItems(ws, { bindings, has });
  const by = (label) => items.find((i) => i.label === label);

  const split = by(commandLabel('pane.splitRight'));
  assert.deepEqual(split, { label: commandLabel('pane.splitRight'), hint: 'Alt+\\', action: { type: 'command', id: 'pane.splitRight' } });
  assert.equal(by(commandLabel('app.palette')), undefined); // the palette itself is not listed
  assert.equal(by(commandLabel('pane.copyMode')), undefined); // no handler yet → hidden
  assert.equal(items.filter((i) => i.action.id === 'pane.breakToTab').length, 0); // has its own entry

  assert.deepEqual(by('탭으로 이동: ha').action, { type: 'ws', cmds: [{ type: 'setFocus', group: 'g2', tab: 't3', pane: null }] });
  assert.deepEqual(by('창으로 이동: api/pane-1').action, { type: 'ws', cmds: [{ type: 'setFocus', group: 'g1', tab: 't2', pane: 'p3' }] });
  assert.deepEqual(by('탭 이동 → work').action, { type: 'ws', cmds: [{ type: 'moveTab', tab: 't2', group: 'g2', index: 1 }] });
  assert.equal(by('탭 이동 → control'), undefined); // own group
  assert.deepEqual(by('그룹 합치기 → work').action, { type: 'ws', cmds: [{ type: 'mergeGroup', src: 'g1', dst: 'g2' }] });
  assert.deepEqual(by('탭 이름 바꾸기').action, { type: 'rename', tab: 't2' });
  assert.deepEqual(by('현재 창을 새 탭으로 떼어내기').action, { type: 'ws', cmds: [{ type: 'breakPaneToTab', pane: 'p2' }] });
  assert.deepEqual(by('설정 오류 보기').action, { type: 'errors' });
  assert.deepEqual(by('사이드바 토글').action, { type: 'sidebar' });

  // the Dispatcher tab cannot be renamed → no rename entry while it is focused
  ws.groups.g1.activeTabId = 't1';
  assert.equal(paletteItems(ws, { bindings, has }).find((i) => i.label === '탭 이름 바꾸기'), undefined);
});

// ---- new-tab wizard flow (UI injected) ----

function fakeDeps(script) {
  const log = [];
  const d = {
    log,
    alive: () => true,
    listDirs: async () => script.dirs || ['api', 'web'],
    listTemplates: async () => ['dispatcher', 'solo', 'team-3'],
    defaultTemplate: async () => 'solo',
    pickDir: async (dirs) => {
      log.push(['pickDir', dirs]);
      return script.dirPicks.shift() ?? null;
    },
    confirm: async (msg) => {
      log.push(['confirm', msg]);
      return script.confirms.shift() ?? false;
    },
    createDir: async (name) => {
      log.push(['createDir', name]);
      if (script.createDirError) throw script.createDirError;
    },
    pickTemplate: async (list, def, dir) => {
      log.push(['pickTemplate', list]);
      d.templateArgs = [def, dir];
      return script.templates.shift() ?? null;
    },
    createTab: async (cmd) => {
      log.push(['createTab', cmd]);
      return script.creates.shift() ?? true;
    },
    toast: (m) => log.push(['toast', m]),
  };
  return d;
}

test('newTabFlow: existing dir → template → CreateTab{name: dir}', async () => {
  const d = fakeDeps({ dirPicks: [{ dir: 'api', create: false }], confirms: [], templates: ['solo'], creates: [true] });
  assert.deepEqual(await newTabFlow(d), { dir: 'api', template: 'solo' });
  assert.deepEqual(d.templateArgs, ['solo', 'api']); // default hint + chosen dir for the step ② title
  assert.deepEqual(d.log, [
    ['pickDir', ['api', 'web']],
    ['pickTemplate', ['solo', 'dispatcher', 'team-3']],
    ['createTab', { type: 'createTab', name: 'api', dir: 'api', template: 'solo', group: null }],
  ]);
});

test('newTabFlow: unknown name → confirm → createDir', async () => {
  const d = fakeDeps({ dirPicks: [{ dir: 'newproj', create: true }], confirms: [true], templates: ['team-3'], creates: [true] });
  assert.deepEqual(await newTabFlow(d), { dir: 'newproj', template: 'team-3' });
  assert.deepEqual(d.log.slice(1, 3), [
    ['confirm', "'newproj' 디렉토리를 만들까요?"],
    ['createDir', 'newproj'],
  ]);
});

test('newTabFlow: declined / invalid / failed creation and duplicate names go back to step 1', async () => {
  const d = fakeDeps({
    dirPicks: [{ dir: 'x', create: true }, { dir: 'a/b', create: true }, { dir: 'api', create: false }],
    confirms: [false],
    templates: ['solo'],
    creates: [false], // duplicate name: the backend error was toasted by createTab
  });
  assert.equal(await newTabFlow(d), null); // 4th pickDir → Esc
  const kinds = d.log.map((e) => e[0]);
  assert.deepEqual(kinds, ['pickDir', 'confirm', 'pickDir', 'toast', 'pickDir', 'pickTemplate', 'createTab', 'pickDir']);
  assert.ok(!kinds.includes('createDir'));

  const e = fakeDeps({ dirPicks: [{ dir: 'x', create: true }], confirms: [true], templates: [], creates: [], createDirError: '이미 있는 폴더입니다: x' });
  assert.equal(await newTabFlow(e), null);
  assert.deepEqual(e.log.map((x) => x[0]), ['pickDir', 'confirm', 'createDir', 'toast', 'pickDir']);
  assert.deepEqual(e.log[3], ['toast', '이미 있는 폴더입니다: x']);
});

test('newTabFlow: Esc at the template step cancels; a closed overlay stops the flow', async () => {
  const d = fakeDeps({ dirPicks: [{ dir: 'api', create: false }], confirms: [], templates: [null], creates: [] });
  assert.equal(await newTabFlow(d), null);
  assert.ok(!d.log.some((e) => e[0] === 'createTab'));

  const f = fakeDeps({ dirPicks: [{ dir: 'n', create: true }], confirms: [false], templates: [], creates: [] });
  // a tab switch closed the confirm (it resolved false) → alive() is false from then on
  f.alive = () => !f.log.some((e) => e[0] === 'confirm');
  assert.equal(await newTabFlow(f), null);
  assert.equal(f.log.filter((e) => e[0] === 'pickDir').length, 1);
});

// ---- startup missing-dir flow ----

test('groupMissing merges tabs that share a dir', () => {
  assert.deepEqual(groupMissing([['t2', 'api'], ['t3', 'web'], ['t4', 'api']]), [
    { dir: 'api', tabs: ['t2', 't4'] },
    { dir: 'web', tabs: ['t3'] },
  ]);
});

test('missingDirsFlow: yes → createDir + RestartPane per pane; no → left failed', async () => {
  const log = [];
  const answers = [true, false];
  await missingDirsFlow({
    missingDirs: async () => [['t2', 'api'], ['t3', 'web']],
    alive: () => true,
    confirm: async (m) => (log.push(['confirm', m]), answers.shift()),
    createDir: async (n) => log.push(['createDir', n]),
    panesOf: (t) => ({ t2: ['p2', 'p3'], t3: ['p5'] })[t],
    restart: async (ids) => log.push(['restart', ids]),
    toast: (m) => log.push(['toast', m]),
  });
  assert.deepEqual(log, [
    ['confirm', "'api' 디렉토리가 없습니다. 만들까요?"],
    ['createDir', 'api'],
    ['restart', ['p2', 'p3']],
    ['confirm', "'web' 디렉토리가 없습니다. 만들까요?"],
  ]);
});

test('missingDirsFlow: createDir error is toasted and the panes are not restarted', async () => {
  const log = [];
  await missingDirsFlow({
    missingDirs: async () => [['t2', 'a/b']],
    alive: () => true,
    confirm: async () => true,
    createDir: async () => {
      throw '폴더 이름에 / \\ : 는 쓸 수 없습니다';
    },
    panesOf: () => ['p2'],
    restart: async (ids) => log.push(['restart', ids]),
    toast: (m) => log.push(['toast', m]),
  });
  assert.deepEqual(log, [['toast', '폴더 이름에 / \\ : 는 쓸 수 없습니다']]);
});

test('missingDirsFlow with alive: () => true keeps asking after a cancelled question (R19)', async () => {
  const log = [];
  // the first confirm was closed by a tab switch (resolves false); the flow continues
  const answers = [false, true];
  await missingDirsFlow({
    missingDirs: async () => [['t2', 'api'], ['t3', 'web']],
    alive: () => true,
    confirm: async (m) => (log.push(['confirm', m]), answers.shift()),
    createDir: async (n) => log.push(['createDir', n]),
    panesOf: (t) => ({ t2: ['p2'], t3: ['p5'] })[t],
    restart: async (ids) => log.push(['restart', ids]),
    toast: (m) => log.push(['toast', m]),
  });
  assert.deepEqual(log, [
    ['confirm', "'api' 디렉토리가 없습니다. 만들까요?"],
    ['confirm', "'web' 디렉토리가 없습니다. 만들까요?"],
    ['createDir', 'web'],
    ['restart', ['p5']],
  ]);
});

test('app.js passes alive: () => true to the startup missing-dir flow (R19)', () => {
  const src = readFileSync(new URL('../../src/app.js', import.meta.url), 'utf8');
  const call = src.slice(src.indexOf('missingDirsFlow({'), src.indexOf('missingDirsFlow({') + 200);
  assert.match(call, /alive: \(\) => true/);
});

test('closeFlow: no running agents exits without asking', async () => {
  const calls = [];
  const ok = await closeFlow({ busy: false }, {
    runningCount: async () => 0,
    confirm: async () => { calls.push('confirm'); return true; },
    exit: async () => { calls.push('exit'); },
  });
  assert.equal(ok, true);
  assert.deepEqual(calls, ['exit']);
});

test('closeFlow: asks with the count; yes exits, no stays', async () => {
  const msgs = [];
  let exited = 0;
  const mk = (answer) => ({
    runningCount: async () => 3,
    confirm: async (m) => { msgs.push(m); return answer; },
    exit: async () => { exited++; },
  });
  const st = { busy: false };
  assert.equal(await closeFlow(st, mk(false)), false);
  assert.equal(exited, 0);
  assert.equal(st.busy, false);
  assert.equal(await closeFlow(st, mk(true)), true);
  assert.equal(exited, 1);
  assert.deepEqual(msgs, ['실행 중인 에이전트 3개를 종료하고 닫을까요?', '실행 중인 에이전트 3개를 종료하고 닫을까요?']);
});

test('closeFlow: a second request while the dialog is open is ignored; busy clears after a failure', async () => {
  const st = { busy: false };
  let release;
  const deps = {
    runningCount: async () => 2,
    confirm: () => new Promise((r) => (release = r)),
    exit: async () => {},
  };
  const first = closeFlow(st, deps);
  await Promise.resolve(); await Promise.resolve();
  assert.equal(await closeFlow(st, deps), false);
  release(false);
  assert.equal(await first, false);
  await assert.rejects(closeFlow(st, { ...deps, runningCount: async () => 0, exit: async () => { throw new Error('x'); } }));
  assert.equal(st.busy, false);
});

test('confirmKey: Y yes, N/Esc no; Enter is yes by default, ignored with enterYes:false', () => {
  const k = (code, extra = {}) => ({ code, repeat: false, ...extra });
  assert.equal(confirmKey(k('KeyY')), true);
  assert.equal(confirmKey(k('KeyN')), false);
  assert.equal(confirmKey(k('Escape')), false);
  assert.equal(confirmKey(k('Enter')), true);
  assert.equal(confirmKey(k('NumpadEnter')), true);
  assert.equal(confirmKey(k('KeyA')), null);
  assert.equal(confirmKey(k('KeyY', { repeat: true })), null);
  const strict = { enterYes: false };
  assert.equal(confirmKey(k('Enter'), strict), null);
  assert.equal(confirmKey(k('NumpadEnter'), strict), null);
  assert.equal(confirmKey(k('KeyY'), strict), true);
  assert.equal(confirmKey(k('KeyN'), strict), false);
});

test('closeFlow: the exit confirm does not accept Enter (spec §8: Y/N)', async () => {
  const opts = [];
  await closeFlow({ busy: false }, {
    runningCount: async () => 1,
    confirm: async (_m, o) => { opts.push(o); return false; },
    exit: async () => {},
  });
  assert.deepEqual(opts, [{ enterYes: false }]);
});

// ---- Task 12: role choice, autostart entry, environment status ----

test('newTabFlow: role Manager → createManager (config-persisting), no template step', async () => {
  const d = fakeDeps({ dirPicks: [{ dir: 'billing', create: false }], confirms: [], templates: [], creates: [] });
  d.pickRole = async (dir) => {
    d.log.push(['pickRole', dir]);
    return 'manager';
  };
  d.createManager = async (req) => {
    d.log.push(['createManager', req]);
    return true;
  };
  assert.deepEqual(await newTabFlow(d), { dir: 'billing', role: 'manager' });
  assert.deepEqual(d.log, [
    ['pickDir', ['api', 'web']],
    ['pickRole', 'billing'],
    ['createManager', { name: 'billing', dir: 'billing' }],
  ]);
});

test('newTabFlow: role 일반 → template → CreateTab; Esc at the role step cancels; failed Manager goes back', async () => {
  const d = fakeDeps({ dirPicks: [{ dir: 'api', create: false }], confirms: [], templates: ['solo'], creates: [true] });
  d.pickRole = async () => 'plain';
  d.createManager = async () => assert.fail('no manager');
  assert.deepEqual(await newTabFlow(d), { dir: 'api', template: 'solo' });
  assert.deepEqual(d.log.map((e) => e[0]), ['pickDir', 'pickTemplate', 'createTab']);

  const e = fakeDeps({ dirPicks: [{ dir: 'api', create: false }], confirms: [], templates: [], creates: [] });
  e.pickRole = async () => null;
  assert.equal(await newTabFlow(e), null);
  assert.ok(!e.log.some((x) => x[0] === 'pickTemplate' || x[0] === 'createTab'));

  const f = fakeDeps({ dirPicks: [{ dir: 'api', create: false }], confirms: [], templates: [], creates: [] });
  f.pickRole = async () => 'manager';
  f.createManager = async (req) => {
    f.log.push(['createManager', req]);
    return false; // the error was toasted
  };
  assert.equal(await newTabFlow(f), null); // back to step 1, then Esc
  assert.deepEqual(f.log.map((x) => x[0]), ['pickDir', 'createManager', 'pickDir']);
});

test('paletteItems: autostart entry shows the current state', () => {
  const ws = fixture();
  const find = (opts) => paletteItems(ws, { bindings: {}, ...opts }).find((i) => i.action.type === 'autostart');
  assert.deepEqual(find({ autostart: true }), { label: '로그인 시 자동 실행 끄기', hint: '현재: 켜짐', action: { type: 'autostart', on: false } });
  assert.deepEqual(find({ autostart: false }), { label: '로그인 시 자동 실행 켜기', hint: '현재: 꺼짐', action: { type: 'autostart', on: true } });
  assert.equal(find({}), undefined); // state unknown → no entry
});

test('envStatusFlag: failing items only, skipped marked, required highlighted', () => {
  const it = (id, ok, extra = {}) => ({ id, ok, detail: `${id} 상세`, fix: `${id} 조치`, skipped: false, required: false, ...extra });
  assert.equal(envStatusFlag(null), null);
  assert.equal(envStatusFlag([it('claude-version', true), it('git-bash', true)]), null);

  const skipped = envStatusFlag([it('claude-version', true), it('git-bash', false, { skipped: true })]);
  assert.equal(skipped.text, '[환경: Git 없음(건너뜀)]');
  assert.equal(skipped.highlight, false);

  const f = envStatusFlag([
    it('claude-version', false),
    it('git-bash', false, { skipped: true }),
    it('folder-trust:billing', false),
    it('holla', false, { required: true }),
  ]);
  assert.equal(f.text, '[환경: Claude 버전, Git 없음(건너뜀), billing 폴더 신뢰, holla 없음]');
  assert.equal(f.highlight, true);
  assert.match(f.title, /holla: holla 상세 — 조치: holla 조치/);
  // only an optional failing item → not highlighted
  assert.equal(envStatusFlag([it('opt', false)]).highlight, false);
  // labels
  assert.equal(envItemLabel('git-bash'), 'Git 없음');
  assert.equal(envItemLabel('folder-trust:a'), 'a 폴더 신뢰');
  assert.equal(envItemLabel('x'), 'x 없음');
});

// ---- settings reload (config_reload, no restart) ----
import { reloadReportText, reloadErrorLines, reloadFlow } from '../../src/overlay-logic.js';

test('paletteItems: "설정 다시 읽기" entry', () => {
  const it = paletteItems(fixture(), {}).find((i) => i.action.type === 'reloadConfig');
  assert.deepEqual(it, { label: '설정 다시 읽기', hint: '재시작 없이', action: { type: 'reloadConfig' } });
  // also while the Dispatcher tab is focused (no tab-specific condition)
  const ws = fixture();
  ws.groups.g1.activeTabId = 't1';
  assert.ok(paletteItems(ws, {}).some((i) => i.action.type === 'reloadConfig'));
});

test('reloadReportText: added / kept tabs and updated prompts, empty parts left out', () => {
  assert.equal(
    reloadReportText({ added: ['a', 'b'], keptMissing: ['c'], promptsUpdated: 2 }),
    '설정을 다시 읽었습니다 — 새 탭: a, b · 닫지 않은 탭: c · 시작 프롬프트 갱신 2',
  );
  assert.equal(reloadReportText({ added: ['handover'], keptMissing: [], promptsUpdated: 0 }), '설정을 다시 읽었습니다 — 새 탭: handover');
  assert.equal(reloadReportText({ added: [], keptMissing: ['old'], promptsUpdated: 0 }), '설정을 다시 읽었습니다 — 닫지 않은 탭: old');
  assert.equal(reloadReportText({ added: [], keptMissing: [], promptsUpdated: 1 }), '설정을 다시 읽었습니다 — 시작 프롬프트 갱신 1');
  assert.equal(reloadReportText({ added: [], keptMissing: [], promptsUpdated: 0 }), '설정을 다시 읽었습니다');
  assert.equal(reloadReportText(null), '설정을 다시 읽었습니다');
  assert.equal(reloadReportText({ added: 'x' }), '설정을 다시 읽었습니다'); // malformed parts ignored
});

test('reloadErrorLines: the backend\'s error list under a "nothing changed" line', () => {
  const head = '설정에 오류가 있어 다시 읽지 않았습니다 — 아무것도 바뀌지 않았습니다';
  assert.deepEqual(reloadErrorLines(['config.json:3:5 — 원인', 'templates/x.json:1:1 — 원인2']), [head, 'config.json:3:5 — 원인', 'templates/x.json:1:1 — 원인2']);
  assert.deepEqual(reloadErrorLines('설정을 읽을 수 없습니다'), [head, '설정을 읽을 수 없습니다']);
  assert.deepEqual(reloadErrorLines(new Error('IPC 실패')), [head, 'IPC 실패']);
  assert.deepEqual(reloadErrorLines([]), [head]);
});

function reloadDeps({ fail = null, keysFail = false } = {}) {
  const log = [];
  return {
    log,
    reload: async () => {
      log.push(['reload']);
      if (fail) throw fail;
      return { added: ['handover'], keptMissing: [], promptsUpdated: 1 };
    },
    keybindings: async () => {
      log.push(['keybindings']);
      if (keysFail) throw new Error('키 설정을 읽지 못했습니다');
      return { 'pane.focusLeft': 'Alt+h' };
    },
    setKeymap: (b) => log.push(['setKeymap', b]),
    toast: (m, kind) => log.push(['toast', String(m), kind]),
    showErrors: (lines) => log.push(['showErrors', lines]),
  };
}

test('reloadFlow: success toasts the report and refreshes the keymap', async () => {
  const d = reloadDeps();
  assert.equal(await reloadFlow(d), true);
  assert.deepEqual(d.log, [
    ['reload'],
    ['toast', '설정을 다시 읽었습니다 — 새 탭: handover · 시작 프롬프트 갱신 1', 'info'],
    ['keybindings'],
    ['setKeymap', { 'pane.focusLeft': 'Alt+h' }],
  ]);
  // the keymap could not be re-read: the reload still succeeded, the problem is toasted
  const k = reloadDeps({ keysFail: true });
  assert.equal(await reloadFlow(k), true);
  assert.deepEqual(k.log.at(-1), ['toast', 'Error: 키 설정을 읽지 못했습니다', undefined]);
  assert.ok(!k.log.some((e) => e[0] === 'setKeymap'));
});

test('app.js wires "설정 다시 읽기" to reloadFlow and re-reads keys on config-reloaded', () => {
  const src = readFileSync(new URL('../../src/app.js', import.meta.url), 'utf8');
  assert.match(src, /case 'reloadConfig':\s*return reloadSettings\(\);/);
  const flow = src.slice(src.indexOf('return reloadFlow({'), src.indexOf('return reloadFlow({') + 260);
  for (const dep of ['api.configReload()', 'api.keybindings()', 'setKeymap: applyKeybindings', 'showErrors']) assert.ok(flow.includes(dep), dep);
  const ev = src.slice(src.indexOf("api.on('config-reloaded'"), src.indexOf("api.on('config-reloaded'") + 120);
  assert.match(ev, /loadKeybindings\(\)/);
});

test('reloadFlow: errors go to the error panel and nothing else happens', async () => {
  const d = reloadDeps({ fail: ['config.json:2:1 — 잘못된 값'] });
  assert.equal(await reloadFlow(d), false);
  assert.deepEqual(d.log, [['reload'], ['showErrors', ['설정에 오류가 있어 다시 읽지 않았습니다 — 아무것도 바뀌지 않았습니다', 'config.json:2:1 — 잘못된 값']]]);
});

// ---- ④ RF1: palette "현재 탭 닫기" (remove-tab spec, R2 a · R3 a)
import { closeTabInfo, closeTabFlow } from '../../src/overlay-logic.js';

test('paletteItems: close-tab entry except on the dispatcher tab', () => {
  const ws = fixture(); // focused: g1 → t2 (api)
  assert.deepEqual(paletteItems(ws, {}).find((i) => i.action.type === 'closeTab'), {
    label: '현재 탭 닫기',
    hint: 'Y/N 확인',
    action: { type: 'closeTab', tab: 't2' },
  });
  ws.groups.g1.activeTabId = 't1'; // the Dispatcher tab cannot be closed
  assert.equal(paletteItems(ws, {}).find((i) => i.action.type === 'closeTab'), undefined);
});

test('closeTabInfo: name, role and running panes of the tab', () => {
  const ws = fixture();
  ws.panes.p2.status = { state: 'running' };
  ws.panes.p3.status = { state: 'exited', code: 0 };
  ws.panes.p4.status = { state: 'running' };
  ws.tabs.t2.role = 'manager';
  assert.deepEqual(closeTabInfo(ws, 't2'), { tab: 't2', name: 'api', role: 'manager', running: 2 });
  assert.deepEqual(closeTabInfo(ws, 't3'), { tab: 't3', name: 'ha', role: null, running: 0 });
  assert.equal(closeTabInfo(ws, 't1'), null, 'the Dispatcher tab');
  assert.equal(closeTabInfo(ws, 'nope'), null);
});

function closeDeps({ answer = true, error = null } = {}) {
  const log = [];
  return {
    log,
    confirm: async (msg, opts) => {
      log.push(['confirm', msg, opts]);
      return answer;
    },
    close: async (tab) => {
      log.push(['close', tab]);
      if (error) throw error;
    },
    showErrors: (lines) => log.push(['showErrors', lines]),
  };
}

test('closeTabFlow: Y/N with Enter ignored, then CloseTab', async () => {
  const d = closeDeps();
  assert.equal(await closeTabFlow({ tab: 't2', name: 'billing', role: 'manager', running: 3 }, d), true);
  const [kind, msg, opts] = d.log[0];
  assert.equal(kind, 'confirm');
  assert.deepEqual(opts, { enterYes: false });
  assert.ok(msg.startsWith("'billing' 상시업무(Manager) 탭을 닫을까요?"), msg);
  for (const w of ['실행 중인 창 3개', 'config.json', '끝나지 않은 인박스 항목', '진행 중인 팀']) assert.ok(msg.includes(w), w);
  assert.deepEqual(d.log[1], ['close', 't2']);
  // handover (R3 a: only this path) and a plain tab: shorter questions
  const h = closeDeps();
  await closeTabFlow({ tab: 't9', name: 'handover', role: 'handover', running: 1 }, h);
  assert.ok(h.log[0][1].startsWith("'handover' 탭을 닫을까요?") && h.log[0][1].includes('config.json'));
  const p = closeDeps();
  await closeTabFlow({ tab: 't3', name: 'ha', role: null, running: 0 }, p);
  assert.equal(p.log[0][1], "'ha' 탭을 닫을까요?");
  // N: nothing is sent
  const n = closeDeps({ answer: false });
  assert.equal(await closeTabFlow({ tab: 't2', name: 'billing', role: 'manager', running: 1 }, n), false);
  assert.deepEqual(n.log.map((e) => e[0]), ['confirm']);
});

test('closeTabFlow: a refusal is shown in the error panel and nothing else is sent', async () => {
  const refusal = "'billing' 상시업무를 닫지 않았습니다 — 끝나지 않은 인박스 항목 2개(새 1·진행 중 1). 항목을 완료(종료 확인)한 뒤 다시 시도하세요.";
  const d = closeDeps({ error: refusal });
  assert.equal(await closeTabFlow({ tab: 't2', name: 'billing', role: 'manager', running: 1 }, d), false);
  assert.deepEqual(d.log.slice(1), [['close', 't2'], ['showErrors', [refusal]]]);
  const e = closeDeps({ error: new Error('창을 찾을 수 없습니다') });
  await closeTabFlow({ tab: 't2', name: 'x', role: null, running: 0 }, e);
  assert.deepEqual(e.log.at(-1), ['showErrors', ['창을 찾을 수 없습니다']]);
});

test('app.js sends "현재 탭 닫기" without the toast path so a refusal reaches the error panel', () => {
  const src = readFileSync(new URL('../../src/app.js', import.meta.url), 'utf8');
  assert.match(src, /case 'closeTab':\s*return closeCurrentTab\(a\.tab\);/);
  const fn = src.slice(src.indexOf('function closeCurrentTab('), src.indexOf('function closeCurrentTab(') + 700);
  for (const w of ['closeTabFlow(', 'closeTabInfo(', "api.send({ type: 'closeTab'", 'showErrors']) assert.ok(fn.includes(w), w);
});
