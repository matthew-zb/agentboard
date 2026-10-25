//! Agent profiles: per-tool differences (command, readiness, submit encoding, pass-through keys).

use crate::config::{located_json_error, sem_err, ConfigError};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const READY_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_IDLE_MS: u64 = 800;
const TAIL_BYTES: usize = 4096;
const ANSI_RE: &str = r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Level {
    L0,
    L1,
    L2,
}

#[derive(Debug, Clone)]
pub enum Ready {
    Pattern(Regex),
    IdleMs(u64),
}

impl PartialEq for Ready {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Ready::Pattern(a), Ready::Pattern(b)) => a.as_str() == b.as_str(),
            (Ready::IdleMs(a), Ready::IdleMs(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submit {
    #[serde(default)]
    pub bracketed_paste: bool,
    #[serde(default = "default_key")]
    pub key: String,
}

fn default_key() -> String {
    "\r".to_string()
}

impl Default for Submit {
    fn default() -> Self {
        Submit { bracketed_paste: false, key: default_key() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentProfile {
    pub name: String,
    pub level: Level,
    pub cmd: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub ready: Ready,
    pub submit: Submit,
    pub passthrough_keys: Vec<String>,
    pub system_prompt: Option<Value>,
    pub mcp: Option<Value>,
    pub idle: Option<Value>,
    pub resume: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawProfile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    level: Option<Level>,
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    ready: Option<Value>,
    #[serde(default)]
    submit: Option<Submit>,
    #[serde(default)]
    passthrough_keys: Vec<String>,
    #[serde(default)]
    system_prompt: Option<Value>,
    #[serde(default)]
    mcp: Option<Value>,
    #[serde(default)]
    idle: Option<Value>,
    #[serde(default)]
    resume: Option<Value>,
}

fn parse_ready(v: Option<Value>, file: &Path) -> Result<Ready, ConfigError> {
    let Some(v) = v else { return Ok(Ready::IdleMs(DEFAULT_IDLE_MS)) };
    let Some(obj) = v.as_object() else {
        return Err(sem_err(file, "ready", "ready는 {\"pattern\": ...} 또는 {\"idleMs\": ...} 객체여야 함"));
    };
    if let Some(k) = obj.keys().find(|k| k.as_str() != "pattern" && k.as_str() != "idleMs") {
        return Err(sem_err(file, format!("ready.{k}"), format!("알 수 없는 필드 '{k}'")));
    }
    match (obj.get("pattern"), obj.get("idleMs")) {
        (Some(_), Some(_)) => Err(sem_err(file, "ready", "pattern과 idleMs를 함께 쓸 수 없음")),
        (None, None) => Err(sem_err(file, "ready", "pattern 또는 idleMs가 필요함")),
        (Some(p), None) => {
            let Some(s) = p.as_str() else {
                return Err(sem_err(file, "ready.pattern", "pattern은 문자열이어야 함"));
            };
            Regex::new(s)
                .map(Ready::Pattern)
                .map_err(|e| sem_err(file, "ready.pattern", format!("잘못된 정규식: {e}")))
        }
        (None, Some(n)) => match n.as_u64() {
            Some(ms) => Ok(Ready::IdleMs(ms)),
            None => Err(sem_err(file, "ready.idleMs", "idleMs는 0 이상의 정수여야 함")),
        },
    }
}

pub fn load_dir(dir: &Path) -> (BTreeMap<String, AgentProfile>, Vec<ConfigError>) {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return (map, errors) };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().map_or(false, |x| x == "json"))
        .collect();
    files.sort();
    for p in files {
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) => {
                errors.push(ConfigError {
                    file: p.clone(),
                    line: 0,
                    col: 0,
                    json_path: None,
                    message: format!("파일을 읽을 수 없음: {e}"),
                });
                continue;
            }
        };
        let raw: RawProfile = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                errors.push(located_json_error(&p, &e));
                continue;
            }
        };
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = raw.name.clone().unwrap_or(stem);
        if name.trim().is_empty() {
            errors.push(sem_err(&p, "name", "에이전트 이름이 비어 있음"));
            continue;
        }
        if raw.cmd.trim().is_empty() {
            errors.push(sem_err(&p, "cmd", "cmd가 비어 있음"));
            continue;
        }
        let ready = match parse_ready(raw.ready, &p) {
            Ok(r) => r,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        if map.contains_key(&name) {
            errors.push(sem_err(&p, "name", format!("에이전트 이름 '{name}'이(가) 다른 파일과 중복됨")));
            continue;
        }
        map.insert(
            name.clone(),
            AgentProfile {
                name,
                level: raw.level.unwrap_or(Level::L0),
                cmd: raw.cmd,
                args: raw.args,
                env: raw.env,
                ready,
                submit: raw.submit.unwrap_or_default(),
                passthrough_keys: raw.passthrough_keys,
                system_prompt: raw.system_prompt,
                mcp: raw.mcp,
                idle: raw.idle,
                resume: raw.resume,
            },
        );
    }
    (map, errors)
}

/// Create `powershell.json` only when missing: `pwsh` if installed, else `powershell.exe`.
pub fn ensure_powershell_profile(dir: &Path) {
    ensure_powershell_profile_with(dir, which::which("pwsh").is_ok());
}

pub(crate) fn ensure_powershell_profile_with(dir: &Path, has_pwsh: bool) {
    let path = dir.join("powershell.json");
    if path.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let v = serde_json::json!({
        "name": "powershell",
        "level": "L0",
        "cmd": if has_pwsh { "pwsh" } else { "powershell.exe" },
        "args": [],
        "env": {},
        "ready": { "pattern": "PS .*> $" },
        "submit": { "bracketedPaste": false, "key": "\r" },
        "passthroughKeys": []
    });
    if let Ok(s) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(&path, s + "\n");
    }
}

/// PATH lookup including PATHEXT (`.exe/.cmd/.bat`).
pub fn resolve_cmd(cmd: &str) -> Option<PathBuf> {
    which::which(cmd).ok()
}

/// Bytes to type into the PTY to submit `text`: newlines become CR (or stay LF inside a bracketed paste).
pub fn encode_submit(text: &str, s: &Submit) -> Vec<u8> {
    // the text must not end the bracketed paste early or inject escape sequences
    let norm = text.replace("\r\n", "\n").replace("\x1b[201~", "").replace('\x1b', "");
    let mut out = Vec::new();
    if s.bracketed_paste {
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(norm.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
    } else {
        out.extend_from_slice(norm.replace('\n', "\r").as_bytes());
    }
    out.extend_from_slice(s.key.as_bytes());
    out
}

pub fn resolve_cwd(tab_workspace: &Path, cwd: Option<&str>) -> PathBuf {
    match cwd {
        Some(c) if !c.is_empty() => tab_workspace.join(c),
        _ => tab_workspace.to_path_buf(),
    }
}

pub struct ReadyDetector {
    ready: Ready,
    #[allow(dead_code)]
    start: Instant,
    tail: Vec<u8>,
    last_output: Option<Instant>,
    done: bool,
    ansi: Regex,
}

impl ReadyDetector {
    pub fn new(ready: &Ready, start: Instant) -> Self {
        ReadyDetector {
            ready: ready.clone(),
            start,
            tail: Vec::new(),
            last_output: None,
            done: false,
            ansi: Regex::new(ANSI_RE).expect("ansi regex"),
        }
    }

    pub fn feed(&mut self, bytes: &[u8], now: Instant) -> bool {
        if self.done {
            return true;
        }
        if !bytes.is_empty() {
            self.last_output = Some(now);
        }
        if let Ready::Pattern(re) = &self.ready {
            self.tail.extend_from_slice(bytes);
            if self.tail.len() > TAIL_BYTES {
                let cut = self.tail.len() - TAIL_BYTES;
                self.tail.drain(..cut);
            }
            let text = String::from_utf8_lossy(&self.tail);
            let clean = self.ansi.replace_all(&text, "");
            let last = clean.rsplit('\n').next().unwrap_or("");
            let last = last.strip_suffix('\r').unwrap_or(last);
            if re.is_match(last) {
                self.done = true;
            }
        }
        self.done
    }

    /// Count `now` as output: an idle rule then needs a quiet period from here on.
    pub fn seed(&mut self, now: Instant) {
        if !self.done {
            self.last_output = Some(now);
        }
    }

    pub fn poll(&mut self, now: Instant) -> bool {
        if self.done {
            return true;
        }
        if let (Ready::IdleMs(ms), Some(last)) = (&self.ready, self.last_output) {
            if now.saturating_duration_since(last) >= Duration::from_millis(*ms) {
                self.done = true;
            }
        }
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const CLAUDE: &str = include_str!("../defaults/agents/claude.json");
    const CMD: &str = include_str!("../defaults/agents/cmd.json");

    fn load(files: &[(&str, &str)]) -> (BTreeMap<String, AgentProfile>, Vec<ConfigError>) {
        let d = tempfile::tempdir().unwrap();
        for (n, c) in files {
            std::fs::write(d.path().join(n), c).unwrap();
        }
        load_dir(d.path())
    }

    #[test]
    fn parses_claude_profile_with_reserved_fields() {
        let (m, errs) = load(&[("claude.json", CLAUDE), ("cmd.json", CMD)]);
        assert!(errs.is_empty(), "{errs:?}");
        let c = &m["claude"];
        assert_eq!(c.level, Level::L2);
        assert_eq!(c.cmd, "claude");
        assert_eq!(c.ready, Ready::IdleMs(800));
        assert_eq!(c.submit, Submit { bracketed_paste: true, key: "\r".into() });
        assert!(c.mcp.is_some() && c.system_prompt.is_some() && c.idle.is_some() && c.resume.is_some());
        let cmd = &m["cmd"];
        assert_eq!(cmd.level, Level::L0);
        match &cmd.ready {
            Ready::Pattern(r) => {
                assert!(r.is_match(r"C:\Users\x>"));
                assert!(!r.is_match("hello"));
            }
            _ => panic!("pattern expected"),
        }
    }

    #[test]
    fn defaults_apply_when_fields_missing() {
        let (m, errs) = load(&[("x.json", r#"{"cmd":"foo"}"#)]);
        assert!(errs.is_empty());
        let p = &m["x"];
        assert_eq!(p.ready, Ready::IdleMs(800));
        assert_eq!(p.submit, Submit::default());
        assert_eq!(p.level, Level::L0);
    }

    #[test]
    fn encode_submit_plain() {
        assert_eq!(encode_submit("a\nb", &Submit { bracketed_paste: false, key: "\r".into() }), b"a\rb\r".to_vec());
    }

    #[test]
    fn encode_submit_cannot_break_out_of_bracketed_paste() {
        let text = "a\x1b[201~\x1b[31mb\x1bc";
        assert_eq!(
            encode_submit(text, &Submit { bracketed_paste: true, key: "\r".into() }),
            b"\x1b[200~a[31mbc\x1b[201~\r".to_vec()
        );
        assert_eq!(encode_submit(text, &Submit { bracketed_paste: false, key: "\r".into() }), b"a[31mbc\r".to_vec());
    }

    #[test]
    fn encode_submit_bracketed() {
        assert_eq!(
            encode_submit("a\nb", &Submit { bracketed_paste: true, key: "\r".into() }),
            b"\x1b[200~a\nb\x1b[201~\r".to_vec()
        );
    }

    #[test]
    fn ready_pattern_matches_last_line_ignoring_ansi() {
        let t = Instant::now();
        let ready = Ready::Pattern(Regex::new("PS .*> $").unwrap());
        let mut d = ReadyDetector::new(&ready, t);
        assert!(!d.feed(b"PS C:\\x> echo hi\r\nhi\r\n", t));
        assert!(!d.poll(t + Duration::from_secs(5)));
        assert!(d.feed(b"\x1b[32mPS C:\\x> \x1b[0m", t));
        assert!(d.poll(t));
        assert!(d.feed(b"more output\r\n", t));

        let mut d = ReadyDetector::new(&ready, t);
        assert!(!d.feed(b"PS C:\\x> \r\nrunning...\r\n", t));
    }

    #[test]
    fn ready_idle_requires_first_output_then_quiet() {
        let t = Instant::now();
        let mut d = ReadyDetector::new(&Ready::IdleMs(800), t);
        assert!(!d.poll(t + Duration::from_secs(5)));
        d.feed(b"x", t);
        assert!(!d.poll(t + Duration::from_millis(799)));
        assert!(d.poll(t + Duration::from_millis(800)));
        assert!(d.poll(t + Duration::from_millis(801)));
    }

    #[test]
    fn ready_idle_resets_on_new_output() {
        let t = Instant::now();
        let mut d = ReadyDetector::new(&Ready::IdleMs(800), t);
        d.feed(b"a", t);
        d.feed(b"b", t + Duration::from_millis(500));
        assert!(!d.poll(t + Duration::from_millis(1200)));
        assert!(d.poll(t + Duration::from_millis(1300)));
    }

    #[test]
    fn resolve_cwd_handles_korean_and_spaces() {
        let base = Path::new("C:/workspace/내 프로젝트");
        assert_eq!(resolve_cwd(base, Some("server")), base.join("server"));
        assert_eq!(resolve_cwd(base, None), base.to_path_buf());
        assert_eq!(resolve_cwd(base, Some("")), base.to_path_buf());
        let _: PathBuf = resolve_cwd(base, None);
    }

    #[test]
    fn powershell_profile_created_once() {
        for (has, cmd) in [(true, "pwsh"), (false, "powershell.exe")] {
            let d = tempfile::tempdir().unwrap();
            let agents = d.path().join("agents");
            ensure_powershell_profile_with(&agents, has);
            let (m, errs) = load_dir(&agents);
            assert!(errs.is_empty(), "{errs:?}");
            assert_eq!(m["powershell"].cmd, cmd);
            assert_eq!(m["powershell"].level, Level::L0);
            assert!(matches!(&m["powershell"].ready, Ready::Pattern(r) if r.as_str() == "PS .*> $"));
            std::fs::write(agents.join("powershell.json"), "MINE").unwrap();
            ensure_powershell_profile_with(&agents, !has);
            assert_eq!(std::fs::read_to_string(agents.join("powershell.json")).unwrap(), "MINE");
        }
    }

    #[test]
    fn invalid_regex_is_config_error() {
        let (m, errs) = load(&[("bad.json", r#"{"cmd":"x","ready":{"pattern":"("}}"#), ("ok.json", r#"{"cmd":"y"}"#)]);
        assert!(!m.contains_key("bad"));
        assert!(m.contains_key("ok"));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].json_path.as_deref(), Some("ready.pattern"));
        assert!(errs[0].file.ends_with("bad.json"));
    }

    #[test]
    fn syntax_error_is_located() {
        let (_, errs) = load(&[("a.json", "{\n \"cmd\": \n}")]);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].line > 0);
    }

    #[test]
    fn resolve_cmd_finds_cmd_exe_and_misses_garbage() {
        if cfg!(windows) {
            assert!(resolve_cmd("cmd").is_some());
        }
        assert!(resolve_cmd("definitely-not-a-real-command-xyz").is_none());
    }
}
