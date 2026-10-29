//! Per-pane session status for the work-status dashboard (③, dashboard spec §2): fed by the
//! role sessions' hooks — SessionStart (busy, transcript path), UserPromptSubmit (busy, last
//! prompt) and Stop (idle, last reply). Hooks reach the app only from role panes, so only they
//! have entries. The board wraps the bridge/attention handlers of the same events: those run
//! first, unchanged, then the entry is updated.
//!
//! **Generations** (as in the bridge): a hook belongs to the pane's current PTY generation (its
//! token is the latest spawn's); none (no process) = ignored. A hook of a newer generation, or
//! any SessionStart, starts a fresh entry, and [`panes`] leaves out entries of an older
//! generation (a restarted pane shows nothing until its new session reports). The launcher's
//! removal path drops the entry with the pane.
//!
//! The board lock is a leaf: nothing else is taken while it is held.

use crate::control::{Caller, HookRouter};
use crate::nightly::Phase;
use crate::workspace::PaneId;
use crate::AppState;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Characters kept of a prompt / reply.
pub const SNIPPET_MAX: usize = 200;
/// Start of a peer or app message as UserPromptSubmit reports it (spikes.md P1).
const PEER_PREFIX: &str = "<cross-session-message";

/// One pane's status (wire shape: `{idle, lastPrompt, lastReply, contextTokens, updatedAt}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardEntry {
    /// Stop seen and no prompt since.
    pub idle: bool,
    /// First [`SNIPPET_MAX`] characters of the last prompt; a peer message as "메시지: <name>".
    pub last_prompt: Option<String>,
    /// First [`SNIPPET_MAX`] characters of the last assistant message (Stop).
    pub last_reply: Option<String>,
    /// Context size from the transcript tail's last usage, read on Stop ([`Board::context_for`]).
    pub context_tokens: Option<u64>,
    /// Epoch milliseconds of the last hook.
    pub updated_at: u64,
    /// PTY generation the entry belongs to.
    #[serde(skip)]
    pub gen: u64,
    /// The session's transcript (`transcript_path` of its hooks).
    #[serde(skip)]
    pub transcript: Option<PathBuf>,
    /// The session's id (`session_id` of SessionStart; D3a — kept in the app, not on the wire).
    #[serde(skip)]
    pub session_id: Option<String>,
    /// Model of the session: SessionStart's `model`, then the transcript's last assistant model.
    pub model: Option<String>,
}

impl BoardEntry {
    fn fresh(gen: u64) -> Self {
        BoardEntry {
            idle: false,
            last_prompt: None,
            last_reply: None,
            context_tokens: None,
            updated_at: 0,
            gen,
            transcript: None,
            session_id: None,
            model: None,
        }
    }
}

/// Which hook updates an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    SessionStart,
    Prompt,
    Stop,
}

/// Entries per pane (`AppState::board`; leaf lock).
#[derive(Default)]
pub struct Board {
    inner: Mutex<HashMap<PaneId, BoardEntry>>,
    /// Context sizes per transcript ([`Board::context_for`]; leaf lock).
    ctx: Mutex<HashMap<PathBuf, CtxCached>>,
}

impl Board {
    /// The pane's entry, whatever its generation.
    pub fn get(&self, pane: &str) -> Option<BoardEntry> {
        self.inner.lock().unwrap().get(pane).cloned()
    }

    /// The pane left the model.
    pub fn forget(&self, pane: &str) {
        self.inner.lock().unwrap().remove(pane);
    }

    /// Apply hook `kind` of generation `gen` (its `stdin` JSON) at `now_ms`.
    pub fn apply(&self, pane: &str, gen: u64, kind: HookKind, stdin: &Value, now_ms: u64) {
        let text = |key: &str| stdin.get(key).and_then(Value::as_str);
        let mut g = self.inner.lock().unwrap();
        let e = g.entry(pane.to_string()).or_insert_with(|| BoardEntry::fresh(gen));
        if kind == HookKind::SessionStart || e.gen != gen {
            *e = BoardEntry::fresh(gen);
        }
        if let Some(t) = text("transcript_path").filter(|t| !t.is_empty()) {
            e.transcript = Some(PathBuf::from(t));
        }
        if let Some(id) = text("session_id").filter(|t| !t.is_empty()) {
            e.session_id = Some(id.to_string());
        }
        match kind {
            HookKind::SessionStart => {
                e.idle = false;
                if let Some(m) = text("model").filter(|t| !t.is_empty()) {
                    e.model = Some(m.to_string());
                }
            }
            HookKind::Prompt => {
                e.idle = false;
                e.last_prompt = text("prompt").map(summarize_prompt);
            }
            HookKind::Stop => {
                e.idle = true;
                if let Some(r) = text("last_assistant_message") {
                    e.last_reply = Some(snippet(r));
                }
            }
        }
        e.updated_at = now_ms;
    }

    fn all(&self) -> Vec<(PaneId, BoardEntry)> {
        self.inner.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// Register the board's handlers; call after `bridge::register_hooks` and
/// `attention::register_hooks` (each runs the handler it replaces first).
pub fn register_hooks(hooks: &HookRouter) {
    hooks.register("SessionStart", on_session_start);
    hooks.register("UserPromptSubmit", on_prompt_submit);
    hooks.register("Stop", on_stop);
}

/// SessionStart: attention (which runs the bridge's) first, then a fresh entry.
pub fn on_session_start(state: &AppState, caller: &Caller, body: &Value) {
    crate::attention::on_session_start(state, caller, body);
    note(state, caller, body, HookKind::SessionStart);
}

/// UserPromptSubmit: the bridge first, then the last prompt.
pub fn on_prompt_submit(state: &AppState, caller: &Caller, body: &Value) {
    crate::bridge::on_prompt_submit(state, caller, body);
    note(state, caller, body, HookKind::Prompt);
}

/// Stop: the bridge first, then idle and the last reply.
pub fn on_stop(state: &AppState, caller: &Caller, body: &Value) {
    crate::bridge::on_stop(state, caller, body);
    note(state, caller, body, HookKind::Stop);
}

fn note(state: &AppState, caller: &Caller, body: &Value, kind: HookKind) {
    let Some(gen) = state.pty.current_gen(&caller.pane) else { return }; // no process
    let stdin = body.get("stdin").unwrap_or(&Value::Null);
    state.board.apply(&caller.pane, gen, kind, stdin, now_ms());
    // the turn ended: its context size, from the transcript tail (no board lock held while reading)
    if kind == HookKind::Stop {
        if let Some(t) = state.board.get(&caller.pane).and_then(|e| e.transcript) {
            let info = state.board.transcript_for(&t);
            state.board.set_transcript_fields(&caller.pane, gen, info);
        }
    }
    state.ui.board_changed();
}

/// Entries of the panes' current generations (the board lock, then the PtyManager `panes`
/// map, one at a time).
pub fn panes(state: &AppState) -> BTreeMap<PaneId, BoardEntry> {
    state.board.all().into_iter().filter(|(pane, e)| state.pty.current_gen(pane) == Some(e.gen)).collect()
}

/// A prompt for display: a peer/app message → "메시지: <from-name>" (its body can be long and
/// is not the user's words), anything else its first [`SNIPPET_MAX`] characters.
pub(crate) fn summarize_prompt(p: &str) -> String {
    let t = p.trim_start();
    if let Some(rest) = t.strip_prefix(PEER_PREFIX) {
        let tag = rest.split('>').next().unwrap_or_default();
        let from = tag.split("from-name=\"").nth(1).and_then(|s| s.split('"').next()).filter(|s| !s.is_empty());
        return snippet(&format!("메시지: {}", from.unwrap_or("?")));
    }
    snippet(p)
}

fn snippet(s: &str) -> String {
    s.trim().chars().take(SNIPPET_MAX).collect()
}

// ───────── context size ─────────

/// Bytes read from the end of a transcript (as session-board did): the last turn is near the end.
pub const TAIL_BYTES: u64 = 512 * 1024;
/// Transcripts remembered by the context cache (sessions restart nightly: old paths go away).
const CTX_CACHE_MAX: usize = 64;

/// The session's context size from its transcript (`path`): the `usage` of the last assistant
/// message in the tail — input + cache-read + cache-creation tokens, i.e. what the next turn
/// sends. `None` without a readable file or usage in the tail.
/// What the dashboard reads from a transcript's tail (session-board's reading, see
/// [`transcript_info_of`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TranscriptInfo {
    /// The last assistant `usage`: input + cache-read + cache-creation tokens.
    pub context_tokens: Option<u64>,
    /// `message.model` of that same assistant record.
    pub model: Option<String>,
    /// The last `last-prompt` record's `lastPrompt`, else the last user message with plain
    /// text content (tool results are not prompts); summarized like hook prompts.
    pub last_prompt: Option<String>,
    /// The last assistant text block (trimmed, first [`SNIPPET_MAX`] characters).
    pub last_reply: Option<String>,
}

/// Read the last [`TAIL_BYTES`] of a transcript (`None` if unreadable) and pick, walking back
/// from the end, the fields of [`TranscriptInfo`]. Unparsable lines are skipped; the first line
/// of a cut tail is dropped.
pub(crate) fn transcript_info_of(path: &Path) -> Option<TranscriptInfo> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let start = f.metadata().ok()?.len().saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let skip = usize::from(start > 0); // the first line of a tail is cut
    let mut info = TranscriptInfo::default();
    let (mut usage_seen, mut prompt_from_record) = (false, false);
    for line in text.lines().skip(skip).collect::<Vec<_>>().into_iter().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let m = v.get("message");
                if !usage_seen {
                    if let Some(u) = m.and_then(|m| m.get("usage")) {
                        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                        info.context_tokens = Some(n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"));
                        info.model = m.and_then(|m| m.get("model")).and_then(Value::as_str).map(str::to_string);
                        usage_seen = true;
                    }
                }
                if info.last_reply.is_none() {
                    let blocks = m.and_then(|m| m.get("content")).and_then(Value::as_array);
                    let text = blocks.into_iter().flatten().find_map(|b| {
                        (b.get("type").and_then(Value::as_str) == Some("text"))
                            .then(|| b.get("text").and_then(Value::as_str))
                            .flatten()
                            .filter(|t| !t.trim().is_empty())
                    });
                    info.last_reply = text.map(snippet);
                }
            }
            Some("last-prompt") if !prompt_from_record => {
                if let Some(p) = v.get("lastPrompt").and_then(Value::as_str) {
                    info.last_prompt = Some(summarize_prompt(p));
                    prompt_from_record = true;
                }
            }
            Some("user") if info.last_prompt.is_none() => {
                if let Some(p) = v.pointer("/message/content").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                    info.last_prompt = Some(summarize_prompt(p));
                }
            }
            _ => {}
        }
        if usage_seen && info.last_reply.is_some() && prompt_from_record {
            break;
        }
    }
    Some(info)
}

/// Context size only (see [`transcript_info_of`]).
#[cfg(test)]
pub(crate) fn context_tokens_of(path: &Path) -> Option<u64> {
    transcript_info_of(path)?.context_tokens
}

/// A transcript's context size as of its (mtime, size).
struct CtxCached {
    mtime: SystemTime,
    size: u64,
    info: Option<TranscriptInfo>,
}

impl Board {
    /// [`transcript_info_of`], cached by (path, mtime, size): an unchanged transcript is not read
    /// again. The cache lock is a leaf held across the tail read.
    pub(crate) fn transcript_for(&self, path: &Path) -> Option<TranscriptInfo> {
        let meta = std::fs::metadata(path).ok()?;
        let (mtime, size) = (meta.modified().ok()?, meta.len());
        let mut cache = self.ctx.lock().unwrap();
        if let Some(c) = cache.get(path).filter(|c| c.mtime == mtime && c.size == size) {
            return c.info.clone();
        }
        let info = transcript_info_of(path);
        if cache.len() >= CTX_CACHE_MAX {
            cache.clear();
        }
        cache.insert(path.to_path_buf(), CtxCached { mtime, size, info: info.clone() });
        info
    }

    /// The cached context size ([`Board::transcript_for`]).
    pub fn context_for(&self, path: &Path) -> Option<u64> {
        self.transcript_for(path)?.context_tokens
    }

    /// Set the pane's context size and model from its transcript, if its entry is still
    /// generation `gen` (a missing model keeps the one SessionStart reported).
    fn set_transcript_fields(&self, pane: &str, gen: u64, info: Option<TranscriptInfo>) {
        if let Some(e) = self.inner.lock().unwrap().get_mut(pane).filter(|e| e.gen == gen) {
            let info = info.unwrap_or_default();
            e.context_tokens = info.context_tokens;
            if info.model.is_some() {
                e.model = info.model;
            }
        }
    }
}

// ───────── board_status ─────────

/// Event emitted after a hook was applied and after the nightly state changed (no payload);
/// the frontend then calls `board_status`.
pub const BOARD_CHANGED: &str = "board-changed";

/// `board_status().nightly`: `{lastRun: "YYYY-MM-DD"|null, phase, pending, held}`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NightlyStatus {
    pub last_run: Option<String>,
    /// `idle` | `notifying` | `waiting` | `restarting`.
    pub phase: &'static str,
    /// Targets that have not called `handoff_done` yet (waiting phase).
    pub pending: Vec<PaneId>,
    /// Panes whose restart waits for the user to stop typing.
    pub held: Vec<PaneId>,
}

/// The `board_status` reply (IPC contract of the dashboard plan).
#[derive(Debug, Serialize)]
pub struct BoardStatus {
    pub panes: BTreeMap<PaneId, BoardEntry>,
    pub nightly: NightlyStatus,
}

/// Panes of their current generation, plus the nightly machine's view (board lock, PtyManager
/// `panes` map, nightly machine lock — one at a time).
pub fn status(state: &AppState) -> BoardStatus {
    let (last_run, phase, held) = state.nightly.view();
    let (name, pending) = match phase {
        Phase::Idle => ("idle", Vec::new()),
        Phase::Notifying => ("notifying", Vec::new()),
        Phase::Waiting { pending, .. } => ("waiting", pending.into_iter().collect()),
        Phase::Restarting => ("restarting", Vec::new()),
    };
    BoardStatus {
        panes: panes(state),
        nightly: NightlyStatus { last_run: last_run.map(|d| d.format("%Y-%m-%d").to_string()), phase: name, pending, held },
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};
    use crate::pty::{PtyManager, PtySink, SpawnSpec};
    use crate::roles::Role;
    use crate::workspace::{Effects, Workspace};
    use crate::{Snapshot, Ui};
    use serde_json::json;
    use std::sync::{mpsc, Arc};

    struct NullSink;
    impl PtySink for NullSink {
        fn output(&self, _: &str, _: u64, _: &[u8]) {}
        fn exited(&self, _: &str, _: u64, _: i32) {}
    }
    /// Counts `board-changed` emits.
    #[derive(Default)]
    struct CountUi {
        board: std::sync::atomic::AtomicUsize,
    }
    impl Ui for CountUi {
        fn workspace_changed(&self, _: &Snapshot) {}
        fn drop_channel(&self, _: &str) {}
        fn board_changed(&self) {
            self.board.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Manager tab "billing"; `pane` = its role pane.
    struct Fx {
        state: AppState,
        pane: String,
        ui: Arc<CountUi>,
        _rx: mpsc::Receiver<Effects>,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            self.state.pty.remove(&self.pane);
        }
    }

    impl Fx {
        fn caller(&self) -> Caller {
            let ws = self.state.ws.lock().unwrap();
            let p = ws.pane_by_id(&self.pane).unwrap();
            Caller {
                pane: p.id.clone(),
                role: p.role.unwrap(),
                tab: p.tab.clone(),
                tab_name: ws.tabs[&p.tab].name.clone(),
                team: None,
                session_name: p.session_name.clone().unwrap(),
            }
        }
        /// (Re)start the pane's process: a fresh generation.
        fn spawn(&self) -> u64 {
            self.state.pty.remove(&self.pane);
            let spec = SpawnSpec {
                cmd: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into(), "prompt $G".into()],
                env: vec![],
                env_remove: vec![],
                cwd: std::env::temp_dir(),
                cols: 120,
                rows: 30,
            };
            self.state.pty.spawn(&self.pane, spec).unwrap()
        }
        /// Deliver a hook the way the control server does: through the router.
        fn hook(&self, event: &str, stdin: Value) {
            let h = self.state.hooks.get(event).unwrap_or_else(|| panic!("no handler for {event}"));
            h(&self.state, &self.caller(), &json!({"event": event, "stdin": stdin, "socket": null, "msgToken": null, "cwd": "."}));
        }
        fn board_events(&self) -> usize {
            self.ui.board.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn entry(&self) -> BoardEntry {
            panes(&self.state).get(&self.pane).cloned().expect("board entry")
        }
    }

    fn fixture() -> Fx {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), r#"{"tabs":[{"name":"billing","role":"manager","dir":"billing"}]}"#)
            .unwrap();
        let loaded = load_all(appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let ws = Workspace::from_config(&loaded);
        let pane = ws.panes.values().find(|p| p.role == Some(Role::Manager)).expect("manager pane").id.clone();
        let (tx, rx) = mpsc::channel();
        let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
        let ui = Arc::new(CountUi::default());
        let state =
            AppState::new(ws, loaded, pty, ui.clone(), tx, crate::persist::Saver::disabled(), Vec::new(), appdata.path().join("run"));
        Fx { state, pane, ui, _rx: rx, _keep: (root, appdata) }
    }

    #[test]
    fn board_tracks_idle_prompt_and_reply_from_hooks() {
        let f = fixture();
        f.spawn();
        f.hook("SessionStart", json!({"hook_event_name": "SessionStart", "source": "startup", "transcript_path": r"C:\t\a.jsonl"}));
        let e = f.entry();
        assert!(!e.idle);
        assert_eq!(e.transcript.as_deref(), Some(std::path::Path::new(r"C:\t\a.jsonl")));
        assert_eq!((e.last_prompt.as_deref(), e.last_reply.as_deref()), (None, None));
        assert!(e.updated_at > 1_700_000_000_000, "epoch ms");
        f.hook("UserPromptSubmit", json!({"prompt": "결제 오류 회신 확인해줘"}));
        let e = f.entry();
        assert_eq!((e.idle, e.last_prompt.as_deref()), (false, Some("결제 오류 회신 확인해줘")));
        f.hook("Stop", json!({"last_assistant_message": "가".repeat(300)}));
        let e = f.entry();
        assert!(e.idle);
        assert_eq!(e.last_reply.as_deref(), Some("가".repeat(SNIPPET_MAX).as_str()), "first 200 characters");
        assert_eq!(e.last_prompt.as_deref(), Some("결제 오류 회신 확인해줘"), "kept across the turn");
        // the bridge still sees the same hooks (board wraps, does not replace)
        assert!(f.state.bridge.reg(&f.pane).is_some_and(|r| r.idle));
        // the wire shape of one entry (IPC contract)
        let v = serde_json::to_value(&e).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["contextTokens", "idle", "lastPrompt", "lastReply", "model", "updatedAt"]);
    }

    #[test]
    fn board_summarizes_peer_message_prompts() {
        assert_eq!(summarize_prompt("<cross-session-message from-name=\"agentboard\">\n[AgentBoard] 인박스 지시\n</cross-session-message>"), "메시지: agentboard");
        assert_eq!(
            summarize_prompt("<cross-session-message from=\"uds:\\\\.\\pipe\\x\" from-name=\"billing-batch-lead\" from-mode=\"prompting\">\n보고\n</cross-session-message>"),
            "메시지: billing-batch-lead"
        );
        assert_eq!(summarize_prompt("<cross-session-message>\nx\n</cross-session-message>"), "메시지: ?");
        assert_eq!(summarize_prompt("  할 일 정리\n둘째 줄"), "할 일 정리\n둘째 줄");
        let f = fixture();
        f.spawn();
        f.hook("UserPromptSubmit", json!({"prompt": "<cross-session-message from-name=\"agentboard\">\n긴 본문\n</cross-session-message>"}));
        assert_eq!(f.entry().last_prompt.as_deref(), Some("메시지: agentboard"));
    }

    #[test]
    fn board_entry_is_dropped_with_the_pane() {
        let f = fixture();
        f.spawn();
        f.hook("Stop", json!({"last_assistant_message": "끝"}));
        assert!(f.state.board.get(&f.pane).is_some());
        // the pane leaves the model: the launcher's removal path forgets it
        f.state.ws.lock().unwrap().panes.remove(&f.pane);
        crate::launcher::run_effects(&f.state, Effects { spawn: vec![], kill: vec![f.pane.clone()] });
        assert_eq!(f.state.board.get(&f.pane), None);
    }

    /// Same rule as the bridge: a hook belongs to the pane's current generation (its token is
    /// the latest spawn's); a new generation starts a fresh entry, and an entry of an older
    /// generation is not reported.
    #[test]
    fn board_ignores_stale_generation() {
        let f = fixture();
        f.spawn();
        f.hook("UserPromptSubmit", json!({"prompt": "old"}));
        f.hook("Stop", json!({"last_assistant_message": "r1"}));
        f.spawn(); // restart: generation 2, no hook yet
        assert!(!panes(&f.state).contains_key(&f.pane), "the old session's status is not shown for the new one");
        f.hook("Stop", json!({"last_assistant_message": "r2"}));
        let e = f.entry();
        assert_eq!((e.idle, e.last_prompt.as_deref(), e.last_reply.as_deref()), (true, None, Some("r2")));
        // no process (removed from the PtyManager): a late hook changes nothing
        f.state.pty.remove(&f.pane);
        f.hook("UserPromptSubmit", json!({"prompt": "late"}));
        assert_eq!(f.state.board.get(&f.pane).unwrap().last_prompt, None);
    }

    // ---- BC2: context size, nightly phase, board_status, board-changed ----

    fn assistant(input: u64, read: u64, created: u64) -> String {
        json!({"type": "assistant", "message": {"role": "assistant", "model": "m",
            "usage": {"input_tokens": input, "cache_read_input_tokens": read, "cache_creation_input_tokens": created, "output_tokens": 9}}})
        .to_string()
    }

    #[test]
    fn context_from_transcript_tail_uses_last_usage() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("s.jsonl");
        let lines = [
            json!({"type": "user", "message": {"role": "user", "content": "a"}}).to_string(),
            assistant(10, 100, 5),
            assistant(3, 40_000, 200),
            json!({"type": "assistant", "message": {"role": "assistant", "content": []}}).to_string(), // no usage
            "not json".to_string(),
            json!({"type": "user", "message": {"role": "user", "content": "b"}}).to_string(),
        ];
        std::fs::write(&t, lines.join("\n") + "\n").unwrap();
        assert_eq!(context_tokens_of(&t), Some(3 + 40_000 + 200), "the last assistant usage: input + cache read + cache creation");
        assert_eq!(context_tokens_of(&d.path().join("missing.jsonl")), None);
        // only the tail is read: a usage before the last TAIL_BYTES does not count
        let big = d.path().join("big.jsonl");
        let filler = json!({"type": "user", "message": {"content": "x".repeat(1000)}}).to_string();
        let mut text = assistant(1, 1, 1) + "\n";
        while text.len() < TAIL_BYTES as usize + 4096 {
            text.push_str(&filler);
            text.push('\n');
        }
        std::fs::write(&big, &text).unwrap();
        assert_eq!(context_tokens_of(&big), None);
        // Stop fills the entry from the session's transcript
        let f = fixture();
        f.spawn();
        f.hook("SessionStart", json!({"transcript_path": t.to_string_lossy()}));
        assert_eq!(f.entry().context_tokens, None, "read on Stop");
        f.hook("Stop", json!({"last_assistant_message": "끝", "transcript_path": t.to_string_lossy()}));
        assert_eq!(f.entry().context_tokens, Some(40_203));
    }

    /// (path, mtime, size) cache: an unchanged file is not read again.
    #[test]
    fn context_cache_reuses_unchanged_file() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("s.jsonl");
        std::fs::write(&t, assistant(100, 0, 0) + "\n").unwrap();
        let board = Board::default();
        assert_eq!(board.context_for(&t), Some(100));
        let mtime = std::fs::metadata(&t).unwrap().modified().unwrap();
        // same size, same mtime, different content: the cached value is used
        std::fs::write(&t, assistant(200, 0, 0) + "\n").unwrap();
        std::fs::File::options().write(true).open(&t).unwrap().set_modified(mtime).unwrap();
        assert_eq!(board.context_for(&t), Some(100));
        // a real change (size) is read again
        std::fs::write(&t, assistant(2000, 0, 0) + "\n").unwrap();
        assert_eq!(board.context_for(&t), Some(2000));
    }

    #[test]
    fn nightly_phase_is_reported() {
        let f = fixture();
        let empty = serde_json::to_value(status(&f.state)).unwrap();
        assert_eq!(empty["nightly"], json!({"lastRun": null, "phase": "idle", "pending": [], "held": []}));
        assert!(empty["panes"].as_object().unwrap().is_empty());
        // the standing panes (dispatcher, billing's Manager) run: tonight's run starts at 03:00
        let ids: Vec<PaneId> = f.state.ws.lock().unwrap().panes.keys().cloned().collect();
        for id in &ids {
            f.state.ws.lock().unwrap().set_status(id, crate::workspace::PaneStatus::Running);
        }
        let before = f.board_events();
        let local = chrono::NaiveDateTime::parse_from_str("2026-10-25 03:00", "%Y-%m-%d %H:%M").unwrap();
        crate::nightly::tick(&f.state, None, local, std::time::Instant::now());
        let s = serde_json::to_value(status(&f.state)).unwrap();
        assert_eq!(s["nightly"]["lastRun"], "2026-10-25");
        assert_eq!(s["nightly"]["phase"], "waiting");
        let mut pending: Vec<String> = serde_json::from_value(s["nightly"]["pending"].clone()).unwrap();
        pending.sort();
        let mut standing: Vec<PaneId> = {
            let ws = f.state.ws.lock().unwrap();
            ws.panes.values().filter(|p| p.role.is_some()).map(|p| p.id.clone()).collect()
        };
        standing.sort();
        assert_eq!(pending, standing);
        assert_eq!(s["nightly"]["held"], json!([]));
        assert!(f.board_events() > before, "a phase change emits board-changed");
        // a tick without a phase change emits nothing
        let quiet = f.board_events();
        crate::nightly::tick(&f.state, None, local, std::time::Instant::now());
        assert_eq!(f.board_events(), quiet);
    }

    #[test]
    fn board_changed_follows_hooks() {
        let f = fixture();
        f.spawn();
        let n = f.board_events();
        f.hook("UserPromptSubmit", json!({"prompt": "a"}));
        f.hook("Stop", json!({"last_assistant_message": "b"}));
        assert_eq!(f.board_events(), n + 2);
        let s = serde_json::to_value(status(&f.state)).unwrap();
        assert_eq!(s["panes"][&f.pane]["lastReply"], "b");
    }

    // ---- ③ D3a: session id / transcript / model per pane ----

    /// One transcript tail gives what the dashboard shows: context size and model from the last
    /// assistant usage, the last reply from the last assistant text, the last prompt from the
    /// last `last-prompt` record (else the last plain user message) — session-board's reading.
    #[test]
    fn transcript_tail_gives_context_model_last_prompt_and_reply() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("s.jsonl");
        let lines = [
            json!({"type": "user", "message": {"role": "user", "content": "첫 지시"}}).to_string(),
            json!({"type": "assistant", "message": {"role": "assistant", "model": "claude-a",
                "content": [{"type": "text", "text": "  답 하나  "}],
                "usage": {"input_tokens": 1, "cache_read_input_tokens": 2, "cache_creation_input_tokens": 3}}}).to_string(),
            json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "content": "x"}]}}).to_string(),
            json!({"type": "assistant", "message": {"role": "assistant", "model": "claude-b",
                "content": [{"type": "tool_use", "name": "Read"}],
                "usage": {"input_tokens": 10, "cache_read_input_tokens": 20, "cache_creation_input_tokens": 30}}}).to_string(),
        ];
        std::fs::write(&t, lines.join("\n") + "\n").unwrap();
        let i = transcript_info_of(&t).unwrap();
        assert_eq!(i.context_tokens, Some(60), "the last usage, even without text");
        assert_eq!(i.model.as_deref(), Some("claude-b"));
        assert_eq!(i.last_reply.as_deref(), Some("답 하나"), "the last assistant text");
        assert_eq!(i.last_prompt.as_deref(), Some("첫 지시"), "tool results are not prompts");
        // a last-prompt record wins over earlier user messages; peer messages are summarized
        let mut more = lines.join("\n") + "\n";
        more.push_str(&json!({"type": "last-prompt", "lastPrompt": "<cross-session-message from-name=\"agentboard\">\n본문\n</cross-session-message>"}).to_string());
        more.push('\n');
        std::fs::write(&t, more).unwrap();
        assert_eq!(transcript_info_of(&t).unwrap().last_prompt.as_deref(), Some("메시지: agentboard"));
        assert_eq!(transcript_info_of(&d.path().join("missing.jsonl")), None);
    }

    #[test]
    fn session_start_stores_session_id_transcript_and_model() {
        let f = fixture();
        f.spawn();
        f.hook(
            "SessionStart",
            json!({"session_id": "9887ea82-aaaa", "transcript_path": r"C:\t\9887ea82-aaaa.jsonl", "model": "claude-haiku-5-5", "source": "startup"}),
        );
        let e = f.entry();
        assert_eq!(e.session_id.as_deref(), Some("9887ea82-aaaa"));
        assert_eq!(e.transcript.as_deref(), Some(std::path::Path::new(r"C:\t\9887ea82-aaaa.jsonl")));
        assert_eq!(e.model.as_deref(), Some("claude-haiku-5-5"));
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["model"], "claude-haiku-5-5", "model is on the wire");
        assert!(v.get("sessionId").is_none() && v.get("transcript").is_none(), "ids and paths stay in the app");
        // Stop: the transcript's model (and size) replace it
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("s.jsonl");
        std::fs::write(&t, assistant(5, 0, 0) + "\n").unwrap();
        f.hook("Stop", json!({"last_assistant_message": "끝", "transcript_path": t.to_string_lossy()}));
        let e = f.entry();
        assert_eq!((e.context_tokens, e.model.as_deref()), (Some(5), Some("m")));
        assert_eq!(e.session_id.as_deref(), Some("9887ea82-aaaa"), "kept through the turn");
    }
}
