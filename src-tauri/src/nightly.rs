//! Nightly refresh (spec §7, Task 11): every day at `nightlyRefresh` (default 03:00, local
//! time) the standing agents — dispatcher, handover, Managers — save their state, report
//! their file pointers to handover, call `handoff_done`, and are then restarted as **new
//! sessions** (`WsCommand::RestartPaneFresh`: start prompt injected again, never `--resume`).
//! Team sessions (Lead/Member) and processless panes (applets, documents) are never touched.
//!
//! # State machine ([`Nightly`], pure; time and the observed workspace are passed in)
//!
//! `Idle → Notifying → Waiting{pending, deadline} → Restarting → Idle`. [`Nightly::step`] runs
//! the transitions that are due at `now` and returns the [`Action`]s to perform. `Notifying`
//! is passed through within one step; `Restarting` lasts only while restarts are held (R25).
//! A target is "live" when its status is `Running` or `Starting`.
//!
//! * **Start** (`Idle`): a time is configured, it is a new day (no record, an earlier date, or
//!   a record more than one day in the future), the local time ≥ the configured time, no
//!   retry is pending, and no run started in this process within [`MIN_GAP`] (monotonic: a
//!   wall-clock date correction cannot cause a second run the same real day).
//!   - **R24**: if every live standing pane was spawned (launcher, this generation) at or after
//!     today's time, the sessions are fresh already: the day is recorded, nothing else happens.
//!   - If the dispatcher had a real keystroke (`pty_write` → `AppState::last_input`, never
//!     UserPromptSubmit — R4) within `nightlyQuietMin`, the start is postponed by
//!     [`RETRY_AFTER`] (5 min).
//!   - Starting records the date ([`Action::Record`] → `nightly.json`) **before** anything
//!     else, so a crash mid-run never repeats the run that day.
//! * **Notifying**: targets = live standing panes (exited/failed ones stay as the user left
//!   them). Each target is tied to its spawn generation (bound later for a starting pane).
//!   Managers and the dispatcher ("workers") get the save-and-report message naming the actual
//!   handover session (`@<session>`); handover gets its own message later.
//! * **Waiting**: `pending` = targets without `handoff_done` ([`Nightly::handoff`]). handover is
//!   notified once no worker blocks, or at half of `nightlyTimeoutMin`; its own `handoff_done`
//!   before that message is not counted (`Handoff::NotYet`). A target that exited, was closed
//!   or runs another generation (restarted meanwhile) stops blocking. Ends when nothing blocks
//!   or the deadline (monotonic) has passed.
//! * **Restarting** (first pass), per target in start order, if still in the model: drop its
//!   undelivered nightly messages ([`Action::Discard`]); not done → [`Action::Incomplete`]
//!   ("야간 정리 미완료: <이름>", owner = its tab, reason in `check`: timeout / exited /
//!   restarted meanwhile); live and same generation → [`Action::Restart`] — unless the user
//!   typed into it within `nightlyQuietMin`: then it is **held** (R25) and retried every tick
//!   until quiet; after [`HOLD_CAP`] (60 min) it is left alone and gets [`Action::Held`]
//!   ("야간 정리 보류: <이름>"). Other panes restart right away (per-pane). A held pane that
//!   exits or is restarted meanwhile is dropped. `handoff_done` now answers `Closed`; a session
//!   of another generation always gets `NotTarget`.
//!
//! Clock rules: "today" and "after the time" use local wall-clock time (chrono `Local`, already
//! a dependency). App started after the time → that day's run happens once unless R24 applies.
//! PC asleep at the time → the run happens on wake if the date has not changed; if it has, the
//! new day's run waits for its own time (a missed day is skipped). Deadlines, quiet, retry,
//! hold and gap windows use `Instant` (immune to clock changes).
//!
//! # Runtime
//!
//! [`start`] runs one scheduler thread: every [`TICK`] (or when [`Runtime::wake`]d, e.g. by
//! `handoff_done`) it [`observe`]s the app, calls `step` and [`perform`]s the actions; it ends
//! on [`Runtime::stop`] (`RunEvent::Exit` only — a vetoed `ExitRequested` keeps it running).
//! Locks: `loaded(read)`, `ws`, the PTY `panes` map (`current_gen`), `last_input`, the spawn-time
//! map and the machine lock (`AppState::nightly`, leaves) are taken one at a time and released
//! before any action runs; actions (`bridge::send` / `discard`, `launcher::execute`, the inbox,
//! the file write) run with no lock held. `Restart` is re-checked when performed: the pane must
//! still be `Running` with the observed generation, otherwise it is left alone.

use crate::config::Config;
use crate::control::{Caller, StateHost};
use crate::roles::Role;
use crate::workspace::{PaneId, PaneStatus, TabId, Workspace, WsCommand};
use crate::AppState;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// R25: a restart held for a typing user is given up after this (the pane is left alone).
pub const HOLD_CAP: Duration = Duration::from_secs(60 * 60);
/// Minimum monotonic time between two run starts in one process (wall-clock corrections).
pub const MIN_GAP: Duration = Duration::from_secs(6 * 60 * 60);
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
    /// Process status is `Starting` (a spawn or restart in progress): live, like `running`.
    pub starting: bool,
    /// Current spawn generation (`PtyManager::current_gen`); `None` when not spawned.
    pub gen: Option<u64>,
    /// Local time the current generation was spawned (runtime only, R24); `None` = unknown.
    pub spawned_at: Option<NaiveDateTime>,
}

impl Target {
    /// Running or starting.
    pub fn live(&self) -> bool {
        self.running || self.starting
    }
}

/// Standing role terminal panes (Dispatcher, Handover, Manager) of `ws`, any status. Team
/// panes (Lead/Member), panes without a role and applets/documents are never targets.
/// `gen` / `spawned_at` are left `None` ([`observe`] fills them from the runtime).
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
            starting: p.status == PaneStatus::Starting,
            gen: None,
            spawned_at: None,
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
    /// Persist the date of the run that is starting (or skipped, R24) in `nightly.json`.
    Record(NaiveDate),
    /// `bridge::send` the text to the pane's session.
    Notify { pane: PaneId, text: String },
    /// Drop the pane's undelivered nightly messages (`bridge::discard`), so neither a later
    /// session gets the old request nor `QUEUE_TIMEOUT` adds a second inbox item.
    Discard { pane: PaneId },
    /// Inbox system item "야간 정리 미완료: <name>" for `tab`, `reason` in `check`.
    Incomplete { pane: PaneId, tab: TabId, tab_name: String, name: String, reason: String },
    /// Inbox system item "야간 정리 보류: <name>" (R25: restart given up after [`HOLD_CAP`]).
    Held { pane: PaneId, tab: TabId, tab_name: String, name: String, reason: String },
    /// `WsCommand::RestartPaneFresh`, only if the pane is still `Running` with generation `gen`
    /// when performed (re-checked by [`perform`]).
    Restart { pane: PaneId, gen: Option<u64> },
}

/// Phase of the machine (see module docs). `Notifying` is transient; `Restarting` lasts only
/// while restarts are held for a typing user (R25).
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
    /// A run is in progress but this session is not one of its targets (started after the
    /// run took its targets, or already restarted by it).
    NotTarget,
    /// handover called before its own nightly message: not recorded (it must first collect
    /// the workers' reports).
    NotYet,
    /// The run is past its waiting phase (restarts in progress): nothing recorded.
    Closed,
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
            Handoff::NotTarget => "이 세션은 이번 야간 정리 대상이 아닙니다(정리 시작 뒤에 실행되었거나 이미 새로 시작됨). 아무 것도 하지 않았습니다.".into(),
            Handoff::NotYet => "아직 기록하지 않았습니다 — handover는 앱이 보내는 인덱스 갱신 요청([AgentBoard] 야간 정리 … 인덱스 문서)을 받고 포인터를 갱신한 뒤에 handoff_done을 호출하세요.".into(),
            Handoff::Closed => "이번 야간 정리는 이미 마감되었습니다(모두 완료했거나 제한 시간이 지남). 아무 것도 하지 않았습니다.".into(),
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
        "제한 시간 {}분 안에 handoff_done을 호출하지 않았습니다 — 작업 계획·진행 내역이 저장·보고되지 않았을 수 있습니다. 세션은 새 세션으로 다시 시작합니다(사용자가 그 창에 입력 중이면 입력이 멈춘 뒤).",
        timeout.as_secs() / 60
    )
}

const EXITED_REASON: &str =
    "야간 정리 중 세션이 끝나 handoff_done을 받지 못했습니다 — 창은 다시 시작하지 않았습니다(창에서 Enter로 다시 시작).";
const RESPAWNED_REASON: &str =
    "야간 정리 중 세션이 다시 시작되어(사용자 재시작 등) 이전 세션의 handoff_done을 받지 못했습니다 — 지금 세션은 이미 새 세션이라 다시 시작하지 않았습니다.";

fn held_reason(cap: Duration) -> String {
    format!(
        "사용자가 이 창에 계속 입력 중이어서 재시작을 {}분 미뤘고, 작업을 끊지 않도록 이번 야간 정리에서는 다시 시작하지 않았습니다 — 필요하면 창을 직접 다시 시작하세요.",
        cap.as_secs() / 60
    )
}

// ───────────────────────── the machine ─────────────────────────

/// Bookkeeping of the run in progress.
#[derive(Debug, Clone)]
struct Run {
    /// Targets at start (live then), in start order. `gen` is the generation the run talks
    /// to: taken at start for a running pane, bound later for one that was starting.
    targets: Vec<Target>,
    half: Instant,
    handover_notified: bool,
    /// Targets without `handoff_done` when the run moved to `Restarting`.
    not_done: BTreeSet<PaneId>,
    /// The first `Restarting` pass ran (items, discards, restarts emitted).
    closed: bool,
    /// R25: panes whose restart waits for the user to stop typing, and since when.
    held: BTreeMap<PaneId, Instant>,
}

impl Run {
    /// The pane as observed now, if it is still the session this run talks to and live.
    fn same<'w>(&self, w: &'w World, pane: &str) -> Option<&'w Target> {
        let mine = self.targets.iter().find(|t| t.pane == pane)?;
        w.target(pane).filter(|now| now.live() && !respawned(mine, now))
    }
}

/// The pane now runs another generation than the run's (restarted during the run).
fn respawned(mine: &Target, now: &Target) -> bool {
    mine.gen.is_some() && now.gen.is_some() && mine.gen != now.gen
}

/// The nightly state machine (pure).
#[derive(Debug, Clone)]
pub struct Nightly {
    last_run: Option<NaiveDate>,
    /// Monotonic start of the last run in this process: a wall-clock correction (wrong RTC
    /// date fixed by NTP) cannot produce a second run within [`MIN_GAP`].
    last_start: Option<Instant>,
    retry_at: Option<Instant>,
    phase: Phase,
    run: Option<Run>,
}

impl Nightly {
    /// `last_run`: date from `nightly.json` (`None` = never ran / unreadable file).
    pub fn new(last_run: Option<NaiveDate>) -> Self {
        Nightly { last_run, last_start: None, retry_at: None, phase: Phase::Idle, run: None }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn last_run(&self) -> Option<NaiveDate> {
        self.last_run
    }

    /// Panes whose restart waits for the user to stop typing (R25), in pane order.
    pub fn held(&self) -> Vec<PaneId> {
        self.run.as_ref().map(|r| r.held.keys().cloned().collect()).unwrap_or_default()
    }

    /// What the dashboard shows of the machine: last run date, phase, held panes.
    pub fn view(&self) -> (Option<NaiveDate>, Phase, Vec<PaneId>) {
        (self.last_run, self.phase.clone(), self.held())
    }

    /// Replace the last run date (the scheduler thread loads the file after `new`). Ignored
    /// while a run is in progress.
    pub fn set_last_run(&mut self, d: Option<NaiveDate>) {
        if self.phase == Phase::Idle {
            self.last_run = d;
        }
    }

    /// `handoff_done` from `pane` whose current spawn generation is `gen`.
    pub fn handoff(&mut self, pane: &str, gen: Option<u64>) -> Handoff {
        let Nightly { phase, run, .. } = self;
        let Some(run) = run.as_mut() else { return Handoff::NotRunning };
        let Some(t) = run.targets.iter_mut().find(|t| t.pane == pane) else { return Handoff::NotTarget };
        // R25: a session spawned after the run took this pane (e.g. already restarted by it)
        // is not the one that was asked
        match (t.gen, gen) {
            (Some(a), Some(b)) if a != b => return Handoff::NotTarget,
            (None, Some(b)) => t.gen = Some(b), // was starting at start: bind now
            _ => {}
        }
        let is_handover = t.role == Role::Handover;
        match phase {
            Phase::Waiting { pending, .. } => {
                if !pending.contains(pane) {
                    Handoff::Already
                } else if is_handover && !run.handover_notified {
                    Handoff::NotYet
                } else {
                    pending.remove(pane);
                    Handoff::Accepted { remaining: pending.len() }
                }
            }
            Phase::Restarting => Handoff::Closed,
            _ => Handoff::NotRunning,
        }
    }

    /// Whether a run should start at `w` (ignoring the dispatcher's keystrokes).
    fn due(&self, w: &World) -> bool {
        let Some(at) = w.settings.at else { return false };
        let today = w.local.date();
        // a new day: never ran, an earlier date, or a record more than a day in the future (a
        // clock that once ran far ahead must not block the refresh); a record of today or
        // tomorrow (clock set back a little) does not run again
        let new_day = match self.last_run {
            None => true,
            Some(d) => d < today || d > today.succ_opt().unwrap_or(today),
        };
        new_day
            && w.local.time() >= at
            && self.retry_at.is_none_or(|r| w.mono >= r)
            && self.last_start.is_none_or(|s| w.mono.saturating_duration_since(s) >= MIN_GAP)
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
                    let date = w.local.date();
                    // R24: every live standing session was started after today's time — they
                    // are fresh already; the day counts, nothing else happens
                    let since = date.and_time(w.settings.at.expect("due checked it"));
                    let live: Vec<&Target> = w.targets.iter().filter(|t| t.live()).collect();
                    if !live.is_empty() && live.iter().all(|t| t.spawned_at.is_some_and(|s| s >= since)) {
                        self.retry_at = None;
                        self.last_run = Some(date);
                        out.push(Action::Record(date));
                        break;
                    }
                    let dispatcher_typing =
                        w.targets.iter().any(|t| t.role == Role::Dispatcher && w.typed_recently(&t.pane));
                    if dispatcher_typing {
                        self.retry_at = Some(w.mono + RETRY_AFTER);
                        break;
                    }
                    self.retry_at = None;
                    self.last_run = Some(date);
                    self.last_start = Some(w.mono);
                    out.push(Action::Record(date));
                    self.phase = Phase::Notifying;
                }
                Phase::Notifying => {
                    let targets: Vec<Target> = w
                        .targets
                        .iter()
                        .filter(|t| t.live())
                        // a starting pane's generation is bound once it runs (or reports)
                        .map(|t| Target { gen: if t.running { t.gen } else { None }, ..t.clone() })
                        .collect();
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
                        closed: false,
                        held: BTreeMap::new(),
                    });
                    self.phase = Phase::Waiting { pending, deadline: w.mono + w.settings.timeout };
                }
                Phase::Waiting { pending, deadline } => {
                    let run = self.run.as_mut().expect("a run while waiting");
                    for t in run.targets.iter_mut().filter(|t| t.gen.is_none()) {
                        if let Some(now) = w.target(&t.pane).filter(|n| n.running) {
                            t.gen = now.gen;
                        }
                    }
                    // a target that exited, was closed or restarted no longer blocks (not done)
                    let blocking: BTreeSet<&PaneId> = pending.iter().filter(|p| run.same(w, p).is_some()).collect();
                    let worker = |p: &&PaneId| run.targets.iter().any(|t| &t.pane == *p && t.role != Role::Handover);
                    if !run.handover_notified && (!blocking.iter().any(worker) || w.mono >= run.half) {
                        run.handover_notified = true;
                        let text = handover_message(&w.settings.handover_index);
                        for t in run.targets.iter().filter(|t| t.role == Role::Handover) {
                            if pending.contains(&t.pane) && run.same(w, &t.pane).is_some() {
                                out.push(Action::Notify { pane: t.pane.clone(), text: text.clone() });
                            }
                        }
                    }
                    if !blocking.is_empty() && w.mono < *deadline {
                        break;
                    }
                    run.not_done = std::mem::take(pending);
                    self.phase = Phase::Restarting;
                }
                Phase::Restarting => {
                    let run = self.run.as_mut().expect("a run while restarting");
                    if !run.closed {
                        run.closed = true;
                        for t in &run.targets {
                            let Some(now) = w.target(&t.pane) else { continue }; // closed: forgotten
                            out.push(Action::Discard { pane: t.pane.clone() });
                            let renewed = respawned(t, now);
                            if run.not_done.contains(&t.pane) {
                                let reason = if renewed {
                                    RESPAWNED_REASON.to_string()
                                } else if !now.live() {
                                    EXITED_REASON.to_string()
                                } else {
                                    timeout_reason(w.settings.timeout)
                                };
                                out.push(Action::Incomplete {
                                    pane: t.pane.clone(),
                                    tab: now.tab.clone(),
                                    tab_name: now.tab_name.clone(),
                                    name: now.name.clone(),
                                    reason,
                                });
                            }
                            if now.live() && !renewed {
                                if w.typed_recently(&t.pane) {
                                    // R25: never restart over what the user is typing
                                    run.held.insert(t.pane.clone(), w.mono);
                                } else {
                                    out.push(Action::Restart { pane: t.pane.clone(), gen: now.gen });
                                }
                            }
                        }
                    } else {
                        let held: Vec<(PaneId, Instant)> = run.held.iter().map(|(p, s)| (p.clone(), *s)).collect();
                        for (pane, since) in held {
                            let Some(now) = run.same(w, &pane) else {
                                run.held.remove(&pane); // closed, exited or restarted meanwhile
                                continue;
                            };
                            if !w.typed_recently(&pane) {
                                out.push(Action::Restart { pane: pane.clone(), gen: now.gen });
                                run.held.remove(&pane);
                            } else if w.mono.saturating_duration_since(since) >= HOLD_CAP {
                                out.push(Action::Held {
                                    pane: pane.clone(),
                                    tab: now.tab.clone(),
                                    tab_name: now.tab_name.clone(),
                                    name: now.name.clone(),
                                    reason: held_reason(HOLD_CAP),
                                });
                                run.held.remove(&pane);
                            }
                        }
                    }
                    if run.held.is_empty() {
                        self.run = None;
                        self.phase = Phase::Idle;
                    }
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
    /// R24: local spawn time per pane and generation (set by the launcher; runtime only, not
    /// persisted). Leaf lock.
    spawns: Mutex<HashMap<PaneId, (u64, NaiveDateTime)>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            machine: Mutex::new(Nightly::new(None)),
            wake: Mutex::new(false),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            spawns: Mutex::new(HashMap::new()),
        }
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

    /// [`Nightly::view`] (machine lock, released).
    pub fn view(&self) -> (Option<NaiveDate>, Phase, Vec<PaneId>) {
        self.machine.lock().unwrap().view()
    }

    /// `handoff_done` from `pane` running generation `gen`: record (machine lock, released),
    /// then wake the scheduler.
    pub fn handoff(&self, pane: &str, gen: Option<u64>) -> Handoff {
        let r = self.machine.lock().unwrap().handoff(pane, gen);
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

    /// The launcher spawned generation `gen` of `pane` at local time `at` (R24). Leaf lock.
    pub fn note_spawn(&self, pane: &str, gen: u64, at: NaiveDateTime) {
        self.spawns.lock().unwrap().insert(pane.to_string(), (gen, at));
    }

    /// The pane left the model.
    pub fn forget(&self, pane: &str) {
        self.spawns.lock().unwrap().remove(pane);
    }

    /// Spawn time of `pane` if it was recorded for generation `gen`.
    pub fn spawned_at(&self, pane: &str, gen: Option<u64>) -> Option<NaiveDateTime> {
        let g = gen?;
        self.spawns.lock().unwrap().get(pane).filter(|(sg, _)| *sg == g).map(|(_, t)| *t)
    }
}

/// The MCP tool `handoff_done` (roles are checked by `mcp_tools::call`: Dispatcher, Handover
/// and Manager only). Always `Ok` — outside a run it is a harmless no-op with a message.
pub fn handoff_done(state: &AppState, caller: &Caller) -> Result<Value, String> {
    let gen = state.pty.current_gen(&caller.pane); // the caller's token belongs to the latest spawn
    let r = state.nightly.handoff(&caller.pane, gen);
    if matches!(r, Handoff::Accepted { .. }) {
        state.ui.board_changed(); // the dashboard's pending list shrank
    }
    Ok(json!(r.message()))
}

/// Everything `step` needs, read from the app (locks one at a time, all released).
pub fn observe(state: &AppState, local: NaiveDateTime, mono: Instant) -> World {
    let settings = Settings::from_config(&state.loaded.read().unwrap().config);
    let mut targets = targets(&state.ws.lock().unwrap());
    for t in &mut targets {
        t.gen = state.pty.current_gen(&t.pane); // panes map lock, released
        t.spawned_at = state.nightly.spawned_at(&t.pane, t.gen);
    }
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
            Action::Discard { pane } => {
                crate::bridge::discard(state, &pane, |t| t.starts_with(MESSAGE_MARK));
            }
            Action::Incomplete { tab_name, name, reason, .. } => {
                post_item(state, &tab_name, &format!("야간 정리 미완료: {name}"), reason);
            }
            Action::Held { tab_name, name, reason, .. } => {
                post_item(state, &tab_name, &format!("야간 정리 보류: {name}"), reason);
            }
            Action::Restart { pane, gen } => {
                // re-checked now (minor 3): an exit or a user restart since the observation
                // leaves the pane alone
                let running = state.ws.lock().unwrap().pane_by_id(&pane).is_some_and(|p| p.status == PaneStatus::Running);
                if !running || state.pty.current_gen(&pane) != gen {
                    continue;
                }
                if let Err(e) = crate::launcher::execute(state, WsCommand::RestartPaneFresh { pane: pane.clone() }) {
                    eprintln!("[agentboard] 야간 정리 재시작 실패 ({pane}): {e}");
                }
            }
        }
    }
}

fn post_item(state: &AppState, tab_name: &str, title: &str, reason: String) {
    if let Err(e) = state.inbox.post_system(tab_name, title, vec![reason], Vec::new()) {
        eprintln!("[agentboard] 인박스 시스템 항목을 만들 수 없습니다: {e}");
    }
}

/// One scheduler round at (`local`, `mono`).
pub fn tick(state: &AppState, file: Option<&Path>, local: NaiveDateTime, mono: Instant) {
    let world = observe(state, local, mono);
    let (before, actions, after) = {
        let mut m = state.nightly.machine.lock().unwrap(); // released at the end of the block
        let before = m.view();
        let actions = m.step(&world);
        (before, actions, m.view())
    };
    perform(state, file, actions);
    if before != after {
        state.ui.board_changed(); // phase, pending, held or last run changed (dashboard)
    }
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
                std::thread::sleep(Duration::from_secs(1)); // not managed yet (never in practice)
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
        Target {
            pane: pane.into(),
            role,
            name: pane.into(),
            tab: format!("t-{pane}"),
            tab_name: pane.into(),
            running: true,
            starting: false,
            gen: Some(1),
            spawned_at: None,
        }
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
        a.iter().filter_map(|a| if let Action::Restart { pane, .. } = a { Some(pane.clone()) } else { None }).collect()
    }
    fn discarded(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Discard { pane } = a { Some(pane.clone()) } else { None }).collect()
    }
    fn held(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Held { pane, .. } = a { Some(pane.clone()) } else { None }).collect()
    }
    const DAY: u64 = 24 * 3600;
    fn incomplete(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Incomplete { pane, .. } = a { Some(pane.clone()) } else { None }).collect()
    }
    fn all_done(n: &mut Nightly, panes: &[&str]) {
        for p in panes {
            assert!(matches!(n.handoff(p, Some(1)), Handoff::Accepted { .. }), "{p}");
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
        for (local, s) in [("2026-10-25 03:04", 300), ("2026-10-25 23:59", 21 * 3600), ("2026-10-26 02:59", DAY - 60)] {
            assert!(n.step(&c.w(local, s)).is_empty(), "{local}");
        }
        let a = n.step(&c.w("2026-10-26 03:00", DAY));
        assert_eq!(a.first(), Some(&Action::Record(day("2026-10-26"))));

        // app started (or PC woke up) after the time: that day's run happens once
        let mut late = Nightly::new(Some(day("2026-10-24")));
        assert_eq!(late.step(&c.w("2026-10-25 15:00", 0)).first(), Some(&Action::Record(day("2026-10-25"))));
        // … but not when nightly.json already says it ran today (crash mid-run, restart)
        let mut again = Nightly::new(Some(day("2026-10-25")));
        assert!(again.step(&c.w("2026-10-25 15:00", 0)).is_empty());
        // a clock set back by a day does not repeat the recorded run (minor 4) …
        let mut back = Nightly::new(Some(day("2026-10-25")));
        assert!(back.step(&c.w("2026-10-24 04:00", 0)).is_empty());
        // … but a record far in the future (a clock that once ran ahead) does not block forever
        assert!(!back.step(&c.w("2026-10-20 04:00", 0)).is_empty());
        // a wrong RTC date corrected forward the same real day: no second run within MIN_GAP
        let mut rtc = Nightly::new(None);
        rtc.step(&c.w("2026-10-24 04:00", 0));
        all_done(&mut rtc, &["d", "m1", "m2"]);
        rtc.step(&c.w("2026-10-24 04:01", 60));
        all_done(&mut rtc, &["h"]);
        assert_eq!(restarted(&rtc.step(&c.w("2026-10-24 04:01", 61))).len(), 4);
        assert!(rtc.step(&c.w("2026-10-25 04:05", 3600)).is_empty(), "NTP fixed the date an hour later");
        assert_eq!(rtc.step(&c.w("2026-10-25 10:00", 6 * 3600)).first(), Some(&Action::Record(day("2026-10-25"))));
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
        for t in ts.iter().filter(|t| t.role != Role::Handover) {
            assert!(matches!(n.handoff(&t.pane, None), Handoff::Accepted { .. }));
        }
        acts.extend(n.step(&c4.w("2026-10-25 03:01", 60)));
        for t in ts.iter().filter(|t| t.role == Role::Handover) {
            assert!(matches!(n.handoff(&t.pane, None), Handoff::Accepted { .. }));
        }
        acts.extend(n.step(&c4.w("2026-10-25 03:02", 120)));
        assert_eq!(restarted(&acts).len(), ts.len());
        for a in &acts {
            if let Action::Notify { pane, .. } | Action::Restart { pane, .. } | Action::Discard { pane } = a {
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

        // (c) a keystroke in a target pane holds only that pane's restart (R25)
        let mut c2 = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c2.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["d", "m1", "m2"]);
        n.step(&c2.w("2026-10-25 03:00", 1));
        all_done(&mut n, &["h"]);
        c2.input.insert("m1".into(), c2.mono(50));
        assert_eq!(restarted(&n.step(&c2.w("2026-10-25 03:01", 60))), ["d", "h", "m2"], "m1 held");
        assert_eq!(n.phase(), &Phase::Restarting);
        assert_eq!(n.handoff("m1", Some(1)), Handoff::Closed);
        let a = n.step(&c2.w("2026-10-25 03:11", 50 + 10 * 60));
        assert_eq!(restarted(&a), ["m1"], "restarted once the user stopped");
        assert_eq!(n.phase(), &Phase::Idle);

        // handoff replies: idempotent, harmless outside a run
        assert_eq!(n.handoff("d", Some(1)), Handoff::NotRunning);
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        assert_eq!(n.handoff("m1", Some(1)), Handoff::Accepted { remaining: 3 });
        assert_eq!(n.handoff("m1", Some(1)), Handoff::Already);
        assert_eq!(n.handoff("p99", Some(1)), Handoff::NotTarget);
        // a session of another generation (restarted since the run began) is not the one asked
        assert_eq!(n.handoff("m2", Some(2)), Handoff::NotTarget);
        // handover before its own message (minor 6): not recorded
        assert_eq!(n.handoff("h", Some(1)), Handoff::NotYet);
        assert!(matches!(n.phase(), Phase::Waiting { pending, .. } if pending.contains("h")));
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
        assert!(pos(d) < pos(&Action::Restart { pane: "d".into(), gen: Some(1) }));
        // undelivered nightly messages are dropped for every target still in the model,
        // exited ones included (minor 1)
        assert_eq!(discarded(&a), ["d", "m1", "m2"]);
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

        // handover reporting before its message is not counted: it is still asked (minor 6)
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        assert_eq!(n.handoff("h", Some(1)), Handoff::NotYet);
        all_done(&mut n, &["d", "m1", "m2"]);
        let a = n.step(&c.w("2026-10-25 03:01", 60));
        assert_eq!(notified(&a), ["h"]);
        assert!(restarted(&a).is_empty());
        all_done(&mut n, &["h"]);
        assert_eq!(restarted(&n.step(&c.w("2026-10-25 03:02", 120))).len(), 4);

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
    fn skips_when_every_session_started_after_the_time() {
        // R24: app started at 08:00 → every standing session spawned after 03:00 today
        let mut c = Clock::new();
        c.targets.iter_mut().for_each(|t| t.spawned_at = Some(at("2026-10-25 08:00")));
        c.targets[1].running = false; // an exited pane does not count either way
        c.targets[1].spawned_at = Some(at("2026-10-24 08:00"));
        let mut n = Nightly::new(Some(day("2026-10-24")));
        assert_eq!(n.step(&c.w("2026-10-25 08:01", 0)), [Action::Record(day("2026-10-25"))]);
        assert_eq!((n.phase(), n.last_run()), (&Phase::Idle, Some(day("2026-10-25"))));
        assert!(n.step(&c.w("2026-10-25 09:00", 3600)).is_empty(), "the day counts as done");
        // one live session older than today's time (or of unknown age): the run happens
        for old in [Some(at("2026-10-25 02:59")), None] {
            let mut c2 = Clock::new();
            c2.targets.iter_mut().for_each(|t| t.spawned_at = Some(at("2026-10-25 08:00")));
            c2.targets[2].spawned_at = old;
            let a = Nightly::new(None).step(&c2.w("2026-10-25 08:01", 0));
            assert_eq!(notified(&a), ["d", "m1", "m2"], "{old:?}");
        }
        // starting panes count as live (spawned now, so fresh)
        let mut c3 = Clock::new();
        c3.targets.iter_mut().for_each(|t| {
            t.running = false;
            t.starting = true;
            t.spawned_at = Some(at("2026-10-25 08:00"));
        });
        assert_eq!(Nightly::new(None).step(&c3.w("2026-10-25 08:01", 0)), [Action::Record(day("2026-10-25"))]);
    }

    #[test]
    fn typing_hold_is_capped_and_never_kills_user_work() {
        // R25: the user keeps typing into m1 → after 60 min it is left alone + "보류" item
        let mut c = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        let t0 = 20 * 60;
        c.input.insert("m1".into(), c.mono(t0 - 1));
        let a = n.step(&c.w("2026-10-25 03:20", t0));
        assert_eq!(restarted(&a), ["d", "h", "m2"]);
        assert_eq!(incomplete(&a), ["d", "h", "m1", "m2"], "nobody reported");
        for k in 1..6 {
            c.input.insert("m1".into(), c.mono(t0 + k * 600 - 1));
            let a = n.step(&c.w("2026-10-25 04:00", t0 + k * 600));
            assert!(a.is_empty(), "still typing after {k}0 min: {a:?}");
        }
        c.input.insert("m1".into(), c.mono(t0 + 3600 - 1));
        let a = n.step(&c.w("2026-10-25 04:20", t0 + 3600));
        assert_eq!(held(&a), ["m1"]);
        assert!(restarted(&a).is_empty(), "never restarted over the user's work");
        let Action::Held { tab, reason, .. } = &a[0] else { panic!("{a:?}") };
        assert_eq!(tab, "t-m1");
        assert!(reason.contains("60분"), "{reason}");
        assert_eq!(n.phase(), &Phase::Idle);

        // a held pane that the user restarts (or that exits) meanwhile is simply dropped
        let mut c = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        c.input.insert("m1".into(), c.mono(t0 - 1));
        n.step(&c.w("2026-10-25 03:20", t0));
        c.targets.iter_mut().find(|t| t.pane == "m1").unwrap().gen = Some(2);
        assert!(n.step(&c.w("2026-10-25 03:21", t0 + 60)).is_empty());
        assert_eq!(n.phase(), &Phase::Idle);
    }

    #[test]
    fn starting_and_restarted_panes_during_a_run() {
        // minor 2: a pane starting at 03:00 is a target (generation bound once it runs)
        let mut c = Clock::new();
        let m2 = c.targets.iter_mut().find(|t| t.pane == "m2").unwrap();
        m2.running = false;
        m2.starting = true;
        m2.gen = Some(7); // the old process of a queued restart, not the session we will talk to
        let mut n = Nightly::new(None);
        assert_eq!(notified(&n.step(&c.w("2026-10-25 03:00", 0))), ["d", "m1", "m2"]);
        let m2 = c.targets.iter_mut().find(|t| t.pane == "m2").unwrap();
        m2.running = true;
        m2.starting = false;
        m2.gen = Some(8);
        n.step(&c.w("2026-10-25 03:01", 60));
        assert_eq!(n.handoff("m2", Some(8)), Handoff::Accepted { remaining: 3 });

        // a target the user restarts during the run (new generation): not blocking, its new
        // session is not asked, an item says why, no second restart
        let mut c = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        all_done(&mut n, &["d", "m2"]);
        let m1 = c.targets.iter_mut().find(|t| t.pane == "m1").unwrap();
        m1.gen = Some(2);
        m1.running = false;
        m1.starting = true;
        assert_eq!(n.handoff("m1", Some(2)), Handoff::NotTarget);
        let a = n.step(&c.w("2026-10-25 03:01", 60));
        assert_eq!(notified(&a), ["h"], "m1 no longer blocks the handover");
        all_done(&mut n, &["h"]);
        let a = n.step(&c.w("2026-10-25 03:02", 120));
        assert_eq!(restarted(&a), ["d", "h", "m2"]);
        let r = a.iter().find_map(|a| match a {
            Action::Incomplete { pane, reason, .. } if pane == "m1" => Some(reason.clone()),
            _ => None,
        });
        assert!(r.unwrap().contains("다시 시작되어"));

        // a pane in Starting at the deadline (same generation) is not reported as exited
        let mut c = Clock::new();
        let mut n = Nightly::new(None);
        n.step(&c.w("2026-10-25 03:00", 0));
        let d = c.targets.iter_mut().find(|t| t.pane == "d").unwrap();
        d.running = false;
        d.starting = true;
        let a = n.step(&c.w("2026-10-25 03:20", 20 * 60));
        let r = a.iter().find_map(|a| match a {
            Action::Incomplete { pane, reason, .. } if pane == "d" => Some(reason.clone()),
            _ => None,
        });
        assert!(!r.unwrap().contains("세션이 끝나"));
        assert!(restarted(&a).contains(&"d".to_string()), "perform re-checks Running before restarting");
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

    /// A real AppState (no Tauri, no processes): dispatcher, handover, Manager `billing` with
    /// team `a` (lead + member), every pane `Running`; effects stay queued in `rx`.
    struct App {
        state: AppState,
        rx: std::sync::mpsc::Receiver<crate::workspace::Effects>,
        ts: Vec<Target>,
        team: Vec<PaneId>,
        root: tempfile::TempDir,
        appdata: tempfile::TempDir,
    }

    fn app() -> App {
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
        App { state, rx, ts, team, root, appdata }
    }

    #[test]
    fn perform_rechecks_restart_and_discards_for_exited_panes() {
        let App { state, rx, ts, root, appdata: _appdata, .. } = app();
        let m = ts.iter().find(|t| t.role == Role::Manager).unwrap().clone();
        let d = ts.iter().find(|t| t.role == Role::Dispatcher).unwrap().clone();
        // minor 3: generation or status changed since the step → no restart
        perform(&state, None, vec![Action::Restart { pane: m.pane.clone(), gen: Some(42) }]);
        assert!(rx.try_recv().is_err(), "another generation now: left alone");
        state.ws.lock().unwrap().set_status(&m.pane, PaneStatus::Exited { code: 0 });
        perform(&state, None, vec![Action::Restart { pane: m.pane.clone(), gen: None }]);
        assert!(rx.try_recv().is_err(), "exited meanwhile: left alone");
        perform(&state, None, vec![Action::Restart { pane: d.pane.clone(), gen: None }]);
        let fx = rx.try_recv().expect("still running, same generation: restarted");
        assert_eq!(fx.spawn, vec![(d.pane.clone(), true)]);
        // minor 1: an exited pane's queued nightly message is dropped, other messages stay
        crate::bridge::send(&state, &m.pane, &worker_message(&[])).unwrap();
        crate::bridge::send(&state, &m.pane, "[AgentBoard] 인박스 지시 — x").unwrap();
        perform(&state, None, vec![Action::Discard { pane: m.pane.clone() }]);
        assert_eq!(state.bridge.queued(&m.pane), 1);
        // the "보류" item (R25)
        perform(
            &state,
            None,
            vec![Action::Held { pane: m.pane.clone(), tab: m.tab.clone(), tab_name: m.tab_name.clone(), name: m.name.clone(), reason: "r".into() }],
        );
        let items = state.inbox.list(false).items;
        assert_eq!(items[0].title, format!("야간 정리 보류: {}", m.name));
        assert_eq!(items[0].owner_name, m.tab_name);
        // R24 spawn times: only for the recorded generation
        state.nightly.note_spawn(&m.pane, 5, at("2026-10-25 08:00"));
        assert_eq!(state.nightly.spawned_at(&m.pane, Some(5)), Some(at("2026-10-25 08:00")));
        assert_eq!(state.nightly.spawned_at(&m.pane, Some(6)), None);
        assert_eq!(state.nightly.spawned_at(&m.pane, None), None);
        state.nightly.forget(&m.pane);
        assert_eq!(state.nightly.spawned_at(&m.pane, Some(5)), None);
        drop(root);
    }

    #[test]
    fn tick_notifies_records_and_restarts_fresh() {
        let App { state, rx, ts, team, root, appdata } = app();
        let file = appdata.path().join(FILE_NAME);
        let pane_of = |r: Role| ts.iter().find(|t| t.role == r).unwrap().clone();
        let (d, h, m) = (pane_of(Role::Dispatcher), pane_of(Role::Handover), pane_of(Role::Manager));
        let caller = |t: &Target| {
            let tab_name = state.ws.lock().unwrap().tabs[&t.tab].name.clone();
            Caller { pane: t.pane.clone(), role: t.role, tab: t.tab.clone(), tab_name, team: None, session_name: t.name.clone() }
        };
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
        assert_eq!(items[0].owner_name, h.tab_name);
        assert!(items[0].check[0].contains("handoff_done"));
        assert_eq!(items[0].source, crate::inbox::WHO_APP);

        // the same day: nothing more
        tick(&state, Some(&file), at("2026-10-25 23:00"), base + min(60));
        assert_eq!(state.bridge.queued(&m.pane), 0);
        assert!(rx.try_recv().is_err());
        drop(root);
    }
}
