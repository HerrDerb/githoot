//! Self-updating from GitHub Releases.
//!
//! Two jobs with very different shapes, deliberately kept apart:
//!
//!   * **The check** is one unauthenticated GET, finishes in well under a second, and only sets state.
//!     It runs inline on the poll thread behind a once-a-day gate.
//!   * **The install** downloads several megabytes, verifies a signature, replaces the running
//!     executable and hands over to it. It runs on a thread of its own so a five-minute download
//!     cannot freeze the notification icon.
//!
//! ## What is trusted, and what is not
//!
//! HTTPS to GitHub proves the bytes came from a host holding a certificate that chains to a root in
//! the **system** trust store. That is weaker than it sounds for this particular job: any corporate
//! TLS-inspection proxy, and any locally installed root CA, can serve arbitrary bytes that would pass.
//! GitHub release assets are also mutable — `release.yml` uploads with `--clobber` — so "GitHub served
//! this" is not the same as "CI built this from the tag".
//!
//! So the trust anchor is not the transport. It is [`PUBLIC_KEY`], compiled into this binary: CI signs
//! a file of SHA-256 digests, and an update is refused unless that signature verifies against the key
//! the *already installed* binary carries. An attacker who can replace release assets replaces the
//! digest file too, which is exactly why a checksum alone would be worthless here — only a signature
//! checked against a pre-installed key breaks that circularity.
//!
//! Everything else in this module (size bounds, magic bytes, the redirect allowlist, the
//! `--print-version` smoke test) catches corruption, truncation and wrong-artifact mistakes. Those are
//! quality controls, not security controls, and the comments say so where they appear.

use crate::{errorln, infoln};
use crate::version::Version;
use reqwest::blocking::Client;
use serde::Deserialize;

/// The repository the updater talks to. Hard-coded rather than configurable: this is the app updating
/// *itself*, and a settable update source would be a way to talk someone into installing anything.
const REPO: &str = "HerrDerb/githoot";

/// The same repository as a page a browser can open, for the tray's Settings entry.
///
/// Here rather than next to the menu wording in `state`, because this module already owns *which*
/// repository this app is: one literal, and a test pinning this URL to `REPO`, so the menu cannot end
/// up offering a fork or an old name the updater never installs from.
pub const REPOSITORY_URL: &str = "https://github.com/HerrDerb/githoot";

/// Matches the User-Agent the other GitHub-facing modules send.
const AGENT: &str = "githoot";

/// How many releases to ask for. The changelog spans every release between the installed version and
/// the newest, so this also bounds how far back that can reach — someone thirty releases behind gets a
/// truncated history rather than a second request.
const RELEASES_PER_PAGE: u32 = 30;

/// The asset published for the platform this binary was built for, or `None` if there is not one.
///
/// Resolved at compile time from `cfg!`, not by trying a URL and reading a 404. That matters for the
/// two real cases: aarch64 Linux and Intel macs are architectures the app runs on perfectly well but
/// which CI does not publish a build for, and they deserve "there is no build for your platform, build
/// from source" rather than a confusing download failure. Note that an x86_64 build running on Apple
/// Silicon under Rosetta reports `x86_64`, so it correctly declines instead of installing an
/// aarch64 bundle over itself.
const ASSET: Option<&str> = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("githoot")
} else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
    Some("githoot.exe")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("githoot-macos-aarch64.zip")
} else {
    None
};

/// The signed digest file and its signature, both published as release assets by CI.
const SUMS_ASSET: &str = "sha256sums.txt";
const SIG_ASSET: &str = "sha256sums.txt.minisig";

/// The minisign public key whose private half lives only in the repository's Actions secrets.
///
/// **Compiled in on purpose.** Fetching a key at runtime, or reading one from disk, would put it on the
/// same channel as the thing it is meant to authenticate and defeat the entire point. Rotating it
/// therefore means shipping a release signed with the old key that carries the new one, which is the
/// unavoidable cost of the guarantee.
///
/// The placeholder is not a valid key, and [`verify_sums`] refuses everything while it is in place, so
/// a half-configured build cannot install anything.
///
/// The base64 line only — not the `untrusted comment:` line that `minisign.pub` starts with, which
/// `PublicKey::from_base64` does not accept. `the_compiled_in_key_parses` catches a bad paste at test
/// time rather than leaving it to fail on a user's machine mid-install.
const PUBLIC_KEY: &str = "RWSYrhd3sxiQUDZtxm8c+p0iRdj+z+fGQKdLq62ojrmfii2OjCG8PX8D";

/// Sentinel for "no signing key has been configured yet". Compared by pointer-independent equality in
/// [`signing_configured`].
const PUBLIC_KEY_PLACEHOLDER: &str = "REPLACE_WITH_MINISIGN_PUBLIC_KEY";

/// Whether a real signing key has been baked in.
///
/// Split out so the check path can still run, and the arrow can still appear, on a build where signing
/// has not been set up — the user is told an update exists and sent to the Releases page, rather than
/// the feature silently doing nothing.
pub fn signing_configured() -> bool {
    PUBLIC_KEY != PUBLIC_KEY_PLACEHOLDER
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum UpdateError {
    /// Could not reach GitHub. Says nothing about whether an update exists.
    Network(String),
    /// GitHub answered, but not with what was asked for.
    Http(String),
    /// The response could not be understood.
    Parse(String),
    /// No published asset for this operating system and architecture.
    UnsupportedPlatform,
    /// Signing has not been configured in this build, so nothing can be verified or installed.
    NoSigningKey,
    /// The signature or a digest did not match. **Never** softened into a warning.
    Integrity(String),
    /// Something about the local filesystem or process prevented the install.
    Local(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "could not reach GitHub: {e}"),
            UpdateError::Http(e) => write!(f, "GitHub reported: {e}"),
            UpdateError::Parse(e) => write!(f, "unreadable response: {e}"),
            UpdateError::UnsupportedPlatform => write!(
                f,
                "no published build for {}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
            UpdateError::NoSigningKey => {
                write!(f, "this build has no update signing key compiled in")
            }
            UpdateError::Integrity(e) => write!(f, "the download failed verification: {e}"),
            UpdateError::Local(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for UpdateError {}

// ── The check ─────────────────────────────────────────────────────────────────

/// A release newer than the running binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: Version,
    /// The tag exactly as GitHub spells it, which is what asset download URLs are built from. Kept
    /// verbatim rather than reconstructed from `version`, so a tag like `v1.4.0` cannot be turned into
    /// `1.4.0` and produce a 404.
    pub tag: String,
    /// Every release from the installed version up to this one, newest first, for the Updates page.
    pub releases: Vec<ReleaseNotes>,
    /// The target release's page.
    pub url: String,
}

/// One release's notes, as GitHub holds them: Markdown, uncut. The Updates page renders them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub version: Version,
    /// `None` for a release with no notes, blank ones included.
    pub body: Option<String>,
    pub url: String,
}

/// Only the fields that are actually used, matching the minimal-struct convention in `github.rs`.
#[derive(Debug, Deserialize)]
struct ReleaseJson {
    tag_name: String,
    /// Absent on a release with no notes at all, hence `Option` rather than a defaulted `String`.
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Asks GitHub for newer releases and, if there are any, returns the newest with its accumulated notes.
///
/// Unauthenticated on purpose. The repository is public, the unauthenticated limit of 60 requests an
/// hour per IP is ample for a daily check, and sending the PR-status token to an endpoint that has
/// nothing to do with PR status would widen that token's exposure for no gain.
///
/// Uses the *list* endpoint rather than `/releases/latest` because the list returns every release
/// **and its body** in one request, which is what makes the changelog free rather than N more calls.
pub fn check(client: &Client, current: Version) -> Result<Option<Available>, UpdateError> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page={RELEASES_PER_PAGE}");
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", AGENT)
        .send()
        .map_err(|e| UpdateError::Network(e.to_string()))?;

    let status = response.status();
    let body = response.text().map_err(|e| UpdateError::Network(e.to_string()))?;
    if !status.is_success() {
        // Truncated: an error body can be a whole HTML page, and this ends up in a log line.
        let snippet: String = body.chars().take(200).collect();
        return Err(UpdateError::Http(format!("{status} for the releases list ({snippet})")));
    }

    let releases: Vec<ReleaseJson> =
        serde_json::from_str(&body).map_err(|e| UpdateError::Parse(e.to_string()))?;

    Ok(newer_than(&releases, current))
}

/// Picks the newest usable release above `current` and keeps every skipped release's notes. Pure, so
/// the whole filter-and-order decision is testable without a network.
fn newer_than(releases: &[ReleaseJson], current: Version) -> Option<Available> {
    // Drafts are invisible to an unauthenticated caller anyway, but filtered regardless so the rule is
    // stated rather than relied upon. Pre-releases are excluded because nothing here asks the user
    // whether they want one, and quietly moving somebody onto a release candidate is not a decision an
    // updater should make on its own.
    let mut candidates: Vec<(Version, &ReleaseJson)> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| Version::parse(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| *v > current)
        .collect();

    // Newest first. Sorted by parsed version rather than trusting GitHub's ordering, which is by
    // creation date — re-tagging or a backported patch release would put those two out of step.
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let (version, newest) = candidates.first().copied()?;
    Some(Available {
        version,
        tag: newest.tag_name.clone(),
        releases: candidates
            .iter()
            .map(|(version, r)| ReleaseNotes {
                version: *version,
                body: r.body.as_deref().map(str::trim).filter(|b| !b.is_empty()).map(str::to_string),
                url: r.html_url.clone(),
            })
            .collect(),
        url: newest.html_url.clone(),
    })
}

// ── Progress ──────────────────────────────────────────────────────────────────

/// The steps of an install, in the order `install` runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Signature,
    Download,
    Digest,
    Test,
    Replace,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Signature, Step::Download, Step::Digest, Step::Test, Step::Replace];

    /// The word the page script and the JSON use.
    pub fn key(self) -> &'static str {
        match self {
            Step::Signature => "signature",
            Step::Download => "download",
            Step::Digest => "digest",
            Step::Test => "test",
            Step::Replace => "replace",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Step::Signature => "Check the release signature",
            Step::Download => "Download",
            Step::Digest => "Check the SHA-256",
            Step::Test => "Test the new build",
            Step::Replace => "Replace this app",
        }
    }
}

/// How an install stands, for the Updates page. One at a time: `begin` is the only way in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Progress {
    #[default]
    Idle,
    /// `step` is `None` between the click and the first step. `done` and `total` are bytes, and only
    /// move during the download.
    Running { version: String, step: Option<Step>, done: u64, total: Option<u64> },
    /// Installed; the app is handing over to the new version, and this server goes with it.
    Restarting { version: String },
    /// Nothing was replaced. A new `begin` may try again.
    Failed { version: String, message: String },
}

impl Progress {
    /// Starts an install of `version`, unless one is already running or restarting.
    pub fn begin(&mut self, version: &str) -> bool {
        if matches!(self, Progress::Running { .. } | Progress::Restarting { .. }) {
            return false;
        }
        *self = Progress::Running { version: version.to_string(), step: None, done: 0, total: None };
        true
    }

    pub fn step(&mut self, next: Step) {
        if let Progress::Running { step, done, total, .. } = self {
            *step = Some(next);
            *done = 0;
            *total = None;
        }
    }

    pub fn bytes(&mut self, now: u64, of: Option<u64>) {
        if let Progress::Running { step: Some(Step::Download), done, total, .. } = self {
            *done = now;
            *total = of;
        }
    }

    pub fn fail(&mut self, message: &str) {
        if let Progress::Running { version, .. } | Progress::Restarting { version } = self {
            *self = Progress::Failed { version: version.clone(), message: message.to_string() };
        }
    }

    fn restarting(&mut self) {
        if let Progress::Running { version, .. } = self {
            *self = Progress::Restarting { version: version.clone() };
        }
    }

    /// The page script's answer to `?progress=1`.
    pub fn json(&self) -> String {
        match self {
            Progress::Idle => serde_json::json!({ "state": "idle" }),
            Progress::Running { version, step, done, total } => serde_json::json!({
                "state": "running",
                "version": version,
                "step": step.map(Step::key),
                "done": done,
                "total": total,
            }),
            Progress::Restarting { version } => serde_json::json!({ "state": "restarting", "version": version }),
            Progress::Failed { version, message } => {
                serde_json::json!({ "state": "failed", "version": version, "message": message })
            }
        }
        .to_string()
    }
}

/// Whether a page is following the install, as the restart sees it.
///
/// The restart takes this server down, and the next one listens on a new port with a new token, so a
/// page that has not been told `restarting` by then never will be: it would sit on the last step it
/// saw. The restart therefore waits until a following page has heard it, or has gone quiet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Followers {
    last_poll: Option<std::time::Instant>,
    told_restarting: Option<std::time::Instant>,
}

/// How long after a page was told `restarting` the restart still waits, so a second tab polling
/// alongside it gets its answer too. One shared flag cannot tell tabs apart.
const AFTER_TOLD: std::time::Duration = std::time::Duration::from_secs(1);
/// How long without a poll before a page counts as closed. The script polls every 700 ms, and a
/// background tab is throttled to about once a second.
const QUIET: std::time::Duration = std::time::Duration::from_secs(5);
/// The longest the restart ever waits, whatever the bookkeeping says.
const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

impl Followers {
    /// A page asked how the install stands, and was answered `progress`.
    pub fn polled(&mut self, progress: &Progress, now: std::time::Instant) {
        self.last_poll = Some(now);
        if matches!(progress, Progress::Restarting { .. }) && self.told_restarting.is_none() {
            self.told_restarting = Some(now);
        }
    }

    /// Whether the restart may go ahead, `since` being when the install finished.
    pub fn may_restart(&self, since: std::time::Instant, now: std::time::Instant) -> bool {
        let passed = |from: std::time::Instant, wait| now.saturating_duration_since(from) >= wait;
        let Some(last_poll) = self.last_poll else { return true };
        passed(since, MAX_WAIT)
            || passed(last_poll, QUIET)
            || self.told_restarting.is_some_and(|told| passed(told, AFTER_TOLD))
    }
}

/// The one install's progress. A `static` because the page that starts it, the thread that runs it
/// and the page that watches it are three different threads with nothing else in common.
static PROGRESS: std::sync::Mutex<Progress> = std::sync::Mutex::new(Progress::Idle);

fn with_progress<T>(f: impl FnOnce(&mut Progress) -> T) -> Option<T> {
    PROGRESS.lock().ok().map(|mut p| f(&mut p))
}

pub fn progress() -> Progress {
    with_progress(|p| p.clone()).unwrap_or_default()
}

/// Claims the install for `version`. `false` while another one runs, which makes a second click a no-op.
pub fn begin(version: &str) -> bool {
    let begun = with_progress(|p| p.begin(version)).unwrap_or(false);
    if begun && let Ok(mut f) = FOLLOWERS.lock() {
        // A page that polled an earlier install is not following this one until it polls again.
        *f = Followers::default();
    }
    begun
}

static FOLLOWERS: std::sync::Mutex<Followers> = std::sync::Mutex::new(Followers { last_poll: None, told_restarting: None });

/// The page script's poll: how the install stands, noting that a page asked and what it was told.
pub fn poll() -> Progress {
    let progress = progress();
    if let Ok(mut f) = FOLLOWERS.lock() {
        f.polled(&progress, std::time::Instant::now());
    }
    progress
}

/// Holds the restart until a page following the install has been told `restarting`, or has gone
/// quiet. Off the UI thread, so the tray keeps working meanwhile.
pub fn await_followers() {
    let since = std::time::Instant::now();
    loop {
        let ready = FOLLOWERS.lock().map(|f| f.may_restart(since, std::time::Instant::now())).unwrap_or(true);
        if ready {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Ends the install as failed, with `message` for the page.
pub fn fail(message: &str) {
    with_progress(|p| p.fail(message));
}

fn set_step(step: Step) {
    with_progress(|p| p.step(step));
}

/// Whether `asked`, the version the page showed, is the release waiting to be installed.
pub fn matches_pending(asked: &str, pending: Option<&Available>) -> bool {
    pending.is_some_and(|a| !asked.is_empty() && a.version.to_string() == asked)
}


/// URL of a named asset on a given release tag.
fn asset_url(tag: &str, asset: &str) -> String {
    // Hard-coded scheme. Never interpolated from a response, so no field GitHub controls can redirect
    // this somewhere else.
    format!("https://github.com/{REPO}/releases/download/{tag}/{asset}")
}

// ── Handing over to the new binary ────────────────────────────────────────────

/// What the UI thread must do to hand over to the freshly installed binary.
///
/// Produced on the update thread once the swap has succeeded, and carried to the UI thread, because
/// only the UI thread can take the tray down cleanly first. The three platforms then do quite
/// different things with it — Linux re-execs over itself, Windows spawns with the await-exit
/// handshake, macOS asks LaunchServices to open the bundle once this process is gone — so this holds
/// the facts rather than the procedure.
#[derive(Debug, Clone)]
pub struct RestartPlan {
    /// The executable to run, or on macOS the `.app` bundle to open.
    pub target: std::path::PathBuf,
    /// Where the previous version was moved to, if it still exists.
    ///
    /// Kept so a failed hand-over has something to fall back to: on Windows the spawn is checked and
    /// rolled back, and on Linux a failing `exec` can try the backup before giving up.
    pub backup: Option<std::path::PathBuf>,
}

// ── Install ───────────────────────────────────────────────────────────────────

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Whole-request cap for an asset download. The shared client's 10s cap (see `github.rs`) is sized for
/// a poll and would abort a 5 MB transfer on any ordinary connection.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Refuse anything absurd before writing a byte. The Linux release binary is ~5.4 MB.
const MAX_ASSET_BYTES: u64 = 100 * 1024 * 1024;
/// A release binary below this is not a release binary. Catches an HTML error page served with a 200.
const MIN_ASSET_BYTES: u64 = 1024 * 1024;
/// The sums file and its signature are a few hundred bytes; this is generous and still bounded.
const MAX_TEXT_BYTES: u64 = 64 * 1024;
/// How long the staged binary gets to answer `--print-version`.
const SMOKE_TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Hosts an asset download is allowed to be redirected to.
///
/// GitHub answers `/releases/download/...` with a redirect to its object store, so redirects cannot
/// simply be refused. Restricting *where* they may land is the cheap half of the defence: it costs
/// nothing and turns "the URL quietly went somewhere else" into a refusal with the host logged.
/// It is not a substitute for the signature — a host on this list serving bad bytes is exactly what
/// the signature catches — but it narrows the surface for free.
const ALLOWED_HOSTS: [&str; 3] =
    ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];

fn host_allowed(host: &str) -> bool {
    ALLOWED_HOSTS.contains(&host) || host.ends_with(".githubusercontent.com")
}

/// A directory that deletes itself, so no error path leaks a half-downloaded payload.
struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Created **beside the install target**, never in `/tmp`.
    ///
    /// The install finishes with `rename`, which fails with `EXDEV` across filesystems, and `/tmp` is
    /// usually a separate tmpfs. Staging on the same filesystem is what makes the final step atomic
    /// (or, on Windows and macOS, two adjacent renames) rather than a copy that can be interrupted
    /// halfway.
    fn beside(target: &Path) -> Result<Self, UpdateError> {
        let parent = target
            .parent()
            .ok_or_else(|| UpdateError::Local("the install target has no parent directory".into()))?;
        let dir = parent.join(format!(".githoot-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| UpdateError::Local(format!("could not create a staging directory: {e}")))?;
        Ok(ScratchDir(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best effort by design: a failure here must not mask the error that is being returned.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds the client used for downloads.
///
/// Separate from the shared polling client for two reasons that cannot both be solved per-request: the
/// timeout has to be minutes rather than seconds, and the redirect allowlist is a client-level policy.
/// Built only when installing, and dropped straight after.
fn download_client() -> Result<Client, UpdateError> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        match attempt.url().host_str() {
            Some(host) if host_allowed(host) => attempt.follow(),
            other => {
                // Logged rather than silently refused: a redirect off the allowlist is the one failure
                // here that is worth being able to look up afterwards.
                errorln!("update refused a redirect to an unexpected host: {other:?}");
                attempt.stop()
            }
        }
    });

    Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(policy)
        .build()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

/// Fetches a small text asset (the sums file or its signature) into memory.
fn fetch_text(client: &Client, url: &str) -> Result<String, UpdateError> {
    let response = client
        .get(url)
        .header("User-Agent", AGENT)
        .send()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Http(format!("{} for {url}", response.status())));
    }
    let mut buf = String::new();
    response
        .take(MAX_TEXT_BYTES)
        .read_to_string(&mut buf)
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    Ok(buf)
}

/// Streams an asset to `dest`, returning how many bytes were written.
/// `io::copy` that reports the running byte count after every chunk, for the page's progress bar.
///
/// `cap` bounds what a lying or endless server can make us write, independently of any
/// `Content-Length`: a read never asks for more than is left under it.
fn copy_counting(
    reader: &mut impl Read,
    writer: &mut impl std::io::Write,
    cap: u64,
    mut report: impl FnMut(u64),
) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    while done < cap {
        let want = buf.len().min((cap - done).try_into().unwrap_or(usize::MAX));
        let read = match reader.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..read])?;
        done += read as u64;
        report(done);
    }
    Ok(done)
}

fn fetch_file(client: &Client, url: &str, dest: &Path) -> Result<u64, UpdateError> {
    let response = client
        .get(url)
        .header("User-Agent", AGENT)
        .send()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Http(format!("{} for {url}", response.status())));
    }

    // Checked before a byte is read, so an absurd advertised size costs nothing.
    let advertised = response.content_length();
    if let Some(len) = advertised
        && len > MAX_ASSET_BYTES
    {
        return Err(UpdateError::Integrity(format!("asset claims {len} bytes, refusing")));
    }

    let mut file = std::fs::File::create(dest)
        .map_err(|e| UpdateError::Local(format!("could not create {}: {e}", dest.display())))?;
    let mut response = response;
    let written = copy_counting(&mut response, &mut file, MAX_ASSET_BYTES, |done| {
        with_progress(|p| p.bytes(done, advertised));
    })
    .map_err(|e| UpdateError::Network(format!("download interrupted: {e}")))?;
    file.sync_all().map_err(|e| UpdateError::Local(format!("could not flush the download: {e}")))?;

    // A stream truncated by a proxy that never sends `close_notify` can surface as a clean EOF, so the
    // length comparison is what actually catches it.
    //
    // This is only valid because `reqwest` is built without the `gzip` feature: with it,
    // `Content-Length` would be the *compressed* size and this check would start rejecting good
    // downloads. If that feature is ever added, delete this comparison rather than debugging it.
    if let Some(len) = advertised
        && len != written
    {
        return Err(UpdateError::Integrity(format!("expected {len} bytes, got {written}")));
    }
    if written < MIN_ASSET_BYTES {
        return Err(UpdateError::Integrity(format!(
            "asset is only {written} bytes, which is too small to be a release build"
        )));
    }
    Ok(written)
}

/// Verifies the sums file against its detached signature using the compiled-in public key.
///
/// **The only authenticity check in this module.** Everything else confirms the download is intact;
/// this is what confirms it came from whoever holds the signing key. A failure here is fatal to the
/// install and is never downgraded to a warning.
fn verify_sums(sums: &str, signature: &str) -> Result<(), UpdateError> {
    if !signing_configured() {
        return Err(UpdateError::NoSigningKey);
    }
    let key = minisign_verify::PublicKey::from_base64(PUBLIC_KEY)
        .map_err(|e| UpdateError::Integrity(format!("the compiled-in public key is invalid: {e}")))?;
    let sig = minisign_verify::Signature::decode(signature)
        .map_err(|e| UpdateError::Integrity(format!("the signature could not be decoded: {e}")))?;
    key.verify(sums.as_bytes(), &sig, false)
        .map_err(|e| UpdateError::Integrity(format!("the sums file is not correctly signed: {e}")))
}

/// Pulls the expected digest for `asset` out of a `sha256sum`-format listing.
///
/// Lines are `<hex>  <name>`. A missing entry is an integrity failure, not a "skip the check": it means
/// the signed manifest does not cover the file about to be installed.
fn digest_for(sums: &str, asset: &str) -> Result<String, UpdateError> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let Some(digest) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        // CI may write paths; compare on the file name only.
        let name = name.trim_start_matches('*');
        if Path::new(name).file_name().and_then(|n| n.to_str()) == Some(asset) {
            return Ok(digest.to_ascii_lowercase());
        }
    }
    Err(UpdateError::Integrity(format!("the signed sums file has no entry for {asset}")))
}

/// SHA-256 of a file, lower-case hex.
fn sha256_file(path: &Path) -> Result<String, UpdateError> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path)
        .map_err(|e| UpdateError::Local(format!("could not read the download: {e}")))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| UpdateError::Local(format!("could not hash the download: {e}")))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Confirms the download at least has the shape of the artefact it claims to be.
///
/// A corruption check, not a security one: anyone able to substitute the file can also make it start
/// with the right bytes. What it does catch cheaply is the common accident — a GitHub HTML error page
/// served with a 200, or an asset built for the wrong architecture.
fn check_magic(path: &Path) -> Result<(), UpdateError> {
    // Exactly 64 bytes, and `read_exact` rather than `read`: the Windows branch needs the four bytes at
    // 0x3C, so "we got fewer bytes than that" has to be its own error rather than something a length
    // guard silently reports as the wrong kind of file. `MIN_ASSET_BYTES` already makes a real asset
    // this small impossible.
    let mut head = [0u8; 64];
    let mut file = std::fs::File::open(path)
        .map_err(|e| UpdateError::Local(format!("could not read the download: {e}")))?;
    file.read_exact(&mut head)
        .map_err(|e| UpdateError::Integrity(format!("the download is too short to identify: {e}")))?;

    // Every branch below asks only "what do the bytes say". The buffer is a fixed 64 bytes and
    // `read_exact` has already guaranteed it is full, so there are deliberately no length guards here.
    // An earlier `head.len() > 0x40` on the Windows arm could never be true and rejected every single
    // Windows update.
    let ok = if cfg!(target_os = "linux") {
        // ELF, 64-bit, little-endian, x86-64.
        head[..4] == [0x7f, b'E', b'L', b'F']
            && head[4] == 2
            && head[5] == 1
            && u16::from_le_bytes([head[18], head[19]]) == 0x3E
    } else if cfg!(target_os = "windows") {
        // MZ, then the PE header at the offset stored at 0x3C, then the machine type.
        &head[..2] == b"MZ" && {
            let at = u32::from_le_bytes([head[0x3C], head[0x3D], head[0x3E], head[0x3F]]) as usize;
            // Only the offset is in the first 64 bytes; the rest needs a seek, so re-read from there.
            let mut pe = [0u8; 6];
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(at as u64)).is_ok()
                && file.read_exact(&mut pe).is_ok()
                && &pe[..4] == b"PE\0\0"
                && u16::from_le_bytes([pe[4], pe[5]]) == 0x8664
        }
    } else {
        // macOS ships a zip of the .app bundle.
        head[..4] == [0x50, 0x4B, 0x03, 0x04]
    };

    if ok {
        Ok(())
    } else {
        let start: String = head.iter().take(8).map(|b| format!("{b:02x}")).collect();
        Err(UpdateError::Integrity(format!(
            "the download is not the expected kind of file (starts {start})"
        )))
    }
}

/// Runs the staged binary with `--print-version` and requires it to report `expected`.
///
/// This is the check that catches a *wrong but valid* artefact: release assets are mutable and CI
/// uploads with `--clobber`, so a re-run can leave the tag pointing at a different build than its notes
/// describe. Proves nothing about authenticity — a hostile binary prints whatever it likes — which is
/// why it runs after the signature, not instead of it.
fn smoke_test(binary: &Path, expected: &Version) -> Result<(), UpdateError> {
    set_step(Step::Test);
    let mut child = std::process::Command::new(binary)
        .arg("--print-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            // Also the `noexec` mount case: the file is present and 0755 but cannot be executed.
            UpdateError::Local(format!("the downloaded binary could not be run: {e}"))
        })?;

    // Bounded wait, so a binary that hangs cannot park the update thread forever.
    let deadline = std::time::Instant::now() + SMOKE_TEST_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                return Err(UpdateError::Integrity(
                    "the downloaded binary did not answer --print-version".into(),
                ));
            }
            Err(e) => return Err(UpdateError::Local(e.to_string())),
        }
    }

    let out = child
        .wait_with_output()
        .map_err(|e| UpdateError::Local(format!("could not read the version: {e}")))?;
    if !out.status.success() {
        return Err(UpdateError::Integrity(format!(
            "the downloaded binary exited {} for --print-version",
            out.status
        )));
    }
    let reported = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match Version::parse(&reported) {
        Some(v) if v == *expected => Ok(()),
        _ => Err(UpdateError::Integrity(format!(
            "the downloaded binary reports {reported:?}, expected {expected}"
        ))),
    }
}

/// Confirms the install directory can be written to, before spending a download on finding out.
///
/// The case this exists for is a binary placed somewhere root-owned, which is a perfectly ordinary
/// thing to do and which no amount of retrying will fix. Deliberately **never** escalates: a
/// self-updater that asks for elevation to install bytes nobody has verified is a privilege-escalation
/// vector, so this reports and stops instead.
fn probe_writable(target: &Path) -> Result<(), UpdateError> {
    let parent = target
        .parent()
        .ok_or_else(|| UpdateError::Local("the install target has no parent directory".into()))?;
    let probe = parent.join(format!(".githoot-probe-{}", std::process::id()));
    let result = std::fs::write(&probe, b"x");
    let _ = std::fs::remove_file(&probe);
    result.map_err(|e| {
        UpdateError::Local(format!(
            "{} cannot be written to ({e}). Install the update by hand, or move the app somewhere \
             you own.",
            parent.display()
        ))
    })
}

/// Where this binary lives, and a refusal if that looks like a build tree.
///
/// Clobbering `target/release/githoot` would mean an update silently overwriting a developer's
/// own build, which is both surprising and pointless since the next `cargo build` undoes it.
fn resolve_current_exe() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe()
        .map_err(|e| UpdateError::Local(format!("could not locate this executable: {e}")))?;
    let shown = exe.to_string_lossy().replace('\\', "/");
    if shown.contains("/target/debug/") || shown.contains("/target/release/") {
        return Err(UpdateError::Local(
            "this looks like a local build (it is inside target/), so it will not be replaced".into(),
        ));
    }
    Ok(exe)
}

/// Whether this binary can update itself here, and why not if it cannot.
///
/// The cheap, local refusals, run when the Updates page renders so it can say so instead of offering a
/// button that would fail, and again at the start of `install`, since a folder can change in between.
pub fn preflight() -> Result<PathBuf, UpdateError> {
    ASSET.ok_or(UpdateError::UnsupportedPlatform)?;
    if !signing_configured() {
        return Err(UpdateError::NoSigningKey);
    }
    let exe = resolve_current_exe()?;
    let target = install_target(&exe)?;
    probe_writable(&target)?;
    Ok(target)
}

/// Downloads, verifies and installs `available`, returning what the UI thread must do next.
///
/// Runs on its own thread, after the Install button on the Updates page: that click is the consent,
/// so nothing here asks. Each step is published to `progress` as it starts, and a success ends in
/// `Restarting`. A failure is the caller's to publish, with `fail`.
pub fn install(available: &Available) -> Result<RestartPlan, UpdateError> {
    let asset = ASSET.ok_or(UpdateError::UnsupportedPlatform)?;
    let target = preflight()?;

    let client = download_client()?;
    let scratch = ScratchDir::beside(&target)?;

    // Signature first: if the manifest is not trustworthy there is no point downloading megabytes to
    // compare against it.
    set_step(Step::Signature);
    let sums = fetch_text(&client, &asset_url(&available.tag, SUMS_ASSET))?;
    let signature = fetch_text(&client, &asset_url(&available.tag, SIG_ASSET))?;
    verify_sums(&sums, &signature)?;
    let expected_digest = digest_for(&sums, asset)?;

    set_step(Step::Download);
    let payload = scratch.path().join(asset);
    let written = fetch_file(&client, &asset_url(&available.tag, asset), &payload)?;

    set_step(Step::Digest);
    let actual_digest = sha256_file(&payload)?;
    if actual_digest != expected_digest {
        return Err(UpdateError::Integrity(format!(
            "digest mismatch for {asset}: expected {expected_digest}, got {actual_digest}"
        )));
    }
    check_magic(&payload)?;
    infoln!("update {}: {written} bytes verified against the signed manifest", available.version);

    let plan = install_verified(&payload, &target, available)?;
    with_progress(Progress::restarting);
    Ok(plan)
}

/// What to launch to restart this same build in place, with nothing replaced.
///
/// Used by the settings watcher, which restarts the app so edited settings are read. Two deliberate
/// differences from the install path:
///
/// * `std::env::current_exe` directly, **not** `resolve_current_exe`. That function refuses anything
///   inside `target/` to stop the updater overwriting a local build — a protection that does not apply
///   when nothing is being written. Refusing to restart a dev build would only make this feature
///   untestable during development.
/// * On macOS a missing `.app` bundle is not an error. `install_target` must fail there because there is
///   nothing to replace; here the bare executable is a perfectly good thing to start again.
pub fn restart_target() -> Result<RestartPlan, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("could not locate this executable: {e}"))?;
    let target = install_target(&exe).unwrap_or(exe);

    // No backup: nothing was moved aside, so there is no previous version to fall back to. `exec_into`
    // and `spawn_successor` both guard on this being `None`.
    Ok(RestartPlan { target, backup: None })
}

/// The path that gets replaced: the executable itself, or on macOS the `.app` bundle around it.
fn install_target(exe: &Path) -> Result<PathBuf, UpdateError> {
    #[cfg(target_os = "macos")]
    {
        // `…/Foo.app/Contents/MacOS/githoot` → `…/Foo.app`. The whole bundle is replaced rather
        // than the inner binary, because CI's ad-hoc signature covers `Info.plist`, which carries a
        // per-release version string — dropping a new binary into an old bundle invalidates it.
        let bundle = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent());
        match bundle {
            Some(b) if b.extension().and_then(|e| e.to_str()) == Some("app") => Ok(b.to_path_buf()),
            _ => Err(UpdateError::Local(
                "this is not running from a .app bundle, so there is nothing to replace".into(),
            )),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(exe.to_path_buf())
    }
}

/// Puts a verified payload in place and returns how to hand over.
///
/// Split per platform because the filesystem rules genuinely differ, not for tidiness. The shared
/// principle is that **there is never a moment with no runnable binary**: Linux gets that for free from
/// an atomic `rename`, while Windows and macOS need two adjacent renames with an immediate undo if the
/// second fails.
#[cfg(all(unix, not(target_os = "macos")))]
fn install_verified(
    payload: &Path,
    target: &Path,
    available: &Available,
) -> Result<RestartPlan, UpdateError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(payload, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| UpdateError::Local(format!("could not make the download executable: {e}")))?;
    smoke_test(payload, &available.version)?;
    set_step(Step::Replace);

    // A hard link, not a copy: same inode, no bytes moved, and it survives the rename below because the
    // rename only rewrites a directory entry. It exists solely so a failing `exec` has a fallback.
    let backup = with_suffix(target, ".old");
    let _ = std::fs::remove_file(&backup);
    let backup = match std::fs::hard_link(target, &backup) {
        Ok(()) => Some(backup),
        // Not fatal. What protects the user here is the atomicity of the rename, not the backup.
        Err(e) => {
            errorln!("could not keep a backup of the current binary ({e}) — continuing");
            None
        }
    };

    // Atomic within a filesystem, which is why the staging directory is beside the target. There is no
    // instant at which `target` resolves to anything other than a complete, executable file.
    std::fs::rename(payload, target)
        .map_err(|e| UpdateError::Local(format!("could not replace {}: {e}", target.display())))?;

    infoln!("update {} installed at {}", available.version, target.display());
    Ok(RestartPlan { target: target.to_path_buf(), backup })
}

#[cfg(target_os = "windows")]
fn install_verified(
    payload: &Path,
    target: &Path,
    available: &Available,
) -> Result<RestartPlan, UpdateError> {
    smoke_test(payload, &available.version)?;
    set_step(Step::Replace);

    // Two renames, because Windows' rules are asymmetric: renaming a running `.exe` is permitted (only
    // the directory entry changes), while writing to it or replacing it is refused — replacing requires
    // deleting the destination, and the loader holds it. `std::fs::rename` is `MoveFileExW` underneath,
    // so no extra API is needed.
    //
    // A crash between the two leaves `…exe.old` and no `…exe`, which this process cannot heal because
    // the healer is the missing file. So: the recovery instruction is logged *before* the window opens,
    // the backup keeps a name a human would recognise, and nothing at all happens between A and B.
    let backup = with_suffix(target, ".old");
    let _ = std::fs::remove_file(&backup);
    infoln!(
        "replacing {} — if this is interrupted, rename {} back",
        target.display(),
        backup.display()
    );

    std::fs::rename(target, &backup)
        .map_err(|e| UpdateError::Local(format!("could not move the current binary aside: {e}")))?;
    if let Err(e) = std::fs::rename(payload, target) {
        // Undo A immediately, so the failure is fully recoverable.
        let _ = std::fs::rename(&backup, target);
        return Err(UpdateError::Local(format!("could not put the new binary in place: {e}")));
    }

    infoln!("update {} installed at {}", available.version, target.display());
    Ok(RestartPlan { target: target.to_path_buf(), backup: Some(backup) })
}

#[cfg(target_os = "macos")]
fn install_verified(
    payload: &Path,
    target: &Path,
    available: &Available,
) -> Result<RestartPlan, UpdateError> {
    // The payload is a zip of the whole `.app`. Extracted with `ditto`, which is part of the base system
    // and is what produced the archive, so it round-trips the bundle structure, the executable bit and
    // any extended attributes. `unzip` is the fallback, with the executable bit re-asserted by hand.
    let staged = payload.parent().unwrap_or(Path::new(".")).join("extracted");
    std::fs::create_dir_all(&staged).map_err(|e| UpdateError::Local(e.to_string()))?;

    let extracted = extract_bundle(payload, &staged)?;

    // Integrity, not authenticity: an attacker can ad-hoc sign anything. What this catches precisely is
    // a truncated or badly extracted bundle. `spctl --assess` is deliberately *not* used — an ad-hoc,
    // non-notarized bundle fails it by design.
    let verified = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(&extracted)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match verified {
        Ok(s) if s.success() => {}
        Ok(s) => {
            return Err(UpdateError::Integrity(format!("codesign rejected the new bundle ({s})")));
        }
        Err(e) => errorln!("could not run codesign ({e}) — continuing on the signature check alone"),
    }

    let inner = extracted.join("Contents/MacOS/githoot");
    smoke_test(&inner, &available.version)?;
    set_step(Step::Replace);

    let backup = with_suffix(target, ".old");
    let _ = std::fs::remove_dir_all(&backup);
    infoln!(
        "replacing {} — if this is interrupted, rename {} back",
        target.display(),
        backup.display()
    );

    std::fs::rename(target, &backup)
        .map_err(|e| UpdateError::Local(format!("could not move the current bundle aside: {e}")))?;
    if let Err(e) = std::fs::rename(&extracted, target) {
        let _ = std::fs::rename(&backup, target);
        // macOS 14+ gates one app modifying another's bundle behind App Management, with a carve-out for
        // self-update only when signing identities match — and an ad-hoc signature has no stable
        // identity. So `EPERM` here is an expected outcome, not a bug, and it degrades to "your update
        // is right here" rather than a half-swap.
        return Err(UpdateError::Local(format!(
            "could not replace {}: {e}. The verified update is at {} — move it into place by hand.",
            target.display(),
            extracted.display()
        )));
    }

    infoln!("update {} installed at {}", available.version, target.display());
    Ok(RestartPlan { target: target.to_path_buf(), backup: Some(backup) })
}

/// Extracts the release zip and returns the path to the `.app` inside it.
#[cfg(target_os = "macos")]
fn extract_bundle(payload: &Path, into: &Path) -> Result<PathBuf, UpdateError> {
    let ditto = std::process::Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(payload)
        .arg(into)
        .status();

    let extracted_ok = match ditto {
        Ok(s) if s.success() => true,
        _ => std::process::Command::new("/usr/bin/unzip")
            .arg("-q")
            .arg(payload)
            .arg("-d")
            .arg(into)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
    };
    if !extracted_ok {
        return Err(UpdateError::Local(
            "could not extract the update (neither ditto nor unzip worked)".into(),
        ));
    }

    // The archive holds `githoot.app` and `README.txt` side by side at the top level, so this
    // looks for the bundle rather than assuming a single entry.
    let entries = std::fs::read_dir(into).map_err(|e| UpdateError::Local(e.to_string()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("app") {
            return Ok(path);
        }
    }
    Err(UpdateError::Integrity("the update archive contained no .app bundle".into()))
}

/// `path` with `suffix` appended to its file name, keeping it in the same directory.
///
/// A recognisable name on purpose: it is what a user or a support note has to be able to point at if a
/// swap is ever interrupted between the two renames.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Removes leftovers from a previous update, best effort.
///
/// Called at startup rather than at the end of an install, because on Windows the old `.exe` cannot be
/// deleted until the process holding it has exited — which, by then, is this process's predecessor.
pub fn clean_up_after_update() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Ok(target) = install_target(&exe) else { return };

    let backup = with_suffix(&target, ".old");
    if backup.exists() {
        // Directory on macOS (a bundle), file elsewhere. Both attempted; whichever applies succeeds.
        let removed = std::fs::remove_file(&backup).or_else(|_| std::fs::remove_dir_all(&backup));
        match removed {
            Ok(()) => infoln!("removed the previous version at {}", backup.display()),
            Err(e) => errorln!("could not remove {} ({e}) — harmless, will retry next start", backup.display()),
        }
    }

    // Staging directories are named with the PID of the process that made them, so any left behind
    // belong to a dead process and are safe to remove.
    if let Some(parent) = target.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".githoot-update-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// A `/bin/sh -c <script>` that outlives the process starting it, for the macOS restart hand-over.
///
/// In a process group of its own, because the app's group is not safe to stay in: when the app was
/// started by its Launch Agent, `launchd` kills the job's whole process group as the app exits (no
/// `AbandonProcessGroup`), and the waiter that should restart it would die with it. The update would
/// then install and never come back, leaving the user to start it by hand.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn detached_shell(script: &str) -> std::process::Command {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new("/bin/sh");
    // 0: a new group led by the shell itself.
    command.arg("-c").arg(script).process_group(0);
    command
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;

    fn process_group_of(pid: &str) -> String {
        let out = std::process::Command::new("ps")
            .args(["-o", "pgid=", "-p", pid])
            .output()
            .expect("ps must run");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The fix for the waiter killed with its parent: it must not share the app's process group.
    #[test]
    fn the_restart_waiter_leaves_the_apps_process_group() {
        let out = detached_shell("ps -o pgid= -p $$").output().expect("the waiter must start");
        let waiter_group = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let own_group = process_group_of(&std::process::id().to_string());
        assert!(!waiter_group.is_empty(), "ps must report the waiter's group");
        assert_ne!(waiter_group, own_group, "the waiter must be in a group of its own");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, body: Option<&str>) -> ReleaseJson {
        ReleaseJson {
            tag_name: tag.to_string(),
            body: body.map(str::to_string),
            html_url: format!("https://example.invalid/{tag}"),
            draft: false,
            prerelease: false,
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).expect("test version parses")
    }

    #[test]
    fn nothing_newer_means_no_update() {
        let releases = [release("v1.2.0", Some("x")), release("v1.1.0", Some("y"))];
        assert_eq!(newer_than(&releases, v("1.2.0")), None);
        assert_eq!(newer_than(&releases, v("9.0.0")), None, "ahead of every release");
        assert_eq!(newer_than(&[], v("1.0.0")), None, "no releases at all");
    }

    #[test]
    fn picks_the_newest_and_keeps_the_tag_verbatim() {
        let releases = [release("v1.3.0", Some("c")), release("v1.4.0", Some("d"))];
        let found = newer_than(&releases, v("1.2.0")).expect("an update");
        assert_eq!(found.version, v("1.4.0"));
        // Verbatim, because the download URL is built from it and "1.4.0" would 404.
        assert_eq!(found.tag, "v1.4.0");
    }

    /// GitHub orders the list by creation date, so a backported patch cut *after* a newer minor would
    /// appear first. The updater must still offer the highest version, not the most recent upload.
    #[test]
    fn ordering_follows_version_not_the_order_github_returned() {
        let releases = [release("v1.2.1", Some("backport")), release("v1.4.0", Some("newer"))];
        let found = newer_than(&releases, v("1.2.0")).expect("an update");
        assert_eq!(found.version, v("1.4.0"));
        assert_eq!(found.releases[0].version, v("1.4.0"), "{:?}", found.releases);
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        let mut draft = release("v2.0.0", Some("draft"));
        draft.draft = true;
        let mut rc = release("v1.9.0", Some("rc"));
        rc.prerelease = true;
        let releases = [draft, rc, release("v1.3.0", Some("real"))];

        let found = newer_than(&releases, v("1.2.0")).expect("an update");
        assert_eq!(found.version, v("1.3.0"), "must skip the draft and the pre-release");
    }

    #[test]
    fn tags_that_are_not_versions_are_skipped_rather_than_guessed_at() {
        let releases = [release("nightly", Some("?")), release("v1.3.0", Some("real"))];
        let found = newer_than(&releases, v("1.2.0")).expect("an update");
        assert_eq!(found.version, v("1.3.0"));
    }

    /// The point of using the list endpoint: notes accumulate across every version skipped, so someone
    /// two releases behind sees both, each with its own page.
    #[test]
    fn notes_accumulate_across_every_skipped_release_newest_first() {
        let releases = [
            release("v1.3.0", Some("did the older thing")),
            release("v1.4.0", Some("did the newer thing")),
            release("v1.2.0", Some("already installed, must not appear")),
        ];
        let found = newer_than(&releases, v("1.2.0")).expect("an update");
        let shown: Vec<(String, Option<&str>, &str)> =
            found.releases.iter().map(|r| (r.version.to_string(), r.body.as_deref(), r.url.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("1.4.0".to_string(), Some("did the newer thing"), "https://example.invalid/v1.4.0"),
                ("1.3.0".to_string(), Some("did the older thing"), "https://example.invalid/v1.3.0"),
            ]
        );
    }

    /// A page scrolls, so nothing is cut any more: the dialog's caps are gone with the dialog.
    #[test]
    fn every_release_keeps_its_whole_body() {
        let long = (0..200).map(|i| format!("line {i} é")).collect::<Vec<_>>().join("\n");
        let found = newer_than(&[release("v1.3.0", Some(&long))], v("1.2.0")).expect("an update");
        assert_eq!(found.releases[0].body.as_deref(), Some(long.as_str()));
    }

    #[test]
    fn a_release_with_no_body_is_kept_with_none() {
        let found = newer_than(&[release("v1.3.0", None), release("v1.4.0", Some("  "))], v("1.2.0")).expect("an update");
        assert!(found.releases.iter().all(|r| r.body.is_none()), "{:?}", found.releases);
    }

    // ── Progress ───────────────────────────────────────────────────────────

    /// One install at a time: a second Install while one runs or restarts is refused, and a failed one
    /// can be tried again.
    #[test]
    fn only_one_install_runs_and_a_failed_one_can_be_retried() {
        let mut p = Progress::Idle;
        assert!(p.begin("1.4.0"));
        assert_eq!(p, Progress::Running { version: "1.4.0".into(), step: None, done: 0, total: None });
        assert!(!p.begin("1.4.0"), "a second click while running");
        p = Progress::Restarting { version: "1.4.0".into() };
        assert!(!p.begin("1.4.0"), "nor while restarting");
        p = Progress::Failed { version: "1.4.0".into(), message: "x".into() };
        assert!(p.begin("1.4.0"), "a failure is not a lock");
    }

    /// Each step replaces the last and zeroes the byte count; bytes only move within the download.
    #[test]
    fn steps_advance_and_bytes_belong_to_the_download() {
        let mut p = Progress::Idle;
        p.begin("1.4.0");
        p.step(Step::Signature);
        p.bytes(10, Some(100));
        assert_eq!(p, Progress::Running { version: "1.4.0".into(), step: Some(Step::Signature), done: 0, total: None }, "not downloading yet");
        p.step(Step::Download);
        p.bytes(40, Some(100));
        assert_eq!(p, Progress::Running { version: "1.4.0".into(), step: Some(Step::Download), done: 40, total: Some(100) });
        p.step(Step::Digest);
        assert_eq!(p, Progress::Running { version: "1.4.0".into(), step: Some(Step::Digest), done: 0, total: None });
        p.fail("boom");
        assert_eq!(p, Progress::Failed { version: "1.4.0".into(), message: "boom".into() });
        p.step(Step::Replace);
        assert!(matches!(p, Progress::Failed { .. }), "a step after a failure does not resurrect it");
    }

    /// What the page polls: a state word, the version, and for a running install which step and how
    /// many bytes. The keys are the page script's contract.
    #[test]
    fn progress_as_json_names_the_state_the_step_and_the_bytes() {
        let j = |p: &Progress| serde_json::from_str::<serde_json::Value>(&p.json()).unwrap();
        assert_eq!(j(&Progress::Idle), serde_json::json!({ "state": "idle" }));
        assert_eq!(
            j(&Progress::Running { version: "1.4.0".into(), step: Some(Step::Download), done: 5, total: Some(9) }),
            serde_json::json!({ "state": "running", "version": "1.4.0", "step": "download", "done": 5, "total": 9 })
        );
        assert_eq!(
            j(&Progress::Running { version: "1.4.0".into(), step: None, done: 0, total: None }),
            serde_json::json!({ "state": "running", "version": "1.4.0", "step": null, "done": 0, "total": null })
        );
        assert_eq!(j(&Progress::Restarting { version: "1.4.0".into() }), serde_json::json!({ "state": "restarting", "version": "1.4.0" }));
        assert_eq!(
            j(&Progress::Failed { version: "1.4.0".into(), message: "<x>".into() }),
            serde_json::json!({ "state": "failed", "version": "1.4.0", "message": "<x>" })
        );
    }

    // ── Followers ──────────────────────────────────────────────────────────

    fn restarting() -> Progress {
        Progress::Restarting { version: "1.4.0".into() }
    }

    fn running() -> Progress {
        Progress::Running { version: "1.4.0".into(), step: Some(Step::Download), done: 0, total: None }
    }

    /// No page polled during the install: nobody is watching, so nothing is worth waiting for.
    #[test]
    fn with_no_page_following_the_restart_goes_at_once() {
        let t = std::time::Instant::now();
        assert!(Followers::default().may_restart(t, t));
    }

    /// The bug: the page last heard "download", and the restart took the server away before its next
    /// poll. A page that is still polling must be told `restarting` first.
    #[test]
    fn a_polling_page_holds_the_restart_until_it_is_told() {
        let t = std::time::Instant::now();
        let ms = std::time::Duration::from_millis;
        let mut f = Followers::default();
        f.polled(&running(), t);
        assert!(!f.may_restart(t + ms(100), t + ms(100)), "the page has only heard download");
        f.polled(&restarting(), t + ms(700));
        assert!(!f.may_restart(t + ms(100), t + ms(800)), "a second tab still gets its poll in");
        assert!(f.may_restart(t + ms(100), t + ms(1700)), "told, and a poll interval later");
    }

    /// Being told `running` again does not count as being told `restarting`.
    #[test]
    fn only_a_restarting_answer_counts_as_told() {
        let t = std::time::Instant::now();
        let ms = std::time::Duration::from_millis;
        let mut f = Followers::default();
        f.polled(&running(), t);
        f.polled(&running(), t + ms(700));
        assert!(!f.may_restart(t + ms(700), t + ms(2000)));
    }

    /// A page that stopped polling was closed: it does not hold the restart up for long.
    #[test]
    fn a_page_gone_quiet_lets_the_restart_go() {
        let t = std::time::Instant::now();
        let s = std::time::Duration::from_secs;
        let mut f = Followers::default();
        f.polled(&running(), t);
        assert!(!f.may_restart(t, t + s(4)));
        assert!(f.may_restart(t, t + s(5)));
    }

    /// Whatever the bookkeeping says, the restart never waits past the cap.
    #[test]
    fn the_restart_never_waits_past_the_cap() {
        let t = std::time::Instant::now();
        let s = std::time::Duration::from_secs;
        let mut f = Followers::default();
        for i in 0..20 {
            f.polled(&running(), t + s(i));
        }
        assert!(!f.may_restart(t, t + s(9)));
        assert!(f.may_restart(t, t + s(10)));
    }

    /// The steps in the order `install` runs them, each with a key for the script and a label.
    #[test]
    fn the_steps_are_listed_in_the_order_they_run() {
        let keys: Vec<&str> = Step::ALL.iter().map(|s| s.key()).collect();
        assert_eq!(keys, ["signature", "download", "digest", "test", "replace"]);
        assert!(Step::ALL.iter().all(|s| !s.label().is_empty()));
    }

    /// Install names the version the page showed. Anything else, or nothing pending at all, is refused,
    /// so nobody installs a release whose notes they did not see.
    #[test]
    fn install_runs_only_for_the_version_the_page_showed() {
        let pending = newer_than(&[release("v1.4.0", Some("x"))], v("1.2.0"));
        assert!(matches_pending("1.4.0", pending.as_ref()));
        assert!(!matches_pending("1.3.0", pending.as_ref()), "a newer release landed after the page loaded");
        assert!(!matches_pending("1.4.0", None), "nothing pending");
        assert!(!matches_pending("", pending.as_ref()));
    }

    // ── The download ───────────────────────────────────────────────────────

    /// The download reports as it goes, ends on the full count, and the size cap still holds.
    #[test]
    fn the_download_reports_its_bytes_and_keeps_the_cap() {
        let data = vec![7u8; 200_000];
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let n = copy_counting(&mut data.as_slice(), &mut out, u64::MAX, |done| seen.push(done)).unwrap();
        assert_eq!((n, out.len()), (200_000, 200_000));
        assert!(seen.len() > 1, "more than one report: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "rising: {seen:?}");
        assert_eq!(seen.last(), Some(&200_000));

        let mut out = Vec::new();
        let n = copy_counting(&mut data.as_slice(), &mut out, 1000, |_| {}).unwrap();
        assert_eq!((n, out.len()), (1000, 1000), "never more than the cap");
    }

    #[test]
    fn asset_urls_point_at_the_tag_not_at_latest() {
        let url = asset_url("v1.4.0", "githoot");
        assert_eq!(url, "https://github.com/HerrDerb/githoot/releases/download/v1.4.0/githoot");
        assert!(url.starts_with("https://"), "the scheme must never be interpolated");
    }

    /// The repository entry in the tray's Settings submenu goes to the same repository the updater
    /// talks to. Two literals for one repository is how the menu ends up pointing at a fork or an old
    /// name while the updater still installs from here; this is the guard against that drift.
    #[test]
    fn the_repository_url_names_the_repo_the_updater_talks_to() {
        assert_eq!(REPOSITORY_URL, format!("https://github.com/{REPO}"));
        assert!(REPOSITORY_URL.starts_with("https://"), "the scheme must never be interpolated");
    }

    // ── Signature verification ─────────────────────────────────────────────
    //
    // The fixture below was produced with a **throwaway** key, deliberately not the production one, so
    // these tests keep working across a key rotation and do not have to be regenerated when it happens.
    // Signatures are public data, so committing them costs nothing.
    //
    // What this pins down is the integration that would otherwise only be discovered on a user's
    // machine: `rsign2` signs in *prehashed* mode by default (the trusted comment says so), and these
    // tests are what prove `minisign-verify` accepts that.

    const FIXTURE_KEY: &str = "RWRSP1s7ph02lSgOjs/sRoqD96ZfhLGQiWDuvACDyGTGaUR9uNujah6q";
    /// The asset names below are **payload bytes covered by `FIXTURE_SIG`**, not brand occurrences.
    /// They deliberately keep the app's former `git-system-tray` naming: the signature was produced
    /// over exactly these bytes with a throwaway key whose secret half is not in the repository, so
    /// editing a single character here — including renaming the app — invalidates the signature and
    /// there is no way to re-sign. What the fixture pins down is the minisign format handshake, which
    /// does not care what the file names say.
    const FIXTURE_SUMS: &str = "aaaa1111  git-system-tray\nbbbb2222  git-system-tray.exe\n";
    const FIXTURE_SIG: &str = "untrusted comment: signature from rsign secret key\nRURSP1s7ph02lQCwMH52Gi3Zoh1jG+gtapvj6PYMIoILCxU0MAnfmJQAEdkSX9Hh3L0A8cPvB2UECff7s0T2zjDKBUZeIqyy4Qw=\ntrusted comment: timestamp:1786700573\tfile:sums.txt\tprehashed\nH1ON98b2Caza1va3kteJKdWdtKn35NZKV1zZNUsRForYSSaEl+rstQqlTvkBmYvTR8pDYahUhPLn5dz1g8BZCA==\n";

    /// Verifies against an explicit key, so the fixture tests do not depend on the production one.
    fn verify_with(key: &str, sums: &str, signature: &str) -> Result<(), UpdateError> {
        let key = minisign_verify::PublicKey::from_base64(key)
            .map_err(|e| UpdateError::Integrity(format!("bad key: {e}")))?;
        let sig = minisign_verify::Signature::decode(signature)
            .map_err(|e| UpdateError::Integrity(format!("bad signature: {e}")))?;
        key.verify(sums.as_bytes(), &sig, false)
            .map_err(|e| UpdateError::Integrity(format!("verify failed: {e}")))
    }

    #[test]
    fn a_correctly_signed_sums_file_verifies() {
        assert!(
            verify_with(FIXTURE_KEY, FIXTURE_SUMS, FIXTURE_SIG).is_ok(),
            "minisign-verify must accept a prehashed rsign2 signature"
        );
    }

    /// `restart_target` must accept what `resolve_current_exe` refuses.
    ///
    /// This test binary lives in `target/debug/`, which is exactly the shape the installer rejects to
    /// avoid overwriting a local build. Nothing is overwritten by a restart, so refusing here would
    /// make the settings watcher impossible to exercise during development — the two functions
    /// disagreeing is the point, and this pins it in both directions.
    #[test]
    fn restart_target_accepts_a_local_build_that_the_installer_refuses() {
        assert!(
            resolve_current_exe().is_err(),
            "this test only means something while running from target/"
        );

        let plan = restart_target().expect("a restart must be possible from a local build");
        assert!(plan.target.exists(), "the restart target must be something that exists");
        assert!(
            plan.backup.is_none(),
            "nothing was moved aside, so offering a backup would be a lie"
        );
    }

    /// The case the whole feature exists to stop: the manifest was altered after signing.
    #[test]
    fn a_tampered_sums_file_is_refused() {
        let tampered = FIXTURE_SUMS.replace("aaaa1111", "aaaa1112");
        assert!(verify_with(FIXTURE_KEY, &tampered, FIXTURE_SIG).is_err());
    }

    /// And the other half: a signature from a key we do not trust must not verify, even though it is a
    /// perfectly valid signature over exactly these bytes.
    #[test]
    fn a_signature_from_another_key_is_refused() {
        let other = "RWSYrhd3sxiQUDZtxm8c+p0iRdj+z+fGQKdLq62ojrmfii2OjCG8PX8D";
        assert_ne!(other, FIXTURE_KEY, "the two keys must genuinely differ");
        assert!(verify_with(other, FIXTURE_SUMS, FIXTURE_SIG).is_err());
    }

    #[test]
    fn a_corrupt_signature_is_refused_rather_than_panicking() {
        for bad in ["", "not a signature", "untrusted comment: x\nnope\n"] {
            assert!(verify_with(FIXTURE_KEY, FIXTURE_SUMS, bad).is_err(), "{bad:?}");
        }
    }

    /// A paste error in `PUBLIC_KEY` — the comment line included, a truncated line — would otherwise
    /// surface only when a user tried to install.
    #[test]
    fn the_compiled_in_key_parses() {
        assert!(signing_configured(), "a real signing key must be compiled in");
        assert!(
            minisign_verify::PublicKey::from_base64(PUBLIC_KEY).is_ok(),
            "PUBLIC_KEY must be the bare base64 line from minisign.pub"
        );
    }

    /// Runs the whole integrity chain against the **real published release**: fetch the signed manifest,
    /// verify its signature with the compiled-in key, download this platform's asset, match its digest,
    /// check its magic bytes, and run it to confirm it honours the `--print-version` contract.
    ///
    /// `#[ignore]`d because it needs the network and several megabytes, but it is the test that would
    /// have caught a release published without a signature, or a signature over the wrong bytes. Run it
    /// after cutting a release:
    /// `cargo test -- --ignored verifies_the_live_release --nocapture`
    ///
    /// Deliberately stops short of installing: the swap and the hand-over need a real desktop session
    /// and a human to confirm, so those stay a manual step.
    #[test]
    #[ignore = "needs network; downloads the real release assets"]
    fn verifies_the_live_release() {
        let poll = crate::portal::github::api::build_client().expect("a polling client");
        // A version below any real release, so the newest one is always the target.
        let ancient = Version::parse("0.0.1").expect("parses");
        let available = check(&poll, ancient)
            .expect("the releases list must be readable")
            .expect("there must be at least one release newer than 0.0.1");
        println!("target: {} (tag {})", available.version, available.tag);

        let client = download_client().expect("a download client");
        let sums = fetch_text(&client, &asset_url(&available.tag, SUMS_ASSET))
            .expect("the release must publish sha256sums.txt");
        let signature = fetch_text(&client, &asset_url(&available.tag, SIG_ASSET))
            .expect("the release must publish sha256sums.txt.minisig");
        verify_sums(&sums, &signature).expect("the live manifest must verify against the built-in key");
        println!("signature verified against the compiled-in key");

        let asset = ASSET.expect("this platform must have a published asset to run this test");
        let expected = digest_for(&sums, asset).expect("the manifest must cover this platform's asset");

        let dir = std::env::temp_dir().join(format!("gst-live-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let payload = dir.join(asset);

        let bytes = fetch_file(&client, &asset_url(&available.tag, asset), &payload)
            .expect("the asset must download");
        let actual = sha256_file(&payload).expect("the download must be hashable");
        assert_eq!(actual, expected, "the published asset must match its signed digest");
        check_magic(&payload).expect("the asset must be the right kind of file");
        println!("{bytes} bytes, digest {actual} matches the signed manifest");

        // Only meaningful where the asset is a bare executable; the macOS asset is a zip.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            smoke_test(&payload, &available.version)
                .expect("the published binary must honour --print-version and report its own version");
            println!("smoke test passed: it reports {}", available.version);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn digests_are_looked_up_by_file_name() {
        // The names are the fixture's own — see `FIXTURE_SUMS`, whose bytes are frozen by a signature.
        assert_eq!(digest_for(FIXTURE_SUMS, "git-system-tray").unwrap(), "aaaa1111");
        assert_eq!(digest_for(FIXTURE_SUMS, "git-system-tray.exe").unwrap(), "bbbb2222");
        // An asset the signed manifest does not cover is an integrity failure, not a skipped check.
        assert!(digest_for(FIXTURE_SUMS, "git-system-tray-macos-aarch64.zip").is_err());
    }

    /// Only the allowlisted hosts may be redirected to.
    #[test]
    fn the_redirect_allowlist_admits_github_and_nothing_else() {
        for good in ["github.com", "objects.githubusercontent.com", "anything.githubusercontent.com"] {
            assert!(host_allowed(good), "{good} must be allowed");
        }
        for bad in ["evil.com", "githubusercontent.com.evil.com", "notgithub.com", ""] {
            assert!(!host_allowed(bad), "{bad} must be refused");
        }
    }

    /// Whatever platform the tests run on, asset selection must agree with the module's own idea of
    /// what is supported, rather than silently returning a name for a platform CI never builds.
    #[test]
    fn asset_selection_matches_the_published_matrix() {
        let expected = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some("githoot"),
            ("windows", "x86_64") => Some("githoot.exe"),
            ("macos", "aarch64") => Some("githoot-macos-aarch64.zip"),
            _ => None,
        };
        assert_eq!(ASSET, expected);
    }

    // ── Artefact shape ─────────────────────────────────────────────────────
    //
    // `check_magic` had no test of its own until it shipped broken: the Windows arm demanded
    // `head.len() > 0x40` from a buffer that is exactly 0x40 long, so it rejected every valid `.exe`.
    // Only `verifies_the_live_release` touched it, and that one is `#[ignore]`d behind the network.
    // These fixtures are a few dozen bytes and run everywhere.

    fn fixture(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gst-magic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("the fixture must be writable");
        path
    }

    /// A structurally real PE header: `MZ`, the PE offset at 0x3C, and `PE\0\0` plus a machine type
    /// where that offset points. Byte 2 is `0x90`, so the first eight bytes read `4d5a900000000000`,
    /// near enough the `4d5a900003000000` from the bug report to be recognisable.
    fn pe_bytes(machine: u16) -> Vec<u8> {
        let mut v = vec![0u8; 0x90];
        v[..2].copy_from_slice(b"MZ");
        v[2] = 0x90;
        v[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        v[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        v
    }

    /// 64-bit little-endian ELF with the given `e_machine`.
    fn elf_bytes(machine: u16) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        v[4] = 2;
        v[5] = 1;
        v[18..20].copy_from_slice(&machine.to_le_bytes());
        v
    }

    fn zip_bytes() -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[..4].copy_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        v
    }

    /// The header of whatever CI publishes for the platform the tests are running on.
    ///
    /// Chosen by `cfg!` to mirror `check_magic` itself, so this asserts something real on all three
    /// platforms rather than passing vacuously on two of them.
    fn this_platforms_asset(good: bool) -> Vec<u8> {
        if cfg!(target_os = "windows") {
            // 0xAA64 is arm64: a valid PE, wrong machine.
            pe_bytes(if good { 0x8664 } else { 0xAA64 })
        } else if cfg!(target_os = "linux") {
            // 0xB7 is aarch64.
            elf_bytes(if good { 0x3E } else { 0xB7 })
        } else if good {
            zip_bytes()
        } else {
            // The macOS asset is a zip, which carries no architecture of its own; a bare Mach-O
            // uploaded in its place is the equivalent accident.
            let mut v = vec![0u8; 64];
            v[..4].copy_from_slice(&[0xCF, 0xFA, 0xED, 0xFE]);
            v
        }
    }

    /// The regression, stated directly: the artefact this platform actually publishes must pass.
    #[test]
    fn the_asset_this_platform_publishes_is_accepted() {
        let path = fixture("good", &this_platforms_asset(true));
        let result = check_magic(&path);
        assert!(result.is_ok(), "a valid asset must pass, got {:?}", result.err().map(|e| e.to_string()));
    }

    /// An asset built for another architecture, which is the mistake the machine-type check exists for.
    #[test]
    fn an_asset_for_the_wrong_architecture_is_refused() {
        let path = fixture("wrong-arch", &this_platforms_asset(false));
        assert!(check_magic(&path).is_err(), "the wrong architecture must not install over this one");
    }

    /// A GitHub error page served with a 200. Wrong on every platform, so no `cfg` needed.
    #[test]
    fn an_html_error_page_is_refused() {
        let mut html = b"<!DOCTYPE html><html><head><title>Not Found</title>".to_vec();
        html.resize(64, b' ');
        let path = fixture("page.html", &html);
        let err = check_magic(&path).expect_err("HTML must never pass for a binary");
        assert!(format!("{err}").contains("not the expected kind of file"), "got {err}");
    }

    /// Shorter than the header itself: a clean refusal, never a panic on the slice. This is the case
    /// `read_exact` exists for, and the one a `read` plus length guard used to get wrong.
    #[test]
    fn a_truncated_download_is_refused_rather_than_panicking() {
        let path = fixture("truncated", b"MZ");
        let err = check_magic(&path).expect_err("two bytes cannot be a release build");
        assert!(format!("{err}").contains("too short"), "got {err}");
    }
}
