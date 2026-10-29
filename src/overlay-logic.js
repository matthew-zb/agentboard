// Pure logic behind the overlays (overlays.js draws them): picker key mapping, picker results →
// WsCommands, palette entries, the new-tab wizard flow and the startup missing-dir flow.
// No DOM access: unit-tested in Node. The UI is injected into the flows as functions.
import { fuzzyRank } from './fuzzy.js';
import { leafIds } from './layout.js';
import { tabGroups } from './groups.js';
import { findNeighbor } from './geometry.js';

/** Number overlay: auto-cancel after this long without input (global constraints). */
export const PICK_TIMEOUT_MS = 2000;
/** Toast lifetime. */
export const TOAST_MS = 3000;

// ---------------------------------------------------------------- small helpers

/** Mirror of launcher.rs `check_dir_name` (the backend stays authoritative): message or null. */
export function checkDirName(name) {
  const s = String(name ?? '');
  if (!s.trim()) return '폴더 이름이 비어 있습니다';
  if (/[/\\:]/.test(s)) return '폴더 이름에 / \\ : 는 쓸 수 없습니다';
  if (s === '.' || s === '..') return '폴더 이름으로 . 이나 .. 은 쓸 수 없습니다';
  return null;
}

/**
 * Wizard step ① choices (ruling R18; the list preselects the first entry):
 * - typed name equals an existing dir (case-insensitive, as on Windows) → that dir first, then
 *   the other fuzzy matches; no create entry;
 * - otherwise, non-empty valid name → `{dir: typed, create: true}` first, then the fuzzy matches
 *   (so Enter never silently picks a dir the name only fuzzy-matches);
 * - invalid name → fuzzy matches, create entry last (picking it shows why it is invalid).
 */
export function dirChoices(dirs, query) {
  const q = String(query ?? '').trim();
  const matches = fuzzyRank(dirs, q).map((i) => ({ dir: dirs[i], create: false }));
  if (!q) return matches;
  const exact = matches.findIndex((c) => c.dir.toLowerCase() === q.toLowerCase());
  if (exact >= 0) return [matches[exact], ...matches.filter((_, i) => i !== exact)];
  const create = { dir: q, create: true };
  return checkDirName(q) ? [...matches, create] : [create, ...matches];
}

/** Most rows the "문서 열기" list draws per keystroke (doc_list_md returns up to 5,000). */
export const DOC_CHOICES_MAX = 200;

/** "문서 열기": `paths` (workspace-root relative) fuzzy-filtered by `query`, best first, capped. */
export function docChoices(paths, query, cap = DOC_CHOICES_MAX) {
  return fuzzyRank(paths || [], query)
    .slice(0, cap)
    .map((i) => paths[i]);
}

/** Templates with the configured default first (unknown default → unchanged). */
export function templateOrder(list, def) {
  if (!def || !list.includes(def)) return [...list];
  return [def, ...list.filter((t) => t !== def)];
}

/** Selection index after ↑/↓ (wraps). */
export function listMove(i, n, delta) {
  if (n <= 0) return 0;
  return (((i + delta) % n) + n) % n;
}

const HJKL = { KeyH: 'left', KeyJ: 'down', KeyK: 'up', KeyL: 'right' };
const EXTRA = { Backslash: '\\', IntlBackslash: '\\', Minus: '-', NumpadSubtract: '-' };

/**
 * Number-picker key (by `KeyboardEvent.code`; modifiers still held from the opening chord are
 * ignored): `{index}` (Digit/Numpad 1..n, n ≤ 9), `{dir}` (hjkl when allowed), `{key}` ('\\' or
 * '-' when listed in extraKeys), `{cancel: true}` (Esc), or null (ignored).
 */
export function pickerKey(e, n, { allowHjkl = false, extraKeys = [] } = {}) {
  const code = e.code || '';
  if (code === 'Escape') return { cancel: true };
  const m = /^(?:Digit|Numpad)([1-9])$/.exec(code);
  if (m) {
    const i = Number(m[1]) - 1;
    return i < Math.min(n, 9) ? { index: i } : null;
  }
  if (allowHjkl && HJKL[code]) return { dir: HJKL[code] };
  const k = EXTRA[code];
  if (k && extraKeys.includes(k)) return { key: k };
  return null;
}

// ---------------------------------------------------------------- planning

/** `{g, tab, pane}` of the focus chain, or null. */
function chain(ws) {
  const g = ws && ws.focus && ws.groups[ws.focus.groupId];
  const tab = g && ws.tabs[g.activeTabId];
  return tab ? { g, tab, pane: tab.lastFocusedPaneId } : null;
}

const setFocus = (group, tab, pane) => ({ type: 'setFocus', group, tab, pane });

/**
 * SetFocus to (group, tab, pane?), preceded by what would keep the target hidden: another group's
 * maximize, or another pane's maximize inside the target tab.
 */
export function focusCommands(ws, group, tab, pane = null) {
  const out = [];
  if (ws.groupMaximized && ws.groupMaximized !== group) out.push({ type: 'setGroupMaximized', group: null });
  const t = ws.tabs[tab];
  if (pane && t && t.maximizedPaneId && t.maximizedPaneId !== pane) out.push({ type: 'setMaximized', tab, pane: null });
  out.push(setFocus(group, tab, pane));
  return out;
}

/**
 * Picker result → WsCommands. `kind`:
 * - 'pane' (pane.pick): `{id}` / `{dir}` (from the focused pane) → SetFocus.
 * - 'moveTo' (tab.moveTo): `{id}` / `{dir}` (from the focused group) → MoveTab to the end of that
 *   group; `{key:'\\'|'-'}` → SplitGroupWith on the focused group (right / down), i.e. the same
 *   as group.splitRight/Down.
 * - 'merge' (group.mergeInto): `{id}` / `{dir}` → MergeGroup{src: focused group, dst}.
 * Own pane / own group / nothing in that direction → [].
 */
export function pickCommands(kind, ws, result, geo = {}) {
  const c = chain(ws);
  if (!c || !result) return [];
  const { g, tab, pane } = c;
  if (kind === 'pane') {
    const id = result.dir ? findNeighbor(geo.paneRects || [], pane, result.dir) : result.id;
    const p = id && ws.panes[id];
    if (!p || id === pane) return [];
    const gid = tabGroups(ws)[p.tab];
    return gid ? focusCommands(ws, gid, p.tab, id) : [];
  }
  if (kind === 'moveTo' && result.key) {
    const dir = result.key === '-' ? 'vertical' : 'horizontal';
    return [{ type: 'splitGroupWith', tab: tab.id, group: g.id, dir, before: false }];
  }
  const target = result.dir ? findNeighbor(geo.groupRects || [], g.id, result.dir) : result.id;
  const tg = target && ws.groups[target];
  if (!tg || tg.id === g.id) return [];
  if (kind === 'moveTo') return [{ type: 'moveTab', tab: tab.id, group: tg.id, index: tg.tabIds.length }];
  if (kind === 'merge') return [{ type: 'mergeGroup', src: g.id, dst: tg.id }];
  return [];
}

// ---------------------------------------------------------------- palette

const LABELS = {
  'pane.focusLeft': '왼쪽 창으로 이동',
  'pane.focusDown': '아래 창으로 이동',
  'pane.focusUp': '위 창으로 이동',
  'pane.focusRight': '오른쪽 창으로 이동',
  'pane.resizeLeft': '창 크기 조절: 왼쪽',
  'pane.resizeDown': '창 크기 조절: 아래',
  'pane.resizeUp': '창 크기 조절: 위',
  'pane.resizeRight': '창 크기 조절: 오른쪽',
  'pane.splitRight': '창 좌우 분할',
  'pane.splitDown': '창 상하 분할',
  'pane.maximize': '창 최대화 토글',
  'pane.close': '창 닫기',
  'pane.restart': '창 재시작',
  'pane.pick': '창 번호로 이동',
  'pane.nextAttention': '주의 창으로 이동',
  'pane.copyMode': '복사 모드',
  'pane.paste': '붙여넣기',
  'pane.breakToTab': '현재 창을 새 탭으로 떼어내기',
  'tab.syncInput': '탭 입력 동기화 토글',
  'tab.prev': '이전 탭',
  'tab.next': '다음 탭',
  'tab.last': '직전 위치로',
  'tab.dispatcher': 'Dispatcher 탭으로',
  'tab.new': '새 탭',
  'tab.moveLeft': '탭을 왼쪽으로 이동',
  'tab.moveRight': '탭을 오른쪽으로 이동',
  'tab.moveDown': '탭을 아래 그룹으로 이동',
  'tab.moveUp': '탭을 위 그룹으로 이동',
  'tab.moveTo': '탭을 다른 그룹으로 보내기',
  'group.splitRight': '그룹 좌우 분할(현재 탭을 새 그룹으로)',
  'group.splitDown': '그룹 상하 분할(현재 탭을 새 그룹으로)',
  'group.mergeInto': '그룹 합치기',
  'group.close': '그룹 닫기',
  'group.maximize': '그룹 최대화 토글',
  'app.palette': '커맨드 팔레트',
};

/** Korean label of a command id (falls back to the id). */
export function commandLabel(id) {
  if (LABELS[id]) return LABELS[id];
  let m = /^tab\.goto([1-9])$/.exec(id);
  if (m) return `${m[1]}번째 탭으로`;
  m = /^group\.goto([1-9])$/.exec(id);
  if (m) return `${m[1]}번째 그룹으로`;
  return id;
}

/** Ids not listed as plain commands (the palette itself; ids with a dedicated entry). */
const NOT_LISTED = new Set(['app.palette', 'pane.breakToTab']);

/**
 * Palette entries `{label, hint?, action}` for the workspace. `bindings` = id → key string (null =
 * unbound); `has(id)` = a handler is registered. Actions:
 * `{type:'command', id}` | `{type:'ws', cmds}` | `{type:'rename', tab}` | `{type:'errors'}` |
 * `{type:'sidebar'}` | `{type:'openDoc'}` (pick a workspace .md file → OpenDoc) |
 * `{type:'reloadConfig'}` (config_reload: settings re-read without restarting any pane) |
 * `{type:'autostart', on}` (login autostart; listed only when `autostart` — the current state,
 * true/false — is known).
 */
export function paletteItems(ws, { bindings = {}, has = () => true, autostart = null } = {}) {
  const out = [];
  const c = chain(ws);
  const groupOrder = leafIds(ws.groupLayout).filter((id) => ws.groups[id]);

  if (c && !c.tab.isDispatcher) out.push({ label: '탭 이름 바꾸기', action: { type: 'rename', tab: c.tab.id } });
  if (c && c.pane) out.push({ label: '현재 창을 새 탭으로 떼어내기', action: { type: 'ws', cmds: [{ type: 'breakPaneToTab', pane: c.pane }] } });
  // ④ RF1: closing a tab (Y/N). Manager / handover tabs too (R3 a: this is handover's only way out)
  if (c && !c.tab.isDispatcher) out.push({ label: '현재 탭 닫기', hint: 'Y/N 확인', action: { type: 'closeTab', tab: c.tab.id } });
  out.push({ label: '설정 오류 보기', action: { type: 'errors' } });
  out.push({ label: '설정 다시 읽기', hint: '재시작 없이', action: { type: 'reloadConfig' } });
  out.push({ label: '문서 열기', hint: 'md 파일', action: { type: 'openDoc' } });
  out.push({ label: '사이드바 토글', action: { type: 'sidebar' } });
  if (autostart === true) out.push({ label: '로그인 시 자동 실행 끄기', hint: '현재: 켜짐', action: { type: 'autostart', on: false } });
  if (autostart === false) out.push({ label: '로그인 시 자동 실행 켜기', hint: '현재: 꺼짐', action: { type: 'autostart', on: true } });

  const ids = [...new Set(Object.keys(bindings))];
  for (const id of ids) {
    if (NOT_LISTED.has(id) || !has(id)) continue;
    const item = { label: commandLabel(id), action: { type: 'command', id } };
    if (bindings[id]) item.hint = bindings[id];
    out.push(item);
  }

  for (const gid of groupOrder) {
    for (const tid of ws.groups[gid].tabIds) {
      const t = ws.tabs[tid];
      if (t) out.push({ label: `탭으로 이동: ${t.name}`, action: { type: 'ws', cmds: focusCommands(ws, gid, tid) } });
    }
  }
  for (const gid of groupOrder) {
    for (const tid of ws.groups[gid].tabIds) {
      const t = ws.tabs[tid];
      if (!t) continue;
      for (const pid of leafIds(t.layout)) {
        const p = ws.panes[pid];
        if (p) out.push({ label: `창으로 이동: ${t.name}/${p.name}`, action: { type: 'ws', cmds: focusCommands(ws, gid, tid, pid) } });
      }
    }
  }
  if (c) {
    for (const gid of groupOrder) {
      if (gid === c.g.id) continue;
      const g = ws.groups[gid];
      out.push({ label: `탭 이동 → ${g.name}`, action: { type: 'ws', cmds: [{ type: 'moveTab', tab: c.tab.id, group: gid, index: g.tabIds.length }] } });
    }
    for (const gid of groupOrder) {
      if (gid === c.g.id) continue;
      out.push({ label: `그룹 합치기 → ${ws.groups[gid].name}`, action: { type: 'ws', cmds: [{ type: 'mergeGroup', src: c.g.id, dst: gid }] } });
    }
  }
  return out;
}

// ---------------------------------------------------------------- flows

/**
 * New-tab wizard (spec §6.3, Task 12 role choice): ① dir (unknown name → confirm → createDir) →
 * ② role (`pickRole(dir)` → 'plain' | 'manager' | null; without `pickRole` always plain) →
 * plain: ③ template (default first) → CreateTab{name: dir, dir, template};
 * manager: `createManager({name: dir, dir})` — the backend's config-persisting Manager path
 * (template `manager` if present, else defaultTemplate; R2).
 * A failed creation (e.g. duplicate tab name; `createTab` / `createManager` resolve false after
 * toasting the error), a declined / invalid / failed dir creation go back to ①. Esc at any step,
 * or `alive()` turning false (an overlay closed by a tab/group switch), ends the flow with null.
 * Resolves `{dir, template}` (plain) or `{dir, role: 'manager'}` on success.
 *
 * deps: listDirs, listTemplates, defaultTemplate, pickDir(dirs) → choice|null,
 * confirm(msg) → bool, createDir(name), pickRole?(dir) → 'plain'|'manager'|null,
 * createManager?({name, dir}) → bool, pickTemplate(list, defaultName|null, dir) → name|null,
 * createTab(cmd) → bool (false after toasting the error),
 * toast(msg), alive() → bool.
 */
export async function newTabFlow(d) {
  const [templates, def] = await Promise.all([
    Promise.resolve(d.listTemplates()).catch(() => []),
    Promise.resolve(d.defaultTemplate()).catch(() => null),
  ]);
  const ordered = templateOrder(templates || [], def);
  for (;;) {
    if (!d.alive()) return null;
    const dirs = await Promise.resolve(d.listDirs()).catch(() => []);
    const choice = await d.pickDir(dirs || []);
    if (!choice || !d.alive()) return null;
    const dir = choice.dir;
    if (choice.create) {
      const bad = checkDirName(dir);
      if (bad) {
        d.toast(bad);
        continue;
      }
      const yes = await d.confirm(`'${dir}' 디렉토리를 만들까요?`);
      if (!d.alive()) return null;
      if (!yes) continue;
      try {
        await d.createDir(dir);
      } catch (e) {
        d.toast(String(e));
        continue;
      }
    }
    if (!d.alive()) return null;
    const role = d.pickRole ? await d.pickRole(dir) : 'plain';
    if (role == null || !d.alive()) return null;
    if (role === 'manager') {
      if (await d.createManager({ name: dir, dir })) return { dir, role: 'manager' };
      continue;
    }
    const template = await d.pickTemplate(ordered, ordered[0] === def ? def : null, dir);
    if (template == null || !d.alive()) return null;
    if (await d.createTab({ type: 'createTab', name: dir, dir, template, group: null })) return { dir, template };
  }
}

/** `[[tabId, dir], …]` → `[{dir, tabs}]` (first-seen order; one question per folder). */
export function groupMissing(list) {
  const out = [];
  for (const [tab, dir] of list || []) {
    let e = out.find((x) => x.dir === dir);
    if (!e) out.push((e = { dir, tabs: [] }));
    e.tabs.push(tab);
  }
  return out;
}

/**
 * Startup check (spec §4.2): for each missing folder ask
 * "'<dir>' 디렉토리가 없습니다. 만들까요?"; yes → createDir, then RestartPane for every pane of the
 * tabs using it; no → the panes stay Failed. Stops when `alive()` turns false (the app passes
 * `() => true`: a tab/group switch only answers the open question "no", ruling R19).
 *
 * deps: missingDirs, confirm, createDir, panesOf(tabId) → ids, restart(ids), toast, alive.
 */
export async function missingDirsFlow(d) {
  const groups = groupMissing(await d.missingDirs());
  for (const { dir, tabs } of groups) {
    if (!d.alive()) return;
    const yes = await d.confirm(`'${dir}' 디렉토리가 없습니다. 만들까요?`);
    if (!d.alive()) return;
    if (!yes) continue;
    try {
      await d.createDir(dir);
    } catch (e) {
      d.toast(String(e));
      continue;
    }
    const ids = tabs.flatMap((t) => d.panesOf(t) || []);
    if (ids.length) await d.restart(ids);
  }
}

/**
 * Key → answer of a Y/N confirm dialog: true (Y, or Enter when `enterYes`), false (N / Esc),
 * null (ignored; also every auto-repeat, so a held Enter from the previous step cannot answer).
 */
export function confirmKey(e, { enterYes = true } = {}) {
  if (e.repeat) return null;
  if (e.code === 'KeyY') return true;
  if (enterYes && (e.code === 'Enter' || e.code === 'NumpadEnter')) return true;
  if (e.code === 'KeyN' || e.code === 'Escape') return false;
  return null;
}

/**
 * Window close request (spec §8): ask only when agents are running. `st.busy` guards against a
 * second close request while the dialog is open. Resolves true when the app was told to exit.
 * deps: { runningCount(), confirm(msg, opts), exit() }
 */
export async function closeFlow(st, deps) {
  if (st.busy) return false;
  st.busy = true;
  try {
    const n = await deps.runningCount();
    // keyboard Y/N only (spec §8): Enter must not close the app with running agents
    if (n > 0 && !(await deps.confirm(`실행 중인 에이전트 ${n}개를 종료하고 닫을까요?`, { enterYes: false }))) return false;
    await deps.exit();
    return true;
  } finally {
    st.busy = false;
  }
}

// ---------------------------------------------------------------- closing a tab (④ RF1)

/** `{tab, name, role ('manager' | 'handover' | null), running}` for the close question; null
 * for the Dispatcher tab (never closed) or an unknown tab. */
export function closeTabInfo(ws, tabId) {
  const t = ws && ws.tabs && ws.tabs[tabId];
  if (!t || t.isDispatcher) return null;
  const running = leafIds(t.layout).filter((id) => {
    const p = ws.panes && ws.panes[id];
    return p && p.status && p.status.state === 'running';
  }).length;
  const role = t.role === 'manager' || t.role === 'handover' ? t.role : null;
  return { tab: t.id, name: t.name, role, running };
}

function closeQuestion({ name, role, running }) {
  const lines = [role === 'manager' ? `'${name}' 상시업무(Manager) 탭을 닫을까요?` : `'${name}' 탭을 닫을까요?`];
  const parts = [];
  if (running > 0) parts.push(`실행 중인 창 ${running}개가 종료됩니다`);
  if (role) parts.push('config.json에서도 빠집니다');
  if (parts.length) lines.push(`${parts.join('. ')}.`);
  if (role === 'manager') {
    lines.push('작업 폴더·이슈·팀 문서는 남습니다. 끝나지 않은 인박스 항목이나 진행 중인 팀이 있으면 닫지 않습니다.');
  }
  return lines.join('\n');
}

/**
 * Palette "현재 탭 닫기": Y/N (Enter does not answer — the same rule as the exit dialog) →
 * CloseTab. A refusal (open inbox items, running teams — decided by the backend, decision c) is
 * shown in the error panel, not as a toast: it is long and says what to do. Resolves true when
 * the tab was closed. deps: { confirm(msg, opts), close(tabId), showErrors(lines) }
 */
export async function closeTabFlow(info, deps) {
  if (!info) return false;
  if (!(await deps.confirm(closeQuestion(info), { enterYes: false }))) return false;
  try {
    await deps.close(info.tab);
  } catch (err) {
    deps.showErrors([err && err.message ? err.message : String(err)]);
    return false;
  }
  return true;
}

// ---------------------------------------------------------------- settings reload

const strings = (v) => (Array.isArray(v) ? v.filter((x) => typeof x === 'string' && x) : []);

/**
 * Toast after `config_reload` succeeded (`{added, keptMissing, promptsUpdated}`), e.g.
 * `설정을 다시 읽었습니다 — 새 탭: a, b · 닫지 않은 탭: c · 시작 프롬프트 갱신 2`; parts that do
 * not apply are left out.
 */
export function reloadReportText(r) {
  const parts = [];
  const added = strings(r && r.added);
  const kept = strings(r && r.keptMissing);
  const n = r && Number.isInteger(r.promptsUpdated) ? r.promptsUpdated : 0;
  if (added.length) parts.push(`새 탭: ${added.join(', ')}`);
  if (kept.length) parts.push(`닫지 않은 탭: ${kept.join(', ')}`);
  if (n > 0) parts.push(`시작 프롬프트 갱신 ${n}`);
  return parts.length ? `설정을 다시 읽었습니다 — ${parts.join(' · ')}` : '설정을 다시 읽었습니다';
}

/** Error panel lines for a refused reload: the backend's `파일:줄:열 — 원인` list (or a single
 * error) under a line saying nothing was changed. */
export function reloadErrorLines(err) {
  const head = '설정에 오류가 있어 다시 읽지 않았습니다 — 아무것도 바뀌지 않았습니다';
  if (Array.isArray(err)) return [head, ...err.map(String)];
  const msg = err && err.message ? err.message : String(err ?? '');
  return msg ? [head, msg] : [head];
}

/**
 * Palette "설정 다시 읽기": `config_reload` (no pane restarts) → report toast, then the key
 * settings are fetched again (keybindings.json may have changed). A refused reload (validation
 * errors) goes to the error panel and nothing else happens. Resolves true on success.
 * deps: { reload(), keybindings(), setKeymap(bindings), toast(msg, kind?), showErrors(lines) }
 */
export async function reloadFlow(deps) {
  let report;
  try {
    report = await deps.reload();
  } catch (err) {
    deps.showErrors(reloadErrorLines(err));
    return false;
  }
  deps.toast(reloadReportText(report), 'info');
  try {
    deps.setKeymap(await deps.keybindings());
  } catch (err) {
    deps.toast(err);
  }
  return true;
}

// ---------------------------------------------------------------- environment (Task 12)

/** Short status-bar label of an environment item id (env_status ids are stable). */
export function envItemLabel(id) {
  if (id === 'claude-version') return 'Claude 버전';
  if (id === 'git-bash') return 'Git 없음';
  if (id.startsWith('folder-trust:')) return `${id.slice('folder-trust:'.length)} 폴더 신뢰`;
  return `${id} 없음`;
}

/**
 * Status-bar flag for the environment check (`env_status` items; null while unknown): only
 * failing items, e.g. `[환경: Git 없음(건너뜀), holla 없음]`. `highlight` when a failing item is
 * not skipped and either required or the Claude version (spec: version too old = error).
 * `title` lists detail and fix per failing item. null when everything is fine.
 */
export function envStatusFlag(items) {
  const bad = (Array.isArray(items) ? items : []).filter((i) => i && !i.ok);
  if (!bad.length) return null;
  const parts = bad.map((i) => `${envItemLabel(String(i.id))}${i.skipped ? '(건너뜀)' : ''}`);
  const highlight = bad.some((i) => !i.skipped && (i.required || i.id === 'claude-version'));
  const title = bad.map((i) => `${i.id}: ${i.detail || ''}${i.fix ? ` — 조치: ${i.fix}` : ''}${i.skipped ? ' (건너뜀)' : ''}`).join('\n');
  return { text: `[환경: ${parts.join(', ')}]`, highlight, title };
}
