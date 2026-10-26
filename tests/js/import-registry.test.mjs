import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, statSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { importRegistry, DISPATCHER_NOTICE, MANAGER_NOTICE, HOLLA_REQUIRE } from '../../scripts/import-registry.mjs';

const SCRIPT = fileURLToPath(new URL('../../scripts/import-registry.mjs', import.meta.url));
const DEFAULTS = fileURLToPath(new URL('../../src-tauri/defaults', import.meta.url));

// shape of C:\workspace\claude\session-board\registry.json (regular[] / adhoc[]), values made up
const registry = (root = 'C:/workspace') => ({
  regular: [
    { role: '디스패처', dir: root, session: 'workspace-07', note: '형님 대화창', startupPrompt: '생존 확인 보고', lastSessionId: 'a' },
    { role: 'RTCmod', dir: `${root}/RTCmod`, session: 'RTCmod', note: 'RTC 모듈', lastSessionId: 'b' },
    { role: 'ha', dir: `${root}/ha`, session: 'ha', note: '월패드', startupPrompt: 'HA intake 확인 루프를 시작하라', lastSessionId: 'c' },
    { role: '중첩', dir: `${root}/srcs/agentboard`, session: 'abdev', note: '중첩 폴더' },
    { role: '한중번역', dir: 'C:/tools/wechat-group-monitor-mcp', session: 'translator-cn', note: '루트 밖', startupPrompt: '번역 루프' },
    { role: 'Handover 리뷰', dir: root, session: 'handover', note: '단일 작성자', startupPrompt: '/loop 10m …' },
  ],
  adhoc: [{ role: 'tmp', dir: `${root}/RTCmod`, session: 'RTCmod-tmp', note: '임시' }],
});

const run = (only, opts = {}) => importRegistry(registry(opts.reg), { only, workspaceRoot: opts.root ?? 'C:/workspace', defaultsDir: DEFAULTS });

test('regular sessions picked with --only become manager tabs with root-relative dirs', () => {
  const r = run(['RTCmod', 'abdev']);
  assert.deepEqual(r.config.tabs, [
    { name: 'RTCmod', role: 'manager', dir: 'RTCmod', template: 'manager' },
    { name: 'abdev', role: 'manager', dir: 'srcs/agentboard', template: 'manager' },
  ]);
  // a backslash root and different letter case name the same folder (Windows)
  const b = run(['RTCmod'], { root: 'c:\\Workspace\\' });
  assert.deepEqual(b.config.tabs.map((t) => t.dir), ['RTCmod']);
});

test('only the --only sessions are imported; dispatcher and handover never become tabs', () => {
  const r = run(['workspace-07', 'handover', 'RTCmod']);
  assert.deepEqual(r.config.tabs.map((t) => t.name), ['RTCmod']);
  assert.deepEqual(r.config.dispatcher, { agent: 'claude', template: 'dispatcher-inbox' });
  assert.ok(r.warnings.some((w) => w.includes('workspace-07') && w.includes('디스패처')));
  assert.ok(r.warnings.some((w) => w.includes("'handover'") && w.includes('AgentBoard 밖') && !w.includes('psmux')));
  assert.ok(!r.config.tabs.some((t) => t.role === 'handover'));
  // nothing chosen → no tabs, a warning, a config with the dispatcher only
  const none = run([]);
  assert.deepEqual(none.config.tabs, []);
  assert.deepEqual(none.config.groups, { group: { name: 'control', tabs: ['dispatcher'] } });
  assert.ok(none.warnings.some((w) => w.includes('옮길 탭이 없습니다')));
});

test('a dir outside workspaceRoot is skipped with a warning; unknown and adhoc names too', () => {
  const r = run(['translator-cn', 'nope', 'RTCmod-tmp', 'RTCmod']);
  assert.deepEqual(r.config.tabs.map((t) => t.name), ['RTCmod']);
  assert.ok(r.warnings.some((w) => w.includes('translator-cn') && w.includes('C:/tools/wechat-group-monitor-mcp') && w.includes('밖')));
  assert.ok(r.warnings.some((w) => w.includes("'nope'") && w.includes('없는 세션')));
  assert.ok(r.warnings.some((w) => w.includes("'RTCmod-tmp'") && w.includes('임시')));
  assert.ok(r.warnings.some((w) => w.includes('새 대화로 시작')), 'lastSessionId is not resumed (mapping #8)');
});

test('startupPrompt gives the tab its own template: the manager template with that prompt', () => {
  const r = run(['ha', 'RTCmod']);
  assert.deepEqual(r.config.tabs.find((t) => t.name === 'ha'), { name: 'ha', role: 'manager', dir: 'ha', template: 'ha-start' });
  assert.deepEqual(Object.keys(r.templates), ['ha-start']);
  const base = JSON.parse(readFileSync(join(DEFAULTS, 'templates', 'manager.json'), 'utf8'));
  const t = r.templates['ha-start'];
  assert.equal(t.name, 'ha-start');
  assert.equal(t.layout.slot, base.layout.slot, 'team slot kept');
  const panes = [];
  const walk = (n) => (n.pane ? panes.push(n.pane) : (n.children || []).forEach(walk));
  walk(t.layout);
  assert.equal(panes.find((p) => p.name === 'manager').prompt, 'HA intake 확인 루프를 시작하라');
  assert.deepEqual(panes.find((p) => p.name === 'shell'), { name: 'shell', agent: 'powershell' }, 'other panes unchanged');
});

test('config: control | work groups, nightlyRefresh null, holla envRequire with the full path', () => {
  const { config } = run(['RTCmod', 'ha']);
  assert.deepEqual(config.groups, {
    split: 'horizontal',
    sizes: [35, 65],
    children: [{ group: { name: 'control', tabs: ['dispatcher'] } }, { group: { name: 'work', tabs: ['RTCmod', 'ha'] } }],
  });
  assert.equal(config.defaultGroup, 'work');
  assert.ok('nightlyRefresh' in config);
  assert.equal(config.nightlyRefresh, null);
  assert.deepEqual(config.envRequire, [
    {
      id: 'holla',
      check: 'C:/workspace/holla-cli/02.src/dist/holla.exe slack version --workspace zigbang',
      fix: 'C:\\workspace\\CLAUDE.md 「holla CLI」 절 참조',
      why: '슬랙 확인',
      required: true,
      source: '',
    },
  ]);
  assert.deepEqual(config.envRequire[0], HOLLA_REQUIRE);
  assert.equal(config.roles, undefined, 'no roles key needed: the files go to .agentboard/roles/');
});

test('role files: the defaults plus the coexistence notice, within the cmd line limit', () => {
  const { roles } = run(['RTCmod']);
  const base = (n) => readFileSync(join(DEFAULTS, 'roles', `${n}.md`), 'utf8');
  assert.deepEqual(Object.keys(roles).sort(), ['dispatcher', 'manager']);
  assert.ok(roles.dispatcher.startsWith(base('dispatcher').trimEnd()));
  assert.ok(roles.dispatcher.includes(DISPATCHER_NOTICE) && DISPATCHER_NOTICE.includes('workspace-07'));
  assert.ok(roles.manager.startsWith(base('manager').trimEnd()));
  assert.ok(roles.manager.includes(MANAGER_NOTICE));
  for (const w of ['progress.md', 'handover-inbox', 'SendMessage', 'handover-single-writer.md', 'inbox_post', '「holla CLI」']) {
    assert.ok(MANAGER_NOTICE.includes(w), w);
  }
  // the same limits as the built-in prompts (agentcfg.rs default_role_prompts_..._fit_a_cmd_line):
  // claude.cmd flattens a prompt to one line (newline → " ⏎ ") and changes "%^&|<>
  for (const [n, md] of Object.entries(roles)) {
    const flat = md.replace(/\r\n/g, '\n').replace(/[\n\r]/g, ' ⏎ ');
    assert.ok([...flat].length <= 4000, `${n}: ${[...flat].length} chars after flattening`);
  }
  for (const notice of [DISPATCHER_NOTICE, MANAGER_NOTICE]) assert.ok(!/["%^&|<>]/.test(notice), notice);
});

const files = (dir) =>
  readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? [p, ...files(p)] : [p];
  });

test('CLI is a dry run: prints the planned files and writes nothing', () => {
  const root = mkdtempSync(join(tmpdir(), 'ab-import-'));
  try {
    mkdirSync(join(root, 'RTCmod'));
    const reg = join(root, 'registry.json');
    writeFileSync(reg, JSON.stringify(registry(root.replace(/\\/g, '/'))));
    const before = files(root).sort();
    const out = execFileSync(process.execPath, [SCRIPT, reg, '--only', 'RTCmod,ha', '--root', root], { cwd: root, encoding: 'utf8' });
    assert.deepEqual(files(root).sort(), before, 'no file or folder created');
    assert.ok(out.includes('dry-run'));
    assert.ok(!/psmux/i.test(out), 'AgentBoard does not use psmux: no file or message mentions it');
    assert.ok(out.includes(join(root, '.agentboard', 'config.json')));
    assert.ok(out.includes('"nightlyRefresh": null'));
    assert.ok(out.includes(join(root, '.agentboard', 'templates', 'ha-start.json')));
    assert.ok(out.includes(join(root, '.agentboard', 'roles', 'dispatcher.md')));
    // an existing config is never overwritten: the plan names config.imported.json instead
    mkdirSync(join(root, '.agentboard'));
    writeFileSync(join(root, '.agentboard', 'config.json'), '{"keep":true}');
    const before2 = files(root).sort();
    const out2 = execFileSync(process.execPath, [SCRIPT, reg, '--only', 'RTCmod', '--root', root], { encoding: 'utf8' });
    assert.ok(out2.includes(join(root, '.agentboard', 'config.imported.json')));
    assert.deepEqual(files(root).sort(), before2);
    assert.equal(readFileSync(join(root, '.agentboard', 'config.json'), 'utf8'), '{"keep":true}');
    // --only is required (the user picks the sessions to move)
    const bad = spawnSync(process.execPath, [SCRIPT, reg, '--root', root], { encoding: 'utf8' });
    assert.equal(bad.status, 2);
    assert.ok(bad.stderr.includes('--only'));
    assert.deepEqual(files(root).sort(), before2);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
