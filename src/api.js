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

/** Apply a WsCommand (`{type: 'splitPane', ...}`); resolves to the Snapshot `{workspace, errors}`. */
export function send(cmd) {
  return invoke('ws_command', { cmd });
}

/** Current Snapshot `{workspace, errors}`. */
export function snapshot() {
  return invoke('ws_snapshot');
}

/** Called with a Snapshot after every model or pane status change; resolves to an unlisten fn. */
export function onWorkspaceChanged(fn) {
  return on('workspace-changed', fn);
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

/** `[[tabId, dir], ...]` for tabs whose working folder does not exist. */
export function missingDirs() {
  return invoke('missing_dirs');
}

/** Listen to a backend event; resolves to an unlisten function. */
export function on(event, fn) {
  return tauri().event.listen(event, (e) => fn(e.payload));
}
