//! MCP tools of the control server (spec §4): definitions, the per-role permission table and
//! dispatch. `control.rs` authenticates the caller and turns `call`'s `Err` into an MCP
//! `isError: true` result whose text is the (Korean) message.
//!
//! Adding a tool (Tasks 6·8·9·11·12): give it a `ToolDef` in [`implemented`] (its roles come
//! from [`PERMISSIONS`], so a tool missing there is never listed) and an arm in [`call`]. A
//! tool body may take `ws` / `loaded` briefly and must go through `launcher::execute` for any
//! model change (R13); it runs on a blocking thread, never inside the async runtime.

use crate::control::Caller;
use crate::inbox::{self, ItemState, NewItem};
use crate::issues;
use crate::roles::Role;
use crate::AppState;
use serde_json::{json, Value};
use std::collections::BTreeMap;
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
            name: "list_team",
            description: "이 탭에 있는 팀들(또는 지정한 한 팀)의 구성과 각 창의 상태를 돌려줍니다.",
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
        _ => Err(format!("알 수 없는 도구입니다: {name}")),
    }
}

/// Content caps of MCP inputs (characters, except `TEXT_MAX_BYTES`).
pub const TITLE_MAX: usize = 200;
pub const ENTRY_MAX: usize = 500;
pub const ENTRIES_MAX: usize = 20;
pub const TEXT_MAX_BYTES: usize = 8 * 1024;

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
/// teams sorted by id, the lead first, then members by session name.
fn list_team(state: &AppState, caller: &Caller, args: &Value) -> Result<Value, String> {
    let filter = match args.get("team") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("team은 문자열이어야 합니다".into()),
    };
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
            ["inbox_post", "inbox_note", "inbox_propose_close", "inbox_list", "issue_create", "list_team", "handoff_done"]
        );
        assert_eq!(names(Role::Dispatcher), ["inbox_list", "handoff_done"]);
        assert_eq!(names(Role::Lead), ["inbox_list", "list_team"]);
        assert_eq!(names(Role::Handover), ["inbox_list", "handoff_done"]);
        assert_eq!(names(Role::Member), ["inbox_list"]);
    }
}
