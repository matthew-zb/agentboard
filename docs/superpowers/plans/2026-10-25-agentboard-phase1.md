# AgentBoard 1단계(멀티플렉서) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 탭 그룹 → 탭 → 분할 창 구조로 여러 CLI 에이전트(Claude Code 등)를 PTY로 구동하고, JSON 설정·템플릿·에이전트 프로필로 탭을 만들며, 모든 조작을 키보드로 할 수 있는 Windows용 Tauri 앱을 만든다.

**Architecture:** Rust가 그룹/탭/레이아웃 모델의 단일 진실 원천이고, 모든 변경은 하나의 `WsCommand` 열거형을 거친다(키보드·마우스·2단계 MCP 공통 경로). 모델 변경은 순수 함수로 `Effects`(실행/종료할 창 목록)를 돌려주고, `launcher`가 그 효과를 PtyManager로 실행한다. 프론트엔드는 vanilla JS ES 모듈로 스냅샷을 렌더링하고 키 입력을 명령으로 바꾼다.

**Tech Stack:** Tauri 2.x, Rust(portable-pty 0.9, regex, serde_json, uuid, which, win32job, webview2-com), vanilla JS ES modules, @xterm/xterm 6.0.0 + addon-fit 0.11.0 / addon-unicode11 0.9.0 / addon-search 0.16.0 / addon-webgl 0.19.0, Node 20+ `node:test`.

**Spec:** `docs/superpowers/specs/2026-10-24-agentboard-design.md` — 각 작업은 스펙 절(§)을 근거로 한다. 구현자는 스펙과 이 계획을 함께 읽는다.

## Global Constraints

- 대상 OS: Windows 11. macOS 코드는 쓰지 않되 `#[cfg(windows)]`로 Windows 전용 코드를 격리한다.
- 프론트엔드: 번들러 없음, `<script type="module">`, 서드파티 JS는 `src/vendor/`에 고정 버전으로 커밋(CDN 사용 금지).
- 사용자에게 보이는 문구는 한국어.
- 경로: `settings.json`·`workspace.json` = `%APPDATA%/agentboard/`; `config.json`·`templates/`·`agents/`·`keybindings.json` = `<workspaceRoot>/.agentboard/`. 기본 `workspaceRoot` = `C:/workspace`.
- Dispatcher 탭 이름 `dispatcher` 예약, 작업공간 = `workspaceRoot`, 닫기 불가.
- 수치: PTY 출력 배치 16ms, 창별 링버퍼 1MB(1,048,576 bytes), xterm scrollback 10,000줄, `ready` 타임아웃 10초, 저장 디바운스 500ms, 포커스 하이라이트 300ms, 번호 오버레이 무입력 자동 취소 2초, 가장자리 드롭 영역 25%.
- 키 판별은 `KeyboardEvent.code` 기준. 앱이 처리한 키는 PTY로 보내지 않는다. 프로필 `passthroughKeys`에 있는 키는 그 창에서 앱이 처리하지 않는다.
- 어떤 이벤트도 포커스·탭을 자동으로 옮기지 않는다(스펙 §6.4). 사용자 동작(키·마우스)만 포커스를 바꾼다.
- 설정 오류 표시 형식: `파일:줄:열 — 원인`.
- 스펙 대비 의도적 차이 2가지:
  - **렌더러 대체:** xterm 6에는 canvas 애드온이 없으므로 WebGL 실패 시 DOM 렌더러로 대체한다.
  - **의미 검증 오류의 위치 표기:** 의미 검증 오류(중복 이름, 절대경로 등)는 줄·열 대신 JSON 경로를 쓴다 — `파일 — layout.children[1].pane.cwd: 원인`.
  - **오류 개수 표시 통합:** 키바인딩 오류도 설정 오류의 하나로 세어, 상태 표시줄에는 `[설정 오류 N]` 하나만 표시한다(스펙의 `[키 오류 N]`을 대체).

## Review Focus

1. **한글 IME가 켜진 상태의 단축키** — `e.key`가 `ㅗ`이고 `isComposing=true`여도 `Alt+h`가 `pane.focusLeft`로 동작해야 한다. → Task 12 테스트 `matches Alt+h while Korean IME composing`.
2. **실행 파일이 없거나 즉시 종료되는 창** — 앱이 죽거나 창이 빈 화면으로 멈추면 안 되고, 창에 오류와 "Enter: 재시작"이 보여야 한다. → Task 2 테스트 `spawn_missing_command_errors`, Task 9 테스트 `failed_spawn_sets_failed_status`.
3. **대량 출력 폭주**(`type` 큰 파일, 빌드 로그) — 메모리가 무한히 늘면 안 되고 링버퍼는 마지막 1MB만 유지해야 한다. → Task 2 테스트 `ring_keeps_last_capacity_bytes`.
4. **한글·공백이 들어간 작업공간 경로**(`C:/workspace/내 프로젝트`) — cwd 해석과 프로세스 실행이 정상이어야 한다. → Task 6 테스트 `resolve_cwd_handles_korean_and_spaces`, Task 2 테스트 `spawn_in_korean_dir`.
5. **템플릿 `cwd`가 `..`로 탭 작업공간을 벗어나는 경우** — 검증 오류로 거부해야 한다. → Task 5 테스트 `rejects_cwd_escaping_workspace`.

---

## File Structure

```
agentboard/
  package.json                     # devDeps: @tauri-apps/cli, @xterm/*; scripts: vendor, test:js
  scripts/vendor.mjs               # node_modules/@xterm/* → src/vendor/ 복사
  .github/workflows/build.yml      # Windows 빌드 (mdviewer 워크플로 축소판)
  src/
    index.html  style.css
    app.js         # 부트스트랩: 스냅샷 수신 → 렌더 → 포커스 적용
    api.js         # invoke/listen 래퍼 (send(cmd), attach, write, resize …)
    keys.js        # 키 문자열 정규화·매칭(순수) + 키 라우터(DOM)
    geometry.js    # 방향 이웃 찾기(순수)
    focus.js       # 주의 창 순환·복귀(순수), DOM 포커스 적용, 하이라이트
    fuzzy.js       # 퍼지 필터(순수)
    groups.js      # 그룹 레이아웃 렌더링
    layout.js      # LayoutNode → 분할 DOM, 경계 드래그
    tabs.js        # 탭 바 렌더링, 드래그앤드롭
    pane.js        # xterm 생성·재사용, 헤더, 데이터 연결, 동기화 입력
    copymode.js    # 복사 모드 상태 기계
    overlays.js    # 번호 선택, 팔레트, 새 탭 마법사, 확인 대화, 드롭 미리보기
    vendor/        # xterm 및 addon
  src-tauri/
    Cargo.toml  tauri.conf.json  build.rs  capabilities/default.json  icons/
    defaults/templates/{dispatcher,solo,team-3}.json
    defaults/agents/{claude,cmd}.json      # powershell.json은 첫 실행 시 생성
    defaults/keybindings.json
    src/
      main.rs  lib.rs        # Tauri 설정, 명령 등록, AppState
      ring.rs                # RingBuffer
      pty.rs                 # PtyManager
      winjob.rs              # Job Object 래퍼
      layout.rs              # LayoutNode<T>
      config.rs              # settings/config/template/keybindings 로드·검증
      profiles.rs            # AgentProfile, ReadyDetector, encode_submit
      workspace.rs           # Workspace 모델 + WsCommand + Effects
      launcher.rs            # Effects 실행, 프롬프트 주입, 종료 처리
      persist.rs             # workspace.json 저장·복원
      webview.rs             # WebView2 설정 (가속키 끄기)
    tests/pty.rs             # PTY 통합 테스트
  tests/js/*.test.mjs        # node:test
```

---

### Task 1: 프로젝트 골격

**Files:**
- Create: `package.json`, `scripts/vendor.mjs`, `src/index.html`, `src/style.css`, `src/app.js`, `src-tauri/{Cargo.toml,tauri.conf.json,build.rs,capabilities/default.json,src/main.rs,src/lib.rs}`, `.github/workflows/build.yml`, `.gitignore`, `tests/js/smoke.test.mjs`
- Copy: `C:/workspace/mdviewer/src-tauri/icons/*` → `src-tauri/icons/` (임시 아이콘)

**Interfaces:**
- Produces: `app_lib` 크레이트(`pub mod` 각 모듈), `npm run vendor`, `npm run test:js`(= `node --test tests/js/`), Rust 테스트 명령 `cargo test --manifest-path src-tauri/Cargo.toml`.

- [ ] **Step 1: 툴체인 확인.** `cargo --version`이 실패하면 rustup(stable, `x86_64-pc-windows-msvc`)과 VS Build Tools(C++ 워크로드)를 설치한다. 이 PC에는 현재 Rust가 없다. 사람의 설치 승인이 필요하면 멈추고 요청한다. 통과 기준: `cargo --version`, `node --version`(≥20) 출력.
- [ ] **Step 2: 설정 파일 작성.**
  - `tauri.conf.json`: mdviewer 것을 기준으로 다음만 남긴다 — `productName "AgentBoard"`, `identifier "com.agentboard.app"`, `version "0.1.0"`, `withGlobalTauri: true`, 창 1400×900·최소 900×600·`dragDropEnabled: false`. 제거: fileAssociations, assetProtocol, CDN CSP. CSP는 `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src ipc: http://ipc.localhost`로 한다.
  - `Cargo.toml` 의존성: `tauri = "2"`, `tauri-plugin-clipboard-manager = "2"`, `serde`(derive), `serde_json`, `portable-pty = "0.9"`, `regex = "1"`, `uuid`(v4), `which = "8"`, `dirs = "6"`. Windows 전용 의존성: `win32job = "2"`, `webview2-com`(버전은 `cargo tree -i webview2-com`으로 tauri/wry가 쓰는 것과 일치시킨다), `windows`(같은 방식). dev 의존성: `tempfile = "3"`.
- [ ] **Step 3: xterm 벤더링 스크립트.** `package.json`의 devDependencies는 스펙 버전으로 고정한다 — `@xterm/xterm@6.0.0`, `addon-fit@0.11.0`, `addon-unicode11@0.9.0`, `addon-search@0.16.0`, `addon-webgl@0.19.0`. `scripts/vendor.mjs`는 각 패키지의 ESM 빌드(`lib/*.mjs`)와 `xterm.css`를 `src/vendor/`로 복사한다. `npm install && npm run vendor` 실행 후 `src/vendor/`를 커밋한다.
- [ ] **Step 4: 스모크 테스트.** `tests/js/smoke.test.mjs`는 `src/vendor/xterm.mjs`가 존재하는지 assert한다. `lib.rs`에는 `#[test] fn it_builds() {}`를 둔다.
- [ ] **Step 5: 실행 확인.** `npm run test:js`와 `cargo test --manifest-path src-tauri/Cargo.toml`이 PASS하고, `npx tauri dev`로 빈 창("AgentBoard")이 뜨면 통과다.
- [ ] **Step 6: CI 워크플로.** `.github/workflows/build.yml`은 mdviewer 워크플로에서 Windows 잡만 남긴다. 트리거 태그는 `agentboard-v*`이고, 산출물은 nsis·msi다.
- [ ] **Step 7: Commit** — `chore: scaffold AgentBoard Tauri app`

---

### Task 2: 링버퍼 · PtyManager · Job Object

**Files:**
- Create: `src-tauri/src/ring.rs`, `src-tauri/src/pty.rs`, `src-tauri/src/winjob.rs`, `src-tauri/tests/pty.rs`

**Interfaces:**
- Produces:
  - `pub struct RingBuffer { … }` — `new(cap: usize)`, `push(&mut self, bytes: &[u8])`, `snapshot(&self) -> Vec<u8>`, `len(&self) -> usize`
  - `pub struct SpawnSpec { pub cmd: PathBuf, pub args: Vec<String>, pub env: Vec<(String,String)>, pub cwd: PathBuf, pub cols: u16, pub rows: u16 }`
  - `pub trait PtySink: Send + Sync { fn output(&self, id: &str, bytes: &[u8]); fn exited(&self, id: &str, code: i32); }`
  - `pub struct PtyManager` — `new(sink: Arc<dyn PtySink>) -> Self`, `spawn(&self, id: &str, spec: SpawnSpec) -> Result<(), String>`, `write(&self, id: &str, bytes: &[u8]) -> Result<(), String>`, `resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String>`, `kill(&self, id: &str)`, `snapshot(&self, id: &str) -> Vec<u8>`, `is_running(&self, id: &str) -> bool`
  - `pub const RING_CAP: usize = 1_048_576; pub const BATCH_MS: u64 = 16;`
  - `winjob::JobGuard::new() -> Result<JobGuard, String>`, `assign(&self, pid: u32) -> Result<(), String>` (KILL_ON_JOB_CLOSE)

- [ ] **Step 1: 실패하는 테스트 작성** (`ring.rs` 내부 테스트)
```rust
#[test] fn ring_keeps_last_capacity_bytes() {
    let mut r = RingBuffer::new(1_048_576);
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    r.push(&data);
    assert_eq!(r.len(), 1_048_576);
    assert_eq!(r.snapshot(), data[data.len()-1_048_576..].to_vec());
}
#[test] fn ring_small_pushes_concatenate() {
    let mut r = RingBuffer::new(8);
    r.push(b"abc"); r.push(b"defgh"); r.push(b"ij");
    assert_eq!(r.snapshot(), b"cdefghij".to_vec());
}
```
- [ ] **Step 2: 실행해서 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml ring::` → 컴파일 에러(FAIL)
- [ ] **Step 3: `RingBuffer` 구현** — `VecDeque<u8>`나 원형 `Vec` 중 무엇을 쓸지는 자유.
- [ ] **Step 4: 통합 테스트 작성** (`tests/pty.rs`)
  - 테스트용 `CollectSink`는 `Mutex<HashMap<String, Vec<u8>>>`와 종료 코드를 모은다.
  - 출력 대기는 최대 5초 폴링 헬퍼로 한다.
```rust
#[test] fn spawn_echo_streams_output_and_exit_code() {
    // cmd.exe /c echo hello-agentboard  → output에 "hello-agentboard", exited code 0
}
#[test] fn spawn_missing_command_errors() {
    // cmd = "definitely-not-a-real-cmd-xyz.exe" → spawn이 Err, sink에 exited 호출 없음, 패닉 없음
}
#[test] fn exit_code_is_reported() {
    // cmd.exe /c exit 3 → exited code 3
}
#[test] fn write_reaches_process() {
    // cmd.exe (대화형) spawn → write(b"echo typed-xyz\r\n") → output에 "typed-xyz"
}
#[test] fn spawn_in_korean_dir() {
    // tempdir/"내 프로젝트" 생성, cwd로 cmd.exe /c cd → output에 "내 프로젝트"
}
#[test] fn kill_terminates_and_snapshot_survives() {
    // ping -n 30 127.0.0.1 spawn → 약간 출력 후 kill → 5초 내 is_running=false, snapshot 비어있지 않음
}
```
- [ ] **Step 5: 실행해서 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml --test pty` → FAIL
- [ ] **Step 6: `PtyManager` 구현**
  - `portable_pty::native_pty_system()`을 쓰고, 창마다 reader 스레드와 flush 스레드를 둔다.
  - reader는 받은 바이트를 링버퍼와 대기 버퍼에 모두 넣는다.
  - flush 스레드는 `BATCH_MS`마다 대기 버퍼가 비어 있지 않으면 `sink.output`을 호출한다.
  - 프로세스 종료 시 남은 대기 버퍼를 먼저 flush한 뒤 `sink.exited`를 호출한다.
  - spawn 직후 child pid를 `JobGuard`에 assign한다. `JobGuard`는 PtyManager가 하나를 소유한다.
- [ ] **Step 7: 실행해서 통과 확인** — `cargo test --manifest-path src-tauri/Cargo.toml` → PASS
- [ ] **Step 8: Commit** — `feat(pty): ring buffer, PtyManager with batching and job object`

---

### Task 3: 단일 창 연결 + WebView 설정 + 스파이크 S1–S3

**Files:**
- Create: `src-tauri/src/webview.rs`, `src/api.js`, `src/pane.js`(최소판)
- Modify: `src-tauri/src/lib.rs`, `src/app.js`, `src/index.html`

**Interfaces:**
- Consumes: Task 2 `PtyManager`, `PtySink`
- Produces:
  - Tauri 명령:
    - `pty_attach(pane: String, channel: Channel<Vec<u8>>) -> Vec<u8>` — 지금까지의 링버퍼를 돌려주고, 그 뒤 출력은 channel로 보낸다. 스냅샷과 구독 등록은 같은 락 안에서 처리한다.
    - `pty_write(pane: String, data: String)`
    - `pty_resize(pane: String, cols: u16, rows: u16)`
  - `AppSink`(PtySink 구현): `Mutex<HashMap<String, Channel<Vec<u8>>>>`로 attach된 채널에 전달한다.
  - JS `api.js`:
    - `attach(paneId, onBytes) -> Promise<Uint8Array>`
    - `write(paneId, str)`
    - `resize(paneId, cols, rows)`
    - `send(cmd) -> Promise<Snapshot>` (Task 9에서 연결)
    - `on(event, fn)`
  - JS `pane.js`:
    - `createTerminal(paneId) -> { term, el, fit() }`
    - 사용 애드온: unicode11(`term.unicode.activeVersion='11'`), fit, search, webgl. WebGL 실패 시 DOM 렌더러로 대체한다.
    - xterm 옵션: `scrollback: 10000`
  - `webview::disable_browser_accelerators(window)` — `with_webview` → `ICoreWebView2Settings3::SetAreBrowserAcceleratorKeysEnabled(false)`, `SetAreDefaultContextMenusEnabled(false)`.

- [ ] **Step 1: 임시 배선.** 앱 시작 시 하드코딩한 `"p-test"` 창에서 `powershell.exe`를 실행한다. 프론트는 attach해서 한 화면에 표시하고, 리사이즈 시 `fit()` 후 `pty_resize`를 호출한다. 이 하드코딩은 Task 9에서 제거한다.
- [ ] **Step 2: 실행 확인** — `npx tauri dev`. PowerShell 프롬프트에서 입력이 되고, `dir`과 한글(`echo 한글테스트`)이 올바른 폭으로 표시되면 통과다.
- [ ] **Step 3: 스파이크 S3.** 같은 창에서 `claude`를 실행해 다음을 확인한다. 결과는 `docs/spikes.md`에 기록한다.
  - TUI 렌더링: 색, 커서, 상자 문자
  - 창 리사이즈 반영
  - 한글 입력(IME 조합)
  - `Ctrl+V` 이미지 붙여넣기 칩
- [ ] **Step 4: 스파이크 S2.** `webview.rs`를 적용한 상태에서 Claude 창에 다음 키를 눌러 확인한다. 실패 시 대안은 스펙 §10 S2이고, 결과는 기록한다.
  - `Ctrl+R`: 페이지 새로고침이 아니라 Claude 기록 검색이 열려야 한다.
  - `Ctrl+P`: 인쇄 대화가 뜨지 않아야 한다.
  - `F5`: 새로고침되지 않아야 한다.
- [ ] **Step 5: 스파이크 S1.** 다음을 확인하고 결과와 적용한 대안을 기록한다.
  - `Alt`만 눌렀다 뗐을 때 xterm 포커스가 유지되는지, 경고음이 나지 않는지
  - 문서 keydown capture에서 `Alt+h`를 `preventDefault`했을 때 경고음이 나지 않는지
  - 문제가 있으면 keyup Alt `preventDefault`를 적용하고, 그래도 안 되면 Rust 서브클래싱으로 `WM_SYSCHAR`를 처리한다.
- [ ] **Step 6: Commit** — `feat: single-pane PTY bridge, WebView2 accelerator off, spikes S1-S3`

---

### Task 4: LayoutNode

**Files:**
- Create: `src-tauri/src/layout.rs`

**Interfaces:**
- Produces:
  - 기본 타입:
    - `pub enum Dir { Horizontal, Vertical }` — serde `"horizontal"|"vertical"`. Horizontal은 좌|우.
    - `pub enum Side { Left, Right, Up, Down }`
  - `pub enum LayoutNode<T> { Split { dir: Dir, sizes: Vec<f32>, children: Vec<LayoutNode<T>>, slot: Option<String> }, Leaf(T) }`
    - serde 직렬화 형식은 `{"split":…,"sizes":…,"children":…,"slot"?:…}` 또는 `{"leaf":…}`이다(workspace.json용).
  - 메서드 (`T: Clone + PartialEq`):
    - `leaves(&self) -> Vec<T>` — 왼쪽→오른쪽, 위→아래 순
    - `contains(&self, t: &T) -> bool`
    - `map<U>(&self, f: impl FnMut(&T) -> U) -> LayoutNode<U>`
    - `split_leaf(&mut self, target: &T, new: T, dir: Dir, before: bool) -> bool` — 같은 방향의 부모 split이면 형제로 삽입하고 크기를 균등하게 재분배한다. 아니면 leaf를 2분할 split으로 바꾼다.
    - `remove_leaf(&mut self, target: &T) -> Option<Option<T>>` — None=없음; Some(Some(s))=제거 후 포커스 후보(같은 부모의 인접 형제 subtree의 첫 leaf); Some(None)=트리가 비게 됨(루트 leaf였음). 제거 후 `normalize`.
    - `normalize(&mut self)` — 자식 1개 split은 그 자식으로 대체, sizes 길이를 children에 맞추고 합 100으로 정규화(누락·0·음수는 균등 분배)
    - `resize(&mut self, target: &T, side: Side, delta: f32) -> bool` — target에서 위로 올라가며 축이 맞고(Left/Right↔Horizontal) 그 방향에 형제가 있는 가장 가까운 split을 찾아, target 쪽 child 크기를 +delta·인접 child를 −delta(각 최소 5%로 클램프). 그런 split이 없으면 false.
    - `set_sizes(&mut self, path: &[usize], sizes: Vec<f32>) -> bool`
    - `leaf_count(&self) -> usize`

- [ ] **Step 1: 실패하는 테스트 작성** (주요 assert만 적음)
```rust
#[test] fn split_leaf_into_new_split() { /* Leaf(a) split a→b Horizontal → Split{H,[50,50],[a,b]} */ }
#[test] fn split_same_dir_inserts_sibling() { /* Split{H,[a,b]} split b→c H → children [a,b,c], sizes ≈[33.3;3] */ }
#[test] fn split_before_places_new_first() { /* before=true → [b,a] */ }
#[test] fn remove_leaf_returns_adjacent_sibling() { /* [a,[b,c]V]H remove a → focus b; tree = Split{V,[b,c]} */ }
#[test] fn remove_last_leaf_reports_empty() { /* Leaf(a) remove a → Some(None) */ }
#[test] fn normalize_fixes_bad_sizes() { /* sizes [0,-5,300] → 합 100, 모두 >0 */ }
#[test] fn resize_moves_nearest_matching_border() { /* [a,[b,c]V]H: resize(b,Left,10) → H sizes [40,60]; resize(b,Up,10) → V false-direction→ b는 위에 형제 없음 → false; resize(c,Up,10) → V sizes [40,60] */ }
#[test] fn resize_clamps_to_min_5() {}
#[test] fn leaves_order_is_reading_order() {}
#[test] fn serde_roundtrip() {}
```
- [ ] **Step 2: 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml layout::` → FAIL
- [ ] **Step 3: 구현** — 모든 메서드는 재귀로 구현한다. `resize`는 "target이 들어 있는 child index를 반환하는 재귀"로 가장 깊은 일치 split부터 시도한다.
- [ ] **Step 4: 통과 확인** — 같은 명령 → PASS
- [ ] **Step 5: Commit** — `feat(layout): generic LayoutNode tree operations`

---

### Task 5: 설정·템플릿·키바인딩 로드와 검증

**Files:**
- Create: `src-tauri/src/config.rs`, `src-tauri/defaults/templates/{dispatcher,solo,team-3}.json`, `src-tauri/defaults/keybindings.json`

**Interfaces:**
- Consumes: Task 4 `LayoutNode`, `Dir`
- Produces:
  - `pub struct Settings { pub workspace_root: PathBuf }` — `load_settings(appdata_dir: &Path) -> Settings`. 파일이 없으면 기본값 `C:/workspace`로 생성한다.
  - `pub struct PaneSpec { pub name: String, pub agent: Option<String>, pub args: Vec<String>, pub env: BTreeMap<String,String>, pub cwd: Option<String>, pub prompt: Option<String>, pub focus: bool }`
  - `pub struct Template { pub name: String, pub layout: LayoutNode<PaneSpec> }` — JSON 노드 `{"pane":{…}}` 또는 `{"split","sizes"?,"children","slot"?}`를 이 구조로 변환한다.
  - `pub struct TabSpec { pub name: String, pub dir: String, pub template: Option<String> }`
  - `pub struct GroupSpec { pub name: String, pub tabs: Vec<String> }`
  - `pub struct Config { pub dispatcher: DispatcherSpec { agent: Option<String>, template: String }, pub default_template: String, pub default_agent: String, pub groups: Option<LayoutNode<GroupSpec>>, pub default_group: Option<String>, pub tabs: Vec<TabSpec> }` — JSON 키는 camelCase
  - `pub struct ConfigError { pub file: PathBuf, pub line: usize, pub col: usize, pub json_path: Option<String>, pub message: String }`
    - `Display` 형식: line>0이면 `"{file}:{line}:{col} — {message}"`, 아니면 `"{file} — {json_path}: {message}"`
  - `pub struct Keybindings(pub BTreeMap<String /*command id*/, String /*key*/>)`
  - `pub struct Loaded { pub settings: Settings, pub config: Config, pub templates: BTreeMap<String, Template>, pub profiles: BTreeMap<String, AgentProfile> /*Task 6*/, pub keybindings: Keybindings, pub errors: Vec<ConfigError> }`
  - `pub fn ensure_dotdir(root: &Path) -> Result<(), String>` — `.agentboard/`와 기본 파일을 없을 때만 복사한다. 기본 파일은 `include_str!`로 내장한다.
  - `pub fn load_all(appdata_dir: &Path) -> Loaded` — profiles 로딩은 Task 6의 `profiles::load_dir`를 호출한다. Task 6 전까지는 빈 맵으로 둔다.
  - `pub fn validate_template(t: &Template, file: &Path, known_agents: &BTreeSet<String>) -> Vec<ConfigError>`
  - `pub fn parse_key(s: &str) -> Result<KeyChord, String>` / `pub struct KeyChord { ctrl, alt, shift: bool, key: String /*정규화: 소문자 글자, 숫자, "\\", "-", ";", "[", "]", "tab", "left" 등*/ }`

- [ ] **Step 1: 기본 파일 작성.**
  - `dispatcher.json`: pane 하나 `{"name":"dispatcher"}`
  - `solo.json`: pane 하나 `{"name":"main","focus":true}`
  - `team-3.json`: 스펙 §4.3 예시 구조로 lead/coder/tests를 둔다. 모든 agent는 `claude`이고 `tests`는 `powershell`이다.
  - `keybindings.json`: 스펙 §6.2 표의 모든 명령 id와 기본 키를 넣는다.
    - 와일드카드 id는 다음과 같이 풀어서 쓴다.
      - `pane.resize*` → `pane.resizeLeft/Down/Up/Right`
      - `tab.move*` → `tab.moveLeft/Right/Down/Up`
      - `tab.goto1..9`, `group.goto1..9`
    - 추가 id: `group.splitRight`, `group.splitDown`, `app.palette`
- [ ] **Step 2: 실패하는 테스트 작성**
```rust
#[test] fn parses_spec_example_template() { /* 스펙 §4.3 JSON(agent codex→claude) → leaves 이름 [lead, coder, tests], lead.focus=true, members slot */ }
#[test] fn syntax_error_has_line_and_col() { /* 3번째 줄 쉼표 누락 → line=3, col>0, Display가 "file:3:" 로 시작 */ }
#[test] fn rejects_absolute_cwd() { /* cwd "C:/x" → json_path "layout.pane.cwd" 오류 */ }
#[test] fn rejects_cwd_escaping_workspace() { /* cwd "../other", "a/../../b" → 오류; "server", "a/../b" → OK */ }
#[test] fn rejects_duplicate_pane_names() {}
#[test] fn rejects_unknown_agent() {}
#[test] fn rejects_split_with_one_child() {}
#[test] fn config_tab_names_unique_and_dispatcher_reserved() { /* tabs에 "dispatcher" 또는 중복 이름 → 오류 */ }
#[test] fn parse_key_normalizes() { /* "Ctrl+Alt+Shift+H" == "ctrl+alt+shift+h"; "Alt+\\" key "\\"; "Alt+Q+W" → Err */ }
#[test] fn keybindings_override_and_conflict() { /* 사용자 {"pane.focusLeft":"Alt+y"} 적용; 두 id가 같은 키 → 오류 1개, 둘 다 기본값 유지; null → 해제 */ }
#[test] fn ensure_dotdir_does_not_overwrite_user_files() {}
#[test] fn broken_config_yields_error_and_dispatcher_only_defaults() { /* config.json 깨짐 → errors 1개, Config는 기본값(tabs 비어있음) */ }
```
- [ ] **Step 3: 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml config::` → FAIL
- [ ] **Step 4: 구현.**
  - 문법·타입 오류는 `serde_json::Error::line()/column()`으로 위치를 잡는다.
  - 의미 검증 오류는 JSON 경로 문자열을 만들어 넣는다.
  - cwd 이탈 검사는 경로 컴포넌트를 순서대로 처리하며 깊이를 센다. `..`에서 깊이가 0 미만이 되면 오류다.
- [ ] **Step 5: 통과 확인** → PASS
- [ ] **Step 6: Commit** — `feat(config): settings/config/template/keybinding loading with located errors`

---

### Task 6: 에이전트 프로필

**Files:**
- Create: `src-tauri/src/profiles.rs`, `src-tauri/defaults/agents/{claude,cmd}.json`
- Modify: `src-tauri/src/config.rs` (`load_all`에서 `profiles::load_dir` 호출, known_agents 전달)

**Interfaces:**
- Produces:
  - `pub enum Level { L0, L1, L2 }`
  - `pub enum Ready { Pattern(Regex), IdleMs(u64) }` — JSON `{"pattern":…}` | `{"idleMs":…}`, 기본 `IdleMs(800)`
  - `pub struct Submit { pub bracketed_paste: bool, pub key: String }` — 기본값 `{false, "\r"}`
  - `pub struct AgentProfile { pub name, pub level: Level, pub cmd: String, pub args: Vec<String>, pub env: BTreeMap<String,String>, pub ready: Ready, pub submit: Submit, pub passthrough_keys: Vec<String>, pub system_prompt: Option<Value>, pub mcp: Option<Value>, pub idle: Option<Value>, pub resume: Option<Value> }` (camelCase)
  - `pub fn load_dir(dir: &Path) -> (BTreeMap<String, AgentProfile>, Vec<ConfigError>)`
  - `pub fn ensure_powershell_profile(dir: &Path)` — `powershell.json`이 없을 때만 생성한다. `which("pwsh")`가 성공하면 `cmd: "pwsh"`, 아니면 `"powershell.exe"`이다. `ready: {"pattern":"PS .*> $"}`, `level: "L0"`.
  - `pub fn resolve_cmd(cmd: &str) -> Option<PathBuf>` — `which` 크레이트 사용(PATHEXT 포함)
  - `pub fn encode_submit(text: &str, s: &Submit) -> Vec<u8>`
  - `pub struct ReadyDetector` — `new(ready: &Ready, start: Instant)`, `feed(&mut self, bytes: &[u8], now: Instant) -> bool`, `poll(&mut self, now: Instant) -> bool`. `Pattern`은 최근 4KB 출력(ANSI 이스케이프 제거 후)의 **마지막 줄**에 매칭한다. `IdleMs`는 첫 출력 이후 n ms 동안 출력이 없으면 ready다. 한 번 true가 되면 계속 true다.
  - `pub fn resolve_cwd(tab_workspace: &Path, cwd: Option<&str>) -> PathBuf`
  - `pub const READY_TIMEOUT: Duration = Duration::from_secs(10);`

- [ ] **Step 1: 기본 프로필 작성.**
  - `claude.json`: 스펙 §4.4 예시를 그대로 쓰고 `"level":"L2"`로 둔다.
  - `cmd.json`: `cmd: "cmd.exe"`, `ready: {"pattern":"^[A-Z]:\\\\.*>$"}`, `L0`.
- [ ] **Step 2: 실패하는 테스트 작성**
```rust
#[test] fn parses_claude_profile_with_reserved_fields() { /* level L2, mcp/systemPrompt Some */ }
#[test] fn encode_submit_plain() { assert_eq!(encode_submit("a\nb", &Submit{bracketed_paste:false,key:"\r".into()}), b"a\rb\r".to_vec()); }
#[test] fn encode_submit_bracketed() { assert_eq!(encode_submit("a\nb", &Submit{bracketed_paste:true,key:"\r".into()}), b"\x1b[200~a\nb\x1b[201~\r".to_vec()); }
#[test] fn ready_pattern_matches_last_line_ignoring_ansi() { /* "\x1b[32mPS C:\\x> \x1b[0m" → true; 중간 줄에만 있으면 false */ }
#[test] fn ready_idle_requires_first_output_then_quiet() { /* 출력 전 poll(t+5s)=false; feed@t → poll(t+799ms)=false, poll(t+800ms)=true */ }
#[test] fn resolve_cwd_handles_korean_and_spaces() { /* ("C:/workspace/내 프로젝트", Some("server")) → ".../내 프로젝트/server"; None·Some("") → 탭 경로 */ }
#[test] fn powershell_profile_created_once() {}
#[test] fn invalid_regex_is_config_error() {}
```
- [ ] **Step 3: 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml profiles::` → FAIL
- [ ] **Step 4: 구현.** ANSI 제거에는 정규식 `\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07`을 쓴다.
- [ ] **Step 5: 통과 확인** → PASS (config 테스트도 함께)
- [ ] **Step 6: Commit** — `feat(profiles): agent profiles, ready detection, submit encoding`

---

### Task 7: Workspace 모델 — 탭과 창

**Files:**
- Create: `src-tauri/src/workspace.rs`

**Interfaces:**
- Consumes: Task 4 `LayoutNode/Dir/Side`, Task 5 `Loaded/Template/PaneSpec/Config`, Task 6 `resolve_cwd`
- Produces (전부 serde, camelCase):
  - `pub type GroupId = String; TabId = String; PaneId = String;` — `g1`/`t1`/`p1` 형태의 증가 카운터이며, 카운터 값도 함께 저장한다.
  - 스펙 §5의 `Workspace`, `Group`, `Tab`, `Pane` 필드를 그대로 쓰되 다음을 추가한다.
    - `Pane.status: PaneStatus { Starting, Running, Exited { code: i32 }, Failed { message: String } }`
    - `Pane.prompt_warning: bool`
    - `Pane.tab: TabId`
    - `Tab.workspace: PathBuf`
    - `Workspace.group_maximized: Option<GroupId>`
  - `pub struct Effects { pub spawn: Vec<(PaneId, bool /*inject_prompt*/)>, pub kill: Vec<PaneId> }`
  - `#[serde(tag="type", rename_all="camelCase")] pub enum WsCommand`:
    - 탭: `CreateTab { name, dir, template: Option<String>, group: Option<GroupId> }`, `CloseTab { tab }`, `RenameTab { tab, name }`
    - 창: `SplitPane { pane, dir: Dir }`, `ClosePane { pane }`, `RestartPane { pane }`, `ResizePane { pane, side: Side, delta: f32 }`, `SetSizes { tab: Option<TabId>, path: Vec<usize>, sizes: Vec<f32> }`, `BreakPaneToTab { pane }`
    - 상태: `SetFocus { group: GroupId, tab: Option<TabId>, pane: Option<PaneId> }`, `SetMaximized { tab, pane: Option<PaneId> }`, `SetSyncInput { tab, on: bool }`, `SetAttention { pane, on: bool }`
    - 그룹(Task 8에서 구현): `MoveTab { tab, group, index: usize }`, `SplitGroupWith { tab, group, dir: Dir, before: bool }`, `MergeGroup { src, dst }`, `CloseGroup { group }`, `SetGroupMaximized { group: Option<GroupId> }`
  - `impl Workspace`:
    - `pub fn from_config(loaded: &Loaded) -> Workspace` — Dispatcher 탭과 config의 탭들을 만들고, `groups` 배치를 적용한다(없으면 그룹 하나). 템플릿이 없거나 잘못됐으면 그 탭은 건너뛴다. 오류 목록은 `loaded.errors`에 이미 있다.
    - `pub fn apply(&mut self, cmd: WsCommand, loaded: &Loaded) -> Result<Effects, String>` — Err는 한국어 메시지이고 모델은 바뀌지 않는다.
    - `pub fn set_status(&mut self, pane: &str, st: PaneStatus)`
    - `pub fn running_count(&self) -> usize`
    - `pub fn pane_by_id(&self, id) -> Option<&Pane>`
  - 동작 규칙:
    - `CreateTab`
      - 이름이 중복되면 Err.
      - 템플릿의 pane마다 `Pane`을 만들고 spawn 목록에 넣는다(`inject_prompt=true`).
      - `lastFocusedPaneId`는 `focus:true`인 창, 없으면 첫 leaf다.
      - 탭이 열리는 그룹은 `group`, 없으면 포커스된 그룹이다.
      - 새 탭이 그 그룹의 활성 탭이 된다.
    - `SplitPane`: 새 창 이름은 `pane-N`(탭 안에서 유일). agent는 원래 창과 같고, cwd는 탭 작업공간, prompt는 없음. 새 창에 포커스를 준다.
    - `ClosePane`
      - 창을 kill하고, `remove_leaf`가 돌려준 형제를 `lastFocused`로 삼는다.
      - 탭이 비면 그 탭을 닫는다. 단 Dispatcher 탭이면 닫지 않고 Err를 돌려준다(마지막 창은 닫을 수 없음).
    - `CloseTab`: Dispatcher면 Err. 탭의 모든 창을 kill한다. 그룹의 활성 탭은 오른쪽 탭, 없으면 왼쪽 탭으로 넘긴다. 그룹이 비면 그룹을 제거한다(Task 8 헬퍼 `remove_group_if_empty`).
    - `RestartPane`: kill 후 spawn(`inject_prompt=false`), 상태는 Starting.
    - `ResizePane`: 탭 레이아웃 `resize`가 false면 그 탭이 속한 그룹을 그룹 레이아웃에서 `resize`한다.
    - `BreakPaneToTab`: 창을 레이아웃에서 떼어 같은 그룹의 새 탭(이름은 `<탭>-<창>`, 중복 시 `-2` 등)으로 옮긴다. 프로세스는 유지한다(spawn/kill 없음). 원래 탭이 비면 닫는다.
    - `SetFocus`: group·tab·pane을 검증한다. 탭이나 그룹이 바뀌면 이전 위치를 `lastLocation`에 기록한다. 포커스를 받은 창은 `attention=false`가 된다.

- [ ] **Step 1: 테스트 헬퍼** `fn loaded_fixture() -> Loaded` — tempdir에 `solo`·`team-3` 템플릿, `claude`·`powershell` 프로필, `config.tabs=[api(team-3)]`를 둔 상태를 만든다.
- [ ] **Step 2: 실패하는 테스트 작성**
```rust
#[test] fn from_config_creates_dispatcher_and_tabs() { /* tabs: dispatcher(workspace=root), api(workspace=root/api, panes lead/coder/tests, lastFocused=lead) */ }
#[test] fn create_tab_spawns_with_prompt_and_activates() { /* Effects.spawn 3개 모두 inject_prompt=true, 그룹 activeTab=새 탭 */ }
#[test] fn create_tab_duplicate_name_errors_without_change() {}
#[test] fn split_pane_adds_and_focuses() {}
#[test] fn close_pane_focuses_sibling_and_kills() {}
#[test] fn closing_last_pane_closes_tab() {}
#[test] fn dispatcher_cannot_be_closed() { /* CloseTab, 마지막 ClosePane 모두 Err, 모델 불변 */ }
#[test] fn restart_respawns_without_prompt() {}
#[test] fn resize_falls_back_to_group_border() {}
#[test] fn break_pane_to_tab_keeps_process() { /* Effects 비어있음, 새 탭에 창, 원 탭에서 제거 */ }
#[test] fn set_focus_records_last_location_and_clears_attention() {}
#[test] fn ws_command_json_shape() { /* r#"{"type":"splitPane","pane":"p1","dir":"vertical"}"# 역직렬화 */ }
```
- [ ] **Step 3: 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml workspace::` → FAIL
- [ ] **Step 4: 구현.** `apply`는 `self`를 clone한 사본에 적용하고, 성공하면 교체한다. 그래야 Err일 때 모델이 바뀌지 않는다.
- [ ] **Step 5: 통과 확인** → PASS
- [ ] **Step 6: Commit** — `feat(workspace): tab/pane model and commands`

---

### Task 8: Workspace 모델 — 그룹

**Files:**
- Modify: `src-tauri/src/workspace.rs`

**Interfaces:**
- Consumes: Task 7 `WsCommand` 그룹 변형
- Produces 규칙:
  - `MoveTab { tab, group, index }`
    - 원래 그룹에서 빼서 대상 그룹의 `index`(범위 밖이면 끝)에 넣고, 대상 그룹의 활성 탭으로 만든다.
    - 포커스는 `(group, tab, tab.lastFocused)`로 옮긴다.
    - 원래 그룹이 비면 제거한다. 원래 그룹의 활성 탭은 CloseTab과 같은 규칙으로 정한다.
    - 같은 그룹 안에서는 순서만 바뀐다.
  - `SplitGroupWith { tab, group, dir, before }`: 새 그룹(이름 `group-N`)을 만들어 `group` leaf 옆에 `split_leaf`로 넣고 탭을 옮긴다. 원래 그룹의 탭이 이것 하나뿐이면 Err("탭이 하나뿐인 그룹은 분할할 수 없습니다")다.
  - `MergeGroup { src, dst }`: src 탭들을 순서대로 dst 끝에 붙인다. 이름과 활성 탭은 dst를 유지한다. 포커스는 원래 포커스 탭을 유지한다. src 그룹은 제거하고 `remove_leaf`·`normalize`를 적용한다. `src==dst`면 Err다.
  - `CloseGroup { group }`: 그룹이 하나뿐이면 Err("그룹이 하나뿐입니다"). 아니면 `MergeGroup { src: group, dst: <remove_leaf가 돌려줄 형제> }`와 같다. 형제는 제거 전에 미리 계산한다.
  - `SetGroupMaximized`: 저장만 한다.
  - `fn remove_group_if_empty(&mut self, g)`: 그룹 레이아웃에서 제거한다. 포커스 그룹이었으면 형제 그룹으로 옮긴다. Dispatcher 탭은 어디에든 있을 수 있다.

- [ ] **Step 1: 실패하는 테스트 작성**
```rust
#[test] fn move_tab_between_groups_and_remove_empty() {}
#[test] fn move_tab_reorders_within_group() {}
#[test] fn move_tab_index_out_of_range_appends() {}
#[test] fn split_group_with_creates_group_before_and_after() { /* before=true → leaves 순서 [new, old] */ }
#[test] fn split_group_with_single_tab_errors() {}
#[test] fn merge_group_appends_in_order_keeps_dst_name() {}
#[test] fn close_group_merges_into_sibling() {}
#[test] fn close_only_group_errors() {}
#[test] fn dispatcher_tab_can_move_groups() {}
#[test] fn merge_never_kills_processes() { /* 모든 그룹 명령의 Effects가 비어있음 */ }
```
- [ ] **Step 2: 실패 확인** → FAIL
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인** — `cargo test --manifest-path src-tauri/Cargo.toml workspace::` → PASS
- [ ] **Step 5: Commit** — `feat(workspace): tab groups — move, split, merge, close`

---

### Task 9: Launcher와 Tauri 명령 계층

**Files:**
- Create: `src-tauri/src/launcher.rs`
- Modify: `src-tauri/src/lib.rs` (AppState, 명령 등록, Task 3 하드코딩 제거), `src/api.js`

**Interfaces:**
- Consumes: Task 2, 5, 6, 7
- Produces:
  - `pub struct AppState { pub ws: Mutex<Workspace>, pub loaded: RwLock<Loaded>, pub pty: PtyManager, pub app: AppHandle, pub prompts: Mutex<HashMap<PaneId, PromptJob>> }`
  - `launcher::run_effects(state: &AppState, fx: Effects)`
    - kill을 먼저 처리한다.
    - spawn할 창마다 프로필을 찾는다(없으면 `config.defaultAgent`). `resolve_cmd`가 실패하면 상태를 `Failed{"명령을 찾을 수 없습니다: <cmd> (프로필 <name>)"}`로 둔다.
    - 실행 인자는 profile.args 뒤에 pane.args를 붙인다. 환경 변수는 profile.env 위에 pane.env를 덮는다.
    - cols/rows는 80×24로 시작하고, 프론트엔드 fit 이후의 resize로 맞춘다.
  - 프롬프트 주입(`inject_prompt` && prompt가 있을 때):
    - `PtySink::output`에서 `ReadyDetector`에 출력을 넣는다. 200ms 주기 타이머 스레드가 `poll`하다가 ready가 되면 `encode_submit`을 write한다.
    - `READY_TIMEOUT` 안에 ready가 안 되면 그대로 주입하고 `prompt_warning=true`로 둔다.
  - `PtySink::exited` → `Exited{code}`. 성공적으로 spawn되면 → `Running`.
  - 모든 상태 변경 뒤 `workspace-changed` 이벤트를 보낸다(페이로드는 `Snapshot`).
  - Tauri 명령:
    - `ws_command(cmd: WsCommand) -> Result<Snapshot, String>`
    - `ws_snapshot() -> Snapshot`
    - `config_errors() -> Vec<String>`
    - `get_keybindings() -> BTreeMap<String,String>`
    - `get_passthrough(pane: String) -> Vec<String>`
    - `list_workspace_dirs() -> Vec<String>` — `.`으로 시작하는 디렉토리 제외, 이름순
    - `create_workspace_dir(name: String) -> Result<(), String>` — 이름에 `/ \ :`가 있거나 `..`이거나 비어 있으면 Err
    - `list_templates() -> Vec<String>`
    - `missing_dirs() -> Vec<(TabId, String /*dir*/)>` — 작업 폴더가 없는 탭 목록
  - `pub struct Snapshot { pub workspace: Workspace, pub errors: usize }`
  - JS `api.js`: `send(cmd)`, `snapshot()`, `onWorkspaceChanged(fn)`, `configErrors()`, `keybindings()`, `passthrough(pane)`, `listDirs()`, `createDir(name)`, `listTemplates()`

- [ ] **Step 1: 실패하는 테스트 작성** (`launcher.rs`, Tauri 없이 테스트할 수 있도록 `run_effects`의 핵심을 `fn plan_spawn(pane:&Pane, loaded:&Loaded) -> Result<SpawnSpec, String>`로 분리)
```rust
#[test] fn plan_spawn_merges_args_and_env() {}
#[test] fn plan_spawn_uses_default_agent() {}
#[test] fn failed_spawn_sets_failed_status() { /* 없는 cmd 프로필 → Err 메시지 "명령을 찾을 수 없습니다" 포함 */ }
#[test] fn plan_spawn_errors_on_missing_cwd() { /* cwd 없음 → Err "작업 폴더가 없습니다: <경로>" */ }
#[test] fn prompt_job_injects_after_ready_or_timeout() { /* PromptJob 상태기계를 가짜 시간으로: ready 시 1회 write; 타임아웃 시 write + warning */ }
```
- [ ] **Step 2: 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml launcher::` → FAIL
- [ ] **Step 3: 구현.** 앱을 시작할 때 다음 순서로 진행한다.
  1. `load_settings`
  2. `ensure_dotdir`와 `ensure_powershell_profile`
  3. `load_all`
  4. `Workspace` 준비 — 일단 `from_config`. Task 10에서 복원으로 바꾼다.
  5. 모든 창 spawn
- [ ] **Step 4: 통과 확인** → PASS
- [ ] **Step 5: 수동 확인.** `npx tauri dev`를 실행하고 devtools 콘솔에서 `await window.__TAURI__.core.invoke('ws_snapshot')`를 호출한다. dispatcher 탭과 config 탭이 보이고, 창 상태가 Running이면 통과다.
- [ ] **Step 6: Commit** — `feat: launcher, prompt injection, Tauri command layer`

---

### Task 10: 저장·복원

**Files:**
- Create: `src-tauri/src/persist.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/launcher.rs`

**Interfaces:**
- Produces:
  - `pub fn save_atomic(path: &Path, ws: &Workspace) -> Result<(), String>` — `workspace.json.tmp`에 쓴 뒤 rename
  - `pub fn load(path: &Path) -> Result<Option<Workspace>, String>` — 파일이 없으면 Ok(None). 파싱에 실패하면 `workspace.json.bak`으로 옮기고 Err를 돌려준다.
  - `pub fn prepare_restored(ws: &mut Workspace) -> Effects` — 모든 창의 상태를 Starting으로, attention·prompt_warning을 false로 두고, 모든 창을 `inject_prompt=false`로 spawn한다.
  - `Saver` — 변경 알림을 받아 500ms 디바운스한 뒤 저장하는 스레드. 앱 종료 시 즉시 flush한다.

- [ ] **Step 1: 실패하는 테스트 작성**
```rust
#[test] fn roundtrip_preserves_groups_tabs_layout_focus() {}
#[test] fn corrupt_file_moves_to_bak() {}
#[test] fn restored_panes_respawn_without_prompt() {}
#[test] fn save_is_atomic_tmp_then_rename() { /* 저장 후 .tmp 없음 */ }
```
- [ ] **Step 2: 실패 확인** — `cargo test --manifest-path src-tauri/Cargo.toml persist::` → FAIL
- [ ] **Step 3: 구현.** 시작 경로를 `load` 결과에 따라 나눈다. Ok(Some)이면 `prepare_restored`를 쓰고, None이나 Err이면 `from_config`를 쓴다. Err일 때는 오류 목록에 "작업 상태 파일이 손상되어 초기 배치로 시작했습니다"를 추가한다. 모든 `ws_command` 성공 후와 상태 변경 후에는 Saver에 알린다.
- [ ] **Step 4: 통과 확인** → PASS
- [ ] **Step 5: Commit** — `feat(persist): atomic workspace save and restore`

---

### Task 11: 프론트엔드 렌더링 + 스파이크 S4

**Files:**
- Create: `src/groups.js`, `src/layout.js`, `src/tabs.js`
- Modify: `src/pane.js`, `src/app.js`, `src/index.html`, `src/style.css`

**Interfaces:**
- Consumes: Task 9 `api.js`, Snapshot 구조
- Produces:
  - `render(snapshot)` (app.js) — 그룹 레이아웃 → 그룹마다 탭 바 + 활성 탭의 창 레이아웃을 그린다. 비활성 탭의 창 DOM은 화면 밖 컨테이너(`#parking`)에 보관한다. 렌더 후 보이는 창마다 `fit()` + `resize`를 호출한다.
  - `pane.js`:
    - `getPane(paneId)` — 없으면 생성 + attach, 있으면 재사용
    - `disposePane(paneId)` — 스냅샷에서 사라진 창에 대해 호출
    - `paneHeader(pane)` — 이름, 프로필, 상태 기호(●/○ 코드/◌/✕ 실패), ⚠, ⟳, ✕
    - 종료나 실패 상태에서는 창 하단에 "Enter: 재시작" 줄을 표시한다. 그 창에 포커스가 있을 때 Enter를 누르면 `RestartPane`을 보낸다.
  - `layout.js`:
    - `renderLayout(node, renderLeaf, onSizes) -> HTMLElement` — flex로 그리고, 경계 드래그가 끝나면 `onSizes(path, sizes)`를 호출한다. 이 함수를 그룹과 탭 레이아웃 양쪽에서 쓴다.
    - 최대화된 창이나 그룹이 있으면 그것만 그린다.
  - `tabs.js` `renderTabBar(group, tabs, focused: bool)` — 활성 탭, 주의 배지, Dispatcher 아이콘을 표시한다. 포커스되지 않은 그룹은 `.dim` 클래스로 흐리게 한다.
  - 상태 표시줄 `#status`: `그룹 › 탭 › 창`, `[SYNC]`, `[설정 오류 N]`
  - 사이드바 `#sidebar`: 그룹 > 탭 > 창 트리, 기본은 숨김.

- [ ] **Step 1: 구현.** 마우스 클릭으로 창 포커스(`SetFocus`), 탭 클릭으로 전환, 헤더 버튼, 경계 드래그(`SetSizes`)를 연결한다.
- [ ] **Step 2: 수동 확인.** `team-3` 탭을 `config.json`에 두고 실행한다. 3분할이 보이고, 탭 전환 후 돌아와도 출력이 유지되며 터미널이 재생성되지 않으면 통과다.
- [ ] **Step 3: 스파이크 S4.** 3×3 템플릿(임시)으로 Claude 9개를 실행한다. 동시에 9개 창에서 `ping -t 127.0.0.1`을 돌려 입력 지연과 CPU를 관찰하고 `docs/spikes.md`에 기록한다.
  - 기준: 타이핑 반응이 체감상 지연 없음, 앱 CPU 30% 이하.
  - 미달이면 다음을 차례로 적용한다.
    - 비가시 탭은 `term.write`를 보류하고 attach 스냅샷으로 따라잡는다.
    - 배치 주기를 늘린다.
- [ ] **Step 4: Commit** — `feat(ui): groups, tab bars, split layouts, pane headers, status bar`

---

### Task 12: 키 라우터 · 방향 이동 · 포커스 + 스파이크 S5

**Files:**
- Create: `src/keys.js`, `src/geometry.js`, `src/focus.js`, `tests/js/keys.test.mjs`, `tests/js/geometry.test.mjs`, `tests/js/focus.test.mjs`
- Modify: `src/app.js`, `src/pane.js`

**Interfaces:**
- Produces:
  - `keys.js`
    - 순수 함수:
      - `parseChord(str) -> {ctrl, alt, shift, code}` — 글자는 `KeyX`, 숫자는 `DigitN`, `\`는 `Backslash`, `-`는 `Minus`, `;`는 `Semicolon`, `[`는 `BracketLeft`, `]`는 `BracketRight`, `tab`은 `Tab`
      - `buildKeymap(bindings: {[id]: str}) -> Map<chordKey, id>`
      - `matchEvent(keymap, e) -> id|null` — `e.code`와 수식키 정확 일치. `e.key`와 `isComposing`은 무시한다.
      - `isPassthrough(chordStr[], e) -> bool`
    - DOM 함수 `installKeyRouter({ keymap, getPassthrough(paneId), run(id) })`
      - document capture 단계에서 keydown을 받는다. 매칭되면 `preventDefault` + `stopPropagation` + `run(id)`를 실행한다.
      - xterm `attachCustomKeyEventHandler`에서는 매칭된 키에 false를 돌려준다.
  - `geometry.js` `findNeighbor(rects: {id,left,top,right,bottom}[], fromId, dir:'left'|'right'|'up'|'down') -> id|null`
    - 후보: 그 방향으로 완전히 넘어간 사각형(예: left면 `r.right <= from.left + 1`)
    - 우선순위:
      1. 수직축(또는 수평축) 겹침이 있는 후보
      2. 경계 간 거리가 최소
      3. 겹침 길이가 최대
  - `focus.js`
    - `nextAttention(order: PaneRef[], current: PaneRef, attention: Set<id>, origin: PaneRef|null) -> {target: PaneRef|null, origin: PaneRef|null}` — 현재 위치 다음의 주의 창으로 순환한다. 주의 창이 없거나 현재 창뿐이면 origin으로 돌아가고 origin을 지운다.
    - `applyFocus(snapshot)` — 포커스 체인을 따라 `term.focus()`를 호출한다.
    - `flash(paneId)` — 300ms 하이라이트
  - 명령 실행표 `run(id)`: 스펙 §6.2의 모든 id를 처리한다.
    - `pane.focus*` / `tab.*` / `group.goto*` / 분할 / 닫기 / 최대화 / 재시작 / `pane.resize*`(delta 5) / `tab.moveLeft·Right·Up·Down`은 각각 해당 `WsCommand`로 바꾼다.
    - `tab.moveLeft/Right`는 그룹 안 순서를 바꾼다. 탭 바 끝이면 `findNeighbor`로 찾은 옆 그룹 끝(왼쪽 이동이면 끝, 오른쪽 이동이면 0번 위치)으로 `MoveTab`을 보낸다.
    - `tab.moveUp/Down`은 위·아래 그룹으로 `MoveTab`을 보낸다.
    - `pane.focus*`는 보이는 모든 창의 사각형을 모아 `findNeighbor`로 대상을 찾은 뒤 `SetFocus`(그룹과 탭 포함)를 보낸다.
    - `tab.last`는 `lastLocation`으로, `tab.dispatcher`는 Dispatcher 탭이 있는 그룹으로 이동한다.
    - 오버레이·복사·동기화·팔레트·주의 관련 id는 Task 13–15에서 연결한다. 그 전까지는 아무 동작도 하지 않는다.
    - 탭·그룹 전환 뒤에는 `flash`를 호출한다.
    - 창 재활성화: `window` focus 이벤트에서 `applyFocus`를 호출한다.

- [ ] **Step 1: 실패하는 테스트 작성**
```js
test('matches Alt+h while Korean IME composing', () => {
  const km = buildKeymap({'pane.focusLeft':'Alt+h'})
  assert.equal(matchEvent(km, {code:'KeyH', key:'ㅗ', altKey:true, ctrlKey:false, shiftKey:false, isComposing:true}), 'pane.focusLeft')
})
test('modifiers must match exactly', () => { /* Ctrl+Alt+h ≠ Alt+h, Alt+Shift+H ≠ Alt+h */ })
test('parseChord symbols', () => { /* 'Alt+\\'→Backslash, 'Ctrl+Alt+-'→Minus, 'Alt+;'→Semicolon */ })
test('passthrough skips app handling', () => {})
test('claude keys are never in default keymap', () => {
  // defaults/keybindings.json 로드 → Alt+B/F/D/Y/V/M/P/T/O, Ctrl+단일글자, Ctrl+Enter, Shift+Tab, Ctrl+V 와 매칭되는 항목 없음
})
// geometry
test('neighbor crosses group boundary', () => { /* 좌측 그룹의 창 A, 우측 그룹 창 B·C(위/아래) → findNeighbor(A,'right') = A와 수직 겹침 큰 쪽 */ })
test('no neighbor at edge returns null', () => {})
test('prefers overlap over closer non-overlapping', () => {})
// focus
test('nextAttention cycles then returns to origin', () => {})
```
- [ ] **Step 2: 실패 확인** — `npm run test:js` → FAIL
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인** — `npm run test:js` → PASS
- [ ] **Step 5: 수동 확인.** 마우스를 쓰지 않고 다음을 수행한다.
  - `Alt+h/j/k/l` 이동(그룹 경계 포함), `Alt+\`·`Alt+-` 분할, `Alt+x` 닫기, `Alt+z` 최대화, `Alt+Shift+H/J/K/L` 크기 조절
  - `Ctrl+Alt+h/l/j/k`, `Alt+1..9`, `Ctrl+Alt+1..9`, `Ctrl+Alt+\`, `Ctrl+Alt+Shift+H/J/K/L`, `Ctrl+Alt+x`, `Ctrl+Alt+z`
  - 한글 IME를 켠 상태에서 위 키를 반복한다.
  - Claude 창에서 `Ctrl+R`, `Ctrl+P`, `Alt+P`, `Alt+V`, `Shift+Tab`이 Claude로 전달되는지 확인한다.
- [ ] **Step 6: 스파이크 S5.** `Alt+Shift+H` 반복 시 입력 언어가 바뀌는지 확인한다(한/영 + 다른 입력 언어를 설치한 상태). 바뀌면 스펙 §10 S5의 대안(`Alt+s` 크기 조절 모드)을 적용하고 스펙을 갱신한다.
- [ ] **Step 7: Commit** — `feat(keys): keyboard router, spatial navigation, focus rules`

---

### Task 13: 오버레이 — 번호 선택, 커맨드 팔레트, 새 탭 마법사, 확인 대화, 오류 보기

**Files:**
- Create: `src/overlays.js`, `src/fuzzy.js`, `tests/js/fuzzy.test.mjs`
- Modify: `src/app.js`

**Interfaces:**
- Produces:
  - `fuzzy.js` `fuzzyFilter(items: string[], query: string) -> string[]` — 대소문자 무시, 부분 순서 일치. 점수는 연속 일치 + 시작 일치 가산이고 점수 내림차순, 동점이면 원래 순서다.
  - `overlays.js` (모든 함수는 Promise를 돌려주고, `Esc`면 `null`)
    - `pickNumber(targets: {id, el}[], {allowHjkl, extraKeys:['\\','-']}) -> Promise<{id}|{dir}|{key}|null>`
      - 대상마다 1부터 번호를 표시하고, 2초 동안 입력이 없으면 null을 돌려준다.
      - 단축키 연결:
        - `pane.pick`: 창 대상으로 번호를 고르면 `SetFocus`
        - `tab.moveTo`: 그룹 대상. 숫자나 hjkl이면 `MoveTab`, `\`·`-`면 `SplitGroupWith`
        - `group.mergeInto`: 그룹 대상으로 `MergeGroup`
    - `palette(entries: {label, run}[]) -> Promise<void>` — 입력창과 목록으로 구성하고 ↑↓ / Enter / Esc로 조작한다. 항목은 다음과 같다.
      - 모든 명령(한국어 라벨 + 현재 키)
      - "탭으로 이동: <이름>", "창으로 이동: <탭>/<창>"
      - "탭 이동 → <그룹>", "그룹 합치기 → <그룹>"
      - "탭 이름 바꾸기"(입력 → `RenameTab`)
      - "현재 창을 새 탭으로 떼어내기"(`BreakPaneToTab`)
      - "설정 오류 보기"
      - "사이드바 토글"
    - `newTabWizard() -> Promise<void>`
      - ① `listDirs()`를 퍼지 목록으로 보여 준다. 목록에 없는 이름을 입력하고 Enter를 누르면 `confirm("'<이름>' 디렉토리를 만들까요?")` 후 `createDir`을 실행한다.
      - ② `listTemplates()` 목록에서 고른다(기본값 `defaultTemplate`이 맨 위).
      - ③ `CreateTab { name: dir, dir, template }`을 보낸다. 이름이 중복되면 오류를 토스트로 표시하고 ①로 돌아간다.
    - `confirm(message) -> Promise<boolean>` — Y/Enter=예, N/Esc=아니오
    - `showErrors(lines: string[])` — 목록 패널, Esc로 닫는다.
    - `toast(message)` — 상태 표시줄 위에 3초 동안 표시한다. `ws_command`의 Err는 모두 이걸로 보여 준다.
  - 탭이나 그룹이 전환되면 열린 오버레이를 닫는다(스펙 §6.4).
  - 시작 시 작업 폴더 확인(스펙 §4.2): 앱 시작 직후 `missing_dirs()`의 항목마다 `confirm("'<dir>' 디렉토리가 없습니다. 만들까요?")`를 띄운다. 예면 `createDir` 후 그 탭의 창마다 `RestartPane`을 보내고, 아니오면 창은 Failed 상태로 둔다.

- [ ] **Step 1: 실패하는 테스트 작성**
```js
test('fuzzy matches subsequence case-insensitively', () => { assert.deepEqual(fuzzyFilter(['mdviewer','hoohootv','ha'], 'mv'), ['mdviewer']) })
test('fuzzy ranks prefix and contiguous higher', () => { /* 'ha' → ['ha', 'hoohootv'...] 순서 */ })
test('empty query returns all in order', () => {})
```
- [ ] **Step 2: 실패 확인** — `npm run test:js` → FAIL
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인** → PASS
- [ ] **Step 5: 수동 확인.** 마우스 없이 다음을 수행한다.
  - `Alt+n`으로 새 디렉토리 생성 → 템플릿 선택 → 탭 생성
  - `Alt+q`로 창 이동
  - `Ctrl+Alt+m` + 숫자 / `\`로 탭 보내기와 새 그룹 만들기
  - `Ctrl+Alt+g`로 그룹 합치기
  - `Alt+;`로 탭 이름 변경, 창 떼어내기, 설정 오류 보기
  - 잘못된 템플릿 파일을 넣고 재시작 → 상태 표시줄 `[설정 오류 1]` → 팔레트에서 `파일:줄:열 — 원인` 확인
- [ ] **Step 6: Commit** — `feat(ui): number picker, command palette, new-tab wizard, dialogs`

---

### Task 14: 복사 모드 · 입력 동기화 · 붙여넣기

**Files:**
- Create: `src/copymode.js`, `tests/js/copymode.test.mjs`
- Modify: `src/pane.js`, `src/app.js`, `src-tauri/capabilities/default.json`(clipboard-manager 읽기/쓰기 권한), `src-tauri/src/lib.rs`(플러그인 등록)

**Interfaces:**
- Produces:
  - `copymode.js`
    - `createCopyState(buffer: {lineCount, lineText(i), viewportTop, rows, cursor:{row,col}})` → 상태
    - `copyKey(state, key) -> {state, action}` — 순수 함수
      - 이동: h/j/k/l, 방향키, PgUp/PgDn(rows만큼), g/G
      - `v`는 선택 시작, `y`는 `{action:'copy', text}` 후 종료
      - `/`는 `{action:'search'}`, n/N은 다음/이전
      - Esc/q는 `{action:'exit'}`
    - DOM 함수 `enterCopyMode(pane)`
      - 테두리 클래스 `.copy`를 붙이고, 이 모드 동안의 키 입력은 PTY로 보내지 않는다.
      - 선택은 `term.select`로 표시하고, 커서 행이 보이도록 `term.scrollToLine`을 호출한다.
      - 검색은 addon-search `findNext/findPrevious`를 쓴다.
      - 복사는 clipboard 플러그인의 `writeText`를 쓴다.
  - 입력 동기화
    - `pane.js`의 `term.onData`에서 탭의 `syncInput`이 true면 같은 탭의 모든 Running 창에 write한다.
    - `tab.syncInput`은 `SetSyncInput` 토글로 연결한다.
  - `pane.paste`: clipboard 플러그인의 `readText` 후 `term.paste(text)`를 호출한다. xterm이 bracketed paste 모드를 처리한다.

- [ ] **Step 1: 실패하는 테스트 작성**
```js
test('copy mode moves and selects lines', () => { /* 버퍼 5줄, k k v j y → text = 줄3+'\n'+줄4 */ })
test('PgUp moves by rows and clamps at 0', () => {})
test('Esc exits without copy', () => {})
```
- [ ] **Step 2: 실패 확인** — `npm run test:js` → FAIL
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인** → PASS
- [ ] **Step 5: 수동 확인.**
  - `Alt+c` → k·v·j·y → 다른 창에서 `Ctrl+Shift+V`로 붙여넣기. 복사 모드 중에 탭을 갔다 와도 모드가 유지되어야 한다.
  - `Alt+i` 후 타이핑 → 탭 안 모든 창에 입력되고 `[SYNC]`가 표시되어야 한다.
- [ ] **Step 6: Commit** — `feat(ui): copy mode, synchronized input, paste`

---

### Task 15: 주의(attention) 표시

**Files:**
- Modify: `src/pane.js`, `src/tabs.js`, `src/app.js`, `src-tauri/defaults/agents/claude.json`(필요 시 문서화만)

**Interfaces:**
- Consumes: Task 7 `SetAttention`, Task 12 `nextAttention`
- Produces:
  - `term.onBell` → 그 창이 포커스 창이 아니면 `SetAttention{on:true}`를 보낸다. 포커스를 받으면 백엔드 `SetFocus`가 해제한다.
  - 표시: 창 헤더 ⚠, 탭 바 배지, 상태 표시줄. **포커스를 자동으로 옮기지 않는다.**
  - `pane.nextAttention`(`Alt+a`): 순서는 그룹 leaves 순 → 탭 순 → 창 leaves 순으로 만든 `order`를 쓰고, `nextAttention`으로 대상을 정한 뒤 `SetFocus`를 보낸다.
  - README에 "Claude Code에서 `/config` → 알림 채널을 terminal bell로 설정" 안내를 적는다.

- [ ] **Step 1: 수동 확인.**
  - 비활성 탭의 창에서 `powershell -c "[console]::beep(); Write-Host \"`a\""`를 실행하거나 Claude 권한 프롬프트를 띄운다 → ⚠와 배지가 표시되고, 포커스는 그대로여야 한다.
  - `Alt+a` → 그 창으로 이동하고 ⚠가 해제되어야 한다.
  - 다시 `Alt+a` → 원래 위치로 돌아와야 한다.
- [ ] **Step 2: Commit** — `feat(ui): attention badges via terminal bell and Alt+a`

---

### Task 16: 마우스 드래그앤드롭 + 스파이크 S6

**Files:**
- Modify: `src/tabs.js`, `src/overlays.js`, `src/app.js`, `src/style.css`
- Reference: `C:/workspace/mdviewer/src/tabs.js:208-265` (dragstart/drop, 수동 더블클릭 판정)

**Interfaces:**
- Consumes: `MoveTab`, `SplitGroupWith`
- Produces:
  - `dropZone(rect, x, y) -> 'left'|'right'|'top'|'bottom'|'center'` (overlays.js, 순수 함수) — 각 변에서 25% 안쪽이면 그 가장자리, 아니면 center. 모서리처럼 두 영역이 겹치면 가까운 변을 우선한다.
  - 탭 `dragstart` 시 모든 그룹 위에 `.drop-overlay`를 깔고, 드래그 중에는 미리보기 사각형을 그린다.
  - 드롭 결과:
    - 탭 바 위(같은 그룹이든 다른 그룹이든)에 놓으면 `MoveTab{index}`. index는 대상 탭 중앙 기준 전/후로 정한다(mdviewer 로직).
    - 그룹 본문 center면 `MoveTab{index: 끝}`
    - 가장자리면 `SplitGroupWith{dir, before: left|top}`
  - 드롭한 뒤 포커스는 옮긴 탭의 마지막 창으로 간다.
- [ ] **Step 1: 실패하는 테스트 작성** (`tests/js/dropzone.test.mjs`)
```js
test('dropZone edges and center', () => { /* rect 0..100: (10,50)→left, (90,50)→right, (50,10)→top, (50,90)→bottom, (50,50)→center, (24.9,50)→left, (25.1,50)→center */ })
test('corner prefers nearer edge', () => { /* (5,20) → left */ })
```
- [ ] **Step 2: 실패 확인** → FAIL; **Step 3: 구현**; **Step 4: 통과 확인** → PASS
- [ ] **Step 5: 스파이크 S6 + 수동 확인.** 터미널 위에서 드래그 이벤트가 오버레이로 들어오는지 확인한다. 실패하면 mousedown/mousemove로 직접 구현하고 결과를 기록한다. 그다음 4가지 드롭을 모두 수행하고, 탭 클릭과 더블클릭이 정상인지 확인한다.
- [ ] **Step 6: Commit** — `feat(ui): drag tabs between groups and onto group edges`

---

### Task 17: 종료 확인 · 문서 · 전체 QA

**Files:**
- Create: `README.md`, `History.md`
- Modify: `src/app.js`, `src-tauri/src/lib.rs`, `docs/spikes.md`

**Interfaces:**
- Produces:
  - 앱 종료: JS `getCurrentWindow().onCloseRequested`에서 `preventDefault`한 뒤 `confirm("실행 중인 에이전트 N개를 종료하고 닫을까요?")`를 띄운다. 예를 누르면 `app_exit` 명령을 실행한다 — Saver flush → PtyManager 전체 kill → `app.exit(0)`. N은 `running_count`이고, 0이면 묻지 않는다.
  - README: 설치·개발 명령, 설정 파일 위치와 예시(스펙 §4), 키 배치 표(§6.2), 알려진 주의점(§6.2 마지막), 터미널 벨 설정 안내.

- [ ] **Step 1: 구현과 문서 작성**
- [ ] **Step 2: 전체 테스트** — `cargo test --manifest-path src-tauri/Cargo.toml && npm run test:js` → 모두 PASS
- [ ] **Step 3: 스펙 §11의 수동 QA 체크리스트 1~7을 모두 수행하고 결과를 `docs/spikes.md`의 "QA 결과" 절에 기록한다.** 특히 5번(재시작 후 고아 프로세스 없음)은 작업 관리자에서 `claude`·`pwsh` 프로세스 수로 확인한다.
- [ ] **Step 4: 릴리스 빌드 확인** — `npx tauri build`로 `src-tauri/target/release/bundle/nsis/*.exe`가 생성되고, 설치본에서 앱이 정상 실행되면 통과다.
- [ ] **Step 5: Commit** — `docs: README, close confirmation, phase 1 QA results`
