//! workspace.json save/restore (spec §4.2): atomic save, corrupt/inconsistent files moved to
//! `.bak`, debounced background `Saver`.
//!
//! Lock rules (see `launcher` docs): the `Saver` thread never holds an app lock while writing;
//! its snapshot closure takes `ws` only to clone. Callers notify the saver without holding
//! any lock that the snapshot closure needs in a conflicting order (notify is a channel send).

use crate::config::Loaded;
use crate::workspace::{Effects, Workspace};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_millis(500);
/// A steady stream of notifications cannot postpone a save longer than this.
const MAX_DELAY: Duration = Duration::from_secs(5);
const FLUSH_WAIT: Duration = Duration::from_secs(5);
/// Shown in the error list when workspace.json had to be discarded.
pub const CORRUPT_MESSAGE: &str = "작업 상태 파일이 손상되어 초기 배치로 시작했습니다";
/// Shown when workspace.json exists but could not be read or backed up; saving is disabled.
pub const UNREADABLE_MESSAGE: &str =
    "저장된 작업 상태 파일을 읽을 수 없어 초기 배치로 시작했습니다. 이번 실행의 변경 사항은 저장되지 않습니다";

fn sibling(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

/// Write `<path>.tmp` (fsynced), then rename over `path`.
pub fn save_atomic(path: &Path, ws: &Workspace) -> Result<(), String> {
    use std::io::Write;
    let json = serde_json::to_vec_pretty(ws).map_err(|e| format!("작업 상태를 직렬화할 수 없습니다: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("폴더를 만들 수 없습니다: {} — {e}", dir.display()))?;
    }
    let tmp = sibling(path, ".tmp");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("작업 상태를 저장할 수 없습니다: {} — {e}", path.display())
    })
}

/// Why `load` produced no workspace although a file exists.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    /// Unparsable or inconsistent; the original was moved to `.bak` (the path is free to rewrite).
    Corrupt(String),
    /// Could not be read, or could not be moved to `.bak`: the original is still at the path and
    /// must NOT be overwritten (start the Saver disabled).
    Unreadable(String),
}

impl LoadError {
    /// Korean line for the user-visible error list.
    pub fn user_message(&self) -> &'static str {
        match self {
            LoadError::Corrupt(_) => CORRUPT_MESSAGE,
            LoadError::Unreadable(_) => UNREADABLE_MESSAGE,
        }
    }
    pub fn detail(&self) -> &str {
        match self {
            LoadError::Corrupt(m) | LoadError::Unreadable(m) => m,
        }
    }
    /// Whether the saver may write to the path afterwards.
    pub fn may_overwrite(&self) -> bool {
        matches!(self, LoadError::Corrupt(_))
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.detail())
    }
}

/// Read with a few retries for transient errors (sharing violations from antivirus/sync tools).
fn read_with_retry(path: &Path, attempts: u32, backoff: Duration) -> std::io::Result<Vec<u8>> {
    let mut last = None;
    for i in 0..attempts {
        match std::fs::read(path) {
            Ok(b) => return Ok(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => last = Some(e),
        }
        if i + 1 < attempts {
            std::thread::sleep(backoff);
        }
    }
    Err(last.expect("attempts >= 1"))
}

/// `Ok(None)` when the file does not exist. A file that cannot be parsed, or whose contents are
/// inconsistent (`Workspace::normalize_and_validate`), is moved to `<path>.bak` and reported as
/// `LoadError::Corrupt`. If it cannot be read (3 tries) or the move fails, the result is
/// `LoadError::Unreadable` and the original stays untouched.
pub fn load(path: &Path) -> Result<Option<Workspace>, LoadError> {
    let bytes = match read_with_retry(path, 3, Duration::from_millis(100)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(LoadError::Unreadable(format!("작업 상태 파일을 읽을 수 없습니다: {} — {e}", path.display()))),
    };
    let parsed = serde_json::from_slice::<Workspace>(&bytes)
        .map_err(|e| format!("{}:{}:{} — {e}", path.display(), e.line(), e.column()))
        .and_then(|mut ws| ws.normalize_and_validate().map(|()| ws));
    match parsed {
        Ok(ws) => Ok(Some(ws)),
        Err(cause) => {
            let bak = sibling(path, ".bak");
            match std::fs::rename(path, &bak) {
                Ok(()) => Err(LoadError::Corrupt(format!("{cause} ({} 로 옮겼습니다)", bak.display()))),
                Err(e) => Err(LoadError::Unreadable(format!("{cause} (백업 실패, 원본 유지: {e})"))),
            }
        }
    }
}

/// Restore plan (spec §9, Task 12) for a workspace loaded from workspace.json: runtime reset,
/// roles re-derived from the current config, team panes / team areas dropped, standing role
/// panes respawned as new sessions with their start prompt, everything else as in phase 1. The
/// model is only touched through `Workspace::restore_roles` (see there for the exact rules).
/// The result passes `normalize_and_validate`; callers still check and fall back to a fresh
/// start if it does not.
pub fn restore_plan(mut ws: Workspace, loaded: &Loaded) -> (Workspace, Effects) {
    let fx = ws.restore_roles(loaded);
    (ws, fx)
}

/// In-place form of [`restore_plan`].
pub fn prepare_restored(ws: &mut Workspace, loaded: &Loaded) -> Effects {
    ws.restore_roles(loaded)
}

enum Msg {
    Dirty,
    Flush(Sender<()>),
}

/// Background saver: `notify()` after every model change; saves 500ms after the last one
/// (at most 5s after the first of a continuous burst). Dropping it saves a pending change.
pub struct Saver {
    /// `None` = disabled (an existing workspace.json must be left untouched).
    tx: Option<Sender<Msg>>,
}

impl Saver {
    /// `snapshot` returns a clone of the model (or `None` when unavailable); it runs on the
    /// saver thread and must release its lock before returning, so no lock is held during IO.
    pub fn new(path: PathBuf, snapshot: impl Fn() -> Option<Workspace> + Send + 'static) -> Saver {
        let (tx, rx) = mpsc::channel::<Msg>();
        let save = move || {
            if let Some(ws) = snapshot() {
                if let Err(e) = save_atomic(&path, &ws) {
                    eprintln!("[agentboard] {e}");
                }
            }
        };
        std::thread::Builder::new()
            .name("saver".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    match first {
                        Msg::Flush(ack) => {
                            save();
                            let _ = ack.send(());
                        }
                        Msg::Dirty => {
                            let started = Instant::now();
                            loop {
                                let left = (started + MAX_DELAY).saturating_duration_since(Instant::now());
                                match rx.recv_timeout(DEBOUNCE.min(left)) {
                                    Ok(Msg::Dirty) if !left.is_zero() => continue,
                                    Ok(Msg::Flush(ack)) => {
                                        save();
                                        let _ = ack.send(());
                                        break;
                                    }
                                    Ok(Msg::Dirty) | Err(RecvTimeoutError::Timeout) => {
                                        save();
                                        break;
                                    }
                                    Err(RecvTimeoutError::Disconnected) => {
                                        save();
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .expect("saver thread");
        Saver { tx: Some(tx) }
    }

    /// A saver that never writes (used when an existing workspace.json could not be preserved).
    pub fn disabled() -> Saver {
        Saver { tx: None }
    }

    pub fn notify(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Dirty);
        }
    }

    /// Save now (blocks until written, at most a few seconds). Used at app exit.
    pub fn flush(&self) {
        let Some(tx) = &self.tx else { return };
        let (ack_tx, ack_rx) = mpsc::channel();
        if tx.send(Msg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(FLUSH_WAIT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};
    use crate::layout::{Dir, LayoutNode};
    use crate::roles::Role;
    use crate::workspace::{Counters, Location, PaneKind, PaneStatus, WsCommand};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn loaded() -> Loaded {
        let root = tempfile::tempdir().unwrap().keep();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(&root).unwrap();
        std::fs::write(
            root.join(".agentboard/config.json"),
            r#"{"tabs":[{"name":"api","dir":"api","template":"team-3"}],
                "groups":{"split":"horizontal","children":[
                    {"group":{"name":"control","tabs":["dispatcher"]}},
                    {"group":{"name":"work","tabs":["api"]}}]}}"#,
        )
        .unwrap();
        let l = load_all(appdata.path());
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        l
    }

    /// Non-trivial workspace: split pane, extra tab, moved focus, status/attention set.
    fn busy(l: &Loaded) -> Workspace {
        let mut ws = Workspace::from_config(l);
        let api = ws.tabs.values().find(|t| t.name == "api").unwrap().clone();
        let work = ws.group_of_tab(&api.id).unwrap().clone();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: Some(api.id.clone()), pane: None }, l).unwrap();
        let p = api.layout.leaves()[1].clone();
        ws.apply(WsCommand::SplitPane { pane: p.clone(), dir: Dir::Vertical }, l).unwrap();
        ws.apply(
            WsCommand::CreateTab { name: "web".into(), dir: "web".into(), template: Some("solo".into()), group: None, role: None },
            l,
        )
        .unwrap();
        ws.apply(WsCommand::SetAttention { pane: p.clone(), on: true, reason: None }, l).unwrap();
        ws.set_status(&p, PaneStatus::Exited { code: 2 });
        ws.set_prompt_warning(&p, true);
        ws
    }

    fn path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("workspace.json")
    }

    #[test]
    fn roundtrip_preserves_groups_tabs_layout_focus() {
        let l = loaded();
        let ws = busy(&l);
        let dir = tempfile::tempdir().unwrap();
        save_atomic(&path(&dir), &ws).unwrap();
        let back = load(&path(&dir)).unwrap().expect("file exists");
        assert_eq!(back, ws);
        assert_eq!(back.group_layout.leaves().len(), 2);
        assert_eq!(back.counters, ws.counters);
    }

    #[test]
    fn unreadable_file_is_not_corrupt_and_stays_untouched() {
        // a directory at the path: present, but not readable as a file
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(path(&dir)).unwrap();
        std::fs::write(path(&dir).join("keep.txt"), "x").unwrap();
        let e = load(&path(&dir)).unwrap_err();
        assert!(matches!(e, LoadError::Unreadable(_)), "{e:?}");
        assert!(!e.may_overwrite());
        assert_eq!(e.user_message(), UNREADABLE_MESSAGE);
        assert_ne!(UNREADABLE_MESSAGE, CORRUPT_MESSAGE);
        // startup uses a disabled saver: a save attempt must not touch the original
        let saver = Saver::disabled();
        saver.notify();
        saver.flush();
        assert!(path(&dir).is_dir() && path(&dir).join("keep.txt").exists());
        assert!(!dir.path().join("workspace.json.tmp").exists());
    }

    #[test]
    fn bak_rename_failure_keeps_original() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(&dir), "{ not json").unwrap();
        let bak = dir.path().join("workspace.json.bak");
        std::fs::create_dir(&bak).unwrap();
        std::fs::write(bak.join("x"), "x").unwrap(); // non-empty dir: rename onto it fails
        let e = load(&path(&dir)).unwrap_err();
        assert!(matches!(e, LoadError::Unreadable(_)), "{e:?}");
        assert!(!e.may_overwrite());
        assert_eq!(std::fs::read_to_string(path(&dir)).unwrap(), "{ not json");
    }

    #[test]
    fn corrupt_file_reports_corrupt_and_may_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(&dir), "{ not json").unwrap();
        let e = load(&path(&dir)).unwrap_err();
        assert!(matches!(e, LoadError::Corrupt(_)));
        assert!(e.may_overwrite());
        assert_eq!(e.user_message(), CORRUPT_MESSAGE);
    }

    #[test]
    fn read_retry_gives_up_on_persistent_error_but_not_on_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = read_with_retry(&path(&dir), 3, Duration::from_millis(1)).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        std::fs::create_dir(path(&dir)).unwrap();
        assert!(read_with_retry(&path(&dir), 3, Duration::from_millis(1)).is_err());
    }

    #[test]
    fn load_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&path(&dir)).unwrap(), None);
    }

    #[test]
    fn corrupt_file_moves_to_bak() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(&dir), "{ not json").unwrap();
        assert!(load(&path(&dir)).is_err());
        assert!(!path(&dir).exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("workspace.json.bak")).unwrap(), "{ not json");
        // a second corrupt file replaces the old .bak
        std::fs::write(path(&dir), "again").unwrap();
        assert!(load(&path(&dir)).is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("workspace.json.bak")).unwrap(), "again");
    }

    type Mutation = (&'static str, Box<dyn Fn(&mut Workspace)>);

    #[test]
    fn integrity_broken_file_moves_to_bak() {
        let l = loaded();
        let good = busy(&l);
        let dir = tempfile::tempdir().unwrap();
        let cases: Vec<Mutation> = vec![
            ("group missing from map", Box::new(|w| {
                let g = w.groups.keys().next().unwrap().clone();
                w.groups.remove(&g);
            })),
            ("tab in no group", Box::new(|w| {
                let g = w.groups.values_mut().find(|g| g.tab_ids.len() > 1).unwrap();
                let t = g.tab_ids.pop().unwrap();
                if g.active_tab_id == t {
                    g.active_tab_id = g.tab_ids[0].clone();
                }
            })),
            ("tab in two groups", Box::new(|w| {
                let t = w.groups.values().next().unwrap().tab_ids[0].clone();
                w.groups.values_mut().last().unwrap().tab_ids.push(t);
            })),
            ("pane with wrong tab", Box::new(|w| {
                w.panes.values_mut().next().unwrap().tab = "t99".into();
            })),
            ("pane missing from map", Box::new(|w| {
                let p = w.panes.keys().next().unwrap().clone();
                w.panes.remove(&p);
            })),
            ("orphan pane", Box::new(|w| {
                let mut p = w.panes.values().next().unwrap().clone();
                p.id = "p99".into();
                w.panes.insert("p99".into(), p);
            })),
            ("focus group missing", Box::new(|w| w.focus.group_id = "g99".into())),
            ("last focused pane foreign", Box::new(|w| {
                w.tabs.values_mut().next().unwrap().last_focused_pane_id = "p99".into();
            })),
            ("active tab foreign", Box::new(|w| {
                w.groups.values_mut().next().unwrap().active_tab_id = "t99".into();
            })),
            ("last location invalid", Box::new(|w| {
                w.last_location = Some(Location { group_id: "g99".into(), tab_id: "t1".into() });
            })),
            ("no dispatcher", Box::new(|w| {
                for t in w.tabs.values_mut() {
                    t.is_dispatcher = false;
                }
            })),
        ];
        for (name, f) in cases {
            let mut bad = good.clone();
            f(&mut bad);
            std::fs::write(path(&dir), serde_json::to_string(&bad).unwrap()).unwrap();
            let r = load(&path(&dir));
            assert!(r.is_err(), "{name}: expected Err, got {r:?}");
            assert!(!path(&dir).exists(), "{name}");
            assert!(dir.path().join("workspace.json.bak").exists(), "{name}");
        }
    }

    #[test]
    fn load_normalizes_layouts_and_bumps_counters() {
        let l = loaded();
        let mut ws = busy(&l);
        // hand-edited: sizes that do not sum to 100, counters behind
        if let LayoutNode::Split { sizes, .. } = &mut ws.group_layout {
            *sizes = vec![1.0, 3.0];
        } else {
            panic!("two groups expected");
        }
        ws.counters = Counters { group: 0, tab: 0, pane: 0 };
        let dir = tempfile::tempdir().unwrap();
        save_atomic(&path(&dir), &ws).unwrap();
        let back = load(&path(&dir)).unwrap().unwrap();
        match &back.group_layout {
            LayoutNode::Split { sizes, .. } => assert_eq!(sizes, &vec![25.0, 75.0]),
            _ => panic!(),
        }
        let max = |ids: Vec<&String>| ids.iter().map(|i| i[1..].parse::<u64>().unwrap()).max().unwrap();
        assert_eq!(back.counters.group, max(back.groups.keys().collect()));
        assert_eq!(back.counters.tab, max(back.tabs.keys().collect()));
        assert_eq!(back.counters.pane, max(back.panes.keys().collect()));
        // the next id is fresh
        let mut b = back.clone();
        b.apply(
            WsCommand::CreateTab { name: "z".into(), dir: "z".into(), template: Some("solo".into()), group: None, role: None },
            &l,
        )
        .unwrap();
        assert_eq!(b.tabs.len(), back.tabs.len() + 1);
    }

    /// A workspace.json written by phase 1 (no kind/role/team/sessionName on panes, no role on
    /// tabs) loads, and the new fields take their defaults.
    #[test]
    fn phase1_workspace_json_still_loads() {
        let phase1 = r#"{
          "groupLayout": {"leaf": "g1"},
          "groups": {"g1": {"id": "g1", "name": "main", "tabIds": ["t1", "t2"], "activeTabId": "t1"}},
          "tabs": {
            "t1": {"id": "t1", "name": "dispatcher", "dir": "", "template": "dispatcher", "isDispatcher": true,
                   "workspace": "C:/workspace", "layout": {"leaf": "p1"}, "lastFocusedPaneId": "p1",
                   "maximizedPaneId": null, "syncInput": false},
            "t2": {"id": "t2", "name": "api", "dir": "api", "template": "solo", "isDispatcher": false,
                   "workspace": "C:/workspace/api",
                   "layout": {"split": "horizontal", "sizes": [50, 50], "children": [{"leaf": "p2"}, {"leaf": "p3"}]},
                   "lastFocusedPaneId": "p2", "maximizedPaneId": null, "syncInput": true}
          },
          "panes": {
            "p1": {"id": "p1", "name": "dispatcher", "tab": "t1", "agent": "claude", "args": [], "env": {},
                   "cwd": "C:/workspace", "prompt": null, "status": {"state": "running"}, "attention": false,
                   "promptWarning": false},
            "p2": {"id": "p2", "name": "main", "tab": "t2", "agent": "claude", "args": ["--x"], "env": {"A": "1"},
                   "cwd": "C:/workspace/api", "prompt": "hi", "status": {"state": "exited", "code": 1},
                   "attention": true, "promptWarning": false},
            "p3": {"id": "p3", "name": "pane-1", "tab": "t2", "agent": "cmd", "args": [], "env": {},
                   "cwd": "C:/workspace/api", "prompt": null, "status": {"state": "starting"}, "attention": false,
                   "promptWarning": true}
          },
          "focus": {"groupId": "g1"},
          "lastLocation": null,
          "groupMaximized": null,
          "counters": {"group": 1, "tab": 2, "pane": 3}
        }"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(&dir), phase1).unwrap();
        let ws = load(&path(&dir)).unwrap().expect("phase-1 file loads");
        assert_eq!(ws.tabs.len(), 2);
        assert!(ws.tabs.values().all(|t| t.role.is_none()));
        for p in ws.panes.values() {
            assert_eq!(p.kind, PaneKind::Terminal, "{}", p.id);
            assert_eq!((p.role, p.team.clone(), p.session_name.clone()), (None, None, None), "{}", p.id);
        }
        assert_eq!(ws.panes["p2"].args, ["--x"]);
        // saving writes the new fields; the result round-trips
        save_atomic(&path(&dir), &ws).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path(&dir)).unwrap()).unwrap();
        assert_eq!(v["panes"]["p1"]["kind"], serde_json::json!({"type": "terminal"}));
        assert_eq!(load(&path(&dir)).unwrap().unwrap(), ws);

        // phase-2 invariants are checked on load
        let base: serde_json::Value = serde_json::from_str(phase1).unwrap();
        let cases: [(&str, fn(&mut serde_json::Value)); 2] = [
            ("applet with a role", |v| {
                v["panes"]["p3"]["kind"] = serde_json::json!({"type": "applet", "kind": "inbox"});
                v["panes"]["p3"]["agent"] = serde_json::json!("");
                v["panes"]["p3"]["role"] = serde_json::json!("member");
            }),
            ("duplicate sessionName", |v| {
                v["panes"]["p2"]["sessionName"] = serde_json::json!("s");
                v["panes"]["p3"]["sessionName"] = serde_json::json!("s");
            }),
        ];
        for (name, f) in cases {
            let mut v = base.clone();
            f(&mut v);
            std::fs::write(path(&dir), v.to_string()).unwrap();
            let r = load(&path(&dir));
            assert!(matches!(r, Err(LoadError::Corrupt(_))), "{name}: {r:?}");
        }
    }

    // ───────── restore rules (Task 12) ─────────

    const MGR_TPL: &str = r#"{"name":"mgr","layout":{"split":"horizontal","sizes":[70,30],"children":[
        {"pane":{"name":"manager","focus":true,"prompt":"시작 리뷰를 하세요"}},
        {"pane":{"name":"shell","agent":"cmd"}}]}}"#;

    /// Loaded config `config` (+ template `mgr`); the temp root lives as long as the guard.
    fn loaded_with(config: &str) -> (Loaded, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/templates/mgr.json"), MGR_TPL).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), config).unwrap();
        let l = load_all(appdata.path());
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        (l, root)
    }

    const ROLE_CONFIG: &str = r#"{"tabs":[
        {"name":"billing","role":"manager","dir":"billing","template":"mgr"},
        {"name":"handover","role":"handover","dir":""},
        {"name":"api","dir":"api","template":"team-3"}]}"#;

    fn tab_id(ws: &Workspace, name: &str) -> String {
        ws.tabs.values().find(|t| t.name == name).unwrap_or_else(|| panic!("no tab {name}")).id.clone()
    }

    /// Through workspace.json, as at a real restart.
    fn reload(ws: &Workspace) -> Workspace {
        let dir = tempfile::tempdir().unwrap();
        save_atomic(&path(&dir), ws).unwrap();
        load(&path(&dir)).unwrap().unwrap()
    }

    fn assert_no_degenerate_splits(n: &LayoutNode<String>) {
        if let LayoutNode::Split { children, sizes, .. } = n {
            assert!(children.len() >= 2, "single-child split: {n:?}");
            assert_eq!(sizes.len(), children.len());
            children.iter().for_each(assert_no_degenerate_splits);
        }
    }

    #[test]
    fn restore_plan_drops_team_panes_and_reinjects_prompts() {
        let (l, _root) = loaded_with(ROLE_CONFIG);
        let mut ws = Workspace::from_config(&l);
        let billing = tab_id(&ws, "billing");
        let before_layout = ws.tabs[&billing].layout.clone();
        for (team, role) in [("a", Role::Lead), ("a", Role::Member), ("a", Role::Member), ("b", Role::Lead)] {
            ws.apply(WsCommand::AddTeamPane { tab: billing.clone(), team: team.into(), role, prompt: Some("팀".into()) }, &l).unwrap();
        }
        // a team pane focused and maximized
        let m1 = ws.panes.values().find(|p| p.session_name.as_deref() == Some("billing-a-m1")).unwrap().id.clone();
        ws.tabs.get_mut(&billing).unwrap().last_focused_pane_id = m1.clone();
        ws.tabs.get_mut(&billing).unwrap().maximized_pane_id = Some(m1);
        assert_eq!(ws.tabs[&billing].team_seq.get("a"), Some(&2));
        assert_eq!(ws.panes.values().filter(|p| p.team.is_some()).count(), 4);
        let saved = reload(&ws);

        let (back, fx) = restore_plan(saved, &l);
        // team panes and areas gone, layout back to the template's, teamSeq reset
        assert!(back.panes.values().all(|p| p.team.is_none() && !matches!(p.role, Some(Role::Lead | Role::Member))));
        assert_eq!(back.tabs[&billing].layout, before_layout, "team area collapsed into the Manager pane");
        assert!(back.tabs.values().all(|t| t.team_seq.is_empty()));
        for t in back.tabs.values() {
            assert_no_degenerate_splits(&t.layout);
        }
        let mgr = back.standing_pane(&billing).unwrap();
        assert_eq!(back.tabs[&billing].last_focused_pane_id, before_layout.leaves()[0]);
        assert_eq!(back.tabs[&billing].maximized_pane_id, None);
        // standing panes: new session + start prompt; everything else as in phase 1
        let inject: BTreeMap<String, bool> = fx.spawn.iter().cloned().collect();
        assert_eq!(inject.len(), back.panes.len(), "every pane is spawned");
        assert!(fx.kill.is_empty());
        let standing: Vec<(String, Option<Role>, Option<String>)> = back
            .panes
            .values()
            .filter(|p| inject[&p.id])
            .map(|p| (back.tabs[&p.tab].name.clone(), p.role, p.session_name.clone()))
            .collect();
        assert_eq!(
            standing,
            [
                ("dispatcher".to_string(), Some(Role::Dispatcher), Some("dispatcher".to_string())),
                ("billing".to_string(), Some(Role::Manager), Some("billing".to_string())),
                ("handover".to_string(), Some(Role::Handover), Some("handover".to_string())),
            ]
        );
        assert_eq!(back.panes[&mgr].prompt.as_deref(), Some("시작 리뷰를 하세요"));
        let api = tab_id(&back, "api");
        assert!(back.panes.values().filter(|p| p.tab == api).all(|p| !inject[&p.id] && p.role.is_none()));
        assert!(back.panes.values().all(|p| p.status == PaneStatus::Starting && !p.attention));
        let mut v = back.clone();
        assert_eq!(v.normalize_and_validate(), Ok(()));
        // the restored workspace hires again from scratch (member numbers restart)
        let mut again = back;
        again.apply(WsCommand::AddTeamPane { tab: billing.clone(), team: "a".into(), role: Role::Lead, prompt: None }, &l).unwrap();
        again.apply(WsCommand::AddTeamPane { tab: billing.clone(), team: "a".into(), role: Role::Member, prompt: None }, &l).unwrap();
        assert!(again.panes.values().any(|p| p.session_name.as_deref() == Some("billing-a-m1")));
    }

    #[test]
    fn non_role_tabs_restore_as_phase1() {
        let l = loaded();
        let ws = reload(&busy(&l));
        let (back, fx) = restore_plan(ws.clone(), &l);
        assert_eq!(fx.spawn.len(), back.panes.len());
        assert!(fx.kill.is_empty());
        let ids: Vec<_> = fx.spawn.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(ids, back.panes.keys().cloned().collect::<Vec<_>>());
        for (id, inject) in &fx.spawn {
            let p = &back.panes[id];
            let t = &back.tabs[&p.tab];
            if t.is_dispatcher {
                assert!(*inject && p.role == Some(Role::Dispatcher), "{id}");
            } else {
                // plain tabs: no role, no prompt injection, everything else as saved
                assert!(!*inject && p.role.is_none() && p.session_name.is_none() && t.role.is_none(), "{id}");
                let mut old = ws.panes[id].clone();
                old.status = PaneStatus::Starting;
                old.clear_attention();
                old.prompt_warning = false;
                assert_eq!(p, &old, "{id}");
            }
        }
        // layout, focus, groups untouched
        assert_eq!(back.group_layout, ws.group_layout);
        assert_eq!(back.groups, ws.groups);
        assert_eq!(back.focus, ws.focus);
        for (tid, t) in &ws.tabs {
            assert_eq!(back.tabs[tid].layout, t.layout, "{tid}");
        }
        assert!(back.panes.values().all(|p| p.status == PaneStatus::Starting && !p.attention && !p.prompt_warning && p.attention_reason.is_none()));
    }

    /// Phase-1 workspace.json (no roles at all) + a phase-2 config: roles come from config.
    #[test]
    fn restore_rederives_roles_from_config() {
        let (l, root) = loaded_with(ROLE_CONFIG);
        let r = root.path().to_string_lossy().replace('\\', "/");
        let phase1 = format!(
            r#"{{
          "groupLayout": {{"leaf": "g1"}},
          "groups": {{"g1": {{"id": "g1", "name": "main", "tabIds": ["t1", "t2", "t3", "t4"], "activeTabId": "t2"}}}},
          "tabs": {{
            "t1": {{"id": "t1", "name": "dispatcher", "dir": "", "template": "dispatcher", "isDispatcher": true,
                   "workspace": "{r}", "layout": {{"leaf": "p1"}}, "lastFocusedPaneId": "p1",
                   "maximizedPaneId": null, "syncInput": false}},
            "t2": {{"id": "t2", "name": "billing", "dir": "billing", "template": "mgr", "isDispatcher": false,
                   "workspace": "{r}/billing",
                   "layout": {{"split": "horizontal", "sizes": [70, 30], "children": [{{"leaf": "p2"}}, {{"leaf": "p3"}}]}},
                   "lastFocusedPaneId": "p3", "maximizedPaneId": null, "syncInput": false}},
            "t3": {{"id": "t3", "name": "ops", "dir": "ops", "template": "mgr", "isDispatcher": false,
                   "workspace": "{r}/ops",
                   "layout": {{"split": "horizontal", "sizes": [70, 30], "children": [{{"leaf": "p4"}}, {{"leaf": "p5"}}]}},
                   "lastFocusedPaneId": "p4", "maximizedPaneId": null, "syncInput": false}},
            "t4": {{"id": "t4", "name": "handover", "dir": "elsewhere", "template": "solo", "isDispatcher": false,
                   "workspace": "{r}/elsewhere", "layout": {{"leaf": "p6"}}, "lastFocusedPaneId": "p6",
                   "maximizedPaneId": null, "syncInput": false}}
          }},
          "panes": {{
            "p1": {{"id": "p1", "name": "dispatcher", "tab": "t1", "agent": "claude", "args": [], "env": {{}},
                   "cwd": "{r}", "prompt": null, "status": {{"state": "running"}}, "attention": false, "promptWarning": false}},
            "p2": {{"id": "p2", "name": "shell", "tab": "t2", "agent": "cmd", "args": [], "env": {{}},
                   "cwd": "{r}/billing", "prompt": null, "status": {{"state": "running"}}, "attention": false, "promptWarning": false}},
            "p3": {{"id": "p3", "name": "manager", "tab": "t2", "agent": "claude", "args": [], "env": {{}},
                   "cwd": "{r}/billing", "prompt": "옛 프롬프트", "status": {{"state": "running"}}, "attention": true, "promptWarning": false}},
            "p4": {{"id": "p4", "name": "manager", "tab": "t3", "agent": "claude", "args": [], "env": {{}},
                   "cwd": "{r}/ops", "prompt": "옛 프롬프트", "status": {{"state": "running"}}, "attention": false, "promptWarning": false}},
            "p5": {{"id": "p5", "name": "shell", "tab": "t3", "agent": "cmd", "args": [], "env": {{}},
                   "cwd": "{r}/ops", "prompt": null, "status": {{"state": "running"}}, "attention": false, "promptWarning": false}},
            "p6": {{"id": "p6", "name": "main", "tab": "t4", "agent": "claude", "args": [], "env": {{}},
                   "cwd": "{r}/elsewhere", "prompt": null, "status": {{"state": "running"}}, "attention": false, "promptWarning": false}}
          }},
          "focus": {{"groupId": "g1"}}, "lastLocation": null, "groupMaximized": null,
          "counters": {{"group": 1, "tab": 4, "pane": 6}}
        }}"#
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(&dir), phase1).unwrap();
        let ws = load(&path(&dir)).unwrap().unwrap();
        let (back, fx) = restore_plan(ws, &l);
        let inject: BTreeMap<String, bool> = fx.spawn.iter().cloned().collect();
        let role = |p: &str| (back.panes[p].role, back.panes[p].session_name.clone(), inject[p]);
        // dispatcher: always; the template's pane by name
        assert_eq!(back.tabs["t1"].role, Some(Role::Dispatcher));
        assert_eq!(role("p1"), (Some(Role::Dispatcher), Some("dispatcher".into()), true));
        // billing: config says manager (same name + dir) → the template's focus pane (by name,
        // not layout position), prompt re-read from the template
        assert_eq!(back.tabs["t2"].role, Some(Role::Manager));
        assert_eq!(role("p3"), (Some(Role::Manager), Some("billing".into()), true));
        assert_eq!(role("p2"), (None, None, false));
        assert_eq!(back.panes["p3"].prompt.as_deref(), Some("시작 리뷰를 하세요"));
        // ops: not in config → plain phase-1 tab, saved prompt kept
        assert_eq!(back.tabs["t3"].role, None);
        assert_eq!(role("p4"), (None, None, false));
        assert_eq!(back.panes["p4"].prompt.as_deref(), Some("옛 프롬프트"));
        // "handover" with a different dir than config → plain as well
        assert_eq!(back.tabs["t4"].role, None);
        assert_eq!(role("p6"), (None, None, false));
        let mut v = back.clone();
        assert_eq!(v.normalize_and_validate(), Ok(()));

        // the role disappears from config later: the saved Manager tab (with a team) turns plain
        let mut with_team = back.clone();
        with_team.apply(WsCommand::AddTeamPane { tab: "t2".into(), team: "x".into(), role: Role::Lead, prompt: None }, &l).unwrap();
        let (l2, _root2) = loaded_with(r#"{"tabs":[{"name":"billing","dir":"billing","template":"mgr"}]}"#);
        let (plain, fx) = restore_plan(reload(&with_team), &l2);
        assert_eq!(plain.tabs["t2"].role, None);
        assert!(plain.panes.values().filter(|p| p.tab == "t2").all(|p| p.role.is_none() && p.session_name.is_none() && p.team.is_none()));
        assert_eq!(plain.tabs["t2"].layout, back.tabs["t2"].layout);
        assert!(fx.spawn.iter().filter(|(p, _)| plain.panes[p].tab == "t2").all(|(_, i)| !*i));
        let mut v = plain.clone();
        assert_eq!(v.normalize_and_validate(), Ok(()));
    }

    #[test]
    fn restore_uses_the_current_tab_name_for_the_session() {
        let (l, _root) = loaded_with(ROLE_CONFIG);
        let mut ws = Workspace::from_config(&l);
        let billing = tab_id(&ws, "billing");
        // rename keeps the live session name (R15); config follows the new name
        ws.apply(WsCommand::RenameTab { tab: billing.clone(), name: "billing2".into() }, &l).unwrap();
        let mgr = ws.standing_pane(&billing).unwrap();
        assert_eq!(ws.panes[&mgr].session_name.as_deref(), Some("billing"));
        let (l2, _r2) = loaded_with(&ROLE_CONFIG.replace(r#""name":"billing""#, r#""name":"billing2""#));
        let (back, _) = restore_plan(reload(&ws), &l2);
        assert_eq!(back.panes[&mgr].session_name.as_deref(), Some("billing2"));
        assert_eq!(back.panes[&mgr].role, Some(Role::Manager));
    }

    #[test]
    fn save_is_atomic_tmp_then_rename() {
        let l = loaded();
        let ws = Workspace::from_config(&l);
        let dir = tempfile::tempdir().unwrap();
        save_atomic(&path(&dir), &ws).unwrap();
        save_atomic(&path(&dir), &ws).unwrap(); // overwrite
        assert!(path(&dir).exists());
        assert!(!dir.path().join("workspace.json.tmp").exists());
        // a missing parent directory is created
        let nested = dir.path().join("a/b/workspace.json");
        save_atomic(&nested, &ws).unwrap();
        assert!(nested.exists());
    }

    fn counting_saver(dir: &tempfile::TempDir) -> (Saver, Arc<AtomicUsize>) {
        let l = loaded();
        let ws = Workspace::from_config(&l);
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let saver = Saver::new(path(dir), move || {
            n2.fetch_add(1, Ordering::SeqCst);
            Some(ws.clone())
        });
        (saver, n)
    }

    #[test]
    fn saver_debounces_bursts_into_one_save() {
        let dir = tempfile::tempdir().unwrap();
        let (saver, n) = counting_saver(&dir);
        for _ in 0..5 {
            saver.notify();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!path(&dir).exists(), "must not save before the debounce elapsed");
        std::thread::sleep(DEBOUNCE + Duration::from_millis(400));
        assert!(path(&dir).exists());
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn saver_flush_saves_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let (saver, n) = counting_saver(&dir);
        saver.notify();
        let t = Instant::now();
        saver.flush();
        assert!(t.elapsed() < DEBOUNCE, "flush must not wait for the debounce");
        assert!(path(&dir).exists());
        assert_eq!(n.load(Ordering::SeqCst), 1);
        // nothing is pending afterwards: the debounce timer does not save again
        std::thread::sleep(DEBOUNCE + Duration::from_millis(300));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }
}
