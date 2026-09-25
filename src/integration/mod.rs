//! Integrations: what GitHoot may do with the pull requests it has found, beyond showing them.
//!
//! [`crate::portal`] is where pull requests come from. This is where they can go. Today there is
//! one integration, the Herdr dispatcher, and this module exists so that is a fact about the
//! contents of the directory and not about the shape of the program: the core (`config`, `page`,
//! `serve`, `main`) knows a list of integrations and the [`Integration`] trait below, and nothing
//! about any one of them. A second integration is a new directory beside `herdr`, not a set of edits
//! across the crate.
//!
//! ## Installed means switched on
//!
//! Every integration ships inside the binary. Installing one writes `integration.<id>.enabled=on`
//! to `config.txt`, and uninstalling it writes `off`; its prompts, state and settings stay on disk, so
//! installing it again picks up where it left off. There is no plugin loading, deliberately: the
//! dispatcher moved *into* the process in 2.4.0 because an external program was the problem (a
//! console window on Windows, a script host antivirus flags, bash and PowerShell drifting apart).
//! Scripts of your own already have the local API.
//!
//! ## What the seam carries
//!
//! - **Pull requests, already judged.** An integration never polls a forge. It gets the lists the
//!   icon and the pages use, per bar, with muted pull requests taken out and only from the portal
//!   kinds it says it understands. The Herdr dispatcher shells out to `gh`, so it declares GitHub,
//!   and a GitLab pull request never reaches it.
//! - **One runner, off the poll thread.** Woken each time a poll publishes, and every thirty seconds
//!   regardless, so a slow `git fetch` never holds up the icon, and turning an integration on takes
//!   effect within one pass without a restart.
//! - **Its own corner of the disk and of `config.txt`.** `~/.githoot/integrations/<id>/` for state,
//!   `integration.<id>.*` for settings. Only keys it declares can be written from the page.
//! - **Its own page.** The Integrations tab lists them; each has a page with the generic part (on or
//!   off, what it is missing, its settings, a dry run) and whatever it adds below.

pub mod herdr;

#[cfg(test)]
mod fake;

use crate::config::Config;
use crate::portal::types::PrEntry;
use crate::portal::PortalKind;
use crate::scheduler::AxisSnapshot;
use crate::state::PrAxis;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A setting an integration declares, beyond `enabled`.
///
/// Every one is live: the runner reads `config.txt` fresh on each pass, so the page never has to
/// say "restart to apply".
pub struct Setting {
    /// The key after `integration.<id>.`, so `cloneRoot` for `integration.herdr.cloneRoot`.
    pub key: &'static str,
    /// What the page's input or checkbox is labelled.
    pub label: &'static str,
    pub kind: Kind,
    /// One sentence for `config.txt`, above the key.
    pub help: &'static str,
}

/// What a setting holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Free text on one line. Empty means the setting's default, which `placeholder` shows greyed.
    Text { placeholder: &'static str },
    /// `on` or `off`, a checkbox on the page. Only an explicit value moves it off its default, so a
    /// typo leaves the default standing in either direction.
    Flag { default_on: bool },
}

/// What the rest of the app needs to know about an integration without running it.
pub struct Info {
    /// The config namespace, the URL segment and the directory name. Lowercase ASCII letters only,
    /// asserted by a test, because it is all three.
    pub id: &'static str,
    /// "Herdr dispatcher". What the tab and the page call it.
    pub name: &'static str,
    /// One plain sentence for the list and for `config.txt`.
    pub summary: &'static str,
    /// The portal kinds whose pull requests it can act on. Everything else is filtered out before
    /// it is called.
    pub portals: &'static [PortalKind],
    pub settings: &'static [Setting],
    /// `Some(reason)` where this build cannot run it. The page says the reason and offers no Install.
    pub unsupported: Option<&'static str>,
}

/// Where an integration keeps its files, and the settings it declared, read fresh for this pass.
pub struct Context {
    /// `~/.githoot/integrations/<id>/`. Not created until something writes into it.
    pub dir: PathBuf,
    settings: BTreeMap<String, String>,
}

impl Context {
    pub fn new(app_asset_path: &Path, cfg: &Config, id: &str) -> Self {
        Context { dir: app_asset_path.join("integrations").join(id), settings: cfg.integration_settings(id) }
    }

    /// A declared setting, trimmed. Empty when it is not set, which each setting defines a meaning for.
    pub fn setting(&self, key: &str) -> &str {
        self.settings.get(key).map(|v| v.trim()).unwrap_or("")
    }

    /// A flag's current answer: its default until the file says otherwise. `false` for a text setting.
    pub fn flag(&self, setting: &Setting) -> bool {
        let value = self.setting(setting.key);
        match setting.kind {
            Kind::Flag { default_on: true } => !crate::config::is_off(value),
            Kind::Flag { default_on: false } => crate::config::is_on(value),
            Kind::Text { .. } => false,
        }
    }
}

/// One bar's pull requests, as an integration receives them.
pub struct Batch {
    pub axis: PrAxis,
    pub entries: Vec<PrEntry>,
    /// How many were left out because they are muted. Only ever reported, never acted on.
    pub muted: usize,
    /// Whether any portal it may see has answered for this bar. `false` means "not known yet", which
    /// an empty `entries` must never be mistaken for: nothing may be concluded from it.
    pub confirmed: bool,
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
}

pub trait Integration: Send + Sync {
    fn info(&self) -> &'static Info;

    /// Once at startup, installed or not: bring shipped files such as default prompts up to date.
    fn prepare(&self, _ctx: &Context) {}

    /// Install or Uninstall was pressed. Called after `config.txt` says so.
    fn installed(&self, _ctx: &Context, _on: bool) {}

    /// One of its settings was written from the page, with the value it now has. Saving the form
    /// writes every setting on it, so this is called for values that did not change as well.
    fn setting_changed(&self, _ctx: &Context, _key: &str, _value: &str) {}

    /// Tools it needs that will not run. Empty when it has everything.
    fn missing(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// One pass over the bars. `dry_run` touches nothing, state files included, and says what it would do.
    fn pass(&self, ctx: &Context, batches: &[Batch], dry_run: bool) -> Said;

    /// HTML for its page, below the generic card. Everything interpolated must go through `page::esc`.
    fn page(&self, _ctx: &Context, _token: &str) -> String {
        String::new()
    }

    /// A form it rendered in [`Integration::page`] was posted. `None` for an action it does not know,
    /// which the server answers with 400. `Ok` carries the line the page shows afterwards.
    fn action(&self, _ctx: &Context, _action: &str, _form: &crate::serve::Form) -> Option<Result<String, String>> {
        None
    }
}

/// Every integration this build knows, installed or not, in the order the tab lists them.
pub fn all() -> &'static [&'static dyn Integration] {
    &[&herdr::Herdr]
}

pub fn find(id: &str) -> Option<&'static dyn Integration> {
    all().iter().copied().find(|i| i.info().id == id)
}

/// Whether `id` is installed and can run here.
pub fn enabled(cfg: &Config, integration: &dyn Integration) -> bool {
    integration.info().unsupported.is_none() && cfg.integration_enabled(integration.info().id)
}

/// The bars as `info` may see them: only portals it declared, only confirmed lists, no muted entries.
///
/// Pure, so the two filters that keep an integration from acting on something it must not are
/// tested rather than trusted.
pub fn batches(info: &Info, snapshots: &[(PrAxis, AxisSnapshot)], is_muted: &dyn Fn(&str) -> bool) -> Vec<Batch> {
    snapshots
        .iter()
        .map(|(axis, snapshot)| {
            let mut batch = Batch { axis: *axis, entries: Vec::new(), muted: 0, confirmed: false };
            let lists: Vec<&Vec<PrEntry>> = snapshot
                .groups
                .iter()
                .filter(|(portal, _)| info.portals.contains(&portal.kind))
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

/// Whether the page may write `key` for `integration`: `enabled`, or one of the settings it declared,
/// with a value that stays on one line.
fn check_setting(integration: &dyn Integration, key: &str, value: &str) -> Result<(), String> {
    let info = integration.info();
    if value.contains(['\n', '\r']) {
        return Err(format!("{}: a setting must stay on one line", info.id));
    }
    let is_flag = match (key, info.settings.iter().find(|s| s.key == key)) {
        ("enabled", _) => true,
        (_, Some(setting)) => matches!(setting.kind, Kind::Flag { .. }),
        (_, None) => return Err(format!("{} has no setting {key:?}", info.id)),
    };
    if is_flag && !matches!(value, "on" | "off") {
        return Err(format!("{}: {key} is on or off, not {value:?}", info.id));
    }
    Ok(())
}

/// What a submitted settings form says for each declared setting, in declaration order.
///
/// A checkbox posts nothing when unticked, which is how a form says off, so every flag gets an answer.
/// A text box absent from the form is left alone; one submitted empty is a deliberate clear.
pub fn form_values(info: &Info, form: &crate::serve::Form) -> Vec<(&'static str, String)> {
    info.settings
        .iter()
        .filter_map(|s| match s.kind {
            Kind::Flag { .. } => Some((s.key, if form.ticked(s.key) { "on" } else { "off" }.to_string())),
            Kind::Text { .. } => form.get(s.key).map(|v| (s.key, v.trim().to_string())),
        })
        .collect()
}

/// Writes one of an integration's settings, the same single-line edit every other setting gets.
pub fn set(app_asset_path: &Path, integration: &dyn Integration, key: &str, value: &str) -> Result<(), String> {
    check_setting(integration, key, value)?;
    crate::config::set_integration(app_asset_path, integration.info().id, key, value.trim())?;
    if key != "enabled" {
        let cfg = Config::load(app_asset_path).0;
        integration.setting_changed(&Context::new(app_asset_path, &cfg, integration.info().id), key, value.trim());
    }
    Ok(())
}

/// Install or uninstall. Refused where the build cannot run it, so the page cannot switch on something
/// that would sit there doing nothing.
pub fn install(app_asset_path: &Path, integration: &dyn Integration, on: bool) -> Result<(), String> {
    if let (true, Some(why)) = (on, integration.info().unsupported) {
        return Err(why.to_string());
    }
    set(app_asset_path, integration, "enabled", if on { "on" } else { "off" })?;
    let cfg = Config::load(app_asset_path).0;
    integration.installed(&Context::new(app_asset_path, &cfg, integration.info().id), on);
    Ok(())
}

// ── The runner ───────────────────────────────────────────────────────────────

/// The longest the runner sleeps without a poll to wake it. Also how soon an Install takes effect
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

/// What the last dry run of each integration said, for its page.
static LAST_DRY_RUN: std::sync::Mutex<BTreeMap<&'static str, Vec<String>>> = std::sync::Mutex::new(BTreeMap::new());

/// One line per integration for the page to show once after a button press.
static FLASH: std::sync::Mutex<BTreeMap<&'static str, String>> = std::sync::Mutex::new(BTreeMap::new());

/// The last setup complaint per integration, so an unchanged one is logged once, not every pass.
static LAST_COMPLAINT: std::sync::Mutex<BTreeMap<&'static str, String>> = std::sync::Mutex::new(BTreeMap::new());

pub fn last_dry_run(id: &str) -> Vec<String> {
    LAST_DRY_RUN.lock().ok().and_then(|m| m.get(id).cloned()).unwrap_or_default()
}

pub fn flash(integration: &dyn Integration, line: String) {
    if let Ok(mut m) = FLASH.lock() {
        m.insert(integration.info().id, line);
    }
}

/// The line left for this page, once: a reload does not repeat it.
pub fn take_flash(id: &str) -> Option<String> {
    FLASH.lock().ok().and_then(|mut m| m.remove(id))
}

fn complain_once(id: &'static str, said: String) {
    let Ok(mut last) = LAST_COMPLAINT.lock() else { return };
    let before = last.get(id).cloned().unwrap_or_default();
    if before == said {
        return;
    }
    if !said.is_empty() {
        crate::errorln!("{id}: {said}");
    } else if !before.is_empty() {
        crate::infoln!("{id}: everything it needs is back");
    }
    last.insert(id, said);
}

/// The bars as they stand, filtered for `integration`.
fn current_batches(integration: &dyn Integration) -> Vec<Batch> {
    let now = crate::mute::unix_now();
    let snapshots: Vec<(PrAxis, AxisSnapshot)> =
        PrAxis::ALL.into_iter().map(|axis| (axis, crate::scheduler::pr_snapshot(axis))).collect();
    batches(integration.info(), &snapshots, &|key| crate::mute::until(key, now).is_some())
}

/// A pass with every effect suppressed, kept for the integration's page. Writes no state either, so
/// pressing Dry run can never change what the next real pass does.
pub fn dry_run_now(app_asset_path: &Path, integration: &'static dyn Integration) {
    let cfg = Config::load(app_asset_path).0;
    let ctx = Context::new(app_asset_path, &cfg, integration.info().id);
    let out = integration.pass(&ctx, &current_batches(integration), true);
    let mut lines = out.said;
    lines.extend(out.trouble);
    lines.extend(out.setup);
    if lines.is_empty() {
        lines.push("nothing to do".to_string());
    }
    if let Ok(mut m) = LAST_DRY_RUN.lock() {
        m.insert(integration.info().id, lines);
    }
}

/// Prepares every integration, then starts the one thread that runs the installed ones.
///
/// Always started, installed or not: it reads `config.txt` on every pass, which is what lets Install
/// take effect without a restart. With nothing installed a pass is one file read.
pub fn start(app_asset_path: PathBuf) {
    let cfg = Config::load(&app_asset_path).0;
    for integration in all() {
        integration.prepare(&Context::new(&app_asset_path, &cfg, integration.info().id));
    }

    std::thread::Builder::new()
        .name("integrations".to_string())
        .spawn(move || loop {
            wait_for_poll();
            let cfg = Config::load(&app_asset_path).0;
            for integration in all() {
                if !enabled(&cfg, *integration) {
                    continue;
                }
                let id = integration.info().id;
                let missing = integration.missing();
                complain_once(id, if missing.is_empty() { String::new() } else { format!("missing {}", missing.join(", ")) });
                if !missing.is_empty() {
                    continue;
                }
                let ctx = Context::new(&app_asset_path, &cfg, id);
                let out = integration.pass(&ctx, &current_batches(*integration), false);
                for line in out.said {
                    crate::infoln!("{id}: {line}");
                }
                for problem in out.trouble {
                    crate::errorln!("{id}: {problem}");
                }
                complain_once(id, out.setup.join("; "));
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| crate::errorln!("integrations: could not start their thread: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::fake::FakePortal;
    use crate::portal::types::PrEntry;

    fn entry(key: &str) -> PrEntry {
        PrEntry::stub(&format!("https://example.invalid/{key}"))
    }

    /// Only GitHub exists as a kind today, so "a portal it did not declare" is tested from the other
    /// side: an integration that declares none sees nothing.
    fn fake_info(_kind: PortalKind) -> crate::portal::PortalInfo {
        FakePortal::named("GitHub").info
    }

    fn snapshot(groups: Vec<(crate::portal::PortalInfo, Option<Vec<PrEntry>>)>) -> AxisSnapshot {
        AxisSnapshot { groups, polled_at: None, version: 1 }
    }

    const GITHUB_ONLY: Info =
        Info { id: "test", name: "Test", summary: "", portals: &[PortalKind::GitHub], settings: &[], unsupported: None };
    const NO_PORTALS: Info =
        Info { id: "test", name: "Test", summary: "", portals: &[], settings: &[], unsupported: None };

    /// The id is a config namespace, a URL segment and a directory name at once, so anything but
    /// lowercase letters is a way to escape one of the three.
    #[test]
    fn every_id_is_unique_and_safe_as_a_key_a_path_and_a_url() {
        let ids: Vec<&str> = all().iter().map(|i| i.info().id).collect();
        for id in &ids {
            assert!(!id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase()), "{id:?}");
            assert_eq!(ids.iter().filter(|other| *other == id).count(), 1, "{id:?} twice");
            assert!(find(id).is_some());
        }
        assert!(find("nope").is_none() && find("").is_none() && find("../herdr").is_none());
    }

    #[test]
    fn the_herdr_dispatcher_is_registered_and_acts_on_github_only() {
        let herdr = find("herdr").expect("herdr must be registered");
        assert_eq!(herdr.info().portals, &[PortalKind::GitHub]);
    }

    #[test]
    fn an_integration_sees_only_the_portals_it_declared() {
        let github = fake_info(PortalKind::GitHub);
        let snaps = vec![(PrAxis::ReviewRequested, snapshot(vec![(github, Some(vec![entry("a")]))]))];
        let seen = batches(&GITHUB_ONLY, &snaps, &|_| false);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].entries.len(), 1);
        assert!(batches(&NO_PORTALS, &snaps, &|_| false).iter().all(|b| b.entries.is_empty()));
    }

    /// A muted pull request is one you asked to stop hearing about. Starting an agent for it would be
    /// the loudest possible way to ignore that.
    #[test]
    fn muted_pull_requests_never_reach_an_integration() {
        let github = fake_info(PortalKind::GitHub);
        let a = entry("a");
        let muted_key = a.key().to_string();
        let snaps = vec![(PrAxis::ChangesRequested, snapshot(vec![(github, Some(vec![a, entry("b")]))]))];
        let seen = batches(&GITHUB_ONLY, &snaps, &|key| key == muted_key);
        assert_eq!(seen[0].entries.len(), 1);
        assert_ne!(seen[0].entries[0].key(), muted_key);
        assert_eq!(seen[0].muted, 1);
    }

    /// An unconfirmed list is "nobody could ask", not "nothing there". Neither is something to act on.
    #[test]
    fn an_unconfirmed_list_is_not_handed_over() {
        let snaps = vec![(PrAxis::ReadyToMerge, snapshot(vec![(fake_info(PortalKind::GitHub), None)]))];
        let seen = batches(&GITHUB_ONLY, &snaps, &|_| false);
        assert!(seen.iter().all(|b| b.entries.is_empty()));
        // And it says so: an empty list nobody confirmed must never pass for a quiet bar.
        assert!(seen.iter().all(|b| !b.confirmed));
    }

    #[test]
    fn a_confirmed_list_is_marked_confirmed_even_when_it_is_empty() {
        let snaps = vec![(PrAxis::ReadyToMerge, snapshot(vec![(fake_info(PortalKind::GitHub), Some(Vec::new()))]))];
        assert!(batches(&GITHUB_ONLY, &snaps, &|_| false)[0].confirmed);
        assert!(!batches(&NO_PORTALS, &snaps, &|_| false)[0].confirmed, "a portal it cannot see confirms nothing");
    }

    /// Install and Uninstall tell the integration, so it can treat the next pass as a fresh start.
    #[test]
    fn install_and_uninstall_tell_the_integration() {
        let dir = std::env::temp_dir().join(format!("githoot-integration-install-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let before = fake::INSTALLS.load(std::sync::atomic::Ordering::SeqCst);
        install(&dir, &fake::Echo, true).unwrap();
        install(&dir, &fake::Echo, false).unwrap();
        assert_eq!(fake::INSTALLS.load(std::sync::atomic::Ordering::SeqCst), before + 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    static FLAGGED: Info = Info {
        id: "test",
        name: "Test",
        summary: "",
        portals: &[PortalKind::GitHub],
        settings: &[
            Setting { key: "loud", label: "Loud", kind: Kind::Flag { default_on: true }, help: "" },
            Setting { key: "quiet", label: "Quiet", kind: Kind::Flag { default_on: false }, help: "" },
            Setting { key: "root", label: "Root", kind: Kind::Text { placeholder: "~" }, help: "" },
        ],
        unsupported: None,
    };

    /// A flag nobody set is its default, and only an explicit value moves it. A typo leaves the
    /// default standing, in either direction, as every other switch in `config.txt` does.
    #[test]
    fn a_flag_reads_its_default_until_it_is_set() {
        let flag = |text: &str, key: &str| {
            let cfg = Config::from_text(text);
            let ctx = Context::new(Path::new("/x"), &cfg, "test");
            ctx.flag(FLAGGED.settings.iter().find(|s| s.key == key).unwrap())
        };
        assert!(flag("", "loud") && !flag("", "quiet"));
        assert!(!flag("integration.test.loud=off\n", "loud") && flag("integration.test.quiet=on\n", "quiet"));
        assert!(flag("integration.test.loud=offf\n", "loud") && !flag("integration.test.quiet=onn\n", "quiet"));
    }

    /// A checkbox posts nothing when unticked, which is how a form says off. Reading absence as
    /// "leave it" would make a flag impossible to switch off from the page. A text box absent from
    /// the form is left alone; one submitted empty is a deliberate clear.
    #[test]
    fn a_settings_form_turns_unticked_boxes_into_off() {
        let form = crate::serve::parse_form("loud=on&root=%2Fsrc");
        assert_eq!(form_values(&FLAGGED, &form), [("loud", "on".to_string()), ("quiet", "off".to_string()), ("root", "/src".to_string())]);
        let form = crate::serve::parse_form("quiet=on");
        assert_eq!(form_values(&FLAGGED, &form), [("loud", "off".to_string()), ("quiet", "on".to_string())]);
    }

    #[test]
    fn only_declared_settings_can_be_written() {
        let herdr = find("herdr").unwrap();
        assert!(check_setting(herdr, "enabled", "on").is_ok());
        assert!(check_setting(herdr, "cloneRoot", "/home/me/src").is_ok());
        assert!(check_setting(herdr, "localApi", "on").is_err(), "not its key");
        assert!(check_setting(herdr, "cloneRoot\nlocalApi", "on").is_err());
        assert!(check_setting(herdr, "cloneRoot", "/x\nlocalApi=on").is_err(), "a value must stay on one line");
        assert!(check_setting(herdr, "enabled", "maybe").is_err());
        assert!(check_setting(herdr, "approved", "on").is_ok() && check_setting(herdr, "approved", "off").is_ok());
        assert!(check_setting(herdr, "approved", "yes please").is_err(), "a flag is on or off");
    }
}
