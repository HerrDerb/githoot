//! GitHub, the first portal.
//!
//! `api` is the GraphQL client, `auth` the GitHub App device flow that gives it a token, and this
//! file is the adapter that presents both as one [`Portal`]. The three Search query strings and the
//! rule that judges each axis live here too: they are GitHub's vocabulary, and the scheduler that
//! used to hold them has no business knowing what `review-requested:@me` means.
//!
//! ## Base URL
//!
//! Every endpoint derives from one base, so a GitHub Enterprise Server install is a different base
//! and nothing else. github.com is the one irregular case: its API lives on `api.github.com` while
//! GHES serves it under `{base}/api`. That irregularity is in exactly one place, [`Endpoints::for_base`].
//! Not exposed in `config.txt` yet: a GHES instance also needs its own GitHub App registered on it,
//! and shipping the URL without the client id would be half a feature that fails at sign-in.

pub mod api;
pub mod auth;
pub mod people;

use std::path::PathBuf;

use reqwest::blocking::Client;

use super::types::{PollResponse, PollResult};
use super::{
    AuthError, CredentialState, HealthReport, PollOutcome, Portal, PortalId,
    PortalInfo, PortalKind, SignInProgress, StatusPage,
};
use crate::state::PrAxis;
use crate::{errorln, infoln};

/// Where every install has pointed until now.
pub const DEFAULT_BASE_URL: &str = "https://github.com";

/// The status page for github.com. GHES publishes none this app can read, so a non-default base
/// gets no status entry at all rather than a wrong one.
const STATUS_PAGE: &str = "https://www.githubstatus.com";

/// Search query for pull requests awaiting the user's review.
///
/// Nobody is excluded here. Who is left out is the `ignoreAuthors` list, one `-author:` each, appended
/// by `review_query`; Dependabot and Renovate are on it by default. Never by label: `-label:dependencies`
/// once hid human pull requests carrying that label (a hand-made library bump) that asked you for review.
///
/// `draft:false`, same as `MERGE_QUERY`: a draft is not ready for review by GitHub's own
/// definition, so a request parked on one is work that cannot be acted on yet. It lights the dot
/// the moment the author marks it ready, because leaving draft state is an update to the PR.
///
/// `review-requested:@me`, not `user-review-requested:@me` — a deliberate choice between two
/// documented qualifiers. The wider one also matches PRs where a *team* the user belongs to was
/// asked, which is still work someone expects picked up, and it clears once anyone on the team
/// reviews. The narrower one would count only requests naming the user directly.
///
/// `sort:updated-desc` from the equivalent UI search is deliberately absent: it is a UI-only
/// qualifier the API does not read, and the order hits come back in is the page's business, not the
/// count's.
const REVIEW_QUERY: &str = "is:pr review-requested:@me state:open draft:false archived:false";

/// Search query for the user's own pull requests that might be approved.
///
/// Server-side: yours, open, not a draft. *Approved* is judged client-side by `api::approved`, and
/// there is deliberately no `review:approved` here. That qualifier reads `reviewDecision`, GitHub's
/// verdict on a repository's review *policy*, and a repository that requires no reviews gets no verdict:
/// every one of its pull requests reports `null`, approved or not, and the qualifier matches none of
/// them. The green bar sat dark over six approved PRs before this was noticed. Reading the reviews
/// themselves is what the PR page does, and it is what this axis does now — see `PR_REVIEWS_DOCUMENT`.
///
/// There is deliberately no CI qualifier either. A red check used to make a hit vanish, the bar meaning
/// *approved and mergeable*, and that hid the one thing worth being told, that somebody approved your
/// work. Failed checks now move an approved pull request to the amber bar instead, and checks still
/// running keep it off green until they finish, both judged client-side by `api::approved` and
/// `api::work_required` from each hit's rollup. So
/// `status:success` is not merely unused but unwanted. (It would not have worked anyway: it reads only
/// GitHub's legacy combined commit status, empty for repos whose checks are all check runs.) Also still
/// unchecked, and always was: branch-protection rules needing more than one approval or named reviewers.
const MERGE_QUERY: &str = "is:pr author:@me state:open draft:false archived:false";

/// Search query for the user's own pull requests that might need work from you.
///
/// Server-side: yours, open, not a draft. Nothing else — **`review:changes_requested` is deliberately
/// gone.** The axis counts two things now, a reviewer's standing objection *or* a merge conflict with
/// someone waiting, and `mergeable` is not a search qualifier, so a hit has to be looked at either
/// way. Narrowing to one of the two halves server-side would have hidden the other.
///
/// `draft:false` is new with it. A draft is work you already know is unfinished, so neither half of
/// this bar is news on one; before the conflict half existed the qualifier was absent, and a draft
/// carrying a changes-requested review did count.
///
/// This is character-for-character `MERGE_QUERY`, and kept a constant of its own so either bar can
/// change its question alone. While the two agree, `searches` sends the question once for both bars.
const CHANGES_QUERY: &str = "is:pr author:@me state:open draft:false archived:false";

/// The red bar's query as the rules shape it: requests naming you only (`user-review-requested`) with
/// team requests off, and one `-author:` per ignored login. Drafts, archived repositories and closed
/// pull requests stay out whatever the rules say.
///
/// The logins were checked on their way into the file (`setting::is_login`), so none can carry a
/// space or a qualifier into the search.
fn review_query(rules: super::types::Rules, ignored: &[String]) -> String {
    let mut q = if rules.team_requests {
        REVIEW_QUERY.to_string()
    } else {
        REVIEW_QUERY.replace("review-requested:@me", "user-review-requested:@me")
    };
    for login in ignored {
        q.push_str(" -author:");
        q.push_str(login);
    }
    q
}

/// The search query behind `axis`'s dot. Each bar's own draft rule, switched off, drops `draft:false`
/// from that bar's search alone, so drafts reach its ordinary rules and no other bar's.
fn pr_query(axis: PrAxis, rules: super::types::Rules, ignored: &[String]) -> String {
    let (query, skip_drafts) = match axis {
        PrAxis::ReviewRequested => (review_query(rules, ignored), rules.skip_drafts_red),
        PrAxis::ReadyToMerge => (MERGE_QUERY.to_string(), rules.skip_drafts_green),
        PrAxis::ChangesRequested => (CHANGES_QUERY.to_string(), rules.skip_drafts_amber),
    };
    if skip_drafts { query } else { query.replace(" draft:false", "") }
}

/// One search a poll sends, and the bars whose dots its hits decide.
#[derive(Debug, PartialEq, Eq)]
struct Search {
    query: String,
    axes: Vec<PrAxis>,
}

/// The searches one poll sends for the `axes` that are wanted.
///
/// The green and amber bars both search your own open pull requests, with the same document and the
/// same variables, so while their query strings agree one search feeds both and each bar judges the
/// same hits by its own rule. That request is the most expensive one this app sends (it reads review
/// threads, see `api::REVIEW_THREADS_CAP`), and GitHub's GraphQL budget is per user, shared with every
/// other tool acting for them. The two part only when one bar's draft rule is off and the other's on.
fn searches(axes: [bool; 3], rules: super::types::Rules, ignored: &[String]) -> Vec<Search> {
    let mut out: Vec<Search> = Vec::new();
    for axis in PrAxis::ALL.into_iter().filter(|axis| axes[axis.index()]) {
        let query = pr_query(axis, rules, ignored);
        // Only bars judged from your own pull requests share: the red bar sends other variables
        // (no review threads), so an equal string alone would not make it the same request.
        let shared = out.iter_mut().find(|s| {
            s.query == query && s.axes.iter().all(|a| pr_judge(*a) != PrJudge::EveryHit)
        });
        match shared {
            Some(search) if pr_judge(axis) != PrJudge::EveryHit => search.axes.push(axis),
            _ => out.push(Search { query, axes: vec![axis] }),
        }
    }
    out
}

/// Which rule decides whether a search hit counts for an axis.
///
/// All three axes issue the same GraphQL document against the same endpoint, so what separates
/// them is the query string and this. Split out of `poll_search` because that does I/O and therefore
/// cannot be asserted, while *which rule judges which axis* is exactly the kind of decision that
/// should not be able to change unnoticed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PrJudge {
    /// Every hit counts. `review-requested:@me` is a real server-side filter, so there is nothing for
    /// a client-side rule to narrow; the hits are fetched only so the page can name them.
    EveryHit,
    /// `api::approved` reads each hit's reviews, because Search's `review:` qualifier reads a field
    /// GitHub leaves empty wherever no review policy exists.
    Approved,
    /// `api::still_on_you` reads each hit's pending requests, which Search cannot express at all.
    StillOnYou,
}

/// The rule behind `axis`'s dot.
fn pr_judge(axis: PrAxis) -> PrJudge {
    match axis {
        PrAxis::ReviewRequested => PrJudge::EveryHit,
        PrAxis::ReadyToMerge => PrJudge::Approved,
        PrAxis::ChangesRequested => PrJudge::StillOnYou,
    }
}

/// Sends one search and answers each of its bars, in the order of `search.axes`.
///
/// One endpoint, one document, three rules. The three `api::poll_*` functions stay separate rather
/// than collapsing into one call taking the rule, because each carries the argument for why its rule
/// exists — and on this axis that argument is the asset, not the code. The one pairing is green with
/// amber, which `searches` only forms when their requests are identical.
fn poll_search(
    client: &Client,
    endpoints: &Endpoints,
    token: &str,
    search: &Search,
    rules: super::types::Rules,
) -> Vec<PollResponse> {
    let url = endpoints.graphql.as_str();
    let query = search.query.as_str();
    match search.axes.as_slice() {
        [PrAxis::ReadyToMerge, PrAxis::ChangesRequested] => {
            api::poll_approved_and_changes_requested(client, url, token, query, rules).into()
        }
        axes => axes
            .iter()
            .map(|&axis| match pr_judge(axis) {
                PrJudge::EveryHit => api::poll_review_requested(client, url, token, query),
                PrJudge::Approved => api::poll_approved(client, url, token, query, rules),
                PrJudge::StillOnYou => api::poll_changes_requested(client, url, token, query, rules),
            })
            .collect(),
    }
}

/// Every URL this portal talks to, derived from one base.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoints {
    pub graphql: String,
    pub oauth: auth::OAuthEndpoints,
    /// The App's install page, a browser page rather than an API.
    pub install: String,
    /// GitHub's user search, for the Ignored authors suggestions.
    pub user_search: String,
}

impl Endpoints {
    /// `base` is `https://github.com` or a GHES origin, with no trailing slash.
    pub fn for_base(base: &str) -> Endpoints {
        let base = base.trim_end_matches('/');
        // github.com keeps its API on a separate host; GHES serves it under the same origin.
        let api = if base == DEFAULT_BASE_URL {
            "https://api.github.com".to_string()
        } else {
            format!("{base}/api")
        };
        // The REST root is `/api/v3` on GHES and the bare host on github.com; the installations
        // read is the one REST call this portal makes.
        let rest = if base == DEFAULT_BASE_URL { api.clone() } else { format!("{api}/v3") };
        Endpoints {
            graphql: format!("{api}/graphql"),
            oauth: auth::OAuthEndpoints {
                device_code: format!("{base}/login/device/code"),
                access_token: format!("{base}/login/oauth/access_token"),
                installations: format!("{rest}/user/installations"),
            },
            install: format!("{base}/apps/{}/installations/new", auth::APP_SLUG),
            user_search: format!("{rest}/search/users"),
        }
    }

    /// Shorthand for the tests; production derives from the configured base.
    #[cfg(test)]
    pub fn github_com() -> Endpoints {
        Endpoints::for_base(DEFAULT_BASE_URL)
    }
}

/// Login suggestions for the settings page while `typed` is typed, with the saved credential if it is
/// still valid, else none. github.com only, as is every GitHub portal until GHES ships.
pub fn suggest_people(app_asset_path: &std::path::Path, typed: &str) -> people::People {
    let Some(token) = auth::valid_saved_token(app_asset_path) else {
        return people::People::default();
    };
    let Ok(client) = api::build_client() else {
        return people::People::default();
    };
    let e = Endpoints::for_base(DEFAULT_BASE_URL);
    people::suggest(&client, &e.graphql, &e.user_search, &token, typed)
}

/// The GitHub-specific settings, read from the same `config.txt` keys they always had.
pub struct GitHubOptions {
    /// The green bar's rules, Copilot's comments among them. Read on every poll, because the settings
    /// page changes them live.
    pub rules: crate::config::RuleSwitches,
    /// Which parts of GitHub count as an outage. Empty means the page-wide indicator.
    pub status_components: Vec<String>,
}

/// GitHub behind the [`Portal`] seam.
pub struct GitHubPortal {
    info: PortalInfo,
    endpoints: Endpoints,
    http: Client,
    app_asset_path: PathBuf,
    /// `None` until a credential is loaded or obtained.
    store: Option<auth::PrTokenStore>,
    options: GitHubOptions,
    /// Shows the install prompt. A field so a test can watch it without a desktop.
    offer_install: fn(&str),
}

impl GitHubPortal {
    pub fn new(
        id: PortalId,
        base_url: &str,
        http: Client,
        app_asset_path: PathBuf,
        options: GitHubOptions,
    ) -> Self {
        let base = base_url.trim_end_matches('/');
        let info = PortalInfo {
            id,
            kind: PortalKind::GitHub,
            display_name: "GitHub".to_string(),
            link_prefix: format!("{base}/"),
            inbox_url: format!("{base}/pulls/inbox"),
            status_page: (base == DEFAULT_BASE_URL).then(|| StatusPage {
                url: STATUS_PAGE.to_string(),
                mascot: "Octocat".to_string(),
            }),
            capabilities: PortalKind::GitHub.capabilities(),
            min_poll_interval: crate::state::MIN_POLL_INTERVAL,
        };
        GitHubPortal {
            info,
            endpoints: Endpoints::for_base(base),
            http,
            app_asset_path,
            store: None,
            options,
            offer_install: crate::serve::offer_onboarding,
        }
    }

    /// Authorized, but is the App installed anywhere? Zero installations means Search would see no
    /// repositories at all, and another click cannot fix that, so the exclamation comes down and a
    /// stated reason replaces it. Not being able to *ask* is not the same as zero: start anyway.
    fn adopt(&mut self, store: auth::PrTokenStore) -> CredentialState {
        let count = store.installation_count();
        self.store = Some(store);
        self.judge_installations(count)
    }

    /// What an installation count means, and nothing else: no log line, no page. Zero installations
    /// is off; not being able to *ask* is not the same as zero, so it starts anyway.
    fn verdict(&self, count: Result<u64, AuthError>) -> CredentialState {
        match count {
            Ok(0) => CredentialState::Off(auth::PR_NOT_INSTALLED.to_string()),
            Ok(_) => CredentialState::Ready,
            Err(e) => {
                errorln!("could not confirm GitHub App installations ({e}) — continuing anyway");
                CredentialState::Ready
            }
        }
    }

    /// Asks GitHub again whether the App is installed anywhere, for onboarding while it waits for an
    /// install. Quiet on purpose: it offers no page, because onboarding is the page, and every reload
    /// would otherwise open another tab. `None` with no credential to ask with, or when asking
    /// failed: a failure here changes nothing, rather than starting anyway as `adopt` does.
    fn recheck_install(&mut self) -> Option<CredentialState> {
        let count = self.store.as_ref()?.installation_count().ok()?;
        Some(self.verdict(Ok(count)))
    }

    /// `adopt`'s verdict, split from the request so it can be asserted without a network. Unlike
    /// [`Self::verdict`] it says so in the log and offers the install, once per adoption.
    fn judge_installations(&self, count: Result<u64, AuthError>) -> CredentialState {
        let state = self.verdict(count);
        if let Some(page) = install_page(&state, &self.endpoints) {
            infoln!("{}", auth::PR_NOT_INSTALLED);
            (self.offer_install)(page);
        }
        state
    }
}

/// The page to offer once a credential is adopted, at startup or after a sign-in. Only the
/// not-installed reason offers one: it is the one `Off` a click on GitHub fixes.
fn install_page<'a>(state: &CredentialState, endpoints: &'a Endpoints) -> Option<&'a str> {
    match state {
        CredentialState::Off(reason) if reason == auth::PR_NOT_INSTALLED => Some(&endpoints.install),
        _ => None,
    }
}

impl Portal for GitHubPortal {
    fn info(&self) -> &PortalInfo {
        &self.info
    }

    fn recheck_install(&mut self) -> Option<CredentialState> {
        GitHubPortal::recheck_install(self)
    }

    fn load_saved_credential(&mut self) -> Result<CredentialState, AuthError> {
        match auth::PrTokenStore::load_saved(&self.app_asset_path, &self.endpoints.oauth)? {
            Some(store) => Ok(self.adopt(store)),
            None => Ok(CredentialState::NeedsAuth),
        }
    }

    fn authenticate(&mut self, progress: &dyn SignInProgress) -> Result<CredentialState, AuthError> {
        let store =
            auth::PrTokenStore::authenticate(&self.app_asset_path, &self.endpoints.oauth, progress)?;
        Ok(self.adopt(store))
    }

    fn sign_out(&mut self) -> Result<(), AuthError> {
        auth::forget_saved(&self.app_asset_path).map_err(|e| AuthError::Storage(e.to_string()))?;
        self.store = None;
        Ok(())
    }

    fn needs_refresh(&self) -> bool {
        self.store.as_ref().is_some_and(auth::PrTokenStore::needs_refresh)
    }

    fn reauthenticate(&mut self) -> Result<(), AuthError> {
        match self.store.as_mut() {
            Some(store) => store.reauthenticate(),
            None => Err(AuthError::AuthorizationRequired),
        }
    }

    fn poll(&mut self, axes: [bool; 3]) -> PollOutcome {
        let mut outcome = PollOutcome::default();
        let rules = self.options.rules.now();
        let ignored = self.options.rules.ignored_authors();
        for search in searches(axes, rules, &ignored) {
            let responses = match self.store.as_ref() {
                Some(store) => poll_search(&self.http, &self.endpoints, store.token(), &search, rules),
                // Asked without a credential. Not a transport failure and not a zero: the one
                // variant that says "only a sign-in fixes this".
                None => search
                    .axes
                    .iter()
                    .map(|_| PollResponse { result: PollResult::Unauthorized, poll_interval: None })
                    .collect(),
            };
            for (axis, response) in search.axes.iter().zip(responses) {
                // Only a failed axis speaks up, and it says what actually failed — this is the line
                // that would have shown the merge-ready `statusCheckRollup` FORBIDDEN outright.
                if let Some(detail) = response.result.problem() {
                    errorln!("{axis:?} PR poll: {detail}");
                }
                outcome.axes[axis.index()] = Some(response);
            }
        }
        outcome
    }

    fn health(&mut self) -> Option<Result<HealthReport, String>> {
        self.info.status_page.as_ref()?;
        Some(super::statuspage::check(&self.http, STATUS_PAGE, &self.options.status_components))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::AuthStyle;

    fn portal(base: &str) -> GitHubPortal {
        GitHubPortal::new(
            PortalId("github".to_string()),
            base,
            Client::new(),
            std::env::temp_dir(),
            GitHubOptions {
                rules: crate::config::RuleSwitches::new(&crate::config::Config::from_text("")),
                status_components: Vec::new(),
            },
        )
    }

    /// The review-requested axis keeps every hit: `review-requested:@me` is a real server-side
    /// filter, so there is nothing left for a client-side rule to narrow.
    #[test]
    fn review_requested_counts_every_hit() {
        assert_eq!(pr_judge(PrAxis::ReviewRequested), PrJudge::EveryHit);
    }

    /// If this literal drifts, the count moves with it and the bar starts meaning something else.
    /// No author is named in it: who is left out is the `ignoreAuthors` list's business.
    #[test]
    fn the_review_query_is_unchanged() {
        assert_eq!(REVIEW_QUERY, "is:pr review-requested:@me state:open draft:false archived:false");
    }

    /// A human's pull request labelled `dependencies` (a hand-made library bump) is still a review
    /// someone asked you for. Authors are excluded by name alone, never by label, whatever the rules.
    #[test]
    fn a_dependencies_label_never_hides_a_review_request() {
        use crate::portal::types::Rules;
        for rules in [Rules::ALL, Rules { team_requests: false, ..Rules::ALL }] {
            let query = review_query(rules, &ignored(&["app/dependabot"]));
            assert!(!query.contains("label:"), "{query}");
        }
    }

    fn ignored(logins: &[&str]) -> Vec<String> {
        logins.iter().map(|l| l.to_string()).collect()
    }

    /// Team requests off asks only for requests that name you (`user-review-requested`).
    #[test]
    fn the_review_query_follows_the_red_bar_rules() {
        use crate::portal::types::Rules;
        assert_eq!(review_query(Rules::ALL, &[]), REVIEW_QUERY);
        let direct = review_query(Rules { team_requests: false, ..Rules::ALL }, &[]);
        assert!(direct.contains("user-review-requested:@me") && !direct.contains(" review-requested:@me"), "{direct}");
        assert!(direct.contains("draft:false"), "drafts stay out whatever the rules say");
    }

    /// Every ignored author is one `-author:` qualifier, so GitHub leaves them out of the count, the
    /// list and the hoot alike. The bot switch no longer touches GitHub's search: Dependabot and
    /// Renovate are left out only because the list names them, which it does by default.
    #[test]
    fn every_ignored_author_is_left_out_of_the_red_search_and_nothing_else_is() {
        use crate::portal::types::Rules;
        let list = ignored(&["app/dependabot", "app/renovate", "octo-intern"]);
        let q = review_query(Rules::ALL, &list);
        assert_eq!(q, format!("{REVIEW_QUERY} -author:app/dependabot -author:app/renovate -author:octo-intern"));
        let q = review_query(Rules::ALL, &[]);
        assert!(!q.contains("dependabot") && !q.contains("renovate") && !q.contains("-author:"), "{q}");
        for axis in [PrAxis::ReadyToMerge, PrAxis::ChangesRequested] {
            assert!(!pr_query(axis, Rules::ALL, &list).contains("-author:"), "{axis:?}: only your own pull requests there");
        }
        assert!(pr_query(PrAxis::ReviewRequested, Rules::ALL, &list).ends_with("-author:octo-intern"));
    }

    /// Each bar has its own draft rule, and it shapes only that bar's search: on keeps `draft:false`,
    /// off drops it and lets drafts through to the bar's ordinary rules.
    #[test]
    fn each_bar_leaves_drafts_out_only_while_its_own_draft_rule_is_on() {
        use crate::portal::types::Rules;
        for axis in PrAxis::ALL {
            assert!(pr_query(axis, Rules::ALL, &[]).contains("draft:false"), "{axis:?}");
        }
        let off = [
            (PrAxis::ReviewRequested, Rules { skip_drafts_red: false, ..Rules::ALL }),
            (PrAxis::ReadyToMerge, Rules { skip_drafts_green: false, ..Rules::ALL }),
            (PrAxis::ChangesRequested, Rules { skip_drafts_amber: false, ..Rules::ALL }),
        ];
        for (axis, rules) in off {
            for other in PrAxis::ALL {
                let q = pr_query(other, rules, &[]);
                assert_eq!(q.contains("draft:false"), other != axis, "{axis:?} off, {other:?}: {q}");
                assert!(q.contains("state:open") && q.contains("archived:false"), "the rest stays: {q}");
            }
        }
    }

    fn searched(axes: [bool; 3], rules: crate::portal::types::Rules) -> Vec<Vec<PrAxis>> {
        searches(axes, rules, &[]).into_iter().map(|s| s.axes).collect()
    }

    /// The saving itself: green and amber ask the identical question, so one poll asks it once.
    #[test]
    fn green_and_amber_share_one_search_while_their_queries_agree() {
        use crate::portal::types::Rules;
        assert_eq!(
            searched([true, true, true], Rules::ALL),
            vec![vec![PrAxis::ReviewRequested], vec![PrAxis::ReadyToMerge, PrAxis::ChangesRequested]]
        );
        let both_drafts_in = Rules { skip_drafts_green: false, skip_drafts_amber: false, ..Rules::ALL };
        assert_eq!(
            searched([false, true, true], both_drafts_in),
            vec![vec![PrAxis::ReadyToMerge, PrAxis::ChangesRequested]]
        );
    }

    /// One bar's draft rule off and the other's on: different questions, so two searches.
    #[test]
    fn green_and_amber_search_apart_once_their_draft_rules_differ() {
        use crate::portal::types::Rules;
        let rules = Rules { skip_drafts_green: false, ..Rules::ALL };
        assert_eq!(
            searched([false, true, true], rules),
            vec![vec![PrAxis::ReadyToMerge], vec![PrAxis::ChangesRequested]]
        );
    }

    /// The red bar's query is about other people's pull requests and never joins the others, and a
    /// bar that is not wanted is never searched for.
    #[test]
    fn only_wanted_bars_are_searched() {
        use crate::portal::types::Rules;
        assert_eq!(searched([true, false, false], Rules::ALL), vec![vec![PrAxis::ReviewRequested]]);
        assert_eq!(searched([false, false, true], Rules::ALL), vec![vec![PrAxis::ChangesRequested]]);
        assert!(searched([false, false, false], Rules::ALL).is_empty());
    }

    /// Each search carries the query its bars would have sent on their own.
    #[test]
    fn a_shared_search_sends_the_query_each_bar_would_have_sent() {
        use crate::portal::types::Rules;
        let list = ignored(&["octo-intern"]);
        for search in searches([true, true, true], Rules::ALL, &list) {
            for axis in &search.axes {
                assert_eq!(search.query, pr_query(*axis, Rules::ALL, &list), "{axis:?}");
            }
        }
    }

    /// The suggestions' second group is GitHub's user search, on the REST root beside the installations read.
    #[test]
    fn user_search_sits_on_the_rest_root() {
        assert_eq!(Endpoints::for_base(DEFAULT_BASE_URL).user_search, "https://api.github.com/search/users");
    }

    /// `review:approved` matches nothing in a repository that requires no reviews — GitHub leaves
    /// `reviewDecision` empty there — so the hits are judged by their reviews, and putting the
    /// qualifier back into the query would silently reintroduce the hole in front of the judge.
    #[test]
    fn ready_to_merge_judges_reviews_without_a_review_qualifier() {
        assert_eq!(pr_judge(PrAxis::ReadyToMerge), PrJudge::Approved);
        assert!(!MERGE_QUERY.contains("review:"), "got {MERGE_QUERY}");
    }

    #[test]
    fn changes_requested_still_narrows_its_hits() {
        assert_eq!(pr_judge(PrAxis::ChangesRequested), PrJudge::StillOnYou);
    }

    /// github.com is the irregular one: API on its own host, REST at the root.
    #[test]
    fn github_com_endpoints_are_the_ones_the_app_always_used() {
        let e = Endpoints::github_com();
        assert_eq!(e.graphql, "https://api.github.com/graphql");
        assert_eq!(e.oauth.device_code, "https://github.com/login/device/code");
        assert_eq!(e.oauth.access_token, "https://github.com/login/oauth/access_token");
        assert_eq!(e.oauth.installations, "https://api.github.com/user/installations");
        assert_eq!(e.install, format!("https://github.com/apps/{}/installations/new", auth::APP_SLUG));
    }

    /// A sign-in that ends with the App installed nowhere opens the install page: authorizing alone
    /// shows nothing, and the click on GitHub is the one step this app cannot take for the user.
    /// Any other outcome, or any other reason to be off, opens nothing.
    #[test]
    fn a_sign_in_with_no_installation_opens_the_install_page() {
        let e = Endpoints::github_com();
        let not_installed = CredentialState::Off(auth::PR_NOT_INSTALLED.to_string());
        assert_eq!(install_page(&not_installed, &e), Some(e.install.as_str()));
        assert_eq!(install_page(&CredentialState::Ready, &e), None);
        assert_eq!(install_page(&CredentialState::NeedsAuth, &e), None);
        assert_eq!(install_page(&CredentialState::Off("something else".to_string()), &e), None);
    }

    /// An Enterprise Server serves everything under its own origin, REST under `/api/v3`.
    #[test]
    fn an_enterprise_base_derives_every_endpoint_from_itself() {
        let e = Endpoints::for_base("https://ghe.example.com/");
        assert_eq!(e.graphql, "https://ghe.example.com/api/graphql");
        assert_eq!(e.oauth.device_code, "https://ghe.example.com/login/device/code");
        assert_eq!(e.oauth.access_token, "https://ghe.example.com/login/oauth/access_token");
        assert_eq!(e.oauth.installations, "https://ghe.example.com/api/v3/user/installations");
    }

    #[test]
    fn the_info_describes_github_as_the_page_and_menu_know_it() {
        let p = portal(DEFAULT_BASE_URL);
        let info = p.info();
        assert_eq!(info.display_name, "GitHub");
        assert_eq!(info.link_prefix, "https://github.com/", "trailing slash is load-bearing");
        assert_eq!(info.inbox_url, "https://github.com/pulls/inbox");
        assert_eq!(info.status_page.as_ref().map(|s| s.mascot.as_str()), Some("Octocat"));
        assert_eq!(info.capabilities.auth_style, AuthStyle::DeviceFlow);
        assert_eq!(info.capabilities.bot_reviewer, Some("Copilot"));
        assert!(info.capabilities.conflict_state && info.capabilities.team_reviewers);
    }

    /// GHES publishes no status page this app can read, so it gets none rather than github.com's.
    #[test]
    fn an_enterprise_portal_has_no_status_page_and_its_own_links() {
        let mut p = portal("https://ghe.example.com");
        assert_eq!(p.info().link_prefix, "https://ghe.example.com/");
        assert_eq!(p.info().inbox_url, "https://ghe.example.com/pulls/inbox");
        assert!(p.info().status_page.is_none());
        assert!(p.health().is_none(), "no status page means no check, not a failed one");
    }

    thread_local! {
        static OFFERED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_offer(page: &str) {
        OFFERED.with(|o| o.borrow_mut().push(page.to_string()));
    }

    /// The app shows nothing without an installation, so zero installations offers the install page
    /// every time a credential is adopted: at startup as well as after a sign-in. Some installations,
    /// or a check that could not be made, offer nothing.
    #[test]
    fn an_app_installed_nowhere_is_offered_on_every_adoption() {
        let mut p = portal(DEFAULT_BASE_URL);
        p.offer_install = record_offer;
        assert_eq!(p.judge_installations(Ok(0)), CredentialState::Off(auth::PR_NOT_INSTALLED.to_string()));
        assert_eq!(p.judge_installations(Ok(2)), CredentialState::Ready);
        assert_eq!(p.judge_installations(Err(AuthError::Network("down".to_string()))), CredentialState::Ready);
        OFFERED.with(|o| assert_eq!(*o.borrow(), vec![p.endpoints.install.clone()]));
    }

    /// Onboarding asks again while it waits for an install. That asking must never offer the page
    /// again, or every reload would open another tab.
    #[test]
    fn asking_again_judges_quietly() {
        let mut p = portal(DEFAULT_BASE_URL);
        p.offer_install = record_offer;
        assert_eq!(p.verdict(Ok(0)), CredentialState::Off(auth::PR_NOT_INSTALLED.to_string()));
        assert_eq!(p.verdict(Ok(1)), CredentialState::Ready);
        OFFERED.with(|o| assert!(o.borrow().is_empty(), "offered: {:?}", o.borrow()));
        assert_eq!(p.recheck_install(), None, "no credential, nothing to ask with");
    }

    /// Signing out deletes the file and forgets the store, and asks nothing of the network.
    #[test]
    fn signing_out_deletes_the_saved_token_and_forgets_it() {
        let dir = std::env::temp_dir().join(format!("githoot-signout-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("pr_token.txt"), "ghu_x").unwrap();
        let mut p = GitHubPortal::new(
            PortalId("github".to_string()),
            DEFAULT_BASE_URL,
            Client::new(),
            dir.clone(),
            GitHubOptions { rules: crate::config::RuleSwitches::new(&crate::config::Config::from_text("")), status_components: Vec::new() },
        );
        p.sign_out().unwrap();
        assert!(!dir.join("pr_token.txt").exists(), "the file is gone");
        assert!(!p.needs_refresh(), "and nothing is held in memory");
        p.sign_out().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without a credential there is nothing to ask with, and the honest answer is the one variant
    /// that means "only a sign-in fixes this". No network is touched.
    #[test]
    fn polling_without_a_credential_answers_unauthorized_only_where_asked() {
        let mut p = portal(DEFAULT_BASE_URL);
        assert!(!p.needs_refresh());
        assert!(matches!(p.reauthenticate(), Err(AuthError::AuthorizationRequired)));

        let outcome = p.poll([true, false, true]);
        assert!(matches!(outcome.axes[0], Some(PollResponse { result: PollResult::Unauthorized, .. })));
        assert!(outcome.axes[1].is_none(), "an axis not in play costs nothing and says nothing");
        assert!(matches!(outcome.axes[2], Some(PollResponse { result: PollResult::Unauthorized, .. })));
    }
}
