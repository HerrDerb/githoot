//! The OAuth 2.0 device authorization grant (RFC 8628), shared by every portal that signs in with it.
//!
//! GitHub and GitLab both hand out tokens this way, and the two differ in three places only: where
//! the endpoints are, which client id asks, and whether a `scope` is sent (a GitHub App's permissions
//! are fixed at registration; a GitLab application asks for `read_api` per authorization). Those three
//! are a [`DeviceFlowConfig`]; everything else here is the RFC, the credential file and the refresh
//! grant, and none of it names a forge.
//!
//! What stays in each portal is what is genuinely its own: GitHub's installations check, the client
//! id it ships, and how its endpoints derive from a base URL.
//!
//! ## Refresh without a secret
//!
//! The refresh grant is sent with a client id and no client secret. A secret baked into a distributed
//! binary is not secret, so both portals are registered as public clients. If a portal requires one
//! anyway, `refresh` fails, `reauthenticate` reports `AuthorizationRequired`, and the user signs in
//! again from the menu: the cost of being wrong is one click per token lifetime, not a broken tray.

use crate::portal::{AuthError, SignInPrompt, SignInProgress};
use crate::{errorln, infoln};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const AGENT: &str = "githoot";

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The portal's device-flow `interval` is respected, but never dips below this.
const MIN_DEVICE_POLL_INTERVAL: u64 = 5;
/// Backoff added when the portal answers `slow_down`, per RFC 8628 §3.5.
const SLOW_DOWN_PENALTY: Duration = Duration::from_secs(5);

/// Refresh this long before actual expiry, so a slow request or minor clock skew can never let
/// the token expire mid-flight.
const EXPIRY_SAFETY_MARGIN: Duration = Duration::from_secs(5 * 60);

/// Everything that makes one portal's device flow different from another's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceFlowConfig {
    /// How error messages name who answered: "GitHub reported: …".
    pub portal_name: String,
    /// Where step 1 asks for a device code.
    pub device_code_url: String,
    /// Where the token is polled for, and where a refresh token is traded in.
    pub token_url: String,
    pub client_id: String,
    /// Sent with the device-code request when `Some`. GitHub Apps take none.
    pub scope: Option<String>,
    /// The saved credential. One file per portal, so signing out of one leaves the other alone.
    pub token_path: PathBuf,
}

impl DeviceFlowConfig {
    /// The portal answered, and the answer was a refusal or nonsense.
    fn portal_error(&self, detail: String) -> AuthError {
        AuthError::Portal { name: self.portal_name.clone(), detail }
    }
}

// ── Device code / token responses ──────────────────────────────────────────────

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    /// The same address with the code already in it (RFC 8628 §3.3.1). GitLab sends it; GitHub does
    /// not. Optional in the RFC, so optional here.
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

/// What the page shows: the code, and the address to go to. The prefilled address when the portal
/// sent one, so the user confirms rather than types; the code is still shown, to check it matches.
fn prompt_for(dc: &DeviceCodeResponse, now_unix: u64) -> SignInPrompt {
    SignInPrompt {
        code: dc.user_code.clone(),
        url: dc.verification_uri_complete.clone().unwrap_or_else(|| dc.verification_uri.clone()),
        expires_at: now_unix.saturating_add(dc.expires_in),
    }
}

/// The token poll response. `expires_in` and `refresh_token` are absent for a GitHub App with token
/// expiry turned off, so both are optional rather than assumed.
#[derive(Deserialize)]
struct TokenPollResponse {
    access_token: Option<String>,
    error: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

/// The shape a rejected OAuth request takes, per RFC 8628 §3.2.
#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: String,
    error_description: Option<String>,
}

/// Turns a response that failed to parse into a message that names the actual problem, instead of an
/// opaque "missing field" error.
fn describe_oauth_failure(portal_name: &str, status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(err) = serde_json::from_str::<OAuthErrorResponse>(body) {
        return match err.error_description {
            Some(desc) => format!("{desc} ({})", err.error),
            None => err.error,
        };
    }
    format!("{portal_name} answered {status} with an unexpected response: {body}")
}

fn build_client() -> Result<Client, AuthError> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| AuthError::Network(e.to_string()))
}

// ── Credential ──────────────────────────────────────────────────────────────────

/// The access token, its expiry if the portal issues one, and a refresh token if it does.
struct Credential {
    access_token: String,
    /// `None` means the token does not expire on a schedule.
    expires_at: Option<SystemTime>,
    refresh_token: Option<String>,
}

impl Credential {
    /// Whether this credential is expired or expiring imminently, so the poll loop can refresh
    /// proactively instead of waiting for a 401.
    fn needs_refresh(&self) -> bool {
        match self.expires_at {
            Some(at) => SystemTime::now() + EXPIRY_SAFETY_MARGIN >= at,
            None => false,
        }
    }
}

fn to_credential(access_token: String, expires_in: Option<u64>, refresh_token: Option<String>) -> Credential {
    let expires_at = expires_in.map(|secs| SystemTime::now() + Duration::from_secs(secs));
    Credential { access_token, expires_at, refresh_token }
}

/// Reads a saved credential from `path`. `None` on anything short of a usable `access_token` —
/// missing file, malformed content, or a file written by an incompatible earlier version all just
/// mean "start fresh with the device flow", never a hard failure.
fn read_credential(path: &Path) -> Option<Credential> {
    let content = std::fs::read_to_string(path).ok()?;

    let mut access_token = None;
    let mut expires_at = None;
    let mut refresh_token = None;

    for line in content.lines() {
        let Some((key, value)) = line.trim().split_once('=') else { continue };
        match key.trim() {
            "access_token" => access_token = Some(value.trim().to_string()).filter(|v| !v.is_empty()),
            "expires_at" => {
                expires_at =
                    value.trim().parse::<u64>().ok().map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
            }
            "refresh_token" => {
                refresh_token = Some(value.trim().to_string()).filter(|v| !v.is_empty());
            }
            _ => {}
        }
    }

    Some(Credential { access_token: access_token?, expires_at, refresh_token })
}

/// Deletes the saved credential. Nothing there is not an error: the outcome is the same.
pub fn forget_saved(token_path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(token_path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Writes a credential with owner-only permissions, since the file holds a live bearer token.
fn save_credential(path: &Path, credential: &Credential) {
    let mut content = format!("access_token={}\n", credential.access_token);
    if let Some(at) = credential.expires_at
        && let Ok(since_epoch) = at.duration_since(UNIX_EPOCH)
    {
        content.push_str(&format!("expires_at={}\n", since_epoch.as_secs()));
    }
    if let Some(refresh_token) = &credential.refresh_token {
        content.push_str(&format!("refresh_token={refresh_token}\n"));
    }

    #[cfg(unix)]
    let written = {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let result = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut file| file.write_all(content.as_bytes()));

        if result.is_ok() {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        result
    };

    #[cfg(not(unix))]
    let written = std::fs::write(path, &content);

    if let Err(e) = written {
        errorln!("warning: could not save PR credential to disk: {e}");
    }
}

/// Exchanges a refresh token for a new access token. Sent without a client secret; see the module
/// doc comment.
///
/// A portal that rotates refresh tokens (GitLab does, on every use) hands back a new one here and
/// the old one is dead from that moment, so the caller must save the result before anything else.
fn refresh(http: &Client, config: &DeviceFlowConfig, refresh_token: &str) -> Result<Credential, AuthError> {
    let response = http
        .post(&config.token_url)
        .header("Accept", "application/json")
        .header("User-Agent", AGENT)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .map_err(|e| AuthError::Network(e.to_string()))?;
    let status = response.status();
    let body = response.text().map_err(|e| AuthError::Network(e.to_string()))?;

    let resp: TokenPollResponse = serde_json::from_str(&body)
        .map_err(|_| config.portal_error(describe_oauth_failure(&config.portal_name, status, &body)))?;

    match resp.access_token {
        Some(access_token) => Ok(to_credential(access_token, resp.expires_in, resp.refresh_token)),
        None => Err(config.portal_error(
            resp.error.unwrap_or_else(|| format!("refresh failed with no error given ({status})")),
        )),
    }
}

/// Step 1 of the device flow: request a device code.
fn request_device_code(http: &Client, config: &DeviceFlowConfig) -> Result<DeviceCodeResponse, AuthError> {
    let mut form = vec![("client_id", config.client_id.as_str())];
    if let Some(scope) = &config.scope {
        form.push(("scope", scope.as_str()));
    }
    let response = http
        .post(&config.device_code_url)
        .header("Accept", "application/json")
        .header("User-Agent", AGENT)
        .form(&form)
        .send()
        .map_err(|e| AuthError::Network(e.to_string()))?;
    let status = response.status();
    let body = response.text().map_err(|e| AuthError::Network(e.to_string()))?;

    serde_json::from_str(&body)
        .map_err(|_| config.portal_error(describe_oauth_failure(&config.portal_name, status, &body)))
}

/// Runs the full device flow and returns the resulting credential.
fn device_code_flow(
    http: &Client,
    config: &DeviceFlowConfig,
    progress: &dyn SignInProgress,
) -> Result<Credential, AuthError> {
    let dc = request_device_code(http, config)?;

    // Whoever started the flow shows the code; this thread only polls. Handed over before the first
    // wait, so the settings page has something to show within a second of the click.
    let now_unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    progress.prompt(prompt_for(&dc, now_unix));

    let mut poll_interval = Duration::from_secs(dc.interval.max(MIN_DEVICE_POLL_INTERVAL));
    let expires_at = Instant::now() + Duration::from_secs(dc.expires_in);

    loop {
        if Instant::now() >= expires_at {
            return Err(AuthError::Expired);
        }

        // The interval is five seconds or more, and a cancel should not have to wait it out: sleep in
        // one-second steps and look up between them.
        let resume_at = Instant::now() + poll_interval;
        while Instant::now() < resume_at {
            if progress.cancelled() {
                return Err(AuthError::Cancelled);
            }
            std::thread::sleep(Duration::from_secs(1).min(resume_at.saturating_duration_since(Instant::now())));
        }
        if progress.cancelled() {
            return Err(AuthError::Cancelled);
        }

        let response = http
            .post(&config.token_url)
            .header("Accept", "application/json")
            .header("User-Agent", AGENT)
            .form(&[
                ("client_id", config.client_id.as_str()),
                ("device_code", dc.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .map_err(|e| AuthError::Network(e.to_string()))?;
        let status = response.status();
        let body = response.text().map_err(|e| AuthError::Network(e.to_string()))?;
        let resp: TokenPollResponse = serde_json::from_str(&body)
            .map_err(|_| config.portal_error(describe_oauth_failure(&config.portal_name, status, &body)))?;

        if let Some(access_token) = resp.access_token {
            return Ok(to_credential(access_token, resp.expires_in, resp.refresh_token));
        }

        match resp.error.as_deref() {
            // Both mean "keep waiting"; only the pacing differs.
            Some("authorization_pending") | None => {}
            Some("slow_down") => poll_interval += SLOW_DOWN_PENALTY,
            Some("expired_token") => return Err(AuthError::Expired),
            Some("access_denied") => return Err(AuthError::Denied),
            Some(other) => return Err(config.portal_error(other.to_string())),
        }
    }
}

// ── Token storage ─────────────────────────────────────────────────────────────

/// Owns one portal's credential and knows how to renew it.
///
/// Lives on the poll thread so a mid-run 401, or an approaching expiry, can be recovered from
/// without restarting the app.
pub struct TokenStore {
    config: DeviceFlowConfig,
    credential: Credential,
    http: Client,
}

impl TokenStore {
    /// Startup path, and deliberately **non-interactive**: reuse a saved credential, refreshing it
    /// first if it is expiring and carries a refresh token.
    ///
    /// `Ok(None)` means there is nothing usable and a device flow is needed. That is reported rather
    /// than run, because launching a browser during startup is the behaviour this design replaces:
    /// the tray icon appears first, wearing the red exclamation, and the user starts the flow from
    /// the menu when it suits them. `Err` is reserved for not being able to build an HTTP client at
    /// all, which no amount of clicking would fix.
    ///
    /// A refresh grant *is* still attempted here, unlike the device flow: it needs no browser and no
    /// human, so there is nothing to defer.
    pub fn load_saved(config: &DeviceFlowConfig) -> Result<Option<Self>, AuthError> {
        let http = build_client()?;

        let Some(saved) = read_credential(&config.token_path) else {
            infoln!("no saved {} credential — waiting for the user to authorize", config.portal_name);
            return Ok(None);
        };

        if !saved.needs_refresh() {
            return Ok(Some(Self { config: config.clone(), credential: saved, http }));
        }

        let Some(refresh_token) = saved.refresh_token.clone() else {
            infoln!("saved {} credential has expired and carries no refresh token", config.portal_name);
            return Ok(None);
        };

        match refresh(&http, config, &refresh_token) {
            Ok(credential) => {
                save_credential(&config.token_path, &credential);
                Ok(Some(Self { config: config.clone(), credential, http }))
            }
            // Could not reach the portal, which says nothing about whether the refresh token is still
            // good — the common cause is a tray app started at login before the network is up. Keep
            // the stale credential and let the poll loop's own `needs_refresh` check retry every
            // cycle, rather than demanding a click for something that heals itself.
            Err(e @ AuthError::Network(_)) => {
                errorln!("could not refresh the {} credential yet ({e}) — retrying on the poll loop", config.portal_name);
                Ok(Some(Self { config: config.clone(), credential: saved, http }))
            }
            Err(e) => {
                errorln!("saved {} credential was rejected ({e}) — waiting for the user to authorize", config.portal_name);
                Ok(None)
            }
        }
    }

    /// The interactive path: runs the full device flow and saves the result.
    ///
    /// Blocks for as long as the flow takes (up to the portal's device-code lifetime), so its caller
    /// must be the poll thread, never the UI thread.
    pub fn authenticate(config: &DeviceFlowConfig, progress: &dyn SignInProgress) -> Result<Self, AuthError> {
        let http = build_client()?;
        let credential = device_code_flow(&http, config, progress)?;
        save_credential(&config.token_path, &credential);
        Ok(Self { config: config.clone(), credential, http })
    }

    pub fn token(&self) -> &str {
        &self.credential.access_token
    }

    /// The client the flow itself used, for a portal's own follow-up call (GitHub's installations).
    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Whether the current credential is expired or expiring imminently.
    pub fn needs_refresh(&self) -> bool {
        self.credential.needs_refresh()
    }

    /// Mid-run recovery, called either after the portal rejects the token or when `needs_refresh`
    /// turns proactive. The refresh grant only.
    ///
    /// When the refresh grant cannot help, this returns `AuthError::AuthorizationRequired` and the
    /// caller raises the exclamation and the Authenticate menu item instead of opening a browser
    /// unannounced hours into a session.
    ///
    /// The portal rejecting the *access* token does not mean the refresh token is bad too, so the grant
    /// is always worth trying first — it recovers silently in the common case.
    pub fn reauthenticate(&mut self) -> Result<(), AuthError> {
        let Some(refresh_token) = self.credential.refresh_token.clone() else {
            return Err(AuthError::AuthorizationRequired);
        };

        match refresh(&self.http, &self.config, &refresh_token) {
            Ok(credential) => {
                save_credential(&self.config.token_path, &credential);
                self.credential = credential;
                Ok(())
            }
            // Passed through unchanged, not converted: a network error must not cost the user a
            // click, so the caller retries next cycle rather than demanding authorization.
            Err(e @ AuthError::Network(_)) => Err(e),
            Err(e) => {
                errorln!("{} credential refresh was rejected ({e}) — authorization required", self.config.portal_name);
                Err(AuthError::AuthorizationRequired)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_at(dir: &Path) -> DeviceFlowConfig {
        DeviceFlowConfig {
            portal_name: "Example".to_string(),
            device_code_url: "https://example.invalid/device".to_string(),
            token_url: "https://example.invalid/token".to_string(),
            client_id: "client".to_string(),
            scope: None,
            token_path: dir.join("token.txt"),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-oauth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test temp dir");
        dir
    }

    /// GitLab sends a link with the code already in it; opening that spares the user typing the code,
    /// and typing it into a blank form after Authorize looked like being asked twice. GitHub sends no
    /// such link, and then the bare address is what the page offers.
    #[test]
    fn the_prompt_links_straight_to_the_prefilled_code_when_the_portal_offers_one() {
        let gitlab: DeviceCodeResponse = serde_json::from_str(
            r#"{"device_code":"d","user_code":"ABCD-1234","verification_uri":"https://gitlab.com/oauth/device",
               "verification_uri_complete":"https://gitlab.com/oauth/device?user_code=ABCD-1234","expires_in":300,"interval":5}"#,
        )
        .unwrap();
        let prompt = prompt_for(&gitlab, 1000);
        assert_eq!(prompt.url, "https://gitlab.com/oauth/device?user_code=ABCD-1234");
        assert_eq!(prompt.code, "ABCD-1234", "still shown, so the user can check it matches");
        assert_eq!(prompt.expires_at, 1300);

        let github: DeviceCodeResponse = serde_json::from_str(
            r#"{"device_code":"d","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(prompt_for(&github, 0).url, "https://github.com/login/device");
    }

    #[test]
    fn a_credential_with_no_expiry_never_needs_refresh() {
        let credential = Credential { access_token: "t".to_string(), expires_at: None, refresh_token: None };
        assert!(!credential.needs_refresh());
    }

    #[test]
    fn a_credential_past_its_expiry_needs_refresh() {
        let credential = Credential {
            access_token: "t".to_string(),
            expires_at: Some(SystemTime::now() - Duration::from_secs(1)),
            refresh_token: None,
        };
        assert!(credential.needs_refresh());
    }

    /// The safety margin exists precisely so a token is renewed before it can expire mid-request,
    /// not only after — so "needs refresh" must trigger before the exact expiry instant.
    #[test]
    fn a_credential_expiring_within_the_safety_margin_needs_refresh() {
        let credential = Credential {
            access_token: "t".to_string(),
            expires_at: Some(SystemTime::now() + Duration::from_secs(30)),
            refresh_token: None,
        };
        assert!(credential.needs_refresh(), "30s left is well inside the 5 minute margin");
    }

    #[test]
    fn a_credential_expiring_well_outside_the_safety_margin_does_not_need_refresh() {
        let credential = Credential {
            access_token: "t".to_string(),
            expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            refresh_token: None,
        };
        assert!(!credential.needs_refresh());
    }

    #[test]
    fn describe_oauth_failure_decodes_the_real_reason() {
        let body = r#"{"error":"unauthorized_client","error_description":"App is not installed."}"#;
        let msg = describe_oauth_failure("GitHub", reqwest::StatusCode::BAD_REQUEST, body);
        assert!(msg.contains("App is not installed"), "got {msg:?}");
        assert!(msg.contains("unauthorized_client"), "got {msg:?}");
    }

    /// The fallback names whoever answered, so a GitLab failure never reads as GitHub's.
    #[test]
    fn describe_oauth_failure_falls_back_to_the_portal_status_and_raw_text() {
        let msg = describe_oauth_failure("GitLab", reqwest::StatusCode::BAD_GATEWAY, "<html>blocked</html>");
        assert!(msg.contains("GitLab"), "got {msg:?}");
        assert!(msg.contains("502"), "got {msg:?}");
        assert!(msg.contains("<html>blocked</html>"), "got {msg:?}");
    }

    #[test]
    fn a_saved_credential_round_trips_through_disk() {
        let dir = scratch("roundtrip");
        let path = dir.join("token.txt");

        let original = Credential {
            access_token: "ghu_abc123".to_string(),
            expires_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            refresh_token: Some("ghr_def456".to_string()),
        };
        save_credential(&path, &original);

        let loaded = read_credential(&path).expect("must read back what was just written");
        assert_eq!(loaded.access_token, original.access_token);
        assert_eq!(loaded.expires_at, original.expires_at);
        assert_eq!(loaded.refresh_token, original.refresh_token);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_credential_with_no_expiry_or_refresh_token_round_trips_too() {
        let dir = scratch("bare");
        let path = dir.join("token.txt");

        let original = Credential { access_token: "ghu_bare".to_string(), expires_at: None, refresh_token: None };
        save_credential(&path, &original);

        let loaded = read_credential(&path).expect("must read back what was just written");
        assert_eq!(loaded.access_token, "ghu_bare");
        assert_eq!(loaded.expires_at, None);
        assert_eq!(loaded.refresh_token, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_file_reads_as_no_saved_credential() {
        let path = std::env::temp_dir().join("githoot-oauth-definitely-does-not-exist.txt");
        assert!(read_credential(&path).is_none());
    }

    /// The behaviour the whole deferred-authorization design rests on: with nothing saved,
    /// `load_saved` must answer "nobody is authorized" *without* touching the network or opening a
    /// browser, so startup can put the tray icon up and wait for a click.
    #[test]
    fn load_saved_reports_needing_authorization_without_running_a_device_flow() {
        let dir = scratch("empty");
        let outcome = TokenStore::load_saved(&config_at(&dir)).expect("building an HTTP client must succeed");
        assert!(outcome.is_none(), "an empty asset directory must mean authorization is needed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A saved, non-expiring credential is reused as-is: no refresh grant, no device flow, no
    /// network at all. This is the ordinary startup path, and it has to stay silent.
    #[test]
    fn load_saved_reuses_a_credential_that_is_not_expiring() {
        let dir = scratch("reuse");
        let config = config_at(&dir);
        save_credential(
            &config.token_path,
            &Credential { access_token: "still_good".to_string(), expires_at: None, refresh_token: None },
        );

        let store = TokenStore::load_saved(&config)
            .expect("building an HTTP client must succeed")
            .expect("a healthy saved credential must be reused");
        assert_eq!(store.token(), "still_good");
        assert!(!store.needs_refresh());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An expired credential with no refresh token cannot be renewed silently, so it must report
    /// needing authorization rather than falling through to a device flow.
    #[test]
    fn load_saved_reports_needing_authorization_for_an_unrenewable_credential() {
        let dir = scratch("stale");
        let config = config_at(&dir);
        save_credential(
            &config.token_path,
            &Credential {
                access_token: "expired".to_string(),
                expires_at: Some(UNIX_EPOCH + Duration::from_secs(1)),
                refresh_token: None,
            },
        );

        let outcome = TokenStore::load_saved(&config).expect("building an HTTP client must succeed");
        assert!(outcome.is_none(), "an unrenewable credential must mean authorization is needed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgetting_a_credential_that_is_not_there_is_not_an_error() {
        let dir = scratch("forget");
        assert!(forget_saved(&dir.join("token.txt")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
