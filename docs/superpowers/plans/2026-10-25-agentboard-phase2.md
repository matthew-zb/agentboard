# AgentBoard 2단계(상시업무 조직 운영) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 1단계 멀티플렉서 위에 dispatcher·handover·Manager 상시 에이전트, 임시 팀(리드·팀원) 채용·해산, Manager가 올리는 인박스와 dispatcher 옆 인박스 애플릿, 이슈 폴더, 야간 정기 정리, 자동 실행을 얹는다.

**Architecture:** 에이전트끼리의 대화는 Claude Code 세션 간 메시징(`SendMessage`, `--name`)에 맡긴다. 앱은 (1) 역할에 맞는 인자·역할 지시·MCP·hook 설정을 붙여 세션을 띄우고, (2) 127.0.0.1 HTTP 제어 서버에서 MCP 도구(인박스·이슈·팀·탭)와 hook 수신을 처리하며, (3) 앱→세션 메시지는 세션 inbox 파이프(대안: 안전한 PTY 입력)로 보낸다. 모델 변경은 1단계처럼 모두 `WsCommand`를 거친다.

**Tech Stack:** 1단계 스택 + `axum`(HTTP 제어 서버, tokio 런타임은 Tauri 것 사용), Windows named pipe(`tokio::net::windows::named_pipe`), `winreg`(자동 실행). MCP는 최소 JSON-RPC over HTTP POST를 직접 구현한다(외부 MCP SDK 없음).

**Spec:** `docs/superpowers/specs/2026-10-25-agentboard-phase2-design.md` (권위 문서). 1단계 스펙·후속 과제: `docs/superpowers/specs/2026-10-24-agentboard-design.md`, `docs/superpowers/phase1-followups.md`.

## Global Constraints

- 1단계 Global Constraints(1단계 계획 문서) 전부 유지: Windows 11, 번들러 없는 ES 모듈, 한국어 UI, `KeyboardEvent.code` 키 판별, **포커스·탭 자동 이동 금지**, 모델 변경은 `Workspace::apply`/전용 메서드만, launcher.rs 잠금 순서 준수(새 잠금은 그 문서에 추가).
- Claude Code **v2.1.234 이상**(Windows 세션 간 메시징). 미달이면 상태 표시줄 오류 + 에이전트 간 메시지만 불가, 탭·MCP·인박스는 동작.
- 역할: `dispatcher`, `handover`, `manager`, `lead`, `member`. 세션 이름: 상시 = 탭 이름, 팀 = `<탭>-<팀id>-lead`, `<탭>-<팀id>-m<N>`.
- 앱이 띄우는 role 세션: `--name`, `--append-system-prompt <역할 지시 파일 내용>`, `--mcp-config <파일>`, `--settings <파일>`(hooks + `"crossSessionInbound": "accept"`). 파일은 `%APPDATA%/agentboard/run/<paneId>.{mcp,settings}.json`.
- 제어 서버: `127.0.0.1` 임의 포트, 경로 `/mcp`(JSON-RPC), `/hook/<event>`. 인증 `Authorization: Bearer <pane 토큰>`(창마다 무작위 32바이트 hex, 재시작마다 새로 발급). 외부 인터페이스 바인딩 금지.
- 인박스 저장: `<workspaceRoot>/.agentboard/inbox/<itemId>.json`, 원자적 쓰기. 상태: `new` → `in_progress` → `close_proposed` → `done`(사용자만), `close_proposed` → `in_progress` 가능.
- 이슈 폴더: `<담당 탭 workspace>/issues/<YYYYMMDD-slug>/{input,output}`, 중복 시 `-2`…. 팀 문서: `<탭 workspace>/teams/<팀id>/{brief.md,roster.md}`.
- 야간 정리: `nightlyRefresh` "HH:MM"(기본 "03:00"), `nightlyTimeoutMin`(기본 20), 사용자 입력 유예 `nightlyQuietMin`(기본 10). 팀 세션은 재시작하지 않는다.
- 팀원 상한 `roles.member.maxPerTeam` 기본 4. 팀 수 제한 없음.
- 사용자에게 보이는 모든 문구 한국어. 모든 새 JSON 설정은 1단계처럼 위치가 있는 검증 오류를 낸다.
- **설계 문서 대비 의도적 차이:** 스펙 §10 예시의 `tabs`에 dispatcher를 넣는 형태 대신, 1단계의 `config.dispatcher` 블록을 유지하고 dispatcher 역할은 거기서 암묵적으로 정한다(1단계 예약어·검증 재사용). `tabs[].role`은 `manager`·`handover`만 받는다.

## Review Focus

1. **남의 토큰·토큰 없음·만료 토큰으로 MCP 호출** — 401과 한국어 오류, 상태 불변. → Task 5 테스트 `rejects_missing_or_stale_token`.
2. **재시작으로 세션의 inbox 파이프 토큰이 바뀐 뒤 앱이 메시지를 보냄** — 옛 토큰을 쓰지 않고, 새 SessionStart 보고를 기다렸다 보내거나 대기열에 남긴다(메시지 유실 없음). → Task 7 테스트 `bridge_waits_for_fresh_session_registration`.
3. **여러 Manager가 동시에 인박스를 쓰는 경우** — 항목 파일이 깨지거나 타임라인이 유실되지 않는다. → Task 6 테스트 `concurrent_notes_are_all_kept`.
4. **이슈 폴더로 자료 복사 시 같은 이름 파일·작업 폴더 밖 대상** — 덮어쓰지 않고 `(2)` 접미사, 대상 경로는 항상 이슈 폴더 안. → Task 6 테스트 `add_files_never_overwrites_or_escapes`.
5. **야간 정리 시각에 사용자가 dispatcher에 입력 중·Manager가 응답 중** — 정리를 미루고, 팀 세션은 건드리지 않는다. → Task 11 테스트 `nightly_defers_while_user_typing_and_skips_teams`.

---

## File Structure (2단계 추가·변경)

```
src-tauri/src/
  roles.rs        # Role enum, RoleConfig 로드·검증, 역할 지시 파일 로딩, 세션 이름 규칙
  agentcfg.rs     # role 세션 실행 인자·run 파일(mcp/settings) 생성, pane 토큰 발급
  control.rs      # axum 제어 서버: /mcp JSON-RPC, /hook/*, 인증·신원
  mcp_tools.rs    # MCP 도구 정의·권한 검사·디스패치 (inbox/issue/team/tab/handoff)
  inbox.rs        # 인박스 저장소: 항목·타임라인·상태 전이·파일 복사
  issues.rs       # 이슈 id·폴더 생성·경로 안전
  bridge.rs       # 앱→세션 메시지: 세션 등록(소켓·토큰)·named pipe 전송·PTY 대안
  nightly.rs      # 야간 정리 상태 기계 + 스케줄러 스레드
  autostart.rs    # HKCU Run 토글
  hookcli.rs      # `agentboard.exe --hook <event>` 모드 (stdin → 제어 서버 POST)
  (변경) config.rs workspace.rs launcher.rs lib.rs main.rs persist.rs
src-tauri/defaults/
  roles/{dispatcher,handover,manager,lead,member}.md
  templates/{dispatcher-inbox,handover,manager}.json
src/
  inbox.js        # 인박스 애플릿 렌더링·키 처리
  inbox-logic.js  # 순수 로직(목록 정렬·필터·상태 라벨·키→액션)
  (변경) pane.js app.js groups.js api.js overlays.js overlay-logic.js
tests/js/inbox.test.mjs
spikes/phase2/    # Task 1 버리는 스파이크 코드 (커밋은 하되 빌드 대상 아님)
```

---

### Task 1: 스파이크 P1–P3 (Claude Code 메시징·hook·MCP 헤더)

**Files:**
- Create: `spikes/phase2/README.md`, `spikes/phase2/*`(자유), `docs/spikes.md`의 "2단계 스파이크" 절

**Interfaces:**
- Produces (문서로): inbox 파이프에 쓰는 **정확한 메시지 줄 형식**과 인증 줄, SessionStart hook이 받는 환경 변수·stdin JSON 예, Notification/Stop hook stdin 예, `--mcp-config` HTTP 헤더 동작 여부, `--settings` 파일로 hook·`crossSessionInbound` 주입 동작 여부. Task 4·5·7·10이 이 결과를 따른다.

- [ ] **Step 0: 전제** — `claude --version` ≥ 2.1.234 확인. 미달이면 멈추고 사용자에게 `claude update` 승인을 요청한다(컨트롤러가 처리).
- [ ] **Step 1: P2** — PowerShell 두 개에서 `claude --name spike-a`, `claude --name spike-b`를 auto 모드로 띄우거나(사람 필요 시 R8: 절차 기록), 자동화 가능하면 `claude -p --name … --settings '{"crossSessionInbound":"accept"}'` 워커로 `SendMessage` 전달·hold 여부를 확인한다.
- [ ] **Step 2: P1** — `--settings`에 SessionStart hook(`{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"<스크립트>"}]}]}}`)을 넣어 `CLAUDE_CODE_MESSAGING_SOCKET`·`CLAUDE_CODE_MESSAGING_TOKEN`을 파일로 떨군 뒤, 별도 프로세스가 그 파이프에 `{"type":"auth","token":…}` + 메시지 줄을 써서 세션에 도착하는지 확인한다. 메시지 줄 스키마는 Claude Code 문서/실험으로 확정해 기록한다.
- [ ] **Step 3: P3** — 작은 HTTP MCP 서버(파이썬/노드 아무거나, `spikes/phase2/`)로 `--mcp-config`에 `"type":"http","url":…,"headers":{"Authorization":"Bearer x"}`가 전달되는지, `tools/list`·`tools/call` 왕복을 확인한다.
- [ ] **Step 4: 기록** — `docs/spikes.md` "2단계 스파이크"에 P1–P3 결과·정확한 형식·실패 시 선택한 대안을 적는다. 사람 손이 필요한 확인은 "사용자 확인 필요"(1단계 R8 방식).
- [ ] **Step 5: Commit** — `chore(spike): phase 2 messaging/hook/MCP spikes`

---

### Task 2: 1단계 부채 정리 — 명령 실행 경로 분리 · spawn generation · 키바인딩 섀도잉

**Files:**
- Modify: `src-tauri/src/launcher.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/pty.rs`, `src-tauri/src/config.rs`

**Interfaces:**
- Produces:
  - `pub fn execute(state: &AppState, cmd: WsCommand) -> Result<Snapshot, String>` (launcher.rs) — apply → submit(effects) → publish. `ws_command` Tauri 명령과 이후 MCP 도구가 모두 이것을 호출한다.
  - `PtySink::exited(&self, id: &str, gen: u64, code: i32)` / `output(&self, id, gen, bytes)` — PtyManager가 spawn마다 증가하는 generation을 붙인다. `PtyManager::spawn(...) -> Result<u64, String>`(반환 generation), `PtyManager::current_gen(id) -> Option<u64>`.
  - launcher는 generation이 현재와 다르면 exited/output을 무시한다(늦은 종료가 새 프로세스 상태를 덮지 않음, 재시작 실패 메시지 보존).
  - `ensure_dotdir`는 새 설치에서 `keybindings.json`을 `{}`로 만든다(기존 파일은 손대지 않음). 기본값은 계속 내장 파일에서 온다.

- [ ] **Step 1: 실패하는 테스트**
```rust
#[test] fn stale_exit_does_not_mark_respawned_pane() { /* spawn → gen1, restart → gen2, gen1 exited 늦게 도착 → 상태 Running 유지 */ }
#[test] fn failed_restart_message_survives_old_exit() {}
#[test] fn execute_returns_emitted_snapshot() {}
#[test] fn new_install_keybindings_file_is_empty_object() {}
```
- [ ] **Step 2: 실패 확인** → `cargo test --manifest-path src-tauri/Cargo.toml` FAIL
- [ ] **Step 3: 구현** — `AppSink`·launcher의 exited/output 경로에 gen 비교. 1단계 테스트(pty 통합 포함)를 새 시그니처로 갱신.
- [ ] **Step 4: 통과 확인** → 전체 PASS
- [ ] **Step 5: Commit** — `refactor: execute() command path, spawn generations, empty default keybindings file`

---

### Task 3: 역할·설정·모델 확장 (role, team, applet 창)

**Files:**
- Create: `src-tauri/src/roles.rs`
- Modify: `src-tauri/src/config.rs`, `src-tauri/src/workspace.rs`, `src-tauri/src/persist.rs`, `src-tauri/src/launcher.rs`

**Interfaces:**
- Produces:
  - `pub enum Role { Dispatcher, Handover, Manager, Lead, Member }` (serde lowercase)
  - `pub struct RoleConfig { pub system_prompt_file: Option<String>, pub agent: Option<String>, pub args: Vec<String>, pub max_per_team: Option<usize> }`, `Config.roles: BTreeMap<Role, RoleConfig>`, `Config.nightly_refresh: Option<String>`("HH:MM" 검증), `nightly_timeout_min: u32`(20), `nightly_quiet_min: u32`(10), `handover_index: Option<String>`(기본 `<handover dir>/INDEX.md`), `TabSpec.role: Option<Role>`(manager|handover만 허용).
  - `pub fn session_name(tab: &str, role: Role, team: Option<&str>, n: usize) -> String` — 상시 = 탭 이름, lead = `<tab>-<team>-lead`, member = `<tab>-<team>-m<n>`.
  - `Pane` 추가 필드: `kind: PaneKind`(`Terminal` | `Applet { kind: String }`), `role: Option<Role>`, `team: Option<String>`, `session_name: Option<String>`. `Tab.role: Option<Role>`. 모두 `#[serde(default)]`(1단계 workspace.json 호환).
  - 템플릿 노드 `{"applet":{"name":"inbox","kind":"inbox"}}` 파싱(PaneSpec에 `applet: Option<String>`), 검증: applet은 agent·cwd·prompt 불가.
  - launcher: `PaneKind::Applet`은 spawn하지 않고 상태 `Running` 고정(프로세스 없음).
  - 팀 slot: 템플릿 split의 `slot: "team"`. 없으면 Manager 창 오른쪽에 팀 영역을 만든다.
- [ ] **Step 1: 실패하는 테스트**
```rust
#[test] fn parses_roles_and_nightly_config() {}
#[test] fn rejects_bad_nightly_time_and_dispatcher_role_in_tabs() {}
#[test] fn applet_node_parses_and_rejects_agent_fields() {}
#[test] fn session_name_rules() { assert_eq!(session_name("billing", Role::Lead, Some("batch"), 0), "billing-batch-lead"); /* member m1 등 */ }
#[test] fn phase1_workspace_json_still_loads() { /* 1단계 포맷 fixture → 새 필드 기본값 */ }
#[test] fn applet_pane_is_never_spawned() {}
```
- [ ] **Step 2: 실패 확인** → FAIL
- [ ] **Step 3: 구현**
- [ ] **Step 4: 통과 확인** → PASS
- [ ] **Step 5: Commit** — `feat(config): roles, team/applet panes, nightly settings`

---

### Task 4: role 세션 실행 구성 (인자·run 파일·토큰·hook CLI)

**Files:**
- Create: `src-tauri/src/agentcfg.rs`, `src-tauri/src/hookcli.rs`
- Modify: `src-tauri/src/launcher.rs`(plan_spawn), `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`(AppState에 토큰 레지스트리)

**Interfaces:**
- Consumes: Task 1 결과(hook stdin·환경 변수·헤더 형식), Task 3 Role/RoleConfig.
- Produces:
  - `pub struct Tokens` — `issue(pane: &str) -> String`, `resolve(token: &str) -> Option<PaneId>`, `revoke(pane)`. 재시작마다 새 토큰.
  - `pub fn role_spawn_extras(pane: &Pane, role: Role, cfg: &Loaded, ctrl_url: &str, token: &str, run_dir: &Path) -> Result<(Vec<String> /*args*/, Vec<(String,String)> /*env*/), String>` — `--name`, `--append-system-prompt`(역할 파일 내용; 없으면 내장 기본), `--mcp-config <run/<pane>.mcp.json>`, `--settings <run/<pane>.settings.json>`. settings에 SessionStart·Notification·Stop hook = `"<agentboard.exe 절대경로>" --hook <event>`, env `AGENTBOARD_URL`, `AGENTBOARD_TOKEN`.
  - `hookcli::run(event: &str) -> i32` — stdin JSON과 `CLAUDE_CODE_MESSAGING_SOCKET/TOKEN` 환경 변수를 읽어 `POST {AGENTBOARD_URL}/hook/{event}`(Bearer). 실패해도 0 반환(세션을 막지 않음), 2초 타임아웃.
  - `main.rs`: 인자 `--hook <event>`면 Tauri를 띄우지 않고 `hookcli::run` 후 종료.
  - plan_spawn: pane.role이 있으면 extras를 붙인다. role 없는 1단계 창은 그대로.
- [ ] **Step 1: 실패하는 테스트** — extras에 4개 플래그와 env가 정확히 들어가는지, run 파일 JSON 내용(헤더·hook 명령·crossSessionInbound), 토큰 재발급 시 옛 토큰 resolve 실패, role 없는 창은 extras 없음.
- [ ] **Step 2: 실패 확인 → Step 3: 구현 → Step 4: 통과 확인**
- [ ] **Step 5: Commit** — `feat(agents): role session launch config, per-pane tokens, hook CLI mode`

---

### Task 5: 제어 서버와 MCP 골격 (인증·신원·권한)

**Files:**
- Create: `src-tauri/src/control.rs`, `src-tauri/src/mcp_tools.rs`
- Modify: `src-tauri/Cargo.toml`(`axum`), `src-tauri/src/lib.rs`(서버 시작, URL을 AppState에)

**Interfaces:**
- Produces:
  - `pub async fn serve(state: AppHandle) -> (SocketAddr)` — 127.0.0.1:0 바인딩, 실제 포트를 AppState.ctrl_url에.
  - `/mcp`: JSON-RPC 2.0 `initialize`(serverInfo `agentboard`, capabilities.tools), `notifications/initialized`(204), `tools/list`(호출자 역할에 허용된 도구만), `tools/call`. 응답은 `application/json` 단건(스트리밍 없음).
  - `pub struct Caller { pub pane: PaneId, pub role: Role, pub tab: TabId, pub team: Option<String>, pub session_name: String }` — 토큰 → 현재 Workspace에서 계산.
  - `mcp_tools::list(role: Role) -> Vec<ToolDef>`, `mcp_tools::call(state, &Caller, name, args: Value) -> Result<Value, String>`; 오류는 MCP `isError: true` + 한국어 텍스트.
  - 권한 표: 스펙 §4 표 그대로(+ `create_manager_tab`). 이 Task에서는 디스패치 골격과 `list_team`·`handoff_done` 자리만 두고, 나머지 도구는 Task 6·8·9에서 채운다(미구현 도구는 목록에 없음).
  - `/hook/<event>`: Task 7·10이 처리기를 등록하는 `HookRouter`(이벤트 이름 → fn). 이 Task에서는 인증과 라우팅만.
- [ ] **Step 1: 실패하는 테스트** (`tests/control.rs`, 실제 서버를 임의 포트로)
```rust
#[test] fn rejects_missing_or_stale_token() {}
#[test] fn tools_list_is_filtered_by_role() {}
#[test] fn initialize_roundtrip() {}
#[test] fn server_binds_loopback_only() {}
```
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: Commit** — `feat(control): local MCP/hook server with per-pane auth`

---

### Task 6: 인박스 저장소·이슈 폴더·MCP 인박스 도구

**Files:**
- Create: `src-tauri/src/inbox.rs`, `src-tauri/src/issues.rs`
- Modify: `src-tauri/src/mcp_tools.rs`, `src-tauri/src/lib.rs`(Tauri 명령·`inbox-changed` 이벤트)

**Interfaces:**
- Produces:
  - `InboxItem { id, owner_tab: TabId, owner_name: String, title, check: Vec<String>, risk: Vec<String>, source: String, refs: Vec<String>, state: ItemState, read: bool, issue: Option<String /*issue id*/>, created, timeline: Vec<TimelineEntry{ at, who, kind: Posted|Note|Instruction|FilesAdded|IssueLinked|CloseProposed{opinion}|Reopened|Done, text }> }`
  - `pub struct Inbox` (내부 Mutex) — `post`, `note`, `propose_close`, `instruct`(사용자 지시 기록 + close_proposed/new → in_progress), `confirm_done`(close_proposed만), `mark_read`, `link_issue`, `list(filter)`, `get`. 파일 단위 원자적 쓰기. 시작 시 디렉토리에서 로드(깨진 파일은 건너뛰고 오류 목록에).
  - `issues::create(tab_workspace: &Path, title: &str, today: NaiveDate) -> Result<(String /*id*/, PathBuf), String>` — `YYYYMMDD-<slug>`(slug: 영숫자·한글·하이픈, 40자), 중복 `-2`.
  - `inbox.add_files(item, paths: &[PathBuf], target: Input|Output) -> Result<Vec<PathBuf>, String>` — 이슈 폴더 필수, 같은 이름은 `이름 (2).ext`, 디렉토리 입력은 거부(한국어).
  - MCP 도구: `inbox_post`, `inbox_note`, `inbox_propose_close`, `inbox_list`, `issue_create` (권한: 스펙 §4).
  - Tauri 명령: `inbox_list(show_done: bool)`, `inbox_get(id)`, `inbox_mark_read(id)`, `inbox_confirm_done(id)`, `inbox_add_files(id, paths, target)`. 변경 시 `inbox-changed`(버전 번호, 1단계 R16 방식).
- [ ] **Step 1: 실패하는 테스트**
```rust
#[test] fn state_machine_user_only_done() {}
#[test] fn reinstruct_from_close_proposed_returns_in_progress() {}
#[test] fn concurrent_notes_are_all_kept() {}
#[test] fn add_files_never_overwrites_or_escapes() {}
#[test] fn issue_id_slug_and_dedupe() {}
#[test] fn only_owner_manager_can_note_or_propose_close() {}
#[test] fn corrupt_item_file_is_skipped_with_error() {}
```
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: Commit** — `feat(inbox): inbox store, issue folders, MCP inbox tools`

---

### Task 7: 앱→세션 메시지 브리지

**Files:**
- Create: `src-tauri/src/bridge.rs`
- Modify: `src-tauri/src/control.rs`(SessionStart/Stop hook 처리기), `src-tauri/src/lib.rs`(pty_write에서 창별 마지막 입력 시각 기록), launcher.rs 잠금 순서 문서

**Interfaces:**
- Consumes: Task 1의 파이프 메시지 형식, Task 4 hook CLI.
- Produces:
  - `SessionReg { pane, gen: u64, socket: String, token: String, idle: bool }` — SessionStart hook이 등록·갱신, Stop hook이 `idle=true`, UserPromptSubmit/출력 시작이 `idle=false`(Task 1에서 가능한 신호로).
  - `pub fn send(state: &AppState, pane: &str, text: &str) -> Result<(), String>` — 등록이 현재 generation이면 파이프로 전송. 아직 등록 전(재시작 직후)이면 **대기열**에 넣고 등록되면 보낸다(최대 10분, 넘으면 오류 반환 + 인박스 시스템 항목). P1 실패 시 대안: 대상 창이 idle(Stop hook 이후 출력 없음)이고 **최근 3초 내 사용자 입력이 없을 때만** `encode_submit`으로 PTY 입력, 아니면 대기열에서 재시도.
  - `last_input: Mutex<HashMap<PaneId, Instant>>` (leaf lock) — `pty_write`에서 갱신. Task 11도 사용.
  - 메시지 머리말 규칙: `[AgentBoard] …` 로 시작(받는 Claude가 앱 메시지임을 알게).
- [ ] **Step 1: 실패하는 테스트** — 가짜 named pipe 서버로 auth 줄+메시지 형식, `bridge_waits_for_fresh_session_registration`(옛 gen 등록 무시, 새 등록 후 대기열 전송), 대기 만료, PTY 대안의 입력 중 보류.
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: Commit** — `feat(bridge): app-to-session messages via inbox pipe with safe fallback`

---

### Task 8: 팀 채용·해산 · 상시업무 탭 생성

**Files:**
- Modify: `src-tauri/src/workspace.rs`(WsCommand 추가), `src-tauri/src/mcp_tools.rs`, `src-tauri/src/config.rs`(config.json에 탭 추가 저장)

**Interfaces:**
- Produces:
  - `WsCommand::AddTeamPane { tab: TabId, team: String, role: Role /*Lead|Member*/, prompt: Option<String> }` — 팀 slot(없으면 Manager 창 오른쪽 영역)에 팀별 하위 영역을 만들고 창 추가. 이름 `session_name` 규칙, member 번호는 팀 안에서 증가. 상한 초과·리드 없는 팀에 member·같은 팀 lead 중복은 Err. **포커스 변경 없음.**
  - `WsCommand::RemoveTeam { tab, team }` — 팀 창 모두 kill·제거, 하위 영역 정리. `ClosePane`으로 팀원 하나 제거.
  - `WsCommand::CreateTab`에 `role: Option<Role>` 추가. `group: Some(g)`이고 g가 포커스 그룹이 아니면 그 그룹의 활성 탭도 바꾸지 않는다(1단계 R12 완성).
  - MCP: `hire_lead(team, brief_path, extra_prompt?)`(brief_path는 호출자 탭 workspace 안이어야 함, lead 프롬프트에 경로 포함), `hire_member(extra_prompt?)`(호출자 팀), `dismiss(agent)`, `dismiss_team(team)`, `list_team(team?)`, `create_manager_tab(name, dir, template?)`(이름·dir 검증 1단계 규칙 재사용, `config.json` `tabs`에 원자적 추가 — 사용자가 손댄 다른 키 보존).
- [ ] **Step 1: 실패하는 테스트**
```rust
#[test] fn hire_lead_then_members_into_team_slot_without_focus_change() {}
#[test] fn multiple_teams_per_tab() {}
#[test] fn member_cap_and_lead_required() {}
#[test] fn lead_can_only_dismiss_own_members() {}
#[test] fn remove_team_kills_all_team_panes() {}
#[test] fn create_manager_tab_persists_config_and_keeps_focus() {}
#[test] fn brief_path_outside_tab_workspace_rejected() {}
```
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: Commit** — `feat(teams): hire/dismiss teams, runtime manager tabs`

---

### Task 9: 주의 표시 강화 (Notification hook · 상태 표시줄 ⚠ 목록)

**Files:**
- Modify: `src-tauri/src/control.rs`(Notification 처리기 → `SetAttention{on:true}`; 포커스된 창이면 무시), `src/groups.js`/`src/app.js`(상태 표시줄), `README.md`

**Interfaces:**
- Produces:
  - Notification hook 중 `permission_prompt`만 → 해당 창 attention(`idle_prompt`는 무시 — 상시 에이전트가 쉬는 동안 늘 발생). 포커스된 창이면 무시.
  - **MCP 도구 `need_user(reason: String)` (모든 역할)** — 호출자 창 attention on + `Pane.attention_reason: Option<String>`(serde default) 저장. SetFocus로 attention이 해제될 때 reason도 지운다. ⚠ 툴팁과 상태 표시줄 목록에 사유 표시.
  - 그룹 최대화 등으로 ⚠ 탭이 화면에 안 보이면 상태 표시줄에 `⚠ billing(사유…), api` (클릭·`Alt+a`로 이동). 순수 함수 `hiddenAttentionTabs(snapshot) -> {tab, reason?}[]` + 테스트.
  - SessionStart가 spawn 후 30초 안에 오지 않은 role 창 → attention + reason "폴더 신뢰 확인 필요"(ledger R5).
- [ ] **Step 1–4** — 테스트(`hidden_attention_tabs`, `need_user_sets_reason_and_focus_clears_it`, permission_prompt → attention·idle_prompt 무시, 포커스 창 무시, `missing_session_start_flags_trust`) → 구현 → PASS
- [ ] **Step 5: Commit** — `feat(attention): notification hook and status-bar attention list`

---

### Task 10: 인박스 애플릿 UI

**Files:**
- Create: `src/inbox.js`, `src/inbox-logic.js`, `tests/js/inbox.test.mjs`
- Modify: `src/pane.js`(applet 창이면 xterm 대신 애플릿 마운트), `src/app.js`, `src/api.js`, `src/style.css`, `src/overlays.js`(다중 경로 입력 prompt 재사용)

**Interfaces:**
- Consumes: Task 6 Tauri 명령·`inbox-changed`, Task 7 지시 전달용 Tauri 명령 `inbox_instruct(id, text)`(백엔드: `inbox.instruct` 기록 + `bridge::send(dispatcher, "[AgentBoard] 인박스 지시 — 항목 <id> (<담당>): <제목>\n확인할 것…\n리스크…\n지시: <text>\n담당 Manager(@<session>)에게 전달하라.")`), `inbox_request_issue(id)`(같은 경로로 "이슈로 등록하라" 전달).
- Produces:
  - 화면: 왼쪽 목록(안 읽음 굵게, 상태 배지, `close_proposed` 강조, 완료 숨김 토글 `h`), 오른쪽 상세(제목·담당·출처·경로·확인할 것·리스크·타임라인·이슈 폴더 경로).
  - 키(애플릿 포커스일 때만, 1단계 키 라우터보다 낮은 우선순위 — 앱 단축키는 그대로 동작): `j/k` 이동, `Enter` 상세, `Esc` 목록, `i` 지시(입력 → `inbox_instruct`), `s` 자료 저장(경로 여러 줄 + input/output 선택 → `inbox_add_files`; 복사 후 백엔드가 dispatcher에게 "[AgentBoard] 항목 <id>에 자료 저장: <경로들> — 담당 Manager에게 알려라"를 bridge로 보낸다(스펙 §5); 이슈 없으면 "이슈 먼저 등록" 안내 후 `u`), `u` 이슈 등록 요청, `c` 종료 확인(`close_proposed`만, 확인 대화 Y/N).
  - 순수 로직 `inbox-logic.js`: 정렬(close_proposed → new 안읽음 → in_progress → 기타, 같은 그룹은 최신순), 필터, 상태 라벨(한국어), 키 → 액션 매핑, 버튼 활성 조건.
  - 마우스: 항목 클릭 선택, 버튼 클릭.
- [ ] **Step 1: 실패하는 테스트** (`inbox.test.mjs`) — 정렬, 키 매핑, `c`는 close_proposed에서만, 완료 숨김 토글, XSS 없는 렌더(텍스트만).
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: 수동 확인 항목** — `docs/spikes.md` "2단계 Task 10"에 "사용자 확인 필요"로: 지시 → dispatcher 화면에 전달 확인, 입력 중 dispatcher 입력란 무손상.
- [ ] **Step 6: Commit** — `feat(ui): inbox applet next to dispatcher`

---

### Task 11: 야간 정기 정리

**Files:**
- Create: `src-tauri/src/nightly.rs`
- Modify: `src-tauri/src/workspace.rs`(`WsCommand::RestartPaneFresh { pane }` — kill 후 `(id, inject_prompt=true)` spawn), `src-tauri/src/mcp_tools.rs`(`handoff_done`), `src-tauri/src/lib.rs`(스케줄러 스레드 시작)

**Interfaces:**
- Produces:
  - `pub struct Nightly` 상태 기계 — `Idle → Notifying → Waiting{pending: set<PaneId>, deadline} → Restarting → Idle`. 순수 `step(now, events) -> Vec<Action>`로 구현하고 스레드는 1분 주기로 `step` 호출.
  - 대상: `role ∈ {Dispatcher, Handover, Manager}`인 창. **Lead·Member 제외.**
  - 시작 조건: 오늘 아직 안 했고 현재 시각 ≥ `nightlyRefresh`, 그리고 dispatcher 창 `last_input`이 `nightlyQuietMin` 이내가 아님(아니면 5분 뒤 재시도). 시각이 지난 뒤 앱이 켜져도 그날 1회 수행.
  - 알림 메시지(`bridge::send`): Manager·dispatcher에 "[AgentBoard] 야간 정리: 작업 계획·진행 내역을 갱신하고, 팀 작업을 방해하지 않는 범위에서 저장한 뒤 handover(@handover)에게 파일 위치를 보고하고 handoff_done을 호출하라." handover에는 Manager들 완료 후(또는 deadline 절반 경과 시) "인덱스 포인터를 갱신하고 handoff_done".
  - 완료: 모두 `handoff_done` 또는 `nightlyTimeoutMin` 경과 → 대상 창 `RestartPaneFresh`. 미완료 창은 인박스 시스템 항목 "야간 정리 미완료: <이름>"(owner = 해당 탭, check에 사유).
  - 마지막 수행일은 `workspace.json`과 별개로 `%APPDATA%/agentboard/nightly.json`에 저장.
- [ ] **Step 1: 실패하는 테스트**
```rust
#[test] fn runs_once_per_day_after_time() {}
#[test] fn nightly_defers_while_user_typing_and_skips_teams() {}
#[test] fn restarts_after_all_handoffs_or_timeout() {}
#[test] fn timeout_creates_incomplete_inbox_items() {}
#[test] fn handover_notified_after_managers() {}
```
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: Commit** — `feat(nightly): scheduled nightly handoff and fresh sessions`

---

### Task 12: 시작 절차 · 복원 규칙 · 자동 실행 · 버전 확인

**Files:**
- Create: `src-tauri/src/autostart.rs`
- Modify: `src-tauri/src/persist.rs`(`prepare_restored`), `src-tauri/src/lib.rs`, `src/overlay-logic.js`(팔레트 "로그인 시 자동 실행 켜기/끄기"), `src/overlays.js`/`overlay-logic.js`(새 탭 마법사에 역할 선택: 일반 / Manager)

**Interfaces:**
- Produces:
  - 복원 규칙(스펙 §9): role 없는 탭 = 1단계 그대로. role 탭 = 레이아웃·위치 복원, 팀 창(lead/member)·팀 영역 **제거**, 상시 창은 `inject_prompt=true` 새 세션. 순수 함수 `restore_plan(ws) -> (Workspace, Effects)` + 테스트.
  - `autostart::{enable, disable, is_enabled}` — `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 값 `AgentBoard` = `"<exe>"`. Tauri 명령 `autostart_get/set`.
  - 시작 시 `claude --version` 확인(타임아웃 5초) → 미달·실패 시 시작 오류 목록에 "Claude Code 2.1.234 이상이 필요합니다(현재 x)".
- [ ] **Step 1: 실패하는 테스트** — `restore_plan_drops_team_panes_and_reinjects_prompts`, `non_role_tabs_restore_as_phase1`, `version_parse_and_compare`, autostart는 레지스트리 키 이름을 주입 가능하게 해 테스트 키로 왕복.
- [ ] **Step 2–4** — FAIL → 구현 → PASS
- [ ] **Step 5: Commit** — `feat: role-aware restore, autostart toggle, Claude version check`

---

### Task 13: 기본 역할 지시·템플릿 · 문서 · 2단계 QA

**Files:**
- Create: `src-tauri/defaults/roles/{dispatcher,handover,manager,lead,member}.md`, `src-tauri/defaults/templates/{dispatcher-inbox,handover,manager}.json`
- Modify: `src-tauri/src/config.rs`(ensure_dotdir가 roles/·새 템플릿 복사, 덮어쓰지 않음), `README.md`, `History.md`, `docs/spikes.md`

**Interfaces:**
- Produces (역할 지시 최소판 — 스펙 §2·§5·§7·§8·§9 반영, 각 1–2쪽):
  - 공통: 사용자 결정이 필요하면 질문을 화면에 쓰고 `need_user(사유)` 호출, `[AgentBoard]`·agentboard 발신 메시지는 사용자 지시를 전달하는 것(R3), 세션 이름 규칙, `SendMessage`로 보낼 대상 규칙(경로: handover→dispatcher→Manager, 지시는 dispatcher 경유, 팀원→리드만), `[AgentBoard]` 메시지는 앱 지시로 취급.
  - dispatcher: handover 메시지 판단·담당 전달, 인박스 지시 전달, `create_manager_tab` 사용 조건.
  - handover: `/loop`는 템플릿 프롬프트가 시작, 인덱스 문서는 **포인터만**(업무·계획 파일·진행 파일·갱신 시각 한 줄), 야간 정리 절차.
  - manager: 보고 = `inbox_post`(확인할 것/리스크 짧고 명료), 종료 = `inbox_propose_close` + 최종 의견, 이슈 = `issue_create`, 팀 = `teams/<id>/brief.md`·`roster.md` 작성 후 `hire_lead`, 끝난 팀 brief에 `상태: 완료`, **시작 시 진행 중 팀 자동 재채용**, 야간 정리 절차.
  - lead: brief 기준 배분, 팀원 결과물 **검증·지적**, `hire_member`/`dismiss`, Manager에게 보고.
  - member: 리드에게만 보고.
  - 템플릿: `dispatcher-inbox` = 좌 dispatcher(시작 프롬프트: 인덱스 리뷰) | 우 inbox 애플릿; `handover` = 단일 창(시작 프롬프트 예시 `/loop 10m <확인 작업>` — 사용자가 수정); `manager` = Manager 창 + `slot: "team"` 영역.
- [ ] **Step 1: 테스트** — `ensure_dotdir_copies_roles_and_new_templates_without_overwrite`, 새 템플릿 3개가 검증 통과, 역할 파일 로딩.
- [ ] **Step 2: 문서** — README 2단계 절(설정 예, 역할, 인박스 키, 야간 정리, 자동 실행, Claude 버전 요구), History V0.2.0.
- [ ] **Step 3: 전체 테스트·릴리스 빌드** — `cargo test`, `npm run test:js`, `npx tauri build`.
- [ ] **Step 4: 수동 QA 목록** — `docs/spikes.md` "2단계 QA"에 사용자 확인 항목: 아침 시작 → handover 전달 → Manager 인박스 항목 → 지시 → 종료 제안 → 종료 확인, 팀 채용·해산·여러 팀, 재부팅 후 팀 자동 재채용, 야간 정리(시각을 가깝게), 권한 대기 ⚠, 입력 중 dispatcher 입력란 무손상.
- [ ] **Step 5: Commit** — `feat: default role prompts and templates, phase 2 docs and QA`
