#![cfg(windows)]
use app_lib::pty::{PtyManager, PtySink, SpawnSpec};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct CollectSink {
    out: Mutex<HashMap<String, Vec<u8>>>,
    exits: Mutex<HashMap<String, i32>>,
    out_at_exit: Mutex<HashMap<String, usize>>,
    output_after_exit: AtomicBool,
    /// generation tagged on each pane's events (last seen)
    out_gen: Mutex<HashMap<String, u64>>,
    exit_gen: Mutex<HashMap<String, u64>>,
    /// when true, output() blocks (simulates a stalled consumer)
    block: AtomicBool,
}
impl PtySink for CollectSink {
    fn output(&self, id: &str, gen: u64, bytes: &[u8]) {
        self.out_gen.lock().unwrap().insert(id.to_string(), gen);
        while self.block.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.exits.lock().unwrap().contains_key(id) {
            self.output_after_exit.store(true, Ordering::SeqCst);
        }
        self.out.lock().unwrap().entry(id.to_string()).or_default().extend_from_slice(bytes);
    }
    fn exited(&self, id: &str, gen: u64, code: i32) {
        self.exit_gen.lock().unwrap().insert(id.to_string(), gen);
        let n = self.out.lock().unwrap().get(id).map(|v| v.len()).unwrap_or(0);
        self.out_at_exit.lock().unwrap().insert(id.to_string(), n);
        self.exits.lock().unwrap().insert(id.to_string(), code);
    }
}
impl CollectSink {
    fn text(&self, id: &str) -> String {
        String::from_utf8_lossy(self.out.lock().unwrap().get(id).map(|v| v.as_slice()).unwrap_or(&[])).into_owned()
    }
    fn exit(&self, id: &str) -> Option<i32> {
        self.exits.lock().unwrap().get(id).copied()
    }
}

fn wait_until(mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

fn spec(cmd: &str, args: &[&str], cwd: PathBuf) -> SpawnSpec {
    SpawnSpec {
        cmd: PathBuf::from(cmd),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: vec![],
        env_remove: vec![],
        cwd,
        cols: 120,
        rows: 30,
    }
}

fn setup() -> (Arc<CollectSink>, PtyManager) {
    let sink = Arc::new(CollectSink::default());
    // per-child job: PtyManager::new would put the test process itself in the job
    let m = PtyManager::new_per_child_job(sink.clone());
    (sink, m)
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

#[test]
fn spawn_echo_streams_output_and_exit_code() {
    let (sink, m) = setup();
    m.spawn("p1", spec("cmd.exe", &["/c", "echo hello-agentboard"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("p1").is_some()), "no exit");
    assert_eq!(sink.exit("p1"), Some(0));
    assert!(sink.text("p1").contains("hello-agentboard"), "got: {:?}", sink.text("p1"));
    let at_exit = *sink.out_at_exit.lock().unwrap().get("p1").unwrap();
    assert!(at_exit > 0, "final flush must precede exited");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sink.out.lock().unwrap().get("p1").unwrap().len(), at_exit, "output after exited");
    assert!(!sink.output_after_exit.load(Ordering::SeqCst));
    assert_eq!(m.job_error(), None);
    // the startup DSR request must not be forwarded to the frontend
    assert!(!sink.out.lock().unwrap().get("p1").unwrap().windows(4).any(|w| w == b"\x1b[6n"));
    assert!(!m.snapshot("p1").windows(4).any(|w| w == b"\x1b[6n"));
    assert!(String::from_utf8_lossy(&m.snapshot("p1")).contains("hello-agentboard"));
    assert!(!m.is_running("p1"));
}

#[test]
fn env_remove_drops_inherited_variable_and_env_still_sets() {
    std::env::set_var("AGENTBOARD_T4_INHERITED", "parent-value");
    let (sink, m) = setup();
    let mut s = spec("cmd.exe", &["/c", "echo [%AGENTBOARD_T4_INHERITED%][%AGENTBOARD_T4_SET%]"], cwd());
    s.env_remove = vec!["AGENTBOARD_T4_INHERITED".into(), "AGENTBOARD_T4_SET".into()];
    s.env = vec![("AGENTBOARD_T4_SET".into(), "child-value".into())];
    m.spawn("envrm", s).unwrap();
    assert!(wait_until(|| sink.exit("envrm").is_some()), "no exit");
    let out = sink.text("envrm");
    // cmd leaves an undefined %VAR% as-is
    assert!(out.contains("[%AGENTBOARD_T4_INHERITED%][child-value]"), "got: {out:?}");
    std::env::remove_var("AGENTBOARD_T4_INHERITED");
}

#[test]
fn spawn_missing_command_errors() {
    let (sink, m) = setup();
    let r = m.spawn("p2", spec("definitely-not-a-real-cmd-xyz.exe", &[], cwd()));
    assert!(r.is_err());
    std::thread::sleep(Duration::from_millis(200));
    assert!(sink.exit("p2").is_none());
    assert!(!m.is_running("p2"));
    assert!(m.snapshot("p2").is_empty());
}

#[test]
fn exit_code_is_reported() {
    let (sink, m) = setup();
    m.spawn("p3", spec("cmd.exe", &["/c", "exit 3"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("p3").is_some()));
    assert_eq!(sink.exit("p3"), Some(3));
}

/// Each pane's input goes through its own writer thread; it ends with the process, and a
/// write or resize after the exit reports it instead of queuing forever.
#[test]
fn writer_thread_ends_when_process_exits() {
    let (sink, m) = setup();
    m.spawn("w1", spec("cmd.exe", &[], cwd())).unwrap();
    assert!(m.writer_alive("w1"), "a running pane has a writer");
    m.spawn("w2", spec("cmd.exe", &["/c", "exit 0"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("w2").is_some()));
    assert!(wait_until(|| !m.writer_alive("w2")), "writer thread still running after the exit");
    let err = m.write("w2", b"x").unwrap_err();
    assert!(err.contains("has exited"), "{err}");
    assert!(m.resize("w2", 100, 30).unwrap_err().contains("has exited"));
    assert!(!m.writer_alive("unknown-id"));
    m.kill("w1");
    assert!(wait_until(|| !m.writer_alive("w1")));
}

#[test]
fn write_reaches_process() {
    let (sink, m) = setup();
    m.spawn("p4", spec("cmd.exe", &[], cwd())).unwrap();
    assert!(m.is_running("p4"));
    m.write("p4", b"echo typed-xyz\r\n").unwrap();
    assert!(
        wait_until(|| sink.text("p4").matches("typed-xyz").count() >= 2),
        "got: {:?}",
        sink.text("p4")
    );
    m.resize("p4", 100, 40).unwrap();
    m.kill("p4");
    assert!(wait_until(|| !m.is_running("p4")));
}

#[test]
fn spawn_in_korean_dir() {
    let (sink, m) = setup();
    let td = tempfile::tempdir().unwrap();
    let dir = td.path().join("내 프로젝트");
    std::fs::create_dir(&dir).unwrap();
    m.spawn("p5", spec("cmd.exe", &["/c", "cd"], dir)).unwrap();
    assert!(wait_until(|| sink.exit("p5").is_some()));
    assert!(sink.text("p5").contains("내 프로젝트"), "got: {:?}", sink.text("p5"));
}

#[test]
fn kill_terminates_and_snapshot_survives() {
    let (sink, m) = setup();
    m.spawn("p6", spec("ping", &["-n", "30", "127.0.0.1"], cwd())).unwrap();
    assert!(wait_until(|| !m.snapshot("p6").is_empty()), "no output");
    m.kill("p6");
    assert!(wait_until(|| !m.is_running("p6")));
    assert!(!m.snapshot("p6").is_empty());
    assert!(wait_until(|| sink.exit("p6").is_some()));
    m.kill("unknown-id");
}

#[test]
fn pending_is_bounded_when_sink_stalls() {
    let (sink, m) = setup();
    let td = tempfile::tempdir().unwrap();
    let big = td.path().join("big.txt");
    let line = "y".repeat(78) + "\r\n";
    std::fs::write(&big, line.repeat(20_000)).unwrap(); // ~1.6MB, more than the 1MB cap
    sink.block.store(true, Ordering::Release);
    m.spawn("p7", spec("cmd.exe", &["/c", "type", big.to_str().unwrap()], cwd())).unwrap();
    // reader should fill pending up to (but never beyond) the cap and then block
    assert!(wait_until(|| m.pending_len("p7") > 500_000), "pending never filled");
    let mut max = 0;
    for _ in 0..30 {
        max = max.max(m.pending_len("p7"));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(max <= app_lib::pty::RING_CAP, "pending {max} exceeded cap");
    sink.block.store(false, Ordering::Release);
    assert!(wait_until(|| sink.exit("p7").is_some()));
    assert!(sink.text("p7").len() > 1_400_000);
}

// ---- production path (PtyManager::new, self-assigned job) via re-exec ----

struct Quiet;
impl PtySink for Quiet {
    fn output(&self, _: &str, _: u64, _: &[u8]) {}
    fn exited(&self, _: &str, _: u64, _: i32) {}
}

/// Helper, only active when re-executed by `job_kills_children_when_app_exits`.
#[test]
fn job_child_helper() {
    if std::env::var("AGENTBOARD_JOB_CHILD").is_err() {
        return;
    }
    let m = PtyManager::new(Arc::new(Quiet));
    m.spawn("j", spec("ping", &["-n", "60", "127.0.0.1"], cwd())).unwrap();
    println!("CHILDPID={}", m.pid("j").unwrap());
    // dropping the manager must NOT kill us (handle is leaked on purpose)
    drop(m);
    std::thread::sleep(Duration::from_millis(200));
    println!("ALIVE_AFTER_DROP");
    // exit without running destructors: only the OS-closed job handle can kill ping
    std::process::exit(0);
}

fn pid_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

#[test]
fn job_kills_children_when_app_exits() {
    if std::env::var("AGENTBOARD_JOB_CHILD").is_ok() {
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "job_child_helper", "--nocapture", "--test-threads=1"])
        .env("AGENTBOARD_JOB_CHILD", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("ALIVE_AFTER_DROP"), "helper output: {text}");
    let pid: u32 = text
        .split("CHILDPID=")
        .nth(1)
        .expect("pid line")
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(wait_until(|| !pid_alive(pid)), "ping {pid} survived app exit");
}

#[test]
fn snapshot_equals_delivered_output() {
    let (sink, m) = setup();
    m.spawn("s1", spec("cmd.exe", &["/c", "echo snap-one & echo snap-two"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("s1").is_some()));
    let delivered = sink.out.lock().unwrap().get("s1").cloned().unwrap();
    assert_eq!(m.snapshot("s1"), delivered);
}

#[test]
fn attach_with_has_no_gap_or_duplicate_under_concurrent_output() {
    let (sink, m) = setup();
    m.spawn(
        "a1",
        spec("cmd.exe", &["/c", "for /L %i in (1,1,6000) do @echo line-%i-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"], cwd()),
    )
    .unwrap();
    // attach as soon as some output exists, while output keeps flowing
    assert!(wait_until(|| sink.out.lock().unwrap().get("a1").map(|v| !v.is_empty()).unwrap_or(false)));
    let mut captured: Option<(Vec<u8>, usize)> = None;
    assert!(m.attach_with("a1", |snap| {
        // runs under the flush lock: nothing can be delivered meanwhile
        let n = sink.out.lock().unwrap().get("a1").map(|v| v.len()).unwrap_or(0);
        captured = Some((snap, n));
    }));
    let (snap, n) = captured.unwrap();
    assert_eq!(snap.len(), n, "snapshot must equal everything delivered so far");
    assert!(wait_until(|| sink.exit("a1").is_some()));
    let delivered = sink.out.lock().unwrap().get("a1").cloned().unwrap();
    assert!(delivered.len() > n, "test needs output after attach");
    assert!(delivered.len() < 1_048_576, "ring must not wrap for this comparison");
    assert_eq!(&delivered[..n], snap.as_slice());
    let mut replay = snap.clone();
    replay.extend_from_slice(&delivered[n..]);
    assert_eq!(replay, m.snapshot("a1"));
    assert!(!m.attach_with("nope", |_| panic!("must not run")));
}

#[test]
fn respawn_starts_with_empty_ring() {
    let (sink, m) = setup();
    m.spawn("r1", spec("cmd.exe", &["/c", "echo first-run-marker"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("r1").is_some()));
    assert!(String::from_utf8_lossy(&m.snapshot("r1")).contains("first-run-marker"));
    sink.exits.lock().unwrap().clear();
    m.spawn("r1", spec("cmd.exe", &["/c", "echo second-run-marker"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("r1").is_some()));
    let snap = String::from_utf8_lossy(&m.snapshot("r1")).into_owned();
    assert!(snap.contains("second-run-marker"));
    assert!(!snap.contains("first-run-marker"), "old process output leaked: {snap:?}");
}

#[test]
fn remove_kills_and_forgets_the_pane() {
    let (sink, m) = setup();
    m.spawn("rm1", spec("ping", &["-n", "30", "127.0.0.1"], cwd())).unwrap();
    assert!(wait_until(|| !m.snapshot("rm1").is_empty()), "no output");
    let pid = m.pid("rm1").unwrap();
    m.remove("rm1");
    assert!(!m.is_running("rm1"));
    assert!(m.snapshot("rm1").is_empty());
    assert!(m.pid("rm1").is_none());
    assert!(!m.attach_with("rm1", |_| panic!("must not run")));
    assert!(m.write("rm1", b"x").is_err());
    assert!(wait_until(|| sink.exit("rm1").is_some()), "removed process was not killed");
    assert!(wait_until(|| !pid_alive(pid)));
    // unknown / already removed ids are a no-op
    m.remove("rm1");
    m.remove("never-existed");
    // an exited pane can be removed too, and the id can be spawned again afterwards
    m.spawn("rm2", spec("cmd.exe", &["/c", "echo old-marker"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("rm2").is_some()));
    m.remove("rm2");
    assert!(m.snapshot("rm2").is_empty());
    sink.exits.lock().unwrap().clear();
    m.spawn("rm2", spec("cmd.exe", &["/c", "echo new-marker"], cwd())).unwrap();
    assert!(wait_until(|| sink.exit("rm2").is_some()));
    let snap = String::from_utf8_lossy(&m.snapshot("rm2")).into_owned();
    assert!(snap.contains("new-marker") && !snap.contains("old-marker"), "{snap:?}");
}

#[test]
fn spawn_generations_tag_events_and_increase() {
    let (sink, m) = setup();
    assert_eq!(m.current_gen("g1"), None);
    let g1 = m.spawn("g1", spec("cmd.exe", &["/c", "echo gen-one"], cwd())).unwrap();
    assert_eq!(m.current_gen("g1"), Some(g1));
    assert!(wait_until(|| sink.exit("g1").is_some()));
    assert_eq!(sink.out_gen.lock().unwrap().get("g1"), Some(&g1));
    assert_eq!(sink.exit_gen.lock().unwrap().get("g1"), Some(&g1));
    // an exited pane keeps its generation until respawned/removed
    assert_eq!(m.current_gen("g1"), Some(g1));

    sink.exits.lock().unwrap().clear();
    let g2 = m.spawn("g1", spec("cmd.exe", &["/c", "echo gen-two"], cwd())).unwrap();
    assert!(g2 > g1, "generations increase: {g1} -> {g2}");
    assert!(wait_until(|| sink.exit("g1").is_some()));
    assert_eq!(sink.exit_gen.lock().unwrap().get("g1"), Some(&g2));
    // global counter: another id never reuses a generation
    let g3 = m.spawn("g2", spec("cmd.exe", &["/c", "exit 0"], cwd())).unwrap();
    assert!(g3 > g2);
    m.remove("g1");
    assert_eq!(m.current_gen("g1"), None);
}

#[test]
fn failed_spawn_leaves_no_generation() {
    let (_sink, m) = setup();
    let g1 = m.spawn("f1", spec("cmd.exe", &["/c", "exit 0"], cwd())).unwrap();
    assert!(wait_until(|| !m.is_running("f1")));
    assert!(m.spawn("f1", spec("definitely-not-a-real-cmd-xyz.exe", &[], cwd())).is_err());
    // the old generation is retired too: its late events are stale
    assert_ne!(m.current_gen("f1"), Some(g1));
    assert_eq!(m.current_gen("f1"), None);
    assert!(m.spawn("f1", spec("cmd.exe", &["/c", "exit 0"], cwd())).is_ok());
}

#[test]
fn concurrent_spawns_of_one_id_admit_exactly_one() {
    let (_sink, m) = setup();
    let m = Arc::new(m);
    for round in 0..5 {
        let id = format!("c{round}");
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (m, b, id) = (m.clone(), barrier.clone(), id.clone());
                std::thread::spawn(move || {
                    b.wait();
                    m.spawn(&id, spec("ping", &["-n", "30", "127.0.0.1"], cwd()))
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let ok: Vec<u64> = results.iter().filter_map(|r| r.as_ref().ok().copied()).collect();
        assert_eq!(ok.len(), 1, "round {round}: {results:?}");
        assert_eq!(m.current_gen(&id), Some(ok[0]));
        let pid = m.pid(&id).unwrap();
        m.remove(&id);
        assert!(wait_until(|| !pid_alive(pid)), "winner process must be the only one and killable");
    }
}
