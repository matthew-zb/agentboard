//! Real-session check of the bridge (Task 7), ignored by default — it starts a real
//! `claude -p --model haiku` session (costs a few cheap tokens):
//! `cargo test --test bridge_real -- --ignored --nocapture`.
//!
//! The session runs with the app's role settings (`agentcfg::settings_json`: hooks →
//! this build's `agentboard.exe --hook`, `crossSessionInbound: accept`) against a real control
//! server + bridge worker. Its hooks register the inbox pipe; `bridge::send_tracked` must
//! deliver over the pipe and the session must answer the message in a new turn.
use app_lib::config::{ensure_dotdir, load_all};
use app_lib::pty::{PtyManager, PtySink, SpawnSpec};
use app_lib::roles::Role;
use app_lib::workspace::Workspace;
use app_lib::{agentcfg, bridge, control, AppState, Snapshot, Ui};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

struct NullSink;
impl PtySink for NullSink {
    fn output(&self, _: &str, _: u64, _: &[u8]) {}
    fn exited(&self, _: &str, _: u64, _: i32) {}
}
struct NullUi;
impl Ui for NullUi {
    fn workspace_changed(&self, _: &Snapshot) {}
    fn drop_channel(&self, _: &str) {}
}

#[test]
#[ignore = "starts a real Claude Code session"]
fn real_claude_session_receives_bridge_message() {
    let root = tempfile::tempdir().unwrap();
    let appdata = tempfile::tempdir().unwrap();
    std::fs::write(appdata.path().join("settings.json"), json!({ "workspaceRoot": root.path().to_string_lossy() }).to_string())
        .unwrap();
    ensure_dotdir(root.path()).unwrap();
    std::fs::write(
        root.path().join(".agentboard/config.json"),
        r#"{"tabs":[{"name":"billing","role":"manager","dir":"billing"}]}"#,
    )
    .unwrap();
    let loaded = load_all(appdata.path());
    let ws = Workspace::from_config(&loaded);
    let pane = ws.panes.values().find(|p| p.role == Some(Role::Manager)).unwrap().id.clone();
    let (tx, _rx) = mpsc::channel();
    let state = Arc::new(AppState::new(
        ws,
        loaded,
        PtyManager::new_per_child_job(Arc::new(NullSink)),
        Arc::new(NullUi),
        tx,
        app_lib::persist::Saver::disabled(),
        Vec::new(),
        appdata.path().join("run"),
    ));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    rt.block_on(control::serve(state.clone())).unwrap();
    bridge::start_worker(state.clone());
    let token = state.tokens.issue(&pane);
    let url = state.ctrl_url.lock().unwrap().clone().unwrap();
    // the claude session runs outside the PtyManager here; a stand-in process gives the pane
    // its current spawn generation
    let spec = SpawnSpec {
        cmd: "cmd.exe".into(),
        args: vec!["/q".into(), "/k".into()],
        env: vec![],
        env_remove: vec![],
        cwd: std::env::temp_dir(),
        cols: 80,
        rows: 24,
    };
    state.pty.spawn(&pane, spec).unwrap();

    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_agentboard"));
    let settings = root.path().join("abverify.settings.json");
    std::fs::write(&settings, agentcfg::settings_json(&exe).to_string()).unwrap();
    let name = format!("abverify-t7-{}", std::process::id());
    let claude = which::which("claude").expect("claude on PATH");
    let mut cmd = Command::new(claude);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .args(["--model", "haiku", "--name", &name, "--settings"])
        .arg(&settings)
        .current_dir(root.path())
        .env("AGENTBOARD_URL", &url)
        .env("AGENTBOARD_TOKEN", &token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // like spikes/phase2/cleanenv.sh: nothing of a parent Claude Code session leaks in
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE") || k == "ANTHROPIC_BASE_URL" {
            cmd.env_remove(k);
        }
    }
    let mut child = cmd.spawn().expect("claude starts");
    let mut stdin = child.stdin.take().unwrap();
    let first = "Reply with exactly: READY. For every later message, reply with exactly GOT: followed by the message text.";
    writeln!(stdin, "{}", json!({"type":"user","message":{"role":"user","content":first}})).unwrap();
    let (ltx, lrx) = mpsc::channel::<String>();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = ltx.send(line);
        }
    });
    let texts = |deadline: Instant, want: &str| -> Option<String> {
        while Instant::now() < deadline {
            let Ok(line) = lrx.recv_timeout(Duration::from_millis(200)) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "assistant" {
                for c in v["message"]["content"].as_array().into_iter().flatten() {
                    if let Some(t) = c["text"].as_str() {
                        println!("assistant: {t}");
                        if t.contains(want) {
                            return Some(t.to_string());
                        }
                    }
                }
            }
        }
        None
    };
    let finish = |child: &mut std::process::Child| {
        let _ = Command::new("taskkill").args(["/PID", &child.id().to_string(), "/T", "/F"]).output();
        let _ = child.wait();
        state.pty.remove(&pane);
    };

    // any first answer: the session is up (the model may not echo READY verbatim)
    let ready = texts(Instant::now() + Duration::from_secs(90), "");
    let reg = state.bridge.reg(&pane);
    println!("registration: gen={:?} pipe={} idle={:?}", reg.as_ref().map(|r| r.gen), reg.as_ref().is_some_and(|r| r.socket.starts_with(r"\\.\pipe\")), reg.as_ref().map(|r| r.idle));
    if ready.is_none() || !reg.as_ref().is_some_and(|r| !r.socket.is_empty()) {
        finish(&mut child);
        panic!("session did not get ready / register its pipe (ready={ready:?})");
    }
    let rx = bridge::send_tracked(&state, &pane, "bridge-7731 한글 확인").unwrap();
    let outcome = rx.recv_timeout(Duration::from_secs(15));
    println!("outcome: {outcome:?}");
    let got = texts(Instant::now() + Duration::from_secs(90), "bridge-7731");
    finish(&mut child);
    assert_eq!(outcome, Ok(Ok(bridge::Via::Pipe)));
    // a peer message is not a user instruction, so the reply format may differ; the session
    // must have started a new turn that saw the text
    got.expect("the session answered the bridge message");
}
