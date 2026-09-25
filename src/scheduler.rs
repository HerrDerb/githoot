//! Poll scheduler.
//!
//! Both platforms share one polling loop (`run_poll_loop`) and differ only in how an update is
//! handed to the UI thread. Linux forwards through an mpsc channel drained by a GTK timer;
//! Windows forwards through `EventLoopProxy::send_event`.
//!
//! The loop waits on `Receiver::recv_timeout` rather than `thread::sleep`. That one substitution
//! buys three things at once: pacing that adapts to GitHub's `x-poll-interval`, an on-demand
//! refresh when the user opens their notifications, and a clean exit when the UI goes away.

use crate::portal::github::api::build_client;
use crate::portal::types::PrEntry;
use crate::portal::{
    AuthError, AuthStatus, CredentialState, Portal, PortalId, PortalInfo, PortalStatus, SignInProgress,
    SignInPrompt,
};
use crate::update::{Available, RestartPlan};
use crate::{errorln, infoln};
use crate::state::{IconState, PollState, PrAxis, MENU_BURST, REFRESH_BURST};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Floor between re-authentication attempts, so a credential GitHub keeps rejecting cannot
/// produce a storm of authorization prompts.
const MIN_REAUTH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// How often to ask GitHub whether a newer release exists.
///
/// Once a day. The poll loop already wakes at least every 15 minutes, so this needs no timer of its
/// own — just an elapsed-time gate, the same shape as `may_retry`. Releases happen at human pace and
/// the unauthenticated rate limit is 60 requests an hour per IP, so anything faster would be spending
/// budget to learn nothing. The first check runs immediately at startup, which is when someone who has
/// just launched the app is most likely to be looking at it.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How often GitHub is asked how it is doing.
///
/// Far shorter than the update interval, because the two answer different kinds of question: a release
/// from yesterday is still there tomorrow, while an incident that started five minutes ago is worth
/// knowing about now and an incident that ended should clear promptly. Not every poll, though — this is
/// a third-party status page and 219 bytes a minute for the life of the process buys nothing, since
/// incidents are declared by humans on a scale of minutes.
const STATUS_CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Guards against two overlapping installs.
///
/// A `static` rather than a loop local because the install runs on a detached thread that outlives the
/// loop iteration that spawned it, so the flag has to live somewhere both can see. Clicking the menu
/// entry twice is the case this exists for.
static UPDATE_IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The last pull requests each axis's poll confirmed, indexed by `PrAxis::index`.
///
/// A `static` for the same reason `UPDATE_IN_FLIGHT` is: the writer is the poll thread and the
/// reader is a menu-click handler on the UI thread, created before the poll loop exists, so there
/// is no value to thread through. `None` means "no confirmed list" — never "no PRs", which is
/// `Some(vec![])`. All three axes fill their slot now; before 1.17.0 `ReviewRequested`'s stayed empty
/// for life, because a `total_count` read has no hits to name.
static PR_URLS: std::sync::Mutex<PrSnapshot> =
    std::sync::Mutex::new(PrSnapshot { portals: Vec::new(), polled_at: None, version: 0 });

/// What the poll thread last published, plus when.
///
/// `Instant` rather than `SystemTime`: the only consumer is the PR page's "as of 47 s ago" line,
/// which is a duration and not a date, so this needs no calendar, no timezone and no crate. It is
/// also monotonic, so a clock adjustment cannot make the page claim the data is from the future.
struct PrSnapshot {
    /// One entry per configured portal, in configuration order. Grouped by portal rather than
    /// merged per axis because the page says which forge a card came from and offers each
    /// portal's own inbox; a merged list would have to guess both from the URL.
    portals: Vec<PortalSnapshot>,
    polled_at: Option<std::time::Instant>,
    /// Bumped on every publish, and the PR page's `ETag`.
    ///
    /// A counter rather than a hash of the contents: the page needs to know whether it is looking at
    /// *this* poll's answer, and a poll that found the same pull requests still moved the clock the
    /// page's age line counts from. Cheap, monotonic, and it starts at zero so a client that has
    /// somehow held a tag across a restart simply gets the body again.
    version: u64,
}

/// One portal's share of the snapshot.
struct PortalSnapshot {
    info: PortalInfo,
    /// How the sign-in stands, for the settings page.
    auth: AuthStatus,
    /// Indexed by `PrAxis::index`. `None` means "no confirmed list", never "no PRs".
    axes: [Option<Vec<PrEntry>>; 3],
}

/// What the page renders for one axis: each portal with its confirmed list, plus the age and the
/// version the ETag is built from.
pub struct AxisSnapshot {
    pub groups: Vec<(PortalInfo, Option<Vec<PrEntry>>)>,
    pub polled_at: Option<std::time::Duration>,
    pub version: u64,
}

/// Where "open my pull requests" goes when no list can be opened: the first portal's inbox, or
/// GitHub's when nothing has been published yet, which is where every install pointed before
/// portals existed.
pub fn inbox_url() -> String {
    let snapshot = PR_URLS.lock().expect("PR-URLs lock poisoned");
    snapshot
        .portals
        .first()
        .map(|p| p.info.inbox_url.clone())
        .unwrap_or_else(|| format!("{}/pulls/inbox", crate::portal::github::DEFAULT_BASE_URL))
}

/// The sign-in currently running on the poll thread, if any, and whether the settings page has
/// asked for it to stop.
///
/// A `static` because the poll thread is *inside* `Portal::authenticate` for the whole flow and reads
/// no channel while there, so a cancel has to reach it some other way; the flow looks here between
/// its polls (see `SignInProgress::cancelled`).
struct SignIn {
    portal: PortalId,
    cancel_requested: bool,
}

static SIGN_IN: std::sync::Mutex<Option<SignIn>> = std::sync::Mutex::new(None);

/// Asks the running sign-in for `portal` to stop. `false` when none is running for it, which the
/// caller treats as already done rather than as an error.
pub fn cancel_sign_in(portal: &PortalId) -> bool {
    let mut running = SIGN_IN.lock().expect("sign-in lock poisoned");
    match running.as_mut() {
        Some(sign_in) if sign_in.portal == *portal => {
            sign_in.cancel_requested = true;
            true
        }
        _ => false,
    }
}

/// How a running sign-in reaches the settings page: the prompt lands in the snapshot the page reads,
/// and a cancel is looked up in `SIGN_IN`.
///
/// The code also goes to the clipboard, as the dialog used to put it, and when the loopback server
/// could not start at all the old dialog is shown instead, since a page nobody can open would leave
/// the code nowhere.
struct PageProgress {
    portal: PortalId,
    name: String,
}

impl SignInProgress for PageProgress {
    fn prompt(&self, prompt: SignInPrompt) {
        infoln!(
            "{}: sign in at {} with code {} (expires in {}s)",
            self.name,
            prompt.url,
            prompt.code,
            prompt.expires_at.saturating_sub(unix_now())
        );
        crate::dialog::copy_to_clipboard(&prompt.code);
        if !crate::serve::is_available() {
            crate::dialog::show_device_code_prompt(&format!("{} PR Status", self.name), &prompt.code, &prompt.url);
        }
        let mut snapshot = PR_URLS.lock().expect("PR-URLs lock poisoned");
        if let Some(p) = snapshot.portals.iter_mut().find(|p| p.info.id == self.portal) {
            p.auth = AuthStatus::SigningIn(Some(prompt));
        }
    }

    fn cancelled(&self) -> bool {
        SIGN_IN
            .lock()
            .expect("sign-in lock poisoned")
            .as_ref()
            .is_some_and(|s| s.portal == self.portal && s.cancel_requested)
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Every portal and how its sign-in stands, for the settings page. Published with the snapshot,
/// and again the moment a sign-in starts, so the page can say "in progress" while the device code
/// dialog is up rather than offering the button a second time.
pub fn portal_statuses() -> Vec<PortalStatus> {
    let snapshot = PR_URLS.lock().expect("PR-URLs lock poisoned");
    snapshot
        .portals
        .iter()
        .map(|p| PortalStatus { info: p.info.clone(), auth: p.auth.clone() })
        .collect()
}

/// The pull requests `axis` last confirmed, per portal, and how long ago that was.
///
/// `serve`'s listener thread is the caller, which is why this exists rather than the server reaching
/// into `PollState`: that stays owned by the poll thread alone. This static was already the channel
/// out of it, and the server is simply a third reader of one that already had two.
pub fn pr_snapshot(axis: PrAxis) -> AxisSnapshot {
    let snapshot = PR_URLS.lock().expect("PR-URLs lock poisoned");
    AxisSnapshot {
        groups: snapshot
            .portals
            .iter()
            .map(|p| (p.info.clone(), p.axes[axis.index()].clone()))
            .collect(),
        polled_at: snapshot.polled_at.map(|at| at.elapsed()),
        version: snapshot.version,
    }
}

/// Reason the poll loop was woken early.
///
/// The first two cause an immediate unconditional poll, differing only in what happens *after* it,
/// because they mean different things: one says the answer is about to change on GitHub's side, the
/// other says the user wants to see the current answer now. `Authenticate` is a different kind of
/// thing altogether — it asks the loop to do work first, and there is no point polling until it has.
pub enum Wake {
    /// The user opened a GitHub page, so what they have read is about to change. Needs follow-up
    /// polls, since GitHub takes a moment to register that things were read.
    Refresh,
    /// The user asked for an update directly, by clicking the icon. Nothing on GitHub's side is
    /// changing, so one poll answers it and a burst would just be traffic.
    PollNow,
    /// The user picked the Install update item.
    ///
    /// Unlike `Authenticate`, the work does **not** happen on the poll thread: downloading several
    /// megabytes would stall notification polling for as long as it takes, so this spawns a dedicated
    /// thread and returns immediately. See `run_poll_loop`.
    UpdateNow,
    /// The user asked for a portal's sign-in to run now: from the tray's Authenticate item, or from
    /// the settings page's button.
    ///
    /// `None` is the menu item, which exists only while some portal is waiting for a sign-in, so it
    /// means "whichever one that is". `Some(id)` is the settings page, which names the portal beside
    /// the button it rendered.
    ///
    /// Handled on the poll thread rather than in the click handler because that is where the
    /// credential lives, and because the flow blocks for as long as the user takes — up to GitHub's
    /// 15-minute device-code lifetime. Running it on the UI thread would freeze the tray for all of
    /// it, which on Linux means the whole GTK main loop.
    Authenticate(Option<PortalId>),
    /// The settings page's Sign out button: forget the portal's credential and wait for a sign-in.
    SignOut(PortalId),
    /// The user picked the Settings item, which has already opened `config.txt` in whatever handles it.
    ///
    /// Routed through here rather than started in the click handler because the watcher needs the same
    /// `restart` closure the update thread uses, and that lives on this side. Handling it only *spawns*
    /// a thread, so polling is not delayed by the fifteen minutes the watch may run for.
    SettingsOpened,
}

/// One rendering instruction for the UI thread.
#[derive(Debug)]
pub struct Update {
    pub icon: IconState,
    pub tooltip: String,
    /// Text for each PR axis's menu item, carrying the exact count, indexed by `PrAxis::index`.
    /// Worded in `state` rather than in each platform's UI code so the two cannot say it
    /// differently.
    pub pr_labels: [String; 3],
    /// Text for the install-update entry, carrying the version. `None` when no update is available, in
    /// which case the entry is hidden and its label is irrelevant.
    pub update_label: Option<String>,
}

/// Everything the poll loop needs that is not a channel or a UI handle.
///
/// Grouped because both platforms' entry points were growing a parallel list of the same four values,
/// and the Linux one had reached nine parameters — at which point the order is the only thing telling
/// two `bool`s apart. Naming them at the call site is worth a struct.
pub struct PollInputs {
    /// Every configured portal with how far its credential got at startup (see
    /// `main::build_portals`). Boxed trait objects because the loop is the one owner and never
    /// needs to know which kind it holds; `CredentialState` travels beside rather than inside so
    /// the adapter stays ignorant of how the loop words "not signed in".
    pub portals: Vec<(Box<dyn Portal>, CredentialState)>,
    /// Held for the whole run because `Wake::SettingsOpened` needs the config path.
    pub app_asset_path: PathBuf,
    /// Whether to look for newer releases at all. See `config::Config::update_check`.
    pub update_check: bool,
    /// Which PR signals the user wants, indexed by `PrAxis::index`.
    ///
    /// The **configuration only**, never ANDed with whether a credential exists. See
    /// `PollState::for_portal` for what folding those two together would break.
    pub pr_enabled: [bool; 3],
    /// Whether an arriving PR plays the hoot. See `config::Config::sound`.
    ///
    /// Checked at the call site rather than inside `sound::hoot`, so the module stays a plain "play
    /// this" with no opinion about settings, and the loop's own log line is silent too when it is off.
    ///
    /// A shared switch rather than the `bool` it was, because the tray's Hoot checkbox changes it while
    /// this loop is running — see `config::Switch`. Read once per cycle, at the moment it matters.
    pub sound: crate::config::Switch,
}

// ─── Shared polling core ──────────────────────────────────────────────────────

/// Spawns the poll thread.
///
/// `emit` returns `false` once the UI is gone, which ends the loop. A panic in here used to
/// freeze the icon at its last value with no trace; now it at least says so in the log.
fn spawn_poll_thread(
    inputs: PollInputs,
    wake_rx: Receiver<Wake>,
    emit: impl FnMut(Update) -> bool + Send + 'static,
    restart: impl Fn(RestartPlan) + Send + Clone + 'static,
) {
    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            run_poll_loop(inputs, wake_rx, emit, restart)
        }));
        if outcome.is_err() {
            errorln!("poll thread panicked — notification updates have stopped");
        }
    });
}

/// One portal in the loop: the adapter, its state, and the bookkeeping that used to be loop locals
/// back when there was only ever one of it.
struct PortalRun {
    portal: Box<dyn Portal>,
    state: PollState,
    /// Whether polls are issued at all. Off while waiting for a sign-in, or when the portal said
    /// there is nothing to see; the state carries the reason for the tooltip either way.
    live: bool,
    /// The last word on the credential, for the settings page. `live` is derived from it.
    credential: CredentialState,
    /// A sign-in is running on this thread right now. Set before `Portal::authenticate` blocks and
    /// published, so a page opened meanwhile says so.
    signing_in: bool,
    last_reauth: Option<Instant>,
    last_status_check: Option<Instant>,
    /// Remembered so a permanent typo in `statusComponents` is said once rather than every five
    /// minutes for as long as the app runs. `None` is "not asked yet", which is why the first
    /// successful check always reports.
    last_unmatched: Option<Vec<String>>,
}

impl PortalRun {
    /// `pr_enabled` is the config, not `[credential is Ready; 3]`. Whether a credential exists is
    /// said by the two calls below; folding it in here would make `require_pr_auth` a no-op on the
    /// very path that exists to obtain one. See `PollState::for_portal`.
    fn new(portal: Box<dyn Portal>, credential: CredentialState, pr_enabled: [bool; 3]) -> Self {
        let mut state = PollState::for_portal(&portal.info().display_name, pr_enabled);
        // Both non-live branches mean "no PR dots", and both are deliberately said differently: one
        // has a menu item waiting to be clicked, the other has a reason clicking cannot address. The
        // three axes share one credential, so whichever it is applies to all three at once.
        let live = match &credential {
            CredentialState::Ready => true,
            CredentialState::NeedsAuth => {
                state.require_pr_auth();
                false
            }
            CredentialState::Off(reason) => {
                for axis in PrAxis::ALL {
                    state.disable_pr(axis, reason.clone());
                }
                false
            }
        };
        PortalRun {
            portal,
            state,
            live,
            credential,
            signing_in: false,
            last_reauth: None,
            last_status_check: None,
            last_unmatched: None,
        }
    }

    fn name(&self) -> &str {
        &self.portal.info().display_name
    }

    /// How the sign-in stands, as the settings page words it.
    fn auth_status(&self) -> AuthStatus {
        if self.signing_in {
            AuthStatus::SigningIn(None)
        } else {
            AuthStatus::from(&self.credential)
        }
    }

    /// One cycle's worth of asking this portal: the axes in play, then, every few minutes, its
    /// own view of its health.
    fn cycle(&mut self) {
        self.state.begin_cycle();

        // Skipped before the request, not after: `apply_pr` would discard the answer for an axis
        // that is not in play, and a poll is the most expensive thing this app does, so issuing it
        // would be pure cost. The portal is told which axes are wanted and must not ask for the rest.
        if self.live {
            let axes = PrAxis::ALL.map(|axis| self.state.pr_in_play(axis));
            if axes.iter().any(|&wanted| wanted) {
                let outcome = self.portal.poll(axes);
                for (axis, response) in PrAxis::ALL.into_iter().zip(outcome.axes) {
                    if let Some(response) = response {
                        self.state.apply_pr(axis, response);
                    }
                }
            }
        }

        // Placed *before* the UI update, unlike the release check, and for the opposite reason. An
        // available release is equally true a minute later, so that check is allowed to miss the
        // cycle it runs in. An outage is the explanation for whatever else this cycle just found —
        // stale counts, unknown axes, a failed poll — so showing the icon first and the reason
        // second gets the order backwards. The cost is that a hanging status page can hold the icon
        // back for the client's 10s cap; the request is 219 bytes, so that is a remote worst case.
        //
        // A failed check deliberately does **not** clear a known outage: see `portal::statuspage`.
        // A portal with no status page answers `None` and is never asked again about it either way.
        if self.last_status_check.is_none_or(|at| at.elapsed() >= STATUS_CHECK_INTERVAL) {
            self.last_status_check = Some(Instant::now());
            match self.portal.health() {
                None => {}
                Some(Ok(report)) => {
                    // Only on a change, and the first answer always counts as one. A name that
                    // matches nothing is a typo the user has to fix, and its only other symptom is a
                    // mark that never appears — indistinguishable from the portal being well.
                    if self.last_unmatched.as_deref() != Some(report.unmatched.as_slice()) {
                        if !report.unmatched.is_empty() {
                            errorln!(
                                "config.txt names components {} does not publish, so they are \
                                 never watched: {}",
                                self.name(),
                                report.unmatched.join(", ")
                            );
                        }
                        self.last_unmatched = Some(report.unmatched);
                    }
                    match report.health {
                        crate::portal::Health::Degraded { description } => {
                            self.state.set_status_degraded(Some(description));
                        }
                        crate::portal::Health::Fine => self.state.set_status_degraded(None),
                    }
                }
                Some(Err(e)) => errorln!("could not read {}'s status page: {e}", self.name()),
            }
        }
    }

    /// Renews the credential when a poll was rejected or expiry is near. `true` means a fresh
    /// credential is worth retrying with at once.
    ///
    /// All three PR axes share one credential, so a rejection on any of them — or the credential
    /// simply approaching its known expiry (see `Portal::needs_refresh`) — triggers one shared
    /// renewal rather than three independent ones. `take_pr_reauth` is called for every axis
    /// unconditionally (not through a short-circuiting `.any()`) so each axis's flag is consumed.
    fn recover_credential(&mut self) -> bool {
        let mut needs_reauth = false;
        for axis in PrAxis::ALL {
            if self.state.take_pr_reauth(axis) {
                needs_reauth = true;
            }
        }
        if self.live && self.portal.needs_refresh() {
            needs_reauth = true;
        }
        if !(needs_reauth && self.live && may_retry(&mut self.last_reauth)) {
            return false;
        }
        match self.portal.reauthenticate() {
            // The silent refresh worked, so the new credential is worth retrying with at once.
            // Nothing user-visible happened, which is the point.
            Ok(()) => true,
            // The portal cannot help without the user. Hand it over rather than opening a browser
            // unannounced — the exclamation goes up and the Authenticate item appears, and
            // `Wake::Authenticate` picks it up from there.
            Err(AuthError::AuthorizationRequired) => {
                infoln!("{} PR status needs authorization — waiting for the menu", self.name());
                self.state.require_pr_auth();
                self.live = false;
                self.credential = CredentialState::NeedsAuth;
                false
            }
            // Anything else is a failure to *ask*, not an answer. Most often the network is down.
            // The credential is kept and the axes are left alone, so this retries on the next cycle
            // instead of costing the user a click it did not need. `MIN_REAUTH_INTERVAL` is what
            // stops that becoming a hot loop.
            Err(e) => {
                errorln!("{} credential renewal could not be attempted ({e}) — will retry", self.name());
                false
            }
        }
    }

    /// The user picked Sign out. The credential goes, the axes go dark with the exclamation up, and
    /// the Authenticate item is back on the menu: exactly the state a fresh install starts in.
    fn sign_out(&mut self) {
        match self.portal.sign_out() {
            Ok(()) => {
                infoln!("{} signed out; the saved credential was deleted", self.name());
                self.state.require_pr_auth();
                self.live = false;
                self.credential = CredentialState::NeedsAuth;
            }
            Err(e) => errorln!("{} could not sign out: {e}", self.name()),
        }
    }

    /// The user picked Authenticate. Blocks for as long as they take, which is exactly why it runs
    /// here and not in the click handler: the tray stays responsive throughout, and the only cost is
    /// that polling pauses while a credential is being obtained — which it could not usefully do
    /// anyway.
    fn authenticate(&mut self, progress: &dyn SignInProgress) {
        match self.portal.authenticate(progress) {
            Ok(CredentialState::Ready) => {
                infoln!("{} PR status authorized", self.name());
                self.state.clear_pr_auth();
                self.live = true;
                self.credential = CredentialState::Ready;
            }
            // Authorized, but there is nothing to see and another click cannot fix that, so the
            // exclamation comes down and a stated reason replaces it. Same answer startup gives.
            Ok(CredentialState::Off(reason)) => {
                infoln!("{reason}");
                self.state.clear_pr_auth();
                for axis in PrAxis::ALL {
                    self.state.disable_pr(axis, reason.clone());
                }
                self.live = false;
                self.credential = CredentialState::Off(reason);
            }
            Ok(CredentialState::NeedsAuth) => {
                errorln!("{} sign-in finished without a credential; try again", self.name());
            }
            // The user stopped it from the settings page. Nothing went wrong.
            Err(AuthError::Cancelled) => infoln!("{} sign-in cancelled", self.name()),
            // Denied, expired, or the network went away mid-flow. The state is left as it was, so
            // the item is still on the menu to try again.
            Err(e) => errorln!("authorization failed: {e}"),
        }
    }
}

/// Every portal's confirmed lists, in configuration order, for the snapshot the page reads.
fn snapshot_of(runs: &[PortalRun]) -> Vec<PortalSnapshot> {
    runs.iter()
        .map(|run| PortalSnapshot {
            info: run.portal.info().clone(),
            auth: run.auth_status(),
            // Muted pull requests ride along with the rest: the page and the API split them out live,
            // against the mute file, so a click shows at once rather than at the next poll.
            axes: PrAxis::ALL.map(|axis| {
                run.state.pr_entries(axis).map(|mut list| {
                    list.extend_from_slice(run.state.pr_muted(axis));
                    list
                })
            }),
        })
        .collect()
}

/// `app_asset_path` is held for the whole run because `Wake::SettingsOpened` needs the config path.
fn run_poll_loop(
    inputs: PollInputs,
    wake_rx: Receiver<Wake>,
    mut emit: impl FnMut(Update) -> bool,
    restart: impl Fn(RestartPlan) + Send + Clone + 'static,
) {
    let PollInputs {
        portals,
        app_asset_path,
        update_check: update_check_enabled,
        pr_enabled,
        sound: sound_enabled,
    } = inputs;
    // The release check's client. Each portal owns its own; this one talks to GitHub Releases
    // unauthenticated and has nothing to do with any portal.
    let client = match build_client() {
        Ok(client) => client,
        Err(e) => {
            errorln!("fatal: could not build HTTP client: {e}");
            return;
        }
    };

    let mut runs: Vec<PortalRun> = portals
        .into_iter()
        .map(|(portal, credential)| PortalRun::new(portal, credential, pr_enabled))
        .collect();

    // Published once before the first cycle, with no lists yet, so a page opened or a menu clicked
    // in the first second already knows which portals exist and where their inboxes are. `polled_at`
    // stays `None`: nothing has been asked, and the page says "not polled yet" accordingly.
    {
        let mut snapshot = PR_URLS.lock().expect("PR-URLs lock poisoned");
        snapshot.portals = snapshot_of(&runs);
    }

    let mut burst: VecDeque<Duration> = VecDeque::new();
    // `None` means "never checked", which is what makes the first check happen immediately.
    let mut last_update_check: Option<Instant> = None;
    // The release the last check found, held so the menu click has something to install without
    // re-asking GitHub. Cleared when a check finds nothing newer.
    let mut pending_update: Option<Available> = None;
    // The version of the newest release above this build, once a check has found one. App-level,
    // not any portal's: see `overview`.
    let mut update_available: Option<String> = None;

    loop {
        for run in &mut runs {
            run.cycle();
        }

        // Published before `emit` for the same reason the outage check is: the menu entries the URLs
        // belong to are about to be relabeled with this cycle's counts, and a click between the two
        // writes must open what the new label claims, not what the old one did.
        {
            let mut snapshot = PR_URLS.lock().expect("PR-URLs lock poisoned");
            *snapshot = PrSnapshot {
                portals: snapshot_of(&runs),
                polled_at: Some(std::time::Instant::now()),
                version: snapshot.version.saturating_add(1),
            };
        }
        // After the lock is released: the integration runner reads these lists as soon as it wakes.
        crate::integration::poll_published();

        // Read here, right after the axes were applied, rather than after `emit`: the flags belong to
        // this cycle's responses, and taking them next to the code that produced them is what keeps a
        // rise meaning one poll's worth of change. The sound itself waits until after `emit`.
        //
        // Taken unconditionally, even with the sound off, so the latch cannot accumulate. That matters
        // more now than it used to: the sound can be switched back on from the menu mid-run, and a
        // latch drained only while hooting was enabled would fire for every arrival missed while it
        // was off — one click, and a hoot for news the user has already seen.
        let per_portal: Vec<[bool; 3]> = runs.iter_mut().map(|run| run.state.take_pr_arrivals()).collect();
        let arrivals = crate::overview::arrivals(&per_portal);

        let (icon, tooltip, pr_labels) = {
            let views: Vec<crate::overview::PortalView> = runs
                .iter()
                .map(|run| crate::overview::PortalView { name: run.name(), state: &run.state })
                .collect();
            (
                crate::overview::icon(&views, update_available.is_some()),
                crate::overview::tooltip(&views, update_available.as_deref()),
                PrAxis::ALL.map(|axis| crate::overview::pr_menu_label(&views, axis)),
            )
        };

        // Update the UI before anything else here can block.
        let update_label = crate::overview::update_menu_label(update_available.as_deref());
        if !emit(Update { icon, tooltip, pr_labels, update_label }) {
            return; // UI has gone away
        }

        // ── The hoot ─────────────────────────────────────────────────────────
        // After `emit`, so the icon is already showing what the sound is about — a hoot with nothing
        // to look at yet would send the user to a tray that has not caught up. One hoot even when two
        // or three axes, or two portals, turn over in the same cycle: `crate::sound::hoot` drops
        // overlapping plays anyway, and three of the same clip at once is a noise rather than a
        // notification.
        if sound_enabled.is_on() && arrivals.iter().any(|&arrived| arrived) {
            let axes: Vec<&str> = PrAxis::ALL
                .iter()
                .filter(|axis| arrivals[axis.index()])
                .map(|axis| axis.menu_label())
                .collect();
            infoln!("hooting: {}", axes.join(", "));
            crate::sound::hoot();
        }

        // ── Is there a newer release? ────────────────────────────────────────
        // Cheap: one unauthenticated GET, gated to once a day, so it runs inline rather than needing a
        // thread. Placed after `emit` deliberately — the icon is already showing this cycle's answer
        // before this can add anything to it, so a slow or hanging check delays only itself.
        //
        // Failures are logged and dropped, never surfaced. Not being able to ask whether an update
        // exists is not something the user can act on, and a dialog for it would be noise.
        if update_check_enabled
            && last_update_check.is_none_or(|at| at.elapsed() >= UPDATE_CHECK_INTERVAL)
        {
            last_update_check = Some(Instant::now());
            match crate::version::Version::current() {
                Some(current) => match crate::update::check(&client, current) {
                    Ok(Some(available)) => {
                        infoln!(
                            "update available: {} (installed {current})",
                            available.version
                        );
                        update_available = Some(available.version.to_string());
                        pending_update = Some(available);
                    }
                    Ok(None) => {
                        // Clears the arrow, which matters right after an install: the new binary is
                        // current, so this is what takes the arrow back down.
                        update_available = None;
                        pending_update = None;
                    }
                    Err(e) => errorln!("update check failed: {e}"),
                },
                // Unreachable from a normal build; see `Version::current`.
                None => errorln!("update check skipped: this build's version is unparseable"),
            }
        }

        // ── Credential recovery ──────────────────────────────────────────────
        // Every portal is asked, not just the first that wants renewing: `recover_credential` also
        // consumes the per-axis flags, and a short-circuit would leave one portal's latched.
        let mut retry_now = false;
        for run in &mut runs {
            if run.recover_credential() {
                retry_now = true;
            }
        }
        if retry_now {
            continue; // retry at once with the fresh credential
        }

        // The slowest portal sets the pace: a portal's own floor (`min_poll_interval`) and whatever
        // backoff its state worked out, and the largest across portals wins. One portal at the
        // default floor is exactly the cadence this loop always had. A queued burst entry wins over
        // normal pacing, so resolve the real delay here.
        let rate_limited = runs.iter().any(|run| run.state.rate_limited());
        let steady = runs
            .iter()
            .map(|run| run.state.next_delay().max(run.portal.info().min_poll_interval))
            .max()
            .unwrap_or(crate::state::MIN_POLL_INTERVAL);
        let delay = match next_pace(&mut burst, rate_limited, steady) {
            Pace::Burst(delay) => delay,
            Pace::Steady(delay) => delay,
        };

        // No healthy-poll heartbeat here on purpose: a clean cycle logs nothing, so the only lines
        // in the file are the failures worth reading. Each failing poll already logged its reason,
        // named by axis.

        match wake_rx.recv_timeout(delay) {
            Ok(wake) => {
                match wake {
                    Wake::Refresh => {
                        infoln!("refresh requested — polling {} more times", REFRESH_BURST.len());
                        burst = REFRESH_BURST.iter().copied().collect();
                    }
                    // A short even burst rather than the widening one: one sample taken within a
                    // second of the click is thin, and anything that changes a moment later would
                    // otherwise stay invisible for the rest of the minute.
                    Wake::PollNow => {
                        infoln!(
                            "menu opened — polling now and {} more times, 5s apart",
                            MENU_BURST.len()
                        );
                        burst = MENU_BURST.iter().copied().collect();
                    }
                    // Deliberately *not* on this thread, unlike `Authenticate`. The download is
                    // megabytes and the timeout is minutes, and polling has to keep working
                    // throughout — a frozen icon during an install would look like a crash.
                    Wake::UpdateNow => {
                        if let Some(available) = pending_update.clone() {
                            // `swap` rather than load-then-store: two menu clicks in quick succession
                            // must not both get past this.
                            if UPDATE_IN_FLIGHT.swap(true, Ordering::SeqCst) {
                                infoln!("an update install is already in progress");
                            } else {
                                let restart = restart.clone();
                                std::thread::spawn(move || {
                                    match crate::update::install(&available) {
                                        // Hands the plan to the UI thread, which is the only one that
                                        // can take the tray down cleanly before the process ends.
                                        Ok(plan) => restart(plan),
                                        Err(crate::update::UpdateError::Declined) => {
                                            infoln!("update declined by the user");
                                        }
                                        Err(e) => crate::dialog::report(
                                            "githoot: update failed",
                                            &format!(
                                                "The update was not installed and the current \
                                                 version is untouched.\n\n{e}"
                                            ),
                                        ),
                                    }
                                    UPDATE_IN_FLIGHT.store(false, Ordering::SeqCst);
                                });
                            }
                        } else {
                            infoln!("install requested, but no update is pending");
                        }
                    }
                    // Opening settings says nothing about GitHub, so this is the one wake that does not
                    // want a poll at all — hence `skip_etag` going back down. All it does is arm the
                    // watcher, which then lives on its own thread.
                    Wake::SettingsOpened => {
                        crate::settings_watch::spawn(
                            crate::config::config_path(&app_asset_path),
                            restart.clone(),
                        );
                    }
                    Wake::SignOut(id) => {
                        match runs.iter_mut().find(|run| run.portal.info().id == id) {
                            Some(run) => run.sign_out(),
                            None => errorln!("sign-out requested for a portal that is not configured"),
                        }
                        // At once, so the page's one reload after the redirect sees the outcome.
                        PR_URLS.lock().expect("PR-URLs lock poisoned").portals = snapshot_of(&runs);
                    }
                    // The named portal, or the one waiting for a sign-in, or the first: with a
                    // single portal those are the same thing.
                    Wake::Authenticate(which) => {
                        let index = match which {
                            Some(id) => runs.iter().position(|run| run.portal.info().id == id),
                            None => Some(
                                runs.iter().position(|run| run.state.pr_needs_auth()).unwrap_or(0),
                            ),
                        };
                        match index {
                            None => errorln!("sign-in requested for a portal that is not configured"),
                            // Also over a working credential: the settings page offers "Sign in
                            // again", and a successful flow simply replaces what is saved.
                            Some(i) => {
                                if runs[i].credential == CredentialState::Ready {
                                    infoln!("{} signing in again over a working credential", runs[i].name());
                                }
                                // Said before the flow blocks, so a settings page opened during the
                                // minutes it can take reads "in progress" rather than offering the
                                // button again. The prompt itself lands later, from the flow.
                                let id = runs[i].portal.info().id.clone();
                                *SIGN_IN.lock().expect("sign-in lock poisoned") =
                                    Some(SignIn { portal: id.clone(), cancel_requested: false });
                                runs[i].signing_in = true;
                                PR_URLS.lock().expect("PR-URLs lock poisoned").portals = snapshot_of(&runs);
                                let progress = PageProgress { portal: id, name: runs[i].name().to_string() };
                                runs[i].authenticate(&progress);
                                runs[i].signing_in = false;
                                *SIGN_IN.lock().expect("sign-in lock poisoned") = None;
                                // Published at once rather than after the next cycle, so a page
                                // refreshing itself sees the outcome instead of a stale prompt.
                                PR_URLS.lock().expect("PR-URLs lock poisoned").portals = snapshot_of(&runs);
                            }
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The sender lives in the UI, so a closed channel means the app is shutting down.
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Which pace the next cycle runs at.
#[derive(Debug, PartialEq, Eq)]
enum Pace {
    /// A queued burst entry, deliberately faster than the floor.
    Burst(Duration),
    /// Normal pacing, whatever `PollState` worked out.
    Steady(Duration),
}

/// Picks the wait before the next cycle.
///
/// A burst beats normal pacing, but never a wait GitHub explicitly demanded. Retrying inside a
/// `retry-after` window is how a short secondary limit becomes a long one, and a burst is never
/// worth that.
///
/// The remaining burst entries are dropped rather than deferred. A burst exists to catch a change
/// the user just caused; once a rate-limit window has elapsed, that moment has passed and the normal
/// cadence is the right thing to return to.
///
/// Pure so this can be tested without a network, which is the point: the previous version read
/// GitHub's instruction, stored it, and then silently discarded it whenever a burst was in flight,
/// and nothing in the suite could see that happen.
fn next_pace(burst: &mut VecDeque<Duration>, rate_limited: bool, steady: Duration) -> Pace {
    if rate_limited {
        burst.clear();
        return Pace::Steady(steady);
    }

    match burst.pop_front() {
        Some(delay) => Pace::Burst(delay),
        None => Pace::Steady(steady),
    }
}

/// Rate-limits credential renewal attempts, stamping the clock when one is allowed.
fn may_retry(last: &mut Option<Instant>) -> bool {
    if last.is_none_or(|at| at.elapsed() >= MIN_REAUTH_INTERVAL) {
        *last = Some(Instant::now());
        true
    } else {
        infoln!("skipping credential renewal — attempted too recently");
        false
    }
}

// ─── Linux ────────────────────────────────────────────────────────────────────

/// How often the GTK main loop checks for pending updates. Cheap — it only touches a channel.
#[cfg(target_os = "linux")]
const UI_DRAIN_INTERVAL: Duration = Duration::from_secs(1);

/// The menu entries the poll loop keeps up to date: their text, and whether they are shown at all.
///
/// A struct rather than two more parameters, because the list was going to keep growing.
#[cfg(target_os = "linux")]
pub struct MenuItems {
    pub reviews: gtk::MenuItem,
    pub ready_to_merge: gtk::MenuItem,
    pub changes_requested: gtk::MenuItem,
    /// The fallback for when none of the three above have anything behind them. Conditional like they
    /// are, but on the opposite condition — see `state::shows_pr_inbox`.
    pub pr_inbox: gtk::MenuItem,
    /// Shown only while PR status is waiting to be authorized. It is the counterpart of the four
    /// above: they are hidden when there is nothing to open, this one is hidden when there is
    /// nothing to authorize.
    pub authenticate: gtk::MenuItem,
    /// Shown only while a newer release exists. Its label carries the version, so it is relabelled as
    /// well as shown and hidden.
    pub update: gtk::MenuItem,
    /// Shown only while GitHub reports an incident. Sibling of `authenticate` in every way except what
    /// it means: both are conditional, both are the counterpart of a mark on the icon, and because
    /// that mark is the *same* exclamation for both, this entry is what tells the two states apart.
    pub status: gtk::MenuItem,
    /// The rule above the body of the menu. Shown only while something sits above it — see the append
    /// order in `main`, and `applied_top_group` below for why its visibility is tracked separately.
    pub top_separator: gtk::SeparatorMenuItem,
}

#[cfg(target_os = "linux")]
impl MenuItems {
    /// The three PR-axis items, in `PrAxis::ALL` order — so per-axis code can loop instead of
    /// hand-repeating itself three times.
    fn pr_items(&self) -> [&gtk::MenuItem; 3] {
        [&self.reviews, &self.ready_to_merge, &self.changes_requested]
    }
}

#[cfg(target_os = "linux")]
pub fn start_notification_scheduler(
    indicator: libappindicator::AppIndicator,
    icons: crate::icons::IconSet<String>,
    menu_items: MenuItems,
    inputs: PollInputs,
    wake_rx: Receiver<Wake>,
    restart_tx: std::sync::mpsc::Sender<RestartPlan>,
) {
    // Use the glib that `gtk` itself was built against, so this timer is attached by the same
    // bindings that run the main loop.
    use gtk::glib;
    // `set_label` lives on an extension trait, and the prelude is the documented way in.
    use gtk::prelude::*;

    let (update_tx, update_rx) = std::sync::mpsc::channel::<Update>();
    // A second channel rather than a variant on `Update`: a restart is a one-off instruction, not part
    // of the per-poll rendering state, and mixing them would mean every poll carried an `Option` that is
    // `None` for the whole life of the process bar once.
    spawn_poll_thread(
        inputs,
        wake_rx,
        move |update| update_tx.send(update).is_ok(),
        move |plan| {
            // Two steps, because this runs on the update thread and GTK is main-thread-only. The plan
            // goes down the channel `main` reads after `gtk::main()` returns, and then the main loop is
            // asked to return at all — without that second half the plan would sit unread forever.
            //
            // `idle_add` rather than `idle_add_local`: this is a cross-thread post, so the closure must
            // be `Send`. It captures nothing, which is what makes that hold.
            let _ = restart_tx.send(plan);
            glib::idle_add(|| {
                gtk::main_quit();
                glib::ControlFlow::Break
            });
        },
    );

    // No `Arc<Mutex<_>>` here: `timeout_add_local` requires only `FnMut + 'static`, and this
    // closure runs on the GTK main thread, so the indicator can simply be owned by it.
    // (`glib::idle_add_once` would look tidier but demands `Send`, which `AppIndicator` is not.)
    let mut indicator = indicator;
    // One slot per PR axis at `PrAxis::index()` — one array instead of three separate bools so the
    // loop below does not have to hand-repeat itself. (A notifications slot used to sit at index 0;
    // see `state::pr_entry_visibility` for what removing it without moving every reader cost.)
    let mut applied: Option<[bool; 3]> = None;
    let mut applied_labels: [Option<String>; 3] = [None, None, None];
    // Tracked separately from `applied` because it is not one of the four signals but a replacement
    // for all of them, and because it drives a menu item the four do not.
    let mut applied_needs_auth: Option<bool> = None;
    let mut applied_status: Option<bool> = None;
    // Whether anything is currently in the top group. Its own memo rather than derived on the fly,
    // because the separator must be written only when it changes — some panels rebuild the whole menu
    // when any item does, which would fight a user who has it open.
    let mut applied_top_group: Option<bool> = None;
    // Likewise separate: an available update is a fifth independent signal drawn in a corner the other
    // four never touch, and it drives its own menu entry.
    let mut applied_update: Option<bool> = None;
    let mut applied_update_label: Option<String> = None;

    glib::timeout_add_local(UI_DRAIN_INTERVAL, move || {
        while let Ok(update) = update_rx.try_recv() {
            // `Unknown` on any axis deliberately leaves that part of the picture alone — a brief
            // failure should change the words, not make the icon flap. Only a confirmed answer
            // moves the image, so an unknown axis falls back to whatever is on screen.
            let current = applied.unwrap_or([false; 3]);
            let wanted = [
                update.icon.review_requested.as_confirmed().unwrap_or(current[0]),
                update.icon.ready_to_merge.as_confirmed().unwrap_or(current[1]),
                update.icon.changes_requested.as_confirmed().unwrap_or(current[2]),
            ];
            let needs_auth = update.icon.needs_auth;
            let update_available = update.icon.update_available;
            let status_degraded = update.icon.status_degraded;
            let exclamation = update.icon.shows_exclamation();

            // Checked before the four signals, because it overrides them: with no credential there
            // is nothing to draw a dot from. `wanted` is still computed and stored above, so the
            // moment authorization succeeds the icon can go straight back to the right variant
            // without waiting for a signal to change.
            if applied_needs_auth != Some(needs_auth) {
                menu_items.authenticate.set_visible(needs_auth);
                applied_needs_auth = Some(needs_auth);
                // Force the icon/menu block below to run: the variant it should show has just
                // changed even if none of the four signals did.
                applied = None;
            }

            // Its own memo rather than riding on `applied_needs_auth`, because the two are independent:
            // GitHub can be down while the credential is fine, and vice versa.
            if applied_status != Some(status_degraded) {
                menu_items.status.set_visible(status_degraded);
                applied_status = Some(status_degraded);
                // Same reason as above: this bit is part of which variant the icon shows.
                applied = None;
            }

            // A separator needs something on both sides. Only the top side is still in question: the
            // body — Authenticate, notifications, the PR entries — used to be able to empty out, leaving
            // this rule directly above the bottom one, which reads as a rendering fault rather than a
            // grouping. The PR-inbox entry appears exactly when the three PR entries do not, so the body
            // can no longer be empty and the old test for it was permanently true.
            let top_group = status_degraded || update_available;
            let separate = top_group;
            if applied_top_group != Some(separate) {
                menu_items.top_separator.set_visible(separate);
                applied_top_group = Some(separate);
            }

            if applied_update != Some(update_available) {
                menu_items.update.set_visible(update_available);
                applied_update = Some(update_available);
                // Same reason as above: the arrow is part of the variant, so the image has to be
                // re-applied even though none of the four signals moved.
                applied = None;
            }

            if applied != Some(wanted) {
                // One call, no branch. The exclamation used to select a whole separate icon because it
                // replaced the bars; now it is just another bit, so every mark composes with every other.
                indicator.set_icon(
                    icons
                        .get(wanted[0], wanted[1], wanted[2], update_available, exclamation)
                        .as_str(),
                );

                // An entry that opens an empty list is a dead end, so it is hidden. `wanted` says
                // it directly: the icon carries a signal exactly when that entry has somewhere to
                // go. Its treatment of `Unknown` carries over too, so a failed poll leaves the menu
                // as it was rather than hiding an entry we simply could not ask about.
                //
                // The three PR entries are hidden outright while waiting to authorize, because none
                // of them can have anything behind them.
                //
                // GTK has per-item visibility, so this is a flag rather than the remove-and-append
                // dance the Windows side needs. `show_all` is called once during setup and never
                // again, so nothing undoes these.
                // `needs_auth`, deliberately not `exclamation`: an outage hides the *bars* because the
                // icon has only one exclamation to give, but the counts behind these entries are the
                // last known good ones and the lists still open. Hiding them would take away working
                // links to punish GitHub for being slow.
                //
                // Indexed by axis on both sides, not zipped. This used to be `.zip(&wanted[1..])`,
                // skipping a notifications slot at index 0 — and when that slot was removed the slice
                // stayed, so every entry showed its *neighbour's* bit: the reviews entry lit up for an
                // approved PR with a bare label, and the changes entry never lit at all. Shipped that
                // way in 2.0.0 and 2.0.1. One index for both arrays cannot drift apart like that.
                let items = menu_items.pr_items();
                let shown = crate::state::pr_entry_visibility(wanted, needs_auth);
                for axis in PrAxis::ALL {
                    items[axis.index()].set_visible(shown[axis.index()]);
                }
                // Deliberately *not* gated on `needs_auth`: a plain URL needs no credential, which is
                // exactly what makes it worth keeping when the three that do need one are hidden.
                let pr_entries = wanted;
                menu_items.pr_inbox.set_visible(crate::state::shows_pr_inbox(pr_entries, needs_auth));

                applied = Some(wanted);
            }

            // Only on change: relabelling a menu item is cheap, but some panels rebuild the whole
            // menu when an item changes, which would fight a user who has it open.
            for (item, (label, applied_label)) in
                menu_items.pr_items().into_iter().zip(update.pr_labels.iter().zip(&mut applied_labels))
            {
                if applied_label.as_deref() != Some(label.as_str()) {
                    item.set_label(label);
                    *applied_label = Some(label.clone());
                }
            }

            // Carries the version, so it changes whenever a newer release appears. Same
            // only-on-change guard as the PR labels, for the same reason.
            if let Some(label) = update.update_label.as_deref()
                && applied_update_label.as_deref() != Some(label)
            {
                menu_items.update.set_label(label);
                applied_update_label = Some(label.to_string());
            }

            indicator.set_title(&update.tooltip);
            // No `set_label`. It used to put a "!" beside the icon whenever a signal was `Unknown`,
            // which worked — but only here: `set_label` is a StatusNotifierItem property, and neither
            // Windows' `Shell_NotifyIcon` nor macOS' `NSStatusItem` has anywhere to put it. So the one
            // platform with a panel label was the only platform that showed a failed poll without
            // hovering. That signal is now the exclamation in the composited icon, which reaches all
            // three — see `IconState::shows_exclamation`.
        }
        glib::ControlFlow::Continue
    });
}

// ─── Windows and macOS ────────────────────────────────────────────────────────

/// The custom event type sent into the winit event loop from outside it.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub enum TrayEvent {
    /// The polling thread has a new notification state to show.
    Update(Update),
    /// A tray menu entry was chosen.
    ///
    /// macOS only. There, menu events arrive through a `muda` callback rather than the polled
    /// channel, and a callback has no way to reach the event loop except as a user event. On
    /// Windows the polled channel is used instead, so this variant would never be constructed.
    #[cfg(target_os = "macos")]
    MenuClick(tray_icon::menu::MenuId),
    /// The tray icon itself was clicked. macOS only, for the same reason.
    #[cfg(target_os = "macos")]
    IconClick,
    /// An update has been installed; hand over to it.
    ///
    /// Arrives from the update thread. Handled on the UI thread because the tray icon has to be dropped
    /// before this process exits, or the shell is left holding a dead icon.
    Restart(RestartPlan),
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn start_notification_scheduler(
    inputs: PollInputs,
    wake_rx: Receiver<Wake>,
    proxy: winit::event_loop::EventLoopProxy<TrayEvent>,
) {
    let restart_proxy = proxy.clone();
    spawn_poll_thread(
        inputs,
        wake_rx,
        move |update| proxy.send_event(TrayEvent::Update(update)).is_ok(),
        move |plan| {
            // Same asymmetry as everywhere else in this module: Linux uses a channel, these two use the
            // event loop proxy. The tray has to come down on the UI thread before the process ends.
            let _ = restart_proxy.send_event(TrayEvent::Restart(plan));
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEADY: Duration = Duration::from_secs(60);

    fn burst_of(secs: &[u64]) -> VecDeque<Duration> {
        secs.iter().map(|s| Duration::from_secs(*s)).collect()
    }

    #[test]
    fn a_queued_burst_drains_in_order_then_returns_to_normal_pacing() {
        let mut burst = burst_of(&[5, 5, 5]);

        for _ in 0..3 {
            assert_eq!(next_pace(&mut burst, false, STEADY), Pace::Burst(Duration::from_secs(5)));
        }
        assert_eq!(next_pace(&mut burst, false, STEADY), Pace::Steady(STEADY));
    }

    #[test]
    fn the_widening_burst_keeps_its_order() {
        // Order matters for REFRESH_BURST specifically, since its whole point is widening gaps.
        let mut burst: VecDeque<Duration> = REFRESH_BURST.iter().copied().collect();
        for expected in REFRESH_BURST {
            assert_eq!(next_pace(&mut burst, false, STEADY), Pace::Burst(expected));
        }
        assert_eq!(next_pace(&mut burst, false, STEADY), Pace::Steady(STEADY));
    }

    #[test]
    fn an_empty_burst_is_normal_pacing() {
        let mut burst = VecDeque::new();
        assert_eq!(next_pace(&mut burst, false, STEADY), Pace::Steady(STEADY));
    }

    /// The regression this exists for. A burst used to win unconditionally, so a `retry-after` was
    /// read, stored, and then thrown away: we would poll again in 5s having just been told to wait
    /// a minute, which is how a short secondary limit turns into a long one.
    #[test]
    fn a_demanded_wait_beats_a_burst() {
        let mut burst = burst_of(&[5, 5, 5]);
        let demanded = Duration::from_secs(600);

        assert_eq!(next_pace(&mut burst, true, demanded), Pace::Steady(demanded));
    }

    /// Abandoned, not merely deferred by one cycle. A burst is chasing a change the user just
    /// caused; ten minutes later that moment is gone and the burst would be pure traffic.
    #[test]
    fn a_demanded_wait_discards_the_rest_of_the_burst() {
        let mut burst = burst_of(&[5, 5, 5]);

        next_pace(&mut burst, true, Duration::from_secs(600));
        assert!(burst.is_empty(), "the burst must not resume after the limit lifts");

        // …and the cycle after really is back to normal pacing.
        assert_eq!(next_pace(&mut burst, false, STEADY), Pace::Steady(STEADY));
    }

    #[test]
    fn a_demanded_wait_with_no_burst_changes_nothing() {
        let mut burst = VecDeque::new();
        let demanded = Duration::from_secs(90);
        assert_eq!(next_pace(&mut burst, true, demanded), Pace::Steady(demanded));
    }

    /// The two bursts are deliberately different shapes, and a copy-paste that merged them would
    /// silently undo the reasoning in both.
    #[test]
    fn the_two_bursts_stay_distinct() {
        assert!(
            MENU_BURST.iter().all(|d| *d == MENU_BURST[0]),
            "the menu burst is evenly spaced: there is no server-side lag to wait out"
        );
        assert!(
            REFRESH_BURST.windows(2).all(|w| w[1] > w[0]),
            "the page-open burst widens: it is waiting out GitHub registering a read"
        );
        assert_eq!(
            MENU_BURST.iter().sum::<Duration>(),
            Duration::from_secs(15),
            "the menu burst covers 15s, as documented"
        );
    }


    // ─── The loop through the seam ──────────────────────────────────────────
    //
    // `PortalRun` is the loop body minus the channel, so a `FakePortal` can drive it end to end
    // without a network. What these hold is the *contract* between the loop and a portal: which
    // axes get asked, what a rejected credential leads to, and that a portal with no status page
    // leaves the outage mark alone.

    use crate::portal::fake::FakePortal;
    use crate::portal::types::{PollResponse, PollResult};
    use crate::portal::{CredentialState, Health, PollOutcome};

    fn response(result: PollResult) -> PollResponse {
        PollResponse { result, poll_interval: None }
    }

    fn fresh(count: u32) -> PollResponse {
        response(PollResult::Fresh { present: count > 0, count: Some(count), prs: Some(Vec::new()) })
    }

    fn ready(fake: FakePortal, pr_enabled: [bool; 3]) -> PortalRun {
        PortalRun::new(Box::new(fake), CredentialState::Ready, pr_enabled)
    }

    /// The portal is told exactly the axes in play and nothing else: an axis the user switched
    /// off must cost no request, and that decision is the loop's, not the adapter's.
    #[test]
    fn a_run_asks_only_for_the_axes_in_play() {
        let fake = FakePortal::default()
            .script(PollOutcome { axes: [Some(fresh(2)), None, Some(fresh(0))] });
        let log = fake.log();
        let mut run = ready(fake, [true, false, true]);
        run.cycle();
        assert_eq!(log.lock().unwrap().asked, vec![[true, false, true]]);
        assert_eq!(run.state.confirmed_count(PrAxis::ReviewRequested), Some(2));
        assert!(!run.state.pr_in_play(PrAxis::ReadyToMerge));
    }

    /// Waiting for a sign-in means no poll at all, an exclamation, and the menu item. The click
    /// runs the portal's own flow and the run comes alive on the spot.
    #[test]
    fn a_run_waiting_for_sign_in_polls_nothing_until_the_menu_click() {
        let fake = FakePortal::default().script(PollOutcome { axes: [Some(fresh(1)), None, None] });
        let log = fake.log();
        let mut run = PortalRun::new(Box::new(fake), CredentialState::NeedsAuth, [true, false, false]);
        run.cycle();
        assert!(log.lock().unwrap().asked.is_empty(), "nothing to ask with");
        assert!(run.state.icon().needs_auth);

        run.authenticate(&crate::portal::fake::Silent);
        assert_eq!(log.lock().unwrap().authenticate_calls, 1);
        assert!(!run.state.icon().needs_auth);
        run.cycle();
        assert_eq!(log.lock().unwrap().asked, vec![[true, false, false]]);
        assert_eq!(run.state.confirmed_count(PrAxis::ReviewRequested), Some(1));
    }

    /// "Signed in, nothing to see" is a dead end the user cannot click away, so it is worded as a
    /// reason on every axis rather than as an exclamation, both at startup and after a sign-in
    /// that ends there.
    #[test]
    fn an_off_portal_explains_itself_and_never_polls() {
        let fake = FakePortal::default();
        let log = fake.log();
        let mut run = PortalRun::new(Box::new(fake), CredentialState::Off("nothing installed".to_string()), [true; 3]);
        run.cycle();
        assert!(log.lock().unwrap().asked.is_empty());
        assert!(!run.state.icon().needs_auth);
        assert!(run.state.tooltip_lines().iter().any(|l| l == "nothing installed"), "{:?}", run.state.tooltip_lines());

        let fake = FakePortal {
            after_sign_in: CredentialState::Off("still nothing".to_string()),
            ..FakePortal::default()
        };
        let mut run = PortalRun::new(Box::new(fake), CredentialState::NeedsAuth, [true; 3]);
        run.authenticate(&crate::portal::fake::Silent);
        assert!(!run.live);
        assert!(!run.state.icon().needs_auth, "the exclamation comes down: another click cannot help");
        assert!(run.state.tooltip_lines().iter().any(|l| l == "still nothing"));
    }

    /// A rejected credential is renewed silently, once, and the loop is told to retry at once. A
    /// second rejection inside `MIN_REAUTH_INTERVAL` is not retried: that gate is what keeps a
    /// credential the portal keeps rejecting from becoming a storm.
    #[test]
    fn a_rejected_credential_is_renewed_once_per_interval() {
        let fake = FakePortal::default()
            .script(PollOutcome { axes: [Some(response(PollResult::Unauthorized)), None, None] })
            .script(PollOutcome { axes: [Some(response(PollResult::Unauthorized)), None, None] });
        let log = fake.log();
        let mut run = ready(fake, [true, false, false]);

        run.cycle();
        assert!(run.recover_credential(), "a renewed credential is worth retrying with at once");
        assert_eq!(log.lock().unwrap().reauthenticate_calls, 1);
        assert!(run.live);

        run.cycle();
        assert!(!run.recover_credential(), "inside the interval: no second attempt");
        assert_eq!(log.lock().unwrap().reauthenticate_calls, 1);
    }

    /// When the portal says only the user can help, the run stops polling and asks for the click
    /// rather than opening a browser unannounced.
    #[test]
    fn a_renewal_the_portal_cannot_do_hands_over_to_the_menu() {
        let mut fake = FakePortal::default()
            .script(PollOutcome { axes: [Some(response(PollResult::Unauthorized)), None, None] });
        fake.renewal = Err(crate::portal::AuthError::AuthorizationRequired);
        let log = fake.log();
        let mut run = ready(fake, [true, false, false]);
        run.cycle();
        assert!(!run.recover_credential());
        assert!(!run.live);
        assert!(run.state.icon().needs_auth);
        run.cycle();
        assert_eq!(log.lock().unwrap().asked.len(), 1, "no further poll while waiting for the click");
    }

    /// Expiry known in advance is renewed before a poll ever fails.
    #[test]
    fn an_expiring_credential_is_renewed_before_it_is_rejected() {
        let fake = FakePortal { refresh_due: true, ..FakePortal::default() };
        let log = fake.log();
        let mut run = ready(fake, [true, false, false]);
        run.cycle();
        assert!(run.recover_credential());
        assert_eq!(log.lock().unwrap().reauthenticate_calls, 1);
    }

    /// `None` from `health` is "this portal has no status page", not "fine" and not "unknown":
    /// the mark is left exactly as it was. A verdict, when there is one, moves it both ways.
    #[test]
    fn a_portal_without_a_status_page_never_touches_the_outage_mark() {
        let mut run = ready(FakePortal::default(), [true, false, false]);
        run.cycle();
        assert!(!run.state.icon().status_degraded);

        let fake = FakePortal {
            health: Some(Ok(Health::Degraded { description: "Wobbly".to_string() })),
            ..FakePortal::default()
        };
        let mut run = ready(fake, [true, false, false]);
        run.cycle();
        assert!(run.state.icon().status_degraded);
        assert!(run.state.tooltip_lines()[0].contains("Wobbly"));
    }

    /// The settings page reads the sign-in state off the snapshot, so every transition the loop
    /// makes has to land there: waiting, signed in, and the dead end.
    #[test]
    fn a_run_reports_how_its_sign_in_stands() {
        let mut run = PortalRun::new(Box::new(FakePortal::default()), CredentialState::NeedsAuth, [true; 3]);
        assert_eq!(run.auth_status(), AuthStatus::NotSignedIn);
        run.signing_in = true;
        assert_eq!(run.auth_status(), AuthStatus::SigningIn(None), "a flow in flight is neither");
        run.signing_in = false;
        run.authenticate(&crate::portal::fake::Silent);
        assert_eq!(run.auth_status(), AuthStatus::SignedIn);
        assert_eq!(snapshot_of(std::slice::from_ref(&run))[0].auth, AuthStatus::SignedIn);

        let mut fake = FakePortal::default().script(PollOutcome {
            axes: [Some(response(PollResult::Unauthorized)), None, None],
        });
        fake.renewal = Err(crate::portal::AuthError::AuthorizationRequired);
        let mut run = ready(fake, [true, false, false]);
        run.cycle();
        run.recover_credential();
        assert_eq!(run.auth_status(), AuthStatus::NotSignedIn, "a rejected, unrenewable credential asks again");

        let run = PortalRun::new(Box::new(FakePortal::default()), CredentialState::Off("nothing installed".to_string()), [true; 3]);
        assert_eq!(run.auth_status(), AuthStatus::Off("nothing installed".to_string()));
    }

    /// Signing out puts the run where a fresh install starts: no polls, exclamation up, waiting for
    /// the click. The same PR can then be news again after the next sign-in, which is right.
    #[test]
    fn signing_out_returns_the_run_to_waiting_for_a_sign_in() {
        let fake = FakePortal::default().script(PollOutcome { axes: [Some(fresh(1)), None, None] });
        let log = fake.log();
        let mut run = ready(fake, [true, false, false]);
        run.cycle();
        assert_eq!(run.auth_status(), AuthStatus::SignedIn);

        run.sign_out();
        assert_eq!(log.lock().unwrap().sign_out_calls, 1);
        assert_eq!(run.auth_status(), AuthStatus::NotSignedIn);
        assert!(run.state.icon().needs_auth);
        run.cycle();
        assert_eq!(log.lock().unwrap().asked.len(), 1, "no poll while waiting for a sign-in");
    }

    /// A cancel is only for the sign-in that is running, and asking when none is running is a
    /// harmless no. The flow reads the flag through `SignInProgress::cancelled` between polls.
    #[test]
    fn cancel_reaches_only_the_running_sign_in() {
        let github = PortalId("github".to_string());
        let gitlab = PortalId("gitlab".to_string());
        *SIGN_IN.lock().unwrap() = None;
        assert!(!cancel_sign_in(&github), "nothing running: nothing to cancel");

        *SIGN_IN.lock().unwrap() = Some(SignIn { portal: github.clone(), cancel_requested: false });
        let progress = PageProgress { portal: github.clone(), name: "GitHub".to_string() };
        let other = PageProgress { portal: gitlab.clone(), name: "GitLab".to_string() };
        assert!(!progress.cancelled());
        assert!(!cancel_sign_in(&gitlab), "another portal's button does not stop this flow");
        assert!(cancel_sign_in(&github));
        assert!(progress.cancelled());
        assert!(!other.cancelled());
        *SIGN_IN.lock().unwrap() = None;
    }

    /// The snapshot keeps every portal's list apart and in configuration order, so the page can
    /// say where a card came from; an axis nobody has confirmed is `None` for that portal alone.
    #[test]
    fn the_snapshot_keeps_each_portals_confirmed_entries_apart() {
        use crate::portal::types::PrEntry;
        let listed = |urls: &[&str]| {
            response(PollResult::Fresh {
                present: !urls.is_empty(),
                count: Some(urls.len() as u32),
                prs: Some(urls.iter().map(|u| PrEntry::stub(u)).collect()),
            })
        };
        let a = FakePortal::named("A").script(PollOutcome { axes: [Some(listed(&["https://a.example/1"])), None, None] });
        let b = FakePortal::named("B").script(PollOutcome { axes: [Some(listed(&["https://b.example/1", "https://b.example/2"])), None, None] });
        let mut runs = vec![ready(a, [true; 3]), ready(b, [true; 3])];
        for run in &mut runs {
            run.cycle();
        }
        let snapshot = snapshot_of(&runs);
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].info.display_name, "A");
        assert_eq!(snapshot[0].axes[0].as_ref().map(Vec::len), Some(1));
        assert_eq!(snapshot[1].axes[0].as_ref().map(Vec::len), Some(2));
        assert!(snapshot[1].axes[1].is_none(), "nobody answered this axis yet");
    }
}
