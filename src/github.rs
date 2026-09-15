//! GitHub pull-request API client.
//!
//! The job of this module is to be *honest*: every outcome GitHub can produce maps to a
//! distinct variant, so the caller is never handed a plausible-looking zero in place of a
//! failure. An early version returned `Ok(0)` for any non-2xx response, which meant an expired
//! token or a rate limit rendered as a confident "nothing is waiting for you".

use crate::infoln;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, ACCEPT, AUTHORIZATION, RETRY_AFTER, USER_AGENT};
use reqwest::StatusCode;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GRAPHQL_URL: &str = "https://api.github.com/graphql";
const AGENT: &str = "githoot-tray";

/// How many search hits the changes-requested GraphQL poll inspects.
///
/// A cap rather than pagination, and it undercounts rather than overcounts: past 100 matching pull
/// requests the extras are simply not seen. Not reachable by the inbox this app exists for, and
/// paginating would trade a real increase in complexity for a case nobody has.
const SEARCH_HITS_CAP: u32 = 100;

/// How many opinionated reviews and pending requests the changes-requested query reads per hit.
///
/// Same undercount-not-overcount trade as `SEARCH_HITS_CAP`: a pull request with more than 20
/// opinionated reviews or 20 pending review requests could be judged on a partial list, which is
/// unreachable by this app's inbox.
const CHANGES_REVIEWS_CAP: u32 = 20;

/// How many review threads a hit is inspected for unresolved Copilot comments.
///
/// Same undercount-not-overcount trade as the caps above: a pull request whose 21st thread is the
/// only open Copilot one is judged on a partial list and stays dark.
///
/// Passed as `0` by the axes with no use for it, which is not a saving to sniff at — this connection
/// multiplies with `SEARCH_HITS_CAP`, so it is the most expensive thing in the document.
const REVIEW_THREADS_CAP: u32 = 20;

/// The login GitHub's automatic reviewer posts under.
///
/// Copilot reviews by **commenting**: it never approves and never requests changes, so
/// `latestOpinionatedReviews` drops it entirely and `still_on_you` cannot see it at all. An unresolved
/// review thread is the only verdict it leaves behind.
///
/// Matched on the stem, with any `[bot]` suffix trimmed first. REST spells it
/// `copilot-pull-request-reviewer[bot]`; GraphQL's `Bot.login` is not documented to agree, and picking
/// one spelling would be a coin toss that fails silently.
const COPILOT_REVIEWER: &str = "copilot-pull-request-reviewer";

/// Kept well under the 60s poll floor so a stalled request cannot delay the next one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// GitHub's fallback guidance when it signals a limit without saying for how long:
/// "wait for at least one minute before retrying".
const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);

/// Cap on how much of an error body ends up in a log line or tooltip.
const MAX_DETAIL_CHARS: usize = 200;

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
    /// The newest thing that has happened to this pull request: the later of `updatedAt` and the
    /// newest review's `submittedAt`.
    ///
    /// The hoot ledger compares this to decide whether a pull request that reappeared actually
    /// *changed*, and taking the max is what makes that safe without knowing which events GitHub
    /// chooses to bump `updatedAt` for — which it does not document, and which a review object
    /// cannot answer for itself since it carries only `submittedAt` and no update stamp. If a
    /// submitted review does not move `updatedAt`, its own timestamp still moves this.
    ///
    /// Fixed-width UTC ISO-8601, so a plain string comparison is chronological.
    pub activity: Option<String>,
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
            activity: None,
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
    fn from_state(state: &str) -> Self {
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

/// What a success-body parser extracts from a 2xx body.
///
/// The endpoints differ only here: everything about status codes and rate limits is shared,
/// so this is the single seam between them.
struct Parsed {
    /// Whether the signal is present at all — the one field every endpoint can answer.
    present: bool,
    /// Exact match count.
    count: Option<u32>,
    /// The exact items `count` counted, when the endpoint reads its hits one by one. Only the
    /// GraphQL queries do; for a server total whose members were never fetched this is `None` — as
    /// it also is when any counted hit came back without a URL, because a partial list would open
    /// fewer pages than the dot claims. `None` means "fall back to the search page", never "no PRs";
    /// `Some(vec![])` is the confirmed empty.
    prs: Option<Vec<PrEntry>>,
}

/// A success-body parser. A trait object rather than a `fn` pointer because two of them now close
/// over the `copilotReviews` setting.
type BodyParser<'a> = &'a dyn Fn(&str) -> Result<Parsed, String>;

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

/// Builds the shared HTTP client.
///
/// `reqwest`'s blocking client already defaults to a 30s timeout, but 30s inside a 60s poll
/// loop makes a single stalled request eat half the interval. 10s is plenty for one small
/// JSON GET.
pub fn build_client() -> reqwest::Result<Client> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
}

/// The GraphQL document behind both axes that judge pull requests by their reviews.
///
/// Search's `review:` qualifier is not a view of the reviews. It is a projection of `reviewDecision`,
/// GitHub's verdict on whether the base branch's *review policy* is satisfied, and a repository with no
/// such policy gets no verdict: `reviewDecision` stays `null` on every pull request, approved or not.
/// Measured on 2026-09-05 across a repository with no required-review rule: 100 of its last 100 pull
/// requests reported `null`, six of them carrying a genuine `APPROVED` review on the head commit, and
/// `review:approved` matched none of them. The PR page still paints its green "Approved" badge, because
/// the page reads the reviews. So this document reads what the page reads: `latestOpinionatedReviews`,
/// one verdict per reviewer, `COMMENTED` already dropped.
///
/// The changes-requested axis needs one list more. Re-requesting a review does not dismiss the reviewer's
/// earlier `CHANGES_REQUESTED` verdict — it only puts a pending request back on them — so the verdict
/// alone cannot tell "still on me" from "handed back". `reviewRequests` can, and Search has no qualifier
/// for it at all. Hence GraphQL for both: the axis's Search query string handed to `search` verbatim, plus
/// the lists needed to judge each hit client-side.
///
/// **The check rollup hangs off the pull request, never off `commits(last:1)`.** Resolving a
/// `PullRequestCommit` needs the App's **Contents: read** — read access to every line of source in
/// every installed repo, to power a tray icon — which this App deliberately does not request. That
/// detour left the merge-ready axis dead from 1.7.0 to 1.10.0, failing every poll with
/// `{"type":"FORBIDDEN","path":["search","nodes",0,"commits","nodes",0]}`. The rollup itself costs
/// nothing: the App still holds Checks: read and Commit statuses: read. It is read for *display*
/// only and never decides a count, which is why a refusal on it degrades one field instead of
/// failing the poll — see `DEGRADABLE_FIELDS`.
///
/// `rateLimit` is free — GitHub does not charge a query for asking what it cost — and it is the only
/// way to turn "three of these per cycle is probably fine" into a number. See `RateLimit`.
const PR_REVIEWS_DOCUMENT: &str = "\
query($q:String!,$hits:Int!,$reviews:Int!,$threads:Int!){\
  rateLimit{limit cost remaining}\
  search(query:$q,type:ISSUE,first:$hits){\
    nodes{...on PullRequest{\
      id url title number isDraft updatedAt mergeable \
      repository{nameWithOwner}\
      author{login}\
      statusCheckRollup{state}\
      latestOpinionatedReviews(first:$reviews){nodes{state submittedAt author{login}}}\
      reviewRequests(first:$reviews){nodes{requestedReviewer{__typename ...on User{login} ...on Team{slug}}}}\
      reviewThreads(first:$threads){nodes{isResolved isOutdated comments(first:1){nodes{author{login}}}}}\
    }}\
  }\
}";

/// Polls the user's own pull requests where a reviewer requested changes and the work is *still on them*.
///
/// `query` is the same Search query string the other axes use, handed to GraphQL's `search` verbatim, so
/// the server-side filter is unchanged and only the client-side intersection is new.
///
/// No `If-None-Match`: GraphQL is a POST and does not answer `304`.
pub fn poll_changes_requested(client: &Client, token: &str, query: &str, copilot: bool) -> PollResponse {
    poll_reviewed(
        client,
        token,
        query,
        &|body| parse_reviewed(body, |pr| work_required(pr, copilot)),
        copilot,
    )
}

/// Polls the pull requests waiting on the user's review.
///
/// The one axis with no client-side rule: `review-requested:@me` is a real server-side filter, so
/// every hit counts and the query string alone decides the number.
///
/// It read `total_count` off REST Search until 1.17.0. A total is a number with no members, and the
/// PR page has to name the pull requests behind the bar, which no total ever could. The query string
/// is handed to `search` unchanged, so the count does not move — except that it now shares the other
/// two axes' cap: past `SEARCH_HITS_CAP` matches the extras are not seen. That undercounts rather
/// than overcounts, and is unreachable by the inbox this app exists for.
pub fn poll_review_requested(client: &Client, token: &str, query: &str) -> PollResponse {
    // No threads: Copilot's comments on a pull request are its author's work, never its reviewer's.
    poll_reviewed(client, token, query, &|body| parse_reviewed(body, |_| true), false)
}

/// Polls the user's own open pull requests and counts the ones a reviewer approved.
///
/// `query` must *not* carry `review:approved`: that qualifier is the `reviewDecision` projection this
/// axis exists to get away from (see `PR_REVIEWS_DOCUMENT`). The server narrows to the user's open,
/// non-draft pull requests; `approved` judges each hit by its reviews.
pub fn poll_approved(client: &Client, token: &str, query: &str, copilot: bool) -> PollResponse {
    poll_reviewed(
        client,
        token,
        query,
        &|body| parse_reviewed(body, |pr| approved(pr, copilot)),
        copilot,
    )
}

/// The shared GraphQL request behind `poll_changes_requested` and `poll_approved`: same document, same
/// variables, one parser apiece.
fn poll_reviewed(
    client: &Client,
    token: &str,
    query: &str,
    parse_ok: BodyParser,
    threads: bool,
) -> PollResponse {
    let body = serde_json::json!({
        "query": PR_REVIEWS_DOCUMENT,
        "variables": {
            "q": query,
            "hits": SEARCH_HITS_CAP,
            "reviews": CHANGES_REVIEWS_CAP,
            // `0` where the axis has no use for them, keeping the document's most expensive
            // connection off the queries that would only pay for it.
            "threads": if threads { REVIEW_THREADS_CAP } else { 0 },
        },
    });
    let request = client.post(GRAPHQL_URL).json(&body);

    send(request, token, parse_ok)
}

/// Shared request/response plumbing.
///
/// No conditional request. `If-None-Match` existed for `/notifications`, the one endpoint that
/// answered `304`; GraphQL is a POST and never does, so the whole ETag path went with the
/// notification feature in 2.0.0.
fn send(
    request: reqwest::blocking::RequestBuilder,
    token: &str,
    parse_ok: BodyParser,
) -> PollResponse {
    let request = request
        .header(ACCEPT, "application/vnd.github+json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(USER_AGENT, AGENT);

    let response = match request.send() {
        Ok(response) => response,
        // A transport failure carries no headers, so there is no interval to learn from it.
        Err(e) => {
            return PollResponse {
                result: PollResult::Transient(format!("request failed: {e}")),
                poll_interval: None,
            }
        }
    };

    let status = response.status();
    let headers = response.headers().clone();
    let poll_interval = header_u64(&headers, "x-poll-interval").map(Duration::from_secs);

    let body = response.text().unwrap_or_default();

    PollResponse { result: classify_with(status, &headers, &body, unix_now(), parse_ok), poll_interval }
}

// ─── The changes-requested payload ────────────────────────────────────────────
//
// Every level is optional or lenient on purpose. GraphQL is free to answer with partial `data`
// alongside `errors`, and a node the token cannot fully see comes back with fields missing rather
// than as a failure. Each such hole is read as "no evidence this was handed back", which keeps the
// bar lit — see `still_on_you` for why that direction is the safe one.

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    #[serde(default)]
    data: Option<GraphQlData>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

/// One GraphQL error, read for its `message` and its `path`.
///
/// `path` names the response node the error applies to, and it is what tells "this one hit lost one
/// display field" from "the query broke". Segments mix field names and array indices, so it is a
/// `serde_json::Value` list rather than a list of strings:
/// `["search","nodes",3,"statusCheckRollup"]`.
///
/// GitHub also sends a `type` (`FORBIDDEN`, `RATE_LIMITED`, …). It is deliberately not read: the
/// decision below is about *what was refused*, not about why, and a rate limit that happened to land
/// on a degradable field is still a poll worth keeping. `serde` ignores unknown fields.
#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
    #[serde(default)]
    path: Option<Vec<serde_json::Value>>,
}

/// Fields a refusal may cost without costing the poll.
///
/// Every one of them is *display only*: nothing here decides whether a hit counts, so degrading it to
/// unknown changes a line of the page and nothing on the icon. An allowlist rather than a heuristic,
/// so no field the count depends on can drift into being silently non-fatal — the reason it is
/// spelled out rather than derived is that the opposite mistake, a whole poll failing on one
/// unreadable field, froze the merge-ready axis from 1.7.0 to 1.10.0.
const DEGRADABLE_FIELDS: [&str; 4] = ["statusCheckRollup", "author", "title", "repository"];

/// Whether `error` costs one hit one field rather than the whole poll.
///
/// True only when the path both descends into an individual search hit (`search`, `nodes`, an index)
/// **and** ends at a named degradable field. A refusal at `search` or `search.nodes` is the whole page
/// being denied, and an error with no path at all cannot be attributed to any hit — both stay fatal,
/// because reporting a confident zero is the one thing this module refuses to do.
fn is_degradable(error: &GraphQlError) -> bool {
    let Some(path) = error.path.as_deref() else { return false };
    let [first, second, third, rest @ ..] = path else { return false };
    if first.as_str() != Some("search") || second.as_str() != Some("nodes") || !third.is_number() {
        return false;
    }
    rest.last().and_then(|s| s.as_str()).is_some_and(|f| DEGRADABLE_FIELDS.contains(&f))
}

/// What the query cost and what is left, when GitHub answered the `rateLimit` field.
///
/// Logged rather than acted on. Three of these documents per cycle is an arithmetic guess until it is
/// a measurement, and the guess is not trustworthy: the obvious reading of GitHub's scoring says the
/// two-axis load already exceeded the ceiling, and it plainly did not. So the number is written to
/// `log.txt` at info and read there.
#[derive(Debug, Deserialize)]
struct RateLimit {
    limit: Option<u32>,
    cost: Option<u32>,
    remaining: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlData {
    search: SearchConnection,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
}

#[derive(Debug, Deserialize)]
struct SearchConnection {
    nodes: Vec<Option<PullRequestNode>>,
}

/// A search hit. Every field is `Option` because a node that is not a pull request matches the
/// inline fragment with an empty object — `is:pr` should prevent that, but the type system is a
/// cheaper guarantee than the query string — and because a node the token cannot fully see comes back
/// with fields missing rather than as a failure.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestNode {
    id: Option<String>,
    /// The PR's own web page. `Option` for the same lenience as everything else here: a hole
    /// in the payload must degrade the menu entry (fall back to the search page), not the count.
    url: Option<String>,
    title: Option<String>,
    number: Option<u64>,
    /// Absent is not a draft. The opposite default would mark every hit with a payload hole as a
    /// draft, which is a claim about the PR rather than an admission of ignorance.
    #[serde(default)]
    is_draft: bool,
    updated_at: Option<String>,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN`. Computed lazily by GitHub: it answers `UNKNOWN`
    /// until something asks, and asking is what starts the computation, so a cold poll sees
    /// `UNKNOWN` and the next one sees the truth. Only `CONFLICTING` is ever treated as a conflict
    /// — see `blocked_by_conflict`.
    mergeable: Option<String>,
    repository: Option<Repository>,
    author: Option<Author>,
    status_check_rollup: Option<StatusCheckRollup>,
    latest_opinionated_reviews: Option<ReviewConnection>,
    review_requests: Option<RequestConnection>,
    /// `None` for the axes that ask for `first: 0`, which is the same as "no open threads".
    review_threads: Option<ThreadConnection>,
}

#[derive(Debug, Deserialize)]
struct ThreadConnection {
    nodes: Vec<Option<ReviewThread>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewThread {
    is_resolved: Option<bool>,
    /// The thread points at a line that has since changed. GitHub's own UI hides these.
    is_outdated: Option<bool>,
    comments: Option<CommentConnection>,
}

#[derive(Debug, Deserialize)]
struct CommentConnection {
    nodes: Vec<Option<Comment>>,
}

#[derive(Debug, Deserialize)]
struct Comment {
    /// `None` for a comment whose author has since been deleted.
    author: Option<Author>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct StatusCheckRollup {
    state: String,
}

#[derive(Debug, Deserialize)]
struct ReviewConnection {
    nodes: Vec<Option<Review>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Review {
    state: String,
    /// When the review was submitted. A review object has no update stamp of its own, so this is the
    /// only timestamp it can contribute to `PrEntry::activity`.
    submitted_at: Option<String>,
    /// `None` for a review whose author has since been deleted.
    author: Option<Author>,
}

#[derive(Debug, Deserialize)]
struct Author {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RequestConnection {
    nodes: Vec<Option<ReviewRequest>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewRequest {
    requested_reviewer: Option<RequestedReviewer>,
}

#[derive(Debug, Deserialize)]
struct RequestedReviewer {
    #[serde(rename = "__typename")]
    typename: String,
    /// Present for a `User`; absent for a `Team`, which is the whole reason `typename` is read.
    login: Option<String>,
    /// A team's own handle. Read only so the page can name who is being waited on; `still_on_you`
    /// still cannot match a team, because a slug is not a login.
    slug: Option<String>,
}

/// Whether this pull request is still waiting on *you* rather than on a reviewer.
///
/// The rule: a pending review request from someone who actually requested changes means you handed it
/// back. A pending request from anyone else does not — adding a fresh reviewer while the original
/// blocker's objection stands leaves the work with you.
///
/// The remaining fallback returns `true`, i.e. keeps the bar lit: a pending *team* request cannot be
/// matched against a blocker's login, and erring that way keeps work you owe someone visible rather
/// than hiding it. A lit bar that should be dark costs a glance; a dark bar that should be lit is the
/// failure this whole module is written to avoid.
fn still_on_you(pr: &PullRequestNode) -> bool {
    let blockers: Vec<&str> = pr
        .latest_opinionated_reviews
        .iter()
        .flat_map(|c| c.nodes.iter().flatten())
        .filter(|review| review.state == "CHANGES_REQUESTED")
        .filter_map(|review| review.author.as_ref().map(|a| a.login.as_str()))
        .collect();

    // Nobody's objection stands, so nothing is on you by this half of the rule.
    //
    // This used to return `true`. The query was `review:changes_requested`, so GitHub had already
    // vouched that every hit carried an objection, and an empty blocker list meant we could not *see*
    // it — a deleted account, or a list truncated by the cap — where trusting the server beat trusting
    // our own reading. That premise is gone: the query now asks for every open pull request of yours,
    // because `mergeable` is not a search qualifier, so an empty list is the ordinary case and no
    // longer evidence of anything. Same stance `approved` has always taken about an empty review list.
    if blockers.is_empty() {
        return false;
    }

    let handed_back = pr
        .review_requests
        .iter()
        .flat_map(|c| c.nodes.iter().flatten())
        .filter_map(|request| request.requested_reviewer.as_ref())
        // A pending *team* request has no login to match. Resolving membership would cost extra
        // requests and org-level permissions to settle a case that barely occurs, since re-requesting
        // a review re-requests the individual. Treated as "not handed back".
        .filter(|reviewer| reviewer.typename == "User")
        .filter_map(|reviewer| reviewer.login.as_deref())
        .any(|login| blockers.contains(&login));

    !handed_back
}

/// Whether GitHub has definitely said the head branch conflicts with the base.
///
/// `UNKNOWN` and a missing field both read as "no conflict known", never as a conflict. GitHub
/// computes `mergeable` lazily and answers `UNKNOWN` until something asks; asking is what starts the
/// computation, so a cold poll sees `UNKNOWN` and the next cycle sees the answer. Counting `UNKNOWN`
/// would light the bar on every freshly pushed pull request and clear it a minute later, which is the
/// cry-wolf this whole axis is written to avoid. Undercounting for one cycle is the cheaper error.
fn is_conflicting(pr: &PullRequestNode) -> bool {
    pr.mergeable.as_deref() == Some("CONFLICTING")
}

/// Whether `login` is GitHub's automatic reviewer, however the API spelled it.
fn is_copilot(login: &str) -> bool {
    login.strip_suffix("[bot]").unwrap_or(login).eq_ignore_ascii_case(COPILOT_REVIEWER)
}

/// How many of Copilot's review threads are still open on this pull request.
///
/// Resolved threads are done, and **outdated** ones point at a line that has since changed — GitHub's
/// own UI hides those, and keeping the bar amber for a comment on code that no longer exists is the
/// cry-wolf this axis is written to avoid. The cost is that pushing something unrelated can outdate a
/// thread you never read, and the bar goes quiet on it; erring quiet is the direction chosen
/// everywhere else here.
///
/// Only the thread's **first** comment is fetched, because that is the one that started it: a human
/// replying to Copilot does not make the thread theirs.
fn copilot_unresolved(pr: &PullRequestNode) -> u32 {
    pr.review_threads
        .iter()
        .flat_map(|c| c.nodes.iter().flatten())
        .filter(|thread| thread.is_resolved != Some(true) && thread.is_outdated != Some(true))
        .filter(|thread| {
            thread
                .comments
                .iter()
                .flat_map(|c| c.nodes.iter().flatten())
                .filter_map(|comment| comment.author.as_ref())
                .any(|author| is_copilot(&author.login))
        })
        .count() as u32
}

/// Whether anybody is attached to this pull request.
///
/// A pending request means someone is waiting to review; a standing opinionated review means someone
/// already has. Either way a person is engaged, which is what turns a conflict from your own untidy
/// branch into work you owe somebody.
fn has_active_reviewers(pr: &PullRequestNode) -> bool {
    let reviewed = pr.latest_opinionated_reviews.iter().flat_map(|c| c.nodes.iter().flatten()).count();
    let requested = pr.review_requests.iter().flat_map(|c| c.nodes.iter().flatten()).count();
    reviewed + requested > 0
}

/// Whether a merge conflict is standing between this pull request and someone who wants to review it.
///
/// The second half of the work-required axis. A conflict on a pull request nobody is attached to
/// blocks nobody and is yours to deal with whenever; a conflict with a reviewer waiting is work you
/// owe them, which is the same situation a standing `CHANGES_REQUESTED` describes.
///
/// Drafts never reach here: `CHANGES_QUERY` carries `draft:false`, so the server filters them out.
fn blocked_by_conflict(pr: &PullRequestNode) -> bool {
    is_conflicting(pr) && has_active_reviewers(pr)
}

/// Whether this pull request needs work from its author before a reviewer can move.
///
/// Two independent reasons, counted once: a reviewer's objection still standing against it
/// (`still_on_you`), or a merge conflict with someone waiting (`blocked_by_conflict`). The bar used
/// to be only the first, and its query said so server-side; it now asks for every open pull request
/// of yours and decides here, because `mergeable` is not a search qualifier.
fn work_required(pr: &PullRequestNode, copilot: bool) -> bool {
    still_on_you(pr) || blocked_by_conflict(pr) || (copilot && copilot_unresolved(pr) > 0)
}

/// Whether a reviewer approved this pull request and nobody's objection stands against it.
///
/// The rule: at least one `APPROVED` among the latest opinionated reviews, and no `CHANGES_REQUESTED`.
/// The veto is what keeps this axis and the changes-requested one disjoint now that neither is filtered
/// server-side, and it is GitHub's own precedence: one reviewer's yes does not outrank another's no.
/// Approvals on a superseded commit count — the PR page shows them, and this bar reports news, it does
/// not gate a merge.
///
/// The fallback direction is the *opposite* of `still_on_you`'s. That predicate narrows a list the server
/// already vouched for, so a hole in the payload leaves the bar lit. This one is the only filter there
/// is: the server hands over every open pull request, and a missing or empty review list is simply no
/// evidence of approval. Lighting a green bar over a PR nobody approved would be the false claim.
fn approved(pr: &PullRequestNode, copilot: bool) -> bool {
    // Anything that puts the pull request on the work-required bar takes it off this one, so the two
    // never light for the same PR. Work before news, and the approval is still there to be told about
    // the moment you deal with it. The conflict veto needs no `has_active_reviewers` check of its own:
    // a pull request cannot be approved without a review, so anything reaching it already has one.
    if is_conflicting(pr) || (copilot && copilot_unresolved(pr) > 0) {
        return false;
    }
    let mut any_approved = false;
    for review in pr.latest_opinionated_reviews.iter().flat_map(|c| c.nodes.iter().flatten()) {
        match review.state.as_str() {
            "CHANGES_REQUESTED" => return false,
            "APPROVED" => any_approved = true,
            _ => {}
        }
    }
    any_approved
}

/// Builds the page's view of one hit, or `None` when it has no URL.
///
/// `None` is the *only* rejection, and it is what makes `parse_reviewed`'s `collect()` poison the
/// whole list on one hole. Every other field is allowed to be missing, because the page can say
/// "unknown" and the icon cannot.
fn entry_from(node: &PullRequestNode) -> Option<PrEntry> {
    let verdicts = node
        .latest_opinionated_reviews
        .iter()
        .flat_map(|c| c.nodes.iter().flatten())
        .filter_map(|review| {
            let state = match review.state.as_str() {
                "APPROVED" => ReviewState::Approved,
                "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
                // `latestOpinionatedReviews` drops `COMMENTED` already; anything else is a state
                // this version has never heard of and has no row to put it in.
                _ => return None,
            };
            Some(Verdict { login: review.author.as_ref()?.login.clone(), state })
        })
        .collect();

    let pending = node
        .review_requests
        .iter()
        .flat_map(|c| c.nodes.iter().flatten())
        .filter_map(|request| request.requested_reviewer.as_ref())
        .filter_map(|reviewer| match reviewer.typename.as_str() {
            "User" => reviewer.login.clone().map(Reviewer::User),
            "Team" => reviewer.slug.clone().map(Reviewer::Team),
            _ => None,
        })
        .collect();

    // The latest of everything dated on this hit. `max` over `Option<&str>` would treat `None` as
    // smaller than any string, which is exactly the wanted behaviour, but spelling it out with
    // `flatten` keeps "no timestamps at all" as `None` rather than as an empty string.
    let newest_review = node
        .latest_opinionated_reviews
        .iter()
        .flat_map(|c| c.nodes.iter().flatten())
        .filter_map(|review| review.submitted_at.as_deref())
        .max();
    let activity = [node.updated_at.as_deref(), newest_review].into_iter().flatten().max();

    Some(PrEntry {
        id: node.id.clone(),
        url: node.url.clone()?,
        title: node.title.clone(),
        repo: node.repository.as_ref().map(|r| r.name_with_owner.clone()),
        number: node.number,
        author: node.author.as_ref().map(|a| a.login.clone()),
        updated_at: node.updated_at.clone(),
        activity: activity.map(str::to_string),
        is_draft: node.is_draft,
        conflicting: is_conflicting(node),
        copilot_unresolved: copilot_unresolved(node),
        checks: node
            .status_check_rollup
            .as_ref()
            .map_or(CheckRollup::Unknown, |r| CheckRollup::from_state(&r.state)),
        verdicts,
        pending,
    })
}

/// Shared body of the two GraphQL parsers: one `PR_REVIEWS_DOCUMENT` answer, one predicate per axis.
///
/// GraphQL answers `200 OK` and puts failures in an `errors` array, so `classify_with` cannot see them
/// from the status line. A parser that read only `data` would turn any such failure into a confident
/// **zero** — a dark bar meaning "the request broke". Errors are therefore checked before anything else
/// and surface as `Err`, which becomes `Transient`, which leaves the previous count standing.
fn parse_reviewed(body: &str, keep: impl Fn(&PullRequestNode) -> bool) -> Result<Parsed, String> {
    let response: GraphQlResponse =
        serde_json::from_str(body).map_err(|e| format!("unparseable PR review payload: {e}"))?;

    // Split before anything else. An error that cost one hit one display field leaves a perfectly
    // good answer behind, and failing on it would freeze the bar at its last count forever — the
    // failure that killed the merge-ready axis for four releases. Everything else is still fatal.
    let (degradable, fatal): (Vec<_>, Vec<_>) =
        response.errors.iter().partition(|e| is_degradable(e));
    if !fatal.is_empty() {
        let joined = fatal.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
        return Err(format!("GraphQL reported an error: {joined}"));
    }
    if !degradable.is_empty() {
        // Once per poll, not once per hit: a token that cannot read the rollup cannot read it on any
        // of a hundred hits, and a hundred identical lines would bury the log this exists to serve.
        infoln!(
            "{} pull request field(s) unreadable, shown as unknown: {}",
            degradable.len(),
            truncate(degradable[0].message.trim(), MAX_DETAIL_CHARS)
        );
    }

    let data = response
        .data
        .ok_or_else(|| "GraphQL answered with neither data nor errors".to_string())?;

    if let Some(limit) = &data.rate_limit {
        infoln!(
            "GraphQL rate limit: cost {}, {} of {} remaining",
            limit.cost.map_or_else(|| "?".to_string(), |v| v.to_string()),
            limit.remaining.map_or_else(|| "?".to_string(), |v| v.to_string()),
            limit.limit.map_or_else(|| "?".to_string(), |v| v.to_string()),
        );
    }

    let counted: Vec<&PullRequestNode> =
        data.search.nodes.iter().flatten().filter(|pr| keep(pr)).collect();
    let count = counted.len() as u32;
    // All-or-nothing: a counted hit without a URL would make the menu entry open fewer pages
    // than the dot claims, so one hole sends the whole click to the search-page fallback.
    let prs: Option<Vec<PrEntry>> = counted.iter().map(|pr| entry_from(pr)).collect();
    Ok(Parsed { present: count > 0, count: Some(count), prs })
}

/// Maps one HTTP response onto a `PollResult`.
///
/// Pure on purpose — `now` is injected rather than read from the clock — so every branch below
/// is unit-testable without a network or a fixture server. All of the historical bugs lived
/// here, so this is where the tests point.
///
/// The status handling is shared by both endpoints and only the success-body parser differs,
/// via `parse_ok`. Duplicating this for search would mean two copies of the 304-before-non-2xx
/// ordering and the rate-limit precedence — i.e. two places for the original bug to come back.
fn classify_with(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
    now: u64,
    parse_ok: BodyParser,
) -> PollResult {
    // Never transient: a rejected token stays rejected until it is replaced.
    if status == StatusCode::UNAUTHORIZED {
        return PollResult::Unauthorized;
    }

    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        return match rate_limit_wait(headers, body, now) {
            Some(retry_after) => PollResult::RateLimited { retry_after },
            // A 403 with no rate-limit signal is a different animal — most likely the token
            // lacks the scope or permission the endpoint needs. Backing off will not help, but
            // claiming there is nothing pending is still the one unacceptable answer.
            None => PollResult::Transient(describe(status, body)),
        };
    }

    if !status.is_success() {
        return PollResult::Transient(describe(status, body));
    }

    match parse_ok(body) {
        Ok(parsed) => PollResult::Fresh {
            present: parsed.present,
            count: parsed.count,
            prs: parsed.prs,
        },
        Err(why) => PollResult::Transient(why),
    }
}

/// GitHub's documented precedence for how long to wait after being limited.
///
/// Returns `None` when the response carries no rate-limit signal at all, which is how the
/// caller distinguishes "slow down" from an unrelated 403.
///
/// The body is read, not just the headers, because a **secondary** rate limit can arrive with
/// neither `retry-after` nor an exhausted primary quota — the quota headers still say there is
/// plenty left, and the only thing naming the limit is the JSON message. Read from headers alone
/// that response looks like a permission failure, which is a `Transient` — and three of those in
/// a row make every axis give up its last known count and blank the tray. Holding the count is
/// the whole point: a secondary limit says "you asked too fast", never "there is nothing there".
fn rate_limit_wait(headers: &HeaderMap, body: &str, now: u64) -> Option<Duration> {
    // 1. `retry-after` is an instruction, not a suggestion. Retrying inside this window is
    //    how a short secondary limit becomes a long one.
    if let Some(secs) = header_u64(headers, RETRY_AFTER.as_str()) {
        return Some(Duration::from_secs(secs));
    }

    // 2. Primary quota exhausted — wait for the reset timestamp.
    if header_u64(headers, "x-ratelimit-remaining") == Some(0) {
        let reset = header_u64(headers, "x-ratelimit-reset").unwrap_or(0);
        let wait = reset.saturating_sub(now);
        return Some(Duration::from_secs(wait.max(DEFAULT_RATE_LIMIT_WAIT.as_secs())));
    }

    // 3. A secondary limit that said so only in the body. Both wordings are matched: GitHub
    //    renamed "abuse detection mechanism" to "secondary rate limit" and still emits the old
    //    text from some endpoints. Lowercased first so a change of capitalisation cannot silently
    //    turn this back into a blanked tray.
    if is_secondary_rate_limit(body) {
        return Some(DEFAULT_RATE_LIMIT_WAIT);
    }

    None
}

/// Whether a 403/429 body is GitHub saying "too fast" rather than "not allowed".
fn is_secondary_rate_limit(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("secondary rate limit") || body.contains("abuse detection mechanism")
}

fn describe(status: StatusCode, body: &str) -> String {
    format!("HTTP {status}: {}", truncate(body.trim(), MAX_DETAIL_CHARS))
}

/// Truncates on a char boundary so a multi-byte body cannot panic the formatter.
fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}…", &text[..idx]),
        None => text.to_string(),
    }
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    header_string(headers, name)?.trim().parse().ok()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        map
    }

    /// Shorthand: classify a REVIEW-REQUESTED (GraphQL) response.
    ///
    /// Named `search` historically, when this axis read REST Search. The name is kept because a dozen
    /// status-line and rate-limit tests below drive it, and none of them looks at the success body —
    /// so repointing it at the GraphQL parser preserves that coverage exactly rather than rewriting it.
    fn search(status: StatusCode, h: &HeaderMap, body: &str, now: u64) -> PollResult {
        classify_with(status, h, body, now, &|b| parse_reviewed(b, |_| true))
    }

    // ── Status-line and rate-limit handling ───────────────────────────────────

    /// Previously `.unwrap_or_default()` swallowed this into "nothing waiting".
    #[test]
    fn a_malformed_body_is_transient_not_clear() {
        assert!(matches!(
            search(StatusCode::OK, &headers(&[]), "not json at all", 0),
            PollResult::Transient(_)
        ));
    }

    // ── Changes-requested body parsing ────────────────────────────────────────

    /// Shorthand: classify a CHANGES-REQUESTED (GraphQL) response.
    fn changes(status: StatusCode, h: &HeaderMap, body: &str, now: u64) -> PollResult {
        classify_with(status, h, body, now, &|b| parse_reviewed(b, |pr| work_required(pr, true)))
    }

    /// One search hit, described by who blocked it and who has a re-review pending.
    ///
    /// `blockers` are logins whose latest opinionated review requested changes; `pending` are
    /// `(typename, login)` pairs for the pending review requests, so a `Team` can be expressed.
    fn hit(blockers: &[&str], pending: &[(&str, &str)]) -> String {
        let reviews = blockers
            .iter()
            .map(|l| format!(r#"{{"state":"CHANGES_REQUESTED","author":{{"login":"{l}"}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let requests = pending
            .iter()
            .map(|(kind, login)| {
                let reviewer = if *kind == "Team" {
                    format!(r#"{{"__typename":"Team","name":"{login}"}}"#)
                } else {
                    format!(r#"{{"__typename":"User","login":"{login}"}}"#)
                };
                format!(r#"{{"requestedReviewer":{reviewer}}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"latestOpinionatedReviews":{{"nodes":[{reviews}]}},"reviewRequests":{{"nodes":[{requests}]}}}}"#
        )
    }

    /// A hit that also carries its web URL, the way the real payload does.
    fn hit_at(url: &str, blockers: &[&str], pending: &[(&str, &str)]) -> String {
        let bare = hit(blockers, pending);
        format!(r#"{{"url":"{url}",{}"#, &bare[1..])
    }

    fn payload(hits: &[String]) -> String {
        format!(r#"{{"data":{{"search":{{"nodes":[{}]}}}}}}"#, hits.join(","))
    }

    fn count_of(body: &str) -> u32 {
        match changes(StatusCode::OK, &headers(&[]), body, 0) {
            PollResult::Fresh { count: Some(n), .. } => n,
            other => panic!("expected Fresh with a count, got {:?}", other),
        }
    }

    /// The bug this whole endpoint switch exists for: changes applied, review re-requested, and the
    /// bar stayed lit because `review:changes_requested` still matched.
    #[test]
    fn a_re_review_pending_from_the_blocker_is_not_on_you() {
        let body = payload(&[hit(&["alice"], &[("User", "alice")])]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { present, count, .. } => {
                assert!(!present, "handed back to the reviewer, so nothing is on you");
                assert_eq!(count, Some(0));
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    #[test]
    fn changes_requested_with_nothing_pending_is_still_on_you() {
        assert_eq!(count_of(&payload(&[hit(&["alice"], &[])])), 1);
    }

    /// Adding a fresh reviewer is not the same as addressing the blocker's objection.
    #[test]
    fn a_pending_request_for_someone_else_leaves_it_on_you() {
        assert_eq!(count_of(&payload(&[hit(&["alice"], &[("User", "bob")])])), 1);
    }

    /// A team has no login to intersect, so it cannot prove the blocker was asked again.
    #[test]
    fn a_pending_team_request_does_not_count_as_handing_it_back() {
        assert_eq!(count_of(&payload(&[hit(&["alice"], &[("Team", "backend")])])), 1);
    }

    /// Nobody's objection stands and nothing conflicts, so there is no work to report.
    ///
    /// This asserted the opposite until the axis grew its conflict half. The query was
    /// `review:changes_requested`, so GitHub had vouched that every hit carried an objection and an
    /// empty blocker list meant we could not see it. The query now asks for every open pull request
    /// of yours, so an empty blocker list is the ordinary case — most of your PRs — and counting it
    /// would light the bar permanently.
    #[test]
    fn a_hit_with_nothing_standing_against_it_is_not_work() {
        assert_eq!(count_of(&payload(&[hit(&[], &[("User", "alice")])])), 0);
        assert_eq!(count_of(&payload(&[r#"{}"#.to_string()])), 0, "a node with no fields at all");
    }

    #[test]
    fn several_blockers_need_all_of_them_asked_again() {
        let one_of_two = payload(&[hit(&["alice", "bob"], &[("User", "alice")])]);
        assert_eq!(
            count_of(&one_of_two),
            0,
            "any blocker asked again means the ball has moved, even if others also objected"
        );
    }

    #[test]
    fn a_mixed_page_counts_only_the_ones_still_on_you() {
        let body = payload(&[
            hit(&["alice"], &[("User", "alice")]), // handed back
            hit(&["bob"], &[]),                    // on you
            hit(&["carol"], &[("User", "dave")]),  // on you: wrong reviewer asked
            hit(&["erin"], &[("Team", "core")]),   // on you: team request proves nothing
        ]);
        assert_eq!(count_of(&body), 3);
    }

    /// The gotcha that makes this endpoint different from the other two: GraphQL reports failure with
    /// `200 OK` and an `errors` array. Reading only `data` would render a broken request as a
    /// confident zero — the one answer this module must never give.
    #[test]
    fn a_graphql_error_at_status_200_is_transient_not_zero() {
        let body = r#"{"data":null,"errors":[{"message":"Something went wrong"}]}"#;
        match changes(StatusCode::OK, &headers(&[]), body, 0) {
            PollResult::Transient(why) => assert!(why.contains("Something went wrong")),
            other => panic!("expected Transient, got {:?}", other),
        }
    }

    /// Partial success is still failure: an `errors` array alongside usable `data` means the page we
    /// were handed is incomplete, so counting it would undercount.
    #[test]
    fn errors_win_even_when_data_is_present() {
        let body = format!(
            r#"{{"data":{{"search":{{"nodes":[{}]}}}},"errors":[{{"message":"partial"}}]}}"#,
            hit(&["alice"], &[])
        );
        assert!(matches!(changes(StatusCode::OK, &headers(&[]), &body, 0), PollResult::Transient(_)));
    }

    #[test]
    fn a_response_with_neither_data_nor_errors_is_transient() {
        assert!(matches!(
            changes(StatusCode::OK, &headers(&[]), "{}", 0),
            PollResult::Transient(_)
        ));
    }

    #[test]
    fn malformed_changes_requested_body_is_transient() {
        assert!(matches!(
            changes(StatusCode::OK, &headers(&[]), "not json at all", 0),
            PollResult::Transient(_)
        ));
    }

    /// An empty page is a real answer, unlike every failure above.
    #[test]
    fn no_hits_means_nothing_is_on_you() {
        match changes(StatusCode::OK, &headers(&[]), &payload(&[]), 0) {
            PollResult::Fresh { present, count, .. } => {
                assert!(!present);
                assert_eq!(count, Some(0));
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// The click target follows the count: only the still-on-you hits' URLs are handed out, in
    /// the order GitHub returned them, so the menu entry opens exactly what the dot claims.
    #[test]
    fn entries_are_collected_for_exactly_the_counted_hits() {
        let body = payload(&[
            hit_at("https://github.com/o/r/pull/1", &["alice"], &[("User", "alice")]), // handed back
            hit_at("https://github.com/o/r/pull/2", &["bob"], &[]),                    // on you
            hit_at("https://github.com/o/r/pull/3", &["carol"], &[("User", "dave")]),  // on you
        ]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { count, prs, .. } => {
                assert_eq!(count, Some(2));
                let opened: Vec<String> =
                    prs.expect("a confirmed list").iter().map(|e| e.url.clone()).collect();
                assert_eq!(
                    opened,
                    vec![
                        "https://github.com/o/r/pull/2".to_string(),
                        "https://github.com/o/r/pull/3".to_string(),
                    ]
                );
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// One counted hit without a URL poisons the whole list: opening two pages under a dot that
    /// says three would be the count and the click disagreeing — the fallback page is honest.
    #[test]
    fn a_counted_hit_without_a_url_yields_no_list_but_keeps_the_count() {
        let body = payload(&[
            hit_at("https://github.com/o/r/pull/2", &["bob"], &[]),
            hit(&["carol"], &[]), // counted, but the payload hole ate its URL
        ]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { count, prs, .. } => {
                assert_eq!(count, Some(2));
                assert_eq!(prs, None);
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// A confirmed-empty page is `Some(vec![])`, not `None` — the difference between "no PRs"
    /// and "could not read the list", which the menu fallback relies on.
    #[test]
    fn no_hits_yields_a_confirmed_empty_list() {
        match changes(StatusCode::OK, &headers(&[]), &payload(&[]), 0) {
            PollResult::Fresh { prs, .. } => assert_eq!(prs, Some(vec![])),
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// The document has to name every variable it uses, or GitHub rejects the whole query.
    #[test]
    fn the_query_document_declares_the_variables_it_sends() {
        for var in ["$q", "$hits", "$reviews"] {
            assert!(
                PR_REVIEWS_DOCUMENT.matches(var).count() >= 2,
                "{var} should be both declared and used"
            );
        }
    }

    // ── Approved body parsing ─────────────────────────────────────────────────

    /// Shorthand: classify an APPROVED (GraphQL) response.
    fn approved_resp(status: StatusCode, h: &HeaderMap, body: &str, now: u64) -> PollResult {
        classify_with(status, h, body, now, &|b| parse_reviewed(b, |pr| approved(pr, true)))
    }

    /// One search hit with the given latest opinionated review states, each from a distinct reviewer.
    fn reviewed(url: &str, states: &[&str]) -> String {
        let reviews = states
            .iter()
            .enumerate()
            .map(|(i, s)| format!(r#"{{"state":"{s}","author":{{"login":"r{i}"}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"url":"{url}","latestOpinionatedReviews":{{"nodes":[{reviews}]}},"reviewRequests":{{"nodes":[]}}}}"#
        )
    }

    /// The case that motivated the axis: a real approval in a repository where `reviewDecision` is
    /// `null`, so `review:approved` never matched it. Read off the reviews, it counts.
    #[test]
    fn an_approval_lights_the_bar_and_hands_out_its_entry() {
        let body = payload(&[reviewed("https://github.com/o/r/pull/2204", &["APPROVED"])]);
        match approved_resp(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { present, count, prs, .. } => {
                assert!(present);
                assert_eq!(count, Some(1));
                assert_eq!(
                    prs.map(|l| l[0].url.clone()),
                    Some("https://github.com/o/r/pull/2204".to_string())
                );
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// One reviewer's yes does not outrank another's no. This is also what keeps the green and amber
    /// bars disjoint now that neither is filtered server-side.
    #[test]
    fn a_standing_objection_vetoes_an_approval() {
        let body = payload(&[
            reviewed("https://github.com/o/r/pull/1", &["APPROVED", "CHANGES_REQUESTED"]),
            reviewed("https://github.com/o/r/pull/2", &["CHANGES_REQUESTED", "APPROVED"]),
            reviewed("https://github.com/o/r/pull/3", &["APPROVED", "APPROVED"]),
        ]);
        match approved_resp(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { count, prs, .. } => {
                assert_eq!(count, Some(1), "only the unanimously approved PR counts");
                assert_eq!(
                    prs.map(|l| l[0].url.clone()),
                    Some("https://github.com/o/r/pull/3".to_string())
                );
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// The server hands over *every* open PR, so this predicate is the only filter there is. Anything
    /// short of an `APPROVED` verdict — no reviews, a review list missing from the payload, or a state
    /// this parser does not know — must read as "not approved", the reverse of `still_on_you`'s lenience.
    #[test]
    fn without_an_approved_verdict_nothing_is_counted() {
        let body = payload(&[
            reviewed("https://github.com/o/r/pull/1", &[]),
            reviewed("https://github.com/o/r/pull/2", &["COMMENTED"]),
            reviewed("https://github.com/o/r/pull/3", &["DISMISSED"]),
            r#"{"url":"https://github.com/o/r/pull/4"}"#.to_string(),
        ]);
        match approved_resp(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { present, count, prs, .. } => {
                assert!(!present);
                assert_eq!(count, Some(0));
                assert_eq!(prs, Some(vec![]), "confirmed empty, not unreadable");
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// Same error path as the changes-requested parser: a GraphQL-level failure must hold the last
    /// count rather than darken the bar.
    #[test]
    fn approved_graphql_errors_are_transient() {
        let body = r#"{"data":null,"errors":[{"message":"API rate limit exceeded"}]}"#;
        assert!(matches!(
            approved_resp(StatusCode::OK, &headers(&[]), body, 0),
            PollResult::Transient(_)
        ));
    }

    // ── Search body parsing ───────────────────────────────────────────────────

    #[test]
    fn no_hits_means_no_review_pending() {
        match search(StatusCode::OK, &headers(&[]), &payload(&[]), 0) {
            PollResult::Fresh { present, count, .. } => {
                assert!(!present);
                assert_eq!(count, Some(0));
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// Every hit counts on this axis, and each one is named — which is the whole reason it left
    /// `total_count` behind: a number the page cannot expand into pull requests is not enough.
    #[test]
    fn every_hit_counts_for_review_requested_and_each_is_named() {
        let body = payload(&[
            hit_at("https://github.com/o/r/pull/1", &["alice"], &[("User", "alice")]),
            hit_at("https://github.com/o/r/pull/2", &[], &[]),
        ]);
        match search(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { present, count, prs, .. } => {
                assert!(present);
                assert_eq!(count, Some(2), "no client-side rule narrows this axis");
                let opened: Vec<String> =
                    prs.expect("a confirmed list").iter().map(|e| e.url.clone()).collect();
                assert_eq!(
                    opened,
                    vec![
                        "https://github.com/o/r/pull/1".to_string(),
                        "https://github.com/o/r/pull/2".to_string(),
                    ]
                );
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// A body carrying neither `data` nor `errors` must not read as "nothing to review".
    #[test]
    fn a_body_with_neither_data_nor_errors_is_transient() {
        assert!(matches!(
            search(StatusCode::OK, &headers(&[]), r#"{"items":[]}"#, 0),
            PollResult::Transient(_)
        ));
    }

    #[test]
    fn malformed_search_body_is_transient() {
        assert!(matches!(
            search(StatusCode::OK, &headers(&[]), "<html>502</html>", 0),
            PollResult::Transient(_)
        ));
    }

    // ── Status handling, shared by both endpoints ─────────────────────────────

    #[test]
    fn unauthorized_is_distinct_from_transient() {
        assert!(matches!(search(StatusCode::UNAUTHORIZED, &headers(&[]), "{}", 0), PollResult::Unauthorized));
        assert!(matches!(search(StatusCode::UNAUTHORIZED, &headers(&[]), "{}", 0), PollResult::Unauthorized));
    }

    #[test]
    fn retry_after_takes_precedence() {
        let h = headers(&[("retry-after", "42"), ("x-ratelimit-remaining", "0")]);
        match search(StatusCode::FORBIDDEN, &h, "", 0) {
            PollResult::RateLimited { retry_after } => assert_eq!(retry_after, Duration::from_secs(42)),
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn exhausted_quota_waits_for_reset() {
        let h = headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1000")]);
        match search(StatusCode::FORBIDDEN, &h, "", 100) {
            PollResult::RateLimited { retry_after } => assert_eq!(retry_after, Duration::from_secs(900)),
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    /// A reset that has already elapsed must not produce a zero-second wait.
    #[test]
    fn stale_reset_falls_back_to_the_minimum() {
        let h = headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "50")]);
        match search(StatusCode::FORBIDDEN, &h, "", 9_999) {
            PollResult::RateLimited { retry_after } => assert_eq!(retry_after, DEFAULT_RATE_LIMIT_WAIT),
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn forbidden_without_rate_limit_signal_is_transient() {
        // e.g. the GitHub App lacks the Pull requests permission.
        assert!(matches!(
            search(StatusCode::FORBIDDEN, &headers(&[]), "missing permission", 0),
            PollResult::Transient(_)
        ));
    }

    /// The real 403 that blanked the tray: a secondary limit, no `retry-after`, and quota headers
    /// that still look healthy. Header-only classification called this a permission failure.
    #[test]
    fn secondary_rate_limit_in_the_body_is_rate_limited_not_transient() {
        let h = headers(&[("x-ratelimit-remaining", "4998")]);
        let body = r#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again.","documentation_url":"https://docs.github.com/rest/overview/rate-limits-for-the-rest-api"}"#;
        match search(StatusCode::FORBIDDEN, &h, body, 0) {
            PollResult::RateLimited { retry_after } => assert_eq!(retry_after, DEFAULT_RATE_LIMIT_WAIT),
            other => panic!("expected RateLimited, got {:?}", other),
        }
        assert!(matches!(
            search(StatusCode::FORBIDDEN, &h, body, 0),
            PollResult::RateLimited { .. }
        ));
    }

    /// GitHub's older wording for the same thing, still emitted by some endpoints.
    #[test]
    fn abuse_detection_wording_is_rate_limited() {
        let body = r#"{"message":"You have triggered an abuse detection mechanism."}"#;
        assert!(matches!(
            search(StatusCode::FORBIDDEN, &headers(&[]), body, 0),
            PollResult::RateLimited { .. }
        ));
    }

    /// An explicit `retry-after` still outranks the body's generic minute.
    #[test]
    fn retry_after_outranks_the_secondary_limit_default() {
        let h = headers(&[("retry-after", "120")]);
        match search(StatusCode::FORBIDDEN, &h, "secondary rate limit", 0) {
            PollResult::RateLimited { retry_after } => assert_eq!(retry_after, Duration::from_secs(120)),
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn too_many_requests_is_rate_limited() {
        let h = headers(&[("retry-after", "7")]);
        assert!(matches!(
            search(StatusCode::TOO_MANY_REQUESTS, &h, "", 0),
            PollResult::RateLimited { .. }
        ));
    }

    #[test]
    fn server_error_is_transient() {
        assert!(matches!(
            search(StatusCode::INTERNAL_SERVER_ERROR, &headers(&[]), "boom", 0),
            PollResult::Transient(_)
        ));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("\u{fc}n\u{ef}c\u{f6}d\u{e9}", 3), "\u{fc}n\u{ef}\u{2026}");
        assert_eq!(truncate("short", 50), "short");
    }

    // ── Copilot's unresolved comments ─────────────────────────────────────────

    /// A hit carrying review threads. `threads` are `(author, resolved, outdated)`.
    fn threaded(url: &str, threads: &[(&str, bool, bool)]) -> String {
        let nodes = threads
            .iter()
            .map(|(author, resolved, outdated)| {
                format!(
                    r#"{{"isResolved":{resolved},"isOutdated":{outdated},"comments":{{"nodes":[{{"author":{{"login":"{author}"}}}}]}}}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let bare = verdict_hit(url, &[], &[]);
        format!(r#"{{"reviewThreads":{{"nodes":[{nodes}]}},{}"#, &bare[1..])
    }

    const COPILOT: &str = "copilot-pull-request-reviewer";

    /// Copilot reviews by *commenting* — it never approves and never requests changes — so
    /// `latestOpinionatedReviews` drops it entirely and `still_on_you` has never been able to see it.
    /// An unresolved thread is the only verdict it leaves.
    #[test]
    fn an_unresolved_copilot_thread_is_work() {
        let body = payload(&[threaded("https://github.com/o/r/pull/1", &[(COPILOT, false, false)])]);
        assert_eq!(changes_count(&body), 1);
    }

    #[test]
    fn a_resolved_copilot_thread_is_not_work() {
        let body = payload(&[threaded("https://github.com/o/r/pull/1", &[(COPILOT, true, false)])]);
        assert_eq!(changes_count(&body), 0);
    }

    /// A thread on a line you have since changed. GitHub's own UI hides these, and the code it
    /// pointed at is gone, so the bar does not keep burning for it.
    #[test]
    fn an_outdated_copilot_thread_is_not_work() {
        let body = payload(&[threaded("https://github.com/o/r/pull/1", &[(COPILOT, false, true)])]);
        assert_eq!(changes_count(&body), 0);
    }

    /// Humans get a verdict to express themselves with, and `still_on_you` already reads it. Counting
    /// every open human discussion thread as work would light the bar on ordinary conversation.
    #[test]
    fn an_unresolved_human_thread_is_not_work() {
        let body = payload(&[threaded("https://github.com/o/r/pull/1", &[("alice", false, false)])]);
        assert_eq!(changes_count(&body), 0);
    }

    /// REST spells the bot with a `[bot]` suffix and GraphQL is not documented to agree. Matching on
    /// the stem rather than picking one spelling makes the question moot.
    #[test]
    fn the_bot_login_matches_with_or_without_the_bot_suffix() {
        for spelling in [COPILOT, "copilot-pull-request-reviewer[bot]"] {
            let body = payload(&[threaded("https://github.com/o/r/pull/1", &[(spelling, false, false)])]);
            assert_eq!(changes_count(&body), 1, "{spelling}");
        }
    }

    /// Only the unresolved, current ones are counted, so the page can say how many.
    #[test]
    fn the_entry_carries_how_many_copilot_comments_are_open() {
        let body = payload(&[threaded(
            "https://github.com/o/r/pull/1",
            &[
                (COPILOT, false, false),
                (COPILOT, false, false),
                (COPILOT, true, false),
                (COPILOT, false, true),
                ("alice", false, false),
            ],
        )]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { prs: Some(list), .. } => assert_eq!(list[0].copilot_unresolved, 2),
            other => panic!("expected Fresh with a list, got {:?}", other),
        }
    }

    /// Same call as merge conflicts: work before news, and the two bars stay disjoint.
    #[test]
    fn unresolved_copilot_comments_veto_the_approved_count() {
        let approved_with_nits = format!(
            r#"{{"reviewThreads":{{"nodes":[{{"isResolved":false,"isOutdated":false,"comments":{{"nodes":[{{"author":{{"login":"{COPILOT}"}}}}]}}}}]}},{}"#,
            &verdict_hit("https://github.com/o/r/pull/1", &[("APPROVED", "alice")], &[])[1..]
        );
        match approved_resp(StatusCode::OK, &headers(&[]), &payload(&[approved_with_nits]), 0) {
            PollResult::Fresh { count, .. } => assert_eq!(count, Some(0)),
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// `copilotReviews=off` takes it back out of both rules, and the axes then stop asking for review
    /// threads at all, so the setting is a cost saving as well as a preference.
    #[test]
    fn the_setting_off_leaves_copilot_comments_out_of_both_bars() {
        let body = payload(&[threaded("https://github.com/o/r/pull/1", &[(COPILOT, false, false)])]);
        let off = classify_with(
            StatusCode::OK,
            &headers(&[]),
            &body,
            0,
            &|b| parse_reviewed(b, |pr| work_required(pr, false)),
        );
        match off {
            PollResult::Fresh { count, .. } => assert_eq!(count, Some(0), "not work when off"),
            other => panic!("expected Fresh, got {:?}", other),
        }

        let approved_with_nits = format!(
            r#"{{"reviewThreads":{{"nodes":[{{"isResolved":false,"isOutdated":false,"comments":{{"nodes":[{{"author":{{"login":"{COPILOT}"}}}}]}}}}]}},{}"#,
            &verdict_hit("https://github.com/o/r/pull/2", &[("APPROVED", "alice")], &[])[1..]
        );
        let still_approved = classify_with(
            StatusCode::OK,
            &headers(&[]),
            &payload(&[approved_with_nits]),
            0,
            &|b| parse_reviewed(b, |pr| approved(pr, false)),
        );
        match still_approved {
            PollResult::Fresh { count, .. } => assert_eq!(count, Some(1), "no veto when off"),
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// A hit with no `reviewThreads` at all — the axes that pass `first: 0` — must not read as work.
    #[test]
    fn a_hit_without_review_threads_is_not_work() {
        let body = payload(&[verdict_hit("https://github.com/o/r/pull/1", &[], &[])]);
        assert_eq!(changes_count(&body), 0);
    }

    // ── Activity, for the hoot ledger ─────────────────────────────────────────

    /// A hit with an explicit `updatedAt` and review timestamps.
    fn dated(url: &str, updated: &str, reviews: &[(&str, &str, &str)]) -> String {
        let nodes = reviews
            .iter()
            .map(|(state, login, at)| {
                format!(
                    r#"{{"state":"{state}","author":{{"login":"{login}"}},"submittedAt":"{at}"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"url":"{url}","updatedAt":"{updated}","latestOpinionatedReviews":{{"nodes":[{nodes}]}},"reviewRequests":{{"nodes":[]}}}}"#
        )
    }

    /// Read through the review-requested parser, which keeps every hit — these tests are about what
    /// an entry *carries*, not about which predicate counts it.
    fn only_changes_entry(body: &str) -> PrEntry {
        match search(StatusCode::OK, &headers(&[]), body, 0) {
            PollResult::Fresh { prs: Some(list), .. } => {
                assert_eq!(list.len(), 1);
                list.into_iter().next().unwrap()
            }
            other => panic!("expected Fresh with a list, got {:?}", other),
        }
    }

    /// `activity` is the newest thing that happened, wherever it is recorded.
    ///
    /// GitHub does not document which events bump a pull request's `updatedAt`, and a review object
    /// carries only `submittedAt` with no update timestamp of its own. Taking the max settles it by
    /// construction: if a submitted review does not move `updatedAt`, its own stamp still moves this.
    #[test]
    fn activity_is_the_latest_of_the_pr_and_its_reviews() {
        let review_newer = dated(
            "https://github.com/o/r/pull/1",
            "2026-09-01T00:00:00Z",
            &[("APPROVED", "alice", "2026-09-05T12:00:00Z")],
        );
        assert_eq!(
            only_changes_entry(&payload(&[review_newer])).activity.as_deref(),
            Some("2026-09-05T12:00:00Z")
        );

        let pr_newer = dated(
            "https://github.com/o/r/pull/2",
            "2026-09-09T00:00:00Z",
            &[("APPROVED", "alice", "2026-09-05T12:00:00Z")],
        );
        assert_eq!(
            only_changes_entry(&payload(&[pr_newer])).activity.as_deref(),
            Some("2026-09-09T00:00:00Z")
        );
    }

    /// The newest of *all* the reviews, not the last one listed.
    #[test]
    fn activity_takes_the_newest_review_whatever_the_order() {
        let body = dated(
            "https://github.com/o/r/pull/1",
            "2026-09-01T00:00:00Z",
            &[
                ("APPROVED", "alice", "2026-09-07T00:00:00Z"),
                ("CHANGES_REQUESTED", "bob", "2026-09-03T00:00:00Z"),
            ],
        );
        assert_eq!(
            only_changes_entry(&payload(&[body])).activity.as_deref(),
            Some("2026-09-07T00:00:00Z")
        );
    }

    /// A payload with no timestamps at all leaves `activity` unknown rather than inventing one.
    #[test]
    fn activity_is_none_when_nothing_is_dated() {
        let bare = verdict_hit("https://github.com/o/r/pull/1", &[], &[]);
        assert_eq!(only_changes_entry(&payload(&[bare])).activity, None);
    }

    /// The node id is the ledger's key when GitHub sends one: unlike a URL it survives a repo rename,
    /// which would otherwise read as a brand-new pull request and hoot once for nothing.
    #[test]
    fn the_node_id_is_read_when_present() {
        let body = r#"{"id":"PR_kwDO","url":"https://github.com/o/r/pull/1","latestOpinionatedReviews":{"nodes":[]},"reviewRequests":{"nodes":[]}}"#.to_string();
        let e = only_changes_entry(&payload(&[body]));
        assert_eq!(e.id.as_deref(), Some("PR_kwDO"));
        assert_eq!(e.key(), "PR_kwDO");
    }

    /// And the URL stands in when it is not, so the ledger always has something to key on.
    #[test]
    fn the_url_is_the_key_when_there_is_no_node_id() {
        let e = only_changes_entry(&payload(&[verdict_hit("https://github.com/o/r/pull/1", &[], &[])]));
        assert_eq!(e.key(), "https://github.com/o/r/pull/1");
    }

    // ── Merge conflicts as work ───────────────────────────────────────────────

    /// A hit with an explicit `mergeable`, plus whoever is attached to it.
    ///
    /// `reviews` are `(state, login)` standing verdicts; `pending` are `(typename, name)` review
    /// requests. Both empty means nobody is looking at it.
    fn conflicted(
        url: &str,
        mergeable: &str,
        reviews: &[(&str, &str)],
        pending: &[(&str, &str)],
    ) -> String {
        let bare = verdict_hit(url, reviews, pending);
        format!(r#"{{"mergeable":"{mergeable}",{}"#, &bare[1..])
    }

    fn changes_count(body: &str) -> u32 {
        match changes(StatusCode::OK, &headers(&[]), body, 0) {
            PollResult::Fresh { count: Some(n), .. } => n,
            other => panic!("expected Fresh with a count, got {:?}", other),
        }
    }

    /// A conflict is work you owe someone, exactly as a changes-requested review is. The reviewer is
    /// waiting and cannot act until you rebase, which is the same situation the amber bar exists for.
    #[test]
    fn a_conflicting_pr_someone_is_waiting_on_counts_as_work() {
        let pending = conflicted("https://github.com/o/r/pull/1", "CONFLICTING", &[], &[("User", "alice")]);
        assert_eq!(changes_count(&payload(&[pending])), 1, "a reviewer is waiting on it");

        let reviewed = conflicted("https://github.com/o/r/pull/2", "CONFLICTING", &[("APPROVED", "bob")], &[]);
        assert_eq!(changes_count(&payload(&[reviewed])), 1, "a reviewer has spoken on it");
    }

    /// A conflict on a pull request nobody is attached to blocks nobody. It is yours to deal with when
    /// you get to it, and lighting the tray for it is the cry-wolf this bar has to avoid.
    #[test]
    fn a_conflicting_pr_nobody_is_reviewing_does_not_count() {
        let alone = conflicted("https://github.com/o/r/pull/1", "CONFLICTING", &[], &[]);
        assert_eq!(changes_count(&payload(&[alone])), 0);
    }

    /// The server-side query no longer filters at all, so a clean pull request with reviewers on it is
    /// the ordinary case and must stay dark.
    #[test]
    fn a_mergeable_pr_with_reviewers_is_not_work() {
        let clean = conflicted("https://github.com/o/r/pull/1", "MERGEABLE", &[("APPROVED", "bob")], &[]);
        assert_eq!(changes_count(&payload(&[clean])), 0);
    }

    /// `mergeable` is computed lazily: GitHub answers `UNKNOWN` until something asks, and asking is
    /// what starts the computation, so the *next* poll has the real answer. Counting `UNKNOWN` as a
    /// conflict would light the bar on every freshly pushed pull request and clear a minute later.
    /// Undercounting for one cycle beats a bar that blinks.
    #[test]
    fn an_uncomputed_mergeable_is_not_yet_a_conflict() {
        for absent in [
            conflicted("https://github.com/o/r/pull/1", "UNKNOWN", &[], &[("User", "alice")]),
            // The field missing outright, which is the same absence of evidence.
            verdict_hit("https://github.com/o/r/pull/2", &[], &[("User", "alice")]),
        ] {
            assert_eq!(changes_count(&payload(&[absent])), 0);
        }
    }

    /// Both halves of the bar, and a pull request that is in both is still one pull request.
    #[test]
    fn the_two_halves_are_counted_together_without_double_counting() {
        let body = payload(&[
            conflicted("https://github.com/o/r/pull/1", "CONFLICTING", &[("CHANGES_REQUESTED", "alice")], &[]),
            conflicted("https://github.com/o/r/pull/2", "MERGEABLE", &[("CHANGES_REQUESTED", "bob")], &[]),
            conflicted("https://github.com/o/r/pull/3", "CONFLICTING", &[("APPROVED", "carol")], &[]),
            conflicted("https://github.com/o/r/pull/4", "MERGEABLE", &[("APPROVED", "dave")], &[]),
        ]);
        assert_eq!(changes_count(&body), 3, "only the clean approved one is not work");
    }

    /// The green bar loses a conflicted pull request to the amber one, so the two never light for the
    /// same PR. A conflict is work before it is news.
    #[test]
    fn a_conflict_vetoes_the_approved_count() {
        let body = payload(&[conflicted(
            "https://github.com/o/r/pull/1",
            "CONFLICTING",
            &[("APPROVED", "alice")],
            &[],
        )]);
        match approved_resp(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { count, .. } => assert_eq!(count, Some(0)),
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// And the ordinary approval is untouched by the veto.
    #[test]
    fn a_clean_approval_still_counts() {
        let body = payload(&[conflicted(
            "https://github.com/o/r/pull/1",
            "MERGEABLE",
            &[("APPROVED", "alice")],
            &[],
        )]);
        match approved_resp(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { count, .. } => assert_eq!(count, Some(1)),
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// The conflict reaches the page too, or a pull request counted for a reason the page cannot show
    /// is a number with no explanation behind it.
    #[test]
    fn a_conflict_is_carried_on_the_entry() {
        let body = payload(&[conflicted(
            "https://github.com/o/r/pull/1",
            "CONFLICTING",
            &[("APPROVED", "alice")],
            &[],
        )]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { prs: Some(list), .. } => assert!(list[0].conflicting),
            other => panic!("expected Fresh with a list, got {:?}", other),
        }
    }

    // ── Rich entries ──────────────────────────────────────────────────────────

    /// A hit carrying every display field the page shows, as the real payload sends it.
    fn rich_hit(url: &str, number: u64, title: &str, repo: &str, author: &str, rollup: &str) -> String {
        let fields = [
            format!(r#""url":"{url}""#),
            format!(r#""title":"{title}""#),
            format!(r#""number":{number}"#),
            r#""isDraft":false"#.to_string(),
            r#""updatedAt":"2026-09-15T09:12:33Z""#.to_string(),
            format!(r#""repository":{{"nameWithOwner":"{repo}"}}"#),
            format!(r#""author":{{"login":"{author}"}}"#),
            format!(r#""statusCheckRollup":{rollup}"#),
            r#""latestOpinionatedReviews":{"nodes":[{"state":"APPROVED","author":{"login":"alice"}}]}"#
                .to_string(),
            r#""reviewRequests":{"nodes":[]}"#.to_string(),
        ];
        format!("{{{}}}", fields.join(","))
    }

    /// A hit built from an explicit review list and pending-request list, for the verdict tests.
    fn verdict_hit(url: &str, reviews: &[(&str, &str)], pending: &[(&str, &str)]) -> String {
        let reviews = reviews
            .iter()
            .map(|(state, login)| format!(r#"{{"state":"{state}","author":{{"login":"{login}"}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let requests = pending
            .iter()
            .map(|(kind, name)| {
                let key = if *kind == "Team" { "slug" } else { "login" };
                format!(r#"{{"requestedReviewer":{{"__typename":"{kind}","{key}":"{name}"}}}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"url":"{url}","latestOpinionatedReviews":{{"nodes":[{reviews}]}},"reviewRequests":{{"nodes":[{requests}]}}}}"#
        )
    }

    /// A partial answer: real `data` alongside a per-node `errors` entry, which is exactly what
    /// GitHub sends when one field of one hit is refused.
    fn with_errors(hits: &[String], error: &str) -> String {
        format!(
            r#"{{"data":{{"search":{{"nodes":[{}]}}}},"errors":[{error}]}}"#,
            hits.join(",")
        )
    }

    fn only_entry(body: &str) -> PrEntry {
        match approved_resp(StatusCode::OK, &headers(&[]), body, 0) {
            PollResult::Fresh { prs: Some(list), .. } => {
                assert_eq!(list.len(), 1, "expected exactly one entry");
                list.into_iter().next().unwrap()
            }
            other => panic!("expected Fresh with a list, got {:?}", other),
        }
    }

    /// Every display field survives the trip from payload to `PrEntry`. This is the whole point of
    /// the change: the poll used to read these and throw them away.
    #[test]
    fn a_rich_hit_is_read_in_full() {
        let body = payload(&[rich_hit(
            "https://github.com/o/r/pull/7",
            7,
            "Fix the debounce",
            "o/r",
            "octocat",
            r#"{"state":"SUCCESS"}"#,
        )]);
        let e = only_entry(&body);
        assert_eq!(e.url, "https://github.com/o/r/pull/7");
        assert_eq!(e.title.as_deref(), Some("Fix the debounce"));
        assert_eq!(e.number, Some(7));
        assert_eq!(e.repo.as_deref(), Some("o/r"));
        assert_eq!(e.author.as_deref(), Some("octocat"));
        assert_eq!(e.updated_at.as_deref(), Some("2026-09-15T09:12:33Z"));
        assert!(!e.is_draft);
        assert_eq!(e.checks, CheckRollup::Success);
        assert_eq!(e.verdicts, vec![Verdict { login: "alice".into(), state: ReviewState::Approved }]);
        assert!(e.pending.is_empty());
    }

    /// A repository with no checks configured answers `null`, and that is not a failure. Painting it
    /// red would invent a problem; grey says what is actually known.
    #[test]
    fn a_null_status_rollup_reads_as_unknown_not_failing() {
        let body = payload(&[rich_hit("https://github.com/o/r/pull/1", 1, "t", "o/r", "a", "null")]);
        assert_eq!(only_entry(&body).checks, CheckRollup::Unknown);
    }

    /// Every display field is lenient: a hole degrades the entry, never the count. Only `url` is
    /// load-bearing, and that rule is asserted separately.
    #[test]
    fn a_hit_missing_its_display_fields_still_lists() {
        let body =
            payload(&[verdict_hit("https://github.com/o/r/pull/1", &[("APPROVED", "a")], &[])]);
        let e = only_entry(&body);
        assert_eq!(e.title, None);
        assert_eq!(e.number, None);
        assert_eq!(e.repo, None);
        assert_eq!(e.author, None);
        assert_eq!(e.checks, CheckRollup::Unknown);
        assert!(!e.is_draft, "a missing isDraft is not a draft");
    }

    /// The page names who said what, so "2 reviewers" never has to stand in for "bob objected".
    #[test]
    fn review_verdicts_name_who_said_what() {
        let body = payload(&[verdict_hit(
            "https://github.com/o/r/pull/1",
            &[("APPROVED", "alice"), ("CHANGES_REQUESTED", "bob")],
            &[("User", "carol")],
        )]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { prs: Some(list), .. } => {
                let e = &list[0];
                assert_eq!(
                    e.verdicts,
                    vec![
                        Verdict { login: "alice".into(), state: ReviewState::Approved },
                        Verdict { login: "bob".into(), state: ReviewState::ChangesRequested },
                    ]
                );
                assert_eq!(e.pending, vec![Reviewer::User("carol".into())]);
            }
            other => panic!("expected Fresh with a list, got {:?}", other),
        }
    }

    /// A team has no login, which is why `still_on_you` cannot match it. The page can still name it,
    /// so "waiting on someone" beats a silently empty row.
    #[test]
    fn a_team_reviewer_is_named_by_slug() {
        let body = payload(&[verdict_hit(
            "https://github.com/o/r/pull/1",
            &[("CHANGES_REQUESTED", "bob")],
            &[("Team", "backend")],
        )]);
        match changes(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { prs: Some(list), .. } => {
                assert_eq!(list[0].pending, vec![Reviewer::Team("backend".into())]);
            }
            other => panic!("expected Fresh with a list, got {:?}", other),
        }
    }

    // ── Degradable errors ─────────────────────────────────────────────────────

    /// A token that cannot read the check rollup must cost that one field, not the whole poll.
    ///
    /// Under the old all-errors-are-fatal rule this answer became `Transient` on every cycle, and the
    /// bar froze at its last count forever. That is the failure that killed the merge-ready axis from
    /// 1.7.0 to 1.10.0, and the rollup is display-only now, so there is nothing to protect by failing.
    #[test]
    fn a_forbidden_on_status_check_rollup_degrades_the_field_and_keeps_the_count() {
        let body = with_errors(
            &[rich_hit("https://github.com/o/r/pull/1", 1, "t", "o/r", "a", "null")],
            r#"{"type":"FORBIDDEN","path":["search","nodes",0,"statusCheckRollup"],"message":"Resource not accessible by integration"}"#,
        );
        match approved_resp(StatusCode::OK, &headers(&[]), &body, 0) {
            PollResult::Fresh { count, prs, .. } => {
                assert_eq!(count, Some(1), "the hit still counts");
                assert_eq!(prs.expect("still a confirmed list")[0].checks, CheckRollup::Unknown);
            }
            other => panic!("expected Fresh, got {:?}", other),
        }
    }

    /// A refusal at the page level is the whole answer being denied. There is nothing to salvage, and
    /// reporting a confident zero is the one thing this module refuses to do.
    #[test]
    fn a_forbidden_at_the_search_root_is_still_fatal() {
        let body = r#"{"data":null,"errors":[{"type":"FORBIDDEN","path":["search"],"message":"nope"}]}"#;
        assert!(matches!(
            approved_resp(StatusCode::OK, &headers(&[]), body, 0),
            PollResult::Transient(_)
        ));
    }

    /// An error naming a field nobody allowed to degrade stays fatal, so the tolerance cannot quietly
    /// grow to cover a field the count depends on.
    #[test]
    fn a_forbidden_on_a_field_outside_the_allowlist_is_still_fatal() {
        let body = with_errors(
            &[rich_hit("https://github.com/o/r/pull/1", 1, "t", "o/r", "a", "null")],
            r#"{"type":"FORBIDDEN","path":["search","nodes",0,"latestOpinionatedReviews"],"message":"nope"}"#,
        );
        assert!(matches!(
            approved_resp(StatusCode::OK, &headers(&[]), &body, 0),
            PollResult::Transient(_)
        ));
    }

    /// An error with no path at all cannot be attributed to one hit, so it is the query breaking.
    #[test]
    fn an_error_without_a_path_is_still_fatal() {
        let body = r#"{"data":null,"errors":[{"message":"Parse error on \"foo\""}]}"#;
        assert!(matches!(
            approved_resp(StatusCode::OK, &headers(&[]), body, 0),
            PollResult::Transient(_)
        ));
    }

    // ── The document ──────────────────────────────────────────────────────────

    /// The document has to fetch every field `entry_from` reads, or the page silently renders a row
    /// of unknowns and nothing fails loudly enough to notice.
    #[test]
    fn the_query_document_fetches_every_field_the_entry_reads() {
        for field in [
            "url",
            "title",
            "number",
            "isDraft",
            "updatedAt",
            "nameWithOwner",
            "statusCheckRollup",
            "mergeable",
            "submittedAt",
            "reviewThreads",
            "isResolved",
            "isOutdated",
            "rateLimit",
        ] {
            assert!(PR_REVIEWS_DOCUMENT.contains(field), "document is missing {field}");
        }
    }

    /// Resolving a `PullRequestCommit` needs the App's **Contents: read**, which it deliberately does
    /// not request. That detour left the merge-ready axis dead from 1.7.0 to 1.10.0, failing every poll
    /// with `{"type":"FORBIDDEN","path":["search","nodes",0,"commits","nodes",0]}`.
    /// `PullRequest.statusCheckRollup` is the same head-commit rollup at no permission cost. Never go
    /// back.
    #[test]
    fn the_document_never_walks_through_commits() {
        assert!(
            !PR_REVIEWS_DOCUMENT.contains("commits"),
            "a PullRequestCommit needs Contents: read; ask the PullRequest for its rollup instead"
        );
    }

}
