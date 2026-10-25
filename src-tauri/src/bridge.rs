//! App → session message bridge (Task 7): the app sends text INTO a running role session
//! (user instructions from the inbox, nightly signals) without touching its input box.
//!
//! * **Primary path — the session's inbox pipe** (docs/spikes.md "2단계 스파이크" P1). Every
//!   hook of a role session reports `CLAUDE_CODE_MESSAGING_SOCKET`/`_TOKEN`; the
//!   SessionStart handler registers them as a [`SessionReg`] tagged with the pane's current
//!   spawn generation. A message is one connection: the auth line, then one user line whose
//!   content is the cross-session envelope with `from-name="agentboard"` (R3) and a body that
//!   starts with [`PREFIX`]. One JSON object per line, UTF-8, multi-line text JSON-escaped.
//! * **Queue.** [`send`] only enqueues (per-pane FIFO) and nudges the worker thread
//!   ([`start_worker`]), so hook handlers and callers never block on a pipe. A registration
//!   of an older generation is never used (the process was restarted): messages wait for the
//!   new session's SessionStart, which flushes the queue in order. A message still queued
//!   after [`QUEUE_TIMEOUT`] fails (error to [`send_tracked`]'s receiver) and becomes a
//!   system inbox item for the pane's tab.
//! * **PTY fallback (R6)**, only when the pipe is unavailable: no usable registration for
//!   the current generation for [`FALLBACK_AFTER`], the pane idle (Stop seen for this
//!   generation, no PTY output and no UserPromptSubmit since) and no keystroke for
//!   [`INPUT_QUIET`], and the profile submits with bracketed paste (R21). Then the body is
//!   typed with the profile's submit encoding. Otherwise the
//!   message stays queued — the fallback never interleaves with the user's typing.
//! * Busy/idle: Stop → idle; UserPromptSubmit or any PTY output → busy. `last_input` is
//!   updated only from real keystrokes (`pty_write` → [`note_input`]), never from
//!   UserPromptSubmit, which also fires for peer messages (R4).
//!
//! Locks (see launcher.rs header): `Bridge::inner` and `AppState::last_input` are leaves;
//! `Bridge::wake` is a leaf (condvar of the worker). `Bridge::pump_lock` is held only by
//! [`pump`], which under it takes `ws`, `loaded`, the PTY `panes` map, `last_input`, `inner`
//! and the inbox **one at a time** (each released before the next); pipe and PTY writes
//! happen with no lock but `pump_lock`. `launcher::on_output` calls [`on_output`] under the
//! PTY flush lock (`flush_lock -> bridge inner`).
use crate::control::{Caller, HookRouter, StateHost};
use crate::inbox::{self, InboxItem, ItemState};
use crate::profiles::{encode_submit, Submit};
use crate::workspace::{PaneId, TabId};
use crate::AppState;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Every message body starts with this, so the receiving Claude knows the app sent it.
pub const PREFIX: &str = "[AgentBoard] ";
/// Sender name in the cross-session envelope (spike P1; no `"`, `<`, `>`, newline).
pub const FROM_NAME: &str = "agentboard";
/// A message not delivered within this time fails.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(600);
/// How long a message waits for a usable pipe registration before the PTY fallback may run.
pub const FALLBACK_AFTER: Duration = Duration::from_secs(5);
/// The PTY fallback needs this long without a keystroke in the pane.
pub const INPUT_QUIET: Duration = Duration::from_secs(3);
/// Longest accepted message (bytes of UTF-8).
pub const MAX_TEXT: usize = 64 * 1024;
/// Worker tick: expiry and fallback checks run at least this often.
const TICK: Duration = Duration::from_millis(500);
/// How long to retry a busy pipe (all instances in use) before giving up for this round.
const PIPE_BUSY_WAIT: Duration = Duration::from_secs(1);
const ERROR_PIPE_BUSY: i32 = 231;

/// What a role session reported about itself, for one spawn generation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionReg {
    pub pane: PaneId,
    /// `PtyManager` generation the hook came from (its token belongs to the latest spawn).
    pub gen: u64,
    /// Inbox pipe path (`\\.\pipe\…`); empty = no usable pipe (not reported, or it failed).
    pub socket: String,
    /// Messaging token; empty together with `socket`.
    pub token: String,
    /// Stop seen and no output / prompt since.
    pub idle: bool,
}

impl SessionReg {
    fn has_pipe(&self) -> bool {
        !self.socket.is_empty() && !self.token.is_empty()
    }
}

/// How a message reached the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Pipe,
    Pty,
}

/// Result reported once per message by [`send_tracked`]'s receiver.
pub type Outcome = Result<Via, String>;

struct Pending {
    id: u64,
    text: String,
    queued: Instant,
    done: Option<mpsc::Sender<Outcome>>,
}

impl Pending {
    fn finish(self, out: Outcome) {
        if let Some(tx) = self.done {
            let _ = tx.send(out);
        }
    }
}

#[derive(Default)]
struct Inner {
    regs: HashMap<PaneId, SessionReg>,
    queues: HashMap<PaneId, VecDeque<Pending>>,
    next_id: u64,
}

/// Registrations + per-pane queues (`AppState::bridge`).
#[derive(Default)]
pub struct Bridge {
    inner: Mutex<Inner>,
    pump_lock: Mutex<()>,
    wake: Mutex<bool>,
    cv: Condvar,
}

impl Bridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pane's registration (any generation).
    pub fn reg(&self, pane: &str) -> Option<SessionReg> {
        self.inner.lock().unwrap().regs.get(pane).cloned()
    }

    /// Number of messages waiting for the pane.
    pub fn queued(&self, pane: &str) -> usize {
        self.inner.lock().unwrap().queues.get(pane).map_or(0, |q| q.len())
    }

    /// Wake the worker (leaf).
    fn nudge(&self) {
        *self.wake.lock().unwrap() = true;
        self.cv.notify_all();
    }

    /// Worker side: wait for a nudge or `timeout`.
    fn wait(&self, timeout: Duration) {
        let mut w = self.wake.lock().unwrap();
        if !*w {
            w = self.cv.wait_timeout(w, timeout).unwrap().0;
        }
        *w = false;
    }
}

/// Register the bridge's hook handlers (SessionStart, Stop, UserPromptSubmit).
pub fn register_hooks(hooks: &HookRouter) {
    hooks.register("SessionStart", on_session_start);
    hooks.register("Stop", on_stop);
    hooks.register("UserPromptSubmit", on_prompt_submit);
}

// ───────────────────────── wire format ─────────────────────────

/// The message body (R21, the single choke point for both paths): `<` → `‹` and `>` → `›`
/// first, so no text — including Manager-authored inbox fields relayed later — can close the
/// envelope or forge another one; then the [`PREFIX`] unless the text already starts with it.
fn body_of(text: &str) -> String {
    let safe = text.replace('<', "\u{2039}").replace('>', "\u{203A}");
    if safe.starts_with(PREFIX) {
        safe
    } else {
        format!("{PREFIX}{safe}")
    }
}

/// The two lines written to the inbox pipe for `text` (spike P1, exact).
pub fn pipe_lines(token: &str, text: &str) -> String {
    let content = format!("<cross-session-message from-name=\"{FROM_NAME}\">\n{}\n</cross-session-message>", body_of(text));
    let q = |s: &str| serde_json::to_string(s).expect("string serializes");
    format!(
        "{{\"type\":\"auth\",\"token\":{}}}\n{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":{}}}}}\n",
        q(token),
        q(&content)
    )
}

/// Only Windows named pipes are ever opened (a session-reported path must not make the app
/// write into a file).
fn is_pipe_path(p: &str) -> bool {
    p.len() > 9 && p[..9].eq_ignore_ascii_case(r"\\.\pipe\") && !p.contains('/')
}

/// One delivery: connect, write the auth + message lines, close. The server answers nothing.
pub fn write_pipe(socket: &str, token: &str, text: &str) -> Result<(), String> {
    if !is_pipe_path(socket) {
        return Err("메시지 통로 경로가 named pipe가 아닙니다".into());
    }
    let data = pipe_lines(token, text);
    let deadline = Instant::now() + PIPE_BUSY_WAIT;
    let mut f = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(socket) {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("메시지 통로에 연결할 수 없습니다 — {e}")),
        }
    };
    f.write_all(data.as_bytes()).map_err(|e| format!("메시지 통로에 쓸 수 없습니다 — {e}"))
}

// ───────────────────────── sending ─────────────────────────

struct Target {
    name: String,
    tab: TabId,
    tab_name: String,
    submit: Submit,
}

/// The pane must be a role terminal pane in the model. Takes `ws` (released), then `loaded`.
fn target(state: &AppState, pane: &str) -> Result<Target, String> {
    let (p, tab_name) = {
        let ws = state.ws.lock().unwrap();
        let p = ws.pane_by_id(pane).cloned().ok_or_else(|| format!("창을 찾을 수 없습니다: {pane}"))?;
        let tab_name = ws.tabs.get(&p.tab).map(|t| t.name.clone()).unwrap_or_default();
        (p, tab_name)
    };
    if p.kind.is_applet() {
        return Err(format!("애플릿 창에는 메시지를 보낼 수 없습니다: {}", p.name));
    }
    if p.role.is_none() {
        return Err(format!("역할 에이전트 창이 아니어서 메시지를 보낼 수 없습니다: {}", p.name));
    }
    let submit = {
        let loaded = state.loaded.read().unwrap();
        crate::launcher::profile_for(&p, &loaded).map(|pr| pr.submit.clone()).unwrap_or_default()
    };
    Ok(Target { name: p.name, tab: p.tab, tab_name, submit })
}

/// Queue `text` for the role session of `pane` and wake the worker. `Ok` means queued, not
/// delivered; applets, non-role or unknown panes, empty or oversized text → `Err`.
pub fn send(state: &AppState, pane: &str, text: &str) -> Result<(), String> {
    enqueue(state, pane, text, None)
}

/// [`send`] plus a receiver that gets exactly one [`Outcome`]: `Ok(Via)` once the message
/// was written to the session's pipe (`Via::Pipe`; the pipe sends no acknowledgement) or typed
/// into its PTY (`Via::Pty`), or
/// `Err` after [`QUEUE_TIMEOUT`] / when the pane leaves the model.
pub fn send_tracked(state: &AppState, pane: &str, text: &str) -> Result<mpsc::Receiver<Outcome>, String> {
    let (tx, rx) = mpsc::channel();
    enqueue(state, pane, text, Some(tx))?;
    Ok(rx)
}

fn enqueue(state: &AppState, pane: &str, text: &str, done: Option<mpsc::Sender<Outcome>>) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("보낼 메시지가 비어 있습니다".into());
    }
    if text.len() > MAX_TEXT {
        return Err(format!("메시지가 너무 깁니다 (최대 {}KB)", MAX_TEXT / 1024));
    }
    target(state, pane)?;
    {
        let mut g = state.bridge.inner.lock().unwrap();
        g.next_id += 1;
        let id = g.next_id;
        g.queues.entry(pane.to_string()).or_default().push_back(Pending {
            id,
            text: text.to_string(),
            queued: Instant::now(),
            done,
        });
    }
    state.bridge.nudge();
    Ok(())
}

/// Drop the pane's undelivered messages whose text satisfies `pred` (the nightly refresh drops
/// its own before restarting a session, so the new session never gets the old run's request).
/// Tracked ones report `Err`. Returns how many were dropped. Leaf lock (`inner`); a message the
/// worker is writing at this very moment may still be delivered.
pub fn discard(state: &AppState, pane: &str, pred: impl Fn(&str) -> bool) -> usize {
    let dropped: Vec<Pending> = {
        let mut g = state.bridge.inner.lock().unwrap();
        let Some(q) = g.queues.get_mut(pane) else { return 0 };
        let (gone, keep): (VecDeque<Pending>, VecDeque<Pending>) = q.drain(..).partition(|p| pred(&p.text));
        *q = keep;
        gone.into()
    };
    let n = dropped.len();
    for p in dropped {
        p.finish(Err("전달 전에 취소되었습니다".into()));
    }
    n
}

// ───────────────────────── delivery ─────────────────────────

enum Step {
    Wait,
    Pipe { id: u64, socket: String, token: String, text: String },
    Pty { id: u64, text: String },
}

#[derive(Debug, PartialEq)]
enum Decision {
    Wait,
    Pipe,
    Pty,
}

/// What to do with the head message (pure): the pipe of a current-generation registration;
/// else the PTY fallback (R6) once the short wait passed, the pane is idle and the user quiet.
/// `can_type`: the profile submits with bracketed paste — without it every newline would
/// submit a separate prompt, so the fallback is refused (R21) and the message waits.
fn decide(
    reg: Option<&SessionReg>,
    cur_gen: Option<u64>,
    queued: Instant,
    now: Instant,
    last_input: Option<Instant>,
    can_type: bool,
) -> Decision {
    let Some(r) = reg.filter(|r| cur_gen == Some(r.gen)) else { return Decision::Wait };
    if r.has_pipe() {
        return Decision::Pipe;
    }
    let waited = now.saturating_duration_since(queued) >= FALLBACK_AFTER;
    let quiet = last_input.is_none_or(|t| now.saturating_duration_since(t) >= INPUT_QUIET);
    if can_type && waited && r.idle && quiet {
        Decision::Pty
    } else {
        Decision::Wait
    }
}

/// Spawn the worker thread: pumps on every nudge and every [`TICK`].
pub fn start_worker<H: StateHost>(host: H) {
    let spawned = std::thread::Builder::new().name("agentboard-bridge".into()).spawn(move || loop {
        match host.get() {
            Some(state) => {
                state.bridge.wait(TICK);
                pump(state, Instant::now());
            }
            None => std::thread::sleep(TICK),
        }
    });
    if let Err(e) = spawned {
        eprintln!("[agentboard] 메시지 전달 스레드를 시작할 수 없습니다: {e}");
    }
}

/// One delivery round over every queue at time `now` (worker; tests call it directly).
pub fn pump(state: &AppState, now: Instant) {
    let _round = state.bridge.pump_lock.lock().unwrap();
    let panes: Vec<PaneId> = {
        let mut g = state.bridge.inner.lock().unwrap();
        g.queues.retain(|_, q| !q.is_empty());
        g.queues.keys().cloned().collect()
    };
    for pane in panes {
        pump_pane(state, &pane, now);
    }
}

fn pump_pane(state: &AppState, pane: &str, now: Instant) {
    let t = match target(state, pane) {
        Ok(t) => t,
        Err(_) => {
            // the pane left the model (or lost its role): nothing will ever deliver these
            let failed: Vec<Pending> = {
                let mut g = state.bridge.inner.lock().unwrap();
                g.regs.remove(pane);
                g.queues.remove(pane).map(Vec::from).unwrap_or_default()
            };
            for p in failed {
                p.finish(Err(format!("창이 닫혀 메시지를 전달하지 못했습니다: {pane}")));
            }
            return;
        }
    };
    loop {
        let cur_gen = state.pty.current_gen(pane);
        let last_input = state.last_input.lock().unwrap().get(pane).copied();
        let (expired, step) = {
            let mut g = state.bridge.inner.lock().unwrap();
            let mut expired = Vec::new();
            let q = g.queues.entry(pane.to_string()).or_default();
            while q.front().is_some_and(|p| now.saturating_duration_since(p.queued) >= QUEUE_TIMEOUT) {
                expired.extend(q.pop_front());
            }
            let head = q.front().map(|p| (p.id, p.text.clone(), p.queued));
            let step = match head {
                None => Step::Wait,
                Some((id, text, queued)) => {
                    let reg = g.regs.get(pane);
                    match decide(reg, cur_gen, queued, now, last_input, t.submit.bracketed_paste) {
                        Decision::Pipe => {
                            let r = reg.expect("decided on a registration");
                            Step::Pipe { id, socket: r.socket.clone(), token: r.token.clone(), text }
                        }
                        Decision::Pty => Step::Pty { id, text },
                        Decision::Wait => Step::Wait,
                    }
                }
            };
            (expired, step)
        };
        for p in expired {
            expire(state, &t, p);
        }
        match step {
            Step::Wait => return,
            Step::Pipe { id, socket, token, text } => match write_pipe(&socket, &token, &text) {
                Ok(()) => finish(state, pane, id, Via::Pipe),
                Err(e) => {
                    eprintln!("[agentboard] 메시지 전달 실패 ({pane}): {e}");
                    // treat the pipe as gone until a hook reports it again (the fallback may run)
                    let mut g = state.bridge.inner.lock().unwrap();
                    if let Some(r) = g.regs.get_mut(pane).filter(|r| r.socket == socket && cur_gen == Some(r.gen)) {
                        r.socket.clear();
                        r.token.clear();
                    }
                    return;
                }
            },
            Step::Pty { id, text } => {
                // re-checked right before typing: a keystroke since the decision → hold
                let typed = state.last_input.lock().unwrap().get(pane).copied();
                if typed != last_input || state.pty.current_gen(pane) != cur_gen {
                    return;
                }
                let bytes = encode_submit(&body_of(&text), &t.submit);
                match state.pty.write(pane, &bytes) {
                    Ok(()) => {
                        if let Some(r) = state.bridge.inner.lock().unwrap().regs.get_mut(pane) {
                            r.idle = false; // a new turn starts; the next one waits for Stop
                        }
                        finish(state, pane, id, Via::Pty);
                    }
                    Err(e) => eprintln!("[agentboard] 메시지 입력 실패 ({pane}): {e}"),
                }
                return; // at most one typed message per idle period
            }
        }
    }
}

/// Pop the delivered head (only if it is still message `id`) and report it.
fn finish(state: &AppState, pane: &str, id: u64, via: Via) {
    let done = {
        let mut g = state.bridge.inner.lock().unwrap();
        let q = g.queues.get_mut(pane);
        match q {
            Some(q) if q.front().is_some_and(|p| p.id == id) => q.pop_front(),
            _ => None,
        }
    };
    if let Some(p) = done {
        p.finish(Ok(via));
    }
}

fn expire(state: &AppState, t: &Target, p: Pending) {
    let preview: String = p.text.chars().take(300).collect();
    let more = if p.text.chars().count() > 300 { "…" } else { "" };
    let posted = state.inbox.post_system(
        &t.tab,
        &t.tab_name,
        &format!("앱 메시지 전달 실패 — {}", t.name),
        vec![format!("전달하지 못한 메시지: {preview}{more}")],
        vec!["세션이 10분 동안 메시지 통로를 보고하지 않았거나(재시작·hook 실패) 계속 작업 중이었습니다".into()],
    );
    if let Err(e) = posted {
        eprintln!("[agentboard] 인박스 시스템 항목을 만들 수 없습니다: {e}");
    }
    p.finish(Err(format!("{} 창의 세션에 10분 동안 메시지를 전달하지 못했습니다", t.name)));
}

// ───────────────────────── inbox relay (Task 10) ─────────────────────────
//
// The inbox applet's actions go to the owner Manager **through the dispatcher** (spec §5): the
// app records the action on the item and queues one message for the dispatcher's role pane.
// Delivery follows the queue rules above (a starting or restarting dispatcher gets it once its
// session registers; after QUEUE_TIMEOUT it becomes a system inbox item). A dispatcher pane that
// is missing, exited or failed is refused up front with a Korean error (nothing recorded), so
// the applet can tell the user instead of the message silently waiting for a process that will
// never come. Locks: `ws` (released) → inbox → bridge, one at a time.
// The action is recorded before `send`; `send` can then only fail if the dispatcher pane left
// the model in between (a tiny window) — the command still returns that error.

/// The dispatcher's role pane when its process is running or starting.
pub fn dispatcher_pane(state: &AppState) -> Result<PaneId, String> {
    use crate::workspace::PaneStatus;
    let ws = state.ws.lock().unwrap();
    let p = ws
        .panes
        .values()
        .find(|p| p.role == Some(crate::roles::Role::Dispatcher) && !p.kind.is_applet())
        .ok_or("dispatcher 창을 찾을 수 없습니다 — 담당 Manager에게 전달할 수 없습니다")?;
    match p.status {
        PaneStatus::Running | PaneStatus::Starting => Ok(p.id.clone()),
        _ => Err("dispatcher 세션이 실행 중이 아닙니다 — dispatcher 창에서 Enter로 재시작한 뒤 다시 시도하세요".into()),
    }
}

/// Session name of the owner tab's standing agent (`@<session>`); the item's owner name when
/// the tab or its agent is gone.
fn owner_session(state: &AppState, it: &InboxItem) -> String {
    let ws = state.ws.lock().unwrap();
    ws.panes
        .values()
        .find(|p| p.tab == it.owner_tab && p.role.is_some_and(|r| r.is_standing()))
        .and_then(|p| p.session_name.clone())
        .unwrap_or_else(|| it.owner_name.clone())
}

fn open_item(state: &AppState, id: &str) -> Result<InboxItem, String> {
    let it = state.inbox.get(id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {id}"))?;
    if it.state == ItemState::Done {
        return Err(format!("완료된 항목입니다: {id}"));
    }
    Ok(it)
}

fn fits(msg: &str) -> Result<(), String> {
    if msg.len() > MAX_TEXT {
        return Err(format!("전달할 메시지가 너무 깁니다 (최대 {}KB)", MAX_TEXT / 1024));
    }
    Ok(())
}

/// `inbox_instruct`: clean the text, record the instruction (state → in_progress) and queue
/// "[AgentBoard] 인박스 지시 — …" for the dispatcher.
pub fn relay_instruct(state: &AppState, id: &str, text: &str) -> Result<(), String> {
    let text = inbox::clean_instruction(text)?;
    let it = open_item(state, id)?;
    let disp = dispatcher_pane(state)?;
    let msg = inbox::instruct_message(&it, &owner_session(state, &it), &text);
    fits(&msg)?;
    state.inbox.instruct(id, &text)?;
    send(state, &disp, &msg)
}

/// `inbox_request_issue`: record `issue_requested` and ask the dispatcher to have the owner
/// Manager create the issue.
pub fn relay_issue_request(state: &AppState, id: &str) -> Result<(), String> {
    let it = open_item(state, id)?;
    let disp = dispatcher_pane(state)?;
    let msg = inbox::issue_request_message(&it, &owner_session(state, &it));
    fits(&msg)?;
    state.inbox.request_issue(id)?;
    send(state, &disp, &msg)
}

/// After `inbox_add_files` copied `files`: "[AgentBoard] 항목 <id>에 자료 저장: …" to the
/// dispatcher (the `files_added` timeline entry is written by the copy itself).
pub fn notify_files_saved(state: &AppState, id: &str, files: &[String]) -> Result<(), String> {
    let it = state.inbox.get(id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {id}"))?;
    let disp = dispatcher_pane(state)?;
    let msg = inbox::files_saved_message(&it, files);
    fits(&msg)?;
    send(state, &disp, &msg)
}

/// `inbox_confirm_done`: `close_proposed` → `done` (Err otherwise), then tell the owner Manager
/// via the dispatcher. The item stays done if the notice cannot be queued; the returned warning
/// says so (`Ok(None)` = notice queued).
pub fn confirm_done(state: &AppState, id: &str) -> Result<Option<String>, String> {
    state.inbox.confirm_done(id)?;
    let notice = (|| {
        let it = state.inbox.get(id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {id}"))?;
        let disp = dispatcher_pane(state)?;
        send(state, &disp, &inbox::done_message(&it, &owner_session(state, &it)))
    })();
    Ok(notice.err().map(|e| format!("완료로 바꿨지만 담당 Manager에게 알리지 못했습니다 — {e}")))
}

// ───────────────────────── signals ─────────────────────────

/// A real keystroke in `pane` (`pty_write`). Leaf lock.
pub fn note_input(state: &AppState, pane: &str) {
    state.last_input.lock().unwrap().insert(pane.to_string(), Instant::now());
}

/// PTY output of the pane's current generation (`launcher::on_output`, under the flush lock):
/// the session is busy. Leaf lock.
pub fn on_output(state: &AppState, pane: &str) {
    let mut g = state.bridge.inner.lock().unwrap();
    if let Some(r) = g.regs.get_mut(pane) {
        r.idle = false;
    }
}

fn reported_pipe(body: &Value) -> Option<(String, String)> {
    let socket = body.get("socket").and_then(Value::as_str).filter(|s| is_pipe_path(s))?;
    let token = body.get("msgToken").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    Some((socket.to_string(), token.to_string()))
}

/// Update the pane's registration from a hook of its current generation. `fresh` (SessionStart)
/// replaces it; other hooks keep it but fill in a missing pipe (a lost SessionStart report).
fn touch(state: &AppState, caller: &Caller, body: &Value, fresh: bool, idle: bool) {
    // a valid token belongs to the latest spawn of the pane (Task 4), so the hook comes from
    // the current generation; none = no process, nothing to register
    let Some(gen) = state.pty.current_gen(&caller.pane) else { return };
    let pipe = reported_pipe(body);
    let has_pipe = {
        let mut g = state.bridge.inner.lock().unwrap();
        let pane = caller.pane.clone();
        let r = g.regs.entry(pane.clone()).or_insert_with(|| SessionReg {
            pane,
            gen,
            socket: String::new(),
            token: String::new(),
            idle: false,
        });
        if fresh || r.gen != gen {
            *r = SessionReg { pane: r.pane.clone(), gen, socket: String::new(), token: String::new(), idle: false };
        }
        if let Some((socket, token)) = pipe {
            if fresh || !r.has_pipe() {
                r.socket = socket;
                r.token = token;
            }
        }
        r.idle = idle;
        r.has_pipe()
    };
    if has_pipe || idle {
        state.bridge.nudge(); // queued messages may go now
    }
    if cfg!(debug_assertions) {
        eprintln!("[hook] {} {}", body.get("event").and_then(Value::as_str).unwrap_or("?"), caller.pane);
    }
}

/// SessionStart: (re-)register the session's pipe for the current generation; busy.
pub fn on_session_start(state: &AppState, caller: &Caller, body: &Value) {
    touch(state, caller, body, true, false);
}

/// Stop: the turn ended — idle.
pub fn on_stop(state: &AppState, caller: &Caller, body: &Value) {
    touch(state, caller, body, false, true);
}

/// UserPromptSubmit (user input or a peer/app message): busy. Never counts as user input (R4).
pub fn on_prompt_submit(state: &AppState, caller: &Caller, body: &Value) {
    touch(state, caller, body, false, false);
}

#[cfg(test)]
mod tests;
