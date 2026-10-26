//! Team folders of a Manager tab: `<tab workspace>/teams/<id>/brief.md`. A team is done when
//! its brief's first line (trimmed) is exactly [`DONE_MARK`] (Manager role prompt: the Manager
//! writes it before `dismiss_team`; an unmarked brief makes the next Manager session re-hire).

use std::path::Path;

/// First line of a finished team's brief.
pub const DONE_MARK: &str = "상태: 완료";
/// Characters of a brief's first line quoted in messages.
const LINE_MAX: usize = 40;

/// Teams whose brief is not marked done, `(id, first line)` sorted by id. A folder under
/// `teams/` without `brief.md` is not a team; an unreadable brief counts as not done.
pub fn briefs_not_done(tab_workspace: &Path) -> Vec<(String, String)> {
    let Ok(dirs) = std::fs::read_dir(tab_workspace.join("teams")) else { return Vec::new() };
    let mut out: Vec<(String, String)> = dirs
        .filter_map(|d| d.ok())
        .filter(|d| d.path().is_dir())
        .filter_map(|d| {
            let brief = d.path().join("brief.md");
            if !brief.is_file() {
                return None;
            }
            let first = match std::fs::read_to_string(&brief) {
                // an editor's BOM is not part of the line
                Ok(text) => {
                    let first = text.trim_start_matches('\u{feff}').lines().next().unwrap_or_default();
                    first.trim().chars().take(LINE_MAX).collect()
                }
                Err(_) => "(읽을 수 없음)".to_string(),
            };
            (first != DONE_MARK).then(|| (d.file_name().to_string_lossy().into_owned(), first))
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_briefs_not_marked_done_are_listed() {
        let d = tempfile::tempdir().unwrap();
        assert!(briefs_not_done(d.path()).is_empty(), "no teams folder");
        let put = |rel: &str, text: &str| {
            let p = d.path().join("teams").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        put("b/brief.md", "상태: 진행 중\n목표\n");
        put("a/brief.md", "\u{feff}# brief\n");
        put("done/brief.md", "  상태: 완료 \r\n");
        put("bom-done/brief.md", "\u{feff}상태: 완료\n");
        put("notes/readme.md", "x");
        put("empty/brief.md", "");
        assert_eq!(
            briefs_not_done(d.path()),
            [("a".to_string(), "# brief".to_string()), ("b".into(), "상태: 진행 중".into()), ("empty".into(), "".into())]
        );
    }
}
