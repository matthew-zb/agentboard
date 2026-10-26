use crate::ring::RingBuffer;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Reader side of [`filter_dsr`]: the reply to ConPTY's first cursor-position request goes into
/// the pane's input queue, so the writer thread stays the only one writing to the PTY.
pub(crate) fn filter_dsr_reply(carry: &mut Vec<u8>, answered: &mut bool, chunk: &[u8], input: &InputQueue) -> Vec<u8> {
    let (out, reply) = filter_dsr(carry, answered, chunk);
    if reply {
        let _ = input.push(Op::Write(DSR_REPLY.to_vec())); // closed = the process is gone anyway
    }
    out
}

// ───────── input queue ─────────
//
// `write`/`resize` are called from the UI thread (sync Tauri commands, so IPC order is call
// order), the bridge and the prompt timer. They only queue; each pane's writer thread applies
// the queue in order, so a process that is slow to take its input (a large paste, a busy
// conhost) never blocks the caller.

/// Most bytes waiting in one pane's queue: a large paste fits; a process that stops reading
/// its input makes `write` fail instead of growing memory.
pub const INPUT_CAP: usize = 8 * 1024 * 1024;
/// Bytes written per writer-lock hold: the waiter (dropping the writer at exit) never waits
/// behind more than one chunk.
const WRITE_CHUNK: usize = 16 * 1024;

/// One input operation, applied by the pane's writer thread in call order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    Write(Vec<u8>),
    Resize(u16, u16),
}

/// Where a writer thread applies input: the pane's PTY ([`PtyInput`]) or a recorder in tests.
pub(crate) trait InputSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String>;
}

/// Why [`InputQueue::push`] refused.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PushError {
    /// The process has exited (the waiter closed the queue).
    Closed,
    /// A write failed; the queue is closed and keeps reporting this.
    Failed(String),
    /// More than [`INPUT_CAP`] bytes are waiting.
    Full,
}

impl PushError {
    /// The error text for pane `id` (shown by the frontend as "[입력 전달 실패: …]").
    pub(crate) fn message(&self, id: &str) -> String {
        match self {
            PushError::Closed => format!("pane {id} has exited"),
            PushError::Failed(e) => format!("창 {id}에 입력을 쓰지 못했습니다 — {e}"),
            PushError::Full => {
                format!("창 {id}의 입력 대기열이 가득 찼습니다(최대 {} MiB) — 프로세스가 입력을 읽지 않습니다", INPUT_CAP / 1024 / 1024)
            }
        }
    }
}

#[derive(Default)]
struct QueueState {
    ops: VecDeque<Op>,
    /// Bytes of the queued (not yet taken) writes.
    bytes: usize,
    closed: bool,
    failed: Option<String>,
}

/// A pane's pending input. Its lock is a leaf: `push` never waits on the process, and the
/// writer thread releases it before touching the PTY.
pub(crate) struct InputQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl InputQueue {
    pub(crate) fn new() -> Self {
        InputQueue { state: Mutex::new(QueueState::default()), ready: Condvar::new() }
    }

    /// Queue `op` and return at once. Back-to-back resizes collapse into the last one.
    pub(crate) fn push(&self, op: Op) -> Result<(), PushError> {
        let mut s = self.state.lock().unwrap();
        if let Some(e) = &s.failed {
            return Err(PushError::Failed(e.clone()));
        }
        if s.closed {
            return Err(PushError::Closed);
        }
        match op {
            Op::Write(bytes) => {
                if bytes.is_empty() {
                    return Ok(());
                }
                if s.bytes + bytes.len() > INPUT_CAP {
                    return Err(PushError::Full);
                }
                s.bytes += bytes.len();
                s.ops.push_back(Op::Write(bytes));
            }
            Op::Resize(cols, rows) => {
                if matches!(s.ops.back(), Some(Op::Resize(..))) {
                    s.ops.pop_back();
                }
                s.ops.push_back(Op::Resize(cols, rows));
            }
        }
        self.ready.notify_one();
        Ok(())
    }

    /// No more input: the writer drains what is queued and ends.
    pub(crate) fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn pending_bytes(&self) -> usize {
        self.state.lock().unwrap().bytes
    }

    /// The writer loop: applies ops in order, with the queue lock released while `sink` works.
    /// Returns once the queue is closed and drained, or at the first failed write (the queue
    /// is then closed and later pushes report the failure — unless the process had already
    /// exited, which stays `Closed`). A failed resize is only logged.
    pub(crate) fn run(&self, sink: &mut dyn InputSink) {
        loop {
            let op = {
                let mut s = self.state.lock().unwrap();
                loop {
                    if let Some(op) = s.ops.pop_front() {
                        if let Op::Write(b) = &op {
                            s.bytes -= b.len();
                        }
                        break op;
                    }
                    if s.closed {
                        return;
                    }
                    s = self.ready.wait(s).unwrap();
                }
            };
            match op {
                Op::Write(bytes) => {
                    if let Err(e) = sink.write(&bytes) {
                        let mut s = self.state.lock().unwrap();
                        if !s.closed {
                            s.failed = Some(e);
                        }
                        s.closed = true;
                        s.ops.clear();
                        s.bytes = 0;
                        return;
                    }
                }
                Op::Resize(cols, rows) => {
                    if let Err(e) = sink.resize(cols, rows) {
                        eprintln!("[agentboard] resize: {e}");
                    }
                }
            }
        }
    }
}

/// The PTY end of a pane's writer thread. The writer lock is held per chunk only.
struct PtyInput {
    entry: Arc<Entry>,
}

impl InputSink for PtyInput {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        for chunk in bytes.chunks(WRITE_CHUNK) {
            let mut w = self.entry.writer.lock().unwrap();
            let w = w.as_mut().ok_or_else(|| "the process has exited".to_string())?;
            w.write_all(chunk).and_then(|_| w.flush()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        let m = self.entry.master.lock().unwrap();
        let m = m.as_ref().ok_or_else(|| "the process has exited".to_string())?;
        m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpawnSpec {
    pub cmd: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Inherited variables to drop before `env` is applied (role panes: `CLAUDECODE`,
    /// `CLAUDE_CODE_*`, R18). Names set in `env` are set again afterwards.
    pub env_remove: Vec<String>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
}

/// Receives PTY events. `gen` is the generation returned by the `spawn` that started the
/// process; a sink should ignore events whose `gen` differs from
/// [`PtyManager::current_gen`] (they belong to an earlier process of the same id).
pub trait PtySink: Send + Sync {
    fn output(&self, id: &str, gen: u64, bytes: &[u8]);
    fn exited(&self, id: &str, gen: u64, code: i32);
}

/// A pane id's registration. `Starting` is the placeholder that makes spawn's
/// "already running?" check and the registration one atomic step under the `panes` lock.
enum Slot {
    Starting(u64),
    Live(Arc<Entry>),
}

impl Slot {
    fn gen(&self) -> u64 {
        match self {
            Slot::Starting(g) => *g,
            Slot::Live(e) => e.gen,
        }
    }
}

struct Entry {
    gen: u64,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    ring: Arc<Mutex<RingBuffer>>,
    running: Arc<AtomicBool>,
    batch: Arc<Batch>,
    pid: Option<u32>,
    /// Pending input; applied by this entry's writer thread (the only PTY writer).
    input: Arc<InputQueue>,
    /// Set when the writer thread has ended (process exited or a write failed).
    writer_done: AtomicBool,
}

pub struct PtyManager {
    sink: Arc<dyn PtySink>,
    panes: Mutex<HashMap<String, Slot>>,
    /// Last issued generation. Global to the manager (not per id), starts at 1, never reused:
    /// a generation identifies exactly one spawn attempt.
    next_gen: AtomicU64,
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
    fn flush(&self, sink: &dyn PtySink, id: &str, gen: u64, finish: bool) {
        let _g = self.flush_lock.lock().unwrap();
        if self.done.load(Ordering::Acquire) {
            return;
        }
        let bytes = std::mem::take(&mut *self.pending.lock().unwrap());
        self.space.notify_all();
        if !bytes.is_empty() {
            self.ring.lock().unwrap().push(&bytes);
            sink.output(id, gen, &bytes);
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
            PtyManager {
                sink,
                panes: Mutex::new(HashMap::new()),
                next_gen: AtomicU64::new(0),
                job,
                self_assigned,
                job_error: Mutex::new(err),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = assign_self;
            PtyManager { sink, panes: Mutex::new(HashMap::new()), next_gen: AtomicU64::new(0), job_error: Mutex::new(None) }
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

    /// Test-only: the current entry's writer thread is still running.
    #[doc(hidden)]
    pub fn writer_alive(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|e| !e.writer_done.load(Ordering::Acquire))
    }

    /// Test-only: bytes currently queued between reader and sink.
    #[doc(hidden)]
    pub fn pending_len(&self, id: &str) -> usize {
        self.entry(id).map(|e| e.batch.pending.lock().unwrap().len()).unwrap_or(0)
    }

    /// Start a process for `id` and return its generation (see [`PtySink`]). Fails while a
    /// process for `id` is running or being started. The check and the registration of a
    /// `Starting` placeholder are one step under the `panes` lock, so concurrent spawns of the
    /// same id cannot both pass; the placeholder also makes the previous generation stale at
    /// once. On failure the id is left unregistered (the previous entry and ring are gone too).
    pub fn spawn(&self, id: &str, spec: SpawnSpec) -> Result<u64, String> {
        let gen = {
            let mut panes = self.panes.lock().unwrap();
            match panes.get(id) {
                Some(Slot::Starting(_)) => return Err(format!("pane {id} is already starting")),
                Some(Slot::Live(e)) if e.running.load(Ordering::Acquire) => {
                    return Err(format!("pane {id} is already running"))
                }
                _ => {}
            }
            let gen = self.next_gen.fetch_add(1, Ordering::SeqCst) + 1;
            panes.insert(id.to_string(), Slot::Starting(gen));
            gen
        };
        let r = self.spawn_inner(id, gen, spec);
        if r.is_err() {
            let mut panes = self.panes.lock().unwrap();
            if matches!(panes.get(id), Some(Slot::Starting(g)) if *g == gen) {
                panes.remove(id);
            }
        }
        r.map(|()| gen)
    }

    fn spawn_inner(&self, id: &str, gen: u64, spec: SpawnSpec) -> Result<(), String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows: spec.rows, cols: spec.cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let mut cb = CommandBuilder::new(&spec.cmd);
        cb.args(&spec.args);
        cb.cwd(&spec.cwd);
        for k in &spec.env_remove {
            cb.env_remove(k);
        }
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
            gen,
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(child.clone_killer()),
            ring: ring.clone(),
            running: running.clone(),
            batch: batch.clone(),
            pid: child.process_id(),
            input: Arc::new(InputQueue::new()),
            writer_done: AtomicBool::new(false),
        });
        let (reader_done_tx, reader_done_rx) = mpsc::channel::<()>();

        // Registered only after everything above succeeded, and only while our placeholder is
        // still there (a `remove` during the start means the pane is gone: undo the process).
        {
            let mut panes = self.panes.lock().unwrap();
            if !matches!(panes.get(id), Some(Slot::Starting(g)) if *g == gen) {
                drop(panes);
                let _ = child.kill();
                return Err(format!("pane {id} was removed while starting"));
            }
            panes.insert(id.to_string(), Slot::Live(entry.clone()));
        }

        // writer thread: applies the input queue (writes, resizes, the DSR reply) in call order
        {
            let entry = entry.clone();
            std::thread::spawn(move || {
                let input = entry.input.clone();
                input.run(&mut PtyInput { entry: entry.clone() });
                entry.writer_done.store(true, Ordering::Release);
            });
        }
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
                    let out = filter_dsr_reply(&mut carry, &mut answered, &buf[..n], &entry.input);
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
                    batch.flush(sink.as_ref(), &id, gen, false);
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
                // No more input (the writer thread drains and ends), then close the pty so the
                // reader reaches EOF, and wait (bounded) for it. The writer holds the writer lock
                // for one chunk at most.
                entry.input.close();
                *entry.writer.lock().unwrap() = None;
                *entry.master.lock().unwrap() = None;
                let _ = reader_done_rx.recv_timeout(DRAIN_MAX);
                // Final flush strictly before `exited`; nothing is delivered after it.
                batch.flush(sink.as_ref(), &id, gen, true);
                running.store(false, Ordering::Release);
                sink.exited(&id, gen, code);
            });
        }
        Ok(())
    }

    /// The current generation's entry (none while it is still starting).
    fn entry(&self, id: &str) -> Option<Arc<Entry>> {
        match self.panes.lock().unwrap().get(id) {
            Some(Slot::Live(e)) => Some(e.clone()),
            _ => None,
        }
    }

    /// Generation of the id's current registration (starting or started, running or exited);
    /// `None` when unknown, removed, or its last spawn failed. Takes only the `panes` lock.
    pub fn current_gen(&self, id: &str) -> Option<u64> {
        self.panes.lock().unwrap().get(id).map(Slot::gen)
    }

    /// Queue `bytes` for the pane's process and return at once; its writer thread writes them
    /// in call order. `Ok` means queued, not delivered. Errors: unknown pane, exited process,
    /// an earlier failed write, or more than [`INPUT_CAP`] bytes waiting.
    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let e = self.entry(id).ok_or_else(|| format!("unknown pane {id}"))?;
        e.input.push(Op::Write(bytes.to_vec())).map_err(|err| err.message(id))
    }

    /// Queue a PTY resize behind the pane's pending input (back-to-back resizes collapse).
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let e = self.entry(id).ok_or_else(|| format!("unknown pane {id}"))?;
        e.input.push(Op::Resize(cols, rows)).map_err(|err| err.message(id))
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
        // a `Starting` placeholder is just dropped: that spawn sees it gone and kills its child
        let Some(Slot::Live(e)) = self.panes.lock().unwrap().remove(id) else { return };
        if e.running.load(Ordering::Acquire) {
            let _ = e.killer.lock().unwrap().kill();
        }
    }

    /// Kill every pane's process (app exit). The Job Object still reaps grandchildren.
    pub fn kill_all(&self) {
        let entries: Vec<Arc<Entry>> = self
            .panes
            .lock()
            .unwrap()
            .values()
            .filter_map(|s| match s {
                Slot::Live(e) => Some(e.clone()),
                Slot::Starting(_) => None,
            })
            .collect();
        for e in entries {
            let _ = e.killer.lock().unwrap().kill();
        }
    }

    pub fn snapshot(&self, id: &str) -> Vec<u8> {
        self.entry(id).map(|e| e.ring.lock().unwrap().snapshot()).unwrap_or_default()
    }

    /// Atomically snapshot the pane's output and run `register` (e.g. subscribe a channel)
    /// while no flush can happen: bytes are either in the snapshot or delivered to the sink
    /// after `register` returns, never both and never neither. Attaches to the current
    /// generation only: if a newer spawn replaced the entry between lookup and lock, it retries
    /// on the new one. Returns false if the pane is unknown or still starting.
    pub fn attach_with(&self, id: &str, register: impl FnOnce(Vec<u8>)) -> bool {
        loop {
            let Some(e) = self.entry(id) else { return false };
            let _g = e.batch.flush_lock.lock().unwrap();
            if self.current_gen(id) != Some(e.gen) {
                continue; // replaced meanwhile (flush_lock -> panes is an allowed nesting)
            }
            let snap = e.ring.lock().unwrap().snapshot();
            register(snap);
            return true;
        }
    }

    /// True while the current generation's process runs, and while a spawn is starting one.
    pub fn is_running(&self, id: &str) -> bool {
        match self.panes.lock().unwrap().get(id) {
            Some(Slot::Starting(_)) => true,
            Some(Slot::Live(e)) => e.running.load(Ordering::Acquire),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    // ---- input queue (writes and resizes never block the caller) ----

    /// Records what the writer applied; optionally fails every write.
    #[derive(Default)]
    struct Recorder {
        ops: Vec<Op>,
        fail_writes: bool,
    }

    impl InputSink for Recorder {
        fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            if self.fail_writes {
                return Err("pipe broken".into());
            }
            self.ops.push(Op::Write(bytes.to_vec()));
            Ok(())
        }
        fn resize(&mut self, cols: u16, rows: u16) -> Result<(), String> {
            self.ops.push(Op::Resize(cols, rows));
            Ok(())
        }
    }

    fn drain(q: &InputQueue) -> Vec<Op> {
        q.close();
        let mut r = Recorder::default();
        q.run(&mut r);
        r.ops
    }

    #[test]
    fn input_queue_keeps_order_of_writes_and_resizes() {
        let q = InputQueue::new();
        let w1 = vec![0x1b, b'[', b'M', 0x20, 0xA1, 0xFF]; // bytes ≥0x80 pass untouched
        for op in [
            Op::Write(w1.clone()),
            Op::Resize(80, 24),
            Op::Write(b"two".to_vec()),
            Op::Resize(100, 30),
            Op::Resize(120, 40),
            Op::Write(b"three".to_vec()),
        ] {
            q.push(op).unwrap();
        }
        let want = vec![Op::Write(w1), Op::Resize(80, 24), Op::Write(b"two".to_vec()), Op::Resize(120, 40), Op::Write(b"three".to_vec())];
        assert_eq!(drain(&q), want, "in call order; back-to-back resizes collapse into the last");
    }

    #[test]
    fn enqueue_returns_while_writer_is_blocked() {
        struct Stuck(mpsc::Receiver<()>);
        impl InputSink for Stuck {
            fn write(&mut self, _: &[u8]) -> Result<(), String> {
                let _ = self.0.recv(); // blocks until released (or the sender is dropped)
                Ok(())
            }
            fn resize(&mut self, _: u16, _: u16) -> Result<(), String> {
                Ok(())
            }
        }
        let q = Arc::new(InputQueue::new());
        let (release, rx) = mpsc::channel();
        let writer = {
            let q = q.clone();
            std::thread::spawn(move || q.run(&mut Stuck(rx)))
        };
        q.push(Op::Write(b"first".to_vec())).unwrap();
        // the writer has taken "first" and is stuck inside the sink
        let end = Instant::now() + Duration::from_secs(5);
        while q.pending_bytes() > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(q.pending_bytes(), 0, "writer never took the first write");
        let t = Instant::now();
        q.push(Op::Write(vec![b'x'; 1 << 20])).unwrap();
        q.push(Op::Resize(100, 30)).unwrap();
        let took = t.elapsed();
        assert!(took < Duration::from_millis(50), "enqueue blocked for {took:?}");
        assert_eq!(q.pending_bytes(), 1 << 20);
        q.close();
        drop(release); // unblock: the writer drains and ends
        writer.join().unwrap();
    }

    #[test]
    fn input_queue_rejects_over_cap_and_after_write_failure() {
        let q = InputQueue::new();
        q.push(Op::Write(vec![0; INPUT_CAP])).unwrap();
        assert_eq!(q.push(Op::Write(vec![0])), Err(PushError::Full));
        q.push(Op::Resize(80, 24)).unwrap(); // a resize carries no bytes
        // a failed write closes the queue; later pushes report that failure
        let q = InputQueue::new();
        q.push(Op::Write(b"a".to_vec())).unwrap();
        q.push(Op::Write(b"b".to_vec())).unwrap();
        let mut r = Recorder { fail_writes: true, ..Default::default() };
        q.run(&mut r); // returns on the failure, though nobody closed the queue
        assert_eq!(q.push(Op::Write(b"c".to_vec())), Err(PushError::Failed("pipe broken".into())));
        assert_eq!(q.push(Op::Resize(1, 1)), Err(PushError::Failed("pipe broken".into())));
        // a closed queue refuses new input
        let q = InputQueue::new();
        q.close();
        assert_eq!(q.push(Op::Write(b"x".to_vec())), Err(PushError::Closed));
        assert_eq!(PushError::Closed.message("p1"), "pane p1 has exited");
    }

    #[test]
    fn dsr_reply_goes_through_the_queue() {
        let q = InputQueue::new();
        let (mut c, mut a) = (Vec::new(), false);
        assert_eq!(filter_dsr_reply(&mut c, &mut a, b"a\x1b[6nb", &q), b"ab");
        assert_eq!(filter_dsr_reply(&mut c, &mut a, b"\x1b[6n", &q), b"\x1b[6n", "only the first request is answered");
        assert_eq!(drain(&q), vec![Op::Write(DSR_REPLY.to_vec())]);
    }
}
