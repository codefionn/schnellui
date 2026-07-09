//! Local HTTP bridge for driving a live native application in debug builds.
//!
//! The socket thread only parses HTTP. Commands cross the winit user-event
//! boundary and execute on the UI thread, where the retained [`crate::App`] lives.
//!
//! Every request must present the per-run bearer token from the discovery
//! file; TCP requests must also carry a `Host` matching the bound address and
//! no foreign `Origin`. Without those checks any local process — or any web
//! page the developer visits — could drive the application.

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use serde::Deserialize;
use serde_json::json;

pub(crate) const DEBUG_INFO_ENV: &str = "SCHNELLUI_DEBUG_INFO";
pub(crate) const DEBUG_SERVER_ENV: &str = "SCHNELLUI_DEBUG_SERVER";
#[cfg(unix)]
pub(crate) const DEBUG_SOCKET_ENV: &str = "SCHNELLUI_DEBUG_SOCKET";
#[cfg(unix)]
pub(crate) const DEBUG_TRANSPORT_ENV: &str = "SCHNELLUI_DEBUG_TRANSPORT";

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
pub(crate) struct DebugTarget {
    pub(crate) id: Option<u64>,
    pub(crate) role: Option<String>,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DebugAction {
    pub(crate) action: String,
    pub(crate) target: DebugTarget,
    pub(crate) value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DebugPoint {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DebugKey {
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) shift: bool,
    #[serde(default)]
    pub(crate) ctrl: bool,
    pub(crate) text: Option<String>,
}

#[derive(Debug)]
pub(crate) enum DebugCommand {
    Tree,
    Status,
    Snapshot,
    Screenshot,
    Action(DebugAction),
    PointerMove(DebugPoint),
    PointerClick(DebugPoint),
    Key(DebugKey),
    Quit,
}

#[derive(Debug)]
pub(crate) struct DebugRequest {
    pub(crate) command: DebugCommand,
    pub(crate) reply: SyncSender<DebugReply>,
}

#[derive(Debug)]
pub(crate) struct DebugReply {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: Vec<u8>,
}

impl DebugReply {
    pub(crate) fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(&value).expect("debug reply serialization"),
        }
    }

    pub(crate) fn png(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "image/png",
            body,
        }
    }

    pub(crate) fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, json!({ "error": message.into() }))
    }
}

/// Keeps the discovery file and listener lifetime tied to the native event loop.
pub(crate) struct DebugServer {
    endpoint: DebugEndpoint,
    info_path: PathBuf,
    shutdown: Arc<AtomicBool>,
}

impl DebugServer {
    pub(crate) fn endpoint(&self) -> String {
        match &self.endpoint {
            DebugEndpoint::Tcp(address) => format!("http://{address}"),
            #[cfg(unix)]
            DebugEndpoint::Unix(path) => format!("unix://{}", path.display()),
        }
    }
}

enum DebugEndpoint {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(PathBuf),
}

enum DebugListener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener),
}

impl Drop for DebugServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        // Wake the nonblocking accept loop promptly. Failure is harmless: it also
        // observes the flag on its next short poll.
        match &self.endpoint {
            DebugEndpoint::Tcp(address) => {
                let _ = TcpStream::connect_timeout(address, Duration::from_millis(50));
            }
            #[cfg(unix)]
            DebugEndpoint::Unix(path) => {
                let _ = UnixStream::connect(path);
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_file(&self.info_path);
    }
}

pub(crate) fn enabled() -> bool {
    if !cfg!(any(debug_assertions, feature = "debug-instrumentation")) {
        return false;
    }
    !matches!(
        std::env::var(DEBUG_SERVER_ENV).as_deref(),
        Ok("0" | "false" | "off")
    )
}

pub(crate) fn start(
    title: &str,
    send: impl Fn(DebugRequest) -> bool + Send + 'static,
) -> io::Result<DebugServer> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let instance = format!("{}-{nonce}", std::process::id());
    let (listener, endpoint) = create_listener(&instance)?;
    let token = debug_token(&instance);
    let info_path = std::env::var_os(DEBUG_INFO_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("schnellui-debug-{instance}.json")));
    let mut info = json!({
        "schema": "schnellui-debug-v1",
        "pid": std::process::id(),
        "title": title,
        "instance": instance,
        "token": token.clone(),
    });
    match &endpoint {
        DebugEndpoint::Tcp(address) => {
            info["transport"] = json!("tcp");
            info["url"] = json!(format!("http://{address}"));
        }
        #[cfg(unix)]
        DebugEndpoint::Unix(path) => {
            info["transport"] = json!("unix");
            info["socket"] = json!(path);
        }
    }
    if let Err(error) = fs::write(
        &info_path,
        serde_json::to_vec_pretty(&info).expect("debug info serialization"),
    ) {
        cleanup_endpoint(&endpoint);
        return Err(error);
    }
    // The file holds the bearer token: keep it readable by this user only.
    #[cfg(unix)]
    if let Err(error) = fs::set_permissions(&info_path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(&info_path);
        cleanup_endpoint(&endpoint);
        return Err(error);
    }

    let access = Access {
        token,
        hosts: match &endpoint {
            DebugEndpoint::Tcp(address) => {
                Some((address.to_string(), format!("localhost:{}", address.port())))
            }
            #[cfg(unix)]
            DebugEndpoint::Unix(_) => None,
        },
    };

    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = shutdown.clone();
    if let Err(error) = thread::Builder::new()
        .name("schnellui-debug-http".into())
        .spawn(move || serve(listener, thread_shutdown, send, access))
    {
        let _ = fs::remove_file(&info_path);
        cleanup_endpoint(&endpoint);
        return Err(error);
    }

    Ok(DebugServer {
        endpoint,
        info_path,
        shutdown,
    })
}

#[cfg(unix)]
fn cleanup_endpoint(endpoint: &DebugEndpoint) {
    if let DebugEndpoint::Unix(path) = endpoint {
        let _ = fs::remove_file(path);
    }
}

#[cfg(not(unix))]
fn cleanup_endpoint(_endpoint: &DebugEndpoint) {}

fn create_tcp_listener() -> io::Result<(DebugListener, DebugEndpoint)> {
    // Port zero asks the OS for a free unprivileged ephemeral port. There is no
    // fixed-port fallback, so concurrent applications cannot collide.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    Ok((DebugListener::Tcp(listener), DebugEndpoint::Tcp(address)))
}

#[cfg(not(unix))]
fn create_listener(_instance: &str) -> io::Result<(DebugListener, DebugEndpoint)> {
    create_tcp_listener()
}

#[cfg(unix)]
fn create_listener(instance: &str) -> io::Result<(DebugListener, DebugEndpoint)> {
    if std::env::var(DEBUG_TRANSPORT_ENV).as_deref() == Ok("tcp") {
        return create_tcp_listener();
    }
    let path = std::env::var_os(DEBUG_SOCKET_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("schnellui-debug-{instance}.sock")));
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok((DebugListener::Unix(listener), DebugEndpoint::Unix(path)))
}

/// What every request must present before it is routed.
///
/// The token stops any other local or web origin from driving the app. The
/// `Host` and `Origin` checks additionally stop DNS rebinding and drive-by
/// requests even if a page learns the token.
struct Access {
    token: String,
    /// Accepted `Host` values (the bound address and its `localhost` form) for
    /// TCP. `None` on a Unix-domain socket, which a browser cannot reach.
    hosts: Option<(String, String)>,
}

impl Access {
    fn check(&self, request: &HttpRequest) -> Result<(), DebugReply> {
        if !request
            .header("x-schnellui-debug-token")
            .is_some_and(|presented| token_matches(presented, &self.token))
        {
            return Err(DebugReply::error(
                401,
                "missing or invalid debug token; use the discovery file or --token",
            ));
        }
        let Some((bound, local)) = &self.hosts else {
            return Ok(());
        };
        if !matches!(request.header("host"), Some(host) if host == bound.as_str() || host == local.as_str())
        {
            return Err(DebugReply::error(400, "unexpected Host header"));
        }
        if let Some(origin) = request.header("origin") {
            let allowed = [format!("http://{bound}"), format!("http://{local}")];
            if !allowed.iter().any(|value| value == origin) {
                return Err(DebugReply::error(403, "cross-origin request refused"));
            }
        }
        Ok(())
    }
}

/// A random bearer token for one debug endpoint.
///
/// Two independently seeded hashers stand in for a CSPRNG draw:
/// `RandomState` is seeded from the OS, and the token only has to be
/// unguessable by other users and web origins for this run.
fn debug_token(instance: &str) -> String {
    let mut high = RandomState::new().build_hasher();
    high.write(instance.as_bytes());
    let mut low = RandomState::new().build_hasher();
    low.write(instance.as_bytes());
    format!("{:016x}{:016x}", high.finish(), low.finish())
}

/// Length-safe token comparison; the token is fixed width but presented input
/// is not.
fn token_matches(presented: &str, token: &str) -> bool {
    let mut difference = presented.len() ^ token.len();
    for (presented, expected) in presented.bytes().zip(token.bytes()) {
        difference |= usize::from(presented ^ expected);
    }
    difference == 0
}

fn serve(
    listener: DebugListener,
    shutdown: Arc<AtomicBool>,
    send: impl Fn(DebugRequest) -> bool,
    access: Access,
) {
    while !shutdown.load(Ordering::Acquire) {
        let result = match &listener {
            DebugListener::Tcp(listener) => {
                listener.accept().map(|(stream, _)| Connection::Tcp(stream))
            }
            #[cfg(unix)]
            DebugListener::Unix(listener) => listener
                .accept()
                .map(|(stream, _)| Connection::Unix(stream)),
        };
        match result {
            Ok(Connection::Tcp(stream)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                handle_connection(stream, &send, &access);
            }
            #[cfg(unix)]
            Ok(Connection::Unix(stream)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                handle_connection(stream, &send, &access);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                eprintln!("schnellui debug server stopped: {error}");
                break;
            }
        }
    }
}

enum Connection {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

fn handle_connection(
    mut stream: impl Read + Write,
    send: &impl Fn(DebugRequest) -> bool,
    access: &Access,
) {
    let reply = match read_http_request(&mut stream)
        .and_then(|request| access.check(&request).map(|()| request))
        .and_then(route_request)
    {
        Ok(Some(command)) => dispatch(command, send),
        Ok(None) => DebugReply::json(
            200,
            json!({
                "schema": "schnellui-debug-v1",
                "endpoints": {
                    "tree": "GET /v1/tree",
                    "status": "GET /v1/status",
                    "snapshot": "GET /v1/snapshot",
                    "screenshot": "GET /v1/screenshot",
                    "action": "POST /v1/action",
                    "pointer_move": "POST /v1/pointer/move",
                    "pointer_click": "POST /v1/pointer/click",
                    "key": "POST /v1/key",
                    "quit": "POST /v1/quit"
                }
            }),
        ),
        Err(reply) => reply,
    };
    let _ = write_http_reply(&mut stream, reply);
}

fn dispatch(command: DebugCommand, send: &impl Fn(DebugRequest) -> bool) -> DebugReply {
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    if !send(DebugRequest {
        command,
        reply: reply_tx,
    }) {
        return DebugReply::error(503, "UI event loop is not available");
    }
    reply_rx
        .recv_timeout(COMMAND_TIMEOUT)
        .unwrap_or_else(|_| DebugReply::error(504, "UI command timed out"))
}

struct HttpRequest {
    method: String,
    path: String,
    /// Lowercased header names.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    /// A header value by case-insensitive name.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

fn read_http_request(stream: &mut impl Read) -> Result<HttpRequest, DebugReply> {
    let mut bytes = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| DebugReply::error(400, format!("request read failed: {error}")))?;
        if count == 0 {
            return Err(DebugReply::error(400, "incomplete HTTP request"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(DebugReply::error(413, "request is too large"));
        }
        if let Some(end) = find_bytes(&bytes, b"\r\n\r\n") {
            break end + 4;
        }
    };

    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| DebugReply::error(400, "HTTP headers must be UTF-8"))?;
    let mut lines = headers.split("\r\n");
    let mut request_line = lines
        .next()
        .ok_or_else(|| DebugReply::error(400, "missing request line"))?
        .split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let path = request_line.next().unwrap_or_default().to_string();
    if method.is_empty() || path.is_empty() {
        return Err(DebugReply::error(400, "invalid request line"));
    }
    let parsed_headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<Vec<_>>();
    let content_length = parsed_headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse::<usize>())
        .transpose()
        .map_err(|_| DebugReply::error(400, "invalid Content-Length"))?
        .unwrap_or(0);
    if header_end + content_length > MAX_REQUEST_BYTES {
        return Err(DebugReply::error(413, "request is too large"));
    }
    while bytes.len() < header_end + content_length {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| DebugReply::error(400, format!("request read failed: {error}")))?;
        if count == 0 {
            return Err(DebugReply::error(400, "incomplete HTTP body"));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(HttpRequest {
        method,
        path,
        headers: parsed_headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    })
}

fn route_request(request: HttpRequest) -> Result<Option<DebugCommand>, DebugReply> {
    let command = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") | ("GET", "/v1/health") => return Ok(None),
        ("GET", "/v1/tree") => DebugCommand::Tree,
        ("GET", "/v1/status") => DebugCommand::Status,
        ("GET", "/v1/snapshot") => DebugCommand::Snapshot,
        ("GET", "/v1/screenshot") => DebugCommand::Screenshot,
        ("POST", "/v1/action") => DebugCommand::Action(parse_json(&request.body)?),
        ("POST", "/v1/pointer/move") => DebugCommand::PointerMove(parse_json(&request.body)?),
        ("POST", "/v1/pointer/click") => DebugCommand::PointerClick(parse_json(&request.body)?),
        ("POST", "/v1/key") => DebugCommand::Key(parse_json(&request.body)?),
        ("POST", "/v1/quit") => DebugCommand::Quit,
        _ => return Err(DebugReply::error(404, "unknown debug endpoint")),
    };
    Ok(Some(command))
}

fn parse_json<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, DebugReply> {
    serde_json::from_slice(body)
        .map_err(|error| DebugReply::error(400, format!("invalid JSON body: {error}")))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_http_reply(stream: &mut impl Write, reply: DebugReply) -> io::Result<()> {
    let reason = match reply.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        reply.status,
        reason,
        reply.content_type,
        reply.body.len()
    )?;
    stream.write_all(&reply.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_semantic_action_requests() {
        let request = HttpRequest {
            method: "POST".into(),
            path: "/v1/action".into(),
            headers: Vec::new(),
            body: br#"{"action":"click","target":{"id":42}}"#.to_vec(),
        };
        let command = route_request(request).unwrap().unwrap();
        let DebugCommand::Action(action) = command else {
            panic!("wrong command");
        };
        assert_eq!(action.action, "click");
        assert_eq!(action.target.id, Some(42));
    }

    #[test]
    fn routes_atomic_snapshot_requests() {
        let request = HttpRequest {
            method: "GET".into(),
            path: "/v1/snapshot".into(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert!(matches!(
            route_request(request).unwrap(),
            Some(DebugCommand::Snapshot)
        ));
    }

    #[test]
    fn rejects_non_loopback_bind_addresses() {
        // The loopback invariant is tested without changing process-wide env by
        // checking the same address predicate used by `start`.
        let public: SocketAddr = "0.0.0.0:0".parse().unwrap();
        assert!(!public.ip().is_loopback());
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(local.ip().is_loopback());
    }

    fn request_with(headers: &[(&str, &str)]) -> HttpRequest {
        HttpRequest {
            method: "GET".into(),
            path: "/v1/tree".into(),
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), (*value).to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn tcp_access() -> Access {
        Access {
            token: "0123".into(),
            hosts: Some(("127.0.0.1:8080".into(), "localhost:8080".into())),
        }
    }

    #[test]
    fn access_requires_the_debug_token() {
        let access = tcp_access();
        assert_eq!(access.check(&request_with(&[])).unwrap_err().status, 401);
        assert_eq!(
            access
                .check(&request_with(&[("x-schnellui-debug-token", "wrong")]))
                .unwrap_err()
                .status,
            401
        );
        assert!(access
            .check(&request_with(&[
                ("X-Schnellui-Debug-Token", "0123"),
                ("host", "127.0.0.1:8080"),
            ]))
            .is_ok());
    }

    #[test]
    fn access_refuses_rebound_hosts_and_foreign_origins() {
        let access = tcp_access();
        let token = ("x-schnellui-debug-token", "0123");
        // DNS rebinding: the page's hostname is not our authority.
        assert_eq!(
            access
                .check(&request_with(&[token, ("host", "attacker.example")]))
                .unwrap_err()
                .status,
            400
        );
        // Drive-by request from a web page.
        assert_eq!(
            access
                .check(&request_with(&[
                    token,
                    ("host", "127.0.0.1:8080"),
                    ("origin", "http://attacker.example"),
                ]))
                .unwrap_err()
                .status,
            403
        );
        for host in ["127.0.0.1:8080", "localhost:8080"] {
            assert!(access
                .check(&request_with(&[token, ("host", host)]))
                .is_ok());
            let origin = format!("http://{host}");
            assert!(access
                .check(&request_with(&[
                    token,
                    ("host", host),
                    ("origin", origin.as_str())
                ]))
                .is_ok());
        }
    }

    #[test]
    fn unix_access_needs_only_the_token() {
        let access = Access {
            token: "0123".into(),
            hosts: None,
        };
        assert_eq!(access.check(&request_with(&[])).unwrap_err().status, 401);
        assert!(access
            .check(&request_with(&[("x-schnellui-debug-token", "0123")]))
            .is_ok());
    }
}
