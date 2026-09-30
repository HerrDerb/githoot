//! GitLab merge-request API client.
//!
//! Same promise as GitHub's client: every outcome GitLab can produce maps to a distinct `PollResult`,
//! so a failure is never rendered as a confident zero.
//!
//! ## One request for every axis
//!
//! GitHub needs one Search per axis. GitLab does not: `currentUser` carries both
//! `reviewRequestedMergeRequests` and `authoredMergeRequests`, so one GraphQL document answers all
//! three axes, and `@include` drops the list no axis in play needs. The two authored axes (approved,
//! needs work) read the same list with different rules. Measured against gitlab.com on 2026-09-28:
//! complexity 157 of the 250 an authenticated request may spend.
//!
//! ## How GitLab's review model differs
//!
//! - **You stay a reviewer after you review.** GitHub drops the review request once you submit;
//!   GitLab keeps you in `reviewers` and moves your `reviewState`. So "waiting on my review" is your
//!   own state being `UNREVIEWED` or `REVIEW_STARTED`, not your mere presence in the list.
//! - **Re-requesting a review resets the reviewer to `UNREVIEWED`** and removes their approval
//!   (`RequestReviewService`). A `REQUESTED_CHANGES` state is therefore still standing by definition,
//!   and "handed back" needs no second list to intersect.
//! - **`UNAPPROVED` is an approval taken back**, set only by `RemoveApprovalService` when a reviewer
//!   withdraws their own. The ball is with the reviewer again, so it counts as owed.
//! - **The approvals are `approvedBy`, not the reviewer state.** A reviewer's `APPROVED` is a column
//!   GitLab updates on the way past; the approval rows can be reset without it moving. So the green
//!   bar reads `approvedBy` only, and a reviewer can approve without ever having been asked.
//! - **No team reviewers.** Every reviewer is a user.

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, ACCEPT, AUTHORIZATION, RETRY_AFTER, USER_AGENT};
use reqwest::StatusCode;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::portal::types::{CheckRollup, PollResponse, PollResult, PrEntry, ReviewState, Reviewer, Rules, Verdict};
use crate::portal::PollOutcome;
use crate::state::PrAxis;

const AGENT: &str = "githoot";

/// How many merge requests each list reads. GitLab's own page-size ceiling, and the same
/// undercount-not-overcount trade GitHub's client makes: past 100 the extras are not seen.
const HITS_CAP: u32 = 100;

/// How many reviewers and approvers each merge request is read with. A merge request judged on a
/// partial list past 20 is unreachable by this app's inbox.
const PEOPLE_CAP: u32 = 20;

/// GitLab's guidance when it limits without saying for how long is to wait; a minute matches
/// GitHub's and the poll floor.
const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);

/// Cap on how much of an error body ends up in a log line or tooltip.
const MAX_DETAIL_CHARS: usize = 200;

/// The one document behind all three axes.
///
/// `username` is read so the review-requested rule can find *you* among each merge request's
/// reviewers. `conflicts` rather than `detailedMergeStatus`: the latter reports only the first
/// blocking check, so a merge request that conflicts *and* awaits approval says `NOT_APPROVED`.
pub const MR_DOCUMENT: &str = "\
query($hits:Int!,$people:Int!,$review:Boolean!,$authored:Boolean!){\
  currentUser{\
    username \
    reviewRequested:reviewRequestedMergeRequests(state:opened,draft:false,first:$hits) @include(if:$review){nodes{...mr}}\
    authored:authoredMergeRequests(state:opened,draft:false,first:$hits) @include(if:$authored){nodes{...mr}}\
  }\
}\
fragment mr on MergeRequest{\
  id iid webUrl title draft updatedAt conflicts sourceBranch \
  project{fullPath}\
  author{username bot}\
  headPipeline{status}\
  reviewers(first:$people){nodes{username mergeRequestInteraction{reviewState}}}\
  approvedBy(first:$people){nodes{username}}\
  labels(first:$people){nodes{title}}\
  diffStatsSummary{additions deletions fileCount}\
}";

/// Asks for the axes marked `true` in one request. No axis in play costs no request at all.
pub fn poll(client: &Client, graphql_url: &str, token: &str, axes: [bool; 3], rules: Rules) -> PollOutcome {
    let mut outcome = PollOutcome::default();
    if !axes.iter().any(|&wanted| wanted) {
        return outcome;
    }
    let review = axes[PrAxis::ReviewRequested.index()];
    let authored = axes[PrAxis::ReadyToMerge.index()] || axes[PrAxis::ChangesRequested.index()];
    let body = serde_json::json!({
        "query": MR_DOCUMENT,
        "variables": { "hits": HITS_CAP, "people": PEOPLE_CAP, "review": review, "authored": authored },
    });
    let request = client
        .post(graphql_url)
        .json(&body)
        .header(ACCEPT, "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(USER_AGENT, AGENT);

    let results = match request.send() {
        Ok(response) => {
            let status = response.status();
            let headers = response.headers().clone();
            let text = response.text().unwrap_or_default();
            classify_with_rules(status, &headers, &text, unix_now(), axes, rules)
        }
        Err(e) => axes.map(|wanted| wanted.then(|| PollResult::Transient(format!("request failed: {e}")))),
    };
    for (slot, result) in outcome.axes.iter_mut().zip(results) {
        *slot = result.map(|result| PollResponse { result, poll_interval: None });
    }
    outcome
}

/// Maps one HTTP response onto a result per asked axis. Pure, with `now` injected, so every branch
/// is testable without a network.
///
/// One response answers every axis, so a failure is every asked axis's failure: the three slots never
/// disagree about whether the request worked, only about what it found.
#[cfg(test)]
fn classify(status: StatusCode, headers: &HeaderMap, body: &str, now: u64, axes: [bool; 3]) -> [Option<PollResult>; 3] {
    classify_with_rules(status, headers, body, now, axes, Rules::ALL)
}

fn classify_with_rules(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
    now: u64,
    axes: [bool; 3],
    rules: Rules,
) -> [Option<PollResult>; 3] {
    let every = |make: &dyn Fn() -> PollResult| axes.map(|wanted| wanted.then(make));

    // Never transient: a rejected token stays rejected until it is replaced.
    if status == StatusCode::UNAUTHORIZED {
        return every(&|| PollResult::Unauthorized);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = rate_limit_wait(headers, now);
        return every(&|| PollResult::RateLimited { retry_after });
    }
    // A 403 from GitLab is a refusal (a token without `read_api`, a blocked account), never a rate
    // limit, which GitLab always answers with 429. Backing off will not help, but reporting nothing
    // pending would be the one unacceptable answer.
    if !status.is_success() {
        let why = describe(status, body);
        return every(&|| PollResult::Transient(why.clone()));
    }

    let lists = match parse(body) {
        Ok(Some(lists)) => lists,
        Ok(None) => return every(&|| PollResult::Unauthorized),
        Err(why) => return every(&|| PollResult::Transient(why.clone())),
    };

    let mut results: [Option<PollResult>; 3] = [None, None, None];
    for axis in PrAxis::ALL {
        if !axes[axis.index()] {
            continue;
        }
        let judged = match axis {
            PrAxis::ReviewRequested => judge(&lists.review_requested, |mr| awaiting_review(mr, &lists.username, rules)),
            PrAxis::ReadyToMerge => judge(&lists.authored, |mr| approved(mr, rules)),
            PrAxis::ChangesRequested => judge(&lists.authored, |mr| work_required(mr, rules)),
        };
        results[axis.index()] = Some(judged);
    }
    results
}

/// How long to wait after a 429: `Retry-After` if GitLab said, else until `RateLimit-Reset`, else a
/// minute. Never shorter than the default, so a stale reset cannot turn into a tight retry loop.
fn rate_limit_wait(headers: &HeaderMap, now: u64) -> Duration {
    if let Some(secs) = header_u64(headers, RETRY_AFTER.as_str()) {
        return Duration::from_secs(secs);
    }
    if let Some(reset) = header_u64(headers, "ratelimit-reset") {
        return Duration::from_secs(reset.saturating_sub(now).max(DEFAULT_RATE_LIMIT_WAIT.as_secs()));
    }
    DEFAULT_RATE_LIMIT_WAIT
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
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

// ─── The payload ──────────────────────────────────────────────────────────────
//
// Lenient at every level, like GitHub's: a hole costs a line of the page, never a number on the icon.
// GitLab answers a field the token may not see with `null` and no error, so an `Option` here is the
// ordinary case, not a defensive one.

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    current_user: Option<CurrentUser>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentUser {
    username: String,
    /// Absent when `@include` dropped it, which reads the same as empty: no axis asked.
    review_requested: Option<Connection<MergeRequest>>,
    authored: Option<Connection<MergeRequest>>,
}

#[derive(Debug, Deserialize)]
struct Connection<T> {
    nodes: Vec<Option<T>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeRequest {
    /// `gid://gitlab/MergeRequest/<n>`, immutable across project moves, which is what the hoot ledger
    /// wants to key on.
    id: Option<String>,
    /// Per-project number, sent as a string.
    iid: Option<String>,
    web_url: Option<String>,
    title: Option<String>,
    #[serde(default)]
    draft: bool,
    updated_at: Option<String>,
    /// Absent is no conflict known, never a conflict.
    #[serde(default)]
    conflicts: bool,
    /// The merge request's branch by name, for saying. A fork's lives on the fork, so an integration
    /// fetches `head_ref` instead.
    source_branch: Option<String>,
    labels: Option<Connection<Label>>,
    diff_stats_summary: Option<DiffStats>,
    project: Option<Project>,
    author: Option<Author>,
    head_pipeline: Option<Pipeline>,
    reviewers: Option<Connection<ReviewerNode>>,
    approved_by: Option<Connection<User>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    full_path: String,
}

#[derive(Debug, Deserialize)]
struct Author {
    username: String,
    #[serde(default)]
    bot: bool,
}

#[derive(Debug, Deserialize)]
struct Pipeline {
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewerNode {
    username: String,
    merge_request_interaction: Option<Interaction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interaction {
    review_state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct User {
    username: String,
}

#[derive(Debug, Deserialize)]
struct Label {
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffStats {
    additions: u64,
    deletions: u64,
    file_count: u64,
}

/// The two lists, flattened, plus who "me" is.
struct Lists {
    username: String,
    review_requested: Vec<MergeRequest>,
    authored: Vec<MergeRequest>,
}

/// `Ok(None)` is GitLab saying it does not know who is asking: `currentUser: null`.
fn parse(body: &str) -> Result<Option<Lists>, String> {
    let response: Response =
        serde_json::from_str(body).map_err(|e| format!("unparseable merge request payload: {e}"))?;
    // GraphQL reports failure at `200 OK`. Errors win even beside usable data: a partial page would
    // undercount, and GitLab reports a field the token cannot see as `null`, not as an error, so an
    // error here is the query itself failing.
    if !response.errors.is_empty() {
        let joined = response.errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
        return Err(format!("GraphQL reported an error: {}", truncate(&joined, MAX_DETAIL_CHARS)));
    }
    let data = response.data.ok_or_else(|| "GraphQL answered with neither data nor errors".to_string())?;
    let Some(user) = data.current_user else { return Ok(None) };
    let flatten = |c: Option<Connection<MergeRequest>>| -> Vec<MergeRequest> {
        c.map(|c| c.nodes.into_iter().flatten().collect()).unwrap_or_default()
    };
    Ok(Some(Lists {
        review_requested: flatten(user.review_requested),
        authored: flatten(user.authored),
        username: user.username,
    }))
}

fn review_state(reviewer: &ReviewerNode) -> Option<&str> {
    reviewer.merge_request_interaction.as_ref()?.review_state.as_deref()
}

fn reviewers(mr: &MergeRequest) -> impl Iterator<Item = &ReviewerNode> {
    mr.reviewers.iter().flat_map(|c| c.nodes.iter().flatten())
}

fn approvers(mr: &MergeRequest) -> impl Iterator<Item = &User> {
    mr.approved_by.iter().flat_map(|c| c.nodes.iter().flatten())
}

/// A reviewer state that means the review is still owed: never started, started, or an approval the
/// reviewer took back without replacing it with a verdict.
fn is_pending(state: Option<&str>) -> bool {
    matches!(state, Some("UNREVIEWED" | "REVIEW_STARTED" | "UNAPPROVED"))
}

/// Whether this merge request is still waiting on *my* review.
///
/// GitLab keeps a reviewer on the list after they review, so the list alone overcounts. My own state
/// decides: `is_pending` is owed; `REVIEWED`, `REQUESTED_CHANGES` and `APPROVED` are done. If I cannot find myself
/// among the reviewers returned (past the cap, or a state GitLab left `null`), the server put it in
/// my review list and that is trusted: a lit bar that should be dark costs a glance, the opposite
/// costs a colleague waiting.
///
/// Bot authors are left out, as GitHub's query leaves out Dependabot and Renovate.
fn awaiting_review(mr: &MergeRequest, me: &str, rules: Rules) -> bool {
    if rules.skip_bots && mr.author.as_ref().is_some_and(|a| a.bot) {
        return false;
    }
    match reviewers(mr).find(|r| r.username == me) {
        Some(mine) => match review_state(mine) {
            None => true,
            state => is_pending(state),
        },
        None => true,
    }
}

fn has_objection(mr: &MergeRequest) -> bool {
    reviewers(mr).any(|r| review_state(r) == Some("REQUESTED_CHANGES"))
}

/// Anybody attached: a reviewer asked, or an approval given.
///
/// Reads the reviewer list as well as the approvals, unlike `approved`: the question here is whether
/// anyone is *waiting*, and a reviewer whose approval was reset is still waiting.
fn has_active_reviewers(mr: &MergeRequest) -> bool {
    reviewers(mr).next().is_some() || approvers(mr).next().is_some()
}

/// Needs work from its author: a standing objection, or a conflict with someone waiting. Same rule as
/// GitHub's, minus the re-request intersection GitLab's reset makes unnecessary.
fn work_required(mr: &MergeRequest, rules: Rules) -> bool {
    has_objection(mr)
        || (rules.conflicts && mr.conflicts && has_active_reviewers(mr))
        || (rules.failed_checks && pipeline_failed(mr) && has_approval(mr))
}

/// The head pipeline definitely failed. Running, cancelled, skipped or no pipeline are not failures.
fn pipeline_failed(mr: &MergeRequest) -> bool {
    mr.head_pipeline.as_ref().is_some_and(|p| pipeline_rollup(&p.status) == CheckRollup::Failure)
}

/// The head pipeline is still running, from created to running. Not good news yet, and not work. A
/// manual job waiting for a press is not counted, or it could hold an approval back forever.
fn pipeline_running(mr: &MergeRequest) -> bool {
    mr.head_pipeline.as_ref().is_some_and(|p| pipeline_rollup(&p.status) == CheckRollup::Pending)
}

/// Somebody approved it and nobody's objection stands: the reviews half of the green bar.
fn has_approval(mr: &MergeRequest) -> bool {
    !has_objection(mr) && approvers(mr).next().is_some()
}

/// Approved by somebody, and nothing standing against it. Anything on the needs-work bar is off this
/// one, so the two never light for the same merge request.
///
/// `approvedBy` only. A reviewer's `APPROVED` state is not read here on purpose: approvals can be
/// reset without that column moving, and a green bar over a merge request nobody currently approves
/// is the false claim this axis must not make.
fn approved(mr: &MergeRequest, rules: Rules) -> bool {
    // A failed pipeline takes it off too: approved with red CI is work, and the amber bar carries it.
    // A pipeline still running holds it back, on neither bar, until it passes.
    if (rules.conflicts && mr.conflicts)
        || (rules.failed_checks && pipeline_failed(mr))
        || (rules.running_checks && pipeline_running(mr))
    {
        return false;
    }
    has_approval(mr)
}

/// Counts the merge requests `keep` accepts and builds their entries; one without a URL poisons the
/// list but not the count, for the reason GitHub's `parse_reviewed` gives.
fn judge(mrs: &[MergeRequest], keep: impl Fn(&MergeRequest) -> bool) -> PollResult {
    let counted: Vec<&MergeRequest> = mrs.iter().filter(|mr| keep(mr)).collect();
    let count = counted.len() as u32;
    let prs: Option<Vec<PrEntry>> = counted.iter().map(|mr| entry_from(mr)).collect();
    PollResult::Fresh { present: count > 0, count: Some(count), prs }
}

/// GitLab's pipeline status in the page's vocabulary.
///
/// Cancelled and skipped read as unknown, which is how GitLab itself paints them: grey, not red. A
/// manual job waiting to be started is `Expected`, the nearest thing to "a result is owed".
fn pipeline_rollup(status: &str) -> CheckRollup {
    match status {
        "SUCCESS" => CheckRollup::Success,
        "FAILED" => CheckRollup::Failure,
        "CREATED" | "WAITING_FOR_RESOURCE" | "PREPARING" | "WAITING_FOR_CALLBACK" | "PENDING" | "RUNNING"
        | "SCHEDULED" => CheckRollup::Pending,
        "MANUAL" => CheckRollup::Expected,
        // CANCELED, CANCELING, SKIPPED, and anything this version has never heard of.
        _ => CheckRollup::Unknown,
    }
}

fn entry_from(mr: &MergeRequest) -> Option<PrEntry> {
    // The verdicts the count is built on, and only those: objections from the reviewer states,
    // approvals from `approvedBy`. A reviewer's stale `APPROVED` state gets no row, for the reason
    // `approved` gives.
    let mut verdicts: Vec<Verdict> = reviewers(mr)
        .filter(|r| review_state(r) == Some("REQUESTED_CHANGES"))
        .map(|r| Verdict { login: r.username.clone(), state: ReviewState::ChangesRequested })
        .collect();
    verdicts.extend(
        approvers(mr).map(|user| Verdict { login: user.username.clone(), state: ReviewState::Approved }),
    );
    let pending = reviewers(mr)
        .filter(|r| is_pending(review_state(r)))
        .map(|r| Reviewer::User(r.username.clone()))
        .collect();

    Some(PrEntry {
        id: mr.id.clone(),
        url: mr.web_url.clone()?,
        title: mr.title.clone(),
        repo: mr.project.as_ref().map(|p| p.full_path.clone()),
        number: mr.iid.as_deref().and_then(|n| n.parse().ok()),
        author: mr.author.as_ref().map(|a| a.username.clone()),
        updated_at: mr.updated_at.clone(),
        is_draft: mr.draft,
        conflicting: mr.conflicts,
        // No automatic reviewer is read yet; see `Capabilities::bot_reviewer` on the portal.
        bot_review: None,
        checks: mr.head_pipeline.as_ref().map_or(CheckRollup::Unknown, |p| pipeline_rollup(&p.status)),
        verdicts,
        pending,
        branch: mr.source_branch.clone(),
        head_ref: mr.iid.as_deref().and_then(|n| n.parse::<u64>().ok()).map(|n| format!("refs/merge-requests/{n}/head")),
        labels: mr.labels.iter().flat_map(|c| c.nodes.iter().flatten()).map(|l| l.title.clone()).collect(),
        changes: mr
            .diff_stats_summary
            .as_ref()
            .map(|d| crate::portal::types::Changes { additions: d.additions, deletions: d.deletions, files: d.file_count }),
    })
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [bool; 3] = [true, true, true];

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
        }
        map
    }

    /// A reviewer as `(username, reviewState)`.
    fn mr(url: &str, reviewers: &[(&str, &str)], approved_by: &[&str], conflicts: bool) -> String {
        let reviewers = reviewers
            .iter()
            .map(|(u, s)| format!(r#"{{"username":"{u}","mergeRequestInteraction":{{"reviewState":"{s}"}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let approvers =
            approved_by.iter().map(|u| format!(r#"{{"username":"{u}"}}"#)).collect::<Vec<_>>().join(",");
        format!(
            r#"{{"webUrl":"{url}","conflicts":{conflicts},"reviewers":{{"nodes":[{reviewers}]}},"approvedBy":{{"nodes":[{approvers}]}}}}"#
        )
    }

    fn payload(review: &[String], authored: &[String]) -> String {
        format!(
            r#"{{"data":{{"currentUser":{{"username":"me","reviewRequested":{{"nodes":[{}]}},"authored":{{"nodes":[{}]}}}}}}}}"#,
            review.join(","),
            authored.join(",")
        )
    }

    fn ok(body: &str) -> [Option<PollResult>; 3] {
        classify(StatusCode::OK, &headers(&[]), body, 0, ALL)
    }

    fn urls(result: &Option<PollResult>) -> Vec<String> {
        match result {
            Some(PollResult::Fresh { prs: Some(prs), count, present }) => {
                assert_eq!(*count, Some(prs.len() as u32));
                assert_eq!(*present, !prs.is_empty());
                prs.iter().map(|e| e.url.clone()).collect()
            }
            other => panic!("expected Fresh with a list, got {other:?}"),
        }
    }

    fn review(body: &str) -> Vec<String> {
        urls(&ok(body)[PrAxis::ReviewRequested.index()])
    }
    fn approved(body: &str) -> Vec<String> {
        urls(&ok(body)[PrAxis::ReadyToMerge.index()])
    }
    fn work(body: &str) -> Vec<String> {
        urls(&ok(body)[PrAxis::ChangesRequested.index()])
    }

    // ── Review requested ─────────────────────────────────────────────────────

    /// The GitLab-specific trap: you stay a reviewer after reviewing, so presence in the list is not
    /// a request still waiting on you. Only your own untouched or in-progress state is.
    #[test]
    fn only_merge_requests_still_awaiting_my_review_count() {
        let body = payload(
            &[
                mr("u/1", &[("me", "UNREVIEWED")], &[], false),
                mr("u/2", &[("me", "REVIEW_STARTED")], &[], false),
                mr("u/3", &[("me", "APPROVED")], &[], false),
                mr("u/4", &[("me", "REQUESTED_CHANGES")], &[], false),
                mr("u/5", &[("me", "REVIEWED")], &[], false),
                mr("u/6", &[("me", "UNAPPROVED")], &[], false),
            ],
            &[],
        );
        assert_eq!(review(&body), ["u/1", "u/2", "u/6"]);
    }

    /// `UNAPPROVED` is set only when a reviewer removes their own approval (GitLab's
    /// `RemoveApprovalService`), so it is a review taken back, not one delivered, and the ball is
    /// with the reviewer again.
    #[test]
    fn a_withdrawn_approval_puts_the_review_back_on_me() {
        assert_eq!(review(&payload(&[mr("u/1", &[("me", "UNAPPROVED")], &[], false)], &[])), ["u/1"]);
    }

    /// GitLab may answer my own state as `null`. The server put the merge request in my review list,
    /// and a hole in the payload is not evidence that I have reviewed it.
    #[test]
    fn my_state_being_null_still_counts() {
        let body = payload(&[r#"{"webUrl":"u/1","reviewers":{"nodes":[{"username":"me","mergeRequestInteraction":null}]}}"#.to_string()], &[]);
        assert_eq!(review(&body), ["u/1"]);
    }

    /// Someone else's state says nothing about whether *you* still owe a review.
    #[test]
    fn another_reviewers_state_is_not_mine() {
        let body = payload(&[mr("u/1", &[("alice", "APPROVED"), ("me", "UNREVIEWED")], &[], false)], &[]);
        assert_eq!(review(&body), ["u/1"]);
    }

    /// GitLab put the merge request in my review list but I cannot see myself in the reviewers it
    /// returned (past the cap, say). The server vouched for it, so it counts: a dark bar over a real
    /// request is the failure this app exists to avoid.
    #[test]
    fn a_request_i_cannot_find_myself_in_trusts_the_server() {
        let body = payload(&[mr("u/1", &[("alice", "APPROVED")], &[], false)], &[]);
        assert_eq!(review(&body), ["u/1"]);
    }

    /// Dependency bots are left out, as GitHub's query leaves out Dependabot and Renovate.
    #[test]
    fn a_merge_request_from_a_bot_is_not_a_review_request() {
        let bot = mr("u/1", &[("me", "UNREVIEWED")], &[], false)
            .replacen('{', r#"{"author":{"username":"renovate-bot","bot":true},"#, 1);
        assert_eq!(review(&payload(&[bot], &[])), Vec::<String>::new());
    }

    // ── Approved ──────────────────────────────────────────────────────────────

    #[test]
    fn an_approval_lights_the_approved_bar() {
        assert_eq!(approved(&payload(&[], &[mr("a/1", &[("alice", "APPROVED")], &["alice"], false)])), ["a/1"]);
    }

    /// A reviewer's `APPROVED` state is a column GitLab updates on the way past; the approvals
    /// themselves are `approvedBy`. Approvals can be reset (new commits, a rule change) without that
    /// state moving, so the state alone is no evidence and the green bar reads the approvals only.
    #[test]
    fn an_approved_state_without_an_approval_behind_it_is_not_approved() {
        assert_eq!(approved(&payload(&[], &[mr("a/1", &[("alice", "APPROVED")], &[], false)])), Vec::<String>::new());
    }

    /// An approval from someone who was never asked still counts; GitLab lets any eligible user approve.
    #[test]
    fn an_approval_from_outside_the_reviewers_counts() {
        assert_eq!(approved(&payload(&[], &[mr("a/1", &[], &["carol"], false)])), ["a/1"]);
    }

    #[test]
    fn a_standing_objection_vetoes_an_approval() {
        let body = payload(&[], &[mr("a/1", &[("alice", "APPROVED"), ("bob", "REQUESTED_CHANGES")], &["alice"], false)]);
        assert_eq!(approved(&body), Vec::<String>::new());
    }

    #[test]
    fn a_conflict_vetoes_an_approval() {
        assert_eq!(approved(&payload(&[], &[mr("a/1", &[("alice", "APPROVED")], &["alice"], true)])), Vec::<String>::new());
    }

    #[test]
    fn nobody_approving_is_not_approved() {
        let body = payload(&[], &[mr("a/1", &[("alice", "UNREVIEWED"), ("bob", "REVIEWED")], &[], false)]);
        assert_eq!(approved(&body), Vec::<String>::new());
    }

    // ── Needs work ────────────────────────────────────────────────────────────

    /// Re-requesting a review resets the reviewer to `UNREVIEWED`, so `REQUESTED_CHANGES` is standing
    /// by definition and handing it back needs no second list.
    #[test]
    fn requested_changes_is_work_and_a_re_request_clears_it() {
        let body = payload(
            &[],
            &[
                mr("w/1", &[("alice", "REQUESTED_CHANGES")], &[], false),
                mr("w/2", &[("alice", "UNREVIEWED")], &[], false),
            ],
        );
        assert_eq!(work(&body), ["w/1"]);
    }

    #[test]
    fn a_conflict_is_work_only_when_someone_is_waiting() {
        let body = payload(
            &[],
            &[
                mr("w/1", &[("alice", "UNREVIEWED")], &[], true),
                mr("w/2", &[], &[], true),
                mr("w/3", &[], &["carol"], true),
            ],
        );
        assert_eq!(work(&body), ["w/1", "w/3"]);
    }

    /// The two authored axes never light for the same merge request.
    #[test]
    fn approved_and_needs_work_are_disjoint() {
        let body = payload(
            &[],
            &[
                mr("x/1", &[("alice", "APPROVED")], &["alice"], true),
                mr("x/2", &[("alice", "APPROVED"), ("bob", "REQUESTED_CHANGES")], &["alice"], false),
                mr("x/3", &[("alice", "APPROVED")], &["alice"], false),
            ],
        );
        assert_eq!(work(&body), ["x/1", "x/2"]);
        assert_eq!(approved(&body), ["x/3"]);
    }

    /// An approved merge request whose pipeline failed is work, not news: amber, not green. Only a
    /// failed pipeline moves it; running, cancelled or none leave the approval where it was.
    #[test]
    fn an_approved_merge_request_with_a_failed_pipeline_is_work_not_news() {
        let with_pipeline = |status: &str| {
            let m = mr("p/1", &[("alice", "APPROVED")], &["alice"], false);
            format!(r#"{{"headPipeline":{{"status":"{status}"}},{}"#, &m[1..])
        };
        let failed = payload(&[], &[with_pipeline("FAILED")]);
        assert_eq!(work(&failed), ["p/1"]);
        assert_eq!(approved(&failed), Vec::<String>::new());
        for fine in ["SUCCESS", "CANCELED", "MANUAL"] {
            let body = payload(&[], &[with_pipeline(fine)]);
            assert_eq!(work(&body), Vec::<String>::new(), "{fine}");
            assert_eq!(approved(&body), ["p/1"], "{fine}");
        }
        // Still running: not green yet, and not work either. A manual job waiting for a press does not
        // count as running, or it could hold the approval back forever.
        for running in ["CREATED", "WAITING_FOR_RESOURCE", "PREPARING", "WAITING_FOR_CALLBACK", "PENDING", "RUNNING", "SCHEDULED"] {
            let body = payload(&[], &[with_pipeline(running)]);
            assert_eq!(approved(&body), Vec::<String>::new(), "{running}: not green while running");
            assert_eq!(work(&body), Vec::<String>::new(), "{running}: not work");
        }
        // Unapproved with a failed pipeline: not this rule's business.
        let m = mr("p/2", &[("alice", "UNREVIEWED")], &[], false);
        let body = payload(&[], &[format!(r#"{{"headPipeline":{{"status":"FAILED"}},{}"#, &m[1..])]);
        assert_eq!(work(&body), Vec::<String>::new());
    }

    /// "Not opened by a bot" switched off lets a bot's merge request onto the red bar.
    #[test]
    fn a_bots_merge_request_counts_when_bots_are_let_in() {
        let bot = mr("u/1", &[("me", "UNREVIEWED")], &[], false).replacen('{', r#"{"author":{"username":"renovate-bot","bot":true},"#, 1);
        let body = payload(&[bot], &[]);
        let red = |rules: Rules| urls(&classify_with_rules(StatusCode::OK, &headers(&[]), &body, 0, ALL, rules)[PrAxis::ReviewRequested.index()]);
        assert_eq!(red(Rules::ALL), Vec::<String>::new());
        assert_eq!(red(Rules { skip_bots: false, ..Rules::ALL }), ["u/1"]);
    }

    /// The green bar's rules switched off are ignored on both bars, as on GitHub.
    #[test]
    fn a_rule_switched_off_is_ignored_on_both_bars() {
        let judge = |body: &str, rules: Rules| -> (Vec<String>, Vec<String>) {
            let results = classify_with_rules(StatusCode::OK, &headers(&[]), body, 0, ALL, rules);
            (urls(&results[PrAxis::ReadyToMerge.index()]), urls(&results[PrAxis::ChangesRequested.index()]))
        };
        let with_pipeline = |status: &str, conflicts: bool| {
            let m = mr("p/1", &[("alice", "APPROVED")], &["alice"], conflicts);
            format!(r#"{{"headPipeline":{{"status":"{status}"}},{}"#, &m[1..])
        };
        let none: Vec<String> = Vec::new();
        let one = vec!["p/1".to_string()];
        let conflict = payload(&[], &[with_pipeline("SUCCESS", true)]);
        assert_eq!(judge(&conflict, Rules::ALL), (none.clone(), one.clone()));
        assert_eq!(judge(&conflict, Rules { conflicts: false, ..Rules::ALL }), (one.clone(), none.clone()));
        let failed = payload(&[], &[with_pipeline("FAILED", false)]);
        assert_eq!(judge(&failed, Rules { failed_checks: false, ..Rules::ALL }), (one.clone(), none.clone()));
        let running = payload(&[], &[with_pipeline("RUNNING", false)]);
        assert_eq!(judge(&running, Rules::ALL), (none.clone(), none.clone()));
        assert_eq!(judge(&running, Rules { running_checks: false, ..Rules::ALL }), (one, none));
    }

    // ── The entry ─────────────────────────────────────────────────────────────

    #[test]
    fn a_full_merge_request_is_read_into_the_entry() {
        let body = r#"{"data":{"currentUser":{"username":"me","reviewRequested":{"nodes":[{
            "id":"gid://gitlab/MergeRequest/42","iid":"7","webUrl":"https://gitlab.com/g/p/-/merge_requests/7",
            "title":"Fix it","draft":false,"updatedAt":"2026-09-28T10:00:00Z","conflicts":true,"sourceBranch":"fix-it",
            "project":{"fullPath":"g/p"},"author":{"username":"alice","bot":false},
            "headPipeline":{"status":"FAILED"},
            "reviewers":{"nodes":[
                {"username":"me","mergeRequestInteraction":{"reviewState":"UNREVIEWED"}},
                {"username":"bob","mergeRequestInteraction":{"reviewState":"REQUESTED_CHANGES"}},
                {"username":"erin","mergeRequestInteraction":{"reviewState":"APPROVED"}},
                {"username":"stale","mergeRequestInteraction":{"reviewState":"APPROVED"}},
                {"username":"dave","mergeRequestInteraction":{"reviewState":"UNAPPROVED"}}]},
            "approvedBy":{"nodes":[{"username":"erin"},{"username":"carol"}]},
            "labels":{"nodes":[{"title":"bug"},{"title":"backend"}]},
            "diffStatsSummary":{"additions":120,"deletions":30,"fileCount":4}}]}}}}"#;
        let results = classify(StatusCode::OK, &headers(&[]), body, 0, [true, false, false]);
        let Some(PollResult::Fresh { prs: Some(prs), .. }) = &results[0] else { panic!("got {results:?}") };
        let e = &prs[0];
        assert_eq!(e.id.as_deref(), Some("gid://gitlab/MergeRequest/42"));
        assert_eq!(e.url, "https://gitlab.com/g/p/-/merge_requests/7");
        assert_eq!(e.title.as_deref(), Some("Fix it"));
        assert_eq!(e.repo.as_deref(), Some("g/p"));
        assert_eq!(e.number, Some(7));
        assert_eq!(e.author.as_deref(), Some("alice"));
        assert_eq!(e.updated_at.as_deref(), Some("2026-09-28T10:00:00Z"));
        assert!(e.conflicting);
        assert_eq!(e.checks, CheckRollup::Failure);
        assert_eq!(e.bot_review, None);
        assert_eq!(
            e.verdicts,
            vec![
                Verdict { login: "bob".into(), state: ReviewState::ChangesRequested },
                Verdict { login: "erin".into(), state: ReviewState::Approved },
                Verdict { login: "carol".into(), state: ReviewState::Approved },
            ],
            "approvals come from approvedBy; a reviewer whose APPROVED state has no approval behind it is not one"
        );
        assert_eq!(e.pending, vec![Reviewer::User("me".into()), Reviewer::User("dave".into())]);
        // GitLab publishes every merge request, forks included, at this ref on the target project.
        assert_eq!(e.branch.as_deref(), Some("fix-it"));
        assert_eq!(e.head_ref.as_deref(), Some("refs/merge-requests/7/head"));
        assert_eq!(e.labels, ["bug", "backend"]);
        assert_eq!(e.changes, Some(crate::portal::types::Changes { additions: 120, deletions: 30, files: 4 }));
    }

    #[test]
    fn pipeline_states_map_onto_the_rollup() {
        for (status, expected) in [
            ("SUCCESS", CheckRollup::Success),
            ("FAILED", CheckRollup::Failure),
            ("RUNNING", CheckRollup::Pending),
            ("PENDING", CheckRollup::Pending),
            ("CREATED", CheckRollup::Pending),
            ("WAITING_FOR_RESOURCE", CheckRollup::Pending),
            ("PREPARING", CheckRollup::Pending),
            ("WAITING_FOR_CALLBACK", CheckRollup::Pending),
            ("SCHEDULED", CheckRollup::Pending),
            ("MANUAL", CheckRollup::Expected),
            ("CANCELED", CheckRollup::Unknown),
            ("CANCELING", CheckRollup::Unknown),
            ("SKIPPED", CheckRollup::Unknown),
            ("SOMETHING_NEW", CheckRollup::Unknown),
        ] {
            assert_eq!(pipeline_rollup(status), expected, "{status}");
        }
    }

    /// A counted merge request without a URL poisons the list, never the count.
    #[test]
    fn a_counted_hit_without_a_url_yields_no_list_but_keeps_the_count() {
        let body = payload(&[r#"{"reviewers":{"nodes":[]}}"#.to_string()], &[]);
        match &ok(&body)[0] {
            Some(PollResult::Fresh { count: Some(1), prs: None, present: true }) => {}
            other => panic!("got {other:?}"),
        }
    }

    // ── What was asked, and what went wrong ──────────────────────────────────

    #[test]
    fn an_axis_not_asked_for_gets_no_answer() {
        let results = classify(StatusCode::OK, &headers(&[]), &payload(&[], &[]), 0, [false, true, false]);
        assert!(results[0].is_none() && results[2].is_none());
        assert!(matches!(results[1], Some(PollResult::Fresh { .. })));
    }

    #[test]
    fn unauthorized_is_unauthorized_on_every_asked_axis() {
        let results = classify(StatusCode::UNAUTHORIZED, &headers(&[]), "", 0, [true, false, true]);
        assert!(matches!(results[0], Some(PollResult::Unauthorized)));
        assert!(results[1].is_none());
        assert!(matches!(results[2], Some(PollResult::Unauthorized)));
    }

    /// GitLab answers an unauthenticated query with `currentUser: null` rather than an error. Reading
    /// that as empty lists would be a confident zero from a request that saw nothing.
    #[test]
    fn no_current_user_means_the_token_was_not_accepted() {
        let results = ok(r#"{"data":{"currentUser":null}}"#);
        assert!(results.iter().all(|r| matches!(r, Some(PollResult::Unauthorized))), "got {results:?}");
    }

    #[test]
    fn a_graphql_error_at_status_200_is_transient_not_zero() {
        let results = ok(r#"{"data":null,"errors":[{"message":"Query has complexity of 300"}]}"#);
        for r in &results {
            match r {
                Some(PollResult::Transient(why)) => assert!(why.contains("complexity")),
                other => panic!("got {other:?}"),
            }
        }
    }

    #[test]
    fn a_malformed_body_is_transient() {
        assert!(ok("not json").iter().all(|r| matches!(r, Some(PollResult::Transient(_)))));
    }

    #[test]
    fn too_many_requests_waits_as_retry_after_says() {
        let results = classify(StatusCode::TOO_MANY_REQUESTS, &headers(&[("retry-after", "90")]), "", 0, ALL);
        assert!(results.iter().all(|r| matches!(r, Some(PollResult::RateLimited { retry_after }) if retry_after.as_secs() == 90)));
    }

    #[test]
    fn too_many_requests_without_retry_after_waits_for_the_reset() {
        let h = headers(&[("ratelimit-reset", "1300")]);
        let results = classify(StatusCode::TOO_MANY_REQUESTS, &h, "", 1000, ALL);
        assert!(matches!(results[0], Some(PollResult::RateLimited { retry_after }) if retry_after.as_secs() == 300));
        let bare = classify(StatusCode::TOO_MANY_REQUESTS, &headers(&[]), "", 1000, ALL);
        assert!(matches!(bare[0], Some(PollResult::RateLimited { retry_after }) if retry_after == DEFAULT_RATE_LIMIT_WAIT));
    }

    /// GitLab's 403 is a permission refusal (a token without `read_api`, say), not a rate limit.
    #[test]
    fn forbidden_is_transient() {
        let results = classify(StatusCode::FORBIDDEN, &headers(&[]), "insufficient_scope", 0, ALL);
        assert!(matches!(&results[0], Some(PollResult::Transient(why)) if why.contains("403")));
    }

    #[test]
    fn the_document_declares_the_variables_it_uses() {
        assert!(MR_DOCUMENT.contains("sourceBranch"), "the branch an integration checks out");
        assert!(MR_DOCUMENT.contains("labels(first:$people){nodes{title}}"), "the labels");
        assert!(MR_DOCUMENT.contains("diffStatsSummary{additions deletions fileCount}"), "the size");
        for var in ["$hits", "$people", "$review", "$authored"] {
            assert!(MR_DOCUMENT.matches(var).count() >= 2, "{var} should be both declared and used");
        }
    }
}
