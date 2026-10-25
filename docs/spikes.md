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
