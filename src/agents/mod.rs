//! Agents: GitHoot starting a coding agent for each pull request that needs you, on a branch of its own.
//!
//! [`crate::portal`] is where pull requests come from; this is the one place GitHoot does something
//! with them beyond showing them. It creates branches and worktrees and starts agents, so it is off
//! until you switch it on (`agents=on`), and a typo leaves it off, as `localApi` does.
//!
//! It used to be "the Herdr dispatcher", an integration behind a seam meant for more of them. There
//! never was a second integration, and where the agent opens became the choice instead: a client, in
//! [`clients`]. So it is part of GitHoot now, with its own page and flat `agents*` keys, and the seam
//! is gone. The move was a hard cut: nothing reads `integration.herdr.*` or its old folder.
//!
//! - **Pull requests, already judged.** It never polls a forge. It gets the lists the icon and the
//!   pages use, per bar, with muted pull requests taken out ([`batches`]).
//! - **One runner, off the poll thread.** Woken each time a poll publishes, and every thirty seconds
//!   regardless, so a slow `git fetch` never holds up the icon, and switching it on takes effect within
//!   one pass without a restart.
//! - **Its own corner of the disk and of `config.txt`.** `~/.githoot/agents/` for state and prompts,
//!   `agents*` for settings. Only keys it declares can be written from the page.

pub mod clients;
pub mod dispatch;
pub mod journal;
pub mod prompts;

use crate::config::Config;
use crate::portal::types::PrEntry;
use crate::portal::PortalKind;
use crate::scheduler::AxisSnapshot;
use crate::state::PrAxis;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Its settings, beyond the switch, are declared in the app-wide shape. Every one is live: the runner
/// reads `config.txt` fresh on each pass, so the page never says "restart".
pub use crate::setting::{Kind, Setting};

/// What the sidebar, the page and `config.txt` call it.
pub const NAME: &str = "Agents";
/// One plain sentence for the page and for `config.txt`.
pub const SUMMARY: &str = "Starts an agent for each pull request that needs you, on a branch of its own.";
/// The switch: `agents=on`. Default off, for the reason `localApi` is: this acts rather than shows.
pub const SWITCH_KEY: &str = "agents";
/// Every key it owns starts with this, the switch included.
pub const KEY_PREFIX: &str = "agents";
/// Every portal, including ones not written yet: nothing in it names a forge. The bars come from
/// GitHoot, the checkout from the portal's head ref and git, and the agent reads the pull request with
/// whatever tool fits its URL.
pub const PORTALS: &[PortalKind] = &PortalKind::ALL;

/// Where it keeps its files, and its settings, read fresh for this pass.
pub struct Context {
    /// `~/.githoot/agents/`. Not created until something writes into it.
    pub dir: PathBuf,
    settings: BTreeMap<String, String>,
}

impl Context {
    pub fn new(app_asset_path: &Path, cfg: &Config) -> Self {
        Context { dir: app_asset_path.join("agents"), settings: cfg.agents_settings() }
    }

    /// A declared setting by its full key (`agentsClient`), trimmed. Empty when it is not set, which
    /// each setting defines a meaning for.
    pub fn setting(&self, key: &str) -> &str {
        self.settings.get(key).map(|v| v.trim()).unwrap_or("")
    }

    /// A flag's current answer: its default until the file says otherwise. `false` for anything else.
    pub fn flag(&self, setting: &Setting) -> bool {
        crate::setting::flag(setting.kind, self.setting(setting.key))
    }
}

/// One bar's pull requests, as a pass receives them.
pub struct Batch {
    pub axis: PrAxis,
    pub entries: Vec<PrEntry>,
    /// How many were left out because they are muted. Only ever reported, never acted on.
    pub muted: usize,
    /// Whether any portal it may see has answered for this bar. `false` means "not known yet", which
    /// an empty `entries` must never be mistaken for: nothing may be concluded from it.
    pub confirmed: bool,
}

/// Something wrong with one of its settings on this machine, such as a folder that is not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The setting at fault, as declared, so the page can mark that box. Empty for none in particular.
    pub key: &'static str,
    /// One sentence, which reads both on its own and beside the box.
    pub text: String,
}

/// What one pass said, sorted by who needs to hear it.
#[derive(Debug, Default)]
pub struct Said {
    /// What it did. Logged at info level.
    pub said: Vec<String>,
    /// What went wrong with one pull request. Logged as errors, every time.
    pub trouble: Vec<String>,
    /// What is wrong with the setup rather than with any pull request. Logged once until it changes.
    pub setup: Vec<String>,
    /// A client only you can start, missing, once per client: said on the desktop as well, once.
    pub absent: Vec<String>,
    /// What happened, in order, with every program a start ran: for the Agents log. Only a real pass
    /// fills it.
    pub journal: Vec<String>,
}

/// The bars as a pass may see them: only `portals`, only confirmed lists, no muted entries.
///
/// Pure, so the two filters that keep it from acting on something it must not are tested rather than
/// trusted.
pub fn batches(portals: &[PortalKind], snapshots: &[(PrAxis, AxisSnapshot)], is_muted: &dyn Fn(&str) -> bool) -> Vec<Batch> {
    snapshots
        .iter()
        .map(|(axis, snapshot)| {
            let mut batch = Batch { axis: *axis, entries: Vec::new(), muted: 0, confirmed: false };
            let lists: Vec<&Vec<PrEntry>> = snapshot
                .groups
                .iter()
                .filter(|(portal, _)| portals.contains(&portal.kind))
                .filter_map(|(_, list)| list.as_ref())
                .collect();
            batch.confirmed = !lists.is_empty();
            for entry in lists.into_iter().flatten() {
                if is_muted(entry.key()) {
                    batch.muted += 1;
                } else {
                    batch.entries.push(entry.clone());
                }
            }
            batch
        })
        .collect()
}

/// Whether the page may write `key`: the switch, or one of the settings it declared, with a value
/// that stays on one line.
fn check_setting(key: &str, value: &str) -> Result<(), String> {
    if key == SWITCH_KEY {
        return if matches!(value, "on" | "off") { Ok(()) } else { Err(format!("{SWITCH_KEY} is on or off, not {value:?}")) };
    }
    let setting = dispatch::SETTINGS.iter().find(|s| s.key == key).ok_or_else(|| format!("{key:?} is not an agents setting"))?;
    crate::setting::check(setting, value)
}

/// Writes one of its settings, the same single-line edit every other setting gets.
pub fn set(app_asset_path: &Path, key: &str, value: &str) -> Result<(), String> {
    check_setting(key, value)?;
    crate::config::set_agents(app_asset_path, key, value.trim())?;
    if key != SWITCH_KEY {
        let cfg = Config::load(app_asset_path).0;
        dispatch::setting_changed(&Context::new(app_asset_path, &cfg), key, value.trim());
    }
    Ok(())
}

/// Switch on or off.
///
/// Answers with what is wrong with its settings as they stand, so switching on can say so at once
/// rather than leaving it to a log line the first time a pull request arrives. Switched on regardless:
/// the settings are on the same page, and fixing them takes effect on the next pass.
pub fn switch(app_asset_path: &Path, on: bool) -> Result<Vec<Problem>, String> {
    set(app_asset_path, SWITCH_KEY, if on { "on" } else { "off" })?;
    let cfg = Config::load(app_asset_path).0;
    let ctx = Context::new(app_asset_path, &cfg);
    dispatch::switched(&ctx, on);
    journal::append(&ctx.dir, &[format!("switched {} on the page", if on { "on" } else { "off" })]);
    Ok(if on { dispatch::problems(&ctx) } else { Vec::new() })
}

// ── The runner ───────────────────────────────────────────────────────────────

/// The longest the runner sleeps without a poll to wake it. Also how soon switching on takes effect
/// when no poll happens to come first.
const EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Bumped by the poll thread each time it publishes, so the runner wakes on fresh lists rather than
/// on a timer that is usually either early or late.
static PUBLISHED: (std::sync::Mutex<u64>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// Called by the scheduler right after it publishes a poll's lists.
pub fn poll_published() {
    let (count, wake) = &PUBLISHED;
    if let Ok(mut n) = count.lock() {
        *n = n.wrapping_add(1);
        wake.notify_all();
    }
}

/// Sleeps until the next publish or [`EVERY`], whichever comes first.
fn wait_for_poll() {
    let (count, wake) = &PUBLISHED;
    let Ok(guard) = count.lock() else {
        std::thread::sleep(EVERY);
        return;
    };
    let seen = *guard;
    let _ = wake.wait_timeout_while(guard, EVERY, |n| *n == seen);
}

/// What the last dry run said, for the page.
static LAST_DRY_RUN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// One line for the page to show once after a button press.
static FLASH: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The last setup complaint, so an unchanged one is logged once, not every pass.
static LAST_COMPLAINT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub fn last_dry_run() -> Vec<String> {
    LAST_DRY_RUN.lock().map(|v| v.clone()).unwrap_or_default()
}

pub fn flash(line: String) {
    if let Ok(mut f) = FLASH.lock() {
        *f = Some(line);
    }
}

/// The line left for the page, once: a reload does not repeat it.
pub fn take_flash() -> Option<String> {
    FLASH.lock().ok().and_then(|mut f| f.take())
}

/// What to say on the desktop about a client that is not there: the first line of a new absence, or
/// nothing while it is still the same one. `shown` remembers whether this absence was already said.
fn notice_once(shown: &mut bool, absent: &[String]) -> Option<String> {
    let new = absent.first().filter(|_| !*shown).cloned();
    *shown = !absent.is_empty();
    new
}

/// One line for the log: `true` for an error, `false` for info.
type Line = (bool, String);

/// What changed about the setup complaint, if anything. An unchanged one says nothing.
fn complain_once(last: &mut String, said: String) -> Option<Line> {
    let before = std::mem::replace(last, said.clone());
    if before == said {
        None
    } else if !said.is_empty() {
        Some((true, format!("{KEY_PREFIX}: {said}")))
    } else {
        Some((false, format!("{KEY_PREFIX}: everything it needs is back")))
    }
}

/// One real pass, and what it leaves for the log. Takes what the pass found as values, and the pass
/// itself as a closure, so that "said once until it changes" is tested rather than trusted: a wrong
/// clone root once wrote the same line every thirty seconds.
fn run_once(missing: &[&str], pass: impl FnOnce() -> Said, problems: impl FnOnce() -> Vec<Problem>, last: &mut String) -> Vec<Line> {
    let mut lines = Vec::new();
    // One complaint per pass, never two: a "missing nothing" said first would overwrite what the
    // pass said last time, and the pass would then say it again as if it were news.
    if !missing.is_empty() {
        lines.extend(complain_once(last, format!("missing {}", missing.join(", "))));
        return lines;
    }
    let out = pass();
    lines.extend(out.said.into_iter().map(|l| (false, format!("{KEY_PREFIX}: {l}"))));
    lines.extend(out.trouble.into_iter().map(|l| (true, format!("{KEY_PREFIX}: {l}"))));
    // What is wrong before any pull request arrives, as well as what the pass ran into, so a wrong
    // folder is in the log from the first pass and not only once a pull request needs it.
    let mut setup: Vec<String> = problems().into_iter().map(|p| p.text).collect();
    setup.extend(out.setup);
    lines.extend(complain_once(last, setup.join("; ")));
    lines
}

/// The bars as they stand, filtered.
pub fn current_batches() -> Vec<Batch> {
    let now = crate::mute::unix_now();
    let snapshots: Vec<(PrAxis, AxisSnapshot)> =
        PrAxis::ALL.into_iter().map(|axis| (axis, crate::scheduler::pr_snapshot(axis))).collect();
    batches(PORTALS, &snapshots, &|key| crate::mute::until(key, now).is_some())
}

/// A pass with every effect suppressed, kept for the page. Writes no state either, so pressing Dry run
/// can never change what the next real pass does.
pub fn dry_run_now(app_asset_path: &Path) {
    let cfg = Config::load(app_asset_path).0;
    let ctx = Context::new(app_asset_path, &cfg);
    let out = dispatch::pass(&ctx, &current_batches(), true);
    let mut lines = out.said;
    lines.extend(out.trouble);
    lines.extend(out.setup);
    if lines.is_empty() {
        lines.push("nothing to do".to_string());
    }
    if let Ok(mut last) = LAST_DRY_RUN.lock() {
        *last = lines;
    }
}

/// Brings the shipped prompts up to date, then starts the one thread that runs passes while it is on.
///
/// Always started, on or not: it reads `config.txt` on every pass, which is what lets the switch take
/// effect without a restart. Switched off, a pass is one file read.
pub fn start(app_asset_path: PathBuf) {
    let cfg = Config::load(&app_asset_path).0;
    dispatch::prepare(&Context::new(&app_asset_path, &cfg));

    std::thread::Builder::new()
        .name("agents".to_string())
        .spawn(move || {
            // Whether the client now missing was already said on the desktop. This thread's alone.
            let mut shown = false;
            loop {
                wait_for_poll();
                let cfg = Config::load(&app_asset_path).0;
                if !cfg.agents_enabled() {
                    continue;
                }
                let ctx = Context::new(&app_asset_path, &cfg);
                let Ok(mut last) = LAST_COMPLAINT.lock() else { continue };
                let missing = dispatch::missing(&ctx);
                let mut absent = Vec::new();
                let mut journal = Vec::new();
                let pass = || {
                    let mut out = dispatch::pass(&ctx, &current_batches(), false);
                    absent.clone_from(&out.absent);
                    journal = std::mem::take(&mut out.journal);
                    out
                };
                let before = last.clone();
                let lines = run_once(&missing, pass, || dispatch::problems(&ctx), &mut last);
                // The setup complaint, when it changed: what is wrong before any pull request needs it.
                if *last != before {
                    journal.push(if last.is_empty() { "everything it needs is back".to_string() } else { last.clone() });
                }
                journal::append(&ctx.dir, &journal);
                for (error, line) in lines {
                    if error {
                        crate::errorln!("{line}");
                    } else {
                        crate::infoln!("{line}");
                    }
                }
                if let Some(why) = notice_once(&mut shown, &absent)
                    && let Err(e) = crate::dialog::notify("An agent is waiting to start", &why)
                {
                    crate::infoln!("agents: no desktop notification: {e}");
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| crate::errorln!("agents: could not start their thread: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::fake::FakePortal;
    use crate::portal::types::PrEntry;

    fn entry(key: &str) -> PrEntry {
        PrEntry::stub(&format!("https://example.invalid/{key}"))
    }

    fn fake_info(kind: PortalKind) -> crate::portal::PortalInfo {
        let mut info = FakePortal::named(&format!("{kind:?}")).info;
        info.kind = kind;
        info
    }

    fn snapshot(groups: Vec<(crate::portal::PortalInfo, Option<Vec<PrEntry>>)>) -> AxisSnapshot {
        AxisSnapshot { groups, polled_at: None, version: 1 }
    }

    const GITHUB_ONLY: &[PortalKind] = &[PortalKind::GitHub];
    const NO_PORTALS: &[PortalKind] = &[];

    #[test]
    fn it_acts_on_every_portal() {
        assert_eq!(PORTALS, &PortalKind::ALL);
    }

    #[test]
    fn a_pass_sees_only_the_portals_it_is_given() {
        let github = fake_info(PortalKind::GitHub);
        let snaps = vec![(PrAxis::ReviewRequested, snapshot(vec![(github, Some(vec![entry("a")]))]))];
        let seen = batches(GITHUB_ONLY, &snaps, &|_| false);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].entries.len(), 1);
        assert!(batches(NO_PORTALS, &snaps, &|_| false).iter().all(|b| b.entries.is_empty()));
    }

    /// A GitLab list does not confirm anything for a pass that cannot see GitLab, and none of its merge
    /// requests reach it.
    #[test]
    fn a_portal_it_is_not_given_neither_reaches_it_nor_confirms_anything() {
        let snaps = vec![(
            PrAxis::ReviewRequested,
            snapshot(vec![
                (fake_info(PortalKind::GitLab), Some(vec![entry("mr")])),
                (fake_info(PortalKind::GitHub), Some(vec![entry("pr")])),
            ]),
        )];
        let seen = batches(GITHUB_ONLY, &snaps, &|_| false);
        let keys: Vec<&str> = seen[0].entries.iter().map(|e| e.key()).collect();
        assert_eq!(keys, ["https://example.invalid/pr"]);

        let gitlab_only = vec![(PrAxis::ReviewRequested, snapshot(vec![(fake_info(PortalKind::GitLab), Some(Vec::new()))]))];
        assert!(!batches(GITHUB_ONLY, &gitlab_only, &|_| false)[0].confirmed);
    }

    /// A muted pull request is one you asked to stop hearing about. Starting an agent for it would be
    /// the loudest possible way to ignore that.
    #[test]
    fn muted_pull_requests_never_reach_a_pass() {
        let github = fake_info(PortalKind::GitHub);
        let a = entry("a");
        let muted_key = a.key().to_string();
        let snaps = vec![(PrAxis::ChangesRequested, snapshot(vec![(github, Some(vec![a, entry("b")]))]))];
        let seen = batches(GITHUB_ONLY, &snaps, &|key| key == muted_key);
        assert_eq!(seen[0].entries.len(), 1);
        assert_ne!(seen[0].entries[0].key(), muted_key);
        assert_eq!(seen[0].muted, 1);
    }

    /// An unconfirmed list is "nobody could ask", not "nothing there". Neither is something to act on.
    #[test]
    fn an_unconfirmed_list_is_not_handed_over() {
        let snaps = vec![(PrAxis::ReadyToMerge, snapshot(vec![(fake_info(PortalKind::GitHub), None)]))];
        let seen = batches(GITHUB_ONLY, &snaps, &|_| false);
        assert!(seen.iter().all(|b| b.entries.is_empty()));
        // And it says so: an empty list nobody confirmed must never pass for a quiet bar.
        assert!(seen.iter().all(|b| !b.confirmed));
    }

    #[test]
    fn a_confirmed_list_is_marked_confirmed_even_when_it_is_empty() {
        let snaps = vec![(PrAxis::ReadyToMerge, snapshot(vec![(fake_info(PortalKind::GitHub), Some(Vec::new()))]))];
        assert!(batches(GITHUB_ONLY, &snaps, &|_| false)[0].confirmed);
        assert!(!batches(NO_PORTALS, &snaps, &|_| false)[0].confirmed, "a portal it cannot see confirms nothing");
    }

    /// Switching on, and on again after off, is "from now on": every bar's baseline is forgotten, so
    /// nothing already waiting gets an agent.
    #[test]
    fn switching_on_or_off_forgets_every_bars_baseline() {
        let dir = std::env::temp_dir().join(format!("githoot-agents-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        for on in [true, false] {
            for axis in PrAxis::ALL {
                std::fs::write(dir.join("agents").join(format!("{}.armed", axis.slug())), "").unwrap();
            }
            switch(&dir, on).unwrap();
            assert_eq!(Config::load(&dir).0.agents_enabled(), on);
            for axis in PrAxis::ALL {
                assert!(!dir.join("agents").join(format!("{}.armed", axis.slug())).exists(), "{on} {axis:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same setup trouble on every pass is said once, not every thirty seconds. Having every tool
    /// it needs is not news either, so it must not make the next pass forget what it already said.
    #[test]
    fn the_same_setup_trouble_is_logged_once_not_every_pass() {
        let stuck = || vec![Problem { key: "agentsCloneRoot", text: "no clone at /nowhere/thing".to_string() }];
        let mut last = String::new();
        let first = run_once(&[], Said::default, stuck, &mut last);
        assert_eq!(first, [(true, "agents: no clone at /nowhere/thing".to_string())]);
        for _ in 0..3 {
            assert_eq!(run_once(&[], Said::default, stuck, &mut last), [], "said again");
        }
        assert_eq!(run_once(&[], Said::default, Vec::new, &mut last), [(false, "agents: everything it needs is back".to_string())]);
        let missing = run_once(&["git"], || panic!("no pass while a tool is missing"), Vec::new, &mut last);
        assert_eq!(missing, [(true, "agents: missing git".to_string())]);
    }

    /// A client you have to start yourself is said on the desktop once when it goes missing, not every
    /// pass while it stays missing; once it is back, the next time it goes missing is news again.
    #[test]
    fn a_missing_client_is_notified_once_per_absence() {
        let mut shown = false;
        let gone = ["Orca is not running. Open Orca.".to_string()];
        assert_eq!(notice_once(&mut shown, &gone), Some("Orca is not running. Open Orca.".to_string()));
        assert_eq!(notice_once(&mut shown, &gone), None, "still missing, already said");
        assert_eq!(notice_once(&mut shown, &[]), None, "back: nothing to say");
        assert_eq!(notice_once(&mut shown, &gone), Some("Orca is not running. Open Orca.".to_string()), "missing again is news");
    }

    /// A flag nobody set is its default, and only an explicit value moves it. A typo leaves the default
    /// standing, in either direction, as every other switch in `config.txt` does.
    #[test]
    fn a_flag_reads_its_default_until_it_is_set() {
        let flag = |text: &str, key: &str| {
            let cfg = Config::from_text(text);
            let ctx = Context::new(Path::new("/x"), &cfg);
            ctx.flag(dispatch::SETTINGS.iter().find(|s| s.key == key).unwrap())
        };
        assert!(flag("", "agentsWorkRequired") && !flag("", "agentsApproved"));
        assert!(!flag("agentsWorkRequired=off\n", "agentsWorkRequired") && flag("agentsApproved=on\n", "agentsApproved"));
        assert!(flag("agentsWorkRequired=offf\n", "agentsWorkRequired") && !flag("agentsApproved=onn\n", "agentsApproved"));
    }

    #[test]
    fn only_declared_settings_can_be_written() {
        assert!(check_setting("agents", "on").is_ok());
        assert!(check_setting("agentsCloneRoot", "/home/me/src").is_ok());
        assert!(check_setting("localApi", "on").is_err(), "not its key");
        assert!(check_setting("agentsCloneRoot\nlocalApi", "on").is_err());
        assert!(check_setting("agentsCloneRoot", "/x\nlocalApi=on").is_err(), "a value must stay on one line");
        assert!(check_setting("agents", "maybe").is_err());
        assert!(check_setting("agentsApproved", "on").is_ok() && check_setting("agentsApproved", "off").is_ok());
        assert!(check_setting("agentsApproved", "yes please").is_err(), "a flag is on or off");
        assert!(check_setting("agentsKind", "claude").is_ok() && check_setting("agentsKind", "codex").is_err(), "Claude only, for now");
        assert!(check_setting("agentsClient", "orca").is_ok() && check_setting("agentsClient", "tmux").is_err());
    }
}
