use crate::layout::{Dir, LayoutNode};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_TEMPLATES: [(&str, &str); 3] = [
    ("dispatcher.json", include_str!("../defaults/templates/dispatcher.json")),
    ("solo.json", include_str!("../defaults/templates/solo.json")),
    ("team-3.json", include_str!("../defaults/templates/team-3.json")),
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

fn sem_err(file: &Path, path: impl Into<String>, message: impl Into<String>) -> ConfigError {
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

#[derive(Deserialize)]
#[serde(untagged)]
enum RawNode {
    Pane {
        pane: RawPane,
    },
    Split {
        split: Dir,
        #[serde(default)]
        sizes: Option<Vec<f32>>,
        children: Vec<RawNode>,
        #[serde(default)]
        slot: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    name: String,
    layout: RawNode,
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

fn convert_node(n: RawNode) -> LayoutNode<PaneSpec> {
    match n {
        RawNode::Pane { pane: p } => LayoutNode::Leaf(PaneSpec {
            name: p.name,
            agent: p.agent,
            args: p.args,
            env: p.env,
            cwd: p.cwd,
            prompt: p.prompt,
            focus: p.focus,
        }),
        RawNode::Split { split, sizes, children, slot } => {
            let sizes = fit_sizes(sizes, children.len());
            LayoutNode::Split { dir: split, sizes, children: children.into_iter().map(convert_node).collect(), slot }
        }
    }
}

/// Parse a template file's text. Single-child splits are kept so `validate_template` can reject them.
pub fn parse_template(text: &str, file: &Path) -> Result<Template, ConfigError> {
    let raw: RawTemplate = serde_json::from_str(text).map_err(|e| located_json_error(file, &e))?;
    Ok(Template { name: raw.name, layout: convert_node(raw.layout) })
}

fn is_absolute_like(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/') || p.starts_with('\\') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// Returns an error message when `p` is not a relative path staying inside its base.
fn check_relative(p: &str) -> Option<&'static str> {
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
            let pp = format!("{path}.pane");
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
#[serde(untagged)]
enum RawGroupNode {
    Group {
        group: RawGroup,
    },
    Split {
        split: Dir,
        #[serde(default)]
        sizes: Option<Vec<f32>>,
        children: Vec<RawGroupNode>,
        #[serde(default)]
        slot: Option<String>,
    },
}

fn convert_group(n: RawGroupNode) -> LayoutNode<GroupSpec> {
    match n {
        RawGroupNode::Group { group } => LayoutNode::Leaf(GroupSpec { name: group.name, tabs: group.tabs }),
        RawGroupNode::Split { split, sizes, children, slot } => LayoutNode::Split {
            dir: split,
            sizes: fit_sizes(sizes, children.len()),
            children: children.into_iter().map(convert_group).collect(),
            slot,
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTab {
    name: String,
    #[serde(default)]
    dir: String,
    #[serde(default)]
    template: Option<String>,
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
    groups: Option<RawGroupNode>,
    #[serde(default)]
    default_group: Option<String>,
    #[serde(default)]
    tabs: Vec<RawTab>,
}

pub fn parse_config(text: &str, file: &Path) -> Result<Config, ConfigError> {
    let raw: RawConfig = serde_json::from_str(text).map_err(|e| located_json_error(file, &e))?;
    let d = Config::default();
    let disp = raw.dispatcher.unwrap_or(RawDispatcher { agent: None, template: None });
    Ok(Config {
        dispatcher: DispatcherSpec {
            agent: disp.agent.or(d.dispatcher.agent),
            template: disp.template.unwrap_or(d.dispatcher.template),
        },
        default_template: raw.default_template.unwrap_or(d.default_template),
        default_agent: raw.default_agent.unwrap_or(d.default_agent),
        groups: raw.groups.map(convert_group),
        default_group: raw.default_group,
        tabs: raw.tabs.into_iter().map(|t| TabSpec { name: t.name, dir: t.dir, template: t.template }).collect(),
    })
}

pub fn validate_config(c: &Config, file: &Path) -> Vec<ConfigError> {
    let mut errs = Vec::new();
    let mut names = BTreeSet::new();
    for (i, t) in c.tabs.iter().enumerate() {
        let p = format!("tabs[{i}]");
        if t.name.trim().is_empty() {
            errs.push(sem_err(file, format!("{p}.name"), "탭 이름이 비어 있음"));
        } else if t.name == "dispatcher" {
            errs.push(sem_err(file, format!("{p}.name"), "'dispatcher'는 예약된 탭 이름임"));
        } else if !names.insert(t.name.clone()) {
            errs.push(sem_err(file, format!("{p}.name"), format!("탭 이름 '{}'이(가) 중복됨", t.name)));
        }
        if let Some(m) = check_relative(&t.dir) {
            errs.push(sem_err(file, format!("{p}.dir"), m));
        }
    }
    if let Some(g) = &c.groups {
        let mut group_names = BTreeSet::new();
        let mut placed = BTreeSet::new();
        walk_groups(g, "groups", file, &names, &mut group_names, &mut placed, &mut errs);
        if let Some(dg) = &c.default_group {
            if !group_names.contains(dg) {
                errs.push(sem_err(file, "defaultGroup", format!("그룹 '{dg}'이(가) groups에 없음")));
            }
        }
    }
    errs
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

/// Create `.agentboard/` and copy embedded defaults — never overwrites existing files.
pub fn ensure_dotdir(root: &Path) -> Result<(), String> {
    let dot = root.join(".agentboard");
    let tdir = dot.join("templates");
    std::fs::create_dir_all(&tdir).map_err(|e| format!("{}: {e}", tdir.display()))?;
    let mut files: Vec<(PathBuf, &str)> =
        DEFAULT_TEMPLATES.iter().map(|(n, c)| (tdir.join(n), *c)).collect();
    files.push((dot.join("keybindings.json"), DEFAULT_KEYBINDINGS));
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
    let config = match std::fs::read_to_string(&cfg_path) {
        Ok(text) => match parse_config(&text, &cfg_path) {
            Ok(c) => {
                let errs = validate_config(&c, &cfg_path);
                if errs.is_empty() {
                    c
                } else {
                    errors.extend(errs);
                    Config::default()
                }
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
    // references from config to templates (non-fatal)
    let mut refs: Vec<(String, &str)> = vec![
        ("dispatcher.template".into(), config.dispatcher.template.as_str()),
        ("defaultTemplate".into(), config.default_template.as_str()),
    ];
    for (i, t) in config.tabs.iter().enumerate() {
        if let Some(n) = &t.template {
            refs.push((format!("tabs[{i}].template"), n.as_str()));
        }
    }
    for (path, name) in refs {
        if !templates.contains_key(name) {
            errors.push(sem_err(&cfg_path, path, format!("템플릿 '{name}'을(를) 찾을 수 없음")));
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
        let c = parse_config(
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
}
