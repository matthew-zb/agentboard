//! Inbox store (spec §5): Manager review results ("확인할 것 / 리스크") as items with a
//! timeline, one JSON file per item in `<workspaceRoot>/.agentboard/inbox/<id>.json`
//! (atomic tmp + rename).
//!
//! # Item lifecycle
//! `new` → `in_progress` → `close_proposed` → `done`. Only the user (Tauri command
//! [`Inbox::confirm_done`]) moves an item to `done`, and only from `close_proposed`. A user
//! instruction ([`Inbox::instruct`]) moves `new`/`close_proposed` to `in_progress` (from
//! `close_proposed` it records `reopened` first). `done` is final.
//!
//! # Who may do what
//! Agent-facing methods take the MCP [`Caller`]: `post` needs a Manager (owner tab = caller's
//! tab); `note`, `propose_close` and `link_issue` need the owner Manager (role Manager and
//! `item.owner_tab == caller.tab`). [`Inbox::list_for`]: Dispatcher sees every item, a Manager
//! its own, other roles none. Agent changes mark the item unread.
//!
//! # Retries
//! An MCP call that hits the 30 s deadline may still complete, and the agent may retry. So an
//! identical `post` (same owner tab, title, check, risk, source, refs; not done; within 10
//! minutes) returns the existing id, an identical consecutive `note` by the same caller is
//! recorded once, re-proposing with the same opinion and re-linking the same issue are no-ops.
//!
//! # Locks
//! `inner` is a leaf except for `inner -> emit_lock` (same pattern as `ws -> emit_lock`, R16):
//! a mutation writes the item file and bumps `version` under `inner`, takes `emit_lock`,
//! releases `inner` and only then notifies (Tauri emit) — `inbox-changed` events leave in
//! version order and never while `inner` is held. Never take `ws`/`loaded` while holding
//! `inner`: callers look up tab data first (released) and call the inbox afterwards. File
//! copies of [`Inbox::add_files`] run with no lock held.

use crate::control::Caller;
use crate::issues;
use crate::roles::Role;
use crate::workspace::TabId;
use chrono::{DateTime, Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Tauri event emitted after every inbox mutation; payload [`InboxChanged`].
pub const INBOX_CHANGED: &str = "inbox-changed";
/// `TimelineEntry::who` of user actions (Tauri commands).
pub const WHO_USER: &str = "user";
/// `TimelineEntry::who` / `InboxItem::source` of app-generated items (nightly refresh).
pub const WHO_APP: &str = "app";
/// Window in which an identical `post` is treated as a retry.
const POST_RETRY_SECS: i64 = 600;

/// Wire: `"new" | "in_progress" | "close_proposed" | "done"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    New,
    InProgress,
    CloseProposed,
    Done,
}

impl ItemState {
    pub fn parse(s: &str) -> Option<ItemState> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }
}

/// Wire: `{"type":"posted"}` … `{"type":"close_proposed","opinion":"…"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineKind {
    Posted,
    Note,
    Instruction,
    FilesAdded,
    IssueLinked,
    CloseProposed { opinion: String },
    Reopened,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    /// Local time, RFC 3339 with offset (`2026-10-25T14:03:07+09:00`).
    pub at: String,
    /// Caller's session name, [`WHO_USER`] or [`WHO_APP`].
    pub who: String,
    pub kind: TimelineKind,
    /// Note / instruction text, the opinion (close_proposed), `"input: a.md, b.pdf"`
    /// (files_added), the issue id (issue_linked); empty otherwise.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// `YYYYMMDD-HHMMSS-xxxx` (local time + 4 random hex) = file stem.
    pub id: String,
    pub owner_tab: TabId,
    /// Owner tab's name when posted ("담당 업무").
    pub owner_name: String,
    pub title: String,
    pub check: Vec<String>,
    pub risk: Vec<String>,
    pub source: String,
    #[serde(default)]
    pub refs: Vec<String>,
    pub state: ItemState,
    pub read: bool,
    /// Linked issue id (`YYYYMMDD-slug[-N]`).
    #[serde(default)]
    pub issue: Option<String>,
    /// Absolute issue folder (`<tab workspace>/issues/<issue>`), recorded when linked so the
    /// folder stays reachable even if the tab is closed later.
    #[serde(default)]
    pub issue_path: Option<PathBuf>,
    pub created: String,
    pub timeline: Vec<TimelineEntry>,
}

/// Arguments of `inbox_post`.
#[derive(Debug, Clone, PartialEq)]
pub struct NewItem {
    pub title: String,
    pub check: Vec<String>,
    pub risk: Vec<String>,
    pub source: String,
    pub refs: Vec<String>,
}

/// Reply of the `inbox_list` Tauri command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxList {
    /// Current inbox version (same counter as the `inbox-changed` payload).
    pub version: u64,
    /// Newest first.
    pub items: Vec<InboxItem>,
}

/// Payload of `inbox-changed`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct InboxChanged {
    pub version: u64,
}

/// Issue sub-folder for [`Inbox::add_files`]. Wire: `"input" | "output"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Input,
    Output,
}

impl Target {
    pub fn dir_name(self) -> &'static str {
        match self {
            Target::Input => "input",
            Target::Output => "output",
        }
    }
}

/// Called with the new version after every mutation (production: emit `inbox-changed`).
pub type Notifier = Arc<dyn Fn(u64) + Send + Sync>;

struct Inner {
    items: BTreeMap<String, InboxItem>,
    version: u64,
}

pub struct Inbox {
    dir: PathBuf,
    inner: Mutex<Inner>,
    emit_lock: Mutex<()>,
    notify: Notifier,
}

fn now() -> DateTime<Local> {
    Local::now()
}

fn stamp(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn entry(who: &str, kind: TimelineKind, text: &str) -> TimelineEntry {
    TimelineEntry { at: stamp(now()), who: who.to_string(), kind, text: text.to_string() }
}

fn not_found(id: &str) -> String {
    format!("인박스 항목을 찾을 수 없습니다: {id}")
}

fn non_empty(s: &str, what: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        Err(format!("{what}이(가) 비어 있습니다"))
    } else {
        Ok(t.to_string())
    }
}

fn not_done(item: &InboxItem) -> Result<(), String> {
    if item.state == ItemState::Done {
        Err(format!("완료된 항목입니다: {}", item.id))
    } else {
        Ok(())
    }
}

/// The caller must be the item's owner Manager.
pub fn check_owner(caller: &Caller, item: &InboxItem) -> Result<(), String> {
    if caller.role != Role::Manager {
        return Err(format!("'{}' 역할은 인박스 항목을 바꿀 수 없습니다", caller.role.as_str()));
    }
    if item.owner_tab != caller.tab {
        return Err(format!("이 항목의 담당 Manager가 아닙니다: {}", item.id));
    }
    Ok(())
}

fn file_name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "(이름 없음)".into())
}

/// `name.ext`, `name (2).ext`, `name (3).ext`, …
fn numbered(name: &str, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    let p = Path::new(name);
    match (p.file_stem(), p.extension()) {
        (Some(stem), Some(ext)) => format!("{} ({n}).{}", stem.to_string_lossy(), ext.to_string_lossy()),
        _ => format!("{name} ({n})"),
    }
}

/// Copy `src` into `dest_dir` under a name nobody uses yet (claimed with `create_new`, which
/// also refuses an existing symlink at that name). Returns the new file.
fn copy_unique(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let name = file_name_of(src);
    for n in 1..=10_000u32 {
        let dest = dest_dir.join(numbered(&name, n));
        let out = std::fs::OpenOptions::new().write(true).create_new(true).open(&dest);
        let mut out = match out {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("파일을 만들 수 없습니다: {} — {e}", file_name_of(&dest))),
        };
        let copied = std::fs::File::open(src).and_then(|mut i| std::io::copy(&mut i, &mut out)).and_then(|_| out.sync_all());
        return match copied {
            Ok(()) => Ok(dest),
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&dest);
                Err(format!("파일을 복사할 수 없습니다: {name} — {e}"))
            }
        };
    }
    Err(format!("같은 이름의 파일이 너무 많습니다: {name}"))
}

impl Inbox {
    /// Load every `<id>.json` in `dir` (a missing dir is an empty inbox). Unreadable or
    /// inconsistent files are skipped (left on disk) and reported as Korean lines naming the
    /// file only.
    pub fn load(dir: PathBuf, notify: Notifier) -> (Inbox, Vec<String>) {
        let mut items = BTreeMap::new();
        let mut errors = Vec::new();
        match std::fs::read_dir(&dir) {
            Ok(rd) => {
                for e in rd.filter_map(|e| e.ok()) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Some(stem) = name.strip_suffix(".json") else { continue };
                    let parsed = std::fs::read_to_string(e.path())
                        .map_err(|e| e.to_string())
                        .and_then(|s| serde_json::from_str::<InboxItem>(&s).map_err(|e| e.to_string()));
                    match parsed {
                        Ok(item) if item.id == stem => {
                            items.insert(item.id.clone(), item);
                        }
                        Ok(_) => errors.push(format!("인박스 항목 파일의 id가 파일 이름과 달라 건너뜁니다: {name}")),
                        Err(err) => errors.push(format!("인박스 항목 파일을 읽을 수 없어 건너뜁니다: {name} — {err}")),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => errors.push(format!("인박스 폴더를 읽을 수 없습니다 — {e}")),
        }
        let inbox = Inbox { dir, inner: Mutex::new(Inner { items, version: 0 }), emit_lock: Mutex::new(()), notify };
        (inbox, errors)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn version(&self) -> u64 {
        self.inner.lock().unwrap().version
    }

    fn write(&self, item: &InboxItem) -> Result<(), String> {
        use std::io::Write;
        let fail = |e: &dyn std::fmt::Display| format!("인박스 항목을 저장할 수 없습니다: {} — {e}", item.id);
        let json = serde_json::to_vec_pretty(item).map_err(|e| fail(&e))?;
        std::fs::create_dir_all(&self.dir).map_err(|e| fail(&e))?;
        let path = self.dir.join(format!("{}.json", item.id));
        let tmp = self.dir.join(format!("{}.json.tmp", item.id));
        let res = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)
        })();
        res.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            fail(&e)
        })
    }

    /// Bump the version, then notify outside `inner` (inner -> emit_lock, see module docs).
    fn publish(&self, mut g: MutexGuard<'_, Inner>) {
        g.version += 1;
        let v = g.version;
        let _emit = self.emit_lock.lock().unwrap();
        drop(g);
        (self.notify)(v);
    }

    /// Store a new item (file first, then memory) and publish.
    fn insert(&self, mut g: MutexGuard<'_, Inner>, mut item: InboxItem) -> Result<String, String> {
        let t = now();
        let base = t.format("%Y%m%d-%H%M%S").to_string();
        loop {
            let mut b = [0u8; 2];
            getrandom::fill(&mut b).map_err(|e| format!("항목 id를 만들 수 없습니다 — {e}"))?;
            let id = format!("{base}-{:02x}{:02x}", b[0], b[1]);
            if !g.items.contains_key(&id) && !self.dir.join(format!("{id}.json")).exists() {
                item.id = id;
                break;
            }
        }
        item.created = stamp(t);
        self.write(&item)?;
        let id = item.id.clone();
        g.items.insert(id.clone(), item);
        self.publish(g);
        Ok(id)
    }

    /// Change one item: `f` edits a copy and says whether anything changed; a change is written
    /// to disk, then kept in memory and published (a failed write changes nothing).
    fn update(&self, id: &str, f: impl FnOnce(&mut InboxItem) -> Result<bool, String>) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        let mut next = g.items.get(id).ok_or_else(|| not_found(id))?.clone();
        if !f(&mut next)? {
            return Ok(());
        }
        self.write(&next)?;
        g.items.insert(id.to_string(), next);
        self.publish(g);
        Ok(())
    }

    /// `inbox_post`: a Manager's new item, owned by its tab. Returns the item id (an identical
    /// retry returns the earlier id).
    pub fn post(&self, caller: &Caller, owner_name: &str, new: NewItem) -> Result<String, String> {
        if caller.role != Role::Manager {
            return Err(format!("'{}' 역할은 인박스 항목을 올릴 수 없습니다", caller.role.as_str()));
        }
        let title = non_empty(&new.title, "제목")?;
        let g = self.inner.lock().unwrap();
        let cutoff = now() - chrono::Duration::seconds(POST_RETRY_SECS);
        let retry = g.items.values().find(|i| {
            i.owner_tab == caller.tab
                && i.state != ItemState::Done
                && i.title == title
                && i.check == new.check
                && i.risk == new.risk
                && i.source == new.source
                && i.refs == new.refs
                && DateTime::parse_from_rfc3339(&i.created).map_or(false, |c| c >= cutoff)
        });
        if let Some(i) = retry {
            return Ok(i.id.clone());
        }
        let item = InboxItem {
            id: String::new(),
            owner_tab: caller.tab.clone(),
            owner_name: owner_name.to_string(),
            title,
            check: new.check,
            risk: new.risk,
            source: new.source,
            refs: new.refs,
            state: ItemState::New,
            read: false,
            issue: None,
            issue_path: None,
            created: String::new(),
            timeline: vec![entry(&caller.session_name, TimelineKind::Posted, "")],
        };
        self.insert(g, item)
    }

    /// App-generated item (e.g. nightly refresh problems) for `owner_tab`; source and author
    /// are [`WHO_APP`].
    pub fn post_system(
        &self,
        owner_tab: &str,
        owner_name: &str,
        title: &str,
        check: Vec<String>,
        risk: Vec<String>,
    ) -> Result<String, String> {
        let title = non_empty(title, "제목")?;
        let g = self.inner.lock().unwrap();
        let item = InboxItem {
            id: String::new(),
            owner_tab: owner_tab.to_string(),
            owner_name: owner_name.to_string(),
            title,
            check,
            risk,
            source: WHO_APP.to_string(),
            refs: Vec::new(),
            state: ItemState::New,
            read: false,
            issue: None,
            issue_path: None,
            created: String::new(),
            timeline: vec![entry(WHO_APP, TimelineKind::Posted, "")],
        };
        self.insert(g, item)
    }

    /// `inbox_note`: the owner Manager adds a note (state unchanged).
    pub fn note(&self, caller: &Caller, id: &str, text: &str) -> Result<(), String> {
        let text = non_empty(text, "메모")?;
        self.update(id, |it| {
            check_owner(caller, it)?;
            not_done(it)?;
            if let Some(last) = it.timeline.last() {
                if last.kind == TimelineKind::Note && last.who == caller.session_name && last.text == text {
                    return Ok(false); // retried call
                }
            }
            it.timeline.push(entry(&caller.session_name, TimelineKind::Note, &text));
            it.read = false;
            Ok(true)
        })
    }

    /// `inbox_propose_close`: the owner Manager proposes closing with a final opinion.
    pub fn propose_close(&self, caller: &Caller, id: &str, opinion: &str) -> Result<(), String> {
        let opinion = non_empty(opinion, "최종 의견")?;
        self.update(id, |it| {
            check_owner(caller, it)?;
            not_done(it)?;
            let kind = TimelineKind::CloseProposed { opinion: opinion.clone() };
            if it.state == ItemState::CloseProposed && it.timeline.last().map_or(false, |l| l.kind == kind) {
                return Ok(false); // retried call
            }
            it.state = ItemState::CloseProposed;
            it.timeline.push(entry(&caller.session_name, kind, &opinion));
            it.read = false;
            Ok(true)
        })
    }

    /// The user's instruction (the applet sends it on via the dispatcher, Task 10): recorded,
    /// state → `in_progress` (`reopened` first when it was `close_proposed`).
    pub fn instruct(&self, id: &str, text: &str) -> Result<(), String> {
        let text = non_empty(text, "지시")?;
        self.update(id, |it| {
            not_done(it)?;
            if it.state == ItemState::CloseProposed {
                it.timeline.push(entry(WHO_USER, TimelineKind::Reopened, ""));
            }
            it.state = ItemState::InProgress;
            it.timeline.push(entry(WHO_USER, TimelineKind::Instruction, &text));
            it.read = true;
            Ok(true)
        })
    }

    /// The user confirms a proposed close: `close_proposed` → `done` (the only way to `done`).
    pub fn confirm_done(&self, id: &str) -> Result<(), String> {
        self.update(id, |it| {
            if it.state != ItemState::CloseProposed {
                return Err("종료 제안된 항목만 완료할 수 있습니다".into());
            }
            it.state = ItemState::Done;
            it.timeline.push(entry(WHO_USER, TimelineKind::Done, ""));
            it.read = true;
            Ok(true)
        })
    }

    pub fn mark_read(&self, id: &str) -> Result<(), String> {
        self.update(id, |it| {
            let changed = !it.read;
            it.read = true;
            Ok(changed)
        })
    }

    /// The owner Manager links an issue folder (made by `issues::create`). Linking the same
    /// issue again is a no-op; another issue is refused.
    pub fn link_issue(&self, caller: &Caller, id: &str, issue: &str, path: &Path) -> Result<(), String> {
        self.update(id, |it| {
            check_owner(caller, it)?;
            if !issues::is_valid_id(issue) {
                return Err(format!("잘못된 이슈 id입니다: {issue}"));
            }
            match &it.issue {
                Some(cur) if cur == issue => return Ok(false),
                Some(cur) => return Err(format!("이미 다른 이슈({cur})에 연결된 항목입니다")),
                None => {}
            }
            not_done(it)?;
            it.issue = Some(issue.to_string());
            it.issue_path = Some(path.to_path_buf());
            it.timeline.push(entry(&caller.session_name, TimelineKind::IssueLinked, issue));
            it.read = false;
            Ok(true)
        })
    }

    /// The user copies files into the linked issue's `input/` or `output/`: regular files only
    /// (directories and missing paths refused before anything is copied), copies never
    /// overwrite (`name (2).ext`), the destination is checked to be the issue's own sub-folder.
    /// Returns the new files. Copies run with no lock held.
    pub fn add_files(&self, id: &str, paths: &[PathBuf], target: Target) -> Result<Vec<PathBuf>, String> {
        if paths.is_empty() {
            return Err("복사할 파일이 없습니다".into());
        }
        let item = self.get(id).ok_or_else(|| not_found(id))?;
        let (Some(issue), Some(issue_path)) = (&item.issue, &item.issue_path) else {
            return Err("이슈 폴더가 연결되지 않은 항목입니다 — 먼저 이슈를 만들어 주세요".into());
        };
        let looks_right = issue_path.file_name().map_or(false, |n| n.to_string_lossy() == issue.as_str())
            && issue_path.parent().and_then(|p| p.file_name()).map_or(false, |n| n == "issues")
            && issue_path.is_dir();
        if !looks_right {
            return Err(format!("연결된 이슈 폴더를 찾을 수 없습니다: {issue}"));
        }
        let dest = issue_path.join(target.dir_name());
        if std::fs::symlink_metadata(&dest).map_or(false, |m| m.file_type().is_symlink()) {
            return Err(format!("이슈 폴더의 {} 폴더가 링크입니다 — 복사하지 않습니다", target.dir_name()));
        }
        std::fs::create_dir_all(&dest).map_err(|e| format!("{} 폴더를 만들 수 없습니다 — {e}", target.dir_name()))?;
        let inside = match (std::fs::canonicalize(&dest), std::fs::canonicalize(issue_path)) {
            (Ok(d), Ok(i)) => d.starts_with(&i) && d != i,
            _ => false,
        };
        if !inside {
            return Err(format!("{} 폴더가 이슈 폴더 밖을 가리킵니다 — 복사하지 않습니다", target.dir_name()));
        }
        for p in paths {
            match std::fs::metadata(p) {
                Err(_) => return Err(format!("파일을 찾을 수 없습니다: {}", file_name_of(p))),
                Ok(m) if m.is_dir() => return Err(format!("폴더는 복사할 수 없습니다: {}", file_name_of(p))),
                Ok(m) if !m.is_file() => return Err(format!("일반 파일이 아닙니다: {}", file_name_of(p))),
                Ok(_) if p.file_name().is_none() => return Err("파일 이름이 없는 경로입니다".into()),
                Ok(_) => {}
            }
        }
        let mut copied = Vec::new();
        let mut failure = None;
        for p in paths {
            match copy_unique(p, &dest) {
                Ok(d) => copied.push(d),
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if !copied.is_empty() {
            let names: Vec<String> = copied.iter().map(|d| file_name_of(d)).collect();
            let text = format!("{}: {}", target.dir_name(), names.join(", "));
            self.update(id, |it| {
                it.timeline.push(entry(WHO_USER, TimelineKind::FilesAdded, &text));
                Ok(true)
            })?;
        }
        match failure {
            Some(e) if copied.is_empty() => Err(e),
            Some(e) => Err(format!("{e} (앞의 {}개는 복사됨)", copied.len())),
            None => Ok(copied),
        }
    }

    pub fn get(&self, id: &str) -> Option<InboxItem> {
        self.inner.lock().unwrap().items.get(id).cloned()
    }

    /// Every item (done ones only with `show_done`), newest first, plus the current version.
    pub fn list(&self, show_done: bool) -> InboxList {
        let g = self.inner.lock().unwrap();
        let items = g.items.values().rev().filter(|i| show_done || i.state != ItemState::Done).cloned().collect();
        InboxList { version: g.version, items }
    }

    /// `inbox_list` for an agent: Dispatcher all items, a Manager its tab's, others none.
    /// `state` filters; without it, done items are left out. Newest first.
    pub fn list_for(&self, caller: &Caller, state: Option<ItemState>) -> Vec<InboxItem> {
        let mine = |i: &InboxItem| match caller.role {
            Role::Dispatcher => true,
            Role::Manager => i.owner_tab == caller.tab,
            _ => false,
        };
        let g = self.inner.lock().unwrap();
        g.items
            .values()
            .rev()
            .filter(|i| mine(i))
            .filter(|i| match state {
                Some(s) => i.state == s,
                None => i.state != ItemState::Done,
            })
            .cloned()
            .collect()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Caller;
    use crate::roles::Role;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    fn caller(role: Role, tab: &str, name: &str) -> Caller {
        Caller { pane: format!("p-{name}"), role, tab: tab.into(), team: None, session_name: name.into() }
    }
    fn mgr() -> Caller {
        caller(Role::Manager, "t-billing", "billing")
    }
    fn new_item(title: &str) -> NewItem {
        NewItem {
            title: title.into(),
            check: vec!["금액 확인".into()],
            risk: vec!["중복 결제".into()],
            source: "slack #billing".into(),
            refs: vec![],
        }
    }
    struct Fx {
        inbox: Inbox,
        dir: tempfile::TempDir,
        last: Arc<AtomicU64>,
    }
    fn fx() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let last = Arc::new(AtomicU64::new(0));
        let l = last.clone();
        let (inbox, errs) = Inbox::load(dir.path().join("inbox"), Arc::new(move |v| l.store(v, Ordering::SeqCst)));
        assert!(errs.is_empty(), "{errs:?}");
        Fx { inbox, dir, last }
    }

    #[test]
    fn state_machine_user_only_done() {
        let f = fx();
        let m = mgr();
        let id = f.inbox.post(&m, "billing", new_item("결제 실패")).unwrap();
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.state, ItemState::New);
        assert!(!it.read);
        assert_eq!(it.owner_tab, "t-billing");
        assert_eq!(it.timeline[0].kind, TimelineKind::Posted);
        assert!(f.last.load(Ordering::SeqCst) >= 1, "post emits inbox-changed");
        // id format: YYYYMMDD-HHMMSS-xxxx
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!((parts.len(), parts[0].len(), parts[1].len(), parts[2].len()), (3, 8, 6, 4), "{id}");

        // done only from close_proposed, and only through the user path
        assert!(f.inbox.confirm_done(&id).unwrap_err().contains("종료 제안"));
        f.inbox.instruct(&id, "금액 다시 확인").unwrap();
        assert_eq!(f.inbox.get(&id).unwrap().state, ItemState::InProgress);
        assert!(f.inbox.confirm_done(&id).is_err());
        f.inbox.propose_close(&m, &id, "환불 완료, 종료해도 됨").unwrap();
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.state, ItemState::CloseProposed, "a Manager can only propose");
        assert!(!it.read, "agent changes mark the item unread");
        assert!(matches!(&it.timeline.last().unwrap().kind, TimelineKind::CloseProposed { opinion } if opinion.contains("환불")));
        f.inbox.mark_read(&id).unwrap();
        assert!(f.inbox.get(&id).unwrap().read);
        let v_before = f.last.load(Ordering::SeqCst);
        f.inbox.confirm_done(&id).unwrap();
        assert!(f.last.load(Ordering::SeqCst) > v_before, "version increases on every mutation");
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.state, ItemState::Done);
        assert_eq!(it.timeline.last().unwrap().kind, TimelineKind::Done);
        assert_eq!(it.timeline.last().unwrap().who, WHO_USER);
        // done is final for agents and instructions
        assert!(f.inbox.propose_close(&m, &id, "x").is_err());
        assert!(f.inbox.note(&m, &id, "x").is_err());
        assert!(f.inbox.instruct(&id, "x").is_err());
        assert!(f.inbox.confirm_done(&id).is_err());
        // list hides done unless asked
        assert!(f.inbox.list(false).items.is_empty());
        assert_eq!(f.inbox.list(true).items.len(), 1);
        // persisted: a fresh load sees the same item
        let (again, errs) = Inbox::load(f.dir.path().join("inbox"), Arc::new(|_| {}));
        assert!(errs.is_empty());
        assert_eq!(again.get(&id).unwrap(), it);
    }

    #[test]
    fn reinstruct_from_close_proposed_returns_in_progress() {
        let f = fx();
        let m = mgr();
        let id = f.inbox.post(&m, "billing", new_item("x")).unwrap();
        f.inbox.propose_close(&m, &id, "끝").unwrap();
        f.inbox.instruct(&id, "하나 더 확인해 줘").unwrap();
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.state, ItemState::InProgress);
        let kinds: Vec<&TimelineKind> = it.timeline.iter().map(|e| &e.kind).collect();
        assert_eq!(kinds.len(), 4, "{kinds:?}");
        assert_eq!(*kinds[2], TimelineKind::Reopened);
        assert_eq!(*kinds[3], TimelineKind::Instruction);
        assert_eq!(it.timeline[3].text, "하나 더 확인해 줘");
        assert_eq!(it.timeline[3].who, WHO_USER);
        // proposing again works and the cycle can repeat
        f.inbox.propose_close(&m, &id, "이제 끝").unwrap();
        f.inbox.confirm_done(&id).unwrap();
        assert!(f.inbox.instruct(&id, "x").is_err());
        assert!(f.inbox.instruct("nope", "x").is_err());
    }

    #[test]
    fn concurrent_notes_are_all_kept() {
        let f = fx();
        let m = mgr();
        let id = f.inbox.post(&m, "billing", new_item("x")).unwrap();
        std::thread::scope(|s| {
            for t in 0..8 {
                let (inbox, m, id) = (&f.inbox, &m, &id);
                s.spawn(move || {
                    for i in 0..10 {
                        inbox.note(m, id, &format!("메모 {t}-{i}")).unwrap();
                    }
                });
            }
        });
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.timeline.iter().filter(|e| e.kind == TimelineKind::Note).count(), 80);
        // and on disk
        let (again, _) = Inbox::load(f.dir.path().join("inbox"), Arc::new(|_| {}));
        assert_eq!(again.get(&id).unwrap().timeline.len(), 81);
        // a retried (timed-out) identical note is not recorded twice
        f.inbox.note(&m, &id, "같은 메모").unwrap();
        f.inbox.note(&m, &id, "같은 메모").unwrap();
        assert_eq!(f.inbox.get(&id).unwrap().timeline.len(), 82);
        // a retried identical post returns the same item
        let id2 = f.inbox.post(&m, "billing", new_item("x")).unwrap();
        assert_eq!(id2, id);
    }

    #[test]
    fn add_files_never_overwrites_or_escapes() {
        let f = fx();
        let m = mgr();
        let id = f.inbox.post(&m, "billing", new_item("x")).unwrap();
        let src = f.dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.md"), "one").unwrap();
        std::fs::write(src.join("noext"), "n").unwrap();
        // no issue yet → refused
        let err = f.inbox.add_files(&id, &[src.join("a.md")], Target::Input).unwrap_err();
        assert!(err.contains("이슈"), "{err}");

        let ws = f.dir.path().join("billing");
        std::fs::create_dir(&ws).unwrap();
        let (iid, idir) = crate::issues::create(&ws, "결제", chrono::NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()).unwrap();
        f.inbox.link_issue(&m, &id, &iid, &idir).unwrap();
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.issue.as_deref(), Some(iid.as_str()));
        assert_eq!(it.timeline.last().unwrap().kind, TimelineKind::IssueLinked);
        // relinking to the same issue is a no-op, to another is refused
        f.inbox.link_issue(&m, &id, &iid, &idir).unwrap();
        assert!(f.inbox.link_issue(&m, &id, "20261025-other", &ws.join("issues/20261025-other")).is_err());

        let out = f.inbox.add_files(&id, &[src.join("a.md"), src.join("noext")], Target::Input).unwrap();
        assert_eq!(out, vec![idir.join("input").join("a.md"), idir.join("input").join("noext")]);
        std::fs::write(src.join("a.md"), "two").unwrap();
        let out = f.inbox.add_files(&id, &[src.join("a.md"), src.join("a.md"), src.join("noext")], Target::Input).unwrap();
        assert_eq!(
            out,
            vec![idir.join("input/a (2).md"), idir.join("input/a (3).md"), idir.join("input/noext (2)")]
        );
        assert_eq!(std::fs::read_to_string(idir.join("input/a.md")).unwrap(), "one", "never overwritten");
        assert_eq!(std::fs::read_to_string(idir.join("input/a (2).md")).unwrap(), "two");
        assert!(src.join("a.md").exists(), "copied, not moved");
        let out = f.inbox.add_files(&id, &[src.join("a.md")], Target::Output).unwrap();
        assert_eq!(out, vec![idir.join("output/a.md")]);
        let it = f.inbox.get(&id).unwrap();
        assert_eq!(it.timeline.iter().filter(|e| e.kind == TimelineKind::FilesAdded).count(), 3);

        // directories and missing sources are refused (Korean), nothing copied
        let before = std::fs::read_dir(idir.join("input")).unwrap().count();
        let err = f.inbox.add_files(&id, &[src.join("a.md"), src.join("sub")], Target::Input).unwrap_err();
        assert!(err.contains("폴더"), "{err}");
        let err = f.inbox.add_files(&id, &[src.join("missing.txt")], Target::Input).unwrap_err();
        assert!(err.contains("missing.txt"), "{err}");
        assert!(!err.contains(&*src.to_string_lossy()), "file name only: {err}");
        assert!(f.inbox.add_files(&id, &[], Target::Input).is_err());
        assert_eq!(std::fs::read_dir(idir.join("input")).unwrap().count(), before);
        // a stored link that does not look like an issue folder is not used
        {
            let mut g = f.inbox.inner.lock().unwrap();
            g.items.get_mut(&id).unwrap().issue_path = Some(f.dir.path().to_path_buf());
        }
        assert!(f.inbox.add_files(&id, &[src.join("a.md")], Target::Input).is_err());
        // nothing was ever written outside the issue folder
        assert!(!f.dir.path().join("input").exists() && !ws.join("input").exists());
    }

    #[test]
    fn only_owner_manager_can_note_or_propose_close() {
        let f = fx();
        let m = mgr();
        let other = caller(Role::Manager, "t-api", "api");
        let disp = caller(Role::Dispatcher, "t-disp", "dispatcher");
        let lead = caller(Role::Lead, "t-billing", "billing-batch-lead");
        let id = f.inbox.post(&m, "billing", new_item("x")).unwrap();
        for c in [&other, &disp, &lead] {
            assert!(f.inbox.note(c, &id, "x").is_err(), "{c:?}");
            assert!(f.inbox.propose_close(c, &id, "x").is_err(), "{c:?}");
            assert!(f.inbox.link_issue(c, &id, "20261025-x", std::path::Path::new("x")).is_err(), "{c:?}");
        }
        assert!(f.inbox.post(&disp, "dispatcher", new_item("y")).is_err(), "only a Manager posts");
        let err = f.inbox.note(&other, &id, "x").unwrap_err();
        assert!(err.contains("담당"), "{err}");
        let it = f.inbox.get(&id).unwrap();
        assert_eq!((it.timeline.len(), it.state), (1, ItemState::New));
        f.inbox.note(&m, &id, "확인 중").unwrap();
        assert_eq!(f.inbox.get(&id).unwrap().timeline[1].who, "billing");
        // visibility: dispatcher all, manager own, others nothing
        f.inbox.post(&other, "api", new_item("z")).unwrap();
        assert_eq!(f.inbox.list_for(&disp, None).len(), 2);
        assert_eq!(f.inbox.list_for(&m, None).len(), 1);
        assert_eq!(f.inbox.list_for(&other, None).len(), 1);
        assert!(f.inbox.list_for(&lead, None).is_empty());
        assert_eq!(f.inbox.list_for(&disp, Some(ItemState::New)).len(), 2);
        assert!(f.inbox.list_for(&disp, Some(ItemState::Done)).is_empty());
    }

    #[test]
    fn corrupt_item_file_is_skipped_with_error() {
        let f = fx();
        let id = f.inbox.post(&mgr(), "billing", new_item("x")).unwrap();
        let dir = f.dir.path().join("inbox");
        std::fs::write(dir.join("20261025-000000-dead.json"), "{ not json").unwrap();
        std::fs::write(dir.join("20261025-000001-beef.json"), r#"{"id":"other"}"#).unwrap();
        std::fs::write(dir.join("leftover.json.tmp"), "x").unwrap();
        std::fs::write(dir.join("readme.txt"), "x").unwrap();
        let (again, errs) = Inbox::load(dir.clone(), Arc::new(|_| {}));
        assert_eq!(again.list(true).items.len(), 1);
        assert!(again.get(&id).is_some());
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("20261025-000000-dead.json")));
        assert!(errs.iter().all(|e| e.contains("인박스") && !e.contains(&*dir.to_string_lossy())), "{errs:?}");
        // the corrupt file is left alone
        assert!(dir.join("20261025-000000-dead.json").exists());
        // a missing directory is simply empty
        let (empty, errs) = Inbox::load(f.dir.path().join("none"), Arc::new(|_| {}));
        assert!(errs.is_empty() && empty.list(true).items.is_empty());
    }

    #[test]
    fn post_system_and_list_order() {
        let f = fx();
        let id = f.inbox.post_system("t-billing", "billing", "야간 정리 시간 초과", vec!["확인".into()], vec![]).unwrap();
        let it = f.inbox.get(&id).unwrap();
        assert_eq!((it.source.as_str(), it.timeline[0].who.as_str()), (WHO_APP, WHO_APP));
        let id2 = f.inbox.post(&mgr(), "billing", new_item("x")).unwrap();
        let l = f.inbox.list(false);
        assert_eq!(l.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().len(), 2);
        assert!(l.items[0].id >= l.items[1].id, "newest first");
        assert!(l.items.iter().any(|i| i.id == id2));
        assert_eq!(l.version, f.last.load(Ordering::SeqCst));
        assert!(f.inbox.mark_read("nope").is_err());
    }
}
