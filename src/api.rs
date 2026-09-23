//! The machine-readable view of one axis, for a script running as you on this machine.
//!
//! This is the `localApi` wire format. It is a **contract with something outside this repo**, which
//! is the whole reason it is shaped by hand here rather than derived off [`PrEntry`]: a rename in
//! `portal::types` must not silently become a breaking change for a dispatcher nobody here can see.
//! `entry_json` destructures its argument for exactly that reason, so a new field on `PrEntry` is a
//! compile error in the one place that has to decide whether it belongs on the wire.
//!
//! ## The rules the shape exists to enforce
//!
//! **`entries` is `null` when the list is not confirmed, never `[]`.** This is the same refusal to
//! report a confident zero that the icon, the tooltip and the page all make, pushed out to a caller
//! that cannot see any of them. A script that reads an outage as an empty board goes quiet at the
//! exact moment it was written to act, so the careless path (`.entries.length`, `jq '.entries[]'`)
//! has to throw rather than yield nothing. `known` is carried beside it for the careful path.
//!
//! **`known` at the top level covers the third hole**, `portals: []`, which happens when nothing is
//! configured or nothing has been published yet. An empty array of portals would otherwise read as
//! a confident zero one level up from the one this module guards.
//!
//! **Every key is always present**, `null` when there is nothing to say. A missing key and a `null`
//! are the same thing to most clients but not to all of them, and one of the two has to be the rule.
//!
//! **`checks` is never `null`**, because `CheckRollup::Unknown` is a real answer covering three
//! indistinguishable causes (see its doc comment). An absent field would let a client infer success.
//!
//! **`muted` says whether you have muted it** (from the PR page, for 3, 7 or 30 days), and
//! `muted_until_unix` until when. Muted pull requests are still listed, because hiding them would be
//! the API deciding for the caller; they do not count on the icon, and a consumer acting on the bar
//! should skip them the way the shipped dispatcher does.
//!
//! **There is no `count` field.** The array length is the count, and a second representation of it
//! is a second thing that can disagree.

use crate::portal::PortalInfo;
use crate::portal::types::{CheckRollup, PrEntry, ReviewState, Reviewer, Verdict};
use crate::scheduler::AxisSnapshot;
use crate::state::PrAxis;
use serde_json::{Value, json};

/// Bumped only for a change a caller written against the old shape could not survive.
const SCHEMA: u64 = 1;

/// One axis, as a local script reads it.
///
/// Takes the snapshot rather than `page::groups` so the machine contract does not travel through
/// the page's view type and pick up its rendering concerns.
pub fn entries_json(axis: PrAxis, snapshot: &AxisSnapshot, now_unix: u64) -> String {
    entries_json_with(axis, snapshot, now_unix, &|key| crate::mute::until(key, now_unix))
}

/// `entries_json`, with "is this muted, and until when" handed in, so tests need no mute file.
pub fn entries_json_with(
    axis: PrAxis,
    snapshot: &AxisSnapshot,
    now_unix: u64,
    muted: &dyn Fn(&str) -> Option<u64>,
) -> String {
    json!({
        "schema": SCHEMA,
        "axis": axis.slug(),
        "version": snapshot.version,
        "generated_at_unix": now_unix,
        // An age at this instant, not a date, for the same reason `page::items_json` sends one:
        // a rendered date would be stale the moment it arrived. `null` before the first poll,
        // never `0`, which would claim data polled this very second.
        "polled_age_seconds": snapshot.polled_at.map(|d| d.as_secs()),
        // False when no portal confirmed anything, *including* when there are no portals at all.
        "known": snapshot.groups.iter().any(|(_, entries)| entries.is_some()),
        "portals": snapshot
            .groups
            .iter()
            .map(|(info, entries)| portal_json(info, entries.as_deref(), muted))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn portal_json(info: &PortalInfo, entries: Option<&[PrEntry]>, muted: &dyn Fn(&str) -> Option<u64>) -> Value {
    json!({
        "id": info.id.0,
        "display_name": info.display_name,
        "inbox_url": info.inbox_url,
        "known": entries.is_some(),
        // `None` becomes `null`, never `[]`. See the module doc.
        "entries": entries.map(|list| list.iter().map(|e| entry_json(e, muted(e.key()))).collect::<Vec<_>>()),
    })
}

fn entry_json(e: &PrEntry, muted_until: Option<u64>) -> Value {
    // Destructured, not field-accessed: a new field on `PrEntry` must not be able to go missing
    // from the machine contract without a compile error right here.
    let PrEntry {
        id,
        url,
        title,
        repo,
        number,
        author,
        updated_at,
        is_draft,
        conflicting,
        bot_review,
        checks,
        verdicts,
        pending,
    } = e;
    json!({
        // What the hoot ledger files this under, and the most stable handle a script can hold.
        // Duplicated as `id` because `key` falls back to the URL and a caller may want to know
        // which of the two it got.
        "key": e.key(),
        "id": id,
        "url": url,
        "title": title,
        "repo": repo,
        "number": number,
        "author": author,
        "updated_at": updated_at,
        "is_draft": is_draft,
        "conflicting": conflicting,
        "bot_review": bot_review.as_ref().map(|b| json!({ "name": b.name, "unresolved": b.unresolved })),
        "checks": checks_str(*checks),
        "verdicts": verdicts.iter().map(verdict_json).collect::<Vec<_>>(),
        "pending_reviewers": pending.iter().map(reviewer_json).collect::<Vec<_>>(),
        "muted": muted_until.is_some(),
        "muted_until_unix": muted_until,
    })
}

fn checks_str(checks: CheckRollup) -> &'static str {
    match checks {
        CheckRollup::Unknown => "unknown",
        CheckRollup::Success => "success",
        CheckRollup::Pending => "pending",
        CheckRollup::Failure => "failure",
        CheckRollup::Error => "error",
        CheckRollup::Expected => "expected",
    }
}

fn verdict_json(v: &Verdict) -> Value {
    json!({
        "login": v.login,
        "state": match v.state {
            ReviewState::Approved => "approved",
            ReviewState::ChangesRequested => "changes_requested",
        },
    })
}

/// A team is tagged rather than flattened to a string because it has no login, which is exactly why
/// `still_on_you` cannot match one. A caller that means to look a reviewer up has to know which.
fn reviewer_json(r: &Reviewer) -> Value {
    match r {
        Reviewer::User(login) => json!({ "kind": "user", "name": login }),
        Reviewer::Team(slug) => json!({ "kind": "team", "name": slug }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::types::BotReview;
    use crate::portal::{AuthStyle, Capabilities, PortalId, PortalKind};
    use std::time::Duration;

    fn info(id: &str) -> PortalInfo {
        PortalInfo {
            id: PortalId(id.to_string()),
            kind: PortalKind::GitHub,
            display_name: "GitHub".to_string(),
            link_prefix: "https://github.com/".to_string(),
            inbox_url: "https://github.com/pulls/inbox".to_string(),
            status_page: None,
            capabilities: Capabilities {
                auth_style: AuthStyle::DeviceFlow,
                conflict_state: true,
                rereview_pending: true,
                team_reviewers: true,
                bot_reviewer: Some("Copilot"),
            },
            min_poll_interval: Duration::from_secs(60),
        }
    }

    fn snapshot(groups: Vec<(PortalInfo, Option<Vec<PrEntry>>)>) -> AxisSnapshot {
        AxisSnapshot { groups, polled_at: Some(Duration::from_secs(47)), version: 12 }
    }

    fn parse(json: &str) -> Value {
        serde_json::from_str(json).expect("the api emits valid JSON")
    }

    /// The one thing this feature could get catastrophically wrong. `None` means "no answer we still
    /// stand behind"; reported as `[]` it would tell a dispatcher there is nothing to do, at the
    /// exact moment a poll was failing.
    #[test]
    fn a_portal_with_no_confirmed_list_serialises_as_null_and_never_as_an_empty_array() {
        let out = entries_json(PrAxis::ReviewRequested, &snapshot(vec![(info("github"), None)]), 0);
        let v = parse(&out);
        assert_eq!(v["portals"][0]["entries"], Value::Null);
        assert_eq!(v["portals"][0]["known"], json!(false));
        assert_eq!(v["known"], json!(false));
    }

    /// The other half: a confirmed empty list is a real answer and must not look like a missing one.
    #[test]
    fn a_confirmed_empty_list_is_distinguishable_from_a_list_that_is_not_known() {
        let empty = entries_json(PrAxis::ReadyToMerge, &snapshot(vec![(info("github"), Some(vec![]))]), 0);
        let unknown = entries_json(PrAxis::ReadyToMerge, &snapshot(vec![(info("github"), None)]), 0);
        assert_ne!(empty, unknown);
        let v = parse(&empty);
        assert_eq!(v["portals"][0]["entries"], json!([]));
        assert_eq!(v["portals"][0]["known"], json!(true));
        assert_eq!(v["known"], json!(true));
    }

    /// The third confident-zero hole, one level up: no portals at all, because none are configured
    /// or the poll loop has not published yet. An empty `portals` array must not read as "all clear".
    #[test]
    fn a_snapshot_with_no_portals_at_all_is_reported_as_not_known() {
        let out = entries_json(PrAxis::ChangesRequested, &snapshot(vec![]), 0);
        let v = parse(&out);
        assert_eq!(v["known"], json!(false));
        assert_eq!(v["portals"], json!([]));
    }

    /// `CheckRollup::Unknown` covers three indistinguishable causes and all three must stay neutral.
    /// Omitting the field would let a client infer success from its absence.
    #[test]
    fn an_unknown_check_rollup_is_an_explicit_value_and_never_a_missing_field() {
        let pr = PrEntry::stub("https://github.com/o/r/pull/1");
        assert_eq!(pr.checks, CheckRollup::Unknown);
        let out = entries_json(PrAxis::ReviewRequested, &snapshot(vec![(info("github"), Some(vec![pr]))]), 0);
        assert_eq!(parse(&out)["portals"][0]["entries"][0]["checks"], json!("unknown"));
    }

    /// A team has no login, which is why `still_on_you` cannot match one. Flattening both kinds to a
    /// bare string would lose the distinction a caller needs to look a reviewer up.
    #[test]
    fn a_pending_team_is_distinguishable_from_a_pending_user() {
        let mut pr = PrEntry::stub("https://github.com/o/r/pull/1");
        pr.pending = vec![Reviewer::User("alice".into()), Reviewer::Team("platform".into())];
        let out = entries_json(PrAxis::ReviewRequested, &snapshot(vec![(info("github"), Some(vec![pr]))]), 0);
        assert_eq!(
            parse(&out)["portals"][0]["entries"][0]["pending_reviewers"],
            json!([{ "kind": "user", "name": "alice" }, { "kind": "team", "name": "platform" }])
        );
    }

    /// The drift guard for a hand-written shape. `entry_json`'s destructuring bind makes a new field
    /// a compile error; this makes a field that compiles but never reaches the wire a test failure.
    #[test]
    fn every_field_of_a_pr_entry_reaches_the_json() {
        let pr = PrEntry {
            id: Some("PR_kwDOAbCd".into()),
            url: "https://github.com/acme/widget/pull/7".into(),
            title: Some("Tighten the poll backoff".into()),
            repo: Some("acme/widget".into()),
            number: Some(7),
            author: Some("alice".into()),
            updated_at: Some("2026-09-20T11:04:00Z".into()),
            is_draft: true,
            conflicting: true,
            bot_review: Some(BotReview { name: "Copilot".into(), unresolved: 3 }),
            checks: CheckRollup::Failure,
            verdicts: vec![Verdict { login: "bob".into(), state: ReviewState::Approved }],
            pending: vec![Reviewer::Team("platform".into())],
        };
        let out = entries_json(PrAxis::ReviewRequested, &snapshot(vec![(info("github"), Some(vec![pr]))]), 1758499200);
        let e = &parse(&out)["portals"][0]["entries"][0];
        assert_eq!(e["key"], json!("PR_kwDOAbCd"));
        assert_eq!(e["id"], json!("PR_kwDOAbCd"));
        assert_eq!(e["url"], json!("https://github.com/acme/widget/pull/7"));
        assert_eq!(e["title"], json!("Tighten the poll backoff"));
        assert_eq!(e["repo"], json!("acme/widget"));
        assert_eq!(e["number"], json!(7));
        assert_eq!(e["author"], json!("alice"));
        assert_eq!(e["updated_at"], json!("2026-09-20T11:04:00Z"));
        assert_eq!(e["is_draft"], json!(true));
        assert_eq!(e["conflicting"], json!(true));
        assert_eq!(e["bot_review"], json!({ "name": "Copilot", "unresolved": 3 }));
        assert_eq!(e["checks"], json!("failure"));
        assert_eq!(e["verdicts"], json!([{ "login": "bob", "state": "approved" }]));
        assert_eq!(e["pending_reviewers"], json!([{ "kind": "team", "name": "platform" }]));
        assert_eq!(e["muted"], json!(false));
        assert_eq!(e["muted_until_unix"], Value::Null);
    }

    /// A muted pull request is still listed, marked, with its end time; the rest are not.
    #[test]
    fn a_muted_entry_is_listed_and_marked_with_its_end() {
        let a = PrEntry { id: Some("PR_a".into()), ..PrEntry::stub("https://github.com/o/r/pull/1") };
        let b = PrEntry { id: Some("PR_b".into()), ..PrEntry::stub("https://github.com/o/r/pull/2") };
        let snap = snapshot(vec![(info("github"), Some(vec![a, b]))]);
        let out = entries_json_with(PrAxis::ChangesRequested, &snap, 0, &|k| (k == "PR_a").then_some(1_758_999_999));
        let v = parse(&out);
        let list = v["portals"][0]["entries"].as_array().unwrap();
        assert_eq!(list.len(), 2, "muted is not hidden");
        assert_eq!(list[0]["muted"], json!(true));
        assert_eq!(list[0]["muted_until_unix"], json!(1_758_999_999));
        assert_eq!(list[1]["muted"], json!(false));
    }

    /// Every optional field is present as `null` rather than absent, so a caller never has to tell a
    /// missing key from an empty one.
    #[test]
    fn an_entry_that_knows_almost_nothing_still_carries_every_key() {
        let pr = PrEntry::stub("https://github.com/o/r/pull/1");
        let out = entries_json(PrAxis::ReviewRequested, &snapshot(vec![(info("github"), Some(vec![pr]))]), 0);
        let e = parse(&out)["portals"][0]["entries"][0].clone();
        for key in ["id", "title", "repo", "number", "author", "updated_at", "bot_review"] {
            assert_eq!(e[key], Value::Null, "{key} should be present and null");
        }
        // `key` falls back to the URL when there is no node id.
        assert_eq!(e["key"], json!("https://github.com/o/r/pull/1"));
        assert_eq!(e["verdicts"], json!([]));
        assert_eq!(e["pending_reviewers"], json!([]));
    }

    /// `0` would claim the data was polled this very second. Before the first poll there is no age,
    /// and saying so is the same honesty the rest of the payload keeps.
    #[test]
    fn the_age_is_absent_rather_than_zero_before_the_first_poll() {
        let never = AxisSnapshot { groups: vec![(info("github"), None)], polled_at: None, version: 0 };
        assert_eq!(parse(&entries_json(PrAxis::ReviewRequested, &never, 0))["polled_age_seconds"], Value::Null);
        let polled = snapshot(vec![(info("github"), None)]);
        assert_eq!(parse(&entries_json(PrAxis::ReviewRequested, &polled, 0))["polled_age_seconds"], json!(47));
    }

    /// The envelope names which axis answered, so a script cannot mix two responses up, and carries
    /// the same version the `ETag` is built from.
    #[test]
    fn the_envelope_names_its_axis_and_the_version_the_etag_is_built_from() {
        for axis in PrAxis::ALL {
            let v = parse(&entries_json(axis, &snapshot(vec![]), 1758499200));
            assert_eq!(v["axis"], json!(axis.slug()));
            assert_eq!(v["version"], json!(12));
            assert_eq!(v["schema"], json!(1));
            assert_eq!(v["generated_at_unix"], json!(1758499200));
        }
    }
}
