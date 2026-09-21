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

use std::path::PathBuf;

use reqwest::blocking::Client;

use super::types::{PollResponse, PollResult};
use super::{
    AuthError, AuthStyle, Capabilities, CredentialState, HealthReport, PollOutcome, Portal, PortalId,
    PortalInfo, PortalKind, StatusPage,
};
use crate::state::PrAxis;
use crate::{errorln, infoln};

/// Where every install has pointed until now.
pub const DEFAULT_BASE_URL: &str = "https://github.com";

/// The status page for github.com. GHES publishes none this app can read, so a non-default base
/// gets no status entry at all rather than a wrong one.
const STATUS_PAGE: &str = "https://www.githubstatus.com";

/// Wording of the menu entry shown while github.com reports an incident.
pub const STATUS_MENU_LABEL: &str = "GitHub is githubing again, check status";

/// Search query for pull requests awaiting the user's review.
///
/// `-label:dependencies` is the conventional Dependabot marker, but it is applied by convention
/// rather than guaranteed — a repo with custom Dependabot config, or Renovate instead, would slip
/// through — so the bot authors are excluded by name as well.
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
const REVIEW_QUERY: &str = "is:pr review-requested:@me state:open draft:false archived:false \
                            -label:dependencies -author:app/dependabot -author:app/renovate";

/// Search query for the user's own pull requests that might be approved.
///
/// Server-side: yours, open, not a draft. *Approved* is judged client-side by `api::approved`, and
/// there is deliberately no `review:approved` here. That qualifier reads `reviewDecision`, GitHub's
/// verdict on a repository's review *policy*, and a repository that requires no reviews gets no verdict:
/// every one of its pull requests reports `null`, approved or not, and the qualifier matches none of
/// them. The green bar sat dark over six approved PRs before this was noticed. Reading the reviews
/// themselves is what the PR page does, and it is what this axis does now — see `PR_REVIEWS_DOCUMENT`.
///
/// There is deliberately no CI qualifier either. A red check used to disqualify a hit — the bar meant
/// *approved and mergeable* — and that hid the one thing worth being told, that somebody approved your
/// work. Whether CI is green is a question you go and answer on the page the entry opens. So
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
/// This is character-for-character `MERGE_QUERY`. Two identical searches per cycle is real waste,
/// and collapsing them into one poll feeding two rules is worth doing — it is left alone here only so
/// a semantic change and a poll-loop refactor do not land in the same commit.
const CHANGES_QUERY: &str = "is:pr author:@me state:open draft:false archived:false";

/// The search query behind `axis`'s dot.
fn pr_query(axis: PrAxis) -> &'static str {
    match axis {
        PrAxis::ReviewRequested => REVIEW_QUERY,
        PrAxis::ReadyToMerge => MERGE_QUERY,
        PrAxis::ChangesRequested => CHANGES_QUERY,
    }
}

/// Which rule decides whether a search hit counts for an axis.
///
/// All three axes issue the same GraphQL document against the same endpoint, so what separates
/// them is the query string and this. Split out of `poll_axis` because that does I/O and therefore
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

/// Issues one axis's poll.
///
/// One endpoint, one document, three query strings and three rules. The three `api::poll_*`
/// functions stay separate rather than collapsing into one call taking the rule, because each carries
/// the argument for why its rule exists — and on this axis that argument is the asset, not the code.
pub fn poll_axis(
    client: &Client,
    endpoints: &Endpoints,
    token: &str,
    axis: PrAxis,
    copilot: bool,
) -> PollResponse {
    let query = pr_query(axis);
    match pr_judge(axis) {
        PrJudge::EveryHit => api::poll_review_requested(client, &endpoints.graphql, token, query),
        PrJudge::Approved => api::poll_approved(client, &endpoints.graphql, token, query, copilot),
        PrJudge::StillOnYou => {
            api::poll_changes_requested(client, &endpoints.graphql, token, query, copilot)
        }
    }
}

/// Every URL this portal talks to, derived from one base.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoints {
    pub graphql: String,
    pub oauth: auth::OAuthEndpoints,
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
        }
    }

    /// Shorthand for the tests; production derives from the configured base.
    #[cfg(test)]
    pub fn github_com() -> Endpoints {
        Endpoints::for_base(DEFAULT_BASE_URL)
    }
}

/// The GitHub-specific settings, read from the same `config.txt` keys they always had.
pub struct GitHubOptions {
    /// Whether Copilot's unresolved comments count as work. Read on every poll, because the menu
    /// checkbox flips it live.
    pub copilot_reviews: crate::config::Switch,
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
                menu_label: STATUS_MENU_LABEL.to_string(),
            }),
            capabilities: Capabilities {
                auth_style: AuthStyle::DeviceFlow,
                conflict_state: true,
                rereview_pending: true,
                team_reviewers: true,
                bot_reviewer: Some("Copilot"),
            },
            min_poll_interval: crate::state::MIN_POLL_INTERVAL,
        };
        GitHubPortal {
            info,
            endpoints: Endpoints::for_base(base),
            http,
            app_asset_path,
            store: None,
            options,
        }
    }

    /// Authorized, but is the App installed anywhere? Zero installations means Search would see no
    /// repositories at all, and another click cannot fix that, so the exclamation comes down and a
    /// stated reason replaces it. Not being able to *ask* is not the same as zero: start anyway.
    fn adopt(&mut self, store: auth::PrTokenStore) -> CredentialState {
        let state = match store.installation_count() {
            Ok(0) => {
                infoln!("{}", auth::PR_NOT_INSTALLED);
                CredentialState::Off(auth::PR_NOT_INSTALLED.to_string())
            }
            Ok(_) => CredentialState::Ready,
            Err(e) => {
                errorln!("could not confirm GitHub App installations ({e}) — continuing anyway");
                CredentialState::Ready
            }
        };
        self.store = Some(store);
        state
    }
}

impl Portal for GitHubPortal {
    fn info(&self) -> &PortalInfo {
        &self.info
    }

    fn load_saved_credential(&mut self) -> Result<CredentialState, AuthError> {
        match auth::PrTokenStore::load_saved(&self.app_asset_path, &self.endpoints.oauth)? {
            Some(store) => Ok(self.adopt(store)),
            None => Ok(CredentialState::NeedsAuth),
        }
    }

    fn authenticate(&mut self) -> Result<CredentialState, AuthError> {
        let store = auth::PrTokenStore::authenticate(&self.app_asset_path, &self.endpoints.oauth)?;
        Ok(self.adopt(store))
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
        let copilot = self.options.copilot_reviews.is_on();
        for axis in PrAxis::ALL {
            if !axes[axis.index()] {
                continue;
            }
            let response = match self.store.as_ref() {
                Some(store) => {
                    poll_axis(&self.http, &self.endpoints, store.token(), axis, copilot)
                }
                // Asked without a credential. Not a transport failure and not a zero: the one
                // variant that says "only a sign-in fixes this".
                None => PollResponse { result: PollResult::Unauthorized, poll_interval: None },
            };
            // Only a failed axis speaks up, and it says what actually failed — this is the line
            // that would have shown the merge-ready `statusCheckRollup` FORBIDDEN outright.
            if let Some(detail) = response.result.problem() {
                errorln!("{axis:?} PR poll: {detail}");
            }
            outcome.axes[axis.index()] = Some(response);
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

    fn portal(base: &str) -> GitHubPortal {
        GitHubPortal::new(
            PortalId("github".to_string()),
            base,
            Client::new(),
            std::env::temp_dir(),
            GitHubOptions {
                copilot_reviews: crate::config::Switch::new(true),
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
    #[test]
    fn the_review_query_is_unchanged() {
        assert_eq!(
            REVIEW_QUERY,
            "is:pr review-requested:@me state:open draft:false archived:false \
-label:dependencies -author:app/dependabot -author:app/renovate"
        );
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
        assert_eq!(
            info.status_page.as_ref().map(|s| s.menu_label.as_str()),
            Some(STATUS_MENU_LABEL)
        );
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
