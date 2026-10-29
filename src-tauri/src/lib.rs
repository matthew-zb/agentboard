pub mod agentcfg;
pub mod attention;
pub mod autostart;
pub mod board;
pub mod bridge;
pub mod config;
pub mod control;
pub mod docs;
pub mod envcheck;
pub mod hookcli;
pub mod inbox;
pub mod issues;
pub mod launcher;
pub mod layout;
pub mod mcp_tools;
pub mod memo;
pub mod nightly;
pub mod persist;
pub mod proctree;
pub mod profiles;
pub mod roles;
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Emit `inbox-changed` (`{"version": n}`) after an inbox mutation (leaf, no app lock).
    fn inbox_changed(&self, _version: u64) {}
    /// Emit `doc-changed` (`{"paneId": id}`): the pane's watched document changed (Task 14).
    fn doc_changed(&self, _pane: &str) {}
    /// Emit `env-changed` (no payload): environment check results or the skip list changed
    /// (Task 12; the status bar re-reads `env_status`).
    fn env_changed(&self) {}
    /// Emit `config-reloaded` (no payload): settings were read again at runtime; the frontend
    /// re-reads keybindings and the error list.
    fn config_reloaded(&self) {}
    /// Emit `board-changed` (no payload): a hook updated the dashboard board, or the nightly
    /// state changed (the dashboard re-reads `board_status`).
    fn board_changed(&self) {}
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
    /// Set by the first confirmed exit (`app_exit`); later calls are ignored.
    pub exiting: AtomicBool,
    /// The memo's last known content (`memo.rs`; leaf lock).
    pub memo: memo::MemoState,
    /// Per-pane session status for the dashboard (`board.rs`; leaf lock).
    pub board: board::Board,
    /// Per-pane bearer tokens of role panes (leaf lock inside).
    pub tokens: agentcfg::Tokens,
    /// Control server base URL (`http://127.0.0.1:<port>`), set by the control server (Task 5)
    /// before the initial spawns. `None` → role panes get `agentcfg::PLACEHOLDER_CTRL_URL`.
    /// Leaf lock.
    pub ctrl_url: Mutex<Option<String>>,
    /// Where role panes' run files go (`%APPDATA%/agentboard/run`).
    pub run_dir: std::path::PathBuf,
    /// Hook event handlers of the control server (Tasks 7·10 register). Leaf lock.
    pub hooks: control::HookRouter,
    /// Nightly refresh (Task 11): state machine (leaf lock) + scheduler wake/stop signals.
    pub nightly: nightly::Runtime,
    /// Inbox store (`<workspaceRoot>/.agentboard/inbox`); its lock is a leaf (+ its own emit
    /// lock), see `inbox` module docs.
    pub inbox: inbox::Inbox,
    /// App → session messages (Task 7): session registrations + per-pane queues; leaf locks.
    pub bridge: bridge::Bridge,
    /// Last real keystroke per pane (`pty_write`, R4). Leaf lock. Task 11 reads it too.
    pub last_input: Mutex<HashMap<PaneId, std::time::Instant>>,
    /// Folder-trust timers of role panes per spawn generation (Task 9, `attention`). Leaf lock.
    pub trust: Mutex<HashMap<PaneId, attention::Trust>>,
    /// Watched documents of document panes (Task 14). Leaf lock.
    pub docs: docs::DocWatch,
    /// Environment check probe results (Task 12, `envcheck`). Leaf locks.
    pub env: envcheck::EnvCache,
}

impl AppState {
    pub fn new(
        ws: Workspace,
        loaded: Loaded,
        pty: PtyManager,
        ui: Arc<dyn Ui>,
        effects: mpsc::Sender<Effects>,
        saver: persist::Saver,
        mut startup_errors: Vec<String>,
        run_dir: std::path::PathBuf,
    ) -> Self {
        // unreadable item files are skipped and listed with the startup errors (file names only)
        let inbox_dir = loaded.settings.workspace_root.join(".agentboard").join("inbox");
        let emit_ui = ui.clone();
        let (inbox, inbox_errors) = inbox::Inbox::load(inbox_dir, Arc::new(move |v| emit_ui.inbox_changed(v)));
        startup_errors.extend(inbox_errors);
        let hooks = control::HookRouter::new();
        bridge::register_hooks(&hooks);
        attention::register_hooks(&hooks); // after the bridge: wraps its SessionStart
        board::register_hooks(&hooks); // last: wraps SessionStart/UserPromptSubmit/Stop (dashboard)
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
            exiting: AtomicBool::new(false),
            memo: memo::MemoState::default(),
            board: board::Board::default(),
            tokens: agentcfg::Tokens::new(),
            ctrl_url: Mutex::new(None),
            run_dir,
            hooks,
            nightly: nightly::Runtime::new(),
            inbox,
            bridge: bridge::Bridge::new(),
            last_input: Mutex::new(HashMap::new()),
            trust: Mutex::new(HashMap::new()),
            docs: docs::DocWatch::new(),
            env: envcheck::EnvCache::new(),
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

    fn inbox_changed(&self, version: u64) {
        let _ = self.app.emit(inbox::INBOX_CHANGED, inbox::InboxChanged { version });
    }

    fn doc_changed(&self, pane: &str) {
        let _ = self.app.emit(docs::DOC_CHANGED, DocChanged { pane_id: pane.to_string() });
    }

    fn env_changed(&self) {
        let _ = self.app.emit(ENV_CHANGED, ());
    }

    fn config_reloaded(&self) {
        let _ = self.app.emit(CONFIG_RELOADED, ());
    }

    fn board_changed(&self) {
        let _ = self.app.emit(board::BOARD_CHANGED, ());
    }
}

/// Event: config.json / templates / profiles / keybindings were read again (`config_reload`).
pub const CONFIG_RELOADED: &str = "config-reloaded";

/// Event: environment check results / skip list changed (Task 12).
pub const ENV_CHANGED: &str = "env-changed";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocChanged {
    pane_id: String,
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

/// Most bytes one `pty_write_raw` call may carry (terminal replies and mouse reports are tiny).
pub const RAW_WRITE_MAX: usize = 64 * 1024;

/// User keystrokes / paste: counts as input (R4 — bridge PTY fallback, nightly quiet check).
fn write_user(state: &AppState, pane: &str, bytes: &[u8]) -> Result<(), String> {
    bridge::note_input(state, pane);
    state.pty.write(pane, bytes)
}

/// Terminal replies (CPR, DA, focus, mouse reports) and xterm binary data: written as-is,
/// never counted as input.
fn write_raw(state: &AppState, pane: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > RAW_WRITE_MAX {
        return Err(format!("입력이 너무 깁니다: {}바이트(최대 {RAW_WRITE_MAX})", bytes.len()));
    }
    state.pty.write(pane, bytes)
}

/// IPC contract: user input only (`term.onData` minus terminal replies, paste, sync fan-out).
#[tauri::command]
fn pty_write(state: State<'_, AppState>, pane: String, data: String) -> Result<(), String> {
    // real keystrokes only (R4): the bridge's PTY fallback and Task 11's quiet check use it
    write_user(&state, &pane, data.as_bytes())
}

/// IPC contract: terminal replies (UTF-8 bytes) and `term.onBinary` (one byte per char), as a
/// number array 0–255 of at most [`RAW_WRITE_MAX`] items. Not counted as user input.
#[tauri::command]
fn pty_write_raw(state: State<'_, AppState>, pane: String, bytes: Vec<u8>) -> Result<(), String> {
    write_raw(&state, &pane, &bytes)
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

/// Read config.json, templates, agent profiles and keybindings again without restarting:
/// `Ok({added, keptMissing, promptsUpdated})`, or the error lines (`file:line:col: …`) with
/// nothing changed (`launcher::reload_config_from`). File IO off the UI thread.
#[tauri::command]
async fn config_reload(app: AppHandle) -> Result<workspace::ReloadReport, Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or_else(|| vec!["앱 상태가 아직 준비되지 않았습니다".to_string()])?;
        launcher::reload_config(&state)
    })
    .await
    .map_err(|e| vec![format!("설정을 다시 읽는 작업이 중단되었습니다 — {e}")])?
}

#[tauri::command]
fn get_keybindings(state: State<'_, AppState>) -> BTreeMap<String, String> {
    state.loaded.read().unwrap().keybindings.0.clone()
}

#[tauri::command]
fn get_passthrough(state: State<'_, AppState>, pane: String) -> Vec<String> {
    let Some(p) = state.ws.lock().unwrap().pane_by_id(&pane).cloned() else { return Vec::new() };
    if p.kind.is_applet() {
        return Vec::new(); // no terminal, no profile
    }
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

/// True for the first call only: one exit runs, a second close click meanwhile does nothing.
fn begin_exit(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

/// Confirmed app exit: save workspace.json (up to `persist::FLUSH_WAIT`), kill every pane
/// process, exit — on a blocking thread, so the window is not frozen meanwhile. Only the first
/// call acts.
#[tauri::command]
async fn app_exit(app: AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return app.exit(0) };
    if !begin_exit(&state.exiting) {
        return;
    }
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(state) = app.try_state::<AppState>() {
            state.saver.flush();
            state.pty.kill_all();
        }
        app.exit(0);
    })
    .await;
}

/// Inbox applet (Task 10): items newest first (done ones only with `show_done`) + version.
#[tauri::command]
fn inbox_list(state: State<'_, AppState>, show_done: bool) -> inbox::InboxList {
    inbox_list_with_owners(&state, show_done)
}

/// `inbox_list` plus the owners that have no tab any more (R29, review #9). `ws` is read and
/// released before the inbox is called.
fn inbox_list_with_owners(state: &AppState, show_done: bool) -> inbox::InboxList {
    let known: std::collections::BTreeSet<String> = {
        let ws = state.ws.lock().unwrap();
        ws.tabs.values().filter(|t| bridge::owner_session_in(&ws, &t.name).is_some()).map(|t| t.name.clone()).collect()
    };
    let mut l = state.inbox.list(show_done);
    let unknown: std::collections::BTreeSet<String> =
        l.items.iter().filter(|i| !known.contains(&i.owner_name)).map(|i| i.owner_name.clone()).collect();
    l.unknown_owners = unknown.into_iter().collect();
    l
}

#[tauri::command]
fn inbox_get(state: State<'_, AppState>, id: String) -> Result<inbox::InboxItem, String> {
    state.inbox.get(&id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {id}"))
}

#[tauri::command]
fn inbox_mark_read(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.inbox.mark_read(&id)
}

/// The user confirms a proposed close (`close_proposed` → `done`) and the owner Manager is told
/// via the dispatcher. Returns a Korean warning when that notice could not be queued.
#[tauri::command]
fn inbox_confirm_done(state: State<'_, AppState>, id: String) -> Result<Option<String>, String> {
    bridge::confirm_done(&state, &id)
}

/// The user's instruction for an item: recorded (state → in_progress) and relayed to the
/// dispatcher, which passes it to the owner Manager. Err (Korean) when the dispatcher is not
/// running, the text is empty/too long, or the item is done.
#[tauri::command]
fn inbox_instruct(state: State<'_, AppState>, id: String, text: String) -> Result<(), String> {
    bridge::relay_instruct(&state, &id, &text)
}

/// Ask (via the dispatcher) the owner Manager to create an issue for the item.
#[tauri::command]
fn inbox_request_issue(state: State<'_, AppState>, id: String) -> Result<(), String> {
    bridge::relay_issue_request(&state, &id)
}

/// Result of `inbox_add_files`: the copied files and, when the dispatcher could not be told,
/// a Korean warning (the files stay copied).
#[derive(serde::Serialize)]
struct AddFilesResult {
    files: Vec<String>,
    warning: Option<String>,
}

/// Copy files into the item's issue `input/` or `output/` (never overwriting); returns the new
/// files. Runs on a blocking thread (copies may be large).
#[tauri::command]
async fn inbox_add_files(
    app: AppHandle,
    id: String,
    paths: Vec<String>,
    target: inbox::Target,
) -> Result<AddFilesResult, String> {
    let paths: Vec<std::path::PathBuf> = paths.into_iter().map(std::path::PathBuf::from).collect();
    let (id2, app2) = (id.clone(), app.clone());
    let res = tauri::async_runtime::spawn_blocking(move || {
        let app = app2;
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        let owner = state.inbox.get(&id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {id}"))?.owner_name;
        // ws taken and released here; the inbox is called without it
        let workspace = mcp_tools::owner_workspace(&state, &owner)?;
        state.inbox.add_files(&id, &paths, target, &workspace)
    })
    .await
    .map_err(|e| format!("파일 복사 작업이 중단되었습니다 — {e}"))??;
    let files: Vec<String> = res.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
    // spec §5: the owner Manager hears about the new files through the dispatcher
    let warning = match app.try_state::<AppState>() {
        Some(state) => bridge::notify_files_saved(&state, &id2, &files).err(),
        None => Some("앱 상태가 아직 준비되지 않았습니다".into()),
    }
    .map(|e| format!("파일은 복사했지만 담당 Manager에게 알리지 못했습니다 — {e}"));
    Ok(AddFilesResult { files, warning })
}

// ───────── document tabs (Task 14) ─────────

fn workspace_root(state: &AppState) -> std::path::PathBuf {
    state.loaded.read().unwrap().settings.workspace_root.clone()
}

/// Dashboard status: per role pane (current generation) idle, last prompt/reply, context size,
/// and the nightly machine's phase. In memory only; `board-changed` says when to re-read.
#[tauri::command]
fn board_status(state: State<'_, AppState>) -> board::BoardStatus {
    board::status(&state)
}

/// `memo_read` reply: the file relative to workspaceRoot (for `doc_watch`) and its whole text.
#[derive(Serialize)]
struct MemoText {
    path: String,
    text: String,
}

/// `loaded` is read (released) before the memo lock is taken.
fn memo_read_in(state: &AppState) -> Result<MemoText, String> {
    let (path, text) = memo::read(&workspace_root(state), &state.memo)?;
    Ok(MemoText { path, text })
}

fn memo_save_in(state: &AppState, text: &str) -> Result<(), String> {
    memo::save(&workspace_root(state), &state.memo, text)
}

/// The memo applet's text: `{path: "MEMO.md", text}` — the whole file; header/body are split by
/// the frontend. Created from the header draft when missing; an external edit since the app's
/// last read/save is backed up first (`memo.rs`). File IO off the UI thread.
#[tauri::command]
async fn memo_read(app: AppHandle) -> Result<MemoText, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        memo_read_in(&state)
    })
    .await
    .map_err(|e| format!("메모를 읽는 작업이 중단되었습니다 — {e}"))?
}

/// Save the memo applet's whole text (header kept by the frontend): the current file goes to
/// `.agentboard/memo.bak` first, then an atomic write.
#[tauri::command]
async fn memo_save(app: AppHandle, text: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        memo_save_in(&state, &text)
    })
    .await
    .map_err(|e| format!("메모를 저장하는 작업이 중단되었습니다 — {e}"))?
}

/// Read a document (`path`: absolute or relative to workspaceRoot). Validated on every call
/// (`docs::resolve_doc`): an existing `.md` inside workspaceRoot after resolving links.
#[tauri::command]
async fn doc_read(app: AppHandle, path: String) -> Result<docs::DocText, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        docs::read_doc(&workspace_root(&state), &path)
    })
    .await
    .map_err(|e| format!("문서를 읽는 작업이 중단되었습니다 — {e}"))?
}

/// `.md` files under workspaceRoot (relative, `/`), excluding `.agentboard`, `node_modules`,
/// `.git`, `target`; at most 5,000.
#[tauri::command]
async fn doc_list_md(app: AppHandle, query: Option<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        Ok(docs::list_md(&workspace_root(&state), query.as_deref(), docs::LIST_MAX))
    })
    .await
    .map_err(|e| format!("문서 목록 작업이 중단되었습니다 — {e}"))?
}

/// Watch the document a document pane shows (replaces its previous one). The watcher thread
/// polls mtimes every second and emits `doc-changed {paneId}`. Runs on a blocking thread
/// (canonicalize can be slow on a network share — review #12).
#[tauri::command]
async fn doc_watch(app: AppHandle, pane_id: String, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        doc_watch_blocking(&state, pane_id, path)
    })
    .await
    .map_err(|e| format!("문서 감시 작업이 중단되었습니다 — {e}"))?
}

fn doc_watch_blocking(state: &AppState, pane_id: String, path: String) -> Result<(), String> {
    // ws taken and released; then loaded(read), released; then the watch map (leaf)
    let is_doc = state.ws.lock().unwrap().pane_by_id(&pane_id).is_some_and(|p| p.kind.is_doc());
    if !is_doc {
        return Err(format!("문서 창이 아닙니다: {pane_id}"));
    }
    // a document missing right now is still watched (inside the root), so it reloads once created
    let abs = docs::resolve_watch_path(&workspace_root(state), &path)?;
    let mtime = docs::mtime_of(&abs);
    state.docs.watch(&pane_id, abs, mtime);
    // the pane may have closed (and been unwatched by run_effects) since the check above:
    // re-check after inserting so no orphan entry stays polled (locks taken one at a time)
    let alive = state.ws.lock().unwrap().pane_by_id(&pane_id).is_some();
    if !alive {
        state.docs.unwatch(&pane_id);
    }
    Ok(())
}

/// An image of a document as a `data:` URL. `src` is validated lexically before any file
/// system call (`docs::check_image_src`: no UNC / device / drive / absolute paths), then
/// resolved against the document's folder and prefix-checked against the canonical
/// workspaceRoot (`docs::resolve_image`). Replaces the asset protocol (fix round 1, R23).
#[tauri::command]
async fn doc_image(app: AppHandle, doc: String, src: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        docs::read_image(&workspace_root(&state), &doc, &src)
    })
    .await
    .map_err(|e| format!("이미지를 읽는 작업이 중단되었습니다 — {e}"))?
}

#[tauri::command]
fn doc_unwatch(state: State<'_, AppState>, pane_id: String) {
    state.docs.unwatch(&pane_id);
}

/// An http(s) link of a document → the system browser (opener plugin's Rust API; the frontend
/// gets no opener capability). Anything else is refused. The browser is started outside the
/// app's job object so that it outlives the app (`docs::open_external`).
#[tauri::command]
fn doc_open_url(url: String) -> Result<(), String> {
    docs::open_external(&url, spawn_url_opener, open_url_directly)
}

/// The opener as a process outside the app's job: `<this exe> --open-url <url>` (main.rs).
/// Arguments go through CreateProcess quoting, no shell.
#[cfg(windows)]
fn spawn_url_opener(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg(docs::OPEN_URL_FLAG).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    winjob::spawn_outside_job(&mut cmd).map(drop) // not waited for: it opens the url and exits
}

#[cfg(not(windows))]
fn spawn_url_opener(_url: &str) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn open_url_directly(url: &str) -> Result<(), String> {
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| format!("브라우저를 열 수 없습니다 — {e}"))
}

/// `agentboard.exe --open-url <url>` (started by [`doc_open_url`]): open it and return the
/// exit code; no Tauri/WebView.
pub fn run_open_url(url: &str) -> i32 {
    docs::run_open_url_with(url, open_url_directly)
}

/// Files of the inbox item's issue folder (`input/`, `output/`), for the inbox detail. The
/// folder is re-derived and checked with `issues::resolve` (stored paths are not trusted).
/// Runs on a blocking thread (canonicalize + read_dir — review #12).
#[tauri::command]
async fn inbox_issue_files(app: AppHandle, id: String) -> Result<docs::IssueFiles, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        let item = state.inbox.get(&id).ok_or_else(|| format!("인박스 항목을 찾을 수 없습니다: {id}"))?;
        let Some(issue) = item.issue else { return Ok(docs::IssueFiles::default()) };
        let workspace = mcp_tools::owner_workspace(&state, &item.owner_name)?;
        docs::issue_files(&issues::resolve(&workspace, &issue)?)
    })
    .await
    .map_err(|e| format!("이슈 파일 목록 작업이 중단되었습니다 — {e}"))?
}

/// The document watcher thread: one `poll` per second; changed panes get `doc-changed`.
fn start_doc_watcher(app: AppHandle) {
    let _ = std::thread::Builder::new().name("doc-watch".into()).spawn(move || loop {
        std::thread::sleep(docs::POLL_INTERVAL);
        let Some(state) = app.try_state::<AppState>() else { continue };
        if state.docs.is_empty() {
            continue;
        }
        for pane in state.docs.poll(docs::mtime_of) {
            state.ui.doc_changed(&pane);
        }
    });
}

// ───────── startup · autostart · environment (Task 12) ─────────

/// Environment check items from the cached probe results (status bar); `None` until the first
/// check after start finished (`env-changed` follows). Never runs the probes itself.
#[tauri::command]
fn env_status(state: State<'_, AppState>) -> Option<Vec<envcheck::EnvItem>> {
    envcheck::current(&state)
}

/// Whether AgentBoard starts at login (HKCU Run value).
#[tauri::command]
fn autostart_get() -> Result<bool, String> {
    autostart::is_enabled()
}

/// Turn starting at login on/off; returns the new state.
#[tauri::command]
fn autostart_set(on: bool) -> Result<bool, String> {
    if on {
        autostart::enable()?;
    } else {
        autostart::disable()?;
    }
    autostart::is_enabled()
}

/// New-tab wizard, role "Manager": same path as MCP `create_manager_tab` (config.json entry,
/// so it survives a restart), but the tab opens in the focused group with the focus.
#[tauri::command]
async fn create_manager_tab(app: AppHandle, name: String, dir: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.try_state::<AppState>().ok_or("앱 상태가 아직 준비되지 않았습니다")?;
        mcp_tools::create_manager_tab_ui(&state, name, dir)
    })
    .await
    .map_err(|e| format!("탭을 만드는 작업이 중단되었습니다 — {e}"))?
}

/// Restore a loaded workspace.json (spec §9 rules, `persist::restore_plan`; config role tabs
/// that could not be added become startup errors). A plan that does not validate (e.g. a
/// hand-edited file) falls back to a fresh start from config with a startup error; the file is
/// first moved to `workspace.json.bak` — if that fails, `saving` is turned off so the saved
/// layout is never overwritten.
fn restore_or_fresh(
    ws: Workspace,
    loaded: &Loaded,
    ws_path: &std::path::Path,
    saving: &mut bool,
    startup_errors: &mut Vec<String>,
    restored: &mut bool,
) -> (Workspace, Effects) {
    let (mut ws, fx, errors) = persist::restore_plan(ws, loaded);
    match ws.normalize_and_validate() {
        Ok(()) => {
            startup_errors.extend(errors);
            *restored = true;
            (ws, fx)
        }
        Err(e) => {
            eprintln!("[agentboard] 복원 규칙 적용 실패: {e}");
            match persist::move_to_bak(ws_path) {
                Ok(()) => startup_errors.push(format!(
                    "저장된 작업 상태를 복원할 수 없어 초기 배치로 시작했습니다(원본은 workspace.json.bak) — {e}"
                )),
                Err(be) => {
                    *saving = false;
                    startup_errors.push(format!(
                        "저장된 작업 상태를 복원할 수 없어 초기 배치로 시작했습니다. 이번 실행의 변경 사항은 저장되지 않습니다 — {e} ({be})"
                    ));
                }
            }
            let ws = Workspace::from_config(loaded);
            let fx = launcher::initial_effects(&ws);
            (ws, fx)
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // R30 / final review #4: registered first — a second launch (autostart + shortcut,
        // installer relaunch) hands over to the running instance, which shows and focuses its
        // window, and exits before restoring a second set of role sessions. The `--hook` CLI
        // mode never gets here (main.rs returns before `run`), so hooks keep working.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        // the main WebView never leaves the app (document links, image maps, …): fix round 1
        .plugin(webview::nav_guard())
        .invoke_handler(tauri::generate_handler![
            pty_attach,
            pty_write,
            pty_write_raw,
            pty_resize,
            ws_command,
            ws_snapshot,
            config_errors,
            get_keybindings,
            config_reload,
            get_passthrough,
            list_workspace_dirs,
            create_workspace_dir,
            list_templates,
            default_template,
            missing_dirs,
            running_count,
            app_exit,
            inbox_list,
            inbox_get,
            inbox_mark_read,
            inbox_confirm_done,
            inbox_add_files,
            inbox_instruct,
            inbox_request_issue,
            inbox_issue_files,
            doc_read,
            doc_list_md,
            doc_watch,
            doc_unwatch,
            doc_open_url,
            doc_image,
            memo_read,
            memo_save,
            board_status,
            env_status,
            autostart_get,
            autostart_set,
            create_manager_tab,
        ])
        .setup(|app| {
            // load_all = load_settings + ensure_dotdir (incl. powershell profile) + load
            let appdata = appdata_dir();
            let loaded = config::load_all(&appdata);
            let ws_path = appdata.join("workspace.json");
            let mut startup_errors = Vec::new();
            // spec: MEMO.md (the memo next to the dispatcher) exists from the start; never overwritten
            if let Err(e) = memo::ensure(&loaded.settings.workspace_root) {
                startup_errors.push(format!("MEMO.md를 만들 수 없습니다 — {e}"));
            }
            let mut saving = true;
            // tab ids are trustworthy for the legacy inbox migration only in a restored layout
            let mut restored = false;
            let (ws, initial) = match persist::load(&ws_path) {
                Ok(Some(ws)) => restore_or_fresh(ws, &loaded, &ws_path, &mut saving, &mut startup_errors, &mut restored),
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
            // control server: bound before AppState exists (a failure becomes a startup error),
            // serving + ctrl_url before the initial spawns so role panes get the real URL
            let listener = match tauri::async_runtime::block_on(control::bind()) {
                Ok(l) => Some(l),
                Err(e) => {
                    eprintln!("[agentboard] 제어 서버 시작 실패: {e}");
                    startup_errors.push(format!(
                        "제어 서버를 시작할 수 없습니다: {e} — 역할 에이전트의 MCP 도구와 hook 보고가 동작하지 않습니다"
                    ));
                    None
                }
            };
            app.manage(AppState::new(ws, loaded, pty, sink, tx, saver, startup_errors, agentcfg::default_run_dir()));
            {
                // R29: items written before name-keyed ownership carry a tab id; map it once
                let state = app.state::<AppState>();
                let ws = state.ws.lock().unwrap().clone();
                let errs = state.inbox.migrate_legacy_owners(|tab, name| legacy_owner(&ws.tabs, restored, tab, name));
                for e in errs {
                    eprintln!("[agentboard] 인박스 항목 이전 실패: {e}");
                }
            }
            if let Some(listener) = listener {
                let handle = app.handle().clone();
                if let Err(e) = tauri::async_runtime::block_on(control::serve_on(handle, listener)) {
                    eprintln!("[agentboard] 제어 서버 시작 실패: {e}");
                }
            }
            // app → session messages: delivered off the hook/command threads
            bridge::start_worker(app.handle().clone());
            // document tabs: mtime polling (Task 14)
            start_doc_watcher(app.handle().clone());
            // nightly refresh scheduler (Task 11); last run date in %APPDATA%/agentboard/nightly.json
            nightly::start(app.handle().clone(), appdata.join(nightly::FILE_NAME));
            board::start_session_poll(app.handle().clone()); // plain claude panes (dashboard D3b)
            // spawning happens on the launcher thread: window creation is not blocked
            launcher::start(app.handle(), rx, initial);
            // environment check (Task 12): probes run off the UI thread; `env-changed` when done
            let handle = app.handle().clone();
            let _ = std::thread::Builder::new().name("envcheck".into()).spawn(move || {
                if let Some(state) = handle.try_state::<AppState>() {
                    let items = envcheck::refresh(&state);
                    let bad: Vec<&str> = items.iter().filter(|i| !i.ok).map(|i| i.id.as_str()).collect();
                    eprintln!("[agentboard] envcheck: items={} failing={bad:?}", items.len());
                }
            });

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
            // only on the real exit: an ExitRequested may be vetoed (Task 17) and the app goes on
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(state) = app.try_state::<AppState>() {
                    state.nightly.stop(); // the scheduler thread ends at its next wake
                }
            }
        });
}

/// Owner name of a pre-R29 inbox item that stored tab id `tab` and name `name`: the stored name
/// when a role tab still has it; otherwise the current name of tab `tab` — but only when the
/// layout was restored from workspace.json (a fresh start renumbers tab ids, so an id could
/// name another Manager). `None` = keep the stored name (owner unknown, dispatcher only).
fn legacy_owner(tabs: &TabMap, restored: bool, tab: &str, name: &str) -> Option<String> {
    if tabs.values().any(|t| t.name == name && t.role.is_some()) {
        return Some(name.to_string());
    }
    if !restored {
        return None;
    }
    tabs.get(tab).filter(|t| t.role.is_some()).map(|t| t.name.clone())
}

type TabMap = std::collections::BTreeMap<crate::workspace::TabId, crate::workspace::Tab>;

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

    /// R29 legacy migration: the stored name wins; a tab id is trusted only in a restored
    /// layout (a fresh start renumbers ids); otherwise the owner stays unknown.
    #[test]
    fn legacy_inbox_owner_mapping() {
        let mut tabs = TabMap::new();
        let mut tab = |id: &str, name: &str, role: Option<crate::roles::Role>| {
            let t = crate::workspace::Tab {
                id: id.into(),
                name: name.into(),
                dir: name.into(),
                template: "solo".into(),
                is_dispatcher: false,
                workspace: std::path::PathBuf::from(name),
                layout: crate::layout::LayoutNode::Leaf("p".into()),
                last_focused_pane_id: "p".into(),
                maximized_pane_id: None,
                sync_input: false,
                role,
                team_seq: Default::default(),
            };
            tabs.insert(id.into(), t);
        };
        use crate::roles::Role::Manager;
        tab("t2", "billing", Some(Manager));
        tab("t5", "ops", Some(Manager));
        tab("t7", "notes", None);
        assert_eq!(legacy_owner(&tabs, false, "t5", "billing").as_deref(), Some("billing"), "name still exists");
        assert_eq!(legacy_owner(&tabs, false, "t5", "renamed"), None, "fresh start: ids are not trusted");
        assert_eq!(legacy_owner(&tabs, true, "t5", "renamed").as_deref(), Some("ops"), "restored: id → current name");
        assert_eq!(legacy_owner(&tabs, true, "t7", "renamed"), None, "not a role tab");
        assert_eq!(legacy_owner(&tabs, true, "t9", "renamed"), None, "gone");
    }

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
            env_remove: vec![],
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

    struct NullUi;
    impl Ui for NullUi {
        fn workspace_changed(&self, _: &Snapshot) {}
        fn drop_channel(&self, _: &str) {}
    }

    /// AppState whose PtyManager runs an idle `cmd.exe` as pane "p" (PTY only, not in the model).
    struct WriteFx {
        state: AppState,
        _rx: mpsc::Receiver<Effects>,
        _keep: (tempfile::TempDir, tempfile::TempDir),
    }

    impl Drop for WriteFx {
        fn drop(&mut self) {
            self.state.pty.remove("p");
        }
    }

    fn write_fx() -> WriteFx {
        let root = tempfile::tempdir().unwrap();
        let appdata = tempfile::tempdir().unwrap();
        let s = serde_json::json!({ "workspaceRoot": root.path().to_string_lossy() });
        std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
        config::ensure_dotdir(root.path()).unwrap();
        let loaded = config::load_all(appdata.path());
        let ws = Workspace::from_config(&loaded);
        let (tx, rx) = mpsc::channel();
        let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
        pty.spawn("p", cmd(&[])).unwrap();
        let state =
            AppState::new(ws, loaded, pty, Arc::new(NullUi), tx, persist::Saver::disabled(), Vec::new(), appdata.path().join("run"));
        WriteFx { state, _rx: rx, _keep: (root, appdata) }
    }

    /// IPC contract of the memo: `memo_read` → `{path: "MEMO.md", text}` under workspaceRoot
    /// (created from the header draft when missing), `memo_save({text})` writes the whole file.
    #[test]
    fn memo_commands_use_the_workspace_root() {
        let f = write_fx();
        let root = workspace_root(&f.state);
        let m = memo_read_in(&f.state).unwrap();
        assert_eq!(serde_json::to_value(&m).unwrap(), serde_json::json!({"path": "MEMO.md", "text": memo::DEFAULT_MEMO}));
        assert_eq!(std::fs::read_to_string(root.join("MEMO.md")).unwrap(), memo::DEFAULT_MEMO);
        memo_save_in(&f.state, "x").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("MEMO.md")).unwrap(), "x");
        assert_eq!(std::fs::read_to_string(root.join(".agentboard/memo.bak")).unwrap(), memo::DEFAULT_MEMO);
    }

    /// The exit (flush up to 5 s, kill all) runs off the UI thread now, so the window stays
    /// responsive: a second close click meanwhile must not start a second exit.
    #[test]
    fn app_exit_runs_once() {
        let flag = std::sync::atomic::AtomicBool::new(false);
        assert!(begin_exit(&flag));
        assert!(!begin_exit(&flag), "an exit is already running");
        assert!(!begin_exit(&flag));
    }

    /// Terminal replies / mouse reports (pty_write_raw) are not user activity: they must not
    /// hold back the bridge's PTY fallback or the nightly quiet check (R4).
    #[test]
    fn raw_write_does_not_note_input_but_user_write_does() {
        let f = write_fx();
        write_raw(&f.state, "p", b"\x1b[I").unwrap();
        assert!(f.state.last_input.lock().unwrap().get("p").is_none(), "a focus report is not input");
        write_user(&f.state, "p", b"x").unwrap();
        assert!(f.state.last_input.lock().unwrap().get("p").is_some(), "a keystroke is input");
    }

    #[test]
    fn raw_write_rejects_oversized_payload() {
        let f = write_fx();
        let err = write_raw(&f.state, "p", &vec![b'a'; RAW_WRITE_MAX + 1]).unwrap_err();
        assert!(err.contains("너무 깁니다"), "{err}");
        assert!(f.state.last_input.lock().unwrap().get("p").is_none());
        // exactly the maximum passes the size check (an unknown pane then fails on its own)
        let err = write_raw(&f.state, "nope", &vec![b'a'; RAW_WRITE_MAX]).unwrap_err();
        assert!(err.contains("unknown pane"), "{err}");
    }
}
