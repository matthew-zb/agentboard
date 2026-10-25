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
