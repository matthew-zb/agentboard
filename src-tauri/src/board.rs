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
use crate::workspace::PaneId;
use crate::AppState;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
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
    /// Context size from the transcript's last usage (Task BC2).
    pub context_tokens: Option<u64>,
    /// Epoch milliseconds of the last hook.
    pub updated_at: u64,
    /// PTY generation the entry belongs to.
    #[serde(skip)]
    pub gen: u64,
    /// The session's transcript (`transcript_path` of its hooks).
    #[serde(skip)]
    pub transcript: Option<PathBuf>,
}

impl BoardEntry {
    fn fresh(gen: u64) -> Self {
        BoardEntry { idle: false, last_prompt: None, last_reply: None, context_tokens: None, updated_at: 0, gen, transcript: None }
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
        match kind {
            HookKind::SessionStart => e.idle = false,
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
    struct NullUi;
    impl Ui for NullUi {
        fn workspace_changed(&self, _: &Snapshot) {}
        fn drop_channel(&self, _: &str) {}
    }

    /// Manager tab "billing"; `pane` = its role pane.
    struct Fx {
        state: AppState,
        pane: String,
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
        let state =
            AppState::new(ws, loaded, pty, Arc::new(NullUi), tx, crate::persist::Saver::disabled(), Vec::new(), appdata.path().join("run"));
        Fx { state, pane, _rx: rx, _keep: (root, appdata) }
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
        assert_eq!(keys, ["contextTokens", "idle", "lastPrompt", "lastReply", "updatedAt"]);
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
}
