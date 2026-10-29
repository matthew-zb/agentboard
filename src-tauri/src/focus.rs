//! 집중근무 (focus-mode spec §3·§4, decisions F1–F5 a). Three modes — normal, 전달 멈춤 (hold),
//! 집중근무 (focus) — set from the palette (Tauri `focus_set`) or by the dispatcher on the user's
//! word (MCP `focus_set`, dispatcher only); one state in the app.
//!
//! What the app enforces: the dispatcher is told (`[AgentBoard] 집중근무 시작|전달 멈춤|해제`)
//! and told again after a new dispatcher session's SessionStart (F5); an agent's `need_user`
//! ⚠ waits while a mode is on and comes back on release (F4; permission and folder-trust ⚠ are
//! not held); the frontend gets the mode and its start time (`focus-changed`) to hide or count
//! inbox items that arrived during it (F3). What only the role prompt can do: the dispatcher not
//! relaying other sessions' reports (Claude Code messaging, outside the app).
//!
//! Saved in `%APPDATA%/agentboard/focus.json` (`{"mode","since"}`); a missing or unreadable file
//! is the normal mode. Deferred `need_user` requests are in memory only — after a restart the
//! panes are new sessions anyway. The state lock is a leaf.

use crate::control::Caller;
use crate::roles::Role;
use crate::workspace::{PaneId, WsCommand};
use crate::AppState;
use chrono::{Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The saved mode, in the app's data folder.
pub const FILE_NAME: &str = "focus.json";

/// Wire / file form: `"normal"` | `"hold"` | `"focus"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FocusMode {
    Normal,
    /// 전달 멈춤: answers to the user's own requests only, the rest as one "(보류 — N건)" line.
    Hold,
    /// 집중근무: nothing from other sessions reaches the user.
    Focus,
}

impl FocusMode {
    pub fn parse(s: &str) -> Option<FocusMode> {
        match s {
            "normal" => Some(FocusMode::Normal),
            "hold" => Some(FocusMode::Hold),
            "focus" => Some(FocusMode::Focus),
            _ => None,
        }
    }
}

/// `focus_status()` / `focus-changed`: `{mode, since}`. `since` is when the current mode period
/// began, RFC 3339 like inbox items' `created` (`null` in the normal mode); switching between
/// hold and focus keeps it — the period has not ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusStatus {
    pub mode: FocusMode,
    pub since: Option<String>,
}

impl Default for FocusStatus {
    fn default() -> Self {
        FocusStatus { mode: FocusMode::Normal, since: None }
    }
}

#[derive(Default)]
struct Inner {
    file: Option<PathBuf>,
    status: FocusStatus,
    /// `need_user` requests held during a mode: (pane, reason), one per pane (the latest).
    deferred: Vec<(PaneId, String)>,
}

/// `AppState::focus`.
#[derive(Default)]
pub struct FocusState {
    inner: Mutex<Inner>,
}

impl FocusState {
    /// Read the saved mode from `file` (missing / unreadable → normal) and save there from now on.
    pub fn load(&self, file: PathBuf) {
        let saved = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<FocusStatus>(&t).ok());
        let status = saved.filter(|s| s.mode != FocusMode::Normal).unwrap_or_default();
        let mut g = self.inner.lock().unwrap();
        g.file = Some(file);
        g.status = status;
    }

    pub fn status(&self) -> FocusStatus {
        self.inner.lock().unwrap().status.clone()
    }

    /// Switch to `mode` (`now`: the start time if a period begins). Returns the new status,
    /// whether it changed, and — when a mode ends — the held `need_user` requests. A save
    /// failure changes nothing.
    fn switch(&self, mode: FocusMode, now: String) -> Result<(FocusStatus, bool, Vec<(PaneId, String)>), String> {
        let mut g = self.inner.lock().unwrap();
        if g.status.mode == mode {
            return Ok((g.status.clone(), false, Vec::new()));
        }
        let since = match mode {
            FocusMode::Normal => None,
            _ => Some(g.status.since.clone().unwrap_or(now)),
        };
        let next = FocusStatus { mode, since };
        if let Some(file) = &g.file {
            let text = serde_json::to_string(&next).map_err(|e| e.to_string())?;
            write_atomic(file, &text).map_err(|e| format!("집중근무 상태를 저장할 수 없습니다 — {e}"))?;
        }
        g.status = next.clone();
        let released = if mode == FocusMode::Normal { std::mem::take(&mut g.deferred) } else { Vec::new() };
        Ok((next, true, released))
    }

    /// Hold `pane`'s `need_user` while a mode is on (its latest reason). `false` in the normal
    /// mode: show it now.
    pub fn defer(&self, pane: &str, reason: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.status.mode == FocusMode::Normal {
            return false;
        }
        g.deferred.retain(|(p, _)| p != pane);
        g.deferred.push((pane.to_string(), reason.to_string()));
        true
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        crate::inbox::rename_with_retry(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// The app message for the dispatcher when the mode becomes `mode` (spec §1 rules, §4 release).
fn message(mode: FocusMode) -> &'static str {
    match mode {
        FocusMode::Focus => {
            "[AgentBoard] 집중근무 시작 — 사용자가 직접 물은 것과 사용자가 시킨 일의 결과만 답한다. 다른 세션의 보고·알림은 사용자에게 전하지 않고(보류 건수 한 줄도 없이), 받는 즉시 DISPATCHER.md 「## 보류(집중근무)」에 한 줄씩 적어 둔다. 예외 기준을 스스로 만들지 않는다. 해제는 사용자가 말할 때까지다."
        }
        FocusMode::Hold => {
            "[AgentBoard] 전달 멈춤 — 사용자 요청의 답만 전하고, 나머지 보고는 「(보류 — N건)」 한 줄로만 알린다. 받은 보고는 DISPATCHER.md 「## 보류(집중근무)」에 한 줄씩 적어 둔다. 예외 기준을 스스로 만들지 않는다."
        }
        FocusMode::Normal => {
            "[AgentBoard] 집중근무 해제 — 보류 정리: DISPATCHER.md 「## 보류(집중근무)」의 보류 건을 결정 요청 → 알림 순으로 표로 요약해 사용자에게 보고하라."
        }
    }
}

fn now_stamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// Switch the mode (palette or the dispatcher's `focus_set`): save, tell the dispatcher if it
/// runs (a failed send does not undo the switch), show held `need_user` ⚠ again when a mode
/// ends, emit `focus-changed`. The same mode again changes nothing.
pub fn set(state: &AppState, mode: FocusMode) -> Result<FocusStatus, String> {
    let (st, changed, released) = state.focus.switch(mode, now_stamp())?;
    if !changed {
        return Ok(st);
    }
    if let Ok(d) = crate::bridge::dispatcher_pane(state) {
        if let Err(e) = crate::bridge::send(state, &d, message(mode)) {
            eprintln!("[agentboard] 집중근무 알림 실패: {e}");
        }
    }
    for (pane, reason) in released {
        // a pane closed meanwhile is simply gone
        let _ = crate::launcher::execute(state, WsCommand::SetAttention { pane, on: true, reason: Some(reason) });
    }
    state.ui.focus_changed(&st);
    Ok(st)
}

/// SessionStart of a role session (`attention::on_session_start`): a new dispatcher session
/// hears the current mode again (F5 — a restart or nightly refresh replaced the one that knew).
pub fn on_session_start(state: &AppState, caller: &Caller) {
    if caller.role != Role::Dispatcher {
        return;
    }
    let mode = state.focus.status().mode;
    if mode == FocusMode::Normal {
        return;
    }
    let text = format!("{} (새 세션에 다시 알림: 지금 이 모드다. DISPATCHER.md의 보류 목록을 이어받는다.)", message(mode));
    if let Err(e) = crate::bridge::send(state, &caller.pane, &text) {
        eprintln!("[agentboard] 집중근무 재알림 실패: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};
    use crate::pty::{PtyManager, PtySink};
    use crate::roles::Role;
    use crate::workspace::{Effects, PaneStatus, Workspace};
    use crate::{Snapshot, Ui};
    use serde_json::{json, Value};
    use std::sync::{mpsc, Arc};

    struct NullSink;
    impl PtySink for NullSink {
        fn output(&self, _: &str, _: u64, _: &[u8]) {}
        fn exited(&self, _: &str, _: u64, _: i32) {}
    }

    /// Records `focus-changed` payloads.
    #[derive(Default)]
    struct RecUi {
        focus: Mutex<Vec<FocusStatus>>,
    }
    impl Ui for RecUi {
        fn workspace_changed(&self, _: &Snapshot) {}
        fn drop_channel(&self, _: &str) {}
        fn focus_changed(&self, st: &FocusStatus) {
            self.focus.lock().unwrap().push(st.clone());
        }
    }

    /// Dispatcher (running) + Manager tab "billing" (running); `appdata/focus.json` loaded.
    struct Fx {
        state: AppState,
        ui: Arc<RecUi>,
        file: PathBuf,
        _rx: mpsc::Receiver<Effects>,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Fx {
        fn pane_of(&self, role: Role) -> String {
            let ws = self.state.ws.lock().unwrap();
            ws.panes.values().find(|p| p.role == Some(role) && !p.kind.is_applet()).unwrap().id.clone()
        }
        fn caller(&self, role: Role) -> Caller {
            let ws = self.state.ws.lock().unwrap();
            let p = ws.panes.values().find(|p| p.role == Some(role) && !p.kind.is_applet()).unwrap();
            Caller {
                pane: p.id.clone(),
                role,
                tab: p.tab.clone(),
                tab_name: ws.tabs[&p.tab].name.clone(),
                team: None,
                session_name: p.session_name.clone().unwrap(),
            }
        }
        fn texts(&self) -> Vec<String> {
            self.state.bridge.queued_texts(&self.pane_of(Role::Dispatcher))
        }
        fn attention(&self, role: Role) -> (bool, Option<String>) {
            let ws = self.state.ws.lock().unwrap();
            let p = &ws.panes[&self.pane_of_locked(&ws, role)];
            (p.attention, p.attention_reason.clone())
        }
        fn pane_of_locked(&self, ws: &Workspace, role: Role) -> String {
            ws.panes.values().find(|p| p.role == Some(role) && !p.kind.is_applet()).unwrap().id.clone()
        }
        fn call(&self, role: Role, tool: &str, args: Value) -> Result<Value, String> {
            crate::mcp_tools::call(&self.state, &self.caller(role), tool, args)
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
        let mut ws = Workspace::from_config(&loaded);
        let ids: Vec<String> = ws.panes.keys().cloned().collect();
        for id in ids {
            ws.set_status(&id, PaneStatus::Running);
        }
        let (tx, rx) = mpsc::channel();
        let ui = Arc::new(RecUi::default());
        let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
        let state = AppState::new(ws, loaded, pty, ui.clone(), tx, crate::persist::Saver::disabled(), Vec::new(), appdata.path().join("run"));
        let file = appdata.path().join(FILE_NAME);
        state.focus.load(file.clone());
        Fx { state, ui, file, _rx: rx, _keep: (root, appdata) }
    }

    #[test]
    fn focus_mode_persists_and_reloads() {
        let f = fixture();
        assert_eq!(f.state.focus.status(), FocusStatus { mode: FocusMode::Normal, since: None });
        let st = set(&f.state, FocusMode::Focus).unwrap();
        assert_eq!(st.mode, FocusMode::Focus);
        let since = st.since.clone().expect("the start time");
        assert!(chrono::DateTime::parse_from_rfc3339(&since).is_ok(), "RFC 3339 like inbox `created`: {since}");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&f.file).unwrap()).unwrap();
        assert_eq!(saved, json!({"mode": "focus", "since": since}));
        // a restart reads it back
        let again = FocusState::default();
        again.load(f.file.clone());
        assert_eq!(again.status(), st);
        // a broken or unknown file is the normal mode
        std::fs::write(&f.file, "{ nope").unwrap();
        let broken = FocusState::default();
        broken.load(f.file.clone());
        assert_eq!(broken.status().mode, FocusMode::Normal);
        // hold ↔ focus is the same period: the start time stays
        assert_eq!(set(&f.state, FocusMode::Hold).unwrap().since.as_deref(), Some(since.as_str()));
        // released: no start time
        let st = set(&f.state, FocusMode::Normal).unwrap();
        assert_eq!(st, FocusStatus { mode: FocusMode::Normal, since: None });
        assert_eq!(FocusMode::parse("hold"), Some(FocusMode::Hold));
        assert_eq!(FocusMode::parse("집중"), None);
    }

    #[test]
    fn focus_set_tells_the_dispatcher_and_the_frontend() {
        let f = fixture();
        set(&f.state, FocusMode::Focus).unwrap();
        set(&f.state, FocusMode::Focus).unwrap(); // the same mode: nothing new
        set(&f.state, FocusMode::Hold).unwrap();
        set(&f.state, FocusMode::Normal).unwrap();
        let texts = f.texts();
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert!(texts[0].starts_with("[AgentBoard] 집중근무 시작"), "{}", texts[0]);
        assert!(texts[1].starts_with("[AgentBoard] 전달 멈춤"), "{}", texts[1]);
        assert!(texts[2].starts_with("[AgentBoard] 집중근무 해제 — 보류 정리"), "{}", texts[2]);
        assert!(texts[2].contains("DISPATCHER.md") && texts[2].contains("결정 요청"), "{}", texts[2]);
        let modes: Vec<FocusMode> = f.ui.focus.lock().unwrap().iter().map(|s| s.mode).collect();
        assert_eq!(modes, [FocusMode::Focus, FocusMode::Hold, FocusMode::Normal], "focus-changed per change");
        // the dispatcher is not running: the mode still changes, no message is queued
        let d = f.pane_of(Role::Dispatcher);
        f.state.ws.lock().unwrap().set_status(&d, PaneStatus::Exited { code: 0 });
        set(&f.state, FocusMode::Focus).unwrap();
        assert_eq!(f.state.focus.status().mode, FocusMode::Focus);
        assert_eq!(f.texts().len(), 3);
    }

    /// F4 a: an agent's decision request (need_user) waits while a mode is on and comes back
    /// on release; a permission prompt (work that is stuck) still shows at once.
    #[test]
    fn need_user_waits_while_focused_and_returns_on_release() {
        let f = fixture();
        set(&f.state, FocusMode::Focus).unwrap();
        let r = f.call(Role::Manager, "need_user", json!({"reason": "배포 승인 필요"})).unwrap();
        assert!(r.as_str().unwrap().contains("집중근무"), "{r}");
        assert_eq!(f.attention(Role::Manager), (false, None), "no ⚠ while focused");
        // the same pane asks again: the latest reason is kept
        f.call(Role::Manager, "need_user", json!({"reason": "배포 승인 필요(재요청)"})).unwrap();
        // a permission prompt is not held back
        let body = json!({"event": "Notification", "stdin": {"notification_type": "permission_prompt"}});
        crate::attention::on_notification(&f.state, &f.caller(Role::Manager), &body);
        assert!(f.attention(Role::Manager).0, "permission prompt ⚠ shows");
        crate::launcher::execute(&f.state, crate::workspace::WsCommand::SetAttention { pane: f.pane_of(Role::Manager), on: false, reason: None }).unwrap();
        set(&f.state, FocusMode::Normal).unwrap();
        assert_eq!(f.attention(Role::Manager), (true, Some("배포 승인 필요(재요청)".into())), "back on release");
        // the hold mode defers too
        set(&f.state, FocusMode::Hold).unwrap();
        crate::launcher::execute(&f.state, crate::workspace::WsCommand::SetAttention { pane: f.pane_of(Role::Manager), on: false, reason: None }).unwrap();
        f.call(Role::Manager, "need_user", json!({"reason": "확인"})).unwrap();
        assert!(!f.attention(Role::Manager).0);
    }

    #[test]
    fn focus_tools_are_dispatcher_only_and_report_status() {
        let f = fixture();
        assert_eq!(f.call(Role::Dispatcher, "focus_status", json!({})).unwrap(), json!({"mode": "normal", "since": null}));
        let r = f.call(Role::Dispatcher, "focus_set", json!({"mode": "focus"})).unwrap();
        assert_eq!(r["mode"], "focus");
        assert!(r["since"].is_string());
        assert_eq!(f.state.focus.status().mode, FocusMode::Focus);
        let e = f.call(Role::Dispatcher, "focus_set", json!({"mode": "집중"})).unwrap_err();
        assert!(e.contains("normal") && e.contains("focus"), "{e}");
        for tool in ["focus_set", "focus_status"] {
            assert!(f.call(Role::Manager, tool, json!({"mode": "normal"})).is_err(), "{tool}");
            for r in [Role::Handover, Role::Manager, Role::Lead, Role::Member] {
                assert!(!crate::mcp_tools::allowed(tool, r), "{tool} {}", r.as_str());
            }
        }
    }

    /// F5 a: a new dispatcher session (restart, nightly refresh) hears the mode again after its
    /// SessionStart; other roles and the normal mode get nothing.
    #[test]
    fn dispatcher_session_start_reannounces_the_mode() {
        let f = fixture();
        on_session_start(&f.state, &f.caller(Role::Dispatcher));
        assert!(f.texts().is_empty(), "normal: nothing to say");
        set(&f.state, FocusMode::Hold).unwrap();
        let n = f.texts().len();
        on_session_start(&f.state, &f.caller(Role::Manager));
        assert_eq!(f.texts().len(), n);
        on_session_start(&f.state, &f.caller(Role::Dispatcher));
        let texts = f.texts();
        assert_eq!(texts.len(), n + 1);
        assert!(texts[n].starts_with("[AgentBoard] 전달 멈춤"), "{}", texts[n]);
        assert!(texts[n].contains("DISPATCHER.md"), "{}", texts[n]);
    }
}
