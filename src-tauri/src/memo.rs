//! The memo next to the dispatcher (spec 2026-10-26 memo design): `workspaceRoot/MEMO.md`,
//! today's items the user picked. Rust only creates the file, backs it up and writes it whole;
//! splitting header and body is the frontend's (memo-logic.js), the dispatcher edits the file
//! directly. Conflicts: the last writer wins, and whenever the content changes — the app's own
//! save, or an external edit that `read` notices — the previous content is kept as
//! `.agentboard/memo.bak` (one copy).

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// The memo file, relative to workspaceRoot (the frontend passes it to `doc_watch`).
pub const MEMO_FILE: &str = "MEMO.md";
/// The one backup, relative to workspaceRoot.
pub const MEMO_BAK: &str = ".agentboard/memo.bak";
/// Header draft written when MEMO.md is missing (writing rules; the body after `## 메모` empty).
pub const DEFAULT_MEMO: &str = include_str!("../defaults/memo.md");

/// The content the app last read or wrote this run (`None` before the first). Its lock is a
/// leaf held across the MEMO.md IO, so reads and saves are serialized.
#[derive(Default)]
pub struct MemoState {
    last: Mutex<Option<String>>,
}

/// Create MEMO.md with [`DEFAULT_MEMO`] when it is missing (app start, first read); an existing
/// file is never touched. `Ok(true)` when it was created.
pub fn ensure(root: &Path) -> Result<bool, String> {
    let path = root.join(MEMO_FILE);
    if path.exists() {
        return Ok(false);
    }
    write_atomic(&path, DEFAULT_MEMO)?;
    Ok(true)
}

/// `memo_read`: `(MEMO_FILE, text)`, creating the file first if needed. If the text differs from
/// what the app last knew (an external edit), that previous content goes to the backup; a
/// failed backup is logged and does not keep the memo from showing.
pub fn read(root: &Path, st: &MemoState) -> Result<(String, String), String> {
    let mut last = st.last.lock().unwrap();
    ensure(root)?;
    let text = read_text(root)?;
    if let Some(prev) = last.as_deref() {
        if prev != text {
            if let Err(e) = write_atomic(&root.join(MEMO_BAK), prev) {
                eprintln!("[agentboard] memo backup: {e}");
            }
        }
    }
    *last = Some(text.clone());
    Ok((MEMO_FILE.to_string(), text))
}

/// `memo_save`: back up the current file content, then write `text` atomically. Nothing is
/// written when `text` equals the file (the backup keeps the older content). If the backup
/// cannot be written, the save is refused: never overwrite without one.
pub fn save(root: &Path, st: &MemoState, text: &str) -> Result<(), String> {
    let mut last = st.last.lock().unwrap();
    let path = root.join(MEMO_FILE);
    let current = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{MEMO_FILE}를 읽을 수 없습니다 — {e}")),
    };
    if current.as_deref() != Some(text) {
        if let Some(cur) = &current {
            write_atomic(&root.join(MEMO_BAK), cur).map_err(|e| format!("백업을 만들지 못해 저장하지 않았습니다 — {e}"))?;
        }
        write_atomic(&path, text)?;
    }
    *last = Some(text.to_string());
    Ok(())
}

fn read_text(root: &Path) -> Result<String, String> {
    std::fs::read_to_string(root.join(MEMO_FILE)).map_err(|e| format!("{MEMO_FILE}를 읽을 수 없습니다 — {e}"))
}

/// `<path>.tmp` (fsynced), then renamed over `path` (retried while Windows reports it busy, as
/// for inbox items): readers — the dispatcher, the doc watcher — never see a partial file.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let fail = |e: &dyn std::fmt::Display| format!("{name}을(를) 쓸 수 없습니다 — {e}");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    let tmp = path.with_file_name(format!("{name}.tmp"));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        crate::inbox::rename_with_retry(&tmp, path)
    })();
    res.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(&e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memo(root: &Path) -> String {
        std::fs::read_to_string(root.join(MEMO_FILE)).unwrap()
    }

    fn bak(root: &Path) -> Option<String> {
        std::fs::read_to_string(root.join(MEMO_BAK)).ok()
    }

    fn put(root: &Path, text: &str) {
        std::fs::write(root.join(MEMO_FILE), text).unwrap();
    }

    #[test]
    fn memo_file_created_only_when_missing() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        assert!(ensure(root).unwrap(), "created");
        assert_eq!(memo(root), DEFAULT_MEMO);
        put(root, "mine");
        assert!(!ensure(root).unwrap(), "an existing file is never touched");
        assert_eq!(memo(root), "mine");
        // read creates it as well (the memo pane may open before anything else)
        std::fs::remove_file(root.join(MEMO_FILE)).unwrap();
        let st = MemoState::default();
        assert_eq!(read(root, &st).unwrap(), (MEMO_FILE.to_string(), DEFAULT_MEMO.to_string()));
        assert_eq!(bak(root), None, "creating is not a change to back up");
    }

    /// The header draft: spec §3 for the product ("사용자", no example items), and the body
    /// (after the first exact "## 메모" line) starts empty.
    #[test]
    fn default_memo_header_draft() {
        assert!(DEFAULT_MEMO.starts_with("# 메모 — 사용자가 고른 오늘 할 일\n"));
        assert!(DEFAULT_MEMO.ends_with("\n## 메모\n\n"), "{DEFAULT_MEMO:?}");
        assert_eq!(DEFAULT_MEMO.lines().filter(|l| l.trim() == "## 메모").count(), 1);
        assert!(!DEFAULT_MEMO.contains("형님") && !DEFAULT_MEMO.contains('\r'));
        // product wording, not a quote of one user's instruction
        assert!(DEFAULT_MEMO.contains("\n- 제목은 20자 미만으로 쓴다. 상세 내용은 쓰지 않는다.\n"), "{DEFAULT_MEMO}");
        assert!(!DEFAULT_MEMO.contains("원문"));
    }

    #[test]
    fn memo_save_backs_up_previous_and_writes_atomically() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join(".agentboard")).unwrap();
        put(root, "A");
        let st = MemoState::default();
        save(root, &st, "B").unwrap();
        assert_eq!(memo(root), "B");
        assert_eq!(bak(root).as_deref(), Some("A"));
        assert!(!root.join("MEMO.md.tmp").exists() && !root.join(".agentboard/memo.bak.tmp").exists());
        // saving the same text again changes nothing, so the backup keeps the older content
        save(root, &st, "B").unwrap();
        assert_eq!(bak(root).as_deref(), Some("A"));
        // a save onto an external edit backs up that edit (last writer wins, nothing silently lost)
        put(root, "C");
        save(root, &st, "D").unwrap();
        assert_eq!((memo(root), bak(root).as_deref()), ("D".to_string(), Some("C")));
    }

    #[test]
    fn memo_read_backs_up_on_external_change() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        put(root, "A");
        let st = MemoState::default();
        assert_eq!(read(root, &st).unwrap().1, "A");
        put(root, "C"); // the dispatcher edits the file
        assert_eq!(read(root, &st).unwrap(), (MEMO_FILE.to_string(), "C".to_string()));
        assert_eq!(bak(root).as_deref(), Some("A"), "the content the app knew before the edit");
    }

    #[test]
    fn memo_read_without_change_does_not_touch_backup() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        put(root, "A");
        let st = MemoState::default();
        read(root, &st).unwrap();
        read(root, &st).unwrap();
        assert_eq!(bak(root), None);
        save(root, &st, "B").unwrap();
        read(root, &st).unwrap(); // the app's own save is not an external change
        assert_eq!(bak(root).as_deref(), Some("A"));
    }
}
