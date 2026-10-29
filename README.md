# AgentBoard

Claude Code 같은 CLI 에이전트를 **그룹 → 탭 → 분할 창** 구조로 한 화면에 띄우고, 마우스 없이 키보드만으로 조작하는 Windows 데스크톱 앱입니다. Tauri v2 (Rust + 시스템 WebView2) + 바닐라 JS + xterm.js. 버전별 변경 내역은 [History.md](History.md)를 참고하세요.

## 주요 기능

- 탭 그룹(좌우/상하 분할) 안에 탭, 탭 안에 분할 창 — 창마다 ConPTY로 실행되는 독립 프로세스(Claude Code, PowerShell, cmd …)
- 모든 조작이 키보드로 가능 (창 이동·크기 조절·분할·탭/그룹 이동·합치기·복사 모드·커맨드 팔레트). 마우스 드래그로 탭을 그룹 사이로 옮길 수도 있음
- 한글 IME 상태에서도 동작하도록 `KeyboardEvent.code` 기준으로 키를 판별
- 입력 동기화(탭 안 모든 창에 같은 입력), 터미널 벨 기반 주의(⚠) 표시와 `Alt+a` 이동
- 종료 후 재실행하면 그룹·탭·레이아웃·포커스를 복원(각 창의 프로세스는 다시 실행)
- 앱 종료 시 실행 중인 에이전트 수를 보여주고 확인, Job Object로 자식 프로세스 트리까지 정리
- (2단계) 상시업무 운영: dispatcher·handover·업무별 Manager·임시 팀(리드·팀원) 역할 에이전트, 앱 내장 MCP 서버, dispatcher 옆 인박스 애플릿, 마크다운 문서 탭, 야간 정기 정리, 로그인 시 자동 실행 — 아래 [2단계](#2단계--상시업무-운영-역할-에이전트) 참고

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
| `settings.json` | `%APPDATA%/agentboard/` | 부트스트랩 전용. `{ "workspaceRoot": "D:/work" }` |
| `workspace.json` | `%APPDATA%/agentboard/` | 앱이 저장하는 실행 상태(직접 편집하지 않음). 깨지면 `.bak`로 옮기고 `config.json` 배치로 시작 |
| `config.json` | `<workspaceRoot>/.agentboard/` | 초기 탭·그룹 배치 |
| `templates/*.json` | `<workspaceRoot>/.agentboard/templates/` | 탭 템플릿(분할 레이아웃) |
| `agents/*.json` | `<workspaceRoot>/.agentboard/agents/` | 에이전트 프로필 |
| `keybindings.json` | `<workspaceRoot>/.agentboard/` | 기본 키 위에 덮어쓰기 |
| `roles/*.md` | `<workspaceRoot>/.agentboard/roles/` | (2단계) 역할 지시 파일 |
| `inbox/*.json` | `<workspaceRoot>/.agentboard/inbox/` | (2단계) 인박스 항목(앱이 저장) |
| `nightly.json` | `%APPDATA%/agentboard/` | (2단계) 야간 정리를 마지막으로 돈 날짜 |

첫 실행 시 `.agentboard/`가 없으면 내장 기본값(`claude`·`cmd`·`powershell` 프로필, `dispatcher`·`dispatcher-inbox`·`handover`·`manager`·`solo`·`team-3` 템플릿, 역할 지시 `roles/*.md`, 키 설정, 그리고 dispatcher 옆에 인박스를 두는 시작용 `config.json`)으로 생성합니다. 이미 있는 `.agentboard/`에는 빠진 기본 파일만 추가하고 사용자 파일은 절대 덮어쓰지 않습니다(`config.json`은 새로 설치할 때만 만듭니다). 같은 이름의 사용자 파일이 우선합니다. 모든 JSON은 로드 시 검증하며, 오류는 `파일:줄:열 — 원인` 형태로 상태 표시줄 `[설정 오류 N]`(클릭하거나 팔레트에서 열기)에 표시됩니다.

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

- `dispatcher` 탭은 이름이 예약되어 있고 작업공간이 `workspaceRoot`이며 닫을 수 없습니다. `agentboard`(대소문자 무관)도 앱이 메시지를 보낼 때 쓰는 이름이라 탭·팀 이름으로 쓸 수 없습니다.
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

- `ready`: `{ "pattern": "<regex>" }`(출력에 매칭) 또는 `{ "idleMs": n }`(첫 출력 후 n ms 무출력). 10초가 지나면 그냥 입력하고 창 헤더에 경고를 표시합니다. 역할 창(2단계)은 다릅니다: 그 세션의 SessionStart hook이 온 뒤에만 이 규칙으로 시작 프롬프트를 넣고, SessionStart가 없으면 넣지 않습니다(폴더 신뢰 질문에 Enter가 들어가지 않도록).
- `passthroughKeys`: 앱 단축키 중 이 창에서는 가로채지 않고 프로그램에 넘길 키(예: `"Alt+h"`).
- `level`, `systemPrompt`, `mcp`, `idle`, `resume`은 파싱·검증만 하고 쓰지 않습니다. 역할 세션의 지시·MCP·인자는 config.json `roles`와 앱이 정합니다(2단계).

### keybindings.json

```json
{ "pane.focusLeft": "Alt+h", "group.splitRight": "Ctrl+Alt+\\", "pane.close": null }
```

명령 id → 키. `null`은 해제입니다. 두 명령이 같은 키를 쓰면 오류로 표시하고 기본값을 유지합니다.

## 2단계 — 상시업무 운영 (역할 에이전트)

사용자는 dispatcher 창에만 머무르고, 상시업무마다 탭 하나에 **Manager** 에이전트가 늘 떠 있습니다. 바쁠 때 Manager가 같은 탭에 **팀 리드**를, 리드가 **팀원**을 채용하고 끝나면 해산합니다. 보고는 dispatcher 옆 **인박스 애플릿**에 모이고, 사용자는 거기서 지시·자료 저장·이슈 등록·종료 확인을 합니다. 새벽에는 **야간 정리**로 상시 세션을 새 세션으로 바꾸고 문서로 이어갑니다.

### 사전 요구사항

- **Claude Code 2.1.234 이상** (Windows 세션 간 메시징). `claude update`로 올립니다. 미달이면 상태 표시줄에 오류가 뜨고 에이전트끼리 메시지(SendMessage)만 안 됩니다. 탭·MCP·인박스는 동작합니다.
- **Git for Windows**(Git Bash). 앱이 역할 세션에 넣는 hook(`"<agentboard.exe>" --hook <이벤트>`)을 Claude Code가 Git Bash로 실행합니다. 없으면 앱→세션 메시지·⚠ 같은 hook 기반 기능이 빠지고, 역할 창의 시작 프롬프트도 들어가지 않습니다(SessionStart를 기다리므로, 직접 입력해야 함). 설치: `winget install --id Git.Git -e`.
- **폴더 신뢰 — 작업 폴더마다 한 번.** 역할 세션을 처음 띄우는 폴더에서는 Claude Code가 "이 폴더를 신뢰하시겠습니까?"를 묻고 멈춥니다. 그 창으로 가서 직접 답하세요. 앱은 Claude의 신뢰 설정(`~/.claude.json`)을 고치지 않고, 그 질문에 시작 프롬프트를 넣지도 않습니다(시작 프롬프트는 세션이 시작된 뒤에 들어갑니다). 30초 안에 세션이 시작되지 않으면 그 탭에 ⚠ "폴더 신뢰 확인 필요"가 뜹니다.
- (handover를 쓸 때) **holla** CLI — handover가 슬랙을 확인할 때 씁니다. 기본 점검 항목이며 dispatcher가 설치를 안내합니다(아래 `envRequire`).

시작할 때 앱이 위 항목을 점검합니다. 실패한 항목만 상태 표시줄에 `[환경: Git 없음, holla 없음]`처럼 작게 보이고(필수 항목·Claude 버전은 강조), dispatcher가 시작 리뷰에서 하나씩 안내합니다. **설치·변경 명령은 사용자가 승인한 뒤에만** dispatcher가 실행하며, 앱은 아무것도 설치하지 않습니다.

### 역할과 메시지 경로

| 역할 | 수명 | 세션 이름 | 하는 일 |
|---|---|---|---|
| dispatcher | 상시 | `dispatcher` | 사용자 창구. handover가 전달한 메시지와 사용자 지시를 담당 Manager에게 넘김. 환경 점검 안내, 새 상시업무 탭 생성 |
| handover | 상시 | 탭 이름 | `/loop`로 슬랙·메일 등을 주기 확인해 새 메시지를 dispatcher에게 전달. 업무 인덱스(`HANDOVER.md`, 포인터만) 관리 |
| manager | 상시(업무별) | 탭 이름 | 업무 처리, 인박스 보고, 이슈 폴더, 팀 채용·해산 |
| lead | 임시 | `<탭>-<팀id>-lead` | brief대로 팀원에게 일을 나누고 결과물을 검증·지적, Manager에게 보고 |
| member | 임시 | `<탭>-<팀id>-m<N>` | 리드의 지시 수행, 리드에게만 보고 |

경로: handover → dispatcher → 담당 Manager → 인박스 항목 → 사용자, 사용자 지시는 dispatcher를 거쳐 Manager로, 팀은 Manager ↔ 리드 ↔ 팀원. 앱이 보내는 메시지(인박스 지시, 야간 정리 신호)는 보낸 이 `agentboard`, 본문 `[AgentBoard] …`로 도착하며, 역할 지시는 **보낸 이가 정확히 `agentboard`인 메시지만** 사용자 지시로 다룹니다. 다른 세션이 보낸 메시지는 `[AgentBoard]`로 시작해도 동료의 메시지이고, `agentboard`는 탭·세션 이름으로 쓸 수 없습니다. 앱이 보내는 본문의 `<` `>`는 `‹` `›`로 바뀝니다(위조 방지).

**외부 내용은 데이터입니다.** 역할 지시는 슬랙·메일·웹·파일, 그리고 다른 세션이 옮겨 온 그런 내용 안의 지시를 따르지 말고 보고만 하라고 합니다. handover가 가져온 메시지는 dispatcher가 "외부 메시지(참고용)"로 Manager에게 넘기고, Manager는 앱 메시지와 dispatcher가 옮긴 인박스 지시("사용자 지시(인박스 항목 …)")만 사용자 권한으로 봅니다. Manager·리드·팀원은 외부 발송·삭제·배포처럼 되돌리기 어려운 일을 사용자 승인(인박스 항목 또는 `need_user`) 뒤에만 합니다. 이것도 프롬프트 수준의 방어이므로, 외부 메시지를 다루는 업무에는 위험한 권한을 주지 마세요.

**권한 분리는 권고 수준입니다.** 메시지 경로는 역할 지시(프롬프트)로만 지켜지며 앱이 막지 않습니다. 앱이 강제하는 것은 MCP 도구의 역할별 허용(아래 표)과 항목 담당 확인뿐입니다. 역할 세션은 모두 같은 Windows 사용자로 실행되므로 다른 창의 run 파일(`%APPDATA%/agentboard/run/<창id>.{mcp,settings}.json`, 토큰 포함)도 읽을 수 있습니다 — 혼자 쓰는 PC를 전제로 합니다.

### 설정 예 (config.json)

```json
{
  "dispatcher": { "agent": "claude", "template": "dispatcher-inbox" },
  "defaultGroup": "work",
  "groups": {
    "split": "horizontal", "sizes": [35, 65],
    "children": [
      { "group": { "name": "control", "tabs": ["dispatcher", "handover"] } },
      { "group": { "name": "work",    "tabs": ["billing"] } }
    ]
  },
  "tabs": [
    { "name": "handover", "role": "handover", "dir": "",        "template": "handover" },
    { "name": "billing",  "role": "manager",  "dir": "billing", "template": "manager" }
  ],
  "handoverIndex": "HANDOVER.md",
  "nightlyRefresh": "03:00",
  "nightlyTimeoutMin": 20,
  "nightlyQuietMin": 10,
  "roles": {
    "manager": { "systemPromptFile": "roles/manager.md" },
    "lead":    { "args": ["--model", "opus"] },
    "member":  { "maxPerTeam": 4 }
  },
  "envRequire": [
    { "id": "holla", "check": "holla slack version",
      "fix": "source의 GitHub 저장소로 holla를 설치한 뒤 슬랙 로그인",
      "why": "handover의 슬랙 확인에 필요", "required": true, "source": "" }
  ],
  "envSkip": ["git-bash"]
}
```

- **dispatcher**는 1단계의 `dispatcher` 블록이 정합니다. `tabs[].role`에는 `"manager"`·`"handover"`만 쓸 수 있습니다.
- **handover** 탭은 하나만, `dir: ""`(작업공간 = `workspaceRoot`)이어야 합니다. **manager** 탭은 전용 폴더(`dir`)가 필요합니다.
- `handoverIndex`: 업무 인덱스 문서, `workspaceRoot` 기준(기본 `HANDOVER.md`). 업무마다 한 줄 — 업무 이름, 계획 파일, 진행 파일, 갱신 시각 — 포인터만 담습니다.
- `nightlyRefresh`(`"HH:MM"`, 기본 `"03:00"`, `null`이면 끔), `nightlyTimeoutMin`(기본 20), `nightlyQuietMin`(기본 10): 아래 "야간 정리".
- `memo`: `{"vi": true}`이면 메모를 vi 키로 편집합니다(기본 꺼짐, 아래 "메모").
- `roles.<역할>`: `systemPromptFile`(`.agentboard/` 기준 상대경로), `agent`(팀 창 프로필, 기본 `defaultAgent`), `args`(그 역할 세션에 붙는 인자), `maxPerTeam`(member만, 팀당 팀원 상한, 기본 4). `systemPromptFile`을 생략하면 `roles/<역할>.md`를 쓰고, 그 파일도 없으면 내장 기본 지시를 씁니다. 첫 실행 때 복사된 `roles/*.md`를 직접 다듬으면 다음에 뜨는 세션부터 적용됩니다. **업그레이드해도 복사된 파일은 바뀌지 않습니다** — 새 버전의 기본 지시를 쓰려면 앱을 끈 상태에서 `.agentboard/roles/`의 해당 `.md`를 지우세요(다음 시작 때 새 기본값이 복사됩니다. 직접 고친 내용이 있으면 먼저 옮겨 두세요). 템플릿(`templates/*.json`)도 같습니다.
  - npm으로 설치한 `claude.cmd`에서는 cmd.exe 제약 때문에 역할 지시가 한 줄로 합쳐지고(줄바꿈 → ` ⏎ `) `" % ^ & | < >`가 비슷한 모양의 다른 문자로 바뀝니다. 지시 파일은 짧게(수천 자 이내) 유지하세요.
- `envRequire`: 사용자 정의 점검 항목. `check` 명령(cmd.exe로 실행, 10초 제한)이 종료 코드 0이면 정상. `required: true`는 건너뛸 수 없습니다. 키가 없으면 위의 holla 항목이 기본이고, `[]`이면 없음. `source`(설치할 GitHub 저장소 주소)가 비어 있으면 dispatcher가 사용자에게 주소를 물어 기록한 뒤, 승인을 받아 그 저장소 README대로 설치합니다. 저장소가 비공개일 수 있어 앱은 주소를 기본값으로 넣지 않습니다.
- `envSkip`: 사용자가 "필요 없다"고 한 점검 항목 id(`claude-version`, `git-bash`, `folder-trust:<탭>`, 사용자 정의 id). dispatcher가 `env_skip`으로 기록하며, 그 기능만 빠지고 다시 묻지 않습니다.
- 앱이 config.json을 고치는 경우: dispatcher의 `create_manager_tab`·`env_skip`·`env_set_source`, 새 탭 마법사의 Manager 생성, Manager·handover 탭의 이름 바꾸기·닫기. 다른 키와 순서는 유지하지만 파일 서식은 다시 써집니다.

### 기본 템플릿

| 템플릿 | 배치 | 시작 프롬프트 |
|---|---|---|
| `dispatcher-inbox` | dispatcher \| (위) 인박스 애플릿 / (아래) 메모 | 환경 점검·인덱스·인박스 리뷰 |
| `handover` | 창 하나 | `/loop 10m {확인 작업}` — **직접 고치세요**(예: `/loop 10m holla로 새 슬랙 멘션과 메일을 확인해 새 메시지만 dispatcher에게 전달`) |
| `manager` | Manager \| 셸(PowerShell), 이 분할이 팀 영역(`"slot": "team"`) | 계획·진행 내역 리뷰, 진행 중인 팀 재채용 |
| `dispatcher`·`solo`·`team-3` | 1단계 그대로 | — |

- 팀 리드는 팀 영역 오른쪽에 팀마다 한 열로, 팀원은 그 리드 아래로 붙습니다. 템플릿에 `slot: "team"` 분할이 없으면 첫 채용 때 Manager 창 오른쪽에 팀 영역이 생기고, 마지막 팀이 해산되면 사라집니다.
- 시작 프롬프트는 앱을 시작할 때, 또는 **설정 다시 읽기** 때 템플릿에서 다시 읽습니다. 떠 있는 창은 재시작하지 않으므로 바뀐 프롬프트는 그 창의 다음 재시작·야간 정리 때부터 들어갑니다.
- **설정 다시 읽기**(팔레트 "설정 다시 읽기", dispatcher의 MCP `config_reload`): 앱을 재시작하지 않고 `config.json`·템플릿·에이전트 프로필·키 설정을 다시 읽습니다. 다른 창은 재시작하지 않습니다.
  - 검증 오류가 하나라도 있으면 아무것도 바꾸지 않고 오류를 보여 줍니다.
  - `tabs[]`에는 있는데 화면에 없는 역할 탭(handover 포함)은 `defaultGroup` 끝에 새 세션으로 띄웁니다. 포커스는 옮기지 않습니다.
  - config에서 빠진 역할 탭은 닫지 않고 알려 주기만 합니다.
  - `settings.json`의 `workspaceRoot`가 바뀌었거나, 띄울 수 없는 역할 탭(같은 이름의 다른 탭, 없는 템플릿, handover 둘)이 있으면 거부합니다.
- config.json이 없으면(새로 설치, 또는 `.agentboard`만 있고 config.json이 없을 때) 시작용 config.json이 `dispatcher-inbox`를 쓰도록 만들어집니다. 있는 config.json은 덮어쓰지 않습니다. **이미 쓰던 설치**에서 인박스를 붙이려면 `"dispatcher": { "template": "dispatcher-inbox" }`로 바꾼 뒤, 앱을 끈 상태에서 `%APPDATA%/agentboard/workspace.json`을 지우세요(저장된 배치가 dispatcher 탭을 예전 모양으로 복원하기 때문입니다. 배치는 config 기준으로 초기화됩니다). 인박스 항목의 담당은 탭 이름으로 기록되므로 이렇게 초기화해도 그대로 유지됩니다.

### 앱 내장 MCP 도구

역할 세션은 `--mcp-config`로 `127.0.0.1`의 앱 MCP 서버에 창별 토큰으로 연결합니다(도구 이름 `mcp__agentboard__<도구>`). 허용 역할은 앱이 검사합니다.

| 도구 | 허용 역할 | 동작 |
|---|---|---|
| `inbox_post(title, check[], risk[], source, refs?)` | manager | 인박스 항목 생성(담당 = 호출한 탭) |
| `inbox_note(item, text)` · `inbox_propose_close(item, final_opinion)` | manager(담당) | 타임라인 메모 · 최종 의견과 함께 종료 제안 |
| `inbox_list(state?)` | 모두 | 항목 조회(Manager는 자기 담당, dispatcher는 전체) |
| `issue_create(title, item?)` | manager | `<작업 폴더>/issues/<YYYYMMDD-제목>/{input,output}` 생성·연결 |
| `hire_lead(team, brief_path, extra_prompt?)` | manager | 팀 리드 창 추가(같은 팀이면 기존 리드 반환) |
| `hire_member(extra_prompt?)` | lead | 팀원 창 추가(팀당 상한) |
| `dismiss(agent)` · `dismiss_team(team)` | manager, lead | 팀원 해고 · 팀 해산(리드는 자기 팀원만) |
| `list_team(team?)` | manager, lead | 팀 구성·상태 |
| `create_manager_tab(name, dir, template?)` | dispatcher | 새 상시업무 탭 + config.json `tabs` 추가(폴더가 미리 있어야 함, 포커스 안 바뀜) |
| `handoff_done()` | dispatcher, handover, manager | 야간 정리 완료 신호 |
| `need_user(reason)` | 모두 | 그 창의 탭에 ⚠ + 사유 |
| `env_status()` | 모두 | 환경 점검 결과 |
| `env_skip(item)` · `env_set_source(item, url)` | dispatcher | 점검 건너뛰기 기록 · 설치 저장소 주소 기록 |
| `config_reload()` | dispatcher | 설정 다시 읽기(재시작 없음, 아래 "기본 템플릿" 절) — 추가된 탭·닫지 않은 탭·갱신한 시작 프롬프트 수 |

팀 문서: Manager는 리드를 채용하기 전에 `<작업 폴더>/teams/<팀id>/brief.md`(첫 줄 `상태: 진행 중`)와 `roster.md`(팀원 구성)를 씁니다. 끝난 팀은 brief에 `상태: 완료`를 적어 재채용 대상에서 뺍니다.

### 인박스 애플릿

dispatcher 탭 오른쪽 창(`{"applet":{"name":"inbox","kind":"inbox"}}`)입니다. 터미널과 애플릿 사이는 `Alt+h`/`Alt+l`로 오갑니다. 항목 상태: 새 항목 → 진행 중 → 종료 제안됨(Manager의 최종 의견) → 완료(**사용자만**). 종료 제안됨에서 다시 지시하면 진행 중으로 돌아갑니다.

| 키 | 동작 |
|---|---|
| `j`/`k` (`↓`/`↑`) | 목록에서 항목 이동, 상세에서 스크롤 |
| `Enter` / `Esc` | 상세 보기(읽음 처리) / 목록으로 |
| `i` | 지시: 여러 줄 입력(`Ctrl+Enter` 보내기) → dispatcher를 거쳐 담당 Manager에게 |
| `s` | 자료 저장: 파일 경로(여러 줄) → 이슈 폴더 `input/` 또는 `output/`에 복사. 이슈가 없으면 먼저 등록할지 묻습니다 |
| `u` | 이슈 등록 요청(담당 Manager가 `issue_create`) |
| `c` | 종료 확인(종료 제안됨 항목만) |
| `h` | 완료 항목 보기/숨기기 |
| `Tab` | 상세의 이슈 파일 목록으로 → `j`/`k`, md 파일에서 `Enter`로 문서 탭 열기 |

지시는 dispatcher 세션의 메시지 수신함(named pipe)으로 들어가므로 dispatcher 입력란에 쓰던 글을 건드리지 않습니다. dispatcher가 꺼져 있으면 보내지 않고 오류만 보여 줍니다.

### 메모

dispatcher 오른쪽 열의 인박스 아래 창(`{"applet":{"name":"memo","kind":"memo"}}`, 기본 비율 인박스 65 : 메모 35, 경계 드래그나 `Alt+Shift+J/K`로 조절)입니다. 사용자가 고른 오늘 할 일의 짧은 제목을 적어 두고 리뷰하는 곳이며, 에이전트가 고르는 업무 목록이 아닙니다.

- 파일은 `<workspaceRoot>/MEMO.md` 하나입니다. 없으면 앱이 시작할 때 작성 기준 헤더를 넣어 만들고, 있으면 건드리지 않습니다.
- **처음 나오는 `## 메모` 줄 아래가 본문**이고, 메모 창에는 본문만 보입니다. 그 위(작성 기준)는 보이지 않습니다. `## 메모` 줄이 없으면 파일 전체를 보여 주고 경고를 띄웁니다.
- 키: 읽기 `j`/`k`·`g`/`G` 스크롤, `e`(또는 더블클릭) 편집, `r` 다시 읽기. 편집 `Ctrl+Enter` 저장, `Esc` 취소(바뀐 내용이 있으면 `Y`/`N`, `Enter`는 무시). 편집 중에도 `Alt+…` 앱 단축키는 동작하고 쓰던 글은 남습니다.
- **vi 편집**(`config.json` `"memo": {"vi": true}`, 기본 꺼짐, 「설정 다시 읽기」 뒤 다음 편집부터): 편집을 열면 보통 모드(`-- NORMAL --`)입니다. 모드 `Esc` `i` `a` `A` `o` `O`, 이동 `h` `j` `k` `l` `w` `b` `0` `$` `gg` `G` `Ctrl+d`/`Ctrl+u`, 편집 `x` `dd` `yy` `p` `P` `u` `Ctrl+r`, 검색 `/` `n` `N`, 명령 `:w`(저장 후 계속) `:wq`·`ZZ`·`Ctrl+Enter`(저장 후 닫기) `:q!`(버리기, 바뀐 내용이 있으면 `Y`/`N`). 키는 자판 위치로 읽어 한글 입력 상태에서도 같고, 보통 모드의 `Esc`는 편집을 닫지 않습니다. 숫자 반복(`3dd`)·비주얼 모드·레지스터는 없습니다.
- dispatcher는 사용자가 요구할 때만 파일을 직접 고칩니다(역할 지시). 마지막에 쓴 쪽이 이기며, 앱은 내용이 바뀔 때마다 직전 내용을 `.agentboard/memo.bak`에 한 벌 남깁니다. 편집 중에 파일이 바뀌면 "외부에서 바뀜 — 저장하면 덮어씀" 알림만 띄웁니다.
- **이미 쓰던 설치**: 복사된 `.agentboard/templates/dispatcher-inbox.json`·`roles/*.md`와 저장된 배치는 바뀌지 않습니다. 메모를 붙이려면 앱을 끈 상태에서 `.agentboard/templates/dispatcher-inbox.json`(필요하면 `roles/dispatcher.md`도)과 `%APPDATA%/agentboard/workspace.json`을 지우고 다시 시작하세요(직접 고친 내용은 먼저 옮겨 두세요).

### 문서 탭

마크다운을 읽기 전용으로 보여 주는 탭입니다(marked·Mermaid·KaTeX·highlight.js 내장, `workspaceRoot` 밖 파일은 열지 않음, 원격 이미지 없음, 파일이 바뀌면 1–2초 안에 다시 그림). 여는 길은 커맨드 팔레트 **"문서 열기"**(md 퍼지 검색)와 인박스 상세의 이슈 파일 목록 두 가지이며, 에이전트가 문서 탭을 열 수는 없습니다. 키: `j`/`k`, `Space`/`Shift+Space`, `g`/`G`, `/` 검색(`n`/`N`), `o` 목차, `[`/`]` 링크 뒤로/앞으로, `r` 새로고침.

### 주의 표시

권한 승인 대기(Notification hook), `need_user`(에이전트가 사용자 결정이 필요할 때 질문을 쓰고 호출), 폴더 신뢰 대기는 그 탭에 ⚠로 보입니다. `Alt+a`로 이동하고 포커스하면 꺼집니다. 그룹 최대화로 탭 바가 가려지면 상태 표시줄에 `⚠ 탭(사유…)` 목록이 뜹니다. 자세한 내용은 아래 "역할 에이전트의 주의(⚠) 표시"를, 역할 없는 창은 "Claude Code 터미널 벨 설정"을 참고하세요.

### 야간 정리

`nightlyRefresh` 시각이 되면 하루 한 번:

1. 앱이 dispatcher·각 Manager에게 `[AgentBoard] 야간 정리` 메시지를 보냅니다. 각자 작업 계획·진행 내역 파일을 저장하고(팀 작업은 방해하지 않음) handover에게 파일 위치를 보고한 뒤 `handoff_done()`.
2. 모두 끝나면(또는 제한 시간의 절반이 지나면) handover에게 인덱스 갱신 요청이 갑니다. handover는 포인터만 고치고 `handoff_done()`. 그 요청 전에 부른 `handoff_done`은 "아직"으로 답하고 기록하지 않습니다.
3. 모두 완료하거나 `nightlyTimeoutMin`이 지나면 dispatcher·handover·Manager 창을 **새 세션**으로 다시 시작하고 시작 프롬프트를 다시 넣습니다(대화 이어가기 없음). 시간 안에 끝내지 못한 세션은 인박스에 "야간 정리 미완료: <이름>" 항목이 생깁니다. **팀 창(리드·팀원)은 재시작하지 않습니다.**

- 시각에 dispatcher에 `nightlyQuietMin`분 안에 입력이 있었으면 5분 뒤 다시 확인합니다. 재시작 직전 어떤 대상 창에 입력 중이면 그 창만 미루고, 60분이 넘게 계속되면 그 창은 재시작하지 않고 인박스에 "야간 정리 보류: <이름>" 항목을 남깁니다(입력한 글을 지우지 않음).
- 시각이 지난 뒤 앱을 처음 켜서 상시 세션이 모두 방금 새로 떴다면 정리 없이 그날을 기록만 합니다. PC가 그 시각에 잠들어 있다가 같은 날 깨어나면 깬 뒤에 한 번 돕니다. 실행 날짜는 `%APPDATA%/agentboard/nightly.json`에 기록됩니다.

### 시작·복원·자동 실행

- 시작할 때는 상시 에이전트(dispatcher·handover·Manager)만 **새 세션 + 시작 프롬프트**로 띄웁니다. 팀 창과 팀 영역은 복원하지 않고, 각 Manager가 시작 리뷰에서 `teams/*/brief.md`가 `상태: 완료`가 아닌 팀을 **자동으로 다시 채용**하며, 리드는 `roster.md`대로 팀원을 다시 채용해 이어서 일합니다.
- **역할 탭의 기준은 config.json입니다.** 이름·dir이 같은 `tabs[]` 항목이 있는 탭만 역할 탭으로 복원되고, config에만 있는 역할 탭은 `defaultGroup`에 포커스를 뺏지 않고 추가됩니다(앱 시작 때, 그리고 "설정 다시 읽기" 때). Manager·handover 탭의 이름을 바꾸거나 닫으면 config.json 항목도 함께 바뀌거나 빠집니다. 역할 없는 탭은 1단계처럼 `workspace.json`에서 복원합니다.
- 역할 탭 안의 다른 창(예: `manager` 템플릿의 셸, 직접 띄운 Claude 창)은 역할 없는 창으로 시작 프롬프트 없이 다시 실행됩니다.
- **한 번에 하나만 실행**: 이미 실행 중일 때 다시 실행하면(자동 실행 뒤 바로 가기 클릭, 설치 후 재실행 등) 새 프로세스는 바로 끝나고 기존 창이 앞으로 옵니다. 역할 세션의 hook(`agentboard.exe --hook …`)은 이 제한과 무관하게 동작합니다.
- **인박스 담당**: 항목은 담당 탭의 **이름**으로 기록됩니다. Manager 탭 이름을 바꾸면 그 항목들도 함께 옮겨 가고, 탭을 닫으면 항목은 "담당 없음(이름)"으로 표시되며 지시·이슈 요청을 보낼 수 없습니다(같은 이름의 Manager 탭을 다시 만들면 다시 연결됩니다).
- **로그인 시 자동 실행**: 커맨드 팔레트의 "로그인 시 자동 실행 켜기/끄기"(HKCU `Run` 키에 현재 exe 경로를 등록). 개발 빌드에서 켜면 개발용 exe가 등록되니 주의하세요.

### 세션 등록부(registry.json)에서 옮겨 오기

- `node scripts/import-registry.mjs <registry.json> --only <세션,…> [--root <workspaceRoot>]` — 기존 세션 등록부(session-board `registry.json`)에서 고른 `regular[]` 세션을 Manager 탭 설정으로 바꿔 **화면에만 출력합니다(dry-run, 파일을 하나도 쓰지 않음)**. `--root`를 생략하면 등록부 디스패처 항목의 dir을 씁니다.
- 출력: `config.json`(dispatcher `dispatcher-inbox`, control | work 그룹, `nightlyRefresh: null`, holla 전체 경로 점검, 메모 vi 편집 `"memo": {"vi": true}`), `startupPrompt`가 있는 세션의 탭별 템플릿(`<세션>-start`), 공존 기간 안내를 붙인 `roles/dispatcher.md`·`roles/manager.md`.
- 이미 있는 파일은 `*.imported.*` 이름으로 표시할 뿐 **덮어쓰지 않습니다** — 내용을 확인한 뒤 직접 반영하세요. 디스패처(`workspace-07`)·handover·workspaceRoot 밖 폴더는 탭으로 만들지 않고 경고합니다.

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

### 역할 에이전트의 주의(⚠) 표시

- **권한 승인 대기**: 앱이 띄운 역할 세션(dispatcher·manager·팀 등)은 Claude Code Notification hook의 `permission_prompt`로 ⚠가 켜집니다(벨 설정 불필요). 지금 보고 있는(포커스된) 창이면 켜지 않으며, `idle_prompt`(쉬는 중 알림)는 쓰지 않습니다. auto 권한 모드에서는 권한 요청 자체가 거의 없습니다.
- **`need_user`**: 에이전트가 사용자 결정이 필요하면 질문을 화면에 쓰고 MCP 도구 `need_user(reason)`을 부릅니다. ⚠ 툴팁에 사유가 보입니다(최대 200자).
- **폴더 신뢰 확인 필요**: 역할 창이 시작 후 30초 안에 SessionStart를 보내지 않으면(대개 Claude의 "이 폴더를 신뢰하시겠습니까?" 질문에서 멈춤) 이 사유로 ⚠가 켜집니다. 그 창으로 가서 직접 답하세요 — 앱은 Claude의 신뢰 설정을 바꾸지 않습니다.
- ⚠는 그 창에 포커스하면(클릭·`Alt+a`·탭 전환) 사유와 함께 꺼집니다. 그룹 최대화 등으로 ⚠ 탭의 탭 바가 화면에 없으면 상태 표시줄에 `⚠ billing(사유…), api`처럼 보이고, 항목을 클릭하거나 `Alt+a`로 이동합니다. ⚠는 앱을 다시 시작하면 지워집니다.

## 로드맵

1단계는 사람이 직접 조작하는 터미널 보드, 2단계(V0.2.0)는 위의 상시업무 운영(역할 에이전트·인박스·야간 정리)입니다. 다음 순서: dispatcher 옆 메모 → 작업 상태 대시보드 → 상시업무 제거 도구. Claude 외 에이전트(Codex·Gemini 등) 프로필과 창을 다른 탭에 붙이기(join)는 보류합니다.
