// Memo applet pure logic (spec 2026-10-26-agentboard-memo-design.md §3, §5): MEMO.md is a header
// (writing rules, not shown) and a body (the user's list). The boundary is the FIRST line that is
// exactly `## 메모` after trimming; the body starts on the next line. Without that line the whole
// file is the body (the applet shows a warning). This is the only place that splits / joins:
// the backend stores whole texts. No DOM, no Tauri: unit-tested in Node. memo.js draws.

export const MEMO_MARKER = '## 메모';

const LINE = /[^\r\n]*(?:\r\n|\n|\r)?/g;

/** `{header, body, marker}`: header = everything up to and including the marker line (its line
 * break too), body = the rest. Line endings are kept as they are (`\r\n` stays). */
export function splitMemo(text) {
  const t = String(text ?? '');
  LINE.lastIndex = 0;
  for (let m = LINE.exec(t); m && m[0] !== ''; m = LINE.exec(t)) {
    const content = m[0].replace(/(?:\r\n|\n|\r)$/, '');
    if (content.trim() === MEMO_MARKER) {
      const end = m.index + m[0].length;
      return { header: t.slice(0, end), body: t.slice(end), marker: true };
    }
  }
  return { header: '', body: t, marker: false };
}

/** Inverse of splitMemo: header + body. A marker line that ended the file without a line break
 * gets one (in the header's style) before a non-empty body. Without a marker: the body alone. */
export function joinMemo(header, body, marker) {
  const b = String(body ?? '');
  if (!marker) return b;
  const h = String(header ?? '');
  if (b && h && !/(?:\r\n|\n|\r)$/.test(h)) return h + memoEol(h) + b;
  return h + b;
}

/** The file's line break: `\r\n` when the text uses it anywhere, else `\n`. */
export function memoEol(text) {
  return String(text ?? '').includes('\r\n') ? '\r\n' : '\n';
}

/** Every line break of `s` (`\r\n`, `\n`, `\r`) as `eol` — a textarea value is LF-only. */
export function normalizeEol(s, eol) {
  return String(s ?? '').replace(/\r\n|\n|\r/g, eol);
}

const READ_KEYS = {
  KeyE: 'edit',
  KeyR: 'reload',
  KeyJ: 'scrollDown',
  ArrowDown: 'scrollDown',
  KeyK: 'scrollUp',
  ArrowUp: 'scrollUp',
  KeyG: 'top',
  Home: 'top',
  End: 'bottom',
};
const SCROLLS = new Set(['scrollDown', 'scrollUp']);

/**
 * Memo action for a keydown (by `KeyboardEvent.code`), or null. `mode`: 'read' | 'edit'.
 * read: e edit, r reload, j/k/↑↓ scroll, g/G top/bottom (Shift only for G). edit: Ctrl+Enter
 * save, Esc cancel; every other key is typing (null). IME composition and Alt/Meta chords (app
 * shortcuts, taken by the key router first) are never memo keys; a held key repeats only scrolling.
 */
export function memoKeyAction(e, mode) {
  if (!e || e.isComposing || e.altKey || e.metaKey) return null;
  if (mode === 'edit') {
    if (e.repeat || e.shiftKey) return null;
    if (e.ctrlKey && (e.code === 'Enter' || e.code === 'NumpadEnter')) return 'save';
    if (!e.ctrlKey && e.code === 'Escape') return 'cancel';
    return null;
  }
  if (e.ctrlKey) return null;
  const a = e.shiftKey ? (e.code === 'KeyG' ? 'bottom' : null) : READ_KEYS[e.code] || null;
  if (a && e.repeat && !SCROLLS.has(a)) return null;
  return a;
}
