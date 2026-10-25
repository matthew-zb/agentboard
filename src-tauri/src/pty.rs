use crate::ring::RingBuffer;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub const RING_CAP: usize = 1_048_576;
pub const BATCH_MS: u64 = 16;

const DRAIN_MAX: Duration = Duration::from_millis(2000);

const DSR: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

/// Strips the first cursor-position request (ESC[6n) from the stream, even when it is
/// split across reads. Returns (bytes to forward, whether to send a reply now).
/// A trailing partial match is held in `carry` until the next chunk.
pub(crate) fn filter_dsr(carry: &mut Vec<u8>, answered: &mut bool, chunk: &[u8]) -> (Vec<u8>, bool) {
    let mut data = std::mem::take(carry);
    data.extend_from_slice(chunk);
    if *answered {
        return (data, false);
    }
    if let Some(pos) = data.windows(DSR.len()).position(|w| w == DSR) {
        data.drain(pos..pos + DSR.len());
        *answered = true;
        return (data, true);
    }
    // hold back the longest suffix that is a proper prefix of DSR
    let keep = (1..DSR.len())
        .rev()
        .find(|&k| data.len() >= k && data[data.len() - k..] == DSR[..k])
        .unwrap_or(0);
    let at = data.len() - keep;
    *carry = data.split_off(at);
    (data, false)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpawnSpec {
    pub cmd: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

pub trait PtySink: Send + Sync {
    fn output(&self, id: &str, bytes: &[u8]);
    fn exited(&self, id: &str, code: i32);
}

struct Entry {
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    ring: Arc<Mutex<RingBuffer>>,
    running: Arc<AtomicBool>,
    batch: Arc<Batch>,
    pid: Option<u32>,
}

pub struct PtyManager {
    sink: Arc<dyn PtySink>,
    panes: Mutex<HashMap<String, Arc<Entry>>>,
    #[cfg(windows)]
    job: Option<crate::winjob::JobGuard>,
    /// true when the app process itself is in the job (children inherit it)
    #[cfg(windows)]
    self_assigned: bool,
    job_error: Mutex<Option<String>>,
}

/// Bytes between the reader and the flusher. Bounded at RING_CAP: the reader blocks
/// while it is full (natural pty backpressure). `flush_lock` serializes take+output so
/// batches reach the sink in order, and `done` (checked under it) guarantees nothing is
/// delivered after the final flush.
struct Batch {
    pending: Mutex<Vec<u8>>,
    space: Condvar,
    flush_lock: Mutex<()>,
    done: AtomicBool,
    /// The single source of truth for snapshots. Appended in `flush` under `flush_lock`
    /// immediately before `sink.output`, so it always equals the delivered bytes.
    ring: Arc<Mutex<RingBuffer>>,
}

impl Batch {
    fn new(ring: Arc<Mutex<RingBuffer>>) -> Self {
        Batch {
            ring,
            pending: Mutex::new(Vec::new()),
            space: Condvar::new(),
            flush_lock: Mutex::new(()),
            done: AtomicBool::new(false),
        }
    }

    /// Reader side: waits for room; drops the bytes if the pane is already finished.
    fn push(&self, bytes: &[u8]) {
        let mut p = self.pending.lock().unwrap();
        while !p.is_empty() && p.len() + bytes.len() > RING_CAP {
            if self.done.load(Ordering::Acquire) {
                return;
            }
            p = self.space.wait_timeout(p, Duration::from_millis(50)).unwrap().0;
        }
        if !self.done.load(Ordering::Acquire) {
            p.extend_from_slice(bytes);
        }
    }

    /// Flush pending bytes; with `finish` also mark the pane done (under the lock).
    fn flush(&self, sink: &dyn PtySink, id: &str, finish: bool) {
        let _g = self.flush_lock.lock().unwrap();
        if self.done.load(Ordering::Acquire) {
            return;
        }
        let bytes = std::mem::take(&mut *self.pending.lock().unwrap());
        self.space.notify_all();
        if !bytes.is_empty() {
            self.ring.lock().unwrap().push(&bytes);
            sink.output(id, &bytes);
        }
        if finish {
            self.done.store(true, Ordering::Release);
            self.pending.lock().unwrap().clear();
            self.space.notify_all();
        }
    }
}

impl PtyManager {
    /// Production constructor: creates a KILL_ON_JOB_CLOSE job and puts the app process
    /// itself into it, so every child inherits it. Do NOT use in tests or short-lived
    /// processes: dropping the manager closes the job and kills this process too. Use
    /// [`PtyManager::new_per_child_job`] there. Failures are logged and exposed through
    /// [`PtyManager::job_error`].
    pub fn new(sink: Arc<dyn PtySink>) -> Self {
        Self::build(sink, true)
    }

    /// Like `new`, but the app process is not placed in the job; each child is assigned
    /// after it starts (tiny window in which grandchildren could escape). Safe for tests.
    pub fn new_per_child_job(sink: Arc<dyn PtySink>) -> Self {
        Self::build(sink, false)
    }

    fn build(sink: Arc<dyn PtySink>, assign_self: bool) -> Self {
        #[cfg(windows)]
        {
            let mut err = None;
            let mut self_assigned = false;
            let job = match crate::winjob::JobGuard::new() {
                Ok(mut j) => {
                    if assign_self {
                        match j.assign_self() {
                            Ok(()) => self_assigned = true,
                            Err(e) => err = Some(format!("self-assign failed, falling back to per-child: {e}")),
                        }
                    }
                    Some(j)
                }
                Err(e) => {
                    err = Some(format!("job creation failed: {e}"));
                    None
                }
            };
            if let Some(e) = &err {
                eprintln!("[agentboard] job object: {e}");
            }
            PtyManager { sink, panes: Mutex::new(HashMap::new()), job, self_assigned, job_error: Mutex::new(err) }
        }
        #[cfg(not(windows))]
        {
            let _ = assign_self;
            PtyManager { sink, panes: Mutex::new(HashMap::new()), job_error: Mutex::new(None) }
        }
    }

    /// Last job object problem (creation, self-assignment or per-child assignment).
    pub fn job_error(&self) -> Option<String> {
        self.job_error.lock().unwrap().clone()
    }

    /// Test-only: OS pid of the pane's process.
    #[doc(hidden)]
    pub fn pid(&self, id: &str) -> Option<u32> {
        self.entry(id).and_then(|e| e.pid)
    }

    /// Test-only: bytes currently queued between reader and sink.
    #[doc(hidden)]
    pub fn pending_len(&self, id: &str) -> usize {
        self.entry(id).map(|e| e.batch.pending.lock().unwrap().len()).unwrap_or(0)
    }

    pub fn spawn(&self, id: &str, spec: SpawnSpec) -> Result<(), String> {
        if self.is_running(id) {
            return Err(format!("pane {id} is already running"));
        }
        let pair = native_pty_system()
            .openpty(PtySize { rows: spec.rows, cols: spec.cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let mut cb = CommandBuilder::new(&spec.cmd);
        cb.args(&spec.args);
        cb.cwd(&spec.cwd);
        for (k, v) in &spec.env {
            cb.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cb).map_err(|e| e.to_string())?;
        drop(pair.slave);

        #[cfg(windows)]
        if !self.self_assigned {
            if let (Some(job), Some(pid)) = (&self.job, child.process_id()) {
                if let Err(e) = job.assign(pid) {
                    eprintln!("[agentboard] job assign: {e}");
                    *self.job_error.lock().unwrap() = Some(e);
                }
            }
        }

        let setup = (|| -> Result<_, String> {
            let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
            let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
            Ok((reader, writer))
        })();
        let (mut reader, writer) = match setup {
            Ok(v) => v,
            Err(e) => {
                let _ = child.kill();
                return Err(e);
            }
        };

        let ring = Arc::new(Mutex::new(RingBuffer::new(RING_CAP)));
        let running = Arc::new(AtomicBool::new(true));
        let batch = Arc::new(Batch::new(ring.clone()));
        let entry = Arc::new(Entry {
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(child.clone_killer()),
            ring: ring.clone(),
            running: running.clone(),
            batch: batch.clone(),
            pid: child.process_id(),
        });
        let (reader_done_tx, reader_done_rx) = mpsc::channel::<()>();

        // Registered only after everything above succeeded.
        self.panes.lock().unwrap().insert(id.to_string(), entry.clone());

        // reader thread
        {
            let batch = batch.clone();
            let entry = entry.clone();
            std::thread::spawn(move || {
                let _tx = reader_done_tx; // dropped on exit => waiter sees end-of-stream
                // ConPTY asks for the cursor position (ESC[6n) at startup and stalls the
                // child until answered. Answer the first request ourselves and strip it
                // from the forwarded stream so a frontend terminal does not answer again.
                let mut carry = Vec::new();
                let mut answered = false;
                let mut buf = [0u8; 8192];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let (out, reply) = filter_dsr(&mut carry, &mut answered, &buf[..n]);
                    if reply {
                        if let Some(w) = entry.writer.lock().unwrap().as_mut() {
                            let _ = w.write_all(DSR_REPLY);
                            let _ = w.flush();
                        }
                    }
                    if !out.is_empty() {
                        batch.push(&out);
                    }
                }
                if !carry.is_empty() {
                    batch.push(&carry);
                }
            });
        }
        // flush thread
        {
            let (batch, sink, id) = (batch.clone(), self.sink.clone(), id.to_string());
            std::thread::spawn(move || {
                while !batch.done.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(BATCH_MS));
                    batch.flush(sink.as_ref(), &id, false);
                }
            });
        }
        // waiter thread: exit detection relies on child.wait()
        {
            let (sink, id) = (self.sink.clone(), id.to_string());
            std::thread::spawn(move || {
                let code = match child.wait() {
                    Ok(st) => st.exit_code() as i32,
                    Err(_) => -1,
                };
                // Close the pty so the reader reaches EOF, then wait (bounded) for it.
                *entry.writer.lock().unwrap() = None;
                *entry.master.lock().unwrap() = None;
                let _ = reader_done_rx.recv_timeout(DRAIN_MAX);
                // Final flush strictly before `exited`; nothing is delivered after it.
                batch.flush(sink.as_ref(), &id, true);
                running.store(false, Ordering::Release);
                sink.exited(&id, code);
            });
        }
        Ok(())
    }

    fn entry(&self, id: &str) -> Option<Arc<Entry>> {
        self.panes.lock().unwrap().get(id).cloned()
    }

    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let e = self.entry(id).ok_or_else(|| format!("unknown pane {id}"))?;
        let mut w = e.writer.lock().unwrap();
        let w = w.as_mut().ok_or_else(|| format!("pane {id} has exited"))?;
        w.write_all(bytes).and_then(|_| w.flush()).map_err(|e| e.to_string())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let e = self.entry(id).ok_or_else(|| format!("unknown pane {id}"))?;
        let m = e.master.lock().unwrap();
        let m = m.as_ref().ok_or_else(|| format!("pane {id} has exited"))?;
        m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())
    }

    pub fn kill(&self, id: &str) {
        if let Some(e) = self.entry(id) {
            let _ = e.killer.lock().unwrap().kill();
        }
    }

    /// The pane left the model: kill its process if still running and forget it (entry and
    /// ring are freed once its threads finish). Afterwards the id is unknown (`attach_with`
    /// false, empty snapshot); `spawn` may reuse it. Unknown ids are a no-op.
    pub fn remove(&self, id: &str) {
        let Some(e) = self.panes.lock().unwrap().remove(id) else { return };
        if e.running.load(Ordering::Acquire) {
            let _ = e.killer.lock().unwrap().kill();
        }
    }

    /// Kill every pane's process (app exit). The Job Object still reaps grandchildren.
    pub fn kill_all(&self) {
        let entries: Vec<Arc<Entry>> = self.panes.lock().unwrap().values().cloned().collect();
        for e in entries {
            let _ = e.killer.lock().unwrap().kill();
        }
    }

    pub fn snapshot(&self, id: &str) -> Vec<u8> {
        self.entry(id).map(|e| e.ring.lock().unwrap().snapshot()).unwrap_or_default()
    }

    /// Atomically snapshot the pane's output and run `register` (e.g. subscribe a channel)
    /// while no flush can happen: bytes are either in the snapshot or delivered to the sink
    /// after `register` returns, never both and never neither. Returns false if unknown pane.
    pub fn attach_with(&self, id: &str, register: impl FnOnce(Vec<u8>)) -> bool {
        let Some(e) = self.entry(id) else { return false };
        let _g = e.batch.flush_lock.lock().unwrap();
        let snap = e.ring.lock().unwrap().snapshot();
        register(snap);
        true
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.entry(id).map(|e| e.running.load(Ordering::Acquire)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsr_whole_is_stripped_and_answered_once() {
        let (mut c, mut a) = (Vec::new(), false);
        let (out, reply) = filter_dsr(&mut c, &mut a, b"ab\x1b[6ncd");
        assert_eq!(out, b"abcd");
        assert!(reply);
        let (out, reply) = filter_dsr(&mut c, &mut a, b"x\x1b[6ny");
        assert_eq!(out, b"x\x1b[6ny");
        assert!(!reply);
    }

    #[test]
    fn dsr_split_across_chunks_is_matched() {
        for split in 1..4 {
            let (mut c, mut a) = (Vec::new(), false);
            let seq = b"\x1b[6n";
            let (o1, r1) = filter_dsr(&mut c, &mut a, &[b"hi".as_slice(), &seq[..split]].concat());
            assert_eq!(o1, b"hi");
            assert!(!r1);
            let (o2, r2) = filter_dsr(&mut c, &mut a, &[&seq[split..], b"!".as_slice()].concat());
            assert_eq!(o2, b"!");
            assert!(r2, "split at {split}");
        }
    }

    #[test]
    fn dsr_partial_prefix_that_is_not_dsr_is_released() {
        let (mut c, mut a) = (Vec::new(), false);
        let (o1, _) = filter_dsr(&mut c, &mut a, b"a\x1b[");
        assert_eq!(o1, b"a");
        let (o2, r2) = filter_dsr(&mut c, &mut a, b"31m");
        assert_eq!(o2, b"\x1b[31m");
        assert!(!r2);
    }
}
