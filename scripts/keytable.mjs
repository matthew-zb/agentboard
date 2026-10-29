// Prints the README key table (markdown) generated from src-tauri/defaults/keybindings.json.
// `node scripts/keytable.mjs` — paste between the KEYTABLE markers in README.md (a test checks they match).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const DEFAULTS = fileURLToPath(new URL('../src-tauri/defaults/keybindings.json', import.meta.url));

const LABELS = {
  'pane.focusLeft': '창 포커스 왼쪽',
  'pane.focusDown': '창 포커스 아래',
  'pane.focusUp': '창 포커스 위',
  'pane.focusRight': '창 포커스 오른쪽',
  'pane.resizeLeft': '창 크기 조절 왼쪽',
  'pane.resizeDown': '창 크기 조절 아래',
  'pane.resizeUp': '창 크기 조절 위',
  'pane.resizeRight': '창 크기 조절 오른쪽',
  'pane.splitRight': '좌우 분할',
  'pane.splitDown': '상하 분할',
  'pane.maximize': '창 최대화 토글',
  'pane.close': '창 닫기',
  'pane.restart': '창 재시작',
  'pane.pick': '창 번호 표시 후 이동',
  'pane.nextAttention': '주의(⚠) 창으로 이동 / 다시 누르면 복귀',
  'pane.copyMode': '복사·스크롤 모드',
  'tab.syncInput': '탭 내 입력 동기화 토글',
  'pane.paste': '텍스트 붙여넣기',
  'tab.prev': '이전 탭(그룹 내)',
  'tab.next': '다음 탭(그룹 내)',
  'tab.last': '직전 위치(탭·그룹 왕복)',
  'tab.dispatcher': 'Dispatcher 탭',
  'tab.new': '새 탭',
  'tab.moveLeft': '탭 이동 왼쪽(끝에서 옆 그룹으로)',
  'tab.moveRight': '탭 이동 오른쪽(끝에서 옆 그룹으로)',
  'tab.moveDown': '탭을 아래 그룹으로',
  'tab.moveUp': '탭을 위 그룹으로',
  'tab.moveTo': '탭을 임의 그룹으로 보내기(번호 선택)',
  'group.splitRight': '그룹 좌우 분할',
  'group.splitDown': '그룹 상하 분할',
  'group.mergeInto': '그룹 합치기(대상 선택)',
  'group.close': '그룹 닫기',
  'group.maximize': '그룹 최대화 토글',
  'app.palette': '커맨드 팔레트',
  'app.dashboard': '작업 상태 탭 열기',
};

const SECTIONS = [
  ['창', (id) => id.startsWith('pane.')],
  ['탭', (id) => id.startsWith('tab.')],
  ['그룹', (id) => id.startsWith('group.')],
  ['전역', (id) => id.startsWith('app.')],
];

/** Collapse tab.goto1..9 / group.goto1..9 into one row each. */
function rows(map) {
  const out = [];
  const gotoSeen = new Set();
  for (const [id, key] of Object.entries(map)) {
    if (key == null) continue;
    const m = /^(tab|group)\.goto(\d)$/.exec(id);
    if (m) {
      if (gotoSeen.has(m[1])) continue;
      gotoSeen.add(m[1]);
      const first = `${m[1]}.goto1`;
      const last = `${m[1]}.goto9`;
      const k1 = map[first];
      const k9 = map[last];
      const range = `${k1}..${k9.split('+').pop()}`;
      out.push({ id: `${m[1]}.goto1..9`, key: range, label: m[1] === 'tab' ? 'N번째 탭(그룹 내)' : 'N번째 그룹으로' });
      continue;
    }
    out.push({ id, key, label: LABELS[id] ?? id });
  }
  return out;
}

export function keyTable(map = JSON.parse(readFileSync(DEFAULTS, 'utf8'))) {
  const all = rows(map);
  const lines = [];
  for (const [title, pred] of SECTIONS) {
    const part = all.filter((r) => pred(r.id));
    if (!part.length) continue;
    lines.push(`**${title}**`, '', '| 동작 | 키 | 명령 id |', '|---|---|---|');
    for (const r of part) lines.push(`| ${r.label} | \`${r.key.replace(/\|/g, '\|')}\` | \`${r.id}\` |`);
    lines.push('');
  }
  const unbound = Object.entries(map).filter(([, k]) => k == null).map(([id]) => `\`${id}\``);
  if (unbound.length) lines.push(`기본 키가 없는(팔레트 전용) 명령: ${unbound.join(', ')}`, '');
  return lines.join('\n').trimEnd();
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  console.log(keyTable());
}
