//! Platform-independent pull-request state machine.
//!
//! The original icon-correctness bug reduced to one thing: the code had a `bool` to describe
//! three situations — present, absent, and "the last poll failed so I genuinely do not know".
//! With no way to say the third, every failure was reported as the second. `Presence` makes the
//! third case representable.
//!
//! There are three *independent* signals, the `PrAxis` bars: review-requested (red), approved
//! (green) and work-required (amber). They come from different queries against different rules, so
//! they fail independently — which is why each gets its own `Track` rather than sharing one failure
//! counter.
//!
//! A fourth used to sit alongside them: unread GitHub notifications, drawn as a blue glyph and
//! polled with a second, user-registered OAuth credential. It was removed in 2.0.0 along with the
//! credential, the `/notifications` endpoint and the blue asset.
//!
//! Nothing here does I/O, so all of it is testable.

use crate::infoln;
use crate::portal::types::{PollResponse, PollResult, PrEntry};
use std::time::Duration;

// ── Values GitHub never sends us, so they are ours to choose ──────────────────
// Kept as named constants precisely so they stay visible as judgement calls rather than
// hiding as magic numbers next to the values GitHub dictates.

/// Floor on poll frequency. GitHub's advertised `x-poll-interval` can raise this but never
/// lower it. 60s matches what the README promises.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Consecutive failures tolerated before an axis stops asserting a stale value. Absorbs a brief
/// network blip without letting a real outage keep looking healthy.
pub const FAILURES_BEFORE_UNKNOWN: u32 = 3;

/// Ceiling on exponential backoff, so a long outage does not stretch retries into hours.
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// Successive waits after the user opens a GitHub page, giving GitHub — and the user — time to
/// register that things were read. Cumulatively ~5s, ~20s, ~45s after the click.
pub const REFRESH_BURST: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(25),
];

/// Successive waits after the user opens the tray menu: every 5s, for 15s.
///
/// Evenly spaced and shorter than `REFRESH_BURST` because the two are waiting for different things.
/// That one widens to wait out GitHub's lag in registering a read; this one is answering someone who
/// is looking at the menu right now, so there is nothing to wait out and no reason to keep going.
pub const MENU_BURST: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(5),
    Duration::from_secs(5),
];

/// Shell_NotifyIcon's `szTip` holds 128 UTF-16 units including the terminator, so tooltips are
/// clamped well below that.
const MAX_TOOLTIP_CHARS: usize = 110;

/// Whether a signal is present, absent, or genuinely unknown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    /// Confirmed present.
    Yes,
    /// Confirmed absent.
    No,
    /// Not confirmed either way. The one state the original code could not express.
    Unknown,
}

impl Presence {
    /// How the icon should render this: only a *confirmed* answer moves the picture.
    /// `Unknown` returns `None`, meaning "leave the image alone and explain in the tooltip".
    pub fn as_confirmed(self) -> Option<bool> {
        match self {
            Presence::Yes => Some(true),
            Presence::No => Some(false),
            Presence::Unknown => None,
        }
    }
}

/// What the UI should draw: one presence per dot/tint, matching `icons::IconSet`'s four
/// independent signals, plus the one flag that overrides all of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IconState {
    /// PR status is waiting for the user to authorize it, so `icons::IconSet::needs_auth` is drawn
    /// and the four fields below are not consulted at all.
    ///
    /// It doubles as the "show the Authenticate menu item" signal, so the icon and the menu cannot
    /// disagree about whether a click is being waited on.
    pub needs_auth: bool,
    /// GitHub says it is having a major or critical incident.
    ///
    /// Draws the **same** exclamation as `needs_auth` — a deliberate choice, not an oversight. The two
    /// are indistinguishable on the icon and separated only by the tooltip and the menu entry, which is
    /// the trade accepted for not spending another mark on the right-hand side.
    ///
    /// Kept as its own field rather than folded into `needs_auth` because the two drive *different*
    /// menu entries: an outage must never offer to re-authorize a credential that is working, and a
    /// missing credential must never point at the status page. [`IconState::shows_exclamation`] is
    /// where the two come back together.
    pub status_degraded: bool,
    /// A signal we asked about and could not get an answer for.
    ///
    /// **Not** simply "some `Presence` is `Unknown`". A freshly built `Track` is `Unknown` too, because
    /// nothing has been polled yet — reading the mark off `Presence` alone put a red exclamation on the
    /// icon for the first second of every launch. This is `Unknown` *and* a failure recorded against it,
    /// which is the difference between "it went wrong" and "we have not asked".
    pub signals_unsure: bool,
    /// A newer release exists, so the up-arrow is drawn in the top-left corner.
    ///
    /// Independent of every other field here, including `needs_auth`: an available update says nothing
    /// about your PRs or your credentials, and the arrow sits in a corner nothing else uses. It also
    /// doubles as the "show the Install update menu item" signal, for the same reason `needs_auth`
    /// does — one source of truth means the icon and the menu cannot contradict each other.
    pub update_available: bool,
    pub review_requested: Presence,
    pub ready_to_merge: Presence,
    pub changes_requested: Presence,
}

impl IconState {
    /// Whether the exclamation replaces the bars.
    ///
    /// The one definition of that, because both platforms pick the icon independently and a copy each
    /// would eventually disagree — which on screen means an icon that says everything is fine while the
    /// menu says GitHub is down.
    ///
    /// **Three causes, one mark.** No credential, GitHub having an incident, or an answer we could not
    /// get. They are separate fields because they drive different menu entries, and they collapse here
    /// because the icon has one exclamation to give — see the module docs on `icons` for why there is no
    /// room for a second.
    ///
    /// The third cause costs the bars, and that is the deliberate trade: one failed poll
    /// hides three PR bars whose own polls succeeded. It buys the thing a per-platform label could not —
    /// the same visible signal on Linux, Windows and macOS, since only the composited image reaches all
    /// three.
    pub fn shows_exclamation(&self) -> bool {
        self.needs_auth || self.status_degraded || self.any_unsure()
    }

    /// Whether any signal was asked about and could not be answered.
    pub fn any_unsure(&self) -> bool {
        self.signals_unsure
    }
}

/// Label of the fallback tray menu item that opens GitHub's own pull-request inbox.
pub const PR_INBOX_MENU_LABEL: &str = "Open PR inbox";

/// Where that entry goes. GitHub's own inbox view, not a search of ours — the whole point of this one
/// is that it needs no query, no count and no credential to be worth clicking.
pub const PR_INBOX_URL: &str = "https://github.com/pulls/inbox";

/// Whether the fallback inbox entry is shown: exactly when no *specific* PR entry is.
///
/// Lives here rather than in either platform's menu code for the same reason
/// [`IconState::shows_exclamation`] does. Both platforms decide menu visibility independently, and a
/// copy each drifts — which on screen means the entry appearing next to the three it exists to stand in
/// for, or vanishing on the one day the menu has nothing else to offer.
///
/// Takes resolved `bool`s rather than `Presence`, because by the time either platform asks, it has
/// already folded `Unknown` into "whatever is on screen" — and that fold is what should govern this
/// too. An axis we merely lost track of keeps its entry, so this entry stays away.
///
/// `needs_auth` counts as showing nothing, because it hides all three PR entries whatever their counts.
/// That is the case this matters most in: a plain URL needs no credential, so it is the one PR link that
/// still works while the app is waiting to be authorized.
/// Which of the three PR menu entries are shown, indexed by `PrAxis::index`.
///
/// Trivial on purpose — an entry is visible when its axis is lit and no credential is missing — and
/// shared by both platforms for the reason `shows_pr_inbox` is. The value of having it as a function
/// is not the logic but the **alignment**: the result is indexed by axis, so the caller indexes the
/// menu items by the same axis and nothing is zipped. The Linux loop used to zip the items against
/// `wanted[1..]`, a slice that skipped a notifications slot at index 0; when that slot was removed the
/// slice stayed, and every entry showed its neighbour's bit. That shipped in 2.0.0 and 2.0.1.
pub fn pr_entry_visibility(lit: [bool; 3], needs_auth: bool) -> [bool; 3] {
    lit.map(|on| !needs_auth && on)
}

pub fn shows_pr_inbox(pr_entries: [bool; 3], needs_auth: bool) -> bool {
    needs_auth || !pr_entries.iter().any(|&on| on)
}

/// Base text of the tray menu item that opens the review list.
///
/// Lives here, next to the code that appends the count to it, so the two cannot drift apart.
pub const REVIEWS_MENU_LABEL: &str = "Open Requested Reviews";

/// Text of the tray menu item that starts a portal's sign-in.
///
/// Shown only while `PollState::pr_needs_auth` holds. Lives here with the other menu wording so the
/// platform UI code in `main.rs` and `scheduler.rs` cannot spell it two different ways. Takes the
/// portal's display name, so two portals waiting to be authorized get two items a user can tell
/// apart; for one GitHub portal it reads exactly as it always did.
pub fn authenticate_menu_label(portal: &str) -> String {
    format!("Authenticate {portal} PR Status")
}

/// Hover text while PR status is waiting to be authorized. Says what is wrong *and* where the fix
/// is, because the red exclamation on its own only says that something is.
const PR_NEEDS_AUTH_TOOLTIP: &str = "PR status: not authorized yet. Use the menu to authorize.";

/// Base text of the tray menu item that installs an available update.
///
/// The version is appended by `update_menu_label`, the same way `pr_menu_label` appends a count: the
/// icon can only say *that* something is available, so the number goes where there is room for it.
pub const UPDATE_MENU_LABEL: &str = "Install update";

/// Text of the tray menu item that opens GitHub's status page.
///
/// Shown only while `PollState::status_degraded` holds. The wording is the author's, and it is doing a
/// job: the icon shows the same exclamation as a missing credential, so this entry is what tells the two
/// apart at a glance.
pub const STATUS_MENU_LABEL: &str = "GitHub is githubing again, check status";

/// Text of the tray's Settings submenu.
///
/// Always present, unlike every other entry here: it needs no credential, no poll and no available
/// update, and it is the one thing that still works when everything else is switched off.
///
/// A submenu since 1.16.0. It used to be a single entry that opened `config.txt`, which is still in
/// there — the two settings people actually change now have a checkbox, and the file keeps the ones
/// that are a list or a level rather than a switch.
pub const SETTINGS_MENU_LABEL: &str = "Settings";

/// Text of the Settings entry that opens `config.txt` itself.
///
/// Named "file" rather than "Open Settings" now that it sits *inside* Settings, where the old wording
/// would have read as a second, different settings screen.
pub const SETTINGS_FILE_MENU_LABEL: &str = "Open settings file";

/// Text of the Settings entry that opens this app's own repository on GitHub.
///
/// Named after the app rather than "Open repository", which inside a tool full of GitHub links would
/// read as one of *your* repositories. The URL itself is `update::REPOSITORY_URL`, next to the
/// repository the updater installs from — the same split as [`STATUS_MENU_LABEL`] and
/// `github_status::STATUS_PAGE_URL`.
pub const REPOSITORY_MENU_LABEL: &str = "Open GitHoot on GitHub";

/// Text of the Settings entry that opens the configuration page.
pub const SETTINGS_PAGE_MENU_LABEL: &str = "Open settings page";

/// Text of the Settings checkbox for counting Copilot's unresolved comments as work.
pub const COPILOT_MENU_LABEL: &str = "Count Copilot comments as work";

/// Text of the Settings checkbox for the hoot.
///
/// Says what it does rather than naming the `sound` key: the file is one click away for anyone who
/// wants the key, and "sound" alone would suggest the app makes noise for more than this one thing.
pub const HOOT_MENU_LABEL: &str = "Hoot on new pull requests";

/// Text of the Settings checkbox for starting with the session.
///
/// The one entry in the whole menu backed by the operating system rather than by `config.txt` — see
/// `autostart`, and `docs/startup.md` for why that stays true.
pub const AUTOSTART_MENU_LABEL: &str = "Start at sign-in";

/// One of the three independent PR-search signals.
///
/// All three share one credential and one query string apiece, and all three land in the same
/// `Track` machinery, and since 1.17.0 all three read the same GraphQL document. What separates them
/// is the rule applied to the hits: review-requested keeps every one, because its query is a real
/// filter; the other two narrow client-side, because Search can express neither "somebody approved
/// this" in a repository that requires no reviews nor "still on me rather than handed back".
/// `scheduler::pr_judge` is the one place that mapping lives.
///
/// The middle axis means **approved**, not mergeable. It used to mean both: the count dropped any
/// approved pull request whose checks were red, on the reasoning that a green bar over red CI claims
/// something false. The cost of that was worse — one failing check hid the fact that anyone had
/// approved the work at all, which is the news the bar exists to deliver. So the bar answers "did
/// somebody approve it", the page behind the menu entry answers "can it actually merge", and the two
/// no longer disagree about what they are counting.
///
/// `ALL` fixes the order used both for tooltip-detail priority and, by convention, for which slot and
/// colour `icons::IconSet` assigns each bar, top to bottom: review-requested (red), approved
/// (green), changes-requested (amber).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrAxis {
    ReviewRequested,
    /// Means **approved**, not mergeable — see the note above.
    ReadyToMerge,
    /// Means **work required from you**: a reviewer's objection still standing, *or* a merge conflict
    /// with someone waiting to review. The variant keeps its narrower original name for the same
    /// reason `ReadyToMerge` does, and for the same reason the `changesRequested` config key does:
    /// renaming it touches forty-odd sites and one user-visible setting for no behaviour at all. Every
    /// string a user actually reads says "work required".
    ChangesRequested,
}

impl PrAxis {
    pub const ALL: [PrAxis; 3] =
        [PrAxis::ReviewRequested, PrAxis::ReadyToMerge, PrAxis::ChangesRequested];

    /// Position within `ALL`, and the index this axis uses in any `[T; 3]` array associated with
    /// the three PR axes (`Update::pr_labels` in `scheduler`, the platform UI's per-axis
    /// "applied" bookkeeping in `main`) — public so those call sites don't need their own copy of
    /// this mapping.
    pub fn index(self) -> usize {
        match self {
            PrAxis::ReviewRequested => 0,
            PrAxis::ReadyToMerge => 1,
            PrAxis::ChangesRequested => 2,
        }
    }

    /// This axis's path segment on GitHoot's own PR page.
    ///
    /// Lives beside `index()` and `menu_label()` so the route and the axis have one mapping with one
    /// test, the same treatment `scheduler::pr_query` and `pr_judge` get. Lowercase ASCII and hyphens
    /// only, which is what lets `serve` compare the raw request path without percent-decoding it.
    pub fn slug(self) -> &'static str {
        match self {
            PrAxis::ReviewRequested => "requested-reviews",
            PrAxis::ReadyToMerge => "approved",
            PrAxis::ChangesRequested => "work-required",
        }
    }

    /// The axis a path segment names, or `None` for anything else — which `serve` answers with a 404.
    pub fn from_slug(slug: &str) -> Option<PrAxis> {
        PrAxis::ALL.into_iter().find(|axis| axis.slug() == slug)
    }

    /// Base text for this axis's tray menu item, before any count is appended.
    pub fn menu_label(self) -> &'static str {
        match self {
            // Reuses the existing constant rather than duplicating the string, so the two can
            // never drift apart.
            PrAxis::ReviewRequested => REVIEWS_MENU_LABEL,
            PrAxis::ReadyToMerge => "Open Approved PRs",
            PrAxis::ChangesRequested => "Open Work Required",
        }
    }

    /// Tooltip phrase when this axis is confirmed present.
    fn tooltip_yes(self, count: Option<u32>) -> String {
        match (self, count) {
            (PrAxis::ReviewRequested, Some(n)) => format!("{n} PR(s) awaiting your review"),
            (PrAxis::ReviewRequested, None) => "PRs awaiting your review".to_string(),
            (PrAxis::ReadyToMerge, Some(n)) => format!("{n} PR(s) approved"),
            (PrAxis::ReadyToMerge, None) => "PRs approved".to_string(),
            (PrAxis::ChangesRequested, Some(n)) => format!("{n} PR(s) needing your work"),
            (PrAxis::ChangesRequested, None) => "PRs needing your work".to_string(),
        }
    }

    /// Tooltip phrase when this axis is confirmed absent.
    fn tooltip_no(self) -> &'static str {
        match self {
            PrAxis::ReviewRequested => "No reviews requested",
            PrAxis::ReadyToMerge => "No approvals yet",
            PrAxis::ChangesRequested => "Nothing needing your work",
        }
    }

    /// Tooltip phrase when this axis's state is unknown.
    fn tooltip_unknown(self) -> &'static str {
        match self {
            PrAxis::ReviewRequested => "Review state unknown",
            PrAxis::ReadyToMerge => "Approval state unknown",
            PrAxis::ChangesRequested => "Changes-requested state unknown",
        }
    }
}

/// Per-signal state: last known value, conditional-request tag, and failure streak.
/// How many pull requests an axis remembers having told the user about.
///
/// Twice `github::SEARCH_HITS_CAP`, and the factor is the whole argument: an axis can show at most a
/// cap's worth at once, so a key must sit absent while a *further* hundred distinct pull requests
/// churn past before it is dropped. That is far more real activity than the eventual-consistency blip
/// this ledger exists to absorb.
///
/// Bounded by size rather than by age on purpose. Eviction is the only thing that can bring the false
/// hoot back: a key forgotten while the index was hiding its pull request would read as new on its
/// return and sound. Nothing else needs the key to age out — a returning id is simply not news. So the bound is set where it cannot plausibly be reached by a blip, and
/// entries are dropped least-recently-present first.
const LEDGER_CAP: usize = 200;

/// What the ledger remembers about one pull request: that it was here, when it was last here, and
/// what its `updatedAt` read then.
///
/// The timestamp is consulted in exactly one place: when a known id comes back after an absence. It
/// is *not* compared while the pull request sits on the list. An earlier ledger carried the activity
/// timestamp, the conflict flag and the open Copilot count, and hooted when any of them moved — which
/// meant a comment, a label or a push on a listed pull request sounded the tray with nothing visible
/// changing (2.0.0 to 2.0.2). That is not what this is. See `Track::note`.
#[derive(Clone, Debug)]
struct Seen {
    /// Which poll last saw it, for eviction order and for spotting an absence.
    last_seen: u64,
    /// GitHub's `updatedAt` as of `last_seen`, to tell a return apart from a blip.
    updated_at: Option<String>,
}

struct Track {
    value: Presence,
    failures: u32,
    detail: Option<String>,
    count: Option<u32>,
    needs_reauth: bool,
    /// The count has just gone up, and nobody has been told yet.
    ///
    /// Latched rather than derived, because the only caller reads it once per cycle and the edge it
    /// describes exists only between two `apply` calls. See `PollState::take_pr_arrivals` for which
    /// transitions count.
    arrived: bool,
    /// Pull requests this axis has already hooted for, keyed by `PrEntry::key`.
    ///
    /// The hoot used to be "the count went up", which is only a proxy for the thing it means. GitHub's
    /// search index is eventually consistent and will drop a pull request from one answer and return
    /// it in the next, so `3 -> 2 -> 3` sounded for a pull request the user had already been told
    /// about. Identity answers the real question, and it closes a gap in the other direction too: one
    /// pull request leaving and another arriving in the same cycle left the count flat and said
    /// nothing at all.
    seen: std::collections::HashMap<String, Seen>,
    /// Counts successful answers, only to order `seen` for eviction.
    polls: u64,
    /// The exact items `count` counted, for the axes whose poll reads its hits one by one
    /// (see `github::PollResult::Fresh::prs`). Held through failures for the same reason `value`
    /// is: a blip must not turn a click that opened the right PRs into one that opens a search
    /// page showing different ones.
    prs: Option<Vec<PrEntry>>,
    /// Whether this track has ever had a confirmed answer, of either kind.
    ///
    /// Exists to tell the two `Unknown`s apart. `value == Unknown` covers both "we have not asked yet"
    /// (a launch, which *is* news when it finds something) and "we asked and lost track" (a failure
    /// streak, which is not). Nothing else can distinguish them, because both look identical in
    /// `value`, and `failures` is reset by the very response that would need to consult it.
    ever_confirmed: bool,
}

impl Track {
    /// Files this poll's pull requests and answers whether any of them is news.
    ///
    /// News is **an id this axis has not seen before**. That is the whole rule, and it is deliberately
    /// blind to everything else about a pull request: a comment, a push, a new review, a conflict
    /// appearing or a Copilot thread opening on a pull request *already on the list* changes nothing
    /// the user can see on the tray, so it makes no sound. A pull request that appears on the list
    /// *because* of one of those — a conflict that puts it on the amber bar — is a new id here, and
    /// hoots for that reason alone.
    ///
    /// It also answers the case a count could never see: one pull request leaving and another arriving
    /// in the same poll leaves the number flat, and the arriving id is still new.
    ///
    /// A known id returning after an absence is judged by its `updatedAt`. GitHub's search index
    /// drops a pull request from one answer and re-serves it in the next *untouched*: same id, same
    /// timestamp, and that is silent. The same id back with a *newer* timestamp left the list for a
    /// reason and came back for a reason — changes requested a second time on a pull request whose
    /// first round you already fixed is the everyday case on the red axis — and that return is news.
    /// Missing a timestamp on either side, the ledger cannot judge and errs quiet.
    fn note(&mut self, prs: &[PrEntry]) -> bool {
        self.polls = self.polls.saturating_add(1);
        let mut news = false;
        for pr in prs {
            let entry = Seen {
                last_seen: self.polls,
                updated_at: pr.updated_at.clone(),
            };
            match self.seen.get(pr.key()) {
                None => news = true,
                Some(before) if before.last_seen < self.polls - 1 => {
                    let absent = self.polls - before.last_seen - 1;
                    let moved = match (&before.updated_at, &pr.updated_at) {
                        (Some(then), Some(now)) => then != now,
                        _ => false,
                    };
                    if moved {
                        infoln!(
                            "{} left for {} poll(s) and is back with newer activity; hoot",
                            pr.key(),
                            absent
                        );
                        news = true;
                    } else {
                        // The blip, caught in the act. Logged because "GitHub sometimes omits a PR"
                        // was an observation before it was a fix, and this is the line that says how
                        // often it really happens — and whether it is the index or our own 100-hit
                        // cap churning, which has a better answer than tolerating it.
                        infoln!(
                            "search dropped {} for {} poll(s) and returned it unchanged; no hoot",
                            pr.key(),
                            absent
                        );
                    }
                }
                Some(_) => {}
            }
            self.seen.insert(pr.key().to_string(), entry);
        }
        self.evict();
        news
    }

    /// Drops the least recently present keys once the ledger is over `LEDGER_CAP`.
    fn evict(&mut self) {
        if self.seen.len() <= LEDGER_CAP {
            return;
        }
        let mut ages: Vec<u64> = self.seen.values().map(|s| s.last_seen).collect();
        // The cut is the `len - CAP`-th oldest stamp; everything at or below it goes.
        ages.sort_unstable();
        let cut = ages[self.seen.len() - LEDGER_CAP - 1];
        self.seen.retain(|_, s| s.last_seen > cut);
    }
}

/// Whether this answer is *more* than the last one.
///
/// **No longer the hoot rule**, only its stand-in. `Track::note` decides by identity now, because a
/// rising count was only ever a proxy: GitHub's search index drops a pull request from one answer and
/// returns it in the next, which reads as `3 -> 2 -> 3` and sounded for something the user had already
/// been told about. This is reached only when an answer carries no list to compare identities against
/// — a payload GitHub only partly sent — where the count really is the best evidence left.
///
/// - Both counts known: strictly greater. `2 -> 5` is three new pull requests and hoots; `5 -> 2` and
///   `2 -> 2` do not. This is what makes a busy axis able to speak again — under the old rule an axis
///   already showing `Yes` was permanently silent no matter how much piled up in it.
/// - No previous count (`None`): nothing to compare, so fall back to `presence_edge`, the pre-count
///   rule. Reachable on the first confirmed answer of a run, after `clear_pr_auth` hands out a fresh
///   `Track`, and for any endpoint that reports presence without a total. No endpoint does today, but
///   such endpoint today and never reach this code, but the type allows it and guessing "no count
///   means new" would hoot on every single poll of such an axis.
/// - No new count on a track that had one: treated as no news. A `present` answer that stopped
///   carrying a total says nothing about direction, and the alternative — hooting because the number
///   became unreadable — is a sound the user cannot act on.
///
/// One consequence of the counts coming from a capped search (`github::SEARCH_HITS_CAP`): once an
/// axis is pinned at the cap, further growth is invisible and stays silent. That is the same
/// undercount the tooltip already lives with, and erring quiet is the right direction for a sound.
fn rose(previous: Option<u32>, current: Option<u32>, presence_edge: bool) -> bool {
    match (previous, current) {
        (Some(before), Some(now)) => now > before,
        (None, _) => presence_edge,
        (Some(_), None) => false,
    }
}

impl Track {
    fn new() -> Self {
        Self {
            // We have not spoken to GitHub yet, so we genuinely do not know.
            value: Presence::Unknown,
            failures: 0,
            detail: Some("starting up".to_string()),
            count: None,
            prs: None,
            seen: std::collections::HashMap::new(),
            polls: 0,
            needs_reauth: false,
            arrived: false,
            ever_confirmed: false,
        }
    }

    fn apply(&mut self, portal: &str, result: PollResult) -> Option<Duration> {
        match result {
            PollResult::Fresh { present, count, prs } => {
                // Read before the write: the new rule is a comparison, so the old number has to be
                // taken here or it is gone.
                let previous_count = self.count;
                self.count = count;
                self.prs = prs;
                self.failures = 0;
                self.detail = None;
                // The presence edge, still computed because it is `rose`'s fallback when there is
                // no number to compare against. Both reads happen before their writes. `No` is a
                // known-empty axis filling up; `!ever_confirmed` is the first answer of a launch,
                // which is news for the same reason even though what it replaced was `Unknown`. A
                // recovered failure streak is neither: it is `Unknown` on a track that has already
                // spoken.
                let was_confirmed_absent = self.value == Presence::No;
                let is_first_answer = !self.ever_confirmed;
                self.ever_confirmed = true;
                self.value = if present { Presence::Yes } else { Presence::No };

                // Identity when the answer names its hits, which every axis now does. `rose` is left
                // for the case where it cannot: a payload hole costs the whole list (see
                // `github::Parsed::prs`), and there is then nothing to compare — so the ledger is
                // held untouched and nothing sounds, which errs quiet, the right way to be wrong
                // about a noise.
                match self.prs.clone() {
                    Some(prs) => {
                        if self.note(&prs) {
                            self.arrived = true;
                        }
                    }
                    None => {
                        if present && rose(previous_count, count, was_confirmed_absent || is_first_answer)
                        {
                            self.arrived = true;
                        }
                    }
                }
                None
            }

            // Not our data being wrong, just GitHub asking us to wait. Hold the value and obey.
            PollResult::RateLimited { retry_after } => {
                self.detail = Some(format!("rate limited, waiting {}s", retry_after.as_secs()));
                Some(retry_after)
            }

            // No grace period: a rejected credential does not recover by waiting, so pretending
            // we still know would be a lie for as long as the app stays open.
            PollResult::Unauthorized => {
                self.failures = self.failures.saturating_add(1);
                self.detail = Some(format!("{portal} rejected the credential"));
                self.value = Presence::Unknown;
                self.prs = None;
                self.needs_reauth = true;
                None
            }

            PollResult::Transient(why) => {
                self.failures = self.failures.saturating_add(1);
                self.detail = Some(why);
                // Hold the last known value for the first few failures so a blip does not make
                // the icon flap, then stop claiming to know.
                if self.failures >= FAILURES_BEFORE_UNKNOWN {
                    self.value = Presence::Unknown;
                }
                None
            }
        }
    }
}

pub struct PollState {
    /// `None` in a slot means that PR axis is off — no credential, or GitHub reported the
    /// credential lacks the scope/permission the search needs, and no search is issued for it.
    pr: [Option<Track>; 3],
    /// Whether the user asked for each PR axis at all, indexed by `PrAxis::index`.
    ///
    /// **Write-once**: set in `new` and never touched again. The distinction from `pr` is the whole
    /// point of the field existing. `pr[i]` says whether an axis is in play *right now*, which any
    /// runtime event may change — a missing credential, a permission failure. This says whether it is
    /// *allowed* to be, which nothing at runtime may change. Keeping the two apart is what lets
    /// `clear_pr_auth` put axes back after a sign-in without resurrecting one the user switched off.
    pr_enabled: [bool; 3],
    /// Why the corresponding `pr` slot is off, when we know. Without this a dark dot for a broken
    /// credential looks exactly like a dark dot because nothing needs attention, which is the one
    /// confusion this module exists to prevent.
    pr_off: [Option<String>; 3],
    /// PR status has no usable credential and needs the user to start a browser round trip.
    ///
    /// Deliberately separate from `pr_off`, even though both mean "no dots". `pr_off` is a dead end
    /// the user cannot clear from here (the GitHub App is not installed anywhere); this is a state
    /// with a menu item waiting to be clicked, so it gets its own icon and its own wording.
    pr_needs_auth: bool,
    /// The display name of the portal this state belongs to: "GitHub". Every line of wording that
    /// names who said something ("GitHub rejected the credential", "GitHub: Partially Degraded
    /// Service") reads it from here, so a second portal's state names itself without a second copy
    /// of the sentence.
    portal_name: String,
    /// GitHub's own description of a major or critical incident, when there is one.
    ///
    /// `None` covers both "GitHub is fine" and "we have not been able to ask", and that conflation is
    /// deliberate: see `github_status`, where a failed check is documented as changing nothing. The
    /// alternative — a third state that shows the mark — would raise an outage warning on the strength
    /// of the user's own connection dropping.
    ///
    /// A `String` because its only consumers are a tooltip line and a menu entry, and it should
    /// quote the words the portal used.
    status_degraded: Option<String>,
    /// Most recent `x-poll-interval`, once GitHub has told us one.
    server_interval: Option<Duration>,
    /// A wait GitHub explicitly demanded; overrides normal pacing for one cycle.
    forced_delay: Option<Duration>,
}

impl PollState {
    /// `pr_enabled` is the user's configuration **only**.
    ///
    /// Deliberately not "configured and we hold a credential". Folding credential presence in here
    /// would make `require_pr_auth` a no-op on precisely the path that exists to recover from a
    /// missing credential — every flag would be false, so there would be no exclamation icon, no
    /// `Authenticate` menu entry, and no way to ever obtain one. A missing credential is said
    /// afterwards, by calling `require_pr_auth` or `disable_pr`.
    /// The state for one GitHub portal. Kept for the tests, which predate portals and say nothing
    /// about them; production goes through `for_portal`.
    #[cfg(test)]
    pub fn new(pr_enabled: [bool; 3]) -> Self {
        Self::for_portal("GitHub", pr_enabled)
    }

    /// One portal's state. `portal_name` is `PortalInfo::display_name`.
    pub fn for_portal(portal_name: &str, pr_enabled: [bool; 3]) -> Self {
        Self {
            portal_name: portal_name.to_string(),
            // Derived from the value stored below, not from a second read of the argument, so the
            // two can never disagree about which axes exist.
            pr: pr_enabled.map(|enabled| enabled.then(Track::new)),
            pr_enabled,
            pr_off: [None, None, None],
            pr_needs_auth: false,
            // Starts clear: assuming an outage before asking would put an exclamation on the icon for
            // the first few seconds of every launch.
            status_degraded: None,
            server_interval: None,
            forced_delay: None,
        }
    }

    /// Whether a given PR axis is *enabled by config*, regardless of whether it is live right now.
    ///
    /// Split from `pr_in_play` because this change makes the two diverge: an axis can be enabled by
    /// config and still not in play, which is exactly the state a missing credential produces.
    #[cfg(test)]
    pub fn pr_enabled(&self, axis: PrAxis) -> bool {
        self.pr_enabled[axis.index()]
    }

    /// Turns a PR axis off and records why, so the tooltip can say so.
    ///
    /// Called at startup when the PR credential cannot be obtained, and mid-run if GitHub reports
    /// that the credential lacks what the search needs.
    pub fn disable_pr(&mut self, axis: PrAxis, reason: String) {
        let i = axis.index();
        // Skipped, and not merely as an optimisation. Setting `pr[i] = None` on an already-disabled
        // axis would be harmless; recording a *reason* is not. `tooltip` prints a reason as a line of
        // its own, so a config-disabled axis would grow a hover line explaining a feature the user
        // switched off. Both callers loop over all three axes for a credential-wide failure, so this
        // is a real path, not a hypothetical one.
        if !self.pr_enabled[i] {
            return;
        }
        self.pr[i] = None;
        self.pr_off[i] = Some(reason);
    }

    /// Records that PR status is waiting for the user to authorize it.
    ///
    /// Silences all three axes, because they share one credential: with none in hand there is
    /// nothing to search with, and issuing three searches that will each be rejected would only
    /// burn rate limit. No per-axis reason is stored — `tooltip` says it once for all three, and
    /// the icon and menu item say it without hovering.
    pub fn require_pr_auth(&mut self) {
        // Nothing to authorize when every axis is switched off: the exclamation and the
        // `Authenticate` entry would be demanding a credential that no search would ever use.
        //
        // The guard lives here rather than at the call sites because `pr_needs_auth` drives three
        // things at once — the icon override, a tooltip line, and the menu entry's visibility — and
        // one flag with three consumers should have one gate.
        if !self.any_pr_enabled() {
            return;
        }
        self.pr_needs_auth = true;
        for axis in PrAxis::ALL {
            self.pr[axis.index()] = None;
            self.pr_off[axis.index()] = None;
        }
    }

    /// Undoes `require_pr_auth` once a credential has been obtained, putting all three axes back in
    /// play so the next cycle can fill them in.
    ///
    /// They come back as fresh `Track`s rather than with their old values restored: whatever was
    /// last known predates the credential going away, and re-asserting it would be claiming an
    /// answer nothing has confirmed since. A fresh `Track` starts at `Presence::Unknown`, which is
    /// the honest answer and also the one that leaves the icon alone until a real poll lands.
    pub fn clear_pr_auth(&mut self) {
        self.pr_needs_auth = false;
        for axis in PrAxis::ALL {
            let i = axis.index();
            // The guard that matters, and the reason `pr_enabled` exists. This used to be an
            // unconditional `Some(Track::new())`, which meant obtaining a credential switched **on**
            // every axis — including ones the config had turned off. Getting a credential says
            // nothing about whether a signal is wanted.
            self.pr[i] = self.pr_enabled[i].then(Track::new);
            // Unconditional, unlike `pr` above: a disabled axis has no reason recorded to begin with
            // (see `disable_pr`), so this is self-healing rather than wrong.
            self.pr_off[i] = None;
        }
    }

    /// Whether any PR axis is enabled at all.
    fn any_pr_enabled(&self) -> bool {
        self.pr_enabled.iter().any(|&enabled| enabled)
    }

    /// Whether `axis` is in play: enabled by config, and not currently silenced.
    ///
    /// The poll loop's question, and so the one accessor here that is not `#[cfg(test)]`. Search has
    /// its own 30-per-minute budget and `apply_pr` discards the answer for an axis that is not in
    /// play, so issuing the request is pure cost.
    ///
    /// Deliberately the *live* predicate rather than `pr_enabled`, so an axis silenced at runtime — no
    /// credential, App not installed — is skipped too, where searching is equally pointless.
    pub fn pr_in_play(&self, axis: PrAxis) -> bool {
        self.pr[axis.index()].is_some()
    }

    /// Records the newest release above this build, or clears it.
    ///
    /// Called after each update check. Passing `None` clears the arrow, which matters after a
    /// successful install: the new binary reports its own version, so the very next check finds
    /// nothing newer and the arrow has to come back down.

    /// Records what GitHub says about itself.
    ///
    /// Only called with a verdict we actually got. A failed check does not call this at all, which is
    /// what leaves the last known answer standing rather than clearing an outage we can no longer see.
    pub fn set_status_degraded(&mut self, description: Option<String>) {
        self.status_degraded = description;
    }


    /// Whether PR status is waiting on the user.
    ///
    /// The UI reads this through `icon().needs_auth` so the icon and the menu item cannot disagree;
    /// the poll loop reads it here to pick which portal a sign-in click is for.
    pub fn pr_needs_auth(&self) -> bool {
        self.pr_needs_auth
    }

    /// Call once per cycle, before applying that cycle's responses.
    pub fn begin_cycle(&mut self) {
        // A forced wait applies to exactly one cycle.
        self.forced_delay = None;
    }

    /// No-op when `axis` is unconfigured, so callers do not have to special-case it.
    pub fn apply_pr(&mut self, axis: PrAxis, response: PollResponse) {
        self.learn_pacing(&response);
        let Some(track) = self.pr[axis.index()].as_mut() else { return };
        let forced = track.apply(&self.portal_name, response.result);
        self.record_forced(forced);
    }

    /// Which PR axes have just gone from a confirmed zero to a confirmed one or more, indexed by
    /// `PrAxis::index`, consuming the flags as it reads them.
    ///
    /// ## Which transitions count
    ///
    /// **A pull-request id the axis has not seen before** — see `Track::note`, and nothing else: a
    /// pull request already on the list can be commented on, pushed to, conflicted or reviewed and it
    /// makes no sound, because nothing on the tray changed. Three new ids landing at once is three
    /// pieces of news and one sound.
    ///
    /// The other direction is silent: a pull request leaving is work leaving.
    ///
    /// Everything the old count rule got right falls out of the ledger for free:
    ///
    /// - The **first** confirmed answer of the process hoots if anything is waiting, because an empty
    ///   ledger makes every pull request unseen. Starting the app and finding work already there is
    ///   exactly when the user wants telling. Same after `clear_pr_auth`, which hands out fresh
    ///   `Track`s on purpose.
    /// - A failure streak recovering on the **same** pull requests is silent, because they are the
    ///   same pull requests. The ledger is untouched by failures, so this needs no `ever_confirmed`
    ///   reasoning of its own. Anything that turned up while the poll was down is unseen, and hoots.
    ///
    /// And it closes a gap the count rule had: one pull request leaving and another arriving in the
    /// same cycle left the number flat and said nothing at all.
    ///
    /// Only an answer carrying no list at all falls back to `rose`.
    ///
    /// ## Why taking, rather than asking
    ///
    /// The edge exists between two `apply` calls, not in the state itself, so a plain getter would
    /// either re-announce the same arrival on every later cycle or need a second call to clear it.
    /// One `take` cannot be read twice by mistake.
    pub fn take_pr_arrivals(&mut self) -> [bool; 3] {
        std::array::from_fn(|i| {
            self.pr[i].as_mut().is_some_and(|track| std::mem::take(&mut track.arrived))
        })
    }

    fn learn_pacing(&mut self, response: &PollResponse) {
        // Learn the server's pacing whatever the outcome — it arrives on error responses too.
        if let Some(interval) = response.poll_interval {
            self.server_interval = Some(interval);
        }
    }

    /// All axes share one sleep, so the stricter demand wins.
    fn record_forced(&mut self, forced: Option<Duration>) {
        if let Some(wait) = forced {
            self.forced_delay = Some(match self.forced_delay {
                Some(existing) => existing.max(wait),
                None => wait,
            });
        }
    }

    pub fn icon(&self) -> IconState {
        // Unavailable reads as a confirmed "no dot"/"no tint" on every axis: we are not failing
        // to find out, there is simply nothing configured to ask with.
        IconState {
            needs_auth: self.pr_needs_auth,
            status_degraded: self.status_degraded.is_some(),
            signals_unsure: self.any_track_failing(),
            // App-level, not this portal's to know: `overview::icon` fills it in. Every other
            // reader of a single `PollState` sees a truthful `false`, never a stale `true`.
            update_available: false,
            review_requested: self.pr_value(PrAxis::ReviewRequested),
            ready_to_merge: self.pr_value(PrAxis::ReadyToMerge),
            changes_requested: self.pr_value(PrAxis::ChangesRequested),
        }
    }

    /// Whether any track has given up on an answer, as opposed to not having asked yet.
    ///
    /// `Presence::Unknown` alone is not enough — see `IconState::signals_unsure`. A track only reaches
    /// `Unknown` with failures against it after `FAILURES_BEFORE_UNKNOWN` transient errors, or
    /// immediately on an unauthorized reply, so a single blip cannot raise the mark.
    fn any_track_failing(&self) -> bool {
        std::iter::empty()
            .chain(self.pr.iter().map(Option::as_ref))
            .flatten()
            .any(|track| track.value == Presence::Unknown && track.failures > 0)
    }

    fn pr_value(&self, axis: PrAxis) -> Presence {
        self.pr[axis.index()].as_ref().map_or(Presence::No, |t| t.value)
    }

    /// The exact pull requests `axis`'s dot is counting, or `None` when the caller should fall back
    /// to the search page: axis off, never answered, an answer the poll could not fully read (see
    /// `github::Parsed::prs`), or a track that has given up (`Unknown`) — a click must not open a
    /// list of PRs the dot no longer stands behind.
    ///
    /// `Some(vec![])` is a confirmed empty and is *not* the same answer. Only that distinction lets a
    /// caller tell "nothing here" from "we do not know", which is the same refusal `pr_menu_label`
    /// makes about showing a stale count.
    pub fn pr_entries(&self, axis: PrAxis) -> Option<Vec<PrEntry>> {
        let track = self.pr[axis.index()].as_ref()?;
        if track.value == Presence::Unknown {
            return None;
        }
        track.prs.clone()
    }

    /// How many pull requests `axis` remembers. Test-only: the bound is the point, not the contents.
    #[cfg(test)]
    pub fn ledger_len(&self, axis: PrAxis) -> usize {
        self.pr[axis.index()].as_ref().map_or(0, |t| t.seen.len())
    }

    /// Text for the tray menu item that opens `axis`'s list, carrying the exact count.
    ///
    /// The icon itself can only carry a dot. A digit is not legible at the 16px the shell asks for,
    /// so the number goes where there is room for it: the tooltip and this menu item. Unlike the
    /// icon, neither has a limit, so the real figure is shown however large it gets.
    /// Test-only since the overview took over the menu: `overview::pr_menu_label` sums
    /// `confirmed_count` across portals and is the identity of this for one.
    #[cfg(test)]
    pub fn pr_menu_label(&self, axis: PrAxis) -> String {
        let base = axis.menu_label();
        match self.pr[axis.index()].as_ref() {
            // Only a *confirmed* count is shown. An `Unknown` axis is still holding the last number
            // it saw, and putting a stale figure in a menu label would assert something we no longer
            // know, which is the one thing this module refuses to do.
            Some(track) if track.value == Presence::Yes => match track.count {
                Some(n) if n > 0 => format!("{base} ({n})"),
                _ => base.to_string(),
            },
            _ => base.to_string(),
        }
    }

    pub fn take_pr_reauth(&mut self, axis: PrAxis) -> bool {
        self.pr[axis.index()]
            .as_mut()
            .map(|t| std::mem::take(&mut t.needs_reauth))
            .unwrap_or(false)
    }

    /// Whether GitHub has explicitly demanded a wait this cycle.
    ///
    /// `next_delay` already folds this in, but a caller running a user-triggered burst bypasses
    /// normal pacing entirely, so it needs to be able to ask the question directly.
    pub fn rate_limited(&self) -> bool {
        self.forced_delay.is_some()
    }

    /// How long to wait before the next scheduled poll.
    pub fn next_delay(&self) -> Duration {
        // Never faster than our own floor, and never faster than GitHub's advertised interval.
        let base = self
            .server_interval
            .unwrap_or(MIN_POLL_INTERVAL)
            .max(MIN_POLL_INTERVAL);

        // An explicit `retry-after` wins — but it still cannot drop us below the floor.
        if let Some(forced) = self.forced_delay {
            return forced.max(base);
        }

        // Back off on the worst-affected axis, but only once it has actually given up on its
        // last known value. While inside the grace period, keep the normal cadence — the common
        // cause of early failures is a tray app launched at login before the network is up, and
        // backing off immediately would mean not noticing WiFi for many minutes.
        //
        // Written as a fold over whichever axes are actually configured, rather than named
        // fields, so it does not need hand-editing every time an axis is added or removed.
        let failures = std::iter::empty::<Option<&Track>>()
            .chain(self.pr.iter().map(Option::as_ref))
            .flatten()
            .map(|t| t.failures)
            .max()
            .unwrap_or(0);

        let exponent = failures.saturating_sub(FAILURES_BEFORE_UNKNOWN - 1);
        if exponent == 0 {
            return base;
        }

        // The trailing `.max(base)` matters: if GitHub ever advertises an interval longer than
        // MAX_BACKOFF, the cap must not pull us back under the interval it just told us to
        // respect.
        let factor = 1u32 << exponent.min(16);
        base.saturating_mul(factor).min(MAX_BACKOFF).max(base)
    }

    /// Hover text. This is the only place `Unknown` becomes visible on Windows, so the reason
    /// belongs here rather than only in the log.
    /// The state lines of the hover text, in order: the outage first, then either the single
    /// not-authorized line or one line per axis. Without the app-level update line and without the
    /// error detail, which `overview::tooltip` appends in that order so the result for one portal
    /// is exactly what `tooltip` returns.
    pub fn tooltip_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        // First, because it reframes everything below it: during an outage a stale count or an
        // "unknown" axis has an explanation, and the user should read that before the numbers.
        if let Some(description) = self.status_degraded.as_deref() {
            lines.push(format!("{}: {description}", self.portal_name));
        }

        // A disabled axis explains itself; there is nothing else left to keep quiet about
        // — it is either a deliberate config choice or there is nothing configured yet, so the
        // line is simply omitted rather than replaced with a reason.
        // One line for all three axes, not one each. They share a single credential, so three
        // copies of the same sentence would say nothing extra while eating the whole
        // `MAX_TOOLTIP_CHARS` budget — and the per-axis lines below would be claiming answers that
        // were never fetched.
        if self.pr_needs_auth {
            lines.push(PR_NEEDS_AUTH_TOOLTIP.to_string());
        } else {
            for axis in PrAxis::ALL {
                match (self.pr[axis.index()].as_ref(), self.pr_off[axis.index()].as_deref()) {
                    (Some(track), _) => lines.push(match track.value {
                        Presence::Yes => axis.tooltip_yes(track.count),
                        Presence::No => axis.tooltip_no().to_string(),
                        Presence::Unknown => axis.tooltip_unknown().to_string(),
                    }),
                    // The dot is off for a reason the user can fix, so hovering has to say which one.
                    (None, Some(reason)) => lines.push(reason.to_string()),
                    (None, None) => {}
                }
            }
        }

        lines
    }

    /// At most one reason something is wrong, taking the axes in `PrAxis::ALL` order — the tooltip
    /// is 128 UTF-16 units on Windows, so more than one error string would not survive truncation
    /// anyway.
    pub fn detail(&self) -> Option<String> {
        self.pr.iter().filter_map(Option::as_ref).find_map(|t| t.detail.clone())
    }

    /// Hover text for this portal alone: the state lines, then the detail, capped. The tray shows
    /// `overview::tooltip`, which is this plus the update line and, with several portals, their
    /// names; for one portal the two are identical, and a test holds them to it. Test-only for
    /// that reason: production has exactly one reader of hover text, and it is the overview.
    #[cfg(test)]
    pub fn tooltip(&self) -> String {
        let mut lines = self.tooltip_lines();
        if let Some(detail) = self.detail() {
            lines.push(detail);
        }
        Self::cap_tooltip(lines.join("
"))
    }

    /// Cuts hover text to what the tray can show, marking the cut. Applied once to the whole text,
    /// never per portal, so several portals cannot each spend the whole budget.
    pub fn cap_tooltip(text: String) -> String {
        match text.char_indices().nth(MAX_TOOLTIP_CHARS) {
            Some((idx, _)) => format!("{}…", &text[..idx]),
            None => text,
        }
    }

    /// The count `axis` currently stands behind: `Some(n)` only for a confirmed-present axis that
    /// counted its hits. `None` for absent, unknown, unconfigured and countless answers alike, so a
    /// caller summing across portals adds only numbers somebody actually vouched for.
    pub fn confirmed_count(&self, axis: PrAxis) -> Option<u32> {
        match self.pr[axis.index()].as_ref() {
            Some(track) if track.value == Presence::Yes => track.count,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn respond(result: PollResult) -> PollResponse {
        PollResponse { result, poll_interval: None }
    }

    fn fresh(present: bool) -> PollResponse {
        respond(PollResult::Fresh {
            present,
            count: None,
            prs: None,
        })
    }

    fn fresh_count(n: u32) -> PollResponse {
        respond(PollResult::Fresh { present: n > 0, count: Some(n), prs: None })
    }

    /// A confirmed changes-requested answer carrying the counted PRs' URLs.
    fn fresh_urls(urls: &[&str]) -> PollResponse {
        respond(PollResult::Fresh {
            present: !urls.is_empty(),
            count: Some(urls.len() as u32),
            prs: Some(urls.iter().map(|u| PrEntry::stub(u)).collect()),
        })
    }

    fn transient() -> PollResponse {
        respond(PollResult::Transient("network down".to_string()))
    }

    /// Only the review-requested axis is configured/driven in most tests below — the two newer PR
    /// axes get their own dedicated tests further down, since most of this suite predates them and
    /// is about the state machine's general behavior, not about having three PR axes specifically.
    fn new_state(review_requested: bool) -> PollState {
        PollState::new([review_requested, false, false])
    }

    /// Drives one full cycle so `begin_cycle` bookkeeping is exercised the way the loop does it.
    fn cycle(state: &mut PollState, reviews: Option<PollResponse>) {
        state.begin_cycle();
        if let Some(r) = reviews {
            state.apply_pr(PrAxis::ReviewRequested, r);
        }
    }

    #[test]
    fn starts_unknown_rather_than_claiming_clear() {
        // The old code booted straight to the "nothing waiting" icon before ever asking.
    }

    #[test]
    fn unconfigured_reviews_read_as_a_confirmed_no_dot() {
        let state = new_state(false);
        assert!(!state.pr_in_play(PrAxis::ReviewRequested));
        // Not Unknown: we are not failing to find out, the feature is simply off.
        assert_eq!(state.icon().review_requested, Presence::No);
        assert_eq!(state.icon().review_requested.as_confirmed(), Some(false));
    }

    #[test]
    fn unconfigured_reviews_ignore_applied_responses() {
        let mut state = new_state(false);
        cycle(&mut state, Some(fresh_count(9)));
        assert_eq!(state.icon().review_requested, Presence::No, "must stay off when unconfigured");
    }

    #[test]
    fn both_axes_track_independently() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(3)));
        assert_eq!(state.icon().review_requested, Presence::Yes);

        cycle(&mut state, Some(fresh_count(0)));
        assert_eq!(state.icon().review_requested, Presence::No);
    }

    #[test]
    fn reauth_flags_clear_on_read() {
        let mut state = new_state(true);
        cycle(&mut state, Some(respond(PollResult::Unauthorized)));
        assert!(state.take_pr_reauth(PrAxis::ReviewRequested));
        assert!(
            !state.take_pr_reauth(PrAxis::ReviewRequested),
            "the flag must clear on read"
        );
    }

    #[test]
    fn holds_last_known_state_for_two_failures_then_admits_ignorance() {
        let mut state = new_state(false);
        cycle(&mut state, None);

        cycle(&mut state, None);
        cycle(&mut state, None);
        cycle(&mut state, None);
    }

    #[test]
    fn not_modified_preserves_state_and_counts_as_success() {
        let mut state = new_state(false);
        cycle(&mut state, None);
        cycle(&mut state, None);
        cycle(&mut state, None);

        cycle(&mut state, None);
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL, "a 304 clears the failure streak");
    }

    #[test]
    fn recovery_resets_the_failure_streak() {
        let mut state = new_state(false);
        cycle(&mut state, None);
        for _ in 0..5 {
            cycle(&mut state, None);
        }

        cycle(&mut state, None);
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL, "backoff must not persist");
    }

    #[test]
    fn server_interval_overrides_our_floor_when_longer() {
        let mut state = new_state(true);
        state.begin_cycle();
        state.apply_pr(
            PrAxis::ReviewRequested,
            PollResponse {
                result: PollResult::Fresh { present: false, count: None, prs: None },
                poll_interval: Some(Duration::from_secs(120)),
            },
        );
        assert_eq!(state.next_delay(), Duration::from_secs(120));
    }

    #[test]
    fn server_interval_below_our_floor_does_not_speed_us_up() {
        let mut state = new_state(true);
        state.begin_cycle();
        state.apply_pr(
            PrAxis::ReviewRequested,
            PollResponse {
                result: PollResult::Fresh { present: false, count: None, prs: None },
                poll_interval: Some(Duration::from_secs(5)),
            },
        );
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL);
    }

    #[test]
    fn retry_after_is_obeyed_but_never_below_the_floor() {
        let mut state = new_state(true);
        cycle(&mut state, Some(rate_limited(600)));
        assert_eq!(state.next_delay(), Duration::from_secs(600));

        // 30s is shorter than our floor; obeying it literally would poll too fast.
        let mut state = new_state(true);
        cycle(&mut state, Some(rate_limited(30)));
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL);
    }

    /// Search has a much tighter budget (30/min) than core, so a search rate-limit must be able
    /// to slow the whole cycle down.
    fn rate_limited(secs: u64) -> PollResponse {
        respond(PollResult::RateLimited { retry_after: Duration::from_secs(secs) })
    }

    #[test]
    fn the_stricter_of_two_axes_forced_delays_wins() {
        let mut state = PollState::new([true, true, false]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, rate_limited(90));
        state.apply_pr(PrAxis::ReadyToMerge, rate_limited(600));
        assert_eq!(state.next_delay(), Duration::from_secs(600), "the longer wait must govern");

        let mut state = PollState::new([true, true, false]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, rate_limited(600));
        state.apply_pr(PrAxis::ReadyToMerge, rate_limited(90));
        assert_eq!(state.next_delay(), Duration::from_secs(600));
    }

    #[test]
    fn rate_limiting_holds_the_icon_rather_than_clearing_it() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(1)));
        // The old code turned a 403 into Ok(0) and cleared the icon here.
        cycle(&mut state, Some(rate_limited(60)));
        assert_eq!(state.icon().review_requested, Presence::Yes);
    }

    /// The point of separating `RateLimited` from `Transient`: a limit is not our data going
    /// stale, so it never expires the way three transient failures do. A secondary limit can
    /// easily outlast `FAILURES_BEFORE_UNKNOWN` cycles, and blanking the tray then would be the
    /// app forgetting a count GitHub never disputed.
    #[test]
    fn a_long_rate_limit_streak_never_gives_up_the_last_count() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(4)));
        for _ in 0..(FAILURES_BEFORE_UNKNOWN + 5) {
            cycle(&mut state, Some(rate_limited(60)));
        }
        assert_eq!(state.icon().review_requested, Presence::Yes);
        assert!(!state.icon().signals_unsure, "a limit is not us losing track");
        assert!(
            state.pr_menu_label(PrAxis::ReviewRequested).contains('4'),
            "the count itself must survive, not just the dot: {}",
            state.pr_menu_label(PrAxis::ReviewRequested)
        );
    }

    /// Paired with `forced_delay_applies_to_one_cycle_only`: the scheduler reads this to decide
    /// whether a burst may run, so it has to be true for exactly as long as the delay itself is.
    #[test]
    fn rate_limited_reports_only_while_the_demand_is_live() {
        let mut state = new_state(true);
        assert!(!state.rate_limited(), "nothing has been demanded yet");

        cycle(&mut state, Some(rate_limited(600)));
        assert!(state.rate_limited());

        cycle(&mut state, Some(fresh(false)));
        assert!(!state.rate_limited(), "a healthy answer clears the demand");
    }

    /// A limit on either axis has to hold the whole cycle back, since both share one sleep.
    #[test]
    fn rate_limited_reports_a_demand_from_the_review_axis_too() {
        let mut state = new_state(true);
        cycle(&mut state, Some(rate_limited(120)));
        assert!(state.rate_limited(), "search has its own, much tighter budget");
    }

    #[test]
    fn forced_delay_applies_to_one_cycle_only() {
        let mut state = new_state(true);
        cycle(&mut state, Some(rate_limited(600)));
        assert_eq!(state.next_delay(), Duration::from_secs(600));

        cycle(&mut state, Some(fresh(false)));
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL);
    }

    #[test]
    fn stays_responsive_while_still_within_the_grace_period() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh(false)));

        cycle(&mut state, Some(transient()));
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL);
        cycle(&mut state, Some(transient()));
        assert_eq!(state.next_delay(), MIN_POLL_INTERVAL);
    }

    #[test]
    fn backoff_starts_when_we_give_up_then_stops_at_the_cap() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh(false)));

        cycle(&mut state, Some(transient()));
        cycle(&mut state, Some(transient()));
        cycle(&mut state, Some(transient()));
        assert_eq!(state.icon().review_requested, Presence::Unknown);
        assert_eq!(state.next_delay(), Duration::from_secs(120));
        cycle(&mut state, Some(transient()));
        assert_eq!(state.next_delay(), Duration::from_secs(240));

        for _ in 0..20 {
            cycle(&mut state, Some(transient()));
        }
        assert_eq!(state.next_delay(), MAX_BACKOFF, "must saturate, not overflow");
    }

    #[test]
    fn a_long_server_interval_survives_the_backoff_cap() {
        let mut state = new_state(true);
        let long = Duration::from_secs(30 * 60);
        for _ in 0..4 {
            state.begin_cycle();
            state.apply_pr(
                PrAxis::ReviewRequested,
                PollResponse {
                    result: PollResult::Transient("nope".to_string()),
                    poll_interval: Some(long),
                },
            );
        }
        // MAX_BACKOFF is 15 min; clamping to it would poll faster than GitHub just asked.
        assert_eq!(state.next_delay(), long);
    }

    #[test]
    fn tooltip_quotes_the_review_count() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(3)));
        assert!(state.tooltip().contains("3 PR(s) awaiting your review"));
    }

    /// A dark dot with no explanation is indistinguishable from "nothing to review", so a
    /// disabled axis has to say why on hover.
    #[test]
    fn a_disabled_review_axis_explains_itself_in_the_tooltip() {
        // Enabled by config, then silenced at runtime — which is the only combination that produces a
        // reason. An axis the config never asked for is silent instead, deliberately: see
        // `a_config_disabled_axis_shows_no_dot_no_count_and_no_tooltip_line`.
        let mut state = new_state(true);
        state.disable_pr(PrAxis::ReviewRequested, "PR status off: sign-in failed".to_string());
        cycle(&mut state, None);

        let tip = state.tooltip();
        assert!(tip.contains("sign-in failed"), "got {tip:?}");
        assert_eq!(
            state.icon().review_requested,
            Presence::No,
            "off means no dot, not an unknown dot"
        );
    }

    #[test]
    fn disabling_reviews_stops_the_axis_asserting_anything() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(5)));
        assert_eq!(state.icon().review_requested, Presence::Yes);

        state.disable_pr(PrAxis::ReviewRequested, "PR status off: sign-in failed".to_string());
        cycle(&mut state, Some(fresh_count(5)));
        assert_eq!(
            state.icon().review_requested,
            Presence::No,
            "a disabled axis must ignore late answers"
        );
    }

    /// The menu is where the exact number lives, since the icon can only carry a dot.
    #[test]
    fn the_menu_label_carries_the_exact_count() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(3)));
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), "Open Requested Reviews (3)");

        // No 9+ ceiling here: unlike a 16px icon, a menu item has room for any figure.
        cycle(&mut state, Some(fresh_count(147)));
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), "Open Requested Reviews (147)");
    }

    #[test]
    fn the_menu_label_drops_the_count_when_there_is_nothing_to_count() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(0)));
        assert_eq!(
            state.pr_menu_label(PrAxis::ReviewRequested),
            REVIEWS_MENU_LABEL,
            "zero is not worth parentheses"
        );

        let mut state = new_state(false);
        cycle(&mut state, None);
        assert_eq!(
            state.pr_menu_label(PrAxis::ReviewRequested),
            REVIEWS_MENU_LABEL,
            "no credential, no count"
        );
    }

    /// A stale number in a menu label would assert something we no longer know.
    #[test]
    fn the_menu_label_drops_a_count_it_can_no_longer_vouch_for() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(4)));
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), "Open Requested Reviews (4)");

        // Enough failures that the axis stops claiming to know.
        for _ in 0..FAILURES_BEFORE_UNKNOWN {
            cycle(&mut state, Some(transient()));
        }
        assert_eq!(state.icon().review_requested, Presence::Unknown);
        assert_eq!(
            state.pr_menu_label(PrAxis::ReviewRequested),
            REVIEWS_MENU_LABEL,
            "an unknown axis must not quote its last known figure"
        );
    }

    /// Within the grace period the value is still asserted, so the count stands with it.
    #[test]
    fn the_menu_label_survives_a_single_blip() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(2)));
        cycle(&mut state, Some(transient()));
        assert_eq!(state.icon().review_requested, Presence::Yes, "one blip must not flap");
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), "Open Requested Reviews (2)");
    }

    #[test]
    fn tooltip_omits_reviews_entirely_when_unconfigured() {
        let mut state = new_state(false);
        cycle(&mut state, None);
        let tip = state.tooltip();
        assert!(!tip.to_lowercase().contains("review"), "got {tip:?}");
    }

    #[test]
    fn tooltip_reports_the_reason_and_stays_short() {
        let mut state = new_state(true);
        cycle(&mut state, Some(fresh_count(1)));
        cycle(&mut state, Some(fresh_count(1)));

        let tip = state.tooltip();
        assert!(
            tip.chars().count() <= MAX_TOOLTIP_CHARS + 1,
            "got {} chars",
            tip.chars().count()
        );
    }

    // ── The three PR axes together ─────────────────────────────────────────────
    // Everything above exercises the state machine generally through the review-requested axis,
    // which predates the other two. These tests are specifically about `PrAxis` generalizing
    // cleanly to three axes rather than two named fields.

    #[test]
    fn all_three_pr_axes_are_independently_configurable() {
        let state = PollState::new([true, false, true]);
        // At construction the config notion and the live notion agree, and this is the only place
        // that is true — so both are pinned here, and their divergence is pinned in
        // `each_pr_axis_can_be_disabled_independently` below.
        for (axis, expected) in [
            (PrAxis::ReviewRequested, true),
            (PrAxis::ReadyToMerge, false),
            (PrAxis::ChangesRequested, true),
        ] {
            assert_eq!(state.pr_enabled(axis), expected, "{axis:?} config");
            assert_eq!(state.pr_in_play(axis), expected, "{axis:?} live");
        }
    }

    #[test]
    fn all_three_pr_axes_track_independently() {
        let mut state = PollState::new([true, true, true]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        state.apply_pr(PrAxis::ReadyToMerge, fresh_count(0));
        state.apply_pr(PrAxis::ChangesRequested, fresh_count(2));

        let icon = state.icon();
        assert_eq!(icon.review_requested, Presence::Yes);
        assert_eq!(icon.ready_to_merge, Presence::No);
        assert_eq!(icon.changes_requested, Presence::Yes);

        // A sustained failure on one axis must not disturb the other two.
        for _ in 0..FAILURES_BEFORE_UNKNOWN {
            state.begin_cycle();
            state.apply_pr(PrAxis::ReadyToMerge, transient());
        }
        let icon = state.icon();
        assert_eq!(icon.review_requested, Presence::Yes, "unrelated axis must be unaffected");
        assert_eq!(icon.ready_to_merge, Presence::Unknown, "the failing axis alone gives up");
        assert_eq!(icon.changes_requested, Presence::Yes, "unrelated axis must be unaffected");
    }

    // ── The fallback PR-inbox entry ───────────────────────────────────────────

    /// The gap this entry fills. Every PR entry hides itself when its axis is dark, on the sound
    /// principle that an entry opening an empty list is a dead end — which left a quiet day with no
    /// way into GitHub's pull requests at all.
    #[test]
    fn the_inbox_entry_fills_in_for_an_empty_pr_list() {
        assert!(shows_pr_inbox([false, false, false], false));
    }

    /// It is a fallback, not a fixture: any entry that can name what it opens says it better.
    #[test]
    fn the_inbox_entry_stands_down_for_any_real_entry() {
        for axis in PrAxis::ALL {
            let mut lit = [false; 3];
            lit[axis.index()] = true;
            assert!(!shows_pr_inbox(lit, false), "{axis:?} alone must speak for itself");
        }
        assert!(!shows_pr_inbox([true, true, true], false));
    }

    /// The one case where it matters most. Waiting to authorize hides all three PR entries whatever
    /// their counts, so this is the only PR link left — and being a plain URL, it is the one that still
    /// works without a credential.
    #[test]
    fn the_inbox_entry_survives_waiting_to_authorize() {
        assert!(shows_pr_inbox([true, true, true], true));
        assert!(shows_pr_inbox([false, false, false], true));
    }

    #[test]
    fn each_pr_axis_has_its_own_menu_label_and_count() {
        let mut state = PollState::new([true, true, true]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        state.apply_pr(PrAxis::ReadyToMerge, fresh_count(1));
        state.apply_pr(PrAxis::ChangesRequested, fresh_count(5));

        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), "Open Requested Reviews (3)");
        assert_eq!(state.pr_menu_label(PrAxis::ReadyToMerge), "Open Approved PRs (1)");
        assert_eq!(state.pr_menu_label(PrAxis::ChangesRequested), "Open Work Required (5)");
    }

    #[test]
    fn each_pr_axis_can_be_disabled_independently() {
        let mut state = PollState::new([true, true, true]);
        state.disable_pr(PrAxis::ReadyToMerge, "merge dot off: test reason".to_string());

        assert!(state.pr_in_play(PrAxis::ReviewRequested));
        assert!(!state.pr_in_play(PrAxis::ReadyToMerge));
        assert!(state.pr_in_play(PrAxis::ChangesRequested));

        // The divergence `pr_enabled` exists for: this axis was silenced at *runtime*, so it is out of
        // play, but it is still enabled by config — which is what lets `clear_pr_auth` know to bring it
        // back while leaving a config-disabled axis alone.
        assert!(
            state.pr_enabled(PrAxis::ReadyToMerge),
            "a runtime disable must not rewrite the user's configuration"
        );

        let tip = state.tooltip();
        assert!(tip.contains("merge dot off: test reason"), "got {tip:?}");
    }

    // ── Waiting on the user to authorize ───────────────────────────────────────

    #[test]
    fn require_pr_auth_silences_every_axis_and_flags_the_icon() {
        let mut state = PollState::new([true, true, true]);
        state.require_pr_auth();

        assert!(state.pr_needs_auth());
        assert!(state.icon().needs_auth, "the icon must carry the override");
        for axis in PrAxis::ALL {
            assert!(
                !state.pr_in_play(axis),
                "{axis:?} must be silenced — one credential is missing, so all three are"
            );
        }
    }

    /// The URL list follows the same lifecycle as the value it belongs to: confirmed answers
    /// replace it, a blip holds it, and a track that has given up stops handing it out. Both
    /// GraphQL axes, and each keeps its own list.
    #[test]
    fn pr_entries_are_held_through_a_blip_and_dropped_with_the_value() {
        for axis in [PrAxis::ReadyToMerge, PrAxis::ChangesRequested] {
            let mut state = PollState::new([true, true, true]);
            assert_eq!(state.pr_entries(axis), None, "no answer yet, so no list to stand behind");

            state.apply_pr(axis, fresh_urls(&["https://github.com/o/r/pull/9"]));
            assert_eq!(state.pr_entries(axis), Some(vec![PrEntry::stub("https://github.com/o/r/pull/9")]));
            for other in PrAxis::ALL.iter().filter(|a| **a != axis) {
                assert_eq!(state.pr_entries(*other), None, "{other:?} must not borrow {axis:?}'s list");
            }

            // A blip holds the list exactly as it holds the count.
            state.apply_pr(axis, transient());
            assert_eq!(state.pr_entries(axis), Some(vec![PrEntry::stub("https://github.com/o/r/pull/9")]));

            // ...but a track that admits ignorance must not keep vouching for the PRs it lost.
            for _ in 0..FAILURES_BEFORE_UNKNOWN {
                state.apply_pr(axis, transient());
            }
            assert_eq!(state.pr_entries(axis), None);
        }
    }

    /// An axis the user switched off has no track, and must answer like one that never spoke.
    #[test]
    fn pr_entries_is_none_when_the_axis_is_off() {
        let state = PollState::new([true, false, false]);
        assert_eq!(state.pr_entries(PrAxis::ReadyToMerge), None);
        assert_eq!(state.pr_entries(PrAxis::ChangesRequested), None);
    }

    /// A silenced axis must not be revivable by a stray response. `apply_pr` is documented as a
    /// no-op when an axis is unconfigured, and this is the case that matters: a search already in
    /// flight when the credential died must not put a dot back on an icon that is telling the user
    /// nothing is known.
    #[test]
    fn responses_arriving_after_require_pr_auth_are_ignored() {
        let mut state = PollState::new([true, true, true]);
        state.require_pr_auth();
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(7));

        assert_eq!(state.icon().review_requested, Presence::No);
        assert!(state.icon().needs_auth);
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), REVIEWS_MENU_LABEL);
    }

    /// The three axes share one credential, so the tooltip must explain it once. Three copies would
    /// fit inside no sensible budget and add nothing.
    #[test]
    fn the_needs_auth_tooltip_is_said_once_not_once_per_axis() {
        let mut state = PollState::new([true, true, true]);
        state.require_pr_auth();

        let tip = state.tooltip();
        assert_eq!(tip.matches(PR_NEEDS_AUTH_TOOLTIP).count(), 1, "got {tip:?}");
        assert!(tip.chars().count() <= MAX_TOOLTIP_CHARS + 1, "got {} chars", tip.chars().count());
        // The per-axis lines must be gone entirely: "No reviews requested" alongside "not
        // authorized" would be answering a question that was never asked.
        assert!(!tip.contains(PrAxis::ReviewRequested.tooltip_no()), "got {tip:?}");
    }

    #[test]
    fn clear_pr_auth_puts_every_axis_back_in_play_without_stale_values() {
        let mut state = PollState::new([true, true, true]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(4));
        assert_eq!(state.icon().review_requested, Presence::Yes);

        state.require_pr_auth();
        state.clear_pr_auth();

        assert!(!state.pr_needs_auth());
        assert!(!state.icon().needs_auth);
        for axis in PrAxis::ALL {
            assert!(state.pr_in_play(axis), "{axis:?} must be searchable again");
        }
        // `Unknown`, not `Yes` and not `No`: the count of 4 predates the credential going away, and
        // nothing has confirmed anything since. `Unknown` is also what stops the icon flickering —
        // the UI leaves that dot as it is until a real answer lands.
        assert_eq!(state.icon().review_requested, Presence::Unknown);
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), REVIEWS_MENU_LABEL);
    }

    /// `require_pr_auth` must clear any earlier `disable_pr` reason, or the tooltip would carry a
    /// stale "off because…" line next to the new "not authorized" one.
    #[test]
    fn require_pr_auth_supersedes_an_earlier_disable_reason() {
        let mut state = PollState::new([true, true, true]);
        state.disable_pr(PrAxis::ReadyToMerge, "merge dot off: earlier reason".to_string());
        state.require_pr_auth();

        let tip = state.tooltip();
        assert!(!tip.contains("earlier reason"), "got {tip:?}");
        assert_eq!(tip, PR_NEEDS_AUTH_TOOLTIP);
    }

    // ── Per-axis configuration ──────────────────────────────────────────────
    //
    // A config-disabled axis has to be invisible in six separate ways: no bar, no count, no tooltip
    // line, no search, no contribution to backoff, and no credential renewal. Most of those are true
    // today only as a side effect of `pr[i] == None`, with nothing saying so — which is what these pin.

    /// One line, but four modules index `[T; 3]` by `PrAxis::index` and nothing asserted that `ALL` is
    /// in that order. The config array makes a fifth.
    #[test]
    fn pr_axis_all_is_in_index_order() {
        assert_eq!(PrAxis::ALL.map(PrAxis::index), [0, 1, 2]);
    }

    #[test]
    fn a_config_disabled_axis_shows_no_dot_no_count_and_no_tooltip_line() {
        let mut state = PollState::new([true, false, true]);
        state.begin_cycle();
        for axis in PrAxis::ALL {
            state.apply_pr(axis, fresh_count(4));
        }

        assert!(!state.pr_enabled(PrAxis::ReadyToMerge));
        assert!(!state.pr_in_play(PrAxis::ReadyToMerge));

        // `No`, and emphatically not `Unknown`. This is what makes the UI need no changes at all: the
        // drains use `as_confirmed().unwrap_or(current)`, so an `Unknown` would leave whatever bar is
        // already on screen lit rather than hiding it.
        assert_eq!(state.icon().ready_to_merge, Presence::No);
        assert_eq!(state.icon().ready_to_merge.as_confirmed(), Some(false));
        // No count appended, so the menu entry reads as bare even though a response arrived.
        assert_eq!(state.pr_menu_label(PrAxis::ReadyToMerge), PrAxis::ReadyToMerge.menu_label());

        let tip = state.tooltip();
        for phrase in [PrAxis::ReadyToMerge.tooltip_no(), PrAxis::ReadyToMerge.tooltip_unknown()] {
            assert!(!tip.contains(phrase), "disabled axis leaked {phrase:?} into {tip:?}");
        }
        // …while the two enabled axes still report.
        assert!(tip.contains("4 PR(s) awaiting your review"), "got {tip:?}");
    }

    /// Asserted as an exact string, not a `contains`: the point is that the PR half contributes
    /// *nothing*, and a stray blank line from `lines.join` or a leaked detail would slip past a
    /// looser check.
    #[test]
    fn every_axis_disabled_leaves_the_pr_half_completely_silent() {
        let mut state = PollState::new([false; 3]);
        state.begin_cycle();

        // Not one word about pull requests: every axis is off, so there is nothing to report and
        // nothing to explain. The tooltip used to have the notifications line to fall back on.
        assert_eq!(state.tooltip(), "");
    }

    /// Paired with the test below on purpose. The plausible slip is writing `all` where `any` was
    /// meant, and this test alone would still pass if that happened.
    #[test]
    fn require_pr_auth_is_ignored_when_every_axis_is_config_disabled() {
        let mut state = PollState::new([false; 3]);
        state.require_pr_auth();

        assert!(!state.pr_needs_auth(), "nothing to authorize when nothing is enabled");
        assert!(!state.icon().needs_auth, "no exclamation for a feature that is switched off");
        assert_eq!(state.tooltip(), "", "and no hover line either");
    }

    #[test]
    fn require_pr_auth_still_flags_the_icon_when_only_one_axis_is_enabled() {
        let mut state = PollState::new([false, true, false]);
        state.require_pr_auth();

        assert!(state.pr_needs_auth());
        assert!(state.icon().needs_auth);
        assert_eq!(state.tooltip(), PR_NEEDS_AUTH_TOOLTIP);
    }

    /// Mimics the credential-wide failure loop in `scheduler`, which calls `disable_pr` for all three
    /// axes. The count is exact: a `contains` would pass even with a third line for the disabled axis.
    #[test]
    fn disable_pr_does_not_give_a_config_disabled_axis_a_reason_to_show() {
        let mut state = PollState::new([true, false, true]);
        let reason = "PR status off: sign-in failed";
        for axis in PrAxis::ALL {
            state.disable_pr(axis, reason.to_string());
        }

        let tip = state.tooltip();
        assert_eq!(tip.matches(reason).count(), 2, "one line per *enabled* axis, got {tip:?}");
    }

    /// The bug this whole field exists to fix. Obtaining a credential says nothing about whether a
    /// signal is wanted, but `clear_pr_auth` used to switch all three back on regardless.
    #[test]
    fn clear_pr_auth_does_not_resurrect_a_config_disabled_axis() {
        let mut state = PollState::new([true, false, true]);
        state.require_pr_auth();
        state.clear_pr_auth();

        assert!(state.pr_in_play(PrAxis::ReviewRequested), "an enabled axis must come back");
        assert!(state.pr_in_play(PrAxis::ChangesRequested), "an enabled axis must come back");
        assert!(
            !state.pr_in_play(PrAxis::ReadyToMerge),
            "authenticating must not switch on an axis the config turned off"
        );
        assert_eq!(state.icon().ready_to_merge, Presence::No);
        assert!(!state.tooltip().contains(PrAxis::ReadyToMerge.tooltip_no()));
    }

    /// Extends the documented in-flight-response race across the one moment `pr[i]` is rewritten,
    /// which is exactly where the old `clear_pr_auth` would have handed the response a live `Track`.
    #[test]
    fn a_response_for_a_config_disabled_axis_is_ignored_even_after_authenticating() {
        let mut state = PollState::new([true, false, false]);
        state.require_pr_auth();
        state.clear_pr_auth();
        state.begin_cycle();
        state.apply_pr(PrAxis::ReadyToMerge, fresh_count(7));

        assert_eq!(state.icon().ready_to_merge, Presence::No);
        assert_eq!(state.pr_menu_label(PrAxis::ReadyToMerge), PrAxis::ReadyToMerge.menu_label());
    }

    /// A disabled axis must not be able to slow the other axes down. True today only because
    /// the backoff fold skips `None` slots, which nothing stated.
    #[test]
    fn a_config_disabled_axis_contributes_no_failures_to_backoff() {
        let mut state = PollState::new([true, false, false]);
        for _ in 0..6 {
            state.begin_cycle();
            state.apply_pr(PrAxis::ReadyToMerge, transient());
        }
        assert_eq!(
            state.next_delay(),
            MIN_POLL_INTERVAL,
            "failures on a disabled axis must not back anything off"
        );

        // …while the same failures on an enabled axis do.
        for _ in 0..4 {
            state.begin_cycle();
            state.apply_pr(PrAxis::ReviewRequested, transient());
        }
        assert!(state.next_delay() > MIN_POLL_INTERVAL, "an enabled axis still backs off");
    }

    /// A stray 401 on a disabled axis must not start a refresh grant or a device flow.
    #[test]
    fn a_config_disabled_axis_never_asks_for_credential_re_auth() {
        let mut state = PollState::new([true, false, false]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReadyToMerge, respond(PollResult::Unauthorized));
        assert!(!state.take_pr_reauth(PrAxis::ReadyToMerge));

        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        assert!(state.take_pr_reauth(PrAxis::ReviewRequested), "an enabled axis still asks");
    }

    /// "No search is issued" has to follow from state, not from a second copy of the config living in
    /// the scheduler. Walks the four situations the poll loop can observe.
    #[test]
    fn the_poll_loop_is_told_which_axes_to_skip() {
        let mut state = PollState::new([true, false, true]);
        let in_play = |s: &PollState| PrAxis::ALL.map(|a| s.pr_in_play(a));

        assert_eq!(in_play(&state), [true, false, true], "from config");

        state.require_pr_auth();
        assert_eq!(in_play(&state), [false; 3], "nothing to search without a credential");

        state.clear_pr_auth();
        assert_eq!(in_play(&state), [true, false, true], "back to the config, not to all three");

        state.disable_pr(PrAxis::ReviewRequested, "x".to_string());
        assert_eq!(in_play(&state), [false, false, true], "a runtime disable also stops the search");
    }

    #[test]
    fn next_delay_backs_off_on_the_worst_of_all_configured_axes() {
        let mut state = PollState::new([true, true, true]);
        state.begin_cycle();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        state.apply_pr(PrAxis::ChangesRequested, fresh_count(0));

        for _ in 0..4 {
            state.begin_cycle();
            state.apply_pr(PrAxis::ReadyToMerge, transient());
        }
        assert_eq!(
            state.next_delay(),
            Duration::from_secs(240),
            "backoff must key off the worst axis even when it is not review-requested"
        );
    }

    // ── GitHub's own health ─────────────────────────────────────────────────

    /// The whole reason `status_degraded` is a separate field: the icon merges the two causes, the menu
    /// must not. An outage offering to re-authorize a working credential would send the user through a
    /// browser round trip for nothing.
    #[test]
    fn an_outage_shows_the_exclamation_without_claiming_a_credential_is_missing() {
        let mut state = PollState::new([true; 3]);
        state.set_status_degraded(Some("Partially Degraded Service".to_string()));

        let icon = state.icon();
        assert!(icon.status_degraded, "the outage bit must be set");
        assert!(!icon.needs_auth, "an outage must not read as a missing credential");
        assert!(icon.shows_exclamation(), "the mark is shown all the same");
    }

    /// And the mirror: a missing credential must not point at GitHub's status page.
    #[test]
    fn a_missing_credential_does_not_claim_an_outage() {
        let mut state = PollState::new([true; 3]);
        state.require_pr_auth();

        let icon = state.icon();
        assert!(icon.needs_auth);
        assert!(!icon.status_degraded, "a missing credential is not GitHub being down");
        assert!(icon.shows_exclamation());
    }

    /// Both at once is one exclamation and two menu entries.
    #[test]
    fn both_causes_at_once_still_show_one_exclamation() {
        let mut state = PollState::new([true; 3]);
        state.require_pr_auth();
        state.set_status_degraded(Some("Major Service Outage".to_string()));

        let icon = state.icon();
        assert!(icon.needs_auth && icon.status_degraded);
        assert!(icon.shows_exclamation());
    }

    #[test]
    fn a_healthy_github_shows_no_mark() {
        let state = PollState::new([true; 3]);
        let icon = state.icon();
        assert!(!icon.status_degraded);
        assert!(!icon.shows_exclamation(), "nothing is wrong, so nothing is marked");
    }

    /// Clearing has to work, or the mark would stay up for the life of the process once an incident
    /// ended — the failure mode that would make the whole signal untrustworthy.
    #[test]
    fn a_resolved_incident_clears_the_mark() {
        let mut state = PollState::new([true; 3]);
        state.set_status_degraded(Some("Major Service Outage".to_string()));
        assert!(state.icon().shows_exclamation());

        state.set_status_degraded(None);
        assert!(!state.icon().shows_exclamation(), "a resolved incident must take the mark down");
        assert!(!state.tooltip().contains("Major Service Outage"));
    }

    /// The tooltip is the only thing that distinguishes the two exclamation causes at a glance, and it
    /// quotes GitHub's own wording rather than paraphrasing it.
    #[test]
    fn the_tooltip_quotes_githubs_own_words_first() {
        let mut state = PollState::new([true; 3]);
        state.set_status_degraded(Some("Partially Degraded Service".to_string()));

        let tooltip = state.tooltip();
        assert!(tooltip.contains("Partially Degraded Service"), "got {tooltip:?}");
        assert!(
            tooltip.lines().next().unwrap().contains("Partially Degraded Service"),
            "the outage explains the lines under it, so it goes first: {tooltip:?}"
        );
    }

    /// An outage must not disturb the PR answers themselves. The icon hides the bars because it has one
    /// exclamation to give, but the counts are still the last known good ones and the menu still opens
    /// the lists.
    #[test]
    fn an_outage_leaves_the_pr_answers_intact() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        state.set_status_degraded(Some("Major Service Outage".to_string()));

        assert!(state.pr_in_play(PrAxis::ReviewRequested), "the axis is still being polled");
        assert_eq!(state.icon().review_requested, Presence::Yes, "the answer is untouched");
        assert!(
            state.pr_menu_label(PrAxis::ReviewRequested).contains('3'),
            "the count is still offered: {}",
            state.pr_menu_label(PrAxis::ReviewRequested)
        );
    }

    /// The startup false alarm this distinction exists to prevent: nothing polled yet means every track
    /// is `Unknown`, and a mark there would flash red on every single launch.
    #[test]
    fn a_freshly_started_app_shows_no_mark() {
        let state = PollState::new([true; 3]);
        assert!(!state.icon().any_unsure(), "not asked yet is not the same as failed");
        assert!(!state.icon().shows_exclamation());
    }

    /// And the case it must catch: a poll that actually failed.
    #[test]
    fn a_failed_poll_raises_the_mark() {
        let mut state = PollState::new([true; 3]);
        // Unauthorized goes to Unknown at once, so one is enough.
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        assert!(state.icon().any_unsure(), "a rejected credential is a failure we must admit");
        assert!(state.icon().shows_exclamation());
    }

    /// A blip must not raise it, which is what the existing failure tolerance is for.
    #[test]
    fn a_single_transient_blip_does_not_raise_the_mark() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Transient("hiccup".into())));
        assert!(!state.icon().shows_exclamation(), "one blip holds the last known value");
    }

    /// And recovery clears it, or the mark would stick for the life of the process.
    #[test]
    fn a_recovered_poll_clears_the_mark() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        assert!(state.icon().shows_exclamation());

        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        assert!(!state.icon().shows_exclamation(), "a recovered poll must take the mark down");
    }

    // ─── Menu entry visibility ────────────────────────────────────────────────

    /// **The 2.0.0/2.0.1 regression, as reported.** Approved and changes-requested lit, reviews
    /// not. The menu showed the *reviews* entry — with a bare label, because its count really was
    /// zero — and the approved entry, and never the changes entry. Each entry was wearing the bit of
    /// the axis after it.
    #[test]
    fn each_entry_shows_its_own_axis_and_not_its_neighbours() {
        let lit = [false, true, true];
        let shown = pr_entry_visibility(lit, false);
        assert!(!shown[PrAxis::ReviewRequested.index()], "nothing awaits review, so no entry");
        assert!(shown[PrAxis::ReadyToMerge.index()]);
        assert!(shown[PrAxis::ChangesRequested.index()]);
        // Stated the strong way too: the result *is* the input, slot for slot.
        assert_eq!(shown, lit);
    }

    /// One axis at a time, so a shift by any amount in either direction fails on some axis.
    #[test]
    fn a_single_lit_axis_shows_exactly_its_own_entry() {
        for axis in PrAxis::ALL {
            let mut lit = [false; 3];
            lit[axis.index()] = true;
            let shown = pr_entry_visibility(lit, false);
            for other in PrAxis::ALL {
                assert_eq!(
                    shown[other.index()],
                    other == axis,
                    "lighting {axis:?} must show only {axis:?}, but {other:?} was {}",
                    shown[other.index()]
                );
            }
        }
    }

    /// Waiting to authorize hides all three whatever their counts: none can have a list behind it.
    #[test]
    fn needs_auth_hides_every_entry() {
        assert_eq!(pr_entry_visibility([true; 3], true), [false; 3]);
    }

    // ─── The hoot ledger ──────────────────────────────────────────────────────

    /// One pull request, with the two things the ledger compares.
    fn pr(key: &str, conflicting: bool) -> PrEntry {
        let mut e = PrEntry::stub(&format!("https://github.com/o/r/pull/{key}"));
        e.id = Some(key.to_string());
        e.conflicting = conflicting;
        e
    }

    /// The same pull request as `pr`, stamped with a given `updatedAt`.
    fn pr_at(key: &str, updated_at: &str) -> PrEntry {
        let mut e = pr(key, false);
        e.updated_at = Some(updated_at.to_string());
        e
    }

    fn fresh_prs(prs: &[PrEntry]) -> PollResponse {
        respond(PollResult::Fresh {
            present: !prs.is_empty(),
            count: Some(prs.len() as u32),
            prs: Some(prs.to_vec()),
        })
    }

    fn hooted(state: &mut PollState) -> bool {
        state.take_pr_arrivals()[PrAxis::ReviewRequested.index()]
    }

    fn ledger_state() -> PollState {
        PollState::new([true; 3])
    }

    /// **The bug this ledger exists for.** GitHub's search index is eventually consistent and drops a
    /// pull request from one answer only to return it in the next. Counting rises, that reads as
    /// `3 -> 2 -> 3` and hoots for a pull request the user was already told about.
    #[test]
    fn a_pr_that_vanishes_and_returns_unchanged_does_not_hoot() {
        let (a, b, c) = (
            pr("a", false),
            pr("b", false),
            pr("c", false),
        );
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[a.clone(), b.clone(), c.clone()]));
        assert!(hooted(&mut state), "the first answer is news");

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[a.clone(), b.clone()]));
        assert!(!hooted(&mut state), "losing one is never news");

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[a, b, c]));
        assert!(!hooted(&mut state), "the same PR in the same state is not new");
    }

    /// **The reported bug: hoots with no visible change.** A pull request already on the list gets a
    /// comment, a label, a push — anything that moves its `updatedAt` — and the ledger called that
    /// news. The count did not change, the list did not change, the tray did not flicker, and it
    /// hooted. The rule is a *new id appearing*, and this id was already here.
    #[test]
    fn activity_on_a_pull_request_already_listed_does_not_hoot() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr_at("a", "2026-09-15T09:00:00Z")]));
        assert!(hooted(&mut state), "the first sighting is news");

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr_at("a", "2026-09-15T09:05:00Z")]));
        assert!(!hooted(&mut state), "still the same pull request, however much it is discussed");
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr_at("a", "2026-09-15T09:10:00Z")]));
        assert!(!hooted(&mut state));
    }

    /// The case the count rule could never see: one leaves and one arrives in the same answer. The
    /// count is flat; the *id* is new; that hoots.
    #[test]
    fn a_swap_that_leaves_the_count_flat_hoots_for_the_new_id() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr("a", false)]));
        let _ = hooted(&mut state);

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr("b", false)]));
        assert!(hooted(&mut state), "b has never been seen on this axis");
    }

    /// The thing the ledger must not break.
    #[test]
    fn a_genuinely_new_pr_still_hoots() {
        let a = pr("a", false);
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(std::slice::from_ref(&a)));
        assert!(hooted(&mut state));

        state.apply_pr(
            PrAxis::ReviewRequested,
            fresh_prs(&[a, pr("b", false)]),
        );
        assert!(hooted(&mut state), "a key never seen before is news");
    }

    /// A known id coming back *untouched* is the index blip, and not news. `updatedAt` is how the
    /// ledger knows it was untouched: GitHub hides and re-serves the same row, and nothing on the pull
    /// request moved while it was hidden.
    #[test]
    fn a_known_pr_returning_unchanged_does_not_hoot() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr_at("a", "2026-09-15T09:00:00Z")]));
        assert!(hooted(&mut state));
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[]));
        assert!(!hooted(&mut state));
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr_at("a", "2026-09-15T09:00:00Z")]));
        assert!(!hooted(&mut state), "same id, same updatedAt: the index blinked, nothing happened");
    }

    /// **The reported bug: changes requested twice, one hoot.** PR 668 had changes requested and
    /// hooted. The fixes went in and it left the red list. Changes were requested *again*, it came
    /// back, and the ledger said "known id" and stayed silent. That is the normal round trip on this
    /// axis, not a corner case. A known id that comes back after an absence *with a newer `updatedAt`*
    /// left for a reason and came back for a reason, and the return is news.
    #[test]
    fn a_known_pr_returning_with_new_activity_hoots() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr_at("668", "2026-09-15T09:00:00Z")]));
        assert!(state.take_pr_arrivals()[PrAxis::ChangesRequested.index()], "first request hoots");

        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[]));
        assert!(!state.take_pr_arrivals()[PrAxis::ChangesRequested.index()], "leaving is silent");

        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr_at("668", "2026-09-15T14:30:00Z")]));
        assert!(
            state.take_pr_arrivals()[PrAxis::ChangesRequested.index()],
            "it left, work happened, it is back: that is a second request for changes and it hoots"
        );

        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr_at("668", "2026-09-15T14:31:00Z")]));
        assert!(
            !state.take_pr_arrivals()[PrAxis::ChangesRequested.index()],
            "once back on the list, activity on it is silent again"
        );
    }

    /// Without an `updatedAt` on either side there is nothing to compare, and the ledger errs quiet:
    /// a returning id it cannot judge is treated as the blip, not as news.
    #[test]
    fn a_known_pr_returning_without_timestamps_stays_silent() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr("a", false)]));
        assert!(hooted(&mut state));
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[]));
        assert!(!hooted(&mut state));
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr("a", false)]));
        assert!(!hooted(&mut state), "no timestamp to judge by: err quiet");
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[]));
        let _ = hooted(&mut state);
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[pr_at("a", "2026-09-15T09:00:00Z")]));
        assert!(!hooted(&mut state), "a timestamp appearing where there was none is not proof of work");
    }

    /// A conflict appearing on a listed pull request changes nothing visible on the tray — same
    /// count, same entry — so it makes no sound. One that puts a pull request *onto* the amber list
    /// is a new id there, and hoots as such (see `a_genuinely_new_pr_still_hoots`).
    #[test]
    fn a_conflict_appearing_on_a_listed_pr_does_not_hoot() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr("a", false)]));
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr("a", true)]));
        assert!(!state.take_pr_arrivals()[PrAxis::ChangesRequested.index()]);
    }

    /// And the other way is not news either: a conflict you resolved is work leaving.
    #[test]
    fn a_conflict_clearing_is_not_news() {
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr("a", true)]));
        let _ = state.take_pr_arrivals();

        state.apply_pr(PrAxis::ChangesRequested, fresh_prs(&[pr("a", false)]));
        assert!(!state.take_pr_arrivals()[PrAxis::ChangesRequested.index()]);
    }

    /// Unchanged behaviour: a run of failed polls holds the list, and recovering on the same pull
    /// requests is not news. This used to rest on the count being held; it now rests on the ledger,
    /// which never saw the failures at all.
    #[test]
    fn recovering_from_failures_on_the_same_prs_is_silent() {
        let a = pr("a", false);
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(std::slice::from_ref(&a)));
        assert!(hooted(&mut state));

        for _ in 0..4 {
            state.apply_pr(PrAxis::ReviewRequested, transient());
        }
        assert!(!hooted(&mut state));

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[a]));
        assert!(!hooted(&mut state), "the same PR after a blip is not a new one");
    }

    /// An answer the parser could not fully read carries no list (see `github::Parsed::prs`), so
    /// there is nothing to compare identities against and the count rule stands in.
    ///
    /// That is not a hole in the fix: the blip this ledger exists for returns a perfectly *readable*
    /// list both times, so it never reaches this path. Only a genuinely broken payload does, and
    /// there the count is the best evidence there is.
    ///
    /// The ledger itself is left untouched, so it still recognises the pull request afterwards.
    #[test]
    fn an_unreadable_list_falls_back_to_the_count_and_keeps_the_ledger() {
        let a = pr("a", false);
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(std::slice::from_ref(&a)));
        assert!(hooted(&mut state));

        state.apply_pr(PrAxis::ReviewRequested, fresh_count(9));
        assert!(hooted(&mut state), "no list to judge, so a rise is the only evidence left");

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[a]));
        assert!(!hooted(&mut state), "and the ledger still remembers it");
    }

    /// Launching into a queue that already has pull requests in it hoots once, which is what the
    /// empty ledger gives for free.
    #[test]
    fn a_first_answer_on_a_busy_queue_hoots_once() {
        let mut state = ledger_state();
        let prs = [pr("a", false), pr("b", false)];
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&prs));
        assert!(hooted(&mut state));
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&prs));
        assert!(!hooted(&mut state), "and not again");
    }

    /// Eviction is housekeeping, and it is the only thing that can bring the false hoot back: a pull
    /// request forgotten while the index was hiding it would sound on its return. So the bound is
    /// generous and by *size* rather than age — a key must sit absent while a whole cap's worth of
    /// other pull requests churn past before it is dropped.
    #[test]
    fn a_key_survives_far_more_absence_than_any_index_blip() {
        let a = pr("a", false);
        let mut state = ledger_state();
        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(std::slice::from_ref(&a)));
        assert!(hooted(&mut state));

        for i in 0..20 {
            state.apply_pr(
                PrAxis::ReviewRequested,
                fresh_prs(&[pr(&format!("x{i}"), false)]),
            );
            let _ = state.take_pr_arrivals();
        }

        state.apply_pr(PrAxis::ReviewRequested, fresh_prs(&[a]));
        assert!(!hooted(&mut state), "twenty polls away is still an index blip, not news");
    }

    /// But it is bounded, or a long-running tray accumulates every pull request it has ever seen.
    #[test]
    fn the_ledger_is_bounded() {
        let mut state = ledger_state();
        for i in 0..(LEDGER_CAP * 2) {
            state.apply_pr(
                PrAxis::ReviewRequested,
                fresh_prs(&[pr(&format!("x{i}"), false)]),
            );
            let _ = state.take_pr_arrivals();
        }
        assert!(state.ledger_len(PrAxis::ReviewRequested) <= LEDGER_CAP);
    }

    // ─── Rising counts (the hoot) ─────────────────────────────────────────────

    /// The plainest case: a *confirmed* zero turning into a confirmed one or more is news, and it is
    /// news exactly once.
    #[test]
    fn a_confirmed_zero_to_one_is_an_arrival_reported_once() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        assert_eq!(state.take_pr_arrivals(), [false; 3], "settling on zero is not an arrival");

        state.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        assert_eq!(state.take_pr_arrivals(), [true, false, false], "0 to 1 must be reported");
        assert_eq!(state.take_pr_arrivals(), [false; 3], "taking it must consume it");
    }

    /// The feature this rule was changed for: an axis that is *already* busy still speaks up when it
    /// gets busier. Under the presence-edge rule this was silent, because `Presence` was `Yes` both
    /// before and after — so three pull requests could land unannounced.
    #[test]
    fn a_growing_count_hoots_again() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        assert_eq!(state.take_pr_arrivals(), [true, false, false]);

        state.apply_pr(PrAxis::ReviewRequested, fresh_count(4));
        assert_eq!(state.take_pr_arrivals(), [true, false, false], "1 to 4 is three new PRs");
        assert_eq!(state.take_pr_arrivals(), [false; 3], "and it is still consumed by the take");
    }

    /// The other direction is not news. Work leaving is what the user wanted; it needs no sound, and
    /// a hoot on every close would make the app unbearable.
    #[test]
    fn a_shrinking_or_flat_count_is_silent() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(5));
        let _ = state.take_pr_arrivals();

        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        assert_eq!(state.take_pr_arrivals(), [false; 3], "5 to 2 is work going away");

        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        assert_eq!(state.take_pr_arrivals(), [false; 3], "2 to 2 is nothing at all");
    }

    /// A dip and a recovery back to the same number is a rise on the way up, and hoots. The pull
    /// request that closed is not the one that opened, so there is genuinely something new to look
    /// at even though the total is where it started.
    #[test]
    fn a_dip_and_a_climb_back_hoots_on_the_way_up() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        assert_eq!(state.take_pr_arrivals(), [false; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        assert_eq!(state.take_pr_arrivals(), [true, false, false]);
    }

    /// And it can fire again after the axis has genuinely gone quiet.
    #[test]
    fn going_quiet_re_arms_the_arrival() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        assert_eq!(state.take_pr_arrivals(), [true, false, false]);
    }

    /// The first confirmed answer of a launch counts, even though what it replaced was `Unknown`
    /// rather than a known zero: starting the app and finding PRs already waiting is exactly when the
    /// user wants telling.
    #[test]
    fn the_first_poll_of_a_launch_hoots_when_prs_are_already_waiting() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        assert_eq!(state.take_pr_arrivals(), [true, false, false], "a launch into a full queue hoots");
    }

    /// But only once. The second poll of that same launch has nothing new to say.
    #[test]
    fn the_launch_hoot_does_not_repeat_on_the_next_poll() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        assert_eq!(state.take_pr_arrivals(), [false; 3]);
    }

    /// And a launch into an empty queue is silent, which is what makes the hoot mean something.
    #[test]
    fn a_launch_into_an_empty_queue_is_silent() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        assert_eq!(state.take_pr_arrivals(), [false; 3]);
    }

    /// A failure streak recovering at the number it left is silent. The count is held through the
    /// failure precisely so this comparison has something real on both sides — otherwise every
    /// network blip would re-announce a number the user has already seen.
    #[test]
    fn recovering_from_unknown_at_the_same_count_is_silent() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        assert_eq!(state.take_pr_arrivals(), [false; 3], "we lost track and nothing had changed");
    }

    /// But recovering at a *higher* count does hoot, and must: those pull requests turned up while
    /// the poll was down, and they are exactly as new to the user as if we had been watching.
    #[test]
    fn recovering_from_unknown_at_a_higher_count_hoots() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        assert_eq!(state.take_pr_arrivals(), [true, false, false]);
    }

    /// `clear_pr_auth` hands out fresh `Track`s on purpose, so the axis has no previous count and
    /// falls back to the launch rule. Signing in and finding PRs waiting hoots, like a launch does.
    #[test]
    fn the_first_poll_after_a_sign_in_hoots() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(4));
        let _ = state.take_pr_arrivals();
        state.clear_pr_auth();
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(4));
        assert_eq!(state.take_pr_arrivals(), [true, false, false], "a fresh track has no history");
    }

    /// Each axis reports for itself, in `PrAxis::index` order.
    #[test]
    fn axes_report_arrivals_independently() {
        let mut state = PollState::new([true; 3]);
        for axis in PrAxis::ALL {
            state.apply_pr(axis, fresh_count(0));
        }
        let _ = state.take_pr_arrivals();
        state.apply_pr(PrAxis::ChangesRequested, fresh_count(1));
        assert_eq!(state.take_pr_arrivals(), [false, false, true]);
    }

    /// Notifications are not a PR axis and must never show up in the array.
    #[test]
    fn notifications_are_not_a_pr_arrival() {
        let mut state = PollState::new([true; 3]);
        assert_eq!(state.take_pr_arrivals(), [false; 3]);
    }

    // ─── Golden wording ──────────────────────────────────────────────────────
    //
    // What the tray says today, verbatim. These exist for the portal refactor: the promise there is
    // "nothing visible changes for a GitHub-only user", and a promise is worth more as a test than
    // as a sentence in a plan. Anything that moves one of these strings is a visible change and has
    // to say so on purpose.

    #[test]
    fn golden_menu_constants() {
        assert_eq!(authenticate_menu_label("GitHub"), "Authenticate GitHub PR Status");
        assert_eq!(STATUS_MENU_LABEL, "GitHub is githubing again, check status");
        assert_eq!(PR_INBOX_URL, "https://github.com/pulls/inbox");
        assert_eq!(PR_INBOX_MENU_LABEL, "Open PR inbox");
        assert_eq!(REVIEWS_MENU_LABEL, "Open Requested Reviews");
        assert_eq!(REPOSITORY_MENU_LABEL, "Open GitHoot on GitHub");
    }

    #[test]
    fn golden_tooltip_with_an_outage_and_every_axis() {
        let mut state = PollState::new([true; 3]);
        state.set_status_degraded(Some("Actions down".to_string()));
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        state.apply_pr(PrAxis::ReadyToMerge, fresh_count(1));
        state.apply_pr(PrAxis::ChangesRequested, fresh_count(0));
        assert_eq!(
            state.tooltip(),
            "GitHub: Actions down
2 PR(s) awaiting your review
1 PR(s) approved
Nothing needing your work"
        );
    }

    #[test]
    fn golden_tooltip_after_a_rejected_credential() {
        let mut state = new_state(true);
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        assert_eq!(state.tooltip(), "Review state unknown
GitHub rejected the credential");
    }

    /// The wording that names a portal comes from the state's own name, so a second portal reads
    /// as itself and the GitHub goldens above stay exactly as they were.
    #[test]
    fn a_portal_state_names_itself_in_its_wording() {
        let mut state = PollState::for_portal("GitLab", [true, false, false]);
        state.set_status_degraded(Some("Wobbly".to_string()));
        state.apply_pr(PrAxis::ReviewRequested, respond(PollResult::Unauthorized));
        assert_eq!(state.tooltip(), "GitLab: Wobbly
Review state unknown
GitLab rejected the credential");
        assert_eq!(authenticate_menu_label("GitLab"), "Authenticate GitLab PR Status");
    }

    #[test]
    fn golden_tooltip_before_authorization() {
        let mut state = PollState::new([true; 3]);
        state.require_pr_auth();
        assert_eq!(state.tooltip(), "PR status: not authorized yet. Use the menu to authorize.");
    }

    #[test]
    fn golden_menu_labels_with_counts() {
        let mut state = PollState::new([true; 3]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        state.apply_pr(PrAxis::ReadyToMerge, fresh_count(0));
        state.apply_pr(PrAxis::ChangesRequested, transient());
        assert_eq!(state.pr_menu_label(PrAxis::ReviewRequested), "Open Requested Reviews (3)");
        assert_eq!(state.pr_menu_label(PrAxis::ReadyToMerge), "Open Approved PRs");
        assert_eq!(state.pr_menu_label(PrAxis::ChangesRequested), "Open Work Required");
    }
}
