//! Attention (spec §5-1, Task 9): making "this agent needs the user" visible.
//!
//! * **Notification hook**: only `notification_type == "permission_prompt"` flags the pane
//!   (reason [`PERMISSION_REASON`]); `idle_prompt` and everything else is ignored (R7: a
//!   standing agent sends idle prompts all the time). A notification of the *focused* pane
//!   (focused group → its active tab → that tab's last focused pane, whether or not the window
//!   has OS focus — the backend cannot know) is ignored: the user is looking at it.
//! * **`need_user(reason)`** (MCP, every role, see `mcp_tools`): flags the calling pane with
//!   the cleaned reason (`workspace::clean_reason`, ≤ 200 chars, no control characters).
//! * **Folder-trust warning** (ledger R5): every spawn of a role pane arms a timer for its
//!   generation ([`arm_trust`]); SessionStart of that generation disarms it. When
//!   [`TRUST_TIMEOUT`] passes without one, the pane gets attention with [`TRUST_REASON`]
//!   ([`poll_trust`], ticked by the launcher's prompt timer). A respawn re-arms for the new
//!   generation; a SessionStart of an older generation never satisfies a newer one; a pane that
//!   was closed, restarted or whose process exited meanwhile is skipped. A SessionStart that
//!   arrives after the warning clears it (the user answered the trust question).
//!
//! The app never edits Claude's folder-trust state. Every model change goes through
//! `launcher::execute` (`SetAttention`). Locks: `AppState::trust` is a leaf, never held while
//! taking `ws` or calling `execute`.

use crate::control::{Caller, HookRouter};
use crate::launcher;
use crate::workspace::{clean_reason, PaneId, Workspace, WsCommand};
use crate::AppState;
use serde_json::Value;
use std::time::{Duration, Instant};

/// A role pane must report SessionStart within this time after its spawn.
pub const TRUST_TIMEOUT: Duration = Duration::from_secs(30);
/// Reason shown when SessionStart did not come (most likely Claude's folder-trust question).
pub const TRUST_REASON: &str = "폴더 신뢰 확인 필요";
/// Reason shown for a permission prompt.
pub const PERMISSION_REASON: &str = "권한 승인 대기";

/// Per-pane trust timer state (`AppState::trust`), always for one spawn generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Spawned; SessionStart expected before `deadline`.
    Armed { gen: u64, deadline: Instant },
    /// SessionStart seen for this generation.
    Started { gen: u64 },
    /// The timer ran out and the warning was raised for this generation (Task 12's
    /// `folder-trust:<tab>` item reads it). A later SessionStart / respawn replaces it.
    TimedOut { gen: u64 },
}

/// SessionStart (wraps the bridge's handler) and Notification. Call after
/// `bridge::register_hooks`.
pub fn register_hooks(hooks: &HookRouter) {
    hooks.register("SessionStart", on_session_start);
    hooks.register("Notification", on_notification);
}

/// The pane at the end of the focus chain.
pub fn focused_pane(ws: &Workspace) -> Option<&PaneId> {
    let g = ws.groups.get(&ws.focus.group_id)?;
    ws.tabs.get(&g.active_tab_id).map(|t| &t.last_focused_pane_id)
}

/// SessionStart: the bridge registration, then this generation counts as started.
pub fn on_session_start(state: &AppState, caller: &Caller, body: &Value) {
    crate::bridge::on_session_start(state, caller, body);
    // a valid token belongs to the latest spawn: the hook is from the current generation
    let Some(gen) = state.pty.current_gen(&caller.pane) else { return };
    state.trust.lock().unwrap().insert(caller.pane.clone(), Trust::Started { gen });
    // a SessionStart after the warning: the trust question was answered
    clear_trust_warning(state, &caller.pane);
}

/// Turn off the pane's ⚠ if (and only if) it is still the trust warning.
fn clear_trust_warning(state: &AppState, pane: &str) {
    let warned = {
        let ws = state.ws.lock().unwrap();
        ws.pane_by_id(pane).is_some_and(|p| p.attention && p.attention_reason.as_deref() == Some(TRUST_REASON))
    };
    if warned {
        let _ = launcher::execute(state, WsCommand::SetAttention { pane: pane.to_string(), on: false, reason: None });
    }
}

/// Notification: `permission_prompt` → attention (unless the pane is focused).
pub fn on_notification(state: &AppState, caller: &Caller, body: &Value) {
    let kind = body.get("stdin").and_then(|s| s.get("notification_type")).and_then(Value::as_str);
    if kind != Some("permission_prompt") {
        return;
    }
    if focused_pane(&state.ws.lock().unwrap()) == Some(&caller.pane) {
        return; // the user is looking at it
    }
    let cmd = WsCommand::SetAttention { pane: caller.pane.clone(), on: true, reason: Some(PERMISSION_REASON.into()) };
    let _ = launcher::execute(state, cmd);
}

/// MCP `need_user(reason)`: attention + reason on the caller's pane.
pub fn need_user(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let raw = match args.get("reason") {
        Some(Value::String(s)) => s,
        None | Some(Value::Null) => return Err("reason이 필요합니다".into()),
        Some(_) => return Err("reason은(는) 문자열이어야 합니다".into()),
    };
    let reason = clean_reason(raw).ok_or("reason이 비어 있습니다 — 한 줄 사유를 쓰세요")?;
    launcher::execute(state, WsCommand::SetAttention { pane: caller.pane.clone(), on: true, reason: Some(reason) })?;
    Ok(Value::String("사용자에게 알렸습니다(탭에 ⚠와 사유 표시). 질문을 화면에 써 두고 답을 기다리세요.".into()))
}

/// A role pane was spawned as generation `gen` at `now`.
pub fn arm_trust(state: &AppState, pane: &str, gen: u64, now: Instant) {
    let mut t = state.trust.lock().unwrap();
    if t.get(pane) == Some(&Trust::Started { gen }) {
        return; // SessionStart already came (raced the spawn bookkeeping)
    }
    t.insert(pane.to_string(), Trust::Armed { gen, deadline: now + TRUST_TIMEOUT });
}

/// Whether `pane`'s current process missed its SessionStart deadline (warning raised, no
/// SessionStart since). Takes `trust`, released, then the PtyManager `panes` map.
pub fn trust_timed_out(state: &AppState, pane: &str) -> bool {
    let timed_out = match state.trust.lock().unwrap().get(pane) {
        Some(Trust::TimedOut { gen }) => *gen,
        _ => return false,
    };
    state.pty.current_gen(pane) == Some(timed_out)
}

/// The pane left the model: drop its timer.
pub fn forget(state: &AppState, pane: &str) {
    state.trust.lock().unwrap().remove(pane);
}

/// Flag every role pane whose timer ran out at `now`. Returns how many were flagged.
pub fn poll_trust(state: &AppState, now: Instant) -> usize {
    let due: Vec<(PaneId, u64)> = {
        let mut t = state.trust.lock().unwrap();
        let due: Vec<(PaneId, u64)> = t
            .iter()
            .filter_map(|(p, s)| match *s {
                Trust::Armed { gen, deadline } if deadline <= now => Some((p.clone(), gen)),
                _ => None,
            })
            .collect();
        for (p, _) in &due {
            t.remove(p); // each timer fires at most once
        }
        due
    };
    let mut flagged = 0;
    for (pane, gen) in due {
        // only the generation the timer was armed for, and only while its process lives
        if state.pty.current_gen(&pane) != Some(gen) || !state.pty.is_running(&pane) {
            continue;
        }
        let is_role = {
            let ws = state.ws.lock().unwrap();
            ws.pane_by_id(&pane).is_some_and(|p| p.role.is_some() && !p.kind.is_applet())
        };
        if !is_role {
            continue;
        }
        if flag_trust(state, &pane, gen) {
            flagged += 1;
            // remembered for the environment check, unless SessionStart came meanwhile
            let mut t = state.trust.lock().unwrap();
            if t.get(&pane).is_none() {
                t.insert(pane.clone(), Trust::TimedOut { gen });
            }
        }
    }
    flagged
}

/// Raise the trust warning for `gen`, then re-check: a SessionStart of that generation that
/// slipped in between the due check and the flag (it found nothing to clear) has recorded
/// `Started{gen}` before looking, so seeing it here means the warning is wrong — take it back.
/// Each side checks after its own write, so one of them always sees the other. Returns
/// whether the warning stands. `trust` is taken alone (leaf), after `execute` returned.
fn flag_trust(state: &AppState, pane: &str, gen: u64) -> bool {
    let cmd = WsCommand::SetAttention { pane: pane.to_string(), on: true, reason: Some(TRUST_REASON.into()) };
    if launcher::execute(state, cmd).is_err() {
        return false;
    }
    let started = state.trust.lock().unwrap().get(pane) == Some(&Trust::Started { gen });
    if started {
        clear_trust_warning(state, pane);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};
    use crate::pty::{PtyManager, PtySink, SpawnSpec};
    use crate::roles::Role;
    use crate::workspace::{Pane, PaneKind};
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

    struct Fx {
        state: AppState,
        /// Manager pane of tab "billing".
        pane: String,
        _rx: mpsc::Receiver<crate::workspace::Effects>,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            let ids: Vec<String> = self.state.ws.lock().unwrap().panes.keys().cloned().collect();
            for id in ids {
                self.state.pty.remove(&id);
            }
        }
    }

    /// Dispatcher tab (focused) + manager tab "billing" + a plain terminal pane "plain1".
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
        let mgr = ws.panes.values().find(|p| p.role == Some(Role::Manager)).expect("manager pane").clone();
        ws.panes.insert("plain1".into(), Pane { id: "plain1".into(), role: None, session_name: None, kind: PaneKind::Terminal, ..mgr.clone() });
        let (tx, rx) = mpsc::channel();
        let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
        let state =
            AppState::new(ws, loaded, pty, Arc::new(NullUi), tx, crate::persist::Saver::disabled(), Vec::new(), appdata.path().join("run"));
        Fx { state, pane: mgr.id, _rx: rx, _keep: (root, appdata) }
    }

    impl Fx {
        fn caller_of(&self, pane: &str) -> Caller {
            let ws = self.state.ws.lock().unwrap();
            let p = ws.pane_by_id(pane).unwrap();
            let tab_name = ws.tabs[&p.tab].name.clone();
            Caller { pane: p.id.clone(), role: p.role.unwrap(), tab: p.tab.clone(), tab_name, team: None, session_name: p.session_name.clone().unwrap() }
        }
        fn caller(&self) -> Caller {
            self.caller_of(&self.pane)
        }
        /// (Re)start a pane's process (an idle `cmd.exe`): a fresh generation.
        fn spawn(&self, pane: &str) -> u64 {
            self.state.pty.remove(pane);
            let spec = SpawnSpec {
                cmd: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into(), "prompt $G".into()],
                env: vec![],
                env_remove: vec![],
                cwd: std::env::temp_dir(),
                cols: 80,
                rows: 24,
            };
            self.state.pty.spawn(pane, spec).unwrap()
        }
        fn att(&self, pane: &str) -> (bool, Option<String>) {
            let ws = self.state.ws.lock().unwrap();
            let p = &ws.panes[pane];
            (p.attention, p.attention_reason.clone())
        }
        fn notify(&self, kind: &str) {
            let body = json!({"event":"Notification","stdin":{"hook_event_name":"Notification","message":"m","notification_type":kind},
                              "socket":null,"msgToken":null,"cwd":"."});
            on_notification(&self.state, &self.caller(), &body);
        }
        fn session_start(&self) {
            let body = json!({"event":"SessionStart","stdin":{"source":"startup"},"socket":null,"msgToken":null,"cwd":"."});
            on_session_start(&self.state, &self.caller(), &body);
        }
        fn focus(&self, pane: &str) {
            let (g, t) = {
                let ws = self.state.ws.lock().unwrap();
                let t = ws.panes[pane].tab.clone();
                (ws.group_of_tab(&t).unwrap().clone(), t)
            };
            launcher::execute(&self.state, WsCommand::SetFocus { group: g, tab: Some(t), pane: Some(pane.into()) }).unwrap();
        }
        fn focused(&self) -> Option<PaneId> {
            focused_pane(&self.state.ws.lock().unwrap()).cloned()
        }
    }

    #[test]
    fn permission_prompt_flags_and_idle_prompt_is_ignored() {
        let f = fixture();
        assert_ne!(f.focused().as_deref(), Some(f.pane.as_str()), "billing must not start focused");
        f.notify("idle_prompt");
        assert_eq!(f.att(&f.pane), (false, None));
        f.notify("auth_success");
        assert_eq!(f.att(&f.pane), (false, None));
        // a body without a notification type (raw stdin text) is ignored
        on_notification(&f.state, &f.caller(), &json!({"event":"Notification","stdin":"not json"}));
        assert_eq!(f.att(&f.pane), (false, None));
        f.notify("permission_prompt");
        assert_eq!(f.att(&f.pane), (true, Some(PERMISSION_REASON.into())));
    }

    #[test]
    fn notification_on_focused_pane_is_ignored() {
        let f = fixture();
        f.focus(&f.pane);
        assert_eq!(f.focused().as_deref(), Some(f.pane.as_str()));
        f.notify("permission_prompt");
        assert_eq!(f.att(&f.pane), (false, None));
    }

    #[test]
    fn need_user_sets_reason_and_focus_clears_it() {
        let f = fixture();
        let call = |args: Value| crate::mcp_tools::call(&f.state, &f.caller(), "need_user", args);
        assert!(call(json!({})).is_err(), "reason is required");
        assert!(call(json!({"reason": 3})).is_err());
        assert!(call(json!({"reason": " \u{7}\n "})).is_err(), "an empty reason is refused");
        assert_eq!(f.att(&f.pane), (false, None));
        let r = call(json!({"reason": "배포 승인\u{1b}[2J 필요\n어느 쪽?"})).unwrap();
        assert!(r.as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(f.att(&f.pane), (true, Some("배포 승인[2J 필요 어느 쪽?".into())));
        // long reasons are cut
        call(json!({"reason": "가".repeat(500)})).unwrap();
        assert_eq!(f.att(&f.pane).1.unwrap().chars().count(), crate::workspace::ATTENTION_REASON_MAX);
        // every role may call it
        for role in [Role::Dispatcher, Role::Handover, Role::Manager, Role::Lead, Role::Member] {
            assert!(crate::mcp_tools::list(role).iter().any(|t| t.name == "need_user"), "{role:?}");
        }
        // focusing the pane clears attention and reason
        f.focus(&f.pane);
        assert_eq!(f.att(&f.pane), (false, None));
    }

    #[test]
    fn missing_session_start_flags_trust() {
        let f = fixture();
        let t0 = Instant::now();
        let gen1 = f.spawn(&f.pane);
        arm_trust(&f.state, &f.pane, gen1, t0);
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT - Duration::from_secs(1)), 0);
        assert_eq!(f.att(&f.pane), (false, None));
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 1);
        assert_eq!(f.att(&f.pane), (true, Some(TRUST_REASON.into())));
        assert!(trust_timed_out(&f.state, &f.pane), "the env check sees the timeout");
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT * 3), 0, "flagged once");
        // focusing the pane clears the ⚠ but not the env item (trust still unanswered)
        f.focus(&f.pane);
        assert!(trust_timed_out(&f.state, &f.pane));
        // a respawn re-arms: the old timeout no longer counts
        let gen2 = f.spawn(&f.pane);
        assert!(!trust_timed_out(&f.state, &f.pane));
        arm_trust(&f.state, &f.pane, gen2, t0);
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 1);
        assert!(trust_timed_out(&f.state, &f.pane));
        // a late SessionStart answers the warning
        f.session_start();
        assert_eq!(f.att(&f.pane), (false, None));
        assert!(!trust_timed_out(&f.state, &f.pane));
    }

    #[test]
    fn session_start_in_time_disarms_only_its_generation() {
        let f = fixture();
        let t0 = Instant::now();
        let gen1 = f.spawn(&f.pane);
        arm_trust(&f.state, &f.pane, gen1, t0);
        f.session_start();
        // the wrapper still does the bridge registration for this generation
        assert_eq!(f.state.bridge.reg(&f.pane).map(|r| r.gen), Some(gen1));
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT * 2), 0);
        assert_eq!(f.att(&f.pane), (false, None));
        // respawn: gen1's SessionStart does not count for gen2
        let gen2 = f.spawn(&f.pane);
        assert_ne!(gen1, gen2);
        arm_trust(&f.state, &f.pane, gen2, t0);
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 1);
        assert_eq!(f.att(&f.pane), (true, Some(TRUST_REASON.into())));
    }

    #[test]
    fn session_start_racing_the_flag_leaves_no_warning() {
        let f = fixture();
        let gen = f.spawn(&f.pane);
        // poll_trust already took the due timer; SessionStart lands before the flag is raised
        // (it finds nothing to clear), then the flag goes up: it must take itself back
        f.session_start();
        assert!(!flag_trust(&f.state, &f.pane, gen));
        assert_eq!(f.att(&f.pane), (false, None));
        // without a SessionStart the warning stands
        let gen2 = f.spawn(&f.pane);
        assert!(flag_trust(&f.state, &f.pane, gen2));
        assert_eq!(f.att(&f.pane), (true, Some(TRUST_REASON.into())));
        // an older generation's Started does not take a newer warning back
        f.state.trust.lock().unwrap().insert(f.pane.clone(), Trust::Started { gen });
        assert!(flag_trust(&f.state, &f.pane, gen2));
    }

    #[test]
    fn stale_closed_or_exited_panes_are_never_flagged() {
        let f = fixture();
        let t0 = Instant::now();
        // armed for gen1, then restarted (gen2, timer not yet re-armed): gen1's timer is stale
        let gen1 = f.spawn(&f.pane);
        arm_trust(&f.state, &f.pane, gen1, t0);
        let _gen2 = f.spawn(&f.pane);
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 0);
        assert_eq!(f.att(&f.pane), (false, None));
        // process gone (exited / removed) before the deadline
        let gen3 = f.spawn(&f.pane);
        arm_trust(&f.state, &f.pane, gen3, t0);
        f.state.pty.remove(&f.pane);
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 0);
        // pane closed before the deadline: no-op, timer dropped
        let gen4 = f.spawn("plain1");
        arm_trust(&f.state, "plain1", gen4, t0); // not a role pane: never flagged
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 0);
        assert_eq!(f.att("plain1"), (false, None));
        let gen5 = f.spawn(&f.pane);
        arm_trust(&f.state, &f.pane, gen5, t0);
        f.state.pty.remove(&f.pane);
        f.state.ws.lock().unwrap().panes.remove(&f.pane);
        assert_eq!(poll_trust(&f.state, t0 + TRUST_TIMEOUT), 0);
        assert!(f.state.trust.lock().unwrap().is_empty(), "due timers are dropped either way");
        arm_trust(&f.state, &f.pane, gen5, t0);
        forget(&f.state, &f.pane);
        assert!(f.state.trust.lock().unwrap().is_empty());
    }
}
