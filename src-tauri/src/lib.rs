// 이후 태스크에서 pub mod 추가: layout, config, profiles,
// workspace, launcher, persist

pub mod pty;
pub mod ring;
pub mod webview;
#[cfg(windows)]
pub mod winjob;

use pty::{PtyManager, PtySink, SpawnSpec};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, Emitter, Manager, State};

/// Delivers PTY output to the attached frontend channel as raw binary. The only state is
/// the channel map; the single ring buffer lives in PtyManager, which snapshots and runs
/// the registration under its flush lock (see `PtyManager::attach_with`).
pub struct AppSink {
    app: AppHandle,
    channels: Mutex<HashMap<String, Channel<InvokeResponseBody>>>,
}

#[derive(Clone, Serialize)]
struct ExitPayload {
    id: String,
    code: i32,
}

impl AppSink {
    fn new(app: AppHandle) -> Self {
        AppSink { app, channels: Mutex::new(HashMap::new()) }
    }
}

impl PtySink for AppSink {
    fn output(&self, id: &str, bytes: &[u8]) {
        let mut chs = self.channels.lock().unwrap();
        if let Some(ch) = chs.get(id) {
            if ch.send(InvokeResponseBody::Raw(bytes.to_vec())).is_err() {
                chs.remove(id);
            }
        }
    }

    fn exited(&self, id: &str, code: i32) {
        self.channels.lock().unwrap().remove(id);
        let _ = self.app.emit("pty-exit", ExitPayload { id: id.to_string(), code });
    }
}

pub struct AppState {
    pub pty: PtyManager,
    pub sink: Arc<AppSink>,
}

#[tauri::command]
fn pty_attach(state: State<'_, AppState>, pane: String, channel: Channel<InvokeResponseBody>) -> Response {
    let mut snap = Vec::new();
    // snapshot + subscribe under PtyManager's flush lock: no loss, no duplicates
    state.pty.attach_with(&pane, |s| {
        snap = s;
        state.sink.channels.lock().unwrap().insert(pane.clone(), channel);
    });
    Response::new(snap)
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
                webview::maybe_open_devtools(&w);
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
