// node_modules/@xterm/* 의 ESM 빌드와 CSS를 src/vendor/ 로 복사한다.
import { copyFileSync, mkdirSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const nm = join(root, 'node_modules', '@xterm');
const out = join(root, 'src', 'vendor');
mkdirSync(out, { recursive: true });

const files = [
  ['xterm/lib/xterm.mjs', 'xterm.mjs'],
  ['xterm/css/xterm.css', 'xterm.css'],
  ['addon-fit/lib/addon-fit.mjs', 'addon-fit.mjs'],
  ['addon-unicode11/lib/addon-unicode11.mjs', 'addon-unicode11.mjs'],
  ['addon-search/lib/addon-search.mjs', 'addon-search.mjs'],
  ['addon-webgl/lib/addon-webgl.mjs', 'addon-webgl.mjs'],
];

for (const [from, to] of files) {
  const src = join(nm, from);
  if (!existsSync(src)) {
    console.error(`missing: ${src}`);
    process.exit(1);
  }
  copyFileSync(src, join(out, to));
  console.log(`vendor: ${to}`);
}
