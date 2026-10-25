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

export async function send(_cmd) {
  throw new Error('not implemented until Task 9');
}

/** Listen to a backend event; resolves to an unlisten function. */
export function on(event, fn) {
  return tauri().event.listen(event, (e) => fn(e.payload));
}
