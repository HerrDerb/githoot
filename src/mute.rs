//! Muted pull requests: `~/.githoot/muted.txt`.
//!
//! A mute takes one pull request out of its bar for a fixed time, 3, 7 or 30 days, chosen from a link
//! on the PR page. While muted it does not light the bar, does not count, does not hoot, and the
//! shipped dispatcher leaves it alone; the page lists it in a "Muted" section at the bottom and the
//! local API still serves it, marked `muted: true`, so nothing is hidden from a caller that asks.
//!
//! **On disk, unlike the hoot ledger**, because a mute measured in days has to outlive the restarts a
//! self-update or a restart-required setting causes several times a week. The file is GitHoot's own,
//! written whole by the page's mute links and the poll loop and nothing else, one
//! `key<TAB>until<TAB>gone_since<TAB>portal<TAB>url<TAB>title` per line. Only the first two are
//! required: a line written by an older version still loads. Expired lines are dropped whenever it is
//! read or written, so it never grows past the pull requests muted right now.
//!
//! **A mute outlives its pull request's place in the bars.** It keeps the title and link it had when
//! it was muted, so the muted page can still say what it is after a merge. Once a poll of its portal
//! no longer finds it anywhere it is marked gone, and seven days later it is dropped, whatever is
//! left of the mute: there is nothing left to snooze.
//!
//! **Unmuting counts as a new pull request.** The poll loop drops a muted key from its hoot ledger
//! (see `PollState::apply_pr_with`), so when the mute ends, by the link or by time, the pull request
//! arrives as if for the first time: the bar lights and the owl hoots. A mute is a snooze, and a
//! snooze that ended silently would be a way to lose a review.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE: &str = "muted.txt";

/// The only durations a mute link offers. The server refuses anything else, so a hand-made post
/// cannot mute for a year.
pub const DAYS: [u64; 3] = [3, 7, 30];

const DAY: u64 = 24 * 60 * 60;

/// How long a mute is kept after its pull request left every bar.
pub const GONE_DAYS: u64 = 7;

/// What a mute link knows about the pull request it mutes.
pub struct About {
    pub portal: String,
    pub url: String,
    pub title: Option<String>,
}

/// One mute. Everything but `until` is `None` on a line an older version wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mute {
    pub until: u64,
    /// When a poll of its portal first found it in no bar. Cleared if it comes back.
    pub gone_since: Option<u64>,
    pub portal: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
}

impl Mute {
    pub fn new(until: u64, about: Option<About>) -> Self {
        let (portal, url, title) = match about {
            Some(a) => (Some(a.portal), Some(a.url), a.title),
            None => (None, None, None),
        };
        Mute { until, gone_since: None, portal, url, title }
    }

    /// When a gone mute is dropped. `None` while its pull request is still in a bar.
    pub fn removed_at(&self) -> Option<u64> {
        self.gone_since.map(|g| g + GONE_DAYS * DAY)
    }

    /// Still muted at `now`: neither run out nor gone for a week.
    pub fn live(&self, now: u64) -> bool {
        self.until > now && self.removed_at().is_none_or(|at| at > now)
    }
}

struct Store {
    path: PathBuf,
    until: HashMap<String, Mute>,
}

/// Shared by the poll thread, which reads it every cycle, and the loopback server, which writes it
/// on a click. Unset in tests, where nothing is muted.
static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Reads the file, or starts empty. Called once from `main`.
pub fn init(app_asset_path: &Path) {
    let path = app_asset_path.join(FILE);
    let until = std::fs::read_to_string(&path).map(|t| parse(&t, unix_now())).unwrap_or_default();
    *STORE.lock().expect("mute lock poisoned") = Some(Store { path, until });
}

/// When `key`'s mute ends, or `None` when it is not muted (including when its mute has expired).
pub fn until(key: &str, now: u64) -> Option<u64> {
    STORE.lock().ok()?.as_ref()?.until.get(key).filter(|m| m.live(now)).map(|m| m.until)
}

/// Every live mute, soonest to end first. For the muted page, which has to reach a muted pull
/// request even when its bar is empty and the bar's menu entry is therefore hidden.
pub fn all(now: u64) -> Vec<(String, Mute)> {
    let Ok(guard) = STORE.lock() else { return Vec::new() };
    let mut list: Vec<_> = guard
        .as_ref()
        .map(|s| s.until.iter().filter(|(_, m)| m.live(now)).map(|(k, m)| (k.clone(), m.clone())).collect())
        .unwrap_or_default();
    list.sort_by(|a, b| a.1.until.cmp(&b.1.until).then_with(|| a.0.cmp(&b.0)));
    list
}

/// Mutes `key` for `days`, or unmutes it when `days` is `0`, and writes the file.
///
/// `Err` is the sentence to log. The caller has already checked `days` against [`DAYS`] and `key`
/// against the pull requests actually on the page, which is where `about` comes from.
pub fn set(key: &str, days: u64, now: u64, about: Option<About>) -> Result<(), String> {
    if key.is_empty() || key.contains(['\t', '\n', '\r']) {
        return Err("not a pull request key".to_string());
    }
    let mut guard = STORE.lock().map_err(|_| "mute lock poisoned".to_string())?;
    let store = guard.as_mut().ok_or("mutes are not available")?;
    store.until.retain(|_, m| m.live(now));
    if days == 0 {
        store.until.remove(key);
    } else {
        store.until.insert(key.to_string(), Mute::new(now + days * DAY, about));
    }
    std::fs::write(&store.path, render(&store.until)).map_err(|e| format!("could not write {FILE}: {e}"))
}

/// Tells the mutes what one poll of `portal` found: the keys of every pull request in any of its
/// bars, muted ones included. Writes the file only when a mark changed.
///
/// The caller passes only a complete answer, every bar of the portal confirmed. A failed poll is not
/// evidence that anything left.
pub fn observe(portal: &str, present: &HashSet<String>, now: u64) {
    let Ok(mut guard) = STORE.lock() else { return };
    let Some(store) = guard.as_mut() else { return };
    store.until.retain(|_, m| m.live(now));
    if observe_in(&mut store.until, portal, present, now)
        && let Err(e) = std::fs::write(&store.path, render(&store.until))
    {
        crate::errorln!("mute: could not write {FILE}: {e}");
    }
}

/// `observe` without the file. `true` when a mark changed.
fn observe_in(mutes: &mut HashMap<String, Mute>, portal: &str, present: &HashSet<String>, now: u64) -> bool {
    let mut changed = false;
    for (key, m) in mutes.iter_mut().filter(|(_, m)| m.portal.as_deref() == Some(portal)) {
        let gone_since = if present.contains(key) { None } else { m.gone_since.or(Some(now)) };
        if gone_since != m.gone_since {
            m.gone_since = gone_since;
            changed = true;
        }
    }
    changed
}

/// The file's lines, minus anything expired or malformed. A line this version cannot read costs that
/// one mute and never the rest of the file, the same tolerance `config.txt` gets.
pub fn parse(text: &str, now: u64) -> HashMap<String, Mute> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let key = f.next()?.trim().to_string();
            let until = f.next()?.trim().parse::<u64>().ok()?;
            let gone_since = f.next().and_then(|g| g.trim().parse::<u64>().ok());
            let mut text = || f.next().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
            let (portal, url, title) = (text(), text(), text());
            Some((key, Mute { until, gone_since, portal, url, title }))
        })
        .filter(|(k, m)| !k.is_empty() && m.live(now))
        .collect()
}

/// Sorted, so the file is stable to read and diff. A tab or a line break inside a field would split
/// the line, so they become spaces; titles are the only field that can hold them.
pub fn render(mutes: &HashMap<String, Mute>) -> String {
    let field = |v: &Option<String>| v.as_deref().unwrap_or("").replace(['\t', '\n', '\r'], " ");
    let mut lines: Vec<_> = mutes
        .iter()
        .map(|(k, m)| {
            let gone = m.gone_since.map(|g| g.to_string()).unwrap_or_default();
            format!("{k}\t{}\t{gone}\t{}\t{}\t{}\n", m.until, field(&m.portal), field(&m.url), field(&m.title))
        })
        .collect();
    lines.sort();
    lines.concat()
}

/// How long is left, as a mute link says it: "3 days", "5 hours", "less than an hour".
pub fn remaining(until: u64, now: u64) -> String {
    let left = until.saturating_sub(now);
    let plural = |n: u64, unit: &str| if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") };
    if left >= DAY {
        plural(left.div_ceil(DAY), "day")
    } else if left >= 3600 {
        plural(left.div_ceil(3600), "hour")
    } else {
        "less than an hour".to_string()
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_758_499_200;

    /// Expired and malformed lines fall out on read; the rest survive.
    #[test]
    fn parsing_keeps_live_mutes_and_drops_the_rest() {
        let text = format!("PR_a\t{}\nPR_b\t{}\nnot a line\nPR_c\tsoon\n\t{}\n", NOW + 60, NOW - 1, NOW + 60);
        let m = parse(&text, NOW);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m.get("PR_a").map(|m| m.until), Some(NOW + 60));
    }

    #[test]
    fn rendering_round_trips_through_parsing() {
        let m: HashMap<_, _> =
            [("PR_b".to_string(), Mute::new(NOW + 5, None)), ("PR_a".to_string(), Mute::new(NOW + 9, Some(about("github", "a"))))].into();
        let text = render(&m);
        assert!(text.starts_with("PR_a\t") && text.contains("\nPR_b\t"), "sorted: {text}");
        assert_eq!(parse(&text, NOW), m);
    }

    fn about(portal: &str, title: &str) -> About {
        About { portal: portal.to_string(), url: "https://github.com/o/r/pull/1".to_string(), title: Some(title.to_string()) }
    }

    /// A mute remembers what it named, so the muted page can still show it once no bar holds it. A
    /// line from an older version, key and time only, still loads.
    #[test]
    fn a_mute_keeps_its_title_link_and_portal_and_old_lines_still_load() {
        let mut m = HashMap::new();
        m.insert("PR_a".to_string(), Mute::new(NOW + 60, Some(about("github", "Fix\tthe\nowl"))));
        let text = format!("{}PR_old\t{}\n", render(&m), NOW + 60);
        let read = parse(&text, NOW);

        let a = &read["PR_a"];
        assert_eq!(a.until, NOW + 60);
        assert_eq!(a.portal.as_deref(), Some("github"));
        assert_eq!(a.url.as_deref(), Some("https://github.com/o/r/pull/1"));
        assert_eq!(a.title.as_deref(), Some("Fix the owl"), "a tab or newline in a title cannot break the line");
        assert_eq!(a.gone_since, None);

        let old = &read["PR_old"];
        assert_eq!((old.until, old.portal.as_deref(), old.title.as_deref()), (NOW + 60, None, None));
    }

    /// A mute whose pull request left every bar of its portal is marked gone once, and the mark does
    /// not move on later polls, so the seven days count from when it left. Coming back clears it. Other
    /// portals' mutes, and old lines that name no portal, are not this poll's to judge.
    #[test]
    fn a_poll_marks_a_vanished_mute_gone_and_a_returning_one_back() {
        let mut m = HashMap::new();
        m.insert("PR_here".to_string(), Mute::new(NOW + 9 * DAY, Some(about("github", "here"))));
        m.insert("PR_gone".to_string(), Mute::new(NOW + 9 * DAY, Some(about("github", "gone"))));
        m.insert("MR_other".to_string(), Mute::new(NOW + 9 * DAY, Some(about("gitlab", "other"))));
        m.insert("PR_legacy".to_string(), Mute::new(NOW + 9 * DAY, None));
        let present: HashSet<String> = ["PR_here".to_string()].into();

        assert!(observe_in(&mut m, "github", &present, NOW), "a change is reported, so the file is written");
        assert_eq!(m["PR_gone"].gone_since, Some(NOW));
        assert_eq!(m["PR_here"].gone_since, None);
        assert_eq!(m["MR_other"].gone_since, None, "another portal's poll says nothing about it");
        assert_eq!(m["PR_legacy"].gone_since, None, "a mute with no portal cannot be judged");

        assert!(!observe_in(&mut m, "github", &present, NOW + DAY), "nothing changed, nothing to write");
        assert_eq!(m["PR_gone"].gone_since, Some(NOW), "the clock does not restart");

        let back: HashSet<String> = ["PR_here".to_string(), "PR_gone".to_string()].into();
        assert!(observe_in(&mut m, "github", &back, NOW + 2 * DAY));
        assert_eq!(m["PR_gone"].gone_since, None, "it came back: still muted, no longer gone");
    }

    /// Seven days after it left, a gone mute is dropped, even with days of mute left.
    #[test]
    fn a_gone_mute_is_dropped_seven_days_after_it_left() {
        let mut gone = Mute::new(NOW + 30 * DAY, Some(about("github", "gone")));
        gone.gone_since = Some(NOW);
        assert!(gone.live(NOW + GONE_DAYS * DAY - 1));
        assert!(!gone.live(NOW + GONE_DAYS * DAY), "seven days after it left");
        assert_eq!(gone.removed_at(), Some(NOW + GONE_DAYS * DAY));

        let m: HashMap<_, _> = [("PR_gone".to_string(), gone)].into();
        assert!(parse(&render(&m), NOW + GONE_DAYS * DAY).is_empty(), "and it is not read back either");
        assert_eq!(parse(&render(&m), NOW).len(), 1);
    }

    #[test]
    fn remaining_time_reads_the_way_a_person_says_it() {
        assert_eq!(remaining(NOW + 3 * DAY, NOW), "3 days");
        assert_eq!(remaining(NOW + DAY + 1, NOW), "2 days", "rounded up: never 'back in 1 day' with 25h left");
        assert_eq!(remaining(NOW + DAY, NOW), "1 day");
        assert_eq!(remaining(NOW + 5 * 3600, NOW), "5 hours");
        assert_eq!(remaining(NOW + 60, NOW), "less than an hour");
    }
}
