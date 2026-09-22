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

/// How big a form body may be. A settings form is a few hundred bytes; the cap is what stops a
/// client claiming a gigabyte and making this thread allocate it.
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Method {
    Get,
    Head,
    Post,
}

/// What a parsed request line and its headers amount to.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub host: Option<String>,
    /// Who the browser says asked. Only writes consult it — see `origin_is_ours`.
    pub origin: Option<String>,
    /// Everything after `?`, which only the settings page reads — see `restart_names`.
    pub query: Option<String>,
    /// The version of the items payload the client already holds, if any.
    pub if_none_match: Option<String>,
    /// How many body bytes still have to be read. `None` for a request with no body.
    pub content_length: Option<usize>,
    /// `HEAD` wants the headers and no body.
    pub body_wanted: bool,
}

/// What the server should answer with.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Page(PrAxis),
    /// The refresh fragment behind a page: its age line and its list, as JSON.
    Items(PrAxis),
    /// The same judged list, as structured JSON, for a script running as you on this machine.
    ///
    /// Distinct from `Items` because that one's `items` field is rendered HTML and is contracted to
    /// the page's own five-second refresh. Served only while `localApi` is on; off, it answers
    /// `NotFound`, because a door you may not open should be absent rather than refused.
    Entries(PrAxis),
    /// The settings form.
    Settings,
    /// Applying a submitted settings form.
    SaveSettings,
    /// Starting a portal's sign-in from the settings page.
    Authenticate,
    Owl,
    /// The path exists but not for this method.
    MethodNotAllowed,
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

    let method = match method {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        _ => return Err(405),
    };
    let body_wanted = method != Method::Head;
    if !target.starts_with('/') {
        return Err(400);
    }

    let path = target.split(['?', '#']).next().unwrap_or(target).to_string();
    let query = target
        .split_once('?')
        .map(|(_, rest)| rest.split('#').next().unwrap_or(rest).to_string());

    let headers: Vec<(&str, &str)> =
        lines.filter_map(|line| line.split_once(':')).map(|(n, v)| (n, v.trim())).collect();
    let header = |want: &str| {
        headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(want))
            .map(|(_, value)| value.to_string())
    };

    let content_length = match header("content-length") {
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if n <= MAX_BODY_BYTES => Some(n),
            Ok(_) => return Err(413),
            Err(_) => return Err(400),
        },
        None => None,
    };

    Ok(Request {
        method,
        path,
        query,
        host: header("host"),
        origin: header("origin"),
        if_none_match: header("if-none-match"),
        content_length,
        body_wanted,
    })
}

/// Whether the browser says *this* page submitted the form.
///
/// The CSRF guard, and it is a different question from the `Host` allowlist. A form on another site
/// can make the browser `POST` here, and the browser fills `Host` in with *our* host, so that check
/// sees nothing wrong. `Origin` is the header that names who asked, and a cross-site form always
/// carries one. Absent counts as not ours: a same-origin form always sends it on a `POST`, so
/// nothing legitimate is refused by requiring it.
///
/// Reads only, which is every route but the save, need none — a cross-origin page cannot see the
/// response anyway, and demanding one would break opening the page from the tray.
fn origin_is_ours(origin: Option<&str>, port: u16) -> bool {
    let allowed =
        [format!("http://{PAGE_HOST}:{port}"), format!("http://127.0.0.1:{port}")];
    origin.is_some_and(|o| allowed.iter().any(|a| a == o))
}

/// One decoded `application/x-www-form-urlencoded` body.
///
/// Keeps every value for a name rather than the last, because a group of checkboxes posts its name
/// once per ticked box — which is exactly how the component list is rendered.
#[derive(Debug, Default)]
pub struct Form(Vec<(String, String)>);

impl Form {
    pub fn get(&self, name: &str) -> Option<&String> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub fn all(&self, name: &str) -> Vec<&str> {
        self.0.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect()
    }

    /// Whether a checkbox was ticked. An unticked box posts nothing at all, which is how a form says
    /// "off" — so absence is the answer, not an empty value.
    pub fn ticked(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

pub fn parse_form(body: &str) -> Form {
    Form(
        body.split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                (percent_decode(name), percent_decode(value))
            })
            .collect(),
    )
}

/// Decodes one form field: `+` is a space, `%XX` is a byte.
///
/// **This is the only percent-decoding in the module, and it is deliberately confined to bodies.**
/// Request *paths* are still compared raw, because the token is hex and the slugs are lowercase
/// ASCII, so decoding there would buy nothing and open a normalization-bug class. A form field is
/// different: it carries component names with spaces and commas in them, which have to survive.
///
/// An invalid or truncated escape is left as written rather than dropped. Losing a byte silently is
/// how a component name turns into one GitHub does not publish, which shows up only as one line in
/// the log.
pub fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
/// What to answer a request for `path` with, given which method asked.
pub fn route_for(
    path: &str,
    host: Option<&str>,
    token: &str,
    port: u16,
    method: Method,
) -> Route {
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
    let (Some(given), Some(leaf), tail) = (segments.next(), segments.next(), segments.next()) else {
        return Route::NotFound;
    };
    if segments.next().is_some() {
        return Route::NotFound;
    }
    if !constant_time_eq(given, token) {
        return Route::NotFound;
    }

    // The three-segment routes: a page's own refresh fragment, and the settings page's sign-in
    // button, both behind the same token and the same `Host` check as the page they belong to.
    if let Some(tail) = tail {
        if (leaf, tail) == ("settings", "authenticate") {
            return match method {
                Method::Post => Route::Authenticate,
                _ => Route::MethodNotAllowed,
            };
        }
        return match (PrAxis::from_slug(leaf), tail, method) {
            (Some(_), "items", Method::Post) => Route::MethodNotAllowed,
            (Some(axis), "items", _) => Route::Items(axis),
            // Whether `localApi` is on is not `route_for`'s business: it is a pure function of the
            // request, and threading config through it would put a setting in the way of every
            // routing test. `handle` answers `NotFound` for this variant when the setting is off.
            (Some(_), "entries", Method::Post) => Route::MethodNotAllowed,
            (Some(axis), "entries", _) => Route::Entries(axis),
            _ => Route::NotFound,
        };
    }

    match (leaf, method) {
        ("owl.png", Method::Post) => Route::MethodNotAllowed,
        ("owl.png", _) => Route::Owl,
        ("settings", Method::Post) => Route::SaveSettings,
        ("settings", _) => Route::Settings,
        // Everything else is a read, so a write to it is the wrong method rather than a miss — the
        // path is right and saying otherwise would be a lie in the status line.
        (slug, method) => match (PrAxis::from_slug(slug), method) {
            (Some(_), Method::Post) => Route::MethodNotAllowed,
            (Some(axis), _) => Route::Page(axis),
            (None, _) => Route::NotFound,
        },
    }
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
/// Which referrer policy a response carries, and it is not a free choice.
///
/// `no-referrer` does more than suppress `Referer`: per the Fetch standard, a non-CORS request whose
/// method is neither `GET` nor `HEAD` has its **`Origin` serialized as `null`** under that policy. So
/// a page served with `no-referrer` cannot tell us it submitted its own form, and `origin_is_ours`
/// refuses every write from it. That cost every save a `403` until it was noticed.
///
/// `SameOrigin` still sends nothing to another site — so the token is exactly as protected — while
/// keeping a real `Origin` on a request back to us. It is safe by construction rather than by luck:
/// if an outbound link is ever added to the settings page, the policy still withholds the referrer
/// from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Referrer {
    /// Nothing, ever. For pages that link out to github.com, and the safe default everywhere else.
    #[default]
    None,
    /// Only to ourselves. For pages that submit a form back to us.
    SameOrigin,
}

impl Referrer {
    pub fn header_value(self) -> &'static str {
        match self {
            Referrer::None => "no-referrer",
            Referrer::SameOrigin => "same-origin",
        }
    }
}

/// Whether `given` is the client's way of saying it already holds version `version`.
///
/// Lenient about quoting: a browser echoes the tag back verbatim, but a hand-rolled client may not
/// quote it, and a proxy may weaken it to `W/"n"`. All three mean the same version, and refusing the
/// last two would only cost a redundant body.
pub fn tag_matches(given: Option<&str>, version: u64) -> bool {
    let Some(given) = given else { return false };
    let bare = given.trim().trim_start_matches("W/").trim_matches('"');
    !bare.is_empty() && bare == version.to_string()
}

/// The answer when the client's copy is already current.
///
/// **No `Content-Length`.** A `304` carries no body by definition, and a client that believed a
/// length here would sit waiting for bytes that never arrive. The `ETag` is repeated so the client
/// can keep using the one it has.
pub fn not_modified_head(version: u64) -> String {
    format!(
        "HTTP/1.1 304 Not Modified\r\n\
         ETag: \"{version}\"\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         \r\n"
    )
}

/// Sends a `304` and closes.
///
/// Shared by the two conditional routes rather than written out in each, because a hand-copied
/// close sequence in two places is a close sequence that will differ in them eventually.
fn send_not_modified(stream: &mut TcpStream, version: u64) {
    let head = not_modified_head(version);
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// The parts of a response head that vary between routes.
///
/// A struct rather than three more parameters: `response_head` was up to eight, which is the point
/// at which a call site stops saying which `None` means what.
#[derive(Debug, Default, Clone, Copy)]
pub struct Head<'a> {
    pub referrer: Referrer,
    /// Names the one `<script>` this response carries, if it carries one.
    pub script_nonce: Option<&'a str>,
    /// The snapshot version this body renders, for the conditional request next time.
    pub etag: Option<u64>,
}

impl<'a> Head<'a> {
    #[cfg(test)]
    pub fn same_origin() -> Self {
        Head { referrer: Referrer::SameOrigin, ..Head::default() }
    }

    pub fn with_nonce(nonce: &'a str) -> Self {
        Head { script_nonce: Some(nonce), ..Head::default() }
    }

    pub fn tagged(version: u64) -> Self {
        Head { etag: Some(version), ..Head::default() }
    }
}

pub fn response_head(status: u16, content_type: &str, len: usize, head: Head) -> String {
    let Head { referrer, script_nonce, etag } = head;
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
         {}Connection: close\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: {}\r\n\
         Content-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; \
         connect-src 'self'; form-action 'self'; base-uri 'none'{}\r\n\
         \r\n",
        // No leading whitespace: a header line that starts with a space is an obs-fold continuation
        // of the one before it, not a header of its own.
        match etag {
            Some(version) => format!("ETag: \"{version}\"\r\n"),
            None => String::new(),
        },
        referrer.header_value(),
        // Named, not blanket-allowed. `'unsafe-inline'` would let anything injected into the page run
        // too; a nonce permits exactly the one `<script>` this response carries. Omitted entirely
        // where there is no script, so `default-src 'none'` keeps covering it.
        match script_nonce {
            Some(nonce) => format!("; script-src 'nonce-{nonce}'"),
            None => String::new(),
        }
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

/// What the settings page needs to read and change the configuration.
///
/// Installed once from `main`, which is where the switches and the wake channel are built. The
/// `Mutex` is around the `Sender` alone: an `mpsc::Sender` is `Send` but not `Sync`, and both
/// listener threads may want to pull a poll forward after a save.
pub struct Settings {
    pub app_asset_path: std::path::PathBuf,
    pub sound: crate::config::Switch,
    pub copilot: crate::config::Switch,
    /// Whether `Route::Entries` is served and the listener binds at boot. A plain `bool`, not a
    /// `Switch`, because the bind happens once at startup: a control that claimed to take effect
    /// mid-run would be exactly the broken switch `config::Switch` exists to prevent.
    pub local_api: bool,
    pub wake: std::sync::Mutex<std::sync::mpsc::Sender<scheduler::Wake>>,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// Whether the machine-readable route is open.
///
/// Its own function rather than an inline read so the off path can be tested without installing
/// `SETTINGS`, which is a `OnceLock` and therefore settable only once per test binary.
fn entries_enabled() -> bool {
    SETTINGS.get().is_some_and(|s| s.local_api)
}

/// Hands the server what it needs to serve and save settings. Called once, from `main`.
///
/// Separate from `start` because the listener binds lazily on a menu click, while these exist from
/// boot — and because a settings page that silently could not save would be worse than none.
pub fn install(settings: Settings) {
    // Before anything can bind, and whatever the setting says: a file naming a port this process
    // does not hold is worse than no file at all.
    clear_endpoint_file(&settings.app_asset_path);
    let _ = SETTINGS.set(settings);
}

/// A bound listener: the port the OS gave us and the token that guards it.
struct Server {
    port: u16,
    token: String,
}

/// Bound on the first menu click, never at boot, and never retried once it has failed.
static SERVER: OnceLock<Option<Server>> = OnceLock::new();

/// Where a local script finds the ephemeral port and this run's token.
///
/// Beside `config.txt` and `pr_token.txt`, and with `pr_token.txt`'s permissions, because it holds a
/// credential of the same shape if not the same value.
const ENDPOINT_FILE: &str = "endpoint.json";

fn endpoint_path(app_asset_path: &std::path::Path) -> std::path::PathBuf {
    app_asset_path.join(ENDPOINT_FILE)
}

/// Removes any endpoint file left behind. Nothing there is not an error: the outcome is the same.
///
/// Called unconditionally from `install`, which both platforms reach before a menu click is
/// possible. Unconditional because the two cases that matter are a predecessor that crashed without
/// one (there is no shutdown path, by design — see `start`) and a user who has just turned the
/// setting off, and in both the right answer is that no address is published.
fn clear_endpoint_file(app_asset_path: &std::path::Path) {
    match std::fs::remove_file(endpoint_path(app_asset_path)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            errorln!("could not remove a stale {ENDPOINT_FILE}: {e}");
        }
        _ => {}
    }
}

/// Publishes the address of the bound listener for a local script.
///
/// **This file is a hint, never a fact.** It outlives a crash, so a reader has to connect and treat
/// a refused connection or a 404 as "GitHoot is not running" rather than acting on the contents.
/// `pid` is advisory for the same reason: pids get reused.
///
/// The URLs name `127.0.0.1` rather than `githoot.localhost`. Browsers resolve every `.localhost`
/// subdomain to loopback themselves; curl and most script HTTP clients do not, and both literals are
/// in the `Host` allowlist, so publishing the numeric one is what makes the URL work with no
/// `--resolve` and no `Host` override.
fn write_endpoint_file(app_asset_path: &std::path::Path, port: u16, token: &str) {
    let host_header = format!("127.0.0.1:{port}");
    let base_url = format!("http://{host_header}/{token}");
    let content = serde_json::json!({
        "schema": 1,
        "pid": std::process::id(),
        "port": port,
        "token": token,
        "host_header": host_header,
        "base_url": base_url,
        // From `PrAxis::ALL`, so a bar cannot exist on the page and be missing here.
        "axes": PrAxis::ALL
            .iter()
            .map(|axis| {
                (axis.slug().to_string(), serde_json::json!(format!("{base_url}/{}/entries", axis.slug())))
            })
            .collect::<serde_json::Map<String, serde_json::Value>>(),
    })
    .to_string();

    let path = endpoint_path(app_asset_path);

    // Owner-only, the same way and for the same reason as `save_credential` in
    // `portal::github::auth`. Knowingly duplicated rather than shared: unifying them means editing
    // the credential writer, and a security-sensitive rewrite does not belong in the same change as
    // an additive feature.
    #[cfg(unix)]
    let written = {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let result = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| file.write_all(content.as_bytes()));

        if result.is_ok() {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        result
    };

    #[cfg(not(unix))]
    let written = std::fs::write(&path, &content);

    // Not fatal. The listener is up and the page works; only a script is left without an address,
    // and it already has to cope with the file being absent between an update's exec and the
    // successor's bind.
    if let Err(e) = written {
        errorln!("could not write {ENDPOINT_FILE}: {e}");
    }
}

/// Binds the listener now rather than on the first menu click. `false` when it could not bind.
///
/// Only for `localApi`: a script has no menu to click, so without this the port a dispatcher is told
/// to read would never open. It is opt-in and off by default because it reverses every argument in
/// `start`'s own doc comment — a socket held open for days for someone who never clicks, a firewall
/// or EDR reaction with no click to explain it, and this binary's history with Defender's ML
/// heuristics. Read that comment before considering a different default.
pub fn start_now() -> bool {
    SERVER.get_or_init(start).is_some()
}

/// Opens `axis`'s GitHoot page, or GitHub's own search page when there is no server to serve one.
///
/// One helper rather than two copies, because the menu is built twice — GTK on Linux, `tray-icon` on
/// Windows and macOS — and a behaviour that lives in both copies is a behaviour that will differ in
/// them eventually.
pub fn open_axis_page(axis: PrAxis) {
    // The PR inbox when there is no listener: GitHub's own search cannot express what two of the
    // three bars count, and the URL that tried to say otherwise did not work either.
    open_url(url_for(axis).unwrap_or_else(scheduler::inbox_url));
}

/// Opens the settings page, or falls back to the settings *file* when there is no listener.
///
/// The fallback is the old behaviour rather than nothing: a browser page that cannot be served is no
/// reason to lose the only way into the configuration.
pub fn open_settings_page() -> bool {
    match SERVER.get_or_init(start).as_ref() {
        Some(server) => {
            open_url(format!("http://{PAGE_HOST}:{}/{}/settings", server.port, server.token));
            true
        }
        None => false,
    }
}

/// Whether the listener is up, so a sign-in knows the settings page can show its code. `false`
/// before the first click as well as after a failed bind; both mean the page is not where the code
/// should go.
pub fn is_available() -> bool {
    SERVER.get().is_some_and(Option::is_some)
}

fn open_url(url: String) {
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

    // Only when asked. A published address for a door that answers 404 would be a standing
    // invitation to debug the wrong thing.
    if let Some(settings) = SETTINGS.get()
        && settings.local_api
    {
        write_endpoint_file(&settings.app_asset_path, port, &token);
        infoln!("localApi is on: wrote {ENDPOINT_FILE} for local scripts");
    }

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

    match route_for(&request.path, request.host.as_deref(), token, port, request.method) {
        Route::Settings => {
            // A fresh nonce per response, as the PR page does; failing to get one drops the copy
            // button's script rather than widening the policy.
            let nonce = new_token().unwrap_or_default();
            let html = match SETTINGS.get() {
                Some(_) => {
                    let (cfg, _) = crate::config::Config::load(&settings_path());
                    let query = request.query.as_deref();
                    page::settings_page(
                        &cfg,
                        token,
                        &restart_names(query),
                        &page::PortalsView {
                            portals: &scheduler::portal_statuses(),
                            signin_started: query_value(query, "signin").as_deref(),
                            signed_out: query_value(query, "signout").as_deref(),
                            now_unix: unix_now(),
                            nonce: &nonce,
                        },
                    )
                }
                None => page::settings_unavailable(token),
            };
            // `same-origin` for the form's `Origin`, plus the nonce for the copy button's script,
            // which the page emits only while a device code is on screen.
            respond_as(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                html.as_bytes(),
                request.body_wanted,
                Head {
                    referrer: Referrer::SameOrigin,
                    script_nonce: (!nonce.is_empty()).then_some(nonce.as_str()),
                    etag: None,
                },
            )
        }
        Route::SaveSettings => save_settings(&mut stream, &head, &request, token, port),
        Route::Authenticate => start_sign_in(&mut stream, &head, &request, token, port),
        Route::MethodNotAllowed => {
            respond(&mut stream, 405, "text/plain; charset=utf-8", b"Method not allowed", request.body_wanted)
        }
        Route::Owl => {
            respond(&mut stream, 200, "image/png", icons::TRAY_ICON, request.body_wanted)
        }
        Route::Items(axis) => {
            let snapshot = scheduler::pr_snapshot(axis);
            let version = snapshot.version;
            // The client already holds this poll's answer, so there is nothing to send. Its age line
            // keeps ticking on its own — see `page::REFRESH_SCRIPT` for why that is what makes a
            // genuine `304` correct here rather than a lie about freshness.
            if tag_matches(request.if_none_match.as_deref(), version) {
                return send_not_modified(&mut stream, version);
            }
            let json = page::items_json(&page::groups(&snapshot.groups), snapshot.polled_at, unix_now());
            respond_as(
                &mut stream,
                200,
                "application/json; charset=utf-8",
                json.as_bytes(),
                request.body_wanted,
                Head::tagged(version),
            )
        }
        Route::Entries(axis) => {
            // Off means off. A URL that does not exist today must not start existing merely because
            // the user once clicked a menu entry and bound the listener for the page's sake.
            if !entries_enabled() {
                return respond(&mut stream, 404, "text/plain; charset=utf-8", b"Not found", request.body_wanted);
            }
            let snapshot = scheduler::pr_snapshot(axis);
            let version = snapshot.version;
            // The same `version` as `Items` carries, which is correct rather than a collision: an
            // `ETag` is scoped to its own URL, and both resources change exactly when the poll loop
            // republishes the snapshot.
            if tag_matches(request.if_none_match.as_deref(), version) {
                return send_not_modified(&mut stream, version);
            }
            let json = crate::api::entries_json(axis, &snapshot, unix_now());
            respond_as(
                &mut stream,
                200,
                "application/json; charset=utf-8",
                json.as_bytes(),
                request.body_wanted,
                Head::tagged(version),
            )
        }
        Route::Page(axis) => {
            let snapshot = scheduler::pr_snapshot(axis);
            // A fresh nonce per response, which is what a nonce is for: it names *this* page's script
            // in *this* response's CSP. Failing to get one drops the script rather than widening the
            // policy — the page still works, it just stops refreshing itself.
            let nonce = new_token().unwrap_or_default();
            let html = page::axis_page(
                axis,
                &page::groups(&snapshot.groups),
                snapshot.polled_at,
                token,
                unix_now(),
                &nonce,
            );
            respond_as(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                html.as_bytes(),
                request.body_wanted,
                if nonce.is_empty() { Head::default() } else { Head::with_nonce(&nonce) },
            )
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
/// A `303 See Other` back to the settings page.
///
/// Post/redirect/get, so reloading after a save does not offer to submit the form again — which on
/// this page would silently rewrite settings the user has since changed in the tray.
fn redirect(stream: &mut TcpStream, location: &str) {
    let head = format!(
        "HTTP/1.1 303 See Other\r\n\
         Location: {location}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: same-origin\r\n\
         \r\n"
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8], with_body: bool) {
    respond_as(stream, status, content_type, body, with_body, Head::default())
}

fn respond_as(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    with_body: bool,
    head: Head,
) {
    let head = response_head(status, content_type, body.len(), head);
    let _ = stream.write_all(head.as_bytes());
    if with_body {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// The keys a just-finished save needs a restart for, read back off the redirect.
///
/// **Filtered against the real key list**, so the banner can only ever name settings that exist. It
/// is text the request supplies and the page echoes, and while `esc` already makes it inert, a page
/// that will print whatever it is handed is a thing to fix rather than to escape.
fn restart_names(query: Option<&str>) -> Vec<&'static str> {
    let Some(query) = query else { return Vec::new() };
    let Some((_, list)) = query.split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == "restart")
    else {
        return Vec::new();
    };
    percent_decode(list)
        .split(',')
        .filter_map(|name| {
            crate::config::WRITABLE_KEYS
                .iter()
                .find(|(key, live)| *key == name && !*live)
                .map(|(key, _)| *key)
        })
        .collect()
}

/// Where `config.txt` lives, for the settings page.
fn settings_path() -> std::path::PathBuf {
    SETTINGS.get().map(|s| s.app_asset_path.clone()).unwrap_or_default()
}

/// Applies a submitted settings form.
///
/// The order is the same one the tray checkboxes use and for the same reason: the live switches are
/// set first so the next poll obeys the new answer whatever the disk does, and the file is written
/// second because it only decides what the *next* start believes.
fn save_settings(stream: &mut TcpStream, head: &str, request: &Request, token: &str, port: u16) {
    if !origin_is_ours(request.origin.as_deref(), port) {
        return respond(stream, 403, "text/plain; charset=utf-8", b"Forbidden", true);
    }
    let Some(settings) = SETTINGS.get() else {
        return respond(stream, 503, "text/plain; charset=utf-8", b"Settings unavailable", true);
    };

    // The head read stops at the blank line, so whatever followed it is the start of the body.
    let already = head.split_once("\r\n\r\n").map(|(_, rest)| rest).unwrap_or_default();
    let body = match read_body(stream, already, request.content_length.unwrap_or(0)) {
        Ok(body) => body,
        Err(status) => return respond(stream, status, "text/plain; charset=utf-8", b"", true),
    };

    let (current, _) = crate::config::Config::load(&settings.app_asset_path);
    let wanted = crate::config::Config::from_form(&parse_form(&body), &current);

    settings.sound.set(wanted.sound);
    settings.copilot.set(wanted.copilot_reviews);

    match crate::config::save(&settings.app_asset_path, &wanted) {
        Ok(changed) => {
            if !changed.is_empty() {
                infoln!("settings page wrote: {}", changed.join(", "));
                // A changed rule with a stale count on screen reads as the save not having worked.
                if let Ok(wake) = settings.wake.lock() {
                    let _ = wake.send(scheduler::Wake::Refresh);
                }
            }
            let restarts: Vec<&str> =
                changed.iter().copied().filter(|key| !crate::config::is_live(key)).collect();
            redirect(stream, &format!("/{token}/settings?restart={}", restarts.join(",")));
        }
        Err(e) => {
            errorln!("settings page could not save: {e}");
            respond(stream, 500, "text/plain; charset=utf-8", b"Could not write config.txt", true)
        }
    }
}

/// The settings page's sign-in button. Same `Origin` guard as a save, for the same reason: a
/// cross-site form must not be able to pop a device code dialog on someone's desktop.
///
/// Only *asks*: the flow runs on the poll thread, where the credential lives and where blocking for
/// as long as the user takes costs nothing but a paused poll. The redirect back names the portal so
/// the page can say the sign-in has started even before the poll thread has published "in progress".
fn start_sign_in(stream: &mut TcpStream, head: &str, request: &Request, token: &str, port: u16) {
    if !origin_is_ours(request.origin.as_deref(), port) {
        return respond(stream, 403, "text/plain; charset=utf-8", b"Forbidden", true);
    }
    let Some(settings) = SETTINGS.get() else {
        return respond(stream, 503, "text/plain; charset=utf-8", b"Settings unavailable", true);
    };
    let already = head.split_once("\r\n\r\n").map(|(_, rest)| rest).unwrap_or_default();
    let body = match read_body(stream, already, request.content_length.unwrap_or(0)) {
        Ok(body) => body,
        Err(status) => return respond(stream, status, "text/plain; charset=utf-8", b"", true),
    };
    let form = parse_form(&body);
    // Only a portal the poll loop knows. Anything else is a stale or hand-made post, and answering
    // 404 rather than starting a flow for "whatever is waiting" keeps the button meaning one thing.
    let Some(status) = form
        .get("portal")
        .and_then(|id| {
            let id: &str = id.as_ref();
            scheduler::portal_statuses().into_iter().find(|p| p.info.id.0 == id)
        })
    else {
        return respond(stream, 404, "text/plain; charset=utf-8", b"Unknown portal", true);
    };
    // The same form, with `cancel` set, is the Cancel button beside a running sign-in. It reaches
    // the flow through a flag rather than a wake, because the poll thread is inside the flow and
    // reads no channel until it returns.
    // `signout` deletes the saved credential. Handed to the poll thread like everything else that
    // touches a credential; the redirect names the portal so the page can say so at once.
    if form.ticked("signout") {
        infoln!("settings page asked to sign out of {}", status.info.display_name);
        if let Ok(wake) = settings.wake.lock() {
            let _ = wake.send(scheduler::Wake::SignOut(status.info.id.clone()));
        }
        return redirect(stream, &format!("/{token}/settings?signout={}", status.info.id.0));
    }
    if form.ticked("cancel") {
        if scheduler::cancel_sign_in(&status.info.id) {
            infoln!("settings page cancelled the {} sign-in", status.info.display_name);
        }
        return redirect(stream, &format!("/{token}/settings"));
    }
    infoln!("settings page asked to sign in to {}", status.info.display_name);
    if let Ok(wake) = settings.wake.lock() {
        let _ = wake.send(scheduler::Wake::Authenticate(Some(status.info.id.clone())));
    }
    redirect(stream, &format!("/{token}/settings?signin={}", status.info.id.0));
}

/// One query parameter, percent-decoded. `None` when absent.
fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| percent_decode(v))
}

/// Reads the rest of the form body, given whatever arrived alongside the head.
fn read_body(stream: &mut TcpStream, already: &str, length: usize) -> Result<String, u16> {
    if length > MAX_BODY_BYTES {
        return Err(413);
    }
    let mut body = already.as_bytes().to_vec();
    body.truncate(length.min(body.len()));
    let mut chunk = [0u8; 512];
    while body.len() < length {
        let read = stream.read(&mut chunk).map_err(|_| 400u16)?;
        if read == 0 {
            return Err(400);
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    String::from_utf8(body).map_err(|_| 400u16)
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

    /// `route_for` as a plain GET, which is what almost every test here means.
    fn route_get(path: &str, host: Option<&str>, token: &str, port: u16) -> Route {
        route_for(path, host, token, port, Method::Get)
    }

    fn test_config() -> crate::config::Config {
        let dir = std::env::temp_dir().join(format!("githoot-serve-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (cfg, _) = crate::config::Config::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        cfg
    }

    fn route_write(path: &str, method: Method) -> Route {
        route_for(path, Some(&ok_host()), TOKEN, PORT, method)
    }

    // ── Request parsing ───────────────────────────────────────────────────────

    // ── Writes: POST, bodies and the Origin guard ─────────────────────────────

    #[test]
    fn a_post_to_the_settings_route_is_parsed() {
        let raw = format!(
            "POST /{TOKEN}/settings HTTP/1.1\r\nHost: {}\r\nContent-Length: 7\r\n\r\nsound=on",
            ok_host()
        );
        let r = parse_request(&raw).expect("should parse");
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.content_length, Some(7));
    }

    /// The sign-in button posts to its own route under the settings page, and only a POST is a
    /// sign-in: a GET there is the wrong method, never a flow started by following a link.
    #[test]
    fn the_sign_in_button_has_a_post_only_route() {
        assert_eq!(route_write(&format!("/{TOKEN}/settings/authenticate"), Method::Post), Route::Authenticate);
        assert_eq!(route_write(&format!("/{TOKEN}/settings/authenticate"), Method::Get), Route::MethodNotAllowed);
        assert_eq!(route_write(&format!("/{TOKEN}/settings/other"), Method::Post), Route::NotFound);
        assert_eq!(route_get(&format!("/{TOKEN}/settings/authenticate"), Some(&ok_host()), "wrong", PORT), Route::NotFound);
    }

    #[test]
    fn a_query_parameter_is_read_and_decoded() {
        assert_eq!(query_value(Some("restart=a,b&signin=git%20hub"), "signin").as_deref(), Some("git hub"));
        assert_eq!(query_value(Some("restart=a"), "signin"), None);
        assert_eq!(query_value(None, "signin"), None);
    }

    /// Only the settings routes accept one. Everything else is a read.
    #[test]
    fn a_post_anywhere_else_is_rejected() {
        assert_eq!(
            route_write(&format!("/{TOKEN}/approved"), Method::Post),
            Route::MethodNotAllowed
        );
    }

    // ── The localApi entries route ────────────────────────────────────────────

    /// Every axis the page can render must also be readable as JSON, or a script and the page would
    /// disagree about which bars exist. Built from `PrAxis::ALL` so a fourth axis cannot be added to
    /// one and forgotten on the other.
    #[test]
    fn the_entries_route_is_parsed_for_every_axis() {
        for axis in PrAxis::ALL {
            let path = format!("/{TOKEN}/{}/entries", axis.slug());
            assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::Entries(axis));
        }
    }

    /// A wrong token must not get a different answer here than it gets anywhere else. `Forbidden`
    /// would confirm that everything after the token was right, which is precisely what the page's
    /// own routes refuse to do.
    #[test]
    fn the_entries_route_with_the_wrong_token_is_a_miss_not_a_refusal() {
        let path = format!("/{TOKEN}/approved/entries");
        assert_eq!(route_get(&path, Some(&ok_host()), "wrong", PORT), Route::NotFound);
    }

    /// DNS rebinding. A hostile page that gets the browser to dial this port still cannot name us,
    /// and the `Host` check runs before the path is looked at, so a rejected host never reveals
    /// whether the route exists.
    #[test]
    fn the_entries_route_refuses_a_host_that_is_not_ours() {
        let path = format!("/{TOKEN}/approved/entries");
        for host in [
            Some("evil.com"),
            Some(&format!("localhost:{PORT}")[..]),
            Some(&format!("[::1]:{PORT}")[..]),
            Some(&format!("{PAGE_HOST}:{}", PORT + 1)[..]),
            None,
        ] {
            assert_eq!(route_get(&path, host, TOKEN, PORT), Route::Forbidden, "{host:?} must not reach entries");
        }
    }

    /// The whole design rests on this route never remembering anything. A `POST` here would be the
    /// first step towards a claim endpoint, so it is the wrong method rather than a miss: the path
    /// is real and saying otherwise would be a lie in the status line.
    #[test]
    fn a_post_to_the_entries_route_is_the_wrong_method_not_a_miss() {
        assert_eq!(route_write(&format!("/{TOKEN}/approved/entries"), Method::Post), Route::MethodNotAllowed);
    }

    /// A polling dispatcher should be able to check the `ETag` without pulling the list, so `HEAD`
    /// has to route exactly as `GET` does.
    #[test]
    fn a_head_of_the_entries_route_is_allowed() {
        assert_eq!(route_write(&format!("/{TOKEN}/approved/entries"), Method::Head), Route::Entries(PrAxis::ReadyToMerge));
    }

    /// A near miss under a real axis must not fall through to the page's own refresh fragment, or a
    /// typo would silently hand a script rendered HTML instead of data.
    #[test]
    fn an_unknown_tail_under_an_axis_is_still_a_miss() {
        for tail in ["entriez", "entries2", ""] {
            let path = format!("/{TOKEN}/approved/{tail}");
            assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::NotFound, "tail {tail:?}");
        }
    }

    /// The CSRF guard. A form on another site can make the browser send a cross-origin `POST` — the
    /// `Host` allowlist cannot see that, because the browser puts *our* host in it. `Origin` is what
    /// names who asked.
    #[test]
    fn a_post_from_another_origin_is_forbidden() {
        for origin in [
            Some("https://evil.com"),
            Some("http://githoot.localhost"),
            Some(&format!("http://githoot.localhost:{}", PORT + 1)[..]),
            None,
        ] {
            assert!(
                !origin_is_ours(origin, PORT),
                "{origin:?} must not be able to write settings"
            );
        }
        assert!(origin_is_ours(Some(&format!("http://githoot.localhost:{PORT}")), PORT));
        assert!(origin_is_ours(Some(&format!("http://127.0.0.1:{PORT}")), PORT));
    }

    /// A read needs no `Origin`: a cross-origin page cannot see the response anyway, and requiring
    /// one would break opening the page from the tray, where the browser sends none.
    #[test]
    fn a_get_needs_no_origin() {
        assert_eq!(route_get(&format!("/{TOKEN}/approved"), Some(&ok_host()), TOKEN, PORT), Route::Page(PrAxis::ReadyToMerge));
    }

    /// The banner names settings, and only settings that exist. It is text the request hands us and
    /// the page prints back.
    #[test]
    fn the_restart_banner_only_ever_names_real_settings() {
        assert_eq!(restart_names(Some("restart=logLevel,statusComponents")), ["logLevel", "statusComponents"]);
        assert!(restart_names(Some("restart=<script>alert(1)</script>")).is_empty());
        assert!(restart_names(Some("restart=nonsense,logLevel")).len() == 1);
        // The two live settings need no restart, so they are never named as needing one.
        assert!(restart_names(Some("restart=sound,copilotReviews")).is_empty());
        assert!(restart_names(Some("")).is_empty());
        assert!(restart_names(None).is_empty());
    }

    #[test]
    fn a_query_string_is_kept_for_the_settings_page() {
        let raw = format!("GET /{TOKEN}/settings?restart=logLevel HTTP/1.1\r\nHost: h\r\n\r\n");
        let r = parse_request(&raw).expect("parses");
        assert_eq!(r.path, format!("/{TOKEN}/settings"));
        assert_eq!(r.query.as_deref(), Some("restart=logLevel"));
    }

    #[test]
    fn form_pairs_are_decoded() {
        let form = parse_form("sound=on&logLevel=info&statusComponents=Git+Operations%2C+Issues");
        assert_eq!(form.get("sound").map(String::as_str), Some("on"));
        assert_eq!(form.get("logLevel").map(String::as_str), Some("info"));
        assert_eq!(
            form.get("statusComponents").map(String::as_str),
            Some("Git Operations, Issues")
        );
    }

    /// Checkboxes post one name repeatedly, once per ticked box.
    #[test]
    fn repeated_names_are_all_kept() {
        let form = parse_form("component=Issues&component=Actions&sound=on");
        assert_eq!(form.all("component"), vec!["Issues", "Actions"]);
        assert!(form.all("missing").is_empty());
    }

    /// An unticked checkbox posts nothing at all, which is how a form says "off".
    #[test]
    fn a_name_that_was_not_posted_is_absent_not_empty() {
        let form = parse_form("sound=on");
        assert_eq!(form.get("updateCheck"), None);
    }

    #[test]
    fn percent_decoding_survives_the_awkward_cases() {
        assert_eq!(percent_decode("a%2Bb"), "a+b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // A truncated or invalid escape is left alone rather than dropping the byte.
        assert_eq!(percent_decode("a%"), "a%");
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    /// The body is capped like the head is, so a client cannot grow this thread's memory.
    #[test]
    fn an_oversized_body_is_refused() {
        let raw = format!(
            "POST /{TOKEN}/settings HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\n\r\n",
            ok_host(),
            MAX_BODY_BYTES + 1
        );
        assert_eq!(parse_request(&raw), Err(413));
    }

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

    /// GET, HEAD and POST are the whole vocabulary. POST joined when the settings form did, and
    /// `route_for` is what keeps it to the one path that accepts a write.
    #[test]
    fn anything_but_get_head_or_post_is_rejected() {
        for method in ["PUT", "DELETE", "OPTIONS", "TRACE", "PATCH"] {
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
            assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::Page(axis));
        }
    }

    #[test]
    fn the_numeric_host_is_accepted_too() {
        let host = format!("127.0.0.1:{PORT}");
        let path = format!("/{TOKEN}/approved");
        assert_eq!(route_get(&path, Some(&host), TOKEN, PORT), Route::Page(PrAxis::ReadyToMerge));
    }

    /// A wrong token answers 404, not 403, so the response does not confirm the path shape was right.
    #[test]
    fn a_wrong_token_is_not_found_rather_than_forbidden() {
        let path = format!("/{}/approved", "f".repeat(32));
        assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::NotFound);
    }

    #[test]
    fn a_token_of_the_right_length_with_one_wrong_byte_is_rejected() {
        let mut wrong = TOKEN.to_string();
        wrong.replace_range(31..32, "0");
        assert_eq!(route_get(&format!("/{wrong}/approved"), Some(&ok_host()), TOKEN, PORT), Route::NotFound);
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
            assert_eq!(route_get(&path, Some(host), TOKEN, PORT), Route::Forbidden, "{host}");
        }
    }

    #[test]
    fn a_request_with_no_host_is_forbidden() {
        assert_eq!(route_get(&format!("/{TOKEN}/approved"), None, TOKEN, PORT), Route::Forbidden);
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
            assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::NotFound, "{path}");
        }
    }

    #[test]
    fn the_logo_needs_the_token_too() {
        assert_eq!(route_get(&format!("/{TOKEN}/owl.png"), Some(&ok_host()), TOKEN, PORT), Route::Owl);
        assert_eq!(route_get("/owl.png", Some(&ok_host()), TOKEN, PORT), Route::NotFound);
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
            let h = response_head(status, "text/html; charset=utf-8", 3, Head::default());
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
            let h = response_head(status, "text/html", 0, Head::default()).to_lowercase();
            assert!(!h.contains("access-control-"), "{h}");
        }
    }

    /// **The bug that made every save return 403.**
    ///
    /// Per the Fetch standard, a non-CORS request whose method is not `GET` or `HEAD` has its `Origin`
    /// serialized as **`null`** when the referrer policy is `no-referrer`. So the header added to stop
    /// the token reaching github.com in a `Referer` was also stopping the browser from telling us who
    /// submitted the form — and `origin_is_ours` refused every save.
    ///
    /// The settings routes use `same-origin` instead, which still sends nothing cross-origin (so the
    /// token is as protected as before) but keeps a real `Origin` on our own form. The PR pages, which
    /// really do link out to github.com, keep `no-referrer`.
    #[test]
    fn the_settings_page_keeps_an_origin_the_browser_will_send() {
        let settings = response_head(200, "text/html", 0, Head::same_origin());
        assert!(settings.contains("Referrer-Policy: same-origin"), "{settings}");
        assert!(!settings.contains("no-referrer"), "no-referrer nulls the Origin on a POST");

        let pr_page = response_head(200, "text/html", 0, Head::default());
        assert!(pr_page.contains("Referrer-Policy: no-referrer"), "{pr_page}");
    }

    /// And the same for the document-level policy, which is what actually governs the form the page
    /// carries — a `<meta name="referrer" content="no-referrer">` nulls the `Origin` exactly as the
    /// header does.
    #[test]
    fn the_settings_document_declares_the_same_policy_as_its_response() {
        let html = page::settings_page(&test_config(), "tok", &[], &page::PortalsView { portals: &[], signin_started: None, signed_out: None, now_unix: 0, nonce: "n" });
        assert!(html.contains(r#"<meta name="referrer" content="same-origin">"#), "got {html}");
        assert!(!html.contains("no-referrer"));
    }

    /// The refresh fragment, behind the same token and `Host` check as the page it belongs to.
    #[test]
    fn a_page_can_fetch_its_own_items() {
        for axis in PrAxis::ALL {
            let path = format!("/{TOKEN}/{}/items", axis.slug());
            assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::Items(axis));
        }
    }

    #[test]
    fn the_items_route_is_a_read_only() {
        assert_eq!(route_write(&format!("/{TOKEN}/approved/items"), Method::Post), Route::MethodNotAllowed);
    }

    /// Three segments is the *only* extra shape allowed, and only for `items`. A fourth, or any other
    /// tail, is not a path this server has.
    #[test]
    fn no_other_deep_path_resolves() {
        for path in [
            format!("/{TOKEN}/approved/nope"),
            format!("/{TOKEN}/approved/items/more"),
            format!("/{TOKEN}/settings/items"),
            format!("/{TOKEN}/owl.png/items"),
        ] {
            assert_eq!(route_get(&path, Some(&ok_host()), TOKEN, PORT), Route::NotFound, "{path}");
        }
    }

    /// A payload nobody has seen yet is served in full, with its version as the `ETag`.
    #[test]
    fn a_first_fetch_of_the_items_gets_the_body_and_a_tag() {
        let head = response_head(200, "application/json", 9, Head::tagged(7));
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(head.contains("ETag: \"7\""), "{head}");
        assert!(head.contains("Content-Length: 9"));
    }

    /// And one the client already holds is answered with nothing at all.
    ///
    /// A `304` carries no body by definition, so it must not claim a length either — a client that
    /// believed a `Content-Length` here would sit waiting for bytes that never come.
    #[test]
    fn an_unchanged_payload_is_answered_with_304_and_no_body() {
        let head = not_modified_head(7);
        assert!(head.starts_with("HTTP/1.1 304 Not Modified"), "{head}");
        assert!(head.contains("ETag: \"7\""), "{head}");
        assert!(!head.contains("Content-Length"), "a 304 has no body to measure: {head}");
        assert!(head.contains("Connection: close"));
        assert!(head.ends_with("\r\n\r\n"));
    }

    #[test]
    fn the_conditional_header_is_read() {
        let raw = format!(
            "GET /{TOKEN}/approved/items HTTP/1.1\r\nHost: h\r\nIf-None-Match: \"12\"\r\n\r\n"
        );
        assert_eq!(parse_request(&raw).expect("parses").if_none_match.as_deref(), Some("\"12\""));
    }

    /// The tag the client sends back has to be recognised whatever the quoting, and a weak validator
    /// is still the same version.
    #[test]
    fn a_tag_matches_its_version_however_it_is_quoted() {
        for given in ["\"12\"", "12", "W/\"12\""] {
            assert!(tag_matches(Some(given), 12), "{given}");
        }
        for given in ["\"13\"", "", "\"\"", "*"] {
            assert!(!tag_matches(Some(given), 12), "{given}");
        }
        assert!(!tag_matches(None, 12), "no tag is not a match");
    }

    /// The script is named by nonce, not allowed by `'unsafe-inline'`: only the exact `<script>` this
    /// response carries may run, and anything injected into the page cannot borrow the permission.
    #[test]
    fn a_script_is_allowed_only_by_its_nonce() {
        let with = response_head(200, "text/html", 0, Head::with_nonce("cafebabe"));
        assert!(with.contains("script-src 'nonce-cafebabe'"), "{with}");
        assert!(!with.contains("unsafe-inline'; script"), "never blanket-inline for scripts");
        // The refresh has to be able to fetch, and `default-src 'none'` would block it.
        assert!(with.contains("connect-src 'self'"), "{with}");

        // Everything else still runs nothing at all.
        let without = response_head(200, "application/json", 0, Head::default());
        assert!(!without.contains("script-src"), "{without}");
        assert!(without.contains("default-src 'none'"));
    }

    #[test]
    fn the_csp_forbids_script_and_allows_only_our_own_images() {
        let h = response_head(200, "text/html", 0, Head::default());
        assert!(h.contains("default-src 'none'"));
        assert!(h.contains("img-src 'self'"));
        // Opened for the settings form, and no wider: `'self'` is this origin, so a form on the page
        // can post back here and nowhere else.
        assert!(h.contains("form-action 'self'"));
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

    // ── The endpoint file ─────────────────────────────────────────────────────

    fn endpoint_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-endpoint-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// It holds this run's token. World-readable, it would hand every other account on the machine
    /// the key to the loopback port, which is the one thing the per-run token was supposed to bound.
    /// Same treatment, and the same reasoning, as `pr_token.txt`.
    #[cfg(unix)]
    #[test]
    fn the_endpoint_file_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = endpoint_dir("owner-only");
        write_endpoint_file(&dir, 49213, "0123456789abcdef0123456789abcdef");
        let mode = std::fs::metadata(endpoint_path(&dir)).expect("written").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "endpoint.json must be readable by nobody else");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// There is no shutdown path by design, so a crash or a `kill -9` leaves this file behind with a
    /// dead port and a worthless token. Removing it at the next start is what keeps a dispatcher
    /// from being pointed at a port some *other* process has since been given.
    ///
    /// It also means turning the setting off and restarting takes the file away, rather than leaving
    /// an address for a door that no longer opens.
    #[test]
    fn a_stale_endpoint_file_is_removed_before_the_listener_binds() {
        let dir = endpoint_dir("stale");
        std::fs::write(endpoint_path(&dir), "{\"port\":1,\"token\":\"dead\"}").expect("seed");
        clear_endpoint_file(&dir);
        assert!(!endpoint_path(&dir).exists(), "a predecessor's file must not outlive it");
        // And a second clear is not an error: nothing there is the outcome we wanted anyway.
        clear_endpoint_file(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Built from `PrAxis::ALL`, so a fourth bar cannot appear on the page and be missing from the
    /// map a script reads. The URLs carry `127.0.0.1` rather than the page's own hostname because
    /// curl and most script HTTP clients do not resolve `.localhost` the way browsers do.
    #[test]
    fn the_endpoint_file_names_every_axis_the_page_serves() {
        let dir = endpoint_dir("axes");
        write_endpoint_file(&dir, 49213, TOKEN);
        let text = std::fs::read_to_string(endpoint_path(&dir)).expect("written");
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        for axis in PrAxis::ALL {
            let url = v["axes"][axis.slug()].as_str().unwrap_or_default().to_string();
            assert_eq!(url, format!("http://127.0.0.1:49213/{TOKEN}/{}/entries", axis.slug()));
        }
        assert_eq!(v["port"], serde_json::json!(49213));
        assert_eq!(v["token"], serde_json::json!(TOKEN));
        assert_eq!(v["host_header"], serde_json::json!("127.0.0.1:49213"));
        assert_eq!(v["pid"], serde_json::json!(std::process::id()));
        assert_eq!(v["schema"], serde_json::json!(1));
        let _ = std::fs::remove_dir_all(&dir);
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

    /// The off path, end to end. No test installs `SETTINGS`, so `entries_enabled` is genuinely
    /// false here and this is the real shut door rather than a simulated one.
    ///
    /// This is the direction that matters. Serving when the setting is on is the harmless failure;
    /// serving when it is off would mean a URL that does not exist today starts existing the moment
    /// somebody clicks a menu entry for the page's sake. The 404 also has to leak nothing: a 403
    /// would confirm the route is real and merely closed.
    #[test]
    fn the_entries_route_is_not_served_when_the_setting_is_off() {
        assert!(
            !entries_enabled(),
            "no ordinary test installs SETTINGS, so the door must read as shut. If this fired, \
             something ran the #[ignore]d `the_local_api_answers_over_a_real_loopback_connection` \
             in the same process — see its doc comment; run it on its own with --ignored."
        );
        let response = round_trip(&format!(
            "GET /{TOKEN}/approved/entries HTTP/1.1\r\nHost: githoot.localhost:{{PORT}}\r\n\r\n"
        ));
        assert!(response.starts_with("HTTP/1.1 404 "), "{response}");
        assert!(!response.contains("application/json"), "a shut door describes nothing: {response}");
        assert!(!response.contains("\"schema\""));
    }

    /// The guard rail against this ever growing a claim endpoint, proved over a real socket rather
    /// than only in `route_for`. The moment a dispatcher can tell GitHoot which pull requests it has
    /// taken, GitHoot is persisting PR state and the whole design is broken.
    ///
    /// Refused before the setting is even consulted, so it holds whether the door is open or shut.
    #[test]
    fn a_real_post_to_the_entries_route_is_refused_end_to_end() {
        let response = round_trip(&format!(
            "POST /{TOKEN}/approved/entries HTTP/1.1\r\nHost: githoot.localhost:{{PORT}}\r\n\
             Content-Length: 2\r\n\r\n{{}}"
        ));
        assert!(response.starts_with("HTTP/1.1 405 "), "{response}");
    }

    /// The `Host` allowlist runs before the path is examined, so a hostile page that reached this
    /// port learns nothing about whether the machine-readable route exists.
    #[test]
    fn a_real_entries_request_with_a_hostile_host_leaks_nothing() {
        for host in ["evil.com", "localhost:{PORT}"] {
            let response = round_trip(&format!(
                "GET /{TOKEN}/approved/entries HTTP/1.1\r\nHost: {host}\r\n\r\n"
            ));
            assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{host}: {response}");
            assert!(!response.contains("application/json"), "{host}: {response}");
        }
    }

    /// The whole `localApi` path with nothing faked: install the real settings, bind the real
    /// listener through `start_now`, read the address back out of the real `endpoint.json`, and ask
    /// for it over a real socket exactly as `curl` would.
    ///
    /// **`#[ignore]`d because it installs `SETTINGS`**, which is a `OnceLock` and therefore settable
    /// once per process. Running it alongside `the_entries_route_is_not_served_when_the_setting_is_off`
    /// would leave that test looking at an open door. `cargo test -- --ignored` runs only the
    /// ignored tests, so the two never meet; `--include-ignored` would make them, and the other
    /// test says so when it fires.
    ///
    /// Run it with:
    /// `cargo test --bin githoot-tray the_local_api_answers_over_a_real_loopback_connection -- --ignored --exact`
    #[test]
    #[ignore = "installs the process-wide SETTINGS and binds a real listener; run on its own"]
    fn the_local_api_answers_over_a_real_loopback_connection() {
        let dir = endpoint_dir("live");
        let (wake_tx, _wake_rx) = std::sync::mpsc::channel();
        install(Settings {
            app_asset_path: dir.clone(),
            sound: crate::config::Switch::new(false),
            copilot: crate::config::Switch::new(false),
            local_api: true,
            wake: std::sync::Mutex::new(wake_tx),
        });
        assert!(start_now(), "the loopback listener should bind");

        // Everything from here on is what a dispatcher does: read the file, dial the address.
        let published: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(endpoint_path(&dir)).expect("endpoint.json"))
                .expect("valid JSON");
        let port = published["port"].as_u64().expect("a port") as u16;
        let url = published["axes"]["approved"].as_str().expect("an approved URL").to_string();
        let path = url.split_once(&format!("127.0.0.1:{port}")).expect("the published host").1;

        let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        client
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes(),
            )
            .expect("write");
        let mut response = String::new();
        client.read_to_string(&mut response).expect("read");

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Content-Type: application/json; charset=utf-8"), "{response}");
        assert!(response.contains("ETag: "), "a dispatcher polls conditionally: {response}");

        let body = response.split_once("\r\n\r\n").expect("a body").1;
        let v: serde_json::Value = serde_json::from_str(body).expect("the body is JSON");
        assert_eq!(v["axis"], serde_json::json!("approved"));
        assert_eq!(v["schema"], serde_json::json!(1));
        // No poll has run in a test process, so the only honest answer is "not known" — and this is
        // the case a dispatcher must not mistake for an empty board.
        assert_eq!(v["known"], serde_json::json!(false));

        let _ = std::fs::remove_dir_all(&dir);
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





