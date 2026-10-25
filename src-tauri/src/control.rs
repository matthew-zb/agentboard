//! Local control server (spec §4): `/mcp` (MCP over plain JSON-RPC, one `application/json`
//! reply per POST, no streaming) and `/hook/<event>` (Claude Code hooks via
//! `agentboard.exe --hook`). Wire behaviour follows docs/spikes.md "P3" exactly.
//!
//! * Binds `127.0.0.1:<ephemeral>` only and publishes `http://127.0.0.1:<port>` in
//!   `AppState::ctrl_url` before the initial role-pane spawns (lib.rs setup).
//! * Every request needs `Host` = `127.0.0.1` / `localhost` (DNS-rebinding guard) and no
//!   `Origin` header (browsers are refused; no CORS headers are ever sent) → else 403.
//! * Every `/mcp` and `/hook/*` request needs `Authorization: Bearer <pane token>`; the caller
//!   ([`Caller`]) is computed from the current workspace on each request. Unknown/stale token
//!   or a pane that is gone / has no role → 401 with a JSON-RPC error body.
//! * Bodies are capped at 1 MB (413). Tool calls and hook handlers run on blocking threads
//!   with a deadline; no app lock is ever held across an `.await`.
//!
//! Locks: `tokens` (leaf, released) then `ws` (released) to build the caller; `hooks` (leaf).
//! Tool bodies / hook handlers follow the launcher lock order.

use crate::mcp_tools;
use crate::roles::Role;
use crate::workspace::{PaneId, TabId};
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Request body cap (MCP and hooks).
pub const BODY_LIMIT: usize = 1024 * 1024;
/// Deadline of one `tools/call`.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline of one hook handler. Equal to the hook CLI's own 2 s budget: hook handlers must
/// finish in < 2 s (no pipe or PTY IO — the bridge queues and its worker delivers) so the
/// replies, and with them a session's hook events, keep their order.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(2);
/// Protocol versions accepted in `initialize`, newest first. A client's version from this list
/// is echoed (spike P3); anything else is answered with the newest.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const SESSION_HEADER: &str = "mcp-session-id";
/// JSON-RPC code of the 401 body (implementation-defined server error range).
const AUTH_ERROR: i64 = -32001;

/// Who is calling: the role pane its bearer token belongs to, as of this request.
#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub pane: PaneId,
    pub role: Role,
    pub tab: TabId,
    /// The tab's current name (inbox ownership, R29; can differ from `session_name` after a
    /// rename until the next restart, R15).
    pub tab_name: String,
    pub team: Option<String>,
    pub session_name: String,
}

/// Resolve `token` against the current workspace. `None`: unknown/stale token, the pane left
/// the model, or it is not a role terminal pane (applets have no token).
pub fn caller_for(state: &AppState, token: &str) -> Option<Caller> {
    let pane = state.tokens.resolve(token)?; // leaf lock, released here
    let ws = state.ws.lock().unwrap();
    let p = ws.pane_by_id(&pane)?;
    let tab = ws.tabs.get(&p.tab)?;
    if p.kind.is_applet() {
        return None;
    }
    Some(Caller {
        pane: p.id.clone(),
        role: p.role?,
        tab: p.tab.clone(),
        tab_name: tab.name.clone(),
        team: p.team.clone(),
        session_name: p.session_name.clone()?,
    })
}

// ───────────────────────── hook router ─────────────────────────

/// Handler of one hook event: `(state, caller, POST body)`. The body is the hook CLI's
/// `{event, stdin, socket, msgToken, cwd}`. Runs on a blocking thread; keep it short (the
/// session waits for the HTTP reply). Never log `msgToken` or bodies.
pub type HookHandler = fn(&AppState, &Caller, &Value);

/// Event name → handler. Tasks 7·10 call [`HookRouter::register`] (replacing the debug
/// logger). Unknown events are answered 204 without a handler. The map lock is a leaf.
pub struct HookRouter {
    map: RwLock<HashMap<String, HookHandler>>,
}

impl Default for HookRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRouter {
    /// The four events of `agentcfg::HOOK_EVENTS`, each with the debug logger.
    pub fn new() -> Self {
        let r = HookRouter { map: RwLock::new(HashMap::new()) };
        for ev in crate::agentcfg::HOOK_EVENTS {
            r.register(ev, log_hook);
        }
        r
    }

    pub fn register(&self, event: &str, handler: HookHandler) {
        self.map.write().unwrap().insert(event.to_string(), handler);
    }

    pub fn get(&self, event: &str) -> Option<HookHandler> {
        self.map.read().unwrap().get(event).copied()
    }
}

/// Debug no-op: `[hook] <event> <pane>` (debug builds only; never the body or tokens).
fn log_hook(_: &AppState, caller: &Caller, body: &Value) {
    if cfg!(debug_assertions) {
        eprintln!("[hook] {} {}", body.get("event").and_then(Value::as_str).unwrap_or("?"), caller.pane);
    }
}

// ───────────────────────── server ─────────────────────────

/// Where handlers find the `AppState`: the Tauri app (production) or an `Arc` (tests).
pub trait StateHost: Clone + Send + Sync + 'static {
    fn get(&self) -> Option<&AppState>;
}

impl StateHost for Arc<AppState> {
    fn get(&self) -> Option<&AppState> {
        Some(self)
    }
}

impl StateHost for tauri::AppHandle {
    fn get(&self) -> Option<&AppState> {
        use tauri::Manager;
        self.try_state::<AppState>().map(|s| s.inner())
    }
}

/// Bind `127.0.0.1:0` (never another interface).
pub async fn bind() -> std::io::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await
}

/// Publish the URL in `ctrl_url` and serve `listener` on a spawned task (must run inside a
/// tokio runtime). Returns the bound address.
pub async fn serve_on<H: StateHost>(host: H, listener: tokio::net::TcpListener) -> std::io::Result<SocketAddr> {
    let addr = listener.local_addr()?;
    if let Some(s) = host.get() {
        *s.ctrl_url.lock().unwrap() = Some(format!("http://127.0.0.1:{}", addr.port()));
    }
    let app = router(host);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("[agentboard] 제어 서버 오류: {e}");
        }
    });
    Ok(addr)
}

/// [`bind`] + [`serve_on`].
pub async fn serve<H: StateHost>(host: H) -> std::io::Result<SocketAddr> {
    serve_on(host, bind().await?).await
}

pub fn router<H: StateHost>(host: H) -> Router {
    Router::new()
        .route("/mcp", post(mcp_post::<H>)) // GET (SSE stream) → 405, as in the spike
        .route("/hook/{event}", post(hook_post::<H>))
        .with_state(host)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn(guard))
}

/// 403 for browser requests (`Origin`) and for a `Host` that is not loopback by name.
async fn guard(req: Request, next: Next) -> Response {
    let h = req.headers();
    let host_ok = h.get(header::HOST).and_then(|v| v.to_str().ok()).map_or(false, loopback_host);
    if h.contains_key(header::ORIGIN) || !host_ok {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    next.run(req).await
}

fn loopback_host(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((n, port)) if port.chars().all(|c| c.is_ascii_digit()) => n,
        _ => host,
    };
    name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost")
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    let mut r = (status, body.to_string()).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim()).filter(|t| !t.is_empty())
}

/// Caller of the request, or the 401 reply.
fn authenticate<H: StateHost>(host: &H, headers: &HeaderMap) -> Result<Caller, Response> {
    let unauthorized = |m: &str| json_response(StatusCode::UNAUTHORIZED, &rpc_error(Value::Null, AUTH_ERROR, m));
    let Some(token) = bearer(headers) else {
        return Err(unauthorized("인증 실패: Authorization: Bearer 토큰이 없습니다"));
    };
    let Some(state) = host.get() else {
        return Err(unauthorized("인증 실패: 앱 상태를 아직 사용할 수 없습니다"));
    };
    caller_for(state, token).ok_or_else(|| unauthorized("인증 실패: 알 수 없거나 만료된 토큰입니다 (창이 다시 시작되었거나 닫혔습니다)"))
}

async fn mcp_post<H: StateHost>(State(host): State<H>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match authenticate(&host, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let mut resp = match serde_json::from_slice::<Value>(&body) {
        Err(_) => json_response(StatusCode::BAD_REQUEST, &rpc_error(Value::Null, -32700, "Parse error")),
        Ok(msg) => mcp_message(host, caller, msg).await,
    };
    if let Some(sid) = headers.get(SESSION_HEADER) {
        resp.headers_mut().entry(SESSION_HEADER).or_insert(sid.clone());
    }
    resp
}

/// One JSON-RPC message (batches are not used by MCP clients since 2025-06-18).
async fn mcp_message<H: StateHost>(host: H, caller: Caller, msg: Value) -> Response {
    let Some(obj) = msg.as_object() else {
        return json_response(StatusCode::BAD_REQUEST, &rpc_error(Value::Null, -32600, "Invalid Request"));
    };
    let method = obj.get("method").and_then(Value::as_str);
    let id = obj.get("id").cloned();
    let (Some(method), Some(id)) = (method, id) else {
        // notification (no id) or a client response (no method): accepted, no body
        return StatusCode::ACCEPTED.into_response();
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let ok = |result: Value| json_response(StatusCode::OK, &json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
            let mut r = ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "agentboard", "version": env!("CARGO_PKG_VERSION") },
            }));
            if let Ok(v) = HeaderValue::from_str(&new_session_id()) {
                r.headers_mut().insert(SESSION_HEADER, v);
            }
            r
        }
        "ping" => ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = mcp_tools::list(caller.role).iter().map(|t| t.to_json()).collect();
            ok(json!({ "tools": tools }))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str).map(str::to_string) else {
                return json_response(StatusCode::OK, &rpc_error(id, -32602, "Invalid params: name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let job = tokio::task::spawn_blocking(move || match host.get() {
                Some(state) => mcp_tools::call(state, &caller, &name, args),
                None => Err("앱 상태를 사용할 수 없습니다".to_string()),
            });
            let outcome = match tokio::time::timeout(TOOL_TIMEOUT, job).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => Err("도구 실행 중 내부 오류가 났습니다".to_string()),
                Err(_) => Err("도구 실행 시간이 초과되었습니다".to_string()),
            };
            let (text, is_error) = match outcome {
                Ok(Value::String(s)) => (s, false),
                Ok(v) => (v.to_string(), false),
                Err(m) => (m, true),
            };
            ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        // includes `server/discover`: exactly 200 + -32601 (spike P3; the only verified reply)
        _ => json_response(StatusCode::OK, &rpc_error(id, -32601, "Method not found")),
    }
}

fn new_session_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        return "agentboard".into();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn valid_event(e: &str) -> bool {
    !e.is_empty() && e.len() <= 64 && e.bytes().all(|b| b.is_ascii_alphabetic())
}

async fn hook_post<H: StateHost>(
    State(host): State<H>,
    Path(event): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match authenticate(&host, &headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if !valid_event(&event) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(handler) = host.get().and_then(|s| s.hooks.get(&event)) else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let job = tokio::task::spawn_blocking(move || {
        if let Some(state) = host.get() {
            handler(state, &caller, &body);
        }
    });
    if tokio::time::timeout(HOOK_TIMEOUT, job).await.is_err() {
        eprintln!("[agentboard] hook 처리 시간 초과: {event}");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names() {
        for ok in ["127.0.0.1", "127.0.0.1:4711", "localhost", "LOCALHOST:1"] {
            assert!(loopback_host(ok), "{ok}");
        }
        for bad in ["evil.example", "evil.example:80", "127.0.0.1.evil.example", "localhost.evil:1", "[::1]:80", "0.0.0.0:1"] {
            assert!(!loopback_host(bad), "{bad}");
        }
    }

    #[test]
    fn bearer_parsing_and_event_names() {
        let mut h = HeaderMap::new();
        assert_eq!(bearer(&h), None);
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer abc"));
        assert_eq!(bearer(&h), Some("abc"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("bearer  abc "));
        assert_eq!(bearer(&h), Some("abc"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert_eq!(bearer(&h), None);
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer(&h), None);
        assert!(valid_event("SessionStart") && !valid_event("") && !valid_event("a/b") && !valid_event("x1"));
    }

    #[test]
    fn hook_router_registers_and_replaces() {
        fn noop(_: &AppState, _: &Caller, _: &Value) {}
        let r = HookRouter::new();
        for ev in crate::agentcfg::HOOK_EVENTS {
            assert!(r.get(ev).is_some(), "{ev}");
        }
        assert!(r.get("Other").is_none());
        r.register("Stop", noop);
        assert_eq!(r.get("Stop").map(|f| f as usize), Some(noop as HookHandler as usize));
    }
}
