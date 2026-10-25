import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  resolveDocLink,
  resolveAssetSrc,
  joinRel,
  createHistory,
  extractToc,
  slugify,
  SANITIZE_CONFIG,
  FINAL_SANITIZE_CONFIG,
  prefilterHtml,
  docKeyAction,
  findMatches,
  fileModeKey,
  docTitle,
  headingId,
  MERMAID_SANITIZE_CONFIG,
  mermaidAttrAllowed,
  stripCssUrls,
  LINK_SELECTOR,
  imgAltText,
} from '../../src/docview-logic.js';
import { docChoices, paletteItems } from '../../src/overlay-logic.js';

test('relative md links resolve against current doc and stay inside root', () => {
  const cur = 'billing/issues/20261025-x/output/보고서.md';
  assert.deepEqual(resolveDocLink(cur, 'plan.md'), { type: 'doc', rel: 'billing/issues/20261025-x/output/plan.md', hash: '' });
  assert.deepEqual(resolveDocLink(cur, './sub/a.md#결론'), { type: 'doc', rel: 'billing/issues/20261025-x/output/sub/a.md', hash: '결론' });
  assert.deepEqual(resolveDocLink(cur, '../input/%EC%9E%90%EB%A3%8C.md'), {
    type: 'doc',
    rel: 'billing/issues/20261025-x/input/자료.md',
    hash: '',
  });
  assert.deepEqual(resolveDocLink(cur, '../../../../HANDOVER.md'), { type: 'doc', rel: 'HANDOVER.md', hash: '' });
  // leaving workspaceRoot → ignored
  assert.deepEqual(resolveDocLink(cur, '../../../../../outside.md'), { type: 'ignore' });
  assert.deepEqual(resolveDocLink('a.md', '../a.md'), { type: 'ignore' });
  // backslashes are separators; '/x' is workspace-root relative
  assert.deepEqual(resolveDocLink('a/b.md', '..\\c\\d.MD'), { type: 'doc', rel: 'c/d.MD', hash: '' });
  assert.deepEqual(resolveDocLink('a/b.md', '/docs/x.md'), { type: 'doc', rel: 'docs/x.md', hash: '' });
  // anchors, external, everything else
  assert.deepEqual(resolveDocLink(cur, '#개요'), { type: 'anchor', id: '개요' });
  assert.deepEqual(resolveDocLink(cur, 'https://example.com/a?b=1'), { type: 'external', url: 'https://example.com/a?b=1' });
  assert.deepEqual(resolveDocLink(cur, 'HTTP://example.com'), { type: 'external', url: 'http://example.com/' });
  for (const bad of [
    'javascript:alert(1)',
    ' JavaScript:alert(1)',
    'file:///C:/x.md',
    'C:\\Users\\x\\a.md',
    'c:/a.md',
    'data:text/html,<b>x</b>',
    'mailto:a@b',
    '//evil.example/a.md',
    '\\\\server\\share\\a.md',
    'notes.txt',
    'image.png',
    '',
    'a.md\u0000.png',
    '%E0%A4%A.md',
    'https://',
  ]) {
    assert.deepEqual(resolveDocLink(cur, bad), { type: 'ignore' }, bad);
  }
  assert.equal(joinRel('a/b', '../../..'), null);
  assert.equal(joinRel('', 'x/./y/../z.md'), 'x/z.md');
  // images: only relative paths inside the root (loaded through doc_image); everything else
  // — data:, remote, rooted, UNC (also percent-encoded), drive paths — is dropped (alt text)
  assert.deepEqual(resolveAssetSrc('docs/a.md', 'img/p.png'), { type: 'local', rel: 'docs/img/p.png' });
  assert.deepEqual(resolveAssetSrc('docs/a.md', '../%ED%95%9C.png'), { type: 'local', rel: '한.png' });
  for (const bad of [
    '../../x.png',
    'https://example.com/t.png',
    'file:///C:/x.png',
    'C:\\x.png',
    'C:x.png',
    '//h/x.png',
    '\\\\attacker\\x.png',
    '%5C%5Cattacker%5Cx.png',
    '\\\\?\\C:\\x.png',
    '/abs.png',
    'data:image/png;base64,AAAA',
    'http://asset.localhost/%5C%5Cattacker.example%5Cs%5Cx.png',
    'javascript:x',
    '',
  ]) {
    assert.deepEqual(resolveAssetSrc('docs/a.md', bad), { type: 'drop' }, bad);
  }
  assert.equal(docTitle('a/b/보고서.md'), '보고서.md');
});

test('history back/forward', () => {
  const h = createHistory({ rel: 'a.md', scroll: 0 });
  assert.equal(h.current().rel, 'a.md');
  assert.equal(h.canBack(), false);
  assert.equal(h.back(0), null);
  h.push({ rel: 'b.md', scroll: 0 }, 120); // leaving a.md at scroll 120
  h.push({ rel: 'c.md', scroll: 0 }, 30);
  assert.equal(h.current().rel, 'c.md');
  const b = h.back(55); // leaving c.md at 55
  assert.deepEqual(b, { rel: 'b.md', scroll: 30 });
  assert.deepEqual(h.back(7), { rel: 'a.md', scroll: 120 });
  assert.equal(h.back(0), null);
  assert.deepEqual(h.forward(0), { rel: 'b.md', scroll: 7 });
  assert.deepEqual(h.forward(1), { rel: 'c.md', scroll: 55 });
  assert.equal(h.forward(0), null);
  // a push after going back drops the forward entries
  h.back(0);
  h.push({ rel: 'd.md', scroll: 0 }, 0);
  assert.equal(h.canForward(), false);
  assert.deepEqual(h.entries().map((e) => e.rel), ['a.md', 'b.md', 'd.md']);
  // pushing the current doc again is a no-op
  h.push({ rel: 'd.md', scroll: 0 }, 0);
  assert.equal(h.entries().length, 3);
  // capped
  const big = createHistory({ rel: '0.md', scroll: 0 }, 5);
  for (let i = 1; i < 10; i++) big.push({ rel: `${i}.md`, scroll: 0 }, 0);
  assert.equal(big.entries().length, 5);
  assert.equal(big.current().rel, '9.md');
});

test('toc extraction from headings incl. duplicates', () => {
  assert.equal(slugify('  Hello, World! '), 'hello-world');
  assert.equal(slugify('결론 및 다음 단계'), '결론-및-다음-단계');
  assert.equal(slugify('!!!'), 'section');
  const toc = extractToc([
    { level: 1, text: '개요' },
    { level: 2, text: 'Setup' },
    { level: 2, text: 'Setup' },
    { level: 3, text: 'setup' },
    { level: 2, text: 'Setup-1' },
    { level: 2, text: '' },
  ]);
  assert.deepEqual(
    toc.map((t) => t.id),
    ['개요', 'setup', 'setup-1', 'setup-2', 'setup-1-1', 'section'],
  );
  assert.deepEqual(toc[1], { level: 2, text: 'Setup', id: 'setup', label: '  Setup' });
  assert.equal(toc[0].label, '개요');
  assert.equal(toc[3].label, '    setup');
});

test('sanitizer strips script and on* attributes', () => {
  // DOMPurify config (the real sanitizer runs in the WebView; Node has no DOM)
  for (const cfg of [SANITIZE_CONFIG, FINAL_SANITIZE_CONFIG]) {
    assert.ok(Object.isFrozen(cfg));
    for (const t of ['script', 'iframe', 'object', 'embed', 'form', 'base', 'meta', 'link', 'style', 'area', 'map', 'portal']) {
      assert.ok(cfg.FORBID_TAGS.includes(t), t);
    }
    for (const a of ['target', 'formaction', 'srcset', 'ping']) assert.ok(cfg.FORBID_ATTR.includes(a), a);
    assert.equal(cfg.ALLOW_DATA_ATTR, false);
    assert.equal(cfg.ALLOW_UNKNOWN_PROTOCOLS, false);
    assert.equal(cfg.ADD_ATTR, undefined);
    assert.equal(cfg.ADD_TAGS, undefined);
  }
  // first pass: agent-written HTML gets no inline style; the final pass (after KaTeX) allows it
  assert.ok(SANITIZE_CONFIG.FORBID_ATTR.includes('style'));
  assert.ok(!FINAL_SANITIZE_CONFIG.FORBID_ATTR.includes('style'));
  assert.equal(SANITIZE_CONFIG.SANITIZE_NAMED_PROPS, true);
  assert.deepEqual(FINAL_SANITIZE_CONFIG.USE_PROFILES, { html: true, svg: true, mathMl: true });
  // the document's own svg/math never survive the first pass
  assert.ok(SANITIZE_CONFIG.FORBID_TAGS.includes('svg') && SANITIZE_CONFIG.FORBID_TAGS.includes('math'));
  assert.ok(FINAL_SANITIZE_CONFIG.FORBID_TAGS.includes('foreignobject'));

  // string pre-filter (defence in depth before DOMPurify)
  const dirty =
    '<p onclick="x()">a</p><SCRIPT>alert(1)</SCRIPT><img src=x onerror=alert(1)>' +
    '<a href="javascript:alert(1)" onMouseOver=\'y()\'>l</a><script src="e.js"></script>' +
    '<svg/onload=alert(1)><iframe src="https://e"></iframe><style>*{}</style><b>ok</b><script>unclosed';
  const out = prefilterHtml(dirty);
  assert.doesNotMatch(out, /<script/i);
  assert.doesNotMatch(out, /\son[a-z]+\s*=/i);
  assert.doesNotMatch(out, /\/on[a-z]+\s*=/i);
  assert.doesNotMatch(out, /javascript:/i);
  assert.doesNotMatch(out, /<iframe/i);
  assert.doesNotMatch(out, /<style/i);
  assert.match(out, /<b>ok<\/b>/);
  assert.match(out, /<p>a<\/p>/);
  // text that merely mentions the words is kept
  assert.equal(prefilterHtml('<p>one=1 onboarding</p>'), '<p>one=1 onboarding</p>');
});

const key = (code, o = {}) => ({ code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, repeat: false, ...o });

test('key map: j/k/g/G/o/[ ]/r', () => {
  assert.equal(docKeyAction(key('KeyJ')), 'lineDown');
  assert.equal(docKeyAction(key('KeyK')), 'lineUp');
  assert.equal(docKeyAction(key('ArrowDown')), 'lineDown');
  assert.equal(docKeyAction(key('ArrowUp')), 'lineUp');
  assert.equal(docKeyAction(key('Space')), 'pageDown');
  assert.equal(docKeyAction(key('Space', { shiftKey: true })), 'pageUp');
  assert.equal(docKeyAction(key('PageDown')), 'pageDown');
  assert.equal(docKeyAction(key('PageUp')), 'pageUp');
  assert.equal(docKeyAction(key('KeyG')), 'top');
  assert.equal(docKeyAction(key('KeyG', { shiftKey: true })), 'bottom');
  assert.equal(docKeyAction(key('Home')), 'top');
  assert.equal(docKeyAction(key('End')), 'bottom');
  assert.equal(docKeyAction(key('Slash')), 'search');
  assert.equal(docKeyAction(key('KeyN')), 'next');
  assert.equal(docKeyAction(key('KeyN', { shiftKey: true })), 'prev');
  assert.equal(docKeyAction(key('KeyO')), 'toc');
  assert.equal(docKeyAction(key('BracketLeft')), 'back');
  assert.equal(docKeyAction(key('BracketRight')), 'forward');
  assert.equal(docKeyAction(key('KeyR')), 'reload');
  // app chords and IME belong to others; Shift only where it means something
  assert.equal(docKeyAction(key('KeyJ', { ctrlKey: true })), null);
  assert.equal(docKeyAction(key('KeyO', { altKey: true })), null);
  assert.equal(docKeyAction(key('KeyR', { metaKey: true })), null);
  assert.equal(docKeyAction(key('KeyJ', { shiftKey: true })), null);
  assert.equal(docKeyAction(key('KeyJ', { isComposing: true })), null);
  assert.equal(docKeyAction(key('KeyX')), null);
  assert.equal(docKeyAction(null), null);
  // held keys repeat only for scrolling
  assert.equal(docKeyAction(key('KeyJ', { repeat: true })), 'lineDown');
  assert.equal(docKeyAction(key('Space', { repeat: true })), 'pageDown');
  assert.equal(docKeyAction(key('KeyR', { repeat: true })), null);
  assert.equal(docKeyAction(key('KeyO', { repeat: true })), null);
  assert.equal(docKeyAction(key('BracketLeft', { repeat: true })), null);
});

test('in-document search finds case-insensitive matches', () => {
  assert.deepEqual(findMatches('Abc abc ABC', 'abc'), [0, 4, 8]);
  assert.deepEqual(findMatches('aaaa', 'aa'), [0, 2]);
  assert.deepEqual(findMatches('결제 실패, 결제', '결제'), [0, 7]);
  assert.deepEqual(findMatches('x', ''), []);
  assert.deepEqual(findMatches('x', '   '), []);
});

test('inbox issue files: Tab enters the file list, j/k/Enter/Esc inside it', () => {
  assert.equal(fileModeKey(key('KeyJ')), 'down');
  assert.equal(fileModeKey(key('ArrowDown')), 'down');
  assert.equal(fileModeKey(key('KeyK')), 'up');
  assert.equal(fileModeKey(key('Enter')), 'open');
  assert.equal(fileModeKey(key('NumpadEnter')), 'open');
  assert.equal(fileModeKey(key('Escape')), 'leave');
  assert.equal(fileModeKey(key('Tab')), 'leave');
  assert.equal(fileModeKey(key('Tab', { shiftKey: true })), 'leave');
  assert.equal(fileModeKey(key('KeyJ', { ctrlKey: true })), null);
  assert.equal(fileModeKey(key('Enter', { repeat: true })), null);
  assert.equal(fileModeKey(key('KeyI')), null);
});

test('palette: 문서 열기 entry and capped fuzzy doc choices', () => {
  const ws = {
    groupLayout: { group: 'g1' },
    groups: { g1: { id: 'g1', name: 'main', tabIds: ['t1'], activeTabId: 't1' } },
    tabs: { t1: { id: 't1', name: 'dispatcher', isDispatcher: true, layout: { pane: 'p1' }, lastFocusedPaneId: 'p1' } },
    panes: { p1: { id: 'p1', name: 'dispatcher', tab: 't1' } },
    focus: { groupId: 'g1' },
  };
  const it = paletteItems(ws, {}).find((i) => i.label === '문서 열기');
  assert.deepEqual(it && it.action, { type: 'openDoc' });
  const paths = ['billing/issues/x/output/보고서.md', 'HANDOVER.md', 'api/README.md'];
  assert.deepEqual(docChoices(paths, '보고'), ['billing/issues/x/output/보고서.md']);
  assert.deepEqual(docChoices(paths, ''), paths);
  const many = Array.from({ length: 1000 }, (_, i) => `d/${i}.md`);
  assert.equal(docChoices(many, '', 200).length, 200);
  assert.equal(docChoices(many, 'd/1', 50).length, 50);
});

test('heading ids are namespaced: no document can produce an app id', () => {
  assert.equal(headingId('p12', 'status'), 'doc-p12-status');
  assert.equal(headingId('p12', '개요'), 'doc-p12-개요');
  for (const appId of ['status', 'groups', 'parking', 'toasts', 'sidebar', 'main', 'mermaid']) {
    const toc = extractToc([{ level: 2, text: appId }]);
    assert.notEqual(headingId('p1', toc[0].id), appId);
    assert.ok(headingId('p1', toc[0].id).startsWith('doc-p1-'));
  }
  // pass 1 keeps DOMPurify's clobbering guards on (ids/names of raw HTML get user-content-)
  assert.equal(SANITIZE_CONFIG.SANITIZE_DOM, true);
  assert.equal(SANITIZE_CONFIG.SANITIZE_NAMED_PROPS, true);
  assert.equal(FINAL_SANITIZE_CONFIG.SANITIZE_DOM, true);
});

test('links: every href-bearing element is intercepted', () => {
  for (const sel of ['a', 'area', '[href]', '[*|href]']) assert.ok(LINK_SELECTOR.split(',').map((x) => x.trim()).includes(sel), sel);
});

test('mermaid output is re-sanitized: no images, scripts, foreign links or css urls', () => {
  const c = MERMAID_SANITIZE_CONFIG;
  assert.ok(Object.isFrozen(c));
  assert.deepEqual(c.USE_PROFILES, { svg: true, svgFilters: true, html: true });
  for (const t of ['img', 'image', 'use', 'script', 'a', 'iframe', 'object', 'embed', 'form', 'area', 'map', 'feimage', 'link', 'meta', 'base', 'audio', 'video']) {
    assert.ok(c.FORBID_TAGS.includes(t), t);
  }
  assert.equal(c.ALLOW_DATA_ATTR, false);
  // href/src only as in-document anchors
  assert.equal(mermaidAttrAllowed('href', '#arrow'), true);
  assert.equal(mermaidAttrAllowed('xlink:href', '#m1'), true);
  for (const [n, v] of [
    ['href', 'https://e/'],
    ['xlink:href', 'http://asset.localhost/%5C%5Cx'],
    ['src', 'x.png'],
    ['href', '//h/x'],
    ['href', ''],
    ['onload', 'x'],
    ['onclick', '#x'],
  ]) {
    assert.equal(mermaidAttrAllowed(n, v), false, `${n}=${v}`);
  }
  assert.equal(mermaidAttrAllowed('class', 'node'), true);
  // style text: only url(#…) survives
  assert.equal(stripCssUrls('marker-end:url(#arrow);'), 'marker-end:url(#arrow);');
  assert.equal(stripCssUrls("fill:url( '#g' )"), "fill:url( '#g' )");
  assert.equal(stripCssUrls('background:url(http://asset.localhost/%5C%5Ca%5Cb.png)'), 'background:none');
  assert.equal(stripCssUrls('a{b:url("//host/x")} c{d:URL(x.png)}'), 'a{b:none} c{d:none}');
  assert.equal(stripCssUrls('@import "//evil/x.css"; .a{}'), ' .a{}');
  assert.equal(stripCssUrls('x:image-set("a.png" 1x)'), 'x:none');
});

test('blocked images become their alt text', () => {
  assert.equal(imgAltText('로고'), '[이미지: 로고]');
  assert.equal(imgAltText(''), '[이미지]');
  assert.equal(imgAltText(null), '[이미지]');
});

test('prefilter is linear on unclosed tags', () => {
  const big = '<style'.repeat(200000);
  const t0 = Date.now();
  const out = prefilterHtml(big);
  assert.ok(Date.now() - t0 < 2000, `${Date.now() - t0} ms`);
  assert.doesNotMatch(out, /<style/i);
  assert.equal(prefilterHtml('<p>a</p><script>x</script><p>b</p>'), '<p>a</p><p>b</p>');
  assert.equal(prefilterHtml('a<script>never closed <p>b</p>'), 'a');
});
