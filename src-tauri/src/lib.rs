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

/// What the launcher pushes out to the frontend. Production: [`AppSink`] (Tauri event +
/// per-pane output channels); tests record. Both methods are leaves (no app lock taken).
pub trait Ui: Send + Sync {
    /// Emit `workspace-changed` with exactly this snapshot.
    fn workspace_changed(&self, snap: &Snapshot);
    /// Forget the pane's output channel (pane left the model, or its process is gone).
    fn drop_channel(&self, id: &str);
}

/// Lock order: see the module docs of `launcher`.
pub struct AppState {
    pub ws: Mutex<Workspace>,
    pub loaded: RwLock<Loaded>,
    pub pty: PtyManager,
    pub prompts: Mutex<HashMap<PaneId, PromptJob>>,
    /// Queue to the launcher worker thread (`launcher::submit`).
    pub effects: Mutex<mpsc::Sender<Effects>>,
    pub ui: Arc<dyn Ui>,
    /// Last issued `Snapshot::version`; incremented only while `ws` is held.
    pub version: AtomicU64,
    /// Serializes "versioned snapshot + emit" (order ws -> emit_lock; leaf).
    pub emit_lock: Mutex<()>,
    /// Debounced workspace.json writer (leaf: `notify` is a channel send).
    pub saver: persist::Saver,
    /// Startup problems outside config files (e.g. a discarded workspace.json), shown in the error list.
    pub startup_errors: Vec<String>,
}

impl AppState {
    pub fn new(
        ws: Workspace,
        loaded: Loaded,
        pty: PtyManager,
        ui: Arc<dyn Ui>,
        effects: mpsc::Sender<Effects>,
        saver: persist::Saver,
        startup_errors: Vec<String>,
    ) -> Self {
        AppState {
            ws: Mutex::new(ws),
            loaded: RwLock::new(loaded),
            pty,
            prompts: Mutex::new(HashMap::new()),
            effects: Mutex::new(effects),
            ui,
            version: AtomicU64::new(0),
            emit_lock: Mutex::new(()),
            saver,
            startup_errors,
        }
    }
}

/// Per-pane frontend output channels. The map lock is taken before the `panes` lock
/// (`channels -> panes`, in `remove_if_current`) and nothing else is acquired under it.
pub struct ChannelMap<C> {
    map: Mutex<HashMap<String, C>>,
}

impl<C> Default for ChannelMap<C> {
    fn default() -> Self {
        ChannelMap { map: Mutex::new(HashMap::new()) }
    }
}

impl<C> ChannelMap<C> {
    pub fn insert(&self, id: &str, ch: C) {
        self.map.lock().unwrap().insert(id.to_string(), ch);
    }

    pub fn remove(&self, id: &str) {
        self.map.lock().unwrap().remove(id);
    }

    /// Hand the pane's channel to `send`; a `false` result (frontend gone) drops the channel.
    pub fn deliver(&self, id: &str, send: impl FnOnce(&C) -> bool) {
        let mut m = self.map.lock().unwrap();
        if let Some(ch) = m.get(id) {
            if !send(ch) {
                m.remove(id);
            }
        }
    }

    /// Drop the channel only if `gen` is still the id's current generation. The check and the
    /// removal happen under the map lock, and a newer generation's channel can only be
    /// inserted (`pty_attach`) after that generation is registered — so an old process's exit
    /// never drops a respawned pane's channel. Returns whether `gen` was current (the caller
    /// then announces the exit).
    pub fn remove_if_current(&self, id: &str, gen: u64, pty: &PtyManager) -> bool {
        let mut m = self.map.lock().unwrap();
        if pty.current_gen(id) != Some(gen) {
            return false;
        }
        m.remove(id);
        true
    }

    #[cfg(test)]
    fn get(&self, id: &str) -> Option<C>
    where
        C: Clone,
    {
        self.map.lock().unwrap().get(id).cloned()
    }
}

/// Delivers PTY output to the attached frontend channel as raw binary and forwards
/// output/exit to the launcher. The single ring buffer lives in PtyManager, which snapshots
/// and runs the registration under its flush lock (see `PtyManager::attach_with`).
pub struct AppSink {
    app: AppHandle,
    channels: ChannelMap<Channel<InvokeResponseBody>>,
}

#[derive(Clone, Serialize)]
struct ExitPayload {
    id: String,
    gen: u64,
    code: i32,
}

impl AppSink {
    fn new(app: AppHandle) -> Self {
        AppSink { app, channels: ChannelMap::default() }
    }
}

impl Ui for AppSink {
    fn workspace_changed(&self, snap: &Snapshot) {
        let _ = self.app.emit(launcher::WORKSPACE_CHANGED, snap);
    }

    fn drop_channel(&self, id: &str) {
        self.channels.remove(id);
    }
}

impl PtySink for AppSink {
    // Runs under the pane's flush lock: flush_lock -> panes map (current_gen),
    // flush_lock -> prompts, flush_lock -> channels. Stale generations are dropped.
    fn output(&self, id: &str, gen: u64, bytes: &[u8]) {
        if let Some(state) = self.app.try_state::<AppState>() {
            if !launcher::on_output(&state, id, gen, bytes) {
                return;
            }
        }
        self.channels.deliver(id, |ch| ch.send(InvokeResponseBody::Raw(bytes.to_vec())).is_ok());
    }

    // Runs on the pane's waiter thread with no PTY lock held.
    fn exited(&self, id: &str, gen: u64, code: i32) {
        let Some(state) = self.app.try_state::<AppState>() else { return };
        if !launcher::on_exited(&state, id, gen, code) {
            return; // stale: the id belongs to a newer spawn (or none) — keep its channel
        }
        // Re-checked under the channel lock: a restart may have registered (and attached) a
        // newer generation since `on_exited` released `ws`.
        if self.channels.remove_if_current(id, gen, &state.pty) {
            let _ = self.app.emit("pty-exit", ExitPayload { id: id.to_string(), gen, code });
        }
    }
}

#[tauri::command]
fn pty_attach(
    state: State<'_, AppState>,
    sink: State<'_, Arc<AppSink>>,
    pane: String,
    channel: Channel<InvokeResponseBody>,
) -> Result<Response, String> {
    let mut snap = Vec::new();
    // snapshot + subscribe under PtyManager's flush lock (current generation): no loss, no duplicates
    let known = state.pty.attach_with(&pane, |s| {
        snap = s;
        sink.channels.insert(&pane, channel);
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

/// Apply a model command (`launcher::execute`); spawn/kill effects run on the launcher worker
/// afterwards (statuses then arrive through `workspace-changed`).
#[tauri::command]
fn ws_command(state: State<'_, AppState>, cmd: WsCommand) -> Result<Snapshot, String> {
    launcher::execute(&state, cmd)
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
            app.manage(sink.clone()); // pty_attach registers output channels on it
            app.manage(AppState::new(ws, loaded, pty, sink, tx, saver, startup_errors));
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
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn it_builds() {}

    struct NullSink;
    impl PtySink for NullSink {
        fn output(&self, _: &str, _: u64, _: &[u8]) {}
        fn exited(&self, _: &str, _: u64, _: i32) {}
    }

    fn cmd(args: &[&str]) -> pty::SpawnSpec {
        pty::SpawnSpec {
            cmd: "cmd.exe".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: vec![],
            cwd: std::env::current_dir().unwrap(),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn stale_exit_keeps_new_generation_channel() {
        let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
        let chs: ChannelMap<&str> = ChannelMap::default();
        let g1 = pty.spawn("c", cmd(&["/c", "exit 0"])).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while pty.is_running("c") && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        chs.insert("c", "gen1-channel");
        // restart: gen2 registered and attached before gen1's exited is delivered
        let g2 = pty.spawn("c", cmd(&[])).unwrap();
        chs.insert("c", "gen2-channel");
        assert!(!chs.remove_if_current("c", g1, &pty), "stale exit must not drop (and so not emit pty-exit)");
        assert_eq!(chs.get("c"), Some("gen2-channel"));
        // the current generation's exit drops its channel
        assert!(chs.remove_if_current("c", g2, &pty));
        assert_eq!(chs.get("c"), None);
        // unknown ids: nothing to drop, not current
        assert!(!chs.remove_if_current("nope", g2, &pty));
        pty.remove("c");
    }
}
