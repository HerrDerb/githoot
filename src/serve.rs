//! GitHoot's own web server: one loopback listener, serving the PR pages `page` renders.
//!
//! **It touches no `PollState` and adds no shared state.** It reads `scheduler`'s existing
//! `PR_URLS` static — already the sanctioned channel from the poll thread to the menu-click handlers
//! on the UI thread — and is simply a third reader of it. The poll thread still owns `PollState`
//! exclusively, the `mpsc` pipeline is unchanged, and there is still no `Arc<Mutex<AppState>>`.
//!
//! Why a server rather than an HTML file written to `~/.githoot-tray/`: a file persists after the
//! app exits, is readable by anything running as the user and by every browser profile on the
//! machine, and goes stale the moment the next poll lands. A render per request cannot go stale, and
//! a token that dies with the process bounds what a leaked URL is worth.
//!
//! What stands between the page and the rest of the machine, in order of what each one stops:
//!
//! | Guard | Stops |
//! |---|---|
//! | Bind to `127.0.0.1` (and `::1`) only | anything off this machine |
//! | 128-bit token in the path | a hostile page in the user's own browser, which can `fetch()` every loopback port |
//! | `Host` allowlist | DNS rebinding, which arrives with `Host: evil.com` |
//! | No CORS header, ever | a cross-origin page reading the body even if it reached the path |
//! | `Referrer-Policy: no-referrer` | the token reaching github.com in a `Referer` on the first click |
//!
//! There is no shutdown, and that is a decision rather than an omission — see `start`.

use crate::state::PrAxis;
use crate::{errorln, infoln, icons, page, scheduler};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A client that connects and says nothing must not wedge the accept loop. Every stream gets these
/// before a byte is read, which is the one thing that makes handling connections serially safe.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// How much of a request head is read before giving up. A browser sends well under 2 KiB; the cap
/// exists so a client that never terminates its head cannot grow this thread's memory.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// What a parsed request line and its `Host` amount to.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub path: String,
    pub host: Option<String>,
    /// `HEAD` wants the headers and no body.
    pub body_wanted: bool,
}

/// What the server should answer with.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Page(PrAxis),
    Owl,
    /// The path did not resolve. Also what a wrong token gets, so the answer does not confirm that
    /// the rest of the path was right.
    NotFound,
    /// The `Host` header named something other than this server.
    Forbidden,
}

/// Splits a request head into the two things this server reads: the target and the `Host`.
///
/// `Err(status)` is what to answer with instead. Only `GET` and `HEAD` are accepted; the HTTP version
/// is ignored and every answer is `HTTP/1.1` regardless, which no browser minds.
///
/// The query string and fragment are stripped, and the remainder is then compared **raw**, never
/// percent-decoded. The token is `[0-9a-f]` and every slug is `[a-z-]`, so decoding would buy nothing
/// and would open a class of normalization bugs for free.
pub fn parse_request(head: &str) -> Result<Request, u16> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(400u16)?;
    let mut parts = request_line.split(' ');
    let (method, target) = (parts.next().ok_or(400u16)?, parts.next().ok_or(400u16)?);

    let body_wanted = match method {
        "GET" => true,
        "HEAD" => false,
        _ => return Err(405),
    };
    if !target.starts_with('/') {
        return Err(400);
    }

    let path = target.split(['?', '#']).next().unwrap_or(target).to_string();
    let host = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim().to_string());

    Ok(Request { path, host, body_wanted })
}

/// The one accepted hostname. `.localhost` is reserved by RFC 6761 and browsers resolve every
/// subdomain of it to loopback themselves, with no `hosts` entry and no admin rights — which is what
/// makes a readable address possible at all. It is also why accepting it costs nothing against DNS
/// rebinding: there is no record for an attacker to flip.
const PAGE_HOST: &str = "githoot.localhost";

/// What to answer a request for `path` with.
///
/// The three guards are applied in the order that leaks least. `Host` first, because a request that
/// is not for us should not have its path examined at all. Then the token, in constant time, and a
/// mismatch answers `NotFound` rather than `Forbidden` so the reply does not confirm that everything
/// after the token was right.
pub fn route(path: &str, host: Option<&str>, token: &str, port: u16) -> Route {
    let allowed = [format!("{PAGE_HOST}:{port}"), format!("127.0.0.1:{port}")];
    match host {
        Some(h) if allowed.iter().any(|a| a == h) => {}
        _ => return Route::Forbidden,
    }

    // No filesystem is ever touched, so this is a pin rather than a defence — but a path that tries
    // to climb has nothing legitimate to say to this server.
    if path.contains("..") {
        return Route::NotFound;
    }

    let mut segments = path.strip_prefix('/').unwrap_or_default().split('/');
    let (Some(given), Some(leaf), None) = (segments.next(), segments.next(), segments.next()) else {
        return Route::NotFound;
    };
    if !constant_time_eq(given, token) {
        return Route::NotFound;
    }

    if leaf == "owl.png" {
        return Route::Owl;
    }
    PrAxis::from_slug(leaf).map_or(Route::NotFound, Route::Page)
}

/// Compares two strings without an early exit on the first differing byte.
///
/// A timing oracle over loopback, against a token with no partial-match feedback, is a stretch. The
/// function is five lines, and having it removes the argument.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The response head, identical in shape for every status this server can produce.
///
/// `Connection: close` on every one is what keeps the HTTP here to a few dozen lines: no keep-alive
/// means no pipelining, no chunked bodies and no connection state to get wrong, at the cost of one
/// extra loopback handshake per navigation.
///
/// `style-src 'unsafe-inline'` is acceptable **only** because `page::STYLESHEET` is a `&'static str`
/// with zero interpolation — nothing user-controlled can reach it. If that ever stops being true,
/// this directive has to go with it.
pub fn response_head(status: u16, content_type: &str, len: usize) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Not Found",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; \
         form-action 'none'; base-uri 'none'\r\n\
         \r\n"
    )
}

/// 128 bits of hex from the platform's CSPRNG.
///
/// The token is what stands between the page and a hostile web page in the user's own browser, which
/// can `fetch()` across every loopback port from JavaScript and gets unlimited guesses. That is why
/// this is a real CSPRNG and not `RandomState`'s hash seed: the latter is not documented as one, its
/// derivation is an implementation detail that has changed before, and "probably unpredictable" is
/// not a thing to put in front of an attacker who can keep trying.
///
/// No new crate on any platform: `/dev/urandom` is std, and `BCryptGenRandom` comes from `winapi`,
/// which is already a Windows dependency — the same argument the `winreg` note in `Cargo.toml` makes.
fn new_token() -> Option<String> {
    let mut buf = [0u8; 16];
    random_bytes(&mut buf).ok()?;
    Some(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn random_bytes(buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

/// `winapi::shared::bcrypt`, not `um` — `bcrypt.rs` lives under `shared/` in winapi 0.3, and getting
/// that wrong is a Windows-only compile error a Linux build cannot see. `cargo check --target
/// x86_64-pc-windows-msvc` catches it without an MSVC linker, since checking does not link.
#[cfg(windows)]
fn random_bytes(buf: &mut [u8]) -> std::io::Result<()> {
    use winapi::shared::bcrypt::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    // The flag means no algorithm handle has to be opened first, which is why the first argument is
    // null. A non-zero `NTSTATUS` is a failure, and `start` treats that as "no page at all" rather
    // than falling back to a weaker source.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("BCryptGenRandom failed: {status:#x}")))
    }
}

/// A bound listener: the port the OS gave us and the token that guards it.
struct Server {
    port: u16,
    token: String,
}

/// Bound on the first menu click, never at boot, and never retried once it has failed.
static SERVER: OnceLock<Option<Server>> = OnceLock::new();

/// Opens `axis`'s GitHoot page, or GitHub's own search page when there is no server to serve one.
///
/// One helper rather than two copies, because the menu is built twice — GTK on Linux, `tray-icon` on
/// Windows and macOS — and a behaviour that lives in both copies is a behaviour that will differ in
/// them eventually.
pub fn open_axis_page(axis: PrAxis) {
    let url = url_for(axis).unwrap_or_else(|| scheduler::pr_list_url(axis));
    if let Err(e) = open::that(&url) {
        errorln!("failed to open browser: {e}");
    }
}

/// The page's URL, binding the listener on the first call.
///
/// Lazy rather than started at boot, for three reasons. The poll loop already skips work it calls
/// "pure cost", and a socket held open for days for someone who never clicks is the same kind. Any
/// firewall or EDR reaction then lands immediately after a deliberate click, where it is explicable,
/// rather than three seconds into sign-in. And this binary has history with Windows Defender's ML
/// heuristics — see the `winresource` note in `Cargo.toml` — so opening a listening socket only when
/// asked is the conservative default.
fn url_for(axis: PrAxis) -> Option<String> {
    let server = SERVER.get_or_init(start).as_ref()?;
    Some(format!("http://{PAGE_HOST}:{}/{}/{}", server.port, server.token, axis.slug()))
}

/// Binds both loopback listeners on one port and spawns a thread for each.
///
/// v4 first, on port 0 so the OS assigns, then v6 pinned to the same number. Both are needed because
/// a browser may resolve `githoot.localhost` to `::1` before `127.0.0.1`, and a v4-only listener
/// would answer that with a connection refused. A failed v6 bind is survivable — browsers fall back —
/// so it is logged and carried on from; a failed v4 bind is not, and the menu falls back to GitHub.
///
/// Literal addresses only. `0.0.0.0` or `[::]` would be a network-reachable listener, which is both
/// the thing this design exists to avoid and the thing that raises the Windows firewall prompt.
///
/// **There is no shutdown, and that is a decision.** `TcpListener::accept` cannot be unblocked from
/// another thread in std, and it does not need to be: every exit path ends the process, so the kernel
/// closes the socket. Port 0 means a successor started by the updater never collides with one of ours
/// left in `TIME_WAIT`.
fn start() -> Option<Server> {
    let token = new_token().or_else(|| {
        // Failing closed. A predictable token is worse than no page at all.
        errorln!("no system randomness available, so the PR page cannot be served safely");
        None
    })?;

    let v4 = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(e) => {
            errorln!("could not open the PR page's local listener: {e}");
            return None;
        }
    };
    let port = match v4.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => {
            errorln!("could not read the PR page's local port: {e}");
            return None;
        }
    };

    for listener in [Some(v4), TcpListener::bind(("::1", port)).ok()].into_iter().flatten() {
        let token = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                // One panic must not take the listener down with it, the same guard the poll thread
                // gets. A wedged page is a dead menu entry until the app restarts.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle(stream, &token, port);
                }));
            }
        });
    }

    infoln!("PR page listening on http://{PAGE_HOST}:{port}/");
    Some(Server { port, token })
}

/// One connection: read the head, answer, close.
fn handle(mut stream: TcpStream, token: &str, port: u16) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let head = match read_head(&mut stream) {
        Ok(head) => head,
        Err(status) => return respond(&mut stream, status, "text/plain; charset=utf-8", b"", true),
    };

    let request = match parse_request(&head) {
        Ok(request) => request,
        Err(status) => return respond(&mut stream, status, "text/plain; charset=utf-8", b"", true),
    };

    match route(&request.path, request.host.as_deref(), token, port) {
        Route::Owl => {
            respond(&mut stream, 200, "image/png", icons::TRAY_ICON, request.body_wanted)
        }
        Route::Page(axis) => {
            let (entries, polled) = scheduler::pr_snapshot(axis);
            let html = page::axis_page(
                axis,
                entries.as_deref(),
                polled,
                token,
                unix_now(),
                &scheduler::pr_list_url(axis),
            );
            respond(&mut stream, 200, "text/html; charset=utf-8", html.as_bytes(), request.body_wanted)
        }
        Route::Forbidden => {
            respond(&mut stream, 403, "text/plain; charset=utf-8", b"Forbidden", request.body_wanted)
        }
        Route::NotFound => {
            respond(&mut stream, 404, "text/plain; charset=utf-8", b"Not found", request.body_wanted)
        }
    }
}

/// Reads until the blank line that ends the head, or gives up.
///
/// The cap is what bounds memory against a client that never sends one. The body is never read at
/// all: `GET` and `HEAD` are the only methods accepted, and with no keep-alive anything left on the
/// wire dies with the connection.
fn read_head(stream: &mut TcpStream) -> Result<String, u16> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    loop {
        let read = stream.read(&mut chunk).map_err(|_| 400u16)?;
        if read == 0 {
            return Err(400);
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(431);
        }
    }
    String::from_utf8(buf).map_err(|_| 400u16)
}

/// Writes one response and closes. A `HEAD` gets the head, `Content-Length` included, and no body.
fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8], with_body: bool) {
    let head = response_head(status, content_type, body.len());
    let _ = stream.write_all(head.as_bytes());
    if with_body {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const PORT: u16 = 49731;

    fn head(method: &str, path: &str, host: &str) -> String {
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: x\r\n\r\n")
    }

    fn ok_host() -> String {
        format!("githoot.localhost:{PORT}")
    }

    // ── Request parsing ───────────────────────────────────────────────────────

    #[test]
    fn a_plain_get_is_parsed() {
        let r = parse_request(&head("GET", "/a/b", "h")).expect("should parse");
        assert_eq!(r.path, "/a/b");
        assert_eq!(r.host.as_deref(), Some("h"));
        assert!(r.body_wanted);
    }

    #[test]
    fn a_head_request_wants_no_body() {
        assert!(!parse_request(&head("HEAD", "/a", "h")).expect("should parse").body_wanted);
    }

    #[test]
    fn anything_but_get_or_head_is_rejected() {
        for method in ["POST", "PUT", "DELETE", "OPTIONS", "TRACE"] {
            assert_eq!(parse_request(&head(method, "/a", "h")), Err(405), "{method}");
        }
    }

    /// HTTP/1.1 requires it, and the `Host` allowlist is one of the three real guards — a request
    /// that names no host cannot be checked against it.
    #[test]
    fn a_missing_host_is_rejected() {
        assert!(parse_request("GET /a HTTP/1.1\r\n\r\n").expect("parses").host.is_none());
    }

    #[test]
    fn a_query_string_and_fragment_are_stripped_from_the_path() {
        assert_eq!(parse_request(&head("GET", "/a/b?x=1", "h")).unwrap().path, "/a/b");
        assert_eq!(parse_request(&head("GET", "/a/b#frag", "h")).unwrap().path, "/a/b");
    }

    #[test]
    fn header_names_are_matched_case_insensitively() {
        let raw = "GET /a HTTP/1.1\r\nHOST: shouty\r\n\r\n";
        assert_eq!(parse_request(raw).unwrap().host.as_deref(), Some("shouty"));
    }

    #[test]
    fn a_head_block_without_a_request_line_is_rejected() {
        assert_eq!(parse_request(""), Err(400));
        assert_eq!(parse_request("garbage\r\n\r\n"), Err(400));
    }

    // ── Routing and the guards ────────────────────────────────────────────────

    #[test]
    fn the_right_token_and_host_reach_every_axis_page() {
        for axis in PrAxis::ALL {
            let path = format!("/{TOKEN}/{}", axis.slug());
            assert_eq!(route(&path, Some(&ok_host()), TOKEN, PORT), Route::Page(axis));
        }
    }

    #[test]
    fn the_numeric_host_is_accepted_too() {
        let host = format!("127.0.0.1:{PORT}");
        let path = format!("/{TOKEN}/approved");
        assert_eq!(route(&path, Some(&host), TOKEN, PORT), Route::Page(PrAxis::ReadyToMerge));
    }

    /// A wrong token answers 404, not 403, so the response does not confirm the path shape was right.
    #[test]
    fn a_wrong_token_is_not_found_rather_than_forbidden() {
        let path = format!("/{}/approved", "f".repeat(32));
        assert_eq!(route(&path, Some(&ok_host()), TOKEN, PORT), Route::NotFound);
    }

    #[test]
    fn a_token_of_the_right_length_with_one_wrong_byte_is_rejected() {
        let mut wrong = TOKEN.to_string();
        wrong.replace_range(31..32, "0");
        assert_eq!(route(&format!("/{wrong}/approved"), Some(&ok_host()), TOKEN, PORT), Route::NotFound);
    }

    /// The DNS-rebinding defence. A rebinding request arrives with the attacker's own name in `Host`.
    /// Bare `localhost` is rejected with the rest: nothing we hand the browser uses it, and it is a
    /// name a resolver can be talked out of, unlike `githoot.localhost`, which browsers pin to
    /// loopback themselves.
    #[test]
    fn any_other_host_is_forbidden() {
        let path = format!("/{TOKEN}/approved");
        for host in [
            "evil.com",
            &format!("evil.com:{PORT}"),
            &format!("localhost:{PORT}"),
            "githoot.localhost",
            &format!("githoot.localhost.evil.com:{PORT}"),
            &format!("githoot.localhost:{}", PORT + 1),
            &format!("127.0.0.1:{}", PORT + 1),
            "127.0.0.1",
        ] {
            assert_eq!(route(&path, Some(host), TOKEN, PORT), Route::Forbidden, "{host}");
        }
    }

    #[test]
    fn a_request_with_no_host_is_forbidden() {
        assert_eq!(route(&format!("/{TOKEN}/approved"), None, TOKEN, PORT), Route::Forbidden);
    }

    #[test]
    fn an_unknown_slug_or_a_traversal_is_not_found() {
        for path in [
            format!("/{TOKEN}/nope"),
            format!("/{TOKEN}/../../etc/passwd"),
            format!("/{TOKEN}"),
            format!("/{TOKEN}/approved/extra"),
            "/".to_string(),
            String::new(),
        ] {
            assert_eq!(route(&path, Some(&ok_host()), TOKEN, PORT), Route::NotFound, "{path}");
        }
    }

    #[test]
    fn the_logo_needs_the_token_too() {
        assert_eq!(route(&format!("/{TOKEN}/owl.png"), Some(&ok_host()), TOKEN, PORT), Route::Owl);
        assert_eq!(route("/owl.png", Some(&ok_host()), TOKEN, PORT), Route::NotFound);
    }

    #[test]
    fn constant_time_compare_agrees_with_equality() {
        assert!(constant_time_eq(TOKEN, TOKEN));
        assert!(!constant_time_eq(TOKEN, &TOKEN[..31]));
        assert!(!constant_time_eq(TOKEN, &format!("{TOKEN}x")));
        let mut off_by_one = TOKEN.to_string();
        off_by_one.replace_range(0..1, "f");
        assert!(!constant_time_eq(TOKEN, &off_by_one));
    }

    // ── Response headers ──────────────────────────────────────────────────────

    #[test]
    fn every_response_closes_the_connection_and_forbids_caching() {
        for status in [200, 403, 404, 405, 431] {
            let h = response_head(status, "text/html; charset=utf-8", 3);
            assert!(h.starts_with(&format!("HTTP/1.1 {status} ")), "{h}");
            assert!(h.contains("Connection: close"), "{h}");
            assert!(h.contains("Cache-Control: no-store"), "{h}");
            assert!(h.contains("Content-Length: 3"), "{h}");
            assert!(h.ends_with("\r\n\r\n"), "{h}");
        }
    }

    /// Without `Access-Control-Allow-Origin` a cross-origin page cannot read the body even if it
    /// somehow reached the right path. There is no case in which this server wants to send one.
    #[test]
    fn no_cors_header_is_ever_sent() {
        for status in [200, 403, 404, 405, 431] {
            let h = response_head(status, "text/html", 0).to_lowercase();
            assert!(!h.contains("access-control-"), "{h}");
        }
    }

    #[test]
    fn the_csp_forbids_script_and_allows_only_our_own_images() {
        let h = response_head(200, "text/html", 0);
        assert!(h.contains("default-src 'none'"));
        assert!(h.contains("img-src 'self'"));
        assert!(!h.contains("script-src"), "nothing to allow: default-src none covers it");
        assert!(h.contains("Referrer-Policy: no-referrer"));
        assert!(h.contains("X-Content-Type-Options: nosniff"));
    }

    // ── The token ─────────────────────────────────────────────────────────────

    #[test]
    fn a_token_is_thirty_two_hex_characters_and_not_a_constant() {
        let a = new_token().expect("the platform should have a CSPRNG");
        let b = new_token().expect("the platform should have a CSPRNG");
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()), "{a}");
        assert_ne!(a, b, "a hardcoded token would pass every other test here");
    }

    // ── Over a real socket ────────────────────────────────────────────────────
    //
    // `read_head` and `respond` are the two functions the pure tests above cannot reach, and they are
    // where a wedged connection or a half-written response would live. One listener, one client, one
    // request each — no fixture server, no new dependency.

    /// Drives `handle` over a real loopback connection and returns the raw response.
    fn round_trip(request: &str) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("loopback should bind");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("a connection");
            handle(stream, TOKEN, port);
        });

        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let request = request.replace("{PORT}", &port.to_string());
        client.write_all(request.as_bytes()).expect("write");
        let mut response = String::new();
        client.read_to_string(&mut response).expect("read");
        server.join().expect("the handler should not panic");
        response
    }

    #[test]
    fn a_real_request_for_a_real_page_comes_back_as_html() {
        let response = round_trip(&format!(
            "GET /{TOKEN}/approved HTTP/1.1\r\nHost: githoot.localhost:{{PORT}}\r\n\r\n"
        ));
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("<!doctype html>"));
        // No poll has run in a test process, so the honest answer is "not known" — never a zero.
        assert!(response.contains("not known"));
    }

    #[test]
    fn a_real_request_with_a_hostile_host_is_refused_without_a_body_leak() {
        let response = round_trip(&format!(
            "GET /{TOKEN}/approved HTTP/1.1\r\nHost: evil.com\r\n\r\n"
        ));
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{response}");
        assert!(!response.contains("<!doctype html>"));
    }

    #[test]
    fn a_real_head_request_gets_the_headers_and_no_body() {
        let response = round_trip(&format!(
            "HEAD /{TOKEN}/approved HTTP/1.1\r\nHost: githoot.localhost:{{PORT}}\r\n\r\n"
        ));
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(!response.contains("<!doctype html>"), "a HEAD carries no body");
        assert!(response.contains("Content-Length: "), "but it still declares the length");
    }

    #[test]
    fn a_real_request_for_the_owl_comes_back_as_the_tray_png() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle(stream, TOKEN, port);
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .write_all(
                format!("GET /{TOKEN}/owl.png HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut raw = Vec::new();
        client.read_to_end(&mut raw).unwrap();
        server.join().unwrap();

        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("a head") + 4;
        let (head, body) = raw.split_at(split);
        assert!(String::from_utf8_lossy(head).contains("Content-Type: image/png"));
        assert_eq!(body, icons::TRAY_ICON, "the page wears the same owl as the tray");
    }

    /// A client that opens a connection and never finishes its head must not wedge the accept loop,
    /// which handles connections one at a time. The read timeout is the whole defence.
    #[test]
    fn a_client_that_never_finishes_its_head_is_dropped_rather_than_answered() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Far below `IO_TIMEOUT`, so the test does not sit here for five seconds; the point is
            // that the read ends rather than blocking for ever.
            stream.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
            read_head(&mut stream).expect_err("an unterminated head is not a request")
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.write_all(b"GET / HTTP/1.1\r\nHost: x").unwrap();
        assert_eq!(server.join().expect("no panic"), 400);
    }

    #[test]
    fn every_axis_round_trips_through_its_slug() {
        for axis in PrAxis::ALL {
            assert_eq!(PrAxis::from_slug(axis.slug()), Some(axis));
        }
        assert_eq!(PrAxis::from_slug("nope"), None);
    }
}

