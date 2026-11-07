//! Streamable HTTP transport for the MCP server (roadmap item deferred by
//! ADR-0019). Hand-rolled HTTP/1.1 over a tokio `TcpListener` — the same
//! no-SDK style as the stdio transport in `mcp.rs`, no new dependencies.
//!
//! Shape: `POST /mcp` routes one JSON-RPC message through
//! `mcp::handle_message` unchanged and answers `application/json`
//! (stateless per request, as the MCP spec allows); `GET /mcp` is the
//! server->client `text/event-stream`, emitting `: keepalive` comments
//! every ~15s plus `notifications/progress` lines for the session named
//! by `Mcp-Session-Id` (minted at initialize). `initialize` also serves
//! as the `initialize` handshake, unchanged. A client that never
//! connects `GET` just misses progress, which is allowed. When a token
//! is configured (`--token` / `VGR_MCP_TOKEN`) every request must carry
//! `Authorization: Bearer <secret>` (401 with a `WWW-Authenticate:
//! Bearer` challenge); non-loopback binds are refused without one, and
//! every request passes a DNS-rebinding guard (`Host` must name the
//! loopback on loopback binds, a foreign `Origin` is refused).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use vygr_core::{Config, VygrError};

use crate::mcp::{handle_message, McpServer, Notify};

/// Hard caps: heads and bodies stay small — this server is a local tool.
const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 10 * 1024 * 1024;
/// SSE keepalive comment cadence.
const KEEPALIVE: Duration = Duration::from_secs(15);
/// Idle keep-alive connections are closed after this long.
const IDLE: Duration = Duration::from_secs(75);

/// Sessions: `Mcp-Session-Id` -> fan-out channel of SSE event lines.
type Sessions = Arc<Mutex<HashMap<String, broadcast::Sender<String>>>>;

/// Bind `addr` (e.g. `127.0.0.1:8080`) and serve MCP over Streamable HTTP.
/// Refuses a non-loopback bind without a bearer token (`VygrError::Config`,
/// exit 3).
pub async fn serve_http(
    http: reqwest::Client,
    addr: &str,
    token: Option<String>,
) -> Result<(), VygrError> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| VygrError::Config(format!("serve --http {addr}: {e}")))?;
    let server = Arc::new(McpServer {
        config: Config::load(),
        http,
    });
    serve_on(listener, server, token).await
}

/// Accept loop for an already-bound listener; shared with the tests, which
/// bind `127.0.0.1:0` and inject a default config.
async fn serve_on(
    listener: TcpListener,
    server: Arc<McpServer>,
    token: Option<String>,
) -> Result<(), VygrError> {
    let local = listener.local_addr().map_err(VygrError::Io)?;
    if !local.ip().is_loopback() && token.is_none() {
        return Err(VygrError::Config(format!(
            "serve --http {local}: refusing a non-loopback bind without --token / VGR_MCP_TOKEN"
        )));
    }
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    eprintln!("vygr: MCP server (Streamable HTTP) listening on http://{local}/mcp");
    loop {
        let (stream, _peer) = listener.accept().await.map_err(VygrError::Io)?;
        // Progress lines and keepalives are tiny; latency beats throughput.
        let _ = stream.set_nodelay(true);
        let server = Arc::clone(&server);
        let sessions = Arc::clone(&sessions);
        let token = token.clone();
        tokio::spawn(async move {
            handle_connection(stream, local, server, token, sessions).await;
        });
    }
}

/// One parsed HTTP/1.1 request. Header names are lowercased; the path has
/// its query string stripped.
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// First value of the named header (case-insensitive), trimmed.
    pub fn header(&self, name: &str) -> Option<&str> {
        header_of(&self.headers, name)
    }
}

fn header_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let name = name.to_ascii_lowercase();
    headers
        .iter()
        .find(|(n, _)| n.as_str() == name.as_str())
        .map(|(_, v)| v.as_str())
}

/// Result of reading one request off the wire.
pub enum ReadOutcome {
    /// A well-formed head (+ body when Content-Length says so).
    Ok(Request),
    /// Malformed head or body: answered with 400, connection closed.
    Malformed(&'static str),
    /// Clean EOF before any bytes (client closed the keep-alive socket).
    Closed,
}

/// Read one request: request line + headers, then the Content-Length body.
/// Bytes past the declared body are pipelined requests: they are handed
/// back through `pending` for the next call instead of being dropped.
pub async fn read_request(stream: &mut BufReader<TcpStream>, pending: &mut Vec<u8>) -> ReadOutcome {
    let mut head = std::mem::take(pending);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = head.windows(4).position(|w| *w == *b"\r\n\r\n") {
            break pos;
        }
        if head.len() > MAX_HEAD {
            return ReadOutcome::Malformed("request head too large");
        }
        match stream.read(&mut chunk).await {
            Ok(0) => {
                return if head.is_empty() {
                    ReadOutcome::Closed
                } else {
                    ReadOutcome::Malformed("truncated request head")
                };
            }
            Ok(n) => head.extend_from_slice(&chunk[..n]),
            // A read error mid-request is indistinguishable from a close.
            Err(_) => return ReadOutcome::Closed,
        }
    };
    let head_str = match std::str::from_utf8(&head[..head_end]) {
        Ok(s) => s,
        Err(_) => return ReadOutcome::Malformed("request head is not UTF-8"),
    };
    let parts = match parse_head(head_str) {
        Ok(p) => p,
        Err(why) => return ReadOutcome::Malformed(why),
    };
    // We decode no transfer encodings (only chunked exists in practice):
    // a body we cannot delimit is refused outright rather than leaving
    // the socket positioned mid-message (request smuggling).
    if header_of(&parts.headers, "transfer-encoding").is_some() {
        return ReadOutcome::Malformed("transfer-encoding is not supported");
    }
    let len = match content_length(&parts.headers) {
        Ok(len) => len.unwrap_or(0),
        Err(why) => return ReadOutcome::Malformed(why),
    };
    // The head read may already have pulled in (part of) the body — common
    // when head and body arrive in one packet — so carry the leftovers over
    // before reading the remainder.
    let mut leftover = head.split_off(head_end + 4);
    let mut body = vec![0u8; len];
    let have = leftover.len().min(len);
    body[..have].copy_from_slice(&leftover[..have]);
    if len > have && stream.read_exact(&mut body[have..]).await.is_err() {
        return ReadOutcome::Malformed("body shorter than Content-Length");
    }
    if leftover.len() > len {
        // A pipelined request starts here: keep it for the next
        // read_request call instead of silently dropping it.
        leftover.drain(..len);
        *pending = leftover;
    }
    ReadOutcome::Ok(Request {
        method: parts.method,
        path: parts.path,
        headers: parts.headers,
        body,
    })
}

/// Parsed request head (no body). Header names are lowercased.
pub struct ParsedHead {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

/// Parse the request line and headers of a complete head (everything
/// before the final blank line). Header names are lowercased, the path is
/// split off its query string.
pub fn parse_head(head: &str) -> Result<ParsedHead, &'static str> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let (method, path, version) = parse_request_line(request_line)?;
    if !version.starts_with("HTTP/1.") {
        return Err("unsupported HTTP version");
    }
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue; // the trailing CRLF of the head, or a stray blank line
        }
        let (name, value) = line.split_once(':').ok_or("malformed header line")?;
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err("malformed header line");
        }
        headers.push((name, value.trim().to_string()));
    }
    Ok(ParsedHead {
        method,
        path,
        headers,
    })
}

/// Parse `METHOD SP request-target SP HTTP-version`.
pub fn parse_request_line(line: &str) -> Result<(String, String, String), &'static str> {
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if method.is_empty() || target.is_empty() || version.is_empty() || parts.next().is_some() {
        return Err("malformed request line");
    }
    Ok((
        method.to_string(),
        target.split('?').next().unwrap_or(target).to_string(),
        version.to_string(),
    ))
}

/// Content-Length of the parsed headers (`None` when absent — fine for
/// GET; POST answers empty bodies with a 400 via the JSON parse error).
/// Non-numeric and oversized values are rejected, and so are conflicting
/// duplicates (RFC 9110 §8.6): a body length that depends on which
/// header a parser picks is the classic request-smuggling primitive.
/// Identical duplicates are harmless and accepted.
pub fn content_length(headers: &[(String, String)]) -> Result<Option<usize>, &'static str> {
    let mut values = headers
        .iter()
        .filter(|(n, _)| n == "content-length")
        .map(|(_, v)| v.as_str());
    let Some(raw) = values.next() else {
        return Ok(None);
    };
    for other in values {
        if other != raw {
            return Err("conflicting content-length headers");
        }
    }
    let len: usize = raw.parse().map_err(|_| "invalid content-length")?;
    if len > MAX_BODY {
        return Err("body too large");
    }
    Ok(Some(len))
}

/// True when the authority names this machine's loopback — `localhost`,
/// `127.0.0.1` or `::1`, with or without a port. This is the only
/// authority a client should ever send a loopback-bound server.
fn host_names_loopback(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6 literal, optionally `[::1]:PORT`.
        let name = rest.split_once(']').map_or(rest, |(n, _)| n);
        return name == "::1";
    }
    // Cut the port only when there is exactly one colon (host:port);
    // bare IPv6 literals like ::1 keep every colon.
    let name = match host.matches(':').count() {
        1 => host.split(':').next().unwrap_or_default(),
        _ => host.as_str(),
    };
    matches!(name, "localhost" | "127.0.0.1" | "::1")
}

/// DNS-rebinding guard: the Streamable HTTP security warning requires
/// servers to validate `Origin` because a web page can reach a loopback
/// server either through rebound DNS (its `Host` is the attacker's
/// domain) or with a blind CORS-simple POST (its `Origin` is the
/// attacker's site). Both name a non-loopback authority and are refused
/// before routing; non-browser clients (curl, agent harnesses) send no
/// `Origin` and the loopback address as `Host`.
fn rebinding_guard(req: &Request, local: std::net::SocketAddr) -> Option<&'static str> {
    if let Some(origin) = req.header("origin") {
        let authority = origin.split_once("://").map_or(origin, |(_, a)| a);
        if !host_names_loopback(authority) {
            return Some("cross-origin request refused (DNS-rebinding guard)");
        }
    }
    // A non-loopback bind (token required) can be reached under any of
    // the machine's addresses, so only loopback binds pin the Host.
    if local.ip().is_loopback() {
        if let Some(host) = req.header("host") {
            if !host_names_loopback(host) {
                return Some("host header is not this loopback server (DNS-rebinding guard)");
            }
        }
    }
    None
}

/// Constant-time-ish byte equality: no early return on the first mismatch.
/// The length is not a secret here (both sides are known token shapes).
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (x, y) in a.iter().zip(b) {
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

/// `Authorization: Bearer <token>` check against the configured secret.
pub fn authorized(req: &Request, token: &str) -> bool {
    let Some(given) = req.header("authorization") else {
        return false;
    };
    let Some(bearer) = given
        .strip_prefix("Bearer ")
        .or_else(|| given.strip_prefix("bearer "))
    else {
        return false;
    };
    constant_time_eq(bearer.trim(), token)
}

/// Connection loop: read requests until the peer closes (or idles out),
/// answering each one. The SSE branch consumes the connection until the
/// client disconnects. `pending` carries bytes of a pipelined request
/// between reads.
async fn handle_connection(
    stream: TcpStream,
    local: std::net::SocketAddr,
    server: Arc<McpServer>,
    token: Option<String>,
    sessions: Sessions,
) {
    let mut reader = BufReader::new(stream);
    let mut pending = Vec::new();
    loop {
        let req = match tokio::time::timeout(IDLE, read_request(&mut reader, &mut pending)).await {
            Ok(ReadOutcome::Ok(req)) => req,
            Ok(ReadOutcome::Malformed(why)) => {
                let body = jsonrpc_error(-32600, &format!("malformed request: {why}"));
                let _ = write_json(&mut reader, "400 Bad Request", &body, &[]).await;
                break;
            }
            _ => break, // clean close, idle timeout, or read error
        };
        if let Some(why) = rebinding_guard(&req, local) {
            let body = jsonrpc_error(-32600, why);
            let _ = write_json(&mut reader, "403 Forbidden", &body, &[]).await;
            continue;
        }
        if let Some(t) = &token {
            if !authorized(&req, t) {
                let body = jsonrpc_error(-32001, "unauthorized: missing or wrong bearer token");
                let extra = [("WWW-Authenticate".to_string(), "Bearer".to_string())];
                let _ = write_json(&mut reader, "401 Unauthorized", &body, &extra).await;
                continue;
            }
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/mcp") => {
                let resp = handle_post(&server, &req, &sessions).await;
                if write_response(&mut reader, &resp).await.is_err() {
                    break;
                }
            }
            ("GET", "/mcp") => {
                let _ = serve_sse(&mut reader, &req, &sessions).await;
                break; // the SSE stream owns the connection until it ends
            }
            _ => {
                let body = jsonrpc_error(-32600, "not found: POST or GET /mcp");
                let _ = write_json(&mut reader, "404 Not Found", &body, &[]).await;
            }
        }
    }
}

/// A JSON-RPC error object used as an HTTP error body: transport-level
/// failures never reach `handle_message` but stay machine-readable.
fn jsonrpc_error(code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": code, "message": message }
    })
}

/// A rendered HTTP response with an exact `Content-Length`.
struct Response {
    status: &'static str,
    content_type: &'static str,
    extra: Vec<(String, String)>,
    body: Vec<u8>,
}

fn json_response(status: &'static str, body: Value, extra: Vec<(String, String)>) -> Response {
    Response {
        status,
        content_type: "application/json",
        extra,
        body: body.to_string().into_bytes(),
    }
}

/// Write a `Content-Length`-framed JSON response.
async fn write_json(
    stream: &mut BufReader<TcpStream>,
    status: &'static str,
    body: &Value,
    extra: &[(String, String)],
) -> std::io::Result<()> {
    write_response(
        stream,
        &Response {
            status,
            content_type: "application/json",
            extra: extra.to_vec(),
            body: body.to_string().into_bytes(),
        },
    )
    .await
}

async fn write_response(stream: &mut BufReader<TcpStream>, resp: &Response) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
        resp.status,
        resp.content_type,
        resp.body.len()
    );
    for (name, value) in &resp.extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.get_mut().write_all(head.as_bytes()).await?;
    stream.get_mut().write_all(&resp.body).await?;
    Ok(())
}

/// `POST /mcp`: parse the JSON-RPC message, route it through the shared
/// `handle_message`, answer `application/json`. Notifications (no id)
/// get `202 Accepted` with no body, per the Streamable HTTP spec for
/// stateless servers. `initialize` mints a session id, returned as
/// `Mcp-Session-Id`; later POSTs are matched back to it by that request
/// header so progress notifications reach the client's GET stream.
async fn handle_post(server: &McpServer, req: &Request, sessions: &Sessions) -> Response {
    let msg: Value = match serde_json::from_slice(&req.body) {
        Ok(msg) => msg,
        Err(e) => {
            let body = jsonrpc_error(-32700, &format!("parse error: {e}"));
            return json_response("400 Bad Request", body, vec![]);
        }
    };
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let (session, issued) = if method == "initialize" {
        let id = new_session_id();
        register_session(sessions, &id);
        (Some(id.clone()), Some(id))
    } else {
        (req.header("mcp-session-id").map(str::to_string), None)
    };
    let notify = progress_notify(sessions, session);
    match handle_message(server, &msg, &notify).await {
        Some(reply) => {
            let extra = issued
                .map(|id| vec![("Mcp-Session-Id".to_string(), id)])
                .unwrap_or_default();
            json_response("200 OK", reply, extra)
        }
        None => Response {
            status: "202 Accepted",
            content_type: "application/json",
            extra: vec![],
            body: Vec::new(),
        },
    }
}

/// `GET /mcp`: the server->client SSE channel. Emits a `: keepalive`
/// comment every ~15s plus any progress lines fanned out to the session.
/// Runs until the client disconnects.
async fn serve_sse(
    stream: &mut BufReader<TcpStream>,
    req: &Request,
    sessions: &Sessions,
) -> std::io::Result<()> {
    let (_tx, mut events) = subscribe(sessions, req.header("mcp-session-id"));
    let head =
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
    stream.get_mut().write_all(head.as_bytes()).await?;
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut session_gone = false;
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                stream.get_mut().write_all(b": keepalive\n\n").await?;
            }
            line = events.recv(), if !session_gone => match line {
                Ok(line) => {
                    stream.get_mut().write_all(line.as_bytes()).await?;
                }
                // Fell behind: skip the missed events, keep the stream.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                // Session unregistered: keepalives still flow.
                Err(broadcast::error::RecvError::Closed) => session_gone = true,
            }
        }
    }
}

/// A `Notify` that fans notifications out to the SSE streams of the named
/// session. Unknown sessions drop the events silently — a client that
/// never connects GET just misses progress, which is allowed.
fn progress_notify(sessions: &Sessions, session: Option<String>) -> Notify {
    let sessions = Arc::clone(sessions);
    Arc::new(move |v: Value| {
        let Some(session) = &session else { return };
        let Ok(map) = sessions.lock() else { return };
        if let Some(tx) = map.get(session) {
            let _ = tx.send(format!("data: {v}\n\n"));
        }
    })
}

/// Register a session with a fresh fan-out channel for its SSE stream.
fn register_session(sessions: &Sessions, id: &str) {
    let (tx, _rx) = broadcast::channel(64);
    if let Ok(mut map) = sessions.lock() {
        map.insert(id.to_string(), tx);
    }
}

/// Subscribe to a session's event stream. Unknown or absent session ids
/// get a private channel that only ever sees keepalives; the sender stays
/// with the caller so the channel does not close under the reader.
fn subscribe(
    sessions: &Sessions,
    session: Option<&str>,
) -> (broadcast::Sender<String>, broadcast::Receiver<String>) {
    if let Some(id) = session {
        if let Ok(map) = sessions.lock() {
            if let Some(tx) = map.get(id) {
                return (tx.clone(), tx.subscribe());
            }
        }
    }
    broadcast::channel(16)
}

/// Mint a session id without extra dependencies: pid + monotonic counter
/// + wall-clock nanos. Opaque per the spec.
fn new_session_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("mcp-{}-{n}-{nanos}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer {
            config: Config::default(),
            http: reqwest::Client::new(),
        }
    }

    // --- request-line / header parser (pure, no sockets) ---

    #[test]
    fn parses_get_and_post_heads() {
        let get = parse_head(
            "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nAccept: text/event-stream\r\n",
        )
        .unwrap();
        assert_eq!(get.method, "GET");
        assert_eq!(get.path, "/mcp");
        assert_eq!(header_of(&get.headers, "host"), Some("127.0.0.1:8080"));
        assert_eq!(header_of(&get.headers, "ACCEPT"), Some("text/event-stream"));
        assert_eq!(header_of(&get.headers, "missing"), None);

        let post = parse_head(
            "POST /mcp?x=1 HTTP/1.1\r\nContent-Length: 41\r\nContent-Type: application/json\r\n",
        )
        .unwrap();
        assert_eq!(post.method, "POST");
        assert_eq!(post.path, "/mcp"); // query string stripped
        assert_eq!(content_length(&post.headers).unwrap(), Some(41));
    }

    #[test]
    fn rejects_malformed_heads() {
        assert!(parse_head("garbage\r\n").is_err());
        // Double space -> empty target.
        assert!(parse_head("GET  /mcp HTTP/1.1\r\n").is_err());
        // Missing version / unsupported version.
        assert!(parse_head("GET /mcp\r\n").is_err());
        assert!(parse_head("GET /mcp HTTP/2\r\n").is_err());
        // Header line without a colon.
        assert!(parse_head("GET /mcp HTTP/1.1\r\nBadHeader\r\n").is_err());
    }

    #[test]
    fn content_length_validation() {
        // Missing is fine at the parser level (GET has no body).
        let none = parse_head("GET /mcp HTTP/1.1\r\n").unwrap();
        assert_eq!(content_length(&none.headers).unwrap(), None);
        let ok = parse_head("POST /mcp HTTP/1.1\r\nContent-Length: 12\r\n").unwrap();
        assert_eq!(content_length(&ok.headers).unwrap(), Some(12));
        // Wrong content-length: non-numeric and over the cap.
        let bad = parse_head("POST /mcp HTTP/1.1\r\nContent-Length: abc\r\n").unwrap();
        assert!(content_length(&bad.headers).is_err());
        let huge = parse_head("POST /mcp HTTP/1.1\r\nContent-Length: 99999999999\r\n").unwrap();
        assert!(content_length(&huge.headers).is_err());
        // Conflicting duplicates are refused (smuggling primitive);
        // identical duplicates are fine.
        let dup = parse_head("POST /mcp HTTP/1.1\r\nContent-Length: 6\r\nContent-Length: 39\r\n")
            .unwrap();
        assert!(content_length(&dup.headers).is_err());
        let same =
            parse_head("POST /mcp HTTP/1.1\r\nContent-Length: 6\r\nContent-Length: 6\r\n").unwrap();
        assert_eq!(content_length(&same.headers).unwrap(), Some(6));
    }

    // --- bearer-token auth ---

    #[test]
    fn bearer_token_check() {
        let req = |auth: Option<&str>| Request {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: auth
                .map(|a| vec![("authorization".to_string(), a.to_string())])
                .unwrap_or_default(),
            body: Vec::new(),
        };
        assert!(authorized(&req(Some("Bearer s3cret")), "s3cret"));
        assert!(authorized(&req(Some("bearer s3cret")), "s3cret"));
        assert!(!authorized(&req(Some("Bearer wrong")), "s3cret"));
        assert!(!authorized(&req(Some("s3cret")), "s3cret")); // no scheme
        assert!(!authorized(&req(Some("Basic s3cret")), "s3cret"));
        assert!(!authorized(&req(None), "s3cret"));
        assert!(authorized(&req(Some("Bearer s3cret")), "s3cret"));
    }

    // --- session / progress wiring ---

    #[tokio::test]
    async fn progress_lines_fan_out_to_the_session_stream() {
        let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
        register_session(&sessions, "mcp-1");
        let (_tx, mut rx) = subscribe(&sessions, Some("mcp-1"));
        let (_tx_other, mut rx_other) = subscribe(&sessions, Some("unknown"));
        let notify = progress_notify(&sessions, Some("mcp-1".to_string()));

        let event = json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": { "progressToken": 7, "message": "level 1/2: 5 sources" }
        });
        notify(event.clone());
        assert_eq!(
            rx.recv().await.unwrap(),
            format!("data: {event}\n\n"),
            "the named session's GET stream receives the SSE line"
        );
        // Other sessions never see it; unregistered ones just drop events.
        assert!(rx_other.try_recv().is_err());
        let silent = progress_notify(&sessions, Some("mcp-ghost".to_string()));
        silent(event.clone()); // must not panic
    }

    // --- real-socket roundtrip tests ---

    /// Spawn the HTTP server on 127.0.0.1:0 with a default config.
    async fn spawn_test_server(token: Option<String>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = Arc::new(server());
        tokio::spawn(async move {
            let _ = serve_on(listener, server, token).await;
        });
        addr
    }

    /// POST one JSON-RPC message on a fresh connection; read status,
    /// lowercased headers and body.
    async fn post(
        addr: std::net::SocketAddr,
        msg: &Value,
        headers: &[(&str, &str)],
    ) -> (u16, Vec<(String, String)>, String) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let body = msg.to_string();
        let mut raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            raw.push_str(&format!("{name}: {value}\r\n"));
        }
        raw.push_str("\r\n");
        raw.push_str(&body);
        stream.write_all(raw.as_bytes()).await.unwrap();
        read_response(&mut stream).await
    }

    /// Read a Content-Length-framed response (not SSE).
    async fn read_response(stream: &mut TcpStream) -> (u16, Vec<(String, String)>, String) {
        read_response_spare(stream, &mut Vec::new()).await
    }

    /// Same, but `spare` carries bytes read past this response's body —
    /// a pipelined reply arriving in the same TCP segment — into the next
    /// call, mirroring what the server must do for pipelined requests.
    async fn read_response_spare(
        stream: &mut TcpStream,
        spare: &mut Vec<u8>,
    ) -> (u16, Vec<(String, String)>, String) {
        let mut buf = std::mem::take(spare);
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(pos) = buf.windows(4).position(|w| *w == *b"\r\n\r\n") {
                break pos;
            }
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed before a response head");
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap();
        let status: u16 = status_line
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("unreadable status line: {status_line}"));
        let mut headers = Vec::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        let len: usize = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        while buf.len() < head_end + 4 + len {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "connection closed mid-body");
            buf.extend_from_slice(&chunk[..n]);
        }
        let mut rest = buf.split_off(head_end + 4);
        let body = rest.drain(..len).collect::<Vec<u8>>();
        *spare = rest;
        (status, headers, String::from_utf8(body).unwrap())
    }

    #[tokio::test]
    async fn http_initialize_roundtrip_returns_session_header() {
        let addr = spawn_test_server(None).await;
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" }
        });
        let (status, headers, body) = post(addr, &init, &[]).await;
        assert_eq!(status, 200);
        let reply: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["serverInfo"]["name"], "vygr");
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        let session = headers
            .iter()
            .find(|(name, _)| name == "mcp-session-id")
            .map(|(_, v)| v.clone())
            .expect("initialize response carries Mcp-Session-Id");
        assert!(session.starts_with("mcp-"), "opaque session id: {session}");

        // Keep-alive: a second request on the same connection works.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        for id in [2u64, 3] {
            let ping = json!({ "jsonrpc": "2.0", "id": id, "method": "ping" });
            let body = ping.to_string();
            let raw = format!(
                "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nMcp-Session-Id: {session}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(raw.as_bytes()).await.unwrap();
            let (status, _, body) = read_response(&mut stream).await;
            assert_eq!(status, 200);
            let reply: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(reply["id"], id);
            assert_eq!(reply["result"], json!({}));
        }

        // A notification (no id) is acknowledged with 202 and no body.
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let body = note.to_string();
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(raw.as_bytes()).await.unwrap();
        let (status, _, body) = read_response(&mut stream).await;
        assert_eq!(status, 202);
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn http_token_auth() {
        let addr = spawn_test_server(Some("s3cret".to_string())).await;
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" }
        });
        // No token -> 401 with a JSON-RPC error body and an RFC 6750
        // challenge for discovery-capable clients.
        let (status, headers, body) = post(addr, &init, &[]).await;
        assert_eq!(status, 401);
        let reply: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["error"]["code"], -32001);
        assert_eq!(
            headers
                .iter()
                .find(|(n, _)| n == "www-authenticate")
                .map(|(_, v)| v.as_str()),
            Some("Bearer")
        );
        // Wrong token -> 401 too.
        let (status, ..) = post(addr, &init, &[("Authorization", "Bearer wrong")]).await;
        assert_eq!(status, 401);
        // The right token gets through.
        let (status, _, body) = post(addr, &init, &[("Authorization", "Bearer s3cret")]).await;
        assert_eq!(status, 200);
        let reply: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["result"]["serverInfo"]["name"], "vygr");
    }

    #[tokio::test]
    async fn http_malformed_requests() {
        let addr = spawn_test_server(None).await;
        // Malformed JSON body -> 400 with the parse-error reply.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 3\r\n\r\nnot")
            .await
            .unwrap();
        let (status, _, body) = read_response(&mut stream).await;
        assert_eq!(status, 400);
        let reply: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(reply["error"]["code"], -32700);
        // POST without Content-Length -> empty body -> 400.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let (status, ..) = read_response(&mut stream).await;
        assert_eq!(status, 400);
        // Wrong Content-Length (body shorter than declared): closing the
        // write half surfaces the truncation as EOF -> 400. (A client that
        // keeps the socket open just stalls until the idle timeout closes
        // the connection.)
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 50\r\n\r\n{}")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        let (status, ..) = read_response(&mut stream).await;
        assert_eq!(status, 400);
        // Conflicting duplicate Content-Length: refused without parsing a
        // body (request smuggling).
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 6\r\nContent-Length: 39\r\n\r\n{\"a\":1}EXTRA",
            )
            .await
            .unwrap();
        let (status, ..) = read_response(&mut stream).await;
        assert_eq!(status, 400);
        // Transfer-Encoding: refused — a body we cannot delimit never
        // reaches the JSON parser.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            )
            .await
            .unwrap();
        let (status, ..) = read_response(&mut stream).await;
        assert_eq!(status, 400);
        // Other paths -> 404 (with a loopback Host; a foreign one is 403).
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let (status, ..) = read_response(&mut stream).await;
        assert_eq!(status, 404);
    }

    #[test]
    fn host_names_loopback_cases() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:8080",
            "localhost",
            "localhost:8080",
            "::1",
            "[::1]",
            "[::1]:8080",
        ] {
            assert!(host_names_loopback(host), "{host} should be loopback");
        }
        for host in [
            "evil.com",
            "evil.com:8080",
            "127.0.0.1.evil.com",
            "localhost.evil.com",
            "1.2.3.4",
            "null",
            "",
        ] {
            assert!(!host_names_loopback(host), "{host} should not be loopback");
        }
    }

    #[tokio::test]
    async fn http_rejects_foreign_host_and_origin() {
        let addr = spawn_test_server(None).await;
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" }
        });
        // A rebound Host (attacker domain resolving to the loopback):
        // 403 before routing, connection stays usable.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let body = init.to_string();
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: evil.com\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(raw.as_bytes()).await.unwrap();
        let (status, ..) = read_response(&mut stream).await;
        assert_eq!(status, 403);
        // A foreign Origin (blind CORS-simple POST from a web page): 403.
        let (status, ..) = post(addr, &init, &[("Origin", "https://evil.com")]).await;
        assert_eq!(status, 403);
        // A loopback origin (same-machine browser tooling) passes.
        let (status, ..) = post(addr, &init, &[("Origin", "http://127.0.0.1")]).await;
        assert_eq!(status, 200);
        // Non-browser clients send no Origin at all.
        let (status, ..) = post(addr, &init, &[]).await;
        assert_eq!(status, 200);
    }

    #[tokio::test]
    async fn http_pipelined_requests_are_not_dropped() {
        let addr = spawn_test_server(None).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let b1 = json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string();
        let b2 = json!({"jsonrpc":"2.0","id":2,"method":"ping"}).to_string();
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
            b1.len(),
            b1,
            b2.len(),
            b2
        );
        stream.write_all(raw.as_bytes()).await.unwrap();
        let mut spare = Vec::new();
        for id in [1u64, 2] {
            let (status, _, body) = read_response_spare(&mut stream, &mut spare).await;
            assert_eq!(status, 200, "pipelined request {id} answered");
            let reply: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(reply["id"], id);
            assert_eq!(reply["result"], json!({}));
        }
    }

    #[tokio::test]
    async fn http_get_mcp_streams_keepalive() {
        let addr = spawn_test_server(None).await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n")
            .await
            .unwrap();
        // The SSE headers, then the keepalive comment (first tick is
        // immediate, so the test needs no sleep).
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "SSE stream closed before a keepalive");
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if text.contains(": keepalive") {
                assert!(text.contains("Content-Type: text/event-stream"));
                break;
            }
        }
    }

    #[tokio::test]
    async fn refuses_non_loopback_bind_without_token() {
        let err = serve_http(reqwest::Client::new(), "0.0.0.0:0", None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, VygrError::Config(_)),
            "expected a Config error, got: {err:?}"
        );
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("non-loopback"));
    }
}
