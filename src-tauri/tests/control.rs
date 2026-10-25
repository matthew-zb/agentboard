//! Control server (Task 5): the real axum server on an ephemeral loopback port, driven with
//! raw HTTP/1.1 over `std::net::TcpStream`, against a Tauri-less `AppState`.
use app_lib::config::{ensure_dotdir, load_all};
use app_lib::control::{self, Caller};
use app_lib::pty::{PtyManager, PtySink};
use app_lib::roles::Role;
use app_lib::workspace::{Pane, Workspace};
use app_lib::{AppState, Snapshot, Ui};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

struct NullSink;
impl PtySink for NullSink {
    fn output(&self, _: &str, _: u64, _: &[u8]) {}
    fn exited(&self, _: &str, _: u64, _: i32) {}
}
struct NullUi;
impl Ui for NullUi {
    fn workspace_changed(&self, _: &Snapshot) {}
    fn drop_channel(&self, _: &str) {}
}

struct Fx {
    state: Arc<AppState>,
    addr: SocketAddr,
    _rt: tokio::runtime::Runtime,
    _keep: (tempfile::TempDir, tempfile::TempDir),
}

impl Fx {
    /// Id of the first pane with `role`.
    fn pane(&self, role: Role) -> String {
        let ws = self.state.ws.lock().unwrap();
        ws.panes.values().find(|p| p.role == Some(role)).unwrap_or_else(|| panic!("no {role:?} pane")).id.clone()
    }
    fn token(&self, role: Role) -> String {
        self.state.tokens.issue(&self.pane(role))
    }
}

/// Dispatcher tab + manager tab "billing" with a team "batch" (lead + member) added by hand.
fn fixture() -> Fx {
    let root = tempfile::tempdir().unwrap();
    let appdata = tempfile::tempdir().unwrap();
    let s = json!({ "workspaceRoot": root.path().to_string_lossy() });
    std::fs::write(appdata.path().join("settings.json"), s.to_string()).unwrap();
    ensure_dotdir(root.path()).unwrap();
    std::fs::write(
        root.path().join(".agentboard/config.json"),
        r#"{"tabs":[{"name":"billing","role":"manager","dir":"billing"}]}"#,
    )
    .unwrap();
    let loaded = load_all(appdata.path());
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    let mut ws = Workspace::from_config(&loaded);
    let mgr = ws.panes.values().find(|p| p.role == Some(Role::Manager)).expect("manager pane").clone();
    let mk = |id: &str, role: Role, session: &str| Pane {
        id: id.into(),
        name: session.into(),
        role: Some(role),
        team: Some("batch".into()),
        session_name: Some(session.into()),
        ..mgr.clone()
    };
    ws.panes.insert("tl1".into(), mk("tl1", Role::Lead, "billing-batch-lead"));
    ws.panes.insert("tm1".into(), mk("tm1", Role::Member, "billing-batch-m1"));
    let (tx, _rx) = mpsc::channel();
    let pty = PtyManager::new_per_child_job(Arc::new(NullSink));
    let state = Arc::new(AppState::new(
        ws,
        loaded,
        pty,
        Arc::new(NullUi),
        tx,
        app_lib::persist::Saver::disabled(),
        Vec::new(),
        appdata.path().join("run"),
    ));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let addr = rt.block_on(control::serve(state.clone())).expect("server binds");
    Fx { state, addr, _rt: rt, _keep: (root, appdata) }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}
impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
}

/// One request on a fresh connection (`Connection: close`), response read to EOF.
fn raw(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        req.push_str(&format!("Host: 127.0.0.1:{}\r\n", addr.port()));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8(buf).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    // bodies here are small JSON; a chunked body would show its size line, so require Content-Length
    Resp { status, headers, body: body.to_string() }
}

fn rpc(addr: SocketAddr, token: Option<&str>, msg: Value) -> Resp {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut h: Vec<(&str, &str)> = vec![("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream")];
    if let Some(a) = &auth {
        h.push(("Authorization", a));
    }
    raw(addr, "POST", "/mcp", &h, &msg.to_string())
}

fn tool_names(fx: &Fx, token: &str) -> Vec<String> {
    let r = rpc(fx.addr, Some(token), json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    assert_eq!(r.status, 200, "{}", r.body);
    let mut v: Vec<String> =
        r.json()["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

fn call(fx: &Fx, token: &str, name: &str, args: Value) -> Value {
    let r = rpc(fx.addr, Some(token), json!({"jsonrpc":"2.0","id":"c1","method":"tools/call","params":{"name":name,"arguments":args}}));
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!(v["id"], "c1");
    v["result"].clone()
}

fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))
}

#[test]
fn rejects_missing_or_stale_token() {
    let fx = fixture();
    let list = json!({"jsonrpc":"2.0","id":7,"method":"tools/list"});
    let check_401 = |r: Resp, what: &str| {
        assert_eq!(r.status, 401, "{what}: {}", r.body);
        assert!(r.header("content-type").unwrap_or("").starts_with("application/json"), "{what}");
        let v = r.json();
        assert_eq!(v["jsonrpc"], "2.0", "{what}");
        assert!(v["error"]["code"].is_i64(), "{what}");
        assert!(has_hangul(v["error"]["message"].as_str().unwrap()), "{what}: Korean message");
    };
    check_401(rpc(fx.addr, None, list.clone()), "missing");
    check_401(rpc(fx.addr, Some("deadbeef"), list.clone()), "unknown");
    check_401(raw(fx.addr, "POST", "/mcp", &[("Authorization", "Basic abc")], &list.to_string()), "not bearer");

    // a reissued token (pane respawned) makes the old one stale
    let old = fx.token(Role::Manager);
    assert_eq!(rpc(fx.addr, Some(&old), list.clone()).status, 200);
    let new = fx.token(Role::Manager);
    check_401(rpc(fx.addr, Some(&old), list.clone()), "stale");
    assert_eq!(rpc(fx.addr, Some(&new), list.clone()).status, 200);

    // a token whose pane left the workspace (not yet revoked) is rejected too
    let lead = fx.token(Role::Lead);
    fx.state.ws.lock().unwrap().panes.remove("tl1");
    check_401(rpc(fx.addr, Some(&lead), list.clone()), "pane gone");

    // hooks: same authentication (no body leak in the reply)
    let hook = |tok: Option<&str>| {
        let a = tok.map(|t| format!("Bearer {t}"));
        let mut h = vec![("Content-Type", "application/json")];
        if let Some(a) = &a {
            h.push(("Authorization", a.as_str()));
        }
        raw(fx.addr, "POST", "/hook/Stop", &h, r#"{"event":"Stop","stdin":{}}"#)
    };
    assert_eq!(hook(None).status, 401);
    assert_eq!(hook(Some(&old)).status, 401);
    assert_eq!(hook(Some(&new)).status, 204);
}

#[test]
fn tools_list_is_filtered_by_role() {
    let fx = fixture();
    let m = fx.token(Role::Manager);
    let d = fx.token(Role::Dispatcher);
    let l = fx.token(Role::Lead);
    let mb = fx.token(Role::Member);
    assert_eq!(
        tool_names(&fx, &m),
        ["handoff_done", "inbox_list", "inbox_note", "inbox_post", "inbox_propose_close", "issue_create", "list_team"]
    );
    assert_eq!(tool_names(&fx, &d), ["handoff_done", "inbox_list"]);
    assert_eq!(tool_names(&fx, &l), ["inbox_list", "list_team"]);
    assert_eq!(tool_names(&fx, &mb), ["inbox_list"]);

    // tool defs are complete MCP tool objects
    let r = rpc(fx.addr, Some(&m), json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).json();
    for t in r["result"]["tools"].as_array().unwrap() {
        assert!(t["description"].as_str().map_or(false, has_hangul), "{t}");
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
    }

    // a role without the permission gets isError + Korean, even for a listed-elsewhere tool
    let res = call(&fx, &mb, "list_team", json!({}));
    assert_eq!(res["isError"], true, "{res}");
    assert!(has_hangul(res["content"][0]["text"].as_str().unwrap()));
    let res = call(&fx, &m, "no_such_tool", json!({}));
    assert_eq!(res["isError"], true);
    // planned but not yet implemented tools are refused as well
    let res = call(&fx, &m, "hire_lead", json!({"team":"x","brief_path":"x"}));
    assert_eq!(res["isError"], true);

    // list_team: the caller tab's teams
    let res = call(&fx, &m, "list_team", json!({}));
    assert_eq!(res["isError"], false, "{res}");
    let v: Value = serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(v["tab"], "billing");
    let teams = v["teams"].as_array().unwrap();
    assert_eq!(teams.len(), 1, "{v}");
    assert_eq!(teams[0]["team"], "batch");
    let sessions: Vec<&str> = teams[0]["panes"].as_array().unwrap().iter().map(|p| p["sessionName"].as_str().unwrap()).collect();
    assert_eq!(sessions, ["billing-batch-lead", "billing-batch-m1"]);
    assert_eq!(teams[0]["panes"][0]["role"], "lead");
    let res = call(&fx, &l, "list_team", json!({"team":"batch"}));
    assert_eq!(res["isError"], false, "{res}");
    let res = call(&fx, &m, "list_team", json!({"team":"nope"}));
    assert_eq!(res["isError"], true);
    let res = call(&fx, &m, "list_team", json!({"team":3}));
    assert_eq!(res["isError"], true);

    // handoff_done records a timestamp for the caller pane
    let mgr = fx.pane(Role::Manager);
    assert!(fx.state.handoffs.lock().unwrap().get(&mgr).is_none());
    let res = call(&fx, &m, "handoff_done", json!({}));
    assert_eq!(res["isError"], false, "{res}");
    assert!(fx.state.handoffs.lock().unwrap().get(&mgr).is_some());
}

#[test]
fn initialize_roundtrip() {
    let fx = fixture();
    let t = fx.token(Role::Manager);
    let tok = Some(t.as_str());

    // 1. server/discover probe: exactly HTTP 200 + JSON-RPC -32601 with the request id
    let r = rpc(fx.addr, tok, json!({"jsonrpc":"2.0","id":"server-discover-probe-1","method":"server/discover","params":{}}));
    assert_eq!(r.status, 200);
    assert!(r.header("content-type").unwrap().starts_with("application/json"));
    assert_eq!(
        r.json(),
        json!({"jsonrpc":"2.0","id":"server-discover-probe-1","error":{"code":-32601,"message":"Method not found"}})
    );

    // 2. initialize: protocolVersion echoed, serverInfo, tools capability, session id header
    let r = rpc(
        fx.addr,
        tok,
        json!({"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claude-code","version":"x"}},"jsonrpc":"2.0","id":0}),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.header("content-type").unwrap().starts_with("application/json"));
    let sid = r.header("mcp-session-id").expect("Mcp-Session-Id").to_string();
    assert!(!sid.is_empty());
    let v = r.json();
    assert_eq!(v["id"], 0);
    assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(v["result"]["serverInfo"]["name"], "agentboard");
    assert!(v["result"]["capabilities"]["tools"].is_object(), "{v}");
    // an unknown version is answered with one the server supports
    let r = rpc(fx.addr, tok, json!({"jsonrpc":"2.0","id":9,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}));
    assert_eq!(r.json()["result"]["protocolVersion"], "2025-11-25");

    // 3. notifications/initialized → 202, empty body; session id echoed
    let auth = format!("Bearer {t}");
    let h = [
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
        ("mcp-session-id", sid.as_str()),
        ("mcp-protocol-version", "2025-11-25"),
    ];
    let r = raw(fx.addr, "POST", "/mcp", &h, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!(r.status, 202);
    assert!(r.body.is_empty(), "{:?}", r.body);

    // 4. GET /mcp → 405
    assert_eq!(raw(fx.addr, "GET", "/mcp", &[("Authorization", auth.as_str()), ("Accept", "text/event-stream")], "").status, 405);

    // 5. tools/list with the session headers; session id comes back
    let r = raw(fx.addr, "POST", "/mcp", &h, r#"{"method":"tools/list","jsonrpc":"2.0","id":1}"#);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("mcp-session-id"), Some(sid.as_str()));
    assert_eq!(r.json()["id"], 1);

    // 6. tools/call with _meta, numeric id
    let r = raw(
        fx.addr,
        "POST",
        "/mcp",
        &h,
        r#"{"method":"tools/call","params":{"name":"list_team","arguments":{},"_meta":{"progressToken":2}},"jsonrpc":"2.0","id":2}"#,
    );
    let v = r.json();
    assert_eq!((v["id"].clone(), v["result"]["isError"].clone()), (json!(2), json!(false)), "{v}");
    assert_eq!(v["result"]["content"][0]["type"], "text");

    // ping, malformed JSON, unknown method
    assert_eq!(rpc(fx.addr, tok, json!({"jsonrpc":"2.0","id":3,"method":"ping"})).json()["result"], json!({}));
    let r = raw(fx.addr, "POST", "/mcp", &[("Authorization", auth.as_str())], "{not json");
    assert_eq!(r.json()["error"]["code"], -32700);
    assert_eq!(rpc(fx.addr, tok, json!({"jsonrpc":"2.0","id":4,"method":"resources/list"})).json()["error"]["code"], -32601);
}

static HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
static HOOK_SEEN: Mutex<Vec<(String, Role, String)>> = Mutex::new(Vec::new());
fn record_hook(_: &AppState, c: &Caller, body: &Value) {
    HOOK_CALLS.fetch_add(1, Ordering::SeqCst);
    HOOK_SEEN.lock().unwrap().push((c.pane.clone(), c.role, body["event"].as_str().unwrap_or("").to_string()));
}

#[test]
fn hooks_route_to_registered_handlers() {
    let fx = fixture();
    fx.state.hooks.register("Notification", record_hook);
    let t = fx.token(Role::Lead);
    let url = format!("http://127.0.0.1:{}", fx.addr.port());
    assert_eq!(fx.state.ctrl_url.lock().unwrap().as_deref(), Some(url.as_str()));
    // the real hook client (Task 4) against the real server
    let body = r#"{"event":"Notification","stdin":{"message":"x"},"socket":null,"msgToken":null,"cwd":"."}"#;
    let code = app_lib::hookcli::post(&url, "Notification", &t, body, Duration::from_secs(2)).unwrap();
    assert_eq!(code, 204);
    assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(HOOK_SEEN.lock().unwrap()[0], ("tl1".to_string(), Role::Lead, "Notification".to_string()));
    // unregistered event → 204, handler not called
    assert_eq!(app_lib::hookcli::post(&url, "Unknown", &t, "{}", Duration::from_secs(2)).unwrap(), 204);
    assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
    // bodies over 1 MB are refused
    let big = format!(r#"{{"event":"Notification","stdin":"{}"}}"#, "a".repeat(1_100_000));
    let auth = format!("Bearer {t}");
    let r = raw(fx.addr, "POST", "/hook/Notification", &[("Authorization", auth.as_str()), ("Content-Type", "application/json")], &big);
    assert_eq!(r.status, 413);
    assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn server_binds_loopback_only() {
    let fx = fixture();
    assert!(fx.addr.ip().is_loopback(), "{}", fx.addr);
    assert_eq!(fx.addr.ip().to_string(), "127.0.0.1");
    assert_ne!(fx.addr.port(), 0);
    assert_eq!(fx.state.ctrl_url.lock().unwrap().clone(), Some(format!("http://127.0.0.1:{}", fx.addr.port())));

    // not reachable through a non-loopback address of this machine (if it has one)
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("192.0.2.1:9").is_ok() {
            if let Ok(local) = sock.local_addr() {
                if !local.ip().is_loopback() && !local.ip().is_unspecified() {
                    let other = SocketAddr::new(local.ip(), fx.addr.port());
                    assert!(TcpStream::connect_timeout(&other, Duration::from_millis(500)).is_err(), "reachable via {other}");
                }
            }
        }
    }

    // DNS-rebinding guard: foreign Host → 403; browser requests (Origin) → 403; no CORS headers
    let t = fx.token(Role::Manager);
    let auth = format!("Bearer {t}");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let r = raw(fx.addr, "POST", "/mcp", &[("Host", "evil.example:80"), ("Authorization", auth.as_str())], body);
    assert_eq!(r.status, 403);
    let r = raw(fx.addr, "POST", "/mcp", &[("Origin", "http://evil.example"), ("Authorization", auth.as_str())], body);
    assert_eq!(r.status, 403);
    assert!(r.header("access-control-allow-origin").is_none());
    let port_host = format!("localhost:{}", fx.addr.port());
    let r = raw(fx.addr, "POST", "/mcp", &[("Host", port_host.as_str()), ("Authorization", auth.as_str())], body);
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.header("access-control-allow-origin").is_none());
}

fn text(res: &Value) -> String {
    res["content"][0]["text"].as_str().unwrap().to_string()
}

/// Task 6: inbox tools over the real server — ownership, visibility, user-only done, issues.
#[test]
fn inbox_tools_over_mcp() {
    let fx = fixture();
    let m = fx.token(Role::Manager);
    let d = fx.token(Role::Dispatcher);
    let l = fx.token(Role::Lead);
    let mgr_pane = fx.pane(Role::Manager);
    let mgr_tab = fx.state.ws.lock().unwrap().panes[&mgr_pane].tab.clone();
    let workspace = fx.state.ws.lock().unwrap().tabs[&mgr_tab].workspace.clone();
    std::fs::create_dir_all(&workspace).unwrap();

    // post: Manager only; returns the id as plain text; owner = caller's tab
    let post = json!({"title":"결제 실패 확인","check":["금액"],"risk":["중복 결제"],"source":"slack #billing"});
    let res = call(&fx, &d, "inbox_post", post.clone());
    assert_eq!(res["isError"], true, "{res}");
    let res = call(&fx, &m, "inbox_post", post.clone());
    assert_eq!(res["isError"], false, "{res}");
    let id = text(&res);
    let item = fx.state.inbox.get(&id).expect("stored");
    assert_eq!((item.owner_tab.as_str(), item.owner_name.as_str()), (mgr_tab.as_str(), "billing"));
    assert_eq!(item.timeline[0].who, "billing");
    // retry of the identical post → same id
    assert_eq!(text(&call(&fx, &m, "inbox_post", post)), id);
    // bad arguments are Korean errors
    let res = call(&fx, &m, "inbox_post", json!({"title":"x","check":"not a list","risk":[],"source":""}));
    assert_eq!(res["isError"], true);
    assert!(has_hangul(&text(&res)));

    // list: dispatcher all, manager own, lead none (not an error)
    let list = |tok: &str, args: Value| -> Vec<Value> {
        let res = call(&fx, tok, "inbox_list", args);
        assert_eq!(res["isError"], false, "{res}");
        serde_json::from_str::<Value>(&text(&res)).unwrap().as_array().unwrap().clone()
    };
    assert_eq!(list(&d, json!({})).len(), 1);
    assert_eq!(list(&m, json!({})).len(), 1);
    assert_eq!(list(&m, json!({}))[0]["ownerTab"], mgr_tab.as_str());
    assert_eq!(list(&m, json!({}))[0]["state"], "new");
    assert!(list(&l, json!({})).is_empty());
    assert!(list(&d, json!({"state":"done"})).is_empty());
    assert_eq!(call(&fx, &d, "inbox_list", json!({"state":"bogus"}))["isError"], true);

    // note / propose_close: only the owner Manager
    assert_eq!(call(&fx, &d, "inbox_note", json!({"item":id,"text":"x"}))["isError"], true);
    let res = call(&fx, &m, "inbox_note", json!({"item":id,"text":"확인 중"}));
    assert_eq!(res["isError"], false, "{res}");
    assert_eq!(call(&fx, &m, "inbox_note", json!({"item":"nope","text":"x"}))["isError"], true);
    let res = call(&fx, &m, "inbox_propose_close", json!({"item":id,"final_opinion":"처리 완료"}));
    assert_eq!(res["isError"], false, "{res}");
    assert_eq!(list(&m, json!({"state":"close_proposed"})).len(), 1);
    // no tool reaches done; the user does (Tauri command path)
    assert_eq!(fx.state.inbox.get(&id).unwrap().state, app_lib::inbox::ItemState::CloseProposed);
    fx.state.inbox.instruct(&id, "하나 더").unwrap();
    assert_eq!(list(&m, json!({"state":"in_progress"})).len(), 1);

    // issue_create: folder in the caller tab's workspace, linked; a retry returns the same issue
    let res = call(&fx, &m, "issue_create", json!({"item":id,"title":"결제 실패"}));
    assert_eq!(res["isError"], false, "{res}");
    let v: Value = serde_json::from_str(&text(&res)).unwrap();
    let issue = v["issue"].as_str().unwrap().to_string();
    assert!(issue.ends_with("-결제-실패"), "{issue}");
    let path = std::path::PathBuf::from(v["path"].as_str().unwrap());
    assert_eq!(path, workspace.join("issues").join(&issue));
    assert!(path.join("input").is_dir() && path.join("output").is_dir());
    let again: Value = serde_json::from_str(&text(&call(&fx, &m, "issue_create", json!({"item":id,"title":"결제 실패"})))).unwrap();
    assert_eq!(again["issue"], issue.as_str());
    assert_eq!(std::fs::read_dir(workspace.join("issues")).unwrap().count(), 1);
    assert_eq!(fx.state.inbox.get(&id).unwrap().issue.as_deref(), Some(issue.as_str()));
    // without an item: just a folder; unknown item: error, nothing created
    let v: Value = serde_json::from_str(&text(&call(&fx, &m, "issue_create", json!({"title":"메모 정리"})))).unwrap();
    assert!(v["issue"].as_str().unwrap().ends_with("-메모-정리"));
    assert_eq!(call(&fx, &m, "issue_create", json!({"item":"nope","title":"x"}))["isError"], true);
    assert_eq!(call(&fx, &d, "issue_create", json!({"title":"x"}))["isError"], true);
    assert_eq!(std::fs::read_dir(workspace.join("issues")).unwrap().count(), 2);

    // the user's file copy lands in the issue folder
    let src = workspace.join("note.md");
    std::fs::write(&src, "x").unwrap();
    let out = fx.state.inbox.add_files(&id, &[src], app_lib::inbox::Target::Output, &workspace).unwrap();
    assert_eq!(out, vec![path.join("output").join("note.md")]);

    // content caps (Korean errors); schema-required fields are enforced
    let long = "가".repeat(201);
    let bad = [
        json!({"title":long,"check":[],"risk":[],"source":""}),
        json!({"title":"x","check":["a".repeat(501)],"risk":[],"source":""}),
        json!({"title":"x","check":vec!["a"; 21],"risk":[],"source":""}),
        json!({"title":"x","risk":[],"source":""}),
        json!({"title":"x","check":[],"risk":[]}),
    ];
    for args in bad {
        let res = call(&fx, &m, "inbox_post", args.clone());
        assert_eq!(res["isError"], true, "{args}");
        assert!(has_hangul(&text(&res)));
    }
    let big = "a".repeat(8 * 1024 + 1);
    assert_eq!(call(&fx, &m, "inbox_note", json!({"item":id,"text":big}))["isError"], true);
    assert_eq!(call(&fx, &m, "inbox_propose_close", json!({"item":id,"final_opinion":big}))["isError"], true);
    assert_eq!(call(&fx, &m, "issue_create", json!({"title":"가".repeat(201)}))["isError"], true);

    // issue_create on a done item: refused before any folder is made
    let id2 = text(&call(&fx, &m, "inbox_post", json!({"title":"끝난 일","check":[],"risk":[],"source":"s"})));
    call(&fx, &m, "inbox_propose_close", json!({"item":id2,"final_opinion":"끝"}));
    fx.state.inbox.confirm_done(&id2).unwrap();
    let before = std::fs::read_dir(workspace.join("issues")).unwrap().count();
    let res = call(&fx, &m, "issue_create", json!({"item":id2,"title":"고아"}));
    assert_eq!(res["isError"], true, "{res}");
    assert_eq!(std::fs::read_dir(workspace.join("issues")).unwrap().count(), before);
}
