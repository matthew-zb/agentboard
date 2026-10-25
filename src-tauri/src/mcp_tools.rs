//! MCP tools of the control server (spec §4): definitions, the per-role permission table and
//! dispatch. `control.rs` authenticates the caller and turns `call`'s `Err` into an MCP
//! `isError: true` result whose text is the (Korean) message.
//!
//! Adding a tool (Tasks 6·8·9·11·12): give it a `ToolDef` in [`implemented`] (its roles come
//! from [`PERMISSIONS`], so a tool missing there is never listed) and an arm in [`call`]. A
//! tool body may take `ws` / `loaded` briefly and must go through `launcher::execute` for any
//! model change (R13); it runs on a blocking thread, never inside the async runtime.

use crate::control::Caller;
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
        "list_team" => list_team(state, caller, &args),
        "handoff_done" => handoff_done(state, caller),
        _ => Err(format!("알 수 없는 도구입니다: {name}")),
    }
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
        assert_eq!(names(Role::Manager), ["list_team", "handoff_done"]);
        assert_eq!(names(Role::Lead), ["list_team"]);
        assert_eq!(names(Role::Handover), ["handoff_done"]);
        assert!(names(Role::Member).is_empty());
    }
}
