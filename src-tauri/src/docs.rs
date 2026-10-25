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
/// Most directory entries one listing walk looks at (`doc_list_md`, `issue_files`).
pub const WALK_BUDGET: usize = 50_000;
/// Longest a listing walk may take.
const WALK_TIME: Duration = Duration::from_secs(3);
/// Largest image `doc_image` returns (bytes).
pub const IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Image extensions `doc_image` serves, with their MIME types.
const IMAGE_TYPES: [(&str, &str); 8] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("svg", "image/svg+xml"),
];
/// Top-level folders of workspaceRoot whose files are never served as images.
const IMAGE_DENY_TOP: [&str; 2] = [".agentboard", ".git"];

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
    list_md_budget(root, query, cap, WALK_BUDGET)
}

/// [`list_md`] that stops after looking at `budget` directory entries (or [`WALK_TIME`]).
pub fn list_md_budget(root: &Path, query: Option<&str>, cap: usize, budget: usize) -> Vec<String> {
    let words: Vec<String> = query.unwrap_or("").split_whitespace().map(str::to_lowercase).collect();
    let started = std::time::Instant::now();
    let mut seen = 0usize;
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    let mut exhausted = false;
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            seen += 1;
            if seen > budget || started.elapsed() > WALK_TIME {
                exhausted = true;
                break;
            }
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_symlink() {
                continue; // junctions and symlinks: not followed
            }
            if ft.is_dir() {
                if !LIST_SKIP.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
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
        if exhausted {
            break;
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
        // depth-first in name order (files of a folder, then its sub-folders); stops at the
        // cap or the walk budget, so the kept files are always the first ones in that order
        let started = std::time::Instant::now();
        let mut seen = 0usize;
        let mut out = Vec::new();
        let mut stack = vec![(dir, String::new())];
        while let Some((d, prefix)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut ents: Vec<(String, bool)> = Vec::new();
            for e in rd.flatten() {
                seen += 1;
                if seen > WALK_BUDGET || started.elapsed() > WALK_TIME {
                    break;
                }
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_symlink() || !(ft.is_dir() || ft.is_file()) {
                    continue;
                }
                ents.push((e.file_name().to_string_lossy().into_owned(), ft.is_dir()));
            }
            ents.sort();
            let mut subdirs = Vec::new();
            for (name, is_dir) in ents {
                if is_dir {
                    subdirs.push(name);
                } else {
                    if out.len() >= ISSUE_FILES_MAX {
                        return out;
                    }
                    let p = d.join(&name);
                    out.push(IssueFile { name: format!("{prefix}{name}"), is_md: is_md(&p), path: p });
                }
            }
            if seen > WALK_BUDGET || started.elapsed() > WALK_TIME {
                break;
            }
            for sd in subdirs.into_iter().rev() {
                stack.push((d.join(&sd), format!("{prefix}{sd}/")));
            }
        }
        out
    };
    Ok(IssueFiles { input: list("input"), output: list("output") })
}

// ───────── watch path for a document that may not exist (yet) ─────────

/// The path to watch for a document pane: [`resolve_doc`] when the file exists; otherwise a
/// missing `.md` whose parent folder exists and resolves inside the root (so a document
/// deleted before the app started is picked up when it is created again).
pub fn resolve_watch_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    match resolve_doc(root, path) {
        Ok(d) => Ok(d.abs),
        Err(e) => {
            let p = Path::new(path.trim());
            let joined = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
            if joined.exists() || !is_md(&joined) || path.chars().any(char::is_control) {
                return Err(e);
            }
            let (Some(parent), Some(name)) = (joined.parent(), joined.file_name()) else { return Err(e) };
            let root_real = std::fs::canonicalize(root).map_err(|_| e.clone())?;
            let parent_real = std::fs::canonicalize(parent).map_err(|_| e.clone())?;
            match parent_real.strip_prefix(&root_real) {
                Ok(r) if r.components().all(|c| matches!(c, Component::Normal(_))) => {
                    Ok(root.join(r).join(name))
                }
                _ => Err(e),
            }
        }
    }
}

// ───────── document images (no asset protocol: validated, read, returned as data: URL) ─────────

fn image_err(src: &str) -> String {
    format!("이미지 경로를 쓸 수 없습니다: {}", src.chars().take(200).collect::<String>())
}

/// `%XX` decoding (once); None on a malformed escape or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3)?;
            let v = u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()?;
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn reserved_device(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit())
}

fn image_mime(p: &Path) -> Option<&'static str> {
    let ext = p.extension()?.to_str()?;
    IMAGE_TYPES.iter().find(|(e, _)| e.eq_ignore_ascii_case(ext)).map(|(_, m)| *m)
}

/// Lexical check of an `<img src>` written in a document, **before any file system call**:
/// percent-decoded once, `?query` / `#fragment` dropped, `\` treated as `/`. Refused: empty,
/// control characters, anything absolute or rooted (`/x`, `\x`, UNC `\\host` / `//host`,
/// device paths `\\?\` `\\.\`), any `:` (drive letters, drive-relative `C:x`, URL schemes,
/// NTFS streams), segments ending in `.` or space, reserved device names, extensions other
/// than images. Returns the relative path (`/` separators; `..` kept for [`resolve_image`]).
pub fn check_image_src(src: &str) -> Result<String, String> {
    let raw = src.trim();
    if raw.is_empty() || raw.len() > 2048 {
        return Err(image_err(src));
    }
    let raw = raw.split(['?', '#']).next().unwrap_or("");
    let d = percent_decode(raw).ok_or_else(|| image_err(src))?;
    if d.is_empty() || d.chars().any(|c| c.is_control()) {
        return Err(image_err(src));
    }
    let d = d.replace('\\', "/");
    if d.starts_with('/') || d.contains(':') {
        return Err(image_err(src));
    }
    let mut segs = Vec::new();
    for seg in d.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg != ".." && (seg.ends_with('.') || seg.ends_with(' ') || reserved_device(seg)) {
            return Err(image_err(src));
        }
        segs.push(seg);
    }
    let last = segs.last().copied().unwrap_or("");
    if last == ".." || image_mime(Path::new(last)).is_none() {
        return Err(image_err(src));
    }
    Ok(segs.join("/"))
}

/// The image `src` of document `doc` (validated by [`resolve_doc`]): lexically checked
/// ([`check_image_src`]), joined to the document's folder with `..` resolved lexically (never
/// above the root, never into `.agentboard` / `.git`), then canonicalized and prefix-checked
/// against the canonical root (junction-safe), an image extension on the real file too.
/// Returns the file and its MIME type.
pub fn resolve_image(root: &Path, doc: &str, src: &str) -> Result<(PathBuf, &'static str), String> {
    let rel_src = check_image_src(src)?;
    let doc = resolve_doc(root, doc)?;
    let mut parts: Vec<&str> = doc.rel.split('/').collect();
    parts.pop(); // the document's folder
    for seg in rel_src.split('/') {
        if seg == ".." {
            if parts.pop().is_none() {
                return Err(image_err(src));
            }
        } else {
            parts.push(seg);
        }
    }
    if parts.first().is_some_and(|t| IMAGE_DENY_TOP.iter().any(|d| d.eq_ignore_ascii_case(t))) {
        return Err(image_err(src));
    }
    let joined = parts.iter().fold(root.to_path_buf(), |p, s| p.join(s));
    let root_real = std::fs::canonicalize(root).map_err(|_| "workspaceRoot 폴더를 찾을 수 없습니다".to_string())?;
    let real = std::fs::canonicalize(&joined).map_err(|_| format!("이미지를 찾을 수 없습니다: {src}"))?;
    let inside = match real.strip_prefix(&root_real) {
        Ok(r) => {
            r.components().all(|c| matches!(c, Component::Normal(_)))
                && !r
                    .components()
                    .next()
                    .is_some_and(|c| IMAGE_DENY_TOP.iter().any(|d| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(d)))
        }
        Err(_) => false,
    };
    if !inside || !real.is_file() {
        return Err(image_err(src));
    }
    let mime = image_mime(&real).ok_or_else(|| image_err(src))?;
    Ok((real, mime))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// `doc_image`: the validated image as a `data:` URL (at most [`IMAGE_MAX_BYTES`]).
pub fn read_image(root: &Path, doc: &str, src: &str) -> Result<String, String> {
    let (path, mime) = resolve_image(root, doc, src)?;
    let len = std::fs::metadata(&path).map(|m| m.len()).map_err(|_| image_err(src))?;
    if len > IMAGE_MAX_BYTES {
        return Err(format!("이미지가 너무 큽니다({} MB 초과): {src}", IMAGE_MAX_BYTES / 1024 / 1024));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("이미지를 읽을 수 없습니다: {src} — {e}"))?;
    Ok(format!("data:{mime};base64,{}", base64(&bytes)))
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

    /// Whether `pane` is watched.
    pub fn contains(&self, pane: &str) -> bool {
        self.map.lock().unwrap().contains_key(pane)
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
    fn image_src_is_checked_lexically_before_any_file_system_call() {
        for ok in ["img/a.png", "./a.PNG", "..\\img\\b.jpg", "sub/%ED%95%9C.webp", "a.png?x=1#y", "a b.svg"] {
            assert!(check_image_src(ok).is_ok(), "{ok}");
        }
        assert_eq!(check_image_src("..\\img\\./b.jpg").unwrap(), "../img/b.jpg");
        assert_eq!(check_image_src("sub/%ED%95%9C.webp").unwrap(), "sub/한.webp");
        for bad in [
            "\\\\attacker\\x.png",
            "//attacker/x.png",
            "%5C%5Cattacker%5Cx.png",
            "%2F%2Fattacker/x.png",
            "\\\\?\\C:\\x.png",
            "\\\\.\\pipe\\x.png",
            "C:\\x.png",
            "C:x.png",
            "c%3A/x.png",
            "/abs.png",
            "\\abs.png",
            "file:///C:/x.png",
            "http://asset.localhost/x.png",
            "a.png:stream",
            "a.txt",
            "img/",
            "..",
            "nul.png",
            "COM1.png",
            "a.png.",
            "dir./a.png",
            "x%00.png",
            "%ZZ.png",
            "",
            "  ",
        ] {
            assert!(check_image_src(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn images_resolve_inside_the_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        write(&root.join("docs/t.md"), "x");
        std::fs::write(root.join("docs").join("a.png"), [0x89u8, b'P', b'N', b'G']).unwrap();
        write(&root.join("img/b.gif"), "GIF");
        write(&root.join(".git/x.png"), "x");
        write(&root.join(".agentboard/y.png"), "x");
        write(&outside.join("o.png"), "x");
        assert_eq!(read_image(&root, "docs/t.md", "a.png").unwrap(), "data:image/png;base64,iVBORw==");
        assert_eq!(read_image(&root, "docs/t.md", "../img/b.gif").unwrap(), format!("data:image/gif;base64,{}", base64(b"GIF")));
        for bad in ["../../outside/o.png", "../../../o.png", "../.git/x.png", "../.agentboard/y.png", "missing.png", "..\\..\\outside\\o.png"] {
            assert!(read_image(&root, "docs/t.md", bad).is_err(), "{bad}");
        }
        // the document itself must validate
        assert!(read_image(&root, "../outside/x.md", "o.png").is_err());
        // a junction inside the root pointing outside is refused after canonicalization
        if crate::issues::tests::link_dir(&root.join("docs").join("lnk"), &outside) {
            assert!(read_image(&root, "docs/t.md", "lnk/o.png").is_err());
        }
        // size cap
        let big = std::fs::File::create(root.join("docs").join("big.png")).unwrap();
        big.set_len(IMAGE_MAX_BYTES + 1).unwrap();
        assert!(read_image(&root, "docs/t.md", "big.png").unwrap_err().contains("큽니다"));
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn missing_documents_can_be_watched_inside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        write(&root.join("docs/t.md"), "x");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(resolve_watch_path(&root, "docs/t.md").unwrap(), root.join("docs").join("t.md"));
        assert_eq!(resolve_watch_path(&root, "docs/gone.md").unwrap(), root.join("docs").join("gone.md"));
        for bad in ["nodir/gone.md", "docs/gone.txt", "../outside/gone.md", "docs"] {
            assert!(resolve_watch_path(&root, bad).is_err(), "{bad}");
        }
        assert!(resolve_watch_path(&root, &outside.join("gone.md").to_string_lossy()).is_err());
    }

    #[test]
    fn listing_walks_have_a_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..40 {
            write(&root.join(format!("d{i:02}/x.md")), "x");
        }
        write(&root.join("Node_Modules/y.md"), "x");
        assert_eq!(list_md(root, None, LIST_MAX).len(), 40, "skip names compare case-insensitively");
        let few = list_md_budget(root, None, LIST_MAX, 10);
        assert!(few.len() < 10, "{}", few.len());
        assert_eq!(WALK_BUDGET, 50_000);
        // issue files: the first files in name order are kept when capped
        let issue = tmp.path().join("issue");
        for i in 0..(ISSUE_FILES_MAX + 20) {
            write(&issue.join(format!("output/f{i:04}.md")), "x");
        }
        std::fs::create_dir_all(issue.join("input")).unwrap();
        let f = issue_files(&issue).unwrap();
        assert_eq!(f.output.len(), ISSUE_FILES_MAX);
        assert_eq!(f.output[0].name, "f0000.md");
        assert_eq!(f.output[ISSUE_FILES_MAX - 1].name, format!("f{:04}.md", ISSUE_FILES_MAX - 1));
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
