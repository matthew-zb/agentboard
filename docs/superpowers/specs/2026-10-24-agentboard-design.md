# AgentBoard — 전체 아키텍처 + 1단계(멀티플렉서) 설계 스펙

- **Date:** 2026-10-24
- **Repo:** `C:/workspace/agentboard` (신규)
- **Status:** Draft (post-brainstorming, pre-plan)
- **Scope of this spec:** 전체 아키텍처(1~3단계)는 방향만, **1단계는 구현 가능한 수준으로 상세히** 정의한다. 2·3단계는 각자 별도 스펙을 쓴다.

## 1. 목표

여러 코딩 에이전트(우선 Claude Code, 이후 Codex/Gemini CLI·셸 등)를 **tmux처럼 분할된 터미널 창**에서 동시에 구동하고, 이들을 **Dispatcher → Manager → Team Lead → 팀원** 계층의 조직으로 운영하는 Windows 데스크톱 앱.

- 탭 = 업무(작업공간 `C:/workspace/<dir>`), 탭 안 = 에이전트 창들의 분할 레이아웃
- 화면 전체는 다시 **탭 그룹**으로 분할된다 (VS Code 에디터 그룹)
- 레이아웃·팀 구성은 JSON 설정(템플릿)으로 정의하고, 팀 생성 시 설정대로 창이 만들어진다
- **모든 조작은 키보드만으로 가능**해야 한다 (마우스는 보조)

### 비목표 (전체)
- 앱이 Claude API를 직접 호출하지 않는다. 각 CLI를 PTY로 실행할 뿐이며 로그인·과금은 각 CLI 설정을 따른다.
- 원격(SSH 호스트) 세션 관리, 협업(다중 사용자) 기능.

## 2. 전체 아키텍처

### 2.1 단계 분할

| 단계 | 내용 | 결과물 |
|---|---|---|
| **1. 멀티플렉서** | Tauri + PTY + xterm.js, 탭 그룹/탭/분할 창, JSON 설정·템플릿·에이전트 프로필, 키보드 조작, 저장·복원 | 설정대로 팀 탭을 띄우는 "Windows용 tmux". 단독으로 쓸모 있음 |
| **2. 조율 API** | 앱 내장 로컬 MCP 서버, 조직 레지스트리, 메시지 버스, hook 연동(idle·세션 id), 기능 수준 검사 | 에이전트가 탭·에이전트를 생성하고 서로 메시지를 주고받음 |
| **3. 조직 역할** | Dispatcher/Manager/Lead/팀원 역할 프롬프트, 고용·보고 규칙, 기본 템플릿, Dispatcher 기본 탭 운영 | 계층형 에이전트 조직 |

### 2.2 구성 요소

```
┌──────────────────────── Tauri 앱 (AgentBoard) ────────────────────────────┐
│ Frontend (vanilla JS)                                                      │
│  • 그룹 레이아웃 → 그룹별 탭 바 → 탭별 창 레이아웃 → 창마다 xterm.js        │
│  • 키보드 라우터, 커맨드 팔레트, 오버레이(창 번호/그룹 번호/드롭 영역)       │
│  • 사이드바: 1단계=그룹/탭/창 목록, 2단계=조직 트리                          │
├────────────────────────────────────────────────────────────────────────────┤
│ Rust backend                                                               │
│  ① PtyManager     창별 ConPTY 프로세스 생성·입력·리사이즈·종료·출력 링버퍼   │  1단계
│  ② Workspace      그룹/탭/레이아웃 트리 모델(단일 진실 원천), 설정 로드·검증, │  1단계
│                   저장·복원                                                │
│  ③ Profiles       에이전트 프로필 로드, 실행 명령 조립, ready/submit 처리    │  1단계(일부)
│  ④ OrgRegistry    에이전트 id·역할·상위자·상태                              │  2단계
│  ⑤ ControlServer  127.0.0.1 MCP 서버, 에이전트별 토큰, 역할별 권한           │  2단계
│  ⑥ MessageBus     메시지 큐 → 대상 idle 시 PTY 주입                          │  2단계
└────────────────────────────────────────────────────────────────────────────┘
```

**핵심 원칙**: 레이아웃/탭/그룹 모델은 **Rust가 소유**한다. 프론트엔드는 렌더링과 입력 처리만 한다. 그래야 2단계에서 MCP 도구(에이전트)가 프론트엔드와 같은 명령 함수로 탭·창을 만들 수 있다. 키보드·마우스·MCP 세 경로가 모두 같은 명령(`create_tab`, `split_pane`, `move_tab` …)을 호출한다.

### 2.3 2·3단계 방향 (요약, 별도 스펙에서 상세화)

- **조직**: Dispatcher(항상 존재, 루트 작업공간) → 업무 탭마다 Manager → Manager가 Team Lead 고용 → Lead가 팀원 고용. 팀원↔Lead, Lead↔Manager, Manager↔Dispatcher로만 소통.
- **MCP 도구**(역할별 권한을 앱이 강제): `create_tab`, `update_config`(Dispatcher), `hire`, `dismiss`, `send_message`(직속 상하만), `list_team`, `read_output`(하위 에이전트 출력 읽기).
- **메시지 전달**: 대상 창 PTY에 `[from: <agent>] …`로 주입. 대상이 작업 중이면 큐에 두고 idle 신호(hook 또는 출력 패턴) 시 주입. 사람이 흐름을 그대로 볼 수 있다.
- **고용된 에이전트의 창 위치**: 템플릿 레이아웃의 `slot`(예: `members`)에 추가.
- **창을 다른 탭에 붙이기(join)**: 조직 소속 변경 규칙과 함께 2단계에서 정의.
- 포커스는 절대 자동으로 옮기지 않는다(§6.4).

## 3. 기술 스택

| 영역 | 선택 | 이유 |
|---|---|---|
| 앱 셸 | Tauri v2 | mdviewer와 동일 스택. 구조·CI·서명 재사용, 배포 ~10MB |
| 프론트엔드 | vanilla JS (번들러 없음) | mdviewer와 동일 |
| 터미널 렌더링 | xterm.js + addon-fit, addon-unicode11(한글 폭), addon-search, addon-webgl(실패 시 canvas) | 표준 |
| PTY | `portable-pty` (Windows ConPTY) | WezTerm 검증, macOS 확장 가능 |
| 비동기 | tokio | 2단계 MCP 서버와 공유 |

1차 대상 OS는 Windows 11. macOS는 구조상 막지 않지만 1단계 범위 밖.

## 4. 설정 파일

### 4.1 파일 위치

| 파일 | 위치 | 편집 주체 |
|---|---|---|
| `settings.json` | `%APPDATA%/agentboard/` | 사람. 부트스트랩 전용: `{ "workspaceRoot": "C:/workspace" }` |
| `config.json` | `<workspaceRoot>/.agentboard/` | 사람, 2단계부터 Dispatcher |
| `templates/*.json` | `<workspaceRoot>/.agentboard/templates/` | 사람, 2단계부터 Dispatcher |
| `agents/*.json` | `<workspaceRoot>/.agentboard/agents/` | 사람 |
| `keybindings.json` | `<workspaceRoot>/.agentboard/` | 사람 (기본값 위에 덮어쓰기) |
| `workspace.json` | `%APPDATA%/agentboard/` | 앱 (실행 상태, 사람이 편집하지 않음) |

설정 파일을 `workspaceRoot` 아래에 두는 이유: Dispatcher의 작업공간이 `workspaceRoot`이므로 2단계에서 Dispatcher가 설정을 직접 읽고 고칠 수 있다.

기본 `templates/`, `agents/`, `keybindings.json`은 앱에 내장되어 있고, 첫 실행 시 `.agentboard/`가 없으면 생성한다. 사용자 파일이 같은 이름이면 사용자 파일이 우선한다.

### 4.2 `config.json`

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

- **Dispatcher 탭**: 이름 `dispatcher` 예약, 작업공간 = `workspaceRoot`. 항상 존재하며 닫을 수 없다(다른 그룹으로 이동은 가능).
- **업무 탭**: 작업공간 = `<workspaceRoot>/<dir>`. 탭 이름은 전역에서 유일. `dir`이 없으면 생성 여부를 묻는다.
- `groups`는 첫 실행(또는 `workspace.json`이 없을 때)의 초기 배치. 이후에는 `workspace.json`의 실제 배치가 우선한다. `groups`에 없는 `tabs` 항목은 `defaultGroup`에 열린다.
- 새 탭(사용자 생성)은 **현재 포커스된 그룹**에 열린다. 2단계 Dispatcher 생성 탭은 `defaultGroup` 또는 지정 그룹.

### 4.3 템플릿 `templates/<name>.json`

```json
{
  "name": "backend-team",
  "layout": {
    "split": "horizontal", "sizes": [60, 40],
    "children": [
      { "pane": { "name": "lead", "agent": "claude", "args": ["--model", "opus"],
                  "cwd": "", "prompt": "너는 팀 리드다...", "focus": true } },
      { "split": "vertical", "slot": "members",
        "children": [
          { "pane": { "name": "coder",  "agent": "claude" } },
          { "pane": { "name": "tests",  "agent": "powershell", "cwd": "server",
                      "prompt": "npm test -- --watch" } }
        ] }
    ]
  }
}
```

레이아웃 노드는 둘 중 하나:
- **split 노드**: `split`(`horizontal` = 좌|우, `vertical` = 위/아래), `children`(2개 이상), `sizes`(퍼센트, 생략 시 균등, 합이 100이 아니면 정규화), `slot`(선택, 2단계용 — 1단계에서는 저장만 하고 무시)
- **pane 노드**: `pane` 객체

pane 필드:

| 필드 | 필수 | 설명 |
|---|---|---|
| `name` | ✅ | 탭 안에서 유일 |
| `agent` | | 에이전트 프로필 이름. 생략 시 `config.defaultAgent` |
| `args` | | 프로필 `args` 뒤에 덧붙일 인자 |
| `env` | | 추가 환경 변수 |
| `cwd` | | **탭 작업공간 기준 상대경로**. 생략/빈 문자열 = 탭 작업공간. 절대경로 금지(검증 오류) |
| `prompt` | | 프로세스가 ready 상태가 되면(§4.4 `ready`) 한 번 입력·제출 |
| `focus` | | 탭이 처음 열릴 때 포커스할 창. 없으면 첫 pane |

### 4.4 에이전트 프로필 `agents/<name>.json`

도구별 차이를 흡수하는 계층. 템플릿은 `cmd`를 직접 쓰지 않고 프로필 이름만 참조한다.

```json
{
  "name": "claude",
  "level": "L2",
  "cmd": "claude",
  "args": [],
  "env": {},
  "ready":  { "idleMs": 800 },
  "submit": { "bracketedPaste": true, "key": "\r" },
  "passthroughKeys": [],
  "systemPrompt": { "arg": "--append-system-prompt" },
  "mcp":    { "arg": "--mcp-config", "format": "claude-json" },
  "idle":   { "via": "hook" },
  "resume": { "via": "hook-session-id", "arg": "--resume" }
}
```

| 필드 | 1단계 사용 | 설명 |
|---|---|---|
| `cmd`, `args`, `env` | ✅ | 실행 명령. `cmd`는 PATH 탐색(`.exe/.cmd/.bat` 포함) |
| `ready` | ✅ | `prompt` 입력 시점. `{ "pattern": "<regex>" }`(출력에 매칭되면) 또는 `{ "idleMs": n }`(첫 출력 후 n ms 무출력). 10초 타임아웃 후에는 그냥 입력하고 창 헤더에 경고 |
| `submit` | ✅ | 텍스트 제출 방식. `bracketedPaste`면 `ESC[200~ … ESC[201~`로 감싸고 `key` 전송. 여러 줄 `prompt`와 2단계 메시지 주입이 같은 방식 사용 |
| `passthroughKeys` | ✅ | 앱 단축키 중 이 창에서는 가로채지 않고 프로그램에 넘길 키 (예: `"Alt+h"`) |
| `level` | 저장만 | `L2`(MCP+hook+resume) / `L1`(MCP) / `L0`(수동). 2단계 역할 배정 검사에 사용 |
| `systemPrompt`, `mcp`, `idle`, `resume` | 형식만 | 2단계에서 사용. 1단계는 파싱·검증만 |

**기능 수준과 역할(2단계 적용)**

| 수준 | 조건 | 가능한 역할 |
|---|---|---|
| L2 | MCP + hook + resume (Claude Code) | 모든 역할 |
| L1 | MCP 지원 (Codex, Gemini 등) | 모든 역할, idle 감지는 출력 추정 |
| L0 | MCP 없음 (aider, 셸 등) | 팀원만. Lead가 `read_output`으로 출력 검토 |

**내장 기본 프로필(1단계)**: `claude`, `powershell`(`pwsh` 없으면 `powershell.exe`), `cmd`. `codex`, `gemini`는 2단계에서 각 CLI 문서를 확인해 추가한다(지금 추측으로 만들지 않는다).

### 4.5 `keybindings.json`

```json
{ "pane.focusLeft": "Alt+h", "group.split.right": "Ctrl+Alt+\\" }
```

명령 id → 키. 기본값(§6.2) 위에 덮어쓴다. `null`이면 해제. 충돌(두 명령이 같은 키)은 로드 시 오류로 표시하고 기본값 유지.

### 4.6 검증

모든 JSON은 로드 시 Rust에서 스키마 검증한다. 오류는 `파일:줄:열 — 원인` 형태로 상태 표시줄과 알림 패널에 보여주고, 잘못된 템플릿으로는 탭을 만들지 않는다. `config.json`이 깨졌으면 Dispatcher 탭만 띄우고 오류를 표시한다.

## 5. 데이터 모델 (Rust, 단일 진실 원천)

```
Workspace {
  groupLayout: LayoutNode<GroupId>      // 그룹 분할 트리
  groups:      Map<GroupId, Group>
  tabs:        Map<TabId, Tab>
  panes:       Map<PaneId, Pane>
  focus:       { groupId }                // 마지막 포커스 그룹
  lastLocation: { groupId, tabId } | null // Ctrl+Alt+j 용
}
Group  { id, name, tabIds: TabId[], activeTabId }
Tab    { id, name, dir, template, isDispatcher, layout: LayoutNode<PaneId>,
         lastFocusedPaneId, maximizedPaneId | null, syncInput: bool }
Pane   { id, name, agent, args, env, cwd(절대경로로 해석됨), prompt,
         status: starting | running | exited(code) , attention: bool }
LayoutNode<T> = Split { dir, sizes: f32[], children: LayoutNode<T>[], slot? }
              | Leaf  { id: T }
```

- 그룹 레이아웃과 탭 내 창 레이아웃은 **같은 `LayoutNode<T>` 구현**을 쓴다(분할, 제거, 크기 조절, 방향 이웃 찾기, 정규화).
- 레이아웃 연산은 순수 함수로 구현해 단위 테스트한다.
- 프론트엔드는 모델 변경 시 `workspace-changed` 이벤트로 스냅샷을 받아 렌더링한다. xterm 인스턴스는 `PaneId`로 유지·재사용하며, 탭/그룹 이동 시 DOM 노드만 옮기고 터미널을 재생성하지 않는다.

## 6. 화면과 조작

### 6.1 화면 구성

```
┌─ control ─────┬─ work-A ───────────────────────────────┐
│[Dispatcher]   │[api] [mdviewer]                         │
│ dispatcher    │ lead ●         │ coder ●                │
│               │                ├────────────            │
│               │                │ tests ⚠                │
│               ├─ work-B ───────────────────────────────┤
│               │[ha] [ota-loop]                          │
│               │ manager ●                               │
└───────────────┴─────────────────────────────────────────┘
 work-A › api › lead                         [SYNC] [키 오류 1]
```

- **그룹**: 각자 탭 바 보유. 포커스된 그룹의 탭 바는 강조, 나머지는 흐리게.
- **탭 바**: 탭 이름, 주의 배지(탭 안에 ⚠ 창이 있으면), Dispatcher 탭은 고정 아이콘.
- **창 헤더**: 이름, 프로필, 상태(● 실행 / ○ 종료(코드) / ◌ 시작 중), ⚠ 주의, 버튼(⟳ 재시작, ✕ 닫기). 포커스 창은 굵은 테두리.
- **상태 표시줄**: `그룹 › 탭 › 창`, 입력 동기화 표시, 설정 오류 개수.
- **사이드바**(토글): 그룹 > 탭 > 창 트리. 2단계에서 조직 트리로 확장.

### 6.2 키보드 (기본값)

모든 앱 단축키는 xterm보다 먼저 처리(`attachCustomKeyEventHandler` + 문서 capture 단계)하며, 매칭되지 않은 키는 그대로 PTY로 전달된다. 키 판별은 **`KeyboardEvent.code`**로 한다(한글 IME 상태에서 `e.key`가 `ㅗ` 등으로 들어오므로).

**창**

| 동작 | 키 | 명령 id |
|---|---|---|
| 창 포커스 이동(그룹 경계 넘어감) | `Alt+h/j/k/l` | `pane.focusLeft/Down/Up/Right` |
| 크기 조절(탭 내 경계가 없으면 그룹 경계) | `Alt+Shift+H/J/K/L` | `pane.resize*` |
| 좌우 / 상하 분할 | `Alt+\` / `Alt+-` | `pane.splitRight` / `pane.splitDown` |
| 최대화 토글 | `Alt+z` | `pane.maximize` |
| 닫기 | `Alt+x` | `pane.close` |
| 재시작 | `Alt+r` | `pane.restart` |
| 창 번호 표시 후 이동 | `Alt+q` | `pane.pick` |
| 주의 창으로 이동(다시 누르면 복귀) | `Alt+a` | `pane.nextAttention` |
| 복사·스크롤 모드 | `Alt+c` | `pane.copyMode` |
| 탭 내 입력 동기화 토글 | `Alt+i` | `tab.syncInput` |
| 텍스트 붙여넣기 | `Ctrl+Shift+V` | `pane.paste` |

**탭**

| 동작 | 키 | 명령 id |
|---|---|---|
| 이전 / 다음 탭 (그룹 내) | `Ctrl+Alt+h` / `Ctrl+Alt+l` | `tab.prev` / `tab.next` |
| N번째 탭 (그룹 내) | `Alt+1..9` | `tab.goto1..9` |
| 직전 위치(탭·그룹 왕복) | `Ctrl+Alt+j` | `tab.last` |
| Dispatcher 탭 | `Ctrl+Alt+k` | `tab.dispatcher` |
| 새 탭 | `Alt+n` | `tab.new` |
| 탭 이동: 순서/옆 그룹 | `Ctrl+Alt+Shift+H/L`(탭 바 끝에서 옆 그룹으로), `Ctrl+Alt+Shift+J/K`(아래/위 그룹) | `tab.move*` |
| 탭을 임의 그룹으로 보내기 | `Ctrl+Alt+m` → 숫자 / `hjkl` / `\`·`-`(새 그룹) / `Esc` | `tab.moveTo` |
| 현재 창을 새 탭으로 떼어내기 | 팔레트 전용 | `pane.breakToTab` |

**그룹**

| 동작 | 키 | 명령 id |
|---|---|---|
| N번째 그룹으로 | `Ctrl+Alt+1..9` | `group.goto1..9` |
| 그룹 분할(현재 탭을 새 그룹으로 이동) | `Ctrl+Alt+\` / `Ctrl+Alt+-` | `group.splitRight` / `group.splitDown` |
| 그룹 합치기(대상 선택) | `Ctrl+Alt+g` → 숫자 / `hjkl` / `Esc` | `group.mergeInto` |
| 그룹 닫기(레이아웃상 형제 그룹과 합침) | `Ctrl+Alt+x` | `group.close` |
| 그룹 최대화 토글 | `Ctrl+Alt+z` | `group.maximize` |

**전역**: 커맨드 팔레트 `Alt+;` (`app.palette`) — 모든 명령, 탭·창·그룹 이름 퍼지 검색, 탭 이름 변경, "탭 이동 → <그룹>", "그룹 합치기 → <그룹>".

**의도적으로 비워둔 키**: Claude Code가 쓰는 `Alt+B/F/D/Y/V/M/P/T/O`, `Alt+↑/↓`, `Ctrl+` 단일 글자 전부, `Ctrl+Enter`, `Shift+Tab`, `Ctrl+V`. Windows 전역 키(`Alt+Tab/F4/Space/Esc`, `Ctrl+Shift+Esc`, Win 조합)도 사용하지 않는다.

**알려진 주의점**
- 다른 프로그램이 `Ctrl+Alt+<글자>`를 전역 단축키로 등록했다면 그쪽이 우선한다(앱에서 감지 불가) → `keybindings.json`으로 변경.
- AltGr 자판(유럽)에서는 `Ctrl+Alt`가 AltGr이다. 한글/영문 자판은 무관.
- `Alt+Shift`는 Windows 입력 언어 전환 키지만 글자 키와 함께 누르면 동작하지 않는다(스파이크로 확인, §10).
- 앱이 가로챈 키는 창 안 프로그램에 전달되지 않는다(예: PSReadLine `Alt+h`). 필요하면 프로필 `passthroughKeys`로 해제.

### 6.3 모드 (키보드 전용 흐름)

- **창 번호(`Alt+q`) / 그룹 번호(`Ctrl+Alt+m`, `Ctrl+Alt+g`)**: 모든 대상에 큰 번호 오버레이 → 숫자 또는 `hjkl` → 실행. `Esc` 취소. 2초 무입력 시 자동 취소.
- **새 탭(`Alt+n`)**: ① `workspaceRoot` 하위 디렉토리 퍼지 목록(새 이름 입력 시 생성 확인) → ② 템플릿 목록 → Enter. 모두 방향키/Enter/Esc.
- **복사 모드(`Alt+c`)**: 창 테두리 색 변경, 입력은 PTY로 가지 않음. `h/j/k/l`·방향키·`PgUp/PgDn`·`g/G` 이동, `v` 선택 시작, `y` 클립보드 복사 후 종료, `/` 검색(`n/N`), `Esc`/`q` 종료. 출력 스크롤백은 xterm 버퍼(기본 10,000줄).
- **입력 동기화(`Alt+i`)**: 탭 안 모든 실행 중 창에 같은 키 입력 전달. 상태 표시줄 `[SYNC]`. 탭 전환해도 탭별로 유지.

### 6.4 포커스 규칙

포커스는 `그룹 → 그룹의 활성 탭 → 탭의 lastFocusedPaneId` 체인으로 기억하며, 포커스가 정해지면 해당 xterm에 실제 DOM 포커스(`term.focus()`)를 준다.

| 상황 | 포커스 |
|---|---|
| 탭 전환 | 그 탭의 마지막 포커스 창 (같은 그룹 내에서만 탭 교체, 다른 그룹 화면 유지) |
| 처음 열린 탭 | 템플릿 `focus: true` 창, 없으면 첫 창 |
| 마지막 포커스 창이 닫힘 | 형제 창 → 없으면 레이아웃상 가장 가까운 창 |
| 최대화 상태로 떠난 탭 | 최대화 유지, 그 창으로 |
| 복사 모드 중 떠난 창 | 복사 모드 유지 |
| 팔레트·오버레이 열린 채 전환 | 닫고 대상 창으로 |
| 탭의 모든 창 종료 | 마지막 창의 "재시작" 버튼(Enter 실행) |
| 앱 창 재활성화(Alt+Tab 등) | 현재 그룹·탭의 마지막 포커스 창 |
| 탭 이동/드롭/그룹 합치기 후 | 옮겨진 탭의 마지막 포커스 창 |

**포커스 자동 이동 금지**: 권한 승인 대기, 작업 완료, (2단계) 메시지 도착 등 어떤 이벤트도 포커스나 탭을 옮기지 않는다 — 사람이 입력 중인 글자가 다른 에이전트로 들어가는 사고를 막기 위함. 대신 창 ⚠·탭 배지·`Alt+a`로 안내한다.

**위치 표시**: 탭/그룹 전환 시 포커스 창 테두리 300ms 1회 하이라이트, 상태 표시줄 경로 갱신.

### 6.5 주의(attention) 감지

1단계: 창 출력에 BEL(`\x07`)이 오면 그 창을 `attention=true`로 표시(포커스를 받으면 해제). Claude Code는 알림 채널을 터미널 벨로 설정해 사용한다. 2단계: Notification hook으로 정확히 감지.

### 6.6 마우스 (보조)

마우스 동작은 키보드와 **같은 Rust 명령**(`move_tab`, `split_group_with`)을 호출한다.

| 끌어다 놓는 곳 | 결과 |
|---|---|
| 같은 탭 바의 다른 위치 | 순서 변경 (mdviewer `tabs.js` 드래그 코드 재사용) |
| 다른 그룹의 탭 바 | 그 그룹의 그 위치로 이동 |
| 그룹 본문 가운데 | 그 그룹 끝에 추가 |
| 그룹 본문 상/하/좌/우 가장자리(각 25%) | 그 방향으로 새 그룹 만들어 이동 |

- 드래그 시작 시 모든 그룹 위에 투명 드롭 오버레이를 깔아 xterm의 마우스 선택과 충돌을 막고, 드롭 영역을 반투명 미리보기로 표시. 드래그 종료 시 제거.
- `draggable`로 인한 dblclick 불안정은 mdviewer처럼 타임스탬프로 직접 판정.
- 기타 마우스: 클릭으로 창 포커스, 경계 드래그로 크기 조절(창·그룹), 탭 클릭 전환, 헤더 버튼. 창 자체를 끌어 옮기는 기능은 없음.

## 7. 백엔드 상세

### 7.1 PtyManager (`pty.rs`)

```
spawn(pane_id, cmd, args, env, cwd, cols, rows) -> Result<()>
write(pane_id, bytes)
resize(pane_id, cols, rows)
kill(pane_id)
snapshot(pane_id) -> Vec<u8>          // 링버퍼 내용 (재연결·2단계 read_output)
```

- `portable-pty` native PTY(ConPTY). 프로세스별 reader 스레드 → 창별 버퍼에 누적 → **16ms 주기로 묶어** Tauri `Channel`로 프론트엔드에 전송.
- 창별 **1MB 링버퍼**를 항상 유지(렌더링 여부와 무관).
- 모든 자식 프로세스는 Windows **Job Object**(`KILL_ON_JOB_CLOSE`)에 넣어 앱 종료·크래시 시 프로세스 트리까지 정리.
- 종료 감지 시 `pane-exited(pane_id, code)` 이벤트.
- 이 API는 Tauri 명령 외에 **Rust 내부 API로도 노출**한다(2단계 MCP 서버가 직접 호출).

### 7.2 Workspace (`workspace.rs`, `layout.rs`, `config.rs`, `profiles.rs`)

- 설정·템플릿·프로필 로드/검증, 템플릿 → 탭 + 창 인스턴스화(프로필로 명령 조립, `cwd` 해석).
- 명령(모두 Tauri command + 내부 API): `create_tab`, `close_tab`, `rename_tab`, `split_pane`, `close_pane`, `restart_pane`, `resize_split`, `break_pane_to_tab`, `move_tab(tab, group, index)`, `split_group_with(tab, group, dir)`, `merge_group(src, dst)`, `close_group`, `set_focus(...)`, `set_maximized`, `set_sync_input`.
- 그룹이 비면 자동 제거, Dispatcher 탭 닫기 거부.
- `prompt` 주입: 프로필 `ready` 조건 감시 → `submit` 방식으로 전송.

### 7.3 저장·복원

- 모델 변경 후 500ms 디바운스로 `workspace.json` 저장(원자적 쓰기: 임시 파일 → rename).
- 복원: 그룹/탭/레이아웃/포커스/최대화/동기화 상태 복원 후 각 창 프로세스를 **다시 실행**(`prompt`는 재주입하지 않음). Claude 대화 이어가기(`--resume`)는 2단계.
- `workspace.json`이 깨졌으면 `.bak`로 옮기고 `config.json` 초기 배치로 시작.

### 7.4 WebView 설정

- WebView2 **브라우저 단축키 비활성화**(`ICoreWebView2Settings3::put_AreBrowserAcceleratorKeysEnabled(false)`, Tauri `with_webview`로 접근). `Ctrl+R`(새로고침)·`Ctrl+P`(인쇄)·`F5` 등이 Claude Code 키(`Ctrl+R` 기록 검색, `Ctrl+P` 이전 기록)를 가로채는 문제 방지.
- 컨텍스트 메뉴 비활성화, 릴리스 빌드에서 devtools 비활성화.

## 8. 오류 처리

| 상황 | 동작 |
|---|---|
| `cmd`를 찾을 수 없음 / 실행 실패 | 창에 오류 메시지 + "재시작"(Enter) + "프로필 열기" 안내 |
| 프로세스 종료 | 헤더 ○ 종료(코드), 창 하단에 "Enter: 재시작" |
| 템플릿/프로필/설정 검증 실패 | 탭 생성 거부, `파일:줄:열 — 원인` 표시 |
| `ready` 10초 타임아웃 | `prompt`를 그냥 입력, 헤더에 경고 아이콘 |
| 키바인딩 충돌·잘못된 키 | 해당 항목 무시·기본값 유지, 상태 표시줄 `[키 오류 N]` |
| `dir` 없음 | 생성 확인 대화(키보드로 Y/N) |
| 앱 종료 | 실행 중 창 수 표시 후 확인(키보드 Y/N), Job Object로 일괄 정리 |

## 9. 저장소 구조

```
agentboard/
  src/
    index.html  style.css
    app.js           // 부트스트랩, 이벤트 연결
    keys.js          // 키 라우터, keybindings 로드, e.code 매칭
    groups.js        // 그룹 레이아웃 렌더링
    tabs.js          // 탭 바 (mdviewer에서 이식), 드래그앤드롭
    layout.js        // LayoutNode 렌더링(분할 DOM, 경계 드래그)
    pane.js          // xterm 래퍼, 헤더, 복사 모드
    overlays.js      // 번호 선택, 드롭 미리보기, 팔레트, 새 탭 마법사
    vendor/          // xterm.js 및 addon (고정 버전)
  src-tauri/src/
    lib.rs  main.rs
    pty.rs  layout.rs  workspace.rs  config.rs  profiles.rs  persist.rs  winjob.rs
  src-tauri/defaults/   // 내장 templates/, agents/, keybindings.json
  docs/superpowers/specs/
```

mdviewer에서 가져올 것: Tauri 설정 골격, GitHub Actions 빌드·서명 워크플로, 아이콘 처리, `tabs.js` 드래그/더블클릭 로직.

## 10. 위험 요소와 초기 스파이크

구현 첫 단계에서 아래를 작은 시제품으로 먼저 확인한다. 실패 시 대안을 적용하고 스펙을 갱신한다.

| # | 확인 항목 | 실패 시 대안 |
|---|---|---|
| S1 | Alt 단독 누름/뗌 시 시스템 메뉴 포커스 이동·경고음 여부, `Alt+글자` 경고음 | keyup에서 `preventDefault`, Rust 측 `WM_SYSKEYDOWN/WM_SYSCHAR` 처리 |
| S2 | WebView2 브라우저 단축키 비활성화가 Tauri v2에서 동작 | keydown capture에서 해당 키 `preventDefault` 후 PTY로 직접 전송 |
| S3 | ConPTY + xterm.js에서 Claude Code TUI 렌더링(한글 폭, 색, 커서, 리사이즈) | unicode11 addon 설정 조정, ConPTY 플래그 조정 |
| S4 | 9개 창 동시 출력 성능(3×3 분할, Claude 9개) | 배치 주기·WebGL 렌더러·비가시 탭 렌더링 중단 |
| S5 | `Alt+Shift+글자`로 입력 언어가 바뀌지 않는지 | 크기 조절을 모드 방식(`Alt+s` → `hjkl` 반복 → `Esc`)으로 대체 |
| S6 | HTML5 드래그앤드롭이 xterm 위에서 동작(오버레이 방식) | 마우스 이벤트로 직접 구현 |

## 11. 테스트

- **Rust 단위 테스트**: `LayoutNode` 연산(분할·제거·정규화·방향 이웃·크기 조절 전파), 그룹 연산(이동·합치기·자동 제거·Dispatcher 보호), 설정·템플릿·프로필·키바인딩 파싱/검증(오류 위치 포함), `cwd` 해석, 링버퍼, `ready` 패턴/idle 판정, `submit` 인코딩.
- **Rust 통합 테스트**: PTY로 `cmd /c echo`, `ping -n 3` 등 실행 → 출력·리사이즈·종료 코드·Job Object 정리 확인.
- **프론트엔드**: 키 라우터의 `e.code` 매칭·passthrough 로직은 순수 함수로 분리해 브라우저 없이 테스트(node 스크립트).
- **수동 QA 체크리스트**
  1. **마우스 없이**: 새 탭 생성 → 분할 → 창 이동(그룹 경계 포함) → 크기 조절 → 복사 → 탭 전환 → 탭을 다른 그룹으로 이동 → 그룹 합치기 → 닫기
  2. 한글 IME 켠 상태에서 모든 단축키 동작
  3. Claude Code에서 `Ctrl+R`, `Ctrl+P`, `Alt+P`, `Alt+V`, `Shift+Tab`, `Ctrl+V`(이미지)가 Claude에 전달됨
  4. 3×3 Claude 동시 실행 성능
  5. 앱 종료 → 재실행 시 그룹/탭/레이아웃/포커스 복원, 고아 프로세스 없음(작업 관리자 확인)
  6. 마우스 드래그로 4가지 드롭 동작
  7. 권한 승인 대기 시 ⚠ 표시, 포커스는 이동하지 않음, `Alt+a`로 이동·복귀

## 12. 1단계 범위 밖 (명시)

- MCP 서버, 조직 레지스트리, 메시지 버스, hook 연동, `--resume` 대화 복원 → 2단계
- 창을 다른 탭에 붙이기(join), 고용 시 `slot` 배치 → 2단계
- 역할 프롬프트, Dispatcher의 설정 편집 → 3단계
- `codex`, `gemini` 내장 프로필 → 2단계(문서 확인 후)
- macOS 빌드, 창(에이전트) 마우스 드래그, 테마 커스터마이즈
