// P3 spike: minimal Streamable-HTTP MCP server with plain JSON responses (no SSE).
// Usage: node mcp-server.mjs <port> <logfile>
// Logs every request (method, url, headers, body) as one JSON line to <logfile>.
import http from 'node:http';
import fs from 'node:fs';

const port = Number(process.argv[2] || 0);
const logFile = process.argv[3] || 'mcp-requests.jsonl';
const SESSION = 'spike-session-1';

function log(entry) {
  fs.appendFileSync(logFile, JSON.stringify({ t: new Date().toISOString(), ...entry }) + '\n');
}

const tools = [
  {
    name: 'echo_who',
    description: 'Returns the caller identity seen by the server (Authorization header) and echoes text.',
    inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
  },
];

function handle(msg, req) {
  const auth = req.headers['authorization'] || '(none)';
  switch (msg.method) {
    case 'initialize':
      return {
        protocolVersion: msg.params?.protocolVersion || '2025-06-18',
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: 'agentboard-spike', version: '0.0.1' },
      };
    case 'tools/list':
      return { tools };
    case 'tools/call':
      if (msg.params?.name === 'echo_who') {
        return { content: [{ type: 'text', text: `auth=${auth} text=${msg.params?.arguments?.text}` }], isError: false };
      }
      return { content: [{ type: 'text', text: 'unknown tool' }], isError: true };
    case 'ping':
      return {};
    default:
      return undefined;
  }
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    log({ method: req.method, url: req.url, headers: req.headers, body });
    if (req.url !== '/mcp') { res.writeHead(404).end(); return; }
    if (req.method === 'GET') { res.writeHead(405, { Allow: 'POST' }).end(); return; } // no SSE stream
    if (req.method === 'DELETE') { res.writeHead(200).end(); return; }
    if (req.method !== 'POST') { res.writeHead(405).end(); return; }
    if (req.headers['authorization'] !== 'Bearer x') {
      res.writeHead(401, { 'Content-Type': 'application/json' }).end(JSON.stringify({ error: 'unauthorized' }));
      return;
    }
    let msg;
    try { msg = JSON.parse(body); } catch { res.writeHead(400).end(); return; }
    if (msg.id === undefined) { res.writeHead(202).end(); return; } // notification
    const result = handle(msg, req);
    const out = result === undefined
      ? { jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: 'Method not found' } }
      : { jsonrpc: '2.0', id: msg.id, result };
    const headers = { 'Content-Type': 'application/json' };
    if (msg.method === 'initialize') headers['Mcp-Session-Id'] = SESSION;
    log({ response: out });
    res.writeHead(200, headers).end(JSON.stringify(out));
  });
});

server.listen(port, '127.0.0.1', () => {
  console.log(`listening ${server.address().port}`);
});
