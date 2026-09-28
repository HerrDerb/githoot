//! PR-status credential: a shared GitHub App's Device Flow.
//!
//! All three PR-search axes (`state::PrAxis`) are driven by one credential from one shared,
//! fine-grained GitHub App — not a personally-registered OAuth App like the one the removed
//! notifications credential used, and not `gh`'s token. The App's Client ID is public by design
//! (Device Flow needs no client secret to authorize), so it is safe to hardcode and ship in the
//! binary: every user still authorizes individually through their own browser and gets their own
//! token, exactly as they would with a personal OAuth App, just without having to register one.
//!
//! Read access to PRs in an organization's private repositories is granted by *installing* the
//! App on that org (GitHub's own org-approval flow for third-party access, which every classic
//! OAuth App already goes through) — not by requesting a broader scope. That is the whole reason
//! this is a GitHub App and not an OAuth App: fine-grained, installable, no scope wide enough to
//! also grant write access the way classic `repo` does.
//!
//! ## What needs verifying against a real, registered App before this is trusted
//!
//! This module was written from GitHub's public Device Flow documentation, not from a live test
//! against a real App — that needs a registered Client ID neither this codebase nor this session
//! has. Two things specifically are best-effort, not confirmed:
//!
//!   1. Whether the refresh-token grant (`grant_type=refresh_token`) succeeds *without* a client
//!      secret for this App. It is sent secret-less on purpose — a secret baked into a distributed
//!      binary is not actually secret — but if GitHub requires one anyway, `refresh` simply fails
//!      and `reauthenticate` falls back to a fresh Device Flow. So the worst case of being wrong
//!      about this is an extra browser prompt roughly once per access-token lifetime (a few hours,
//!      if the App has token expiry turned on at all — see below), not a broken credential.
//!   2. The exact response shape of `GET /user/installations`. It is assumed to match the
//!      documented "List app installations accessible to the user access token" shape
//!      (`{"total_count": N, "installations": [...]}`), read the same way `github.rs` already
//!      reads `total_count` from search responses.
//!
//! Registering the App with "Expire user authorization tokens" turned **off** sidesteps both
//! questions entirely — the token poll response is then the same simple long-lived shape the
//! notifications credential already uses, and `refresh`/`needs_refresh` never come into play.

use crate::portal::oauth::{DeviceFlowConfig, TokenStore};
use crate::portal::{AuthError, SignInProgress};
use serde::Deserialize;
use std::path::Path;

/// Overridable for local testing against a freshly registered App without rebuilding — the
/// shipped binary falls back to `CLIENT_ID`. Not documented for end users: this App is meant to
/// be shared, not personally configured.
const CLIENT_ID_ENV: &str = "GITHUB_APP_CLIENT_ID";

/// The shared GitHub App's Client ID. Public by design; safe to hardcode — see this module's doc
/// comment for why. The Device Flow / refresh-token behavior behind it is still unverified
/// against the live API (also see the doc comment) even though the ID itself is real now.
const CLIENT_ID: &str = "Iv23lipB1miHw6m9SG6n";

const PR_TOKEN_FILE: &str = "pr_token.txt";
const AGENT: &str = "githoot";

/// The three URLs the device flow and the installations read talk to, derived from the portal's
/// base by `super::Endpoints::for_base` so a GitHub Enterprise Server is a different base and
/// nothing else in here changes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OAuthEndpoints {
    pub device_code: String,
    pub access_token: String,
    pub installations: String,
}

fn client_id() -> String {
    std::env::var(CLIENT_ID_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| CLIENT_ID.to_string())
}

/// The shared device flow, pointed at GitHub. No `scope`: a GitHub App's permissions are fixed at
/// registration, not requested per authorization.
fn flow_config(app_asset_path: &Path, oauth: &OAuthEndpoints) -> DeviceFlowConfig {
    DeviceFlowConfig {
        portal_name: "GitHub".to_string(),
        device_code_url: oauth.device_code.clone(),
        token_url: oauth.access_token.clone(),
        client_id: client_id(),
        scope: None,
        token_path: app_asset_path.join(PR_TOKEN_FILE),
    }
}

/// Deletes the saved credential. Nothing there is not an error: the outcome is the same.
pub fn forget_saved(app_asset_path: &Path) -> std::io::Result<()> {
    crate::portal::oauth::forget_saved(&app_asset_path.join(PR_TOKEN_FILE))
}

// ── GitHub App installations ────────────────────────────────────────────────────

/// `GET /user/installations`'s `total_count`. Everything else in the response is unused: the only
/// question asked here is whether the App is installed anywhere at all.
#[derive(Deserialize)]
struct InstallationsResponse {
    total_count: u64,
}

/// Number of accounts/organizations that have installed the App for this user.
///
/// A user who has never installed the App anywhere still gets a token that authenticates fine,
/// but PR search would then silently see zero repositories — a confident, wrong "nothing to
/// report" indistinguishable from genuinely having nothing to report. Called once at startup, not
/// every poll — installations do not change fast enough to justify the extra request on the hot
/// path.
fn installation_count(store: &TokenStore, oauth: &OAuthEndpoints) -> Result<u64, AuthError> {
    let response = store
        .http()
        .get(&oauth.installations)
        .header("Accept", "application/vnd.github+json")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", store.token()))
        .header("User-Agent", AGENT)
        .send()
        .map_err(|e| AuthError::Network(e.to_string()))?;
    let status = response.status();
    let body = response.text().map_err(|e| AuthError::Network(e.to_string()))?;

    let resp: InstallationsResponse = serde_json::from_str(&body).map_err(|_| AuthError::Portal {
        name: "GitHub".to_string(),
        detail: format!("GitHub answered {status} with an unexpected installations response: {body}"),
    })?;
    Ok(resp.total_count)
}

// ── Token storage ─────────────────────────────────────────────────────────────

/// Tooltip reason when the user is authorized but the App is installed on no account.
///
/// Shared by the two places that can discover it — `main.rs` at startup and the poll loop right
/// after a menu-driven sign-in — so the same condition cannot be worded two ways.
pub const PR_NOT_INSTALLED: &str = "PR status off: install the GitHub App to see your PRs";

/// Owns the PR-status credential and knows how to renew it: the shared [`TokenStore`], plus the
/// installations check only a GitHub App has.
pub struct PrTokenStore {
    oauth: OAuthEndpoints,
    store: TokenStore,
}

impl PrTokenStore {
    /// Startup path, non-interactive. See [`TokenStore::load_saved`].
    pub fn load_saved(app_asset_path: &Path, oauth: &OAuthEndpoints) -> Result<Option<Self>, AuthError> {
        Ok(TokenStore::load_saved(&flow_config(app_asset_path, oauth))?
            .map(|store| Self { oauth: oauth.clone(), store }))
    }

    /// The interactive path. See [`TokenStore::authenticate`].
    pub fn authenticate(
        app_asset_path: &Path,
        oauth: &OAuthEndpoints,
        progress: &dyn SignInProgress,
    ) -> Result<Self, AuthError> {
        let store = TokenStore::authenticate(&flow_config(app_asset_path, oauth), progress)?;
        Ok(Self { oauth: oauth.clone(), store })
    }

    pub fn token(&self) -> &str {
        self.store.token()
    }

    /// Number of accounts/organizations that have installed the App, so the caller can tell a
    /// genuinely empty PR search apart from one that can't see any repositories at all yet.
    pub fn installation_count(&self) -> Result<u64, AuthError> {
        installation_count(&self.store, &self.oauth)
    }

    pub fn needs_refresh(&self) -> bool {
        self.store.needs_refresh()
    }

    pub fn reauthenticate(&mut self) -> Result<(), AuthError> {
        self.store.reauthenticate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth() -> OAuthEndpoints {
        crate::portal::github::Endpoints::github_com().oauth
    }

    /// GitHub's credential stays where every install has it, so an upgrade keeps its sign-in.
    #[test]
    fn the_github_credential_file_has_not_moved() {
        let dir = std::env::temp_dir();
        let config = flow_config(&dir, &oauth());
        assert_eq!(config.token_path, dir.join("pr_token.txt"));
        assert_eq!(config.scope, None, "a GitHub App asks for no scope");
        assert_eq!(config.device_code_url, "https://github.com/login/device/code");
        assert_eq!(config.token_url, "https://github.com/login/oauth/access_token");
    }

    /// Not setting the env var here: mutating global process environment from a test risks
    /// flaking against whatever else the run does in parallel, and Rust 2024 makes that mutation
    /// `unsafe` for exactly this reason. This just checks the two are consistent with whatever
    /// the process actually inherited.
    #[test]
    fn client_id_defaults_to_the_constant_but_respects_the_env_var() {
        let resolved = client_id();
        assert!(!resolved.is_empty());
        match std::env::var(CLIENT_ID_ENV) {
            Ok(set) if !set.trim().is_empty() => assert_eq!(resolved, set.trim()),
            _ => assert_eq!(resolved, CLIENT_ID),
        }
    }
}
