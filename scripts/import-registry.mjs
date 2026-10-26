// Move sessions of the existing session registry (session-board registry.json,
// C:\workspace\claude\session-board\registry.json) into an AgentBoard config — DRY RUN ONLY: prints the files it would create and their content, writes
// nothing (user decision 10-26: operating files and an existing config are never changed).
//
//   node scripts/import-registry.mjs <registry.json> --only RTCmod[,ha…] [--root C:/workspace]
//
// Coexistence period (docs/superpowers/specs/2026-10-26-현재운영_재사용_매핑.md §3): only the chosen
// regular[] sessions become Manager tabs, under the same names (the copy in the existing operation
// outside AgentBoard is closed when one moves); the dispatcher (workspace-07) and handover keep
// running outside AgentBoard — AgentBoard gets config.dispatcher, no handover tab; nightly refresh
// off; holla checked by its full path.
// `--root` defaults to the dispatcher entry's dir in the registry.
import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkDirName } from '../src/overlay-logic.js';

const DEFAULTS = fileURLToPath(new URL('../src-tauri/defaults', import.meta.url));
const DISPATCHER_SESSION = 'workspace-07';
const HANDOVER_SESSION = 'handover';
const RESERVED_NAMES = ['dispatcher', 'agentboard'];

export const HOLLA_REQUIRE = {
  id: 'holla',
  check: 'C:/workspace/holla-cli/02.src/dist/holla.exe slack version --workspace zigbang',
  fix: 'C:\\workspace\\CLAUDE.md 「holla CLI」 절 참조',
  why: '슬랙 확인',
  required: true,
  source: '',
};

export const DISPATCHER_NOTICE =
  '이 AgentBoard dispatcher 는 공존 기간용이다. AgentBoard 밖에서 도는 형님 대화창 세션 workspace-07 은 지금도 그대로 있다. ' +
  'Handover.md·메모리·각 세션 지시문의 workspace-07 은 그 세션을 가리킨다. 완전 전환 뒤에는 workspace-07 이 너를 가리킨다.';

export const MANAGER_NOTICE =
  '진행 보고는 지금처럼 progress.md·C:\\workspace\\claude\\handover-inbox 보고 파일 + handover 세션(AgentBoard 밖)에 SendMessage ' +
  '(규격 C:\\workspace\\claude\\session-board\\docs\\design\\handover-single-writer.md §3). ' +
  '인박스(inbox_post)는 형님의 결정·확인이 필요한 건에만 쓴다. 슬랙은 C:\\workspace\\CLAUDE.md 「holla CLI」 절을 따른다.';

const isDispatcher = (e) => e.session === DISPATCHER_SESSION || /디스패처|dispatcher/i.test(String(e.role || ''));
const isHandover = (e) => e.session === HANDOVER_SESSION || /^handover/i.test(String(e.role || ''));

const normPath = (p) => String(p ?? '').replace(/\\/g, '/').replace(/\/+$/, '');

/** `dir` relative to `root` with `/` ('' = the root itself), or null when outside it. Windows
 * paths compare case-insensitively; both separators are accepted. */
export function relDir(root, dir) {
  const r = normPath(root);
  const d = normPath(dir);
  if (!r || !d) return null;
  if (d.toLowerCase() === r.toLowerCase()) return '';
  if (!d.toLowerCase().startsWith(`${r.toLowerCase()}/`)) return null;
  return d.slice(r.length + 1);
}

/** The manager template with `prompt` as its Manager pane's start prompt, named `name`. */
function startTemplate(base, name, prompt) {
  const t = JSON.parse(JSON.stringify(base));
  t.name = name;
  const panes = [];
  const walk = (n) => (n && n.pane ? panes.push(n.pane) : ((n && n.children) || []).forEach(walk));
  walk(t.layout);
  const target = panes.find((p) => p.name === 'manager') || panes.find((p) => p.prompt) || panes[0];
  if (target) target.prompt = prompt;
  return t;
}

const withNotice = (md, notice) => `${String(md).trimEnd()}\n\n## 공존 기간 (이 PC)\n${notice}\n`;

/**
 * Pure conversion (reads only the built-in defaults under `defaultsDir`): the `only` regular[]
 * sessions → `{config, templates: {name: json}, roles: {dispatcher, manager}: markdown, warnings}`.
 */
export function importRegistry(registry, { only = [], workspaceRoot, defaultsDir = DEFAULTS } = {}) {
  const warnings = [];
  const regular = Array.isArray(registry && registry.regular) ? registry.regular.filter(Boolean) : [];
  const adhoc = Array.isArray(registry && registry.adhoc) ? registry.adhoc.filter(Boolean) : [];
  const read = (...p) => readFileSync(join(defaultsDir, ...p), 'utf8');
  const managerTpl = JSON.parse(read('templates', 'manager.json'));
  const tabs = [];
  const templates = {};
  const taken = new Set();
  let fresh = false;
  for (const name of [...new Set(only.map((s) => String(s).trim()).filter(Boolean))]) {
    const e = regular.find((r) => r.session === name);
    if (!e) {
      warnings.push(
        adhoc.some((a) => a.session === name)
          ? `'${name}': 임시(adhoc) 세션은 옮기지 않습니다`
          : `'${name}': 등록부 regular[]에 없는 세션입니다`,
      );
      continue;
    }
    if (isDispatcher(e)) {
      warnings.push(`'${name}': 디스패처는 탭으로 옮기지 않습니다 — 공존 기간에는 기존 운영(AgentBoard 밖)의 형님 대화창 그대로이고, AgentBoard dispatcher는 config.dispatcher가 맡습니다`);
      continue;
    }
    if (isHandover(e)) {
      warnings.push(`'${name}': handover는 공존 기간에 기존 운영(AgentBoard 밖) 하나만 둡니다 — 탭으로 만들지 않습니다`);
      continue;
    }
    if (RESERVED_NAMES.includes(name.toLowerCase()) || taken.has(name.toLowerCase())) {
      warnings.push(`'${name}': 탭 이름으로 쓸 수 없거나 이미 있는 이름이라 건너뜁니다`);
      continue;
    }
    const rel = relDir(workspaceRoot, e.dir);
    if (rel === null) {
      warnings.push(`'${name}': 작업 폴더 ${e.dir}이(가) workspaceRoot(${normPath(workspaceRoot)}) 밖이라 건너뜁니다`);
      continue;
    }
    if (rel === '') {
      warnings.push(`'${name}': 작업 폴더가 workspaceRoot 자체라 Manager 탭(전용 폴더 필요)으로 옮길 수 없어 건너뜁니다`);
      continue;
    }
    const bad = rel.split('/').map(checkDirName).find(Boolean);
    if (bad) {
      warnings.push(`'${name}': 작업 폴더 '${rel}' — ${bad}. 건너뜁니다`);
      continue;
    }
    let template = 'manager';
    if (typeof e.startupPrompt === 'string' && e.startupPrompt.trim()) {
      template = `${name}-start`;
      templates[template] = startTemplate(managerTpl, template, e.startupPrompt);
    }
    tabs.push({ name, role: 'manager', dir: rel, template });
    taken.add(name.toLowerCase());
    if (e.lastSessionId) fresh = true;
  }
  if (!tabs.length) warnings.push('옮길 탭이 없습니다 — config에는 dispatcher만 들어갑니다');
  if (fresh) warnings.push('옮긴 세션은 새 대화로 시작합니다 — lastSessionId로 이어 띄우기는 아직 지원하지 않습니다(매핑 #8)');
  const config = {
    dispatcher: { agent: 'claude', template: 'dispatcher-inbox' },
    defaultGroup: tabs.length ? 'work' : 'control',
    groups: tabs.length
      ? {
          split: 'horizontal',
          sizes: [35, 65],
          children: [{ group: { name: 'control', tabs: ['dispatcher'] } }, { group: { name: 'work', tabs: tabs.map((t) => t.name) } }],
        }
      : { group: { name: 'control', tabs: ['dispatcher'] } },
    tabs,
    nightlyRefresh: null, // coexistence: off (no handover tab; the existing operation keeps the daily rhythm)
    envRequire: [{ ...HOLLA_REQUIRE }],
  };
  const roles = {
    dispatcher: withNotice(read('roles', 'dispatcher.md'), DISPATCHER_NOTICE),
    manager: withNotice(read('roles', 'manager.md'), MANAGER_NOTICE),
  };
  return { config, templates, roles, warnings };
}

/** `name.ext` → `name.imported.ext` (a file that exists is never the target). */
const imported = (p) => p.replace(/(\.[^./\\]+)$/, '.imported$1');

/** The dry-run report: target paths (an existing file → its `.imported` name) and contents. */
export function renderPlan(result, root, exists = existsSync) {
  const dot = join(root, '.agentboard');
  const files = [
    [join(dot, 'config.json'), `${JSON.stringify(result.config, null, 2)}\n`],
    ...Object.entries(result.templates).map(([n, t]) => [join(dot, 'templates', `${n}.json`), `${JSON.stringify(t, null, 2)}\n`]),
    ...Object.entries(result.roles).map(([n, md]) => [join(dot, 'roles', `${n}.md`), md]),
  ];
  const out = [
    '# import-registry (dry-run) — 파일을 하나도 쓰지 않습니다. 아래 내용을 확인한 뒤 직접 반영하세요.',
    `workspaceRoot: ${root}`,
    `옮길 탭: ${result.config.tabs.map((t) => `${t.name}(${t.dir})`).join(', ') || '없음'}`,
  ];
  if (result.warnings.length) out.push('경고:', ...result.warnings.map((w) => `- ${w}`));
  out.push(
    '옮길 때: 그 세션의 등록부 항목을 regular[]에서 agentboard[]로 옮기고(start-all이 다시 띄우지 않게), 기존 운영 쪽 그 세션은 닫는다 — 형님 승인 뒤.',
  );
  for (const [p, body] of files) {
    const target = exists(p) ? imported(p) : p;
    const note = target === p ? '새 파일' : `${p}이(가) 이미 있음 — 덮어쓰지 않고 이 이름으로`;
    out.push('', `=== ${target} (${note}) ===`, body.trimEnd());
  }
  return `${out.join('\n')}\n`;
}

function usage(msg) {
  process.stderr.write(`${msg}\n사용법: node scripts/import-registry.mjs <registry.json> --only <세션,…> [--root <workspaceRoot>]\n`);
  process.exit(2);
}

function main(argv) {
  let file = null;
  let only = null;
  let root = null;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--only') only = argv[++i];
    else if (a === '--root') root = argv[++i];
    else if (!file) file = a;
    else usage(`알 수 없는 인자: ${a}`);
  }
  if (!file) usage('registry.json 경로가 필요합니다');
  if (!only) usage('--only <세션,…> 이 필요합니다 — 옮길 세션은 형님이 고릅니다');
  let registry;
  try {
    registry = JSON.parse(readFileSync(file, 'utf8'));
  } catch (err) {
    usage(`등록부를 읽을 수 없습니다: ${err.message}`);
  }
  const disp = Array.isArray(registry.regular) ? registry.regular.find((e) => e && isDispatcher(e)) : null;
  root = root || (disp && disp.dir);
  if (!root) usage('--root <workspaceRoot> 가 필요합니다(등록부에 디스패처 항목이 없음)');
  const result = importRegistry(registry, { only: only.split(','), workspaceRoot: root, defaultsDir: DEFAULTS });
  process.stdout.write(renderPlan(result, root));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main(process.argv.slice(2));
}
