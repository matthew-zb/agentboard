//! Workspace model — the single source of truth for groups, tabs and panes (spec §5).
//!
//! Every change goes through [`WsCommand`] via [`Workspace::apply`]. Operations are pure: they
//! never touch processes and instead return [`Effects`] (panes to spawn/kill) for the launcher.

use crate::config::{check_relative, GroupSpec, Loaded, PaneSpec, Template};
use crate::layout::{Dir, LayoutNode, Side};
use crate::profiles::resolve_cwd;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub type GroupId = String;
pub type TabId = String;
pub type PaneId = String;

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
    CreateTab { name: String, dir: String, template: Option<String>, group: Option<GroupId> },
    CloseTab { tab: TabId },
    RenameTab { tab: TabId, name: String },
    // panes
    SplitPane { pane: PaneId, dir: Dir },
    ClosePane { pane: PaneId },
    RestartPane { pane: PaneId },
    ResizePane { pane: PaneId, side: Side, delta: f32 },
    /// `tab: None` targets the group layout.
    SetSizes { tab: Option<TabId>, path: Vec<usize>, sizes: Vec<f32> },
    BreakPaneToTab { pane: PaneId },
    // state
    SetFocus { group: GroupId, tab: Option<TabId>, pane: Option<PaneId> },
    SetMaximized { tab: TabId, pane: Option<PaneId> },
    SetSyncInput { tab: TabId, on: bool },
    SetAttention { pane: PaneId, on: bool },
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
        let mut made: Vec<(String, TabId)> = vec![("dispatcher".into(), ws.build_tab("dispatcher", "", dtpl, true, loaded))];
        for t in &cfg.tabs {
            let tn = t.template.as_deref().unwrap_or(&cfg.default_template);
            let Some(tpl) = loaded.templates.get(tn) else { continue };
            made.push((t.name.clone(), ws.build_tab(&t.name, &t.dir, tpl, false, loaded)));
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
        let fx = next.apply_inner(cmd, loaded)?;
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

    /// Runtime state of a restored workspace: every pane `Starting` with attention and prompt
    /// warning cleared. Returns the spawn effects for all panes, without prompt injection.
    pub fn reset_runtime_state(&mut self) -> Effects {
        for p in self.panes.values_mut() {
            p.status = PaneStatus::Starting;
            p.attention = false;
            p.prompt_warning = false;
        }
        Effects { spawn: self.panes.keys().map(|id| (id.clone(), false)).collect(), kill: Vec::new() }
    }

    /// Panes whose process is alive (`starting` or `running`).
    pub fn running_count(&self) -> usize {
        self.panes.values().filter(|p| matches!(p.status, PaneStatus::Starting | PaneStatus::Running)).count()
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
            CreateTab { name, dir, template, group } => {
                self.check_tab_name(&name, None)?;
                if let Some(m) = check_relative(&dir) {
                    return Err(format!("디렉토리 '{dir}': {m}"));
                }
                let gid = match group {
                    Some(g) => {
                        self.group(&g)?;
                        g
                    }
                    None => self.focus.group_id.clone(),
                };
                let tn = template.unwrap_or_else(|| loaded.config.default_template.clone());
                let tpl = loaded.templates.get(&tn).ok_or_else(|| format!("템플릿 '{tn}'을(를) 찾을 수 없습니다"))?;
                let tid = self.build_tab(&name, &dir, tpl, false, loaded);
                fx.spawn = self.tabs[&tid].layout.leaves().into_iter().map(|p| (p, true)).collect();
                self.groups.get_mut(&gid).unwrap().tab_ids.push(tid.clone());
                self.activate_tab(&gid, &tid);
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
                let id = self.new_pane_id();
                self.panes.insert(
                    id.clone(),
                    Pane {
                        id: id.clone(),
                        name,
                        tab: src.tab.clone(),
                        agent: src.agent.clone(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd,
                        prompt: None,
                        status: PaneStatus::Starting,
                        attention: false,
                        prompt_warning: false,
                    },
                );
                let t = self.tabs.get_mut(&src.tab).unwrap();
                t.layout.split_leaf(&pane, id.clone(), dir, false);
                t.last_focused_pane_id = id.clone();
                t.maximized_pane_id = None;
                fx.spawn.push((id, false));
            }
            ClosePane { pane } => {
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
                p.attention = false;
                p.prompt_warning = false;
                fx.kill.push(pane.clone());
                fx.spawn.push((pane, false));
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
            SetAttention { pane, on } => {
                self.pane(&pane)?;
                self.panes.get_mut(&pane).unwrap().attention = on;
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

    /// Tab names are unique across the workspace; `dispatcher` is reserved.
    fn check_tab_name(&self, name: &str, except: Option<&str>) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("탭 이름이 비어 있습니다".into());
        }
        if name == "dispatcher" {
            return Err("'dispatcher'는 예약된 탭 이름입니다".into());
        }
        if self.tabs.values().any(|t| t.name == name && Some(t.id.as_str()) != except) {
            return Err(format!("탭 이름 '{name}'이(가) 이미 있습니다"));
        }
        Ok(())
    }

    /// `base`, or `base-2`, `base-3`, … whichever is free.
    fn unique_tab_name(&self, base: &str) -> String {
        let taken = |n: &str| n == "dispatcher" || self.tabs.values().any(|t| t.name == n);
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base}-{n}")).find(|n| !taken(n)).unwrap()
    }

    /// Create a tab (not yet placed in a group) and its panes from a template.
    fn build_tab(&mut self, name: &str, dir: &str, tpl: &Template, is_dispatcher: bool, loaded: &Loaded) -> TabId {
        let cfg = &loaded.config;
        let root = &loaded.settings.workspace_root;
        let workspace = if is_dispatcher || dir.is_empty() { root.clone() } else { root.join(dir) };
        let default_agent = if is_dispatcher {
            cfg.dispatcher.agent.clone().unwrap_or_else(|| cfg.default_agent.clone())
        } else {
            cfg.default_agent.clone()
        };
        let id = self.new_tab_id();
        let specs: Vec<PaneSpec> = tpl.layout.leaves();
        let ids: Vec<PaneId> = specs
            .iter()
            .map(|s| {
                let pid = self.new_pane_id();
                self.panes.insert(
                    pid.clone(),
                    Pane {
                        id: pid.clone(),
                        name: s.name.clone(),
                        tab: id.clone(),
                        agent: s.agent.clone().unwrap_or_else(|| default_agent.clone()),
                        args: s.args.clone(),
                        env: s.env.clone(),
                        cwd: resolve_cwd(&workspace, s.cwd.as_deref()),
                        prompt: s.prompt.clone(),
                        status: PaneStatus::Starting,
                        attention: false,
                        prompt_warning: false,
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
            p.attention = false;
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
        WsCommand::CreateTab { name: name.into(), dir: name.into(), template: Some(template.into()), group: None }
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
        ws.apply(WsCommand::CreateTab { name: "s".into(), dir: "".into(), template: None, group: None }, &l).unwrap();
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
            WsCommand::CreateTab { name: "y".into(), dir: "../out".into(), template: None, group: None },
            WsCommand::CreateTab { name: "z".into(), dir: "z".into(), template: None, group: Some("g99".into()) },
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
        for bad in ["dispatcher", "  "] {
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
    fn set_focus_records_last_location_and_clears_attention() {
        let l = loaded_fixture();
        let mut ws = Workspace::from_config(&l);
        let g = ws.focus.group_id.clone();
        let d = tab(&ws, "dispatcher").id.clone();
        let api = tab(&ws, "api").id.clone();
        let coder = pane(&ws, "api", "coder");
        ws.apply(WsCommand::SetAttention { pane: coder.clone(), on: true }, &l).unwrap();
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
        ws.apply(WsCommand::SetAttention { pane: dp.clone(), on: true }, &l).unwrap();
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
        ws.apply(WsCommand::CreateTab { name: "w2".into(), dir: "w2".into(), template: None, group: Some(work.clone()) }, &l)
            .unwrap();
        assert_eq!(ws.groups[&work].active_tab_id, tab(&ws, "w2").id);
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
        ws.apply(WsCommand::SetAttention { pane: p.clone(), on: true }, &l).unwrap();
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
        WsCommand::CreateTab { name: name.into(), dir: name.into(), template: Some("solo".into()), group: Some(group.into()) }
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
}
