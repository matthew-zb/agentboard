//! Nightly refresh (spec §7, Task 11): every day at `nightlyRefresh` (default 03:00, local
//! time) the standing agents — dispatcher, handover, Managers — save their state, report
//! their file pointers to handover, call `handoff_done`, and are then restarted as **new
//! sessions** (`WsCommand::RestartPaneFresh`: start prompt injected again, never `--resume`).
//! Team sessions (Lead/Member) and processless panes (applets, documents) are never touched.
//!
//! # State machine ([`Nightly`], pure; time and the observed workspace are passed in)
//!
//! `Idle → Notifying → Waiting{pending, deadline} → Restarting → Idle`. [`Nightly::step`] runs
//! the transitions that are due at `now` and returns the [`Action`]s to perform;
//! `Notifying` and `Restarting` are passed through within one step (they only emit actions).
//!
//! * **Start** (`Idle`): a time is configured, the last run date ≠ today (local), the local
//!   time ≥ the configured time, and no retry is pending. If the dispatcher had a real
//!   keystroke (`pty_write` → `AppState::last_input`, never UserPromptSubmit — R4) within
//!   `nightlyQuietMin`, the run is postponed by [`RETRY_AFTER`] (5 min). Starting records the
//!   date ([`Action::Record`] → `nightly.json`) **before** anything else, so a crash mid-run
//!   never repeats the run that day (the app's own restart gives fresh sessions anyway).
//! * **Notifying**: the targets are the standing panes whose process is `Running` now (exited
//!   or failed panes stay as the user left them). Managers and the dispatcher ("workers") get
//!   the save-and-report message naming the actual handover session (`@<session>`); handover
//!   gets its own message later.
//! * **Waiting**: `pending` = targets without `handoff_done` ([`Nightly::handoff`]).
//!   handover is notified once every worker is done (or gone), or when half of
//!   `nightlyTimeoutMin` has passed. A target that exited or was closed stops blocking.
//!   Ends when nothing is pending or the deadline (monotonic clock) has passed — but never
//!   while a target pane had a keystroke within `nightlyQuietMin` (a restart would throw away
//!   what the user is typing); then it waits for the next tick.
//! * **Restarting**: per target, in start order: not done → [`Action::Incomplete`] (an inbox
//!   system item "야간 정리 미완료: <이름>" for its tab, the reason in `check`); still in the
//!   model and running → [`Action::Restart`]. Closed panes are forgotten.
//!
//! Clock rules: "today" and "after the time" use local wall-clock time (chrono `Local`, already
//! a dependency). App started after the time → that day's run happens once. PC asleep at the
//! time → the run happens on wake if the date has not changed; if it has, the new day's run
//! waits for its own time (a missed day is skipped). Wall-clock jumps backwards never repeat
//! a run of a recorded date; a date that differs from the recorded one (also backwards) counts
//! as a new day. Deadlines and the quiet/retry windows use `Instant` (immune to clock changes).
//!
//! # Runtime
//!
//! [`start`] runs one scheduler thread: every [`TICK`] (or when [`Runtime::wake`]d, e.g. by
//! `handoff_done`) it [`observe`]s the app, calls `step` and [`perform`]s the actions; it ends
//! on [`Runtime::stop`] (app exit). Locks: `loaded(read)`, `ws`, `last_input` and the
//! machine lock (`AppState::nightly`, leaf) are taken one at a time and released before any
//! action runs; actions (`bridge::send`, `launcher::execute`, the inbox, the file write) run
//! with no lock held.

use crate::config::Config;
use crate::control::{Caller, StateHost};
use crate::roles::Role;
use crate::workspace::{PaneId, PaneStatus, TabId, Workspace, WsCommand};
use crate::AppState;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Scheduler period.
pub const TICK: Duration = Duration::from_secs(60);
/// A start postponed because the user is typing in the dispatcher is retried after this.
pub const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);
/// Every nightly message starts with this (lets a restart drop its undelivered ones).
pub const MESSAGE_MARK: &str = "[AgentBoard] 야간 정리";
/// File (under `%APPDATA%/agentboard`) with the last run date.
pub const FILE_NAME: &str = "nightly.json";

// ───────────────────────── inputs / outputs ─────────────────────────

/// Nightly settings from `config.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `None` = nightly refresh off (`"nightlyRefresh": null`).
    pub at: Option<NaiveTime>,
    pub timeout: Duration,
    pub quiet: Duration,
    /// `handoverIndex` (relative to workspaceRoot), named in handover's message.
    pub handover_index: String,
}

impl Settings {
    pub fn from_config(c: &Config) -> Self {
        Settings {
            at: c.nightly_refresh.as_deref().and_then(|s| NaiveTime::parse_from_str(s, "%H:%M").ok()),
            timeout: Duration::from_secs(u64::from(c.nightly_timeout_min.max(1)) * 60),
            quiet: Duration::from_secs(u64::from(c.nightly_quiet_min) * 60),
            handover_index: c.handover_index_path().to_string(),
        }
    }
}

/// A standing agent pane as observed now.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub pane: PaneId,
    pub role: Role,
    /// Session name (`@<name>`; the pane name if it has none).
    pub name: String,
    pub tab: TabId,
    pub tab_name: String,
    /// Process status is `Running`.
    pub running: bool,
}

/// Standing role terminal panes (Dispatcher, Handover, Manager) of `ws`, any status. Team
/// panes (Lead/Member), panes without a role and applets/documents are never targets.
pub fn targets(ws: &Workspace) -> Vec<Target> {
    ws.panes
        .values()
        .filter(|p| !p.kind.is_applet() && p.role.is_some_and(|r| r.is_standing()))
        .map(|p| Target {
            pane: p.id.clone(),
            role: p.role.unwrap(),
            name: p.session_name.clone().unwrap_or_else(|| p.name.clone()),
            tab: p.tab.clone(),
            tab_name: ws.tabs.get(&p.tab).map(|t| t.name.clone()).unwrap_or_default(),
            running: p.status == PaneStatus::Running,
        })
        .collect()
}

/// What one step sees.
#[derive(Debug, Clone)]
pub struct World {
    /// Local wall-clock time.
    pub local: NaiveDateTime,
    /// Monotonic time (deadlines, quiet windows).
    pub mono: Instant,
    pub settings: Settings,
    pub targets: Vec<Target>,
    /// Last real keystroke per target pane.
    pub last_input: BTreeMap<PaneId, Instant>,
}

impl World {
    fn typed_recently(&self, pane: &str) -> bool {
        let quiet = self.settings.quiet;
        !quiet.is_zero()
            && self.last_input.get(pane).is_some_and(|t| self.mono.saturating_duration_since(*t) < quiet)
    }

    fn target(&self, pane: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.pane == pane)
    }
}

/// Work for the runtime (performed in order, with no lock held).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Persist the date of the run that is starting (`nightly.json`).
    Record(NaiveDate),
    /// `bridge::send` the text to the pane's session.
    Notify { pane: PaneId, text: String },
    /// Inbox system item "야간 정리 미완료: <name>" for `tab`, `reason` in `check`.
    Incomplete { pane: PaneId, tab: TabId, tab_name: String, name: String, reason: String },
    /// `WsCommand::RestartPaneFresh` (after dropping the pane's undelivered nightly messages).
    Restart { pane: PaneId },
}

/// Phase of the machine (see module docs). `Notifying` / `Restarting` are transient.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Notifying,
    Waiting { pending: BTreeSet<PaneId>, deadline: Instant },
    Restarting,
}

/// Reply of `handoff_done`.
#[derive(Debug, Clone, PartialEq)]
pub enum Handoff {
    /// Recorded; `remaining` targets are still pending.
    Accepted { remaining: usize },
    /// This pane already reported in this run (idempotent).
    Already,
    /// A run is in progress but this pane is not one of its targets.
    NotTarget,
    /// No run in progress: nothing to do.
    NotRunning,
}

impl Handoff {
    /// Tool result text (Korean).
    pub fn message(&self) -> String {
        match self {
            Handoff::Accepted { remaining: 0 } => {
                "야간 정리 완료 신호를 받았습니다. 곧 이 세션은 새 세션으로 다시 시작됩니다.".into()
            }
            Handoff::Accepted { remaining } => format!(
                "야간 정리 완료 신호를 받았습니다(남은 대상 {remaining}개). 모두 끝나거나 제한 시간이 지나면 새 세션으로 다시 시작됩니다."
            ),
            Handoff::Already => "이미 야간 정리 완료 신호를 받았습니다. 더 할 일은 없습니다.".into(),
            Handoff::NotTarget => "이 세션은 이번 야간 정리 대상이 아닙니다(정리 시작 뒤에 실행됨). 아무 것도 하지 않았습니다.".into(),
            Handoff::NotRunning => "지금은 야간 정리 중이 아닙니다. 아무 것도 하지 않았습니다.".into(),
        }
    }
}

// ───────────────────────── messages ─────────────────────────

/// To Managers and the dispatcher. `handovers`: session names of the running handover panes.
pub fn worker_message(handovers: &[String]) -> String {
    if handovers.is_empty() {
        format!(
            "{MESSAGE_MARK}: 작업 계획·진행 내역을 갱신하고, 팀 작업을 방해하지 않는 범위에서 저장한 뒤 handoff_done을 호출하라. (실행 중인 handover 세션이 없어 파일 위치 보고는 생략한다)"
        )
    } else {
        let at: Vec<String> = handovers.iter().map(|h| format!("@{h}")).collect();
        format!(
            "{MESSAGE_MARK}: 작업 계획·진행 내역을 갱신하고, 팀 작업을 방해하지 않는 범위에서 저장한 뒤 handover({})에게 파일 위치를 보고하고 handoff_done을 호출하라.",
            at.join(", ")
        )
    }
}

/// To handover, after the workers (or at half the timeout).
pub fn handover_message(index: &str) -> String {
    format!(
        "{MESSAGE_MARK}: 받은 보고대로 인덱스 문서({index})의 포인터만 갱신하고 handoff_done을 호출하라. 아직 보고하지 않은 업무는 기존 포인터를 그대로 둔다."
    )
}

fn timeout_reason(timeout: Duration) -> String {
    format!(
        "제한 시간 {}분 안에 handoff_done을 호출하지 않았습니다 — 작업 계획·진행 내역이 저장·보고되지 않았을 수 있습니다. 세션은 새 세션으로 다시 시작했습니다.",
        timeout.as_secs() / 60
    )
}

const EXITED_REASON: &str =
    "야간 정리 중 세션이 끝나 handoff_done을 받지 못했습니다 — 창은 다시 시작하지 않았습니다(창에서 Enter로 다시 시작).";

// ───────────────────────── the machine ─────────────────────────

/// Bookkeeping of the run in progress.
#[derive(Debug, Clone)]
struct Run {
    /// Targets at start (running then), in start order.
    targets: Vec<Target>,
    half: Instant,
    handover_notified: bool,
    /// Targets without `handoff_done` when the run moved to `Restarting`.
    not_done: BTreeSet<PaneId>,
}

/// The nightly state machine (pure).
#[derive(Debug, Clone)]
pub struct Nightly {
    last_run: Option<NaiveDate>,
    retry_at: Option<Instant>,
    phase: Phase,
    run: Option<Run>,
}

impl Nightly {
    /// `last_run`: date from `nightly.json` (`None` = never ran / unreadable file).
    pub fn new(last_run: Option<NaiveDate>) -> Self {
        Nightly { last_run, retry_at: None, phase: Phase::Idle, run: None }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn last_run(&self) -> Option<NaiveDate> {
        self.last_run
    }

    /// Replace the last run date (the scheduler thread loads the file after `new`). Ignored
    /// while a run is in progress.
    pub fn set_last_run(&mut self, d: Option<NaiveDate>) {
        if self.phase == Phase::Idle {
            self.last_run = d;
        }
    }

    /// `handoff_done` from `pane`.
    pub fn handoff(&mut self, pane: &str) -> Handoff {
        let (Phase::Waiting { pending, .. }, Some(run)) = (&mut self.phase, &self.run) else {
            return Handoff::NotRunning;
        };
        if pending.remove(pane) {
            Handoff::Accepted { remaining: pending.len() }
        } else if run.targets.iter().any(|t| t.pane == pane) {
            Handoff::Already
        } else {
            Handoff::NotTarget
        }
    }

    /// Whether a run should start at `w` (ignoring the dispatcher's keystrokes).
    fn due(&self, w: &World) -> bool {
        let Some(at) = w.settings.at else { return false };
        self.last_run != Some(w.local.date())
            && w.local.time() >= at
            && self.retry_at.is_none_or(|r| w.mono >= r)
    }

    /// Run every transition due at `w`; returns the actions to perform, in order.
    pub fn step(&mut self, w: &World) -> Vec<Action> {
        let mut out = Vec::new();
        loop {
            match &mut self.phase {
                Phase::Idle => {
                    if !self.due(w) {
                        break;
                    }
                    let dispatcher_typing =
                        w.targets.iter().any(|t| t.role == Role::Dispatcher && w.typed_recently(&t.pane));
                    if dispatcher_typing {
                        self.retry_at = Some(w.mono + RETRY_AFTER);
                        break;
                    }
                    let date = w.local.date();
                    self.retry_at = None;
                    self.last_run = Some(date);
                    out.push(Action::Record(date));
                    self.phase = Phase::Notifying;
                }
                Phase::Notifying => {
                    let targets: Vec<Target> = w.targets.iter().filter(|t| t.running).cloned().collect();
                    if targets.is_empty() {
                        self.phase = Phase::Idle;
                        break;
                    }
                    let handovers: Vec<String> =
                        targets.iter().filter(|t| t.role == Role::Handover).map(|t| t.name.clone()).collect();
                    let text = worker_message(&handovers);
                    for t in targets.iter().filter(|t| t.role != Role::Handover) {
                        out.push(Action::Notify { pane: t.pane.clone(), text: text.clone() });
                    }
                    let pending = targets.iter().map(|t| t.pane.clone()).collect();
                    self.run = Some(Run {
                        half: w.mono + w.settings.timeout / 2,
                        targets,
                        handover_notified: false,
                        not_done: BTreeSet::new(),
                    });
                    self.phase = Phase::Waiting { pending, deadline: w.mono + w.settings.timeout };
                }
                Phase::Waiting { pending, deadline } => {
                    let run = self.run.as_mut().expect("a run while waiting");
                    // a target that exited or was closed no longer blocks (it stays not done)
                    let live = |p: &PaneId| w.target(p).is_some_and(|t| t.running);
                    let blocking: BTreeSet<&PaneId> = pending.iter().filter(|p| live(p)).collect();
                    let worker = |p: &&PaneId| run.targets.iter().any(|t| &t.pane == *p && t.role != Role::Handover);
                    if !run.handover_notified && (!blocking.iter().any(worker) || w.mono >= run.half) {
                        run.handover_notified = true;
                        let text = handover_message(&w.settings.handover_index);
                        for t in run.targets.iter().filter(|t| t.role == Role::Handover) {
                            if pending.contains(&t.pane) && live(&t.pane) {
                                out.push(Action::Notify { pane: t.pane.clone(), text: text.clone() });
                            }
                        }
                    }
                    if !blocking.is_empty() && w.mono < *deadline {
                        break;
                    }
                    // never restart over what the user is typing: wait for a quiet tick
                    if run.targets.iter().any(|t| live(&t.pane) && w.typed_recently(&t.pane)) {
                        break;
                    }
                    run.not_done = std::mem::take(pending);
                    self.phase = Phase::Restarting;
                }
                Phase::Restarting => {
                    let run = self.run.take().expect("a run while restarting");
                    for t in &run.targets {
                        let Some(now) = w.target(&t.pane) else { continue }; // closed: forgotten
                        if run.not_done.contains(&t.pane) {
                            let reason = if now.running { timeout_reason(w.settings.timeout) } else { EXITED_REASON.to_string() };
                            out.push(Action::Incomplete {
                                pane: t.pane.clone(),
                                tab: now.tab.clone(),
                                tab_name: now.tab_name.clone(),
                                name: now.name.clone(),
                                reason,
                            });
                        }
                        if now.running {
                            out.push(Action::Restart { pane: t.pane.clone() });
                        }
                    }
                    self.phase = Phase::Idle;
                    break;
                }
            }
        }
        out
    }
}

// ───────────────────────── nightly.json ─────────────────────────

/// Last run date from `path`; missing, unreadable or corrupt → `None` (never ran).
pub fn load_last_run(path: &Path) -> Option<NaiveDate> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    NaiveDate::parse_from_str(v.get("lastRun")?.as_str()?, "%Y-%m-%d").ok()
}

/// Write `{"lastRun":"YYYY-MM-DD"}` atomically (`<path>.tmp`, fsync, rename).
pub fn save_last_run(path: &Path, date: NaiveDate) -> Result<(), String> {
    use std::io::Write;
    let text = serde_json::to_string_pretty(&json!({ "lastRun": date.format("%Y-%m-%d").to_string() }))
        .map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("폴더를 만들 수 없습니다: {} — {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("야간 정리 기록을 저장할 수 없습니다: {} — {e}", path.display())
    })
}

// ───────────────────────── runtime ─────────────────────────

/// `AppState::nightly`: the machine (leaf lock) and the scheduler's wake/stop signals.
pub struct Runtime {
    machine: Mutex<Nightly>,
    wake: Mutex<bool>,
    cv: Condvar,
    stop: AtomicBool,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime { machine: Mutex::new(Nightly::new(None)), wake: Mutex::new(false), cv: Condvar::new(), stop: AtomicBool::new(false) }
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current phase (clone).
    pub fn phase(&self) -> Phase {
        self.machine.lock().unwrap().phase().clone()
    }

    /// `handoff_done`: record (machine lock, released), then wake the scheduler.
    pub fn handoff(&self, pane: &str) -> Handoff {
        let r = self.machine.lock().unwrap().handoff(pane);
        if matches!(r, Handoff::Accepted { .. }) {
            self.wake();
        }
        r
    }

    /// Run the scheduler's next tick now.
    pub fn wake(&self) {
        *self.wake.lock().unwrap() = true;
        self.cv.notify_all();
    }

    /// End the scheduler thread (app exit).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Wait for a wake or `timeout`.
    fn wait(&self, timeout: Duration) {
        let mut w = self.wake.lock().unwrap();
        if !*w {
            w = self.cv.wait_timeout(w, timeout).unwrap().0;
        }
        *w = false;
    }

    fn set_last_run(&self, d: Option<NaiveDate>) {
        self.machine.lock().unwrap().set_last_run(d);
    }
}

/// The MCP tool `handoff_done` (roles are checked by `mcp_tools::call`: Dispatcher, Handover
/// and Manager only). Always `Ok` — outside a run it is a harmless no-op with a message.
pub fn handoff_done(state: &AppState, caller: &Caller) -> Result<Value, String> {
    Ok(json!(state.nightly.handoff(&caller.pane).message()))
}

/// Everything `step` needs, read from the app (locks one at a time, all released).
pub fn observe(state: &AppState, local: NaiveDateTime, mono: Instant) -> World {
    let settings = Settings::from_config(&state.loaded.read().unwrap().config);
    let targets = targets(&state.ws.lock().unwrap());
    let last_input = {
        let li = state.last_input.lock().unwrap();
        targets.iter().filter_map(|t| li.get(&t.pane).map(|i| (t.pane.clone(), *i))).collect()
    };
    World { local, mono, settings, targets, last_input }
}

/// Perform `actions` with no lock held. `file`: where `Record` writes (`None` in some tests).
pub fn perform(state: &AppState, file: Option<&Path>, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Record(date) => {
                if let Some(f) = file {
                    if let Err(e) = save_last_run(f, date) {
                        eprintln!("[agentboard] {e}");
                    }
                }
            }
            Action::Notify { pane, text } => {
                if let Err(e) = crate::bridge::send(state, &pane, &text) {
                    eprintln!("[agentboard] 야간 정리 알림 실패 ({pane}): {e}");
                }
            }
            Action::Incomplete { tab, tab_name, name, reason, .. } => {
                let title = format!("야간 정리 미완료: {name}");
                if let Err(e) = state.inbox.post_system(&tab, &tab_name, &title, vec![reason], Vec::new()) {
                    eprintln!("[agentboard] 인박스 시스템 항목을 만들 수 없습니다: {e}");
                }
            }
            Action::Restart { pane } => {
                // a nightly message the old session never got must not reach the new one
                crate::bridge::discard(state, &pane, |t| t.starts_with(MESSAGE_MARK));
                if let Err(e) = crate::launcher::execute(state, WsCommand::RestartPaneFresh { pane: pane.clone() }) {
                    eprintln!("[agentboard] 야간 정리 재시작 실패 ({pane}): {e}");
                }
            }
        }
    }
}

/// One scheduler round at (`local`, `mono`).
pub fn tick(state: &AppState, file: Option<&Path>, local: NaiveDateTime, mono: Instant) {
    let world = observe(state, local, mono);
    let actions = state.nightly.machine.lock().unwrap().step(&world); // released here
    perform(state, file, actions);
}

/// Start the scheduler thread: loads `file`'s last run date, then ticks every [`TICK`] (and on
/// wake) until [`Runtime::stop`].
pub fn start<H: StateHost>(host: H, file: PathBuf) {
    let spawned = std::thread::Builder::new().name("agentboard-nightly".into()).spawn(move || {
        let last = load_last_run(&file);
        if let Some(s) = host.get() {
            s.nightly.set_last_run(last);
        }
        loop {
            let Some(state) = host.get() else {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            state.nightly.wait(TICK);
            if state.nightly.stopped() {
                break;
            }
            tick(state, Some(&file), chrono::Local::now().naive_local(), Instant::now());
        }
    });
    if let Err(e) = spawned {
        eprintln!("[agentboard] 야간 정리 스레드를 시작할 수 없습니다: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }
    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }
    fn min(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }
    fn settings() -> Settings {
        Settings {
            at: Some(NaiveTime::from_hms_opt(3, 0, 0).unwrap()),
            timeout: min(20),
            quiet: min(10),
            handover_index: "HANDOVER.md".into(),
        }
    }
    fn tgt(pane: &str, role: Role) -> Target {
        Target { pane: pane.into(), role, name: pane.into(), tab: format!("t-{pane}"), tab_name: pane.into(), running: true }
    }
    /// dispatcher `d`, handover `h`, Managers `m1`, `m2`.
    fn standing() -> Vec<Target> {
        vec![tgt("d", Role::Dispatcher), tgt("h", Role::Handover), tgt("m1", Role::Manager), tgt("m2", Role::Manager)]
    }

    struct Clock {
        base: Instant,
        targets: Vec<Target>,
        input: BTreeMap<PaneId, Instant>,
        settings: Settings,
    }
    impl Clock {
        fn new() -> Self {
            Clock { base: Instant::now(), targets: standing(), input: BTreeMap::new(), settings: settings() }
        }
        /// World at local time `local`, `secs` seconds after the clock's base instant.
        fn w(&self, local: &str, secs: u64) -> World {
            World {
                local: at(local),
                mono: self.base + Duration::from_secs(secs),
                settings: self.settings.clone(),
                targets: self.targets.clone(),
                last_input: self.input.clone(),
            }
        }
        fn mono(&self, secs: u64) -> Instant {
            self.base + Duration::from_secs(secs)
        }
    }

    fn notified(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Notify { pane, .. } = a { Some(pane.clone()) } else { None }).collect()
    }
    fn restarted(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Restart { pane } = a { Some(pane.clone()) } else { None }).collect()
    }
    fn incomplete(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Incomplete { pane, .. } = a { Some(pane.clone()) } else { None }).collect()
    }
    fn all_done(n: &mut Nightly, panes: &[&str]) {
        for p in panes {
            assert!(matches!(n.handoff(p), Handoff::Accepted { .. }), "{p}");
        }
    }

    #[test]
    fn runs_once_per_day_after_time() {
        let c = Clock::new();
        let mut n = Nightly::new(None);
        assert!(n.step(&c.w("2026-10-25 02:59", 0)).is_empty(), "before the time");
        assert_eq!(n.phase(), &Phase::Idle);

        let a = n.step(&c.w("2026-10-25 03:00", 60));
        assert_eq!(a.first(), Some(&Action::Record(day("2026-10-25"))), "date recorded before anything else");
        assert_eq!(notified(&a), ["d", "m1", "m2"]);
        assert!(matches!(n.phase(), Phase::Waiting { .. }));
        assert_eq!(n.last_run(), Some(day("2026-10-25")));
        // still waiting: no second start
        assert!(n.step(&c.w("2026-10-25 03:01", 120)).iter().all(|a| !matches!(a, Action::Record(_))));

        all_done(&mut n, &["d", "m1", "m2"]);
        let a = n.step(&c.w("2026-10-25 03:02", 180));
        assert_eq!(notified(&a), ["h"]);
        all_done(&mut n, &["h"]);
        let a = n.step(&c.w("2026-10-25 03:03", 240));
        assert_eq!(restarted(&a), ["d", "h", "m1", "m2"]);
        assert_eq!(n.phase(), &Phase::Idle);

        // the same day never again; the next day only after its own time
        for (local, s) in [("2026-10-25 03:04", 300), ("2026-10-25 23:59", 400), ("2026-10-26 02:59", 500)] {
            assert!(n.step(&c.w(local, s)).is_empty(), "{local}");
        }
        let a = n.step(&c.w("2026-10-26 03:00", 600));
        assert_eq!(a.first(), Some(&Action::Record(day("2026-10-26"))));

        // app started (or PC woke up) after the time: that day's run happens once
        let mut late = Nightly::new(Some(day("2026-10-24")));
        assert_eq!(late.step(&c.w("2026-10-25 15:00", 0)).first(), Some(&Action::Record(day("2026-10-25"))));
        // … but not when nightly.json already says it ran today (crash mid-run, restart)
        let mut again = Nightly::new(Some(day("2026-10-25")));
        assert!(again.step(&c.w("2026-10-25 15:00", 0)).is_empty());
        // a clock set back to an earlier date counts as a new day (differs from the record)
        let mut back = Nightly::new(Some(day("2026-10-25")));
        assert!(!back.step(&c.w("2026-10-20 04:00", 0)).is_empty());
        // switched off
        let mut c2 = Clock::new();
        c2.settings.at = None;
        let mut off = Nightly::new(None);
        assert!(off.step(&c2.w("2026-10-25 15:00", 0)).is_empty());
        // nothing running: the day still counts, nothing else happens
        let mut c3 = Clock::new();
        c3.targets.iter_mut().for_each(|t| t.running = false);
        let mut idle = Nightly::new(None);
        assert_eq!(idle.step(&c3.w("2026-10-25 03:00", 0)), [Action::Record(day("2026-10-25"))]);
        assert_eq!(idle.phase(), &Phase::Idle);
    }

    #[test]
    fn nightly_defers_while_user_typing_and_skips_teams() {
        let mut c = Clock::new();
        // the user typed into the dispatcher 2 minutes before 03:00 (quiet window 10 min)
        c.input.insert("d".into(), c.mono(0));
        let mut n = Nightly::new(None);
        assert!(n.step(&c.w("2026-10-25 03:00", 120)).is_empty(), "postponed");
        assert_eq!(n.phase(), &Phase::Idle);
        assert_eq!(n.last_run(), None, "a postponed run is not recorded");
        // retried 5 minutes later, not before
        assert!(n.step(&c.w("2026-10-25 03:04", 360)).is_empty());
        // 03:05: retry due, but the last keystroke was only 7 minutes ago → postponed again
        assert!(n.step(&c.w("2026-10-25 03:05", 420)).is_empty());
        // 03:10: the user stopped 12 minutes ago
        let a = n.step(&c.w("2026-10-25 03:10", 720));
        assert_eq!(a.first(), Some(&Action::Record(day("2026-10-25"))));
        // keystrokes in a Manager pane do not postpone the start (only the dispatcher's)
        let mut c2 = Clock::new();
        c2.input.insert("m1".into(), c2.mono(0));
        assert!(!Nightly::new(None).step(&c2.w("2026-10-25 03:00", 60)).is_empty());
        // quiet 0 = no grace period
        let mut c3 = Clock::new();
        c3.settings.quiet = Duration::ZERO;
        c3.input.insert("d".into(), c3.mono(60));
        assert!(!Nightly::new(None).step(&c3.w("2026-10-25 03:00", 60)).is_empty());

        // teams, applets and plain panes are never targets
        let ws = team_workspace();
        let ts = targets(&ws);
        let roles: BTreeSet<Role> = ts.iter().map(|t| t.role).collect();
        assert_eq!(roles, [Role::Dispatcher, Role::Handover, Role::Manager].into_iter().collect());
        assert!(ws.panes.values().any(|p| p.role == Some(Role::Lead)) && ws.panes.values().any(|p| p.role == Some(Role::Member)));
        let team: BTreeSet<&str> = ws.panes.values().filter(|p| p.team.is_some()).map(|p| p.id.as_str()).collect();
        let mut c4 = Clock::new();
        c4.targets = ts.clone();
        let mut n = Nightly::new(None);
        let mut acts = n.step(&c4.w("2026-10-25 03:00", 0));
        for t in &ts {
            n.handoff(&t.pane);
        }
        acts.extend(n.step(&c4.w("2026-10-25 03:01", 60)));
        acts.extend(n.step(&c4.w("2026-10-25 03:02", 120)));
        assert_eq!(restarted(&acts).len(), ts.len());
        for a in &acts {
            if let Action::Notify { pane, .. } | Action::Restart { pane } = a {
                assert!(!team.contains(pane.as_str()), "team pane {pane} touched");
            }
        }
    }

    /// dispatcher + handover + Manager `billing` with team `a` (lead + member), all running.
    fn team_workspace() -> Workspace {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        std::fs::write(
            appdata.path().join("settings.json"),
            json!({ "workspaceRoot": root.path().to_string_lossy() }).to_string(),
        )
        .unwrap();
        crate::config::ensure_dotdir(root.path()).unwrap();
        std::fs::write(
            root.path().join(".agentboard/config.json"),
            r#"{"tabs":[{"name":"handover","role":"handover","dir":""},{"name":"billing","role":"manager","dir":"billing"}]}"#,
        )
        .unwrap();
        let loaded = crate::config::load_all(appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let mut ws = Workspace::from_config(&loaded);
        let billing = ws.tabs.values().find(|t| t.name == "billing").unwrap().id.clone();
        for role in [Role::Lead, Role::Member] {
            let cmd = WsCommand::AddTeamPane { tab: billing.clone(), team: "a".into(), role, prompt: Some("go".into()) };
            ws.apply(cmd, &loaded).unwrap();
        }
        let ids: Vec<PaneId> = ws.panes.keys().cloned().collect();
        for id in ids {
            ws.set_status(&id, PaneStatus::Running);
        }
        ws
    }

    #[test]
    fn restarts_after_all_handoffs_or_timeout() {
        let c = Clock::new();
        // (a) everyone reports: restart right away, no inbox items
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["d", "m1", "m2"]);
        n.step(&c.w("2026-10-25 03:01", 60));
        assert!(matches!(n.phase(), Phase::Waiting { pending, .. } if pending.iter().eq(["h"].iter())));
        all_done(&mut n, &["h"]);
        let a = n.step(&c.w("2026-10-25 03:01", 61));
        assert_eq!(restarted(&a), ["d", "h", "m1", "m2"]);
        assert!(incomplete(&a).is_empty());

        // (b) nobody reports: nothing before the deadline, everything at it
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        let a = n.step(&c.w("2026-10-25 03:19", 20 * 60 - 1));
        assert!(restarted(&a).is_empty());
        assert!(matches!(n.phase(), Phase::Waiting { deadline, .. } if *deadline == c.mono(20 * 60)));
        let a = n.step(&c.w("2026-10-25 03:20", 20 * 60));
        assert_eq!(restarted(&a), ["d", "h", "m1", "m2"]);
        assert_eq!(n.phase(), &Phase::Idle);

        // (c) a keystroke in a target pane holds the restart (never kill what the user types)
        let mut c2 = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c2.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["d", "m1", "m2", "h"]);
        c2.input.insert("m1".into(), c2.mono(50));
        assert!(restarted(&n.step(&c2.w("2026-10-25 03:01", 60))).is_empty(), "held");
        assert!(matches!(n.phase(), Phase::Waiting { .. }));
        let a = n.step(&c2.w("2026-10-25 03:11", 50 + 10 * 60));
        assert_eq!(restarted(&a), ["d", "h", "m1", "m2"]);

        // handoff replies: idempotent, harmless outside a run
        assert_eq!(n.handoff("d"), Handoff::NotRunning);
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        assert_eq!(n.handoff("m1"), Handoff::Accepted { remaining: 3 });
        assert_eq!(n.handoff("m1"), Handoff::Already);
        assert_eq!(n.handoff("p99"), Handoff::NotTarget);
    }

    #[test]
    fn timeout_creates_incomplete_inbox_items() {
        let mut c = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["m1"]);
        // m2 exits during the run (not restarted), handover's tab is closed (forgotten)
        c.targets.iter_mut().find(|t| t.pane == "m2").unwrap().running = false;
        c.targets.retain(|t| t.pane != "h");
        n.step(&c.w("2026-10-25 03:05", 300));
        let a = n.step(&c.w("2026-10-25 03:20", 20 * 60));
        assert_eq!(incomplete(&a), ["d", "m2"]);
        assert_eq!(restarted(&a), ["d", "m1"]);
        let d = a.iter().find(|a| matches!(a, Action::Incomplete { pane, .. } if pane == "d")).unwrap();
        let Action::Incomplete { tab, tab_name, name, reason, .. } = d else { unreachable!() };
        assert_eq!((tab.as_str(), tab_name.as_str(), name.as_str()), ("t-d", "d", "d"));
        assert!(reason.contains("20분") && reason.contains("handoff_done"), "{reason}");
        let m2 = a.iter().find(|a| matches!(a, Action::Incomplete { pane, .. } if pane == "m2")).unwrap();
        assert!(matches!(m2, Action::Incomplete { reason, .. } if reason.contains("세션이 끝나")));
        // the item comes before the restart of the same pane
        let pos = |want: &Action| a.iter().position(|x| x == want).unwrap();
        assert!(pos(d) < pos(&Action::Restart { pane: "d".into() }));
    }

    #[test]
    fn handover_notified_after_managers() {
        let c = Clock::new();
        let mut n = Nightly::new(None);
        let a = n.step(&c.w("2026-10-25 03:00", 0));
        assert!(!notified(&a).contains(&"h".to_string()), "handover waits for the workers");
        let text = |a: &[Action], p: &str| {
            a.iter()
                .find_map(|a| match a {
                    Action::Notify { pane, text } if pane == p => Some(text.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(text(&a, "m1"), worker_message(&["h".into()]));
        assert!(text(&a, "m1").contains("handover(@h)") && text(&a, "d").starts_with(MESSAGE_MARK));
        all_done(&mut n, &["m1", "m2"]);
        assert!(notified(&n.step(&c.w("2026-10-25 03:01", 60))).is_empty(), "the dispatcher still works");
        all_done(&mut n, &["d"]);
        let a = n.step(&c.w("2026-10-25 03:02", 120));
        assert_eq!(notified(&a), ["h"]);
        assert_eq!(text(&a, "h"), handover_message("HANDOVER.md"));
        assert!(notified(&n.step(&c.w("2026-10-25 03:03", 180))).is_empty(), "notified once");

        // half the timeout passed: handover goes ahead with what it has
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["m1"]);
        assert!(notified(&n.step(&c.w("2026-10-25 03:09", 10 * 60 - 1))).is_empty());
        assert_eq!(notified(&n.step(&c.w("2026-10-25 03:10", 10 * 60))), ["h"]);

        // handover reported early by itself: not asked again
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["h", "d", "m1", "m2"]);
        let a = n.step(&c.w("2026-10-25 03:01", 60));
        assert!(notified(&a).is_empty());
        assert_eq!(restarted(&a).len(), 4);

        // no Manager / dispatcher running: handover is asked right away; no handover: the
        // workers' message says so
        let mut c2 = Clock::new();
        c2.targets.retain(|t| t.pane == "h");
        assert_eq!(notified(&Nightly::new(None).step(&c2.w("2026-10-25 03:00", 0))), ["h"]);
        let mut c3 = Clock::new();
        c3.targets.retain(|t| t.pane != "h");
        let a = Nightly::new(None).step(&c3.w("2026-10-25 03:00", 0));
        assert_eq!(text(&a, "m1"), worker_message(&[]));
        assert!(!text(&a, "m1").contains('@'));
    }

    #[test]
    fn nightly_json_round_trip_and_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub").join(FILE_NAME);
        assert_eq!(load_last_run(&f), None, "missing = never ran");
        save_last_run(&f, day("2026-10-25")).unwrap();
        assert_eq!(load_last_run(&f), Some(day("2026-10-25")));
        assert!(!f.with_extension("json.tmp").exists(), "no temp file left");
        save_last_run(&f, day("2026-10-26")).unwrap();
        assert_eq!(load_last_run(&f), Some(day("2026-10-26")), "replaced");
        for bad in ["", "{", "[]", r#"{"lastRun":5}"#, r#"{"lastRun":"2026-13-01"}"#, r#"{"other":"2026-10-25"}"#] {
            std::fs::write(&f, bad).unwrap();
            assert_eq!(load_last_run(&f), None, "{bad:?}");
        }
    }

    #[test]
    fn settings_from_config() {
        let mut c = crate::config::Config {
            nightly_refresh: Some("23:30".into()),
            nightly_timeout_min: 15,
            nightly_quiet_min: 0,
            ..Default::default()
        };
        let s = Settings::from_config(&c);
        assert_eq!(s.at, NaiveTime::from_hms_opt(23, 30, 0));
        assert_eq!((s.timeout, s.quiet), (min(15), Duration::ZERO));
        assert_eq!(s.handover_index, "HANDOVER.md");
        c.nightly_refresh = None;
        assert_eq!(Settings::from_config(&c).at, None);
    }
    // ── runtime: a real AppState without Tauri; effects are queued, never run ──

    struct NullSink;
    impl crate::pty::PtySink for NullSink {
        fn output(&self, _: &str, _: u64, _: &[u8]) {}
        fn exited(&self, _: &str, _: u64, _: i32) {}
    }
    struct NullUi;
    impl crate::Ui for NullUi {
        fn workspace_changed(&self, _: &crate::Snapshot) {}
        fn drop_channel(&self, _: &str) {}
    }

    #[test]
    fn tick_notifies_records_and_restarts_fresh() {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        std::fs::write(
            appdata.path().join("settings.json"),
            json!({ "workspaceRoot": root.path().to_string_lossy() }).to_string(),
        )
        .unwrap();
        let loaded = {
            crate::config::ensure_dotdir(root.path()).unwrap();
            std::fs::write(
                root.path().join(".agentboard/config.json"),
                r#"{"tabs":[{"name":"handover","role":"handover","dir":""},{"name":"billing","role":"manager","dir":"billing"}]}"#,
            )
            .unwrap();
            crate::config::load_all(appdata.path())
        };
        let mut ws = Workspace::from_config(&loaded);
        let billing = ws.tabs.values().find(|t| t.name == "billing").unwrap().id.clone();
        for role in [Role::Lead, Role::Member] {
            let cmd = WsCommand::AddTeamPane { tab: billing.clone(), team: "a".into(), role, prompt: Some("go".into()) };
            ws.apply(cmd, &loaded).unwrap();
        }
        let ids: Vec<PaneId> = ws.panes.keys().cloned().collect();
        for id in &ids {
            ws.set_status(id, PaneStatus::Running);
        }
        let ts = targets(&ws);
        let team: Vec<PaneId> = ws.panes.values().filter(|p| p.team.is_some()).map(|p| p.id.clone()).collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let pty = crate::pty::PtyManager::new_per_child_job(std::sync::Arc::new(NullSink));
        let state = AppState::new(
            ws,
            loaded,
            pty,
            std::sync::Arc::new(NullUi),
            tx,
            crate::persist::Saver::disabled(),
            Vec::new(),
            appdata.path().join("run"),
        );
        let file = appdata.path().join(FILE_NAME);
        let pane_of = |r: Role| ts.iter().find(|t| t.role == r).unwrap().clone();
        let (d, h, m) = (pane_of(Role::Dispatcher), pane_of(Role::Handover), pane_of(Role::Manager));
        let caller = |t: &Target| Caller { pane: t.pane.clone(), role: t.role, tab: t.tab.clone(), team: None, session_name: t.name.clone() };
        let base = Instant::now();

        // outside a run: harmless
        let r = crate::mcp_tools::call(&state, &caller(&m), "handoff_done", json!({})).unwrap();
        assert!(r.as_str().unwrap().contains("야간 정리 중이 아닙니다"), "{r}");

        tick(&state, Some(&file), at("2026-10-25 02:59"), base);
        assert_eq!(state.bridge.queued(&m.pane), 0);
        tick(&state, Some(&file), at("2026-10-25 03:00"), base);
        assert_eq!(load_last_run(&file), Some(day("2026-10-25")), "recorded at start");
        assert_eq!((state.bridge.queued(&d.pane), state.bridge.queued(&m.pane), state.bridge.queued(&h.pane)), (1, 1, 0));
        for p in &team {
            assert_eq!(state.bridge.queued(p), 0, "team pane {p} not notified");
        }

        // the workers report through the MCP tool (idempotent)
        for t in [&d, &m, &m] {
            crate::mcp_tools::call(&state, &caller(t), "handoff_done", json!({})).unwrap();
        }
        tick(&state, Some(&file), at("2026-10-25 03:01"), base + min(1));
        assert_eq!(state.bridge.queued(&h.pane), 1, "handover asked after the workers");
        assert!(rx.try_recv().is_err(), "nothing restarted yet");

        // handover never reports: at the deadline everything restarts fresh
        tick(&state, Some(&file), at("2026-10-25 03:20"), base + min(20));
        assert_eq!(state.nightly.phase(), Phase::Idle);
        let mut restarted: Vec<PaneId> = Vec::new();
        while let Ok(fx) = rx.try_recv() {
            assert_eq!(fx.kill.len(), 1);
            assert_eq!(fx.spawn, vec![(fx.kill[0].clone(), true)], "start prompt injected again");
            restarted.push(fx.kill[0].clone());
        }
        let mut want: Vec<PaneId> = ts.iter().map(|t| t.pane.clone()).collect();
        restarted.sort();
        want.sort();
        assert_eq!(restarted, want, "standing panes only");
        assert_eq!(state.bridge.queued(&h.pane), 0, "the undelivered nightly message is dropped");
        let items = state.inbox.list(false).items;
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].title, format!("야간 정리 미완료: {}", h.name));
        assert_eq!(items[0].owner_tab, h.tab);
        assert!(items[0].check[0].contains("handoff_done"));
        assert_eq!(items[0].source, crate::inbox::WHO_APP);

        // the same day: nothing more
        tick(&state, Some(&file), at("2026-10-25 23:00"), base + min(60));
        assert_eq!(state.bridge.queued(&m.pane), 0);
        assert!(rx.try_recv().is_err());
        drop(root);
    }
}
