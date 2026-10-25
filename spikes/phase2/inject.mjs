// Write a message into a session's inbox named pipe.
// node inject.mjs <SessionStart-hook-json> <text> [lineJson]
// Reads CLAUDE_CODE_MESSAGING_SOCKET / _TOKEN from the hook dump written by hooklog.mjs,
// sends the auth line, then one message line, then ends the connection.
// Default message line: {"type":"user","message":{"role":"user","content":<text>}}
// If lineJson is given it is sent verbatim instead (for format experiments).
import net from 'node:net';
import fs from 'node:fs';

const [hookFile, text, lineJson] = process.argv.slice(2);
const { env } = JSON.parse(fs.readFileSync(hookFile, 'utf8'));
const sock = env.CLAUDE_CODE_MESSAGING_SOCKET;
const token = env.CLAUDE_CODE_MESSAGING_TOKEN;
const auth = JSON.stringify({ type: 'auth', token });
const msg = lineJson ?? JSON.stringify({ type: 'user', message: { role: 'user', content: text } });
const c = net.connect(sock, () => {
  c.write(auth + '\n' + msg + '\n');
  c.end();
});
let reply = '';
c.on('data', (d) => (reply += d));
c.on('close', () => { console.log(`sent to ${sock}; server replied: ${JSON.stringify(reply)}`); });
c.on('error', (e) => { console.error('error', e.message); process.exit(1); });
