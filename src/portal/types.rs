//! The pull-request vocabulary every portal speaks.
//!
//! Nothing in here knows which portal an answer came from. `PrEntry` is what the page renders and
//! the hoot ledger files, `PollResult` is every outcome a poll can have, and both are the seam the
//! state machine sits on: `state` and `page` import from here and never from a portal module, which
//! is what lets a second portal arrive as a new file rather than as edits to the core.
//!
//! The field names still carry GitHub's shape in places (`node id`, `nameWithOwner`) because that is
//! where they were born; the doc comments say what each field *means* so another portal can fill it
//! honestly or leave it `None`.

use std::time::Duration;

/// One pull request, as the page shows it.
///
/// `url` is the only field the *count* depends on, and the only one that is not `Option`: a counted
/// hit without a URL poisons the whole list (see `parse_reviewed`), because a click that opens fewer
/// pages than the dot claims is the count and the click disagreeing. Everything else is lenient in
/// both directions — GitHub answers with partial `data` for a node the token cannot fully see, and a
/// hole here must cost a line of the page, never a number on the icon.
#[derive(Debug, Clone, PartialEq)]
pub struct PrEntry {
    /// GitHub's immutable node id, when it sent one. The hoot ledger keys on this rather than the
    /// URL because a repo rename changes a URL, and a pull request that merely moved would read as a
    /// brand-new one and hoot for nothing. `Option` like every other payload field — `key` falls back.
    pub id: Option<String>,
    pub url: String,
    pub title: Option<String>,
    /// `owner/name`, GitHub's `nameWithOwner`.
    pub repo: Option<String>,
    pub number: Option<u64>,
    /// `None` for a pull request whose author has since been deleted.
    pub author: Option<String>,
    /// GitHub's ISO-8601, kept raw. Rendering it is the page's job, not the client's.
    pub updated_at: Option<String>,
    pub is_draft: bool,
    /// The head branch conflicts with the base. Only ever `true` for a definite `CONFLICTING`; an
    /// uncomputed `UNKNOWN` is absence of evidence, not evidence of a clean merge.
    pub conflicting: bool,
    /// Open review threads started by GitHub's automatic reviewer, ignoring resolved and outdated
    /// ones. Zero whenever the axis did not ask for threads — see `COPILOT_REVIEWER`.
    pub copilot_unresolved: u32,
    pub checks: CheckRollup,
    /// One verdict per reviewer, `COMMENTED` already dropped by GitHub's own
    /// `latestOpinionatedReviews`.
    pub verdicts: Vec<Verdict>,
    /// Reviewers with a re-review outstanding.
    pub pending: Vec<Reviewer>,
}

/// A bare entry carrying nothing but its URL.
///
/// Lives here rather than in each test module so `state`'s tests, which care only about *which* PRs a
/// track holds, need not restate every display field to say so.
#[cfg(test)]
impl PrEntry {
    pub fn stub(url: &str) -> Self {
        PrEntry {
            id: None,
            url: url.to_string(),
            title: None,
            repo: None,
            number: None,
            author: None,
            updated_at: None,
            is_draft: false,
            conflicting: false,
            copilot_unresolved: 0,
            checks: CheckRollup::Unknown,
            verdicts: Vec::new(),
            pending: Vec::new(),
        }
    }
}

impl PrEntry {
    /// What the hoot ledger files this pull request under.
    pub fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.url)
    }
}

/// The head commit's combined check state.
///
/// `Unknown` covers three genuinely indistinguishable cases: a repository with no checks configured
/// (GitHub answers `null`), a payload hole, and a `FORBIDDEN` degraded away by `DEGRADABLE_FIELDS`.
/// All three must render neutral. Painting them red would invent a failure, which is the same lie in
/// the other direction as reporting a poll error as a zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckRollup {
    Unknown,
    Success,
    Pending,
    Failure,
    Error,
    Expected,
}

impl CheckRollup {
    pub(crate) fn from_state(state: &str) -> Self {
        match state {
            "SUCCESS" => CheckRollup::Success,
            "PENDING" => CheckRollup::Pending,
            "FAILURE" => CheckRollup::Failure,
            "ERROR" => CheckRollup::Error,
            "EXPECTED" => CheckRollup::Expected,
            // A state this version has never heard of is not evidence of anything.
            _ => CheckRollup::Unknown,
        }
    }
}

/// One reviewer's standing verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub login: String,
    pub state: ReviewState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
}

/// Someone a review is pending from.
///
/// A team is named by its slug because it has no login — which is exactly why `still_on_you` cannot
/// match one and treats a pending team request as "not handed back". The page can still say who is
/// being waited on, which beats an empty row that looks like nobody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reviewer {
    User(String),
    Team(String),
}

/// Everything GitHub can tell us, kept distinguishable because the caller must react
/// differently to each one.
#[derive(Debug)]
pub enum PollResult {
    /// A 200 with a usable body. `present` is authoritative.
    Fresh {
        present: bool,
        /// Exact match count. Always `Some` now that every endpoint reads its hits, but kept an
        /// `Option` because `Parsed` is the seam a future endpoint would arrive through.
        count: Option<u32>,
        /// The exact items `count` counted, when the endpoint reads its hits one by one — see
        /// `Parsed::prs`. Carried so the menu entry can show the very pull requests its dot is
        /// counting, which no search URL can express.
        prs: Option<Vec<PrEntry>>,
    },
    /// 401 — the token is dead. Waiting will not fix this; only re-authentication will.
    Unauthorized,
    /// 403/429 carrying a rate-limit signal. Hold state and wait exactly as instructed.
    RateLimited { retry_after: Duration },
    /// Anything else: transport failure, 5xx, unparseable body, or a 403 that is not about
    /// rate limiting (a permission the query needs, say). State is unknown, not clear.
    Transient(String),
}

impl PollResult {
    /// The detail worth logging when a poll did **not** go cleanly, or `None` when it did.
    ///
    /// `Fresh` is the expected outcome, so it stays silent — logging it on every cycle is what
    /// buried the one line that mattered. Every other variant names
    /// what went wrong and carries the same message the tooltip would show, so a non-OK response
    /// or a transport failure is never swallowed the way the GraphQL `FORBIDDEN` was: the field
    /// error rides in on `Transient`'s string.
    pub fn problem(&self) -> Option<String> {
        match self {
            PollResult::Fresh { .. } => None,
            PollResult::Unauthorized => Some("token rejected by GitHub (401)".to_string()),
            PollResult::RateLimited { retry_after } => {
                Some(format!("rate limited — holding for {}s", retry_after.as_secs()))
            }
            PollResult::Transient(detail) => Some(detail.clone()),
        }
    }
}

#[derive(Debug)]
pub struct PollResponse {
    pub result: PollResult,
    /// From `x-poll-interval`. GitHub raises this under load and we must obey it.
    pub poll_interval: Option<Duration>,
}
