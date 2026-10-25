// node_modules 의 브라우저용 빌드를 src/vendor/ 로 복사한다 (번들러 없음, CDN 없음).
// - xterm: ESM 빌드와 CSS
// - 문서 탭(Task 14): marked, DOMPurify, KaTeX(+woff2 글꼴), highlight.js(코어 + 일부 언어), mermaid
// 버전은 package.json 에 정확히 고정한다. 라이선스 파일은 src/vendor/licenses/ 에 함께 둔다.
import { copyFileSync, mkdirSync, existsSync, readFileSync, writeFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const nm = join(root, 'node_modules');
const out = join(root, 'src', 'vendor');
mkdirSync(out, { recursive: true });

function need(src) {
  if (!existsSync(src)) {
    console.error(`missing: ${src}`);
    process.exit(1);
  }
  return src;
}

/** Copy, dropping a trailing `//# sourceMappingURL=` line (the maps are not vendored). */
function copy(from, to, { stripMap = false } = {}) {
  const src = need(join(nm, from));
  const dst = join(out, to);
  mkdirSync(dirname(dst), { recursive: true });
  if (stripMap) {
    const text = readFileSync(src, 'utf8').replace(/\n\/\/# sourceMappingURL=\S+\s*$/, '\n');
    writeFileSync(dst, text);
  } else {
    copyFileSync(src, dst);
  }
  console.log(`vendor: ${to}`);
}

const files = [
  ['@xterm/xterm/lib/xterm.mjs', 'xterm.mjs'],
  ['@xterm/xterm/css/xterm.css', 'xterm.css'],
  ['@xterm/addon-fit/lib/addon-fit.mjs', 'addon-fit.mjs'],
  ['@xterm/addon-unicode11/lib/addon-unicode11.mjs', 'addon-unicode11.mjs'],
  ['@xterm/addon-search/lib/addon-search.mjs', 'addon-search.mjs'],
  ['@xterm/addon-webgl/lib/addon-webgl.mjs', 'addon-webgl.mjs'],
];
for (const [from, to] of files) copy(from, to);

// ---- document tabs (Task 14)
copy('marked/lib/marked.esm.js', 'marked.esm.js', { stripMap: true });
copy('dompurify/dist/purify.es.mjs', 'purify.es.mjs', { stripMap: true });
copy('katex/dist/katex.mjs', 'katex/katex.mjs');
copy('katex/dist/contrib/auto-render.mjs', 'katex/contrib/auto-render.mjs');
copy('katex/dist/katex.min.css', 'katex/katex.min.css');
for (const f of readdirSync(need(join(nm, 'katex/dist/fonts')))) {
  if (f.endsWith('.woff2')) copy(`katex/dist/fonts/${f}`, `katex/fonts/${f}`);
}
// mermaid: the single-file IIFE build (sets globalThis.mermaid); loaded lazily by docview.js
copy('mermaid/dist/mermaid.min.js', 'mermaid.min.js', { stripMap: true });

// highlight.js: the npm core is CommonJS (`module.exports = highlight`, no require); wrap it as
// an ES module. Language files under es/languages are self-contained ES modules.
{
  const src = readFileSync(need(join(nm, 'highlight.js/lib/core.js')), 'utf8');
  if (/\brequire\(/.test(src)) {
    console.error('highlight.js core now uses require(): update the wrapper');
    process.exit(1);
  }
  mkdirSync(join(out, 'hljs'), { recursive: true });
  writeFileSync(
    join(out, 'hljs', 'core.mjs'),
    `// highlight.js 11.9.0 lib/core.js wrapped as an ES module by scripts/vendor.mjs (BSD-3-Clause)\n` +
      `const module = { exports: {} };\n${src}\nexport default module.exports;\n`,
  );
  console.log('vendor: hljs/core.mjs');
}
export const HLJS_LANGS = [
  'bash', 'c', 'cpp', 'csharp', 'css', 'diff', 'dockerfile', 'dos', 'go', 'ini', 'java', 'javascript',
  'json', 'kotlin', 'lua', 'makefile', 'markdown', 'php', 'plaintext', 'powershell', 'python', 'ruby',
  'rust', 'scss', 'shell', 'sql', 'swift', 'typescript', 'xml', 'yaml',
];
for (const l of HLJS_LANGS) copy(`highlight.js/es/languages/${l}.js`, `hljs/languages/${l}.js`);
copy('highlight.js/styles/github-dark.min.css', 'hljs/github-dark.min.css');

// licenses of everything vendored above
const licenses = [
  ['@xterm/xterm/LICENSE', 'xterm.LICENSE.txt'],
  ['marked/LICENSE.md', 'marked.LICENSE.md'],
  ['dompurify/LICENSE', 'dompurify.LICENSE.txt'],
  ['katex/LICENSE', 'katex.LICENSE.txt'],
  ['highlight.js/LICENSE', 'highlight.js.LICENSE.txt'],
  ['mermaid/LICENSE', 'mermaid.LICENSE.txt'],
];
for (const [from, to] of licenses) copy(from, `licenses/${to}`);
