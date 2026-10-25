// Key chords: parsing/matching on KeyboardEvent.code (pure, IME-safe) + the DOM key router.
// No top-level DOM access: the pure helpers are unit-tested in Node.

const SYMBOLS = {
  '\\': 'Backslash',
  '-': 'Minus',
  ';': 'Semicolon',
  '[': 'BracketLeft',
  ']': 'BracketRight',
  '=': 'Equal',
  ',': 'Comma',
  '.': 'Period',
  '/': 'Slash',
  "'": 'Quote',
  '`': 'Backquote',
};

const NAMED = {
  tab: 'Tab',
  left: 'ArrowLeft',
  right: 'ArrowRight',
  up: 'ArrowUp',
  down: 'ArrowDown',
  enter: 'Enter',
  esc: 'Escape',
  escape: 'Escape',
  space: 'Space',
  backspace: 'Backspace',
  delete: 'Delete',
  home: 'Home',
  end: 'End',
  pageup: 'PageUp',
  pagedown: 'PageDown',
  insert: 'Insert',
  plus: 'Equal', // '+' is Shift+Equal on US/KR layouts
  comma: 'Comma',
};

/** One key token (lowercased) → KeyboardEvent.code, or null. */
function tokenCode(t) {
  if (t.length === 1) {
    if (t >= 'a' && t <= 'z') return `Key${t.toUpperCase()}`;
    if (t >= '0' && t <= '9') return `Digit${t}`;
    return SYMBOLS[t] || null;
  }
  if (NAMED[t]) return NAMED[t];
  let m = /^f([1-9]|1[0-2])$/.exec(t);
  if (m) return `F${m[1]}`;
  // code names are accepted as well (e.g. passthroughKeys "Ctrl+KeyR")
  m = /^key([a-z])$/.exec(t);
  if (m) return `Key${m[1].toUpperCase()}`;
  m = /^digit([0-9])$/.exec(t);
  if (m) return `Digit${m[1]}`;
  const byCode = Object.values(SYMBOLS).find((c) => c.toLowerCase() === t);
  return byCode || null;
}

/**
 * `"Ctrl+Alt+Shift+H"` → `{ctrl, alt, shift, code}` (case-insensitive, same grammar as
 * config.rs parse_key). Letters → `KeyX`, digits → `DigitN`, `\` → `Backslash`, `-` → `Minus`,
 * `;` → `Semicolon`, `[`/`]` → `BracketLeft/Right`, `tab` → `Tab`. Invalid → null.
 */
export function parseChord(str) {
  if (typeof str !== 'string') return null;
  const s = str.trim();
  if (!s) return null;
  const chord = { ctrl: false, alt: false, shift: false, code: null };
  for (const raw of s.split('+')) {
    const t = raw.trim().toLowerCase();
    if (!t) return null;
    const mod = t === 'ctrl' || t === 'control' ? 'ctrl' : t === 'alt' ? 'alt' : t === 'shift' ? 'shift' : null;
    if (mod) {
      if (chord[mod]) return null;
      chord[mod] = true;
      continue;
    }
    const code = tokenCode(t);
    if (!code || chord.code) return null;
    chord.code = code;
  }
  return chord.code ? chord : null;
}

/** Canonical map key for a chord: `"CAS:KeyH"` with `-` for absent modifiers. */
export function chordKey(c) {
  return `${c.ctrl ? 'C' : '-'}${c.alt ? 'A' : '-'}${c.shift ? 'S' : '-'}:${c.code}`;
}

function eventKey(e) {
  if (!e || !e.code || e.metaKey) return null; // Win-key combos are never ours
  return chordKey({ ctrl: !!e.ctrlKey, alt: !!e.altKey, shift: !!e.shiftKey, code: e.code });
}

/** `{id: "Alt+h" | null}` → Map<chordKey, id>. Null (unbound) and invalid strings are skipped. */
export function buildKeymap(bindings) {
  const km = new Map();
  for (const [id, str] of Object.entries(bindings || {})) {
    const c = parseChord(str);
    if (c) km.set(chordKey(c), id);
  }
  return km;
}

/** Command id for a keyboard event: exact `e.code` + ctrl/alt/shift. `e.key`/`isComposing` ignored. */
export function matchEvent(keymap, e) {
  if (!keymap) return null;
  const k = eventKey(e);
  return (k && keymap.get(k)) || null;
}

/** True if the event equals one of the profile's `passthroughKeys` chords. */
export function isPassthrough(chordStrs, e) {
  const k = eventKey(e);
  if (!k || !Array.isArray(chordStrs)) return false;
  return chordStrs.some((s) => {
    const c = parseChord(s);
    return !!c && chordKey(c) === k;
  });
}

const REPEATABLE = /^(pane\.focus|pane\.resize|tab\.(prev|next)$)/;

/** Held-down keys repeat only for movement/resize; split/close/etc. run once per press. */
export function isRepeatable(id) {
  return REPEATABLE.test(id);
}

// ---------------------------------------------------------------- DOM router

let router = null;

/**
 * Install the app key router (document keydown, capture phase).
 * opts: `keymap` (from buildKeymap), `getPassthrough(paneId) -> string[]` (sync; cached by the
 * caller), `run(id, event)`, optional `focusedPane() -> paneId` (used when the event target is
 * not inside a `.pane`).
 * A matched key is swallowed (preventDefault + stopPropagation) and `run(id)` is called, also
 * during IME composition; ids without a handler are swallowed as no-ops by the caller.
 * Returns `{setKeymap(km), setModal(fn|null), termKeyFilter(paneId, e), dispose()}`.
 */
export function installKeyRouter(opts) {
  if (router) router.dispose();
  let keymap = opts.keymap || new Map();
  let modal = null;

  const paneOf = (e) => {
    const el = e && e.target && e.target.closest ? e.target.closest('.pane[data-pane]') : null;
    if (el) return el.dataset.pane;
    return opts.focusedPane ? opts.focusedPane() : null;
  };

  /** id the app owns for this event in `paneId`, or null (unmatched or passthrough). */
  const appId = (e, paneId) => {
    const id = matchEvent(keymap, e);
    if (!id) return null;
    if (paneId && isPassthrough(opts.getPassthrough(paneId), e)) return null;
    return id;
  };

  const onKeyDown = (e) => {
    if (modal && modal(e)) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const id = appId(e, paneOf(e));
    if (!id) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.repeat && !isRepeatable(id)) return;
    opts.run(id, e);
  };

  // S1: a lone Alt press/release can activate the window menu (focus loss, beep) in WebView2.
  // Swallow the Alt keyup and the keyup of chords we handled; nothing downstream needs them.
  const onKeyUp = (e) => {
    if (e.key === 'Alt' || e.code === 'AltLeft' || e.code === 'AltRight' || appId(e, paneOf(e))) {
      e.preventDefault();
    }
  };

  document.addEventListener('keydown', onKeyDown, true);
  document.addEventListener('keyup', onKeyUp, true);

  router = {
    setKeymap(km) {
      keymap = km || new Map();
    },
    /** While set, `fn(e)` sees every keydown first; returning true consumes it (overlays). */
    setModal(fn) {
      modal = fn || null;
    },
    /** For xterm attachCustomKeyEventHandler: false = xterm must not process the key. */
    termKeyFilter(paneId, e) {
      return !appId(e, paneId);
    },
    dispose() {
      document.removeEventListener('keydown', onKeyDown, true);
      document.removeEventListener('keyup', onKeyUp, true);
      if (router === self) router = null;
    },
  };
  const self = router;
  return self;
}

/** The installed router (or null). */
export function keyRouter() {
  return router;
}
