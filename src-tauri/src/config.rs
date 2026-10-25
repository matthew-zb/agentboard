use crate::layout::{Dir, LayoutNode};
use crate::roles::{Role, RoleConfig, DEFAULT_MAX_PER_TEAM};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_TEMPLATES: [(&str, &str); 3] = [
    ("dispatcher.json", include_str!("../defaults/templates/dispatcher.json")),
    ("solo.json", include_str!("../defaults/templates/solo.json")),
    ("team-3.json", include_str!("../defaults/templates/team-3.json")),
];
const DEFAULT_AGENTS: [(&str, &str); 2] = [
    ("claude.json", include_str!("../defaults/agents/claude.json")),
    ("cmd.json", include_str!("../defaults/agents/cmd.json")),
];
const DEFAULT_KEYBINDINGS: &str = include_str!("../defaults/keybindings.json");

// ───────────────────────── errors ─────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError {
    pub file: PathBuf,
    pub line: usize,
    pub col: usize,
    pub json_path: Option<String>,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}:{} — {}", self.file.display(), self.line, self.col, self.message)
        } else if let Some(p) = &self.json_path {
            write!(f, "{} — {}: {}", self.file.display(), p, self.message)
        } else {
            write!(f, "{} — {}", self.file.display(), self.message)
        }
    }
}

pub(crate) fn sem_err(file: &Path, path: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError { file: file.to_path_buf(), line: 0, col: 0, json_path: Some(path.into()), message: message.into() }
}

fn io_err(file: &Path, e: &std::io::Error) -> ConfigError {
    ConfigError { file: file.to_path_buf(), line: 0, col: 0, json_path: None, message: format!("파일을 읽을 수 없음: {e}") }
}

/// Located error from a serde_json failure (syntax or type).
pub fn located_json_error(file: &Path, e: &serde_json::Error) -> ConfigError {
    let full = e.to_string();
    let msg = match full.rfind(" at line ") {
        Some(i) => &full[..i],
        None => &full,
    };
    ConfigError {
        file: file.to_path_buf(),
        line: e.line().max(1),
        col: e.column().max(1),
        json_path: None,
        message: format!("JSON 오류: {msg}"),
    }
}

// ───────────────────────── settings ─────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub workspace_root: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { workspace_root: PathBuf::from("C:/workspace") }
    }
}

fn load_settings_checked(appdata_dir: &Path) -> (Settings, Option<ConfigError>) {
    let file = appdata_dir.join("settings.json");
    match std::fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(located_json_error(&file, &e))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let s = Settings::default();
            let _ = std::fs::create_dir_all(appdata_dir);
            if let Ok(json) = serde_json::to_string_pretty(&s) {
                let _ = std::fs::write(&file, json + "\n");
            }
            (s, None)
        }
        Err(e) => (Settings::default(), Some(io_err(&file, &e))),
    }
}

pub fn load_settings(appdata_dir: &Path) -> Settings {
    load_settings_checked(appdata_dir).0
}

// ───────────────────────── templates ─────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaneSpec {
    pub name: String,
    /// `Some(kind)` for an applet node (`{"applet":{"name","kind"}}`): an app screen, no process.
    pub applet: Option<String>,
    pub agent: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub prompt: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub layout: LayoutNode<PaneSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPane {
    name: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    focus: bool,
}

/// `{"applet":{...}}`. agent/args/env/cwd/prompt are accepted here only so `validate_template`
/// can reject them with a precise message.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApplet {
    name: String,
    kind: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    focus: bool,
}

/// Applet kinds the frontend knows.
pub const APPLET_KINDS: [&str; 1] = ["inbox"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    name: String,
    layout: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSplitRest {
    #[serde(default)]
    sizes: Option<Vec<f32>>,
    #[serde(default)]
    slot: Option<String>,
}

/// Percent sizes: equal when absent or mismatched, otherwise scaled to sum 100.
fn fit_sizes(sizes: Option<Vec<f32>>, n: usize) -> Vec<f32> {
    let equal = || vec![100.0 / n.max(1) as f32; n];
    match sizes {
        Some(s) if s.len() == n && s.iter().all(|x| *x > 0.0 && x.is_finite()) => {
            let sum: f32 = s.iter().sum();
            s.into_iter().map(|x| x * 100.0 / sum).collect()
        }
        _ => equal(),
    }
}

fn pane_spec(p: RawPane) -> PaneSpec {
    PaneSpec { name: p.name, applet: None, agent: p.agent, args: p.args, env: p.env, cwd: p.cwd, prompt: p.prompt, focus: p.focus }
}

fn applet_spec(a: RawApplet) -> PaneSpec {
    PaneSpec { name: a.name, applet: Some(a.kind), agent: a.agent, args: a.args, env: a.env, cwd: a.cwd, prompt: a.prompt, focus: a.focus }
}

/// Parse a layout node from JSON: a leaf (`{key: {...}}` for one of `keys`) or a split. Errors
/// carry a JSON path. `leaf` gets the leaf key, its value and its path.
fn parse_node<T>(
    v: &serde_json::Value,
    path: &str,
    keys: &[&str],
    file: &Path,
    leaf: &dyn Fn(&str, &serde_json::Value, &str) -> Result<T, ConfigError>,
) -> Result<LayoutNode<T>, ConfigError> {
    let Some(obj) = v.as_object() else {
        return Err(sem_err(file, path, "노드는 객체여야 함"));
    };
    let present: Vec<&str> = keys.iter().copied().filter(|k| obj.contains_key(*k)).collect();
    let label = keys.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join("/");
    match (present.as_slice(), obj.contains_key("split")) {
        ([_, ..], true) => Err(sem_err(file, path, format!("노드에 {label}와 'split'을 함께 쓸 수 없음"))),
        ([], false) => Err(sem_err(file, path, format!("노드에는 {label} 또는 'split'이 필요함"))),
        ([_, _, ..], false) => Err(sem_err(file, path, format!("노드에 {label} 중 하나만 쓸 수 있음"))),
        ([key], false) => {
            if let Some(extra) = obj.keys().find(|k| k.as_str() != *key) {
                return Err(sem_err(file, path, format!("알 수 없는 필드 '{extra}'")));
            }
            leaf(key, &obj[*key], &format!("{path}.{key}")).map(LayoutNode::Leaf)
        }
        ([], true) => {
            let dir: Dir = serde_json::from_value(obj["split"].clone()).map_err(|e| {
                sem_err(file, format!("{path}.split"), format!("split은 \"horizontal\" 또는 \"vertical\"이어야 함 ({e})"))
            })?;
            let mut rest = obj.clone();
            rest.remove("split");
            let children_v = rest.remove("children");
            let rest: RawSplitRest = serde_json::from_value(serde_json::Value::Object(rest))
                .map_err(|e| sem_err(file, path, format!("분할 노드 필드 오류: {e}")))?;
            let Some(serde_json::Value::Array(arr)) = children_v else {
                return Err(sem_err(file, format!("{path}.children"), "children은 배열이어야 함"));
            };
            let mut children = Vec::new();
            for (i, c) in arr.iter().enumerate() {
                children.push(parse_node(c, &format!("{path}.children[{i}]"), keys, file, leaf)?);
            }
            let sizes = fit_sizes(rest.sizes, children.len());
            Ok(LayoutNode::Split { dir, sizes, children, slot: rest.slot })
        }
    }
}

/// Parse a template file's text. Single-child splits are kept so `validate_template` can reject them.
pub fn parse_template(text: &str, file: &Path) -> Result<Template, ConfigError> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| located_json_error(file, &e))?;
    let raw: RawTemplate =
        serde_json::from_value(v).map_err(|e| sem_err(file, "$", format!("템플릿 형식 오류: {e}")))?;
    let layout = parse_node(&raw.layout, "layout", &["pane", "applet"], file, &|key, v, p| {
        if key == "applet" {
            serde_json::from_value::<RawApplet>(v.clone())
                .map(applet_spec)
                .map_err(|e| sem_err(file, p, format!("애플릿 정의 오류: {e}")))
        } else {
            serde_json::from_value::<RawPane>(v.clone())
                .map(pane_spec)
                .map_err(|e| sem_err(file, p, format!("창 정의 오류: {e}")))
        }
    })?;
    Ok(Template { name: raw.name, layout })
}

fn is_absolute_like(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/') || p.starts_with('\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// Like [`check_relative`], with the message naming `base` (e.g. `.agentboard 폴더`,
/// `workspaceRoot`) instead of the tab workspace.
pub(crate) fn check_relative_in(p: &str, base: &str) -> Option<String> {
    check_relative(p).map(|_| {
        if is_absolute_like(p) {
            format!("절대경로는 사용할 수 없음 ({base} 기준 상대경로만 허용)")
        } else {
            format!("{base} 밖으로 벗어나는 경로는 사용할 수 없음")
        }
    })
}

/// Returns an error message when `p` is not a relative path staying inside its base.
pub(crate) fn check_relative(p: &str) -> Option<&'static str> {
    if is_absolute_like(p) {
        return Some("절대경로는 사용할 수 없음 (탭 작업공간 기준 상대경로만 허용)");
    }
    let mut depth: i32 = 0;
    for c in p.split(['/', '\\']) {
        match c {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Some("작업공간 밖으로 벗어나는 경로는 사용할 수 없음");
                }
            }
            _ => depth += 1,
        }
    }
    None
}

pub fn validate_template(t: &Template, file: &Path, known_agents: &BTreeSet<String>) -> Vec<ConfigError> {
    let mut errs = Vec::new();
    if t.name.trim().is_empty() {
        errs.push(sem_err(file, "name", "템플릿 이름이 비어 있음"));
    }
    let mut seen = BTreeSet::new();
    let mut focus_count = 0;
    walk_template(&t.layout, "layout", file, known_agents, &mut seen, &mut focus_count, &mut errs);
    if focus_count > 1 {
        errs.push(sem_err(file, "layout", "focus는 하나의 창에만 지정할 수 있음"));
    }
    errs
}

fn walk_template(
    n: &LayoutNode<PaneSpec>,
    path: &str,
    file: &Path,
    agents: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    focus_count: &mut usize,
    errs: &mut Vec<ConfigError>,
) {
    match n {
        LayoutNode::Leaf(p) => {
            let pp = format!("{path}.{}", if p.applet.is_some() { "applet" } else { "pane" });
            if let Some(kind) = &p.applet {
                if !APPLET_KINDS.contains(&kind.as_str()) {
                    errs.push(sem_err(file, format!("{pp}.kind"), format!("알 수 없는 애플릿 종류 '{kind}' (inbox)")));
                }
                let given = [
                    ("agent", p.agent.is_some()),
                    ("args", !p.args.is_empty()),
                    ("env", !p.env.is_empty()),
                    ("cwd", p.cwd.is_some()),
                    ("prompt", p.prompt.is_some()),
                ];
                for (field, _) in given.iter().filter(|(_, set)| *set) {
                    errs.push(sem_err(file, format!("{pp}.{field}"), format!("애플릿 창에는 {field}을(를) 쓸 수 없음 (프로세스가 없음)")));
                }
            }
            if p.name.trim().is_empty() {
                errs.push(sem_err(file, format!("{pp}.name"), "창 이름이 비어 있음"));
            } else if !seen.insert(p.name.clone()) {
                errs.push(sem_err(file, format!("{pp}.name"), format!("창 이름 '{}'이(가) 중복됨", p.name)));
            }
            if let Some(a) = &p.agent {
                if !agents.contains(a) {
                    errs.push(sem_err(file, format!("{pp}.agent"), format!("알 수 없는 에이전트 '{a}'")));
                }
            }
            if let Some(m) = p.cwd.as_deref().and_then(check_relative) {
                errs.push(sem_err(file, format!("{pp}.cwd"), m));
            }
            if p.focus {
                *focus_count += 1;
            }
        }
        LayoutNode::Split { children, .. } => {
            if children.len() < 2 {
                errs.push(sem_err(file, format!("{path}.children"), "분할에는 자식이 2개 이상 필요함"));
            }
            for (i, c) in children.iter().enumerate() {
                walk_template(c, &format!("{path}.children[{i}]"), file, agents, seen, focus_count, errs);
            }
        }
    }
}

// ───────────────────────── config.json ─────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TabSpec {
    pub name: String,
    pub dir: String,
    pub template: Option<String>,
    /// `manager` | `handover` (the Dispatcher role is implied by `config.dispatcher`).
    pub role: Option<Role>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupSpec {
    pub name: String,
    pub tabs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatcherSpec {
    pub agent: Option<String>,
    pub template: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub dispatcher: DispatcherSpec,
    pub default_template: String,
    pub default_agent: String,
    pub groups: Option<LayoutNode<GroupSpec>>,
    pub default_group: Option<String>,
    pub tabs: Vec<TabSpec>,
    /// Per-role settings (`roles.<role>`); roles without an entry use defaults.
    pub roles: BTreeMap<Role, RoleConfig>,
    /// Nightly refresh time "HH:MM" (24h). Absent = "03:00"; explicit `null` = off.
    pub nightly_refresh: Option<String>,
    /// Minutes to wait for `handoff_done` before restarting anyway (>= 1).
    pub nightly_timeout_min: u32,
    /// The refresh is postponed while the user typed into the Dispatcher within this many
    /// minutes (0 = no grace period).
    pub nightly_quiet_min: u32,
    /// handover index document, relative to workspaceRoot (`None` = `HANDOVER.md`).
    pub handover_index: Option<String>,
    /// Environment check items the user chose to skip (`envSkip`, Task 12). Required custom
    /// items are never treated as skipped even when listed here.
    pub env_skip: Vec<String>,
    /// Custom environment check items (`envRequire`, Task 12). Absent key → [`default_env_require`]
    /// (the holla item); an explicit `[]` → none.
    pub env_require: Vec<EnvRequire>,
    /// Indices (in `tabs`) that `parse_config` already reported invalid; `sanitize_config`
    /// drops them silently so later JSON paths keep the file's numbering.
    pub(crate) rejected_tabs: BTreeSet<usize>,
}

pub const DEFAULT_NIGHTLY_REFRESH: &str = "03:00";
pub const DEFAULT_NIGHTLY_TIMEOUT_MIN: u32 = 20;
pub const DEFAULT_NIGHTLY_QUIET_MIN: u32 = 10;
pub const DEFAULT_HANDOVER_INDEX: &str = "HANDOVER.md";

impl Config {
    /// handover index path relative to workspaceRoot.
    pub fn handover_index_path(&self) -> &str {
        self.handover_index.as_deref().unwrap_or(DEFAULT_HANDOVER_INDEX)
    }

    /// `roles.member.maxPerTeam` (default 4).
    pub fn max_per_team(&self) -> usize {
        self.roles.get(&Role::Member).and_then(|r| r.max_per_team).unwrap_or(DEFAULT_MAX_PER_TEAM)
    }

    /// Settings of `role` (defaults when `roles` has no entry for it).
    pub fn role_config(&self, role: Role) -> RoleConfig {
        self.roles.get(&role).cloned().unwrap_or_default()
    }
}

/// "HH:MM", 00:00–23:59, two digits each.
pub fn valid_hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' || !b.iter().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit()) {
        return false;
    }
    let h = (b[0] - b'0') * 10 + (b[1] - b'0');
    let m = (b[3] - b'0') * 10 + (b[4] - b'0');
    h < 24 && m < 60
}

impl Default for Config {
    fn default() -> Self {
        Config {
            dispatcher: DispatcherSpec { agent: Some("claude".into()), template: "dispatcher".into() },
            default_template: "solo".into(),
            default_agent: "claude".into(),
            groups: None,
            default_group: None,
            tabs: Vec::new(),
            roles: BTreeMap::new(),
            nightly_refresh: Some(DEFAULT_NIGHTLY_REFRESH.into()),
            nightly_timeout_min: DEFAULT_NIGHTLY_TIMEOUT_MIN,
            nightly_quiet_min: DEFAULT_NIGHTLY_QUIET_MIN,
            handover_index: None,
            env_skip: Vec::new(),
            env_require: default_env_require(),
            rejected_tabs: BTreeSet::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGroup {
    name: String,
    #[serde(default)]
    tabs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTab {
    name: String,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDispatcher {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    template: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    dispatcher: Option<RawDispatcher>,
    #[serde(default)]
    default_template: Option<String>,
    #[serde(default)]
    default_agent: Option<String>,
    #[serde(default)]
    groups: Option<serde_json::Value>,
    #[serde(default)]
    default_group: Option<String>,
    #[serde(default)]
    tabs: Vec<RawTab>,
    #[serde(default)]
    roles: Option<serde_json::Value>,
    /// Outer `None` = absent (default time), `Some(None)` = explicit `null` (off).
    #[serde(default, deserialize_with = "present")]
    nightly_refresh: Option<Option<String>>,
    #[serde(default)]
    nightly_timeout_min: Option<u32>,
    #[serde(default)]
    nightly_quiet_min: Option<u32>,
    #[serde(default)]
    handover_index: Option<String>,
    #[serde(default)]
    env_skip: Option<serde_json::Value>,
    #[serde(default)]
    env_require: Option<serde_json::Value>,
}

/// Distinguishes an explicit `null` from an absent field (used with `#[serde(default)]`).
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// Returns the parsed config plus errors for parts that were dropped (currently: a malformed
/// `groups` layout). Syntax/type errors of the file itself are `Err` (located line:col).
pub fn parse_config(text: &str, file: &Path) -> Result<(Config, Vec<ConfigError>), ConfigError> {
    let raw: RawConfig = serde_json::from_str(text).map_err(|e| located_json_error(file, &e))?;
    let d = Config::default();
    let mut errs = Vec::new();
    let groups = match raw.groups {
        None => None,
        Some(v) => match parse_node(&v, "groups", &["group"], file, &|_, v, p| {
            serde_json::from_value::<RawGroup>(v.clone())
                .map(|g| GroupSpec { name: g.name, tabs: g.tabs })
                .map_err(|e| sem_err(file, p, format!("그룹 정의 오류: {e}")))
        }) {
            Ok(g) => Some(g),
            Err(e) => {
                errs.push(e);
                None
            }
        },
    };
    let disp = raw.dispatcher.unwrap_or(RawDispatcher { agent: None, template: None });
    let roles = match &raw.roles {
        None => BTreeMap::new(),
        Some(v) => {
            let (r, e) = crate::roles::parse_roles(v, file);
            errs.extend(e);
            r
        }
    };
    let nightly_refresh = match raw.nightly_refresh {
        None => d.nightly_refresh.clone(),
        Some(None) => None,
        Some(Some(t)) if valid_hhmm(&t) => Some(t),
        Some(Some(t)) => {
            errs.push(sem_err(
                file,
                "nightlyRefresh",
                format!("시각 '{t}'이(가) 올바르지 않음 (\"HH:MM\" 24시간 형식) — 기본값 {DEFAULT_NIGHTLY_REFRESH} 사용"),
            ));
            d.nightly_refresh.clone()
        }
    };
    let nightly_timeout_min = match raw.nightly_timeout_min {
        Some(0) => {
            errs.push(sem_err(file, "nightlyTimeoutMin", format!("1 이상이어야 함 — 기본값 {DEFAULT_NIGHTLY_TIMEOUT_MIN} 사용")));
            DEFAULT_NIGHTLY_TIMEOUT_MIN
        }
        Some(n) => n,
        None => DEFAULT_NIGHTLY_TIMEOUT_MIN,
    };
    let handover_index = match raw.handover_index {
        Some(h) if h.trim().is_empty() => {
            errs.push(sem_err(file, "handoverIndex", format!("경로가 비어 있음 — 기본값 {DEFAULT_HANDOVER_INDEX} 사용")));
            None
        }
        Some(h) => match check_relative_in(&h, "workspaceRoot") {
            Some(m) => {
                errs.push(sem_err(file, "handoverIndex", format!("{m} — 기본값 {DEFAULT_HANDOVER_INDEX} 사용")));
                None
            }
            None => Some(h),
        },
        None => None,
    };
    // tabs[].role: manager | handover only. A bad role drops the tab (reported here, skipped by
    // sanitize_config so the remaining JSON paths keep the file's indices).
    let mut rejected_tabs = BTreeSet::new();
    let mut tabs = Vec::new();
    for (i, t) in raw.tabs.into_iter().enumerate() {
        let role = match t.role.as_deref() {
            None => None,
            Some("manager") => Some(Role::Manager),
            Some("handover") => Some(Role::Handover),
            Some(other) => {
                let why = if other == "dispatcher" {
                    "dispatcher 역할은 config.dispatcher로 정함".to_string()
                } else {
                    format!("탭 역할 '{other}'은(는) 쓸 수 없음")
                };
                errs.push(sem_err(file, format!("tabs[{i}].role"), format!("{why} — tabs[].role은 \"manager\" 또는 \"handover\"만 가능")));
                rejected_tabs.insert(i);
                None
            }
        };
        tabs.push(TabSpec { name: t.name, dir: t.dir, template: t.template, role });
    }
    let env_skip = match &raw.env_skip {
        None => Vec::new(),
        Some(v) => parse_env_skip(v, file, &mut errs),
    };
    let env_require = match &raw.env_require {
        None => default_env_require(),
        Some(v) => parse_env_require(v, file, &mut errs),
    };
    let cfg = Config {
        dispatcher: DispatcherSpec {
            agent: disp.agent.or(d.dispatcher.agent),
            template: disp.template.unwrap_or(d.dispatcher.template),
        },
        default_template: raw.default_template.unwrap_or(d.default_template),
        default_agent: raw.default_agent.unwrap_or(d.default_agent),
        groups,
        default_group: raw.default_group,
        tabs,
        roles,
        nightly_refresh,
        nightly_timeout_min,
        nightly_quiet_min: raw.nightly_quiet_min.unwrap_or(DEFAULT_NIGHTLY_QUIET_MIN),
        handover_index,
        env_skip,
        env_require,
        rejected_tabs,
    };
    Ok((cfg, errs))
}

/// Drop only the offending items (bad tabs; an invalid group layout) and report each as an error.
pub fn sanitize_config(mut c: Config, file: &Path) -> (Config, Vec<ConfigError>) {
    let mut errs = Vec::new();
    let mut names = BTreeSet::new();
    let mut kept = Vec::new();
    let rejected = std::mem::take(&mut c.rejected_tabs);
    let mut handover_seen = false;
    for (i, t) in std::mem::take(&mut c.tabs).into_iter().enumerate() {
        if rejected.contains(&i) {
            continue; // already reported by parse_config
        }
        let p = format!("tabs[{i}]");
        let before = errs.len();
        if t.name.trim().is_empty() {
            errs.push(sem_err(file, format!("{p}.name"), "탭 이름이 비어 있음"));
        } else if t.name == "dispatcher" {
            errs.push(sem_err(file, format!("{p}.name"), "'dispatcher'는 예약된 탭 이름임"));
        } else if names.contains(&t.name) {
            errs.push(sem_err(file, format!("{p}.name"), format!("탭 이름 '{}'이(가) 중복됨", t.name)));
        }
        if let Some(m) = check_relative(&t.dir) {
            errs.push(sem_err(file, format!("{p}.dir"), m));
        }
        if t.role == Some(Role::Handover) {
            // spec §8 (R11): the handover workspace is workspaceRoot; at most one handover tab
            if !t.dir.is_empty() {
                errs.push(sem_err(file, format!("{p}.dir"), "handover 탭의 dir은 빈 값(\"\")이어야 함 — 작업공간은 workspaceRoot"));
            } else if handover_seen {
                errs.push(sem_err(file, format!("{p}.role"), "handover 탭은 하나만 둘 수 있음"));
            }
        }
        if errs.len() == before {
            handover_seen |= t.role == Some(Role::Handover);
            names.insert(t.name.clone());
            kept.push(t);
        }
    }
    c.tabs = kept;
    if let Some(g) = &c.groups {
        let mut group_names = BTreeSet::new();
        let mut placed = BTreeSet::new();
        let before = errs.len();
        walk_groups(g, "groups", file, &names, &mut group_names, &mut placed, &mut errs);
        if errs.len() != before {
            c.groups = None;
            c.default_group = None;
        } else if let Some(dg) = &c.default_group {
            if !group_names.contains(dg) {
                errs.push(sem_err(file, "defaultGroup", format!("그룹 '{dg}'이(가) groups에 없음")));
                c.default_group = None;
            }
        }
    }
    (c, errs)
}

/// Path of `config.json` under `workspace_root`.
pub fn config_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".agentboard").join("config.json")
}

/// Serializes every app-side config.json read-modify-write (Task 8 `create_manager_tab`; later
/// writers such as `env_skip` take it too) for the whole append → apply → rollback sequence, so
/// a rollback never undoes another call's entry. Outermost: taken before `ws` / `loaded`.
pub static CONFIG_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Outcome of [`append_config_tab`].
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigEdit {
    /// The file already had this exact tab entry; nothing was written.
    Unchanged,
    /// Written; the previous text (`None`: there was no file) for [`restore_config_file`].
    Written(Option<String>),
}

/// The `tabs[]` entry of `t` as written to config.json (`template` / `role` only when set).
pub fn tab_spec_json(t: &TabSpec) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), t.name.clone().into());
    m.insert("dir".into(), t.dir.clone().into());
    if let Some(r) = t.role {
        m.insert("role".into(), r.as_str().into());
    }
    if let Some(tpl) = &t.template {
        m.insert("template".into(), tpl.clone().into());
    }
    serde_json::Value::Object(m)
}

/// Append tab `t` to `tabs` of the config.json at `path` (`create_manager_tab`, Task 8). The
/// file is round-tripped as a JSON value — never through the typed `Config` — so every other key
/// the user wrote is kept, in its order; it is written atomically (`.tmp` + rename). An entry
/// with the same name that is identical is left alone (`Unchanged`, a retried call); a
/// different one is an error.
pub fn append_config_tab(path: &Path, t: &TabSpec) -> Result<ConfigEdit, String> {
    let prev = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("config.json을 읽을 수 없습니다 — {e}")),
    };
    let mut root: serde_json::Value = match &prev {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("config.json을 해석할 수 없어 탭을 추가하지 않았습니다 — {e}"))?,
        None => serde_json::json!({}),
    };
    let obj = root.as_object_mut().ok_or("config.json의 최상위가 객체가 아니어서 탭을 추가하지 않았습니다")?;
    let tabs = obj.entry("tabs").or_insert_with(|| serde_json::json!([]));
    let arr = tabs.as_array_mut().ok_or("config.json의 tabs가 배열이 아니어서 탭을 추가하지 않았습니다")?;
    let entry = tab_spec_json(t);
    if let Some(old) = arr.iter().find(|v| v.get("name").and_then(|n| n.as_str()) == Some(t.name.as_str())) {
        let same = old.get("dir").and_then(|d| d.as_str()) == Some(t.dir.as_str())
            && old.get("role").and_then(|r| r.as_str()) == t.role.map(|r| r.as_str());
        return if same { Ok(ConfigEdit::Unchanged) } else { Err(format!("config.json에 이름이 '{}'인 다른 탭이 이미 있습니다", t.name)) };
    }
    arr.push(entry);
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    text.push('\n');
    write_file_atomic(path, &text)?;
    Ok(ConfigEdit::Written(prev))
}

/// Undo an [`append_config_tab`] `Written(prev)`: put the previous text back (or remove the
/// file it created).
pub fn restore_config_file(path: &Path, prev: Option<&str>) -> Result<(), String> {
    match prev {
        Some(text) => write_file_atomic(path, text),
        None => std::fs::remove_file(path).map_err(|e| format!("config.json을 되돌릴 수 없습니다 — {e}")),
    }
}

/// Write `<path>.tmp` and rename it over `path` (readers never see a partial file).
fn write_file_atomic(path: &Path, text: &str) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("config.json을 쓸 수 없습니다 — {e}")
        })
}

// ───────────────────────── envRequire / envSkip (Task 12) ─────────────────────────

/// One custom environment check (`config.json` `envRequire[]`): `check` runs through the shell
/// (exit code 0 = fine, 10 s timeout, see `envcheck`). `source` is the repository URL the
/// dispatcher installs from (http(s) only; `None` = not known yet — the dispatcher asks the user
/// and records it with `env_set_source`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvRequire {
    pub id: String,
    pub check: String,
    pub fix: String,
    pub why: String,
    pub required: bool,
    pub source: Option<String>,
}

/// Built-in environment check ids (`envcheck`); custom ids may not reuse them.
pub const ENV_BUILTIN_IDS: [&str; 2] = ["claude-version", "git-bash"];
/// Prefix of the per-tab folder-trust item ids (`folder-trust:<tab name>`).
pub const ENV_TRUST_PREFIX: &str = "folder-trust:";
/// Longest custom id / check command / source URL (characters).
pub const ENV_ID_MAX: usize = 64;
pub const ENV_TEXT_MAX: usize = 1000;

/// The default `envRequire` (used when config.json has no `envRequire` key): holla, required by
/// handover's Slack checks. No default `source`: the repository may be private (ledger R12).
pub fn default_env_require() -> Vec<EnvRequire> {
    vec![EnvRequire {
        id: "holla".into(),
        check: "holla slack version".into(),
        fix: "source의 GitHub 저장소로 holla(@circlesac/holla)를 설치한 뒤 `holla slack auth`로 슬랙 로그인".into(),
        why: "handover의 슬랙 확인에 필요".into(),
        required: true,
        source: None,
    }]
}

/// `envRequire[]` entry as written to config.json (`source` always present, "" = unknown).
pub fn env_require_json(e: &EnvRequire) -> serde_json::Value {
    serde_json::json!({
        "id": e.id,
        "check": e.check,
        "fix": e.fix,
        "why": e.why,
        "required": e.required,
        "source": e.source.clone().unwrap_or_default(),
    })
}

/// A repository URL for `source` / `env_set_source`: `http://` or `https://`, a host, no
/// whitespace / control / quote characters, no credentials (`@` before the path), ≤ 1000 chars.
pub fn check_source_url(url: &str) -> Result<(), String> {
    if url.chars().count() > ENV_TEXT_MAX {
        return Err(format!("주소가 너무 깁니다(최대 {ENV_TEXT_MAX}자)"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '\\')) {
        return Err("주소에 공백·제어 문자·따옴표를 쓸 수 없습니다".into());
    }
    let lower = url.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return Err("http:// 또는 https:// 주소만 쓸 수 있습니다".into());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("주소에 호스트가 없습니다".into());
    }
    if authority.contains('@') {
        return Err("주소에 사용자 정보(@)를 넣을 수 없습니다".into());
    }
    Ok(())
}

fn parse_env_skip(v: &serde_json::Value, file: &Path, errs: &mut Vec<ConfigError>) -> Vec<String> {
    let Some(arr) = v.as_array() else {
        errs.push(sem_err(file, "envSkip", "문자열 배열이어야 함 — 무시함"));
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, x) in arr.iter().enumerate() {
        match x.as_str() {
            Some(s) if !s.trim().is_empty() => {
                if !out.iter().any(|o| o == s) {
                    out.push(s.to_string());
                }
            }
            _ => errs.push(sem_err(file, format!("envSkip[{i}]"), "비어 있지 않은 문자열이어야 함 — 무시함")),
        }
    }
    out
}

fn parse_env_require(v: &serde_json::Value, file: &Path, errs: &mut Vec<ConfigError>) -> Vec<EnvRequire> {
    let Some(arr) = v.as_array() else {
        errs.push(sem_err(file, "envRequire", "배열이어야 함 — 사용자 정의 점검 항목 없이 시작함"));
        return Vec::new();
    };
    let mut out: Vec<EnvRequire> = Vec::new();
    'items: for (i, x) in arr.iter().enumerate() {
        let p = format!("envRequire[{i}]");
        let Some(obj) = x.as_object() else {
            errs.push(sem_err(file, &p, "객체여야 함 — 이 항목은 무시함"));
            continue;
        };
        for k in obj.keys() {
            if !["id", "check", "fix", "why", "required", "source"].contains(&k.as_str()) {
                errs.push(sem_err(file, format!("{p}.{k}"), "알 수 없는 필드 — 이 항목은 무시함"));
                continue 'items;
            }
        }
        let text = |k: &str, req: bool| -> Result<String, String> {
            match obj.get(k) {
                None | Some(serde_json::Value::Null) if !req => Ok(String::new()),
                None | Some(serde_json::Value::Null) => Err("필수 필드가 없음".into()),
                Some(serde_json::Value::String(s)) => Ok(s.clone()),
                Some(_) => Err("문자열이어야 함".into()),
            }
        };
        let mut fields = BTreeMap::new();
        for (k, req) in [("id", true), ("check", true), ("fix", false), ("why", false), ("source", false)] {
            match text(k, req) {
                Ok(s) if s.chars().any(char::is_control) => {
                    errs.push(sem_err(file, format!("{p}.{k}"), "제어 문자(줄바꿈 등)를 쓸 수 없음 — 이 항목은 무시함"));
                    continue 'items;
                }
                Ok(s) if s.chars().count() > ENV_TEXT_MAX => {
                    errs.push(sem_err(file, format!("{p}.{k}"), format!("너무 김(최대 {ENV_TEXT_MAX}자) — 이 항목은 무시함")));
                    continue 'items;
                }
                Ok(s) => {
                    fields.insert(k, s);
                }
                Err(m) => {
                    errs.push(sem_err(file, format!("{p}.{k}"), format!("{m} — 이 항목은 무시함")));
                    continue 'items;
                }
            }
        }
        let required = match obj.get("required") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => {
                errs.push(sem_err(file, format!("{p}.required"), "true 또는 false여야 함 — 이 항목은 무시함"));
                continue;
            }
        };
        let id = fields.remove("id").unwrap_or_default();
        let id_bad = if id.is_empty() || id.chars().count() > ENV_ID_MAX {
            Some(format!("id는 1~{ENV_ID_MAX}자여야 함"))
        } else if !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            Some("id는 영문·숫자·'-'·'_'·'.'만 쓸 수 있음".into())
        } else if ENV_BUILTIN_IDS.contains(&id.as_str()) {
            Some(format!("'{id}'은(는) 기본 점검 항목 id라 쓸 수 없음"))
        } else if out.iter().any(|e| e.id == id) {
            Some(format!("id '{id}'이(가) 중복됨"))
        } else {
            None
        };
        if let Some(m) = id_bad {
            errs.push(sem_err(file, format!("{p}.id"), format!("{m} — 이 항목은 무시함")));
            continue;
        }
        let check = fields.remove("check").unwrap_or_default();
        if check.trim().is_empty() {
            errs.push(sem_err(file, format!("{p}.check"), "점검 명령이 비어 있음 — 이 항목은 무시함"));
            continue;
        }
        let source = fields.remove("source").unwrap_or_default();
        if !source.is_empty() {
            if let Err(m) = check_source_url(&source) {
                errs.push(sem_err(file, format!("{p}.source"), format!("{m} — 이 항목은 무시함")));
                continue;
            }
        }
        out.push(EnvRequire {
            id,
            check,
            fix: fields.remove("fix").unwrap_or_default(),
            why: fields.remove("why").unwrap_or_default(),
            required,
            source: (!source.is_empty()).then_some(source),
        });
    }
    out
}

/// Read-modify-write config.json as a JSON value (every other key and its order kept; written
/// atomically). `edit` returns whether it changed anything (`false` → nothing is written,
/// `Unchanged`). A missing file starts as `{}`. Callers hold [`CONFIG_WRITE`].
fn edit_config_json(
    path: &Path,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<bool, String>,
) -> Result<ConfigEdit, String> {
    let prev = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("config.json을 읽을 수 없습니다 — {e}")),
    };
    let mut root: serde_json::Value = match &prev {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("config.json을 해석할 수 없어 바꾸지 않았습니다 — {e}"))?,
        None => serde_json::json!({}),
    };
    let obj = root.as_object_mut().ok_or("config.json의 최상위가 객체가 아니어서 바꾸지 않았습니다")?;
    if !edit(obj)? {
        return Ok(ConfigEdit::Unchanged);
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    text.push('\n');
    write_file_atomic(path, &text)?;
    Ok(ConfigEdit::Written(prev))
}

/// Add `id` to `envSkip` of the config.json at `path` (already listed → `Unchanged`).
pub fn add_env_skip(path: &Path, id: &str) -> Result<ConfigEdit, String> {
    edit_config_json(path, |obj| {
        let skip = obj.entry("envSkip").or_insert_with(|| serde_json::json!([]));
        let arr = skip.as_array_mut().ok_or("config.json의 envSkip이 배열이 아니어서 바꾸지 않았습니다")?;
        if arr.iter().any(|v| v.as_str() == Some(id)) {
            return Ok(false);
        }
        arr.push(id.into());
        Ok(true)
    })
}

/// Set `source` of `envRequire` item `id` in the config.json at `path`. Without an
/// `envRequire` key the defaults (`defaults`, i.e. what the app was using) are written out
/// first, so the item exists in the file. The URL must already be checked.
pub fn set_env_source(path: &Path, id: &str, url: &str, defaults: &[EnvRequire]) -> Result<ConfigEdit, String> {
    edit_config_json(path, |obj| {
        let list = obj
            .entry("envRequire")
            .or_insert_with(|| serde_json::Value::Array(defaults.iter().map(env_require_json).collect()));
        let arr = list.as_array_mut().ok_or("config.json의 envRequire가 배열이 아니어서 바꾸지 않았습니다")?;
        let item = arr
            .iter_mut()
            .find(|v| v.get("id").and_then(|i| i.as_str()) == Some(id))
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| format!("config.json의 envRequire에 '{id}' 항목이 없습니다"))?;
        if item.get("source").and_then(|s| s.as_str()) == Some(url) {
            return Ok(false);
        }
        item.insert("source".into(), url.into());
        Ok(true)
    })
}

pub fn validate_config(c: &Config, file: &Path) -> Vec<ConfigError> {
    sanitize_config(c.clone(), file).1
}

fn walk_groups(
    n: &LayoutNode<GroupSpec>,
    path: &str,
    file: &Path,
    tab_names: &BTreeSet<String>,
    group_names: &mut BTreeSet<String>,
    placed: &mut BTreeSet<String>,
    errs: &mut Vec<ConfigError>,
) {
    match n {
        LayoutNode::Leaf(g) => {
            let gp = format!("{path}.group");
            if !group_names.insert(g.name.clone()) {
                errs.push(sem_err(file, format!("{gp}.name"), format!("그룹 이름 '{}'이(가) 중복됨", g.name)));
            }
            for (i, t) in g.tabs.iter().enumerate() {
                let tp = format!("{gp}.tabs[{i}]");
                if t != "dispatcher" && !tab_names.contains(t) {
                    errs.push(sem_err(file, &tp, format!("tabs에 없는 탭 '{t}'")));
                } else if !placed.insert(t.clone()) {
                    errs.push(sem_err(file, &tp, format!("탭 '{t}'이(가) 여러 그룹에 있음")));
                }
            }
        }
        LayoutNode::Split { children, .. } => {
            if children.len() < 2 {
                errs.push(sem_err(file, format!("{path}.children"), "분할에는 자식이 2개 이상 필요함"));
            }
            for (i, c) in children.iter().enumerate() {
                walk_groups(c, &format!("{path}.children[{i}]"), file, tab_names, group_names, placed, errs);
            }
        }
    }
}

// ───────────────────────── keybindings ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keybindings(pub BTreeMap<String, String>);

const NAMED_KEYS: [&str; 17] = [
    "tab", "left", "right", "up", "down", "enter", "esc", "space", "backspace", "delete", "home", "end", "pageup",
    "pagedown", "insert", "plus", "comma",
];

pub fn parse_key(s: &str) -> Result<KeyChord, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("빈 키".into());
    }
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut key: Option<String> = None;
    for tok in s.split('+') {
        let t = tok.trim().to_lowercase();
        if t.is_empty() {
            return Err(format!("키 '{s}'에 빈 항목이 있음"));
        }
        let flag = match t.as_str() {
            "ctrl" | "control" => Some(&mut ctrl),
            "alt" => Some(&mut alt),
            "shift" => Some(&mut shift),
            _ => None,
        };
        if let Some(flag) = flag {
            if *flag {
                return Err(format!("키 '{s}'에 수정자가 중복됨"));
            }
            *flag = true;
            continue;
        }
        let t = if t == "escape" { "esc".to_string() } else { t };
        let valid = match t.chars().count() {
            1 => t.chars().all(|c| c.is_ascii_alphanumeric() || "\\-;[]=,./'`".contains(c)),
            _ => {
                NAMED_KEYS.contains(&t.as_str())
                    || (t.starts_with('f') && t[1..].parse::<u8>().map_or(false, |n| (1..=12).contains(&n)))
            }
        };
        if !valid {
            return Err(format!("알 수 없는 키 '{tok}'"));
        }
        if key.replace(t).is_some() {
            return Err(format!("키 '{s}'에 일반 키가 둘 이상 있음"));
        }
    }
    match key {
        Some(key) => Ok(KeyChord { ctrl, alt, shift, key }),
        None => Err(format!("키 '{s}'에 일반 키가 없음")),
    }
}

fn default_keybindings() -> BTreeMap<String, String> {
    serde_json::from_str::<BTreeMap<String, Option<String>>>(DEFAULT_KEYBINDINGS)
        .expect("embedded keybindings.json is valid")
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect()
}

/// Defaults overlaid with the user file's text (None = no user file).
pub fn load_keybindings(user: Option<&str>, file: &Path) -> (Keybindings, Vec<ConfigError>) {
    let defaults = default_keybindings();
    let mut errs = Vec::new();
    let Some(text) = user else { return (Keybindings(defaults), errs) };
    let overrides: BTreeMap<String, Option<String>> = match serde_json::from_str(text) {
        Ok(o) => o,
        Err(e) => {
            errs.push(located_json_error(file, &e));
            return (Keybindings(defaults), errs);
        }
    };
    let mut map = defaults.clone();
    let mut overridden: BTreeSet<String> = BTreeSet::new();
    for (id, v) in overrides {
        if !defaults.contains_key(&id) && !default_ids_with_null().contains(&id) {
            errs.push(sem_err(file, &id, "알 수 없는 명령 id"));
            continue;
        }
        match v {
            None => {
                map.remove(&id);
            }
            Some(k) => match parse_key(&k) {
                Ok(_) => {
                    map.insert(id.clone(), k);
                    overridden.insert(id);
                }
                Err(m) => errs.push(sem_err(file, &id, m)),
            },
        }
    }
    // Resolve conflicts: every overridden id involved in a clash reverts to its default.
    for _ in 0..8 {
        let mut by_chord: BTreeMap<KeyChord, Vec<String>> = BTreeMap::new();
        for (id, k) in &map {
            if let Ok(c) = parse_key(k) {
                by_chord.entry(c).or_default().push(id.clone());
            }
        }
        let clashes: Vec<_> = by_chord.into_iter().filter(|(_, ids)| ids.len() > 1).collect();
        if clashes.is_empty() {
            break;
        }
        for (chord, ids) in clashes {
            let mut label = String::new();
            if chord.ctrl {
                label += "Ctrl+";
            }
            if chord.alt {
                label += "Alt+";
            }
            if chord.shift {
                label += "Shift+";
            }
            label += &chord.key;
            errs.push(sem_err(file, ids.join(", "), format!("키 '{label}'이(가) 여러 명령에 지정됨 — 기본값 유지")));
            for id in ids {
                if overridden.remove(&id) {
                    match defaults.get(&id) {
                        Some(d) => {
                            map.insert(id, d.clone());
                        }
                        None => {
                            map.remove(&id);
                        }
                    }
                }
            }
        }
    }
    (Keybindings(map), errs)
}

fn default_ids_with_null() -> BTreeSet<String> {
    serde_json::from_str::<BTreeMap<String, Option<String>>>(DEFAULT_KEYBINDINGS)
        .expect("embedded keybindings.json is valid")
        .into_keys()
        .collect()
}

// ───────────────────────── loading ─────────────────────────

pub struct Loaded {
    pub settings: Settings,
    pub config: Config,
    pub templates: BTreeMap<String, Template>,
    pub profiles: BTreeMap<String, crate::profiles::AgentProfile>,
    pub keybindings: Keybindings,
    pub errors: Vec<ConfigError>,
}

fn embedded_template(name: &str) -> Template {
    let (file, text) = DEFAULT_TEMPLATES.iter().find(|(n, _)| n.trim_end_matches(".json") == name).expect("embedded template");
    parse_template(text, Path::new(file)).expect("embedded template is valid")
}

/// Create `.agentboard/` and copy embedded defaults (keybindings.json starts as `{}`: user
/// overrides only) — never overwrites existing files.
pub fn ensure_dotdir(root: &Path) -> Result<(), String> {
    let dot = root.join(".agentboard");
    let tdir = dot.join("templates");
    let adir = dot.join("agents");
    std::fs::create_dir_all(&tdir).map_err(|e| format!("{}: {e}", tdir.display()))?;
    std::fs::create_dir_all(&adir).map_err(|e| format!("{}: {e}", adir.display()))?;
    let mut files: Vec<(PathBuf, &str)> =
        DEFAULT_TEMPLATES.iter().map(|(n, c)| (tdir.join(n), *c)).collect();
    files.extend(DEFAULT_AGENTS.iter().map(|(n, c)| (adir.join(n), *c)));
    crate::profiles::ensure_powershell_profile(&adir);
    // Overrides only: defaults always come from the embedded file, so a full copy here would
    // shadow future default changes. Existing files (incl. phase-1 full copies) stay as they are.
    files.push((dot.join("keybindings.json"), "{}\n"));
    for (path, content) in files {
        if !path.exists() {
            std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().map_or(false, |x| x == "json"))
        .collect();
    v.sort();
    v
}

/// Load everything; never panics. Problems become `errors` and a usable default is substituted.
pub fn load_all(appdata_dir: &Path) -> Loaded {
    let mut errors = Vec::new();
    let (settings, e) = load_settings_checked(appdata_dir);
    errors.extend(e);
    let root = settings.workspace_root.clone();
    let dot = root.join(".agentboard");
    if let Err(m) = ensure_dotdir(&root) {
        errors.push(ConfigError { file: dot.clone(), line: 0, col: 0, json_path: None, message: format!("기본 설정 파일 생성 실패: {m}") });
    }

    // profiles
    let (profiles, perrs) = crate::profiles::load_dir(&dot.join("agents"));
    errors.extend(perrs);
    let known: BTreeSet<String> = profiles.keys().cloned().collect();

    // templates
    let mut templates = BTreeMap::new();
    for path in json_files(&dot.join("templates")) {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                errors.push(io_err(&path, &e));
                continue;
            }
        };
        match parse_template(&text, &path) {
            Ok(t) => {
                let errs = validate_template(&t, &path, &known);
                if !errs.is_empty() {
                    errors.extend(errs);
                } else if templates.contains_key(&t.name) {
                    errors.push(sem_err(&path, "name", format!("템플릿 이름 '{}'이(가) 다른 파일과 중복됨", t.name)));
                } else {
                    templates.insert(t.name.clone(), t);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    // config
    let cfg_path = dot.join("config.json");
    let mut config = match std::fs::read_to_string(&cfg_path) {
        Ok(text) => match parse_config(&text, &cfg_path) {
            Ok((c, perrs)) => {
                errors.extend(perrs);
                let (c, errs) = sanitize_config(c, &cfg_path);
                errors.extend(errs);
                c
            }
            Err(e) => {
                errors.push(e);
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            errors.push(io_err(&cfg_path, &e));
            Config::default()
        }
    };
    // references from config to templates; missing ones are reported, never fatal
    for (path, name) in [("dispatcher.template", &config.dispatcher.template), ("defaultTemplate", &config.default_template)] {
        if !templates.contains_key(name) {
            errors.push(sem_err(&cfg_path, path, format!("템플릿 '{name}'을(를) 찾을 수 없음 — 내장 기본값 사용")));
        }
    }
    for t in &config.tabs {
        if let Some(n) = &t.template {
            if !templates.contains_key(n) {
                errors.push(sem_err(&cfg_path, format!("tabs[name={}].template", t.name), format!("템플릿 '{n}'을(를) 찾을 수 없음")));
            }
        }
    }
    // the Dispatcher tab and the default template must always be usable
    for (slot, builtin) in [(0, "dispatcher"), (1, "solo")] {
        let cur = if slot == 0 { &mut config.dispatcher.template } else { &mut config.default_template };
        if !templates.contains_key(cur.as_str()) {
            *cur = builtin.to_string();
        }
        if !templates.contains_key(builtin) && *cur == builtin {
            templates.insert(builtin.to_string(), embedded_template(builtin));
        }
    }

    // keybindings
    let kb_path = dot.join("keybindings.json");
    let user_kb = match std::fs::read_to_string(&kb_path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            errors.push(io_err(&kb_path, &e));
            None
        }
    };
    let (keybindings, kerrs) = load_keybindings(user_kb.as_deref(), &kb_path);
    errors.extend(kerrs);

    Loaded { settings, config, templates, profiles, keybindings, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn f() -> &'static Path {
        Path::new("t.json")
    }
    fn agents(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }
    fn tpl(src: &str) -> Template {
        parse_template(src, f()).expect("parse")
    }
    fn solo_with(pane: &str) -> String {
        format!(r#"{{"name":"x","layout":{{"pane":{pane}}}}}"#)
    }

    const SPEC: &str = r#"{
      "name": "backend-team",
      "layout": {
        "split": "horizontal", "sizes": [60, 40],
        "children": [
          { "pane": { "name": "lead", "agent": "claude", "args": ["--model", "opus"],
                      "cwd": "", "prompt": "lead", "focus": true } },
          { "split": "vertical", "slot": "members",
            "children": [
              { "pane": { "name": "coder",  "agent": "claude" } },
              { "pane": { "name": "tests",  "agent": "powershell", "cwd": "server",
                          "prompt": "npm test -- --watch" } }
            ] }
        ]
      }
    }"#;

    #[test]
    fn parses_spec_example_template() {
        let t = tpl(SPEC);
        let names: Vec<String> = t.layout.leaves().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["lead", "coder", "tests"]);
        assert!(t.layout.leaves()[0].focus);
        assert_eq!(t.layout.leaves()[0].args, ["--model", "opus"]);
        match &t.layout {
            LayoutNode::Split { dir, sizes, children, .. } => {
                assert_eq!(*dir, Dir::Horizontal);
                assert_eq!(sizes.as_slice(), &[60.0f32, 40.0][..]);
                match &children[1] {
                    LayoutNode::Split { slot, .. } => assert_eq!(slot.as_deref(), Some("members")),
                    _ => panic!("expected split"),
                }
            }
            _ => panic!("expected split"),
        }
        assert!(validate_template(&t, f(), &agents(&["claude", "powershell"])).is_empty());
    }

    #[test]
    fn syntax_error_has_line_and_col() {
        let src = "{\n \"name\": \"x\"\n \"layout\": {}\n}";
        let e = parse_template(src, Path::new("a/t.json")).unwrap_err();
        assert_eq!(e.line, 3);
        assert!(e.col > 0);
        assert!(e.to_string().starts_with("a/t.json:3:"), "{}", e);
    }

    #[test]
    fn rejects_absolute_cwd() {
        for cwd in ["C:/x", "C:\\x", "/x", "\\\\srv\\share"] {
            let t = tpl(&solo_with(&format!(r#"{{"name":"a","cwd":{}}}"#, serde_json::to_string(cwd).unwrap())));
            let errs = validate_template(&t, f(), &agents(&[]));
            assert_eq!(errs.len(), 1, "{cwd}");
            assert_eq!(errs[0].json_path.as_deref(), Some("layout.pane.cwd"));
        }
    }

    #[test]
    fn rejects_cwd_escaping_workspace() {
        for (cwd, ok) in [("../other", false), ("a/../../b", false), ("server", true), ("a/../b", true), ("", true)] {
            let t = tpl(&solo_with(&format!(r#"{{"name":"a","cwd":"{cwd}"}}"#)));
            let errs = validate_template(&t, f(), &agents(&[]));
            assert_eq!(errs.is_empty(), ok, "{cwd}");
        }
    }

    #[test]
    fn rejects_duplicate_pane_names() {
        let src = r#"{"name":"x","layout":{"split":"horizontal","children":[{"pane":{"name":"a"}},{"pane":{"name":"a"}}]}}"#;
        let errs = validate_template(&tpl(src), f(), &agents(&[]));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].json_path.as_deref(), Some("layout.children[1].pane.name"));
    }

    #[test]
    fn rejects_unknown_agent() {
        let t = tpl(&solo_with(r#"{"name":"a","agent":"codex"}"#));
        let errs = validate_template(&t, f(), &agents(&["claude"]));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].json_path.as_deref(), Some("layout.pane.agent"));
        assert!(validate_template(&t, f(), &agents(&["codex"])).is_empty());
    }

    #[test]
    fn rejects_split_with_one_child() {
        let src = r#"{"name":"x","layout":{"split":"vertical","children":[{"pane":{"name":"a"}}]}}"#;
        let errs = validate_template(&tpl(src), f(), &agents(&[]));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].json_path.as_deref(), Some("layout.children"));
    }

    #[test]
    fn config_tab_names_unique_and_dispatcher_reserved() {
        let (c, _) = parse_config(
            r#"{"tabs":[{"name":"dispatcher","dir":"a"},{"name":"b","dir":"b"},{"name":"b","dir":"c"}]}"#,
            Path::new("config.json"),
        )
        .unwrap();
        let errs = validate_config(&c, Path::new("config.json"));
        let paths: Vec<_> = errs.iter().map(|e| e.json_path.clone().unwrap()).collect();
        assert_eq!(paths, ["tabs[0].name", "tabs[2].name"]);
    }

    #[test]
    fn parse_key_normalizes() {
        assert_eq!(parse_key("Ctrl+Alt+Shift+H").unwrap(), parse_key("ctrl+alt+shift+h").unwrap());
        let k = parse_key("Alt+\\").unwrap();
        assert!(k.alt && !k.ctrl && !k.shift);
        assert_eq!(k.key, "\\");
        assert_eq!(parse_key("Alt+-").unwrap().key, "-");
        assert_eq!(parse_key("Alt+Left").unwrap().key, "left");
        assert!(parse_key("Alt+Q+W").is_err());
        assert!(parse_key("Alt+").is_err());
        assert!(parse_key("Ctrl+Ctrl+a").is_err());
        assert!(parse_key("Alt+Shift").is_err());
    }

    #[test]
    fn default_keybindings_are_valid_and_complete() {
        let (kb, errs) = load_keybindings(None, Path::new("keybindings.json"));
        assert!(errs.is_empty(), "{errs:?}");
        for id in ["pane.focusLeft", "pane.resizeRight", "tab.goto9", "group.goto1", "tab.moveUp", "group.splitDown", "app.palette", "pane.paste"] {
            assert!(kb.0.contains_key(id), "{id}");
        }
        assert_eq!(kb.0["pane.focusLeft"], "Alt+h");
        assert!(!kb.0.contains_key("pane.breakToTab"));
    }

    #[test]
    fn keybindings_override_and_conflict() {
        let p = Path::new("keybindings.json");
        let (kb, errs) = load_keybindings(Some(r#"{"pane.focusLeft":"Alt+y"}"#), p);
        assert!(errs.is_empty());
        assert_eq!(kb.0["pane.focusLeft"], "Alt+y");

        let (kb, errs) = load_keybindings(Some(r#"{"pane.focusLeft":"Alt+y","pane.focusDown":"alt+Y"}"#), p);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(kb.0["pane.focusLeft"], "Alt+h");
        assert_eq!(kb.0["pane.focusDown"], "Alt+j");

        let (kb, errs) = load_keybindings(Some(r#"{"pane.focusLeft":null}"#), p);
        assert!(errs.is_empty());
        assert!(!kb.0.contains_key("pane.focusLeft"));

        let (_, errs) = load_keybindings(Some(r#"{"pane.nope":"Alt+y","pane.close":"Alt+Q+W"}"#), p);
        assert_eq!(errs.len(), 2);
    }

    #[test]
    fn ensure_dotdir_does_not_overwrite_user_files() {
        let d = tempfile::tempdir().unwrap();
        ensure_dotdir(d.path()).unwrap();
        let dot = d.path().join(".agentboard");
        assert!(dot.join("templates/solo.json").exists());
        assert!(dot.join("templates/team-3.json").exists());
        assert!(dot.join("keybindings.json").exists());
        std::fs::write(dot.join("templates/solo.json"), "MINE").unwrap();
        ensure_dotdir(d.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dot.join("templates/solo.json")).unwrap(), "MINE");
    }

    #[test]
    fn new_install_keybindings_file_is_empty_object() {
        let root = tempfile::tempdir().unwrap();
        ensure_dotdir(root.path()).unwrap();
        let kb = root.path().join(".agentboard/keybindings.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&kb).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({}), "new installs get an empty override file");
        // an empty user file = all embedded defaults, no errors
        let a = appdata_with_root(root.path());
        let l = load_all(a.path());
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        assert_eq!(l.keybindings.0, load_keybindings(None, &kb).0 .0);
        // an existing file (e.g. a phase-1 full copy or user edits) is never touched
        std::fs::write(&kb, r#"{"pane.focusLeft":"Alt+y"}"#).unwrap();
        ensure_dotdir(root.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&kb).unwrap(), r#"{"pane.focusLeft":"Alt+y"}"#);
        let l = load_all(a.path());
        assert_eq!(l.keybindings.0["pane.focusLeft"], "Alt+y");
        assert_eq!(l.keybindings.0["pane.focusDown"], "Alt+j", "non-overridden ids keep defaults");
    }

    fn appdata_with_root(root: &Path) -> tempfile::TempDir {
        let a = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.to_string_lossy() });
        std::fs::write(a.path().join("settings.json"), s.to_string()).unwrap();
        a
    }

    #[test]
    fn settings_created_with_default() {
        let a = tempfile::tempdir().unwrap();
        let s = load_settings(&a.path().join("sub"));
        assert_eq!(s.workspace_root, PathBuf::from("C:/workspace"));
        assert!(a.path().join("sub/settings.json").exists());
    }

    #[test]
    fn broken_config_yields_error_and_dispatcher_only_defaults() {
        let root = tempfile::tempdir().unwrap();
        let a = appdata_with_root(root.path());
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), "{ \"tabs\": [ }").unwrap();
        let l = load_all(a.path());
        let cfg_errs: Vec<_> = l.errors.iter().filter(|e| e.file.ends_with("config.json")).collect();
        assert_eq!(cfg_errs.len(), 1, "{:?}", l.errors);
        assert!(l.config.tabs.is_empty());
        assert_eq!(l.config.dispatcher.template, "dispatcher");
        assert_eq!(l.config.default_template, "solo");
        assert_eq!(l.config.default_agent, "claude");
    }

    #[test]
    fn load_all_valid_and_invalid_templates() {
        let root = tempfile::tempdir().unwrap();
        let a = appdata_with_root(root.path());
        ensure_dotdir(root.path()).unwrap();
        let dot = root.path().join(".agentboard");
        std::fs::write(dot.join("templates/bad.json"), solo_with(r#"{"name":"a","cwd":"C:/x"}"#).replace("\"x\"", "\"bad\"")).unwrap();
        std::fs::write(dot.join("templates/broken.json"), "{").unwrap();
        let l = load_all(a.path());
        assert!(l.templates.contains_key("solo"));
        assert!(l.templates.contains_key("dispatcher"));
        assert!(!l.templates.contains_key("bad"));
        assert!(l.errors.iter().any(|e| e.file.ends_with("bad.json")));
        assert!(l.errors.iter().any(|e| e.file.ends_with("broken.json") && e.line > 0));
        assert_eq!(l.keybindings.0["app.palette"], "Alt+;");
    }

    #[test]
    fn fresh_root_loads_team3_without_unknown_agent_errors() {
        let root = tempfile::tempdir().unwrap();
        let a = appdata_with_root(root.path());
        let l = load_all(a.path());
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        assert!(l.templates.contains_key("team-3"));
        for n in ["claude", "cmd", "powershell"] {
            assert!(l.profiles.contains_key(n), "{n}");
        }
    }

    fn load_with(files: &[(&str, &str)]) -> Loaded {
        let root = tempfile::tempdir().unwrap();
        let a = appdata_with_root(root.path());
        ensure_dotdir(root.path()).unwrap();
        let dot = root.path().join(".agentboard");
        for (n, c) in files {
            std::fs::write(dot.join(n), c).unwrap();
        }
        load_all(a.path())
    }

    #[test]
    fn duplicate_tab_keeps_other_tabs() {
        let l = load_with(&[("config.json", r#"{"tabs":[{"name":"a","dir":"a"},{"name":"a","dir":"b"},{"name":"c","dir":"c"}]}"#)]);
        let names: Vec<_> = l.config.tabs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(l.errors.iter().filter(|e| e.file.ends_with("config.json")).count(), 1);
    }

    #[test]
    fn bad_group_ref_drops_groups_keeps_tabs() {
        let l = load_with(&[(
            "config.json",
            r#"{"groups":{"split":"horizontal","children":[{"group":{"name":"g","tabs":["dispatcher"]}},{"group":{"name":"w","tabs":["nope"]}}]},
                "defaultGroup":"w","tabs":[{"name":"a","dir":"a"}]}"#,
        )]);
        assert!(l.config.groups.is_none());
        assert!(l.config.default_group.is_none());
        assert_eq!(l.config.tabs.len(), 1);
        assert!(l.errors.iter().any(|e| e.json_path.as_deref() == Some("groups.children[1].group.tabs[0]")));
    }

    #[test]
    fn bad_default_group_dropped_alone() {
        let l = load_with(&[(
            "config.json",
            r#"{"groups":{"group":{"name":"w","tabs":["a"]}},"defaultGroup":"zzz","tabs":[{"name":"a","dir":"a"}]}"#,
        )]);
        assert!(l.config.groups.is_some());
        assert!(l.config.default_group.is_none());
        assert_eq!(l.errors.iter().filter(|e| e.file.ends_with("config.json")).count(), 1, "{:?}", l.errors);
    }

    fn perr(src: &str) -> ConfigError {
        parse_template(src, f()).unwrap_err()
    }

    #[test]
    fn unknown_pane_field_names_field_and_path() {
        let e = perr(r#"{"name":"x","layout":{"split":"horizontal","children":[{"pane":{"name":"a"}},{"pane":{"name":"b","focuss":true}}]}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout.children[1].pane"));
        assert!(e.message.contains("focuss"), "{}", e.message);
    }

    #[test]
    fn split_typo_and_mixed_nodes_rejected() {
        let e = perr(r#"{"name":"x","layout":{"split":"horizontal","size":[60,40],"children":[{"pane":{"name":"a"}},{"pane":{"name":"b"}}]}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout"));
        assert!(e.message.contains("size"), "{}", e.message);
        let e = perr(r#"{"name":"x","layout":{"pane":{"name":"a"},"split":"horizontal","children":[]}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout"));
        let e = perr(r#"{"name":"x","layout":{"foo":1}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout"));
    }

    #[test]
    fn bad_split_direction_has_path() {
        let e = perr(r#"{"name":"x","layout":{"split":"horiz","children":[{"pane":{"name":"a"}},{"pane":{"name":"b"}}]}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout.split"));
        assert!(e.to_string().contains("layout.split"));
    }

    #[test]
    fn malformed_groups_layout_keeps_rest_of_config() {
        let l = load_with(&[("config.json", r#"{"groups":{"split":"horiz","children":[]},"tabs":[{"name":"a","dir":"a"}]}"#)]);
        assert!(l.config.groups.is_none());
        assert_eq!(l.config.tabs.len(), 1);
        assert!(l.errors.iter().any(|e| e.json_path.as_deref() == Some("groups.split")));
    }

    // ── phase 2: roles, nightly settings, applet panes ──

    fn cfg_errs(src: &str) -> (Config, Vec<ConfigError>) {
        let p = Path::new("config.json");
        let (c, mut errs) = parse_config(src, p).expect("parse");
        let (c, e2) = sanitize_config(c, p);
        errs.extend(e2);
        (c, errs)
    }
    fn paths(errs: &[ConfigError]) -> Vec<String> {
        errs.iter().map(|e| e.json_path.clone().unwrap_or_default()).collect()
    }

    #[test]
    fn append_config_tab_keeps_other_keys_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let text = "{\n  \"nightlyRefresh\": \"04:30\",\n  \"tabs\": [{\"name\": \"a\", \"dir\": \"a\", \"x-note\": 1}],\n  \"defaultAgent\": \"claude\"\n}\n";
        std::fs::write(&path, text).unwrap();
        let spec = TabSpec { name: "ops".into(), dir: "ops".into(), template: None, role: Some(Role::Manager) };
        assert_eq!(append_config_tab(&path, &spec).unwrap(), ConfigEdit::Written(Some(text.to_string())));
        let out = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["tabs"][1], serde_json::json!({"name":"ops","dir":"ops","role":"manager"}));
        assert_eq!(v["tabs"][0]["x-note"], 1, "unknown keys are kept");
        assert_eq!(v["nightlyRefresh"], "04:30");
        let (n, t, d) = (out.find("nightlyRefresh").unwrap(), out.find("\"tabs\"").unwrap(), out.find("defaultAgent").unwrap());
        assert!(n < t && t < d, "key order kept: {out}");
        assert!(!dir.path().join("config.json.tmp").exists());
        // a retried identical append changes nothing; a clashing name is refused
        assert_eq!(append_config_tab(&path, &spec).unwrap(), ConfigEdit::Unchanged);
        let other = TabSpec { dir: "elsewhere".into(), ..spec.clone() };
        assert!(append_config_tab(&path, &other).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), out);
        // restore puts the old text back
        restore_config_file(&path, Some(text)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // a missing file is created, and restoring removes it again
        let p2 = dir.path().join("new.json");
        let e = append_config_tab(&p2, &spec).unwrap();
        assert_eq!(e, ConfigEdit::Written(None));
        restore_config_file(&p2, None).unwrap();
        assert!(!p2.exists());
        // unparsable file: refused, untouched
        std::fs::write(&path, "{ nope").unwrap();
        assert!(append_config_tab(&path, &spec).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ nope");
    }

    #[test]
    fn parses_roles_and_nightly_config() {
        let (c, errs) = cfg_errs(
            r#"{"tabs":[{"name":"handover","role":"handover","dir":"","template":"solo"},
                        {"name":"billing","role":"manager","dir":"billing"},
                        {"name":"plain","dir":"plain"}],
                "nightlyRefresh":"23:30","nightlyTimeoutMin":30,"nightlyQuietMin":5,
                "handoverIndex":"docs/HANDOVER.md",
                "roles":{"dispatcher":{"systemPromptFile":"roles/dispatcher.md"},
                         "lead":{"agent":"claude","args":["--model","opus"]},
                         "member":{"systemPromptFile":"roles/member.md","maxPerTeam":6}}}"#,
        );
        assert!(errs.is_empty(), "{errs:?}");
        let roles: Vec<_> = c.tabs.iter().map(|t| t.role).collect();
        assert_eq!(roles, [Some(Role::Handover), Some(Role::Manager), None]);
        assert_eq!(c.nightly_refresh.as_deref(), Some("23:30"));
        assert_eq!((c.nightly_timeout_min, c.nightly_quiet_min), (30, 5));
        assert_eq!(c.handover_index.as_deref(), Some("docs/HANDOVER.md"));
        assert_eq!(c.handover_index_path(), "docs/HANDOVER.md");
        assert_eq!(c.roles[&Role::Dispatcher].system_prompt_file.as_deref(), Some("roles/dispatcher.md"));
        assert_eq!(c.roles[&Role::Lead].agent.as_deref(), Some("claude"));
        assert_eq!(c.roles[&Role::Lead].args, ["--model", "opus"]);
        assert_eq!(c.roles[&Role::Member].max_per_team, Some(6));
        assert_eq!(c.max_per_team(), 6);
        assert!(!c.roles.contains_key(&Role::Manager));

        // defaults
        let (d, errs) = cfg_errs("{}");
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(d.nightly_refresh.as_deref(), Some("03:00"));
        assert_eq!((d.nightly_timeout_min, d.nightly_quiet_min), (20, 10));
        assert_eq!(d.handover_index, None);
        assert_eq!(d.handover_index_path(), "HANDOVER.md");
        assert!(d.roles.is_empty());
        assert_eq!(d.max_per_team(), 4);
        // explicit null switches the nightly refresh off
        let (n, errs) = cfg_errs(r#"{"nightlyRefresh":null}"#);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(n.nightly_refresh, None);
    }

    #[test]
    fn rejects_bad_nightly_time_and_dispatcher_role_in_tabs() {
        for bad in ["3:00", "24:00", "03:60", "0300", "aa:bb", "", "03:00 "] {
            let (c, errs) = cfg_errs(&format!(r#"{{"nightlyRefresh":"{bad}"}}"#));
            assert_eq!(paths(&errs), ["nightlyRefresh"], "{bad:?}");
            assert_eq!(c.nightly_refresh.as_deref(), Some("03:00"), "falls back to the default");
        }
        for ok in ["00:00", "23:59", "07:05"] {
            let (_, errs) = cfg_errs(&format!(r#"{{"nightlyRefresh":"{ok}"}}"#));
            assert!(errs.is_empty(), "{ok}: {errs:?}");
        }
        let (c, errs) = cfg_errs(r#"{"nightlyTimeoutMin":0,"nightlyQuietMin":0}"#);
        assert_eq!(paths(&errs), ["nightlyTimeoutMin"]);
        assert_eq!((c.nightly_timeout_min, c.nightly_quiet_min), (20, 0), "quiet 0 = no grace period");

        // tabs[].role: only manager | handover; the offending tab alone is dropped
        let (c, errs) = cfg_errs(
            r#"{"tabs":[{"name":"a","role":"dispatcher","dir":"a"},{"name":"b","role":"lead","dir":"b"},
                        {"name":"c","role":"boss","dir":"c"},{"name":"d","role":"manager","dir":"d"},
                        {"name":"d","dir":"x"}]}"#,
        );
        assert_eq!(paths(&errs), ["tabs[0].role", "tabs[1].role", "tabs[2].role", "tabs[4].name"], "{errs:?}");
        assert!(errs[0].message.contains("manager"), "{}", errs[0].message);
        let names: Vec<_> = c.tabs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["d"]);

        // roles: unknown role key / bad fields reported, the rest kept
        let (c, errs) = cfg_errs(
            r#"{"roles":{"boss":{},"lead":{"agentt":"x"},"manager":{"maxPerTeam":3},
                         "member":{"maxPerTeam":0},"handover":{"systemPromptFile":"C:/x.md"},
                         "dispatcher":{"systemPromptFile":"roles/d.md"}}}"#,
        );
        let mut p = paths(&errs);
        p.sort();
        assert_eq!(
            p,
            ["roles.boss", "roles.handover.systemPromptFile", "roles.lead", "roles.manager.maxPerTeam", "roles.member.maxPerTeam"],
            "{errs:?}"
        );
        assert_eq!(c.roles.keys().copied().collect::<Vec<_>>(), [Role::Dispatcher]);
        // a wrongly typed roles value is a located error, not a dropped config
        let (_, errs) = cfg_errs(r#"{"roles":[],"tabs":[{"name":"a","dir":"a"}]}"#);
        assert_eq!(paths(&errs), ["roles"]);
        // handoverIndex must stay inside the root
        let (c, errs) = cfg_errs(r#"{"handoverIndex":"../x.md"}"#);
        assert_eq!(paths(&errs), ["handoverIndex"]);
        assert_eq!(c.handover_index_path(), "HANDOVER.md");
    }

    #[test]
    fn handover_tab_rules_in_config() {
        let (c, errs) = cfg_errs(
            r#"{"tabs":[{"name":"h1","role":"handover","dir":"sub"},{"name":"h2","role":"handover","dir":""},
                        {"name":"h3","role":"handover"},{"name":"m","role":"manager","dir":""}]}"#,
        );
        assert_eq!(paths(&errs), ["tabs[0].dir", "tabs[2].role"], "{errs:?}");
        let names: Vec<_> = c.tabs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["h2", "m"], "other tabs keep the phase-1 empty-dir rule");
    }

    #[test]
    fn applet_node_parses_and_rejects_agent_fields() {
        let src = r#"{"name":"dispatcher-inbox","layout":{"split":"horizontal","sizes":[60,40],"children":[
            {"pane":{"name":"dispatcher","focus":true}},
            {"applet":{"name":"inbox","kind":"inbox"}}]}}"#;
        let t = tpl(src);
        let leaves = t.layout.leaves();
        assert_eq!(leaves[0].applet, None);
        assert_eq!(leaves[1].name, "inbox");
        assert_eq!(leaves[1].applet.as_deref(), Some("inbox"));
        assert!(validate_template(&t, f(), &agents(&["claude"])).is_empty());

        for (field, val) in [("agent", r#""claude""#), ("cwd", r#""x""#), ("prompt", r#""hi""#), ("args", r#"["a"]"#)] {
            let t = tpl(&format!(r#"{{"name":"x","layout":{{"applet":{{"name":"inbox","kind":"inbox","{field}":{val}}}}}}}"#));
            let errs = validate_template(&t, f(), &agents(&["claude"]));
            assert_eq!(paths(&errs), [format!("layout.applet.{field}")], "{field}");
        }
        // unknown kind / unknown field / missing kind
        let t = tpl(r#"{"name":"x","layout":{"applet":{"name":"a","kind":"clock"}}}"#);
        assert_eq!(paths(&validate_template(&t, f(), &agents(&[]))), ["layout.applet.kind"]);
        let e = perr(r#"{"name":"x","layout":{"applet":{"name":"a","kind":"inbox","foo":1}}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout.applet"));
        assert!(e.message.contains("foo"), "{}", e.message);
        let e = perr(r#"{"name":"x","layout":{"applet":{"name":"a"}}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout.applet"));
        // pane and applet together, applet names share the pane-name namespace
        let e = perr(r#"{"name":"x","layout":{"pane":{"name":"a"},"applet":{"name":"b","kind":"inbox"}}}"#);
        assert_eq!(e.json_path.as_deref(), Some("layout"));
        let t = tpl(r#"{"name":"x","layout":{"split":"horizontal","children":[{"pane":{"name":"a"}},{"applet":{"name":"a","kind":"inbox"}}]}}"#);
        assert_eq!(paths(&validate_template(&t, f(), &agents(&[]))), ["layout.children[1].applet.name"]);
        // a template made only of applets has no terminal pane
        let t = tpl(r#"{"name":"x","layout":{"applet":{"name":"a","kind":"inbox"}}}"#);
        assert!(validate_template(&t, f(), &agents(&[])).is_empty());
    }

    #[test]
    fn missing_or_broken_dispatcher_template_falls_back_to_embedded() {
        for content in [None, Some("{")] {
            let root = tempfile::tempdir().unwrap();
            let a = appdata_with_root(root.path());
            ensure_dotdir(root.path()).unwrap();
            let tp = root.path().join(".agentboard/templates");
            match content {
                None => std::fs::remove_file(tp.join("dispatcher.json")).unwrap(),
                Some(c) => std::fs::write(tp.join("dispatcher.json"), c).unwrap(),
            }
            std::fs::remove_file(tp.join("solo.json")).unwrap();
            let l = load_all(a.path());
            assert!(l.templates.contains_key(&l.config.dispatcher.template));
            assert!(l.templates.contains_key(&l.config.default_template));
            assert_eq!(l.templates["dispatcher"].layout.leaves()[0].name, "dispatcher");
            // a broken file is reported; a deleted one just falls back silently
            assert_eq!(!l.errors.is_empty(), content.is_some(), "{:?}", l.errors);
        }
    }
}
