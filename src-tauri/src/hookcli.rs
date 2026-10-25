//! `agentboard.exe --hook <Event>`: the command Claude Code runs for the hooks in a role
//! session's settings file (agentcfg). No Tauri, no WebView, no async runtime: read the hook's
//! stdin JSON, add the session's messaging socket/token from the environment, POST it to the
//! app's control server and exit 0 — a hook must never block or fail the session.
//!
//! Request: `POST {AGENTBOARD_URL}/hook/{event}`, `Authorization: Bearer {AGENTBOARD_TOKEN}`,
//! `Content-Type: application/json`, body
//! `{"event":"<Event>","stdin":<parsed stdin JSON, or the raw text when not JSON>,
//!   "socket":<CLAUDE_CODE_MESSAGING_SOCKET or null>,"msgToken":<CLAUDE_CODE_MESSAGING_TOKEN or null>,
//!   "cwd":"<hook process cwd>"}`.
//! Nothing is written to stdout (SessionStart stdout would become model context) and the
//! tokens are never printed.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Whole HTTP exchange (connect + send + status line).
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
/// Claude Code writes stdin and closes it at once; never wait longer than this for it.
pub const STDIN_TIMEOUT: Duration = Duration::from_millis(1500);
/// Upper bound on the stdin we forward.
const STDIN_MAX: u64 = 4 * 1024 * 1024;

/// `Some(event)` when the process was started as `<exe> --hook <event>` (`Some("")` when the
/// event is missing: still hook mode, so Tauri is not started).
pub fn hook_event(args: &[String]) -> Option<String> {
    match args.get(1).map(String::as_str) {
        Some("--hook") => Some(args.get(2).cloned().unwrap_or_default()),
        _ => None,
    }
}

/// Event names go into the URL path: ASCII letters only (`SessionStart`, `Stop`, …).
pub fn valid_event(event: &str) -> bool {
    !event.is_empty() && event.len() <= 64 && event.chars().all(|c| c.is_ascii_alphabetic())
}

/// Hook mode entry point. Always returns 0.
pub fn run(event: &str) -> i32 {
    if let Err(e) = try_run(event) {
        // stderr only (never the token); Claude Code ignores stderr of a 0 exit
        eprintln!("[agentboard hook {event}] {e}");
    }
    0
}

fn try_run(event: &str) -> Result<(), String> {
    if !valid_event(event) {
        return Err("잘못된 hook 이벤트 이름".into());
    }
    let url = std::env::var("AGENTBOARD_URL").map_err(|_| "AGENTBOARD_URL 없음")?;
    let token = std::env::var("AGENTBOARD_TOKEN").map_err(|_| "AGENTBOARD_TOKEN 없음")?;
    let stdin = read_stdin(STDIN_TIMEOUT);
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let body = build_body(
        event,
        &stdin,
        std::env::var("CLAUDE_CODE_MESSAGING_SOCKET").ok(),
        std::env::var("CLAUDE_CODE_MESSAGING_TOKEN").ok(),
        &cwd,
    );
    let status = post(&url, event, &token, &body.to_string(), HTTP_TIMEOUT)?;
    if !(200..300).contains(&status) {
        return Err(format!("제어 서버 응답 {status}"));
    }
    Ok(())
}

/// Read all of stdin on a helper thread; give up after `timeout` (the thread is abandoned —
/// the process exits right after).
fn read_stdin(timeout: Duration) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::stdin().lock().take(STDIN_MAX).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx.recv_timeout(timeout).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// The POST body (see module docs).
pub fn build_body(event: &str, stdin: &str, socket: Option<String>, msg_token: Option<String>, cwd: &str) -> Value {
    let parsed = serde_json::from_str::<Value>(stdin).unwrap_or_else(|_| Value::String(stdin.to_string()));
    json!({ "event": event, "stdin": parsed, "socket": socket, "msgToken": msg_token, "cwd": cwd })
}

/// `http://host:port[/base]` → (host, port, base path without trailing `/`). Only plain http.
pub fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = authority.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    if host.is_empty() || port == 0 {
        return None;
    }
    Some((host.to_string(), port, path.trim_end_matches('/').to_string()))
}

/// Minimal blocking HTTP/1.1 POST; returns the response status code.
pub fn post(url: &str, event: &str, token: &str, body: &str, timeout: Duration) -> Result<u16, String> {
    let deadline = Instant::now() + timeout;
    let (host, port, base) = parse_http_url(url).ok_or("AGENTBOARD_URL 형식 오류")?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("주소 오류: {e}"))?
        .next()
        .ok_or("주소 없음")?;
    let left = |d: Instant| d.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
    let mut s = TcpStream::connect_timeout(&addr, left(deadline)).map_err(|e| format!("연결 실패: {e}"))?;
    s.set_write_timeout(Some(left(deadline))).ok();
    let head = format!(
        "POST {base}/hook/{event} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body.as_bytes())).map_err(|e| format!("전송 실패: {e}"))?;
    let mut resp = Vec::new();
    let mut chunk = [0u8; 256];
    // only the status line is needed; each read gets only what is left of the 2 s budget
    while !resp.contains(&b'\n') {
        if Instant::now() >= deadline {
            return Err("응답 시간 초과".into());
        }
        s.set_read_timeout(Some(left(deadline))).ok();
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => resp.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(format!("응답 없음: {e}")),
        }
    }
    let line = String::from_utf8_lossy(&resp);
    line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| "응답 형식 오류".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn hook_mode_is_detected_from_args() {
        assert_eq!(hook_event(&s(&["agentboard.exe", "--hook", "Stop"])).as_deref(), Some("Stop"));
        assert_eq!(hook_event(&s(&["agentboard.exe", "--hook"])).as_deref(), Some(""));
        assert_eq!(hook_event(&s(&["agentboard.exe"])), None);
        assert_eq!(hook_event(&s(&["agentboard.exe", "--other", "--hook", "Stop"])), None);
        assert!(valid_event("SessionStart") && !valid_event("") && !valid_event("../x") && !valid_event("a b"));
        assert_eq!(run("bad/event"), 0, "always 0");
    }

    #[test]
    fn body_has_parsed_or_raw_stdin_and_session_env() {
        let b = build_body("SessionStart", r#"{"session_id":"x","source":"startup"}"#, Some(r"\\.\pipe\p".into()), Some("m".into()), "C:/w");
        assert_eq!(
            b,
            json!({"event":"SessionStart","stdin":{"session_id":"x","source":"startup"},
                   "socket":"\\\\.\\pipe\\p","msgToken":"m","cwd":"C:/w"})
        );
        let b = build_body("Stop", "not json", None, None, "");
        assert_eq!(b["stdin"], "not json");
        assert_eq!(b["socket"], Value::Null);
        assert_eq!(b["msgToken"], Value::Null);
    }

    #[test]
    fn url_parsing() {
        assert_eq!(parse_http_url("http://127.0.0.1:5000"), Some(("127.0.0.1".into(), 5000, "".into())));
        assert_eq!(parse_http_url("http://127.0.0.1:5000/"), Some(("127.0.0.1".into(), 5000, "".into())));
        assert_eq!(parse_http_url("http://localhost:1/x/"), Some(("localhost".into(), 1, "/x".into())));
        assert_eq!(parse_http_url("https://127.0.0.1:5000"), None);
        assert_eq!(parse_http_url("http://127.0.0.1"), None);
        assert_eq!(parse_http_url("http://127.0.0.1:0"), None, "placeholder URL is never contacted");
    }

    #[test]
    fn post_sends_bearer_and_json_body() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (c, _) = l.accept().unwrap();
            let mut r = std::io::BufReader::new(c.try_clone().unwrap());
            let mut head = Vec::new();
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                head.push(line.trim_end().to_string());
            }
            let len: usize = head
                .iter()
                .find_map(|h| h.strip_prefix("Content-Length: "))
                .unwrap()
                .parse()
                .unwrap();
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            (&c).write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n").unwrap();
            (head, String::from_utf8(body).unwrap())
        });
        let body = build_body("Stop", "{}", None, None, "C:/w").to_string();
        let code = post(&format!("http://127.0.0.1:{port}"), "Stop", "tok", &body, HTTP_TIMEOUT).unwrap();
        assert_eq!(code, 204);
        let (head, got) = server.join().unwrap();
        assert_eq!(head[0], "POST /hook/Stop HTTP/1.1");
        assert!(head.contains(&"Authorization: Bearer tok".to_string()), "{head:?}");
        assert!(head.contains(&"Content-Type: application/json".to_string()));
        assert_eq!(serde_json::from_str::<Value>(&got).unwrap(), serde_json::from_str::<Value>(&body).unwrap());
    }

    #[test]
    fn post_total_time_is_bounded_with_a_trickling_server() {
        // the server sends the status line one byte every 150 ms: per-read timeouts alone
        // would never fire; the overall deadline must
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let _keep = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            for b in b"HTTP/1.1 204 No Content".iter().cycle().take(40) {
                if c.write_all(&[*b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        let t = Instant::now();
        assert!(post(&format!("http://127.0.0.1:{port}"), "Stop", "t", "{}", Duration::from_millis(600)).is_err());
        assert!(t.elapsed() < Duration::from_millis(1200), "{:?}", t.elapsed());
    }

    #[test]
    fn post_times_out_on_silent_server() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let _keep = std::thread::spawn(move || {
            let c = l.accept();
            std::thread::sleep(Duration::from_secs(3));
            drop(c);
        });
        let t = Instant::now();
        assert!(post(&format!("http://127.0.0.1:{port}"), "Stop", "t", "{}", Duration::from_millis(300)).is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
