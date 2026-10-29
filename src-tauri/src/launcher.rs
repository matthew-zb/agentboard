//! Launcher: turns model `Effects` into PTY processes, injects first prompts once the agent
//! is ready, and maps PTY exits back onto pane status.
//!
//! # Lock order (deadlock rules)
//!
//! App locks: `AppState::ws` (Mutex), `AppState::loaded` (RwLock, read-only except `create_manager_tab`),
//! `AppState::prompts` (Mutex), `AppState::effects` (Mutex<Sender>), `AppSink::channels`
//! (Mutex, reached through `AppState::ui` / the managed `Arc<AppSink>`). PtyManager-internal
//! locks: per-pane `flush_lock`, the `panes` map, writer/master/killer, the per-pane input
//! queue (`InputQueue`).
//!
//! * Allowed nestings (outer -> inner): `ws -> loaded(read)`, `ws -> prompts`,
//!   `ws -> PtyManager::is_running/current_gen/job_error` (only the `panes` map lock /
//!   job_error lock + an atomic; never blocks on a process), `flush_lock -> panes`
//!   (sink `output` checks `current_gen`; `attach_with` re-checks the generation),
//!   `flush_lock -> prompts` (sink `output` feeds the ReadyDetector), `flush_lock -> channels`
//!   (sink `output` and `pty_attach`), `channels -> panes` (`ChannelMap::remove_if_current`
//!   on sink `exited`). The `panes` map lock is a leaf: PtyManager never takes `flush_lock`
//!   or any app lock while holding it.
//! * PtyManager `spawn/kill/remove` are called only from the launcher worker thread
//!   (`run_effects`); MCP / nightly / other threads must go through `execute` → `submit`
//!   (R13). `write`/`resize`/`attach_with` may be called from command threads.
//! * Input queue (pty.rs): `write`/`resize` take the `panes` map lock (released), then the
//!   pane's queue lock — a leaf held only to push, never across process IO — and return at
//!   once (`Ok` = queued). Each pane's writer thread pops under the queue lock, releases it,
//!   then takes the pane's writer (per chunk) or master lock to apply the op; it takes no app
//!   lock and never `flush_lock`. The reader thread queues the DSR reply (queue lock only).
//!   The waiter closes the queue, then takes writer → master to drop them. So `write` and
//!   `resize` never block on a process and may be called under any app lock; IPC order of
//!   `pty_write` / `pty_write_raw` / `pty_resize` (sync commands on the UI thread) is the
//!   order the PTY sees.
//!
//! # Spawn generations
//!
//! Every `PtyManager::spawn` attempt gets a fresh generation (manager-global u64, from 1,
//! never reused) and registers it under `panes` before the process starts; the PTY threads
//! tag `output`/`exited` with it. `current_gen(id)` is the latest registration — `None`
//! after `remove` or a failed spawn. `on_output`/`on_exited` ignore any event whose
//! generation is not current (a late exit of a restarted/retired process never touches the
//! model, prompts or the frontend channel). Prompt jobs carry the generation they were
//! spawned for (`PromptJob::bind`, unbound while their spawn is in progress): output feeds
//! and exits remove only a matching job, and `poll_prompts` writes only into that
//! generation. AppSink drops the output channel / emits `pty-exit` only if the generation is
//! still current, re-checked under the channel lock. A restart whose plan fails retires the old
//! generation (`remove`) before writing `Failed`, so the message survives the old exit. If
//! the old process will not die within `KILL_WAIT`, its generation stays current: when it
//! finally exits, `Exited` replaces the "won't terminate" failure (truthful; restartable).
//! * `ws -> emit_lock` (R16): `publish` stamps a version under `ws`, takes `emit_lock`,
//!   releases `ws`, emits, then releases `emit_lock` — emits go out in version order and
//!   never while `ws` is held.
//! * `Saver` (persist.rs): `notify` is a channel send (leaf); `publish` calls it while still
//!   holding `emit_lock` (emit_lock -> saver channel; nothing is acquired inside). Its thread takes `ws` only to
//!   clone, releases it, then writes the file — never `ws` during IO, never `emit_lock`.
//! * `prompts`, `channels`, `effects` and `emit_lock` are leaves: nothing is acquired while
//!   holding them.
//! * `AppState::tokens` (the `Tokens` map lock) and `AppState::ctrl_url` are leaves too.
//!   `spawn_pane` takes them one at a time (issue the role pane's token, read the URL) before
//!   `loaded(read)`; `plan_spawn` then writes the pane's run files under `loaded(read)` only
//!   (small local files, no other lock, never under `ws`).
//! * Control server (control.rs): `AppState::hooks` (HookRouter map) is a leaf. The caller of a request is computed by taking `tokens`
//!   (released) and then `ws` (released); nothing is held across an `.await`. Tool bodies and
//!   hook handlers run on blocking threads and follow the rules here (model changes only via
//!   `execute`, R13).
//! * Inbox (inbox.rs, Task 6): `AppState::inbox`'s item lock is a leaf apart from
//!   `inbox -> inbox emit lock` (R16 pattern: the item file is written and the version bumped
//!   under the item lock, the emit lock is taken, the item lock released, then
//!   `Ui::inbox_changed` emits). Never take `ws`/`loaded` while holding it: MCP tools and
//!   commands read tab data from `ws` first (released), then call the inbox. `add_files`
//!   copies with no lock held.
//! * Bridge (bridge.rs, Task 7): `Bridge::inner` (registrations + queues), `Bridge::wake`
//!   (worker condvar) and `AppState::last_input` are leaves. `flush_lock -> bridge inner`
//!   (sink `output` → `bridge::on_output`, after `prompts` is released). `Bridge::pump_lock`
//!   is taken only by `bridge::pump` (worker thread / tests) and is outermost: under it `ws`,
//!   `loaded(read)`, the `panes` map (`current_gen`), `last_input`, `inner` and the inbox are
//!   taken one at a time, each released before the next; the pipe write and the PTY fallback
//!   `PtyManager::write` run holding only `pump_lock` (R13: write is allowed off the worker).
//!   Hook handlers take only `panes` (released) then `inner`/`wake`; they never write pipes.
//!   Inbox relay (Task 10, `bridge::relay_*` from Tauri commands): `ws` (released), then the
//!   inbox, then `inner`/`wake` via `send` — one at a time, never nested.
//! * Attention (attention.rs, Task 9): `AppState::trust` (folder-trust timers) is a leaf.
//! * Board session poll (board.rs, D3b, its own thread every 3 s): `ws` (released), then the
//!   PtyManager `panes` map per pane (released), then process snapshots / session-file and
//!   transcript reads with no app lock (the context cache lock alone around a read), then the
//!   board lock alone to store; `board-changed` after, with no lock held.
//! * Board (board.rs): `AppState::board` (entries) and its context cache are leaves. Hook
//!   handlers take the PtyManager `panes` map (`current_gen`, released), then the board lock
//!   (released), then on Stop the cache lock alone around the transcript tail read, then the
//!   board lock again; `board::status` takes the nightly machine lock, the board lock and
//!   `panes` one at a time. `nightly::tick` reads the machine's view before and after `step`
//!   under the machine lock and emits `board-changed` after `perform`, with no lock held.
//! * Focus mode (focus.rs): `AppState::focus` is a leaf, held only around the mode / the held
//!   `need_user` list and the focus.json write. `focus::set` releases it before telling the
//!   dispatcher (bridge), re-showing ⚠ (`execute`) and emitting; `need_user` checks it before
//!   `execute`, never inside.
//! * Memo (memo.rs): `AppState::memo` (`MemoState.last`) is a leaf, held across the MEMO.md /
//!   memo.bak IO (reads and saves are serialized); `memo_read`/`memo_save` read workspaceRoot
//!   from `loaded` (released) first and run on a blocking thread.
//!   `spawn_pane` arms it after `spawn` returns (no app lock held); `poll_trust` (prompt timer
//!   thread) and the SessionStart / Notification handlers take it, release it, then read
//!   `current_gen` / `ws` one at a time and change the model only via `execute`.
//! * Documents (docs.rs, Task 14): `AppState::docs` (watch map) is a leaf. `doc_watch` takes
//!   `ws` (released), then `loaded(read)` (released), then the map; the watcher thread takes
//!   only the map (released before any file system call and before emitting `doc-changed`).
//!   `OpenDoc` validates the path under `ws -> loaded(read)` (file system metadata only).
//! * Nightly (nightly.rs, Task 11): the machine lock and the wake flag of `AppState::nightly`
//!   are leaves (`handoff_done` takes the machine, releases it, then wakes). The scheduler
//!   thread takes `loaded(read)`, `ws`, `last_input` and the machine one at a time (each
//!   released) and performs its actions (`bridge::send` / `discard`, `execute`, the inbox,
//!   the `nightly.json` write) with no lock held. Its spawn-time map is a leaf too
//!   (`spawn_pane` writes it after `spawn` returns; `run_effects` removes it for a gone pane).
//! * `loaded(write)` (Task 8): taken only by the MCP tool `create_manager_tab`, alone — no other
//!   lock held, none taken inside — around the atomic config.json append and the in-memory
//!   `config.tabs` push (and their rollback). It is never taken under `ws`; `execute` runs
//!   after it is released. Team tools (`hire_*`, `dismiss*`) read `ws` briefly (released) and
//!   change the model only through `execute`. `config::CONFIG_WRITE` (static Mutex) is
//!   outermost: `create_manager_tab` holds it across append → `execute` → rollback and takes
//!   `loaded(write)` / `ws` only inside it, one at a time; nothing takes it while holding an
//!   app lock. Task 12: `env_skip` / `env_set_source` (MCP, dispatcher) take `CONFIG_WRITE`,
//!   then `loaded(write)` alone around the atomic config.json edit and the in-memory update,
//!   then emit `env-changed` after releasing both. The new-tab wizard's Manager choice
//!   (`create_manager_tab` Tauri command) runs the same code path as the MCP tool.
//!   R27 (fix round 1): `execute` of `RenameTab` / `CloseTab` takes `CONFIG_WRITE` itself, then
//!   reads the tab (`ws`, released); for a Manager / handover tab: dry run under
//!   `ws -> loaded(read)`, released; config.json edit under `loaded(write)` alone; (rename, R29)
//!   the inbox owner names under the inbox lock alone; then the model; rollback on failure. So
//!   no code may call `execute` with one of those commands while holding `CONFIG_WRITE`
//!   (nothing does: `create_manager_tab` only executes `CreateTab` under it). Before a
//!   `CloseTab`, still under `CONFIG_WRITE`, `removal_blockers` reads `ws` (released), then the
//!   inbox (leaf), then the tab's `teams/` folder with no lock: none nested.
//! * Config reload (`reload_config_from`): `CONFIG_WRITE` → `load_all` with no app lock →
//!   `ws` → `loaded(read)` (released) → `loaded(write)` while `ws` is still held (the allowed
//!   `ws -> loaded` direction; nothing takes `ws` while holding `loaded`) → both released, then
//!   `submit` / `publish` / Ui emits.
//! * Environment check (envcheck.rs, Task 12): `AppState::env` (probe results) and its
//!   refresh lock are leaves; the probe processes run holding only the refresh lock. Reading
//!   items takes `ws` (clone, released), `loaded(read)`, and `trust` → PtyManager `panes`
//!   (`attention::trust_timed_out`), never nested with another app lock.
//! * Never call `PtyManager::spawn/kill/remove/write/resize/attach_with` while holding `ws`,
//!   `loaded` or `prompts`: spawn/kill/write can block, and the PTY threads call back into
//!   the sink (`output` -> prompts, `exited` -> ws).
//! * Never emit events while holding `ws`: snapshots are cloned under the lock, emitted after.
//!
//! Commands mutate the model under `ws` (apply), release it, and hand the `Effects` to a
//! single worker thread (`submit`), which runs `run_effects` in order without app locks held
//! across PtyManager calls.

use crate::agentcfg::PLACEHOLDER_CTRL_URL;
use crate::config::{self, Loaded};
use crate::roles::Role;
use crate::profiles::{encode_submit, AgentProfile, ReadyDetector, READY_TIMEOUT};
use crate::pty::SpawnSpec;
use crate::workspace::{Effects, Pane, PaneStatus, TabId, Workspace, WsCommand};
use crate::{AppState, Snapshot};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

pub const START_COLS: u16 = 80;
pub const START_ROWS: u16 = 24;
pub const WORKSPACE_CHANGED: &str = "workspace-changed";
const PROMPT_POLL: Duration = Duration::from_millis(200);
/// How long a restart waits for the killed process before giving up.
const KILL_WAIT: Duration = Duration::from_secs(3);

// ───────────────────────── pure parts ─────────────────────────

/// Profile used for `pane`: its own agent, else `config.defaultAgent`.
pub fn profile_for<'a>(pane: &Pane, loaded: &'a Loaded) -> Option<&'a AgentProfile> {
    loaded.profiles.get(&pane.agent).or_else(|| loaded.profiles.get(&loaded.config.default_agent))
}

/// Per-spawn inputs of a role pane's extras (`agentcfg::role_spawn_extras`; ignored for panes
/// without a role) and of every pane's inherited-environment cleanup.
pub struct RoleLaunch<'a> {
    pub ctrl_url: &'a str,
    pub token: &'a str,
    pub run_dir: &'a std::path::Path,
    /// Variable names of the app's own environment (every pane). tmux/psmux markers are passed
    /// on to no pane (`agentcfg::MULTIPLEXER_VARS`); session-bound Claude Code variables not to
    /// a role session (`agentcfg::SESSION_BOUND_VARS`, R19). Injected for testability.
    pub parent_env: &'a [String],
}

/// Core of spawning: profile lookup, cwd check, command resolution, args/env merge.
/// The working folder is checked before the command so the user sees the real cause.
pub fn plan_spawn(pane: &Pane, loaded: &Loaded, launch: &RoleLaunch) -> Result<SpawnSpec, String> {
    if pane.kind.is_applet() {
        return Err("애플릿 창은 프로세스를 실행하지 않습니다".into());
    }
    let profile = profile_for(pane, loaded).ok_or_else(|| {
        format!("프로필을 찾을 수 없습니다: {} (기본값 {})", pane.agent, loaded.config.default_agent)
    })?;
    if !pane.cwd.is_dir() {
        return Err(format!("작업 폴더가 없습니다: {}", pane.cwd.display()));
    }
    let cmd = crate::profiles::resolve_cmd(&profile.cmd)
        .ok_or_else(|| format!("명령을 찾을 수 없습니다: {} (프로필 {})", profile.cmd, profile.name))?;
    // profile args, then (role panes) the role extras and `roles.<role>.args`, then pane args
    let mut args: Vec<String> = profile.args.clone();
    let mut env = profile.env.clone();
    let mut role_env = Vec::new();
    // every pane: not a tmux pane, even when the app was started from a psmux shell
    let mut env_remove = crate::agentcfg::inherited_multiplexer_vars(launch.parent_env.iter().cloned());
    if let Some(role) = pane.role {
        let (mut extra_args, extra_env) = crate::agentcfg::role_spawn_extras(
            pane,
            role,
            loaded,
            launch.ctrl_url,
            launch.token,
            launch.run_dir,
        )?;
        // R18: a .cmd/.bat shim runs through cmd.exe, which stops at the first newline — the
        // prompt (the last extra) becomes one line there.
        if crate::agentcfg::is_batch(&cmd) {
            if let Some(prompt) = extra_args.last_mut() {
                *prompt = crate::agentcfg::single_line(prompt);
            }
        }
        args.extend(extra_args);
        args.extend(loaded.config.role_config(role).args);
        role_env = extra_env;
        let mut drop = crate::agentcfg::inherited_claude_vars(launch.parent_env.iter().cloned());
        drop.append(&mut env_remove);
        env_remove = drop;
    }
    args.extend(pane.args.iter().cloned());
    env.extend(pane.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env.extend(role_env); // the app's identity variables win over profile/pane env
    Ok(SpawnSpec {
        cmd,
        args,
        env: env.into_iter().collect(),
        env_remove,
        cwd: pane.cwd.clone(),
        cols: START_COLS,
        rows: START_ROWS,
    })
}

/// First-prompt injection state machine for one pane (time is passed in, so it is testable).
///
/// Phase-1 panes ([`PromptJob::new`]): ready by the profile's `ready` rule, or after
/// `READY_TIMEOUT` with the header warning. Role panes ([`PromptJob::new_gated`], R28 / final
/// review #2): nothing happens until [`PromptJob::release`] — the SessionStart hook of the
/// job's generation ([`release_prompt`]). Before that the screen may be Claude's folder-trust
/// dialog, which is idle and would take the prompt's Enter as an answer; without a SessionStart
/// the prompt is never typed and the R5 trust ⚠ covers the wait. After the release a fresh
/// detector applies (idle counted from the release) with `READY_TIMEOUT` from the release.
pub struct PromptJob {
    detector: ReadyDetector,
    ready: crate::profiles::Ready,
    deadline: Instant,
    payload: Vec<u8>,
    done: bool,
    /// Generation of the spawn this job belongs to; `None` while that spawn has not returned
    /// yet (the job is registered before `PtyManager::spawn` so the first output is seen).
    gen: Option<u64>,
    gate: Gate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// Phase-1 behaviour.
    Open,
    /// Role pane, waiting for SessionStart.
    Waiting,
    /// Role pane, SessionStart seen.
    Released,
}

impl PromptJob {
    pub fn new(profile: &AgentProfile, prompt: &str, start: Instant) -> Self {
        PromptJob {
            detector: ReadyDetector::new(&profile.ready, start),
            ready: profile.ready.clone(),
            deadline: start + READY_TIMEOUT,
            payload: encode_submit(prompt, &profile.submit),
            done: false,
            gen: None,
            gate: Gate::Open,
        }
    }

    /// A role pane's start prompt: typed only after [`PromptJob::release`].
    pub fn new_gated(profile: &AgentProfile, prompt: &str, start: Instant) -> Self {
        PromptJob { gate: Gate::Waiting, ..PromptJob::new(profile, prompt, start) }
    }

    pub fn is_gated(&self) -> bool {
        self.gate != Gate::Open
    }

    pub fn is_released(&self) -> bool {
        self.gate == Gate::Released
    }

    /// SessionStart of this job's generation: start the readiness check now (fresh detector,
    /// idle counted from `now`, deadline `now + READY_TIMEOUT`). Repeated calls are no-ops.
    pub fn release(&mut self, now: Instant) {
        if self.gate != Gate::Waiting {
            return;
        }
        self.gate = Gate::Released;
        self.detector = ReadyDetector::new(&self.ready, now);
        self.detector.seed(now);
        self.deadline = now + READY_TIMEOUT;
    }

    pub fn gen(&self) -> Option<u64> {
        self.gen
    }

    /// Bind the job to the generation its spawn returned.
    pub fn bind(&mut self, gen: u64) {
        self.gen = Some(gen);
    }

    /// Whether output/exit of `gen` concerns this job. An unbound job accepts output of the
    /// current generation only (callers check that first): it can only be its own spawn's,
    /// because the previous process's final output precedes its `is_running == false`, which
    /// `spawn_pane` waits for before registering the job.
    fn owns(&self, gen: u64) -> bool {
        self.gen.map_or(true, |g| g == gen)
    }

    /// Output from the pane (called on the PTY flush path: must stay cheap). A gated job
    /// ignores output until it is released (the detector is replaced then anyway).
    pub fn feed(&mut self, bytes: &[u8], now: Instant) {
        if !self.done && self.gate != Gate::Waiting {
            self.detector.feed(bytes, now);
        }
    }

    /// `Some((bytes, warning))` exactly once: when ready (`warning=false`) or when
    /// `READY_TIMEOUT` has passed without readiness (`warning=true`).
    pub fn poll(&mut self, now: Instant) -> Option<(Vec<u8>, bool)> {
        if self.done || self.gate == Gate::Waiting {
            return None;
        }
        let warning = if self.detector.poll(now) {
            false
        } else if now >= self.deadline {
            true
        } else {
            return None;
        };
        self.done = true;
        Some((std::mem::take(&mut self.payload), warning))
    }
}

/// Tabs whose working directory does not exist: `(tab id, tab.dir)`.
pub fn missing_dirs(ws: &Workspace) -> Vec<(TabId, String)> {
    ws.tabs.values().filter(|t| !t.workspace.is_dir()).map(|t| (t.id.clone(), t.dir.clone())).collect()
}

/// Longest folder name (characters), the usual Windows component limit.
pub const DIR_NAME_MAX: usize = 255;
/// Characters Windows does not allow in a file or folder name.
const DIR_NAME_FORBIDDEN: &str = "<>:\"/\\|?*";
/// Folders the app (`.agentboard`) and git keep for themselves (case-insensitive like NTFS).
const DIR_NAME_RESERVED: [&str; 2] = [".agentboard", ".git"];

/// True for a Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM0`–`COM9`, `LPT0`–`LPT9`,
/// `COM¹²³`, `LPT¹²³`), case-insensitive, with or without an extension (`con.txt`, `aux.tar.gz`).
fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) {
        return true;
    }
    let digit = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"));
    let mut chars = digit.unwrap_or_default().chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_digit() || "¹²³".contains(c))
}

/// Validation of one folder name the app creates or records for a new tab
/// (`create_workspace_dir`, and each part of [`check_rel_dir`]). Besides `/ \ :` and `.`/`..`,
/// refuses what Windows would reject or silently change: forbidden and control characters,
/// leading/trailing spaces and a trailing dot (Win32 strips them, so `billing.` would name the
/// existing `billing`), device names, over-long names, and `.agentboard` / `.git`.
/// Not applied to folders already in config.json (they load as before).
pub fn check_dir_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("폴더 이름이 비어 있습니다".into());
    }
    if name.chars().count() > DIR_NAME_MAX {
        return Err(format!("폴더 이름이 너무 깁니다(최대 {DIR_NAME_MAX}자)"));
    }
    if name.chars().any(char::is_control) {
        return Err("폴더 이름에 제어 문자(줄바꿈·탭 등)는 쓸 수 없습니다".into());
    }
    if let Some(c) = name.chars().find(|c| DIR_NAME_FORBIDDEN.contains(*c)) {
        return Err(format!("폴더 이름에 {c} 는 쓸 수 없습니다 (< > : \" / \\ | ? * 불가)"));
    }
    if name == "." || name == ".." {
        return Err("폴더 이름으로 . 이나 .. 은 쓸 수 없습니다".into());
    }
    if name != name.trim() || name.ends_with('.') {
        return Err("폴더 이름의 앞뒤 공백과 끝의 점(.)은 쓸 수 없습니다 — Windows가 지워 다른 폴더를 가리키게 됩니다".into());
    }
    if is_device_name(name) {
        return Err(format!("'{name}'은(는) Windows 장치 이름이라 폴더 이름으로 쓸 수 없습니다"));
    }
    if DIR_NAME_RESERVED.iter().any(|r| r.eq_ignore_ascii_case(name)) {
        return Err(format!("'{name}'은(는) 앱·git이 쓰는 폴더라 쓸 수 없습니다"));
    }
    Ok(())
}

/// Validation of a new work tab's folder, relative to workspaceRoot (MCP `create_manager_tab`
/// and the wizard's Manager choice): `/`- or `\`-separated parts, each a valid
/// [`check_dir_name`] — so no `.`/`..`, no empty part, nothing absolute.
pub fn check_rel_dir(dir: &str) -> Result<(), String> {
    if dir.is_empty() {
        return Err("폴더 경로가 비어 있습니다".into());
    }
    for part in dir.split(['/', '\\']) {
        if part.is_empty() {
            return Err("경로에 빈 부분이 있습니다(// 또는 앞뒤의 /)".into());
        }
        check_dir_name(part).map_err(|e| if part == dir { e } else { format!("'{part}': {e}") })?;
    }
    Ok(())
}

// ───────────────────────── runtime ─────────────────────────

/// Config errors as display lines, plus the job object problem (if any) as a non-file line.
pub fn error_lines(state: &AppState) -> Vec<String> {
    let mut v: Vec<String> = state.loaded.read().unwrap().errors.iter().map(|e| e.to_string()).collect();
    v.extend(state.startup_errors.iter().cloned());
    if let Some(e) = state.pty.job_error() {
        v.push(format!("프로세스 관리(Job Object) — {e}"));
    }
    v
}

/// Pure: a snapshot stamped with the next version. Callers hold the `ws` lock (R16), so
/// version order equals model order.
pub fn versioned(ws: &Workspace, counter: &AtomicU64, errors: usize) -> Snapshot {
    let version = counter.fetch_add(1, Ordering::SeqCst) + 1;
    Snapshot { workspace: ws.clone(), errors, version }
}

/// Versioned snapshot of the model, not emitted (`ws_snapshot`).
pub fn snapshot(state: &AppState) -> Snapshot {
    let errors = error_lines(state).len();
    let ws = state.ws.lock().unwrap();
    versioned(&ws, &state.version, errors)
}

/// Take a versioned snapshot and emit it as `workspace-changed`; returns exactly what was
/// emitted. `emit_lock` is acquired before `ws` is released (ws -> emit_lock), so emits
/// leave in version order while the emit itself runs without `ws` held.
pub fn publish(state: &AppState) -> Snapshot {
    let errors = error_lines(state).len();
    let ws = state.ws.lock().unwrap();
    let snap = versioned(&ws, &state.version, errors);
    let _emit = state.emit_lock.lock().unwrap();
    drop(ws);
    state.saver.notify(); // every model change is published; the saver debounces (500ms)
    state.ui.workspace_changed(&snap);
    snap
}

/// The one command path (Tauri `ws_command`, phase-2 MCP tools): apply under `ws` (+ `loaded`
/// read), release both, queue the effects for the launcher worker, then publish. Returns the
/// exact snapshot that was emitted. A rejected command changes nothing, emits nothing.
pub fn execute(state: &AppState, cmd: WsCommand) -> Result<Snapshot, String> {
    match cmd {
        WsCommand::RenameTab { .. } | WsCommand::CloseTab { .. } => {
            // CONFIG_WRITE first, then the tab's name/dir are read: two concurrent renames of
            // the same role tab cannot both act on the name read before the other's edit
            let _cfg = config::CONFIG_WRITE.lock().unwrap_or_else(|e| e.into_inner());
            // decision c (④): MCP and UI closes both pass here, before anything changes
            if let WsCommand::CloseTab { tab } = &cmd {
                removal_blockers(state, tab)?;
            }
            match role_tab_config_edit(state, &cmd) {
                Some((name, dir, edit)) => execute_with_config(state, cmd, &name, &dir, &edit),
                None => execute_model(state, cmd),
            }
        }
        _ => execute_model(state, cmd),
    }
}

/// Decision c (remove-tab spec §2): a Manager tab is not closed while it has unfinished inbox
/// items (owner = its name, state ≠ done — a closing proposal too) or a running team: a team
/// pane (lead/member) in the tab, or a `teams/<id>/brief.md` whose first line is not
/// `상태: 완료` (catches a team between a restart and the Manager's re-hire). The Korean refusal
/// lists both kinds and how to finish them. Other tabs (plain, handover; the Dispatcher tab is
/// refused by the model) pass. Folders, issues, teams and done items are never touched here.
pub fn removal_blockers(state: &AppState, tab: &str) -> Result<(), String> {
    let (name, workspace, live) = {
        let ws = state.ws.lock().unwrap();
        let Some(t) = ws.tabs.get(tab).filter(|t| t.role == Some(Role::Manager)) else { return Ok(()) };
        // team id -> a lead is among its panes
        let mut live: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
        for p in ws.panes.values().filter(|p| p.tab == tab) {
            if let Some(team) = &p.team {
                *live.entry(team.clone()).or_default() |= p.role == Some(Role::Lead);
            }
        }
        (t.name.clone(), t.workspace.clone(), live)
    };
    let items = state.inbox.open_count_by_owner(&name);
    let mut teams: std::collections::BTreeMap<String, String> =
        live.into_iter().map(|(id, lead)| (id, if lead { "리드 실행 중" } else { "팀원 실행 중" }.to_string())).collect();
    for (id, line) in crate::teams::briefs_not_done(&workspace) {
        teams.entry(id).or_insert_with(|| format!("brief 첫 줄 '{line}'"));
    }
    if items.total() == 0 && teams.is_empty() {
        return Ok(());
    }
    let mut parts = Vec::new();
    if items.total() > 0 {
        let kinds: Vec<String> = [("새", items.new), ("진행 중", items.in_progress), ("종료 제안", items.close_proposed)]
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        parts.push(format!("끝나지 않은 인박스 항목 {}개({})", items.total(), kinds.join("·")));
    }
    if !teams.is_empty() {
        let list: Vec<String> = teams.iter().map(|(id, why)| format!("{id}: {why}")).collect();
        parts.push(format!("진행 중인 팀 {}개({})", teams.len(), list.join(", ")));
    }
    let fix = match (items.total() > 0, !teams.is_empty()) {
        (true, true) => "항목을 완료(종료 확인)하고, 팀은 brief 첫 줄을 '상태: 완료'로 바꾼 뒤 해산(dismiss_team)하고 다시 시도하세요.",
        (true, false) => "항목을 완료(종료 확인)하고 다시 시도하세요.",
        _ => "팀은 brief 첫 줄을 '상태: 완료'로 바꾼 뒤 해산(dismiss_team)하고 다시 시도하세요.",
    };
    Err(format!("'{name}' 상시업무를 닫지 않았습니다 — {}. {fix}", parts.join(", ")))
}

/// R27: renaming / closing a Manager or handover tab also edits its config.json `tabs[]`
/// entry (config is the source of truth for role tabs). `(name, dir, edit)` of that entry, or
/// `None` for every other command. Takes `ws` briefly (released).
fn role_tab_config_edit(state: &AppState, cmd: &WsCommand) -> Option<(String, String, config::TabEntryEdit)> {
    let (tab, edit) = match cmd {
        WsCommand::RenameTab { tab, name } => (tab, config::TabEntryEdit::Rename(name.clone())),
        WsCommand::CloseTab { tab } => (tab, config::TabEntryEdit::Remove),
        _ => return None,
    };
    let ws = state.ws.lock().unwrap();
    let t = ws.tabs.get(tab)?;
    matches!(t.role, Some(Role::Manager | Role::Handover)).then(|| (t.name.clone(), t.dir.clone(), edit))
}

/// Same pattern as `create_manager_tab`, called with `CONFIG_WRITE` held (by `execute`): dry
/// run on a copy of the model, then config.json (atomic, other keys kept) + the loaded config,
/// then (rename, R29) the owner name of the tab's inbox items, then the model; a failed step
/// puts the earlier ones back. A config or inbox write failure refuses the command (Korean
/// error, nothing changed). Lock order: CONFIG_WRITE → (`ws`, `loaded`) one at a time →
/// inbox (leaf), never nested.
fn execute_with_config(state: &AppState, cmd: WsCommand, name: &str, dir: &str, edit: &config::TabEntryEdit) -> Result<Snapshot, String> {
    {
        let ws = state.ws.lock().unwrap();
        let loaded = state.loaded.read().unwrap();
        ws.clone().apply(cmd.clone(), &loaded)?;
    }
    let what = if matches!(edit, config::TabEntryEdit::Remove) { "을(를) 닫지" } else { "의 이름을 바꾸지" };
    let (path, prev_text, prev_cfg) = {
        let mut l = state.loaded.write().unwrap();
        let path = config::config_path(&l.settings.workspace_root);
        let res = config::edit_tab_entry(&path, name, dir, edit)
            .map_err(|e| format!("config.json을 고칠 수 없어 탭 '{name}'{what} 않았습니다 — {e}"))?;
        let prev_cfg = l.config.clone();
        l.config.apply_tab_entry_edit(name, dir, edit);
        let prev_text = match res {
            config::ConfigEdit::Written(prev) => Some(prev),
            config::ConfigEdit::Unchanged => None,
        };
        (path, prev_text, prev_cfg)
    };
    let rollback_config = |res: Result<Snapshot, String>| -> Result<Snapshot, String> {
        state.loaded.write().unwrap().config = prev_cfg.clone();
        if let Some(prev) = &prev_text {
            if let Err(re) = config::restore_config_file(&path, prev.as_deref()) {
                return res.map_err(|e| format!("{e} (config.json을 되돌리지 못했습니다: {re})"));
            }
        }
        res
    };
    // R29: the tab's inbox items follow the new name (all or nothing)
    let undo = match edit {
        config::TabEntryEdit::Rename(new) => match state.inbox.rename_owner(name, new) {
            Ok(u) => u,
            Err(e) => return rollback_config(Err(format!("인박스 항목의 담당을 바꿀 수 없어 탭 '{name}'의 이름을 바꾸지 않았습니다 — {e}"))),
        },
        config::TabEntryEdit::Remove => Vec::new(),
    };
    let res = execute_model(state, cmd);
    if res.is_err() {
        state.inbox.rename_owner_undo(&undo);
        return rollback_config(res);
    }
    state.ui.env_changed(); // folder-trust ids follow the tab
    if matches!(edit, config::TabEntryEdit::Remove) {
        state.inbox.touch(); // its items lost their owner: the applet re-reads
    }
    res
}

/// Config reload without restarting anything (`config_reload`, palette and dispatcher MCP):
/// `load_all` again (settings, config.json, templates, agent profiles, keybindings, role
/// settings). Any validation error — or a moved workspaceRoot, or a config role tab that cannot
/// be opened — returns the messages (`file:line:col: …` like the status bar) and changes
/// nothing. Otherwise the model is updated (`Workspace::reload_roles`: missing role tabs added,
/// standing prompts refreshed, tabs gone from config only reported) and `loaded` replaced
/// together; the added tabs' panes start; the frontend is told (`config-reloaded`: re-read
/// keybindings / errors; `env-changed`: folder-trust items follow the tabs).
/// Lock order: CONFIG_WRITE (no config edit in between) → `ws` → `loaded(write)` (the existing
/// `ws -> loaded` nesting), both released before effects / publish.
/// [`reload_config_from`] for the running app (Tauri command and MCP `config_reload`): the
/// appdata folder is the parent of `AppState::run_dir` (`<appdata>/run`,
/// `agentcfg::default_run_dir`; test states are built the same way).
pub fn reload_config(state: &AppState) -> Result<crate::workspace::ReloadReport, Vec<String>> {
    let appdata = state.run_dir.parent().map(std::path::Path::to_path_buf).unwrap_or_else(crate::appdata_dir);
    reload_config_from(state, &appdata)
}

pub fn reload_config_from(state: &AppState, appdata: &std::path::Path) -> Result<crate::workspace::ReloadReport, Vec<String>> {
    let _cfg = config::CONFIG_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let new = config::load_all(appdata); // file IO, no app lock held
    if !new.errors.is_empty() {
        return Err(new.errors.iter().map(|e| e.to_string()).collect());
    }
    let (fx, report) = {
        let mut ws = state.ws.lock().unwrap();
        let root = state.loaded.read().unwrap().settings.workspace_root.clone();
        if new.settings.workspace_root != root {
            return Err(vec![format!(
                "settings.json의 workspaceRoot가 바뀌었습니다({} → {}) — 앱을 다시 시작해야 반영됩니다",
                root.display(),
                new.settings.workspace_root.display()
            )]);
        }
        let out = ws.reload_roles(&new)?;
        *state.loaded.write().unwrap() = new; // under `ws`: no command sees the new model with the old config
        out
    };
    submit(state, fx);
    publish(state);
    state.ui.env_changed();
    state.ui.config_reloaded();
    Ok(report)
}

fn execute_model(state: &AppState, cmd: WsCommand) -> Result<Snapshot, String> {
    let fx = {
        let mut ws = state.ws.lock().unwrap();
        let loaded = state.loaded.read().unwrap();
        ws.apply(cmd, &loaded)?
    };
    submit(state, fx);
    Ok(publish(state))
}

pub fn emit_changed(state: &AppState) {
    publish(state);
}

/// Queue effects for the launcher worker (run in submission order, off the caller's thread).
pub fn submit(state: &AppState, fx: Effects) {
    if fx.spawn.is_empty() && fx.kill.is_empty() {
        return;
    }
    let _ = state.effects.lock().unwrap().send(fx);
}

/// Every pane of a freshly built workspace, with prompt injection.
pub fn initial_effects(ws: &Workspace) -> Effects {
    Effects { spawn: ws.panes.keys().map(|id| (id.clone(), true)).collect(), kill: Vec::new() }
}

/// Start the effects worker (runs `initial` first) and the prompt timer.
pub fn start(app: &AppHandle, rx: mpsc::Receiver<Effects>, initial: Effects) {
    let h = app.clone();
    std::thread::Builder::new()
        .name("launcher".into())
        .spawn(move || {
            let state = h.state::<AppState>();
            let tabs = state.ws.lock().unwrap().tabs.len();
            let n = initial.spawn.len();
            run_effects(&state, initial);
            let (running, failed) = {
                let ws = state.ws.lock().unwrap();
                let failed = ws.panes.values().filter(|p| matches!(p.status, PaneStatus::Failed { .. })).count();
                (ws.running_count(), failed)
            };
            eprintln!(
                "[agentboard] startup: tabs={tabs} panes={n} running={running} failed={failed} config_errors={}",
                error_lines(&state).len()
            );
            while let Ok(fx) = rx.recv() {
                run_effects(&state, fx);
            }
        })
        .expect("launcher thread");

    let h = app.clone();
    std::thread::Builder::new()
        .name("prompt-timer".into())
        .spawn(move || loop {
            std::thread::sleep(PROMPT_POLL);
            let state = h.state::<AppState>();
            poll_prompts(&state, Instant::now());
            crate::attention::poll_trust(&state, Instant::now()); // R5 folder-trust timers
        })
        .expect("prompt timer thread");
}

/// Execute effects: kills first, then spawns, then one `workspace-changed`.
/// Holds no app lock while calling into PtyManager. A killed pane that is no longer in the
/// model (closed, not restarted) is removed from PtyManager and its output channel dropped.
pub fn run_effects(state: &AppState, fx: Effects) {
    for id in &fx.kill {
        state.prompts.lock().unwrap().remove(id);
        let gone = state.ws.lock().unwrap().pane_by_id(id).is_none();
        if gone {
            state.pty.remove(id);
            state.ui.drop_channel(id);
            // a removed pane's identity ends here (only role panes have these)
            state.tokens.revoke(id);
            crate::attention::forget(state, id);
            crate::agentcfg::remove_run_files(&state.run_dir, id);
            state.nightly.forget(id); // its spawn time (R24)
            state.docs.unwatch(id); // a closed document pane stops being polled
            state.board.forget(id); // its dashboard status
        } else {
            state.pty.kill(id);
        }
    }
    if fx.spawn.is_empty() {
        return;
    }
    for (id, inject) in &fx.spawn {
        spawn_pane(state, id, *inject);
    }
    emit_changed(state);
}

fn spawn_pane(state: &AppState, id: &str, inject: bool) {
    // restart: the old process was killed above; PtyManager refuses an id that is still live
    let deadline = Instant::now() + KILL_WAIT;
    while state.pty.is_running(id) {
        if Instant::now() >= deadline {
            let message = "이전 프로세스가 종료되지 않아 다시 시작할 수 없습니다".to_string();
            state.ws.lock().unwrap().set_status(id, PaneStatus::Failed { message });
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let Some(pane) = state.ws.lock().unwrap().pane_by_id(id).cloned() else { return }; // closed meanwhile
    if pane.kind.is_applet() {
        // An applet is an app screen: never a process, always shown as Running. No PtyManager
        // call, no prompt job (locks taken one at a time, nothing nested).
        state.prompts.lock().unwrap().remove(id);
        state.ws.lock().unwrap().set_status(id, PaneStatus::Running);
        return;
    }
    // Role panes get a fresh token on every spawn (the previous one stops resolving).
    // `tokens` and `ctrl_url` are leaves, taken one at a time before `loaded`.
    let token = if pane.role.is_some() { state.tokens.issue(id) } else { String::new() };
    let ctrl_url = state.ctrl_url.lock().unwrap().clone().unwrap_or_else(|| PLACEHOLDER_CTRL_URL.to_string());
    // every pane: tmux/psmux markers are dropped for all, Claude session variables for role panes
    let parent_env: Vec<String> = std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).collect();
    let launch = RoleLaunch { ctrl_url: &ctrl_url, token: &token, run_dir: &state.run_dir, parent_env: &parent_env };
    let (plan, job) = {
        let loaded = state.loaded.read().unwrap();
        let plan = plan_spawn(&pane, &loaded, &launch);
        let job = match (&plan, inject, pane.prompt.as_deref()) {
            (Ok(_), true, Some(p)) if !p.trim().is_empty() => profile_for(&pane, &loaded).map(|prof| {
                // R28: a role pane's start prompt waits for its SessionStart (never typed into
                // the folder-trust dialog); other panes keep the phase-1 readiness rule
                if pane.role.is_some() {
                    PromptJob::new_gated(prof, p, Instant::now())
                } else {
                    PromptJob::new(prof, p, Instant::now())
                }
            }),
            _ => None,
        };
        (plan, job)
    };
    let spec = match plan {
        Ok(s) => s,
        Err(message) => {
            state.tokens.revoke(id);
            // The old process (if any) is dead but its `exited` may still be in flight. Forget
            // its generation first, so that event is stale and cannot overwrite the failure.
            state.pty.remove(id);
            state.ui.drop_channel(id);
            state.ws.lock().unwrap().set_status(id, PaneStatus::Failed { message });
            return;
        }
    };
    // Registered (unbound) before spawning so the very first output reaches the detector;
    // bound to the generation once `spawn` returns it.
    if let Some(job) = job {
        state.prompts.lock().unwrap().insert(id.to_string(), job);
    }
    // A failed spawn leaves the id unregistered (see PtyManager::spawn): old events are stale.
    match state.pty.spawn(id, spec) {
        Ok(gen) => {
            if let Some(job) = state.prompts.lock().unwrap().get_mut(id) {
                if job.gen().is_none() {
                    job.bind(gen);
                }
            }
            // R24: the nightly refresh skips sessions started after today's refresh time (leaf)
            state.nightly.note_spawn(id, gen, chrono::Local::now().naive_local());
            // R5: a role session must report SessionStart for this generation in time (leaf lock)
            if pane.role.is_some() {
                crate::attention::arm_trust(state, id, gen, Instant::now());
            }
            // A very fast exit may already have happened: claim Running only while this
            // generation is current and alive (otherwise `on_exited` sets / has set Exited).
            // Checked under `ws` so it is ordered against `on_exited`, which decides under `ws`.
            let mut ws = state.ws.lock().unwrap();
            if state.pty.current_gen(id) == Some(gen) && state.pty.is_running(id) {
                ws.set_status(id, PaneStatus::Running);
            }
        }
        Err(e) => {
            state.tokens.revoke(id);
            state.prompts.lock().unwrap().remove(id);
            state.ui.drop_channel(id); // like the plan failure: the old generation is retired
            state.ws.lock().unwrap().set_status(id, PaneStatus::Failed { message: format!("실행 실패: {e}") });
        }
    }
}

/// Sink `output`: runs on the PTY flush path under `flush_lock` — only feeds a detector.
/// Returns false (and does nothing) when `gen` is not the id's current generation; the sink
/// then must not forward the bytes either.
pub fn on_output(state: &AppState, id: &str, gen: u64, bytes: &[u8]) -> bool {
    if state.pty.current_gen(id) != Some(gen) {
        return false;
    }
    {
        let mut jobs = state.prompts.lock().unwrap();
        if let Some(job) = jobs.get_mut(id).filter(|j| j.owns(gen)) {
            job.feed(bytes, Instant::now());
        }
    }
    crate::bridge::on_output(state, id); // busy (leaf lock, after prompts is released)
    true
}

/// Sink `exited`. Returns false when the event is stale — `gen` is not the id's current
/// generation (a newer spawn owns the id, or a failed restart retired it) — and was ignored.
pub fn on_exited(state: &AppState, id: &str, gen: u64, code: i32) -> bool {
    {
        let mut ws = state.ws.lock().unwrap();
        if state.pty.current_gen(id) != Some(gen) {
            return false;
        }
        ws.set_status(id, PaneStatus::Exited { code });
        // only this generation's job: an unbound one belongs to a spawn still in progress
        let mut jobs = state.prompts.lock().unwrap();
        if jobs.get(id).is_some_and(|j| j.gen() == Some(gen)) {
            jobs.remove(id);
        }
    }
    emit_changed(state);
    true
}

/// SessionStart of `pane`'s generation `gen` (hook path: `attention::on_session_start`):
/// release that generation's gated start prompt. A job of another generation (stale hook,
/// or a newer spawn) is left alone; an unbound job is accepted only while `gen` is current
/// (it can only be that spawn's). Only the `prompts` lock (leaf) and the PtyManager `panes`
/// map are taken, one at a time; the write itself stays with `poll_prompts` (timer thread),
/// which re-checks the generation — no spawn/kill off the worker (R13).
pub fn release_prompt(state: &AppState, pane: &str, gen: u64, now: Instant) {
    let current = state.pty.current_gen(pane) == Some(gen);
    let mut jobs = state.prompts.lock().unwrap();
    if let Some(job) = jobs.get_mut(pane) {
        let mine = match job.gen() {
            Some(g) => g == gen,
            None => current,
        };
        if mine {
            job.release(now);
        }
    }
}

/// Timer tick: inject due prompts (ready or timed out). The write happens outside every app lock.
pub fn poll_prompts(state: &AppState, now: Instant) {
    let due: Vec<(String, Vec<u8>, bool, Option<u64>)> = {
        let mut jobs = state.prompts.lock().unwrap();
        let mut due = Vec::new();
        jobs.retain(|id, job| match job.poll(now) {
            Some((bytes, warn)) => {
                due.push((id.clone(), bytes, warn, job.gen()));
                false
            }
            None => true,
        });
        due
    };
    let mut changed = false;
    for (id, bytes, warn, gen) in due {
        // never type a prompt into a different process than the one it was meant for
        if gen.is_some() && state.pty.current_gen(&id) != gen {
            continue;
        }
        if let Err(e) = state.pty.write(&id, &bytes) {
            eprintln!("[agentboard] prompt inject {id}: {e}");
        }
        if warn {
            state.ws.lock().unwrap().set_prompt_warning(&id, true);
            changed = true;
        }
    }
    if changed {
        emit_changed(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_all, ensure_dotdir};
    use crate::profiles::{encode_submit, Ready, READY_TIMEOUT};
    use crate::roles::Role;
    use crate::workspace::{PaneKind, PaneStatus, WsCommand};
    use regex::Regex;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::Duration;

    struct Fx {
        loaded: Loaded,
        root: tempfile::TempDir,
        _appdata: tempfile::TempDir,
    }

    fn fixture() -> Fx {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), r#"{"defaultAgent":"cmd"}"#).unwrap();
        let mut loaded = load_all(appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let mut env = BTreeMap::new();
        env.insert("A".to_string(), "profile".to_string());
        env.insert("B".to_string(), "profile".to_string());
        loaded.profiles.insert(
            "tool".into(),
            profile("tool", "cmd.exe", vec!["--p1".into(), "--p2".into()], env),
        );
        loaded.profiles.insert("ghost".into(), profile("ghost", "agentboard-no-such-cmd-xyz", vec![], BTreeMap::new()));
        Fx { loaded, root, _appdata: appdata }
    }

    fn profile(name: &str, cmd: &str, args: Vec<String>, env: BTreeMap<String, String>) -> AgentProfile {
        AgentProfile {
            name: name.into(),
            level: crate::profiles::Level::L0,
            cmd: cmd.into(),
            args,
            env,
            ready: Ready::Pattern(Regex::new(r"> $").unwrap()),
            submit: Default::default(),
            passthrough_keys: vec!["Ctrl+KeyR".into()],
            system_prompt: None,
            mcp: None,
            idle: None,
            resume: None,
        }
    }

    fn pane(agent: &str, cwd: PathBuf) -> Pane {
        Pane {
            id: "p1".into(),
            name: "main".into(),
            tab: "t1".into(),
            agent: agent.into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd,
            prompt: None,
            status: PaneStatus::Starting,
            attention: false,
            prompt_warning: false,
            attention_reason: None,
            kind: PaneKind::Terminal,
            role: None,
            team: None,
            session_name: None,
            doc_path: None,
        }
    }

    fn no_role() -> RoleLaunch<'static> {
        RoleLaunch { ctrl_url: "", token: "", run_dir: std::path::Path::new(""), parent_env: &[] }
    }

    #[test]
    fn plan_spawn_role_pane_gets_extras() {
        let fx = fixture();
        let run = tempfile::tempdir().unwrap();
        let mut p = pane("tool", fx.root.path().to_path_buf());
        p.args = vec!["--pane".into()];
        p.role = Some(Role::Manager);
        p.session_name = Some("billing".into());
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:4321", token: "tk", run_dir: run.path(), parent_env: &[] };
        let spec = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        let exe = std::env::current_exe().unwrap();
        let (extras, xenv) = crate::agentcfg::role_spawn_extras_with_exe(
            &p,
            Role::Manager,
            &fx.loaded,
            "http://127.0.0.1:4321",
            "tk",
            run.path(),
            &exe,
        )
        .unwrap();
        let mut want = vec!["--p1".to_string(), "--p2".to_string()];
        want.extend(extras);
        want.push("--pane".into());
        assert_eq!(spec.args, want, "profile args, role extras, pane args");
        assert_eq!(spec.args[3], "billing");
        let env: BTreeMap<_, _> = spec.env.into_iter().collect();
        for (k, v) in xenv {
            assert_eq!(env.get(&k), Some(&v), "{k}");
        }
        assert_eq!(env.get("A").map(String::as_str), Some("profile"));
        assert!(run.path().join("p1.mcp.json").is_file() && run.path().join("p1.settings.json").is_file());
    }

    #[test]
    fn plan_spawn_flattens_prompt_only_for_batch_shims() {
        let mut fx = fixture();
        let shim = fx.root.path().join("fakeclaude.cmd");
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        let shim_s = shim.to_string_lossy().into_owned();
        fx.loaded.profiles.insert("shim".into(), profile("shim", &shim_s, vec![], BTreeMap::new()));
        let run = tempfile::tempdir().unwrap();
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:1", token: "t", run_dir: run.path(), parent_env: &[] };
        for (agent, batch) in [("shim", true), ("tool", false)] {
            let mut p = pane(agent, fx.root.path().to_path_buf());
            p.args = vec!["--pane".into()];
            p.role = Some(Role::Manager);
            p.session_name = Some("billing".into());
            let spec = plan_spawn(&p, &fx.loaded, &launch).unwrap();
            assert_eq!(crate::agentcfg::is_batch(&spec.cmd), batch, "{agent}: {}", spec.cmd.display());
            let i = spec.args.iter().position(|a| a == "--append-system-prompt").unwrap();
            assert!(spec.args[..i].contains(&"--settings".to_string()) && spec.args[..i].contains(&"--mcp-config".to_string()));
            let prompt = &spec.args[i + 1];
            let want = crate::agentcfg::default_role_prompt(Role::Manager);
            if batch {
                assert!(!prompt.contains('\n') && !prompt.contains('\r'), "{agent}: one line for cmd.exe");
                assert_eq!(*prompt, crate::agentcfg::single_line(want));
            } else {
                assert_eq!(prompt, want, "{agent}: .exe keeps the newlines");
            }
            assert_eq!(spec.args.last().unwrap(), "--pane");
        }
    }

    #[test]
    fn role_spawn_spec_drops_only_session_bound_vars() {
        let fx = fixture();
        let run = tempfile::tempdir().unwrap();
        // the app's environment is injected: no process-global set_var
        let parent: Vec<String> = ["PATH", "CLAUDECODE", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_GIT_BASH_PATH", "CLAUDE_CODE_USE_BEDROCK"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:1", token: "t", run_dir: run.path(), parent_env: &parent };
        let mut p = pane("tool", fx.root.path().to_path_buf());
        let plain = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        assert!(plain.env_remove.is_empty(), "plain panes drop only tmux/psmux markers (none here), not Claude's");
        p.role = Some(Role::Manager);
        p.session_name = Some("billing".into());
        let spec = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        assert_eq!(
            spec.env_remove,
            vec!["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_CODE_SSE_PORT"]
        );
        assert!(!spec.env_remove.iter().any(|k| k == "CLAUDE_CODE_GIT_BASH_PATH" || k == "CLAUDE_CODE_USE_BEDROCK" || k == "PATH"));
    }

    /// AgentBoard started from a psmux (tmux) shell: its panes are ConPTY terminals, not tmux
    /// panes, so no pane — plain or role — may inherit the tmux/psmux markers (Claude Code would
    /// warn "tmux focus-events off" and switch its keyboard protocol). Nothing else is dropped.
    #[test]
    fn spawn_env_drops_tmux_and_psmux_vars() {
        let fx = fixture();
        let run = tempfile::tempdir().unwrap();
        let parent: Vec<String> = ["PATH", "TMUX", "TMUX_PANE", "PSMUX_SESSION", "PSMUX_TARGET_SESSION", "psmux_other",
            "CLAUDE_CODE_GIT_BASH_PATH", "TMUXINATOR_CONFIG", "MY_PSMUX_NOTE"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:1", token: "t", run_dir: run.path(), parent_env: &parent };
        let mut p = pane("tool", fx.root.path().to_path_buf());
        let plain = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        p.role = Some(Role::Manager);
        p.session_name = Some("billing".into());
        let role = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        for (kind, spec) in [("plain", &plain), ("role", &role)] {
            for k in ["TMUX", "TMUX_PANE", "PSMUX_SESSION", "PSMUX_TARGET_SESSION", "psmux_other"] {
                assert!(spec.env_remove.iter().any(|r| r == k), "{kind} pane inherits {k}: {:?}", spec.env_remove);
            }
            for k in ["PATH", "CLAUDE_CODE_GIT_BASH_PATH", "TMUXINATOR_CONFIG", "MY_PSMUX_NOTE"] {
                assert!(!spec.env_remove.iter().any(|r| r == k), "{kind} pane loses {k}: {:?}", spec.env_remove);
            }
        }
    }

    #[test]
    fn plan_spawn_role_args_from_config() {
        let fx = fixture();
        std::fs::write(
            fx.root.path().join(".agentboard/config.json"),
            r#"{"defaultAgent":"cmd","roles":{"lead":{"args":["--model","haiku"]}}}"#,
        )
        .unwrap();
        let mut loaded = load_all(fx._appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        loaded.profiles = fx.loaded.profiles.clone();
        let run = tempfile::tempdir().unwrap();
        let mut p = pane("tool", fx.root.path().to_path_buf());
        p.args = vec!["--pane".into()];
        p.role = Some(Role::Lead);
        p.session_name = Some("t-a-lead".into());
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:1", token: "t", run_dir: run.path(), parent_env: &[] };
        let spec = plan_spawn(&p, &loaded, &launch).unwrap();
        let n = spec.args.len();
        assert_eq!(&spec.args[n - 3..], &["--model".to_string(), "haiku".to_string(), "--pane".to_string()]);
    }

    #[test]
    fn plan_spawn_without_role_has_no_extras() {
        let fx = fixture();
        let run = tempfile::tempdir().unwrap();
        let p = pane("tool", fx.root.path().to_path_buf());
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:4321", token: "tk", run_dir: run.path(), parent_env: &[] };
        let spec = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        assert_eq!(spec.args, vec!["--p1", "--p2"]);
        assert!(spec.env.iter().all(|(k, _)| !k.starts_with("AGENTBOARD_")));
        assert_eq!(std::fs::read_dir(run.path()).unwrap().count(), 0, "no run files");
    }

    #[test]
    fn plan_spawn_merges_args_and_env() {
        let fx = fixture();
        let mut p = pane("tool", fx.root.path().to_path_buf());
        p.args = vec!["--pane".into()];
        p.env.insert("B".into(), "pane".into());
        p.env.insert("C".into(), "pane".into());
        let spec = plan_spawn(&p, &fx.loaded, &no_role()).unwrap();
        assert_eq!(spec.args, vec!["--p1", "--p2", "--pane"]);
        let env: BTreeMap<_, _> = spec.env.into_iter().collect();
        assert_eq!(env.get("A").map(String::as_str), Some("profile"));
        assert_eq!(env.get("B").map(String::as_str), Some("pane"));
        assert_eq!(env.get("C").map(String::as_str), Some("pane"));
        assert_eq!(spec.cwd, fx.root.path());
        assert_eq!((spec.cols, spec.rows), (80, 24));
        let file = spec.cmd.file_name().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(file, "cmd.exe");
        assert!(spec.cmd.is_absolute());
    }

    #[test]
    fn plan_spawn_uses_default_agent() {
        let fx = fixture();
        let p = pane("no-such-profile", fx.root.path().to_path_buf());
        let spec = plan_spawn(&p, &fx.loaded, &no_role()).unwrap();
        let file = spec.cmd.file_name().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(file, "cmd.exe"); // defaultAgent = cmd
        assert_eq!(profile_for(&p, &fx.loaded).unwrap().name, "cmd");
    }

    #[test]
    fn failed_spawn_sets_failed_status() {
        let fx = fixture();
        let p = pane("ghost", fx.root.path().to_path_buf());
        let err = plan_spawn(&p, &fx.loaded, &no_role()).unwrap_err();
        assert!(err.contains("명령을 찾을 수 없습니다"), "{err}");
        assert_eq!(err, "명령을 찾을 수 없습니다: agentboard-no-such-cmd-xyz (프로필 ghost)");
    }

    #[test]
    fn plan_spawn_errors_on_missing_cwd() {
        let fx = fixture();
        let missing = fx.root.path().join("nope");
        // checked before the command, so even an unresolvable profile reports the folder
        let p = pane("ghost", missing.clone());
        let err = plan_spawn(&p, &fx.loaded, &no_role()).unwrap_err();
        assert_eq!(err, format!("작업 폴더가 없습니다: {}", missing.display()));
    }

    #[test]
    fn prompt_job_injects_after_ready_or_timeout() {
        let fx = fixture();
        let prof = &fx.loaded.profiles["tool"];
        let want = encode_submit("hello\nworld", &prof.submit);
        let t0 = Instant::now();

        // ready path: one write, no warning
        let mut job = PromptJob::new(prof, "hello\nworld", t0);
        job.feed(b"loading...", t0 + Duration::from_secs(1));
        assert_eq!(job.poll(t0 + Duration::from_secs(1)), None);
        job.feed(b"\r\n\x1b[32m> \x1b[0m", t0 + Duration::from_secs(2));
        assert_eq!(job.poll(t0 + Duration::from_secs(2)), Some((want.clone(), false)));
        assert_eq!(job.poll(t0 + Duration::from_secs(3)), None);
        assert_eq!(job.poll(t0 + Duration::from_secs(30)), None);

        // timeout path: write + warning, once
        let mut job = PromptJob::new(prof, "hello\nworld", t0);
        job.feed(b"still loading", t0 + Duration::from_secs(1));
        assert_eq!(job.poll(t0 + READY_TIMEOUT - Duration::from_millis(1)), None);
        assert_eq!(job.poll(t0 + READY_TIMEOUT), Some((want, true)));
        assert_eq!(job.poll(t0 + READY_TIMEOUT + Duration::from_secs(1)), None);
    }

    /// R28 / final review #2: a role pane's start prompt waits for SessionStart of its
    /// generation — never typed into Claude's folder-trust dialog on idle or timeout.
    #[test]
    fn gated_prompt_job_waits_for_session_start() {
        let fx = fixture();
        let mut prof = fx.loaded.profiles["tool"].clone();
        prof.ready = Ready::IdleMs(800);
        let want = encode_submit("start", &prof.submit);
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_millis(n);

        let mut job = PromptJob::new_gated(&prof, "start", t0);
        assert!(job.is_gated());
        // the trust dialog: static output, idle for minutes — nothing is typed, ever
        job.feed(b"Do you trust the files in this folder?", s(500));
        for at in [2_000, 11_000, 60_000, 600_000] {
            assert_eq!(job.poll(s(at)), None, "{at} ms");
        }
        // SessionStart: ready once the session has been quiet for idleMs *after* it
        job.release(s(600_000));
        assert_eq!(job.poll(s(600_100)), None, "not idle since the release yet");
        job.feed(b"\x1b[2J welcome", s(600_300));
        assert_eq!(job.poll(s(600_900)), None);
        assert_eq!(job.poll(s(601_100)), Some((want.clone(), false)));
        assert_eq!(job.poll(s(700_000)), None, "once");

        // released but never quiet: READY_TIMEOUT after the release, with the warning
        let mut job = PromptJob::new_gated(&prof, "start", t0);
        job.release(s(1_000));
        for at in (1_000..11_000).step_by(100) {
            job.feed(b".", s(at));
            assert_eq!(job.poll(s(at)), None, "{at}");
        }
        assert_eq!(job.poll(s(1_000) + READY_TIMEOUT), Some((want.clone(), true)));

        // a pattern that matched the dialog before SessionStart does not count
        let mut pprof = prof.clone();
        pprof.ready = Ready::Pattern(Regex::new(r"[>]\s*$").unwrap());
        let mut job = PromptJob::new_gated(&pprof, "start", t0);
        job.feed(b"> 1. Yes, proceed\r\n> ", s(100));
        assert_eq!(job.poll(s(20_000)), None);
        job.release(s(20_000));
        assert_eq!(job.poll(s(20_001)), None, "fresh detector after the release");
        job.feed(b"\r\n> ", s(20_500));
        assert_eq!(job.poll(s(20_500)), Some((want, false)));

        // phase-1 jobs are not gated
        assert!(!PromptJob::new(&prof, "x", t0).is_gated());
    }

    #[test]
    fn role_pane_prompt_is_injected_only_after_its_session_start() {
        let rt = runtime();
        let (state, id) = (&rt.state, rt.pane.as_str());
        assert!(state.ws.lock().unwrap().pane_by_id(id).unwrap().role.is_some(), "the dispatcher pane");
        state.ws.lock().unwrap().panes.get_mut(id).unwrap().prompt = Some("echo start".into());
        let fx = initial_effects(&state.ws.lock().unwrap()); // guard dropped before run_effects
        run_effects(state, fx);
        let gen = state.pty.current_gen(id).expect("spawned");
        let gated = || state.prompts.lock().unwrap().get(id).map(|j| (j.gen(), j.is_gated(), j.is_released()));
        assert_eq!(gated(), Some((Some(gen), true, false)));
        // no SessionStart: no injection however long it takes (the trust ⚠ covers the wait)
        poll_prompts(state, Instant::now() + Duration::from_secs(120));
        assert_eq!(gated(), Some((Some(gen), true, false)), "still waiting");
        assert!(!state.ws.lock().unwrap().pane_by_id(id).unwrap().prompt_warning);
        // a SessionStart of another generation does not release it
        release_prompt(state, id, gen + 100, Instant::now());
        assert_eq!(gated(), Some((Some(gen), true, false)));
        // the SessionStart hook path (attention → bridge → launcher) releases this generation
        let caller = {
            let ws = state.ws.lock().unwrap();
            let p = ws.pane_by_id(id).unwrap();
            crate::control::Caller {
                pane: p.id.clone(),
                role: p.role.unwrap(),
                tab: p.tab.clone(),
                tab_name: ws.tabs[&p.tab].name.clone(),
                team: None,
                session_name: p.session_name.clone().unwrap(),
            }
        };
        crate::attention::on_session_start(state, &caller, &serde_json::json!({}));
        assert_eq!(gated(), Some((Some(gen), true, true)));
        poll_prompts(state, Instant::now() + READY_TIMEOUT + Duration::from_secs(1));
        assert_eq!(gated(), None, "typed once released");
        state.pty.remove(id);
    }

    #[test]
    fn missing_dirs_lists_tabs_without_folder() {
        let fx = fixture();
        std::fs::write(
            fx.root.path().join(".agentboard/config.json"),
            r#"{"tabs":[{"name":"api","dir":"api"},{"name":"web","dir":"web"}]}"#,
        )
        .unwrap();
        std::fs::create_dir(fx.root.path().join("web")).unwrap();
        let loaded = load_all(fx._appdata.path());
        let ws = Workspace::from_config(&loaded);
        let missing = missing_dirs(&ws);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].1, "api");
        assert_eq!(ws.tabs[&missing[0].0].name, "api");
    }

    #[test]
    fn snapshot_versions_strictly_increase() {
        let fx = fixture();
        let ws = Workspace::from_config(&fx.loaded);
        let counter = AtomicU64::new(0);
        let vs: Vec<u64> = (0..5).map(|_| versioned(&ws, &counter, 2).version).collect();
        assert!(vs.windows(2).all(|w| w[1] > w[0]), "{vs:?}");
        assert!(vs[0] >= 1);
        let s = versioned(&ws, &counter, 2);
        assert_eq!(s.errors, 2);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["version"], 6);
        assert!(json.get("workspace").is_some());
    }

    // ── runtime tests: a real AppState without Tauri (recording Ui, silent PTY sink) ──

    #[derive(Default)]
    struct RecUi {
        snaps: std::sync::Mutex<Vec<Snapshot>>,
        dropped: std::sync::Mutex<Vec<String>>,
    }
    impl crate::Ui for RecUi {
        fn workspace_changed(&self, snap: &Snapshot) {
            self.snaps.lock().unwrap().push(snap.clone());
        }
        fn drop_channel(&self, id: &str) {
            self.dropped.lock().unwrap().push(id.to_string());
        }
    }

    /// PTY events are not routed back: tests deliver (stale) events by hand, deterministically.
    struct NullSink;
    impl crate::pty::PtySink for NullSink {
        fn output(&self, _: &str, _: u64, _: &[u8]) {}
        fn exited(&self, _: &str, _: u64, _: i32) {}
    }

    struct Rt {
        state: AppState,
        ui: std::sync::Arc<RecUi>,
        rx: mpsc::Receiver<Effects>,
        pane: String,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Drop for Rt {
        fn drop(&mut self) {
            let ids: Vec<String> = self.state.ws.lock().unwrap().panes.keys().cloned().collect();
            for id in ids {
                self.state.pty.remove(&id);
            }
        }
    }

    /// Workspace from config with every pane running `cmd.exe` in an existing folder.
    fn runtime() -> Rt {
        let Fx { loaded, root, _appdata } = fixture();
        let mut ws = Workspace::from_config(&loaded);
        for p in ws.panes.values_mut() {
            p.agent = "cmd".into();
            std::fs::create_dir_all(&p.cwd).unwrap();
        }
        let pane = ws.panes.keys().next().expect("dispatcher pane").clone();
        let ui = std::sync::Arc::new(RecUi::default());
        let (tx, rx) = mpsc::channel();
        let pty = crate::pty::PtyManager::new_per_child_job(std::sync::Arc::new(NullSink));
        let state = AppState::new(ws, loaded, pty, ui.clone(), tx, crate::persist::Saver::disabled(), Vec::new(), _appdata.path().join("run"));
        Rt { state, ui, rx, pane, _keep: (root, _appdata) }
    }

    fn status(state: &AppState, id: &str) -> PaneStatus {
        state.ws.lock().unwrap().pane_by_id(id).unwrap().status.clone()
    }

    fn restart(rt: &Rt) {
        execute(&rt.state, WsCommand::RestartPane { pane: rt.pane.clone() }).unwrap();
        run_effects(&rt.state, rt.rx.try_recv().expect("restart effects submitted"));
    }

    #[test]
    fn stale_exit_does_not_mark_respawned_pane() {
        let rt = runtime();
        let (state, id) = (&rt.state, rt.pane.as_str());
        let fx = initial_effects(&state.ws.lock().unwrap()); // guard dropped before run_effects
        run_effects(state, fx);
        let gen1 = state.pty.current_gen(id).expect("spawned");
        assert_eq!(status(state, id), PaneStatus::Running);

        restart(&rt);
        let gen2 = state.pty.current_gen(id).expect("respawned");
        assert_ne!(gen1, gen2);
        assert_eq!(status(state, id), PaneStatus::Running);

        // the old process's events arrive late: ignored
        assert!(!on_output(state, id, gen1, b"old"));
        assert!(!on_exited(state, id, gen1, 0));
        assert_eq!(status(state, id), PaneStatus::Running);

        // the current generation still counts
        assert!(on_output(state, id, gen2, b"new"));
        state.pty.kill(id);
        let end = Instant::now() + Duration::from_secs(5);
        while state.pty.is_running(id) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(on_exited(state, id, gen2, 7));
        assert_eq!(status(state, id), PaneStatus::Exited { code: 7 });
    }

    #[test]
    fn failed_restart_message_survives_old_exit() {
        let rt = runtime();
        let (state, id) = (&rt.state, rt.pane.as_str());
        let fx = initial_effects(&state.ws.lock().unwrap()); // guard dropped before run_effects
        run_effects(state, fx);
        let gen1 = state.pty.current_gen(id).expect("spawned");

        // the restart cannot plan: the profile's command disappeared
        state.loaded.write().unwrap().profiles.get_mut("cmd").unwrap().cmd = "agentboard-no-such-cmd-xyz".into();
        restart(&rt);
        let want = PaneStatus::Failed {
            message: "명령을 찾을 수 없습니다: agentboard-no-such-cmd-xyz (프로필 cmd)".into(),
        };
        assert_eq!(status(state, id), want);
        assert_ne!(state.pty.current_gen(id), Some(gen1), "the old generation is no longer current");

        // the killed process's exit arrives after the failure was recorded
        assert!(!on_exited(state, id, gen1, 1));
        assert_eq!(status(state, id), want, "failure message must survive");
        assert!(rt.ui.dropped.lock().unwrap().contains(&id.to_string()), "old output channel dropped");
    }

    fn job_gen(state: &AppState, id: &str) -> Option<Option<u64>> {
        state.prompts.lock().unwrap().get(id).map(|j| j.gen())
    }

    fn wait_dead(state: &AppState, id: &str) {
        let end = Instant::now() + Duration::from_secs(5);
        while state.pty.is_running(id) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!state.pty.is_running(id));
    }

    #[test]
    fn stale_exit_keeps_new_generation_prompt_job() {
        let rt = runtime();
        let (state, id) = (&rt.state, rt.pane.as_str());
        state.ws.lock().unwrap().panes.get_mut(id).unwrap().prompt = Some("echo hi".into());
        let fx = initial_effects(&state.ws.lock().unwrap());
        run_effects(state, fx);
        let gen1 = state.pty.current_gen(id).expect("spawned");
        assert_eq!(job_gen(state, id), Some(Some(gen1)), "job is bound to the generation it was spawned for");

        // gen1 dies; a restart with injection registers gen2 before gen1's exit is delivered
        state.pty.kill(id);
        wait_dead(state, id);
        spawn_pane(state, id, true);
        let gen2 = state.pty.current_gen(id).expect("respawned");
        assert_ne!(gen1, gen2);
        assert_eq!(job_gen(state, id), Some(Some(gen2)));
        assert!(!on_exited(state, id, gen1, 0));
        assert_eq!(job_gen(state, id), Some(Some(gen2)), "the new generation's job survives a stale exit");

        // window: the next spawn's job is registered (unbound) while the old generation is
        // still current — its exit is accepted but must not take the pending job with it
        state.pty.kill(id);
        wait_dead(state, id);
        let job = {
            let loaded = state.loaded.read().unwrap();
            PromptJob::new(&loaded.profiles["cmd"], "echo next", Instant::now())
        };
        state.prompts.lock().unwrap().insert(id.to_string(), job);
        assert!(on_exited(state, id, gen2, 0));
        assert_eq!(job_gen(state, id), Some(None), "pending job of the next spawn survives");

        // a job bound to the exiting generation goes with it
        state.prompts.lock().unwrap().get_mut(id).unwrap().bind(gen2);
        assert!(on_exited(state, id, gen2, 0));
        assert_eq!(job_gen(state, id), None);
    }

    #[test]
    fn restart_fresh_is_generation_safe_and_reinjects_prompt() {
        let rt = runtime();
        let (state, id) = (&rt.state, rt.pane.as_str());
        state.ws.lock().unwrap().panes.get_mut(id).unwrap().prompt = Some("echo start".into());
        let fx = initial_effects(&state.ws.lock().unwrap());
        run_effects(state, fx);
        let gen1 = state.pty.current_gen(id).expect("spawned");
        // the first prompt is consumed (as if injected): no job left for gen1
        state.prompts.lock().unwrap().remove(id);

        // nightly path: execute → the worker queue (nothing runs inline) → run_effects
        execute(state, WsCommand::RestartPaneFresh { pane: id.to_string() }).unwrap();
        assert_eq!(status(state, id), PaneStatus::Starting);
        assert_eq!(state.pty.current_gen(id), Some(gen1), "execute itself kills/spawns nothing");
        let fx = rt.rx.try_recv().expect("effects submitted to the launcher worker");
        assert_eq!(fx, Effects { spawn: vec![(id.to_string(), true)], kill: vec![id.to_string()] });
        run_effects(state, fx);
        let gen2 = state.pty.current_gen(id).expect("respawned");
        assert_ne!(gen1, gen2, "a new process (fresh session)");
        assert_eq!(status(state, id), PaneStatus::Running);
        assert_eq!(job_gen(state, id), Some(Some(gen2)), "start prompt queued again, for the new generation only");
        assert!(state.nightly.spawned_at(id, Some(gen2)).is_some(), "spawn time recorded for the nightly R24 check");
        assert!(state.nightly.spawned_at(id, Some(gen1)).is_none(), "only the current generation's");

        // the killed process's late events change nothing
        assert!(!on_output(state, id, gen1, b"old"));
        assert!(!on_exited(state, id, gen1, 1));
        assert_eq!(status(state, id), PaneStatus::Running);
        assert_eq!(job_gen(state, id), Some(Some(gen2)), "stale exit keeps the new start prompt");
        // and a prompt job is never typed into another generation (poll_prompts checks gen)
        let p = state.ws.lock().unwrap().pane_by_id(id).cloned().unwrap();
        let launch = RoleLaunch { ctrl_url: "http://127.0.0.1:1", token: "t", run_dir: &state.run_dir, parent_env: &[] };
        let spec = plan_spawn(&p, &state.loaded.read().unwrap(), &launch).unwrap();
        assert!(!spec.args.iter().any(|a| a.contains("resume")), "never --resume: {:?}", spec.args);
    }

    #[test]
    fn execute_returns_emitted_snapshot() {
        let rt = runtime();
        let state = &rt.state;
        let snap = execute(state, WsCommand::SplitPane { pane: rt.pane.clone(), dir: crate::layout::Dir::Vertical }).unwrap();
        let emitted = rt.ui.snaps.lock().unwrap().last().cloned().expect("emitted");
        assert_eq!(snap.version, emitted.version);
        assert_eq!(serde_json::to_value(&snap).unwrap(), serde_json::to_value(&emitted).unwrap());
        assert_eq!(snap.workspace.panes.len(), 2);
        // effects went to the launcher worker, not run inline
        let fx = rt.rx.try_recv().expect("effects submitted");
        assert_eq!(fx.spawn.len(), 1);
        assert!(state.pty.current_gen(&fx.spawn[0].0).is_none(), "nothing spawned by execute itself");

        // a rejected command: error, no emit, no effects
        let n = rt.ui.snaps.lock().unwrap().len();
        let err = execute(state, WsCommand::RestartPane { pane: "nope".into() });
        assert!(err.is_err());
        assert_eq!(rt.ui.snaps.lock().unwrap().len(), n);
        assert!(rt.rx.try_recv().is_err());
    }

    #[test]
    fn applet_pane_is_never_spawned() {
        let Fx { loaded, root, _appdata } = fixture();
        std::fs::write(
            root.path().join(".agentboard/templates/dispatcher-inbox.json"),
            r#"{"name":"dispatcher-inbox","layout":{"split":"horizontal","children":[
                {"pane":{"name":"dispatcher","focus":true}},{"applet":{"name":"inbox","kind":"inbox"}}]}}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join(".agentboard/config.json"),
            r#"{"defaultAgent":"cmd","dispatcher":{"agent":"cmd","template":"dispatcher-inbox"}}"#,
        )
        .unwrap();
        drop(loaded);
        let loaded = load_all(_appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let ws = Workspace::from_config(&loaded);
        let id = |name: &str| ws.panes.values().find(|p| p.name == name).unwrap().id.clone();
        let (inbox, disp) = (id("inbox"), id("dispatcher"));
        let ui = std::sync::Arc::new(RecUi::default());
        let (tx, rx) = mpsc::channel();
        let pty = crate::pty::PtyManager::new_per_child_job(std::sync::Arc::new(NullSink));
        let state = AppState::new(ws, loaded, pty, ui.clone(), tx, crate::persist::Saver::disabled(), Vec::new(), _appdata.path().join("run"));
        let rt = Rt { state, ui, rx, pane: disp.clone(), _keep: (root, _appdata) };
        let state = &rt.state;

        let fx = initial_effects(&state.ws.lock().unwrap());
        assert!(fx.spawn.iter().any(|(p, _)| *p == inbox));
        run_effects(state, fx);
        assert_eq!(status(state, &inbox), PaneStatus::Running, "applet shows Running without a process");
        assert_eq!(state.pty.current_gen(&inbox), None, "no PTY for an applet");
        assert!(!state.pty.is_running(&inbox));
        assert!(state.prompts.lock().unwrap().get(&inbox).is_none());
        assert!(state.pty.current_gen(&disp).is_some(), "the terminal pane next to it is spawned");

        // restart / restored-workspace respawn: still no process
        execute(state, WsCommand::RestartPane { pane: inbox.clone() }).unwrap();
        assert_eq!(status(state, &inbox), PaneStatus::Starting);
        run_effects(state, rt.rx.try_recv().expect("restart effects"));
        assert_eq!(status(state, &inbox), PaneStatus::Running);
        assert_eq!(state.pty.current_gen(&inbox), None);
        let fx = state.ws.lock().unwrap().reset_runtime_state();
        run_effects(state, fx);
        assert_eq!(status(state, &inbox), PaneStatus::Running);
        assert_eq!(state.pty.current_gen(&inbox), None);
        // the planner refuses an applet outright (defence in depth)
        let p = state.ws.lock().unwrap().pane_by_id(&inbox).cloned().unwrap();
        assert!(plan_spawn(&p, &state.loaded.read().unwrap(), &no_role()).is_err());
    }

    fn token_in_run_file(state: &AppState, id: &str) -> String {
        let text = std::fs::read_to_string(state.run_dir.join(format!("{id}.mcp.json"))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let auth = v["mcpServers"]["agentboard"]["headers"]["Authorization"].as_str().unwrap();
        auth.strip_prefix("Bearer ").unwrap().to_string()
    }

    #[test]
    fn role_spawn_reissues_token_and_removal_revokes_it() {
        let rt = runtime();
        let (state, id) = (&rt.state, rt.pane.clone());
        assert_eq!(state.ws.lock().unwrap().pane_by_id(&id).unwrap().role, Some(Role::Dispatcher));
        let fx = initial_effects(&state.ws.lock().unwrap());
        run_effects(state, fx);
        let t1 = token_in_run_file(state, &id);
        assert_eq!(state.tokens.resolve(&t1).as_deref(), Some(id.as_str()));
        let settings = std::fs::read_to_string(state.run_dir.join(format!("{id}.settings.json"))).unwrap();
        assert!(settings.contains("--hook SessionStart"));
        let mcp = std::fs::read_to_string(state.run_dir.join(format!("{id}.mcp.json"))).unwrap();
        assert!(mcp.contains(&format!("{PLACEHOLDER_CTRL_URL}/mcp")), "placeholder until ctrl_url is set");

        *state.ctrl_url.lock().unwrap() = Some("http://127.0.0.1:6000".into());
        restart(&rt);
        let t2 = token_in_run_file(state, &id);
        assert_ne!(t1, t2);
        assert_eq!(state.tokens.resolve(&t1), None, "old token is dead after a respawn");
        assert_eq!(state.tokens.resolve(&t2).as_deref(), Some(id.as_str()));
        let mcp = std::fs::read_to_string(state.run_dir.join(format!("{id}.mcp.json"))).unwrap();
        assert!(mcp.contains("http://127.0.0.1:6000/mcp"));

        // the pane leaves the model: token revoked, run files deleted
        state.ws.lock().unwrap().panes.remove(&id);
        run_effects(state, Effects { spawn: vec![], kill: vec![id.clone()] });
        assert_eq!(state.tokens.resolve(&t2), None);
        assert!(!state.run_dir.join(format!("{id}.mcp.json")).exists());
        assert!(!state.run_dir.join(format!("{id}.settings.json")).exists());
    }

    #[test]
    fn check_dir_name_rejects_bad_names() {
        for bad in ["", "  ", "..", ".", "a/b", "a\\b", "c:"] {
            assert!(check_dir_name(bad).is_err(), "{bad:?}");
        }
        for ok in ["api", "my-project", "한글"] {
            assert!(check_dir_name(ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn check_dir_name_rejects_windows_reserved_and_invalid() {
        for bad in [
            "CON", "con.txt", "NUL", "COM1", "LPT9", "a?b", "a*b", "a<b", "a|b", "a\"b", "a:b", "foo.", "foo ",
            " foo", "a\u{7}b", ".agentboard", ".git", ".GIT", "com¹", "LPT³.log", "COM0", "lpt0", "aux.tar.gz",
        ] {
            assert!(check_dir_name(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(check_dir_name(&"a".repeat(256)).is_err(), "longer than 255 characters");
    }

    #[test]
    fn check_dir_name_keeps_common_names() {
        for ok in ["api", "my-project", "한글", "v1.2", "a.b", "con-fig", ".config", "COM10", "console", "nul-x"] {
            assert!(check_dir_name(ok).is_ok(), "{ok:?} should be accepted");
        }
        assert!(check_dir_name(&"a".repeat(255)).is_ok());
    }

    #[test]
    fn check_rel_dir_requires_normalized_segments() {
        for ok in ["a/b", r"a\b", "billing", "팀/결제"] {
            assert!(check_rel_dir(ok).is_ok(), "{ok:?}");
        }
        for bad in ["a/../b", "./a", "a//b", "a/CON", "billing.", "", "billing/", "/a", r"C:\x", "a/.git"] {
            assert!(check_rel_dir(bad).is_err(), "{bad:?}");
        }
    }

    // ---- config reload (no restart): validate, swap, add missing role tabs ----

    #[derive(Default)]
    struct ReloadUi {
        reloaded: std::sync::atomic::AtomicUsize,
    }
    impl crate::Ui for ReloadUi {
        fn workspace_changed(&self, _: &Snapshot) {}
        fn drop_channel(&self, _: &str) {}
        fn config_reloaded(&self) {
            self.reloaded.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct ReloadFx {
        state: AppState,
        rx: mpsc::Receiver<Effects>,
        ui: std::sync::Arc<ReloadUi>,
        root: PathBuf,
        appdata: PathBuf,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Drop for ReloadFx {
        fn drop(&mut self) {
            // a failed assertion may have poisoned `ws`: a second panic here would abort the run
            let ids: Vec<String> = self.state.ws.lock().unwrap_or_else(|e| e.into_inner()).panes.keys().cloned().collect();
            for id in ids {
                self.state.pty.remove(&id);
            }
        }
    }

    impl ReloadFx {
        fn write(&self, rel: &str, text: &str) {
            std::fs::write(self.root.join(".agentboard").join(rel), text).unwrap();
        }
        fn reload(&self) -> Result<crate::workspace::ReloadReport, Vec<String>> {
            reload_config_from(&self.state, &self.appdata)
        }
        fn tab(&self, name: &str) -> Option<crate::workspace::Tab> {
            self.state.ws.lock().unwrap().tabs.values().find(|t| t.name == name).cloned()
        }
        fn standing(&self, name: &str) -> crate::workspace::Pane {
            let ws = self.state.ws.lock().unwrap();
            let t = ws.tabs.values().find(|t| t.name == name).unwrap();
            ws.panes[&ws.standing_pane(&t.id).unwrap()].clone()
        }
        /// Start the pane's process (an idle cmd.exe): a real generation to compare later.
        fn run(&self, pane: &str) -> u64 {
            let spec = SpawnSpec {
                cmd: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into()],
                env: vec![],
                env_remove: vec![],
                cwd: std::env::temp_dir(),
                cols: 80,
                rows: 24,
            };
            self.state.pty.spawn(pane, spec).unwrap()
        }
    }

    const RELOAD_CONFIG: &str = r#"{"defaultAgent": "cmd",
  "tabs": [{"name": "billing", "role": "manager", "dir": "billing", "template": "manager"}, {"name": "api", "dir": "api"}],
  "defaultGroup": "work",
  "groups": {"split": "horizontal", "children": [
    {"group": {"name": "control", "tabs": ["dispatcher"]}},
    {"group": {"name": "work", "tabs": ["billing", "api"]}}]}}
"#;

    fn reload_fx() -> ReloadFx {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), RELOAD_CONFIG).unwrap();
        let loaded = load_all(appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let ws = Workspace::from_config(&loaded);
        let (tx, rx) = mpsc::channel();
        let ui = std::sync::Arc::new(ReloadUi::default());
        let pty = crate::pty::PtyManager::new_per_child_job(std::sync::Arc::new(NullSink));
        let state = AppState::new(ws, loaded, pty, ui.clone(), tx, crate::persist::Saver::disabled(), Vec::new(), appdata.path().join("run"));
        ReloadFx { state, rx, ui, root: root.path().to_path_buf(), appdata: appdata.path().to_path_buf(), _keep: (root, appdata) }
    }

    #[test]
    fn reload_with_errors_changes_nothing() {
        let f = reload_fx();
        let (ws, cfg) = (f.state.ws.lock().unwrap().clone(), f.state.loaded.read().unwrap().config.clone());
        let unchanged = |f: &ReloadFx| {
            assert_eq!(*f.state.ws.lock().unwrap(), ws);
            assert_eq!(f.state.loaded.read().unwrap().config, cfg);
            assert!(f.rx.try_recv().is_err(), "nothing spawned or killed");
            assert_eq!(f.ui.reloaded.load(Ordering::SeqCst), 0);
        };
        // a syntax error: located like the status bar's error list
        f.write("config.json", "{\n  \"tabs\": [\n    {\"name\": \"x\" \"dir\": \"x\"}\n  ]\n}\n");
        let errs = f.reload().unwrap_err();
        assert!(errs.len() == 1 && errs[0].contains("config.json") && errs[0].contains(":3:"), "{errs:?}");
        unchanged(&f);
        // a validation error (a valid file is not enough)
        f.write("config.json", &RELOAD_CONFIG.replace("\"defaultAgent\": \"cmd\",", "\"defaultAgent\": \"cmd\", \"nightlyRefresh\": \"25:00\","));
        assert!(f.reload().unwrap_err().iter().any(|e| e.contains("nightlyRefresh")));
        unchanged(&f);
        // a config role tab that cannot be opened: the whole reload is refused
        f.write("config.json", &RELOAD_CONFIG.replace(r#"{"name": "api", "dir": "api"}"#, r#"{"name": "api", "role": "manager", "dir": "api2"}"#));
        let errs = f.reload().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("'api'") && e.contains("같은 이름의 다른 탭")), "{errs:?}");
        unchanged(&f);
        // the workspace root itself cannot move under a running app
        f.write("config.json", RELOAD_CONFIG);
        let other = tempfile::tempdir().unwrap();
        std::fs::write(f.appdata.join("settings.json"), serde_json::json!({"workspaceRoot": other.path().to_string_lossy()}).to_string()).unwrap();
        assert!(f.reload().unwrap_err().iter().any(|e| e.contains("workspaceRoot")));
        unchanged(&f);
    }

    #[test]
    fn reload_adds_missing_role_tabs_without_restarting_others() {
        let f = reload_fx();
        let billing = f.standing("billing").id;
        let gen = f.run(&billing);
        let before: std::collections::BTreeSet<String> = f.state.ws.lock().unwrap().panes.keys().cloned().collect();
        f.write(
            "config.json",
            &RELOAD_CONFIG.replace(
                r#"{"name": "api", "dir": "api"}"#,
                r#"{"name": "api", "dir": "api"}, {"name": "handover", "role": "handover", "dir": ""}, {"name": "ops", "role": "manager", "dir": "ops", "template": "manager"}"#,
            ),
        );
        let r = f.reload().unwrap();
        assert_eq!(r.added, ["handover", "ops"]);
        assert!(r.kept_missing.is_empty() && r.prompts_updated == 0, "{r:?}");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v, serde_json::json!({"added": ["handover", "ops"], "keptMissing": [], "promptsUpdated": 0}));
        let (h, o) = (f.tab("handover").unwrap(), f.tab("ops").unwrap());
        assert_eq!((h.role, o.role), (Some(Role::Handover), Some(Role::Manager)));
        assert_eq!(f.standing("handover").session_name.as_deref(), Some("handover"));
        // only the new tabs' panes start (with their start prompts); nothing else is touched
        let fx = f.rx.try_recv().expect("effects of the new tabs");
        assert!(fx.kill.is_empty());
        let new: std::collections::BTreeSet<String> =
            f.state.ws.lock().unwrap().panes.keys().filter(|p| !before.contains(*p)).cloned().collect();
        let spawned: std::collections::BTreeSet<String> = fx.spawn.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(spawned, new);
        assert!(fx.spawn.iter().all(|(_, inject)| *inject));
        assert!(f.rx.try_recv().is_err());
        assert_eq!((f.state.pty.current_gen(&billing), f.state.pty.is_running(&billing)), (Some(gen), true), "billing was not restarted");
        // the loaded config is the new one; the frontend is told to re-read keys etc.
        assert!(f.state.loaded.read().unwrap().config.tabs.iter().any(|t| t.name == "handover"));
        assert_eq!(f.ui.reloaded.load(Ordering::SeqCst), 1);
        let mut ws = f.state.ws.lock().unwrap().clone();
        assert_eq!(ws.normalize_and_validate(), Ok(()));
        // a second reload with the same files changes nothing
        let r = f.reload().unwrap();
        assert!(r.added.is_empty() && r.kept_missing.is_empty() && r.prompts_updated == 0);
        assert!(f.rx.try_recv().map_or(true, |fx| fx.spawn.is_empty() && fx.kill.is_empty()));
    }

    #[test]
    fn reload_updates_stored_start_prompt_without_restart() {
        let f = reload_fx();
        let billing = f.standing("billing");
        let gen = f.run(&billing.id);
        let tpl = std::fs::read_to_string(f.root.join(".agentboard/templates/manager.json")).unwrap();
        let old = billing.prompt.clone().unwrap();
        f.write("templates/manager.json", &tpl.replace(&old, "새 시작 리뷰"));
        let r = f.reload().unwrap();
        assert_eq!(r.prompts_updated, 1, "{r:?}");
        assert_eq!(f.standing("billing").prompt.as_deref(), Some("새 시작 리뷰"), "used at the next restart / nightly refresh");
        assert!(f.rx.try_recv().map_or(true, |fx| fx.spawn.is_empty() && fx.kill.is_empty()), "no restart");
        assert_eq!(f.state.pty.current_gen(&billing.id), Some(gen));
        // the shell pane of the same tab (not a standing agent) is not a target
        let shell = {
            let ws = f.state.ws.lock().unwrap();
            ws.panes.values().find(|p| p.tab == billing.tab && p.name == "shell").cloned().unwrap()
        };
        assert_eq!(shell.role, None);
    }

    #[test]
    fn reload_reports_but_keeps_tabs_missing_from_config() {
        let f = reload_fx();
        let billing = f.standing("billing").id;
        f.write("config.json", &RELOAD_CONFIG.replace(r#"{"name": "billing", "role": "manager", "dir": "billing", "template": "manager"}, "#, "").replace(r#""tabs": ["billing", "api"]"#, r#""tabs": ["api"]"#));
        let r = f.reload().unwrap();
        assert_eq!(r.kept_missing, ["billing"]);
        assert!(r.added.is_empty());
        let t = f.tab("billing").expect("not closed");
        assert_eq!(t.role, Some(Role::Manager), "keeps its role until the user closes it");
        assert!(f.state.ws.lock().unwrap().panes.contains_key(&billing));
        assert!(f.rx.try_recv().map_or(true, |fx| fx.kill.is_empty()), "nothing killed");
    }

    /// `memo.vi` follows a reload like every other setting (the frontend re-reads on
    /// `config-reloaded`).
    #[test]
    fn reload_applies_the_memo_vi_setting() {
        let f = reload_fx();
        assert!(!f.state.loaded.read().unwrap().config.memo.vi);
        f.write("config.json", &RELOAD_CONFIG.replace("\"defaultAgent\": \"cmd\",", "\"defaultAgent\": \"cmd\", \"memo\": {\"vi\": true},"));
        f.reload().unwrap();
        assert!(f.state.loaded.read().unwrap().config.memo.vi);
        assert_eq!(f.ui.reloaded.load(Ordering::SeqCst), 1);
    }

    /// R12: a tab added by a reload goes to the end of defaultGroup in the background.
    #[test]
    fn reload_keeps_focus_and_active_tab() {
        let f = reload_fx();
        let (focus, last, actives) = {
            let ws = f.state.ws.lock().unwrap();
            let actives: Vec<_> = ws.groups.values().map(|g| (g.id.clone(), g.active_tab_id.clone())).collect();
            (ws.focus.clone(), ws.last_location.clone(), actives)
        };
        f.write(
            "config.json",
            &RELOAD_CONFIG.replace(r#"{"name": "api", "dir": "api"}"#, r#"{"name": "api", "dir": "api"}, {"name": "ops", "role": "manager", "dir": "ops"}"#),
        );
        f.reload().unwrap();
        let ws = f.state.ws.lock().unwrap();
        assert_eq!(ws.focus, focus);
        assert_eq!(ws.last_location, last);
        let now: Vec<_> = ws.groups.values().map(|g| (g.id.clone(), g.active_tab_id.clone())).collect();
        assert_eq!(now, actives);
        let work = ws.groups.values().find(|g| g.name == "work").unwrap();
        assert_eq!(work.tab_ids.last().map(|t| ws.tabs[t].name.as_str()), Some("ops"));
    }
}
