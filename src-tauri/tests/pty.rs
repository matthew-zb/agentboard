#![cfg(windows)]
use app_lib::pty::{PtyManager, PtySink, SpawnSpec};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct CollectSink {
    out: Mutex<HashMap<String, Vec<u8>>>,
    exits: Mutex<HashMap<String, i32>>,
    out_at_exit: Mutex<HashMap<String, usize>>,
}
impl PtySink for CollectSink {
    fn output(&self, id: &str, bytes: &[u8]) {
        self.out.lock().unwrap().entry(id.to_string()).or_default().extend_from_slice(bytes);
    }
    fn exited(&self, id: &str, code: i32) {
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
        cwd,
        cols: 120,
        rows: 30,
    }
}

fn setup() -> (Arc<CollectSink>, PtyManager) {
    let sink = Arc::new(CollectSink::default());
    let m = PtyManager::new(sink.clone());
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
    assert!(String::from_utf8_lossy(&m.snapshot("p1")).contains("hello-agentboard"));
    assert!(!m.is_running("p1"));
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
