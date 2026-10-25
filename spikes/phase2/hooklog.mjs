// Hook logger: node hooklog.mjs <event> <outDir>
// Writes <outDir>/<event>-<n>.json with the hook's stdin JSON and relevant env vars.
// The messaging token is written to the file (spike only; the real app POSTs it to its control server).
import fs from 'node:fs';
import path from 'node:path';

const [event, outDir] = process.argv.slice(2);
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (c) => (input += c));
process.stdin.on('end', () => {
  let stdin;
  try { stdin = JSON.parse(input); } catch { stdin = { _raw: input }; }
  const pick = ['CLAUDE_CODE_MESSAGING_SOCKET', 'CLAUDE_CODE_MESSAGING_TOKEN', 'CLAUDE_PROJECT_DIR', 'CLAUDE_ENV_FILE', 'CLAUDE_CODE_SESSION_ID', 'CLAUDE_EFFORT'];
  const env = Object.fromEntries(pick.map((k) => [k, process.env[k] ?? null]));
  const envNames = Object.keys(process.env).filter((k) => /^(CLAUDE|ANTHROPIC)/.test(k)).sort();
  fs.mkdirSync(outDir, { recursive: true });
  let n = 0;
  while (fs.existsSync(path.join(outDir, `${event}-${n}.json`))) n++;
  fs.writeFileSync(path.join(outDir, `${event}-${n}.json`), JSON.stringify({ event, pid: process.pid, ppid: process.ppid, env, envNames, stdin }, null, 2));
  process.exit(0);
});
