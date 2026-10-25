# AgentBoard

Claude Code 같은 CLI 에이전트를 **그룹 → 탭 → 분할 창** 구조로 한 화면에 띄우고, 마우스 없이 키보드만으로 조작하는 Windows 데스크톱 앱입니다. Tauri v2 (Rust + 시스템 WebView2) + 바닐라 JS + xterm.js. 버전별 변경 내역은 [History.md](History.md)를 참고하세요.

## 주요 기능

- 탭 그룹(좌우/상하 분할) 안에 탭, 탭 안에 분할 창 — 창마다 ConPTY로 실행되는 독립 프로세스(Claude Code, PowerShell, cmd …)
- 모든 조작이 키보드로 가능 (창 이동·크기 조절·분할·탭/그룹 이동·합치기·복사 모드·커맨드 팔레트). 마우스 드래그로 탭을 그룹 사이로 옮길 수도 있음
- 한글 IME 상태에서도 동작하도록 `KeyboardEvent.code` 기준으로 키를 판별
- 입력 동기화(탭 안 모든 창에 같은 입력), 터미널 벨 기반 주의(⚠) 표시와 `Alt+a` 이동
- 종료 후 재실행하면 그룹·탭·레이아웃·포커스를 복원(각 창의 프로세스는 다시 실행)
- 앱 종료 시 실행 중인 에이전트 수를 보여주고 확인, Job Object로 자식 프로세스 트리까지 정리

## 설치·개발·빌드

사전 요구사항:

- Windows 11
- Node.js **22 이상** (`npm run test:js`의 glob이 Node 21+ 필요)
- Rust stable (MSVC 툴체인)
- Visual Studio Build Tools (C++ 데스크톱 개발 워크로드), WebView2 런타임

```powershell
npm install        # 의존성 설치
npm run vendor     # node_modules/@xterm/* → src/vendor/ 복사 (버전 고정, 이미 커밋되어 있음)
npx tauri dev      # 개발 실행
npm run test:js    # 프런트엔드 순수 로직 테스트
cargo test --manifest-path src-tauri/Cargo.toml   # Rust 테스트
npx tauri build    # 릴리스 빌드 → src-tauri/target/release/bundle/nsis/*.exe, msi/*.msi
```

개발 중 DevTools: `$env:AGENTBOARD_DEVTOOLS='1'; npx tauri dev`

태그 `agentboard-v*`를 푸시하면 GitHub Actions(`.github/workflows/build.yml`)가 테스트 후 Windows 설치 파일을 빌드해 Release에 올립니다.

## 설정 파일

| 파일 | 위치 | 설명 |
|---|---|---|
| `settings.json` | `%APPDATA%/agentboard/` | 부트스트랩 전용. `{ "workspaceRoot": "C:/workspace" }` |
| `workspace.json` | `%APPDATA%/agentboard/` | 앱이 저장하는 실행 상태(직접 편집하지 않음). 깨지면 `.bak`로 옮기고 `config.json` 배치로 시작 |
| `config.json` | `<workspaceRoot>/.agentboard/` | 초기 탭·그룹 배치 |
| `templates/*.json` | `<workspaceRoot>/.agentboard/templates/` | 탭 템플릿(분할 레이아웃) |
| `agents/*.json` | `<workspaceRoot>/.agentboard/agents/` | 에이전트 프로필 |
| `keybindings.json` | `<workspaceRoot>/.agentboard/` | 기본 키 위에 덮어쓰기 |

첫 실행 시 `.agentboard/`가 없으면 내장 기본값(`claude`·`cmd`·`powershell` 프로필, `dispatcher`·`solo`·`team-3` 템플릿, 키 설정)으로 생성합니다. 같은 이름의 사용자 파일이 우선합니다. 모든 JSON은 로드 시 검증하며, 오류는 `파일:줄:열 — 원인` 형태로 상태 표시줄 `[설정 오류 N]`(클릭하거나 팔레트에서 열기)에 표시됩니다.

### config.json

```json
{
  "dispatcher": { "agent": "claude", "template": "dispatcher" },
  "defaultTemplate": "solo",
  "defaultAgent": "claude",
  "groups": {
    "split": "horizontal", "sizes": [25, 75],
    "children": [
      { "group": { "name": "control", "tabs": ["dispatcher"] } },
      { "group": { "name": "work",    "tabs": ["api", "mdviewer"] } }
    ]
  },
  "defaultGroup": "work",
  "tabs": [
    { "name": "api",      "dir": "api",      "template": "backend-team" },
    { "name": "mdviewer", "dir": "mdviewer", "template": "solo" }
  ]
}
```

- `dispatcher` 탭은 이름이 예약되어 있고 작업공간이 `workspaceRoot`이며 닫을 수 없습니다.
- 업무 탭의 작업공간은 `<workspaceRoot>/<dir>`입니다. 폴더가 없으면 시작할 때 만들지 묻습니다.
- `groups`는 첫 실행(또는 `workspace.json`이 없을 때)의 초기 배치입니다. `groups`에 없는 탭은 `defaultGroup`에 열립니다.

### templates/backend-team.json

```json
{
  "name": "backend-team",
  "layout": {
    "split": "horizontal", "sizes": [60, 40],
    "children": [
      { "pane": { "name": "lead", "agent": "claude", "args": ["--model", "opus"],
                  "cwd": "", "prompt": "너는 팀 리드다...", "focus": true } },
      { "split": "vertical",
        "children": [
          { "pane": { "name": "coder", "agent": "claude" } },
          { "pane": { "name": "tests", "agent": "powershell", "cwd": "server",
                      "prompt": "npm test -- --watch" } }
        ] }
    ]
  }
}
```

split 노드(`horizontal` = 좌|우, `vertical` = 위/아래, `sizes` 퍼센트)와 pane 노드(`name`, `agent`, `args`, `cwd`, `prompt`, `focus`)로 구성합니다. `prompt`는 프로세스가 ready 상태가 되면 한 번 입력·제출됩니다.

### agents/claude.json

```json
{
  "name": "claude",
  "level": "L2",
  "cmd": "claude",
  "args": [],
  "env": {},
  "ready":  { "idleMs": 800 },
  "submit": { "bracketedPaste": true, "key": "\r" },
  "passthroughKeys": []
}
```

- `ready`: `{ "pattern": "<regex>" }`(출력에 매칭) 또는 `{ "idleMs": n }`(첫 출력 후 n ms 무출력). 10초가 지나면 그냥 입력하고 창 헤더에 경고를 표시합니다.
- `passthroughKeys`: 앱 단축키 중 이 창에서는 가로채지 않고 프로그램에 넘길 키(예: `"Alt+h"`).
- `level`, `systemPrompt`, `mcp`, `idle`, `resume`은 2단계용으로 1단계에서는 파싱·검증만 합니다.

### keybindings.json

```json
{ "pane.focusLeft": "Alt+h", "group.splitRight": "Ctrl+Alt+\\", "pane.close": null }
```

명령 id → 키. `null`은 해제입니다. 두 명령이 같은 키를 쓰면 오류로 표시하고 기본값을 유지합니다.

## 키 배치

앱 단축키는 xterm보다 먼저 처리되며, 매칭되지 않은 키는 그대로 프로그램으로 전달됩니다. `Alt+q`(창), `Ctrl+Alt+m`(탭 이동), `Ctrl+Alt+g`(그룹 합치기)는 번호 오버레이를 띄우고 숫자 또는 `hjkl`로 대상을 고릅니다(`Esc` 취소, 2초 무입력 시 자동 취소). 이 표는 `src-tauri/defaults/keybindings.json`에서 `node scripts/keytable.mjs`로 생성하며, 테스트가 일치 여부를 검사합니다.

<!-- KEYTABLE:START -->
**창**

| 동작 | 키 | 명령 id |
|---|---|---|
| 창 포커스 왼쪽 | `Alt+h` | `pane.focusLeft` |
| 창 포커스 아래 | `Alt+j` | `pane.focusDown` |
| 창 포커스 위 | `Alt+k` | `pane.focusUp` |
| 창 포커스 오른쪽 | `Alt+l` | `pane.focusRight` |
| 창 크기 조절 왼쪽 | `Alt+Shift+H` | `pane.resizeLeft` |
| 창 크기 조절 아래 | `Alt+Shift+J` | `pane.resizeDown` |
| 창 크기 조절 위 | `Alt+Shift+K` | `pane.resizeUp` |
| 창 크기 조절 오른쪽 | `Alt+Shift+L` | `pane.resizeRight` |
| 좌우 분할 | `Alt+\` | `pane.splitRight` |
| 상하 분할 | `Alt+-` | `pane.splitDown` |
| 창 최대화 토글 | `Alt+z` | `pane.maximize` |
| 창 닫기 | `Alt+x` | `pane.close` |
| 창 재시작 | `Alt+r` | `pane.restart` |
| 창 번호 표시 후 이동 | `Alt+q` | `pane.pick` |
| 주의(⚠) 창으로 이동 / 다시 누르면 복귀 | `Alt+a` | `pane.nextAttention` |
| 복사·스크롤 모드 | `Alt+c` | `pane.copyMode` |
| 텍스트 붙여넣기 | `Ctrl+Shift+V` | `pane.paste` |

**탭**

| 동작 | 키 | 명령 id |
|---|---|---|
| 탭 내 입력 동기화 토글 | `Alt+i` | `tab.syncInput` |
| 이전 탭(그룹 내) | `Ctrl+Alt+h` | `tab.prev` |
| 다음 탭(그룹 내) | `Ctrl+Alt+l` | `tab.next` |
| N번째 탭(그룹 내) | `Alt+1..9` | `tab.goto1..9` |
| 직전 위치(탭·그룹 왕복) | `Ctrl+Alt+j` | `tab.last` |
| Dispatcher 탭 | `Ctrl+Alt+k` | `tab.dispatcher` |
| 새 탭 | `Alt+n` | `tab.new` |
| 탭 이동 왼쪽(끝에서 옆 그룹으로) | `Ctrl+Alt+Shift+H` | `tab.moveLeft` |
| 탭 이동 오른쪽(끝에서 옆 그룹으로) | `Ctrl+Alt+Shift+L` | `tab.moveRight` |
| 탭을 아래 그룹으로 | `Ctrl+Alt+Shift+J` | `tab.moveDown` |
| 탭을 위 그룹으로 | `Ctrl+Alt+Shift+K` | `tab.moveUp` |
| 탭을 임의 그룹으로 보내기(번호 선택) | `Ctrl+Alt+m` | `tab.moveTo` |

**그룹**

| 동작 | 키 | 명령 id |
|---|---|---|
| N번째 그룹으로 | `Ctrl+Alt+1..9` | `group.goto1..9` |
| 그룹 좌우 분할 | `Ctrl+Alt+\` | `group.splitRight` |
| 그룹 상하 분할 | `Ctrl+Alt+-` | `group.splitDown` |
| 그룹 합치기(대상 선택) | `Ctrl+Alt+g` | `group.mergeInto` |
| 그룹 닫기 | `Ctrl+Alt+x` | `group.close` |
| 그룹 최대화 토글 | `Ctrl+Alt+z` | `group.maximize` |

**전역**

| 동작 | 키 | 명령 id |
|---|---|---|
| 커맨드 팔레트 | `Alt+;` | `app.palette` |

기본 키가 없는(팔레트 전용) 명령: `pane.breakToTab`
<!-- KEYTABLE:END -->

앱 종료(창 닫기)는 실행 중인 에이전트가 있으면 `Y`(종료) 또는 `N`/`Esc`(취소)로 확인합니다. 실수로 닫히지 않도록 `Enter`는 이 확인 창에서 아무 동작도 하지 않습니다.

의도적으로 비워 둔 키: Claude Code가 쓰는 `Alt+B/F/D/Y/V/M/P/T/O`, `Alt+↑/↓`, `Ctrl+` 단일 글자 전부, `Ctrl+Enter`, `Shift+Tab`, `Ctrl+V`, 그리고 Windows 전역 키(`Alt+Tab/F4/Space/Esc`, `Ctrl+Shift+Esc`, Win 조합).

## 알려진 주의점

- 다른 프로그램이 `Ctrl+Alt+<글자>`를 전역 단축키로 등록했다면 그쪽이 우선합니다(앱에서 감지할 수 없음). `keybindings.json`으로 바꾸세요.
- AltGr 자판(유럽 배열)에서는 `Ctrl+Alt`가 AltGr입니다. 한글/영문 자판은 영향이 없습니다.
- `Alt+Shift`는 Windows 입력 언어 전환 키지만 글자 키와 함께 누르면 동작하지 않는 것으로 설계했습니다(실제 동작은 수동 확인 대상, `docs/spikes.md` S5).
- 앱이 가로챈 키는 창 안 프로그램에 전달되지 않습니다(예: PSReadLine의 `Alt+h`). 필요하면 프로필 `passthroughKeys`로 해제하세요.
- 창 안에서 실행한 GUI 프로그램(예: `code .`로 새로 띄운 VS Code, `claude /login`이 새로 띄운 브라우저)도 AgentBoard의 Job Object(`KILL_ON_JOB_CLOSE`)에 속하므로 **AgentBoard를 종료하면 함께 종료됩니다**. 계속 쓸 프로그램은 AgentBoard 밖에서 실행하세요.

## Claude Code 터미널 벨 설정

권한 승인 대기 등을 AgentBoard가 ⚠로 표시하려면 Claude Code가 터미널 벨(BEL)을 울려야 합니다. Claude Code에서 `/config` → 알림 채널(notification channel)을 **terminal bell**로 설정하세요(설정 값 `preferredNotifChannel: "terminal_bell"`). 이후 비활성 탭의 창이 벨을 울리면 창 헤더와 탭 바에 ⚠가 뜨고, 포커스는 옮겨지지 않습니다. `Alt+a`로 해당 창으로 이동하고, 다시 누르면 원래 위치로 돌아옵니다.

## 로드맵

1단계(현재)는 사람이 직접 조작하는 터미널 보드입니다. 2단계에서는 앱 내장 MCP 서버를 통해 Dispatcher 에이전트가 탭·창을 만들고, 다른 에이전트에 메시지를 주입하고 출력을 검토하며(프로필의 `level`·`idle`·`resume` 활용, Claude 대화 이어가기, Codex·Gemini 프로필 추가), 3단계에서는 역할 기반 팀 자동 편성과 작업 상태 대시보드 같은 오케스트레이션을 다룹니다.
