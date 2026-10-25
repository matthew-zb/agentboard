# AgentBoard 수동 확인 전체 색인 (한 번에 돌리기)

아래 절에 흩어진 **사용자 확인 필요** 항목의 위치와 순서다. 결과는 각 절의 체크박스와 "결과:" 줄에 기록한다. 마지막의 "QA 결과"(스펙 §11 1~7)는 위 항목을 다 거친 뒤 한 번에 확인해도 된다.

권장 순서: 준비 한 번(`npx tauri dev`, 또는 릴리스 설치본) → 아래 순서대로.

| # | 절 | 내용 | 비고 |
|---|---|---|---|
| 1 | 기본 동작 확인 (Step 2) | 입력·`dir`·한글 폭·리사이즈 | Task 3 |
| 2 | S3 | Claude Code TUI(색·상자 문자·리사이즈·IME·이미지 붙여넣기) | 스파이크 |
| 3 | S2 | WebView2 가속키(`Ctrl+R/P`, `F5`)·컨텍스트 메뉴·DevTools | 스파이크 |
| 4 | S1 | Alt 단독 키 포커스·경고음 | 스파이크 |
| 5 | Task 9 | `ws_snapshot` 확인 | |
| 6 | Task 11 Step 2 / Step 3 | 화면 렌더링, S4 성능(Claude 9개 동시) | 스파이크 S4 |
| 7 | Task 12 Step 5 / Step 6 | 키 라우터(마우스 없이), S5 `Alt+Shift+글자` 입력 언어 전환 | 스파이크 S5 |
| 8 | Task 13 Step 5 | 오버레이: 번호 선택·팔레트·새 탭 마법사·확인 대화·오류 보기 | |
| 9 | Task 14 Step 5 | 복사 모드·입력 동기화·붙여넣기 | |
| 10 | Task 15 Step 1 | 주의(⚠) 표시, `Alt+a` (Claude 터미널 벨 설정 필요) | |
| 11 | Task 16 S6 / Step 5 | 드래그앤드롭 4종, 터미널 위 드래그 이벤트 | 스파이크 S6 |
| 12 | Task 17 | 앱 종료 확인 대화, 종료 시 프로세스 정리 | 아래 |
| 13 | QA 결과 (스펙 §11 1~7) | 최종 체크리스트 | 아래 |
| 14 | 2단계 스파이크 P1 / P2 | 입력란에 쓰던 글이 있을 때 파이프 메시지 도착, (선택) 대화형 TUI끼리 SendMessage | 2단계 |

---

# 스파이크 S1–S3 (Task 3)

상태: 빌드·실행 확인만 자동으로 끝냈다. 아래 수동 항목은 모두 **사용자 확인 필요**이며, 결과 칸은 비어 있다(관찰하지 않은 결과는 적지 않는다).

자동 확인 결과: `cargo build`·`cargo test`(15 통과)·`npm run test:js`(통과) 정상. `npx tauri dev`로 `agentboard.exe`가 기동되고 콘솔에 `webview settings failed`/`with_webview failed`/패닉 로그 없음(= `SetAreBrowserAcceleratorKeysEnabled(false)` 호출이 오류 없이 반환됨).

준비: `npx tauri dev` 실행 → 창이 뜨면 `p-test` 창(PowerShell)이 보인다.

## 기본 동작 확인 (Step 2)
- [ ] 사용자 확인 필요 — PowerShell 프롬프트에 입력이 된다.
- [ ] 사용자 확인 필요 — `dir` 출력이 어긋나지 않는다.
- [ ] 사용자 확인 필요 — `echo 한글테스트`가 올바른 폭(글자 겹침/빈칸 없음)으로 표시된다.
- [ ] 사용자 확인 필요 — 창 크기를 바꾸면 프롬프트 줄바꿈 폭이 따라간다(`$Host.UI.RawUI.WindowSize`로 열·행 확인).

## S3 — ConPTY + xterm.js에서 Claude Code TUI
같은 창에서 `claude` 실행 후 확인.
- [ ] 사용자 확인 필요 — 색, 커서 위치, 상자 문자(┌─┐│└┘)가 깨지지 않는다.
- [ ] 사용자 확인 필요 — 창 리사이즈 시 TUI가 새 크기로 다시 그려진다.
- [ ] 사용자 확인 필요 — 한글 입력(IME 조합 중 글자가 커서 위치에 보이고 확정 시 입력됨).
- [ ] 사용자 확인 필요 — `Ctrl+V`로 이미지 붙여넣기 시 이미지 칩이 나타난다(클립보드에 이미지 복사 후).

기대 결과: 위 4개 모두 통과.
실패 시 대안(스펙 §10 S3): `pane.js`의 unicode11 addon 설정 조정, ConPTY 플래그 조정.
결과: (미기록)

## S2 — WebView2 브라우저 단축키 비활성화
`src-tauri/src/webview.rs`가 적용된 상태에서 `claude` 창에서 확인.
- [ ] 사용자 확인 필요 — `Ctrl+R`: 페이지 새로고침이 아니라 Claude 기록 검색이 열린다.
- [ ] 사용자 확인 필요 — `Ctrl+P`: 인쇄 대화가 뜨지 않는다.
- [ ] 사용자 확인 필요 — `F5`: 새로고침되지 않는다(화면·세션 유지).
- [ ] 사용자 확인 필요 — 릴리스 빌드: 우클릭 시 기본 컨텍스트 메뉴가 뜨지 않는다. 디버그 빌드(`tauri dev`)에서는 의도적으로 메뉴를 남겨 우클릭 > "Inspect"로 DevTools를 연다.
- [ ] 사용자 확인 필요 — 디버그 빌드: F12는 가속키라 동작하지 않을 수 있다. `AGENTBOARD_DEVTOOLS=1`로 실행하면(PowerShell: `$env:AGENTBOARD_DEVTOOLS='1'; npx tauri dev`) DevTools 창이 시작 시 열리는지 확인. Ctrl+R/F5는 디버그에서도 막혀 있어야 한다.

DevTools 정책(R9): 가속키 비활성화는 디버그·릴리스 공통. 컨텍스트 메뉴 비활성화는 릴리스만. 디버그 전용 `AGENTBOARD_DEVTOOLS=1`이 DevTools를 연다.

기대 결과: 위 항목 모두 통과.
실패 시 대안(스펙 §10 S2): document keydown capture에서 해당 키를 `preventDefault` 한 뒤 PTY로 직접 전송(`api.write`).
결과: (미기록)

## S1 — Alt 키 처리
Task 11에서 `src/app.js`의 임시 프로브(`Alt+H` `preventDefault`)를 제거했다. `Alt+H` 항목은 Task 12 키 라우터가 들어온 뒤 확인한다.
- [ ] 사용자 확인 필요 — `Alt`만 눌렀다 뗐을 때 xterm 포커스가 유지된다(이후 바로 타이핑됨).
- [ ] 사용자 확인 필요 — `Alt`만 눌렀다 뗐을 때 경고음이 나지 않는다.
- [ ] 사용자 확인 필요 — `Alt+H`(프로브 적용 상태)에서 경고음이 나지 않는다.
- [ ] 사용자 확인 필요 — 그 외 `Alt+글자`(예: `Alt+J`)에서 경고음 여부 기록.

기대 결과: 포커스 유지, 경고음 없음.
실패 시 대안(스펙 §10 S1, 순서대로): (1) keyup의 Alt에 `preventDefault` 적용, (2) 그래도 안 되면 Rust 서브클래싱으로 `WM_SYSKEYDOWN/WM_SYSCHAR` 처리.
적용한 대안: Task 12 키 라우터가 예방 차원에서 (1)을 먼저 적용했다(`src/keys.js`: document capture 단계에서 `Alt` 단독 keyup과 앱이 처리한 단축키의 keyup에 `preventDefault`). 효과는 아직 관찰하지 않았다 — 위 항목과 아래 "Task 12" 절에서 확인한다. 그래도 경고음·메뉴 활성화가 있으면 (2)로 간다.

## Task 9 — Launcher와 Tauri 명령 계층
자동 확인: `cargo test`(launcher 단위 테스트), `npx tauri dev` 실행 시 콘솔의 시작 요약 줄
`[agentboard] startup: tabs=… panes=… running=… failed=… config_errors=…`과 패닉 없음.
- [ ] 사용자 확인 필요 — `$env:AGENTBOARD_DEVTOOLS='1'; npx tauri dev`로 실행하고 DevTools 콘솔에서
  `await window.__TAURI__.core.invoke('ws_snapshot')`를 호출한다. `workspace.tabs`에 dispatcher 탭과
  config.json의 탭이 보이고, `workspace.panes[*].status.state`가 `running`이면 통과.
- [ ] 사용자 확인 필요 — 화면에 포커스된 탭의 창 하나가 나타나 입력이 된다(Task 11 전까지의 임시 화면).

결과: (미기록)

## Task 11 — 화면 렌더링 + 스파이크 S4 (사용자 확인 필요)

자동 확인: `cargo test`·`npm run test:js` 통과. `npx tauri dev` 기동 후 개발 로그에 패닉·오류 없음.
추가로 `__TAURI__`를 흉내 낸 모의 스냅샷(그룹 2개, 3분할 탭 + 숨은 탭)을 헤드리스 Chromium에 띄워 다음을 확인했다:
- 탭 전환 시 숨은 탭의 창은 `#parking`으로 옮겨진다.
- 돌아오면 같은 xterm DOM이 재사용된다.
- 창 클릭과 탭 클릭은 `SetFocus` 명령을 보낸다.
- 경계 드래그는 `SetSizes`를 보낸다(탭 경계는 `tab`, 그룹 경계는 `tab:null`).
- ⟳를 누르면 `RestartPane` → ◌ → ●로 바뀌고 다시 attach한다.
- 명령 오류는 상태 표시줄에 표시된다.
- `pty_resize`는 열·행이 바뀔 때만 호출된다.

이 확인은 실제 WebView2 창이나 실제 PTY에서 본 결과가 아니다. 아래 항목은 모두 사용자 확인이 필요하다.

### 준비 파일 (`C:/workspace/.agentboard/`)
`templates/grid-3x3.json` (S4용 임시 템플릿, Claude 9개):
```json
{
  "name": "grid-3x3",
  "layout": {
    "split": "vertical",
    "children": [
      { "split": "horizontal", "children": [
        { "pane": { "name": "c1", "agent": "claude", "focus": true } },
        { "pane": { "name": "c2", "agent": "claude" } },
        { "pane": { "name": "c3", "agent": "claude" } } ] },
      { "split": "horizontal", "children": [
        { "pane": { "name": "c4", "agent": "claude" } },
        { "pane": { "name": "c5", "agent": "claude" } },
        { "pane": { "name": "c6", "agent": "claude" } } ] },
      { "split": "horizontal", "children": [
        { "pane": { "name": "c7", "agent": "claude" } },
        { "pane": { "name": "c8", "agent": "claude" } },
        { "pane": { "name": "c9", "agent": "claude" } } ] }
    ]
  }
}
```
`templates/grid-3x3-ping.json`도 만든다. 위 파일을 복사해 `name`을 `grid-3x3-ping`으로 바꾸고, 각 창을 다음처럼 바꾼다:

```json
{ "pane": { "name": "c1", "agent": "cmd", "args": ["/k", "ping", "-t", "127.0.0.1"] } }
```

이 템플릿은 `ping` 부하를 줄 때 쓴다. 대신 Claude 창에서 `!ping -t 127.0.0.1`을 실행해도 된다.

`config.json` (Step 2와 S4에서 함께 사용; `dir`은 `C:/workspace` 아래에 실제로 있는 폴더여야 한다):
```json
{
  "tabs": [
    { "name": "team", "dir": "agentboard", "template": "team-3" },
    { "name": "s4", "dir": "agentboard", "template": "grid-3x3" },
    { "name": "s4-ping", "dir": "agentboard", "template": "grid-3x3-ping" }
  ]
}
```
주의: 모든 탭의 창은 시작 시 실행된다. Step 2만 확인할 때는 `s4`·`s4-ping` 항목을 빼고 실행한다.

### Step 2 — 화면 확인
- [ ] 사용자 확인 필요 — `team` 탭에서 3분할(lead 60% | coder/tests 상하)이 보인다. 각 창 헤더에 이름·프로필·상태 기호(● 실행)가 표시되고, 포커스 창은 파란 테두리로 강조된다.
- [ ] 사용자 확인 필요 — 다음 순서로 확인한다:
  1. 각 창에 글자를 출력한다.
  2. 탭 바에서 `dispatcher`를 클릭한다.
  3. 다시 `team`을 클릭한다.

  출력이 그대로 남아 있으면 통과다. 터미널이 재생성되면(화면이 지워지거나 스크롤백이 사라지면) 실패다.
- [ ] 사용자 확인 필요 — 창을 클릭하면 테두리와 상태 표시줄(`그룹 › 탭 › 창`)이 바뀌고, 바로 입력할 수 있다.
- [ ] 사용자 확인 필요 — 창 경계를 끌어 크기를 바꾼다. 놓은 뒤에도 크기가 유지되고, TUI가 새 크기로 다시 그려진다.
- [ ] 사용자 확인 필요 — `tests`(powershell)에서 `exit`를 입력한다:
  1. 헤더가 `○ 0`으로 바뀌고, 창 아래에 "Enter: 재시작" 줄이 보인다.
  2. Enter를 누르면 ◌ → ●로 바뀌고 새 프롬프트가 나온다.
- [ ] 사용자 확인 필요 — 헤더 ⟳는 창을 재시작하고, ✕는 창을 닫는다.
- [ ] 사용자 확인 필요 — 창 크기를 바꾸면 모든 보이는 창의 줄바꿈 폭이 따라간다.
- [ ] 사용자 확인 필요 — 콘솔 오류가 없는지 DevTools 콘솔에서 확인한다(`$env:AGENTBOARD_DEVTOOLS='1'; npx tauri dev`).

결과: (미기록)

### Step 3 — 스파이크 S4 (성능)
1. 위 `config.json`으로 실행하고 `s4` 탭을 연다. Claude 9개가 모두 ●가 될 때까지 기다린다.
2. `s4-ping` 탭을 연다. 또는 `s4` 탭의 9개 창에서 각각 `!ping -t 127.0.0.1`을 실행한다.
3. 출력이 계속 흐르는 동안 다음 두 가지를 관찰한다:
   - 한 창에서 타이핑해 반응 지연을 본다.
   - 작업 관리자에서 `agentboard.exe`와 WebView2(`msedgewebview2.exe`) 프로세스의 CPU 합계를 본다.
4. 핑 탭을 숨긴 상태(`team` 탭 표시)에서 CPU도 기록한다.

- 기준: 타이핑 반응이 체감상 지연 없음, 앱 CPU 30% 이하.
- 참고: 브라우저의 WebGL 컨텍스트 수 제한(보통 16개) 때문에, 창이 많으면 오래된 창이 컨텍스트를 잃고 DOM 렌더러로 바뀔 수 있다(`onContextLoss` → dispose). 이 경우 콘솔에 WebGL 경고가 보이는지도 기록한다.
- 미달이면 다음을 차례로 적용한다:
  1. 비가시 탭은 `term.write`를 보류하고 attach 스냅샷으로 따라잡는다.
  2. 배치 주기(16ms)를 늘린다.

| 항목 | 값 |
|---|---|
| 9개 Claude 대기 시 CPU | (미기록) |
| 9개 ping 출력 중 CPU (보이는 탭) | (미기록) |
| 9개 ping 출력 중 CPU (숨은 탭) | (미기록) |
| 타이핑 지연 체감 | (미기록) |
| WebGL 컨텍스트 손실 여부 | (미기록) |
| 적용한 완화책 | (미기록) |

## Task 12 — 키 라우터 · 방향 이동 · 포커스 + 스파이크 S5 (사용자 확인 필요)

자동 확인:
- `npm run test:js` 통과. `keys`/`geometry`/`focus`/`commands`의 순수 함수 테스트 포함.
- `npx tauri dev` 기동, 시작 요약 줄 정상, 패닉 없음.
- `__TAURI__`를 흉내 낸 모의 스냅샷(그룹 2개, 3분할 탭)을 헤드리스 Chromium에 띄우고 합성 키 이벤트로 다음을 확인했다.
  - `Alt+l`(IME 조합 중 `key='ㅣ'`), `Alt+j`, `Alt+l`(그룹 경계 넘어감) → `SetFocus`가 차례로 전송되고 DOM 포커스가 따라간다.
  - `passthroughKeys: ["Alt+h"]` 창에서는 `Alt+h`를 가로채지 않는다.
  - `Ctrl+Alt+j`(직전 위치)와 `Alt+1`이 동작하고, 탭·그룹 전환 뒤 테두리 하이라이트가 켜졌다 꺼진다.
  - `Alt+q`(Task 13 전까지 미연결)는 삼켜지고 아무 명령도 보내지 않는다.
  - `Ctrl+R`은 PTY로 전달된다(`pty_write`). `Alt+z`는 `SetMaximized`만 보내고 PTY에는 쓰지 않는다.
  - 명령 오류(`ClosePane` 실패)는 상태 표시줄에 표시된다.
  - `Alt+x`를 누르고 있어 생기는 반복 입력은 실행하지 않는다. `Alt` 단독 keyup은 `preventDefault` 된다.
  - `SetFocus` 응답이 늦는 동안 옛 포커스를 담은 스냅샷이 도착해도 DOM 포커스가 되돌아가지 않는다.

이 확인은 실제 WebView2 창이나 실제 키보드·IME로 본 결과가 아니다. 아래 항목은 모두 사용자 확인이 필요하다.

준비: Task 11 절의 `config.json`(`team` 탭, team-3 템플릿)을 쓴다. 그룹 경계 확인용으로 `config.json`에 그룹을 둘 이상 두거나, 실행 후 `Ctrl+Alt+\`로 그룹을 나눈다.

### Step 5 — 수동 확인 (마우스 사용 금지)
창:
- [ ] 사용자 확인 필요 — `Alt+h/j/k/l`로 창 포커스가 이동하고, 그룹 경계도 넘어간다. 포커스 테두리와 상태 표시줄 경로가 따라온다.
- [ ] 사용자 확인 필요 — 이동한 창에서 바로 타이핑된다(DOM 포커스).
- [ ] 사용자 확인 필요 — `Alt+\`는 좌우 분할, `Alt+-`는 상하 분할이다. 새 창이 시작된다.
- [ ] 사용자 확인 필요 — `Alt+x`는 창을 닫는다. `Alt+z`는 최대화를 토글한다. `Alt+r`은 재시작(◌ → ●)이다.
- [ ] 사용자 확인 필요 — `Alt+Shift+H/J/K/L`은 경계를 그 방향으로 5%씩 옮긴다(tmux식: 해당 방향 경계가 있으면 늘리고, 없으면 반대쪽 경계를 당겨 줄인다). 탭 안에 그 축의 경계가 없으면 그룹 경계가 움직인다. 누르고 있으면 반복된다.

탭·그룹:
- [ ] 사용자 확인 필요 — `Ctrl+Alt+h/l`은 그룹 안에서 이전·다음 탭으로 간다(양 끝에서 순환).
- [ ] 사용자 확인 필요 — `Ctrl+Alt+j`는 직전 위치로 왕복한다. `Ctrl+Alt+k`는 Dispatcher 탭으로 간다.
- [ ] 사용자 확인 필요 — `Alt+1..9`는 N번째 탭, `Ctrl+Alt+1..9`는 N번째 그룹으로 간다. 전환 뒤 포커스 창 테두리가 300ms 동안 한 번 강조된다.
- [ ] 사용자 확인 필요 — `Ctrl+Alt+\`(`Ctrl+Alt+-`)는 현재 탭을 오른쪽(아래) 새 그룹으로 옮긴다.
- [ ] 사용자 확인 필요 — `Ctrl+Alt+Shift+H/L`은 탭 순서를 바꾼다. 탭 바 끝에서는 옆 그룹으로 넘어간다(왼쪽 그룹이면 끝, 오른쪽 그룹이면 맨 앞).
- [ ] 사용자 확인 필요 — `Ctrl+Alt+Shift+J/K`는 아래·위 그룹으로 탭을 보낸다.
- [ ] 사용자 확인 필요 — `Ctrl+Alt+x`는 그룹을 닫는다(형제 그룹과 합침). `Ctrl+Alt+z`는 그룹 최대화를 토글한다.
- [ ] 사용자 확인 필요 — 아직 연결 전인 키(`Alt+q`, `Alt+a`, `Alt+c`, `Alt+i`, `Alt+n`, `Ctrl+Alt+m`, `Ctrl+Alt+g`, `Alt+;`, `Ctrl+Shift+V`)를 눌러도 터미널에 글자가 들어가지 않는다.

IME·전달:
- [ ] 사용자 확인 필요 — 한글 IME를 켠 상태(한글 조합 중 포함)에서 위 키를 반복해도 같은 결과가 나온다. 조합 중이던 글자가 남거나 엉뚱한 창에 들어가지 않는다.
- [ ] 사용자 확인 필요 — Claude 창에서 `Ctrl+R`, `Ctrl+P`, `Alt+P`, `Alt+V`, `Shift+Tab`이 Claude로 전달된다.
- [ ] 사용자 확인 필요 — S1: `Alt`만 눌렀다 떼도 포커스가 유지되고 경고음이 없다. `Alt+h`에서도 경고음이 없다.
- [ ] 사용자 확인 필요 — 앱 창을 `Alt+Tab`으로 떠났다 돌아오면 같은 창에서 바로 타이핑된다.
- [ ] 사용자 확인 필요 — 다른 창에서 벨(⚠)이 울려도 포커스가 옮겨지지 않는다.
- [ ] 사용자 확인 필요 — (선택) 프로필 `passthroughKeys`에 `"Alt+h"`를 넣은 창에서는 `Alt+h`가 프로그램으로 전달된다. 예: powershell 프로필, PSReadLine.

결과: (미기록)

### Step 6 — 스파이크 S5 (`Alt+Shift+글자`와 입력 언어 전환)
준비: Windows 설정 > 시간 및 언어 > 언어에서 한국어와 다른 입력 언어(예: English (US))를 둘 다 설치한다. 입력 언어 전환 키가 기본값(`Alt+Shift`)인지 확인한다.

1. 앱에서 한 창에 포커스를 두고, 작업 표시줄의 입력 언어 표시(가/A, ENG/한)를 확인한다.
2. `Alt+Shift+H`를 10회 반복한다. 누르는 순서는 Alt → Shift → H다.
3. 이어서 `Alt+Shift+L`을 10회, `Alt+Shift+J`와 `Alt+Shift+K`를 각 5회 누른다.
4. 매번 입력 언어 표시가 바뀌는지 본다. 경계가 움직이는지도 본다.
5. 순서를 바꿔 Shift → Alt → H로도 시도한다.
6. 키를 다 뗀 뒤 입력 언어가 처음과 같은지 확인한다.

- 통과 기준: 입력 언어가 한 번도 바뀌지 않고, 경계가 매번 움직인다.
- 실패 시 대안(스펙 §10 S5): 크기 조절을 모드 방식으로 바꾼다. `Alt+s`로 크기 조절 모드에 들어가 `h/j/k/l`을 반복하고 `Esc`로 끝낸다. 이 경우 스펙 §6.2 키 표와 §6.3 모드 목록을 갱신하고, `pane.resizeMode` 명령을 `commands.js` 레지스트리에 추가한다. 이 대안은 아직 구현하지 않았다.

| 항목 | 결과 |
|---|---|
| Alt → Shift → 글자: 입력 언어 바뀜? | (미기록) |
| Shift → Alt → 글자: 입력 언어 바뀜? | (미기록) |
| 경계가 매번 움직임? | (미기록) |
| 대안 적용 여부 | (미기록) |

## Task 13 — 오버레이: 번호 선택 · 커맨드 팔레트 · 새 탭 마법사 · 확인 대화 · 오류 보기 (사용자 확인 필요)

구현자는 앱 창을 직접 조작할 수 없으므로(R8) 아래 결과는 모두 **사용자 확인 필요**다. 구현 중에는 단위 테스트와 브라우저 모의 하네스(가짜 `__TAURI__`, 스크래치 폴더, 커밋 안 함)로만 확인했다.

동작 요약:
- 오버레이가 열려 있는 동안 키보드는 오버레이가 가진다. 앱 단축키는 동작하지 않고, 입력창에는 `Alt`·`Ctrl+Alt` 조합을 포함한 모든 글자가 들어간다. `Esc`로 닫는다.
- 탭·그룹이 바뀌면(키·마우스·팔레트) 열린 오버레이는 닫힌다. 한 번에 하나만 열린다.
- `Ctrl+Alt+m` 다음 `\`·`-`는 **현재 포커스된 그룹**을 오른쪽·아래로 분할해 현재 탭을 옮긴다(`Ctrl+Alt+\`·`Ctrl+Alt+-`와 같다). 그룹에 탭이 하나뿐이면 백엔드가 거부하고, 그 오류는 토스트로 표시된다.
- `ws_command` 오류는 모두 상태 표시줄 위에 3초 동안 토스트로 표시된다.

### Step 5 — 수동 확인 (마우스 사용 금지)
- [ ] 사용자 확인 필요 — `Alt+n` → 목록에 없는 이름(예: `t13-test`) 입력 → Enter → "'t13-test' 디렉토리를 만들까요?"에서 `Y` → 템플릿 목록(기본 템플릿이 맨 위, "기본값" 표시)에서 Enter → 현재 그룹에 새 탭이 생기고 포커스된다. `C:/workspace/t13-test` 폴더가 생겼는지 확인한다.
- [ ] 사용자 확인 필요 — `Alt+n` → 이미 탭이 있는 폴더 이름 → 템플릿 Enter → 오류 토스트가 뜨고 ① 단계로 돌아온다. `Esc`로 취소한다.
- [ ] 사용자 확인 필요 — `Alt+q` → 보이는 창마다 큰 번호 → 숫자로 그 창에 포커스. 다시 `Alt+q` 후 `h/j/k/l`로 방향 이동. `Alt+q` 후 2초 동안 아무것도 누르지 않으면 번호가 사라진다.
- [ ] 사용자 확인 필요 — `Ctrl+Alt+m` + 숫자 → 현재 탭이 그 그룹 끝으로 이동한다. `Ctrl+Alt+m` + `\` → 오른쪽에 새 그룹이 생기고 탭이 옮겨진다(현재 그룹에 탭이 2개 이상일 때).
- [ ] 사용자 확인 필요 — `Ctrl+Alt+g` + 숫자(또는 `hjkl`) → 현재 그룹이 고른 그룹에 합쳐진다.
- [ ] 사용자 확인 필요 — `Alt+;` → `이름` 입력 → "탭 이름 바꾸기" Enter → 새 이름 입력 후 Enter → 탭 바와 상태 표시줄 이름이 바뀐다. 입력 중 `Alt+h`, `Ctrl+Alt+l`을 눌러도 포커스·탭이 바뀌지 않는다.
- [ ] 사용자 확인 필요 — `Alt+;` → "현재 창을 새 탭으로 떼어내기" → 창이 새 탭으로 옮겨진다.
- [ ] 사용자 확인 필요 — `C:/workspace/.agentboard/templates/bad.json`에 잘못된 JSON(예: `{"layout": }`)을 넣고 재시작 → 상태 표시줄 `[설정 오류 1]` → `Alt+;` → "설정 오류 보기" → `bad.json:줄:열 — 원인` 형식의 줄이 보인다. `Esc`로 닫는다.
- [ ] 사용자 확인 필요 — 한글 IME를 켠 상태에서 팔레트에 한글을 입력하면 조합이 정상이고, `Alt+q` 번호 오버레이 중 `ㅗ`(h) 등을 눌러도 터미널에 글자가 들어가지 않는다.
- [ ] 사용자 확인 필요 — 오버레이를 닫으면 키 입력이 원래 창으로 다시 간다.
- [ ] 사용자 확인 필요 — (시작 시 폴더 확인) `config.json` tabs에 존재하지 않는 `dir`을 가진 탭을 넣고 시작 → "'<dir>' 디렉토리가 없습니다. 만들까요?" → `Y`면 폴더가 생기고 그 탭의 창이 재시작되어 실행된다. `N`이면 창은 실패 상태로 남는다.

결과: (미기록)

## Task 14 — 복사 모드 · 입력 동기화 · 붙여넣기 (사용자 확인 필요)

구현자는 앱 창을 직접 조작할 수 없으므로(R8) 아래는 모두 **사용자 확인 필요**다. `npx tauri dev`는 시작 오류 없이 떴다(`config_errors=0`).

동작 요약:
- `Alt+c`: 포커스된 창에서 복사 모드 토글. 창 테두리가 주황색이 되고 헤더에 키 안내가 보인다. 복사 모드 중 키는 PTY로 가지 않는다(IME 조합 입력도 차단). `Alt+h` 같은 앱 단축키는 그대로 동작한다.
- 커서는 줄 단위로 강조된다(`selectLines`). 열(col)은 그리지 않는다 — 한글 전각 셀과 열 매핑이 모호하기 때문이다. 선택은 줄 단위.
- 복사 모드는 창별 상태다. 탭을 갔다 와도 유지되고, 다른 창에서는 그 창의 키를 가로채지 않는다.
- `/` 검색 입력 중에도 복사 모드는 유지된다(오버레이가 위에 쌓였다가 닫힘).
- `Alt+i`: 탭의 입력 동기화 토글. 켜면 상태 표시줄에 `[SYNC]`. 입력은 탭의 실행 중인 모든 창에 한 번씩 전달된다.
- `Ctrl+Shift+V`: 클립보드 텍스트를 포커스된 창에 붙여넣기(bracketed paste는 xterm이 처리). 동기화 중이면 같은 경로로 모든 창에 전달된다. 복사 모드 중에는 무시된다.

### Step 5 — 수동 확인
- [ ] 사용자 확인 필요 — `Alt+c` → `k`·`v`·`j`·`y` → "복사했습니다" 토스트, 복사 모드 종료 → 다른 창에서 `Ctrl+Shift+V`로 같은 텍스트가 붙는다.
- [ ] 사용자 확인 필요 — `Alt+c` 후 `Alt+l`(다른 창) → 그 창에 타이핑하면 정상 입력, `Alt+h`로 돌아오면 복사 모드가 유지된다. 탭을 갔다 와도 유지된다.
- [ ] 사용자 확인 필요 — 복사 모드에서 `PageUp/PageDown`, `g`/`G`, 방향키가 동작하고 커서 줄이 보이도록 스크롤된다. 한글 IME를 켠 채 `ㅓ`(j) 등을 눌러도 이동만 하고 PTY에 글자가 가지 않는다.
- [ ] 사용자 확인 필요 — `/` → 검색어 → Enter → 일치 위치로 이동·강조, `n`/`N`으로 다음/이전, 없으면 토스트. `/` 입력 중 `Esc` 후에도 복사 모드가 유지된다. `Esc`/`q`로 종료하면 강조가 사라진다.
- [ ] 사용자 확인 필요 — `Alt+i` → `[SYNC]` 표시 → 타이핑하면 탭의 모든 실행 중 창에 입력되고 중복 입력이 없다. 종료된 창에는 전달되지 않는다. `Alt+i`로 해제.
- [ ] 사용자 확인 필요 — 동기화 중 `Ctrl+Shift+V` → 모든 창에 한 번씩만 붙는다.

알려진 한계: 동기화 중 터미널이 스스로 보내는 응답(예: 커서 위치 질의 답)도 같은 경로라 다른 창에도 전달된다.

결과: (미기록)

## Task 15 — 주의(attention) 표시 (사용자 확인 필요)

구현자는 앱 창을 직접 조작할 수 없으므로(R8) 아래는 모두 **사용자 확인 필요**다. `npx tauri dev`는 시작 오류 없이 떴다.

동작 요약:
- `term.onBell` → 그 창이 포커스 창(포커스된 그룹 → 활성 탭 → lastFocused)이 아니면 `SetAttention{on:true}`. 이미 ⚠인 창은 다시 보내지 않는다. 앱 창 자체가 포커스를 잃은 상태면 포커스된 창이어도 표시하고, 창이 다시 포커스를 받으면 `SetFocus`(같은 창)로 해제한다.
- 첫 attach 때 재생되는 스냅샷 안의 BEL은 무시한다(과거 출력).
- 포커스는 절대 자동으로 옮기지 않는다. 표시: 창 헤더 ⚠, 탭 바 배지, 상태 표시줄(Task 11).
- `Alt+a`(`pane.nextAttention`): 그룹 → 탭 → 창 순서로 다음 ⚠ 창으로 `SetFocus`. 남은 ⚠가 없으면 처음 시작한 위치로 돌아간다.

### Claude Code 터미널 벨 설정 (README에 포함할 것 — Task 17)
Claude Code에서 `/config` → 알림 채널(notification channel)을 terminal bell로 설정한다(`preferredNotifChannel: "terminal_bell"`). 그래야 권한 승인 대기 시 BEL이 울려 AgentBoard가 ⚠를 표시한다.

### Step 1 — 수동 확인
- [ ] 사용자 확인 필요 — 비활성 탭의 창에서 `powershell -c "[console]::beep(); Write-Host \"`a\""`를 실행하거나 Claude 권한 프롬프트를 띄운다 → 헤더 ⚠와 탭 배지가 표시되고, 포커스는 그대로다.
- [ ] 사용자 확인 필요 — `Alt+a` → 그 창으로 이동하고 ⚠가 해제된다.
- [ ] 사용자 확인 필요 — 다시 `Alt+a` → 원래 위치로 돌아온다.
- [ ] 사용자 확인 필요 — 앱 창이 다른 앱 뒤에 있을 때 벨 → ⚠ 표시, 창을 다시 활성화하면 ⚠ 해제.

결과: (미기록)

## Task 16 — 마우스 드래그앤드롭 + 스파이크 S6 (사용자 확인 필요)

구현자는 앱 창을 직접 조작할 수 없으므로(R8) 아래는 모두 **사용자 확인 필요**다. `npx tauri dev`는 시작 오류 없이 떴다. `tauri.conf.json`에 `dragDropEnabled: false`가 설정되어 있음을 확인했다(WebView2에서 HTML5 DnD를 쓰기 위한 조건).

구현 요약 (`src/dnd.js`, `#groups`에 이벤트 위임):
- 탭은 `draggable`. `dragstart` 다음 프레임에 각 그룹 본문(탭 바 아래) 위에 `.drop-overlay`를 깔고, 드래그 중 `dropZone`(가장자리 25%)에 따라 미리보기 사각형(`.drop-preview`)을 그린다.
- 탭 바 위에 놓기 → `MoveTab{index}` (대상 탭 중앙 기준 앞/뒤, 같은 그룹 안에서는 "제거 후 최종 위치"로 변환, 제자리면 명령 안 보냄). 빈 영역 → 끝.
- 그룹 본문 center → `MoveTab{index: 끝}`. 가장자리 → `SplitGroupWith{dir, before}` (left/right = horizontal, top/bottom = vertical, left/top = before).
- 오버레이는 dragend / drop / Esc / 창 blur / 드래그 후 첫 mousemove·mouseup 때 모두 제거된다.
- 탭 선택은 mousedown 대신 click에서 처리한다(mousedown의 preventDefault는 드래그 시작을 막고, mousedown에서 보낸 명령이 탭 DOM을 다시 그려 드래그를 끊을 수 있다). click 직후 터미널로 DOM 포커스를 되돌린다. 더블클릭에는 동작이 없다.

### S6 — 터미널 위에서 드래그 이벤트가 오버레이로 들어오는가
- [ ] 사용자 확인 필요 — 탭을 잡고 다른 그룹의 터미널 영역 위로 끌면 파란 미리보기 사각형이 나타난다(xterm이 이벤트를 삼키지 않음).
- [ ] 사용자 확인 필요 — 드래그 중 Esc / 창 밖에서 놓기 / Alt+Tab 후 `.drop-overlay`가 남지 않는다.
- [ ] 사용자 확인 필요 — 드래그가 아예 시작되지 않거나 오버레이가 이벤트를 받지 못하면(WebView2 문제) **대체안**: 탭에서 `mousedown`/`mousemove`/`mouseup`으로 직접 구현한다. 4px 이상 움직이면 드래그 시작, 고스트 요소를 따라다니게 하고, `document.elementFromPoint`로 오버레이/탭 바를 찾아 같은 `dropZone`/`tabBarDrop`/`bodyDrop`을 호출한다. 결과를 아래에 기록한다.

### Step 5 — 4가지 드롭 + 클릭
- [ ] 사용자 확인 필요 — 드롭이 실제로 실행되는지(조용히 사라지지 않는지): 드래그 중 마우스를 멈췄다 움직이거나 다른 창을 잠깐 건드려도 오버레이가 유지되고, 놓으면 이동/분할이 실제로 일어난다.
- [ ] 사용자 확인 필요 — 같은 그룹 탭 바 안에서 순서 변경(왼쪽/오른쪽 이동, 제자리에 놓으면 아무 일도 없음).
- [ ] 사용자 확인 필요 — 다른 그룹 탭 바 위(탭의 앞/뒤, 빈 영역=끝)로 이동.
- [ ] 사용자 확인 필요 — 다른 그룹 본문 중앙에 놓으면 그 그룹 탭 끝에 추가된다.
- [ ] 사용자 확인 필요 — 그룹 본문 가장자리(왼/오/위/아래)에 놓으면 그 방향으로 새 그룹이 생긴다. 탭이 하나뿐인 자기 그룹의 가장자리에 놓으면 한글 오류 토스트.
- [ ] 사용자 확인 필요 — 놓은 뒤 포커스가 옮긴 탭의 마지막 창으로 가고 바로 타이핑된다.
- [ ] 사용자 확인 필요 — Dispatcher 탭도 끌어서 옮길 수 있다.
- [ ] 사용자 확인 필요 — 탭 클릭이 정상(활성 탭 전환, 터미널 포커스 유지)이고, 더블클릭해도 아무 일도 일어나지 않는다.

결과: (미기록)

## Task 17 — 앱 종료 확인 (사용자 확인 필요)

구현자는 앱 창을 직접 조작할 수 없으므로(R8) 아래는 모두 **사용자 확인 필요**다.

동작 요약:
- 창의 X 버튼(또는 `Alt+F4`)은 `onCloseRequested`에서 `preventDefault`된다. `running_count`가 0이면 묻지 않고 `app_exit`, 1 이상이면 "실행 중인 에이전트 N개를 종료하고 닫을까요?" 확인 대화(`Y`/`Enter` 종료, `N`/`Esc` 취소)를 띄운다. 대화가 열려 있는 동안 또 닫기를 눌러도 무시한다.
- `app_exit`: `workspace.json` 저장(flush) → 모든 창 프로세스 kill → 앱 종료. Job Object가 손자 프로세스까지 정리한다.
- 닫기 요청 시 이미 열려 있던 다른 오버레이는 닫힌다.

- [ ] 사용자 확인 필요 — 실행 중인 창이 있는 상태에서 X → 확인 대화에 올바른 개수가 표시된다. `N` → 앱이 그대로 유지된다.
- [ ] 사용자 확인 필요 — 대화가 열린 상태에서 `Alt+F4`를 한 번 더 눌러도 대화가 하나만 유지된다.
- [ ] 사용자 확인 필요 — `Y` → 앱이 닫히고, 작업 관리자에 `claude`·`pwsh`(`powershell`) 프로세스가 남지 않는다.
- [ ] 사용자 확인 필요 — 모든 창을 `Alt+x`로 닫거나 프로세스를 종료해 실행 중 0개인 상태에서 X → 묻지 않고 바로 닫힌다.

결과: (미기록)

## QA 결과 (스펙 §11 수동 체크리스트 1~7)

구현자가 직접 수행할 수 없어 **전 항목이 사용자 확인 필요**다. 관찰하지 않은 결과는 적지 않는다. 자동 검증(`cargo test`, `npm run test:js`, 릴리스 빌드 산출물 생성)은 Task 17 보고서에 있다. 릴리스 설치본(`src-tauri/target/release/bundle/nsis/*.exe`)에서 실행해도 되고 `npx tauri dev`로 해도 된다. 설치본 실행 확인(Task 17 Step 4 후반)도 여기서 함께 한다.

준비: `C:/workspace/.agentboard/config.json`에 탭 몇 개(예: `solo` 템플릿, `team-3` 템플릿)를 두고 실행한다. Claude Code(`claude`)가 PATH에 있어야 한다.

1. **마우스 없이 전체 흐름** — [ ] 사용자 확인 필요
   마우스를 쓰지 않고: `Alt+n`(새 탭, 폴더·템플릿 선택) → `Alt+\`·`Alt+-`(분할) → `Alt+h/j/k/l`(창 이동, 그룹 경계 넘기) → `Alt+Shift+H/J/K/L`(크기 조절) → `Alt+c`(복사: `v`·`y`) → `Ctrl+Shift+V` 붙여넣기 → `Alt+1..9`, `Ctrl+Alt+h/l`(탭 전환) → `Ctrl+Alt+Shift+H/L` 또는 `Ctrl+Alt+m`(탭을 다른 그룹으로) → `Ctrl+Alt+g`(그룹 합치기) → `Alt+x`·`Ctrl+Alt+x`(닫기). 중간에 막히거나 마우스가 필요한 단계가 없어야 한다.
2. **한글 IME 켠 상태에서 모든 단축키** — [ ] 사용자 확인 필요
   한/영 키로 한글 입력 상태로 만든 뒤 1번의 단축키를 다시 눌러 동일하게 동작하는지 본다(`e.code` 기준 매칭).
3. **Claude Code로 키 전달** — [ ] 사용자 확인 필요
   `claude` 창에서 `Ctrl+R`(기록 검색), `Ctrl+P`, `Alt+P`(모델 전환), `Alt+V`, `Shift+Tab`(모드 전환), `Ctrl+V`(클립보드 이미지 → 이미지 칩)가 모두 Claude Code에 전달되고 앱/WebView가 가로채지 않는다.
4. **3×3 Claude 동시 실행 성능** — [ ] 사용자 확인 필요
   Task 11의 S4 절 설정으로 Claude 9개를 동시에 띄워 입력 지연·CPU/메모리를 본다(수치를 S4 "결과:"에 적는다).
5. **종료 → 재실행 복원, 고아 프로세스 없음** — [ ] 사용자 확인 필요
   그룹 2개·탭 여러 개·분할·포커스를 만든 뒤 앱을 닫는다(Task 17 확인 대화에서 `Y`). 즉시 작업 관리자(세부 정보)에서 `claude`·`pwsh`/`powershell`·`conhost`·`OpenConsole` 프로세스가 남지 않는지 센다(종료 전 개수와 비교). 다시 실행해 그룹/탭/레이아웃/포커스/최대화/동기화 상태가 복원되고 각 창 프로세스가 새로 실행되는지 본다. 앱을 작업 관리자에서 강제 종료한 경우에도 자식이 사라지는지(Job Object) 한 번 확인한다.
6. **마우스 드래그 4가지 드롭** — [ ] 사용자 확인 필요
   Task 16 Step 5 절의 항목(탭 바 순서 변경, 다른 그룹 탭 바, 그룹 본문 중앙, 그룹 가장자리 분할).
7. **권한 승인 대기 시 ⚠** — [ ] 사용자 확인 필요
   Claude Code의 `/config`에서 알림 채널을 terminal bell로 설정(README 참고). 비활성 탭의 Claude가 권한 승인을 기다릴 때 ⚠가 표시되고 포커스는 그대로이며, `Alt+a`로 이동하고 다시 `Alt+a`로 돌아온다.

최종 결과: (미기록)

# 2단계 스파이크 (P1–P3, 2단계 Task 1)

환경: Windows 11 Home 10.0.26200, Claude Code **2.1.310**(네이티브), Node 22.23.1, Python 3.11, Git Bash 있음. 코드·재실행 방법은 `spikes/phase2/README.md`, 실제로 받은 원본 예는 `spikes/phase2/samples/`(토큰은 `<redacted>`). 모든 스파이크 세션 이름은 `abspike-220-*`이고 다른 세션에는 아무것도 보내지 않았다. 띄운 프로세스는 모두 종료했다.

**요약: P1·P2·P3 모두 성공. 대안(PTY 주입, URL 토큰)은 쓰지 않는다.**

| # | 결과 | 한 줄 |
|---|---|---|
| P1 | 성공 | `--settings` 파일의 SessionStart hook이 소켓·토큰을 받고, 외부 프로세스가 파이프에 auth 줄 + 메시지 줄을 쓰면 세션이 **peer 메시지**로 받아 새 턴을 시작한다(`-p`·대화형 ConPTY 모두) |
| P2 | 성공 | `--name` 세션끼리 `SendMessage`가 auto 모드에서 hold 없이 1초 안에 도착(`accept`가 있을 때·키가 없을 때 모두) |
| P3 | 성공 | `--mcp-config` HTTP + `headers.Authorization`이 모든 요청에 실리고, 평문 JSON 응답으로 initialize·tools/list·tools/call 왕복 |

## P1 — SessionStart hook → inbox named pipe

### `--settings` 파일
파일 경로를 그대로 준다(`--settings C:/…/x.settings.json`, 슬래시 경로 OK). hook과 `crossSessionInbound`가 함께 적용된다. 쓴 형식:

```json
{"crossSessionInbound":"accept",
 "hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node C:/workspace/agentboard/spikes/phase2/hooklog.mjs SessionStart C:/…/hooks"}]}],
          "UserPromptSubmit":[…같은 꼴…],"Stop":[…],"Notification":[…]}}
```
- hook 명령(shell form)은 **Git Bash**로 실행된다(Git Bash가 없으면 PowerShell, 문서). **검증된 것은 공백 없는 경로의 shell form뿐이다**: `"command":"node C:/workspace/agentboard/spikes/phase2/hooklog.mjs SessionStart C:/…/hooks"`.

#### 앱용 settings 파일 (복사용)
`agentboard.exe`가 공백 있는 경로(예: `C:\Program Files\AgentBoard\agentboard.exe`)에 있을 때의 형태:

```json
{
  "crossSessionInbound": "accept",
  "hooks": {
    "SessionStart":     [{"hooks":[{"type":"command","command":"\"C:/Program Files/AgentBoard/agentboard.exe\" --hook SessionStart"}]}],
    "UserPromptSubmit": [{"hooks":[{"type":"command","command":"\"C:/Program Files/AgentBoard/agentboard.exe\" --hook UserPromptSubmit"}]}],
    "Stop":             [{"hooks":[{"type":"command","command":"\"C:/Program Files/AgentBoard/agentboard.exe\" --hook Stop"}]}],
    "Notification":     [{"hooks":[{"type":"command","command":"\"C:/Program Files/AgentBoard/agentboard.exe\" --hook Notification"}]}]
  }
}
```
따옴표·이스케이프 규칙:
- `command` 문자열은 Git Bash가 해석한다. exe 경로를 **큰따옴표로 감싸고**, JSON 안에서는 그 큰따옴표를 `\"`로 쓴다.
- 경로 구분자는 **슬래시(`C:/Program Files/…`)를 권장**한다. 역슬래시를 쓰려면 JSON에서 `\\`로 두 번 쓴다: `"\"C:\\Program Files\\AgentBoard\\agentboard.exe\" --hook SessionStart"`. 그러면 bash는 `"C:\Program Files\AgentBoard\agentboard.exe"`를 받는다. bash 큰따옴표 안의 `\P`·`\A`는 그대로 남지만, 경로에 `$`, `` ` ``, `"`, `\`가 연달아 오는 경우는 깨질 수 있다. 그래서 슬래시가 안전하다.
- Rust에서는 `serde_json`으로 만들면 JSON 이스케이프는 자동이다. 앱은 `format!("\"{}\" --hook {}", exe_path.replace('\\', "/"), event)`만 만들면 된다.
- **미검증**: 공백 있는 경로를 따옴표로 감싼 shell form(위 JSON)은 스파이크에서 실제로 실행해 보지 않았다. 검증된 것은 공백 없는 경로뿐이다. Task 4/5에서 첫 실행 때 SessionStart 보고가 오는지로 확인할 것.
- **미검증**: exec form(`"command":"C:/Program Files/AgentBoard/agentboard.exe","args":["--hook","SessionStart"]`, 문서상 실제 `.exe`면 셸 없이 실행). 따옴표 문제가 없어 더 안전해 보이지만 실행해 보지 않았다.

### hook이 받는 환경 변수 (SessionStart뿐 아니라 모든 hook에 있음)
```
CLAUDE_CODE_MESSAGING_SOCKET=\\.\pipe\LOCAL\cc-msg-<32 hex>
CLAUDE_CODE_MESSAGING_TOKEN=<32 hex>
CLAUDE_CODE_SESSION_ID=<uuid>          (= stdin.session_id)
CLAUDE_PROJECT_DIR=C:/…/cwd            (슬래시 경로)
CLAUDE_ENV_FILE=C:\Users\…\.claude\session-env\<uuid>\sessionstart-hook-0.sh
CLAUDE_PID, CLAUDECODE=1, CLAUDE_CODE_ENTRYPOINT, CLAUDE_CODE_CHILD_SESSION, CLAUDE_CODE_SESSION_ATTENDED …
```

### SessionStart stdin
```json
{"session_id":"9887ea82-…","transcript_path":"C:\\Users\\<user>\\.claude\\projects\\<cwd-slug>\\9887ea82-….jsonl",
 "cwd":"C:\\…\\p1","hook_event_name":"SessionStart","source":"startup","session_title":"abspike-220-a"}
```
대화형은 여기에 `"scratchpad_dir"`, `"model":"claude-haiku-5-5"`가 더 붙는다. **`session_title` = `--name` 값**이다. `source`: `startup`/`resume`/`clear`/`compact`/`fork`(문서).

### 파이프에 쓰는 정확한 형식 (확정)
named pipe에 연결(Node `net.connect(path)`) → 아래 두 줄을 쓰고 → 연결 종료. 서버는 아무것도 답하지 않는다(빈 응답).
```
{"type":"auth","token":"<CLAUDE_CODE_MESSAGING_TOKEN>"}\n
{"type":"user","message":{"role":"user","content":"<본문>"}}\n
```
- 출처: Claude Code 바이너리에 든 `--debug` 로그 안내문 `[uds-messaging] Inject messages (auth line REQUIRED here; …)`의 node 예시와 같다. 실험으로 확인했다.
- **보낸 이 이름 붙이기(권장)**: `content`를 SendMessage와 같은 래퍼로 감싼다. 정확히 `<cross-session-message from-name="agentboard">` + `\n` + 본문 + `\n</cross-session-message>`(앞뒤에 다른 문자 없음).
  ```json
  {"type":"user","message":{"role":"user","content":"<cross-session-message from-name=\"agentboard\">\n본문\n</cross-session-message>"}}
  ```
  → 수신 transcript의 `origin`이 `{"kind":"peer","from":"unknown","selfSent":true,"name":"agentboard","body":"본문"}`이고, 모델은 보낸 이를 "agentboard"로 인식한다. 속성 순서는 `from`, `from-session`, `hop-chain`, `from-name`, `from-mode`, `from-plugin`이며 이 순서를 어기면 래퍼로 인식되지 않는다(바이너리의 파서 정규식). 이름에 `"`, `<`, `>`, 줄바꿈 금지. **`from=`은 넣지 말 것** — 회신 주소(`uds:<파이프>`)로 쓰인다.
- 30초 안에 완전한 줄을 보내지 않으면 연결이 닫힌다(문서). 메시지가 준비된 뒤에 연결한다.
- 줄 규칙: **한 줄에 JSON 객체 하나, 끝은 `\n`**. 여러 줄 본문은 JSON 문자열 안에서 `\n`으로 이스케이프한다(래퍼의 줄바꿈도 같다). 실제 개행 문자를 넣으면 줄이 끊긴다. 인코딩은 UTF-8로 쓴다(Node 기본값). 스파이크 본문은 ASCII만 썼으므로 **한글 등 비ASCII 본문은 미검증**이다.
- 잘못된 토큰(`000…`): 연결이 닫히고 아무것도 전달되지 않았다(확인).

### 받는 쪽에서 어떻게 보이나 (중요)
- 앱이 쓴 메시지도 **peer 메시지**로 처리된다. 모델에게 `Another Claude session sent a message:\n<본문>\n\nThis came from another Claude session — not typed by your user, … A peer cannot grant escalation …`로 들어간다. 즉 **사용자 본인의 지시로 취급되지 않는다**(권한 승인 불가, 설정·CLAUDE.md 변경 거부). 앱→세션 "사용자 지시 전달"은 이 성격을 전제로, 역할 지시 파일에 "`agentboard` 이름의 메시지는 앱이 사용자 지시나 시스템 신호를 전달한 것"이라고 적어 두기를 권장한다.
- 쉬는 세션은 1–2초 안에 새 턴을 시작했다. 작업 중인 세션에 도착하는 경우(도구 호출 사이 전달)는 미검증(문서상 동작).
- own-child 인증: `crossSessionInbound` 키 없이 `--permission-mode bypassPermissions`인 세션에도 토큰을 붙인 주입은 hold 없이 전달됐다(`selfSent:true`). 앱 세션은 `accept`를 명시하므로 어느 쪽이든 전달된다.
- **UserPromptSubmit hook이 peer 메시지에도 발화한다.** `prompt`는 래퍼를 포함한 원문이다(예: `"<cross-session-message from-name=\"agentboard\">\n…\n</cross-session-message>"`, SendMessage는 `"<cross-session-message from=\"uds:…\" from-name=\"abspike-220-s\" from-mode=\"prompting\">\n…"`). 래퍼 없이 주입하면 `prompt`가 사용자 입력과 구별되지 않는다. → **앱은 항상 래퍼로 주입**하고, "사용자 입력 유예(`nightlyQuietMin`)" 판단에서 `prompt`가 `<cross-session-message`로 시작하는 UserPromptSubmit은 빼야 한다.

### 대화형(ConPTY, pywinpty) 확인
- 앱 pane과 같은 대화형 세션(`claude --model haiku --name … --settings …`)에서도 SessionStart hook → 파이프 주입 → 새 턴 → Stop hook(`last_assistant_message:"GOT pty-pipe-msg-444"`)까지 auto 모드·default 모드 모두 성공했다.
- [ ] 사용자 확인 필요 — 입력란에 쓰다 만 글이 있을 때 파이프 메시지가 와도 입력란 내용이 보존되는지(스파이크는 입력란이 빈 상태에서만 확인).

## P2 — `--name` 세션끼리 SendMessage

`claude -p --input-format stream-json --output-format stream-json --verbose --model haiku --name abspike-220-{r1,r2,s} --permission-mode auto --settings <파일>` 워커 3개(`worker.mjs`, stdin을 열어 두어 장기 실행). r1 = `crossSessionInbound:"accept"`, r2 = 키 없음(기본 규칙), s = 송신자.

- 송신자 모델의 호출(`SendMessage`는 `-p`에서 지연 도구라 ToolSearch로 먼저 로드됨): `{"to":"abspike-220-r1","message":"p2-hello-220","summary":"…"}`
- 결과: `{"success":true,"message":"“…” → abspike-220-r1 (another Claude session on this machine; in that session's inbox, not yet read by its Claude — a [Cross-session delivery notice] follows if that session holds it …)","msg_id":"c0196588-…"}`
- r1·r2 모두 1초 안에 새 턴으로 받았다(`GOT: p2-hello-220`), hold 없음. 수신 origin: `{"kind":"peer","from":"uds:\\\\.\\pipe\\LOCAL\\cc-msg-…","msg_id":"…","name":"abspike-220-s","fromMode":"prompting","body":"p2-hello-220"}`.
- 이름만으로 주소 지정이 된다(같은 이름의 세션이 하나일 때). 세션 이름 규칙(스펙 §3)과 맞는다.
- [ ] 사용자 확인 필요(선택) — 대화형 TUI 두 개끼리의 SendMessage. 전달 경로(같은 파이프)와 대화형 수신은 P1 대화형에서 확인했으므로 위험은 낮다.

## P3 — `--mcp-config` HTTP + Authorization

`{"mcpServers":{"ab":{"type":"http","url":"http://127.0.0.1:47811/mcp","headers":{"Authorization":"Bearer x"}}}}`, `claude -p --strict-mcp-config --mcp-config <파일> --allowedTools mcp__ab__echo_who …` → 도구 결과 `auth=Bearer x text=ping`. 요청 원본: `spikes/phase2/samples/mcp-requests.jsonl`.

Claude Code가 보내는 순서(모든 요청에 `authorization: Bearer x`, `accept-encoding: identity`, `user-agent: claude-code/2.1.310 (…)`):

| # | 요청 | 특이 헤더 | 스파이크 응답 |
|---|---|---|---|
| 1 | `POST /mcp` `{"jsonrpc":"2.0","id":"server-discover-probe-1","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",…}}}` | `mcp-method: server/discover`, `mcp-protocol-version: 2026-07-28`, `accept: application/json, text/event-stream` | 200 + JSON-RPC 오류 `-32601` → 클라이언트가 2번으로 대체 |
| 2 | `POST` `{"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"roots":{"listChanged":true},"elicitation":{"form":{},"url":{}}},"clientInfo":{"name":"claude-code",…}},"jsonrpc":"2.0","id":0}` | `mcp-protocol-version` 없음 | 200 `application/json`, 같은 protocolVersion 반환, `Mcp-Session-Id: spike-session-1` 헤더 |
| 3 | `POST` `{"jsonrpc":"2.0","method":"notifications/initialized"}` | 이후 모든 요청에 `mcp-protocol-version: 2025-11-25`, `mcp-session-id: spike-session-1` | 202 빈 본문 |
| 4 | `GET /mcp` | `accept: text/event-stream` | 405 → 문제없이 계속 |
| 5 | `POST` `{"method":"tools/list","jsonrpc":"2.0","id":1}` | | 200 `{"result":{"tools":[…]}}` |
| 6 | `POST` `{"method":"tools/call","params":{"name":"echo_who","arguments":{"text":"ping"},"_meta":{"claudecode/toolUseId":"toolu_…","progressToken":2}},"jsonrpc":"2.0","id":2}` | | 200 `{"result":{"content":[{"type":"text","text":"…"}],"isError":false}}` |

control.rs 구현 지침: `server/discover`에는 **정확히 HTTP 200 + JSON-RPC 오류 `{"jsonrpc":"2.0","id":<요청 id>,"error":{"code":-32601,"message":"Method not found"}}`**로 답한다. 검증된 것은 이 응답뿐이다. 404·405·500 같은 다른 HTTP 응답에 클라이언트가 initialize로 대체하는지는 **미검증**이므로, Task 5는 반드시 200 + -32601로 답한다. `initialize`는 요청의 `protocolVersion`을 돌려주고, 알림(id 없음)은 202, `GET`은 405, 응답은 항상 `Content-Type: application/json` 단일 JSON. `Mcp-Session-Id`는 주면 그대로 되돌아온다(안 줬을 때의 동작은 미검증 — 주는 쪽을 권장). id는 숫자·문자열 둘 다 온다. 도구 이름은 `mcp__<서버키>__<도구>`.
- 대안(URL 경로 토큰)은 필요 없다.
- 참고: P3 실행은 부모 세션 환경을 물려받아 user-agent에 `claude-desktop`이 찍혔다(아래 "환경 변수 정리").

## hook stdin 예 (Notification · Stop · UserPromptSubmit)

```json
// Stop (-p, 대화형 같은 꼴)
{"session_id":"…","transcript_path":"…","cwd":"…","prompt_id":"e81a18b0-…","permission_mode":"default","effort":{"level":"medium"},
 "hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"GOT: pipe-hello-7731","background_tasks":[],"session_crons":[]}
// UserPromptSubmit
{"session_id":"…","transcript_path":"…","cwd":"…","prompt_id":"…","permission_mode":"auto","hook_event_name":"UserPromptSubmit","prompt":"<입력 또는 peer 원문>","session_title":"abspike-220-a"}
// Notification — 권한 승인 대기 (대화형 default 모드, 프롬프트 후 약 10초)
{"session_id":"…","transcript_path":"…","cwd":"C:\\workspace","scratchpad_dir":"…","prompt_id":"…","hook_event_name":"Notification","message":"Claude needs your permission","notification_type":"permission_prompt"}
// Notification — idle (Stop 후 약 60초 무입력)
{"session_id":"…","transcript_path":"…","cwd":"C:\\workspace","scratchpad_dir":"…","prompt_id":"…","hook_event_name":"Notification","message":"Claude is waiting for your input","notification_type":"idle_prompt"}
```
- Notification은 `-p` 워커에서는 한 번도 발화하지 않았다(대화형 전용).
- **`permission_prompt`는 `--permission-mode default`인 대화형 세션에서만 관찰했다.** 이 PC의 대화형 기본값은 `auto`이고, auto 실행에서는 같은 Write 요청이 프롬프트 없이 허용됐다. 그래서 permission_prompt가 오지 않았고 실제로 파일이 써졌다. 즉 권한 대기 주의(⚠) 표시는 **권한 모드에 따라 다르다**. auto 세션은 대부분 권한 대기 자체가 생기지 않으며, auto 분류기가 묻는 경우의 동작은 미검증이다.
- `idle_prompt`: 정상 종료(Stop) 후 약 60초 동안 입력이 없을 때 1회 왔다(auto 실행, Stop 17:28:00 → Notification 17:29:07).
- Esc 경우: 권한 프롬프트를 Esc로 중단하면 Stop이 오지 않았고, 이후 110초 동안 `idle_prompt`도 오지 않았다(아래 항목).
- 시험 주의: auto 모드의 대화형 시험은 **실제로 파일을 쓴다**(스파이크에서 `C:/workspace/abspike-should-not-exist.txt`가 생겨 바로 지움). 권한 관련 시험은 `--permission-mode default`로 하고 거부(Esc)한다.
- 대화형에서 권한 프롬프트를 **Esc로 중단**하면 Stop hook이 오지 않았고, 이후 110초 동안 `idle_prompt`도 오지 않았다. → 주의 표시·야간 정리의 "idle" 판단을 Stop 하나에만 기대지 말 것(UserPromptSubmit 뒤 Stop 없이 끝나는 경우가 있다).
- `permission_mode`는 이 PC의 대화형 기본값이 `auto`로 나왔다(사용자 설정).

## 그 밖의 발견 (후속 Task가 알아야 할 것)
- **폴더 신뢰 대화 상자**: 대화형 `claude`를 신뢰되지 않은 폴더에서 띄우면 "Quick safety check … ❯ No, exit / Yes, I trust this folder"가 먼저 뜬다. `~/.claude.json`에 `C:/workspace/agentboard`가 `hasTrustDialogAccepted:false`로 따로 기록돼 있어 부모 `C:/workspace`의 신뢰를 물려받지 못했다. 앱이 role 세션을 새 작업 폴더에서 띄우면 이 대화 상자에서 멈추고 SessionStart hook도 그 전에는 오지 않는다 → 사용자가 한 번 수락해야 한다. 스파이크는 사용자 설정을 바꾸지 않으려고 수락하지 않고 신뢰된 `C:/workspace`에서 대화형을 돌렸다.
- **환경 변수 정리**: 부모 프로세스에 `CLAUDE_CODE_MESSAGING_SOCKET/TOKEN` 등이 있어도 자식 세션은 자기 값을 새로 내보냈다(확인). 그래도 앱이 Claude 세션 안에서 실행되는 경우를 대비해 role 세션을 띄울 때 `CLAUDE*` 변수를 지우기를 권장한다(`spikes/phase2/cleanenv.sh` 참고).
- 토큰은 그 세션의 모든 hook·Bash 명령(자식 프로세스)에 보인다. 앱 제어 서버로 보고할 때만 쓰고 로그에 남기지 않는다.

결과: P1 성공 / P2 성공 / P3 성공 (2026-10-25, Claude Code 2.1.310). 대안 미채택.

## 2단계 Task 4 검증 — 실제 hook → `agentboard.exe --hook` → HTTP POST

2026-10-25, Claude Code 2.1.310. 한 번 실행: `claude -p --model haiku --name abverify-t4-<난수> --settings <파일> "Reply with exactly: OK"`(cwd `C:/workspace`, `cleanenv.sh`로 부모 `CLAUDE*` 변수 제거, 환경에 `AGENTBOARD_URL=http://127.0.0.1:<임의 포트>`, `AGENTBOARD_TOKEN=<시험 토큰>`). settings는 앱이 만드는 것과 같은 꼴(`crossSessionInbound: accept` + SessionStart/UserPromptSubmit/Stop/Notification). hook 명령은 Task 4 dev 빌드 `agentboard.exe`를 **공백이 있는 폴더**(`…/hookverify/hook bin/agentboard.exe`)에 복사해 `"\"C:/…/hook bin/agentboard.exe\" --hook SessionStart"` 형태로 줬다. 받는 쪽은 요청을 JSONL로 적고 204로 답하는 작은 Node HTTP 리스너.

결과: **성공.** 모델 답 `OK`, claude 종료 코드 0. 리스너가 받은 요청:
- `POST /hook/SessionStart`, `POST /hook/UserPromptSubmit`, `POST /hook/Stop`(이 순서. Notification은 `-p`라 오지 않음, 스파이크와 같음)
- 모두 `Authorization: Bearer <시험 토큰>`, `Content-Type: application/json`
- 본문 키 `event`, `stdin`, `socket`, `msgToken`, `cwd`. `socket` = `\\.\pipe\LOCAL\cc-msg-<32 hex>`, `msgToken` = 32자, `cwd` = `C:\workspace`
- `stdin`은 파싱된 hook JSON 그대로. SessionStart에는 `session_title` = `--name` 값이 있었다
→ 스파이크의 미검증 항목 "공백 있는 경로를 따옴표로 감싼 shell form"이 이제 **확인됨**. hook 모드는 Tauri 창을 띄우지 않고 바로 끝났다(남은 프로세스 없음).

## 2단계 Task 7 검증 — 실제 세션에 브리지 메시지 전달

`cargo test --test bridge_real -- --ignored --nocapture`(`src-tauri/tests/bridge_real.rs`). Claude Code 2.1.310, `claude -p --input-format stream-json --output-format stream-json --verbose --model haiku --name abverify-t7-<n> --settings <앱의 역할 settings 파일>`(`agentcfg::settings_json`: hook → 이 빌드의 `agentboard.exe --hook`, `crossSessionInbound: accept`), `AGENTBOARD_URL/TOKEN`은 실제 제어 서버(`control::serve`)와 발급 토큰, 부모 세션의 `CLAUDE*` 변수는 지움. 브리지 작업 스레드(`bridge::start_worker`) 실행 중.

- hook 순서: `SessionStart` → `UserPromptSubmit` → (첫 답) → `Stop`. SessionStart 처리기가 파이프(`\.\pipe\LOCAL\cc-msg-…`)를 현재 generation으로 등록했다.
- `bridge::send_tracked(.., "bridge-7731 한글 확인")` → 결과 `Ok(Pipe)`(1초 안). 세션은 `UserPromptSubmit` → 새 턴 → `Stop`으로 받았고, 답에 "from AgentBoard"와 본문(`bridge-7731`, `한글`)이 나왔다. 즉 **auth 줄 + `from-name="agentboard"` 래퍼 + `[AgentBoard] ` 머리말 형식이 실제로 전달되고, 비ASCII(한글) 본문도 깨지지 않는다**(스파이크 미검증 항목 해소).
- 받은 쪽은 peer 메시지로 취급하므로 첫 턴의 "GOT: 로 답하라" 같은 형식 지시를 따르지 않을 수 있다(스파이크의 "사용자 지시로 취급되지 않는다"와 같음). 역할 지시 파일(Task 13)에 `agentboard` 메시지의 성격을 적어 둘 것.
- 연결 직후 닫아도(쓰기 후 바로 handle drop) 메시지는 유실되지 않았다.
- 시험 뒤 claude 프로세스 트리는 `taskkill /T`로 종료했다. 비용: haiku 두 턴.

## 2단계 Task 9 — 주의(⚠) 표시 수동 확인 (사용자 확인 필요, 미실시)

자동 테스트는 hook 본문·generation·타이머를 흉내 낸다. 아래는 실제 역할 세션(앱이 띄운 `claude`, `--settings`의 hook이 `agentboard.exe --hook`)으로 확인할 항목이다. 결과를 이 절에 적을 것.

1. **permission_prompt → ⚠**: manager 탭 세션을 `--permission-mode default`로(또는 `/permissions`로 자동 허용을 끄고) 띄운 뒤 다른 탭으로 옮겨 두고, 그 세션에 파일 쓰기를 시킨다. 권한 프롬프트가 뜨면 그 탭에 ⚠, ⚠ 툴팁에 "권한 승인 대기". 그 창이 포커스된 상태에서 같은 일을 하면 ⚠가 뜨지 않아야 한다.
2. **idle_prompt 무시**: 같은 세션이 답을 마친 뒤(Stop) 약 60초 이상 입력 없이 둔다. `idle_prompt` Notification이 와도 ⚠가 뜨지 않아야 한다(디버그 빌드 콘솔에서 hook 수신은 확인 가능).
3. **need_user**: 다른 탭에 있는 역할 세션에게 "사용자 결정이 필요한 질문을 화면에 쓰고 need_user를 호출하라"고 시킨다. 그 탭 ⚠ 툴팁(탭 바·창 헤더)에 사유가 보이는지 확인. 그다음 다른 그룹을 최대화(`Ctrl+Alt+z`)해 그 탭 바를 가리면 상태 표시줄에 `⚠ <탭>(사유…)`가 보이고, 항목에 마우스를 올리면 전체 사유, 클릭하면 그 창으로 이동하며 ⚠·목록이 사라져야 한다. `Alt+a`로도 같은 창에 가야 한다.
4. **폴더 신뢰 경고(R5)**: 한 번도 신뢰하지 않은 새 폴더(`workspaceRoot` 아래 새 폴더)를 dir로 하는 manager 탭을 만든다(예: `create_manager_tab` 또는 config.json). Claude가 "이 폴더를 신뢰하시겠습니까?"에서 멈춘 채 30초가 지나면 그 탭에 ⚠ + "폴더 신뢰 확인 필요". 그 창에서 신뢰를 승인하면 SessionStart가 오고 ⚠가 꺼져야 한다(포커스로 이미 꺼졌을 수도 있음 — 그 경우 다른 탭으로 옮긴 상태에서 30초 경과를 다시 확인). 이미 신뢰한 폴더에서는 30초가 지나도 ⚠가 뜨지 않아야 한다. 확인할 가정: Claude가 신뢰 질문에 답하기 전에는 `--settings`의 SessionStart hook을 실행하지 않는다.
5. 앱을 껐다 켜면 ⚠와 사유가 모두 사라져 있어야 한다.
## 2단계 Task 10 — 인박스 애플릿 수동 확인 (사용자 확인 필요, 미실시)

자동 테스트는 정렬·키 매핑·버튼 조건·텍스트만 렌더(가짜 DOM)와 백엔드 메시지 형식·타임라인·dispatcher 미실행 오류까지 본다. 아래는 실제 앱에서 확인할 항목이다. 기본 템플릿에는 아직 애플릿이 없으므로, `.agentboard/templates/`에 dispatcher 창 오른쪽에 `{"applet":{"name":"inbox","kind":"inbox"}}`를 둔 템플릿을 만들어 `config.dispatcher.template`으로 지정하고 확인한다(저장된 workspace.json이 있으면 dispatcher 탭이 예전 레이아웃으로 복원되므로 그 파일을 치워 둔 뒤 실행). 결과를 이 절에 적을 것.

1. **지시 → dispatcher 화면에 전달**: manager 탭 세션에게 `inbox_post`로 항목을 하나 올리게 한다. 애플릿 목록에 굵게(안 읽음) 나타나는지, `j/k`·`Enter`로 상세가 보이는지 확인. `i` → 여러 줄 지시 입력 → `Ctrl+Enter`. dispatcher 터미널에 `[AgentBoard] 인박스 지시 — 항목 <id> (<담당>): <제목>` 메시지가 새 턴으로 들어오고, dispatcher가 담당 Manager에게 전달하는지 확인. 항목은 "진행 중", 타임라인에 "지시"가 남아야 한다.
2. **입력 중 dispatcher 입력란 무손상**: dispatcher 입력란에 글자를 반쯤 쳐 둔 채 `Alt+l`로 애플릿에 가서 `i`로 지시를 보낸다. 지시 입력창에서 `j/k/h/c`를 쳐도 목록이 움직이지 않아야 하고, 돌아왔을 때 dispatcher 입력란의 글자가 그대로여야 한다(브리지는 pipe로 보내므로 입력란에 타이핑하지 않는다).
3. **앱 단축키 우선**: 애플릿에 포커스가 있을 때 `Alt+h/l`, `Alt+1..`, `Ctrl+Alt+…` 등 1단계 단축키가 그대로 동작해야 한다.
4. **자료 저장**: 이슈가 없는 항목에서 `s` → "이슈를 먼저 등록" 확인 → `Y`로 이슈 등록 요청이 dispatcher에 가는지. Manager가 `issue_create`로 이슈를 만든 뒤 `s` → 경로 여러 줄(탐색기 "경로로 복사"의 따옴표 포함) → input/output 선택 → 이슈 폴더에 복사되고 dispatcher에 `[AgentBoard] 항목 <id>에 자료 저장: …` 알림이 가는지.
5. **종료 확인**: Manager가 `inbox_propose_close`로 종료를 제안하면 목록에서 강조(노란 테두리)되는지. 다른 상태 항목에서 `c`는 안내만 뜨고, 종료 제안 항목에서 `c` → `Y`로 완료가 되며 dispatcher에 종료 확인 알림이 가는지. `h`로 완료 항목 보기/숨기기.
6. **dispatcher가 꺼져 있을 때**: dispatcher 창 프로세스를 끝낸 상태(○)에서 `i`로 지시하면 "dispatcher 세션이 실행 중이 아닙니다 …" 토스트가 뜨고 타임라인에 아무것도 기록되지 않아야 한다.
7. 탭 전환·그룹 최대화·창 분할 후에도 애플릿이 한 번만 그려지고(키 한 번에 한 칸 이동), 애플릿 창을 닫았다 다시 열어도 정상인지.
8. **포커스 유지**: 애플릿에서 `Tab`/`Shift+Tab`을 눌러도 포커스가 dispatcher 터미널로 넘어가지 않아야 한다(이후 `j`가 dispatcher 입력란에 찍히면 실패). 애플릿 창 헤더를 클릭한 뒤에도 `j/k`가 동작해야 한다.
9. **설계 확인(사용자 판단)**: 상세 보기에서 `j/k`는 항목 이동이 아니라 상세 스크롤이다(`Esc`로 목록에 돌아가 이동). 안 읽은 새 항목을 열면 읽음 처리되어 목록 뒤쪽 그룹으로 옮겨진다(정렬 규칙). 상세 보기 중 그 항목이 목록에서 빠지면(예: 종료 확인 후 완료 숨김) 다른 항목 상세로 넘어가지 않고 목록 보기로 돌아간다.

## 2단계 Task 14 — 문서 탭 수동 확인 (사용자 확인 필요, 미실시)

자동 테스트는 경로 검증(밖·`..`·junction·비 md·없는 파일 거부), 같은 문서 탭 재사용·이름 중복 규칙, md 목록 제외 폴더·상한, 저장·복원, 링크·이미지 경로 해석, 이력, 목차 id, 정화 설정·사전 필터, 키 매핑을 본다. 개발 중 브라우저 하네스(가짜 `__TAURI__`)에서 실제 렌더 파이프라인을 돌려 스크립트·`on*`·iframe·form·`javascript:` 링크가 남지 않고 KaTeX·highlight.js·Mermaid가 그려지는 것까지는 확인했다. 아래는 실제 앱에서 확인할 항목이다. 결과를 이 절에 적을 것.

1. **렌더**: 커맨드 팔레트(app.palette 키) → "문서 열기" → md 파일 선택. ```` ```mermaid ```` 블록이 다이어그램으로, `$x^2$`·`$$…$$`가 수식으로, ```` ```js ```` 코드가 색으로 표시되는지. 상대 경로 이미지(`![](img/a.png)`, 작업 폴더 안)가 보이고, 외부·`data:`·절대 경로·UNC 이미지는 `[이미지: 대체 텍스트]`로 바뀌는지. Mermaid 라벨이 글자로 보이는지(HTML 라벨은 끔).
2. **자동 새로고침**: 열어 둔 산출물 md를 에이전트(또는 메모장)가 고치면 1–2초 안에 다시 그려지고 스크롤 위치가 유지되는지. 파일을 지우면 "파일을 찾을 수 없습니다", 다시 만들면 돌아오는지.
3. **인박스에서 열기**: 이슈가 있는 항목 상세에서 input/output 파일 목록이 보이는지. `Tab` → `j/k` → md 파일에서 `Enter`로 문서 탭이 현재 그룹에 열리고 포커스를 받는지(같은 파일을 다시 열면 기존 탭으로 이동). md가 아닌 파일은 경로 안내만 뜨는지. `Esc`/`Tab`으로 목록에서 나오는지.
4. **팔레트에서 열기**: "문서 열기" 목록에 `.agentboard`·`node_modules`·`.git`·`target` 아래 파일이 없고, 한글 일부를 쳐서 퍼지 검색되는지.
5. **한글 파일명**: `보고서.md` 같은 이름이 탭 이름·목록·내용 읽기에서 깨지지 않는지. 같은 이름의 다른 폴더 파일은 `보고서.md-2`로 열리는지.
6. **키**: 문서 창에서 `j/k`, `Space/Shift+Space`, `g/G`, `/`(검색 입력 후 `n/N`, 빈 값이면 해제), `o`(목차 선택 → 이동), 문서 안 md 링크 클릭 후 `[`/`]` 뒤로/앞으로, `r` 새로고침. 1단계 앱 단축키(`Alt+h/l` 등)가 여전히 먼저 동작하고, `Tab`으로 포커스가 옆 터미널로 새지 않는지. 헤더 클릭 후에도 키가 먹는지.
7. **링크 보안**: `https://` 링크는 시스템 브라우저로 열리고 앱 화면은 그대로인지. `file:`·`javascript:`·`C:\…` 링크, 작업 폴더 밖 `../..` md 링크는 아무 일도 하지 않는지(안내 토스트만).
8. **복원**: 문서 탭을 연 채 앱을 껐다 켜면 같은 탭이 복원되는지. 그 사이 파일을 지웠으면 탭에 "파일을 찾을 수 없습니다"가 보이는지.
9. **(수정 1차) 탐색 차단**: 문서에 `<img usemap>` + `<map><area href="https://…">`, `<a target="_blank">`를 넣고 클릭·가운데 클릭·Shift+클릭해도 AgentBoard 창이 다른 페이지로 바뀌거나 새 창이 뜨지 않는지(앱 로그에 `blocked navigation`만). `## Status`, `## Parking`, `# Mermaid` 제목이 있는 문서를 열어도 상태 표시줄·탭 전환·Mermaid가 정상인지.

## 2단계 Task 11 — 야간 정리 수동 확인 (사용자 확인 필요, 미실시)

자동 테스트는 상태 기계(시작 조건·하루 1회·입력 중 연기·handover 순서·완료/시간 초과·미완료 항목·팀 제외), `nightly.json` 읽기·쓰기, `handoff_done` 권한·멱등성, `RestartPaneFresh`의 generation 안전성(새 세션 + 시작 프롬프트 재주입, `--resume` 없음)을 본다. 아래는 실제 역할 세션으로 확인할 항목이다. 결과를 이 절에 적을 것. 스케줄러는 1분마다 돌므로 시각은 1–2분 여유를 둔다.

1. **시각을 가깝게**: `config.json`에 `"nightlyRefresh"`를 지금으로부터 2–3분 뒤("HH:MM", 24시간)로, `"nightlyTimeoutMin": 5`로 두고 앱을 켠다(`%APPDATA%\agentboard\nightly.json`이 있으면 지워 둔다). 시각이 되면 dispatcher·각 Manager 세션에 `[AgentBoard] 야간 정리: … handover(@<handover 세션 이름>)에게 파일 위치를 보고하고 handoff_done을 호출하라.` 메시지가 새 턴으로 들어오는지, handover에는 아직 오지 않는지, 팀 리드·팀원 창에는 아무것도 오지 않는지 확인. `nightly.json`에 오늘 날짜(`lastRun`)가 바로 기록되는지.
2. **handover 순서**: Manager·dispatcher가 모두 `handoff_done`을 부른 뒤 다음 1분 안에 handover에 `인덱스 문서(HANDOVER.md)의 포인터만 갱신하고 handoff_done…` 메시지가 오는지. 한 Manager가 끝내 부르지 않으면 제한 시간의 절반(위 설정이면 2분 30초)이 지난 뒤 handover에 오는지.
3. **새 세션으로 재시작**: 모두 완료하면 곧(1분 이내), 아니면 제한 시간이 지나면 dispatcher·handover·Manager 창이 새로 시작되고 시작 프롬프트(handover 인덱스를 읽고 … 진행 준비)가 다시 들어가는지. 이전 대화가 이어지지 않는지(`/resume` 목록에 새 세션으로 보임). 팀 창(리드·팀원)은 그대로 하던 일을 계속하는지. 이미 꺼져 있던(○) 상시 창은 재시작되지 않는지.
4. **미완료 항목**: 제한 시간 안에 `handoff_done`을 부르지 않은 세션마다 인박스에 "야간 정리 미완료: <세션 이름>" 항목(담당 = 그 탭, 확인할 것에 사유)이 생기는지.
5. **입력 중 연기**: 시각 직전부터 dispatcher 입력란에 글자를 쳐 둔다. `nightlyQuietMin`(기본 10분) 동안은 시작하지 않고 5분 간격으로 다시 확인하는지(`nightly.json`이 갱신되지 않음). 재시작 직전에 어떤 대상 창에 입력하면 그 입력을 버리지 않도록 재시작이 조용해질 때까지 미뤄지는지.
6. **하루 1회 / 앱 재시작**: 정리가 끝난 뒤 같은 날 앱을 껐다 켜도 다시 돌지 않는지. 시각이 지난 뒤 앱을 처음 켜면(그날 기록 없음) 1분쯤 뒤 한 번 도는지. `"nightlyRefresh": null`이면 돌지 않는지.
7. **handoff_done 응답**: 야간 정리가 아닐 때 Manager에게 `handoff_done`을 부르게 하면 "지금은 야간 정리 중이 아닙니다…"만 돌아오고 아무 일도 없는지. 팀 리드가 부르면 권한 오류(한국어)인지.
