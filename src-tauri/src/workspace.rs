//! Workspace model — the single source of truth for groups, tabs and panes (spec §5).
//!
//! Every change goes through [`WsCommand`] via [`Workspace::apply`]. Operations are pure: they
//! never touch processes and instead return [`Effects`] (panes to spawn/kill) for the launcher.

use crate::config::{check_relative, GroupSpec, Loaded, PaneSpec, Template};
use crate::docs::{self, DOC_APPLET};

/// `PaneKind::Applet { kind }` of the work-status dashboard (③ D1a).
pub const DASHBOARD_APPLET: &str = "dashboard";
/// Name of the dashboard tab (phase-1 dedupe when a tab already has it).
pub const DASHBOARD_TAB: &str = "상태";
use crate::layout::{Dir, LayoutNode, Side};
use crate::profiles::resolve_cwd;
use crate::roles::{session_name, Role};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub type GroupId = String;
pub type TabId = String;
pub type PaneId = String;

/// `slot` of the split holding a Manager tab's team area (Task 8, R15).
///
/// Layout representation: the team area is the split with `slot: "team"` — either one from the
/// template, or made by the first hire by splitting the standing pane to the right
/// (`horizontal`, sizes `[55, 45]`). Each team is one child of it (a "column"): the lead's leaf
/// alone, or a vertical split with `slot: "team:<id>"` holding the lead then its members.
/// Splits never have fewer than two children, so the column split appears with the first
/// member and collapses (normalize) when only the lead is left, and an auto-made area collapses
/// back into the standing pane when its last team goes. Team membership itself is `Pane.team`;
/// the slots only say where new team panes go.
pub const TEAM_SLOT: &str = "team";
/// Longest team id.
pub const TEAM_ID_MAX: usize = 24;
/// Sizes of a team area made by the first hire: standing pane | teams.
const TEAM_AREA_SIZES: [f32; 2] = [55.0, 45.0];

/// Team ids: 1–24 characters of `[a-z0-9-]` (they end up in session names and folder names);
/// `agentboard` is reserved (the app's sender name).
pub fn valid_team_id(id: &str) -> bool {
    !id.is_empty() && id != crate::roles::AGENTBOARD && id.len() <= TEAM_ID_MAX && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `slot` of a team's column split: `team:<id>`.
pub fn team_column_slot(team: &str) -> String {
    format!("{TEAM_SLOT}:{team}")
}

/// Member number from a session name `…-m<N>` (0 for leads / anything else).
fn member_no(p: &Pane) -> usize {
    if p.role != Some(Role::Member) {
        return 0;
    }
    p.session_name.as_deref().and_then(|s| s.rsplit_once("-m")).and_then(|(_, n)| n.parse().ok()).unwrap_or(0)
}

/// First split (pre-order) whose `slot` is `slot`.
fn find_slot_mut<'a>(n: &'a mut LayoutNode<PaneId>, slot: &str) -> Option<&'a mut LayoutNode<PaneId>> {
    if matches!(n, LayoutNode::Split { slot: Some(s), .. } if s == slot) {
        return Some(n);
    }
    match n {
        LayoutNode::Split { children, .. } => children.iter_mut().find_map(|c| find_slot_mut(c, slot)),
        LayoutNode::Leaf(_) => None,
    }
}

/// The leaf node holding `id`.
fn find_leaf_mut<'a>(n: &'a mut LayoutNode<PaneId>, id: &str) -> Option<&'a mut LayoutNode<PaneId>> {
    match n {
        LayoutNode::Leaf(x) if x == id => Some(n),
        LayoutNode::Leaf(_) => None,
        LayoutNode::Split { children, .. } => children.iter_mut().find_map(|c| find_leaf_mut(c, id)),
    }
}

/// Process state of a pane. Internally tagged on `state`:
/// `{"state":"starting"}` | `{"state":"running"}` | `{"state":"exited","code":3}` |
/// `{"state":"failed","message":"…"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum PaneStatus {
    Starting,
    Running,
    Exited { code: i32 },
    Failed { message: String },
}

/// What a pane shows. Wire shape: `{"type":"terminal"}` | `{"type":"applet","kind":"inbox"}`.
/// An applet is an app screen rendered by the frontend: it never has a process (the launcher
/// marks it `Running` without spawning) and its `agent` is empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PaneKind {
    #[default]
    Terminal,
    Applet { kind: String },
}

impl PaneKind {
    pub fn is_applet(&self) -> bool {
        matches!(self, PaneKind::Applet { .. })
    }
    /// A document tab's pane (`Applet { kind: "doc" }`).
    pub fn is_doc(&self) -> bool {
        matches!(self, PaneKind::Applet { kind } if kind == DOC_APPLET)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    pub id: PaneId,
    pub name: String,
    /// Owning tab.
    pub tab: TabId,
    /// Concrete profile name (already resolved against the config defaults).
    pub agent: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Absolute working directory (`resolve_cwd(tab.workspace, spec.cwd)`).
    pub cwd: PathBuf,
    pub prompt: Option<String>,
    pub status: PaneStatus,
    pub attention: bool,
    pub prompt_warning: bool,
    // ── phase 2 (all default when loading a phase-1 workspace.json) ──
    #[serde(default)]
    pub kind: PaneKind,
    /// Agent role; `None` for phase-1 panes, applets and extra panes of a role tab.
    #[serde(default)]
    pub role: Option<Role>,
    /// Team id for `Lead` / `Member` panes.
    #[serde(default)]
    pub team: Option<String>,
    /// Claude Code session name (`--name`, see `roles::session_name`); set for role panes only.
    #[serde(default)]
    pub session_name: Option<String>,
    /// Why the pane wants the user (`need_user`, a permission prompt, the folder-trust
    /// warning); `None` for a plain bell. Only meaningful while `attention` is on: every path
    /// that clears `attention` clears it too. Runtime state like `attention` (reset on load).
    #[serde(default)]
    pub attention_reason: Option<String>,
    /// Document panes (`Applet { kind: "doc" }`, Task 14) only: the document as opened
    /// (`<workspaceRoot>/<rel>`). Never trusted on use — `docs::resolve_doc` re-validates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_path: Option<PathBuf>,
}

impl Pane {
    /// Attention off, reason gone (focus, restart, load, `SetAttention{on:false}`).
    pub fn clear_attention(&mut self) {
        self.attention = false;
        self.attention_reason = None;
    }
}

/// Longest attention reason kept (characters); longer ones are cut.
pub const ATTENTION_REASON_MAX: usize = 200;

/// An attention reason as stored: control and format (Cf: zero-width, bidi, BOM, tags)
/// characters dropped (line breaks and tabs become spaces), whitespace runs collapsed,
/// trimmed, cut to [`ATTENTION_REASON_MAX`] characters. `None` when nothing is left.
pub fn clean_reason(s: &str) -> Option<String> {
    // Unicode general category Cf (format: zero-width, bidi controls, BOM, tags, …); std has
    // no category lookup, so the Cf ranges are listed (Unicode 15).
    let format = |c: char| {
        matches!(c,
            '\u{00AD}' | '\u{0600}'..='\u{0605}' | '\u{061C}' | '\u{06DD}' | '\u{070F}' | '\u{0890}'..='\u{0891}'
            | '\u{08E2}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}' | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}')
    };
    let mut out = String::new();
    for c in s.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c.is_control() || format(c) || (c == ' ' && (out.is_empty() || out.ends_with(' '))) {
            continue;
        }
        out.push(c);
    }
    let out: String = out.trim_end().chars().take(ATTENTION_REASON_MAX).collect();
    let out = out.trim_end().to_string();
    (!out.is_empty()).then_some(out)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: TabId,
    pub name: String,
    /// Directory relative to `workspaceRoot` as given by the user/config.
    pub dir: String,
    /// Template the tab was created from (a broken-off tab inherits its origin's template).
    pub template: String,
    pub is_dispatcher: bool,
    /// Absolute tab workspace (`workspaceRoot/dir`).
    pub workspace: PathBuf,
    pub layout: LayoutNode<PaneId>,
    pub last_focused_pane_id: PaneId,
    pub maximized_pane_id: Option<PaneId>,
    pub sync_input: bool,
    /// `Dispatcher` for the Dispatcher tab, `Manager` / `Handover` from `config.tabs[].role`,
    /// `None` for phase-1 tabs (and tabs created / broken off at run time).
    #[serde(default)]
    pub role: Option<Role>,
    /// Last member number issued per live team of this tab (`<tab>-<team>-m<N>`): numbers are
    /// never reused while the team exists; the entry goes away with the team (Task 8).
    #[serde(default)]
    pub team_seq: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub tab_ids: Vec<TabId>,
    pub active_tab_id: TabId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Focus {
    pub group_id: GroupId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub group_id: GroupId,
    pub tab_id: TabId,
}

/// Last issued number for each id kind (`g3` → group = 3). Persisted so restored workspaces
/// never reuse ids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub group: u64,
    pub tab: u64,
    pub pane: u64,
}

/// What a config reload did (`config_reload` reply: `{added, keptMissing, promptsUpdated}`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadReport {
    /// Config role tabs that were not open and now are (config order).
    pub added: Vec<String>,
    /// Open Manager / handover tabs that config.json no longer lists: reported, not closed.
    pub kept_missing: Vec<String>,
    /// Standing panes whose stored start prompt changed to the template's.
    pub prompts_updated: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub group_layout: LayoutNode<GroupId>,
    pub groups: BTreeMap<GroupId, Group>,
    pub tabs: BTreeMap<TabId, Tab>,
    pub panes: BTreeMap<PaneId, Pane>,
    pub focus: Focus,
    pub last_location: Option<Location>,
    pub group_maximized: Option<GroupId>,
    pub counters: Counters,
}

/// Process side effects of a command, executed by the launcher.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effects {
    /// Panes to start; the flag says whether to inject the pane's `prompt` once ready.
    pub spawn: Vec<(PaneId, bool)>,
    pub kill: Vec<PaneId>,
}

/// The one command surface (keyboard, mouse, MCP). Wire shape: `{"type":"splitPane","pane":"p1","dir":"vertical"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WsCommand {
    // tabs
    /// `group: None` = the focused group, and the new tab becomes the focus (user action).
    /// `group: Some(g)` (MCP) adds the tab in the background: neither focus nor any group's
    /// active tab changes (R12). `role`: `None` (plain tab) | `manager` | `handover`.
    CreateTab {
        name: String,
        dir: String,
        template: Option<String>,
        group: Option<GroupId>,
        #[serde(default)]
        role: Option<Role>,
    },
    CloseTab { tab: TabId },
    RenameTab { tab: TabId, name: String },
    // panes
    SplitPane { pane: PaneId, dir: Dir },
    ClosePane { pane: PaneId },
    RestartPane { pane: PaneId },
    /// Nightly refresh (Task 11): like [`WsCommand::RestartPane`] — kill, then spawn a new
    /// session (never `--resume`) — but the pane's start prompt is injected again, as at app
    /// start. Generation-safe through the launcher (the old process's late events are stale).
    RestartPaneFresh { pane: PaneId },
    ResizePane { pane: PaneId, side: Side, delta: f32 },
    /// `tab: None` targets the group layout.
    SetSizes { tab: Option<TabId>, path: Vec<usize>, sizes: Vec<f32> },
    BreakPaneToTab { pane: PaneId },
    // teams (phase 2, Task 8)
    /// Add a `lead` (new team) or `member` pane to team `team` of Manager tab `tab`, in the
    /// tab's team area (see [`TEAM_SLOT`]). Never changes focus.
    AddTeamPane {
        tab: TabId,
        team: String,
        role: Role,
        #[serde(default)]
        prompt: Option<String>,
    },
    /// Kill and remove every pane of team `team` in `tab`; its column (and an emptied team
    /// area) disappears.
    RemoveTeam { tab: TabId, team: String },
    // documents (Task 14)
    /// User action only (inbox detail, palette — no MCP tool sends it). `path` (absolute or
    /// relative to workspaceRoot) must be an existing `.md` inside workspaceRoot after
    /// canonicalization. Already open → SetFocus to that pane; else a new one-pane tab named
    /// after the file (phase-1 dedupe) in `group` (default: the focused group), which takes the
    /// focus either way.
    OpenDoc { path: String, group: Option<GroupId> },
    /// User action (palette, `app.dashboard` key). The work-status dashboard, at most one: an
    /// open one takes the focus (wherever it was moved); else a one-pane tab "상태" (dashboard
    /// applet, no process) at the end of `group` (default: the focused group), focused.
    OpenDashboard { group: Option<GroupId> },
    // state
    SetFocus { group: GroupId, tab: Option<TabId>, pane: Option<PaneId> },
    SetMaximized { tab: TabId, pane: Option<PaneId> },
    SetSyncInput { tab: TabId, on: bool },
    /// `on: false` also clears the reason; `on: true` with a reason replaces it (cleaned by
    /// [`clean_reason`]), without one keeps it (a bell does not erase a pending question).
    SetAttention {
        pane: PaneId,
        on: bool,
        #[serde(default)]
        reason: Option<String>,
    },
    // groups (Task 8)
    /// `index` is the final position after the tab is removed from its old spot; out of range appends.
    MoveTab { tab: TabId, group: GroupId, index: usize },
    SplitGroupWith { tab: TabId, group: GroupId, dir: Dir, before: bool },
    MergeGroup { src: GroupId, dst: GroupId },
    CloseGroup { group: GroupId },
    SetGroupMaximized { group: Option<GroupId> },
}

fn no_pane(id: &str) -> String {
    format!("창 '{id}'을(를) 찾을 수 없습니다")
}
fn no_tab(id: &str) -> String {
    format!("탭 '{id}'을(를) 찾을 수 없습니다")
}
fn no_group(id: &str) -> String {
    format!("그룹 '{id}'을(를) 찾을 수 없습니다")
}

impl Workspace {
    /// Build the initial workspace: the Dispatcher tab plus every config tab whose template
    /// exists (others are skipped; `load_all` already reported them), placed per `config.groups`
    /// (or one group "main"). Focus = first group in reading order, each group's first tab active.
    pub fn from_config(loaded: &Loaded) -> Workspace {
        let cfg = &loaded.config;
        let mut specs: Vec<GroupSpec> = Vec::new();
        let group_layout = match &cfg.groups {
            Some(gl) => gl.map(|g| {
                specs.push(g.clone());
                format!("g{}", specs.len())
            }),
            None => {
                specs.push(GroupSpec { name: "main".into(), tabs: Vec::new() });
                LayoutNode::Leaf("g1".to_string())
            }
        };
        let groups = specs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let id = format!("g{}", i + 1);
                (id.clone(), Group { id, name: s.name.clone(), tab_ids: Vec::new(), active_tab_id: String::new() })
            })
            .collect();
        let mut ws = Workspace {
            group_layout,
            groups,
            tabs: BTreeMap::new(),
            panes: BTreeMap::new(),
            focus: Focus { group_id: String::new() },
            last_location: None,
            group_maximized: None,
            counters: Counters { group: specs.len() as u64, tab: 0, pane: 0 },
        };

        // tabs — Dispatcher first
        let dtpl = loaded
            .templates
            .get(&cfg.dispatcher.template)
            .expect("load_all guarantees the dispatcher template");
        let mut made: Vec<(String, TabId)> =
            vec![("dispatcher".into(), ws.build_tab("dispatcher", "", dtpl, true, Some(Role::Dispatcher), loaded))];
        for t in &cfg.tabs {
            let tn = t.template.as_deref().unwrap_or(&cfg.default_template);
            let Some(tpl) = loaded.templates.get(tn) else { continue };
            made.push((t.name.clone(), ws.build_tab(&t.name, &t.dir, tpl, false, t.role, loaded)));
        }

        // placement
        let mut placed = BTreeSet::new();
        for (i, s) in specs.iter().enumerate() {
            let gid = format!("g{}", i + 1);
            for name in &s.tabs {
                if let Some((_, tid)) = made.iter().find(|(n, _)| n == name) {
                    if placed.insert(tid.clone()) {
                        ws.groups.get_mut(&gid).unwrap().tab_ids.push(tid.clone());
                    }
                }
            }
        }
        let default_gid = cfg
            .default_group
            .as_ref()
            .and_then(|n| specs.iter().position(|s| &s.name == n))
            .map(|i| format!("g{}", i + 1))
            .unwrap_or_else(|| ws.group_layout.leaves()[0].clone());
        for (_, tid) in &made {
            if placed.contains(tid) {
                continue;
            }
            let disp = ws.tabs[tid].is_dispatcher;
            let g = ws.groups.get_mut(&default_gid).unwrap();
            if disp {
                g.tab_ids.insert(0, tid.clone());
            } else {
                g.tab_ids.push(tid.clone());
            }
        }
        for g in ws.groups.values_mut() {
            g.active_tab_id = g.tab_ids.first().cloned().unwrap_or_default();
        }
        for gid in ws.group_layout.leaves() {
            ws.remove_group_if_empty(&gid);
        }
        ws.focus.group_id = ws.group_layout.leaves()[0].clone();
        ws
    }

    /// Apply one command. On `Err` (Korean message) the model is left untouched: the command
    /// runs on a clone that replaces `self` only on success.
    pub fn apply(&mut self, cmd: WsCommand, loaded: &Loaded) -> Result<Effects, String> {
        let mut next = self.clone();
        // commands that create or remove role/team panes re-check the persisted invariants, so a
        // broken state becomes an Err here instead of a corrupt workspace.json on the next start
        let recheck = matches!(
            cmd,
            WsCommand::CreateTab { .. }
                | WsCommand::AddTeamPane { .. }
                | WsCommand::RemoveTeam { .. }
                | WsCommand::ClosePane { .. }
                | WsCommand::OpenDoc { .. }
                | WsCommand::OpenDashboard { .. }
        );
        let fx = next.apply_inner(cmd, loaded)?;
        if recheck {
            next.check_roles()?;
        }
        next.sanitize_last_location();
        *self = next;
        Ok(fx)
    }

    /// Unknown pane ids are ignored (the process may outlive its pane).
    pub fn set_status(&mut self, pane: &str, st: PaneStatus) {
        if let Some(p) = self.panes.get_mut(pane) {
            p.status = st;
        }
    }

    /// Marks that the first prompt was injected without a ready signal. Unknown ids ignored.
    pub fn set_prompt_warning(&mut self, pane: &str, on: bool) {
        if let Some(p) = self.panes.get_mut(pane) {
            p.prompt_warning = on;
        }
    }

    /// Make a deserialized workspace safe to use (R11): normalize the group layout and every tab
    /// layout (serde does not), check referential integrity, and raise each id counter to at least
    /// the highest existing id number (a hand-edited file may be behind). `Err` (Korean message)
    /// means the file must be treated as corrupt; `self` may be partially normalized then.
    pub fn normalize_and_validate(&mut self) -> Result<(), String> {
        self.group_layout.normalize();
        for t in self.tabs.values_mut() {
            t.layout.normalize();
        }
        let bad = |m: String| Err(format!("작업 상태 파일의 구조가 올바르지 않습니다: {m}"));

        // groups <-> group layout
        let leaves = self.group_layout.leaves();
        let leaf_set: BTreeSet<&GroupId> = leaves.iter().collect();
        if leaf_set.len() != leaves.len() || leaf_set.len() != self.groups.len() || !self.groups.keys().all(|g| leaf_set.contains(g)) {
            return bad("그룹 배치와 그룹 목록이 맞지 않습니다".into());
        }
        // tabs <-> groups
        let mut seen_tabs = BTreeSet::new();
        for (gid, g) in &self.groups {
            if *gid != g.id || g.tab_ids.is_empty() || !g.tab_ids.contains(&g.active_tab_id) {
                return bad(format!("그룹 '{gid}'"));
            }
            for t in &g.tab_ids {
                if !self.tabs.contains_key(t) || !seen_tabs.insert(t) {
                    return bad(format!("그룹 '{gid}'의 탭 '{t}'"));
                }
            }
        }
        if seen_tabs.len() != self.tabs.len() {
            return bad("어느 그룹에도 속하지 않은 탭이 있습니다".into());
        }
        // panes <-> tab layouts
        let mut seen_panes = BTreeSet::new();
        for (tid, t) in &self.tabs {
            if *tid != t.id {
                return bad(format!("탭 '{tid}'"));
            }
            let leaves = t.layout.leaves();
            if !leaves.contains(&t.last_focused_pane_id) || t.maximized_pane_id.as_ref().is_some_and(|m| !leaves.contains(m)) {
                return bad(format!("탭 '{tid}'의 포커스 창"));
            }
            for p in leaves {
                match self.panes.get(&p) {
                    Some(pane) if pane.tab == *tid && seen_panes.insert(p.clone()) => {}
                    _ => return bad(format!("탭 '{tid}'의 창 '{p}'")),
                }
            }
        }
        if seen_panes.len() != self.panes.len() || self.panes.iter().any(|(k, p)| *k != p.id) {
            return bad("어느 탭에도 속하지 않은 창이 있습니다".into());
        }
        if self.tabs.values().filter(|t| t.is_dispatcher).count() != 1 {
            return bad("Dispatcher 탭이 없습니다".into());
        }
        if let Err(m) = self.check_roles() {
            return bad(m);
        }
        // focus
        if !self.groups.contains_key(&self.focus.group_id) {
            return bad("포커스 그룹".into());
        }
        if self.group_maximized.as_ref().is_some_and(|g| !self.groups.contains_key(g)) {
            return bad("최대화된 그룹".into());
        }
        if let Some(l) = &self.last_location {
            if !self.groups.get(&l.group_id).is_some_and(|g| g.tab_ids.contains(&l.tab_id)) {
                return bad("이전 위치".into());
            }
        }
        // counters never behind existing ids
        let max_of = |ids: Vec<&String>| ids.iter().filter_map(|i| i.get(1..)?.parse::<u64>().ok()).max().unwrap_or(0);
        self.counters.group = self.counters.group.max(max_of(self.groups.keys().collect()));
        self.counters.tab = self.counters.tab.max(max_of(self.tabs.keys().collect()));
        self.counters.pane = self.counters.pane.max(max_of(self.panes.keys().collect()));
        Ok(())
    }

    /// Phase-2 invariants: applets carry no agent/role/team/session name; a document path is
    /// set exactly on document panes; the Dispatcher role
    /// belongs to the Dispatcher tab only; at most one handover tab; per tab at most one
    /// standing-role pane, and its role equals the tab's role; session names are unique.
    fn check_roles(&self) -> Result<(), String> {
        for (pid, p) in &self.panes {
            if p.kind.is_applet() && (p.role.is_some() || p.team.is_some() || p.session_name.is_some() || !p.agent.is_empty()) {
                return Err(format!("애플릿 창 '{pid}'에 에이전트·역할·팀·세션 이름이 있습니다"));
            }
            if p.kind.is_doc() != p.doc_path.is_some() {
                return Err(format!("창 '{pid}'의 문서 경로가 창 종류와 맞지 않습니다"));
            }
        }
        if self.tabs.values().filter(|t| t.role == Some(Role::Handover)).count() > 1 {
            return Err("handover 탭이 둘 이상입니다".into());
        }
        for (tid, t) in &self.tabs {
            // Dispatcher role <=> Dispatcher tab (a phase-1 Dispatcher tab has no role); tab
            // roles are standing roles only
            let role_ok = match t.role {
                None => true,
                Some(Role::Dispatcher) => t.is_dispatcher,
                Some(r) => !t.is_dispatcher && r.is_standing(),
            };
            if !role_ok {
                return Err(format!("탭 '{tid}'의 역할"));
            }
            let standing: Vec<&Pane> = t
                .layout
                .leaves()
                .iter()
                .filter_map(|p| self.panes.get(p))
                .filter(|p| p.role.is_some_and(|r| r.is_standing()))
                .collect();
            if standing.len() > 1 {
                return Err(format!("탭 '{tid}'에 상시 에이전트 창이 둘 이상입니다"));
            }
            if let Some(p) = standing.first() {
                if p.role != t.role {
                    return Err(format!("창 '{}'의 역할이 탭 '{tid}'의 역할과 다릅니다", p.id));
                }
            }
        }
        // teams: lead/member panes ⇔ a valid team id, a session name, a Manager tab; one lead per
        // team, and no member without its lead
        let mut leads: BTreeMap<(&TabId, &String), usize> = BTreeMap::new();
        let mut member_teams = BTreeSet::new();
        for (pid, p) in &self.panes {
            let team_role = matches!(p.role, Some(Role::Lead | Role::Member));
            match (&p.team, team_role) {
                (None, false) => {}
                (Some(team), true) => {
                    if !valid_team_id(team) || p.session_name.is_none() || p.kind.is_applet() {
                        return Err(format!("팀 창 '{pid}'의 팀 id·세션 이름"));
                    }
                    if self.tabs.get(&p.tab).map(|t| t.role) != Some(Some(Role::Manager)) {
                        return Err(format!("팀 창 '{pid}'이(가) Manager 탭 밖에 있습니다"));
                    }
                    if p.role == Some(Role::Lead) {
                        *leads.entry((&p.tab, team)).or_default() += 1;
                    } else {
                        member_teams.insert((&p.tab, team));
                    }
                }
                _ => return Err(format!("창 '{pid}'의 역할과 팀이 맞지 않습니다")),
            }
        }
        if let Some(((_, team), _)) = leads.iter().find(|(_, n)| **n > 1) {
            return Err(format!("팀 '{team}'에 리드가 둘 이상입니다"));
        }
        if let Some((_, team)) = member_teams.iter().find(|k| !leads.contains_key(*k)) {
            return Err(format!("팀 '{team}'에 리드가 없습니다"));
        }
        let mut names = BTreeSet::new();
        for p in self.panes.values() {
            if let Some(n) = &p.session_name {
                if n.eq_ignore_ascii_case(crate::roles::AGENTBOARD) {
                    return Err(format!("세션 이름 '{n}'은(는) 예약되어 있습니다"));
                }
                if !names.insert(n) {
                    return Err(format!("세션 이름 '{n}'이(가) 중복됩니다"));
                }
            }
        }
        Ok(())
    }

    /// Runtime state of a restored workspace: every pane `Starting` with attention and prompt
    /// warning cleared. Returns the spawn effects for all panes, without prompt injection.
    pub fn reset_runtime_state(&mut self) -> Effects {
        for p in self.panes.values_mut() {
            p.status = PaneStatus::Starting;
            p.clear_attention();
            p.prompt_warning = false;
        }
        Effects { spawn: self.panes.keys().map(|id| (id.clone(), false)).collect(), kill: Vec::new() }
    }

    /// Restore rules (spec §9, Task 12) for a workspace loaded from workspace.json; config is the
    /// source of truth for which tabs carry a role. Returns the spawn effects.
    ///
    /// 1. Runtime state reset ([`Workspace::reset_runtime_state`]).
    /// 2. Roles re-derived from `loaded.config` (phase-1 files have none; config may have changed):
    ///    the Dispatcher tab is always `Dispatcher`; another tab takes the role of the config tab
    ///    with the **same name and dir** (`manager` / `handover`, or none). A tab with no such
    ///    entry (removed from config, or its dir changed) becomes a plain phase-1 tab.
    /// 3. Team panes (lead / member) are dropped with their columns (leaves removed, layouts
    ///    normalized: no single-child splits, an auto-made team area collapses into the Manager
    ///    pane) and every `team_seq` is cleared — Managers re-hire from `teams/*/brief.md`.
    ///    Team docs on disk are not touched. A tab left with no pane at all is removed.
    /// 4. In a role tab the standing pane keeps its place: the pane that already had the
    ///    standing role, else the terminal pane named like the template's focus (or first
    ///    terminal) pane, else the first terminal pane in layout order. It gets the role and
    ///    `session_name` = the current tab name (a fresh session, so a renamed tab's name is
    ///    used now); its prompt is re-read from the template pane of the same name when the
    ///    template still has one. Every other pane gets no role / session name.
    ///    A tab whose focused pane was a dropped team pane gets its standing pane focused.
    /// 5. R27: config role tabs (`manager` / `handover`) missing from workspace.json are added
    ///    as at first start (template from config, else defaultTemplate) at the end of
    ///    `defaultGroup` (else the focused group) — no focus or active-tab change. A config tab
    ///    whose name is already used by another (plain) tab, whose template is missing, or a
    ///    second handover is skipped with a Korean message in the returned list (user tabs are
    ///    never renamed).
    /// 6. Effects: every pane is spawned; standing role panes with `inject = true` (new session +
    ///    start prompt, as at first start), all panes of added tabs too (as `initial_effects`),
    ///    all others without injection (phase-1 restore; this includes extra agent panes of a
    ///    role tab — only standing agents follow the start procedure, spec §9).
    ///    Applets (inbox, documents) get the usual no-process spawn.
    pub fn restore_roles(&mut self, loaded: &Loaded) -> (Effects, Vec<String>) {
        self.reset_runtime_state();
        let cfg = &loaded.config;
        // 2. tab roles from config
        for t in self.tabs.values_mut() {
            t.role = if t.is_dispatcher {
                Some(Role::Dispatcher)
            } else {
                cfg.tabs.iter().find(|s| s.name == t.name && s.dir == t.dir).and_then(|s| s.role)
            };
            t.team_seq.clear();
        }
        // a handover tab's workspace must be the root (config guarantees dir "" for it)
        // 3. drop team panes
        let team: Vec<PaneId> =
            self.panes.values().filter(|p| p.team.is_some() || matches!(p.role, Some(Role::Lead | Role::Member))).map(|p| p.id.clone()).collect();
        let mut refocus: BTreeSet<TabId> = BTreeSet::new();
        for id in &team {
            let tab = self.panes[id].tab.clone();
            self.panes.remove(id);
            let Some(t) = self.tabs.get_mut(&tab) else { continue };
            match t.layout.remove_leaf(id) {
                Some(None) => {
                    self.detach_tab(&tab);
                }
                _ => {
                    t.layout.normalize();
                    let leaves = t.layout.leaves();
                    if !leaves.contains(&t.last_focused_pane_id) {
                        t.last_focused_pane_id = leaves[0].clone();
                        refocus.insert(tab.clone());
                    }
                    if t.maximized_pane_id.as_ref().is_some_and(|m| !leaves.contains(m)) {
                        t.maximized_pane_id = None;
                    }
                }
            }
        }
        if !self.groups.contains_key(&self.focus.group_id) {
            self.focus.group_id = self.group_layout.leaves()[0].clone();
        }
        if self.group_maximized.as_ref().is_some_and(|g| !self.groups.contains_key(g)) {
            self.group_maximized = None;
        }
        self.sanitize_last_location();
        // 4. standing panes
        let mut standing: BTreeSet<PaneId> = BTreeSet::new();
        let tab_ids: Vec<TabId> = self.tabs.keys().cloned().collect();
        for tid in tab_ids {
            let t = &self.tabs[&tid];
            let (role, name, leaves) = (t.role, t.name.clone(), t.layout.leaves());
            let tpl = loaded.templates.get(&t.template);
            let pick = role.and_then(|_| {
                let terminal = |p: &&PaneId| self.panes.get(*p).is_some_and(|p| !p.kind.is_applet());
                let had = leaves.iter().filter(terminal).find(|p| self.panes[*p].role.is_some_and(|r| r.is_standing()));
                let by_tpl = tpl.and_then(|tpl| {
                    let specs = tpl.layout.leaves();
                    let s = specs.iter().find(|s| s.focus && s.applet.is_none()).or_else(|| specs.iter().find(|s| s.applet.is_none()))?;
                    leaves.iter().filter(terminal).find(|p| self.panes[*p].name == s.name)
                });
                had.or(by_tpl).or_else(|| leaves.iter().find(terminal)).cloned()
            });
            for pid in &leaves {
                let Some(p) = self.panes.get_mut(pid) else { continue };
                if Some(pid) == pick.as_ref() {
                    p.role = role;
                    p.session_name = role.map(|r| session_name(&name, r, None, 0));
                    if let Some(spec) = tpl.and_then(|tpl| tpl.layout.leaves().into_iter().find(|s| s.name == p.name && s.applet.is_none())) {
                        p.prompt = spec.prompt.clone();
                    }
                    standing.insert(pid.clone());
                } else {
                    p.role = None;
                    p.session_name = None;
                }
                p.team = None;
            }
            if let (Some(pick), true) = (&pick, refocus.contains(&tid)) {
                self.tabs.get_mut(&tid).unwrap().last_focused_pane_id = pick.clone();
            }
        }
        // 5. config role tabs missing from workspace.json (R27)
        let (added, _, errors) = self.add_missing_role_tabs(loaded, "다시 시작하세요");
        // 6. effects
        let spawn = self.panes.keys().map(|id| (id.clone(), standing.contains(id) || added.contains(id))).collect();
        (Effects { spawn, kill: Vec::new() }, errors)
    }

    /// Step 5 of [`Workspace::restore_roles`] (R27), also used by a config reload: open the
    /// config role tabs (`manager` / `handover`) that are not open — same name, dir and role —
    /// as at first start (template from config, else defaultTemplate) at the end of
    /// `defaultGroup` (else the focused group), with no focus or active-tab change. A config tab
    /// whose name is used by another tab, whose template is missing, or a second handover is
    /// skipped with a Korean message (`retry` ends the advice: what the user does after fixing
    /// it). Returns the added panes, the added tab names (config order) and the messages.
    fn add_missing_role_tabs(&mut self, loaded: &Loaded, retry: &str) -> (BTreeSet<PaneId>, Vec<String>, Vec<String>) {
        let cfg = &loaded.config;
        let mut errors = Vec::new();
        let mut added: BTreeSet<PaneId> = BTreeSet::new();
        let mut names = Vec::new();
        for spec in cfg.tabs.iter().filter(|s| s.role.is_some()) {
            if self.tabs.values().any(|t| t.name == spec.name && t.dir == spec.dir && t.role == spec.role) {
                continue;
            }
            if self.tabs.values().any(|t| t.name == spec.name) {
                errors.push(format!(
                    "config.json의 역할 탭 '{}'을(를) 열지 않았습니다 — 같은 이름의 다른 탭이 이미 있습니다(그 탭의 이름을 바꾸거나 닫은 뒤 {retry})",
                    spec.name
                ));
                continue;
            }
            if spec.role == Some(Role::Handover) && self.tabs.values().any(|t| t.role == Some(Role::Handover)) {
                errors.push(format!("config.json의 handover 탭 '{}'을(를) 열지 않았습니다 — handover 탭이 이미 있습니다", spec.name));
                continue;
            }
            let tn = spec.template.as_deref().unwrap_or(&cfg.default_template);
            let Some(tpl) = loaded.templates.get(tn) else {
                errors.push(format!("config.json의 역할 탭 '{}'을(를) 열지 않았습니다 — 템플릿 '{tn}'이(가) 없습니다", spec.name));
                continue;
            };
            let tid = self.build_tab(&spec.name, &spec.dir, tpl, false, spec.role, loaded);
            added.extend(self.tabs[&tid].layout.leaves());
            names.push(spec.name.clone());
            let leaves = self.group_layout.leaves();
            let gid = cfg
                .default_group
                .as_ref()
                .and_then(|n| leaves.iter().find(|g| self.groups.get(*g).is_some_and(|g| &g.name == n)).cloned())
                .unwrap_or_else(|| self.focus.group_id.clone());
            self.groups.get_mut(&gid).unwrap().tab_ids.push(tid);
        }
        (added, names, errors)
    }

    /// Config reload: each role tab's standing pane takes its start prompt from the tab's
    /// template again (the template pane of the same name, as restore step 4 does). Nothing is
    /// restarted — the prompt is typed at the pane's next fresh start (nightly refresh, restart
    /// with a prompt, app start). Returns how many prompts changed.
    fn refresh_standing_prompts(&mut self, loaded: &Loaded) -> usize {
        let mut changed = 0;
        for t in self.tabs.values() {
            let Some(tpl) = loaded.templates.get(&t.template) else { continue };
            for pid in t.layout.leaves() {
                let Some(p) = self.panes.get_mut(&pid).filter(|p| p.role.is_some_and(|r| r.is_standing()) && !p.kind.is_applet()) else {
                    continue;
                };
                let Some(spec) = tpl.layout.leaves().into_iter().find(|s| s.name == p.name && s.applet.is_none()) else { continue };
                if p.prompt != spec.prompt {
                    p.prompt = spec.prompt.clone();
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Config reload with the newly loaded `loaded` (no restart, the model's third mutation path
    /// besides `apply` and restore prep): open the config role tabs that are not open
    /// ([`Workspace::add_missing_role_tabs`]), refresh the standing panes' stored start prompts,
    /// and report open Manager / handover tabs that config.json no longer lists (they stay open
    /// with their roles). All or nothing: a tab that cannot be opened, or a model that would not
    /// validate, is an `Err` and nothing changes. Effects spawn only the added tabs' panes
    /// (new sessions with their start prompts); no other pane is touched.
    pub fn reload_roles(&mut self, loaded: &Loaded) -> Result<(Effects, ReloadReport), Vec<String>> {
        let mut next = self.clone();
        let cfg = &loaded.config;
        let kept_missing: Vec<String> = next
            .tabs
            .values()
            .filter(|t| matches!(t.role, Some(Role::Manager | Role::Handover)))
            .filter(|t| !cfg.tabs.iter().any(|s| s.name == t.name && s.dir == t.dir && s.role == t.role))
            .map(|t| t.name.clone())
            .collect();
        let (added, names, errors) = next.add_missing_role_tabs(loaded, "설정을 다시 읽으세요");
        if !errors.is_empty() {
            return Err(errors);
        }
        let prompts_updated = next.refresh_standing_prompts(loaded);
        next.normalize_and_validate().map_err(|e| vec![format!("설정을 반영할 수 없습니다 — {e}")])?;
        *self = next;
        let spawn = added.into_iter().map(|id| (id, true)).collect();
        Ok((Effects { spawn, kill: Vec::new() }, ReloadReport { added: names, kept_missing, prompts_updated }))
    }

    /// Panes whose process is alive (`starting` or `running`). Applets have no process.
    pub fn running_count(&self) -> usize {
        self.panes
            .values()
            .filter(|p| !p.kind.is_applet() && matches!(p.status, PaneStatus::Starting | PaneStatus::Running))
            .count()
    }

    pub fn pane_by_id(&self, id: &str) -> Option<&Pane> {
        self.panes.get(id)
    }

    /// Group currently containing `tab`.
    pub fn group_of_tab(&self, tab: &str) -> Option<&GroupId> {
        self.groups.values().find(|g| g.tab_ids.iter().any(|t| t == tab)).map(|g| &g.id)
    }

    fn apply_inner(&mut self, cmd: WsCommand, loaded: &Loaded) -> Result<Effects, String> {
        use WsCommand::*;
        let mut fx = Effects::default();
        match cmd {
            CreateTab { name, dir, template, group, role } => {
                self.check_tab_name(&name, None)?;
                if let Some(m) = check_relative(&dir) {
                    return Err(format!("디렉토리 '{dir}': {m}"));
                }
                match role {
                    None | Some(Role::Manager) => {}
                    Some(Role::Handover) => {
                        if !dir.is_empty() {
                            return Err("handover 탭의 디렉토리는 빈 값이어야 합니다 — 작업공간은 workspaceRoot".into());
                        }
                        if self.tabs.values().any(|t| t.role == Some(Role::Handover)) {
                            return Err("handover 탭은 하나만 둘 수 있습니다".into());
                        }
                    }
                    Some(r) => return Err(format!("'{}' 역할의 탭은 만들 수 없습니다", r.as_str())),
                }
                // a role tab's standing pane takes the tab name as its session name; a renamed tab
                // keeps its old session name (R15) and team panes have their own, so check them all
                if role.is_some() && self.session_taken(&name) {
                    return Err(format!("세션 이름 '{name}'이(가) 이미 다른 에이전트가 쓰고 있습니다 — 다른 탭 이름을 쓰세요"));
                }
                let background = group.is_some();
                let gid = match group {
                    Some(g) => {
                        self.group(&g)?;
                        g
                    }
                    None => self.focus.group_id.clone(),
                };
                let tn = template.unwrap_or_else(|| loaded.config.default_template.clone());
                let tpl = loaded.templates.get(&tn).ok_or_else(|| format!("템플릿 '{tn}'을(를) 찾을 수 없습니다"))?;
                let tid = self.build_tab(&name, &dir, tpl, false, role, loaded);
                fx.spawn = self.tabs[&tid].layout.leaves().into_iter().map(|p| (p, true)).collect();
                self.groups.get_mut(&gid).unwrap().tab_ids.push(tid.clone());
                // R12: an explicit group (MCP) never moves focus or any group's active tab
                if !background {
                    self.activate_tab(&gid, &tid);
                }
            }
            CloseTab { tab } => {
                let t = self.tab(&tab)?;
                if t.is_dispatcher {
                    return Err("Dispatcher 탭은 닫을 수 없습니다".into());
                }
                fx.kill = t.layout.leaves();
                for p in &fx.kill {
                    self.panes.remove(p);
                }
                self.detach_tab(&tab);
            }
            RenameTab { tab, name } => {
                if self.tab(&tab)?.is_dispatcher {
                    return Err("Dispatcher 탭의 이름은 바꿀 수 없습니다".into());
                }
                self.check_tab_name(&name, Some(&tab))?;
                self.tabs.get_mut(&tab).unwrap().name = name;
            }
            SplitPane { pane, dir } => {
                let src = self.pane(&pane)?.clone();
                let t = &self.tabs[&src.tab];
                let names: BTreeSet<String> = t.layout.leaves().iter().map(|p| self.panes[p].name.clone()).collect();
                let name = (1..).map(|n| format!("pane-{n}")).find(|n| !names.contains(n)).unwrap();
                let cwd = t.workspace.clone();
                // the new pane is always a plain terminal (an applet has no agent to copy)
                let agent = if src.kind.is_applet() { loaded.config.default_agent.clone() } else { src.agent.clone() };
                let id = self.new_pane_id();
                self.panes.insert(
                    id.clone(),
                    Pane {
                        id: id.clone(),
                        name,
                        tab: src.tab.clone(),
                        agent,
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd,
                        prompt: None,
                        status: PaneStatus::Starting,
                        attention: false,
                        prompt_warning: false,
                        attention_reason: None,
                        kind: PaneKind::Terminal,
                        role: None,
                        team: None,
                        session_name: None,
                        doc_path: None,
                    },
                );
                let t = self.tabs.get_mut(&src.tab).unwrap();
                t.layout.split_leaf(&pane, id.clone(), dir, false);
                t.last_focused_pane_id = id.clone();
                t.maximized_pane_id = None;
                fx.spawn.push((id, false));
            }
            ClosePane { pane } => {
                self.check_not_standing(&pane)?;
                if self.pane(&pane)?.role == Some(Role::Lead) {
                    return Err("팀 리드 창은 따로 닫을 수 없습니다 — dismiss_team으로 팀을 해산하세요".into());
                }
                let tid = self.pane(&pane)?.tab.clone();
                let t = self.tabs.get_mut(&tid).unwrap();
                match t.layout.remove_leaf(&pane) {
                    Some(Some(cand)) => {
                        if t.last_focused_pane_id == pane {
                            t.last_focused_pane_id = cand;
                        }
                        if t.maximized_pane_id.as_ref() == Some(&pane) {
                            t.maximized_pane_id = None;
                        }
                    }
                    Some(None) => {
                        if t.is_dispatcher {
                            return Err("Dispatcher 탭의 마지막 창은 닫을 수 없습니다".into());
                        }
                        self.detach_tab(&tid);
                    }
                    None => return Err(no_pane(&pane)),
                }
                self.panes.remove(&pane);
                fx.kill.push(pane);
            }
            RestartPane { pane } => {
                self.pane(&pane)?;
                self.set_status(&pane, PaneStatus::Starting);
                let p = self.panes.get_mut(&pane).unwrap();
                p.clear_attention();
                p.prompt_warning = false;
                fx.kill.push(pane.clone());
                fx.spawn.push((pane, false));
            }
            RestartPaneFresh { pane } => {
                self.pane(&pane)?;
                self.set_status(&pane, PaneStatus::Starting);
                let p = self.panes.get_mut(&pane).unwrap();
                p.clear_attention();
                p.prompt_warning = false;
                fx.kill.push(pane.clone());
                fx.spawn.push((pane, true));
            }
            ResizePane { pane, side, delta } => {
                let tid = self.pane(&pane)?.tab.clone();
                if !self.tabs.get_mut(&tid).unwrap().layout.resize(&pane, side, delta) {
                    // no border inside the tab → move the group border (no-op when none exists)
                    if let Some(gid) = self.group_of_tab(&tid).cloned() {
                        self.group_layout.resize(&gid, side, delta);
                    }
                }
            }
            SetSizes { tab, path, sizes } => {
                let layout_ok = match &tab {
                    Some(t) => {
                        self.tab(t)?;
                        self.tabs.get_mut(t).unwrap().layout.set_sizes(&path, sizes)
                    }
                    None => self.group_layout.set_sizes(&path, sizes),
                };
                if !layout_ok {
                    return Err(format!("분할 경로 {path:?}이(가) 잘못되었습니다"));
                }
            }
            BreakPaneToTab { pane } => {
                self.check_not_standing(&pane)?;
                if self.pane(&pane)?.team.is_some() {
                    return Err("팀 창은 다른 탭으로 떼어낼 수 없습니다 — 팀은 Manager 탭 안에서만 일합니다".into());
                }
                let tid = self.pane(&pane)?.tab.clone();
                let pane_name = self.panes[&pane].name.clone();
                let origin = self.tabs[&tid].clone();
                if origin.is_dispatcher && origin.layout.leaf_count() == 1 {
                    return Err("Dispatcher 탭의 마지막 창은 떼어낼 수 없습니다".into());
                }
                let name = self.unique_tab_name(&format!("{}-{}", origin.name, pane_name));
                let t = self.tabs.get_mut(&tid).unwrap();
                let origin_empty = match t.layout.remove_leaf(&pane) {
                    Some(Some(cand)) => {
                        if t.last_focused_pane_id == pane {
                            t.last_focused_pane_id = cand;
                        }
                        if t.maximized_pane_id.as_ref() == Some(&pane) {
                            t.maximized_pane_id = None;
                        }
                        false
                    }
                    Some(None) => true,
                    None => return Err(no_pane(&pane)),
                };
                let nid = self.new_tab_id();
                self.tabs.insert(
                    nid.clone(),
                    Tab {
                        id: nid.clone(),
                        name,
                        dir: origin.dir.clone(),
                        template: origin.template.clone(),
                        is_dispatcher: false,
                        workspace: origin.workspace.clone(),
                        layout: LayoutNode::Leaf(pane.clone()),
                        last_focused_pane_id: pane.clone(),
                        maximized_pane_id: None,
                        sync_input: false,
                        // a broken-off tab is a plain tab; the pane keeps its own role
                        role: None,
                        team_seq: BTreeMap::new(),
                    },
                );
                self.panes.get_mut(&pane).unwrap().tab = nid.clone();
                let gid = self.group_of_tab(&tid).cloned().ok_or_else(|| no_tab(&tid))?;
                let g = self.groups.get_mut(&gid).unwrap();
                let idx = g.tab_ids.iter().position(|t| *t == tid).unwrap();
                g.tab_ids.insert(idx + 1, nid.clone());
                self.activate_tab(&gid, &nid);
                if origin_empty {
                    self.detach_tab(&tid);
                }
            }
            AddTeamPane { tab, team, role, prompt } => {
                let id = self.add_team_pane(&tab, &team, role, prompt, loaded)?;
                fx.spawn.push((id, true));
            }
            RemoveTeam { tab, team } => fx.kill = self.remove_team(&tab, &team)?,
            OpenDoc { path, group } => fx = self.open_doc(&path, group, loaded)?,
            OpenDashboard { group } => fx = self.open_dashboard(group, loaded)?,
            SetFocus { group, tab, pane } => {
                let g = self.group(&group)?;
                let tid = match (&tab, &pane) {
                    (Some(t), _) => {
                        self.tab(t)?;
                        if !g.tab_ids.contains(t) {
                            return Err(format!("탭 '{t}'이(가) 그룹 '{group}'에 없습니다"));
                        }
                        t.clone()
                    }
                    (None, Some(p)) => {
                        let pt = self.pane(p)?.tab.clone();
                        if !g.tab_ids.contains(&pt) {
                            return Err(format!("창 '{p}'이(가) 그룹 '{group}'에 없습니다"));
                        }
                        pt
                    }
                    (None, None) => g.active_tab_id.clone(),
                };
                if let Some(p) = &pane {
                    self.pane(p)?;
                    if !self.tabs[&tid].layout.contains(p) {
                        return Err(format!("창 '{p}'이(가) 탭 '{tid}'에 없습니다"));
                    }
                }
                let prev = self.focus_location();
                if let Some(p) = pane {
                    self.tabs.get_mut(&tid).unwrap().last_focused_pane_id = p;
                }
                self.move_focus(prev, &group, &tid);
            }
            SetMaximized { tab, pane } => {
                let t = self.tab(&tab)?;
                if let Some(p) = &pane {
                    if !t.layout.contains(p) {
                        return Err(format!("창 '{p}'이(가) 탭 '{tab}'에 없습니다"));
                    }
                }
                let t = self.tabs.get_mut(&tab).unwrap();
                if let Some(p) = &pane {
                    t.last_focused_pane_id = p.clone();
                }
                t.maximized_pane_id = pane;
            }
            SetSyncInput { tab, on } => {
                self.tab(&tab)?;
                self.tabs.get_mut(&tab).unwrap().sync_input = on;
            }
            SetAttention { pane, on, reason } => {
                self.pane(&pane)?;
                let p = self.panes.get_mut(&pane).unwrap();
                if on {
                    p.attention = true;
                    if let Some(r) = reason.as_deref().and_then(clean_reason) {
                        p.attention_reason = Some(r);
                    }
                } else {
                    p.clear_attention();
                }
            }
            MoveTab { tab, group, index } => {
                self.tab(&tab)?;
                self.group(&group)?;
                let src = self.group_of_tab(&tab).cloned().ok_or_else(|| no_tab(&tab))?;
                let prev = self.focus_location();
                if src == group {
                    let g = self.groups.get_mut(&group).unwrap();
                    g.tab_ids.retain(|t| *t != tab);
                    let at = index.min(g.tab_ids.len());
                    g.tab_ids.insert(at, tab.clone());
                } else {
                    self.take_tab_from_group(&src, &tab);
                    let g = self.groups.get_mut(&group).unwrap();
                    let at = index.min(g.tab_ids.len());
                    g.tab_ids.insert(at, tab.clone());
                    self.remove_group_if_empty(&src);
                }
                self.move_focus(prev, &group, &tab);
            }
            SplitGroupWith { tab, group, dir, before } => {
                self.tab(&tab)?;
                let target = self.group(&group)?;
                let src = self.group_of_tab(&tab).cloned().ok_or_else(|| no_tab(&tab))?;
                if src == group && target.tab_ids.len() == 1 {
                    return Err("탭이 하나뿐인 그룹은 분할할 수 없습니다".into());
                }
                let prev = self.focus_location();
                let gid = self.new_group_id();
                let mut n = self.counters.group;
                let name = loop {
                    let candidate = format!("group-{n}");
                    if !self.groups.values().any(|g| g.name == candidate) {
                        break candidate;
                    }
                    n += 1;
                };
                self.take_tab_from_group(&src, &tab);
                self.groups.insert(
                    gid.clone(),
                    Group { id: gid.clone(), name, tab_ids: vec![tab.clone()], active_tab_id: tab.clone() },
                );
                assert!(self.group_layout.split_leaf(&group, gid.clone(), dir, before), "group existence was checked above");
                self.remove_group_if_empty(&src);
                self.move_focus(prev, &gid, &tab);
            }
            MergeGroup { src, dst } => self.merge_groups(&src, &dst)?,
            CloseGroup { group } => {
                self.group(&group)?;
                let mut probe = self.group_layout.clone();
                let Some(Some(sibling)) = probe.remove_leaf(&group) else {
                    return Err("그룹이 하나뿐입니다".into());
                };
                self.merge_groups(&group, &sibling)?;
            }
            SetGroupMaximized { group } => {
                if let Some(g) = &group {
                    self.group(g)?;
                }
                self.group_maximized = group;
            }
        }
        Ok(fx)
    }

    // ───────── teams (phase 2, Task 8) ─────────

    /// Panes of team `team` in tab `tab`: the lead first, then members in hire order.
    pub fn team_panes(&self, tab: &str, team: &str) -> Vec<&Pane> {
        let mut v: Vec<&Pane> =
            self.panes.values().filter(|p| p.tab == tab && p.team.as_deref() == Some(team)).collect();
        v.sort_by_key(|p| (p.role != Some(Role::Lead), member_no(p), p.id.clone()));
        v
    }

    /// The tab's standing agent pane (Dispatcher / Handover / Manager), if any.
    pub fn standing_pane(&self, tab: &str) -> Option<PaneId> {
        let t = self.tabs.get(tab)?;
        t.layout.leaves().into_iter().find(|p| self.panes.get(p).is_some_and(|p| p.role.is_some_and(|r| r.is_standing())))
    }

    fn session_taken(&self, name: &str) -> bool {
        self.panes.values().any(|p| p.session_name.as_deref() == Some(name))
    }

    /// `AddTeamPane`: checks, creates the pane, places it (see [`TEAM_SLOT`]). Returns its id.
    fn add_team_pane(&mut self, tab: &str, team: &str, role: Role, prompt: Option<String>, loaded: &Loaded) -> Result<PaneId, String> {
        let t = self.tab(tab)?;
        if t.role != Some(Role::Manager) {
            return Err("팀은 Manager 탭에만 둘 수 있습니다".into());
        }
        if !valid_team_id(team) {
            return Err(format!("팀 id '{team}'이(가) 올바르지 않습니다 — 영문 소문자·숫자·'-'로 1~{TEAM_ID_MAX}자"));
        }
        let (tab_name, workspace) = (t.name.clone(), t.workspace.clone());
        let panes = self.team_panes(tab, team);
        let lead = panes.iter().find(|p| p.role == Some(Role::Lead)).map(|p| p.id.clone());
        let members = panes.iter().filter(|p| p.role == Some(Role::Member)).count();
        let last_no = panes.iter().map(|p| member_no(p)).max().unwrap_or(0);
        let (sname, number) = match role {
            Role::Lead => {
                if lead.is_some() {
                    return Err(format!("팀 '{team}'에는 이미 리드가 있습니다"));
                }
                (session_name(&tab_name, Role::Lead, Some(team), 0), None)
            }
            Role::Member => {
                if lead.is_none() {
                    return Err(format!("팀 '{team}'에 리드가 없습니다 — 먼저 팀 리드를 채용하세요"));
                }
                let max = loaded.config.max_per_team();
                if members >= max {
                    return Err(format!("팀 '{team}'의 팀원이 상한({max}명)에 찼습니다"));
                }
                // never reuse a number while the team exists
                let mut n = self.tabs[tab].team_seq.get(team).copied().unwrap_or(0).max(last_no) + 1;
                while self.session_taken(&session_name(&tab_name, Role::Member, Some(team), n)) {
                    n += 1;
                }
                (session_name(&tab_name, Role::Member, Some(team), n), Some(n))
            }
            _ => return Err("팀 창의 역할은 lead 또는 member만 가능합니다".into()),
        };
        if self.session_taken(&sname) {
            return Err(format!("세션 이름 '{sname}'이(가) 이미 쓰이고 있습니다"));
        }
        let agent = loaded.config.role_config(role).agent.unwrap_or_else(|| loaded.config.default_agent.clone());
        if !loaded.profiles.contains_key(&agent) {
            return Err(format!("에이전트 프로필 '{agent}'을(를) 찾을 수 없습니다"));
        }
        let id = self.new_pane_id();
        self.panes.insert(
            id.clone(),
            Pane {
                id: id.clone(),
                name: sname.clone(),
                tab: tab.to_string(),
                agent,
                args: Vec::new(), // roles.<role>.args are added at spawn time (launcher::plan_spawn)
                env: BTreeMap::new(),
                cwd: workspace,
                prompt,
                status: PaneStatus::Starting,
                attention: false,
                prompt_warning: false,
                attention_reason: None,
                kind: PaneKind::Terminal,
                role: Some(role),
                team: Some(team.to_string()),
                session_name: Some(sname),
                doc_path: None,
            },
        );
        match &lead {
            None => self.place_team_column(tab, &id),
            Some(lead) => self.place_member(tab, team, lead, &id)?,
        }
        if let Some(n) = number {
            self.tabs.get_mut(tab).unwrap().team_seq.insert(team.to_string(), n);
        }
        Ok(id)
    }

    /// A new team's column (its lead) goes into the tab's team area: appended to the split with
    /// `slot: "team"` (non-team children keep their sizes, team columns share the rest equally),
    /// or — without one — the standing pane is split to the right into a new team area
    /// `{split: horizontal, sizes: [55, 45], slot: "team", children: [standing, column]}`.
    fn place_team_column(&mut self, tab: &str, new: &PaneId) {
        let team_ids: BTreeSet<PaneId> =
            self.panes.values().filter(|p| p.tab == tab && p.team.is_some() && p.id != *new).map(|p| p.id.clone()).collect();
        let standing = self.standing_pane(tab);
        let t = self.tabs.get_mut(tab).unwrap();
        if let Some(LayoutNode::Split { sizes, children, .. }) = find_slot_mut(&mut t.layout, TEAM_SLOT) {
            let is_team: Vec<bool> = children.iter().map(|c| c.leaves().iter().any(|p| team_ids.contains(p))).collect();
            let k = is_team.iter().filter(|b| **b).count();
            children.push(LayoutNode::Leaf(new.clone()));
            if k == 0 || sizes.len() != is_team.len() {
                *sizes = vec![100.0 / children.len() as f32; children.len()];
            } else {
                let team_total: f32 = sizes.iter().zip(&is_team).filter(|(_, t)| **t).map(|(s, _)| *s).sum();
                let each = team_total / (k + 1) as f32;
                for (s, t) in sizes.iter_mut().zip(&is_team) {
                    if *t {
                        *s = each;
                    }
                }
                sizes.push(each);
            }
            return;
        }
        let target = match standing.as_ref().and_then(|s| find_leaf_mut(&mut t.layout, s)) {
            Some(leaf) => leaf,
            None => &mut t.layout,
        };
        let old = std::mem::replace(target, LayoutNode::Leaf(String::new()));
        *target = LayoutNode::Split {
            dir: Dir::Horizontal,
            sizes: TEAM_AREA_SIZES.to_vec(),
            children: vec![old, LayoutNode::Leaf(new.clone())],
            slot: Some(TEAM_SLOT.to_string()),
        };
    }

    /// A member goes to the bottom of its team's column (`slot: "team:<id>"`, vertical); a lead
    /// still alone gets its column split made here (lead on top).
    fn place_member(&mut self, tab: &str, team: &str, lead: &PaneId, new: &PaneId) -> Result<(), String> {
        let slot = team_column_slot(team);
        let t = self.tabs.get_mut(tab).unwrap();
        if let Some(LayoutNode::Split { dir: Dir::Vertical, sizes, children, .. }) = find_slot_mut(&mut t.layout, &slot) {
            children.push(LayoutNode::Leaf(new.clone()));
            *sizes = vec![100.0 / children.len() as f32; children.len()];
            return Ok(());
        }
        let target = find_leaf_mut(&mut t.layout, lead).ok_or_else(|| format!("팀 '{team}'의 리드 창이 화면 배치에 없습니다"))?;
        let old = std::mem::replace(target, LayoutNode::Leaf(String::new()));
        *target = LayoutNode::Split {
            dir: Dir::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![old, LayoutNode::Leaf(new.clone())],
            slot: Some(slot),
        };
        Ok(())
    }

    /// `RemoveTeam`: drops the team's column from the team area (its width goes to the other
    /// team columns; an emptied auto-made area collapses back into the standing pane) and
    /// removes the panes. Returns the panes to kill.
    fn remove_team(&mut self, tab: &str, team: &str) -> Result<Vec<PaneId>, String> {
        self.tab(tab)?;
        let ids: Vec<PaneId> = self.team_panes(tab, team).iter().map(|p| p.id.clone()).collect();
        if ids.is_empty() {
            return Err(format!("팀 '{team}'이(가) 이 탭에 없습니다"));
        }
        let gone: BTreeSet<PaneId> = ids.iter().cloned().collect();
        let others: BTreeSet<PaneId> = self
            .panes
            .values()
            .filter(|p| p.tab == tab && p.team.as_ref().is_some_and(|t| t != team))
            .map(|p| p.id.clone())
            .collect();
        let standing = self.standing_pane(tab);
        let t = self.tabs.get_mut(tab).unwrap();
        let mut column_removed = false;
        if let Some(LayoutNode::Split { sizes, children, .. }) = find_slot_mut(&mut t.layout, TEAM_SLOT) {
            // the column must hold exactly this team's panes (a user split inside it → per pane)
            let col = children.iter().position(|c| {
                let lv = c.leaves();
                lv.len() == gone.len() && lv.iter().all(|p| gone.contains(p))
            });
            if let Some(i) = col {
                let sized = sizes.len() == children.len();
                children.remove(i);
                if sized {
                    let freed = sizes.remove(i);
                    let is_team: Vec<bool> = children.iter().map(|c| c.leaves().iter().any(|p| others.contains(p))).collect();
                    let k = is_team.iter().filter(|b| **b).count();
                    if k > 0 {
                        for (s, t) in sizes.iter_mut().zip(&is_team) {
                            if *t {
                                *s += freed / k as f32;
                            }
                        }
                    }
                }
                column_removed = true;
            }
        }
        if column_removed {
            t.layout.normalize();
        } else {
            for id in &ids {
                if let Some(None) = t.layout.remove_leaf(id) {
                    return Err("팀을 해산하면 탭에 창이 남지 않습니다".into());
                }
            }
        }
        if gone.contains(&t.last_focused_pane_id) {
            t.last_focused_pane_id = standing.unwrap_or_else(|| t.layout.leaves()[0].clone());
        }
        if t.maximized_pane_id.as_ref().is_some_and(|m| gone.contains(m)) {
            t.maximized_pane_id = None;
        }
        t.team_seq.remove(team);
        for id in &ids {
            self.panes.remove(id);
        }
        Ok(ids)
    }

    // ───────── documents (Task 14) ─────────

    /// `OpenDoc`: validate, then focus the pane already showing that document, or make a new
    /// document tab. Either way focus moves (user action).
    /// `OpenDashboard` (see there).
    fn open_dashboard(&mut self, group: Option<GroupId>, loaded: &Loaded) -> Result<Effects, String> {
        if let Some(g) = &group {
            self.group(g)?;
        }
        let existing = self.panes.values().find(|p| p.kind == PaneKind::Applet { kind: DASHBOARD_APPLET.into() });
        if let Some(p) = existing {
            let (pid, tid) = (p.id.clone(), p.tab.clone());
            let gid = self.group_of_tab(&tid).cloned().ok_or_else(|| no_tab(&tid))?;
            if self.tabs[&tid].maximized_pane_id.as_ref().is_some_and(|m| *m != pid) {
                self.tabs.get_mut(&tid).unwrap().maximized_pane_id = None;
            }
            self.apply_inner(WsCommand::SetFocus { group: gid, tab: Some(tid), pane: Some(pid) }, loaded)?;
            return Ok(Effects::default());
        }
        let root = loaded.settings.workspace_root.clone();
        let gid = group.unwrap_or_else(|| self.focus.group_id.clone());
        let name = self.unique_tab_name(DASHBOARD_TAB);
        self.check_tab_name(&name, None)?;
        let tid = self.new_tab_id();
        let pid = self.new_pane_id();
        self.panes.insert(
            pid.clone(),
            Pane {
                id: pid.clone(),
                name: DASHBOARD_TAB.into(),
                tab: tid.clone(),
                agent: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: root.clone(),
                prompt: None,
                status: PaneStatus::Starting,
                attention: false,
                prompt_warning: false,
                attention_reason: None,
                kind: PaneKind::Applet { kind: DASHBOARD_APPLET.into() },
                role: None,
                team: None,
                session_name: None,
                doc_path: None,
            },
        );
        self.tabs.insert(
            tid.clone(),
            Tab {
                id: tid.clone(),
                name,
                dir: String::new(),
                template: String::new(),
                is_dispatcher: false,
                workspace: root,
                layout: LayoutNode::Leaf(pid.clone()),
                last_focused_pane_id: pid.clone(),
                maximized_pane_id: None,
                sync_input: false,
                role: None,
                team_seq: BTreeMap::new(),
            },
        );
        self.groups.get_mut(&gid).unwrap().tab_ids.push(tid.clone());
        let prev = self.focus_location();
        self.move_focus(prev, &gid, &tid);
        // the launcher marks the applet Running (no process)
        Ok(Effects { spawn: vec![(pid, false)], kill: Vec::new() })
    }

    fn open_doc(&mut self, path: &str, group: Option<GroupId>, loaded: &Loaded) -> Result<Effects, String> {
        if let Some(g) = &group {
            self.group(g)?;
        }
        let root = &loaded.settings.workspace_root;
        let doc = docs::resolve_doc(root, path)?;
        // an open tab with the same document: compare validated forms (spelling-independent);
        // a stored path that no longer validates (file gone) simply does not match
        let existing = self.panes.values().find(|p| {
            p.kind.is_doc()
                && p.doc_path.as_ref().is_some_and(|d| docs::resolve_doc(root, &d.to_string_lossy()).is_ok_and(|v| v.rel == doc.rel))
        });
        if let Some(p) = existing {
            let (pid, tid) = (p.id.clone(), p.tab.clone());
            let gid = self.group_of_tab(&tid).cloned().ok_or_else(|| no_tab(&tid))?;
            if self.tabs[&tid].maximized_pane_id.as_ref().is_some_and(|m| *m != pid) {
                self.tabs.get_mut(&tid).unwrap().maximized_pane_id = None;
            }
            self.apply_inner(WsCommand::SetFocus { group: gid, tab: Some(tid), pane: Some(pid) }, loaded)?;
            return Ok(Effects::default());
        }
        let gid = group.unwrap_or_else(|| self.focus.group_id.clone());
        let name = self.unique_tab_name(&doc.name());
        self.check_tab_name(&name, None)?;
        let tid = self.new_tab_id();
        let pid = self.new_pane_id();
        self.panes.insert(
            pid.clone(),
            Pane {
                id: pid.clone(),
                name: doc.name(),
                tab: tid.clone(),
                agent: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: root.clone(),
                prompt: None,
                status: PaneStatus::Starting,
                attention: false,
                prompt_warning: false,
                attention_reason: None,
                kind: PaneKind::Applet { kind: DOC_APPLET.into() },
                role: None,
                team: None,
                session_name: None,
                doc_path: Some(doc.abs.clone()),
            },
        );
        self.tabs.insert(
            tid.clone(),
            Tab {
                id: tid.clone(),
                name,
                dir: String::new(),
                template: String::new(),
                is_dispatcher: false,
                workspace: root.clone(),
                layout: LayoutNode::Leaf(pid.clone()),
                last_focused_pane_id: pid.clone(),
                maximized_pane_id: None,
                sync_input: false,
                role: None,
                team_seq: BTreeMap::new(),
            },
        );
        self.groups.get_mut(&gid).unwrap().tab_ids.push(tid.clone());
        let prev = self.focus_location();
        self.move_focus(prev, &gid, &tid);
        // the launcher marks the applet Running (no process)
        Ok(Effects { spawn: vec![(pid, false)], kill: Vec::new() })
    }

    // ───────── helpers ─────────

    fn pane(&self, id: &str) -> Result<&Pane, String> {
        self.panes.get(id).ok_or_else(|| no_pane(id))
    }
    fn tab(&self, id: &str) -> Result<&Tab, String> {
        self.tabs.get(id).ok_or_else(|| no_tab(id))
    }
    fn group(&self, id: &str) -> Result<&Group, String> {
        self.groups.get(id).ok_or_else(|| no_group(id))
    }

    /// R16: a standing agent pane (Dispatcher/Handover/Manager) is never closed or broken off on
    /// its own — the standing task goes away with its tab (CloseTab). Restart is allowed.
    fn check_not_standing(&self, pane: &str) -> Result<(), String> {
        if self.pane(pane)?.role.is_some_and(|r| r.is_standing()) {
            return Err("상시 에이전트 창은 따로 닫거나 떼어낼 수 없습니다 — 탭을 닫으세요".into());
        }
        Ok(())
    }

    fn new_group_id(&mut self) -> GroupId {
        self.counters.group += 1;
        format!("g{}", self.counters.group)
    }
    fn new_tab_id(&mut self) -> TabId {
        self.counters.tab += 1;
        format!("t{}", self.counters.tab)
    }
    fn new_pane_id(&mut self) -> PaneId {
        self.counters.pane += 1;
        format!("p{}", self.counters.pane)
    }

    /// Tab names are unique across the workspace; `dispatcher` and `agentboard` are reserved.
    fn check_tab_name(&self, name: &str, except: Option<&str>) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("탭 이름이 비어 있습니다".into());
        }
        if let Some(why) = crate::roles::reserved_name(name) {
            return Err(why);
        }
        // the name becomes a session name (--name) and is typed into prompts
        if name.chars().any(char::is_control) {
            return Err("탭 이름에 제어 문자(줄바꿈·탭 등)를 쓸 수 없습니다".into());
        }
        if self.tabs.values().any(|t| t.name == name && Some(t.id.as_str()) != except) {
            return Err(format!("탭 이름 '{name}'이(가) 이미 있습니다"));
        }
        Ok(())
    }

    /// `base`, or `base-2`, `base-3`, … whichever is free.
    fn unique_tab_name(&self, base: &str) -> String {
        let taken = |n: &str| crate::roles::reserved_name(n).is_some() || self.tabs.values().any(|t| t.name == n);
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base}-{n}")).find(|n| !taken(n)).unwrap()
    }

    /// Create a tab (not yet placed in a group) and its panes from a template.
    ///
    /// `role` is the tab's role (`Dispatcher` for the Dispatcher tab, `Manager` / `Handover`
    /// from config). Exactly one pane of a role tab is the standing agent and carries the role
    /// and `session_name` (= tab name): the template's `focus` pane when it is a terminal, else
    /// the first terminal pane. Other terminal panes of the tab (helpers such as a shell) and
    /// applets get no role. A handover tab's workspace is workspaceRoot (its `dir` is "").
    fn build_tab(
        &mut self,
        name: &str,
        dir: &str,
        tpl: &Template,
        is_dispatcher: bool,
        role: Option<Role>,
        loaded: &Loaded,
    ) -> TabId {
        let cfg = &loaded.config;
        let root = &loaded.settings.workspace_root;
        let at_root = is_dispatcher || role == Some(Role::Handover) || dir.is_empty();
        let workspace = if at_root { root.clone() } else { root.join(dir) };
        let default_agent = if is_dispatcher {
            cfg.dispatcher.agent.clone().unwrap_or_else(|| cfg.default_agent.clone())
        } else {
            cfg.default_agent.clone()
        };
        let id = self.new_tab_id();
        let specs: Vec<PaneSpec> = tpl.layout.leaves();
        let standing = role.and_then(|_| {
            let focus = specs.iter().position(|s| s.focus && s.applet.is_none());
            focus.or_else(|| specs.iter().position(|s| s.applet.is_none()))
        });
        let ids: Vec<PaneId> = specs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let pid = self.new_pane_id();
                let (kind, agent) = match &s.applet {
                    Some(k) => (PaneKind::Applet { kind: k.clone() }, String::new()),
                    None => (PaneKind::Terminal, s.agent.clone().unwrap_or_else(|| default_agent.clone())),
                };
                let pane_role = if standing == Some(i) { role } else { None };
                self.panes.insert(
                    pid.clone(),
                    Pane {
                        id: pid.clone(),
                        name: s.name.clone(),
                        tab: id.clone(),
                        agent,
                        args: s.args.clone(),
                        env: s.env.clone(),
                        cwd: resolve_cwd(&workspace, s.cwd.as_deref()),
                        prompt: s.prompt.clone(),
                        status: PaneStatus::Starting,
                        attention: false,
                        prompt_warning: false,
                        attention_reason: None,
                        kind,
                        role: pane_role,
                        team: None,
                        session_name: pane_role.map(|r| session_name(name, r, None, 0)),
                        doc_path: None,
                    },
                );
                pid
            })
            .collect();
        let mut it = ids.iter();
        let layout = tpl.layout.map(|_| it.next().unwrap().clone());
        let last = specs.iter().position(|s| s.focus).map(|i| ids[i].clone()).unwrap_or_else(|| ids[0].clone());
        self.tabs.insert(
            id.clone(),
            Tab {
                id: id.clone(),
                name: name.to_string(),
                dir: if is_dispatcher { String::new() } else { dir.to_string() },
                template: tpl.name.clone(),
                is_dispatcher,
                workspace,
                layout,
                last_focused_pane_id: last,
                maximized_pane_id: None,
                sync_input: false,
                role,
                team_seq: BTreeMap::new(),
            },
        );
        id
    }

    /// Remove a tab from its group and the tab map (its panes are handled by the caller).
    /// The group's active tab moves to the right neighbour, else the left one; an emptied group
    /// is removed.
    fn detach_tab(&mut self, tab: &str) {
        self.tabs.remove(tab);
        if self.last_location.as_ref().is_some_and(|l| l.tab_id == tab) {
            self.last_location = None;
        }
        let Some(gid) = self.group_of_tab(tab).cloned() else { return };
        self.take_tab_from_group(&gid, tab);
        self.remove_group_if_empty(&gid);
    }

    /// Take `tab` out of group `gid`'s tab list (the tab stays in the tab map). If it was the
    /// group's active tab, the right neighbour becomes active, else the left one. The group may
    /// be left empty; callers decide whether to remove it.
    fn take_tab_from_group(&mut self, gid: &GroupId, tab: &str) {
        let g = self.groups.get_mut(gid).unwrap();
        let idx = g.tab_ids.iter().position(|t| t == tab).unwrap();
        g.tab_ids.remove(idx);
        if g.active_tab_id == tab {
            g.active_tab_id = g.tab_ids.get(idx).or_else(|| idx.checked_sub(1).and_then(|i| g.tab_ids.get(i))).cloned().unwrap_or_default();
        }
    }

    /// The focused group's active tab as a location.
    fn focus_location(&self) -> Option<Location> {
        self.groups.get(&self.focus.group_id).map(|g| Location { group_id: g.id.clone(), tab_id: g.active_tab_id.clone() })
    }

    /// `last_location` must name an existing group that contains the tab, and must differ from the
    /// current focus (a "go back" that does nothing is stale). Called after every command.
    fn sanitize_last_location(&mut self) {
        let valid = self.last_location.as_ref().is_some_and(|l| self.groups.get(&l.group_id).is_some_and(|g| g.tab_ids.contains(&l.tab_id)));
        if !valid || self.last_location == self.focus_location() {
            self.last_location = None;
        }
    }

    /// Focus `(gid, tid)`: that tab becomes the group's active tab, `prev` (the location before the
    /// command) becomes `last_location` when it differs and still exists, a `last_location` equal to
    /// the new focus is stale and cleared, the tab's last focused pane loses its attention mark, and a
    /// group / pane maximize that would hide that pane is cleared (R22).
    fn move_focus(&mut self, prev: Option<Location>, gid: &GroupId, tid: &TabId) {
        let new = Location { group_id: gid.clone(), tab_id: tid.clone() };
        if let Some(prev) = prev {
            let alive = self.groups.get(&prev.group_id).is_some_and(|g| g.tab_ids.contains(&prev.tab_id));
            if alive && prev != new {
                self.last_location = Some(prev);
            }
        }
        if self.last_location.as_ref() == Some(&new) {
            self.last_location = None;
        }
        self.focus.group_id = gid.clone();
        self.groups.get_mut(gid).unwrap().active_tab_id = tid.clone();
        let focused = self.tabs[tid].last_focused_pane_id.clone();
        if let Some(p) = self.panes.get_mut(&focused) {
            p.clear_attention();
        }
        // R22: never leave focus on a pane hidden by a maximize
        if self.group_maximized.as_ref().is_some_and(|g| g != gid) {
            self.group_maximized = None;
        }
        let t = self.tabs.get_mut(tid).unwrap();
        if t.maximized_pane_id.as_ref().is_some_and(|m| *m != focused) {
            t.maximized_pane_id = None;
        }
    }

    /// Make `tid` the active tab of `gid`. In the focused group this is a focus move (records
    /// `last_location`); in another group focus is left alone (R12).
    fn activate_tab(&mut self, gid: &GroupId, tid: &TabId) {
        if *gid == self.focus.group_id {
            let prev = self.focus_location();
            self.move_focus(prev, gid, tid);
        } else {
            self.groups.get_mut(gid).unwrap().active_tab_id = tid.clone();
        }
    }

    /// Append `src`'s tabs to `dst` (name and active tab of `dst` kept) and remove `src`. Focus
    /// stays on the originally focused tab, now in whichever group holds it.
    fn merge_groups(&mut self, src: &GroupId, dst: &GroupId) -> Result<(), String> {
        self.group(src)?;
        self.group(dst)?;
        if src == dst {
            return Err("같은 그룹끼리는 병합할 수 없습니다".into());
        }
        let prev = self.focus_location();
        let moved = std::mem::take(&mut self.groups.get_mut(src).unwrap().tab_ids);
        self.groups.get_mut(dst).unwrap().tab_ids.extend(moved);
        self.remove_group_if_empty(src);
        if let Some(p) = &prev {
            let gid = self.group_of_tab(&p.tab_id).cloned().expect("focused tab still lives in some group");
            self.move_focus(prev.clone(), &gid, &p.tab_id);
        }
        Ok(())
    }

    /// Remove `g` from the group layout and the group map when it has no tabs. A focused group
    /// hands focus to the layout's focus candidate (adjacent sibling). The last remaining group
    /// is never removed (cannot happen in practice: the Dispatcher tab always lives somewhere).
    fn remove_group_if_empty(&mut self, g: &GroupId) {
        let Some(grp) = self.groups.get(g) else { return };
        if !grp.tab_ids.is_empty() || self.group_layout == LayoutNode::Leaf(g.clone()) {
            return;
        }
        if let Some(Some(cand)) = self.group_layout.remove_leaf(g) {
            if self.focus.group_id == *g {
                self.focus.group_id = cand;
            }
        }
        self.groups.remove(g);
        if self.group_maximized.as_ref() == Some(g) {
            self.group_maximized = None;
        }
        if self.last_location.as_ref().is_some_and(|l| l.group_id == *g) {
            self.last_location = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ensure_dotdir, load_all};

    fn loaded_with(config: &str) -> Loaded {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), config).unwrap();
        let l = load_all(appdata.path());
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        l
    }

    /// `solo`/`team-3` templates, `claude`/`powershell` profiles, `config.tabs=[api(team-3)]`.
    fn loaded_fixture() -> Loaded {
        let l = loaded_with(r#"{"tabs":[{"name":"api","dir":"api","template":"team-3"}]}"#);
        for t in ["solo", "team-3"] {
            assert!(l.templates.contains_key(t));
        }
        for p in ["claude", "powershell"] {
            assert!(l.profiles.contains_key(p));
        }
        l
    }

    /// Two groups side by side: control=[dispatcher] | work=[api].
    fn loaded_groups() -> Loaded {
        loaded_with(
            r#"{"tabs":[{"name":"api","dir":"api","template":"team-3"}],
                "groups":{"split":"horizontal","children":[
                    {"group":{"name":"control","tabs":["dispatcher"]}},
                    {"group":{"name":"work","tabs":["api"]}}]}}"#,
        )
    }

    fn root(l: &Loaded) -> PathBuf {
        l.settings.workspace_root.clone()
    }
    fn tab<'a>(ws: &'a Workspace, name: &str) -> &'a Tab {
        ws.tabs.values().find(|t| t.name == name).unwrap_or_else(|| panic!("no tab {name}"))
    }
    fn has_tab(ws: &Workspace, name: &str) -> bool {
        ws.tabs.values().any(|t| t.name == name)
    }
    fn pane(ws: &Workspace, tab_name: &str, pane_name: &str) -> PaneId {
        let t = tab(ws, tab_name);
        t.layout.leaves().into_iter().find(|p| ws.panes[p].name == pane_name).unwrap()
    }
    fn group_of(ws: &Workspace, tab_id: &str) -> GroupId {
        ws.groups.values().find(|g| g.tab_ids.iter().any(|t| t == tab_id)).unwrap().id.clone()
    }
    fn pane_names(ws: &Workspace, tab_name: &str) -> Vec<String> {
        tab(ws, tab_name).layout.leaves().iter().map(|p| ws.panes[p].name.clone()).collect()
    }
    fn sizes(n: &LayoutNode<String>) -> Vec<f32> {
        match n {
            LayoutNode::Split { sizes, .. } => sizes.clone(),
            _ => panic!("not a split"),
        }
    }
    fn near(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }
    fn create(name: &str, template: &str) -> WsCommand {
        WsCommand::CreateTab { name: name.into(), dir: name.into(), template: Some(template.into()), group: None, role: None }
    }

    #[test]
    fn team_ids_reserve_agentboard() {
        assert!(valid_team_id("batch") && valid_team_id("a-1"));
        for bad in ["", "agentboard", "Batch", "a_b", &"x".repeat(TEAM_ID_MAX + 1)] {
            assert!(!valid_team_id(bad), "{bad}");
        }
    }

    #[test]
    fn create_tab_refuses_reserved_names() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let before = ws.clone();
        for bad in ["agentboard", "AGENTBOARD", "dispatcher"] {
            assert!(ws.apply(create(bad, "solo"), &l).is_err(), "{bad}");
            assert_eq!(ws, before);
        }
    }

    #[test]
    fn from_config_creates_dispatcher_and_tabs() {
        let l = loaded_fixture();
        let ws = Workspace::from_config(&l);
        assert_eq!(ws.groups.len(), 1);
        let g = ws.groups.values().next().unwrap();
        assert_eq!(g.name, "main");
        assert_eq!(ws.group_layout, LayoutNode::Leaf(g.id.clone()));
        let d = tab(&ws, "dispatcher");
        let api = tab(&ws, "api");
        assert_eq!(g.tab_ids, vec![d.id.clone(), api.id.clone()]);
        assert!(d.is_dispatcher && !api.is_dispatcher);
        assert_eq!(d.workspace, root(&l));
        assert_eq!(d.template, "dispatcher");
        assert_eq!(ws.panes[&d.last_focused_pane_id].agent, "claude");
        assert_eq!(api.workspace, root(&l).join("api"));
        assert_eq!(api.template, "team-3");
        assert_eq!(pane_names(&ws, "api"), ["lead", "coder", "tests"]);
        assert_eq!(api.last_focused_pane_id, pane(&ws, "api", "lead"));
        let tests = &ws.panes[&pane(&ws, "api", "tests")];
        assert_eq!(tests.agent, "powershell");
        assert_eq!(tests.cwd, root(&l).join("api"));
        assert_eq!(tests.tab, api.id);
        assert_eq!(tests.status, PaneStatus::Starting);
        // focus: first group in reading order, its first tab
        assert_eq!(ws.focus.group_id, g.id);
        assert_eq!(g.active_tab_id, d.id);
        assert_eq!(ws.last_location, None);
        assert_eq!(ws.counters, Counters { group: 1, tab: 2, pane: 4 });
        assert_eq!(ws.running_count(), 4);
    }

    #[test]
    fn from_config_applies_group_layout() {
        let l = loaded_groups();
        let ws = Workspace::from_config(&l);
        let names: Vec<_> = ws.group_layout.leaves().iter().map(|g| ws.groups[g].name.clone()).collect();
        assert_eq!(names, ["control", "work"]);
        let d = tab(&ws, "dispatcher");
        let api = tab(&ws, "api");
        assert_eq!(ws.groups[&group_of(&ws, &d.id)].name, "control");
        assert_eq!(ws.groups[&group_of(&ws, &api.id)].name, "work");
        assert_eq!(ws.groups[&ws.focus.group_id].name, "control");
        // unlisted tabs go to defaultGroup
        let l = loaded_with(
            r#"{"tabs":[{"name":"a","dir":"a"},{"name":"b","dir":"b"}],"defaultGroup":"w",
                "groups":{"split":"vertical","children":[
                    {"group":{"name":"c","tabs":["a"]}},{"group":{"name":"w","tabs":[]}}]}}"#,
        );
        let ws = Workspace::from_config(&l);
        let w = ws.groups.values().find(|g| g.name == "w").unwrap();
        let names: Vec<_> = w.tab_ids.iter().map(|t| ws.tabs[t].name.clone()).collect();
        assert_eq!(names, ["dispatcher", "b"]);
        assert_eq!(tab(&ws, "a").template, "solo");
    }

    #[test]
    fn from_config_skips_tab_with_missing_template() {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(
            root.path().join(".agentboard/config.json"),
            r#"{"tabs":[{"name":"x","dir":"x","template":"nope"},{"name":"y","dir":"y"}]}"#,
        )
        .unwrap();
        let l = load_all(appdata.path());
        assert!(!l.errors.is_empty());
        let ws = Workspace::from_config(&l);
        assert!(!has_tab(&ws, "x"));
        assert!(has_tab(&ws, "y"));
    }

    #[test]
    fn create_tab_spawns_with_prompt_and_activates() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let fx = ws.apply(create("web", "team-3"), &l).unwrap();
        let web = tab(&ws, "web").clone();
        assert_eq!(fx.spawn.len(), 3);
        assert!(fx.spawn.iter().all(|(_, inject)| *inject));
        assert_eq!(fx.spawn.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), web.layout.leaves());
        assert!(fx.kill.is_empty());
        let g = &ws.groups[&ws.focus.group_id];
        assert_eq!(g.active_tab_id, web.id);
        assert_eq!(g.tab_ids.last(), Some(&web.id));
        assert_eq!(web.workspace, root(&l).join("web"));
        assert_eq!(web.last_focused_pane_id, pane(&ws, "web", "lead"));
        // default template when none given
        ws.apply(WsCommand::CreateTab { name: "s".into(), dir: "".into(), template: None, group: None, role: None }, &l).unwrap();
        assert_eq!(tab(&ws, "s").template, "solo");
        assert_eq!(tab(&ws, "s").workspace, root(&l));
    }

    #[test]
    fn create_tab_duplicate_name_errors_without_change() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let before = ws.clone();
        for cmd in [
            create("api", "solo"),
            create("dispatcher", "solo"),
            create("x", "missing-template"),
            WsCommand::CreateTab { name: "y".into(), dir: "../out".into(), template: None, group: None, role: None },
            WsCommand::CreateTab { name: "z".into(), dir: "z".into(), template: None, group: Some("g99".into()), role: None },
        ] {
            assert!(ws.apply(cmd, &l).is_err());
            assert_eq!(ws, before);
        }
    }

    #[test]
    fn split_pane_adds_and_focuses() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let tests = pane(&ws, "api", "tests");
        let fx = ws.apply(WsCommand::SplitPane { pane: tests.clone(), dir: Dir::Vertical }, &l).unwrap();
        assert_eq!(fx.spawn.len(), 1);
        assert!(fx.kill.is_empty());
        let new = fx.spawn[0].0.clone();
        assert_eq!(pane_names(&ws, "api"), ["lead", "coder", "tests", "pane-1"]);
        let p = &ws.panes[&new];
        assert_eq!(p.agent, "powershell");
        assert_eq!(p.cwd, root(&l).join("api"));
        assert_eq!(p.prompt, None);
        assert_eq!(p.tab, tab(&ws, "api").id);
        assert_eq!(tab(&ws, "api").last_focused_pane_id, new);
        let fx = ws.apply(WsCommand::SplitPane { pane: pane(&ws, "api", "lead"), dir: Dir::Horizontal }, &l).unwrap();
        assert_eq!(ws.panes[&fx.spawn[0].0].name, "pane-2");
        assert_eq!(ws.panes[&fx.spawn[0].0].agent, "claude");
        let before = ws.clone();
        assert!(ws.apply(WsCommand::SplitPane { pane: "p999".into(), dir: Dir::Vertical }, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn close_pane_focuses_sibling_and_kills() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let lead = pane(&ws, "api", "lead");
        let fx = ws.apply(WsCommand::ClosePane { pane: lead.clone() }, &l).unwrap();
        assert_eq!(fx.kill, vec![lead.clone()]);
        assert!(fx.spawn.is_empty());
        assert!(ws.pane_by_id(&lead).is_none());
        assert_eq!(pane_names(&ws, "api"), ["coder", "tests"]);
        assert_eq!(tab(&ws, "api").last_focused_pane_id, pane(&ws, "api", "coder"));
        // closing a non-focused pane keeps the focus where it was
        let tests = pane(&ws, "api", "tests");
        ws.apply(WsCommand::ClosePane { pane: tests }, &l).unwrap();
        assert_eq!(tab(&ws, "api").last_focused_pane_id, pane(&ws, "api", "coder"));
    }

    #[test]
    fn closing_last_pane_closes_tab() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        ws.apply(create("s", "solo"), &l).unwrap();
        let sid = tab(&ws, "s").id.clone();
        let p = pane(&ws, "s", "main");
        let fx = ws.apply(WsCommand::ClosePane { pane: p.clone() }, &l).unwrap();
        assert_eq!(fx.kill, vec![p.clone()]);
        assert!(!has_tab(&ws, "s"));
        assert!(ws.panes.get(&p).is_none());
        let g = &ws.groups[&ws.focus.group_id];
        assert!(!g.tab_ids.contains(&sid));
        // s was rightmost and active → left neighbour (api) becomes active
        assert_eq!(g.active_tab_id, tab(&ws, "api").id);
    }

    #[test]
    fn close_tab_activates_right_then_left_and_removes_empty_group() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        ws.apply(create("b", "solo"), &l).unwrap();
        let api = tab(&ws, "api").id.clone();
        let gid = ws.focus.group_id.clone();
        ws.apply(WsCommand::SetFocus { group: gid.clone(), tab: Some(api.clone()), pane: None }, &l).unwrap();
        let killed: Vec<_> = tab(&ws, "api").layout.leaves();
        let fx = ws.apply(WsCommand::CloseTab { tab: api }, &l).unwrap();
        assert_eq!(fx.kill, killed);
        assert!(killed.iter().all(|p| ws.pane_by_id(p).is_none()));
        assert_eq!(ws.groups[&gid].active_tab_id, tab(&ws, "b").id); // right neighbour

        // a group left without tabs disappears and focus moves to the sibling group
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let work = group_of(&ws, &tab(&ws, "api").id);
        let control = group_of(&ws, &tab(&ws, "dispatcher").id);
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: None, pane: None }, &l).unwrap();
        ws.apply(WsCommand::CloseTab { tab: tab(&ws, "api").id.clone() }, &l).unwrap();
        assert!(!ws.groups.contains_key(&work));
        assert_eq!(ws.group_layout, LayoutNode::Leaf(control.clone()));
        assert_eq!(ws.focus.group_id, control);
        // (control, dispatcher) is now the focus itself, so the "go back" target is stale
        assert_eq!(ws.last_location, None);

        // lastLocation pointing at a closed tab/group is dropped
        let mut ws = Workspace::from_config(&l);
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: None, pane: None }, &l).unwrap();
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: None, pane: None }, &l).unwrap();
        assert_eq!(ws.last_location.as_ref().map(|l| l.group_id.clone()), Some(work.clone()));
        ws.apply(WsCommand::CloseTab { tab: tab(&ws, "api").id.clone() }, &l).unwrap();
        assert_eq!(ws.last_location, None);
        assert_eq!(ws.focus.group_id, control);
    }

    #[test]
    fn dispatcher_cannot_be_closed() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let before = ws.clone();
        let d = tab(&ws, "dispatcher").clone();
        assert!(ws.apply(WsCommand::CloseTab { tab: d.id.clone() }, &l).is_err());
        assert_eq!(ws, before);
        assert!(ws.apply(WsCommand::ClosePane { pane: d.last_focused_pane_id.clone() }, &l).is_err());
        assert_eq!(ws, before);
        assert!(ws.apply(WsCommand::BreakPaneToTab { pane: d.last_focused_pane_id.clone() }, &l).is_err());
        assert_eq!(ws, before);
        assert!(ws.apply(WsCommand::RenameTab { tab: d.id.clone(), name: "x".into() }, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn rename_tab_checks_names() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let api = tab(&ws, "api").id.clone();
        let before = ws.clone();
        for bad in ["dispatcher", "  ", "agentboard", "AgentBoard"] {
            assert!(ws.apply(WsCommand::RenameTab { tab: api.clone(), name: bad.into() }, &l).is_err());
            assert_eq!(ws, before);
        }
        ws.apply(WsCommand::RenameTab { tab: api.clone(), name: "api2".into() }, &l).unwrap();
        assert_eq!(ws.tabs[&api].name, "api2");
        ws.apply(create("other", "solo"), &l).unwrap();
        assert!(ws.apply(WsCommand::RenameTab { tab: api.clone(), name: "other".into() }, &l).is_err());
    }

    #[test]
    fn restart_respawns_without_prompt() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let p = pane(&ws, "api", "coder");
        ws.set_status(&p, PaneStatus::Exited { code: 3 });
        assert_eq!(ws.pane_by_id(&p).unwrap().status, PaneStatus::Exited { code: 3 });
        let fx = ws.apply(WsCommand::RestartPane { pane: p.clone() }, &l).unwrap();
        assert_eq!(fx, Effects { spawn: vec![(p.clone(), false)], kill: vec![p.clone()] });
        assert_eq!(ws.pane_by_id(&p).unwrap().status, PaneStatus::Starting);
    }

    #[test]
    fn restart_fresh_respawns_with_start_prompt() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let p = pane(&ws, "api", "coder");
        {
            let q = ws.panes.get_mut(&p).unwrap();
            q.status = PaneStatus::Running;
            q.attention = true;
            q.attention_reason = Some("x".into());
            q.prompt_warning = true;
        }
        let fx = ws.apply(WsCommand::RestartPaneFresh { pane: p.clone() }, &l).unwrap();
        assert_eq!(fx, Effects { spawn: vec![(p.clone(), true)], kill: vec![p.clone()] }, "prompt injected again");
        let q = ws.pane_by_id(&p).unwrap();
        assert_eq!(q.status, PaneStatus::Starting);
        assert!(!q.attention && q.attention_reason.is_none() && !q.prompt_warning);
        let before = ws.clone();
        assert!(ws.apply(WsCommand::RestartPaneFresh { pane: "nope".into() }, &l).is_err());
        assert_eq!(ws, before);
        // wire shape (the frontend never sends it, but it is a normal command)
        let cmd: WsCommand = serde_json::from_str(r#"{"type":"restartPaneFresh","pane":"p1"}"#).unwrap();
        assert_eq!(cmd, WsCommand::RestartPaneFresh { pane: "p1".into() });
    }

    #[test]
    fn running_count_counts_live_panes() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let ids: Vec<_> = ws.panes.keys().cloned().collect();
        ws.set_status(&ids[0], PaneStatus::Exited { code: 0 });
        ws.set_status(&ids[1], PaneStatus::Failed { message: "x".into() });
        ws.set_status(&ids[2], PaneStatus::Running);
        ws.set_status("nope", PaneStatus::Running);
        assert_eq!(ws.running_count(), 2);
    }

    // ── phase 2: roles, handover, applet panes ──

    const INBOX_TPL: &str = r#"{"name":"dispatcher-inbox","layout":{"split":"horizontal","sizes":[60,40],"children":[
        {"pane":{"name":"dispatcher","focus":true}},{"applet":{"name":"inbox","kind":"inbox"}}]}}"#;
    const MANAGER_TPL: &str = r#"{"name":"manager","layout":{"split":"horizontal","children":[
        {"pane":{"name":"shell","agent":"powershell"}},{"pane":{"name":"manager","focus":true}},{"pane":{"name":"extra"}}]}}"#;

    /// Load with extra template files; returns errors instead of asserting none.
    fn loaded_files(config: &str, templates: &[(&str, &str)]) -> Loaded {
        let root = tempfile::tempdir().unwrap().keep();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(&root).unwrap();
        for (n, c) in templates {
            std::fs::write(root.join(".agentboard/templates").join(n), c).unwrap();
        }
        std::fs::write(root.join(".agentboard/config.json"), config).unwrap();
        load_all(appdata.path())
    }

    #[test]
    fn handover_tab_workspace_is_root_and_unique() {
        let l = loaded_files(
            r#"{"tabs":[{"name":"handover","role":"handover","dir":""},
                        {"name":"h-sub","role":"handover","dir":"sub"},
                        {"name":"h2","role":"handover"},
                        {"name":"billing","role":"manager","dir":"billing"}]}"#,
            &[],
        );
        let cfg_errs: Vec<_> = l.errors.iter().filter_map(|e| e.json_path.clone()).collect();
        assert_eq!(cfg_errs, ["tabs[1].dir", "tabs[2].role"], "{:?}", l.errors);
        let ws = Workspace::from_config(&l);
        let h = tab(&ws, "handover");
        assert_eq!(h.role, Some(Role::Handover));
        assert_eq!(h.workspace, root(&l), "handover works in workspaceRoot");
        assert_eq!(h.dir, "");
        assert!(!has_tab(&ws, "h-sub") && !has_tab(&ws, "h2"));
        assert_eq!(ws.tabs.values().filter(|t| t.role == Some(Role::Handover)).count(), 1);
        let b = tab(&ws, "billing");
        assert_eq!(b.role, Some(Role::Manager));
        assert_eq!(b.workspace, root(&l).join("billing"));
        let p = &ws.panes[&b.layout.leaves()[0]];
        assert_eq!(p.cwd, root(&l).join("billing"));
    }

    #[test]
    fn standing_panes_get_roles_and_session_names() {
        let l = loaded_files(
            r#"{"dispatcher":{"template":"dispatcher-inbox"},
                "tabs":[{"name":"billing","role":"manager","dir":"billing","template":"manager"},
                        {"name":"plain","dir":"plain","template":"team-3"}]}"#,
            &[("dispatcher-inbox.json", INBOX_TPL), ("manager.json", MANAGER_TPL)],
        );
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let ws = Workspace::from_config(&l);

        let d = tab(&ws, "dispatcher");
        assert_eq!(d.role, Some(Role::Dispatcher));
        let dp = &ws.panes[&pane(&ws, "dispatcher", "dispatcher")];
        assert_eq!((dp.kind.clone(), dp.role), (PaneKind::Terminal, Some(Role::Dispatcher)));
        assert_eq!(dp.session_name.as_deref(), Some("dispatcher"));
        let inbox = &ws.panes[&pane(&ws, "dispatcher", "inbox")];
        assert_eq!(inbox.kind, PaneKind::Applet { kind: "inbox".into() });
        assert_eq!((inbox.role, inbox.session_name.clone(), inbox.prompt.clone()), (None, None, None));

        // only the focus terminal pane is the standing agent
        let m = &ws.panes[&pane(&ws, "billing", "manager")];
        assert_eq!(m.role, Some(Role::Manager));
        assert_eq!(m.session_name.as_deref(), Some("billing"));
        for other in ["shell", "extra"] {
            let p = &ws.panes[&pane(&ws, "billing", other)];
            assert_eq!((p.role, p.session_name.clone()), (None, None), "{other}");
        }
        assert!(ws.panes.values().all(|p| p.team.is_none()));
        // phase-1 tabs carry no role at all
        assert_eq!(tab(&ws, "plain").role, None);
        assert!(tab(&ws, "plain").layout.leaves().iter().all(|p| ws.panes[p].role.is_none()));
        // a tab created at run time has no role either
        let mut ws2 = ws.clone();
        ws2.apply(create("web", "solo"), &l).unwrap();
        assert_eq!(tab(&ws2, "web").role, None);
        assert!(ws2.panes[&pane(&ws2, "web", "main")].role.is_none());
    }

    fn role_ws() -> (Loaded, Workspace) {
        let l = loaded_files(
            r#"{"dispatcher":{"template":"dispatcher-inbox"},
                "tabs":[{"name":"handover","role":"handover","dir":""},
                        {"name":"billing","role":"manager","dir":"billing","template":"manager"}]}"#,
            &[("dispatcher-inbox.json", INBOX_TPL), ("manager.json", MANAGER_TPL)],
        );
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let ws = Workspace::from_config(&l);
        (l, ws)
    }

    #[test]
    fn validate_rejects_broken_role_invariants() {
        let (_, good) = role_ws();
        let mut ok = good.clone();
        assert_eq!(ok.normalize_and_validate(), Ok(()));
        type M = (&'static str, Box<dyn Fn(&mut Workspace)>);
        let p = |w: &Workspace, t: &str, n: &str| pane(w, t, n);
        let cases: Vec<M> = vec![
            ("applet with a role", Box::new(move |w| {
                let id = p(w, "dispatcher", "inbox");
                w.panes.get_mut(&id).unwrap().role = Some(Role::Member);
            })),
            ("applet with a team", Box::new(move |w| {
                let id = p(w, "dispatcher", "inbox");
                w.panes.get_mut(&id).unwrap().team = Some("t".into());
            })),
            ("applet with a session name", Box::new(move |w| {
                let id = p(w, "dispatcher", "inbox");
                w.panes.get_mut(&id).unwrap().session_name = Some("x".into());
            })),
            ("applet with an agent", Box::new(move |w| {
                let id = p(w, "dispatcher", "inbox");
                w.panes.get_mut(&id).unwrap().agent = "claude".into();
            })),
            ("two handover tabs", Box::new(|w| {
                let id = tab(w, "billing").id.clone();
                w.tabs.get_mut(&id).unwrap().role = Some(Role::Handover);
                for pid in w.tabs[&id].layout.leaves() {
                    if w.panes[&pid].role.is_some() {
                        w.panes.get_mut(&pid).unwrap().role = Some(Role::Handover);
                    }
                }
            })),
            ("two standing panes in a tab", Box::new(move |w| {
                let id = p(w, "billing", "extra");
                let q = w.panes.get_mut(&id).unwrap();
                q.role = Some(Role::Manager);
                q.session_name = Some("billing-2".into());
            })),
            ("standing role mismatching the tab", Box::new(move |w| {
                let id = p(w, "billing", "manager");
                w.panes.get_mut(&id).unwrap().role = Some(Role::Handover);
            })),
            ("dispatcher role outside the dispatcher tab", Box::new(|w| {
                let id = tab(w, "billing").id.clone();
                w.tabs.get_mut(&id).unwrap().role = Some(Role::Dispatcher);
            })),
            ("duplicate sessionName", Box::new(move |w| {
                let id = p(w, "billing", "shell");
                w.panes.get_mut(&id).unwrap().session_name = Some("handover".into());
            })),
        ];
        for (name, f) in cases {
            let mut bad = good.clone();
            f(&mut bad);
            assert!(bad.normalize_and_validate().is_err(), "{name}");
        }
    }

    #[test]
    fn standing_pane_cannot_be_closed_or_broken_off() {
        let (l, mut ws) = role_ws();
        for (t, n) in [("billing", "manager"), ("handover", "main"), ("dispatcher", "dispatcher")] {
            let id = pane(&ws, t, n);
            for cmd in [WsCommand::ClosePane { pane: id.clone() }, WsCommand::BreakPaneToTab { pane: id.clone() }] {
                let before = ws.clone();
                let err = ws.apply(cmd, &l).unwrap_err();
                assert!(err.contains("상시 에이전트 창"), "{t}/{n}: {err}");
                assert_eq!(ws, before);
            }
            // restart stays allowed
            ws.apply(WsCommand::RestartPane { pane: id.clone() }, &l).unwrap();
        }
        // helper panes and applets of a role tab still close / break off normally
        ws.apply(WsCommand::ClosePane { pane: pane(&ws, "billing", "extra") }, &l).unwrap();
        ws.apply(WsCommand::BreakPaneToTab { pane: pane(&ws, "billing", "shell") }, &l).unwrap();
        ws.apply(WsCommand::ClosePane { pane: pane(&ws, "dispatcher", "inbox") }, &l).unwrap();
        // the whole standing task goes away with CloseTab
        let b = tab(&ws, "billing").id.clone();
        ws.apply(WsCommand::CloseTab { tab: b }, &l).unwrap();
        assert!(!has_tab(&ws, "billing"));
        let mut v = ws.clone();
        assert_eq!(v.normalize_and_validate(), Ok(()));
    }

    #[test]
    fn applet_panes_have_no_process() {
        let l = loaded_files(
            r#"{"dispatcher":{"template":"dispatcher-inbox"}}"#,
            &[("dispatcher-inbox.json", INBOX_TPL)],
        );
        let mut ws = Workspace::from_config(&l);
        let inbox = pane(&ws, "dispatcher", "inbox");
        let disp = pane(&ws, "dispatcher", "dispatcher");
        ws.set_status(&inbox, PaneStatus::Running);
        ws.set_status(&disp, PaneStatus::Running);
        assert_eq!(ws.running_count(), 1, "an applet is not a live process");
        // splitting an applet makes a terminal pane with the default agent
        ws.apply(WsCommand::SplitPane { pane: inbox.clone(), dir: Dir::Vertical }, &l).unwrap();
        let new = ws.tabs[&ws.panes[&inbox].tab].last_focused_pane_id.clone();
        assert_eq!(ws.panes[&new].kind, PaneKind::Terminal);
        assert_eq!(ws.panes[&new].agent, l.config.default_agent);
        assert_eq!(ws.panes[&new].role, None);
    }

    #[test]
    fn resize_falls_back_to_group_border() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let d = tab(&ws, "dispatcher").last_focused_pane_id.clone();
        assert!(near(&sizes(&ws.group_layout), &[50.0, 50.0]));
        ws.apply(WsCommand::ResizePane { pane: d, side: Side::Right, delta: 10.0 }, &l).unwrap();
        assert!(near(&sizes(&ws.group_layout), &[60.0, 40.0]));
        // a border inside the tab wins over the group border
        let lead = pane(&ws, "api", "lead");
        ws.apply(WsCommand::ResizePane { pane: lead, side: Side::Right, delta: 10.0 }, &l).unwrap();
        assert!(near(&sizes(&tab(&ws, "api").layout), &[70.0, 30.0]));
        assert!(near(&sizes(&ws.group_layout), &[60.0, 40.0]));
        // SetSizes on tab and group layouts
        let api = tab(&ws, "api").id.clone();
        ws.apply(WsCommand::SetSizes { tab: Some(api.clone()), path: vec![], sizes: vec![1.0, 3.0] }, &l).unwrap();
        assert!(near(&sizes(&tab(&ws, "api").layout), &[25.0, 75.0]));
        ws.apply(WsCommand::SetSizes { tab: None, path: vec![], sizes: vec![30.0, 70.0] }, &l).unwrap();
        assert!(near(&sizes(&ws.group_layout), &[30.0, 70.0]));
        let before = ws.clone();
        assert!(ws.apply(WsCommand::SetSizes { tab: Some(api), path: vec![7], sizes: vec![50.0, 50.0] }, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn break_pane_to_tab_keeps_process() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let api = tab(&ws, "api").clone();
        let coder = pane(&ws, "api", "coder");
        let fx = ws.apply(WsCommand::BreakPaneToTab { pane: coder.clone() }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        let nt = tab(&ws, "api-coder").clone();
        assert_eq!(nt.layout, LayoutNode::Leaf(coder.clone()));
        assert_eq!(nt.last_focused_pane_id, coder);
        assert_eq!(nt.workspace, api.workspace);
        assert_eq!(ws.panes[&coder].tab, nt.id);
        assert_eq!(pane_names(&ws, "api"), ["lead", "tests"]);
        let g = &ws.groups[&group_of(&ws, &api.id)];
        assert_eq!(g.tab_ids, vec![tab(&ws, "dispatcher").id.clone(), api.id.clone(), nt.id.clone()]);
        assert_eq!(g.active_tab_id, nt.id);
        // name collision gets a suffix
        ws.apply(create("api-tests", "solo"), &l).unwrap();
        ws.apply(WsCommand::BreakPaneToTab { pane: pane(&ws, "api", "tests") }, &l).unwrap();
        assert!(has_tab(&ws, "api-tests-2"));
        // breaking the only pane closes the origin tab but keeps the process
        let fx = ws.apply(WsCommand::BreakPaneToTab { pane: coder.clone() }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        assert!(!has_tab(&ws, "api-coder"));
        assert_eq!(tab(&ws, "api-coder-coder").layout, LayoutNode::Leaf(coder.clone()));
        assert!(ws.pane_by_id(&coder).is_some());
    }

    #[test]
    fn attention_reason_lifecycle() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let g = ws.focus.group_id.clone();
        let api = tab(&ws, "api").id.clone();
        let coder = pane(&ws, "api", "coder");
        let set = |ws: &mut Workspace, on: bool, reason: Option<&str>| {
            ws.apply(WsCommand::SetAttention { pane: coder.clone(), on, reason: reason.map(str::to_string) }, &l).unwrap();
        };
        set(&mut ws, true, Some("  배포\n승인\u{7}  필요\u{202E} "));
        assert!(ws.panes[&coder].attention);
        assert_eq!(ws.panes[&coder].attention_reason.as_deref(), Some("배포 승인 필요"));
        set(&mut ws, true, None); // a bell keeps the pending reason
        assert_eq!(ws.panes[&coder].attention_reason.as_deref(), Some("배포 승인 필요"));
        set(&mut ws, false, None);
        assert!(!ws.panes[&coder].attention && ws.panes[&coder].attention_reason.is_none());
        // focus clears both
        set(&mut ws, true, Some("질문"));
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(api.clone()), pane: Some(coder.clone()) }, &l).unwrap();
        assert!(!ws.panes[&coder].attention && ws.panes[&coder].attention_reason.is_none());
        // restart and a restored workspace clear it too
        set(&mut ws, true, Some("질문"));
        ws.apply(WsCommand::RestartPane { pane: coder.clone() }, &l).unwrap();
        assert!(ws.panes[&coder].attention_reason.is_none());
        set(&mut ws, true, Some("질문"));
        ws.reset_runtime_state();
        assert!(ws.panes[&coder].attention_reason.is_none());
        // wire shape: camelCase, and an old command / pane without the field still parses
        let c: WsCommand = serde_json::from_str(r#"{"type":"setAttention","pane":"p1","on":true}"#).unwrap();
        assert_eq!(c, WsCommand::SetAttention { pane: "p1".into(), on: true, reason: None });
        let v = serde_json::to_value(&ws.panes[&coder]).unwrap();
        assert!(v.as_object().unwrap().contains_key("attentionReason"));
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("attentionReason");
        let p: Pane = serde_json::from_value(old).unwrap();
        assert_eq!(p.attention_reason, None);
    }

    #[test]
    fn clean_reason_strips_controls_and_caps_length() {
        assert_eq!(clean_reason(""), None);
        assert_eq!(clean_reason(" \u{1b}[31m \t\r\n "), Some("[31m".into()));
        assert_eq!(clean_reason("\u{0}\u{7}\u{200F}"), None);
        // format characters (Cf): zero-width, word joiner, BOM, Arabic letter mark, tags
        assert_eq!(clean_reason("a\u{200B}b\u{2060}c\u{FEFF}d\u{061C}e\u{E0041}f\u{00AD}g"), Some("abcdefg".into()));
        let long = "가".repeat(ATTENTION_REASON_MAX + 50);
        assert_eq!(clean_reason(&long).unwrap().chars().count(), ATTENTION_REASON_MAX);
        assert_eq!(clean_reason("a  b\n\nc"), Some("a b c".into()));
    }

    #[test]
    fn set_focus_records_last_location_and_clears_attention() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let g = ws.focus.group_id.clone();
        let d = tab(&ws, "dispatcher").id.clone();
        let api = tab(&ws, "api").id.clone();
        let coder = pane(&ws, "api", "coder");
        ws.apply(WsCommand::SetAttention { pane: coder.clone(), on: true, reason: None }, &l).unwrap();
        assert!(ws.panes[&coder].attention);
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(api.clone()), pane: Some(coder.clone()) }, &l).unwrap();
        assert!(!ws.panes[&coder].attention);
        assert_eq!(ws.groups[&g].active_tab_id, api);
        assert_eq!(ws.tabs[&api].last_focused_pane_id, coder);
        assert_eq!(ws.last_location, Some(Location { group_id: g.clone(), tab_id: d.clone() }));
        // a pane change inside the same tab keeps lastLocation
        let lead = pane(&ws, "api", "lead");
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: None, pane: Some(lead.clone()) }, &l).unwrap();
        assert_eq!(ws.tabs[&api].last_focused_pane_id, lead);
        assert_eq!(ws.last_location, Some(Location { group_id: g.clone(), tab_id: d.clone() }));
        // switching tabs clears attention of that tab's last focused pane
        let dp = ws.tabs[&d].last_focused_pane_id.clone();
        ws.apply(WsCommand::SetAttention { pane: dp.clone(), on: true, reason: None }, &l).unwrap();
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(d.clone()), pane: None }, &l).unwrap();
        assert!(!ws.panes[&dp].attention);
        assert_eq!(ws.last_location, Some(Location { group_id: g.clone(), tab_id: api.clone() }));
        // invalid targets
        let before = ws.clone();
        for cmd in [
            WsCommand::SetFocus { group: "g99".into(), tab: None, pane: None },
            WsCommand::SetFocus { group: g.clone(), tab: Some("t99".into()), pane: None },
            WsCommand::SetFocus { group: g.clone(), tab: Some(d.clone()), pane: Some(coder.clone()) },
        ] {
            assert!(ws.apply(cmd, &l).is_err());
            assert_eq!(ws, before);
        }
    }

    #[test]
    fn maximize_and_sync_input() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let api = tab(&ws, "api").id.clone();
        let coder = pane(&ws, "api", "coder");
        ws.apply(WsCommand::SetMaximized { tab: api.clone(), pane: Some(coder.clone()) }, &l).unwrap();
        assert_eq!(ws.tabs[&api].maximized_pane_id, Some(coder.clone()));
        assert_eq!(ws.tabs[&api].last_focused_pane_id, coder);
        ws.apply(WsCommand::ClosePane { pane: coder }, &l).unwrap();
        assert_eq!(ws.tabs[&api].maximized_pane_id, None);
        ws.apply(WsCommand::SetSyncInput { tab: api.clone(), on: true }, &l).unwrap();
        assert!(ws.tabs[&api].sync_input);
        let d = tab(&ws, "dispatcher").last_focused_pane_id.clone();
        assert!(ws.apply(WsCommand::SetMaximized { tab: api, pane: Some(d) }, &l).is_err());
    }

    /// R22: focusing a pane hidden by a pane maximize clears that maximize; focusing the
    /// maximized pane itself (or just its tab) keeps it.
    #[test]
    fn set_focus_clears_pane_maximize_hiding_the_target() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let g = ws.focus.group_id.clone();
        let api = tab(&ws, "api").id.clone();
        let d = tab(&ws, "dispatcher").id.clone();
        let coder = pane(&ws, "api", "coder");
        let lead = pane(&ws, "api", "lead");
        ws.apply(WsCommand::SetMaximized { tab: api.clone(), pane: Some(coder.clone()) }, &l).unwrap();
        // the maximized pane itself / its tab without a pane / another tab: maximize kept
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(api.clone()), pane: Some(coder.clone()) }, &l).unwrap();
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(d.clone()), pane: None }, &l).unwrap();
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(api.clone()), pane: None }, &l).unwrap();
        assert_eq!(ws.tabs[&api].maximized_pane_id, Some(coder.clone()));
        // a hidden pane of that tab: maximize cleared, focus on it
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(api.clone()), pane: Some(lead.clone()) }, &l).unwrap();
        assert_eq!(ws.tabs[&api].maximized_pane_id, None);
        assert_eq!(ws.tabs[&api].last_focused_pane_id, lead);
    }

    /// R22: focusing a group other than the maximized one clears the group maximize.
    #[test]
    fn set_focus_clears_group_maximize_hiding_the_target() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        let api = tab(&ws, "api").id.clone();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: None, pane: None }, &l).unwrap();
        ws.apply(WsCommand::SetGroupMaximized { group: Some(work.clone()) }, &l).unwrap();
        // focus inside the maximized group keeps it
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: Some(api), pane: Some(pane(&ws, "api", "tests")) }, &l).unwrap();
        assert_eq!(ws.group_maximized, Some(work.clone()));
        // focus onto the hidden group clears it
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: None, pane: None }, &l).unwrap();
        assert_eq!(ws.group_maximized, None);
        assert_eq!(ws.focus.group_id, control);
    }

    /// CreateTab / BreakPaneToTab in the focused group record the tab left behind as
    /// last_location; in another group they leave focus alone (R12).
    #[test]
    fn create_and_break_record_last_location() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let g = ws.focus.group_id.clone();
        let d = tab(&ws, "dispatcher").id.clone();
        let api = tab(&ws, "api").id.clone();
        ws.apply(create("web", "solo"), &l).unwrap();
        assert_eq!(ws.last_location, Some(Location { group_id: g.clone(), tab_id: d.clone() }));
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(api.clone()), pane: None }, &l).unwrap();
        ws.apply(WsCommand::BreakPaneToTab { pane: pane(&ws, "api", "coder") }, &l).unwrap();
        assert_eq!(ws.groups[&g].active_tab_id, tab(&ws, "api-coder").id);
        assert_eq!(ws.last_location, Some(Location { group_id: g.clone(), tab_id: api.clone() }));

        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        let before_focus = ws.focus_location();
        ws.apply(WsCommand::CreateTab { name: "w2".into(), dir: "w2".into(), template: None, group: Some(work.clone()), role: None }, &l)
            .unwrap();
        // R12 (Task 8): another group's active tab does not change either
        assert_eq!(ws.groups[&work].active_tab_id, tab(&ws, "api").id);
        assert_eq!(ws.focus.group_id, control);
        assert_eq!(ws.focus_location(), before_focus);
        assert_eq!(ws.last_location, None);
        ws.apply(WsCommand::BreakPaneToTab { pane: pane(&ws, "api", "lead") }, &l).unwrap();
        assert_eq!(ws.groups[&work].active_tab_id, tab(&ws, "api-lead").id);
        assert_eq!(ws.focus_location(), before_focus);
        assert_eq!(ws.last_location, None);
    }

    #[test]
    fn restart_clears_attention_and_prompt_warning() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let p = pane(&ws, "api", "coder");
        ws.apply(WsCommand::SetAttention { pane: p.clone(), on: true, reason: None }, &l).unwrap();
        ws.set_prompt_warning(&p, true);
        ws.apply(WsCommand::RestartPane { pane: p.clone() }, &l).unwrap();
        assert!(!ws.panes[&p].attention);
        assert!(!ws.panes[&p].prompt_warning);
    }

    /// `last_location` is None or names an existing group containing the tab, and is not the focus.
    fn assert_inv(ws: &Workspace) {
        if let Some(l) = &ws.last_location {
            assert!(ws.groups.get(&l.group_id).is_some_and(|g| g.tab_ids.contains(&l.tab_id)), "stale {l:?}");
            assert_ne!(ws.last_location, ws.focus_location());
        }
        assert!(ws.groups.contains_key(&ws.focus.group_id));
    }
    fn gid_named(ws: &Workspace, name: &str) -> GroupId {
        ws.groups.values().find(|g| g.name == name).unwrap_or_else(|| panic!("no group {name}")).id.clone()
    }
    fn tab_names(ws: &Workspace, g: &str) -> Vec<String> {
        ws.groups[g].tab_ids.iter().map(|t| ws.tabs[t].name.clone()).collect()
    }
    fn create_in(name: &str, group: &str) -> WsCommand {
        WsCommand::CreateTab { name: name.into(), dir: name.into(), template: Some("solo".into()), group: Some(group.into()), role: None }
    }

    #[test]
    fn move_tab_between_groups_and_remove_empty() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        let api = tab(&ws, "api").id.clone();
        let d = tab(&ws, "dispatcher").id.clone();
        let fx = ws.apply(WsCommand::MoveTab { tab: api.clone(), group: control.clone(), index: 0 }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        assert_eq!(tab_names(&ws, &control), ["api", "dispatcher"]);
        assert!(!ws.groups.contains_key(&work));
        assert_eq!(ws.group_layout, LayoutNode::Leaf(control.clone()));
        assert_eq!(ws.groups[&control].active_tab_id, api);
        assert_eq!(ws.focus.group_id, control);
        assert_eq!(ws.last_location, Some(Location { group_id: control.clone(), tab_id: d }));
        // source group survives; its active tab follows the CloseTab rule (right, else left)
        let mut ws = Workspace::from_config(&l);
        ws.apply(create_in("x", &work), &l).unwrap();
        assert_inv(&ws);
        ws.apply(create_in("y", &work), &l).unwrap();
        assert_inv(&ws);
        let x = tab(&ws, "x").id.clone();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: Some(x.clone()), pane: None }, &l).unwrap();
        assert_inv(&ws);
        let before_loc = ws.last_location.clone();
        assert!(before_loc.is_some());
        ws.apply(WsCommand::MoveTab { tab: x.clone(), group: control.clone(), index: 1 }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(tab_names(&ws, &work), ["api", "y"]);
        assert_eq!(ws.groups[&work].active_tab_id, tab(&ws, "y").id);
        assert_eq!(tab_names(&ws, &control), ["dispatcher", "x"]);
        assert_eq!(ws.groups[&control].active_tab_id, x);
        assert_eq!(ws.focus.group_id, control);
        // the previous location (work, x) no longer exists → lastLocation is left alone
        assert_eq!(ws.last_location, before_loc);
        // errors leave the model unchanged
        let before = ws.clone();
        assert!(ws.apply(WsCommand::MoveTab { tab: "t99".into(), group: control.clone(), index: 0 }, &l).is_err());
        assert!(ws.apply(WsCommand::MoveTab { tab: x, group: "g99".into(), index: 0 }, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn move_tab_reorders_within_group() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        ws.apply(create("b", "solo"), &l).unwrap();
        assert_inv(&ws);
        ws.apply(create("c", "solo"), &l).unwrap();
        assert_inv(&ws);
        let g = ws.focus.group_id.clone();
        let c = tab(&ws, "c").id.clone();
        let d = tab(&ws, "dispatcher").id.clone();
        let fx = ws.apply(WsCommand::MoveTab { tab: c.clone(), group: g.clone(), index: 1 }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        assert_eq!(tab_names(&ws, &g), ["dispatcher", "c", "api", "b"]);
        assert_eq!(ws.groups[&g].active_tab_id, c);
        assert_eq!(ws.groups.len(), 1);
        // the dispatcher can be reordered too
        ws.apply(WsCommand::MoveTab { tab: d, group: g.clone(), index: 2 }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(tab_names(&ws, &g), ["c", "api", "dispatcher", "b"]);
    }

    #[test]
    fn move_tab_index_out_of_range_appends() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let api = tab(&ws, "api").id.clone();
        ws.apply(create_in("x", &gid_named(&ws, "work")), &l).unwrap();
        assert_inv(&ws);
        ws.apply(WsCommand::MoveTab { tab: api, group: control.clone(), index: 99 }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(tab_names(&ws, &control), ["dispatcher", "api"]);
        // same group, out of range → end
        let d = tab(&ws, "dispatcher").id.clone();
        ws.apply(WsCommand::MoveTab { tab: d, group: control.clone(), index: 99 }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(tab_names(&ws, &control), ["api", "dispatcher"]);
    }

    #[test]
    fn split_group_with_creates_group_before_and_after() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        ws.apply(create("b", "solo"), &l).unwrap();
        assert_inv(&ws);
        ws.apply(create("c", "solo"), &l).unwrap();
        assert_inv(&ws);
        let old = ws.focus.group_id.clone();
        let b = tab(&ws, "b").id.clone();
        let c = tab(&ws, "c").id.clone();
        let fx = ws
            .apply(WsCommand::SplitGroupWith { tab: b.clone(), group: old.clone(), dir: Dir::Horizontal, before: true }, &l)
            .unwrap();
        assert_eq!(fx, Effects::default());
        let leaves = ws.group_layout.leaves();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[1], old);
        let new1 = leaves[0].clone();
        assert_ne!(new1, old);
        assert_eq!(ws.groups[&new1].tab_ids, vec![b.clone()]);
        assert_eq!(ws.groups[&new1].active_tab_id, b);
        assert!(ws.groups[&new1].name.starts_with("group-"));
        assert_eq!(tab_names(&ws, &old), ["dispatcher", "api", "c"]);
        assert_eq!(ws.focus.group_id, new1);
        // after
        ws.apply(WsCommand::SplitGroupWith { tab: c.clone(), group: old.clone(), dir: Dir::Horizontal, before: false }, &l)
            .unwrap();
        let leaves = ws.group_layout.leaves();
        assert_eq!(leaves.len(), 3);
        assert_eq!(&leaves[..2], &[new1.clone(), old.clone()]);
        let new2 = leaves[2].clone();
        assert_eq!(ws.groups[&new2].tab_ids, vec![c]);
        assert_ne!(ws.groups[&new1].name, ws.groups[&new2].name);
        // a different direction nests
        let api = tab(&ws, "api").id.clone();
        ws.apply(WsCommand::SplitGroupWith { tab: api, group: old, dir: Dir::Vertical, before: false }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.group_layout.leaf_count(), 4);
    }

    #[test]
    fn split_group_with_single_tab_errors() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let work = gid_named(&ws, "work");
        let api = tab(&ws, "api").id.clone();
        let before = ws.clone();
        let e = ws
            .apply(WsCommand::SplitGroupWith { tab: api, group: work, dir: Dir::Horizontal, before: false }, &l)
            .unwrap_err();
        assert_eq!(e, "탭이 하나뿐인 그룹은 분할할 수 없습니다");
        assert_eq!(ws, before);
    }

    #[test]
    fn split_group_with_moves_only_tab_to_other_group_edge() {
        // R4: a group's only tab may go to another group's edge; the emptied source is removed
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        let api = tab(&ws, "api").id.clone();
        let fx = ws
            .apply(WsCommand::SplitGroupWith { tab: api.clone(), group: control.clone(), dir: Dir::Vertical, before: true }, &l)
            .unwrap();
        assert_eq!(fx, Effects::default());
        assert!(!ws.groups.contains_key(&work));
        let leaves = ws.group_layout.leaves();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[1], control);
        assert_eq!(ws.groups[&leaves[0]].tab_ids, vec![api]);
        assert_eq!(ws.groups.len(), 2);
        assert_eq!(ws.focus.group_id, leaves[0]);
    }

    #[test]
    fn merge_group_appends_in_order_keeps_dst_name() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        ws.apply(create_in("x", &work), &l).unwrap();
        assert_inv(&ws);
        ws.apply(create_in("y", &work), &l).unwrap();
        assert_inv(&ws);
        let d = tab(&ws, "dispatcher").id.clone();
        let fx = ws.apply(WsCommand::MergeGroup { src: work.clone(), dst: control.clone() }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        assert_eq!(tab_names(&ws, &control), ["dispatcher", "api", "x", "y"]);
        assert_eq!(ws.groups[&control].name, "control");
        assert_eq!(ws.groups[&control].active_tab_id, d);
        assert!(!ws.groups.contains_key(&work));
        assert_eq!(ws.group_layout, LayoutNode::Leaf(control.clone()));
        assert_eq!(ws.focus.group_id, control);
        // focus on the src group keeps its focused tab, now living in dst
        let mut ws = Workspace::from_config(&l);
        let api = tab(&ws, "api").id.clone();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: None, pane: None }, &l).unwrap();
        assert_inv(&ws);
        ws.apply(WsCommand::MergeGroup { src: work.clone(), dst: control.clone() }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.focus.group_id, control);
        assert_eq!(ws.groups[&control].active_tab_id, api);
        assert_eq!(ws.last_location, Some(Location { group_id: control.clone(), tab_id: d }));
        // errors
        let before = ws.clone();
        assert!(ws.apply(WsCommand::MergeGroup { src: control.clone(), dst: control.clone() }, &l).is_err());
        assert!(ws.apply(WsCommand::MergeGroup { src: "g99".into(), dst: control }, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn close_group_merges_into_sibling() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        let fx = ws.apply(WsCommand::CloseGroup { group: work.clone() }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        assert_eq!(tab_names(&ws, &control), ["dispatcher", "api"]);
        assert_eq!(ws.group_layout, LayoutNode::Leaf(control.clone()));
        assert!(!ws.groups.contains_key(&work));
        // closing the first of three merges into its adjacent sibling
        let l = loaded_with(
            r#"{"tabs":[{"name":"a","dir":"a"},{"name":"b","dir":"b"}],
                "groups":{"split":"horizontal","children":[
                    {"group":{"name":"one","tabs":["dispatcher"]}},
                    {"group":{"name":"two","tabs":["a"]}},
                    {"group":{"name":"three","tabs":["b"]}}]}}"#,
        );
        let mut ws = Workspace::from_config(&l);
        let (one, two) = (gid_named(&ws, "one"), gid_named(&ws, "two"));
        ws.apply(WsCommand::CloseGroup { group: one }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(tab_names(&ws, &two), ["a", "dispatcher"]);
        assert_eq!(ws.group_layout.leaf_count(), 2);
    }

    #[test]
    fn close_only_group_errors() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let before = ws.clone();
        let g = ws.focus.group_id.clone();
        assert_eq!(ws.apply(WsCommand::CloseGroup { group: g }, &l), Err("그룹이 하나뿐입니다".to_string()));
        assert_eq!(ws, before);
        assert!(ws.apply(WsCommand::CloseGroup { group: "g99".into() }, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn dispatcher_tab_can_move_groups() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let work = gid_named(&ws, "work");
        let d = tab(&ws, "dispatcher").id.clone();
        ws.apply(WsCommand::MoveTab { tab: d.clone(), group: work.clone(), index: 0 }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(tab_names(&ws, &work), ["dispatcher", "api"]);
        assert_eq!(group_of(&ws, &d), work);
        assert!(ws.tabs[&d].is_dispatcher);
        assert_eq!(ws.groups.len(), 1);
        assert_eq!(ws.group_layout, LayoutNode::Leaf(work));
    }

    #[test]
    fn merge_never_kills_processes() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        let api = tab(&ws, "api").id.clone();
        ws.apply(create_in("x", &work), &l).unwrap();
        assert_inv(&ws);
        let alive = ws.running_count();
        let x = tab(&ws, "x").id.clone();
        let cmds = vec![
            WsCommand::SetGroupMaximized { group: Some(work.clone()) },
            WsCommand::MoveTab { tab: api, group: control.clone(), index: 0 },
            WsCommand::SplitGroupWith { tab: x, group: control, dir: Dir::Horizontal, before: false },
        ];
        for c in cmds {
            assert_eq!(ws.apply(c, &l).unwrap(), Effects::default());
        }
        let groups = ws.group_layout.leaves();
        let fx = ws.apply(WsCommand::MergeGroup { src: groups[1].clone(), dst: groups[0].clone() }, &l).unwrap();
        assert_eq!(fx, Effects::default());
        assert_eq!(ws.group_layout.leaf_count(), 1);
        assert_eq!(ws.running_count(), alive);
        // CloseGroup too
        let mut ws = Workspace::from_config(&l);
        let before = ws.running_count();
        assert_eq!(ws.apply(WsCommand::CloseGroup { group: work }, &l).unwrap(), Effects::default());
        assert_eq!(ws.running_count(), before);
    }

    #[test]
    fn group_removal_clears_stale_last_location_and_maximized() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        ws.apply(create_in("b", &control), &l).unwrap();
        assert_inv(&ws);
        let (d, b) = (tab(&ws, "dispatcher").id.clone(), tab(&ws, "b").id.clone());
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: Some(d.clone()), pane: None }, &l).unwrap();
        assert_inv(&ws);
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: Some(b.clone()), pane: None }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.last_location, Some(Location { group_id: control.clone(), tab_id: d.clone() }));
        // closing the focused tab falls back to the dispatcher → lastLocation now equals the focus
        ws.apply(WsCommand::CloseTab { tab: b }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.groups[&control].active_tab_id, d);
        // R13: (control, dispatcher) is the focus itself now, so the stale "go back" is cleared
        assert_eq!(ws.last_location, None);
        ws.apply(WsCommand::SetGroupMaximized { group: Some(work.clone()) }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.group_maximized, Some(work.clone()));
        ws.apply(WsCommand::CloseGroup { group: work.clone() }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.last_location, None);
        assert_eq!(ws.group_maximized, None);
        // SetGroupMaximized validates the group
        let before = ws.clone();
        assert!(ws.apply(WsCommand::SetGroupMaximized { group: Some(work) }, &l).is_err());
        assert_eq!(ws, before);
        ws.apply(WsCommand::SetGroupMaximized { group: None }, &l).unwrap();
        assert_inv(&ws);
    }

    #[test]
    fn last_location_never_points_at_a_tab_that_left_its_group() {
        let l = loaded_groups();
        let mut ws = Workspace::from_config(&l);
        let control = gid_named(&ws, "control");
        let work = gid_named(&ws, "work");
        ws.apply(create_in("x", &control), &l).unwrap();
        ws.apply(create_in("y", &work), &l).unwrap();
        let (x, api) = (tab(&ws, "x").id.clone(), tab(&ws, "api").id.clone());
        // an explicit group creates in the background (R12): look at x first
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: Some(x.clone()), pane: None }, &l).unwrap();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: Some(api.clone()), pane: None }, &l).unwrap();
        assert_eq!(ws.last_location, Some(Location { group_id: control.clone(), tab_id: x.clone() }));
        ws.apply(WsCommand::CloseTab { tab: api }, &l).unwrap();
        assert_inv(&ws);
        assert_eq!(ws.last_location, Some(Location { group_id: control, tab_id: x.clone() }));
        // x leaves its group: the "go back" target no longer exists
        ws.apply(WsCommand::MoveTab { tab: x, group: work.clone(), index: 0 }, &l).unwrap();
        assert_inv(&ws);
        // never the stale (control, x); the valid previous location (work, y) is recorded instead
        assert_eq!(ws.last_location, Some(Location { group_id: work, tab_id: tab(&ws, "y").id.clone() }));
    }

    #[test]
    fn ws_command_json_shape() {
        let c: WsCommand = serde_json::from_str(r#"{"type":"splitPane","pane":"p1","dir":"vertical"}"#).unwrap();
        assert_eq!(c, WsCommand::SplitPane { pane: "p1".into(), dir: Dir::Vertical });
        let c: WsCommand = serde_json::from_str(r#"{"type":"setFocus","group":"g1","tab":null,"pane":null}"#).unwrap();
        assert_eq!(c, WsCommand::SetFocus { group: "g1".into(), tab: None, pane: None });
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"type":"setFocus","group":"g1","tab":null,"pane":null})
        );
        let c: WsCommand =
            serde_json::from_str(r#"{"type":"createTab","name":"x","dir":"x","template":null,"group":null}"#).unwrap();
        assert!(matches!(c, WsCommand::CreateTab { .. }));
        let c: WsCommand = serde_json::from_str(r#"{"type":"resizePane","pane":"p2","side":"left","delta":5}"#).unwrap();
        assert_eq!(c, WsCommand::ResizePane { pane: "p2".into(), side: Side::Left, delta: 5.0 });
        let c: WsCommand = serde_json::from_str(r#"{"type":"breakPaneToTab","pane":"p2"}"#).unwrap();
        assert_eq!(c, WsCommand::BreakPaneToTab { pane: "p2".into() });
        let c: WsCommand =
            serde_json::from_str(r#"{"type":"splitGroupWith","tab":"t1","group":"g1","dir":"horizontal","before":true}"#)
                .unwrap();
        assert!(matches!(c, WsCommand::SplitGroupWith { before: true, .. }));
        let c: WsCommand = serde_json::from_str(r#"{"type":"setGroupMaximized","group":null}"#).unwrap();
        assert_eq!(c, WsCommand::SetGroupMaximized { group: None });
        assert!(serde_json::from_str::<WsCommand>(r#"{"type":"SplitPane","pane":"p1","dir":"vertical"}"#).is_err());
    }

    #[test]
    fn snapshot_json_shape() {
        assert_eq!(serde_json::to_value(PaneStatus::Running).unwrap(), serde_json::json!({"state":"running"}));
        assert_eq!(
            serde_json::to_value(PaneStatus::Exited { code: 3 }).unwrap(),
            serde_json::json!({"state":"exited","code":3})
        );
        assert_eq!(
            serde_json::to_value(PaneStatus::Failed { message: "m".into() }).unwrap(),
            serde_json::json!({"state":"failed","message":"m"})
        );
        let l = loaded_fixture();
        let ws = Workspace::from_config(&l);
        let v = serde_json::to_value(&ws).unwrap();
        for k in ["groupLayout", "groups", "tabs", "panes", "focus", "lastLocation", "groupMaximized", "counters"] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert_eq!(v["groupLayout"], serde_json::json!({"leaf":"g1"}));
        assert_eq!(v["focus"], serde_json::json!({"groupId":"g1"}));
        assert_eq!(v["counters"], serde_json::json!({"group":1,"tab":2,"pane":4}));
        let t = &v["tabs"]["t1"];
        for k in ["isDispatcher", "lastFocusedPaneId", "maximizedPaneId", "syncInput", "workspace", "layout"] {
            assert!(t.get(k).is_some(), "{k}");
        }
        assert_eq!(v["groups"]["g1"]["activeTabId"], "t1");
        assert_eq!(v["groups"]["g1"]["tabIds"], serde_json::json!(["t1", "t2"]));
        let p = &v["panes"]["p1"];
        assert_eq!(p["status"], serde_json::json!({"state":"starting"}));
        assert_eq!(p["promptWarning"], false);
        assert_eq!(p["tab"], "t1");
        // stable round-trip (persistence)
        let back: Workspace = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(back, ws);
        assert_eq!(serde_json::to_string(&back).unwrap(), serde_json::to_string(&ws).unwrap());
    }

    // ── phase 2: teams (Task 8) ──

    const SLOT_TPL: &str = r#"{"name":"mgr-slot","layout":{"split":"horizontal","sizes":[60,40],"children":[
        {"pane":{"name":"manager","focus":true}},
        {"split":"vertical","slot":"team","children":[{"pane":{"name":"notes","agent":"powershell"}},{"pane":{"name":"log","agent":"powershell"}}]}]}}"#;

    fn add(tab: &str, team: &str, role: Role) -> WsCommand {
        WsCommand::AddTeamPane { tab: tab.into(), team: team.into(), role, prompt: Some("go".into()) }
    }
    fn find_slot<'a>(n: &'a LayoutNode<PaneId>, slot: &str) -> Option<&'a LayoutNode<PaneId>> {
        match n {
            LayoutNode::Split { slot: Some(s), .. } if s == slot => Some(n),
            LayoutNode::Split { children, .. } => children.iter().find_map(|c| find_slot(c, slot)),
            LayoutNode::Leaf(_) => None,
        }
    }
    fn split_parts(n: &LayoutNode<PaneId>) -> (Dir, Vec<f32>, Vec<LayoutNode<PaneId>>) {
        match n {
            LayoutNode::Split { dir, sizes, children, .. } => (*dir, sizes.clone(), children.clone()),
            LayoutNode::Leaf(_) => panic!("not a split"),
        }
    }
    fn by_session(ws: &Workspace, s: &str) -> PaneId {
        ws.panes.values().find(|p| p.session_name.as_deref() == Some(s)).unwrap_or_else(|| panic!("no session {s}")).id.clone()
    }
    fn leaf(p: &str) -> LayoutNode<PaneId> {
        LayoutNode::Leaf(p.to_string())
    }
    fn valid(ws: &Workspace) {
        let mut v = ws.clone();
        assert_eq!(v.normalize_and_validate(), Ok(()));
    }

    #[test]
    fn hire_lead_then_members_into_team_slot_without_focus_change() {
        let (l, mut ws) = role_ws();
        let b = tab(&ws, "billing").id.clone();
        let mgr = pane(&ws, "billing", "manager");
        let focus = ws.focus_location();
        let last_loc = ws.last_location.clone();
        let active: Vec<_> = ws.groups.values().map(|g| g.active_tab_id.clone()).collect();
        let lf = ws.tabs[&b].last_focused_pane_id.clone();

        let fx = ws.apply(add(&b, "batch", Role::Lead), &l).unwrap();
        let lead = by_session(&ws, "billing-batch-lead");
        assert_eq!(fx, Effects { spawn: vec![(lead.clone(), true)], kill: vec![] });
        let p = &ws.panes[&lead];
        assert_eq!((p.role, p.team.as_deref(), p.name.as_str()), (Some(Role::Lead), Some("batch"), "billing-batch-lead"));
        assert_eq!((p.tab.clone(), p.cwd.clone(), p.kind.clone()), (b.clone(), root(&l).join("billing"), PaneKind::Terminal));
        assert_eq!((p.agent.as_str(), p.prompt.as_deref()), ("claude", Some("go")));
        // no template slot → the Manager pane is split to the right into a team area
        let (dir, sz, ch) = split_parts(find_slot(&ws.tabs[&b].layout, TEAM_SLOT).expect("team area"));
        assert_eq!((dir, ch), (Dir::Horizontal, vec![leaf(&mgr), leaf(&lead)]));
        assert!(near(&sz, &[55.0, 45.0]), "{sz:?}");

        // members stack under their lead in a vertical `team:batch` column
        ws.apply(add(&b, "batch", Role::Member), &l).unwrap();
        ws.apply(add(&b, "batch", Role::Member), &l).unwrap();
        let (m1, m2) = (by_session(&ws, "billing-batch-m1"), by_session(&ws, "billing-batch-m2"));
        assert_eq!(ws.panes[&m1].name, "billing-batch-m1");
        let (dir, _, ch) = split_parts(find_slot(&ws.tabs[&b].layout, "team:batch").expect("team column"));
        assert_eq!((dir, ch), (Dir::Vertical, vec![leaf(&lead), leaf(&m1), leaf(&m2)]));
        let (_, _, area) = split_parts(find_slot(&ws.tabs[&b].layout, TEAM_SLOT).unwrap());
        assert_eq!(area.len(), 2, "still one column for the team");

        // focus, last location, active tabs and the tab's focused pane never moved
        assert_eq!(ws.focus_location(), focus);
        assert_eq!(ws.last_location, last_loc);
        assert_eq!(ws.groups.values().map(|g| g.active_tab_id.clone()).collect::<Vec<_>>(), active);
        assert_eq!(ws.tabs[&b].last_focused_pane_id, lf);

        // a dismissed member's number is never reused while the team exists
        ws.apply(WsCommand::ClosePane { pane: m2.clone() }, &l).unwrap();
        ws.apply(add(&b, "batch", Role::Member), &l).unwrap();
        assert!(ws.panes.values().any(|p| p.session_name.as_deref() == Some("billing-batch-m3")));
        assert!(!ws.panes.values().any(|p| p.session_name.as_deref() == Some("billing-batch-m2")));
        valid(&ws);
        // the result survives a persist round-trip
        let back: Workspace = serde_json::from_value(serde_json::to_value(&ws).unwrap()).unwrap();
        assert_eq!(back, ws);

        // a template `slot: "team"` split receives the team columns itself
        let l = loaded_files(
            r#"{"tabs":[{"name":"ops","role":"manager","dir":"ops","template":"mgr-slot"}]}"#,
            &[("mgr-slot.json", SLOT_TPL)],
        );
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let mut ws = Workspace::from_config(&l);
        let o = tab(&ws, "ops").id.clone();
        let (notes, log) = (pane(&ws, "ops", "notes"), pane(&ws, "ops", "log"));
        ws.apply(add(&o, "x", Role::Lead), &l).unwrap();
        let lx = by_session(&ws, "ops-x-lead");
        let (dir, _, ch) = split_parts(find_slot(&ws.tabs[&o].layout, TEAM_SLOT).unwrap());
        assert_eq!((dir, ch), (Dir::Vertical, vec![leaf(&notes), leaf(&log), leaf(&lx)]));
        assert_eq!(ws.tabs[&o].layout.leaves()[0], pane(&ws, "ops", "manager"), "manager pane not split");
        // dismissing the last team keeps the template's own slot split
        ws.apply(WsCommand::RemoveTeam { tab: o.clone(), team: "x".into() }, &l).unwrap();
        let (_, _, ch) = split_parts(find_slot(&ws.tabs[&o].layout, TEAM_SLOT).unwrap());
        assert_eq!(ch, vec![leaf(&notes), leaf(&log)]);
        valid(&ws);
    }

    #[test]
    fn multiple_teams_per_tab() {
        let (l, mut ws) = role_ws();
        let b = tab(&ws, "billing").id.clone();
        let mgr = pane(&ws, "billing", "manager");
        for t in ["a", "b", "c-2"] {
            ws.apply(add(&b, t, Role::Lead), &l).unwrap();
        }
        ws.apply(add(&b, "a", Role::Member), &l).unwrap();
        ws.apply(add(&b, "c-2", Role::Member), &l).unwrap();
        let (la, lb, lc) = (by_session(&ws, "billing-a-lead"), by_session(&ws, "billing-b-lead"), by_session(&ws, "billing-c-2-lead"));
        let (dir, sz, ch) = split_parts(find_slot(&ws.tabs[&b].layout, TEAM_SLOT).unwrap());
        assert_eq!(dir, Dir::Horizontal);
        assert_eq!(ch.len(), 4, "manager + one column per team");
        assert_eq!(ch[0], leaf(&mgr));
        assert_eq!(ch[2], leaf(&lb));
        assert_eq!(split_parts(&ch[1]).2, vec![leaf(&la), leaf(&by_session(&ws, "billing-a-m1"))]);
        assert_eq!(split_parts(&ch[3]).2, vec![leaf(&lc), leaf(&by_session(&ws, "billing-c-2-m1"))]);
        // the manager keeps its 55 %, the teams share the rest
        assert!(near(&sz, &[55.0, 15.0, 15.0, 15.0]), "{sz:?}");
        // each team numbers its members on its own
        assert!(ws.panes.values().filter(|p| p.role == Some(Role::Member)).all(|p| p.name.ends_with("-m1")));
        valid(&ws);

        // team ids: [a-z0-9-]{1,24}, unique per tab (a second lead for a team is refused)
        let before = ws.clone();
        let long = "x".repeat(25);
        for team in ["A", "", "a_b", "../x", long.as_str(), "a"] {
            assert!(ws.apply(add(&b, team, Role::Lead), &l).is_err(), "{team:?}");
            assert_eq!(ws, before);
        }
        assert!(ws.apply(add(&b, &"y".repeat(24), Role::Lead), &l).is_ok());
        // teams live in Manager tabs only
        for t in ["dispatcher", "handover"] {
            let id = tab(&ws, t).id.clone();
            let err = ws.apply(add(&id, "a", Role::Lead), &l).unwrap_err();
            assert!(err.contains("Manager"), "{err}");
        }
        // another Manager tab (created at run time with the role) may reuse a team id
        ws.apply(
            WsCommand::CreateTab { name: "ops".into(), dir: "ops".into(), template: Some("manager".into()), group: None, role: Some(Role::Manager) },
            &l,
        )
        .unwrap();
        let o = tab(&ws, "ops").id.clone();
        assert_eq!(ws.tabs[&o].role, Some(Role::Manager));
        assert_eq!(ws.panes[&pane(&ws, "ops", "manager")].session_name.as_deref(), Some("ops"));
        ws.apply(add(&o, "a", Role::Lead), &l).unwrap();
        assert!(ws.panes.values().any(|p| p.session_name.as_deref() == Some("ops-a-lead")));
        valid(&ws);
    }

    #[test]
    fn member_cap_and_lead_required() {
        let l = loaded_files(
            r#"{"tabs":[{"name":"billing","role":"manager","dir":"billing","template":"manager"}],
                "roles":{"member":{"maxPerTeam":2,"agent":"powershell"}}}"#,
            &[("manager.json", MANAGER_TPL)],
        );
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let mut ws = Workspace::from_config(&l);
        let b = tab(&ws, "billing").id.clone();
        // a member needs its team's lead first
        let before = ws.clone();
        let err = ws.apply(add(&b, "t", Role::Member), &l).unwrap_err();
        assert!(err.contains("리드"), "{err}");
        assert_eq!(ws, before);
        ws.apply(add(&b, "t", Role::Lead), &l).unwrap();
        ws.apply(add(&b, "t", Role::Member), &l).unwrap();
        ws.apply(add(&b, "t", Role::Member), &l).unwrap();
        let before = ws.clone();
        let err = ws.apply(add(&b, "t", Role::Member), &l).unwrap_err();
        assert!(err.contains("상한") && err.contains('2'), "{err}");
        assert_eq!(ws, before);
        // roles.<role>.agent picks the profile; the lead falls back to defaultAgent
        assert_eq!(ws.panes[&by_session(&ws, "billing-t-m1")].agent, "powershell");
        assert_eq!(ws.panes[&by_session(&ws, "billing-t-lead")].agent, "claude");
        // a freed seat can be filled again, with a new number
        ws.apply(WsCommand::ClosePane { pane: by_session(&ws, "billing-t-m1") }, &l).unwrap();
        ws.apply(add(&b, "t", Role::Member), &l).unwrap();
        by_session(&ws, "billing-t-m3");
        // only lead / member panes are team panes
        for r in [Role::Manager, Role::Dispatcher, Role::Handover] {
            assert!(ws.apply(add(&b, "t", r), &l).is_err());
        }
        // the cap is per team
        ws.apply(add(&b, "u", Role::Lead), &l).unwrap();
        ws.apply(add(&b, "u", Role::Member), &l).unwrap();
        ws.apply(add(&b, "u", Role::Member), &l).unwrap();
        valid(&ws);
    }

    #[test]
    fn remove_team_kills_all_team_panes() {
        let (l, mut ws) = role_ws();
        let b = tab(&ws, "billing").id.clone();
        let original = ws.tabs[&b].layout.clone();
        let mgr = pane(&ws, "billing", "manager");
        for (t, r) in [("a", Role::Lead), ("a", Role::Member), ("a", Role::Member), ("b", Role::Lead), ("b", Role::Member)] {
            ws.apply(add(&b, t, r), &l).unwrap();
        }
        let a_ids: BTreeSet<PaneId> = ws.panes.values().filter(|p| p.team.as_deref() == Some("a")).map(|p| p.id.clone()).collect();
        assert_eq!(a_ids.len(), 3);
        // the user looks at a member of team a, maximized
        let g = group_of(&ws, &b);
        let a_m1 = by_session(&ws, "billing-a-m1");
        ws.apply(WsCommand::SetFocus { group: g.clone(), tab: Some(b.clone()), pane: Some(a_m1.clone()) }, &l).unwrap();
        ws.apply(WsCommand::SetMaximized { tab: b.clone(), pane: Some(a_m1.clone()) }, &l).unwrap();

        let fx = ws.apply(WsCommand::RemoveTeam { tab: b.clone(), team: "a".into() }, &l).unwrap();
        assert!(fx.spawn.is_empty());
        assert_eq!(fx.kill.iter().cloned().collect::<BTreeSet<_>>(), a_ids);
        assert!(a_ids.iter().all(|p| !ws.panes.contains_key(p) && !ws.tabs[&b].layout.contains(p)));
        assert!(!ws.tabs[&b].team_seq.contains_key("a"));
        assert_eq!(ws.tabs[&b].last_focused_pane_id, mgr, "focus falls back to the Manager pane");
        assert_eq!(ws.tabs[&b].maximized_pane_id, None);
        // the remaining team gets the freed width back
        let (_, sz, ch) = split_parts(find_slot(&ws.tabs[&b].layout, TEAM_SLOT).unwrap());
        assert_eq!(ch.len(), 2);
        assert!(near(&sz, &[55.0, 45.0]), "{sz:?}");
        valid(&ws);

        // unknown team → error, nothing changes
        let before = ws.clone();
        assert!(ws.apply(WsCommand::RemoveTeam { tab: b.clone(), team: "a".into() }, &l).is_err());
        assert!(ws.apply(WsCommand::RemoveTeam { tab: "t99".into(), team: "b".into() }, &l).is_err());
        assert_eq!(ws, before);

        // the last team leaves no team area behind: the layout is back to the template's
        let fx = ws.apply(WsCommand::RemoveTeam { tab: b.clone(), team: "b".into() }, &l).unwrap();
        assert_eq!(fx.kill.len(), 2);
        assert!(find_slot(&ws.tabs[&b].layout, TEAM_SLOT).is_none());
        assert_eq!(ws.tabs[&b].layout, original);
        assert!(ws.panes.values().all(|p| p.team.is_none()));
        valid(&ws);
        // a new team with an old id starts numbering again
        ws.apply(add(&b, "a", Role::Lead), &l).unwrap();
        ws.apply(add(&b, "a", Role::Member), &l).unwrap();
        by_session(&ws, "billing-a-m1");
    }

    #[test]
    fn team_panes_cannot_break_off_and_lead_cannot_close() {
        let (l, mut ws) = role_ws();
        let b = tab(&ws, "billing").id.clone();
        ws.apply(add(&b, "a", Role::Lead), &l).unwrap();
        ws.apply(add(&b, "a", Role::Member), &l).unwrap();
        let (lead, m1) = (by_session(&ws, "billing-a-lead"), by_session(&ws, "billing-a-m1"));
        let before = ws.clone();
        for p in [&lead, &m1] {
            let err = ws.apply(WsCommand::BreakPaneToTab { pane: p.clone() }, &l).unwrap_err();
            assert!(err.contains("팀"), "{err}");
        }
        let err = ws.apply(WsCommand::ClosePane { pane: lead.clone() }, &l).unwrap_err();
        assert!(err.contains("dismiss_team"), "{err}");
        assert_eq!(ws, before);
        // closing a member = dismissing it
        let fx = ws.apply(WsCommand::ClosePane { pane: m1.clone() }, &l).unwrap();
        assert_eq!(fx.kill, vec![m1]);
        assert!(find_slot(&ws.tabs[&b].layout, "team:a").is_none(), "a lone lead needs no column split");
        valid(&ws);
        // closing the Manager tab takes its teams along
        let fx = ws.apply(WsCommand::CloseTab { tab: b }, &l).unwrap();
        assert!(fx.kill.contains(&lead));
        assert!(ws.panes.values().all(|p| p.team.is_none()));
    }

    #[test]
    fn validate_rejects_broken_team_invariants() {
        let (l, mut good) = role_ws();
        let b = tab(&good, "billing").id.clone();
        good.apply(add(&b, "a", Role::Lead), &l).unwrap();
        good.apply(add(&b, "a", Role::Member), &l).unwrap();
        valid(&good);
        type M = (&'static str, Box<dyn Fn(&mut Workspace)>);
        let cases: Vec<M> = vec![
            ("member without a lead", Box::new(|w| {
                let id = by_session(w, "billing-a-lead");
                w.panes.get_mut(&id).unwrap().role = Some(Role::Member);
            })),
            ("two leads", Box::new(|w| {
                let id = by_session(w, "billing-a-m1");
                w.panes.get_mut(&id).unwrap().role = Some(Role::Lead);
            })),
            ("member without a team", Box::new(|w| {
                let id = by_session(w, "billing-a-m1");
                w.panes.get_mut(&id).unwrap().team = None;
            })),
            ("team on a plain pane", Box::new(|w| {
                let id = pane(w, "billing", "shell");
                w.panes.get_mut(&id).unwrap().team = Some("a".into());
            })),
            ("bad team id", Box::new(|w| {
                for p in w.panes.values_mut().filter(|p| p.team.is_some()) {
                    p.team = Some("A/..".into());
                }
            })),
            ("team pane without a session name", Box::new(|w| {
                let id = by_session(w, "billing-a-m1");
                w.panes.get_mut(&id).unwrap().session_name = None;
            })),
            ("team in a non-Manager tab", Box::new(|w| {
                let id = tab(w, "billing").id.clone();
                w.tabs.get_mut(&id).unwrap().role = None;
                let m = pane(w, "billing", "manager");
                w.panes.get_mut(&m).unwrap().role = None;
                w.panes.get_mut(&m).unwrap().session_name = None;
            })),
        ];
        for (name, f) in cases {
            let mut bad = good.clone();
            f(&mut bad);
            assert!(bad.normalize_and_validate().is_err(), "{name}");
        }
    }

    #[test]
    fn role_tab_cannot_reuse_a_live_session_name() {
        let (l, mut ws) = role_ws();
        let b = tab(&ws, "billing").id.clone();
        ws.apply(add(&b, "batch", Role::Lead), &l).unwrap();
        // (a) R15: a renamed Manager tab keeps its session name "billing"
        ws.apply(WsCommand::RenameTab { tab: b.clone(), name: "ops".into() }, &l).unwrap();
        let mk = |name: &str, role| WsCommand::CreateTab {
            name: name.into(),
            dir: "x".into(),
            template: Some("manager".into()),
            group: None,
            role,
        };
        let before = ws.clone();
        // (a) the old name, (b) a team pane's session name
        for name in ["billing", "billing-batch-lead"] {
            let err = ws.apply(mk(name, Some(Role::Manager)), &l).unwrap_err();
            assert!(err.contains("세션 이름"), "{name}: {err}");
            assert_eq!(ws, before);
        }
        // a plain tab has no session, so the name is free for it
        ws.apply(mk("billing", None), &l).unwrap();
        valid(&ws);
        // hiring checks every session name in the workspace, not only the tab's
        let mut ws = before.clone();
        let hp = ws.panes.values().find(|p| p.role == Some(Role::Handover)).unwrap().id.clone();
        ws.panes.get_mut(&hp).unwrap().session_name = Some("ops-z-lead".into());
        let err = ws.apply(add(&b, "z", Role::Lead), &l).unwrap_err();
        assert!(err.contains("세션 이름"), "{err}");
    }

    #[test]
    fn tab_names_reject_control_characters() {
        let (l, mut ws) = role_ws();
        let before = ws.clone();
        for name in ["a\nb", "tab\u{7}", "x\ty", "\u{1b}[31m"] {
            let cmd = WsCommand::CreateTab { name: name.into(), dir: "x".into(), template: None, group: None, role: Some(Role::Manager) };
            assert!(ws.apply(cmd, &l).is_err(), "{name:?}");
            let b = tab(&ws, "billing").id.clone();
            assert!(ws.apply(WsCommand::RenameTab { tab: b, name: name.into() }, &l).is_err(), "{name:?}");
        }
        assert_eq!(ws, before);
    }

    #[test]
    fn create_tab_role_and_background_group() {
        let (l, mut ws) = role_ws();
        let mk = |name: &str, dir: &str, role| WsCommand::CreateTab {
            name: name.into(),
            dir: dir.into(),
            template: None,
            group: None,
            role,
        };
        let before = ws.clone();
        for (cmd, why) in [
            (mk("x", "x", Some(Role::Dispatcher)), "dispatcher"),
            (mk("x", "x", Some(Role::Lead)), "lead"),
            (mk("x", "x", Some(Role::Member)), "member"),
            (mk("h2", "", Some(Role::Handover)), "second handover"),
        ] {
            assert!(ws.apply(cmd, &l).is_err(), "{why}");
            assert_eq!(ws, before);
        }
        ws.apply(mk("m", "m", Some(Role::Manager)), &l).unwrap();
        let m = tab(&ws, "m");
        assert_eq!(m.role, Some(Role::Manager));
        let standing: Vec<_> = m.layout.leaves().into_iter().filter(|p| ws.panes[p].role == Some(Role::Manager)).collect();
        assert_eq!(standing.len(), 1);
        assert_eq!(ws.panes[&standing[0]].session_name.as_deref(), Some("m"));
        valid(&ws);
        // an explicit group — even the focused one — gets the tab in the background
        let g = ws.focus.group_id.clone();
        let loc = ws.focus_location();
        ws.apply(WsCommand::CreateTab { name: "bg".into(), dir: "bg".into(), template: None, group: Some(g.clone()), role: None }, &l)
            .unwrap();
        assert_eq!(ws.focus_location(), loc);
        assert_eq!(ws.groups[&g].tab_ids.last(), Some(&tab(&ws, "bg").id));
    }

    // ── phase 2: document tabs (Task 14) ──

    /// `loaded_groups()` with a real workspaceRoot holding a few documents.
    fn doc_ws() -> (tempfile::TempDir, Loaded, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let mut l = loaded_groups();
        l.settings.workspace_root = tmp.path().to_path_buf();
        for f in ["HANDOVER.md", "billing/issues/x/output/보고서.md", "api/HANDOVER.md", "notes.txt", "dispatcher.md"] {
            let p = tmp.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "# x\n").unwrap();
        }
        let ws = Workspace::from_config(&l);
        (tmp, l, ws)
    }
    fn open(path: &str) -> WsCommand {
        WsCommand::OpenDoc { path: path.into(), group: None }
    }
    fn doc_panes(ws: &Workspace) -> Vec<&Pane> {
        ws.panes.values().filter(|p| p.kind == PaneKind::Applet { kind: DOC_APPLET.into() }).collect()
    }
    fn has_hangul(s: &str) -> bool {
        s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))
    }

    #[test]
    fn open_doc_rejects_outside_root_and_non_md() {
        let (tmp, l, mut ws) = doc_ws();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "s").unwrap();
        let before = ws.clone();
        let secret = outside.path().join("secret.md").to_string_lossy().into_owned();
        for bad in [secret.as_str(), "../secret.md", "notes.txt", "billing", "missing.md", ""] {
            let e = ws.apply(open(bad), &l).unwrap_err();
            assert!(has_hangul(&e), "Korean error for {bad:?}: {e}");
            assert_eq!(ws, before, "{bad:?}: model untouched");
        }
        // a junction under the root pointing outside is refused after resolution
        if crate::issues::tests::link_dir(&tmp.path().join("lnk"), outside.path()) {
            assert!(ws.apply(open("lnk/secret.md"), &l).unwrap_err().contains("밖"));
            assert_eq!(ws, before);
        }
        // an unknown group is an error too
        let cmd = WsCommand::OpenDoc { path: "HANDOVER.md".into(), group: Some("g99".into()) };
        assert!(ws.apply(cmd, &l).is_err());
        assert_eq!(ws, before);
    }

    #[test]
    fn open_doc_reuses_existing_tab() {
        let (tmp, l, mut ws) = doc_ws();
        let work = gid_named(&ws, "work");
        let control = gid_named(&ws, "control");
        // focus the work group, then open: new tab there, named after the file, focused
        let api = tab(&ws, "api").id.clone();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: Some(api.clone()), pane: None }, &l).unwrap();
        let fx = ws.apply(open("billing/issues/x/output/보고서.md"), &l).unwrap();
        let t = tab(&ws, "보고서.md").clone();
        assert_eq!(group_of(&ws, &t.id), work);
        assert_eq!(ws.focus_location(), Some(Location { group_id: work.clone(), tab_id: t.id.clone() }));
        assert_eq!(ws.last_location, Some(Location { group_id: work.clone(), tab_id: api.clone() }));
        let pid = t.layout.leaves()[0].clone();
        let p = &ws.panes[&pid];
        assert_eq!(p.kind, PaneKind::Applet { kind: DOC_APPLET.into() });
        assert_eq!(p.name, "보고서.md");
        assert_eq!(p.agent, "");
        assert_eq!((p.role, p.team.clone(), p.session_name.clone()), (None, None, None));
        assert_eq!(p.doc_path.as_deref(), Some(tmp.path().join("billing/issues/x/output/보고서.md").as_path()));
        assert_eq!(fx.spawn, vec![(pid.clone(), false)], "the launcher marks the applet Running");
        assert!(fx.kill.is_empty());
        assert!(!t.is_dispatcher && t.role.is_none());
        assert_eq!(t.workspace, tmp.path());
        valid(&ws);

        // the same file under another spelling, from another group: focus moves to the tab
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: None, pane: None }, &l).unwrap();
        let n = ws.tabs.len();
        let abs = tmp.path().join("billing\\issues\\x\\..\\x\\output\\보고서.md");
        let fx = ws.apply(open(&abs.to_string_lossy()), &l).unwrap();
        assert_eq!(ws.tabs.len(), n, "no second tab");
        assert!(fx.spawn.is_empty());
        assert_eq!(ws.focus_location(), Some(Location { group_id: work.clone(), tab_id: t.id.clone() }));
        assert_eq!(ws.tabs[&t.id].last_focused_pane_id, pid);

        // same file name elsewhere → phase-1 dedupe; reserved name too
        ws.apply(open("HANDOVER.md"), &l).unwrap();
        ws.apply(open("api/HANDOVER.md"), &l).unwrap();
        assert!(has_tab(&ws, "HANDOVER.md") && has_tab(&ws, "HANDOVER.md-2"));
        ws.apply(open("dispatcher.md"), &l).unwrap();
        assert!(has_tab(&ws, "dispatcher.md"));
        assert_eq!(doc_panes(&ws).len(), 4);

        // an explicit group gets the tab and the focus (opening a document is a user action)
        let fresh = tmp.path().join("new.md");
        std::fs::write(&fresh, "x").unwrap();
        ws.apply(WsCommand::OpenDoc { path: "new.md".into(), group: Some(control.clone()) }, &l).unwrap();
        let nt = tab(&ws, "new.md").id.clone();
        assert_eq!(group_of(&ws, &nt), control);
        assert_eq!(ws.focus_location(), Some(Location { group_id: control.clone(), tab_id: nt }));
        valid(&ws);

        // a maximize hiding the doc pane is cleared on reuse
        ws.apply(WsCommand::SetGroupMaximized { group: Some(control.clone()) }, &l).unwrap();
        ws.apply(open("billing/issues/x/output/보고서.md"), &l).unwrap();
        assert_eq!(ws.group_maximized, None);
        assert_eq!(ws.focus.group_id, work);

        // closing the tab closes the pane; reopening makes a new tab
        ws.apply(WsCommand::CloseTab { tab: t.id.clone() }, &l).unwrap();
        assert!(!ws.panes.contains_key(&pid));
        ws.apply(open("billing/issues/x/output/보고서.md"), &l).unwrap();
        assert!(has_tab(&ws, "보고서.md"));
        // wire shape
        let v = serde_json::to_value(open("a.md")).unwrap();
        assert_eq!(v, serde_json::json!({"type":"openDoc","path":"a.md","group":null}));
    }

    #[test]
    fn doc_tab_persists_and_restores() {
        let (tmp, l, mut ws) = doc_ws();
        ws.apply(open("billing/issues/x/output/보고서.md"), &l).unwrap();
        let pid = doc_panes(&ws)[0].id.clone();
        let file = tmp.path().join("ws.json");
        crate::persist::save_atomic(&file, &ws).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["panes"][&pid]["kind"], serde_json::json!({"type":"applet","kind":"doc"}));
        assert!(v["panes"][&pid]["docPath"].as_str().unwrap().ends_with("보고서.md"));
        // the document vanishing does not break the restore (the tab shows "not found")
        std::fs::remove_file(tmp.path().join("billing/issues/x/output/보고서.md")).unwrap();
        let mut back = crate::persist::load(&file).unwrap().unwrap();
        let fx = crate::persist::prepare_restored(&mut back, &l);
        assert_eq!(back.clone().normalize_and_validate(), Ok(()));
        assert!(fx.spawn.contains(&(pid.clone(), false)));
        assert_eq!(back.panes[&pid].doc_path, ws.panes[&pid].doc_path);
        assert_eq!(back.panes[&pid].kind, ws.panes[&pid].kind);

        // invariants: a doc pane needs its path, nothing else may carry one
        let mut broken = back.clone();
        broken.panes.get_mut(&pid).unwrap().doc_path = None;
        assert!(broken.normalize_and_validate().unwrap_err().contains(&pid));
        let mut broken = back.clone();
        let other = broken.panes.keys().find(|k| **k != pid).unwrap().clone();
        broken.panes.get_mut(&other).unwrap().doc_path = Some(tmp.path().join("HANDOVER.md"));
        assert!(broken.normalize_and_validate().is_err());
        let mut broken = back.clone();
        broken.panes.get_mut(&pid).unwrap().agent = "claude".into();
        assert!(broken.normalize_and_validate().is_err());
        // a pane without the field (phase 1 / before Task 14) loads with None
        let mut raw = v.clone();
        let some_terminal = raw["panes"].as_object().unwrap().keys().find(|k| **k != pid).unwrap().clone();
        raw["panes"][&some_terminal].as_object_mut().unwrap().remove("docPath");
        let mut w: Workspace = serde_json::from_value(raw).unwrap();
        assert_eq!(w.panes[&some_terminal].doc_path, None);
        w.normalize_and_validate().unwrap();
    }

    // ---- ③ D1a: the dashboard tab ("상태"), one at most ----

    fn dashboard_panes(ws: &Workspace) -> Vec<&Pane> {
        ws.panes.values().filter(|p| p.kind == PaneKind::Applet { kind: DASHBOARD_APPLET.into() }).collect()
    }

    #[test]
    fn open_dashboard_opens_one_status_tab_and_reuses_it() {
        let (_tmp, l, mut ws) = doc_ws();
        let work = gid_named(&ws, "work");
        let control = gid_named(&ws, "control");
        let api = tab(&ws, "api").id.clone();
        ws.apply(WsCommand::SetFocus { group: work.clone(), tab: Some(api.clone()), pane: None }, &l).unwrap();
        let fx = ws.apply(WsCommand::OpenDashboard { group: None }, &l).unwrap();
        let t = tab(&ws, "상태").clone();
        assert_eq!(ws.groups[&work].tab_ids.last(), Some(&t.id), "at the end of the focused group");
        assert_eq!(ws.focus_location(), Some(Location { group_id: work.clone(), tab_id: t.id.clone() }), "a user action: focused");
        assert_eq!(ws.last_location, Some(Location { group_id: work.clone(), tab_id: api.clone() }));
        let p = ws.panes[&t.layout.leaves()[0]].clone();
        assert_eq!(p.kind, PaneKind::Applet { kind: DASHBOARD_APPLET.into() });
        assert_eq!((p.role, p.team.clone(), p.session_name.clone(), p.doc_path.clone()), (None, None, None, None));
        assert_eq!(fx.spawn, vec![(p.id.clone(), false)], "the launcher marks the applet Running");
        assert!(fx.kill.is_empty());
        // again, from another group: the same tab takes the focus, nothing is added
        let d = tab(&ws, "dispatcher").id.clone();
        ws.apply(WsCommand::SetFocus { group: control.clone(), tab: Some(d), pane: None }, &l).unwrap();
        let n = ws.tabs.len();
        let fx = ws.apply(WsCommand::OpenDashboard { group: None }, &l).unwrap();
        assert!(fx.spawn.is_empty() && fx.kill.is_empty());
        assert_eq!((ws.tabs.len(), dashboard_panes(&ws).len()), (n, 1));
        assert_eq!(ws.focus_location(), Some(Location { group_id: work.clone(), tab_id: t.id.clone() }));
        assert_eq!(ws.clone().normalize_and_validate(), Ok(()));
    }

    #[test]
    fn open_dashboard_names_around_a_plain_status_tab_and_takes_a_group() {
        let (_tmp, l, mut ws) = doc_ws();
        let control = gid_named(&ws, "control");
        ws.apply(WsCommand::CreateTab { name: "상태".into(), dir: "api".into(), template: None, group: None, role: None }, &l).unwrap();
        ws.apply(WsCommand::OpenDashboard { group: Some(control.clone()) }, &l).unwrap();
        let t = tab(&ws, "상태-2").clone();
        assert_eq!(ws.groups[&control].tab_ids.last(), Some(&t.id), "in the given group");
        assert_eq!(dashboard_panes(&ws).len(), 1);
        assert!(ws.apply(WsCommand::OpenDashboard { group: Some("g99".into()) }, &l).is_err(), "unknown group");
    }

    #[test]
    fn dashboard_tab_persists_and_restores() {
        let (tmp, l, mut ws) = doc_ws();
        ws.apply(WsCommand::OpenDashboard { group: None }, &l).unwrap();
        let pid = dashboard_panes(&ws)[0].id.clone();
        let file = tmp.path().join("ws.json");
        crate::persist::save_atomic(&file, &ws).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["panes"][&pid]["kind"], serde_json::json!({"type": "applet", "kind": "dashboard"}));
        let mut back = crate::persist::load(&file).unwrap().unwrap();
        let fx = crate::persist::prepare_restored(&mut back, &l);
        assert_eq!(back.clone().normalize_and_validate(), Ok(()));
        assert!(fx.spawn.contains(&(pid.clone(), false)));
        // reopening after the restore goes to the restored tab
        let n = back.tabs.len();
        back.apply(WsCommand::OpenDashboard { group: None }, &l).unwrap();
        assert_eq!((back.tabs.len(), dashboard_panes(&back).len()), (n, 1));
        assert_eq!(back.focus_location().map(|f| f.tab_id), Some(back.panes[&pid].tab.clone()));
    }

    #[test]
    fn open_dashboard_wire_shape() {
        let c: WsCommand = serde_json::from_str(r#"{"type":"openDashboard"}"#).unwrap();
        assert_eq!(c, WsCommand::OpenDashboard { group: None });
        let c: WsCommand = serde_json::from_str(r#"{"type":"openDashboard","group":"g2"}"#).unwrap();
        assert_eq!(c, WsCommand::OpenDashboard { group: Some("g2".into()) });
    }
}
