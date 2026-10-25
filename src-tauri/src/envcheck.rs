//! Environment check (spec §9, Task 12): what the app needs from the machine, reported to agents
//! through MCP `env_status` (every role) and to the user in the status bar.
//!
//! Items (ids are stable — role prompts and the README refer to them):
//! * `claude-version` — `claude --version` (5 s timeout) must be ≥ [`MIN_CLAUDE`]; below it
//!   only agent-to-agent messages stop working.
//! * `git-bash` — Git Bash as Claude Code finds it: `CLAUDE_CODE_GIT_BASH_PATH` if set (must be
//!   an existing file), else `git` on PATH and `bin\bash.exe` of that Git installation. Role
//!   hooks run through it (R8).
//! * `folder-trust:<tab name>` — one per tab with a standing role agent: failing when that
//!   agent's current process did not report SessionStart within 30 s (Task 9's timer, most
//!   likely Claude's folder-trust question). The app never edits trust state (R5).
//! * custom items from `config.json` `envRequire` (default: `holla`, required) — the `check`
//!   command runs through `cmd.exe /D /S /C` (always present, unlike Git Bash which is itself
//!   an item; npm `.cmd` shims work), no window, 10 s timeout, exit code 0 = fine. A timeout
//!   kills the whole process tree (job object, KILL_ON_JOB_CLOSE).
//!
//! The app never installs anything: `fix` is advice for the dispatcher, which runs installs only
//! after the user approves (R9/R12). Skipping (`env_skip`, recorded in `envSkip`) marks an item
//! `skipped`; required custom items cannot be skipped.
//!
//! Probing is slow (processes), so results are cached in [`EnvCache`] ([`refresh`] runs every
//! probe in parallel, off the UI thread); [`check_all`] itself is pure and cheap and is
//! recomputed on every read, so skip lists, config and folder-trust state are always current.
//! Locks: the cache's two mutexes are leaves (`refresh_lock` is held only while processes run,
//! with no app lock); readers take `ws` (clone, released), then `loaded(read)`, then `trust`,
//! one at a time.

use crate::config::{EnvRequire, Loaded, ENV_TRUST_PREFIX};
use crate::workspace::Workspace;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Lowest Claude Code version with Windows cross-session messaging.
pub const MIN_CLAUDE: (u32, u32, u32) = (2, 1, 234);
pub const CLAUDE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// `env_status` (MCP) reruns the probes when the cached results are older than this.
pub const CACHE_TTL: Duration = Duration::from_secs(10);
/// Captured stdout of a check (the rest is read and discarded).
pub const OUTPUT_CAP: u64 = 64 * 1024;

/// One environment check result (MCP `env_status`, status bar).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvItem {
    /// `claude-version` | `git-bash` | `folder-trust:<tab>` | custom `envRequire` id.
    pub id: String,
    pub ok: bool,
    /// What was found (version, path, exit code, timeout …).
    pub detail: String,
    /// Why the app needs it.
    pub why: String,
    /// What stops working without it.
    pub missing_features: String,
    /// Install / fix command or instructions (the dispatcher runs it only after the user agrees).
    pub fix: String,
    /// Listed in `envSkip` (never for required items).
    pub skipped: bool,
    /// Required custom item: cannot be skipped, highlighted until fixed.
    pub required: bool,
    /// Custom items: repository URL to install from (`None` = unknown → ask the user, then
    /// `env_set_source`). Always `None` for built-in items.
    pub source: Option<String>,
}

/// Outcome of one shell check.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckOutcome {
    /// Exit code 0.
    Ok,
    /// Non-zero exit code.
    Exit(i32),
    /// Did not finish in time; the process tree was killed.
    Timeout,
    /// Could not be started.
    Error(String),
    /// Not probed yet (e.g. the command changed since the last refresh).
    NotRun,
}

/// What [`check_all`] needs from the machine; tests substitute a fake.
pub trait EnvProbe {
    /// Output of `claude --version`, or why it could not be run.
    fn claude_version(&self) -> Result<String, String>;
    /// Git Bash location, or why none was found.
    fn git_bash(&self) -> Result<PathBuf, String>;
    /// Run a custom check command.
    fn run_check(&self, command: &str) -> CheckOutcome;
    /// Whether the folder-trust timer of `pane`'s current process ran out (Task 9).
    fn trust_timed_out(&self, pane: &str) -> bool;
}

/// `x.y.z` from the first token that looks like one (`"2.1.310 (Claude Code)"`).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    s.split(|c: char| c.is_whitespace() || c == 'v' || c == '(' || c == ')').find_map(|tok| {
        let mut it = tok.split('.');
        let a = it.next()?.parse().ok()?;
        let b = it.next()?.parse().ok()?;
        let c: String = it.next()?.chars().take_while(|c| c.is_ascii_digit()).collect();
        Some((a, b, c.parse().ok()?))
    })
}

pub fn version_ok(v: (u32, u32, u32)) -> bool {
    v >= MIN_CLAUDE
}

fn min_claude_str() -> String {
    format!("{}.{}.{}", MIN_CLAUDE.0, MIN_CLAUDE.1, MIN_CLAUDE.2)
}

/// Every item, in a fixed order: claude-version, git-bash, folder-trust per standing role tab
/// (tab id order), then custom items in config order. Pure apart from `probe`.
pub fn check_all(cfg: &Loaded, ws: &Workspace, probe: &dyn EnvProbe) -> Vec<EnvItem> {
    let c = &cfg.config;
    let skipped = |id: &str| c.env_skip.iter().any(|s| s == id);
    let mut out = Vec::new();

    // Claude Code version
    let (ok, detail, fix) = match probe.claude_version() {
        Ok(text) => match parse_version(&text) {
            Some(v) if version_ok(v) => (true, format!("{}.{}.{}", v.0, v.1, v.2), String::new()),
            Some(v) => (
                false,
                format!("{}.{}.{} — {} 이상 필요", v.0, v.1, v.2, min_claude_str()),
                "claude update".to_string(),
            ),
            None => (false, format!("버전을 알 수 없습니다: {}", one_line(&text)), "claude update".to_string()),
        },
        Err(e) => (
            false,
            format!("Claude Code를 실행할 수 없습니다 — {e}"),
            "Claude Code 설치(예: npm install -g @anthropic-ai/claude-code) 후 claude update".to_string(),
        ),
    };
    out.push(EnvItem {
        id: "claude-version".into(),
        ok,
        detail,
        why: format!("Windows 세션 간 메시징(에이전트 간 메시지)에 Claude Code {} 이상이 필요", min_claude_str()),
        missing_features: "에이전트 간 메시지 전달 — 탭·MCP 도구·인박스는 그대로 동작".into(),
        fix,
        skipped: skipped("claude-version"),
        required: false,
        source: None,
    });

    // Git Bash
    let (ok, detail) = match probe.git_bash() {
        Ok(p) => (true, p.display().to_string()),
        Err(e) => (false, e),
    };
    out.push(EnvItem {
        id: "git-bash".into(),
        ok,
        detail,
        why: "역할 에이전트의 hook(상태 보고)을 Claude Code가 Git Bash로 실행".into(),
        missing_features: "hook 기반 기능: 앱→세션 메시지용 세션 등록, 권한 대기 알림, 폴더 신뢰 감지".into(),
        fix: if ok { String::new() } else { "winget install --id Git.Git -e".into() },
        skipped: skipped("git-bash"),
        required: false,
        source: None,
    });

    // folder trust, one item per tab with a standing role agent. Without Git Bash no hook runs,
    // so a missing SessionStart may not mean an unanswered trust question.
    let no_hooks = !ok;
    for t in ws.tabs.values() {
        let Some(pane) = ws.standing_pane(&t.id) else { continue };
        let id = format!("{ENV_TRUST_PREFIX}{}", t.name);
        let timed_out = probe.trust_timed_out(&pane);
        out.push(EnvItem {
            ok: !timed_out,
            detail: if timed_out {
                let mut d = format!("'{}' 에이전트가 30초 안에 시작을 알리지 않았습니다 — 폴더 신뢰 질문을 기다리는 중일 수 있습니다 ({})", t.name, t.workspace.display());
                if no_hooks {
                    d.push_str(" — Git Bash가 없어 hook이 실행되지 않았을 수도 있습니다(git-bash 항목 참고)");
                }
                d
            } else {
                t.workspace.display().to_string()
            },
            why: "역할 에이전트가 이 작업 폴더에서 시작하려면 Claude Code의 폴더 신뢰가 필요".into(),
            missing_features: format!("'{}' 탭 에이전트의 시작(세션 등록·시작 프롬프트·MCP 도구)", t.name),
            fix: if timed_out {
                format!("'{}' 탭의 창에서 Claude Code의 폴더 신뢰 질문에 '예'로 답하세요(폴더마다 한 번). 앱은 신뢰 설정을 바꾸지 않습니다", t.name)
            } else {
                String::new()
            },
            skipped: skipped(&id),
            required: false,
            source: None,
            id,
        });
    }

    // custom items
    for e in &c.env_require {
        out.push(custom_item(e, probe.run_check(&e.check), !e.required && skipped(&e.id)));
    }
    out
}

fn custom_item(e: &EnvRequire, outcome: CheckOutcome, skipped: bool) -> EnvItem {
    let secs = CHECK_TIMEOUT.as_secs();
    let (ok, detail) = match outcome {
        CheckOutcome::Ok => (true, format!("'{}' 정상(종료 코드 0)", e.check)),
        CheckOutcome::Exit(code) => (false, format!("'{}' 실패(종료 코드 {code})", e.check)),
        CheckOutcome::Timeout => (false, format!("'{}' 시간 초과({secs}초) — 실패로 봅니다", e.check)),
        CheckOutcome::Error(m) => (false, format!("'{}'을(를) 실행할 수 없습니다 — {m}", e.check)),
        CheckOutcome::NotRun => (false, format!("'{}' 아직 점검하지 않았습니다", e.check)),
    };
    EnvItem {
        id: e.id.clone(),
        ok,
        detail,
        why: e.why.clone(),
        missing_features: if e.why.is_empty() { format!("'{}'이(가) 필요한 기능", e.id) } else { e.why.clone() },
        fix: e.fix.clone(),
        skipped,
        required: e.required,
        source: e.source.clone(),
    }
}

fn one_line(s: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(200).collect()
}

// ───────────────────────── real probes ─────────────────────────

/// Result of running a command line.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub outcome: CheckOutcome,
    pub stdout: String,
}

/// Run `command` through `cmd.exe /D /S /C "<command>"` with no window, stdin closed, and a
/// timeout. The process is put into its own job object (KILL_ON_JOB_CLOSE) right after it
/// starts; when the timeout passes — or when it finished — the job is closed, which kills the
/// whole tree (a child started in the few microseconds before the assignment could escape;
/// accepted). Blocking: call off the UI thread.
#[cfg(windows)]
pub fn run_shell(command: &str, timeout: Duration) -> RunResult {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
    let mut cmd = Command::new(comspec);
    cmd.raw_arg(format!("/D /S /C \"{command}\""))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return RunResult { outcome: CheckOutcome::Error(e.to_string()), stdout: String::new() },
    };
    let job = crate::winjob::JobGuard::new().ok();
    if let Some(j) = &job {
        let _ = j.assign(child.id());
    }
    let mut out = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.take(OUTPUT_CAP).read_to_end(&mut buf);
            // keep draining past the cap so a chatty check never blocks on a full pipe
            let _ = std::io::copy(o, &mut std::io::sink());
        }
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(st)) => break if st.success() { CheckOutcome::Ok } else { CheckOutcome::Exit(st.code().unwrap_or(-1)) },
            Ok(None) if Instant::now() >= deadline => {
                // the tree goes with the job; kill the shell itself too in case the job is missing
                drop(job);
                let _ = child.kill();
                let _ = child.wait();
                return RunResult { outcome: CheckOutcome::Timeout, stdout: String::new() };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => break CheckOutcome::Error(e.to_string()),
        }
    };
    drop(job); // leftovers (a background child of the check) die with the job
    let stdout = rx.recv_timeout(Duration::from_millis(500)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    RunResult { outcome, stdout }
}

#[cfg(not(windows))]
pub fn run_shell(_command: &str, _timeout: Duration) -> RunResult {
    RunResult { outcome: CheckOutcome::Error("Windows 전용".into()), stdout: String::new() }
}

/// `claude --version` (through the shell, so npm `.cmd` shims resolve like in a terminal).
pub fn probe_claude_version() -> Result<String, String> {
    let r = run_shell("claude --version", CLAUDE_TIMEOUT);
    match r.outcome {
        CheckOutcome::Ok => Ok(r.stdout.trim().to_string()),
        CheckOutcome::Exit(c) => Err(format!("claude를 찾을 수 없거나 실행에 실패했습니다(종료 코드 {c})")),
        CheckOutcome::Timeout => Err(format!("{}초 안에 응답하지 않았습니다", CLAUDE_TIMEOUT.as_secs())),
        CheckOutcome::Error(m) => Err(m),
        CheckOutcome::NotRun => Err("점검하지 않았습니다".into()),
    }
}

/// Git Bash the way Claude Code finds it: `CLAUDE_CODE_GIT_BASH_PATH`, else `git` on PATH →
/// `<git root>\bin\bash.exe` (git.exe lives in `cmd\`, `bin\` or `mingw64\bin\`).
pub fn find_git_bash(env_path: Option<&std::ffi::OsStr>, git: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = env_path.filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("CLAUDE_CODE_GIT_BASH_PATH가 가리키는 파일이 없습니다: {}", p.display()))
        };
    }
    let git = git.ok_or("Git for Windows(git)를 PATH에서 찾을 수 없습니다")?;
    let candidates: Vec<PathBuf> = git.ancestors().skip(1).take(3).map(|d| d.join("bin").join("bash.exe")).collect();
    candidates
        .into_iter()
        .find(|c| c.is_file())
        .ok_or_else(|| format!("git({})은 있지만 Git Bash(bin\\bash.exe)를 찾을 수 없습니다", git.display()))
}

pub fn probe_git_bash() -> Result<PathBuf, String> {
    find_git_bash(std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").as_deref(), which::which("git").ok())
}

// ───────────────────────── cache ─────────────────────────

/// Probe results of one refresh (folder trust is read live, never cached).
#[derive(Debug, Clone)]
pub struct ProbeResults {
    pub at: Instant,
    pub claude: Result<String, String>,
    pub git_bash: Result<PathBuf, String>,
    /// check command → outcome.
    pub checks: BTreeMap<String, CheckOutcome>,
}

/// [`EnvProbe`] over cached results plus a live folder-trust lookup.
pub struct CachedProbe<'a> {
    pub results: &'a ProbeResults,
    pub timed_out: &'a dyn Fn(&str) -> bool,
}

impl EnvProbe for CachedProbe<'_> {
    fn claude_version(&self) -> Result<String, String> {
        self.results.claude.clone()
    }
    fn git_bash(&self) -> Result<PathBuf, String> {
        self.results.git_bash.clone()
    }
    fn run_check(&self, command: &str) -> CheckOutcome {
        self.results.checks.get(command).cloned().unwrap_or(CheckOutcome::NotRun)
    }
    fn trust_timed_out(&self, pane: &str) -> bool {
        (self.timed_out)(pane)
    }
}

/// `AppState::env`: the last probe results. Both locks are leaves.
#[derive(Default)]
pub struct EnvCache {
    results: Mutex<Option<ProbeResults>>,
    /// Serializes refreshes (held while the probe processes run; nothing else is taken under it
    /// except `results`).
    refresh_lock: Mutex<()>,
}

impl EnvCache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self) -> Option<ProbeResults> {
        self.results.lock().unwrap().clone()
    }
    /// Replace the results (tests inject them this way).
    pub fn set(&self, r: ProbeResults) {
        *self.results.lock().unwrap() = Some(r);
    }
}

/// Run every probe in parallel (claude, Git Bash, each distinct check command) and store the
/// results. Blocking (≤ ~10 s): call from a worker / MCP thread. Concurrent callers wait for
/// the running refresh and reuse its results.
pub fn refresh_probes(cache: &EnvCache, commands: Vec<String>) -> ProbeResults {
    let started = Instant::now();
    let _g = cache.refresh_lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = cache.get() {
        if r.at >= started {
            return r; // another caller refreshed while we waited
        }
    }
    let claude = std::thread::spawn(probe_claude_version);
    let git = std::thread::spawn(probe_git_bash);
    let mut uniq = commands;
    uniq.sort();
    uniq.dedup();
    let checks: Vec<_> = uniq
        .into_iter()
        .map(|c| {
            let c2 = c.clone();
            (c, std::thread::spawn(move || run_shell(&c2, CHECK_TIMEOUT).outcome))
        })
        .collect();
    let r = ProbeResults {
        at: Instant::now(),
        claude: claude.join().unwrap_or_else(|_| Err("점검 중 오류".into())),
        git_bash: git.join().unwrap_or_else(|_| Err("점검 중 오류".into())),
        checks: checks
            .into_iter()
            .map(|(c, h)| (c, h.join().unwrap_or_else(|_| CheckOutcome::Error("점검 중 오류".into()))))
            .collect(),
    };
    cache.set(r.clone());
    r
}

/// The current items from cached results (`None` before the first refresh finished).
pub fn current(state: &crate::AppState) -> Option<Vec<EnvItem>> {
    let results = state.env.get()?;
    Some(items_from(state, &results))
}

fn items_from(state: &crate::AppState, results: &ProbeResults) -> Vec<EnvItem> {
    let ws = state.ws.lock().unwrap().clone();
    let timed_out = |pane: &str| crate::attention::trust_timed_out(state, pane);
    let probe = CachedProbe { results, timed_out: &timed_out };
    let loaded = state.loaded.read().unwrap();
    check_all(&loaded, &ws, &probe)
}

/// Re-probe (blocking), then tell the UI. Returns the fresh items.
pub fn refresh(state: &crate::AppState) -> Vec<EnvItem> {
    let commands: Vec<String> = state.loaded.read().unwrap().config.env_require.iter().map(|e| e.check.clone()).collect();
    let results = refresh_probes(&state.env, commands);
    state.ui.env_changed();
    items_from(state, &results)
}

/// Items for `env_status`: cached when younger than [`CACHE_TTL`] and covering every
/// configured check, else a refresh.
pub fn status(state: &crate::AppState) -> Vec<EnvItem> {
    let fresh = state.env.get().filter(|r| {
        let cmds_known = state.loaded.read().unwrap().config.env_require.iter().all(|e| r.checks.contains_key(&e.check));
        r.at.elapsed() < CACHE_TTL && cmds_known
    });
    match fresh {
        Some(r) => items_from(state, &r),
        None => refresh(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};

    pub struct Fake {
        pub claude: Result<String, String>,
        pub git: Result<PathBuf, String>,
        pub checks: BTreeMap<String, CheckOutcome>,
        pub timed_out: Vec<String>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Fake {
                claude: Ok("2.1.310 (Claude Code)".into()),
                git: Ok(PathBuf::from("C:/Program Files/Git/bin/bash.exe")),
                checks: BTreeMap::new(),
                timed_out: Vec::new(),
            }
        }
    }

    impl EnvProbe for Fake {
        fn claude_version(&self) -> Result<String, String> {
            self.claude.clone()
        }
        fn git_bash(&self) -> Result<PathBuf, String> {
            self.git.clone()
        }
        fn run_check(&self, command: &str) -> CheckOutcome {
            self.checks.get(command).cloned().unwrap_or(CheckOutcome::Ok)
        }
        fn trust_timed_out(&self, pane: &str) -> bool {
            self.timed_out.iter().any(|p| p == pane)
        }
    }

    fn loaded(config: &str) -> (Loaded, tempfile::TempDir, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), config).unwrap();
        let l = load_all(appdata.path());
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        (l, root, appdata)
    }

    fn item<'a>(items: &'a [EnvItem], id: &str) -> &'a EnvItem {
        items.iter().find(|i| i.id == id).unwrap_or_else(|| panic!("no item {id}: {items:?}"))
    }

    #[test]
    fn version_parse_and_compare() {
        assert_eq!(parse_version("2.1.310 (Claude Code)"), Some((2, 1, 310)));
        assert_eq!(parse_version("  2.1.234\r\n"), Some((2, 1, 234)));
        assert_eq!(parse_version("v2.10.0"), Some((2, 10, 0)));
        assert_eq!(parse_version("claude 3.0.1-beta"), Some((3, 0, 1)));
        assert_eq!(parse_version("garbage"), None);
        assert_eq!(parse_version("2.1"), None);
        assert!(version_ok((2, 1, 234)));
        assert!(version_ok((2, 1, 310)));
        assert!(version_ok((2, 2, 0)));
        assert!(version_ok((10, 0, 0)));
        assert!(!version_ok((2, 1, 233)));
        assert!(!version_ok((2, 0, 999)));
        assert!(!version_ok((1, 9, 999)));

        // through check_all
        let (l, _r, _a) = loaded(r#"{"envRequire":[]}"#);
        let ws = Workspace::from_config(&l);
        let low = Fake { claude: Ok("2.1.215 (Claude Code)".into()), ..Default::default() };
        let it = check_all(&l, &ws, &low);
        let c = item(&it, "claude-version");
        assert!(!c.ok && c.detail.contains("2.1.234 이상") && c.fix == "claude update", "{c:?}");
        let it = check_all(&l, &ws, &Fake::default());
        assert!(item(&it, "claude-version").ok);
        let missing = Fake { claude: Err("없음".into()), ..Default::default() };
        let it = check_all(&l, &ws, &missing);
        assert!(!item(&it, "claude-version").ok);
    }

    #[test]
    fn env_items_respect_skip_list() {
        let (l, _r, _a) = loaded(
            r#"{"tabs":[{"name":"billing","role":"manager","dir":"billing"}],
                "envSkip":["git-bash","opt","folder-trust:billing"],
                "envRequire":[{"id":"opt","check":"opt --v","fix":"설치","why":"선택 기능"},
                              {"id":"other","check":"other --v"}]}"#,
        );
        let ws = Workspace::from_config(&l);
        let mgr = ws.standing_pane(ws.tabs.values().find(|t| t.name == "billing").unwrap().id.as_str()).unwrap();
        let probe = Fake {
            git: Err("Git for Windows(git)를 PATH에서 찾을 수 없습니다".into()),
            checks: [("opt --v".to_string(), CheckOutcome::Exit(1)), ("other --v".to_string(), CheckOutcome::Exit(2))].into(),
            timed_out: vec![mgr],
            ..Default::default()
        };
        let it = check_all(&l, &ws, &probe);
        let ids: Vec<&str> = it.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["claude-version", "git-bash", "folder-trust:dispatcher", "folder-trust:billing", "opt", "other"]);
        let g = item(&it, "git-bash");
        assert!(!g.ok && g.skipped && g.fix == "winget install --id Git.Git -e", "{g:?}");
        let o = item(&it, "opt");
        assert!(!o.ok && o.skipped && !o.required && o.detail.contains("종료 코드 1"), "{o:?}");
        assert_eq!((o.why.as_str(), o.fix.as_str()), ("선택 기능", "설치"));
        let other = item(&it, "other");
        assert!(!other.ok && !other.skipped);
        let ft = item(&it, "folder-trust:billing");
        assert!(!ft.ok && ft.skipped && ft.fix.contains("폴더 신뢰"), "{ft:?}");
        assert!(item(&it, "folder-trust:dispatcher").ok);
        assert!(!item(&it, "claude-version").skipped);
    }

    #[test]
    fn default_env_require_is_the_holla_item_and_explicit_empty_list_disables_it() {
        let (l, _r, _a) = loaded("{}");
        assert_eq!(l.config.env_require, crate::config::default_env_require());
        let h = &l.config.env_require[0];
        assert_eq!((h.id.as_str(), h.check.as_str(), h.required, h.source.clone()), ("holla", "holla slack version", true, None));
        let ws = Workspace::from_config(&l);
        let it = check_all(&l, &ws, &Fake { checks: [("holla slack version".to_string(), CheckOutcome::Exit(1))].into(), ..Default::default() });
        let h = item(&it, "holla");
        assert!(!h.ok && h.required && h.source.is_none() && h.why == "handover의 슬랙 확인에 필요", "{h:?}");
        let (l, _r, _a) = loaded(r#"{"envRequire":[]}"#);
        assert!(l.config.env_require.is_empty());
    }

    #[test]
    fn custom_required_item_cannot_be_skipped_in_check_all() {
        // a hand-edited envSkip naming a required item has no effect
        let (l, _r, _a) = loaded(r#"{"envSkip":["holla"]}"#);
        let ws = Workspace::from_config(&l);
        let it = check_all(&l, &ws, &Fake { checks: [("holla slack version".to_string(), CheckOutcome::Exit(1))].into(), ..Default::default() });
        let h = item(&it, "holla");
        assert!(h.required && !h.skipped && !h.ok);
    }

    #[test]
    fn env_require_validation_is_located() {
        let root = tempfile::tempdir().unwrap();
        let f = root.path().join("config.json");
        let text = r#"{"envSkip":["a", 3],"envRequire":[
            {"id":"ok1","check":"x"},
            {"id":"git-bash","check":"x"},
            {"id":"bad id","check":"x"},
            {"id":"ok1","check":"y"},
            {"id":"nocheck"},
            {"id":"s","check":"x","source":"ftp://h/x"},
            {"id":"u","check":"x","what":1},
            {"id":"r","check":"x","required":"yes"},
            5]}"#;
        let (c, errs) = crate::config::parse_config(text, &f).unwrap();
        let paths: Vec<String> = errs.iter().map(|e| e.json_path.clone().unwrap()).collect();
        assert_eq!(
            paths,
            [
                "envSkip[1]", "envRequire[1].id", "envRequire[2].id", "envRequire[3].id", "envRequire[4].check",
                "envRequire[5].source", "envRequire[6].what", "envRequire[7].required", "envRequire[8]"
            ]
        );
        assert_eq!(c.env_skip, ["a"]);
        assert_eq!(c.env_require.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ok1"]);
    }

    #[test]
    fn custom_check_timeout_is_failure() {
        // pure: a timed-out check is a failing item
        let (l, _r, _a) = loaded(r#"{"envRequire":[{"id":"slow","check":"slow","required":true}]}"#);
        let ws = Workspace::from_config(&l);
        let it = check_all(&l, &ws, &Fake { checks: [("slow".to_string(), CheckOutcome::Timeout)].into(), ..Default::default() });
        let s = item(&it, "slow");
        assert!(!s.ok && s.detail.contains("시간 초과(10초)"), "{s:?}");

        // real: the shell and its child are killed at the timeout
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("late.txt");
        let cmd = format!("cmd /d /c \"ping -n 4 127.0.0.1 >nul & echo late> {}\"", marker.display());
        let t = Instant::now();
        let r = run_shell(&cmd, Duration::from_millis(800));
        assert_eq!(r.outcome, CheckOutcome::Timeout);
        assert!(t.elapsed() < Duration::from_secs(3), "returned at the timeout: {:?}", t.elapsed());
        std::thread::sleep(Duration::from_secs(5));
        assert!(!marker.exists(), "the grandchild survived the timeout");

        // output past the capture cap is drained, not mistaken for a hang
        let big = dir.path().join("big.txt");
        std::fs::write(&big, "x".repeat(300 * 1024)).unwrap();
        let t = Instant::now();
        let r = run_shell(&format!("type \"{}\"", big.display()), CHECK_TIMEOUT);
        assert_eq!(r.outcome, CheckOutcome::Ok);
        assert_eq!(r.stdout.len() as u64, OUTPUT_CAP);
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());

        // exit codes
        assert_eq!(run_shell("exit 0", CHECK_TIMEOUT).outcome, CheckOutcome::Ok);
        assert_eq!(run_shell("exit 3", CHECK_TIMEOUT).outcome, CheckOutcome::Exit(3));
        let r = run_shell("echo 2.1.310 (Claude Code)", CHECK_TIMEOUT);
        assert_eq!((r.outcome, parse_version(&r.stdout)), (CheckOutcome::Ok, Some((2, 1, 310))));
        assert!(matches!(run_shell("this-command-does-not-exist-xyz", CHECK_TIMEOUT).outcome, CheckOutcome::Exit(_)));
    }

    #[test]
    fn git_bash_follows_claude_code_rule() {
        let dir = tempfile::tempdir().unwrap();
        let git_root = dir.path().join("Git");
        std::fs::create_dir_all(git_root.join("cmd")).unwrap();
        std::fs::create_dir_all(git_root.join("bin")).unwrap();
        let git = git_root.join("cmd").join("git.exe");
        std::fs::write(&git, "").unwrap();
        // git on PATH but no bash
        assert!(find_git_bash(None, Some(git.clone())).unwrap_err().contains("bash.exe"));
        std::fs::write(git_root.join("bin").join("bash.exe"), "").unwrap();
        assert_eq!(find_git_bash(None, Some(git.clone())).unwrap(), git_root.join("bin").join("bash.exe"));
        // mingw64\bin\git.exe layout
        let deep = git_root.join("mingw64").join("bin").join("git.exe");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        std::fs::write(&deep, "").unwrap();
        assert_eq!(find_git_bash(None, Some(deep)).unwrap(), git_root.join("bin").join("bash.exe"));
        // no git
        assert!(find_git_bash(None, None).is_err());
        // env var wins, and must exist
        let custom = dir.path().join("bash.exe");
        std::fs::write(&custom, "").unwrap();
        assert_eq!(find_git_bash(Some(custom.as_os_str()), None).unwrap(), custom);
        let missing = dir.path().join("nope.exe");
        assert!(find_git_bash(Some(missing.as_os_str()), Some(git)).unwrap_err().contains("CLAUDE_CODE_GIT_BASH_PATH"));
    }
}
