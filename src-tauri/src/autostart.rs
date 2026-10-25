//! Start AgentBoard at login (spec §9): the value `AgentBoard` = `"<exe>"` (quoted) under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Toggled from the command palette
//! ("로그인 시 자동 실행 켜기/끄기", Tauri `autostart_get` / `autostart_set`).
//!
//! Plain advapi32 calls (like `winjob`'s kernel32 ones) — no extra crate. The key / value names
//! are parameters of the `*_at` functions so tests use a throw-away key (and delete it).

use std::path::Path;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "AgentBoard";

#[cfg(windows)]
mod ffi {
    #[link(name = "advapi32")]
    extern "system" {
        pub fn RegSetKeyValueW(key: isize, sub: *const u16, name: *const u16, ty: u32, data: *const u8, len: u32) -> i32;
        pub fn RegGetValueW(key: isize, sub: *const u16, name: *const u16, flags: u32, ty: *mut u32, data: *mut u8, len: *mut u32) -> i32;
        pub fn RegDeleteKeyValueW(key: isize, sub: *const u16, name: *const u16) -> i32;
        pub fn RegDeleteKeyW(key: isize, sub: *const u16) -> i32;
    }
    pub const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    pub const REG_SZ: u32 = 1;
    pub const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    pub const ERROR_SUCCESS: i32 = 0;
    pub const ERROR_FILE_NOT_FOUND: i32 = 2;
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The registry data for `exe`: the path in double quotes (paths with spaces).
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Write `name` = `"<exe>"` under HKCU\`key` (the key is created if missing).
#[cfg(windows)]
pub fn enable_at(key: &str, name: &str, exe: &Path) -> Result<(), String> {
    let data = wide(&run_value(exe));
    let (k, n) = (wide(key), wide(name));
    // SAFETY: valid NUL-terminated UTF-16 buffers; length in bytes including the NUL.
    let rc = unsafe {
        ffi::RegSetKeyValueW(ffi::HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), ffi::REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32)
    };
    if rc == ffi::ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("자동 실행을 등록할 수 없습니다(레지스트리 오류 {rc})"))
    }
}

/// Remove the value (absent already → Ok).
#[cfg(windows)]
pub fn disable_at(key: &str, name: &str) -> Result<(), String> {
    let (k, n) = (wide(key), wide(name));
    // SAFETY: valid NUL-terminated buffers.
    let rc = unsafe { ffi::RegDeleteKeyValueW(ffi::HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
    match rc {
        ffi::ERROR_SUCCESS | ffi::ERROR_FILE_NOT_FOUND => Ok(()),
        _ => Err(format!("자동 실행을 해제할 수 없습니다(레지스트리 오류 {rc})")),
    }
}

/// The value's data, `None` when the key or value does not exist.
#[cfg(windows)]
pub fn read_at(key: &str, name: &str) -> Result<Option<String>, String> {
    let (k, n) = (wide(key), wide(name));
    let mut len: u32 = 0;
    // SAFETY: size query with a null buffer, then a read into a buffer of that size.
    let rc = unsafe {
        ffi::RegGetValueW(ffi::HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), ffi::RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut len)
    };
    if rc == ffi::ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if rc != ffi::ERROR_SUCCESS {
        return Err(format!("자동 실행 상태를 읽을 수 없습니다(레지스트리 오류 {rc})"));
    }
    let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
    let mut len = (buf.len() * 2) as u32;
    let rc = unsafe {
        ffi::RegGetValueW(
            ffi::HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            ffi::RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut u8,
            &mut len,
        )
    };
    match rc {
        ffi::ERROR_SUCCESS => {
            let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            Ok(Some(String::from_utf16_lossy(&buf[..end])))
        }
        ffi::ERROR_FILE_NOT_FOUND => Ok(None),
        _ => Err(format!("자동 실행 상태를 읽을 수 없습니다(레지스트리 오류 {rc})")),
    }
}

pub fn is_enabled_at(key: &str, name: &str) -> Result<bool, String> {
    Ok(read_at(key, name)?.is_some())
}

/// Enabled **for `exe`**: the value exists and names this executable (compared
/// case-insensitively, quotes optional). A value left by a moved / older / dev build counts as
/// off, so the palette offers "켜기", which rewrites it.
pub fn is_enabled_for_at(key: &str, name: &str, exe: &Path) -> Result<bool, String> {
    let Some(v) = read_at(key, name)? else { return Ok(false) };
    let stored = v.trim().trim_matches('"');
    Ok(stored.eq_ignore_ascii_case(&exe.display().to_string()))
}

/// Delete an (empty of subkeys) key under HKCU — tests clean up their key with it.
#[cfg(windows)]
pub fn delete_key(key: &str) -> Result<(), String> {
    let k = wide(key);
    // SAFETY: valid NUL-terminated buffer.
    let rc = unsafe { ffi::RegDeleteKeyW(ffi::HKEY_CURRENT_USER, k.as_ptr()) };
    match rc {
        ffi::ERROR_SUCCESS | ffi::ERROR_FILE_NOT_FOUND => Ok(()),
        _ => Err(format!("레지스트리 키를 지울 수 없습니다(오류 {rc})")),
    }
}

/// Register the running executable to start at login.
pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("실행 파일 경로를 알 수 없습니다 — {e}"))?;
    enable_at(RUN_KEY, VALUE_NAME, &exe)
}

pub fn disable() -> Result<(), String> {
    disable_at(RUN_KEY, VALUE_NAME)
}

/// Whether login autostart is set up for the running executable (see [`is_enabled_for_at`]).
pub fn is_enabled() -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| format!("실행 파일 경로를 알 수 없습니다 — {e}"))?;
    is_enabled_for_at(RUN_KEY, VALUE_NAME, &exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deletes the test key even when an assertion fails.
    struct TestKey(String);
    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = disable_at(&self.0, VALUE_NAME);
            let _ = delete_key(&self.0);
        }
    }

    #[test]
    fn autostart_round_trips_on_an_injected_key() {
        let mut rnd = [0u8; 4];
        getrandom::fill(&mut rnd).unwrap();
        let key = TestKey(format!(r"Software\AgentBoardTest-{}-{:02x}{:02x}{:02x}{:02x}", std::process::id(), rnd[0], rnd[1], rnd[2], rnd[3]));
        let exe = Path::new(r"C:\Program Files\AgentBoard\agentboard.exe");
        assert_eq!(is_enabled_at(&key.0, VALUE_NAME), Ok(false), "missing key = off");
        enable_at(&key.0, VALUE_NAME, exe).unwrap();
        assert_eq!(is_enabled_at(&key.0, VALUE_NAME), Ok(true));
        assert_eq!(read_at(&key.0, VALUE_NAME).unwrap().as_deref(), Some(r#""C:\Program Files\AgentBoard\agentboard.exe""#));
        assert_eq!(is_enabled_for_at(&key.0, VALUE_NAME, exe), Ok(true));
        assert_eq!(is_enabled_for_at(&key.0, VALUE_NAME, Path::new(r"c:\program files\agentboard\AGENTBOARD.exe")), Ok(true));
        // a stale value (another exe) is reported as off; enabling rewrites it
        let other = Path::new(r"C:\old\agentboard.exe");
        assert_eq!(is_enabled_for_at(&key.0, VALUE_NAME, other), Ok(false));
        enable_at(&key.0, VALUE_NAME, other).unwrap();
        assert_eq!(is_enabled_for_at(&key.0, VALUE_NAME, other), Ok(true));
        assert_eq!(is_enabled_for_at(&key.0, VALUE_NAME, exe), Ok(false));
        // enabling twice keeps one value
        enable_at(&key.0, VALUE_NAME, exe).unwrap();
        disable_at(&key.0, VALUE_NAME).unwrap();
        assert_eq!(is_enabled_at(&key.0, VALUE_NAME), Ok(false));
        disable_at(&key.0, VALUE_NAME).unwrap(); // idempotent
        drop(key);
    }

    #[test]
    fn run_value_is_quoted() {
        assert_eq!(run_value(Path::new(r"C:\a b\x.exe")), r#""C:\a b\x.exe""#);
    }
}
