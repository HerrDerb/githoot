//! A portal that says whatever a test tells it to.
//!
//! This file imports nothing from `portal::github`, and that is its first job: it compiles only if
//! the [`Portal`] trait can be implemented without a single GitHub type, which is the whole promise
//! of the seam. Its second job is to let `scheduler` and the overview be driven without a network.

use std::collections::VecDeque;
use std::time::Duration;

use super::{
    AuthError, AuthStyle, Capabilities, CredentialState, Health, HealthReport, PollOutcome, Portal,
    PortalId, PortalInfo, PortalKind,
};

pub struct FakePortal {
    pub info: PortalInfo,
    /// Handed back one per `poll`, in order. An empty queue answers with nothing on every axis.
    pub scripted: VecDeque<PollOutcome>,
    pub credential: CredentialState,
    pub health: Option<Result<Health, String>>,
    pub refresh_due: bool,
    pub authenticate_calls: u32,
    pub reauthenticate_calls: u32,
    /// Every `axes` argument `poll` was called with, so a test can assert what was asked for.
    pub asked: Vec<[bool; 3]>,
}

impl FakePortal {
    pub fn named(name: &str) -> Self {
        FakePortal {
            info: PortalInfo {
                id: PortalId(name.to_lowercase()),
                // No `Fake` variant: the kind names a real forge, and a fake standing in for one is
                // the point.
                kind: PortalKind::GitHub,
                display_name: name.to_string(),
                link_prefix: format!("https://{}.example/", name.to_lowercase()),
                inbox_url: format!("https://{}.example/inbox", name.to_lowercase()),
                status_page: None,
                capabilities: Capabilities {
                    auth_style: AuthStyle::PastedToken,
                    conflict_state: false,
                    rereview_pending: false,
                    team_reviewers: false,
                    bot_reviewer: None,
                },
                min_poll_interval: Duration::from_secs(60),
            },
            scripted: VecDeque::new(),
            credential: CredentialState::Ready,
            health: None,
            refresh_due: false,
            authenticate_calls: 0,
            reauthenticate_calls: 0,
            asked: Vec::new(),
        }
    }

    pub fn script(mut self, outcome: PollOutcome) -> Self {
        self.scripted.push_back(outcome);
        self
    }
}

impl Default for FakePortal {
    fn default() -> Self {
        FakePortal::named("Fake")
    }
}

impl Portal for FakePortal {
    fn info(&self) -> &PortalInfo {
        &self.info
    }

    fn load_saved_credential(&mut self) -> Result<CredentialState, AuthError> {
        Ok(self.credential.clone())
    }

    fn authenticate(&mut self) -> Result<CredentialState, AuthError> {
        self.authenticate_calls += 1;
        self.credential = CredentialState::Ready;
        Ok(CredentialState::Ready)
    }

    fn needs_refresh(&self) -> bool {
        self.refresh_due
    }

    fn reauthenticate(&mut self) -> Result<(), AuthError> {
        self.reauthenticate_calls += 1;
        self.refresh_due = false;
        Ok(())
    }

    fn poll(&mut self, axes: [bool; 3]) -> PollOutcome {
        self.asked.push(axes);
        self.scripted.pop_front().unwrap_or_default()
    }

    fn health(&mut self) -> Option<Result<HealthReport, String>> {
        match self.health.take() {
            None => None,
            Some(Ok(health)) => Some(Ok(HealthReport { health, unmatched: Vec::new() })),
            Some(Err(e)) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::types::{PollResponse, PollResult};

    fn fresh(count: u32) -> PollResponse {
        PollResponse {
            result: PollResult::Fresh { present: count > 0, count: Some(count), prs: Some(Vec::new()) },
            poll_interval: None,
        }
    }

    fn count_of(response: &Option<PollResponse>) -> Option<u32> {
        match response {
            Some(PollResponse { result: PollResult::Fresh { count, .. }, .. }) => *count,
            _ => None,
        }
    }

    /// The seam's contract in one line: a portal is a trait object, and one exists without GitHub.
    #[test]
    fn a_portal_is_an_object_that_needs_no_github() {
        let portal: Box<dyn Portal> = Box::new(FakePortal::default());
        assert_eq!(portal.info().display_name, "Fake");
    }

    #[test]
    fn the_fake_answers_its_script_one_poll_at_a_time() {
        let mut portal = FakePortal::default()
            .script(PollOutcome { axes: [Some(fresh(2)), None, Some(fresh(0))] })
            .script(PollOutcome { axes: [Some(fresh(3)), None, None] });

        let first = portal.poll([true, false, true]);
        assert_eq!(count_of(&first.axes[0]), Some(2));
        assert!(first.axes[1].is_none());
        assert_eq!(count_of(&first.axes[2]), Some(0));

        let second = portal.poll([true, false, false]);
        assert_eq!(count_of(&second.axes[0]), Some(3));

        let third = portal.poll([true, true, true]);
        assert!(third.axes.iter().all(Option::is_none), "an exhausted script answers nothing");
        assert_eq!(portal.asked, vec![[true, false, true], [true, false, false], [true, true, true]]);
    }

    #[test]
    fn credentials_and_health_are_scripted_too() {
        let mut portal = FakePortal::default();
        portal.credential = CredentialState::NeedsAuth;
        assert_eq!(portal.load_saved_credential().unwrap(), CredentialState::NeedsAuth);
        assert_eq!(portal.authenticate().unwrap(), CredentialState::Ready);
        assert_eq!(portal.authenticate_calls, 1);

        portal.refresh_due = true;
        assert!(portal.needs_refresh());
        portal.reauthenticate().unwrap();
        assert_eq!(portal.reauthenticate_calls, 1);
        assert!(!portal.needs_refresh(), "a renewal clears the due flag");

        assert!(portal.health().is_none(), "a portal with no status page says nothing");
        portal.health = Some(Ok(Health::Degraded { description: "Wobbly".to_string() }));
        let report = portal.health().unwrap().unwrap();
        assert_eq!(report.health, Health::Degraded { description: "Wobbly".to_string() });
    }

    #[test]
    fn the_error_names_the_portal_that_spoke() {
        let e = AuthError::Portal { name: "GitHub".to_string(), detail: "bad_verification_code".to_string() };
        assert_eq!(e.to_string(), "GitHub reported: bad_verification_code");
    }
}
