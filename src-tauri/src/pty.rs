use crate::ring::RingBuffer;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const RING_CAP: usize = 1_048_576;
pub const BATCH_MS: u64 = 16;

/// After the child exits, wait until the reader has been quiet this long
/// (ConPTY may deliver trailing output slightly late), but never longer than DRAIN_MAX.
const DRAIN_QUIET: Duration = Duration::from_millis(100);
const DRAIN_MAX: Duration = Duration::from_millis(1000);

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
}

pub struct PtyManager {
    sink: Arc<dyn PtySink>,
    panes: Mutex<HashMap<String, Arc<Entry>>>,
    #[cfg(windows)]
    job: Option<crate::winjob::JobGuard>,
}

/// Pending (not yet flushed) bytes. `flush_lock` serializes take+output so
/// batches reach the sink in order even when two threads flush.
struct Batch {
    pending: Mutex<Vec<u8>>,
    flush_lock: Mutex<()>,
}

impl Batch {
    fn flush(&self, sink: &dyn PtySink, id: &str) {
        let _g = self.flush_lock.lock().unwrap();
        let bytes = std::mem::take(&mut *self.pending.lock().unwrap());
        if !bytes.is_empty() {
            sink.output(id, &bytes);
        }
    }
}

impl PtyManager {
    pub fn new(sink: Arc<dyn PtySink>) -> Self {
        PtyManager {
            sink,
            panes: Mutex::new(HashMap::new()),
            #[cfg(windows)]
            job: crate::winjob::JobGuard::new().ok(),
        }
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
        if let (Some(job), Some(pid)) = (&self.job, child.process_id()) {
            let _ = job.assign(pid);
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
        let entry = Arc::new(Entry {
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(child.clone_killer()),
            ring: ring.clone(),
            running: running.clone(),
        });
        let batch = Arc::new(Batch { pending: Mutex::new(Vec::new()), flush_lock: Mutex::new(()) });
        let last_read = Arc::new(Mutex::new(Instant::now()));
        let done = Arc::new(AtomicBool::new(false));

        // Registered only after everything above succeeded.
        self.panes.lock().unwrap().insert(id.to_string(), entry.clone());

        // reader thread
        {
            let (ring, batch, last_read) = (ring.clone(), batch.clone(), last_read.clone());
            let entry = entry.clone();
            std::thread::spawn(move || {
                // ConPTY asks for the cursor position (ESC[6n) at startup and stalls the
                // child until answered. Answer the first request ourselves so panes start
                // even before a frontend terminal is attached.
                let mut answered_dsr = false;
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if !answered_dsr && buf[..n].windows(4).any(|w| w == b"[6n") {
                                answered_dsr = true;
                                if let Some(w) = entry.writer.lock().unwrap().as_mut() {
                                    let _ = w.write_all(b"[1;1R");
                                    let _ = w.flush();
                                }
                            }
                            ring.lock().unwrap().push(&buf[..n]);
                            batch.pending.lock().unwrap().extend_from_slice(&buf[..n]);
                            *last_read.lock().unwrap() = Instant::now();
                        }
                    }
                }
            });
        }
        // flush thread
        {
            let (batch, done, sink, id) = (batch.clone(), done.clone(), self.sink.clone(), id.to_string());
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(BATCH_MS));
                    batch.flush(sink.as_ref(), &id);
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
                let start = Instant::now();
                while start.elapsed() < DRAIN_MAX && last_read.lock().unwrap().elapsed() < DRAIN_QUIET {
                    std::thread::sleep(Duration::from_millis(10));
                }
                batch.flush(sink.as_ref(), &id);
                running.store(false, Ordering::Release);
                done.store(true, Ordering::Release);
                // Close the pty so the reader unblocks; the ring stays in the entry.
                *entry.writer.lock().unwrap() = None;
                *entry.master.lock().unwrap() = None;
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

    pub fn snapshot(&self, id: &str) -> Vec<u8> {
        self.entry(id).map(|e| e.ring.lock().unwrap().snapshot()).unwrap_or_default()
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.entry(id).map(|e| e.running.load(Ordering::Acquire)).unwrap_or(false)
    }
}
