//! Whether a forge on status.io is having a bad day. GitLab.com is the one this was written for.
//!
//! The sibling of [`super::statuspage`], which reads Atlassian Statuspage for GitHub. Two readers rather
//! than one because the two services share nothing but the idea: status.io answers
//! `https://api.status.io/1.0/status/<page id>`, unauthenticated, with a numeric `status_code` on the
//! page as a whole and on every component. Which reader a portal uses is the portal adapter's choice,
//! in its `health()`; everything after that (the mark, the tooltip, the menu entry) is shared.
//!
//! ## What counts as an outage
//!
//! status.io's codes, from its documentation: 100 Operational, 200 maintenance, 300 Degraded
//! Performance, 400 Partial Service Disruption, 500 Service Disruption, 600 Security Event. From 300
//! up raises the mark, the same line `statuspage` draws at `minor`. Maintenance does not, because it
//! is announced. A code this version has never seen does not either, so an unfamiliar value cannot
//! invent an outage.
//!
//! The page-wide status and the components are both read, and the worst of them counts. A component
//! can be degraded while the page still says Operational, and the components are also what the
//! tooltip names, which is more use than the page's one word.
//!
//! ## Why a failure here is not an outage
//!
//! The same inversion as `statuspage`: this is an alarm, and an alarm raised because our own request
//! to a third-party page failed would cry wolf on the user's flaky wifi. An unreadable answer is `Err`,
//! which the scheduler logs and otherwise ignores.

use reqwest::blocking::Client;
use serde::Deserialize;

use super::{Health, HealthReport};

/// The lowest code that raises the mark: Degraded Performance.
const DEGRADED_FROM: u16 = 300;
/// The highest code status.io documents: Security Event. Anything above is unknown and ignored.
const KNOWN_UP_TO: u16 = 600;

#[derive(Debug, Deserialize)]
struct Response {
    result: Result_,
}

#[derive(Debug, Deserialize)]
struct Result_ {
    status_overall: Status,
    #[serde(default)]
    status: Vec<Component>,
}

#[derive(Debug, Deserialize)]
struct Status {
    status: String,
    status_code: u16,
}

#[derive(Debug, Deserialize)]
struct Component {
    name: String,
    status: String,
    status_code: u16,
}

/// Whether `code` raises the mark.
fn degraded(code: u16) -> bool {
    (DEGRADED_FROM..=KNOWN_UP_TO).contains(&code)
}

/// The verdict a status.io answer gives. `Err` for a body that is not one.
pub fn verdict(body: &str) -> Result<Health, String> {
    let page = serde_json::from_str::<Response>(body).map_err(|e| format!("not a status.io answer: {e}"))?.result;
    let affected: Vec<&Component> = page.status.iter().filter(|c| degraded(c.status_code)).collect();
    // The page's own word when it has one for this, else the worst component's.
    let word = if degraded(page.status_overall.status_code) {
        page.status_overall.status.as_str()
    } else {
        match affected.iter().max_by_key(|c| c.status_code) {
            Some(worst) => worst.status.as_str(),
            None => return Ok(Health::Fine),
        }
    };
    let names: Vec<&str> = affected.iter().map(|c| c.name.as_str()).collect();
    let description = if names.is_empty() { word.to_string() } else { format!("{word}: {}", names.join(", ")) };
    Ok(Health::Degraded { description })
}

/// Asks status.io how `page_id` is doing. `Err` means we could not find out, which is not an outage.
pub fn check(client: &Client, page_id: &str) -> Result<HealthReport, String> {
    let response = client
        .get(format!("https://api.status.io/1.0/status/{page_id}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, "githoot")
        .send()
        .map_err(|e| format!("could not reach the status page: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("the status page answered {status}"));
    }
    let body = response.text().map_err(|e| format!("could not read the status page: {e}"))?;
    Ok(HealthReport { health: verdict(&body)?, unmatched: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A status.io answer, trimmed from GitLab.com's of 2026-10-01 to what is read, with `overall` on
    /// the page and each `(name, code)` as a component.
    fn body(overall: (&str, u16), components: &[(&str, &str, u16)]) -> String {
        let components: Vec<String> = components
            .iter()
            .map(|(name, status, code)| format!(r#"{{"id":"x","name":"{name}","status":"{status}","status_code":{code},"containers":[]}}"#))
            .collect();
        format!(
            r#"{{"result":{{"status_overall":{{"updated":"2026-10-01T12:12:07.169Z","status":"{}","status_code":{}}},"status":[{}],"incidents":[],"maintenance":{{"active":[],"upcoming":[]}}}}}}"#,
            overall.0,
            overall.1,
            components.join(",")
        )
    }

    const OK: (&str, &str, u16) = ("Website", "Operational", 100);

    #[test]
    fn everything_operational_is_fine() {
        assert_eq!(verdict(&body(("Operational", 100), &[OK, ("API", "Operational", 100)])), Ok(Health::Fine));
    }

    /// The page-wide word is what the tooltip leads with, and the components at fault follow it.
    #[test]
    fn a_disruption_is_degraded_and_names_what_is_affected() {
        let said = verdict(&body(
            ("Partial Service Disruption", 400),
            &[OK, ("CI/CD", "Partial Service Disruption", 400), ("API", "Degraded Performance", 300)],
        ));
        assert_eq!(said, Ok(Health::Degraded { description: "Partial Service Disruption: CI/CD, API".to_string() }));
    }

    /// A component can be degraded while the page still says Operational. The worst one speaks.
    #[test]
    fn a_degraded_component_counts_even_when_the_page_says_operational() {
        let said = verdict(&body(("Operational", 100), &[OK, ("Git Operations", "Degraded Performance", 300)]));
        assert_eq!(said, Ok(Health::Degraded { description: "Degraded Performance: Git Operations".to_string() }));
    }

    /// Maintenance is announced, and a code nobody documented cannot invent an outage.
    #[test]
    fn maintenance_and_unknown_codes_are_not_an_outage() {
        assert_eq!(verdict(&body(("Planned Maintenance", 200), &[("API", "Planned Maintenance", 200)])), Ok(Health::Fine));
        assert_eq!(verdict(&body(("Something New", 700), &[("API", "Something New", 700)])), Ok(Health::Fine));
    }

    #[test]
    fn every_documented_code_from_degraded_up_raises_the_mark() {
        for code in [300, 400, 500, 600] {
            assert!(matches!(verdict(&body(("Bad", code), &[])), Ok(Health::Degraded { .. })), "{code}");
        }
        assert_eq!(verdict(&body(("Service Disruption", 500), &[])), Ok(Health::Degraded { description: "Service Disruption".to_string() }));
    }

    /// GitLab.com's whole answer as it came on 2026-10-01, every field it sends, all operational. The
    /// trimmed bodies above test the rule; this one tests that nothing in the real shape trips it.
    #[test]
    fn gitlab_coms_real_answer_reads_as_fine() {
        assert_eq!(verdict(include_str!("fixtures/statusio-gitlab-2026-10-01.json")), Ok(Health::Fine));
    }

    /// Hits GitLab.com's real status page through status.io, with the id the GitLab portal uses.
    /// Ignored by default, like the GitHub one. The only thing that can catch status.io changing its
    /// shape, or GitLab moving its page.
    #[test]
    #[ignore = "needs network; queries GitLab.com's real status page on status.io"]
    fn reads_gitlab_coms_live_status_page() {
        let client = crate::portal::github::api::build_client().expect("a client");
        match check(&client, crate::portal::gitlab::STATUSIO_PAGE_ID) {
            Ok(report) => println!("live GitLab health: {:?}", report.health),
            Err(e) => panic!("could not read GitLab.com's live status page: {e}"),
        }
    }

    /// Not status.io's shape: we could not find out, which the scheduler must not read as an outage.
    #[test]
    fn a_body_that_is_not_a_status_answer_is_an_error() {
        assert!(verdict("<html>maintenance page</html>").is_err());
        assert!(verdict(r#"{"result":{}}"#).is_err());
    }
}
