//! The app's own process trees (dashboard D3b): a PTY child and its descendants, and process
//! creation times (to tell a live process from an earlier one that had the same id). Only ever
//! asked about pids the app itself started — never used to list other programs.
//!
//! Windows: one Toolhelp32 process snapshot per call, walked by parent id; a process counts as a
//! descendant only if it was created after its parent (a dead parent's id may have been reused).

/// `root` first, then its descendants (breadth first, at most [`MAX_DEPTH`] levels). Empty when
/// `root` is not a running process.
pub fn self_and_descendants(root: u32) -> Vec<u32> {
    imp::self_and_descendants(root)
}

/// Creation time of `pid` as the FILETIME value (100 ns since 1601), if the process exists.
pub fn creation_time(pid: u32) -> Option<u64> {
    imp::creation_time(pid)
}

/// Levels below the PTY child that are searched (shell → claude → its helpers).
pub const MAX_DEPTH: usize = 4;

#[cfg(windows)]
mod imp {
    use super::MAX_DEPTH;
    use std::collections::{HashMap, HashSet};

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessEntry32W {
        dwSize: u32,
        cntUsage: u32,
        th32ProcessID: u32,
        th32DefaultHeapID: usize,
        th32ModuleID: u32,
        cntThreads: u32,
        th32ParentProcessID: u32,
        pcPriClassBase: i32,
        dwFlags: u32,
        szExeFile: [u16; 260],
    }

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn GetProcessTimes(h: isize, created: *mut FileTime, exited: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetExitCodeProcess(h: isize, code: *mut u32) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }

    const TH32CS_SNAPPROCESS: u32 = 0x2;
    const INVALID_HANDLE_VALUE: isize = -1;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    /// (pid, parent pid) of every process in one snapshot.
    fn snapshot() -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        // SAFETY: plain Win32 calls on a struct of the documented layout; the handle is closed.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE || snap == 0 {
                return out;
            }
            let mut e: ProcessEntry32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<ProcessEntry32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e);
            while ok != 0 {
                out.push((e.th32ProcessID, e.th32ParentProcessID));
                ok = Process32NextW(snap, &mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    pub fn creation_time(pid: u32) -> Option<u64> {
        // SAFETY: the handle is checked and closed; out-params are local.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h == 0 {
                return None;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE;
            let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
            let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0;
            CloseHandle(h);
            (alive && ok).then_some((u64::from(c.high) << 32) | u64::from(c.low))
        }
    }

    pub fn self_and_descendants(root: u32) -> Vec<u32> {
        let Some(root_created) = creation_time(root) else { return Vec::new() };
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent) in snapshot() {
            if pid != parent {
                children.entry(parent).or_default().push(pid);
            }
        }
        let mut out = vec![root];
        let mut seen: HashSet<u32> = HashSet::from([root]);
        let mut level = vec![(root, root_created)];
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            for (pid, created) in &level {
                for kid in children.get(pid).into_iter().flatten() {
                    if !seen.insert(*kid) {
                        continue;
                    }
                    // a reused parent id: the "child" is older than this parent
                    if let Some(kc) = creation_time(*kid).filter(|kc| kc >= created) {
                        out.push(*kid);
                        next.push((*kid, kc));
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            level = next;
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn self_and_descendants(_root: u32) -> Vec<u32> {
        Vec::new()
    }
    pub fn creation_time(_pid: u32) -> Option<u64> {
        None
    }
}
