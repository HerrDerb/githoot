//! GitLab, the second portal: gitlab.com and self-managed, which share one API.
//!
//! `api` is the GraphQL client and this file is the adapter. Sign-in is the shared device flow in
//! `portal::oauth`, pointed at GitLab's endpoints and asking for `read_api`.
//!
//! ## The OAuth application
//!
//! GitLab's device flow needs an OAuth application registered on the instance. For gitlab.com a shared
//! GitHoot application ships in the binary, the way the GitHub App does: its id is public by design
//! and every user still authorizes individually. A self-managed instance cannot use it, so there the
//! id comes from `portal.<name>.clientId` in `config.txt` (or `GITLAB_APP_CLIENT_ID`, for testing),
//! and without one the portal is `Off` with a reason saying what to set, never a sign-in button that
//! fails. Either application must be **non-confidential** with the `read_api` scope: the refresh
//! grant is sent without a secret, see `portal::oauth`.
//!
//! `read_api` is wider than the GitHub App's permissions: it reads everything the user can read,
//! repository contents included. There is no narrower GitLab scope that still lists merge requests.
//!
//! ## What is not verified against a live instance
//!
//! Written from GitLab's documentation and the gitlab.com schema, not from a signed-in session. The
//! document's fields and complexity were checked against gitlab.com on 2026-09-28; the device flow
//! and the secret-less refresh for a non-confidential application were not, since neither can be
//! exercised without a registered application.

pub mod api;

use std::path::PathBuf;

use reqwest::blocking::Client;

use super::oauth::{DeviceFlowConfig, TokenStore};
use super::types::{PollResponse, PollResult};
use super::{
    AuthError, CredentialState, HealthReport, PollOutcome, Portal, PortalId,
    PortalInfo, PortalKind, SignInProgress, StatusPage,
};
use crate::errorln;
use crate::state::PrAxis;

pub const DEFAULT_BASE_URL: &str = "https://gitlab.com";

/// gitlab.com's status page, where a click on the menu entry goes.
const STATUS_PAGE: &str = "https://status.gitlab.com";
/// The same page on status.io's API. The id is in status.gitlab.com's own markup.
pub(super) const STATUSIO_PAGE_ID: &str = "5b36dc6502d06804c08349f7";

/// The shared GitHoot application on gitlab.com: non-confidential, `read_api`, device flow. Public by
/// design, like GitHub's client id; see the module doc comment. gitlab.com only.
pub const DEFAULT_CLIENT_ID: &str = "6a0ec843837912fcaa47d5b4ffc7c5a01c90a321e5845d067875dae16b1ca0fe";

/// Overrides a missing `clientId`, for testing against a freshly registered application.
const CLIENT_ID_ENV: &str = "GITLAB_APP_CLIENT_ID";

/// Read-only access to the API, which is what the merge-request lists need. `read_user` alone would
/// see `currentUser` and none of its merge requests.
const SCOPE: &str = "read_api";

/// Every URL this portal talks to, derived from one base. gitlab.com and self-managed agree on the
/// paths, so unlike GitHub there is no irregular case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoints {
    pub graphql: String,
    pub device_code: String,
    pub token: String,
}

impl Endpoints {
    pub fn for_base(base: &str) -> Endpoints {
        let base = base.trim_end_matches('/');
        Endpoints {
            graphql: format!("{base}/api/graphql"),
            device_code: format!("{base}/oauth/authorize_device"),
            token: format!("{base}/oauth/token"),
        }
    }
}

/// The shipped application, for gitlab.com and nowhere else.
fn shipped_client_id(base: &str) -> Option<&'static str> {
    (base.trim_end_matches('/') == DEFAULT_BASE_URL).then_some(DEFAULT_CLIENT_ID)
}

/// The client id to sign in with: the configured one, else the environment's, else the shipped one,
/// else none. Blank counts as absent at every step.
fn resolve_client_id(configured: Option<&str>, env: Option<String>, shipped: Option<&str>) -> Option<String> {
    let present = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    present(configured.map(str::to_string))
        .or_else(|| present(env))
        .or_else(|| present(shipped.map(str::to_string)))
}

/// GitLab behind the [`Portal`] seam.
pub struct GitLabPortal {
    info: PortalInfo,
    endpoints: Endpoints,
    http: Client,
    /// `None` when neither the config nor the environment names an OAuth application.
    flow: Option<DeviceFlowConfig>,
    /// `None` until a credential is loaded or obtained.
    store: Option<TokenStore>,
    /// The green bar's rules, read on every poll.
    rules: crate::config::RuleSwitches,
}

impl GitLabPortal {
    pub fn new(
        id: PortalId,
        base_url: &str,
        http: Client,
        app_asset_path: PathBuf,
        client_id: Option<&str>,
        rules: crate::config::RuleSwitches,
    ) -> Self {
        let base = base_url.trim_end_matches('/');
        let endpoints = Endpoints::for_base(base);
        let shipped = shipped_client_id(base);
        let flow = resolve_client_id(client_id, std::env::var(CLIENT_ID_ENV).ok(), shipped).map(|client_id| DeviceFlowConfig {
            portal_name: "GitLab".to_string(),
            device_code_url: endpoints.device_code.clone(),
            token_url: endpoints.token.clone(),
            client_id,
            scope: Some(SCOPE.to_string()),
            // Named after the portal, so two GitLab instances keep two credentials.
            token_path: app_asset_path.join(format!("{}_token.txt", id.0)),
        });
        let info = PortalInfo {
            id,
            kind: PortalKind::GitLab,
            display_name: "GitLab".to_string(),
            link_prefix: format!("{base}/"),
            inbox_url: format!("{base}/dashboard/merge_requests"),
            // gitlab.com's health is on status.io, read by `statusio`. A self-managed instance has no
            // public page, and gitlab.com's would say nothing about it: no entry rather than a wrong one.
            status_page: (base == DEFAULT_BASE_URL).then(|| StatusPage {
                url: STATUS_PAGE.to_string(),
                mascot: "Tanuki".to_string(),
            }),
            capabilities: PortalKind::GitLab.capabilities(),
            min_poll_interval: crate::state::MIN_POLL_INTERVAL,
        };
        GitLabPortal { info, endpoints, http, flow, store: None, rules }
    }

    /// What the tooltip says when there is no OAuth application to sign in with. Only a self-managed
    /// instance can get here; gitlab.com has the shipped one.
    fn no_client_id(&self) -> String {
        format!("GitLab off: register an application on your instance and set portal.{}.clientId in config.txt", self.info.id.0)
    }
}

impl Portal for GitLabPortal {
    fn info(&self) -> &PortalInfo {
        &self.info
    }

    fn load_saved_credential(&mut self) -> Result<CredentialState, AuthError> {
        let Some(flow) = &self.flow else { return Ok(CredentialState::Off(self.no_client_id())) };
        match TokenStore::load_saved(flow)? {
            Some(store) => {
                self.store = Some(store);
                Ok(CredentialState::Ready)
            }
            None => Ok(CredentialState::NeedsAuth),
        }
    }

    fn authenticate(&mut self, progress: &dyn SignInProgress) -> Result<CredentialState, AuthError> {
        let Some(flow) = &self.flow else {
            return Err(AuthError::Portal { name: "GitLab".to_string(), detail: self.no_client_id() });
        };
        self.store = Some(TokenStore::authenticate(flow, progress)?);
        Ok(CredentialState::Ready)
    }

    fn sign_out(&mut self) -> Result<(), AuthError> {
        if let Some(flow) = &self.flow {
            super::oauth::forget_saved(&flow.token_path).map_err(|e| AuthError::Storage(e.to_string()))?;
        }
        self.store = None;
        Ok(())
    }

    fn needs_refresh(&self) -> bool {
        self.store.as_ref().is_some_and(TokenStore::needs_refresh)
    }

    fn reauthenticate(&mut self) -> Result<(), AuthError> {
        match self.store.as_mut() {
            Some(store) => store.reauthenticate(),
            None => Err(AuthError::AuthorizationRequired),
        }
    }

    fn poll(&mut self, axes: [bool; 3]) -> PollOutcome {
        let outcome = match self.store.as_ref() {
            Some(store) => api::poll(&self.http, &self.endpoints.graphql, store.token(), axes, self.rules.now()),
            // Asked without a credential: the one variant that says "only a sign-in fixes this".
            None => PollOutcome {
                axes: axes.map(|wanted| {
                    wanted.then_some(PollResponse { result: PollResult::Unauthorized, poll_interval: None })
                }),
            },
        };
        for axis in PrAxis::ALL {
            if let Some(detail) = outcome.axes[axis.index()].as_ref().and_then(|r| r.result.problem()) {
                errorln!("GitLab {axis:?} poll: {detail}");
            }
        }
        outcome
    }

    fn health(&mut self) -> Option<Result<HealthReport, String>> {
        self.info.status_page.as_ref()?;
        Some(super::statusio::check(&self.http, STATUSIO_PAGE_ID))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portal(base: &str, client_id: Option<&str>) -> GitLabPortal {
        GitLabPortal::new(PortalId("gitlab".to_string()), base, Client::new(), std::env::temp_dir(), client_id, crate::config::RuleSwitches::new(&crate::config::Config::from_text("")))
    }

    #[test]
    fn gitlab_com_endpoints() {
        let e = Endpoints::for_base(DEFAULT_BASE_URL);
        assert_eq!(e.graphql, "https://gitlab.com/api/graphql");
        assert_eq!(e.device_code, "https://gitlab.com/oauth/authorize_device");
        assert_eq!(e.token, "https://gitlab.com/oauth/token");
    }

    /// Self-managed is the same paths under its own origin, with or without a trailing slash.
    #[test]
    fn a_self_managed_base_derives_every_endpoint_from_itself() {
        let e = Endpoints::for_base("https://git.example.com/");
        assert_eq!(e.graphql, "https://git.example.com/api/graphql");
        assert_eq!(e.device_code, "https://git.example.com/oauth/authorize_device");
        assert_eq!(e.token, "https://git.example.com/oauth/token");
    }

    #[test]
    fn the_info_describes_gitlab_honestly() {
        let p = portal("https://git.example.com", Some("abc"));
        let info = p.info();
        assert_eq!(info.kind, PortalKind::GitLab);
        assert_eq!(info.display_name, "GitLab");
        assert_eq!(info.link_prefix, "https://git.example.com/", "trailing slash is load-bearing");
        assert_eq!(info.inbox_url, "https://git.example.com/dashboard/merge_requests");
        assert!(info.status_page.is_none());
        assert!(!info.capabilities.team_reviewers, "GitLab has no team reviewers");
        assert!(info.capabilities.conflict_state && info.capabilities.rereview_pending);
        assert_eq!(info.capabilities.bot_reviewer, None);
    }

    /// gitlab.com publishes its health on status.io; a self-managed instance publishes none, and gets
    /// no entry rather than gitlab.com's, which would say nothing about the instance in use.
    #[test]
    fn gitlab_com_has_a_status_page_and_a_self_managed_instance_has_none() {
        let com = portal(DEFAULT_BASE_URL, Some("abc"));
        let page = com.info().status_page.clone().expect("gitlab.com publishes one");
        assert_eq!(page.url, "https://status.gitlab.com");
        assert_eq!(page.mascot, "Tanuki");
        let mut own = portal("https://git.example.com", Some("abc"));
        assert!(own.info().status_page.is_none());
        assert!(own.health().is_none(), "nothing to ask, so nothing is asked");
    }

    #[test]
    fn the_flow_asks_for_read_api_and_keeps_a_credential_per_portal() {
        let dir = std::env::temp_dir();
        let p = GitLabPortal::new(PortalId("work".to_string()), DEFAULT_BASE_URL, Client::new(), dir.clone(), Some(" abc "), crate::config::RuleSwitches::new(&crate::config::Config::from_text("")));
        let flow = p.flow.expect("a configured client id gives a flow");
        assert_eq!(flow.client_id, "abc", "trimmed");
        assert_eq!(flow.scope.as_deref(), Some("read_api"));
        assert_eq!(flow.token_path, dir.join("work_token.txt"));
        assert_eq!(flow.portal_name, "GitLab");
    }

    #[test]
    fn the_configured_client_id_wins_over_the_environment_which_wins_over_the_shipped_one() {
        assert_eq!(resolve_client_id(Some("cfg"), Some("env".into()), Some("ship")).as_deref(), Some("cfg"));
        assert_eq!(resolve_client_id(None, Some("env".into()), Some("ship")).as_deref(), Some("env"));
        assert_eq!(resolve_client_id(None, None, Some("ship")).as_deref(), Some("ship"));
        assert_eq!(resolve_client_id(Some("  "), None, None), None, "blank is not a client id");
        assert_eq!(resolve_client_id(None, None, None), None);
    }

    /// gitlab.com has a shipped application, so Install needs no typing there. A self-managed
    /// instance cannot use it: the application is registered on gitlab.com, not on theirs.
    #[test]
    fn only_gitlab_com_gets_the_shipped_application() {
        assert_eq!(shipped_client_id(DEFAULT_BASE_URL), Some(DEFAULT_CLIENT_ID));
        assert_eq!(shipped_client_id("https://gitlab.com/"), Some(DEFAULT_CLIENT_ID));
        assert_eq!(shipped_client_id("https://git.example.com"), None);
        assert_eq!(DEFAULT_CLIENT_ID.len(), 64, "a gitlab.com application id");
    }

    /// Without an OAuth application there is nothing to sign in with, and a sign-in button that can
    /// only fail would be the wrong thing to show. Off, with what to set.
    #[test]
    fn without_a_client_id_the_portal_is_off_and_says_what_to_set() {
        let mut p = portal(DEFAULT_BASE_URL, None);
        p.flow = None; // whatever the environment says
        match p.load_saved_credential() {
            Ok(CredentialState::Off(reason)) => assert!(reason.contains("portal.gitlab.clientId"), "got {reason}"),
            other => panic!("expected Off, got {other:?}"),
        }
        assert!(matches!(p.authenticate(&crate::portal::fake::Silent), Err(AuthError::Portal { .. })));
    }

    #[test]
    fn polling_without_a_credential_answers_unauthorized_only_where_asked() {
        let mut p = portal(DEFAULT_BASE_URL, Some("abc"));
        assert!(!p.needs_refresh());
        assert!(matches!(p.reauthenticate(), Err(AuthError::AuthorizationRequired)));
        let outcome = p.poll([true, false, true]);
        assert!(matches!(outcome.axes[0], Some(PollResponse { result: PollResult::Unauthorized, .. })));
        assert!(outcome.axes[1].is_none());
        assert!(matches!(outcome.axes[2], Some(PollResponse { result: PollResult::Unauthorized, .. })));
    }

    #[test]
    fn nothing_asked_costs_nothing() {
        let outcome = api::poll(&Client::new(), "https://gitlab.invalid/api/graphql", "t", [false; 3], crate::portal::types::Rules::ALL);
        assert!(outcome.axes.iter().all(Option::is_none));
    }

    #[test]
    fn signing_out_deletes_only_this_portals_credential() {
        let dir = std::env::temp_dir().join(format!("githoot-gitlab-signout-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("gitlab_token.txt"), "access_token=x").unwrap();
        std::fs::write(dir.join("pr_token.txt"), "ghu_x").unwrap();
        let mut p = GitLabPortal::new(PortalId("gitlab".to_string()), DEFAULT_BASE_URL, Client::new(), dir.clone(), Some("abc"), crate::config::RuleSwitches::new(&crate::config::Config::from_text("")));
        p.sign_out().unwrap();
        assert!(!dir.join("gitlab_token.txt").exists());
        assert!(dir.join("pr_token.txt").exists(), "GitHub's credential is not GitLab's to delete");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
