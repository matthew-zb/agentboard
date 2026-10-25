pub mod config;
pub mod launcher;
pub mod layout;
pub mod persist;
pub mod profiles;
pub mod workspace;
pub mod pty;
pub mod ring;
pub mod webview;
#[cfg(windows)]
pub mod winjob;

use config::Loaded;
use launcher::PromptJob;
use pty::{PtyManager, PtySink};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::{AppHandle, Emitter, Manager, State};
use workspace::{Effects, PaneId, TabId, Workspace, WsCommand};

/// Payload of `ws_command` / `ws_snapshot` and of the `workspace-changed` event.
/// `errors` = number of config error lines (`config_errors().len()`).
/// `version` strictly increases in model order (stamped under the `ws` lock); the frontend
/// drops snapshots older than the last one it delivered (R16).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub workspace: Workspace,
    pub errors: usize,
    pub version: u64,
}

/// Lock order: see the module docs of `launcher`.
pub struct AppState {
    pub ws: Mutex<Workspace>,
    pub loaded: RwLock<Loaded>,
    pub pty: PtyManager,
    pub app: AppHandle,
    pub prompts: Mutex<HashMap<PaneId, PromptJob>>,
    /// Queue to the launcher worker thread (`launcher::submit`).
    pub effects: Mutex<mpsc::Sender<Effects>>,
    pub sink: Arc<AppSink>,
    /// Last issued `Snapshot::version`; incremented only while `ws` is held.
    pub version: AtomicU64,
    /// Serializes "versioned snapshot + emit" (order ws -> emit_lock; leaf).
    pub emit_lock: Mutex<()>,
    /// Debounced workspace.json writer (leaf: `notify` is a channel send).
    pub saver: persist::Saver,
    /// Startup problems outside config files (e.g. a discarded workspace.json), shown in the error list.
    pub startup_errors: Vec<String>,
}

/// Delivers PTY output to the attached frontend channel as raw binary and forwards
/// output/exit to the launcher. The single ring buffer lives in PtyManager, which snapshots
/// and runs the registration under its flush lock (see `PtyManager::attach_with`).
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

    /// Forget the pane's output channel (the pane left the model). `channels` is a leaf lock.
    pub(crate) fn drop_channel(&self, id: &str) {
        self.channels.lock().unwrap().remove(id);
    }
}

impl PtySink for AppSink {
    // Runs under the pane's flush lock: flush_lock -> prompts, flush_lock -> channels.
    fn output(&self, id: &str, bytes: &[u8]) {
        if let Some(state) = self.app.try_state::<AppState>() {
            launcher::on_output(&state, id, bytes);
        }
        let mut chs = self.channels.lock().unwrap();
        if let Some(ch) = chs.get(id) {
            if ch.send(InvokeResponseBody::Raw(bytes.to_vec())).is_err() {
                chs.remove(id);
            }
        }
    }

    // Runs on the pane's waiter thread with no PTY lock held.
    fn exited(&self, id: &str, code: i32) {
        if let Some(state) = self.app.try_state::<AppState>() {
            if !launcher::on_exited(&state, id, code) {
                return; // stale: a restarted process already owns this id
            }
        }
        self.channels.lock().unwrap().remove(id);
        let _ = self.app.emit("pty-exit", ExitPayload { id: id.to_string(), code });
    }
}

#[tauri::command]
fn pty_attach(state: State<'_, AppState>, pane: String, channel: Channel<InvokeResponseBody>) -> Result<Response, String> {
    let mut snap = Vec::new();
    // snapshot + subscribe under PtyManager's flush lock: no loss, no duplicates
    let known = state.pty.attach_with(&pane, |s| {
        snap = s;
        state.sink.channels.lock().unwrap().insert(pane.clone(), channel);
    });
    if !known {
        return Err(format!("프로세스가 없는 창입니다: {pane}"));
    }
    Ok(Response::new(snap))
}

#[tauri::command]
fn pty_write(state: State<'_, AppState>, pane: String, data: String) -> Result<(), String> {
    state.pty.write(&pane, data.as_bytes())
}

#[tauri::command]
fn pty_resize(state: State<'_, AppState>, pane: String, cols: u16, rows: u16) -> Result<(), String> {
    state.pty.resize(&pane, cols, rows)
}

/// Apply a model command; spawn/kill effects run on the launcher worker afterwards
/// (statuses then arrive through `workspace-changed`).
#[tauri::command]
fn ws_command(state: State<'_, AppState>, cmd: WsCommand) -> Result<Snapshot, String> {
    let fx = {
        let mut ws = state.ws.lock().unwrap();
        let loaded = state.loaded.read().unwrap();
        ws.apply(cmd, &loaded)?
    };
    launcher::submit(&state, fx);
    Ok(launcher::publish(&state))
}

#[tauri::command]
fn ws_snapshot(state: State<'_, AppState>) -> Snapshot {
    launcher::snapshot(&state)
}

#[tauri::command]
fn config_errors(state: State<'_, AppState>) -> Vec<String> {
    launcher::error_lines(&state)
}

#[tauri::command]
fn get_keybindings(state: State<'_, AppState>) -> BTreeMap<String, String> {
    state.loaded.read().unwrap().keybindings.0.clone()
}

#[tauri::command]
fn get_passthrough(state: State<'_, AppState>, pane: String) -> Vec<String> {
    let Some(p) = state.ws.lock().unwrap().pane_by_id(&pane).cloned() else { return Vec::new() };
    let loaded = state.loaded.read().unwrap();
    launcher::profile_for(&p, &loaded).map(|prof| prof.passthrough_keys.clone()).unwrap_or_default()
}

/// Directories directly under `workspaceRoot`, excluding names starting with `.`, sorted.
#[tauri::command]
fn list_workspace_dirs(state: State<'_, AppState>) -> Vec<String> {
    let root = state.loaded.read().unwrap().settings.workspace_root.clone();
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

#[tauri::command]
fn create_workspace_dir(state: State<'_, AppState>, name: String) -> Result<(), String> {
    launcher::check_dir_name(&name)?;
    let root = state.loaded.read().unwrap().settings.workspace_root.clone();
    let path = root.join(&name);
    if path.exists() {
        return Err(format!("이미 있는 폴더입니다: {name}"));
    }
    std::fs::create_dir(&path).map_err(|e| format!("폴더를 만들 수 없습니다: {} — {e}", path.display()))
}

#[tauri::command]
fn list_templates(state: State<'_, AppState>) -> Vec<String> {
    state.loaded.read().unwrap().templates.keys().cloned().collect()
}

/// `config.json` defaultTemplate (the new-tab wizard lists it first).
#[tauri::command]
fn default_template(state: State<'_, AppState>) -> String {
    state.loaded.read().unwrap().config.default_template.clone()
}

#[tauri::command]
fn missing_dirs(state: State<'_, AppState>) -> Vec<(TabId, String)> {
    launcher::missing_dirs(&state.ws.lock().unwrap())
}

/// Number of running agents (panes with a live process) for the close confirmation.
#[tauri::command]
fn running_count(state: State<'_, AppState>) -> usize {
    state.ws.lock().unwrap().running_count()
}

/// Confirmed app exit: save workspace.json, kill every pane process, exit.
#[tauri::command]
fn app_exit(app: AppHandle, state: State<'_, AppState>) {
    state.saver.flush();
    state.pty.kill_all();
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            pty_attach,
            pty_write,
            pty_resize,
            ws_command,
            ws_snapshot,
            config_errors,
            get_keybindings,
            get_passthrough,
            list_workspace_dirs,
            create_workspace_dir,
            list_templates,
            default_template,
            missing_dirs,
            running_count,
            app_exit,
        ])
        .setup(|app| {
            // load_all = load_settings + ensure_dotdir (incl. powershell profile) + load
            let appdata = appdata_dir();
            let loaded = config::load_all(&appdata);
            let ws_path = appdata.join("workspace.json");
            let mut startup_errors = Vec::new();
            let mut saving = true;
            let (ws, initial) = match persist::load(&ws_path) {
                Ok(Some(mut ws)) => {
                    let fx = persist::prepare_restored(&mut ws);
                    (ws, fx)
                }
                Ok(None) => {
                    let ws = Workspace::from_config(&loaded);
                    let fx = launcher::initial_effects(&ws);
                    (ws, fx)
                }
                Err(e) => {
                    eprintln!("[agentboard] workspace.json 복원 실패: {e}");
                    startup_errors.push(e.user_message().to_string());
                    // an original that could not be preserved must never be overwritten
                    saving = e.may_overwrite();
                    let ws = Workspace::from_config(&loaded);
                    let fx = launcher::initial_effects(&ws);
                    (ws, fx)
                }
            };
            let handle = app.handle().clone();
            // clone under `ws`, released before the saver writes (no lock held during IO)
            let saver = if saving {
                persist::Saver::new(ws_path, move || {
                    handle.try_state::<AppState>().map(|s| s.ws.lock().unwrap().clone())
                })
            } else {
                persist::Saver::disabled()
            };

            let sink = Arc::new(AppSink::new(app.handle().clone()));
            let pty = PtyManager::new(sink.clone());
            let (tx, rx) = mpsc::channel();
            app.manage(AppState {
                ws: Mutex::new(ws),
                loaded: RwLock::new(loaded),
                pty,
                app: app.handle().clone(),
                prompts: Mutex::new(HashMap::new()),
                effects: Mutex::new(tx),
                sink,
                version: AtomicU64::new(0),
                emit_lock: Mutex::new(()),
                saver,
                startup_errors,
            });
            // spawning happens on the launcher thread: window creation is not blocked
            launcher::start(app.handle(), rx, initial);

            if let Some(w) = app.get_webview_window("main") {
                webview::disable_browser_accelerators(&w);
                webview::maybe_open_devtools(&w);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building AgentBoard")
        .run(|app, event| {
            // Flush the pending save on every way out (Task 17 may veto the exit; an extra save is harmless).
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                if let Some(state) = app.try_state::<AppState>() {
                    state.saver.flush();
                }
            }
        });
}

/// `%APPDATA%/agentboard` (settings.json, workspace.json) — not Tauri's identifier-based dir.
pub fn appdata_dir() -> std::path::PathBuf {
    dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join("agentboard")
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_builds() {}
}
