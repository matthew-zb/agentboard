//! Launcher: turns model `Effects` into PTY processes, injects first prompts once the agent
//! is ready, and maps PTY exits back onto pane status.
//!
//! # Lock order (deadlock rules)
//!
//! App locks: `AppState::ws` (Mutex), `AppState::loaded` (RwLock, read-only except `create_manager_tab`),
//! `AppState::prompts` (Mutex), `AppState::effects` (Mutex<Sender>), `AppSink::channels`
//! (Mutex, reached through `AppState::ui` / the managed `Arc<AppSink>`). PtyManager-internal
//! locks: per-pane `flush_lock`, the `panes` map, writer/master/killer.
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
//! * Control server (control.rs): `AppState::hooks` (HookRouter map) and
//!   `AppState::handoffs` are leaves. The caller of a request is computed by taking `tokens`
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
//!   `spawn_pane` arms it after `spawn` returns (no app lock held); `poll_trust` (prompt timer
//!   thread) and the SessionStart / Notification handlers take it, release it, then read
//!   `current_gen` / `ws` one at a time and change the model only via `execute`.
//! * Documents (docs.rs, Task 14): `AppState::docs` (watch map) is a leaf. `doc_watch` takes
//!   `ws` (released), then `loaded(read)` (released), then the map; the watcher thread takes
//!   only the map (released before any file system call and before emitting `doc-changed`).
//!   `OpenDoc` validates the path under `ws -> loaded(read)` (file system metadata only).
//! * `loaded(write)` (Task 8): taken only by the MCP tool `create_manager_tab`, alone — no other
//!   lock held, none taken inside — around the atomic config.json append and the in-memory
//!   `config.tabs` push (and their rollback). It is never taken under `ws`; `execute` runs
//!   after it is released. Team tools (`hire_*`, `dismiss*`) read `ws` briefly (released) and
//!   change the model only through `execute`. `config::CONFIG_WRITE` (static Mutex) is
//!   outermost: `create_manager_tab` holds it across append → `execute` → rollback and takes
//!   `loaded(write)` / `ws` only inside it, one at a time; nothing takes it while holding an
//!   app lock.
//! * Never call `PtyManager::spawn/kill/remove/write/resize/attach_with` while holding `ws`,
//!   `loaded` or `prompts`: spawn/kill/write can block, and the PTY threads call back into
//!   the sink (`output` -> prompts, `exited` -> ws).
//! * Never emit events while holding `ws`: snapshots are cloned under the lock, emitted after.
//!
//! Commands mutate the model under `ws` (apply), release it, and hand the `Effects` to a
//! single worker thread (`submit`), which runs `run_effects` in order without app locks held
//! across PtyManager calls.

use crate::agentcfg::PLACEHOLDER_CTRL_URL;
use crate::config::Loaded;
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

/// Per-spawn inputs of a role pane's extras (`agentcfg::role_spawn_extras`). Ignored for
/// panes without a role.
pub struct RoleLaunch<'a> {
    pub ctrl_url: &'a str,
    pub token: &'a str,
    pub run_dir: &'a std::path::Path,
    /// Variable names of the app's own environment; the session-bound ones are not passed on
    /// to a role session (`agentcfg::SESSION_BOUND_VARS`, R19). Injected for testability.
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
    let mut env_remove = Vec::new();
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
        env_remove = crate::agentcfg::inherited_claude_vars(launch.parent_env.iter().cloned());
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
pub struct PromptJob {
    detector: ReadyDetector,
    deadline: Instant,
    payload: Vec<u8>,
    done: bool,
    /// Generation of the spawn this job belongs to; `None` while that spawn has not returned
    /// yet (the job is registered before `PtyManager::spawn` so the first output is seen).
    gen: Option<u64>,
}

impl PromptJob {
    pub fn new(profile: &AgentProfile, prompt: &str, start: Instant) -> Self {
        PromptJob {
            detector: ReadyDetector::new(&profile.ready, start),
            deadline: start + READY_TIMEOUT,
            payload: encode_submit(prompt, &profile.submit),
            done: false,
            gen: None,
        }
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

    /// Output from the pane (called on the PTY flush path: must stay cheap).
    pub fn feed(&mut self, bytes: &[u8], now: Instant) {
        if !self.done {
            self.detector.feed(bytes, now);
        }
    }

    /// `Some((bytes, warning))` exactly once: when ready (`warning=false`) or when
    /// `READY_TIMEOUT` has passed without readiness (`warning=true`).
    pub fn poll(&mut self, now: Instant) -> Option<(Vec<u8>, bool)> {
        if self.done {
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

/// Validation for `create_workspace_dir`.
pub fn check_dir_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("폴더 이름이 비어 있습니다".into());
    }
    if name.contains(['/', '\\', ':']) {
        return Err("폴더 이름에 / \\ : 는 쓸 수 없습니다".into());
    }
    if name == "." || name == ".." {
        return Err("폴더 이름으로 . 이나 .. 은 쓸 수 없습니다".into());
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
            state.docs.unwatch(id); // a closed document pane stops being polled
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
    let parent_env: Vec<String> = if pane.role.is_some() {
        std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).collect()
    } else {
        Vec::new()
    };
    let launch = RoleLaunch { ctrl_url: &ctrl_url, token: &token, run_dir: &state.run_dir, parent_env: &parent_env };
    let (plan, job) = {
        let loaded = state.loaded.read().unwrap();
        let plan = plan_spawn(&pane, &loaded, &launch);
        let job = match (&plan, inject, pane.prompt.as_deref()) {
            (Ok(_), true, Some(p)) if !p.trim().is_empty() => {
                profile_for(&pane, &loaded).map(|prof| PromptJob::new(prof, p, Instant::now()))
            }
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
        assert!(plain.env_remove.is_empty(), "phase-1 panes inherit everything as before");
        p.role = Some(Role::Manager);
        p.session_name = Some("billing".into());
        let spec = plan_spawn(&p, &fx.loaded, &launch).unwrap();
        assert_eq!(
            spec.env_remove,
            vec!["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_CODE_SSE_PORT"]
        );
        assert!(!spec.env_remove.iter().any(|k| k == "CLAUDE_CODE_GIT_BASH_PATH" || k == "CLAUDE_CODE_USE_BEDROCK" || k == "PATH"));
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
}
