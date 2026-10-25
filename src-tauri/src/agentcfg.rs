//! Role session launch configuration (phase 2, spec §3/§10): the extra `claude` arguments of a
//! role pane, its run files (`%APPDATA%/agentboard/run/<paneId>.{mcp,settings}.json`) and the
//! per-pane bearer tokens the control server authenticates with.
//!
//! Shapes follow docs/spikes.md "2단계 스파이크" (P1 settings + hook command quoting, P3
//! `--mcp-config` HTTP + `headers.Authorization`). The run files hold the pane's bearer token:
//! never log their contents.

use crate::config::Loaded;
use crate::roles::Role;
use crate::workspace::{Pane, PaneId};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Used until the control server (Task 5) stores its real URL in `AppState::ctrl_url`:
/// role panes then start with a URL nothing listens on (hooks / MCP calls fail quietly).
pub const PLACEHOLDER_CTRL_URL: &str = "http://127.0.0.1:0";

/// Hook events the app registers in every role session's settings file. `UserPromptSubmit`
/// is there for Task 7 (session busy signal); the brief's minimum is SessionStart /
/// Notification / Stop.
pub const HOOK_EVENTS: [&str; 4] = ["SessionStart", "UserPromptSubmit", "Stop", "Notification"];

/// MCP server key in the `--mcp-config` file: tools appear as `mcp__agentboard__<tool>`.
pub const MCP_SERVER_KEY: &str = "agentboard";

/// Embedded fallback role instructions (used when `roles.<role>.systemPromptFile` is not set
/// or the file does not exist).
pub fn default_role_prompt(role: Role) -> &'static str {
    match role {
        Role::Dispatcher => include_str!("../defaults/roles/dispatcher.md"),
        Role::Handover => include_str!("../defaults/roles/handover.md"),
        Role::Manager => include_str!("../defaults/roles/manager.md"),
        Role::Lead => include_str!("../defaults/roles/lead.md"),
        Role::Member => include_str!("../defaults/roles/member.md"),
    }
}

// ───────────────────────── tokens ─────────────────────────

/// Per-pane bearer tokens (32 random bytes, hex). One live token per pane: `issue` replaces
/// the previous one (every spawn gets a fresh token), `revoke` forgets it. The inner map lock
/// is a leaf (nothing is acquired while holding it).
#[derive(Default)]
pub struct Tokens {
    by_token: Mutex<HashMap<String, PaneId>>,
}

impl Tokens {
    pub fn new() -> Self {
        Self::default()
    }

    /// New token for `pane`; the pane's older token stops resolving.
    pub fn issue(&self, pane: &str) -> String {
        let token = random_hex32();
        let mut m = self.by_token.lock().unwrap();
        m.retain(|_, p| p != pane);
        m.insert(token.clone(), pane.to_string());
        token
    }

    pub fn resolve(&self, token: &str) -> Option<PaneId> {
        self.by_token.lock().unwrap().get(token).cloned()
    }

    pub fn revoke(&self, pane: &str) {
        self.by_token.lock().unwrap().retain(|_, p| p != pane);
    }
}

/// 32 bytes from the OS RNG as 64 lowercase hex digits.
fn random_hex32() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("OS random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ───────────────────────── run files ─────────────────────────

/// `%APPDATA%/agentboard/run`.
pub fn default_run_dir() -> PathBuf {
    crate::appdata_dir().join("run")
}

pub fn mcp_file(run_dir: &Path, pane: &str) -> PathBuf {
    run_dir.join(format!("{pane}.mcp.json"))
}

pub fn settings_file(run_dir: &Path, pane: &str) -> PathBuf {
    run_dir.join(format!("{pane}.settings.json"))
}

/// Delete a pane's run files (they contain its token). Missing files are fine.
pub fn remove_run_files(run_dir: &Path, pane: &str) {
    let _ = std::fs::remove_file(mcp_file(run_dir, pane));
    let _ = std::fs::remove_file(settings_file(run_dir, pane));
}

/// `{"mcpServers":{"agentboard":{"type":"http","url":"<ctrl>/mcp","headers":{"Authorization":"Bearer <token>"}}}}`
pub fn mcp_config_json(ctrl_url: &str, token: &str) -> serde_json::Value {
    json!({
        "mcpServers": {
            MCP_SERVER_KEY: {
                "type": "http",
                "url": format!("{}/mcp", ctrl_url.trim_end_matches('/')),
                "headers": { "Authorization": format!("Bearer {token}") }
            }
        }
    })
}

/// Shell-form hook command (Git Bash): `"C:/path with spaces/agentboard.exe" --hook <event>`.
pub fn hook_command(exe: &Path, event: &str) -> String {
    format!("\"{}\" --hook {}", exe.to_string_lossy().replace('\\', "/"), event)
}

/// `{"crossSessionInbound":"accept","hooks":{"<Event>":[{"hooks":[{"type":"command","command":…}]}],…}}`
pub fn settings_json(exe: &Path) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for ev in HOOK_EVENTS {
        hooks.insert(ev.to_string(), json!([{ "hooks": [{ "type": "command", "command": hook_command(exe, ev) }] }]));
    }
    json!({ "crossSessionInbound": "accept", "hooks": hooks })
}

/// Role instructions: `<workspaceRoot>/.agentboard/<roles.<role>.systemPromptFile>` when set
/// and present, else the embedded default. A file that exists but cannot be read is an error.
pub fn role_prompt(role: Role, cfg: &Loaded) -> Result<String, String> {
    let Some(rel) = cfg.config.role_config(role).system_prompt_file else {
        return Ok(default_role_prompt(role).to_string());
    };
    let path = cfg.settings.workspace_root.join(".agentboard").join(&rel);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default_role_prompt(role).to_string()),
        Err(e) => Err(format!("역할 지시 파일을 읽을 수 없습니다: {} — {e}", path.display())),
    }
}

/// Whether `cmd` is a batch shim (`.cmd`/`.bat`, e.g. an npm-installed `claude.cmd`):
/// CreateProcess runs it through cmd.exe, which ends the command line at the first newline.
pub fn is_batch(cmd: &Path) -> bool {
    cmd.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// Safe form of a free-text argument for batch shims (R18/R19): each line break becomes
/// ` ⏎ ` (cmd.exe ends the command line at a newline), then cmd.exe metacharacters are
/// replaced by look-alikes — `"` would end the quoting early and make `& | < > ^` live,
/// `%VAR%` expands even inside quotes. Native executables get the text unchanged.
pub fn single_line(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace(['\n', '\r'], " ⏎ ")
        .chars()
        .map(|c| match c {
            '"' => '\'',
            '%' => '％',
            '^' => '＾',
            '&' => '＆',
            '|' => '｜',
            '<' => '＜',
            '>' => '＞',
            c => c,
        })
        .collect()
}

/// Variables Claude Code sets for one running session / its subprocesses (R19). A role
/// session started from an app that itself runs under Claude Code (dev) must not inherit
/// them — e.g. a stale `CLAUDE_CODE_MESSAGING_SOCKET/TOKEN` would point at the parent
/// session. Everything else (user configuration such as `CLAUDE_CODE_GIT_BASH_PATH`,
/// `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_OAUTH_TOKEN`) is inherited.
/// Sources: code.claude.com/docs/en/env-vars ("set by Claude Code" / "set automatically")
/// and the phase-2 spike's hook environment dump.
pub const SESSION_BOUND_VARS: [&str; 12] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_JOB_DIR",
];

/// The names in `vars` (the app's environment) that are in [`SESSION_BOUND_VARS`]
/// (case-insensitive, as Windows env names are), sorted, deduplicated.
pub fn inherited_claude_vars(vars: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = vars
        .into_iter()
        .filter(|k| SESSION_BOUND_VARS.iter().any(|s| s.eq_ignore_ascii_case(k)))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Write `contents` next to `path` and rename over it (readers never see a partial file).
fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| format!("실행 파일을 쓸 수 없습니다: {} — {e}", path.display()))
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Extra arguments and environment of a role pane; writes its two run files.
/// Args: `--name <session>`, `--append-system-prompt <role instructions>`,
/// `--mcp-config <run>/<pane>.mcp.json`, `--settings <run>/<pane>.settings.json`.
/// Env: `AGENTBOARD_URL=<ctrl_url>`, `AGENTBOARD_TOKEN=<token>` (read by the hook CLI).
/// Hooks run this executable (`std::env::current_exe`).
pub fn role_spawn_extras(
    pane: &Pane,
    role: Role,
    cfg: &Loaded,
    ctrl_url: &str,
    token: &str,
    run_dir: &Path,
) -> Result<(Vec<String>, Vec<(String, String)>), String> {
    let exe = std::env::current_exe().map_err(|e| format!("실행 파일 경로를 알 수 없습니다: {e}"))?;
    role_spawn_extras_with_exe(pane, role, cfg, ctrl_url, token, run_dir, &exe)
}

/// [`role_spawn_extras`] with an explicit hook executable (tests).
pub fn role_spawn_extras_with_exe(
    pane: &Pane,
    role: Role,
    cfg: &Loaded,
    ctrl_url: &str,
    token: &str,
    run_dir: &Path,
    exe: &Path,
) -> Result<(Vec<String>, Vec<(String, String)>), String> {
    if pane.id.is_empty() || !pane.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("창 id를 파일 이름으로 쓸 수 없습니다: {}", pane.id));
    }
    let name = pane
        .session_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("역할 창에 세션 이름이 없습니다: {}", pane.name))?;
    let prompt = role_prompt(role, cfg)?;
    std::fs::create_dir_all(run_dir)
        .map_err(|e| format!("실행 폴더를 만들 수 없습니다: {} — {e}", run_dir.display()))?;
    let mcp = mcp_file(run_dir, &pane.id);
    let settings = settings_file(run_dir, &pane.id);
    write_atomic(&mcp, &serde_json::to_string_pretty(&mcp_config_json(ctrl_url, token)).unwrap())?;
    write_atomic(&settings, &serde_json::to_string_pretty(&settings_json(exe)).unwrap())?;
    // R18: the multi-line prompt goes LAST — if a `.cmd` shim cuts the command line at a
    // newline, the run-file flags are already in (plan_spawn also flattens it for .cmd/.bat).
    let args = vec![
        "--name".to_string(),
        name,
        "--settings".to_string(),
        slash(&settings),
        "--mcp-config".to_string(),
        slash(&mcp),
        "--append-system-prompt".to_string(),
        prompt,
    ];
    let env = vec![
        ("AGENTBOARD_URL".to_string(), ctrl_url.to_string()),
        ("AGENTBOARD_TOKEN".to_string(), token.to_string()),
    ];
    Ok((args, env))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};
    use crate::workspace::{PaneKind, PaneStatus};
    use std::collections::BTreeMap;

    pub(crate) fn loaded_with(config: &str) -> (Loaded, tempfile::TempDir, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), config).unwrap();
        let loaded = load_all(appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        (loaded, root, appdata)
    }

    fn role_pane(id: &str, role: Role, session: &str) -> Pane {
        Pane {
            id: id.into(),
            name: "main".into(),
            tab: "t1".into(),
            agent: "claude".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: PathBuf::from("."),
            prompt: None,
            status: PaneStatus::Starting,
            attention: false,
            prompt_warning: false,
            attention_reason: None,
            kind: PaneKind::Terminal,
            role: Some(role),
            team: None,
            session_name: Some(session.into()),
            doc_path: None,
        }
    }

    fn read_json(p: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn extras_have_four_flags_and_env() {
        let (loaded, _r, _a) = loaded_with(r#"{"defaultAgent":"cmd"}"#);
        let run = tempfile::tempdir().unwrap();
        let p = role_pane("p7", Role::Manager, "billing");
        let exe = Path::new(r"C:\Program Files\AgentBoard\agentboard.exe");
        let (args, env) =
            role_spawn_extras_with_exe(&p, Role::Manager, &loaded, "http://127.0.0.1:5555", "tok123", run.path(), exe)
                .unwrap();
        let mcp = slash(&run.path().join("p7.mcp.json"));
        let settings = slash(&run.path().join("p7.settings.json"));
        assert_eq!(args.len(), 8);
        assert_eq!(args[0], "--name");
        assert_eq!(args[1], "billing");
        assert_eq!(&args[2..6], &["--settings".to_string(), settings, "--mcp-config".to_string(), mcp]);
        assert_eq!(args[6], "--append-system-prompt", "the multi-line prompt is last (R18)");
        assert_eq!(args[7], default_role_prompt(Role::Manager));
        assert!(args[7].contains('\n'), "unflattened here; plan_spawn flattens only for .cmd/.bat");
        assert_eq!(
            env,
            vec![
                ("AGENTBOARD_URL".to_string(), "http://127.0.0.1:5555".to_string()),
                ("AGENTBOARD_TOKEN".to_string(), "tok123".to_string()),
            ]
        );
    }

    #[test]
    fn run_files_have_auth_header_hooks_and_inbound_accept() {
        let (loaded, _r, _a) = loaded_with(r#"{"defaultAgent":"cmd"}"#);
        let run = tempfile::tempdir().unwrap();
        let sub = run.path().join("run"); // created on demand
        let p = role_pane("p3", Role::Lead, "billing-batch-lead");
        let exe = Path::new(r"C:\Program Files\AgentBoard\agentboard.exe");
        role_spawn_extras_with_exe(&p, Role::Lead, &loaded, "http://127.0.0.1:5555/", "abc", &sub, exe).unwrap();
        assert_eq!(
            read_json(&sub.join("p3.mcp.json")),
            json!({"mcpServers":{"agentboard":{"type":"http","url":"http://127.0.0.1:5555/mcp",
                    "headers":{"Authorization":"Bearer abc"}}}})
        );
        let s = read_json(&sub.join("p3.settings.json"));
        assert_eq!(s["crossSessionInbound"], "accept");
        for ev in ["SessionStart", "Notification", "Stop", "UserPromptSubmit"] {
            assert_eq!(
                s["hooks"][ev],
                json!([{"hooks":[{"type":"command",
                    "command": format!("\"C:/Program Files/AgentBoard/agentboard.exe\" --hook {ev}")}]}]),
                "{ev}"
            );
        }
        assert_eq!(s["hooks"].as_object().unwrap().len(), 4);
        // reissue overwrites in place with the new token
        role_spawn_extras_with_exe(&p, Role::Lead, &loaded, "http://127.0.0.1:5555", "def", &sub, exe).unwrap();
        assert_eq!(read_json(&sub.join("p3.mcp.json"))["mcpServers"]["agentboard"]["headers"]["Authorization"], "Bearer def");
        remove_run_files(&sub, "p3");
        assert!(!sub.join("p3.mcp.json").exists() && !sub.join("p3.settings.json").exists());
    }

    #[test]
    fn role_prompt_from_file_or_default() {
        let (loaded, root, _a) = loaded_with(
            r#"{"defaultAgent":"cmd","roles":{"manager":{"systemPromptFile":"roles/manager.md"},
                "lead":{"systemPromptFile":"roles/missing.md"}}}"#,
        );
        std::fs::create_dir_all(root.path().join(".agentboard/roles")).unwrap();
        std::fs::write(root.path().join(".agentboard/roles/manager.md"), "사용자 지시").unwrap();
        assert_eq!(role_prompt(Role::Manager, &loaded).unwrap(), "사용자 지시");
        assert_eq!(role_prompt(Role::Lead, &loaded).unwrap(), default_role_prompt(Role::Lead));
        assert_eq!(role_prompt(Role::Member, &loaded).unwrap(), default_role_prompt(Role::Member));
        for r in Role::ALL {
            assert!(!default_role_prompt(r).trim().is_empty());
        }
    }

    #[test]
    fn extras_reject_missing_session_name_and_odd_ids() {
        let (loaded, _r, _a) = loaded_with(r#"{"defaultAgent":"cmd"}"#);
        let run = tempfile::tempdir().unwrap();
        let exe = Path::new("C:/a/agentboard.exe");
        let mut p = role_pane("p1", Role::Member, "x");
        p.session_name = None;
        assert!(role_spawn_extras_with_exe(&p, Role::Member, &loaded, "u", "t", run.path(), exe).is_err());
        let p = role_pane("../evil", Role::Member, "x");
        assert!(role_spawn_extras_with_exe(&p, Role::Member, &loaded, "u", "t", run.path(), exe).is_err());
    }

    #[test]
    fn batch_detection_and_single_line() {
        assert!(is_batch(Path::new(r"C:\npm\claude.cmd")) && is_batch(Path::new("C:/x/run.BAT")));
        assert!(!is_batch(Path::new(r"C:\bin\claude.exe")) && !is_batch(Path::new("claude")));
        assert_eq!(single_line("a\nb\r\nc\rd"), "a ⏎ b ⏎ c ⏎ d");
        assert_eq!(single_line("one"), "one");
        let nasty = "say \"hi\" & echo %PATH% | more < in > out ^x\nnext";
        let safe = single_line(nasty);
        assert_eq!(safe, "say 'hi' ＆ echo ％PATH％ ｜ more ＜ in ＞ out ＾x ⏎ next");
        for c in ['"', '%', '^', '&', '|', '<', '>', '\n', '\r'] {
            assert!(!safe.contains(c), "{c:?} left in {safe}");
        }
    }

    #[test]
    fn only_session_bound_vars_are_dropped() {
        let names = [
            "PATH",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SSE_PORT",
            "claude_code_messaging_socket",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_SESSION_ID",
            // user configuration: inherited
            "CLAUDE_CODE_GIT_BASH_PATH",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
            "AGENTBOARD_URL",
        ];
        assert_eq!(
            inherited_claude_vars(names.iter().map(|s| s.to_string())),
            vec![
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_MESSAGING_TOKEN",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_CODE_SSE_PORT",
                "claude_code_messaging_socket",
            ]
        );
        let all = inherited_claude_vars(SESSION_BOUND_VARS.iter().map(|s| s.to_string()));
        assert_eq!(all.len(), SESSION_BOUND_VARS.len());
    }

    #[test]
    fn token_reissue_invalidates_old_token() {
        let t = Tokens::new();
        let a1 = t.issue("p1");
        let b = t.issue("p2");
        assert_eq!(a1.len(), 64);
        assert!(a1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a1, b);
        assert_eq!(t.resolve(&a1).as_deref(), Some("p1"));
        let a2 = t.issue("p1");
        assert_ne!(a1, a2);
        assert_eq!(t.resolve(&a1), None, "old token no longer resolves");
        assert_eq!(t.resolve(&a2).as_deref(), Some("p1"));
        t.revoke("p1");
        assert_eq!(t.resolve(&a2), None);
        assert_eq!(t.resolve(&b).as_deref(), Some("p2"), "other panes keep theirs");
        assert_eq!(t.resolve(""), None);
    }
}
