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
적용한 대안: (미기록)

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
