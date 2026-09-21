//! One icon, one tooltip and one set of menu labels out of several portals' states.
//!
//! `PollState` is one portal's state machine and knows nothing about the others. This module is the
//! only place that looks at more than one at a time, and it is pure: no I/O, no clock, so every merge
//! rule below is a unit test.
//!
//! Every rule is the identity for a single portal. That is not a coincidence, it is the acceptance
//! test: a GitHub-only install must produce byte-for-byte what it produced before this module
//! existed, and the goldens in `state` and here are what hold it to that.
//!
//! The one piece of app-level state that lives here rather than in any portal is the available
//! update. A newer GitHoot release says nothing about any portal, so it is an input to the overview
//! and not a field on a `PollState`.

use crate::state::{IconState, PollState, PrAxis, Presence, UPDATE_MENU_LABEL};

/// One portal's state, with the name its lines should carry when there is more than one.
pub struct PortalView<'a> {
    pub name: &'a str,
    pub state: &'a PollState,
}

/// The icon, merged.
///
/// A dot is lit if any portal lights it: work waiting anywhere is work waiting. An axis nobody can
/// answer is `Unknown` only when no portal answers `Yes` for it, because a confirmed "yes, here" is
/// an answer even while another portal is down. The exclamation flags (`needs_auth`,
/// `status_degraded`, `signals_unsure`) are "any", for the same reason a single failing axis raises
/// it today.
pub fn icon(views: &[PortalView], update_available: bool) -> IconState {
    let icons: Vec<IconState> = views.iter().map(|v| v.state.icon()).collect();
    IconState {
        needs_auth: icons.iter().any(|i| i.needs_auth),
        status_degraded: icons.iter().any(|i| i.status_degraded),
        signals_unsure: icons.iter().any(|i| i.signals_unsure),
        update_available,
        review_requested: merge_presence(icons.iter().map(|i| i.review_requested)),
        ready_to_merge: merge_presence(icons.iter().map(|i| i.ready_to_merge)),
        changes_requested: merge_presence(icons.iter().map(|i| i.changes_requested)),
    }
}

/// `Yes` beats `Unknown` beats `No`. With nothing to merge (no portals at all) the answer is a
/// confirmed `No`: there is nothing configured to ask with, which is the same reading an
/// unconfigured axis gets inside `PollState`.
fn merge_presence(presences: impl Iterator<Item = Presence>) -> Presence {
    let mut merged = Presence::No;
    for p in presences {
        match p {
            Presence::Yes => return Presence::Yes,
            Presence::Unknown => merged = Presence::Unknown,
            Presence::No => {}
        }
    }
    merged
}

/// The hover text.
///
/// One portal reads exactly as `PollState::tooltip` always has, with the update line in the place it
/// always had: after the state lines and before the one error detail. Several portals prefix every
/// line with the portal's name so "3 PR(s) approved" says where; a line that already names its
/// portal (the outage line does) is left alone. The length cap is applied once, at the end, because
/// Windows truncates the whole tooltip, not each portal's share of it.
pub fn tooltip(views: &[PortalView], update_available: Option<&str>) -> String {
    let several = views.len() > 1;
    let mut lines = Vec::new();
    for view in views {
        for line in view.state.tooltip_lines() {
            lines.push(prefixed(several, view.name, line));
        }
    }
    if let Some(version) = update_available {
        lines.push(format!("Update available: {version}"));
    }
    // At most one reason, the first portal's, in portal order. The tooltip is 128 UTF-16 units on
    // Windows and a second error string would not survive truncation anyway.
    if let Some((view, detail)) = views.iter().find_map(|v| v.state.detail().map(|d| (v, d))) {
        lines.push(prefixed(several, view.name, detail));
    }
    PollState::cap_tooltip(lines.join("\n"))
}

fn prefixed(several: bool, name: &str, line: String) -> String {
    if several && !line.starts_with(&format!("{name}: ")) {
        format!("{name}: {line}")
    } else {
        line
    }
}

/// Text for `axis`'s menu item: the base label plus the confirmed count summed across portals, or
/// the bare label when that sum is zero. Only `Presence::Yes` tracks contribute, for the reason
/// `PollState::pr_menu_label` gives: a stale number in a menu asserts something no longer known.
pub fn pr_menu_label(views: &[PortalView], axis: PrAxis) -> String {
    let base = axis.menu_label();
    let total: u32 = views.iter().filter_map(|v| v.state.confirmed_count(axis)).sum();
    if total > 0 {
        format!("{base} ({total})")
    } else {
        base.to_string()
    }
}

/// Text for the install-update entry, carrying the version. `None` hides the entry.
pub fn update_menu_label(version: Option<&str>) -> Option<String> {
    version.map(|v| format!("{UPDATE_MENU_LABEL}: {v}"))
}

/// Which axes have news this cycle, across portals. An arrival anywhere is news for that axis, and
/// the caller still plays one hoot per cycle however many axes or portals turned over.
pub fn arrivals(per_portal: &[[bool; 3]]) -> [bool; 3] {
    let mut merged = [false; 3];
    for arrivals in per_portal {
        for (slot, &arrived) in merged.iter_mut().zip(arrivals) {
            *slot |= arrived;
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::types::{PollResponse, PollResult, PrEntry};

    fn respond(result: PollResult) -> PollResponse {
        PollResponse { result, poll_interval: None }
    }

    fn fresh_count(n: u32) -> PollResponse {
        respond(PollResult::Fresh { present: n > 0, count: Some(n), prs: None })
    }

    fn fresh_urls(urls: &[&str]) -> PollResponse {
        respond(PollResult::Fresh {
            present: !urls.is_empty(),
            count: Some(urls.len() as u32),
            prs: Some(urls.iter().map(|u| PrEntry::stub(u)).collect()),
        })
    }

    fn transient() -> PollResponse {
        respond(PollResult::Transient("network down".to_string()))
    }

    fn view<'a>(name: &'a str, state: &'a PollState) -> PortalView<'a> {
        PortalView { name, state }
    }

    // ─── One portal is the identity ─────────────────────────────────────────

    #[test]
    fn one_portal_produces_exactly_the_states_own_icon() {
        let mut state = PollState::for_portal("GitHub", [true; 3]);
        state.set_status_degraded(Some("Actions down".to_string()));
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        state.apply_pr(PrAxis::ReadyToMerge, transient());
        state.apply_pr(PrAxis::ChangesRequested, fresh_count(0));
        let merged = icon(&[view("GitHub", &state)], false);
        let own = state.icon();
        assert_eq!(merged.needs_auth, own.needs_auth);
        assert_eq!(merged.status_degraded, own.status_degraded);
        assert_eq!(merged.signals_unsure, own.signals_unsure);
        assert_eq!(merged.review_requested, own.review_requested);
        assert_eq!(merged.ready_to_merge, own.ready_to_merge);
        assert_eq!(merged.changes_requested, own.changes_requested);
        assert!(!merged.update_available);
        assert!(icon(&[view("GitHub", &state)], true).update_available);
    }

    #[test]
    fn one_portal_produces_exactly_the_states_own_tooltip_and_labels() {
        let mut state = PollState::for_portal("GitHub", [true; 3]);
        state.set_status_degraded(Some("Actions down".to_string()));
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        state.apply_pr(PrAxis::ReadyToMerge, fresh_count(0));
        state.apply_pr(PrAxis::ChangesRequested, transient());
        let views = [view("GitHub", &state)];
        assert_eq!(tooltip(&views, None), state.tooltip());
        for axis in PrAxis::ALL {
            assert_eq!(pr_menu_label(&views, axis), state.pr_menu_label(axis));
        }
        assert_eq!(pr_menu_label(&views, PrAxis::ReviewRequested), "Open Requested Reviews (3)");
    }

    /// Golden, moved here from `state` with the update line: the update is app-level now, and this
    /// is where it joins the tooltip. Same text, same position, before the one error detail.
    #[test]
    fn golden_tooltip_with_an_update_waiting_keeps_its_place() {
        let mut state = PollState::for_portal("GitHub", [true, false, false]);
        state.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        let views = [view("GitHub", &state)];
        assert_eq!(tooltip(&views, Some("2.9.0")), "2 PR(s) awaiting your review\nUpdate available: 2.9.0");
        assert_eq!(update_menu_label(Some("2.9.0")).as_deref(), Some("Install update: 2.9.0"));
        assert_eq!(update_menu_label(None), None);

        state.apply_pr(PrAxis::ReviewRequested, transient());
        let views = [view("GitHub", &state)];
        assert_eq!(
            tooltip(&views, Some("2.9.0")),
            "2 PR(s) awaiting your review\nUpdate available: 2.9.0\nnetwork down",
            "the update line sits between the state lines and the error detail, as it always did"
        );
    }

    #[test]
    fn no_portals_at_all_reads_as_a_confirmed_nothing() {
        let merged = icon(&[], false);
        assert_eq!(merged.review_requested, Presence::No);
        assert!(!merged.needs_auth && !merged.signals_unsure && !merged.status_degraded);
        assert_eq!(tooltip(&[], None), "");
        assert_eq!(pr_menu_label(&[], PrAxis::ReadyToMerge), "Open Approved PRs");
    }

    // ─── Several portals ───────────────────────────────────────────────────

    #[test]
    fn presence_merges_yes_over_unknown_over_no() {
        let mut yes = PollState::for_portal("A", [true; 3]);
        yes.apply_pr(PrAxis::ReviewRequested, fresh_count(1));
        yes.apply_pr(PrAxis::ReadyToMerge, fresh_count(0));
        yes.apply_pr(PrAxis::ChangesRequested, fresh_count(0));
        let mut unknown = PollState::for_portal("B", [true; 3]);
        for _ in 0..3 {
            unknown.apply_pr(PrAxis::ReviewRequested, transient());
            unknown.apply_pr(PrAxis::ReadyToMerge, transient());
        }
        unknown.apply_pr(PrAxis::ChangesRequested, fresh_count(0));
        let merged = icon(&[view("A", &yes), view("B", &unknown)], false);
        assert_eq!(merged.review_requested, Presence::Yes, "a yes anywhere is a yes");
        assert_eq!(merged.ready_to_merge, Presence::Unknown, "no yes, one unknown: unknown");
        assert_eq!(merged.changes_requested, Presence::No, "everyone confirmed no");
        assert!(merged.signals_unsure, "B is failing, and that shows even though A is fine");
    }

    #[test]
    fn needs_auth_and_outage_are_any_portal() {
        let mut a = PollState::for_portal("A", [true, false, false]);
        a.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        let mut b = PollState::for_portal("B", [true, false, false]);
        b.require_pr_auth();
        b.set_status_degraded(Some("Wobbly".to_string()));
        let merged = icon(&[view("A", &a), view("B", &b)], false);
        assert!(merged.needs_auth);
        assert!(merged.status_degraded);
    }

    #[test]
    fn counts_sum_across_portals_and_only_confirmed_ones_count() {
        let mut a = PollState::for_portal("A", [true; 3]);
        a.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        a.apply_pr(PrAxis::ReadyToMerge, fresh_count(4));
        let mut b = PollState::for_portal("B", [true; 3]);
        b.apply_pr(PrAxis::ReviewRequested, fresh_count(3));
        // B once knew 9 approved, then lost track: a stale number must not be summed in.
        b.apply_pr(PrAxis::ReadyToMerge, fresh_count(9));
        for _ in 0..3 {
            b.apply_pr(PrAxis::ReadyToMerge, transient());
        }
        let views = [view("A", &a), view("B", &b)];
        assert_eq!(pr_menu_label(&views, PrAxis::ReviewRequested), "Open Requested Reviews (5)");
        assert_eq!(pr_menu_label(&views, PrAxis::ReadyToMerge), "Open Approved PRs (4)");
        assert_eq!(pr_menu_label(&views, PrAxis::ChangesRequested), "Open Work Required");
    }

    #[test]
    fn several_portals_name_their_lines_and_are_capped_once() {
        let mut a = PollState::for_portal("GitHub", [true, false, false]);
        a.set_status_degraded(Some("Actions down".to_string()));
        a.apply_pr(PrAxis::ReviewRequested, fresh_count(2));
        let mut b = PollState::for_portal("GitLab", [true, false, false]);
        b.apply_pr(PrAxis::ReviewRequested, fresh_count(0));
        let views = [view("GitHub", &a), view("GitLab", &b)];
        assert_eq!(
            tooltip(&views, None),
            "GitHub: Actions down\nGitHub: 2 PR(s) awaiting your review\nGitLab: No reviews requested",
            "the outage line already names its portal and is not prefixed twice"
        );

        let mut c = PollState::for_portal("Bitbucket", [true; 3]);
        c.apply_pr(PrAxis::ReviewRequested, fresh_count(11));
        c.apply_pr(PrAxis::ReadyToMerge, fresh_count(12));
        c.apply_pr(PrAxis::ChangesRequested, fresh_count(13));
        let long = tooltip(&[view("GitHub", &a), view("GitLab", &b), view("Bitbucket", &c)], Some("9.9.9"));
        assert!(long.ends_with('…'), "got {long:?}");
        assert!(long.chars().count() <= 111, "one cap, applied to the whole text");
    }

    #[test]
    fn arrivals_are_or_ed_and_ledgers_stay_apart() {
        assert_eq!(arrivals(&[]), [false; 3]);
        assert_eq!(arrivals(&[[true, false, false], [false, false, true]]), [true, false, true]);

        // The same URL on two portals is two pull requests, because each state keeps its own
        // ledger. Both hoot, and neither silences the other.
        let mut a = PollState::for_portal("A", [true, false, false]);
        let mut b = PollState::for_portal("B", [true, false, false]);
        a.apply_pr(PrAxis::ReviewRequested, fresh_urls(&["https://x.example/pull/1"]));
        b.apply_pr(PrAxis::ReviewRequested, fresh_urls(&["https://x.example/pull/1"]));
        let merged = arrivals(&[a.take_pr_arrivals(), b.take_pr_arrivals()]);
        assert_eq!(merged, [true, false, false]);
        assert_eq!(a.ledger_len(PrAxis::ReviewRequested), 1);
        assert_eq!(b.ledger_len(PrAxis::ReviewRequested), 1);
    }
}
