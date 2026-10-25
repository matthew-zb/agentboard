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

pub struct JobGuard {
    job: Job,
}

impl JobGuard {
    pub fn new() -> Result<JobGuard, String> {
        let job = Job::create().map_err(|e| e.to_string())?;
        let mut info = job.query_extended_limit_info().map_err(|e| e.to_string())?;
        info.limit_kill_on_job_close();
        job.set_extended_limit_info(&info).map_err(|e| e.to_string())?;
        Ok(JobGuard { job })
    }

    /// Put the current (app) process into the job so every child it creates inherits
    /// the job with no start-up race. KILL_ON_JOB_CLOSE then also applies to the app
    /// itself: only call this from a process that lives as long as the job.
    pub fn assign_self(&self) -> Result<(), String> {
        self.job.assign_current_process().map_err(|e| {
            // SAFETY: read immediately after the failing call, same thread.
            format!("assign_current_process failed: {e} (GetLastError={})", unsafe { GetLastError() })
        })
    }

    pub fn assign(&self, pid: u32) -> Result<(), String> {
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if h == 0 {
                return Err(format!("OpenProcess({pid}) failed (GetLastError={})", GetLastError()));
            }
            let r = self.job.assign_process(h).map_err(|e| e.to_string());
            CloseHandle(h);
            r
        }
    }
}
