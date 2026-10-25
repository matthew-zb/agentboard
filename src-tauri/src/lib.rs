// 이후 태스크에서 pub mod 추가: layout, config, profiles,
// workspace, launcher, persist

pub mod pty;
pub mod ring;
pub mod webview;
#[cfg(windows)]
pub mod winjob;

use pty::{PtyManager, PtySink, SpawnSpec, RING_CAP};
use ring::RingBuffer;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct SinkState {
    channels: HashMap<String, Channel<InvokeResponseBody>>,
    /// Bytes exactly as delivered through `output`, per pane. Kept here (not in the
    /// PtyManager ring) so a snapshot is consistent with what the channel has received.
    rings: HashMap<String, RingBuffer>,
}

/// Delivers PTY output to the attached frontend channel as raw binary.
/// One lock covers ring + channel map: `attach` snapshots and subscribes under it, and
/// `output` appends and sends under it, so no byte is lost or delivered twice.
pub struct AppSink {
    app: AppHandle,
    state: Mutex<SinkState>,
}

#[derive(Clone, Serialize)]
struct ExitPayload {
    id: String,
    code: i32,
}

impl AppSink {
    fn new(app: AppHandle) -> Self {
        AppSink { app, state: Mutex::new(SinkState::default()) }
    }

    pub fn attach(&self, id: &str, channel: Channel<InvokeResponseBody>) -> Vec<u8> {
        let mut st = self.state.lock().unwrap();
        let snap = st.rings.get(id).map(|r| r.snapshot()).unwrap_or_default();
        st.channels.insert(id.to_string(), channel);
        snap
    }
}

impl PtySink for AppSink {
    fn output(&self, id: &str, bytes: &[u8]) {
        let mut st = self.state.lock().unwrap();
        st.rings.entry(id.to_string()).or_insert_with(|| RingBuffer::new(RING_CAP)).push(bytes);
        if let Some(ch) = st.channels.get(id) {
            if ch.send(InvokeResponseBody::Raw(bytes.to_vec())).is_err() {
                st.channels.remove(id);
            }
        }
    }

    fn exited(&self, id: &str, code: i32) {
        let _ = self.app.emit("pty-exit", ExitPayload { id: id.to_string(), code });
    }
}

pub struct AppState {
    pub pty: PtyManager,
    pub sink: Arc<AppSink>,
}

#[tauri::command]
fn pty_attach(state: State<'_, AppState>, pane: String, channel: Channel<InvokeResponseBody>) -> Response {
    Response::new(state.sink.attach(&pane, channel))
}

#[tauri::command]
fn pty_write(state: State<'_, AppState>, pane: String, data: String) -> Result<(), String> {
    state.pty.write(&pane, data.as_bytes())
}

#[tauri::command]
fn pty_resize(state: State<'_, AppState>, pane: String, cols: u16, rows: u16) -> Result<(), String> {
    state.pty.resize(&pane, cols, rows)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![pty_attach, pty_write, pty_resize])
        .setup(|app| {
            let sink = Arc::new(AppSink::new(app.handle().clone()));
            let pty = PtyManager::new(sink.clone());
            // TEMP(Task 9): hardcoded single pane; removed when the workspace launcher lands.
            if let Err(e) = pty.spawn(
                "p-test",
                SpawnSpec {
                    cmd: "powershell.exe".into(),
                    args: vec![],
                    env: vec![],
                    cwd: "C:/workspace".into(),
                    cols: 120,
                    rows: 30,
                },
            ) {
                eprintln!("[agentboard] spawn p-test failed: {e}");
            }
            app.manage(AppState { pty, sink });
            if let Some(w) = app.get_webview_window("main") {
                webview::disable_browser_accelerators(&w);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AgentBoard");
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {}
}
