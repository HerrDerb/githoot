//! Portals: the code forges GitHoot can watch.
//!
//! Today there is one, GitHub. This module exists so that is a fact about the contents of the
//! directory and not about the shape of the program: the core (`state`, `page`, `scheduler`) speaks
//! the vocabulary in `types` and the [`Portal`] trait below, and nothing else. A second portal is a
//! new directory beside `github`, not a set of edits across the crate.
//!
//! ## What the seam has to carry
//!
//! The trait is shaped by the portals that are *not* here yet, because a seam cut to fit one side
//! is not a seam:
//!
//! - **One call answers every axis.** GitLab returns reviews requested, authored-and-approved and
//!   authored-and-blocked from a single GraphQL document. Forcing three calls would triple its cost
//!   for nothing, so [`Portal::poll`] takes the axes wanted and answers all of them at once. GitHub
//!   issues three requests inside its own `poll`, as it always did.
//! - **Auth styles differ.** GitHub and GitLab hand out tokens through a device flow started from
//!   the menu. Bitbucket Cloud has neither a device flow nor PKCE, so its user pastes a token they
//!   made on the site. Both are "get me a credential", so both are methods on the portal, and the
//!   difference is declared in [`Capabilities::auth_style`] for the UI's wording.
//! - **Not every portal knows everything.** Bitbucket cannot say whether a branch conflicts and has
//!   no notion of a re-review being pending. [`Capabilities`] says so, and an honest adapter leaves
//!   the field it cannot fill at its "no evidence" value rather than guessing.
//! - **Rate budgets differ.** Bitbucket allows a thousand requests an hour, and the reviews-requested
//!   list there costs one request per repository. [`PortalInfo::min_poll_interval`] is the floor a
//!   portal asks for, and the scheduler paces to the slowest.

// Until the scheduler drives portals through the trait (a few commits from now) nothing outside
// this module constructs these types. Removed with the first real caller.
#![allow(dead_code)]

pub mod github;
pub mod statuspage;
pub mod types;

#[cfg(test)]
pub mod fake;

use std::time::Duration;

use types::PollResponse;

/// Stable identity of one configured portal instance.
///
/// The config section name: `github` for the implicit one every existing install has. Never shown to
/// the user, who sees [`PortalInfo::display_name`]; this is for telling two portals of the same kind
/// apart, which a display name cannot be relied on to do.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PortalId(pub String);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortalKind {
    GitHub,
}

/// How a portal's credential comes to exist. Read by the UI for wording, never by the scheduler:
/// both styles go through [`Portal::authenticate`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthStyle {
    /// A browser round trip started from the menu: GitHub App device flow, GitLab 17.9+.
    DeviceFlow,
    /// The user makes a token on the portal's own site and hands it over. Bitbucket Cloud.
    PastedToken,
}

/// What a portal can and cannot tell us. Declared once so the page can hide a pill it would only
/// ever render at its "no evidence" value, rather than every adapter re-explaining the gap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Capabilities {
    pub auth_style: AuthStyle,
    /// Whether the portal says when a head branch conflicts with its base. GitHub and GitLab do;
    /// Bitbucket Cloud has no such field.
    pub conflict_state: bool,
    /// Whether "a re-review was asked for" is distinguishable from a standing verdict.
    pub rereview_pending: bool,
    /// Whether a review can be requested of a team rather than a person.
    pub team_reviewers: bool,
    /// The automatic reviewer whose unresolved comments count as work, when the portal has one.
    /// `Some("Copilot")` for GitHub.
    pub bot_reviewer: Option<&'static str>,
}

/// A portal's status page and how the menu names it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusPage {
    /// Where a click sends the user. The page, not the JSON endpoint.
    pub url: String,
    /// The menu entry shown while the portal is degraded.
    pub menu_label: String,
}

/// Everything the UI and the page need to know about a portal without holding the portal itself.
///
/// The adapter lives on the poll thread and owns a credential and an HTTP client; the menu and the
/// loopback server run elsewhere and only need names and URLs. This is cloned out of the adapter
/// before it moves, so the two threads never share it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PortalInfo {
    pub id: PortalId,
    pub kind: PortalKind,
    /// "GitHub". Goes into "Authenticate {display_name} PR Status" and prefixes tooltip lines.
    pub display_name: String,
    /// Only URLs starting with this become links on the page; anything else is shown as text. The
    /// trailing slash is load-bearing: `https://github.com/` refuses `https://github.com.evil.com/`.
    pub link_prefix: String,
    /// Where "open my pull requests" goes when no confirmed list exists to open instead.
    pub inbox_url: String,
    /// `None` for a portal that publishes no machine-readable status.
    pub status_page: Option<StatusPage>,
    pub capabilities: Capabilities,
    /// The slowest this portal wants to be asked. The scheduler never polls faster than the largest
    /// floor among the configured portals.
    pub min_poll_interval: Duration,
}

/// Whether a portal is in a position to be polled.
///
/// Three outcomes rather than an `Option`, because "no dots on the icon" has three quite different
/// meanings and each is said differently. Collapsing them is exactly the confusion this codebase is
/// shaped around avoiding: a dark icon that means "nothing needs you" must never look like one that
/// means "nobody could ask".
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CredentialState {
    /// A usable credential. Polling can start.
    Ready,
    /// Nothing usable, or what was there can no longer be renewed silently. Red exclamation,
    /// `Authenticate` on the menu, one click from being fixed.
    NeedsAuth,
    /// Signed in, but there is nothing to see and clicking would not change that. The reason
    /// travels with it, for the tooltip.
    Off(String),
}

/// One poll's answers, indexed by `PrAxis::index`.
///
/// `None` for an axis the caller did not ask for. An adapter that answers all three from one round
/// trip fills all three slots from that one body; one that must ask three times fills each as it
/// goes, and a failure on one axis is that axis's `PollResult`, not the whole outcome's.
#[derive(Debug, Default)]
pub struct PollOutcome {
    pub axes: [Option<PollResponse>; 3],
}

/// What a portal says about its own health, when it publishes any.
#[derive(Debug, PartialEq, Eq)]
pub enum Health {
    /// Operational, or scheduled maintenance.
    Fine,
    /// Carries the portal's own wording, so the tooltip quotes it rather than paraphrasing:
    /// "Partially Degraded Service" is more use than anything this app would invent.
    Degraded { description: String },
}

/// A health verdict, plus the configured component names that matched nothing the portal publishes.
///
/// The two travel together because only the caller knows whether it has said so already: the check
/// runs every few minutes and a typo is permanent, so complaining from in here would repeat the same
/// line forever. See `scheduler`, which says it once and again only when it changes.
#[derive(Debug)]
pub struct HealthReport {
    pub health: Health,
    /// Empty on a page-wide check, which has no names to match.
    pub unmatched: Vec<String>,
}

/// Why a credential could not be obtained or renewed.
#[derive(Debug)]
pub enum AuthError {
    Network(String),
    Denied,
    Expired,
    /// The portal answered, and the answer was a refusal or nonsense. Named so the message can
    /// quote who said it.
    Portal { name: String, detail: String },
    /// Nothing left to try without the user: there is no refresh token, or the portal rejected the
    /// one we have. The only way forward needs a browser and a human, so this is reported rather
    /// than started; the caller raises the tray's Authenticate item and waits.
    ///
    /// Deliberately distinct from `Network`: unreachable is not the same as invalid, and a laptop
    /// launched before its WiFi is up must not be told to sign in again.
    AuthorizationRequired,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Network(e) => write!(f, "network error during authorization: {e}"),
            AuthError::Denied => write!(f, "authorization was denied"),
            AuthError::Expired => write!(f, "device code expired before authorization completed"),
            AuthError::Portal { name, detail } => write!(f, "{name} reported: {detail}"),
            AuthError::AuthorizationRequired => write!(f, "authorization required"),
        }
    }
}

impl std::error::Error for AuthError {}

/// One code forge, as the poll loop sees it.
///
/// `Send` and not `Sync`: exactly one thread owns a portal, the poll thread, and everything another
/// thread needs about it is in the [`PortalInfo`] it cloned out beforehand. Every method is
/// synchronous because the whole app is: the poll thread already serialises its requests, and the
/// blocking `reqwest` client is what every adapter uses.
///
/// Object-safe on purpose. The scheduler holds `Vec<Box<dyn Portal>>`, and the test double in
/// `fake` is the compile-time proof that nothing in this signature needs a GitHub type.
pub trait Portal: Send {
    fn info(&self) -> &PortalInfo;

    /// The startup path, and deliberately **non-interactive**: reuse a saved credential, refreshing
    /// it first if that needs no human. `Err` means no HTTP client could be built at all, which no
    /// amount of clicking would fix; `Ok(NeedsAuth)` is the everyday first-run answer.
    fn load_saved_credential(&mut self) -> Result<CredentialState, AuthError>;

    /// The interactive path. Blocks for as long as the user takes, so its caller must be the poll
    /// thread and never the UI thread.
    fn authenticate(&mut self) -> Result<CredentialState, AuthError>;

    /// Whether the credential is expired or about to be. `false` for a portal whose tokens do not
    /// expire on a schedule.
    fn needs_refresh(&self) -> bool;

    /// Silent renewal. `Err(AuthError::AuthorizationRequired)` hands the problem to the user.
    fn reauthenticate(&mut self) -> Result<(), AuthError>;

    /// Asks for the axes marked `true`, indexed by `PrAxis::index`. An axis marked `false` must cost
    /// no request: the caller has already decided it is not in play.
    fn poll(&mut self, axes: [bool; 3]) -> PollOutcome;

    /// The portal's own view of its health. `None` means it publishes none and nothing should be
    /// shown; `Some(Err)` means we could not find out, which is not an outage. How often to ask is
    /// the scheduler's decision, not the portal's.
    fn health(&mut self) -> Option<Result<HealthReport, String>>;
}
