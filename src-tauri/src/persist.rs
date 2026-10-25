//! workspace.json save/restore (spec §4.2): atomic save, corrupt/inconsistent files moved to
//! `.bak`, debounced background `Saver`.
//!
//! Lock rules (see `launcher` docs): the `Saver` thread never holds an app lock while writing;
//! its snapshot closure takes `ws` only to clone. Callers notify the saver without holding
//! any lock that the snapshot closure needs in a conflicting order (notify is a channel send).

use crate::workspace::{Effects, Workspace};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_millis(500);
/// A steady stream of notifications cannot postpone a save longer than this.
const MAX_DELAY: Duration = Duration::from_secs(5);
const FLUSH_WAIT: Duration = Duration::from_secs(5);
/// Shown in the error list when workspace.json had to be discarded.
pub const CORRUPT_MESSAGE: &str = "작업 상태 파일이 손상되어 초기 배치로 시작했습니다";

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

/// `Ok(None)` when the file does not exist. A file that cannot be parsed, or whose contents are
/// inconsistent (`Workspace::normalize_and_validate`), is moved to `<path>.bak` and reported as `Err`.
pub fn load(path: &Path) -> Result<Option<Workspace>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("작업 상태 파일을 읽을 수 없습니다: {} — {e}", path.display())),
    };
    let parsed = serde_json::from_slice::<Workspace>(&bytes)
        .map_err(|e| format!("{}:{}:{} — {e}", path.display(), e.line(), e.column()))
        .and_then(|mut ws| ws.normalize_and_validate().map(|()| ws));
    match parsed {
        Ok(ws) => Ok(Some(ws)),
        Err(cause) => {
            let bak = sibling(path, ".bak");
            let moved = match std::fs::rename(path, &bak) {
                Ok(()) => format!("{} 로 옮겼습니다", bak.display()),
                Err(e) => format!("백업 실패: {e}"),
            };
            Err(format!("{cause} ({moved})"))
        }
    }
}

/// Restored workspace -> runtime reset; every pane is spawned without prompt injection (R14:
/// the model is only touched through `Workspace::reset_runtime_state`).
pub fn prepare_restored(ws: &mut Workspace) -> Effects {
    ws.reset_runtime_state()
}

enum Msg {
    Dirty,
    Flush(Sender<()>),
}

/// Background saver: `notify()` after every model change; saves 500ms after the last one
/// (at most 5s after the first of a continuous burst). Dropping it saves a pending change.
pub struct Saver {
    tx: Mutex<Sender<Msg>>,
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
        Saver { tx: Mutex::new(tx) }
    }

    pub fn notify(&self) {
        let _ = self.tx.lock().unwrap().send(Msg::Dirty);
    }

    /// Save now (blocks until written, at most a few seconds). Used at app exit.
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        if self.tx.lock().unwrap().send(Msg::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv_timeout(FLUSH_WAIT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all, Loaded};
    use crate::layout::{Dir, LayoutNode};
    use crate::workspace::{Counters, Location, PaneStatus, WsCommand};
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
            WsCommand::CreateTab { name: "web".into(), dir: "web".into(), template: Some("solo".into()), group: None },
            l,
        )
        .unwrap();
        ws.apply(WsCommand::SetAttention { pane: p.clone(), on: true }, l).unwrap();
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
            WsCommand::CreateTab { name: "z".into(), dir: "z".into(), template: Some("solo".into()), group: None },
            &l,
        )
        .unwrap();
        assert_eq!(b.tabs.len(), back.tabs.len() + 1);
    }

    #[test]
    fn restored_panes_respawn_without_prompt() {
        let l = loaded();
        let mut ws = busy(&l);
        let fx = prepare_restored(&mut ws);
        assert_eq!(fx.spawn.len(), ws.panes.len());
        assert!(fx.spawn.iter().all(|(_, inject)| !*inject));
        assert!(fx.kill.is_empty());
        let ids: Vec<_> = fx.spawn.iter().map(|(p, _)| p.clone()).collect();
        assert_eq!(ids, ws.panes.keys().cloned().collect::<Vec<_>>());
        assert!(ws.panes.values().all(|p| p.status == PaneStatus::Starting && !p.attention && !p.prompt_warning));
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
