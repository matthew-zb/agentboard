//! Minimal stub; Task 6 replaces this with the full agent profile type.

use crate::config::{located_json_error, ConfigError};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct AgentProfile {
    pub name: String,
}

pub fn load_dir(dir: &Path) -> (BTreeMap<String, AgentProfile>, Vec<ConfigError>) {
    let mut map = BTreeMap::new();
    let mut errors = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return (map, errors) };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |x| x == "json"))
        .collect();
    files.sort();
    for p in files {
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) => {
                errors.push(ConfigError { file: p.clone(), line: 0, col: 0, json_path: None, message: format!("파일을 읽을 수 없음: {e}") });
                continue;
            }
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let name = v.get("name").and_then(|n| n.as_str()).map(String::from).unwrap_or(stem);
                map.insert(name.clone(), AgentProfile { name });
            }
            Err(e) => errors.push(located_json_error(&p, &e)),
        }
    }
    (map, errors)
}
