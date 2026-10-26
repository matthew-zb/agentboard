//! Windows Job Object wrapper: child processes die when the job (app) closes.
use win32job::Job;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CloseHandle(h: isize) -> i32;
    fn GetLastError() -> u32;
}
const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_SET_QUOTA: u32 = 0x0100;

/// A KILL_ON_JOB_CLOSE job with JOB_OBJECT_LIMIT_BREAKAWAY_OK.
///
/// NOTE for future code: once the app process is self-assigned, every process it
/// launches inherits the job and is killed when the app exits. Launches that must
/// outlive the app (open in editor, updater, relaunch) must pass
/// CREATE_BREAKAWAY_FROM_JOB (0x0100_0000) in their creation flags; BREAKAWAY_OK is set
/// on the job precisely so that this is allowed. PTY children intentionally stay in it.
pub struct JobGuard {
    job: std::mem::ManuallyDrop<Job>,
    /// set once the app process is in the job; the handle is then never closed
    leaked: bool,
}

/// win32job errors carry the OS error only in their Debug form (it already holds
/// GetLastError from the failing call), so format with {:?}.
fn fmt_err(e: win32job::JobError) -> String {
    format!("{e:?}")
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if !self.leaked {
            // SAFETY: dropped exactly once, here.
            unsafe { std::mem::ManuallyDrop::drop(&mut self.job) };
        }
    }
}

impl JobGuard {
    pub fn new() -> Result<JobGuard, String> {
        let job = Job::create().map_err(fmt_err)?;
        let mut info = job.query_extended_limit_info().map_err(fmt_err)?;
        info.limit_kill_on_job_close();
        info.limit_breakaway_ok();
        job.set_extended_limit_info(&info).map_err(fmt_err)?;
        Ok(JobGuard { job: std::mem::ManuallyDrop::new(job), leaked: false })
    }

    /// Put the current (app) process into the job so every child it creates inherits
    /// the job with no start-up race.
    ///
    /// On success the job handle is intentionally leaked: closing it (JobGuard or
    /// PtyManager dropped while the app runs) would, with KILL_ON_JOB_CLOSE, kill the
    /// app itself. The OS closes the handle at process exit, which still kills all
    /// children. On failure nothing is leaked and per-child assignment stays usable.
    pub fn assign_self(&mut self) -> Result<(), String> {
        self.job
            .assign_current_process()
            .map_err(|e| format!("assign_current_process failed: {}", fmt_err(e)))?;
        self.leaked = true;
        Ok(())
    }

    pub fn assign(&self, pid: u32) -> Result<(), String> {
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if h == 0 {
                // read immediately after the failing call
                return Err(format!("OpenProcess({pid}) failed (GetLastError={})", GetLastError()));
            }
            let r = self.job.assign_process(h).map_err(fmt_err);
            CloseHandle(h);
            r
        }
    }
}

/// Creation flag: the child is not part of the caller's job. Allowed because the job sets
/// JOB_OBJECT_LIMIT_BREAKAWAY_OK (see [`JobGuard::new`]).
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Start `cmd` outside the app's job object, so it outlives the app (the browser a document
/// link opens). Fails — e.g. ERROR_ACCESS_DENIED — when an outer job (a launcher's, an IDE's)
/// forbids breakaway; callers then fall back to an ordinary start.
pub fn spawn_outside_job(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW).spawn()
}
