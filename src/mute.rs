//! Muted pull requests: `~/.githoot/muted.txt`.
//!
//! A mute takes one pull request out of its bar for a fixed time, 3, 7 or 30 days, chosen from a link
//! on the PR page. While muted it does not light the bar, does not count, does not hoot, and the
//! shipped dispatcher leaves it alone; the page lists it in a "Muted" section at the bottom and the
//! local API still serves it, marked `muted: true`, so nothing is hidden from a caller that asks.
//!
//! **On disk, unlike the hoot ledger**, because a mute measured in days has to outlive the restarts a
//! self-update or a restart-required setting causes several times a week. The file is GitHoot's own,
//! written whole by the page's mute links and nothing else, one `key<TAB>until_unix` per line.
//! Expired lines are dropped whenever it is read or written, so it never grows past the pull
//! requests muted right now.
//!
//! **Unmuting counts as a new pull request.** The poll loop drops a muted key from its hoot ledger
//! (see `PollState::apply_pr_with`), so when the mute ends, by the link or by time, the pull request
//! arrives as if for the first time: the bar lights and the owl hoots. A mute is a snooze, and a
//! snooze that ended silently would be a way to lose a review.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE: &str = "muted.txt";

/// The only durations a mute link offers. The server refuses anything else, so a hand-made post
/// cannot mute for a year.
pub const DAYS: [u64; 3] = [3, 7, 30];

const DAY: u64 = 24 * 60 * 60;

struct Store {
    path: PathBuf,
    until: HashMap<String, u64>,
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
    STORE.lock().ok()?.as_ref()?.until.get(key).copied().filter(|&t| t > now)
}

/// Every live mute, soonest to end first. For the muted page, which has to reach a muted pull
/// request even when its bar is empty and the bar's menu entry is therefore hidden.
pub fn all(now: u64) -> Vec<(String, u64)> {
    let Ok(guard) = STORE.lock() else { return Vec::new() };
    let mut list: Vec<_> = guard
        .as_ref()
        .map(|s| s.until.iter().filter(|(_, t)| **t > now).map(|(k, &t)| (k.clone(), t)).collect())
        .unwrap_or_default();
    list.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// Mutes `key` for `days`, or unmutes it when `days` is `0`, and writes the file.
///
/// `Err` is the sentence to log. The caller has already checked `days` against [`DAYS`] and `key`
/// against the pull requests actually on the page.
pub fn set(key: &str, days: u64, now: u64) -> Result<(), String> {
    if key.is_empty() || key.contains(['\t', '\n', '\r']) {
        return Err("not a pull request key".to_string());
    }
    let mut guard = STORE.lock().map_err(|_| "mute lock poisoned".to_string())?;
    let store = guard.as_mut().ok_or("mutes are not available")?;
    store.until.retain(|_, &mut t| t > now);
    if days == 0 {
        store.until.remove(key);
    } else {
        store.until.insert(key.to_string(), now + days * DAY);
    }
    std::fs::write(&store.path, render(&store.until)).map_err(|e| format!("could not write {FILE}: {e}"))
}

/// The file's lines, minus anything expired or malformed. A line this version cannot read costs that
/// one mute and never the rest of the file, the same tolerance `config.txt` gets.
pub fn parse(text: &str, now: u64) -> HashMap<String, u64> {
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(k, t)| Some((k.trim().to_string(), t.trim().parse::<u64>().ok()?)))
        .filter(|(k, t)| !k.is_empty() && *t > now)
        .collect()
}

/// Sorted, so the file is stable to read and diff.
pub fn render(until: &HashMap<String, u64>) -> String {
    let mut lines: Vec<_> = until.iter().map(|(k, t)| format!("{k}\t{t}\n")).collect();
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
        assert_eq!(m.get("PR_a"), Some(&(NOW + 60)));
    }

    #[test]
    fn rendering_round_trips_through_parsing() {
        let m: HashMap<_, _> = [("PR_b".to_string(), NOW + 5), ("PR_a".to_string(), NOW + 9)].into();
        let text = render(&m);
        assert_eq!(text, format!("PR_a\t{}\nPR_b\t{}\n", NOW + 9, NOW + 5), "sorted");
        assert_eq!(parse(&text, NOW), m);
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
