//! Phase-2 roles (spec §2): the `Role` enum, per-role settings from `config.json` `roles`, and
//! the session-name rule used for `--name` (spec §3).
//!
//! Role assignment: the Dispatcher tab (phase-1 `config.dispatcher` block) implies
//! `Role::Dispatcher`; `tabs[].role` accepts only `manager` | `handover`; `lead` / `member`
//! panes are created at run time by the team tools (Task 8). Loading the role prompt files
//! (`systemPromptFile`) is Task 4.

use crate::config::{check_relative_in, sem_err, ConfigError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Wire shape: `"dispatcher" | "handover" | "manager" | "lead" | "member"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Dispatcher,
    Handover,
    Manager,
    Lead,
    Member,
}

impl Role {
    pub const ALL: [Role; 5] = [Role::Dispatcher, Role::Handover, Role::Manager, Role::Lead, Role::Member];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Dispatcher => "dispatcher",
            Role::Handover => "handover",
            Role::Manager => "manager",
            Role::Lead => "lead",
            Role::Member => "member",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// Standing roles live for the app's lifetime (one per tab); team roles are temporary.
    pub fn is_standing(self) -> bool {
        matches!(self, Role::Dispatcher | Role::Handover | Role::Manager)
    }
}

/// Default `roles.member.maxPerTeam`.
pub const DEFAULT_MAX_PER_TEAM: usize = 4;

/// `config.json` `roles.<role>`: `{"systemPromptFile"?, "agent"?, "args"?, "maxPerTeam"?}`.
/// `systemPromptFile` is relative to `<workspaceRoot>/.agentboard/`; `maxPerTeam` is only
/// valid for `member`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleConfig {
    #[serde(default)]
    pub system_prompt_file: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub max_per_team: Option<usize>,
}

/// Parse the `roles` object. Each bad entry is dropped alone and reported with its JSON path.
pub fn parse_roles(v: &serde_json::Value, file: &Path) -> (BTreeMap<Role, RoleConfig>, Vec<ConfigError>) {
    let mut out = BTreeMap::new();
    let mut errs = Vec::new();
    let Some(obj) = v.as_object() else {
        errs.push(sem_err(file, "roles", "roles는 객체여야 함"));
        return (out, errs);
    };
    for (key, val) in obj {
        let p = format!("roles.{key}");
        let Some(role) = Role::parse(key) else {
            errs.push(sem_err(
                file,
                &p,
                format!("알 수 없는 역할 '{key}' (dispatcher, handover, manager, lead, member 중 하나)"),
            ));
            continue;
        };
        let rc: RoleConfig = match serde_json::from_value(val.clone()) {
            Ok(rc) => rc,
            Err(e) => {
                errs.push(sem_err(file, &p, format!("역할 설정 오류: {e}")));
                continue;
            }
        };
        let before = errs.len();
        if let Some(f) = &rc.system_prompt_file {
            if f.trim().is_empty() {
                errs.push(sem_err(file, format!("{p}.systemPromptFile"), "경로가 비어 있음"));
            } else if let Some(m) = check_relative_in(f, ".agentboard 폴더") {
                errs.push(sem_err(file, format!("{p}.systemPromptFile"), m));
            }
        }
        if let Some(a) = &rc.agent {
            if a.trim().is_empty() {
                errs.push(sem_err(file, format!("{p}.agent"), "에이전트 이름이 비어 있음"));
            }
        }
        match (role, rc.max_per_team) {
            (_, None) => {}
            (Role::Member, Some(0)) => errs.push(sem_err(file, format!("{p}.maxPerTeam"), "1 이상이어야 함")),
            (Role::Member, Some(_)) => {}
            (_, Some(_)) => errs.push(sem_err(file, format!("{p}.maxPerTeam"), "maxPerTeam은 member 역할에만 쓸 수 있음")),
        }
        if errs.len() == before {
            out.insert(role, rc);
        }
    }
    (out, errs)
}

/// Names that may never be a tab / session name: `dispatcher` (the Dispatcher tab) and
/// `agentboard` in any case — the app's bridge sends as `from-name="agentboard"` and role
/// prompts grant that sender user authority (R21), so no session may carry that name.
/// Returns the Korean reason.
pub fn reserved_name(name: &str) -> Option<String> {
    if name == "dispatcher" {
        return Some("'dispatcher'는 예약된 탭 이름입니다".into());
    }
    if name.eq_ignore_ascii_case(AGENTBOARD) {
        return Some(format!("'{name}'은(는) 앱이 메시지를 보낼 때 쓰는 예약된 이름입니다"));
    }
    None
}

/// The app's sender name on the cross-session bridge (`from-name`).
pub const AGENTBOARD: &str = "agentboard";

/// Session name (`--name`) of an agent: standing roles use the tab name; a team lead is
/// `<tab>-<team>-lead`, member `n` is `<tab>-<team>-m<n>` (numbering from 1, chosen by the
/// caller). A team role without a team id uses `team` as the id.
pub fn session_name(tab: &str, role: Role, team: Option<&str>, n: usize) -> String {
    let team = team.unwrap_or("team");
    match role {
        Role::Dispatcher | Role::Handover | Role::Manager => tab.to_string(),
        Role::Lead => format!("{tab}-{team}-lead"),
        Role::Member => format!("{tab}-{team}-m{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_rules() {
        assert_eq!(session_name("billing", Role::Lead, Some("batch"), 0), "billing-batch-lead");
        assert_eq!(session_name("billing", Role::Member, Some("batch"), 1), "billing-batch-m1");
        assert_eq!(session_name("billing", Role::Member, Some("batch"), 12), "billing-batch-m12");
        assert_eq!(session_name("billing", Role::Manager, None, 0), "billing");
        assert_eq!(session_name("billing", Role::Manager, Some("ignored"), 3), "billing");
        assert_eq!(session_name("dispatcher", Role::Dispatcher, None, 0), "dispatcher");
        assert_eq!(session_name("handover", Role::Handover, None, 0), "handover");
    }

    #[test]
    fn reserved_names() {
        for n in ["dispatcher", "agentboard", "AgentBoard", "AGENTBOARD"] {
            assert!(reserved_name(n).is_some(), "{n}");
        }
        for n in ["billing", "agentboard2", "my-agentboard", "Dispatcher2"] {
            assert!(reserved_name(n).is_none(), "{n}");
        }
    }

    #[test]
    fn path_errors_name_the_right_base() {
        let v = serde_json::json!({"manager":{"systemPromptFile":"C:/x.md"},"lead":{"systemPromptFile":"../../x.md"}});
        let (ok, errs) = parse_roles(&v, Path::new("config.json"));
        assert!(ok.is_empty());
        assert_eq!(errs.len(), 2);
        for e in &errs {
            let s = format!("{e:?}");
            assert!(s.contains(".agentboard 폴더") && !s.contains("탭 작업공간"), "{s}");
        }
        assert_eq!(
            crate::config::check_relative_in("/abs", "workspaceRoot").unwrap(),
            "절대경로는 사용할 수 없음 (workspaceRoot 기준 상대경로만 허용)"
        );
        assert_eq!(crate::config::check_relative_in("a/../b", "workspaceRoot"), None);
    }

    #[test]
    fn role_serde_is_lowercase() {
        for r in Role::ALL {
            assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(r.as_str()));
            assert_eq!(serde_json::from_value::<Role>(serde_json::json!(r.as_str())).unwrap(), r);
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("Manager"), None);
        assert!(Role::Manager.is_standing() && !Role::Lead.is_standing() && !Role::Member.is_standing());
    }
}
