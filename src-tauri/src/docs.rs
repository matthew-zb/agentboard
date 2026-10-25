//! Document tabs (Task 14, spec §5-2) — backend side: path validation, reading, listing,
//! issue-folder file lists and the mtime polling watcher.
//!
//! **Paths are validated on every use.** A document must be an existing `.md` file inside
//! `workspaceRoot`: the path is canonicalized (every symlink / junction resolved) and the result
//! must lie strictly inside the canonical root. A path stored in workspace.json or sent by the
//! frontend is never trusted — `doc_read` / `doc_watch` run [`resolve_doc`] again each time.
//! Agents have no way to open a document tab (no MCP tool reaches `OpenDoc`).

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// `PaneKind::Applet { kind }` of a document pane.
pub const DOC_APPLET: &str = "doc";
/// Most files `doc_list_md` returns.
pub const LIST_MAX: usize = 5000;
/// Directory names `doc_list_md` never descends into.
pub const LIST_SKIP: [&str; 4] = [".agentboard", "node_modules", ".git", "target"];
/// Largest document `doc_read` returns (bytes).
pub const DOC_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Watcher period.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Event emitted when a watched document changed: `{"paneId": "p7"}`.
pub const DOC_CHANGED: &str = "doc-changed";
/// Longest external URL handed to the system browser.
const URL_MAX: usize = 2048;
/// Most files listed per issue sub-folder.
const ISSUE_FILES_MAX: usize = 500;

/// A validated document: `abs` = `<workspaceRoot>/<rel>` (not canonical, as the user sees it),
/// `rel` = path under the root with `/` separators and the on-disk spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocPath {
    pub abs: PathBuf,
    pub rel: String,
}

impl DocPath {
    /// File name (the tab name).
    pub fn name(&self) -> String {
        self.rel.rsplit('/').next().unwrap_or(&self.rel).to_string()
    }
}

fn is_md(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Validate `path` (absolute, or relative to `root`; `/` or `\`) as a document: an existing
/// `.md` file whose canonical path is strictly inside the canonical `root`. Korean errors.
pub fn resolve_doc(root: &Path, path: &str) -> Result<DocPath, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("문서 경로가 비어 있습니다".into());
    }
    if path.chars().any(char::is_control) {
        return Err("문서 경로에 제어 문자가 있습니다".into());
    }
    let p = Path::new(path);
    let joined = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let root_real = std::fs::canonicalize(root).map_err(|_| "workspaceRoot 폴더를 찾을 수 없습니다".to_string())?;
    let real = std::fs::canonicalize(&joined).map_err(|_| format!("파일을 찾을 수 없습니다: {path}"))?;
    // checked after every link is resolved: a junction inside the root pointing out is refused
    let rel = match real.strip_prefix(&root_real) {
        Ok(r) if !r.as_os_str().is_empty() && r.components().all(|c| matches!(c, Component::Normal(_))) => r.to_path_buf(),
        _ => return Err(format!("workspaceRoot 밖의 파일은 열 수 없습니다: {path}")),
    };
    if !real.is_file() {
        return Err(format!("문서 파일이 아닙니다: {path}"));
    }
    if !is_md(&joined) || !is_md(&real) {
        return Err(format!("문서 탭은 .md 파일만 열 수 있습니다: {path}"));
    }
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Ok(DocPath { abs: root.join(&rel), rel: parts.join("/") })
}

/// `doc_read` result. `mtime` = milliseconds since the Unix epoch (0 if unknown).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocText {
    pub text: String,
    pub mtime: u64,
    /// Path under workspaceRoot (`/` separators): links and images resolve against it.
    pub rel: String,
    /// Absolute path (display).
    pub path: String,
    /// workspaceRoot as given in settings (images: `root` + `rel` → asset URL).
    pub root: String,
}

fn millis(t: Option<SystemTime>) -> u64 {
    t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// Validate (every call) and read a document. Invalid UTF-8 is replaced, a BOM dropped.
pub fn read_doc(root: &Path, path: &str) -> Result<DocText, String> {
    let d = resolve_doc(root, path)?;
    let meta = std::fs::metadata(&d.abs).map_err(|_| format!("파일을 찾을 수 없습니다: {}", d.rel))?;
    if meta.len() > DOC_MAX_BYTES {
        return Err(format!("문서가 너무 큽니다({} MB 초과): {}", DOC_MAX_BYTES / 1024 / 1024, d.rel));
    }
    let bytes = std::fs::read(&d.abs).map_err(|e| format!("문서를 읽을 수 없습니다: {} — {e}", d.rel))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
    Ok(DocText {
        text,
        mtime: millis(meta.modified().ok()),
        rel: d.rel,
        path: d.abs.to_string_lossy().into_owned(),
        root: root.to_string_lossy().into_owned(),
    })
}

/// `.md` files under `root` (relative, `/` separators, sorted; walked depth-first in name
/// order, so a capped list keeps the first folders), skipping [`LIST_SKIP`] folders and links (never followed: no
/// loops, nothing outside). `query` = whitespace-separated words that must all appear
/// (case-insensitive). At most `cap` entries.
pub fn list_md(root: &Path, query: Option<&str>, cap: usize) -> Vec<String> {
    let words: Vec<String> = query.unwrap_or("").split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_symlink() {
                continue; // junctions and symlinks: not followed
            }
            if ft.is_dir() {
                if !LIST_SKIP.contains(&name.as_str()) {
                    dirs.push(name);
                }
            } else if ft.is_file() && is_md(Path::new(&name)) {
                files.push(name);
            }
        }
        files.sort();
        dirs.sort();
        for f in files {
            let rel = format!("{prefix}{f}");
            let low = rel.to_lowercase();
            if words.iter().all(|w| low.contains(w.as_str())) {
                out.push(rel);
                if out.len() >= cap {
                    out.sort();
                    return out;
                }
            }
        }
        for d in dirs.into_iter().rev() {
            stack.push((dir.join(&d), format!("{prefix}{d}/")));
        }
    }
    out.sort();
    out
}

/// An http(s) URL for the system browser, or a Korean error (everything else is refused).
pub fn check_external_url(url: &str) -> Result<String, String> {
    let bad = || Err("http/https 주소만 브라우저로 열 수 있습니다".to_string());
    if url.len() > URL_MAX || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad();
    }
    let lower = url.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")) else {
        return bad();
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.starts_with('@') || host.contains('\\') {
        return bad();
    }
    Ok(url.to_string())
}

/// One file of an issue folder (`name` relative to input/ or output/, `/` separators).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueFile {
    pub name: String,
    pub path: PathBuf,
    pub is_md: bool,
}

/// `inbox_issue_files` result.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct IssueFiles {
    pub input: Vec<IssueFile>,
    pub output: Vec<IssueFile>,
}

/// Files under `<issue>/input` and `<issue>/output` (recursive, links skipped, sorted by name,
/// at most 500 each). A sub-folder that is itself a link, or does not resolve to a direct child of
/// the issue folder, lists nothing. `issue_dir` comes from [`crate::issues::resolve`].
pub fn issue_files(issue_dir: &Path) -> Result<IssueFiles, String> {
    let issue_real = std::fs::canonicalize(issue_dir).map_err(|_| "이슈 폴더를 찾을 수 없습니다".to_string())?;
    let list = |sub: &str| -> Vec<IssueFile> {
        let dir = issue_dir.join(sub);
        let linked = std::fs::symlink_metadata(&dir).map_or(true, |m| m.file_type().is_symlink());
        let inside = std::fs::canonicalize(&dir).is_ok_and(|r| r.parent() == Some(issue_real.as_path()));
        if linked || !inside {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut stack = vec![(dir, String::new())];
        while let Some((d, prefix)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                let name = e.file_name().to_string_lossy().into_owned();
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    stack.push((d.join(&name), format!("{prefix}{name}/")));
                } else if ft.is_file() && out.len() < ISSUE_FILES_MAX {
                    let p = d.join(&name);
                    out.push(IssueFile { name: format!("{prefix}{name}"), is_md: is_md(&p), path: p });
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    };
    Ok(IssueFiles { input: list("input"), output: list("output") })
}

// ───────── watcher ─────────

/// Watched documents per pane (`AppState::docs`). Its lock is a leaf: nothing is acquired while
/// holding it, and file system calls happen outside it.
pub struct DocWatch {
    map: Mutex<BTreeMap<String, (PathBuf, Option<SystemTime>)>>,
}

impl Default for DocWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl DocWatch {
    pub fn new() -> Self {
        DocWatch { map: Mutex::new(BTreeMap::new()) }
    }

    /// Watch `path` for `pane` (replacing what the pane watched), starting from `mtime`.
    pub fn watch(&self, pane: &str, path: PathBuf, mtime: Option<SystemTime>) {
        self.map.lock().unwrap().insert(pane.to_string(), (path, mtime));
    }

    pub fn unwatch(&self, pane: &str) {
        self.map.lock().unwrap().remove(pane);
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One polling round: stat every watched file with no lock held and return the panes whose
    /// mtime changed (a file appearing or disappearing counts), each once per change. A pane
    /// re-watched or unwatched during the round keeps its newer entry and is not reported.
    pub fn poll(&self, stat: impl Fn(&Path) -> Option<SystemTime>) -> Vec<String> {
        let list: Vec<(String, PathBuf, Option<SystemTime>)> =
            self.map.lock().unwrap().iter().map(|(k, (p, t))| (k.clone(), p.clone(), *t)).collect();
        let seen: Vec<(String, PathBuf, Option<SystemTime>)> = list
            .into_iter()
            .filter_map(|(k, p, old)| {
                let now = stat(&p);
                (now != old).then_some((k, p, now))
            })
            .collect();
        let mut changed = Vec::new();
        let mut map = self.map.lock().unwrap();
        for (k, p, now) in seen {
            if let Some(e) = map.get_mut(&k) {
                if e.0 == p {
                    e.1 = now;
                    changed.push(k);
                }
            }
        }
        changed
    }
}

/// Current modification time of a file (None when missing).
pub fn mtime_of(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn resolve_and_read_validate_every_call() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        write(&root.join("billing/issues/x/output/보고서.md"), "\u{feff}# 결과\n");
        write(&root.join("a.txt"), "x");
        write(&root.join("dir.md/inner.md"), "x");
        write(&outside.join("secret.md"), "s");

        // relative (forward or back slashes) and absolute spellings resolve to the same doc
        let d = resolve_doc(&root, "billing/issues/x/output/보고서.md").unwrap();
        assert_eq!(d.rel, "billing/issues/x/output/보고서.md");
        assert_eq!(d.abs, root.join("billing").join("issues").join("x").join("output").join("보고서.md"));
        assert_eq!(d.name(), "보고서.md");
        let d2 = resolve_doc(&root, &root.join("billing\\issues\\x\\..\\x\\output\\보고서.md").to_string_lossy()).unwrap();
        assert_eq!(d2, d);
        assert_eq!(resolve_doc(&root, "./billing/issues/x/output/보고서.md").unwrap(), d);

        let r = read_doc(&root, "billing/issues/x/output/보고서.md").unwrap();
        assert_eq!(r.text, "# 결과\n", "BOM stripped");
        assert_eq!(r.rel, d.rel);
        assert!(r.mtime > 0);

        for (bad, why) in [
            ("../outside/secret.md", "밖"),
            ("a.txt", ".md"),
            ("dir.md", "파일"),
            ("nope.md", "찾을 수 없습니다"),
            ("", "비어"),
        ] {
            let e = resolve_doc(&root, bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
            assert!(read_doc(&root, bad).is_err());
        }
        let e = resolve_doc(&root, &outside.join("secret.md").to_string_lossy()).unwrap_err();
        assert!(e.contains("밖"), "{e}");
        // the root itself is not a document; a missing root is an error
        assert!(resolve_doc(&root, &root.to_string_lossy()).is_err());
        assert!(resolve_doc(&tmp.path().join("none"), "a.md").is_err());

        // a junction inside the root that points outside is refused (re-checked after resolving)
        if crate::issues::tests::link_dir(&root.join("link"), &outside) {
            let e = resolve_doc(&root, "link/secret.md").unwrap_err();
            assert!(e.contains("밖"), "{e}");
            assert!(read_doc(&root, "link/secret.md").is_err());
        } else {
            eprintln!("junction/symlink creation refused by the OS — skipping the junction part");
        }

        // oversized files are refused instead of freezing the viewer
        write(&root.join("big.md"), &"x".repeat(DOC_MAX_BYTES as usize + 1));
        assert!(read_doc(&root, "big.md").unwrap_err().contains("큽니다"));
    }

    #[test]
    fn list_md_skips_excluded_dirs_and_caps() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in [
            "HANDOVER.md",
            "billing/issues/x/output/보고서.md",
            "billing/notes.MD",
            "api/README.md",
            ".agentboard/roles/manager.md",
            "node_modules/pkg/README.md",
            ".git/info.md",
            "api/target/doc.md",
            "api/node_modules/x.md",
            "billing/a.txt",
            "billing/b.markdown",
        ] {
            write(&root.join(f), "x");
        }
        let all = list_md(root, None, LIST_MAX);
        assert_eq!(all, ["HANDOVER.md", "api/README.md", "billing/issues/x/output/보고서.md", "billing/notes.MD"]);
        // query: case-insensitive, every whitespace-separated word must appear
        assert_eq!(list_md(root, Some("readme"), LIST_MAX), ["api/README.md"]);
        assert_eq!(list_md(root, Some("billing 보고"), LIST_MAX), ["billing/issues/x/output/보고서.md"]);
        assert_eq!(list_md(root, Some("   "), LIST_MAX).len(), 4);
        // capped
        for i in 0..30 {
            write(&root.join(format!("many/{i:02}.md")), "x");
        }
        assert_eq!(list_md(root, None, 10).len(), 10);
        assert_eq!(LIST_MAX, 5000);
        // a junction is not followed (no loops, nothing outside the root)
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("out.md"), "x");
        if crate::issues::tests::link_dir(&root.join("lnk"), outside.path()) {
            assert!(list_md(root, Some("lnk"), LIST_MAX).is_empty());
        }
        assert!(list_md(&root.join("missing"), None, LIST_MAX).is_empty());
    }

    #[test]
    fn watcher_reports_mtime_changes_once() {
        let w = DocWatch::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let t1 = t0 + Duration::from_secs(1);
        w.watch("p1", PathBuf::from("a.md"), Some(t0));
        w.watch("p2", PathBuf::from("b.md"), Some(t0));
        fn stat<'a>(m: &'a std::collections::HashMap<&'static str, Option<SystemTime>>) -> impl Fn(&Path) -> Option<SystemTime> + 'a {
            move |p: &Path| m.get(p.to_str().unwrap()).copied().flatten()
        }
        let mut fs = std::collections::HashMap::from([("a.md", Some(t0)), ("b.md", Some(t0))]);
        assert!(w.poll(stat(&fs)).is_empty());
        fs.insert("a.md", Some(t1));
        assert_eq!(w.poll(stat(&fs)), ["p1"]);
        assert!(w.poll(stat(&fs)).is_empty(), "reported once");
        // deleted, then re-created: both are changes
        fs.insert("b.md", None);
        assert_eq!(w.poll(stat(&fs)), ["p2"]);
        fs.insert("b.md", Some(t1));
        assert_eq!(w.poll(stat(&fs)), ["p2"]);
        // re-watching a pane replaces its path; unwatch stops reports
        w.watch("p1", PathBuf::from("b.md"), Some(t1));
        assert_eq!(w.len(), 2);
        w.unwatch("p2");
        fs.insert("b.md", Some(t1 + Duration::from_secs(5)));
        assert_eq!(w.poll(stat(&fs)), ["p1"]);
        w.unwatch("p1");
        assert_eq!(w.len(), 0);
        assert_eq!(POLL_INTERVAL, Duration::from_secs(1));
    }

    #[test]
    fn external_urls_are_http_only() {
        assert_eq!(check_external_url("https://example.com/a?b=1").unwrap(), "https://example.com/a?b=1");
        assert!(check_external_url("HTTP://example.com").is_ok());
        for bad in [
            "javascript:alert(1)",
            "file:///C:/Windows/notepad.exe",
            "C:\\Windows\\notepad.exe",
            "ms-settings:privacy",
            "https://",
            "http://exa mple.com",
            "https://example.com/\u{0}",
            "",
        ] {
            assert!(check_external_url(bad).is_err(), "{bad}");
        }
        assert!(check_external_url(&format!("https://e.com/{}", "a".repeat(3000))).is_err());
    }

    #[test]
    fn issue_files_list_input_and_output() {
        let tmp = tempfile::tempdir().unwrap();
        let issue = tmp.path().join("issues").join("20261025-x");
        write(&issue.join("input/메일.md"), "x");
        write(&issue.join("input/첨부.pdf"), "x");
        write(&issue.join("output/report.md"), "x");
        write(&issue.join("output/sub/b.md"), "x");
        let f = issue_files(&issue).unwrap();
        let names = |v: &Vec<IssueFile>| v.iter().map(|f| (f.name.clone(), f.is_md)).collect::<Vec<_>>();
        assert_eq!(names(&f.input), [("메일.md".to_string(), true), ("첨부.pdf".to_string(), false)]);
        assert_eq!(names(&f.output), [("report.md".to_string(), true), ("sub/b.md".to_string(), true)]);
        assert_eq!(f.output[1].path, issue.join("output").join("sub").join("b.md"));
        // a junction at output/ pointing elsewhere is not listed
        let outside = tmp.path().join("outside");
        write(&outside.join("o.md"), "x");
        let issue2 = tmp.path().join("issues").join("20261025-y");
        std::fs::create_dir_all(issue2.join("input")).unwrap();
        if crate::issues::tests::link_dir(&issue2.join("output"), &outside) {
            let f = issue_files(&issue2).unwrap();
            assert!(f.output.is_empty());
        }
    }
}
