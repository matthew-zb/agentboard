//! Launcher: turns model `Effects` into PTY processes, injects first prompts once the agent
//! is ready, and maps PTY exits back onto pane status.
//!
//! # Lock order (deadlock rules)
//!
//! App locks: `AppState::ws` (Mutex), `AppState::loaded` (RwLock, read-only in phase 1),
//! `AppState::prompts` (Mutex), `AppState::effects` (Mutex<Sender>), `AppSink::channels`
//! (Mutex). PtyManager-internal locks: per-pane `flush_lock`, the `panes` map,
//! writer/master/killer.
//!
//! * Allowed nestings (outer -> inner): `ws -> loaded(read)`, `ws -> prompts`,
//!   `ws -> PtyManager::is_running/job_error` (only the `panes` map lock / job_error lock +
//!   an atomic; never blocks on a process), `flush_lock -> prompts` (sink `output` feeds the
//!   ReadyDetector), `flush_lock -> channels` (sink `output` and `pty_attach`).
//! * `ws -> emit_lock` (R16): `publish` stamps a version under `ws`, takes `emit_lock`,
//!   releases `ws`, emits, then releases `emit_lock` — emits go out in version order and
//!   never while `ws` is held.
//! * `Saver` (persist.rs): `notify` is a channel send (leaf); `publish` calls it while still
//!   holding `emit_lock` (emit_lock -> saver channel; nothing is acquired inside). Its thread takes `ws` only to
//!   clone, releases it, then writes the file — never `ws` during IO, never `emit_lock`.
//! * `prompts`, `channels`, `effects` and `emit_lock` are leaves: nothing is acquired while
//!   holding them.
//! * Never call `PtyManager::spawn/kill/write/resize/attach_with` while holding `ws`,
//!   `loaded` or `prompts`: spawn/kill/write can block, and the PTY threads call back into
//!   the sink (`output` -> prompts, `exited` -> ws).
//! * Never emit events while holding `ws`: snapshots are cloned under the lock, emitted after.
//!
//! Commands mutate the model under `ws` (apply), release it, and hand the `Effects` to a
//! single worker thread (`submit`), which runs `run_effects` in order without app locks held
//! across PtyManager calls.

use crate::config::Loaded;
use crate::profiles::{encode_submit, AgentProfile, ReadyDetector, READY_TIMEOUT};
use crate::pty::SpawnSpec;
use crate::workspace::{Effects, Pane, PaneStatus, TabId, Workspace};
use crate::{AppState, Snapshot};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

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

/// Pure core of spawning: profile lookup, cwd check, command resolution, args/env merge.
/// The working folder is checked before the command so the user sees the real cause.
pub fn plan_spawn(pane: &Pane, loaded: &Loaded) -> Result<SpawnSpec, String> {
    let profile = profile_for(pane, loaded).ok_or_else(|| {
        format!("프로필을 찾을 수 없습니다: {} (기본값 {})", pane.agent, loaded.config.default_agent)
    })?;
    if !pane.cwd.is_dir() {
        return Err(format!("작업 폴더가 없습니다: {}", pane.cwd.display()));
    }
    let cmd = crate::profiles::resolve_cmd(&profile.cmd)
        .ok_or_else(|| format!("명령을 찾을 수 없습니다: {} (프로필 {})", profile.cmd, profile.name))?;
    let args = profile.args.iter().chain(pane.args.iter()).cloned().collect();
    let mut env = profile.env.clone();
    env.extend(pane.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    Ok(SpawnSpec { cmd, args, env: env.into_iter().collect(), cwd: pane.cwd.clone(), cols: START_COLS, rows: START_ROWS })
}

/// First-prompt injection state machine for one pane (time is passed in, so it is testable).
pub struct PromptJob {
    detector: ReadyDetector,
    deadline: Instant,
    payload: Vec<u8>,
    done: bool,
}

impl PromptJob {
    pub fn new(profile: &AgentProfile, prompt: &str, start: Instant) -> Self {
        PromptJob {
            detector: ReadyDetector::new(&profile.ready, start),
            deadline: start + READY_TIMEOUT,
            payload: encode_submit(prompt, &profile.submit),
            done: false,
        }
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
    let _ = state.app.emit(WORKSPACE_CHANGED, snap.clone());
    snap
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
        })
        .expect("prompt timer thread");
}

/// Execute effects: kills first, then spawns, then one `workspace-changed`.
/// Holds no app lock while calling into PtyManager.
pub fn run_effects(state: &AppState, fx: Effects) {
    for id in &fx.kill {
        state.prompts.lock().unwrap().remove(id);
        state.pty.kill(id);
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
    let (plan, job) = {
        let loaded = state.loaded.read().unwrap();
        let plan = plan_spawn(&pane, &loaded);
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
            state.ws.lock().unwrap().set_status(id, PaneStatus::Failed { message });
            return;
        }
    };
    // registered before spawning so the very first output reaches the detector
    if let Some(job) = job {
        state.prompts.lock().unwrap().insert(id.to_string(), job);
    }
    match state.pty.spawn(id, spec) {
        Ok(()) => {
            // A very fast exit may already have happened: claim Running only while alive
            // (otherwise `on_exited` sets / has set Exited). Checked under `ws` so it is
            // ordered against `on_exited`, which also decides under `ws`.
            let mut ws = state.ws.lock().unwrap();
            if state.pty.is_running(id) {
                ws.set_status(id, PaneStatus::Running);
            }
        }
        Err(e) => {
            state.prompts.lock().unwrap().remove(id);
            state.ws.lock().unwrap().set_status(id, PaneStatus::Failed { message: format!("실행 실패: {e}") });
        }
    }
}

/// Sink `output`: runs on the PTY flush path under `flush_lock` — only feeds a detector.
pub fn on_output(state: &AppState, id: &str, bytes: &[u8]) {
    let mut jobs = state.prompts.lock().unwrap();
    if let Some(job) = jobs.get_mut(id) {
        job.feed(bytes, Instant::now());
    }
}

/// Sink `exited`. Returns false when the event is stale — a restarted process for the same
/// id is already running — and was ignored.
pub fn on_exited(state: &AppState, id: &str, code: i32) -> bool {
    {
        let mut ws = state.ws.lock().unwrap();
        if state.pty.is_running(id) {
            return false;
        }
        ws.set_status(id, PaneStatus::Exited { code });
        state.prompts.lock().unwrap().remove(id);
    }
    emit_changed(state);
    true
}

/// Timer tick: inject due prompts (ready or timed out). The write happens outside every app lock.
pub fn poll_prompts(state: &AppState, now: Instant) {
    let due: Vec<(String, Vec<u8>, bool)> = {
        let mut jobs = state.prompts.lock().unwrap();
        let mut due = Vec::new();
        jobs.retain(|id, job| match job.poll(now) {
            Some((bytes, warn)) => {
                due.push((id.clone(), bytes, warn));
                false
            }
            None => true,
        });
        due
    };
    let mut changed = false;
    for (id, bytes, warn) in due {
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
    use crate::workspace::PaneStatus;
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
        }
    }

    #[test]
    fn plan_spawn_merges_args_and_env() {
        let fx = fixture();
        let mut p = pane("tool", fx.root.path().to_path_buf());
        p.args = vec!["--pane".into()];
        p.env.insert("B".into(), "pane".into());
        p.env.insert("C".into(), "pane".into());
        let spec = plan_spawn(&p, &fx.loaded).unwrap();
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
        let spec = plan_spawn(&p, &fx.loaded).unwrap();
        let file = spec.cmd.file_name().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(file, "cmd.exe"); // defaultAgent = cmd
        assert_eq!(profile_for(&p, &fx.loaded).unwrap().name, "cmd");
    }

    #[test]
    fn failed_spawn_sets_failed_status() {
        let fx = fixture();
        let p = pane("ghost", fx.root.path().to_path_buf());
        let err = plan_spawn(&p, &fx.loaded).unwrap_err();
        assert!(err.contains("명령을 찾을 수 없습니다"), "{err}");
        assert_eq!(err, "명령을 찾을 수 없습니다: agentboard-no-such-cmd-xyz (프로필 ghost)");
    }

    #[test]
    fn plan_spawn_errors_on_missing_cwd() {
        let fx = fixture();
        let missing = fx.root.path().join("nope");
        // checked before the command, so even an unresolvable profile reports the folder
        let p = pane("ghost", missing.clone());
        let err = plan_spawn(&p, &fx.loaded).unwrap_err();
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
