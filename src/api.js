// Thin wrappers over the Tauri global API. No top-level window access (importable in Node).
const tauri = () => globalThis.__TAURI__;

export function toBytes(v) {
  if (v instanceof Uint8Array) return v;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (Array.isArray(v)) return Uint8Array.from(v);
  return new Uint8Array(0);
}

/**
 * Subscribe to a pane's output. Live bytes arrive through onBytes(Uint8Array) as raw
 * binary; resolves with the snapshot (Uint8Array) taken atomically with the subscription.
 * Live chunks may be delivered before this promise resolves; the caller must buffer them.
 */
export async function attach(paneId, onBytes) {
  const { invoke, Channel } = tauri().core;
  const channel = new Channel();
  channel.onmessage = (msg) => onBytes(toBytes(msg));
  const snap = await invoke('pty_attach', { pane: paneId, channel });
  return toBytes(snap);
}

export function write(paneId, str) {
  return tauri().core.invoke('pty_write', { pane: paneId, data: str });
}

export function resize(paneId, cols, rows) {
  return tauri().core.invoke('pty_resize', { pane: paneId, cols, rows });
}

const invoke = (name, args) => tauri().core.invoke(name, args);

/**
 * Version gate (R16): snapshots are emitted from several backend threads and may arrive out
 * of order. `accept(snap)` is true only for a version greater than the last accepted one.
 * Pure — no Tauri access — so it is unit-testable.
 */
export function makeVersionGate() {
  let last = 0;
  return {
    accept(snap) {
      if (!snap || typeof snap.version !== 'number' || snap.version <= last) return false;
      last = snap.version;
      return true;
    },
    get last() {
      return last;
    },
  };
}

// Module-wide last-seen snapshot version shared by send/snapshot/onWorkspaceChanged.
const gate = makeVersionGate();

/**
 * Apply a WsCommand (`{type: 'splitPane', ...}`). Resolves to the Snapshot
 * `{workspace, errors, version}` it produced, or `null` when a newer snapshot was already
 * delivered (the caller then already has newer state). Rejects with the Korean error.
 */
export async function send(cmd) {
  const snap = await invoke('ws_command', { cmd });
  return gate.accept(snap) ? snap : null;
}

/** Current Snapshot `{workspace, errors, version}` (always returned; also advances the gate). */
export async function snapshot() {
  const snap = await invoke('ws_snapshot');
  gate.accept(snap);
  return snap;
}

/**
 * Called with a Snapshot after every model or pane status change, skipping snapshots older
 * than the last delivered one; resolves to an unlisten fn.
 */
export function onWorkspaceChanged(fn) {
  return on('workspace-changed', (snap) => {
    if (gate.accept(snap)) fn(snap);
  });
}

/** Config error lines (`파일:줄:열 — 원인`), job object problems included. */
export function configErrors() {
  return invoke('config_errors');
}

/** Action → key string map from keybindings.json (defaults merged). */
export function keybindings() {
  return invoke('get_keybindings');
}

/** Keys the pane's agent profile wants delivered to the PTY instead of the app. */
export function passthrough(paneId) {
  return invoke('get_passthrough', { pane: paneId });
}

/** Directory names under workspaceRoot (no dot-dirs), sorted. */
export function listDirs() {
  return invoke('list_workspace_dirs');
}

/** Create `workspaceRoot/<name>`; rejects with a Korean message. */
export function createDir(name) {
  return invoke('create_workspace_dir', { name });
}

export function listTemplates() {
  return invoke('list_templates');
}

/** `config.json` defaultTemplate name. */
export function defaultTemplate() {
  return invoke('default_template');
}

/** `[[tabId, dir], ...]` for tabs whose working folder does not exist. */
export function missingDirs() {
  return invoke('missing_dirs');
}

/** Listen to a backend event; resolves to an unlisten function. */
export function on(event, fn) {
  return tauri().event.listen(event, (e) => fn(e.payload));
}

/** Write text to the system clipboard (clipboard-manager plugin). */
export function writeClipboardText(text) {
  return tauri().clipboardManager.writeText(text);
}

/** Clipboard text, '' when empty / not text. */
export async function readClipboardText() {
  const t = await tauri().clipboardManager.readText();
  return typeof t === 'string' ? t : '';
}

export function runningCount() {
  return invoke('running_count');
}

export function appExit() {
  return invoke('app_exit');
}

// ---------------------------------------------------------------- inbox (Task 6 / Task 10)

/** `{version, items}`: every item (done ones too with `showDone`), newest first. */
export function inboxList(showDone) {
  return invoke('inbox_list', { showDone: !!showDone });
}

export function inboxMarkRead(id) {
  return invoke('inbox_mark_read', { id });
}

/** close_proposed → done; resolves to a Korean warning (string) when the Manager was not told, else null. */
export function inboxConfirmDone(id) {
  return invoke('inbox_confirm_done', { id });
}

/** Copy files into the item's issue `input|output`; resolves `{files: string[], warning: string|null}`. */
export function inboxAddFiles(id, paths, target) {
  return invoke('inbox_add_files', { id, paths, target });
}

/** Record the user's instruction and relay it to the dispatcher (rejects with a Korean error). */
export function inboxInstruct(id, text) {
  return invoke('inbox_instruct', { id, text });
}

/** Ask (via the dispatcher) the owner Manager to create an issue for the item. */
export function inboxRequestIssue(id) {
  return invoke('inbox_request_issue', { id });
}

/** `{input, output}` file lists (`{name, path, isMd}`) of the item's issue folder. */
export function inboxIssueFiles(id) {
  return invoke('inbox_issue_files', { id });
}

// ---------------------------------------------------------------- document tabs (Task 14)

/** `{text, mtime, rel, path, root}`; the backend validates `path` on every call. */
export function docRead(path) {
  return invoke('doc_read', { path });
}

/** `.md` files under workspaceRoot (relative, `/`), at most 5,000. */
export function docListMd(query = null) {
  return invoke('doc_list_md', { query });
}

/** Poll the document shown in `paneId` (1 s); a change emits `doc-changed {paneId}`. */
export function docWatch(paneId, path) {
  return invoke('doc_watch', { paneId, path });
}

export function docUnwatch(paneId) {
  return invoke('doc_unwatch', { paneId });
}

/** http(s) only, opened in the system browser by the backend. */
export function docOpenUrl(url) {
  return invoke('doc_open_url', { url });
}

/** A document image (`src` as written in document `doc`) as a data: URL; validated in Rust. */
export function docImage(doc, src) {
  return invoke('doc_image', { doc, src });
}
