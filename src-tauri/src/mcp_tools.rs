//! MCP tools of the control server (spec §4): definitions, the per-role permission table and
//! dispatch. `control.rs` authenticates the caller and turns `call`'s `Err` into an MCP
//! `isError: true` result whose text is the (Korean) message.
//!
//! Adding a tool (Tasks 6·8·9·11·12): give it a `ToolDef` in [`implemented`] (its roles come
//! from [`PERMISSIONS`], so a tool missing there is never listed) and an arm in [`call`]. A
//! tool body may take `ws` / `loaded` briefly and must go through `launcher::execute` for any
//! model change (R13); it runs on a blocking thread, never inside the async runtime.

use crate::config::{self, ConfigEdit, TabSpec};
use crate::control::Caller;
use crate::inbox::{self, ItemState, NewItem};
use crate::issues;
use crate::launcher;
use crate::roles::Role;
use crate::workspace::{valid_team_id, PaneId, WsCommand, TEAM_ID_MAX};
use crate::AppState;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use Role::{Dispatcher as D, Handover as H, Lead as L, Manager as M, Member as Mb};

/// Spec §4 permission table (+ `create_manager_tab`): tool → roles allowed to call it.
/// Per-item ownership ("담당 Manager", "자기 팀원") is checked inside the tool.
pub const PERMISSIONS: &[(&str, &[Role])] = &[
    ("inbox_post", &[M]),
    ("inbox_note", &[M]),
    ("inbox_propose_close", &[M]),
    ("inbox_list", &[D, H, M, L, Mb]),
    ("issue_create", &[M]),
    ("hire_lead", &[M]),
    ("hire_member", &[L]),
    ("dismiss", &[M, L]),
    ("dismiss_team", &[M, L]),
    ("list_team", &[M, L]),
    ("handoff_done", &[M, D, H]),
    ("create_manager_tab", &[D]),
];

/// Whether `role` may call `tool` (per [`PERMISSIONS`]; unknown tools: no).
pub fn allowed(tool: &str, role: Role) -> bool {
    PERMISSIONS.iter().any(|(n, roles)| *n == tool && roles.contains(&role))
}

/// One MCP tool as listed by `tools/list`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of `arguments` (`{"type":"object",…}`).
    pub input_schema: Value,
}

impl ToolDef {
    /// MCP wire shape: `{"name","description","inputSchema"}`.
    pub fn to_json(&self) -> Value {
        json!({ "name": self.name, "description": self.description, "inputSchema": self.input_schema })
    }
}

/// Every tool that has an implementation (whatever the caller's role).
fn implemented() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "inbox_post",
            description: "검토 결과를 사용자 인박스에 새 항목으로 올립니다(담당 = 호출한 Manager의 탭). 반환: 항목 id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "항목 제목" },
                    "check": { "type": "array", "items": { "type": "string" }, "description": "사용자가 확인할 것" },
                    "risk": { "type": "array", "items": { "type": "string" }, "description": "리스크 포인트" },
                    "source": { "type": "string", "description": "출처(슬랙 채널·메일 제목, 시각)" },
                    "refs": { "type": "array", "items": { "type": "string" }, "description": "참고 경로·링크(선택)" }
                },
                "required": ["title", "check", "risk", "source"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "inbox_note",
            description: "자기 담당 인박스 항목의 타임라인에 진행 메모를 남깁니다.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "item": { "type": "string", "description": "항목 id" },
                    "text": { "type": "string", "description": "메모 내용" }
                },
                "required": ["item", "text"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "inbox_propose_close",
            description: "자기 담당 항목을 최종 의견과 함께 '종료 제안됨'으로 바꿉니다. 완료 확인은 사용자만 합니다.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "item": { "type": "string", "description": "항목 id" },
                    "final_opinion": { "type": "string", "description": "최종 의견" }
                },
                "required": ["item", "final_opinion"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "inbox_list",
            description: "인박스 항목을 조회합니다(Manager는 자기 담당, dispatcher는 전체). state를 주지 않으면 완료되지 않은 항목만 돌려줍니다.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": { "type": "string", "enum": ["new", "in_progress", "close_proposed", "done"], "description": "이 상태의 항목만" }
                },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "issue_create",
            description: "자기 작업 폴더에 이슈 폴더(issues/<YYYYMMDD-제목>/{input,output})를 만들고, 항목을 주면 연결합니다. 반환: 이슈 id와 경로.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "item": { "type": "string", "description": "연결할 인박스 항목 id(선택, 자기 담당만)" },
                    "title": { "type": "string", "description": "이슈 제목(폴더 이름에 쓰임)" }
                },
                "required": ["title"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "hire_lead",
            description: "자기 탭에 팀을 만들고 팀 리드 창을 띄웁니다(팀 영역에 분할, 포커스는 바뀌지 않음). 먼저 팀 업무 문서(teams/<팀id>/brief.md)를 쓰고 그 경로를 넘기세요. 팀 수 제한은 없습니다. 이미 리드가 있는 팀이면 그 리드를 돌려줍니다.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "team": { "type": "string", "description": "팀 id: 영문 소문자·숫자·'-' 1~24자 (예: batch)" },
                    "brief_path": { "type": "string", "description": "팀 업무 문서 경로(이 탭 작업 폴더 기준 상대경로 또는 그 안의 절대경로)" },
                    "extra_prompt": { "type": "string", "description": "리드에게 줄 추가 지시(선택)" }
                },
                "required": ["team", "brief_path"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "hire_member",
            description: "자기 팀 영역에 팀원 창을 하나 추가합니다(팀당 상한 roles.member.maxPerTeam, 기본 4). 반환: 팀원의 세션 이름.",
            input_schema: json!({
                "type": "object",
                "properties": { "extra_prompt": { "type": "string", "description": "팀원에게 줄 추가 지시(선택)" } },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "dismiss",
            description: "팀원 창 하나를 종료합니다. Manager는 자기 탭의 팀원, 팀 리드는 자기 팀원만. 리드는 dismiss_team으로 팀째 해산합니다.",
            input_schema: json!({
                "type": "object",
                "properties": { "agent": { "type": "string", "description": "팀원의 세션 이름(예: billing-batch-m1) 또는 창 id" } },
                "required": ["agent"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "dismiss_team",
            description: "Manager: 팀의 모든 창을 종료하고 팀 영역을 정리합니다. 팀 리드: 자기 팀원을 모두 종료합니다(리드는 남음).",
            input_schema: json!({
                "type": "object",
                "properties": { "team": { "type": "string", "description": "팀 id" } },
                "required": ["team"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "list_team",
            description: "이 탭에 있는 팀들(또는 지정한 한 팀)의 구성과 각 창의 상태를 돌려줍니다. 팀 리드는 자기 팀만 봅니다.",
            input_schema: json!({
                "type": "object",
                "properties": { "team": { "type": "string", "description": "팀 id. 생략하면 탭의 모든 팀" } },
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "handoff_done",
            description: "야간 정리에서 진행 내역 저장·보고를 마쳤다고 앱에 알립니다.",
            input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
        ToolDef {
            name: "create_manager_tab",
            description: "새 상시업무 탭과 Manager를 만들고 config.json의 tabs에 role manager로 추가합니다. 탭은 defaultGroup(없으면 포커스가 아닌 그룹)에 열리며 포커스·활성 탭은 바뀌지 않습니다. 폴더(dir)는 미리 있어야 합니다 — 없으면 만들지 사용자에게 먼저 확인하세요.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "탭 이름(= Manager 세션 이름), 중복·'dispatcher' 불가" },
                    "dir": { "type": "string", "description": "작업 폴더, workspaceRoot 기준 상대경로" },
                    "template": { "type": "string", "description": "템플릿 이름(선택, 기본 manager — 없으면 defaultTemplate)" }
                },
                "required": ["name", "dir"],
                "additionalProperties": false
            }),
        },
    ]
}

/// Tools `role` may use that are implemented, in definition order.
pub fn list(role: Role) -> Vec<ToolDef> {
    implemented().into_iter().filter(|t| allowed(t.name, role)).collect()
}

/// Run tool `name` for `caller`. `Ok` value: a string is sent as is, anything else as JSON text.
pub fn call(state: &AppState, caller: &Caller, name: &str, args: Value) -> Result<Value, String> {
    if !implemented().iter().any(|t| t.name == name) {
        return Err(format!("알 수 없는 도구입니다: {name}"));
    }
    if !allowed(name, caller.role) {
        return Err(format!("'{}' 역할은 {name} 도구를 쓸 수 없습니다", caller.role.as_str()));
    }
    let args = match args {
        Value::Null => json!({}),
        Value::Object(_) => args,
        _ => return Err("arguments는 객체여야 합니다".into()),
    };
    match name {
        "inbox_post" => inbox_post(state, caller, &args),
        "inbox_note" => {
            let text = req_str(&args, "text")?;
            cap_bytes("text", &text)?;
            state.inbox.note(caller, &req_str(&args, "item")?, &text)?;
            Ok(json!("메모를 기록했습니다."))
        }
        "inbox_propose_close" => {
            let opinion = req_str(&args, "final_opinion")?;
            cap_bytes("final_opinion", &opinion)?;
            state.inbox.propose_close(caller, &req_str(&args, "item")?, &opinion)?;
            Ok(json!("종료를 제안했습니다. 사용자가 확인하면 완료됩니다."))
        }
        "inbox_list" => {
            let filter = match opt_str(&args, "state")? {
                None => None,
                Some(s) => Some(ItemState::parse(&s).ok_or_else(|| format!("알 수 없는 상태입니다: {s}"))?),
            };
            Ok(json!(state.inbox.list_for(caller, filter)))
        }
        "issue_create" => issue_create(state, caller, &args),
        "list_team" => list_team(state, caller, &args),
        "handoff_done" => handoff_done(state, caller),
        "hire_lead" => hire_lead(state, caller, &args),
        "hire_member" => hire_member(state, caller, &args),
        "dismiss" => dismiss(state, caller, &args),
        "dismiss_team" => dismiss_team(state, caller, &args),
        "create_manager_tab" => create_manager_tab(state, caller, &args),
        _ => Err(format!("알 수 없는 도구입니다: {name}")),
    }
}

/// Content caps of MCP inputs (characters, except `TEXT_MAX_BYTES`).
pub const TITLE_MAX: usize = 200;
pub const ENTRY_MAX: usize = 500;
pub const ENTRIES_MAX: usize = 20;
pub const TEXT_MAX_BYTES: usize = 8 * 1024;
/// Longest path / tab name argument (characters).
pub const PATH_MAX: usize = 1000;
pub const NAME_MAX: usize = 64;

fn cap_chars(key: &str, s: &str, max: usize) -> Result<(), String> {
    if s.chars().count() > max {
        return Err(format!("{key}이(가) 너무 깁니다(최대 {max}자)"));
    }
    Ok(())
}

fn cap_bytes(key: &str, s: &str) -> Result<(), String> {
    if s.len() > TEXT_MAX_BYTES {
        return Err(format!("{key}이(가) 너무 깁니다(최대 8KB)"));
    }
    Ok(())
}

/// Required array of strings: at most [`ENTRIES_MAX`] entries of at most [`ENTRY_MAX`] chars.
fn capped_list(args: &Value, key: &str, required: bool) -> Result<Vec<String>, String> {
    if required && matches!(args.get(key), None | Some(Value::Null)) {
        return Err(format!("{key}이(가) 필요합니다"));
    }
    let v = str_list(args, key)?;
    if v.len() > ENTRIES_MAX {
        return Err(format!("{key} 항목이 너무 많습니다(최대 {ENTRIES_MAX}개)"));
    }
    for e in &v {
        cap_chars(key, e, ENTRY_MAX)?;
    }
    Ok(v)
}

/// Owner tab's current workspace from `ws` (released before returning). A tab that no longer
/// exists is refused: stored issue paths are never trusted on their own.
pub fn tab_workspace(state: &AppState, tab: &str) -> Result<std::path::PathBuf, String> {
    let ws = state.ws.lock().unwrap();
    ws.tabs
        .get(tab)
        .map(|t| t.workspace.clone())
        .ok_or_else(|| "담당 탭이 없어 이슈 폴더를 확인할 수 없습니다".to_string())
}

fn opt_str(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key}은(는) 문자열이어야 합니다")),
    }
}

fn req_str(args: &Value, key: &str) -> Result<String, String> {
    opt_str(args, key)?.ok_or_else(|| format!("{key}이(가) 필요합니다"))
}

/// Optional array of strings (missing / null → empty).
fn str_list(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let bad = || format!("{key}은(는) 문자열 배열이어야 합니다");
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(bad)).collect(),
        Some(_) => Err(bad()),
    }
}

/// Name and workspace of the caller's tab; `ws` is released before the inbox is used.
fn caller_tab(state: &AppState, caller: &Caller) -> Result<(String, std::path::PathBuf), String> {
    let ws = state.ws.lock().unwrap();
    let tab = ws.tabs.get(&caller.tab).ok_or("호출한 창의 탭을 찾을 수 없습니다")?;
    Ok((tab.name.clone(), tab.workspace.clone()))
}

/// Returns the new item's id as plain text.
fn inbox_post(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let title = req_str(args, "title")?;
    cap_chars("title", &title, TITLE_MAX)?;
    let source = req_str(args, "source")?;
    cap_chars("source", &source, ENTRY_MAX)?;
    let new = NewItem {
        title,
        check: capped_list(args, "check", true)?,
        risk: capped_list(args, "risk", true)?,
        source,
        refs: capped_list(args, "refs", false)?,
    };
    let (tab_name, _) = caller_tab(state, caller)?;
    Ok(json!(state.inbox.post(caller, &tab_name, new)?))
}

/// `{"issue": id, "path", "input", "output"}`. With `item`: the caller must own it, and an
/// item that already has an issue gets that issue back (a retried call makes no new folder).
fn issue_create(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let title = req_str(args, "title")?;
    if title.trim().is_empty() {
        return Err("title이(가) 비어 있습니다".into());
    }
    cap_chars("title", &title, TITLE_MAX)?;
    let reply = |id: &str, path: &std::path::Path| {
        json!({ "issue": id, "path": path, "input": path.join("input"), "output": path.join("output") })
    };
    let item = opt_str(args, "item")?;
    if let Some(item_id) = &item {
        let it = state.inbox.get(item_id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {item_id}"))?;
        inbox::check_owner(caller, &it)?;
        if it.state == ItemState::Done {
            return Err(format!("완료된 항목입니다: {item_id}"));
        }
        if let Some(id) = &it.issue {
            // re-derived and validated from the tab's current workspace, never the stored path
            let path = issues::resolve(&tab_workspace(state, &caller.tab)?, id)?;
            return Ok(reply(id, &path));
        }
    }
    let (_, workspace) = caller_tab(state, caller)?;
    let (id, path) = issues::create(&workspace, &title, chrono::Local::now().date_naive())?;
    if let Some(item_id) = &item {
        state
            .inbox
            .link_issue(caller, item_id, &id, &path)
            .map_err(|e| format!("이슈 폴더({id})는 만들었지만 항목에 연결하지 못했습니다: {e}"))?;
    }
    Ok(reply(&id, &path))
}

/// `{"tab": <tab name>, "teams": [{"team", "panes": [{"pane","name","role","sessionName","status"}]}]}`;
/// teams sorted by id, the lead first, then members by session name. A lead gets its own team
/// only (asking for another one is an error).
fn list_team(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let mut filter = match args.get("team") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("team은 문자열이어야 합니다".into()),
    };
    if caller.role == Role::Lead {
        // a lead sees its own team only
        let own = caller.team.clone().ok_or("팀이 없는 리드입니다")?;
        if filter.as_ref().is_some_and(|f| *f != own) {
            return Err("팀 리드는 자기 팀만 볼 수 있습니다".into());
        }
        filter = Some(own);
    }
    let ws = state.ws.lock().unwrap();
    let tab = ws.tabs.get(&caller.tab).ok_or("호출한 창의 탭을 찾을 수 없습니다")?;
    let mut teams: BTreeMap<String, Vec<(u8, String, Value)>> = BTreeMap::new();
    for p in ws.panes.values().filter(|p| p.tab == caller.tab) {
        let Some(team) = &p.team else { continue };
        if filter.as_ref().map_or(false, |f| f != team) {
            continue;
        }
        let session = p.session_name.clone().unwrap_or_default();
        let rank = if p.role == Some(Role::Lead) { 0 } else { 1 };
        let entry = json!({
            "pane": p.id,
            "name": p.name,
            "role": p.role,
            "sessionName": p.session_name,
            "status": p.status,
        });
        teams.entry(team.clone()).or_default().push((rank, session, entry));
    }
    if let Some(f) = &filter {
        if !teams.contains_key(f) {
            return Err(format!("팀 '{f}'이(가) 이 탭에 없습니다"));
        }
    }
    let teams: Vec<Value> = teams
        .into_iter()
        .map(|(team, mut panes)| {
            panes.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            json!({ "team": team, "panes": panes.into_iter().map(|p| p.2).collect::<Vec<_>>() })
        })
        .collect();
    Ok(json!({ "tab": tab.name, "teams": teams }))
}

// ───────────────────────── teams (Task 8) ─────────────────────────

/// `{"team","pane","sessionName","existing"}` of the lead of `team` in `tab`, if any.
fn lead_of(state: &AppState, tab: &str, team: &str) -> Option<Value> {
    let ws = state.ws.lock().unwrap();
    let p = ws.team_panes(tab, team).into_iter().find(|p| p.role == Some(Role::Lead))?;
    Some(json!({ "team": team, "pane": p.id, "sessionName": p.session_name, "existing": true }))
}

/// One line (a prompt is typed into the PTY; a newline would submit early).
fn one_line(s: &str) -> String {
    s.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

fn with_extra(base: String, extra: Option<&str>) -> String {
    match extra.map(one_line).filter(|e| !e.is_empty()) {
        Some(e) => format!("{base} 추가 지시: {e}"),
        None => base,
    }
}

/// `brief_path` (relative to the tab workspace, or absolute) resolved and checked at use time:
/// it must be an existing file whose canonical path (junctions/symlinks resolved) lies inside
/// the canonical workspace. Returns `<workspace>/<path inside it>`.
pub fn resolve_brief(workspace: &Path, brief: &str) -> Result<PathBuf, String> {
    if brief.trim().is_empty() {
        return Err("brief_path가 비어 있습니다".into());
    }
    let given = Path::new(brief);
    let full = if given.is_absolute() { given.to_path_buf() } else { workspace.join(given) };
    let ws_real = std::fs::canonicalize(workspace).map_err(|_| "이 탭의 작업 폴더를 찾을 수 없습니다".to_string())?;
    let real = std::fs::canonicalize(&full).map_err(|_| format!("팀 업무 문서를 찾을 수 없습니다: {brief}"))?;
    let inside = real.strip_prefix(&ws_real).map_err(|_| format!("팀 업무 문서는 이 탭의 작업 폴더 안에 있어야 합니다: {brief}"))?;
    if !real.is_file() {
        return Err(format!("팀 업무 문서가 파일이 아닙니다: {brief}"));
    }
    Ok(workspace.join(inside))
}

/// `hire_lead(team, brief_path, extra_prompt?)` (Manager). Idempotent per (tab, team): a team
/// that already has a lead gets that lead back (`existing: true`), so a retry after a tool
/// timeout never makes a second one.
fn hire_lead(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let team = req_str(args, "team")?;
    if !valid_team_id(&team) {
        return Err(format!("팀 id '{team}'이(가) 올바르지 않습니다 — 영문 소문자·숫자·'-'로 1~{TEAM_ID_MAX}자"));
    }
    let brief = req_str(args, "brief_path")?;
    cap_chars("brief_path", &brief, PATH_MAX)?;
    let extra = opt_str(args, "extra_prompt")?;
    if let Some(e) = &extra {
        cap_bytes("extra_prompt", e)?;
    }
    if let Some(v) = lead_of(state, &caller.tab, &team) {
        return Ok(v);
    }
    let (tab_name, workspace) = caller_tab(state, caller)?;
    let brief_path = resolve_brief(&workspace, &brief)?;
    let sname = crate::roles::session_name(&tab_name, Role::Lead, Some(&team), 0);
    let prompt = with_extra(
        format!(
            "당신은 팀 '{team}'의 팀 리드입니다(세션 이름 {sname}). 팀 업무 문서 {} 를 먼저 읽고 그 기준으로 일하세요. \
             팀원은 hire_member로 채용해 일을 나누고, 결과물을 검증한 뒤 Manager 세션 '{}'에게 보고하세요.",
            brief_path.display(),
            caller.session_name
        ),
        extra.as_deref(),
    );
    let cmd = WsCommand::AddTeamPane { tab: caller.tab.clone(), team: team.clone(), role: Role::Lead, prompt: Some(prompt) };
    match launcher::execute(state, cmd) {
        Ok(_) => {
            let mut v = lead_of(state, &caller.tab, &team).ok_or("팀 리드 창을 만들지 못했습니다")?;
            v["existing"] = json!(false);
            v["briefPath"] = json!(brief_path);
            Ok(v)
        }
        // an identical concurrent call won the race
        Err(e) => lead_of(state, &caller.tab, &team).ok_or(e),
    }
}

/// `hire_member(extra_prompt?)` (Lead): a member of the caller's team.
/// `{"team","pane","sessionName"}`. Not idempotent (a retry hires another member, up to the cap).
fn hire_member(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let team = caller.team.clone().ok_or("팀이 없는 리드입니다")?;
    let extra = opt_str(args, "extra_prompt")?;
    if let Some(e) = &extra {
        cap_bytes("extra_prompt", e)?;
    }
    let before: BTreeSet<PaneId> = state.ws.lock().unwrap().panes.keys().cloned().collect();
    let prompt = with_extra(
        format!(
            "당신은 팀 '{team}'의 팀원입니다. 팀 리드 세션 '{}'의 지시를 따르고, 결과는 팀 리드에게만 보고하세요.",
            caller.session_name
        ),
        extra.as_deref(),
    );
    let cmd = WsCommand::AddTeamPane { tab: caller.tab.clone(), team: team.clone(), role: Role::Member, prompt: Some(prompt) };
    launcher::execute(state, cmd)?;
    let ws = state.ws.lock().unwrap();
    let p = ws
        .team_panes(&caller.tab, &team)
        .into_iter()
        .filter(|p| p.role == Some(Role::Member) && !before.contains(&p.id))
        .last()
        .ok_or("팀원 창을 만들지 못했습니다")?;
    Ok(json!({ "team": team, "pane": p.id, "sessionName": p.session_name }))
}

/// `dismiss(agent)`: one member (session name or pane id) of the caller's tab. A Manager may
/// dismiss any member there; a lead only its own team's. Leads go with their team
/// (`dismiss_team`), so members are never left without one.
fn dismiss(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let agent = req_str(args, "agent")?;
    let (pane, role, team) = {
        let ws = state.ws.lock().unwrap();
        let p = ws
            .panes
            .get(&agent)
            .or_else(|| ws.panes.values().find(|p| p.session_name.as_deref() == Some(agent.as_str())))
            .filter(|p| p.tab == caller.tab)
            .ok_or_else(|| format!("이 탭에서 '{agent}' 창을 찾을 수 없습니다(이미 해고되었을 수 있습니다)"))?;
        (p.id.clone(), p.role, p.team.clone())
    };
    match (caller.role, role) {
        (Role::Lead, Some(Role::Member)) if team == caller.team => {}
        (Role::Lead, _) => return Err("팀 리드는 자기 팀원만 해고할 수 있습니다".into()),
        (_, Some(Role::Member)) => {}
        (_, Some(Role::Lead)) => {
            return Err(format!("팀 리드는 따로 해고할 수 없습니다 — dismiss_team으로 팀 '{}'을(를) 해산하세요", team.unwrap_or_default()))
        }
        _ => return Err("팀 리드·팀원 창만 해고할 수 있습니다".into()),
    }
    launcher::execute(state, WsCommand::ClosePane { pane })?;
    Ok(json!(format!("'{agent}'을(를) 해고했습니다.")))
}

/// `dismiss_team(team)`: a Manager removes the whole team (all panes, its column, an emptied
/// team area); a lead lets all of its own members go and stays itself.
fn dismiss_team(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let team = req_str(args, "team")?;
    if caller.role == Role::Lead {
        if caller.team.as_deref() != Some(team.as_str()) {
            return Err("팀 리드는 자기 팀의 팀원만 정리할 수 있습니다".into());
        }
        let members: Vec<PaneId> = {
            let ws = state.ws.lock().unwrap();
            ws.team_panes(&caller.tab, &team).into_iter().filter(|p| p.role == Some(Role::Member)).map(|p| p.id.clone()).collect()
        };
        let mut n = 0;
        for pane in members {
            // a member closed meanwhile (user / another call) is simply skipped
            if launcher::execute(state, WsCommand::ClosePane { pane }).is_ok() {
                n += 1;
            }
        }
        return Ok(json!(format!("팀 '{team}'의 팀원 {n}명을 해고했습니다. 팀 리드는 남습니다.")));
    }
    launcher::execute(state, WsCommand::RemoveTeam { tab: caller.tab.clone(), team: team.clone() })?;
    Ok(json!(format!("팀 '{team}'을(를) 해산했습니다.")))
}

// ───────────────────────── create_manager_tab (Task 8) ─────────────────────────

/// `create_manager_tab(name, dir, template?)` (Dispatcher): a new standing-task tab with its
/// Manager, opened in the background (R12) in `defaultGroup` (else the first group that is not
/// focused, else the focused one), and appended to config.json `tabs` (all other keys kept).
/// The folder must already exist — creating it is the user's decision. Steps: validate on a
/// copy of the model → write config.json (under `loaded` write) → apply; a failed apply puts
/// config.json back. A retry for the same name + dir returns the existing tab.
fn create_manager_tab(state: &AppState, _caller: &Caller, args: &Value) -> Result<Value, String> {
    let name = req_str(args, "name")?;
    let dir = req_str(args, "dir")?;
    let template = opt_str(args, "template")?;
    cap_chars("name", &name, NAME_MAX)?;
    cap_chars("dir", &dir, PATH_MAX)?;
    if dir.trim().is_empty() {
        return Err("상시업무 탭에는 전용 작업 폴더(dir)가 필요합니다".into());
    }
    if let Some(m) = config::check_relative(&dir) {
        return Err(format!("디렉토리 '{dir}': {m}"));
    }
    let (root, tpl, spec_template, default_group) = {
        let l = state.loaded.read().unwrap();
        let (tpl, spec_template) = match &template {
            Some(t) => (t.clone(), Some(t.clone())),
            None if l.templates.contains_key("manager") => ("manager".to_string(), Some("manager".to_string())),
            None => (l.config.default_template.clone(), None), // R2
        };
        (l.settings.workspace_root.clone(), tpl, spec_template, l.config.default_group.clone())
    };
    let reply = |id: &str, group: &str, existing: bool| {
        json!({ "tab": id, "name": name, "dir": dir, "template": tpl, "group": group, "existing": existing })
    };
    // a retried call (tool timeout) finds its tab already there
    {
        let ws = state.ws.lock().unwrap();
        if let Some(t) = ws.tabs.values().find(|t| t.name == name) {
            if t.role == Some(Role::Manager) && t.dir == dir {
                let g = ws.group_of_tab(&t.id).map(|g| ws.groups[g].name.clone()).unwrap_or_default();
                return Ok(reply(&t.id, &g, true));
            }
        }
    }
    let full = root.join(&dir);
    if !full.is_dir() {
        return Err(format!(
            "폴더 '{dir}'이(가) 없습니다 — 폴더를 만들지 사용자에게 먼저 확인하고, 사용자가 원하면 만든 뒤 다시 호출하세요"
        ));
    }
    let inside = match (std::fs::canonicalize(&root), std::fs::canonicalize(&full)) {
        (Ok(r), Ok(f)) => f.starts_with(&r) && f != r,
        _ => false,
    };
    if !inside {
        return Err(format!("폴더 '{dir}'이(가) workspaceRoot 밖을 가리킵니다"));
    }
    let (group, group_name) = {
        let ws = state.ws.lock().unwrap();
        let leaves = ws.group_layout.leaves();
        let by_name = default_group.and_then(|n| leaves.iter().find(|g| ws.groups[*g].name == n).cloned());
        let g = by_name
            .or_else(|| leaves.iter().find(|g| **g != ws.focus.group_id).cloned())
            .unwrap_or_else(|| ws.focus.group_id.clone());
        let n = ws.groups[&g].name.clone();
        (g, n)
    };
    let cmd = WsCommand::CreateTab {
        name: name.clone(),
        dir: dir.clone(),
        template: Some(tpl.clone()),
        group: Some(group.clone()),
        role: Some(Role::Manager),
    };
    // dry run: refuse before anything is written (name/dir rules, template, group)
    {
        let ws = state.ws.lock().unwrap();
        let l = state.loaded.read().unwrap();
        ws.clone().apply(cmd.clone(), &l)?;
    }
    let spec = TabSpec { name: name.clone(), dir: dir.clone(), template: spec_template, role: Some(Role::Manager) };
    let cfg_path = config::config_path(&root);
    // held through append → apply → rollback: no other config write can land in between, so
    // restoring the previous text undoes exactly this call's entry
    let _cfg = config::CONFIG_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let (edit, pushed) = {
        let mut l = state.loaded.write().unwrap();
        let edit = config::append_config_tab(&cfg_path, &spec)?;
        let pushed = !l.config.tabs.iter().any(|t| t.name == spec.name);
        if pushed {
            l.config.tabs.push(spec.clone());
        }
        (edit, pushed)
    };
    if let Err(e) = launcher::execute(state, cmd) {
        let mut l = state.loaded.write().unwrap();
        if pushed {
            l.config.tabs.retain(|t| t.name != spec.name);
        }
        if let ConfigEdit::Written(prev) = &edit {
            if let Err(re) = config::restore_config_file(&cfg_path, prev.as_deref()) {
                return Err(format!("{e} (config.json을 되돌리지 못했습니다: {re} — 탭 '{name}' 항목을 직접 지우세요)"));
            }
        }
        return Err(e);
    }
    let id = {
        let ws = state.ws.lock().unwrap();
        ws.tabs.values().find(|t| t.name == name).map(|t| t.id.clone()).unwrap_or_default()
    };
    Ok(reply(&id, &group_name, false))
}

/// Stub for the nightly refresh (Task 11): remember when the pane reported done.
fn handoff_done(state: &AppState, caller: &Caller) -> Result<Value, String> {
    state.handoffs.lock().unwrap().insert(caller.pane.clone(), SystemTime::now());
    Ok(json!("야간 정리 완료 신호를 받았습니다."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_table_matches_spec() {
        assert!(allowed("inbox_list", Role::Member));
        assert!(allowed("create_manager_tab", Role::Dispatcher));
        assert!(!allowed("create_manager_tab", Role::Manager));
        assert!(allowed("hire_member", Role::Lead) && !allowed("hire_member", Role::Manager));
        assert!(allowed("dismiss", Role::Lead) && !allowed("dismiss", Role::Member));
        assert!(!allowed("nope", Role::Dispatcher));
        // every implemented tool has a permission row
        for t in implemented() {
            assert!(PERMISSIONS.iter().any(|(n, _)| *n == t.name), "{}", t.name);
        }
    }

    #[test]
    fn list_filters_by_role() {
        let names = |r| list(r).into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(
            names(Role::Manager),
            [
                "inbox_post", "inbox_note", "inbox_propose_close", "inbox_list", "issue_create", "hire_lead", "dismiss",
                "dismiss_team", "list_team", "handoff_done"
            ]
        );
        assert_eq!(names(Role::Dispatcher), ["inbox_list", "handoff_done", "create_manager_tab"]);
        assert_eq!(names(Role::Lead), ["inbox_list", "hire_member", "dismiss", "dismiss_team", "list_team"]);
        assert_eq!(names(Role::Handover), ["inbox_list", "handoff_done"]);
        assert_eq!(names(Role::Member), ["inbox_list"]);
    }

    // ── team tools · create_manager_tab (Task 8) ──

    use crate::config::{ensure_dotdir, load_all};
    use crate::pty::{PtyManager, PtySink};
    use crate::workspace::{Effects, Workspace};
    use crate::{Snapshot, Ui};
    use std::sync::{mpsc, Arc};

    struct NullSink;
    impl PtySink for NullSink {
        fn output(&self, _: &str, _: u64, _: &[u8]) {}
        fn exited(&self, _: &str, _: u64, _: i32) {}
    }
    struct NullUi;
    impl Ui for NullUi {
        fn workspace_changed(&self, _: &Snapshot) {}
        fn drop_channel(&self, _: &str) {}
    }

    /// A Tauri-less AppState; effects are queued but never run (no process is started).
    struct Fx {
        state: AppState,
        root: PathBuf,
        appdata: PathBuf,
        _rx: mpsc::Receiver<Effects>,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    fn fx(config: &str) -> Fx {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        ensure_dotdir(root.path()).unwrap();
        std::fs::write(root.path().join(".agentboard/config.json"), config).unwrap();
        let loaded = load_all(appdata.path());
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let ws = Workspace::from_config(&loaded);
        for t in ws.tabs.values() {
            std::fs::create_dir_all(&t.workspace).unwrap();
        }
        let (tx, rx) = mpsc::channel();
        let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
        let state = AppState::new(
            ws,
            loaded,
            pty,
            Arc::new(NullUi),
            tx,
            crate::persist::Saver::disabled(),
            Vec::new(),
            appdata.path().join("run"),
        );
        Fx { root: root.path().to_path_buf(), appdata: appdata.path().to_path_buf(), state, _rx: rx, _keep: (root, appdata) }
    }

    impl Fx {
        /// Caller of the pane with session name `s`.
        fn who(&self, s: &str) -> Caller {
            let ws = self.state.ws.lock().unwrap();
            let p = ws.panes.values().find(|p| p.session_name.as_deref() == Some(s)).unwrap_or_else(|| panic!("no session {s}"));
            Caller { pane: p.id.clone(), role: p.role.unwrap(), tab: p.tab.clone(), team: p.team.clone(), session_name: s.into() }
        }
        fn call(&self, who: &str, tool: &str, args: Value) -> Result<Value, String> {
            call(&self.state, &self.who(who), tool, args)
        }
        fn sessions(&self) -> BTreeSet<String> {
            self.state.ws.lock().unwrap().panes.values().filter_map(|p| p.session_name.clone()).collect()
        }
        fn has(&self, s: &str) -> bool {
            self.sessions().contains(s)
        }
        fn brief(&self, rel: &str) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "# brief\n").unwrap();
        }
        fn valid(&self) {
            let mut ws = self.state.ws.lock().unwrap().clone();
            assert_eq!(ws.normalize_and_validate(), Ok(()));
        }
        fn config_text(&self) -> String {
            std::fs::read_to_string(self.root.join(".agentboard/config.json")).unwrap()
        }
    }

    const TWO_MANAGERS: &str = r#"{"tabs":[{"name":"billing","role":"manager","dir":"billing"},{"name":"ops","role":"manager","dir":"ops"}]}"#;

    #[test]
    fn lead_can_only_dismiss_own_members() {
        let f = fx(TWO_MANAGERS);
        f.brief("billing/teams/a/brief.md");
        f.brief("billing/teams/b/brief.md");
        f.call("billing", "hire_lead", json!({"team":"a","brief_path":"teams/a/brief.md"})).unwrap();
        f.call("billing", "hire_lead", json!({"team":"b","brief_path":"teams/b/brief.md"})).unwrap();
        for _ in 0..2 {
            f.call("billing-a-lead", "hire_member", json!({})).unwrap();
        }
        let r = f.call("billing-b-lead", "hire_member", json!({"extra_prompt":"테스트 담당"})).unwrap();
        assert_eq!(r["sessionName"], "billing-b-m1", "{r}");
        assert!(f.has("billing-a-m2") && f.has("billing-b-m1"));
        {
            let ws = f.state.ws.lock().unwrap();
            let m = ws.panes.values().find(|p| p.session_name.as_deref() == Some("billing-b-m1")).unwrap();
            let prompt = m.prompt.as_deref().unwrap();
            assert!(prompt.contains("billing-b-lead") && prompt.contains("테스트 담당"), "{prompt}");
        }

        // a lead touches nothing outside its own members
        for (tool, args) in [
            ("dismiss", json!({"agent":"billing-b-m1"})),
            ("dismiss", json!({"agent":"billing-b-lead"})),
            ("dismiss", json!({"agent":"billing-a-lead"})),
            ("dismiss", json!({"agent":"billing"})),
            ("dismiss_team", json!({"team":"b"})),
        ] {
            let err = f.call("billing-a-lead", tool, args.clone()).unwrap_err();
            assert!(err.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)), "{tool} {args}: {err}");
        }
        assert!(f.has("billing-b-m1") && f.has("billing-b-lead"));
        // its own members: by session name or pane id
        f.call("billing-a-lead", "dismiss", json!({"agent":"billing-a-m1"})).unwrap();
        assert!(!f.has("billing-a-m1"));
        let m2 = f.who("billing-a-m2").pane;
        f.call("billing-a-lead", "dismiss", json!({"agent": m2})).unwrap();
        assert!(!f.has("billing-a-m2"));
        // a lead's dismiss_team on its own team lets the members go, the lead stays
        f.call("billing-a-lead", "hire_member", json!({})).unwrap();
        assert!(f.has("billing-a-m3"), "numbers are not reused: {:?}", f.sessions());
        f.call("billing-a-lead", "dismiss_team", json!({"team":"a"})).unwrap();
        assert!(f.has("billing-a-lead") && !f.has("billing-a-m3"));
        // list_team: a lead sees its own team only
        let v = f.call("billing-a-lead", "list_team", json!({})).unwrap();
        let teams: Vec<&str> = v["teams"].as_array().unwrap().iter().map(|t| t["team"].as_str().unwrap()).collect();
        assert_eq!(teams, ["a"]);
        assert!(f.call("billing-a-lead", "list_team", json!({"team":"b"})).is_err());
        let v = f.call("billing", "list_team", json!({})).unwrap();
        assert_eq!(v["teams"].as_array().unwrap().len(), 2, "{v}");

        // another tab's Manager cannot touch billing's teams
        assert!(f.call("ops", "dismiss", json!({"agent":"billing-b-m1"})).is_err());
        assert!(f.call("ops", "dismiss_team", json!({"team":"b"})).is_err());
        // the Manager: any member of its tab, never a lead alone, whole teams
        let err = f.call("billing", "dismiss", json!({"agent":"billing-b-lead"})).unwrap_err();
        assert!(err.contains("dismiss_team"), "{err}");
        f.call("billing", "dismiss", json!({"agent":"billing-b-m1"})).unwrap();
        assert!(!f.has("billing-b-m1"));
        f.call("billing", "dismiss_team", json!({"team":"b"})).unwrap();
        assert!(!f.has("billing-b-lead"));
        assert!(f.call("billing", "dismiss_team", json!({"team":"b"})).is_err());
        // members cannot dismiss at all
        f.call("billing-a-lead", "hire_member", json!({})).unwrap();
        assert!(f.call("billing-a-m4", "dismiss", json!({"agent":"billing-a-m4"})).is_err());
        f.valid();
    }

    #[test]
    fn brief_path_outside_tab_workspace_rejected() {
        let f = fx(TWO_MANAGERS);
        f.brief("billing/teams/a/brief.md");
        f.brief("other/brief.md");
        f.brief("ops/brief.md");
        std::fs::create_dir_all(f.root.join("billing/teams/empty")).unwrap();
        let outside_abs = f.root.join("other/brief.md").to_string_lossy().to_string();
        let mut bad = vec![
            json!("../other/brief.md"),
            json!("../ops/brief.md"),
            json!(outside_abs),
            json!(f.root.join(".agentboard/config.json").to_string_lossy()),
            json!("teams/a/missing.md"),
            json!("teams/empty"),
            json!(""),
            json!(3),
        ];
        // a junction inside the workspace pointing outside is refused too
        let link = f.root.join("billing").join("link"); // backslashes only: mklink reads "/x" as a switch
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(f.root.join("other"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if made {
            bad.push(json!("link/brief.md"));
        } else {
            eprintln!("mklink /J unavailable; junction case skipped");
        }
        for p in bad {
            let err = f.call("billing", "hire_lead", json!({"team":"a","brief_path":p})).unwrap_err();
            assert!(!err.is_empty(), "{p}");
            assert!(!f.has("billing-a-lead"), "{p} must be refused");
        }
        // inside the workspace: relative or absolute
        let r = f.call("billing", "hire_lead", json!({"team":"a","brief_path":"teams/a/brief.md"})).unwrap();
        assert_eq!((r["sessionName"].as_str(), r["existing"].as_bool()), (Some("billing-a-lead"), Some(false)), "{r}");
        let expect = f.root.join("billing").join("teams").join("a").join("brief.md").display().to_string();
        {
            let ws = f.state.ws.lock().unwrap();
            let lead = ws.panes.values().find(|p| p.session_name.as_deref() == Some("billing-a-lead")).unwrap();
            let prompt = lead.prompt.as_deref().unwrap();
            assert!(prompt.contains(&expect), "{prompt}");
            assert!(prompt.contains("billing"), "names the Manager session: {prompt}");
            assert!(!prompt.contains('\n'));
        }
        let abs = f.root.join("billing/teams/a/brief.md").to_string_lossy().to_string();
        f.call("billing", "hire_lead", json!({"team":"b","brief_path":abs})).unwrap();
        // a retried hire (e.g. after a timeout) returns the existing lead, never a second one
        let r = f.call("billing", "hire_lead", json!({"team":"a","brief_path":"teams/a/brief.md"})).unwrap();
        assert_eq!(r["existing"], true, "{r}");
        assert_eq!(f.sessions().iter().filter(|s| s.ends_with("-lead")).count(), 2);
        // bad team ids are refused before anything else
        assert!(f.call("billing", "hire_lead", json!({"team":"Bad Id","brief_path":"teams/a/brief.md"})).is_err());
        f.valid();
    }

    #[test]
    fn create_manager_tab_persists_config_and_keeps_focus() {
        let cfg = r#"{
  "nightlyRefresh": "04:30",
  "tabs": [{"name": "billing", "role": "manager", "dir": "billing"}],
  "defaultGroup": "work",
  "groups": {"split": "horizontal", "children": [
    {"group": {"name": "control", "tabs": ["dispatcher"]}},
    {"group": {"name": "work", "tabs": ["billing"]}}]}
}
"#;
        let f = fx(cfg);
        let (focus, actives, last) = {
            let ws = f.state.ws.lock().unwrap();
            let actives: Vec<_> = ws.groups.values().map(|g| (g.id.clone(), g.active_tab_id.clone())).collect();
            (ws.focus.clone(), actives, ws.last_location.clone())
        };
        let tab_count = |name: &str| f.state.ws.lock().unwrap().tabs.values().filter(|t| t.name == name).count();

        // the folder must exist: creating it is the user's call
        let err = f.call("dispatcher", "create_manager_tab", json!({"name":"ops","dir":"ops"})).unwrap_err();
        assert!(err.contains("폴더") && err.contains("사용자"), "{err}");
        assert_eq!(tab_count("ops"), 0);
        assert_eq!(f.config_text(), cfg);
        std::fs::create_dir_all(f.root.join("ops")).unwrap();

        let r = f.call("dispatcher", "create_manager_tab", json!({"name":"ops","dir":"ops"})).unwrap();
        assert_eq!(r["name"], "ops", "{r}");
        {
            let ws = f.state.ws.lock().unwrap();
            let t = ws.tabs.values().find(|t| t.name == "ops").unwrap();
            assert_eq!(t.role, Some(Role::Manager));
            assert_eq!(t.workspace, f.root.join("ops"));
            let standing: Vec<_> = t.layout.leaves().into_iter().filter(|p| ws.panes[p].role == Some(Role::Manager)).collect();
            assert_eq!(ws.panes[&standing[0]].session_name.as_deref(), Some("ops"));
            // in defaultGroup "work", appended, in the background
            let work = ws.groups.values().find(|g| g.name == "work").unwrap();
            assert_eq!(work.tab_ids.last(), Some(&t.id));
            assert_eq!(ws.focus, focus);
            assert_eq!(ws.last_location, last);
            let now: Vec<_> = ws.groups.values().map(|g| (g.id.clone(), g.active_tab_id.clone())).collect();
            assert_eq!(now, actives, "no group's active tab changed");
        }
        f.valid();
        // config.json: the tab is appended, everything else kept (values and key order)
        let text = f.config_text();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["tabs"][1], json!({"name":"ops","dir":"ops","role":"manager"}), "{text}");
        assert_eq!(v["nightlyRefresh"], "04:30");
        assert_eq!(v["groups"]["children"][1]["group"]["tabs"], json!(["billing"]));
        let pos = |k: &str| text.find(k).unwrap();
        assert!(pos("nightlyRefresh") < pos("\"tabs\"") && pos("\"tabs\"") < pos("defaultGroup") && pos("defaultGroup") < pos("\"groups\""));
        assert!(f.state.loaded.read().unwrap().config.tabs.iter().any(|t| t.name == "ops" && t.role == Some(Role::Manager)));
        let reloaded = load_all(&f.appdata);
        assert!(reloaded.errors.is_empty(), "{:?}", reloaded.errors);
        assert!(reloaded.config.tabs.iter().any(|t| t.name == "ops" && t.role == Some(Role::Manager)));

        // a retried call returns the same tab, nothing is duplicated
        let r = f.call("dispatcher", "create_manager_tab", json!({"name":"ops","dir":"ops"})).unwrap();
        assert_eq!(r["existing"], true, "{r}");
        assert_eq!(tab_count("ops"), 1);
        assert_eq!(f.config_text(), text);

        // phase-1 name/dir rules; nothing written on refusal
        std::fs::create_dir_all(f.root.join("x")).unwrap();
        for args in [
            json!({"name":"dispatcher","dir":"x"}),
            json!({"name":"billing","dir":"x"}),
            json!({"name":"","dir":"x"}),
            json!({"name":"y","dir":"../out"}),
            json!({"name":"y","dir":f.root.join("x").to_string_lossy()}),
            json!({"name":"y","dir":""}),
            json!({"name":"y","dir":"x","template":"nope"}),
        ] {
            assert!(f.call("dispatcher", "create_manager_tab", args.clone()).is_err(), "{args}");
            assert_eq!(f.config_text(), text, "{args}");
        }
        assert_eq!(tab_count("y"), 0);
        // a renamed Manager keeps its session name: the old name is not free for a new Manager
        let b = f.who("billing").tab;
        crate::launcher::execute(&f.state, WsCommand::RenameTab { tab: b, name: "billing-old".into() }).unwrap();
        std::fs::create_dir_all(f.root.join("billing2")).unwrap();
        let err = f.call("dispatcher", "create_manager_tab", json!({"name":"billing","dir":"billing2"})).unwrap_err();
        assert!(err.contains("세션 이름"), "{err}");
        assert_eq!(f.config_text(), text, "nothing written");
        assert!(!f.state.loaded.read().unwrap().config.tabs.iter().any(|t| t.dir == "billing2"));
        f.valid();
        // control characters never reach a session name or a prompt
        assert!(f.call("dispatcher", "create_manager_tab", json!({"name":"a\nb","dir":"x"})).is_err());
        assert_eq!(f.config_text(), text);
        // only the dispatcher may create standing tabs
        assert!(f.call("billing", "create_manager_tab", json!({"name":"y","dir":"x"})).is_err());

        // one group only: the tab opens there, still without taking focus
        let f = fx(r#"{"tabs":[]}"#);
        std::fs::create_dir_all(f.root.join("solo-work")).unwrap();
        let here = |ws: &Workspace| (ws.focus.clone(), ws.groups[&ws.focus.group_id].active_tab_id.clone());
        let before = here(&f.state.ws.lock().unwrap());
        f.call("dispatcher", "create_manager_tab", json!({"name":"w","dir":"solo-work","template":"solo"})).unwrap();
        let ws = f.state.ws.lock().unwrap();
        assert_eq!(here(&ws), before);
        let v: Value = serde_json::from_str(&f.config_text()).unwrap();
        assert_eq!(v["tabs"][0], json!({"name":"w","dir":"solo-work","role":"manager","template":"solo"}));
    }
}
