// Long-running non-interactive Claude Code worker driven over stream-json.
// node worker.mjs <outDir> <seconds> <firstPrompt|-> -- <extra claude args...>
// Spawns: claude -p --input-format stream-json --output-format stream-json --verbose <extra>
// keeps stdin open for <seconds>, logs every stdout line to <outDir>/stdout.jsonl, then kills the process tree.
import { spawn, execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const argv = process.argv.slice(2);
const sep = argv.indexOf('--');
const [outDir, secs, firstPrompt] = argv.slice(0, sep);
const extra = argv.slice(sep + 1);
fs.mkdirSync(outDir, { recursive: true });
const out = fs.createWriteStream(path.join(outDir, 'stdout.jsonl'));
const err = fs.createWriteStream(path.join(outDir, 'stderr.txt'));

const args = ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose', ...extra];
fs.writeFileSync(path.join(outDir, 'args.json'), JSON.stringify(args, null, 2));
const child = spawn('claude', args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
fs.writeFileSync(path.join(outDir, 'pid'), String(child.pid));
let buf = '';
child.stdout.on('data', (d) => {
  buf += d;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i); buf = buf.slice(i + 1);
    out.write(line + '\n');
    try {
      const m = JSON.parse(line);
      const brief = m.type === 'assistant'
        ? m.message?.content?.map((c) => c.type === 'text' ? `text:${c.text}` : c.type === 'tool_use' ? `tool_use:${c.name} ${JSON.stringify(c.input)}` : c.type).join(' | ')
        : m.type === 'user' ? JSON.stringify(m.message?.content).slice(0, 300)
        : m.type === 'result' ? `result:${m.subtype} ${String(m.result).slice(0, 200)}`
        : m.subtype || '';
      console.log(`[${new Date().toISOString().slice(11, 19)}] ${m.type} ${brief}`);
    } catch { console.log('raw ' + line.slice(0, 200)); }
  }
});
child.stderr.on('data', (d) => err.write(d));
child.on('exit', (c) => { console.log(`claude exited ${c}`); process.exit(0); });

if (firstPrompt && firstPrompt !== '-') {
  child.stdin.write(JSON.stringify({ type: 'user', message: { role: 'user', content: firstPrompt } }) + '\n');
}
setTimeout(() => {
  console.log('time up, killing');
  try { execSync(`taskkill /PID ${child.pid} /T /F`, { stdio: 'ignore' }); } catch {}
  setTimeout(() => process.exit(0), 1000);
}, Number(secs) * 1000);
