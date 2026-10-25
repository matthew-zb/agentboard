//! Issue folders (spec §6): `<tab workspace>/issues/<YYYYMMDD-slug>/{input,output}`.
//!
//! The id is made only of ASCII alphanumerics, Hangul and `-` (no separators, no dots), so
//! joining it under `issues/` can never leave the tab workspace; the `issues` directory itself
//! is checked (canonicalized) to still be inside the workspace, so a junction planted there is
//! refused. A clash gets `-2`, `-3`, … — claimed with `create_dir` (not `_all`), so two
//! concurrent creations never share a folder.

use chrono::NaiveDate;
use std::path::{Path, PathBuf};

/// Maximum slug length in characters.
pub const SLUG_MAX: usize = 40;
/// Slug used when the title has no usable character.
pub const EMPTY_SLUG: &str = "issue";

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}

/// Slug of `title`: ASCII alphanumerics (lowercased) and Hangul kept, every other run of
/// characters becomes one `-`, no leading/trailing `-`, at most [`SLUG_MAX`] characters.
pub fn slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() || is_hangul(c) {
            s.push(c.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s: String = s.chars().take(SLUG_MAX).collect();
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() {
        EMPTY_SLUG.to_string()
    } else {
        s
    }
}

/// Whether `id` looks like an issue id this module produces (used to validate stored links).
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64 * 4
        && id.chars().all(|c| c.is_ascii_alphanumeric() || is_hangul(c) || c == '-')
}

/// Create `<tab_workspace>/issues/<YYYYMMDD-slug>[-N]/{input,output}`; returns the id and the
/// issue folder. The tab workspace must exist.
pub fn create(tab_workspace: &Path, title: &str, today: NaiveDate) -> Result<(String, PathBuf), String> {
    if !tab_workspace.is_dir() {
        return Err("담당 탭의 작업 폴더가 없어 이슈 폴더를 만들 수 없습니다".into());
    }
    let issues = tab_workspace.join("issues");
    std::fs::create_dir_all(&issues).map_err(|e| format!("issues 폴더를 만들 수 없습니다 — {e}"))?;
    let ws_real = std::fs::canonicalize(tab_workspace).map_err(|e| format!("작업 폴더를 확인할 수 없습니다 — {e}"))?;
    let issues_real = std::fs::canonicalize(&issues).map_err(|e| format!("issues 폴더를 확인할 수 없습니다 — {e}"))?;
    if !issues_real.starts_with(&ws_real) {
        return Err("issues 폴더가 작업 폴더 밖을 가리킵니다 — 이슈 폴더를 만들지 않습니다".into());
    }
    let base = format!("{}-{}", today.format("%Y%m%d"), slug(title));
    for n in 1..=1000u32 {
        let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let dir = issues.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                for sub in ["input", "output"] {
                    std::fs::create_dir(dir.join(sub))
                        .map_err(|e| format!("이슈 폴더의 {sub} 폴더를 만들 수 없습니다 — {e}"))?;
                }
                return Ok((id, dir));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("이슈 폴더를 만들 수 없습니다: {id} — {e}")),
        }
    }
    Err(format!("같은 이름의 이슈 폴더가 너무 많습니다: {base}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()
    }

    #[test]
    fn issue_id_slug_and_dedupe() {
        assert_eq!(slug("Billing 오류: 결제 실패!"), "billing-오류-결제-실패");
        assert_eq!(slug("  ../..\\evil/  "), "evil");
        assert_eq!(slug("???"), EMPTY_SLUG);
        assert_eq!(slug("a_b.c d--e"), "a-b-c-d-e");
        let long = "가".repeat(60);
        assert_eq!(slug(&long).chars().count(), SLUG_MAX);
        assert!(!slug(&format!("{}- x", "a".repeat(39))).ends_with('-'));

        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("billing");
        std::fs::create_dir(&ws).unwrap();
        let (id, dir) = create(&ws, "결제 실패 / ../x", day()).unwrap();
        assert_eq!(id, "20261025-결제-실패-x");
        assert_eq!(dir, ws.join("issues").join(&id));
        assert!(dir.join("input").is_dir() && dir.join("output").is_dir());
        let (id2, dir2) = create(&ws, "결제 실패 x", day()).unwrap();
        assert_eq!(id2, "20261025-결제-실패-x-2");
        assert!(dir2.join("output").is_dir());
        let (id3, _) = create(&ws, "결제 실패 x", day()).unwrap();
        assert_eq!(id3, "20261025-결제-실패-x-3");
        assert!(is_valid_id(&id3) && !is_valid_id("a/b") && !is_valid_id("..") && !is_valid_id(""));
        // never escapes: everything created lives under <ws>/issues
        for e in std::fs::read_dir(tmp.path()).unwrap() {
            assert_eq!(e.unwrap().file_name(), "billing");
        }
        // a missing workspace is an error (Korean), nothing is created
        let err = create(&tmp.path().join("nope"), "x", day()).unwrap_err();
        assert!(err.contains("작업 폴더"), "{err}");
        assert!(!tmp.path().join("nope").exists());
    }
}
