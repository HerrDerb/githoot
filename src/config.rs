//! User-editable settings: `~/.githoot/config.txt`.
//!
//! One `key=value` per line, `#` comments and blank lines ignored — the same tolerant shape
//! `access_token.rs` already uses for `client_id.txt`, so there is nothing new to explain to
//! someone who has already edited that file. Missing file, missing key, or an unrecognised value
//! are all just "use the default": a config file is a convenience, not something startup should
//! ever fail over.
//!
//! The file is **written on first run** with every setting at its default, so the settings are
//! discoverable by opening it rather than only by reading the README. An existing file is never
//! rewritten wholesale — not even to add a key it is missing.
//!
//! The one thing that edits an existing file is the tray's Hoot toggle, through [`set_sound`], and it
//! changes exactly one value line: comments, blank lines, spacing and keys this version has never
//! heard of all survive it byte for byte. That is the same promise stated the other way round — the
//! file belongs to the user, and the app may change a value in it but never its shape.

use crate::log::Level;
use crate::portal::{PortalId, PortalKind};
use crate::{errorln, infoln};
use crate::state::PrAxis;
use std::path::Path;

const CONFIG_FILE: &str = "config.txt";

// ── Keys ──────────────────────────────────────────────────────────────────────
// Named constants rather than inline strings, because each appears three times: in the lookup, in
// the generated template, and in the round-trip test that proves those two agree.

const KEY_UPDATE_CHECK: &str = "updateCheck";
const KEY_REVIEW_REQUESTED: &str = "reviewRequested";
const KEY_READY_TO_MERGE: &str = "readyToMerge";
const KEY_CHANGES_REQUESTED: &str = "changesRequested";
const KEY_LOG_LEVEL: &str = "logLevel";
const KEY_SOUND: &str = "sound";
const KEY_STATUS_COMPONENTS: &str = "statusComponents";
const KEY_COPILOT_REVIEWS: &str = "copilotReviews";
const KEY_LOCAL_API: &str = "localApi";
const KEY_DISPATCHER: &str = "dispatcher";
const KEY_CLONE_ROOT: &str = "dispatcherCloneRoot";
const KEY_WORKTREE_ROOT: &str = "dispatcherWorktreeRoot";

/// Every component GitHub publishes on its status page, in the order the page lists them.
///
/// Named in the *comment* above `statusComponents` in a fresh `config.txt`, not in its value — see
/// `DEFAULT_STATUS_COMPONENTS`. Listing them all there is what keeps adding one back an edit rather
/// than a research task against GitHub's status page.
///
/// Hardcoding it means it can go stale, which is why the ignored
/// `every_component_named_in_the_default_config_still_exists` test in `github_status` checks it
/// against the live payload; a stale entry costs nothing worse than an unwatched component and a
/// line in the log.
///
/// One name is deliberately absent: `Visit www.githubstatus.com for more information`, a Statuspage
/// placeholder rather than a service, and never anything but operational.
const KNOWN_STATUS_COMPONENTS: [&str; 11] = [
    "Git Operations",
    "Webhooks",
    "API Requests",
    "Issues",
    "Pull Requests",
    "Actions",
    "Packages",
    "Pages",
    "Copilot",
    "Codespaces",
    "Copilot AI Model Providers",
];

/// What a fresh `config.txt` actually watches: the parts of GitHub a pull-request tray touches.
///
/// The shipped value used to be all eleven, and that was wrong in a way that took a while to see.
/// GitHub's page-wide verdict is one judgement over everything it runs, and a component being
/// degraded is enough to make it read "Partially Degraded Service" — so Copilot having a bad
/// afternoon put a red exclamation on a tray that never calls Copilot. Cry wolf often enough and the
/// mark stops meaning anything, which costs more than the outage it was reporting.
///
/// The three left out (`Copilot`, `Codespaces`, `Copilot AI Model Providers`) are still named in the
/// comment beside the key, so putting one back is an edit and not a research task.
///
/// **Existing files are not rewritten**, so this changes nothing for anyone already installed: a
/// `config.txt` with no `statusComponents` line keeps watching the whole page, which is what it has
/// always meant. Only a fresh file gets the narrower watch.
const DEFAULT_STATUS_COMPONENTS: [&str; 8] = [
    "Git Operations",
    "Webhooks",
    "API Requests",
    "Issues",
    "Pull Requests",
    "Actions",
    "Packages",
    "Pages",
];

/// The component list a fresh `config.txt` is written with.
///
/// Test-only: the shipped file gets the list from `DEFAULT_STATUS_COMPONENTS` directly. This exists
/// so the live test in `github_status` can hold names up against what GitHub actually publishes.
#[cfg(test)]
pub fn default_status_components() -> Vec<String> {
    DEFAULT_STATUS_COMPONENTS.iter().map(|name| name.to_string()).collect()
}

/// Every component GitHub publishes, for the same live check.
#[cfg(test)]
pub fn all_status_components() -> Vec<String> {
    KNOWN_STATUS_COMPONENTS.iter().map(|name| name.to_string()).collect()
}

/// Keys that used to work, paired with what replaced them.
///
/// These are **not** aliases. The old spelling is not read, deliberately — that is the clean break.
/// They are listed only so `Config::load` can say so out loud, because the alternative is a feature
/// quietly changing behaviour on upgrade: an old `update_check=off` would stop being read and the
/// updater someone deliberately disabled would switch back on.
///
/// `notifications` → `notificationIndication` used to be the other entry. Both spellings are gone
/// now: the feature they named was removed in 2.0.0, so there is no replacement to point at and an
/// old line is simply an unknown key, which `parse` has always ignored.
const RENAMED_KEYS: [(&str, &str); 1] = [("update_check", KEY_UPDATE_CHECK)];

/// Whether [`Config::load`] found no `config.txt` and successfully wrote one.
///
/// The app's one reliable "we have never met this user before" signal, and the whole reason `load`
/// returns anything besides a `Config`. Used by `autostart` to decide whether to ask about starting at
/// sign-in — a question that must be asked exactly once, on a genuinely fresh install.
///
/// A named enum rather than a `bool`, because a returned `(Config, bool)` reads as nothing at the call
/// site and an inverted test would be invisible.
///
/// **`Yes` requires the write to have succeeded.** A home directory that cannot be written leaves no
/// file, so the next start would look like a first run too, and the one thing worse than never asking a
/// question is asking it on every single launch. Only a question whose answer can be remembered is
/// worth asking.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum FirstRun {
    Yes,
    No,
}

/// Settings read from `config.txt`.
/// One portal as the settings describe it. See [`Config::portals`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PortalConfig {
    /// The section name, `github` for the implicit portal. Never shown; see `PortalInfo`.
    pub id: PortalId,
    pub kind: PortalKind,
    /// Origin with no trailing slash. The adapter derives every endpoint from it.
    pub base_url: String,
}

/// The id of the portal every existing install has without a line in the file to say so.
const IMPLICIT_PORTAL: &str = "github";

pub struct Config {
    /// Whether to check GitHub for newer releases once a day. On unless explicitly turned off.
    pub update_check: bool,
    /// Whether each PR signal is wanted, indexed by `PrAxis::index`.
    ///
    /// Private, and read only through [`Config::pr_enabled`]. A public `[bool; 3]` invites a caller
    /// to build or destructure it in declaration order rather than axis order, and transposing two
    /// entries silently swaps which bar a setting controls — it compiles, type-checks, and is
    /// visible only by looking at the tray.
    pr_enabled: [bool; 3],
    /// Whether a rising PR count plays the hoot. On unless explicitly turned off — see `sound`.
    ///
    /// Only the *sound* is switched off. The icon, the tooltip and the menu counts are untouched, so
    /// silencing this loses nothing but the noise, which is why it needs no more than one flag.
    pub sound: bool,
    /// How much detail the log file carries. `Error` by default — only failures — so the file stays
    /// quiet and readable; set `logLevel=info` to add the lifecycle narration when diagnosing.
    pub log_level: Level,
    /// Which parts of GitHub may raise the outage mark, as the user spelled them.
    ///
    /// **Empty means the whole page**, which is both the old behaviour and the only sane reading of an
    /// absent key: `config.txt` is never rewritten, so no file written before this key existed will
    /// ever grow it, and treating absence as "watch nothing" would silently retire the feature for
    /// every existing install. A fresh file is written with every component named, so the two paths
    /// agree on the day of writing and diverge only as GitHub adds components.
    ///
    /// Kept as the user typed it, not folded or canonicalised: it is what the log quotes back when a
    /// name matches nothing. Folding happens at match time, in `github_status`.
    pub status_components: Vec<String>,
    /// Whether unresolved comments from GitHub's automatic reviewer count as work.
    ///
    /// Copilot reviews by commenting and never by a verdict, so without this its feedback is
    /// invisible to the amber bar. On by default; some teams treat its comments as suggestions rather
    /// than work, and for them this is noise.
    pub copilot_reviews: bool,
    /// Whether the judged lists are served as JSON to scripts running as you on this machine.
    ///
    /// The only default-off setting in this file, and the only one that changes when the loopback
    /// listener binds: on, it binds at startup instead of on the first menu click, and writes
    /// `endpoint.json` so a script can find the ephemeral port and this run's token. Off, the route
    /// answers 404 and nothing is written, which is exactly how the app behaved before it existed.
    pub local_api: bool,
    /// Whether GitHoot starts a Herdr agent for each pull request that needs one.
    ///
    /// **The only setting that makes GitHoot do something rather than show something.** On, a
    /// thread creates branches and worktrees and launches agents under your own `gh`; off, GitHoot
    /// reads GitHub and draws an icon, which is everything it did before this existed. Off by
    /// default and left off by a typo, for the same reason `localApi` is.
    pub dispatcher: bool,
    /// Where your clones live, one directory per repository name. Empty means `~/projects`.
    ///
    /// A setting rather than an environment variable, and that is not a style choice: GitHoot is
    /// started from a tray, a shortcut or autostart, none of which carry a shell's environment. A
    /// knob you can only set by launching the app a particular way is a knob most people cannot
    /// reach, and the dispatcher silently finds no clones for all of them.
    pub clone_root: String,
    /// Where the dispatcher's per-pull-request worktrees go. Empty means `~/worktrees`.
    pub worktree_root: String,
}

/// Where the settings file lives.
///
/// Public because the tray's Settings entry opens this exact path and then watches it for edits
/// (`settings_watch`). One definition, so the menu can never open a file the app does not read.
pub fn config_path(app_asset_path: &Path) -> std::path::PathBuf {
    app_asset_path.join(CONFIG_FILE)
}

/// Rewrites the hoot setting in `config.txt`, leaving every other byte of the file alone.
///
/// The counterpart of the menu's Hoot toggle. Nothing else writes a setting: the toggle is the only
/// place in the app that changes a value the user owns, which is why this takes a single named
/// setting rather than a whole `Config` — a "write the config back" function would rewrite the file
/// from the parsed values and silently discard every comment, blank line and unread key in it.
/// A setting the tray menu can change while the app is running, shared with the poll loop.
///
/// A setting used to be a plain `bool` read out of `config.txt` at startup and copied into the poll
/// loop, which was fine while a text editor was the only way to change one — editing the file already
/// meant restarting to apply it. A checkbox does not: unticking a box and then still hearing the next
/// hoot is a broken switch, whatever the log says. The same goes for any other box added beside it.
///
/// A shared flag rather than a channel, because there is nothing to deliver — the loop does not need
/// to *react* to the change, only to read the current answer at the one moment it matters.
/// A message would add a queue, an ordering question and a wake-up for a value that is one bit wide.
///
/// `Relaxed` is the right ordering for exactly that reason: nothing else is published alongside this
/// flag, so there is nothing for it to order. The worst a race can do is act on the old answer once,
/// in the same instant the user clicked — indistinguishable from clicking a moment later.
///
/// Lives here rather than in `sound`, where it started, because it is a *setting* that can change at
/// runtime and nothing about it is about audio. The hoot was simply the first box on the menu.
#[derive(Clone)]
pub struct Switch(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Switch {
    pub fn new(on: bool) -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(on)))
    }

    pub fn is_on(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set(&self, on: bool) {
        self.0.store(on, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn set_sound(app_asset_path: &Path, on: bool) -> Result<(), String> {
    set_flag(&config_path(app_asset_path), KEY_SOUND, on)
}

/// Every key a user can change, paired with whether it takes effect without a restart.
///
/// Public because the settings page renders from it: one list, so a key cannot appear in the form
/// without the page knowing whether to warn about it, and cannot be added to the file without
/// appearing in the form.
pub const WRITABLE_KEYS: [(&str, bool); 12] = [
    (KEY_REVIEW_REQUESTED, false),
    (KEY_READY_TO_MERGE, false),
    (KEY_CHANGES_REQUESTED, false),
    (KEY_COPILOT_REVIEWS, true),
    (KEY_SOUND, true),
    (KEY_UPDATE_CHECK, false),
    (KEY_LOG_LEVEL, false),
    (KEY_STATUS_COMPONENTS, false),
    // Not live: the listener binds once, at startup.
    (KEY_LOCAL_API, false),
    // Live, all three: the dispatch thread is always running and reads the setting and both paths
    // fresh on every pass, so turning it on or correcting a root takes effect within one pass. The
    // settings page must not claim a restart is needed when it is not.
    (KEY_DISPATCHER, true),
    (KEY_CLONE_ROOT, true),
    (KEY_WORKTREE_ROOT, true),
];

/// Every component GitHub publishes, for the settings page's checkboxes.
pub fn all_components() -> &'static [&'static str] {
    &KNOWN_STATUS_COMPONENTS
}

/// The value each key currently holds in `wanted`, as it would be written.
fn value_of(wanted: &Config, key: &str) -> String {
    let flag = |on: bool| if on { "on".to_string() } else { "off".to_string() };
    match key {
        KEY_REVIEW_REQUESTED => flag(wanted.pr_enabled[0]),
        KEY_READY_TO_MERGE => flag(wanted.pr_enabled[1]),
        KEY_CHANGES_REQUESTED => flag(wanted.pr_enabled[2]),
        KEY_COPILOT_REVIEWS => flag(wanted.copilot_reviews),
        KEY_LOCAL_API => flag(wanted.local_api),
        KEY_DISPATCHER => flag(wanted.dispatcher),
        KEY_CLONE_ROOT => wanted.clone_root.clone(),
        KEY_WORKTREE_ROOT => wanted.worktree_root.clone(),
        KEY_SOUND => flag(wanted.sound),
        KEY_UPDATE_CHECK => flag(wanted.update_check),
        KEY_LOG_LEVEL => match wanted.log_level {
            Level::Info => "info".to_string(),
            Level::Error => "error".to_string(),
        },
        KEY_STATUS_COMPONENTS => wanted.status_components.join(", "),
        other => unreachable!("{other} is not a writable key"),
    }
}

/// Writes `wanted` to `config.txt`, touching only the keys whose value actually changes.
///
/// Returns the keys it wrote. The settings page uses that to say which of them need a restart, and
/// to keep quiet when a save changed nothing.
///
/// **Only keys in `WRITABLE_KEYS` can be written**, and the value for each is produced here from a
/// typed `Config` rather than passed through from the request. A settings form that could name its
/// own keys would be a way to write arbitrary lines into the file.
///
/// Each key goes through the same single-line edit `set_sound` makes, so comments, blank lines,
/// spacing and keys this version has never heard of survive byte for byte. A key the file does not
/// have yet is appended rather than the file regenerated.
pub fn save(app_asset_path: &Path, wanted: &Config) -> Result<Vec<&'static str>, String> {
    let path = config_path(app_asset_path);
    let (current, _) = Config::load(app_asset_path);
    let changed: Vec<&'static str> = WRITABLE_KEYS
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| value_of(wanted, key) != value_of(&current, key))
        .collect();

    if changed.is_empty() {
        return Ok(changed);
    }
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();
    for key in &changed {
        content = with_value_set(&content, key, &value_of(wanted, key));
    }
    std::fs::write(&path, content)
        .map_err(|e| format!("could not write {} ({e})", path.display()))?;
    Ok(changed)
}

/// Whether `key` takes effect without a restart.
pub fn is_live(key: &str) -> bool {
    WRITABLE_KEYS.iter().any(|(k, live)| *k == key && *live)
}

/// Writes the `copilotReviews` value, the same surgical single-line edit `set_sound` makes.
pub fn set_copilot_reviews(app_asset_path: &Path, on: bool) -> Result<(), String> {
    set_flag(&config_path(app_asset_path), KEY_COPILOT_REVIEWS, on)
}

/// The Dispatcher tab's own switch, so the one page that explains what the dispatcher does is also
/// the page that turns it on. It writes the same single line a tick of the Settings checkbox does,
/// leaving every other byte of the file alone.
pub fn set_dispatcher(app_asset_path: &Path, on: bool) -> Result<(), String> {
    set_flag(&config_path(app_asset_path), KEY_DISPATCHER, on)
}

/// Reads the file, replaces one value, writes it back.
///
/// A missing or unreadable file reads as empty and so grows the one key, rather than failing: the
/// user asked for a setting, and the file is a convenience — the same posture the rest of this module
/// takes. `Config::load` has already written the default file by the time any menu exists, so this is
/// only reached when something has since deleted it.
fn set_flag(path: &Path, key: &str, on: bool) -> Result<(), String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let updated = with_value_set(&content, key, if on { "on" } else { "off" });
    std::fs::write(path, updated)
        .map_err(|e| format!("could not write {} ({e})", path.display()))
}

/// The file with one setting's value changed, and nothing else different.
///
/// Pure, so the whole risk in writing a file someone hand-edited can be tested without a disk.
///
/// Three rules, each of which is a way this could quietly destroy someone's file:
///
/// - **The line that wins is the line that is rewritten.** `parse` lets a later duplicate override an
///   earlier one, so editing the first of two would leave the app reading the second — a file whose
///   visible top line disagrees with the app's behaviour.
/// - **Comments are never matched.** A `# sound=on` line is a note, and rewriting it would destroy the
///   note *and* leave the real setting unchanged.
/// - **Line endings are preserved per line.** A `config.txt` edited in Notepad is CRLF, and rebuilding
///   the file with `\n` would rewrite every line of it — exactly the wholesale rewrite this function
///   exists to avoid.
fn with_value_set(content: &str, key: &str, value: &str) -> String {
    // `split_inclusive` rather than `lines`: it keeps each line's own terminator, which is what makes
    // the CRLF promise above possible.
    let mut pieces: Vec<String> = content.split_inclusive('\n').map(str::to_string).collect();

    match pieces.iter().rposition(|piece| line_sets(piece, key)) {
        Some(i) => {
            let terminator = line_ending(&pieces[i]);
            pieces[i] = format!("{key}={value}{terminator}");
        }
        // Not there at all: the ordinary case for an existing install, since the file is never
        // rewritten and so never grows a key added after it was written.
        None => {
            let newline = if content.contains("\r\n") { "\r\n" } else { "\n" };
            // A last line with no terminator would otherwise have the new key glued onto its end.
            if let Some(last) = pieces.last_mut()
                && !last.ends_with('\n')
            {
                last.push_str(newline);
            }
            pieces.push(format!("{key}={value}{newline}"));
        }
    }

    pieces.concat()
}

/// Whether this line sets `key`. The writer's half of `parse`, and it has to agree with it: the same
/// comment rule, the same trimming, and an **exact** key match, or `sound` would rewrite a
/// `soundVolume` line and append the setting it was asked for below it.
fn line_sets(line: &str, key: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    trimmed.split_once('=').is_some_and(|(k, _)| k.trim() == key)
}

/// A line's own terminator, so a rewritten line keeps it. Empty for a last line that has none.
fn line_ending(line: &str) -> &'static str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

impl Config {
    /// Reads `config.txt`, writing a default one first if there is none.
    ///
    /// Never fails. A file that cannot be written is logged and ignored; a file that cannot be read
    /// leaves every setting at its default.
    ///
    /// Also reports whether it had to create the file, which is this app's first-run signal — see
    /// [`FirstRun`]. Returned from here rather than offered as a separate `is_first_run` helper on
    /// purpose: `load` is what creates the file, so anything asking the question separately would have
    /// to run *before* `load` and would silently answer `No` forever the day someone reordered startup.
    pub fn load(app_asset_path: &Path) -> (Self, FirstRun) {
        let path = config_path(app_asset_path);
        let first_run = write_default_if_absent(&path);

        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let values = parse(&content);
        warn_about_renamed_keys(&values);
        (Self::from_values(&values), first_run)
    }

    /// The key-to-setting mapping, with no I/O.
    ///
    /// Split out purely so it can be tested: the whole risk in this file is a key being wired to the
    /// wrong setting, and `load` cannot be exercised without a filesystem. `parse` plus this is the
    /// entire behaviour of `load` bar reading the file.
    /// Reads a submitted settings form, falling back to `current` for anything it does not mention.
    ///
    /// Takes `crate::serve::Form` rather than a map because a group of checkboxes posts its name once
    /// per ticked box, and an **unticked** box posts nothing at all — which is how a form says "off",
    /// and why absence is read as `false` here rather than as "leave it alone".
    ///
    /// `current` covers the one field a form cannot express: a `logLevel` the page did not offer.
    pub fn from_form(form: &crate::serve::Form, current: &Self) -> Self {
        Config {
            update_check: form.ticked(KEY_UPDATE_CHECK),
            pr_enabled: PrAxis::ALL.map(|axis| form.ticked(pr_key(axis))),
            sound: form.ticked(KEY_SOUND),
            copilot_reviews: form.ticked(KEY_COPILOT_REVIEWS),
            local_api: form.ticked(KEY_LOCAL_API),
            dispatcher: form.ticked(KEY_DISPATCHER),
            // Absent from the form entirely means "not shown to me", which is not the same as
            // "cleared": the current value is kept. A box submitted empty is a deliberate clear,
            // and clears back to the default.
            clone_root: form.get(KEY_CLONE_ROOT).map(|v| v.trim().to_string()).unwrap_or_else(|| current.clone_root.clone()),
            worktree_root: form
                .get(KEY_WORKTREE_ROOT)
                .map(|v| v.trim().to_string())
                .unwrap_or_else(|| current.worktree_root.clone()),
            log_level: form
                .get(KEY_LOG_LEVEL)
                .and_then(|v| Level::parse(v))
                .unwrap_or(current.log_level),
            // Only names GitHub actually publishes, so a hand-crafted post cannot write a component
            // that would never match and would only ever show up as one line in the log.
            status_components: form
                .all(KEY_STATUS_COMPONENTS)
                .into_iter()
                .filter(|name| KNOWN_STATUS_COMPONENTS.contains(name))
                .map(str::to_string)
                .collect(),
        }
    }

    fn from_values(values: &std::collections::HashMap<&str, &str>) -> Self {
        Config {
            // Default **on**: an auto-update mechanism that is off until you find out it exists does
            // not do the job it was asked to do. `is_on` cannot express a default-on flag, hence
            // `is_off`.
            update_check: !values.get(KEY_UPDATE_CHECK).is_some_and(|v| is_off(v)),
            // Built by mapping over the axes rather than as a literal, so the axis name appears on
            // both sides of each pairing and the three cannot be written in the wrong order.
            pr_enabled: PrAxis::ALL.map(|axis| !values.get(pr_key(axis)).is_some_and(|v| is_off(v))),
            // Default **on**, for the reason `update_check` is: a notification sound nobody knows
            // about does not notify. `is_off` rather than `!is_on`, so `sound=onn` stays on.
            sound: !values.get(KEY_SOUND).is_some_and(|v| is_off(v)),
            // Unrecognised (or absent) falls back to the quiet default, the same way a typo'd bool
            // does — see `Level::parse`.
            log_level: values.get(KEY_LOG_LEVEL).and_then(|v| Level::parse(v)).unwrap_or(Level::Error),
            // The one list-valued setting. Absent, empty, or nothing but separators all come out
            // empty, which means the page-wide indicator rather than "watch nothing" — see the field.
            status_components: values
                .get(KEY_STATUS_COMPONENTS)
                .map(|v| split_list(v))
                .unwrap_or_default(),
            // Default **on**: Copilot's comments are feedback on your pull request whoever wrote
            // them, and an amber bar that cannot see them is the state this key was added to fix.
            // `is_off` rather than `!is_on`, so a typo leaves the default standing.
            copilot_reviews: !values.get(KEY_COPILOT_REVIEWS).is_some_and(|v| is_off(v)),
            // Default **off**, alone in this file, because it opens a listening socket at boot and
            // puts this run's token on disk. `is_on` rather than `!is_off`, which would read a
            // *missing* key as on and open the door on every install that never asked.
            local_api: values.get(KEY_LOCAL_API).is_some_and(|v| is_on(v)),
            dispatcher: values.get(KEY_DISPATCHER).is_some_and(|v| is_on(v)),
            clone_root: values.get(KEY_CLONE_ROOT).map(|v| v.trim().to_string()).unwrap_or_default(),
            worktree_root: values.get(KEY_WORKTREE_ROOT).map(|v| v.trim().to_string()).unwrap_or_default(),
        }
    }

    /// Whether `axis`'s bar, menu entry and search are wanted at all.
    pub fn pr_enabled(&self, axis: PrAxis) -> bool {
        self.pr_enabled[axis.index()]
    }

    /// Whether any PR signal is wanted.
    ///
    /// With none of them enabled there is no reason to obtain a PR credential at all, which is what
    /// keeps a switched-off feature from making a network call or raising a sign-in dialog.
    /// The portals to watch, in the order they should appear.
    ///
    /// Always exactly one today: the implicit GitHub portal every existing `config.txt` describes
    /// without naming it. That is deliberate, not a stub. Naming portals in the file
    /// (`portal.<name>.type=github|gitlab|bitbucket`, `portal.<name>.url=`, and per-portal
    /// `enabled`, `interval` and `clientId` after it) is reserved for the release that ships a
    /// second kind of portal, and the rule for that day is already fixed here: the moment any
    /// `portal.` key is present the implicit one disappears, so an old file and a new file never
    /// describe two different models at once. The flat `key=value` parser reads dotted keys as they
    /// are, so nothing about the file format changes when they arrive.
    ///
    /// GitHub Enterprise Server is not a URL swap either: its device flow needs a GitHub App
    /// registered on that instance, so `url` ships together with `clientId` or not at all.
    pub fn portals(&self) -> Vec<PortalConfig> {
        vec![PortalConfig {
            id: PortalId(IMPLICIT_PORTAL.to_string()),
            kind: PortalKind::GitHub,
            base_url: crate::portal::github::DEFAULT_BASE_URL.to_string(),
        }]
    }

    pub fn any_pr_enabled(&self) -> bool {
        PrAxis::ALL.iter().any(|&axis| self.pr_enabled(axis))
    }
}

/// The config key for `axis`. The one place the mapping lives.
pub fn pr_key(axis: PrAxis) -> &'static str {
    match axis {
        PrAxis::ReviewRequested => KEY_REVIEW_REQUESTED,
        PrAxis::ReadyToMerge => KEY_READY_TO_MERGE,
        PrAxis::ChangesRequested => KEY_CHANGES_REQUESTED,
    }
}

/// The file written on first run: every setting, at its default, with a line explaining each.
///
/// Pure, and separated from the write so the round-trip test can prove the template and
/// [`Config::load`] agree without touching a disk. Values are *active* rather than commented out, so
/// the file states what is actually in force and editing one means changing a value.
fn default_config() -> String {
    // Joined rather than written out, so the list has exactly one definition and the file cannot name
    // a component the code has never heard of.
    let components = DEFAULT_STATUS_COMPONENTS.join(", ");
    // The ones the shipped value leaves out, so the comment can name them as the things to add back.
    let others: Vec<&str> = KNOWN_STATUS_COMPONENTS
        .iter()
        .copied()
        .filter(|c| !DEFAULT_STATUS_COMPONENTS.contains(c))
        .collect();
    let others = others.join(", ");
    format!(
        "# githoot settings\n\
         #\n\
         # One key=value per line. Lines starting with # are ignored. Anything not recognised as an\n\
         # off value (off, false, 0, no) leaves the setting at its default, so a typo cannot silently\n\
         # switch something off.\n\
         #\n\
         # This file was created automatically with every setting at its default. It is never\n\
         # rewritten, so your edits are safe.\n\
         \n\
         # Check GitHub for a newer release once a day, and at startup.\n\
         {KEY_UPDATE_CHECK}=on\n\
         \n\
         # The three pull-request signals, shown as coloured bars down the right of the icon.\n\
         # Turning one off removes its bar and its menu entry, and stops it being searched for.\n\
         {KEY_REVIEW_REQUESTED}=on\n\
         {KEY_READY_TO_MERGE}=on\n\
         {KEY_CHANGES_REQUESTED}=on\n\
         \n\
         # Play a short hoot whenever a pull-request count goes up. Only the sound is affected:\n\
         # the icon, tooltip and counts behave the same either way.\n\
         {KEY_SOUND}=on\n\
         \n\
         # How much the log file records. \"error\" (the default) logs only failures; \"info\" adds\n\
         # the normal lifecycle detail (startup, sign-in, updates, each poll) for diagnosing.\n\
         {KEY_LOG_LEVEL}=error\n\
         \n\
         # Whether unresolved comments from GitHub's automatic reviewer count as work on the amber\n\
         # bar. Copilot reviews by commenting and never by approving or requesting changes, so\n\
         # without this its feedback is invisible here. Resolved and outdated threads never count.\n\
         {KEY_COPILOT_REVIEWS}=on\n\
         \n\
         # Which parts of GitHub may put the exclamation on the icon, comma separated, one line.\n\
         # Delete the ones you do not care about and they stop raising the mark; add any of the\n\
         # others below to watch them too. GitHub's own page-wide verdict says \"degraded\" whenever\n\
         # any single component is, including the ones a pull-request tray never touches, which is\n\
         # why the line below names only the parts this app actually uses.\n\
         #\n\
         # Also available: {others}\n\
         #\n\
         # Names must match GitHub's exactly, bar case. An empty list watches the whole page.\n\
         {KEY_STATUS_COMPONENTS}={components}\n\
         \n\
         # Serve the judged pull-request lists as JSON to scripts running as you on this machine.\n\
         #\n\
         # The only setting here that is off by default, and the only one that needs an explicit\n\
         # on: unlike every other line in this file, a typo leaves it shut rather than open.\n\
         #\n\
         # On, the local port opens at startup instead of on your first menu click, and\n\
         # endpoint.json is written beside this file (owner-only) with that port and this run's\n\
         # token, so a script can find the address. Off, nothing is written and the route answers\n\
         # 404. Restart to apply.\n\
         {KEY_LOCAL_API}=off\n\
         \n\
         # Start a Herdr agent for each pull request that needs one.\n\
         #\n\
         # The only setting that makes GitHoot do something rather than show something: it creates\n\
         # branches and worktrees and launches agents, under your own gh credential. Needs herdr,\n\
         # gh and git on PATH. Off by default, and like the line above a typo leaves it off.\n\
         # Read docs/dispatcher.md before turning it on. Takes effect within one pass.\n\
         {KEY_DISPATCHER}=off\n\
         \n\
         # Where the dispatcher looks for your clones, one directory per repository name. A pull\n\
         # request in owner/thing needs a clone at <this>/thing. Empty means ~/projects.\n\
         #\n\
         # Set this if your clones are anywhere else. Without it the dispatcher finds nothing and\n\
         # says so once per pull request in the log.\n\
         {KEY_CLONE_ROOT}=\n\
         \n\
         # Where the dispatcher puts the worktree it makes for each pull request. Empty means\n\
         # ~/worktrees. One directory per pull request, named githoot/pr-<number>-<repo>.\n\
         {KEY_WORKTREE_ROOT}=\n"
    )
}

/// Writes the default file if there is none, and reports whether it did.
///
/// Guarded on existence, **not** on the parsed settings being empty: a file holding nothing but
/// comments is a deliberate act, and overwriting it would throw away someone's notes.
///
/// A failed write is [`FirstRun::No`], not `Yes` — see [`FirstRun`] for why "we could not remember
/// meeting you" must not be read as "we have just met you".
fn write_default_if_absent(path: &Path) -> FirstRun {
    if path.exists() {
        return FirstRun::No;
    }
    match std::fs::write(path, default_config()) {
        Ok(()) => {
            infoln!("wrote a default {} — every setting at its default", CONFIG_FILE);
            FirstRun::Yes
        }
        // Not fatal, and not worth a dialog: the defaults still apply, so the app behaves exactly as
        // it would have. Same treatment `access_token` gives a failed `client_id.txt` write.
        Err(e) => {
            errorln!("could not write a default {CONFIG_FILE} ({e}) — using defaults");
            FirstRun::No
        }
    }
}

/// Says so when the file still uses a key that has been renamed.
///
/// The old key genuinely has no effect. Without this the only symptom would be a feature quietly
/// behaving differently after an upgrade, which is the kind of thing people spend an hour on.
fn warn_about_renamed_keys(values: &std::collections::HashMap<&str, &str>) {
    for (old, new) in RENAMED_KEYS {
        if values.contains_key(old) {
            errorln!(
                "{CONFIG_FILE} uses \"{old}\", which is no longer read. Rename it to \"{new}\"."
            );
        }
    }
}

/// Whether a value reads as "off".
///
/// The mirror of `is_on`, needed because a default-on setting cannot be expressed with `is_on`: absent
/// has to mean on, so only an explicit off may turn it off. Deliberately not `!is_on(v)` — that would
/// make a typo like `updateCheck=yse` read as off, silently disabling a feature the user was trying to
/// confirm. An unrecognised value leaves the default alone.
fn is_on(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "true" | "1" | "yes")
}

fn is_off(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "off" | "false" | "0" | "no")
}

/// Parses `key=value` lines into a lookup, tolerating comments, blank lines, and stray
/// whitespace. Later duplicate keys win, matching how most `key=value` config formats behave.
fn parse(content: &str) -> std::collections::HashMap<&str, &str> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

/// Splits a comma-separated value into its entries, trimmed, with the empties dropped.
///
/// Empties are dropped rather than kept because a trailing comma is the most ordinary edit there is
/// (delete the last name, leave the comma), and an entry of `""` would be a watch on a component
/// whose name is nothing.
fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Saving from the settings page ─────────────────────────────────────────

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-cfg-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Only the keys that actually changed are written, so saving a form you did not touch leaves the
    /// file alone and the page can say so.
    #[test]
    fn saving_writes_only_what_changed() {
        let dir = temp_dir("only-changed");
        let (mut cfg, _) = Config::load(&dir);
        assert_eq!(save(&dir, &cfg).expect("save"), Vec::<&str>::new(), "nothing touched");

        cfg.sound = !cfg.sound;
        assert_eq!(save(&dir, &cfg).expect("save"), vec![KEY_SOUND]);

        let (after, _) = Config::load(&dir);
        assert_eq!(after.sound, cfg.sound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The surgical edit `set_sound` makes, kept: a save must not cost the user their comments.
    #[test]
    fn saving_preserves_comments_and_unknown_keys() {
        let dir = temp_dir("preserve");
        let path = config_path(&dir);
        std::fs::write(&path, "# my note\nsound=on\nsomeFutureKey=42\n").expect("seed");

        let (mut cfg, _) = Config::load(&dir);
        cfg.sound = false;
        save(&dir, &cfg).expect("save");

        let after = std::fs::read_to_string(&path).expect("read");
        assert!(after.contains("# my note"), "got {after:?}");
        assert!(after.contains("someFutureKey=42"), "got {after:?}");
        assert!(after.contains("sound=off"), "got {after:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The component list survives a round trip, which is the key the settings page exists for.
    #[test]
    fn saving_round_trips_the_component_list() {
        let dir = temp_dir("components");
        let (mut cfg, _) = Config::load(&dir);
        cfg.status_components = vec!["Issues".to_string(), "Actions".to_string()];
        save(&dir, &cfg).expect("save");

        let (after, _) = Config::load(&dir);
        assert_eq!(after.status_components, ["Issues", "Actions"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_level_round_trips() {
        let dir = temp_dir("loglevel");
        let (mut cfg, _) = Config::load(&dir);
        cfg.log_level = Level::Info;
        save(&dir, &cfg).expect("save");
        let (after, _) = Config::load(&dir);
        assert!(matches!(after.log_level, Level::Info));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every key the page can write must be one `load` reads, and every key `load` reads must be one
    /// the page can write — or a setting exists that the page silently cannot reach.
    #[test]
    fn the_writable_keys_are_exactly_the_keys_that_are_read() {
        let mut writable: Vec<&str> = WRITABLE_KEYS.iter().map(|(k, _)| *k).collect();
        let text = default_config();
        let mut generated: Vec<&str> = parse(&text).keys().copied().collect();
        writable.sort_unstable();
        generated.sort_unstable();
        assert_eq!(writable, generated);
    }

    /// `live` means "takes effect without a restart", which is what the settings page tells the
    /// user. It is not a synonym for "has a tray checkbox": the dispatcher has no tray entry and is
    /// still live, because its thread re-reads the setting on every pass. Everything else here
    /// binds at startup, and saying otherwise on the page would be a broken promise, not a typo.
    #[test]
    fn exactly_the_settings_that_need_no_restart_are_live() {
        let live: Vec<&str> =
            WRITABLE_KEYS.iter().filter(|(_, live)| *live).map(|(k, _)| *k).collect();
        assert_eq!(live, [KEY_COPILOT_REVIEWS, KEY_SOUND, KEY_DISPATCHER, KEY_CLONE_ROOT, KEY_WORKTREE_ROOT]);
        assert!(is_live(KEY_SOUND) && !is_live(KEY_LOG_LEVEL));
        assert!(!is_live(KEY_LOCAL_API), "the listener binds once, at startup");
    }

    // ── The live switch ───────────────────────────────────────────────────────
    /// The whole point of the type: the menu holds one handle and the poll loop another, and a click
    /// on the first has to be visible from the second. A `bool` copied into the loop could not do it,
    /// which is what made unticking the box take a restart.
    #[test]
    fn a_clone_sees_what_the_original_was_set_to() {
        let menus_copy = Switch::new(true);
        let poll_loops_copy = menus_copy.clone();
        assert!(poll_loops_copy.is_on(), "it starts where it was built");

        menus_copy.set(false);
        assert!(!poll_loops_copy.is_on(), "unticking the box must silence the loop at once");

        menus_copy.set(true);
        assert!(poll_loops_copy.is_on(), "and ticking it must bring the hoot back");
    }

    /// The first-run signal, end to end through a real directory. Both halves matter: `Yes` exactly
    /// once, and `No` on every start after it, because `autostart` asks a question that must never be
    /// asked twice.
    #[test]
    fn load_reports_a_first_run_only_when_it_created_the_file() {
        let dir = std::env::temp_dir().join(format!("githoot-config-first-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test temp dir");

        let (_, first) = Config::load(&dir);
        assert_eq!(first, FirstRun::Yes, "an absent config.txt is a first run");
        assert!(config_path(&dir).exists(), "load must have written the default file");

        let (_, second) = Config::load(&dir);
        assert_eq!(second, FirstRun::No, "a config.txt that already exists is not a first run");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A write that fails leaves nothing to remember an answer in, so it must not read as a first run
    /// — otherwise every single launch would look like one and re-ask. Forced by pointing at a path
    /// whose parent does not exist, which fails identically on all three platforms.
    #[test]
    fn a_config_that_could_not_be_written_is_not_a_first_run() {
        let path = std::env::temp_dir()
            .join(format!("githoot-config-unwritable-{}", std::process::id()))
            .join("no-such-parent")
            .join(CONFIG_FILE);
        assert_eq!(write_default_if_absent(&path), FirstRun::No);
        assert!(!path.exists(), "nothing should have been created");
    }

    #[test]
    fn recognises_off_values_case_insensitively() {
        for v in ["off", "Off", "FALSE", "0", "no", " off "] {
            assert!(is_off(v), "{v:?} should read as off");
        }
    }

    /// The reason `is_off` is not `!is_on`. A default-on setting is turned off only by an explicit
    /// off value, so a typo has to leave it **on** — otherwise someone who mistyped while trying to
    /// confirm a setting would silently disable it.
    #[test]
    fn a_typo_does_not_read_as_off() {
        for v in ["yse", "onn", "enabled", "", "maybe", "1 "] {
            assert!(!is_off(v), "{v:?} must not read as off, or a typo disables the feature");
        }
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let content = "# comment\n\nupdateCheck=on\n  # indented comment\n";
        let values = parse(content);
        assert_eq!(values.get("updateCheck"), Some(&"on"));
    }

    #[test]
    fn whitespace_around_key_and_value_is_trimmed() {
        let values = parse("  updateCheck = on  \n");
        assert_eq!(values.get("updateCheck"), Some(&"on"));
    }

    #[test]
    fn a_missing_key_is_absent_not_a_default_guess() {
        assert_eq!(parse("").get("updateCheck"), None);
        assert_eq!(parse("other=on").get("updateCheck"), None);
    }

    #[test]
    fn a_line_with_no_equals_sign_is_ignored_rather_than_panicking() {
        let values = parse("updateCheck\nupdateCheck=on\n");
        assert_eq!(values.get("updateCheck"), Some(&"on"));
    }

    // ── The generated file ──────────────────────────────────────────────────

    /// The template and the defaults must agree, and this is the only thing making that true: the
    /// file says `updateCheck=on` and the code defaults it to on, in two separate places that could
    /// drift. Parsing the template back and checking every key is what catches a drift.
    #[test]
    fn the_generated_file_states_exactly_the_documented_defaults() {
        // Bound first: `parse` borrows from its input, so the template has to outlive the map.
        let template = default_config();
        let values = parse(&template);

        assert_eq!(values.get(KEY_UPDATE_CHECK), Some(&"on"));
        assert_eq!(values.get(KEY_LOG_LEVEL), Some(&"error"));
        assert_eq!(values.get(KEY_SOUND), Some(&"on"));
        for axis in PrAxis::ALL {
            assert_eq!(values.get(pr_key(axis)), Some(&"on"), "{axis:?}");
        }
        // The one key whose written value is not its absent-key default: see
        // `the_template_is_explicit_where_an_absent_key_is_not`.
        assert!(values.get(KEY_STATUS_COMPONENTS).is_some_and(|v| v.contains("Pull Requests")));
        // The two keys shipped off. Written explicitly anyway, so the file says the doors exist.
        assert_eq!(values.get(KEY_LOCAL_API), Some(&"off"));
        assert_eq!(values.get(KEY_DISPATCHER), Some(&"off"));
        // Written empty on purpose: the file names the knob and its default in one place.
        assert_eq!(values.get(KEY_CLONE_ROOT), Some(&""));
        assert_eq!(values.get(KEY_WORKTREE_ROOT), Some(&""));
        // And nothing else, so a key added to the template without being read is caught.
        assert_eq!(values.len(), 12, "unexpected keys in the template: {values:?}");
    }

    /// A fresh file watches the parts a pull-request tray actually touches, and no more.
    ///
    /// A single degraded component makes GitHub's page-wide verdict read "Partially Degraded
    /// Service", so shipping the full list meant Copilot having a bad afternoon put a red
    /// exclamation on a tray that never calls it. Cry wolf often enough and the mark stops meaning
    /// anything.
    #[test]
    fn a_fresh_config_watches_only_the_parts_a_pr_tray_touches() {
        let text = default_config();
        let values = parse(&text);
        let watched = split_list(values.get(KEY_STATUS_COMPONENTS).expect("the key"));
        assert_eq!(watched, default_status_components());
        for absent in ["Copilot", "Codespaces", "Copilot AI Model Providers"] {
            assert!(!watched.iter().any(|c| c == absent), "{absent} should not be watched");
        }
    }

    /// The ones left out have to be *named* somewhere, or adding one back is a research task against
    /// GitHub's status page rather than an edit. The value is the short list; the comment is the menu.
    #[test]
    fn the_comment_names_every_component_github_publishes() {
        let text = default_config();
        for component in KNOWN_STATUS_COMPONENTS {
            assert!(text.contains(component), "the template never mentions {component}");
        }
    }

    /// A typo in the shipped list would name a component GitHub does not publish, which shows up only
    /// as one line in the log and a watch that never fires.
    #[test]
    fn every_shipped_component_is_one_github_publishes() {
        for component in DEFAULT_STATUS_COMPONENTS {
            assert!(
                KNOWN_STATUS_COMPONENTS.contains(&component),
                "{component} is shipped but is not a component GitHub publishes"
            );
        }
    }

    /// Every key the template writes must be one `load` actually reads. A key present in the file
    /// but ignored by the code is worse than a missing one: it looks like it works.
    #[test]
    fn every_generated_key_is_one_that_is_read() {
        let known = [
            KEY_UPDATE_CHECK,
            KEY_REVIEW_REQUESTED,
            KEY_READY_TO_MERGE,
            KEY_CHANGES_REQUESTED,
            KEY_LOG_LEVEL,
            KEY_SOUND,
            KEY_STATUS_COMPONENTS,
            KEY_COPILOT_REVIEWS,
            KEY_LOCAL_API,
            KEY_DISPATCHER,
            KEY_CLONE_ROOT,
            KEY_WORKTREE_ROOT,
        ];
        let text = default_config();
        for key in parse(&text).keys() {
            assert!(known.contains(key), "{key:?} is written but never read");
        }
    }

    // ── localApi ────────────────────────────────────────────────────────────

    /// The only default-**off** key in the file, and it has to stay that way: it opens a listening
    /// socket at boot and writes this run's token to disk. Every other setting here can be wrong in
    /// the user's favour; this one cannot.
    ///
    /// Also pins the typo direction. `copilotReviews` uses `is_off` so a typo leaves a default-on
    /// setting on; this one uses `is_on` so a typo leaves a default-off setting off. Both mean "an
    /// unrecognised value never moves the setting", which `!is_off` would get backwards here by
    /// turning *absence* into on.
    #[test]
    fn the_local_api_setting_defaults_to_off() {
        assert!(!values("").local_api, "an empty config must leave the local API shut");
        assert!(!values("localApi=off").local_api);
        assert!(!values("localApi=yeah nah").local_api, "an unrecognised value must not open it");
        assert!(values("localApi=on").local_api, "only an explicit on opens it");
        assert!(values("localApi=true").local_api);
    }

    /// The nine-site wiring, end to end through the file: a `value_of` arm pointed at the wrong
    /// field, or a missing `WRITABLE_KEYS` entry, both show up here as a value that will not stick.
    #[test]
    fn the_local_api_setting_round_trips_through_a_save() {
        let dir = temp_dir("local-api-round-trip");
        let _ = std::fs::create_dir_all(&dir);
        let (mut cfg, _) = Config::load(&dir);
        assert!(!cfg.local_api, "a fresh config.txt must ship it off");

        cfg.local_api = true;
        let written = save(&dir, &cfg).expect("save should succeed");
        assert!(written.contains(&KEY_LOCAL_API), "the key must actually be written");
        assert!(Config::load(&dir).0.local_api, "and must survive a reload");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── The hoot switch ─────────────────────────────────────────────────────

    fn values(content: &str) -> Config {
        Config::from_values(&parse(content))
    }

    /// Default on: the hoot is the feature someone just asked for, and a notification sound that is
    /// off until you find the setting does not do its job — the same reasoning as `updateCheck`.
    #[test]
    fn sound_is_on_when_the_key_is_absent() {
        assert!(values("").sound, "an empty config must leave the hoot on");
        assert!(values("other=off
").sound);
    }

    #[test]
    fn sound_is_off_only_when_explicitly_turned_off() {
        for v in ["off", "Off", "FALSE", "0", "no"] {
            assert!(!values(&format!("sound={v}
")).sound, "sound={v} must silence it");
        }
    }

    /// A default-on flag must survive a typo, or someone confirming the setting turns it off.
    #[test]
    fn a_mistyped_sound_value_leaves_the_hoot_on() {
        for v in ["offf", "yse", "", "quiet"] {
            assert!(values(&format!("sound={v}
")).sound, "sound={v} must not silence it");
        }
    }

    /// Every line the template writes is either a comment, blank, or a flush-left `key=value`. A
    /// missing `\n\` continuation in the source produces a real newline plus the source's own
    /// indentation, which `parse` still reads — so the only symptom is a file that looks broken to
    /// the person opening it.
    #[test]
    fn no_line_of_the_template_is_indented() {
        for line in default_config().lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace: {line:?}");
            assert!(!line.starts_with(' '), "indented line: {line:?}");
        }
    }

    /// The template must survive its own comment stripping — a `#` in the wrong column, or a missing
    /// newline escape, would produce a file that parses to nothing while looking fine in the source.
    #[test]
    fn the_generated_file_is_mostly_comments_and_still_parses() {
        let text = default_config();
        assert!(text.starts_with('#'), "should open with an explanatory header");
        assert!(text.ends_with('\n'), "should end with a newline");
        assert!(text.lines().filter(|l| l.starts_with('#')).count() >= 8, "should explain itself");
    }

    // ── Renamed keys ────────────────────────────────────────────────────────

    /// The old names must not be read. This is the clean break, asserted rather than assumed: an
    /// accidentally reinstated alias would make the rename a no-op and the warning a lie.
    #[test]
    fn the_old_key_names_are_not_read() {
        let values = parse("update_check=off\n");
        assert_eq!(values.get(KEY_UPDATE_CHECK), None);
        // …but they are recognised well enough to be warned about.
        for (old, _) in RENAMED_KEYS {
            assert!(values.contains_key(old), "{old:?} should be seen, just not obeyed");
        }
    }

    /// Each renamed key must point at a key that exists, or the warning would tell someone to use a
    /// name nothing reads.
    #[test]
    fn every_rename_points_at_a_real_key() {
        let text = default_config();
        let template = parse(&text);
        for (old, new) in RENAMED_KEYS {
            assert!(template.contains_key(new), "{old:?} points at {new:?}, which is not a real key");
        }
    }

    // ── Keys wired to the right settings ────────────────────────────────────
    //
    // The one real risk in this file: a key reaching the wrong setting. It compiles, type-checks, and
    // is visible only by watching the tray, so it is tested here rather than trusted.

    fn from(text: &str) -> Config {
        Config::from_values(&parse(text))
    }

    #[test]
    fn an_empty_file_gives_every_documented_default() {
        let cfg = from("");
        assert!(cfg.update_check, "updateCheck defaults on");
        assert_eq!(cfg.log_level, Level::Error, "logLevel defaults to error");
        for axis in PrAxis::ALL {
            assert!(cfg.pr_enabled(axis), "{axis:?} defaults on");
        }
        assert!(cfg.any_pr_enabled());
    }

    /// The generated file must produce exactly the same `Config` as no file at all. This is what
    /// stops the template and the defaults drifting apart — and it compares the parsed *settings*,
    /// not the text, so it would catch a key written with the wrong value.
    ///
    /// `status_components` is the deliberate exception and is checked separately, by
    /// `the_template_is_explicit_where_an_absent_key_is_not`: a fresh file names every component,
    /// an absent key means the whole page, and on the day of writing those two are the same set.
    #[test]
    fn the_generated_file_produces_the_same_settings_as_no_file() {
        let template = default_config();
        let generated = Config::from_values(&parse(&template));
        let defaults = from("");

        assert_eq!(generated.update_check, defaults.update_check);
        assert_eq!(generated.log_level, defaults.log_level);
        for axis in PrAxis::ALL {
            assert_eq!(generated.pr_enabled(axis), defaults.pr_enabled(axis), "{axis:?}");
        }
    }

    /// Turning off one PR key must disable exactly that axis. Checked for all three, because a
    /// transposed mapping would still pass if only one were tested.
    #[test]
    fn each_pr_key_disables_exactly_its_own_axis() {
        for target in PrAxis::ALL {
            let cfg = from(&format!("{}=off\n", pr_key(target)));
            for axis in PrAxis::ALL {
                let expected = axis != target;
                assert_eq!(
                    cfg.pr_enabled(axis),
                    expected,
                    "{}=off should disable {target:?} and nothing else, but {axis:?} is wrong",
                    pr_key(target)
                );
            }
        }
    }

    #[test]
    fn all_three_pr_keys_off_means_no_pr_signals_at_all() {
        let cfg = from("reviewRequested=off\nreadyToMerge=off\nchangesRequested=off\n");
        assert!(!cfg.any_pr_enabled(), "this is what skips the PR sign-in entirely");
        // …and the unrelated settings are untouched.
        assert!(cfg.update_check);
    }

    #[test]
    fn the_two_default_on_settings_are_turned_off_only_by_an_explicit_off() {
        assert!(!from("updateCheck=off\n").update_check);
        assert!(!from("readyToMerge=no\n").pr_enabled(PrAxis::ReadyToMerge));
        // A typo leaves them on, which is the whole reason `is_off` is not `!is_on`.
        assert!(from("updateCheck=yse\n").update_check);
        assert!(from("readyToMerge=onn\n").pr_enabled(PrAxis::ReadyToMerge));
    }

    /// The clean break, asserted end to end rather than only at the parse layer: the old spellings
    /// must not reach the settings they used to control.
    #[test]
    fn the_old_key_names_no_longer_change_anything() {
        let cfg = from("update_check=off\n");
        assert!(cfg.update_check, "the old name must not switch update checks off");
    }

    // ── Per-axis keys ───────────────────────────────────────────────────────

    // ── Watched status components ───────────────────────────────────────────

    /// Absent means the whole page, which is what every config.txt written before this key existed
    /// says — and those files are never rewritten. See `default_config`.
    #[test]
    fn status_components_is_empty_when_the_key_is_absent() {
        assert!(from("").status_components.is_empty());
        assert!(from("updateCheck=on
").status_components.is_empty());
    }

    #[test]
    fn status_components_are_split_on_commas_and_trimmed() {
        let cfg = from("statusComponents=Issues,  Pull Requests ,Git Operations
");
        assert_eq!(cfg.status_components, ["Issues", "Pull Requests", "Git Operations"]);
    }

    /// A trailing comma, a double comma, or a value of nothing but separators must not produce a
    /// watch on the empty name — which would match nothing and read as "watching something".
    #[test]
    fn empty_entries_are_dropped() {
        assert_eq!(from("statusComponents=Issues,,  ,Actions,
").status_components, ["Issues", "Actions"]);
        assert!(from("statusComponents=
").status_components.is_empty());
        assert!(from("statusComponents=  , ,
").status_components.is_empty());
    }

    /// The user's own spelling survives, because it is what the log quotes back when a name matches
    /// no live component. Case folding happens at match time, not here.
    #[test]
    fn the_configured_spelling_is_kept_verbatim() {
        assert_eq!(from("statusComponents= ISSUES 
").status_components, ["ISSUES"]);
    }

    /// The written *value* is the short list, not every component. The rest are named in the comment
    /// beside it — asserted by `the_comment_names_every_component_github_publishes` — so trimming and
    /// extending are both edits rather than research. This is the one place a stale list would show.
    #[test]
    fn the_generated_template_watches_the_shipped_list() {
        let template = default_config();
        let cfg = Config::from_values(&parse(&template));
        assert_eq!(cfg.status_components, DEFAULT_STATUS_COMPONENTS.to_vec());
        assert!(cfg.status_components.contains(&"Pull Requests".to_string()));
    }

    /// The deliberate exception to "the template equals the defaults": a fresh file names the
    /// components explicitly, an absent key means the whole page. Asserted so the divergence is a
    /// decision on the record rather than a drift someone later "fixes".
    #[test]
    fn the_template_is_explicit_where_an_absent_key_is_not() {
        let template = default_config();
        assert!(!Config::from_values(&parse(&template)).status_components.is_empty());
        assert!(from("").status_components.is_empty());
    }

    /// The template is one `key=value` line like every other, since `parse` has no continuation
    /// syntax — a wrapped list would silently lose everything after the first line.
    #[test]
    fn the_component_list_is_a_single_line() {
        let template = default_config();
        let line = template
            .lines()
            .find(|l| l.starts_with(KEY_STATUS_COMPONENTS))
            .expect("the template must state the key");
        assert!(line.contains("Pages"), "the whole list must fit on one line: {line}");
        // The comment's "Also available" roster is a separate line and must not leak into the value.
        assert!(!line.contains("Codespaces"), "the comment's extras are not watched: {line}");
    }

    /// The three axes must map to three *distinct* keys. Duplicating one would silently tie two bars
    /// to the same setting.
    #[test]
    fn each_axis_has_its_own_distinct_key() {
        let keys: Vec<&str> = PrAxis::ALL.iter().map(|&a| pr_key(a)).collect();
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "duplicate axis keys: {keys:?}");
    }

    // ── Writing one setting back ────────────────────────────────────────────
    //
    // The menu's toggles edit this file. Every test here is about the same fear: that changing one
    // value throws away something the user wrote.

    /// The whole point. One line changes; every comment, blank line and unrelated setting is
    /// byte-for-byte what it was.
    #[test]
    fn setting_a_value_leaves_the_rest_of_the_file_untouched() {
        let before = "# my notes\n\
                      updateCheck=off\n\
                      \n\
                      # the hoot\n\
                      sound=on\n\
                      logLevel=info\n";
        let after = with_value_set(before, KEY_SOUND, "off");
        assert_eq!(
            after,
            "# my notes\n\
             updateCheck=off\n\
             \n\
             # the hoot\n\
             sound=off\n\
             logLevel=info\n"
        );
    }

    /// `parse` lets a later duplicate win, so the writer has to edit *that* one. Editing the first
    /// would produce a file whose visible top line disagrees with what the app actually reads.
    #[test]
    fn the_line_that_wins_is_the_line_that_is_rewritten() {
        let after = with_value_set("sound=on\nsound=on\n", KEY_SOUND, "off");
        assert_eq!(after, "sound=on\nsound=off\n");
        assert!(!Config::from_values(&parse(&after)).sound, "the file must read back as off");
    }

    /// A commented-out line is someone's note, not a setting. Rewriting it would both destroy the
    /// note and leave the real value unchanged, which is the worst of both.
    #[test]
    fn a_commented_out_line_is_not_mistaken_for_the_setting() {
        let after = with_value_set("# sound=on\nupdateCheck=on\n", KEY_SOUND, "off");
        assert!(after.starts_with("# sound=on\n"), "the comment must survive: {after:?}");
        assert!(!Config::from_values(&parse(&after)).sound, "the setting must still take: {after:?}");
    }

    /// The case every existing install is in: `config.txt` is never rewritten, so a file written
    /// before a key existed will not have it. Appending is what makes the toggle work there.
    #[test]
    fn a_key_the_file_does_not_have_is_appended() {
        let after = with_value_set("updateCheck=on\n", KEY_SOUND, "off");
        assert!(after.starts_with("updateCheck=on\n"), "got {after:?}");
        assert!(!Config::from_values(&parse(&after)).sound, "got {after:?}");
    }

    /// A file whose last line has no newline is ordinary — plenty of editors write one. Appending
    /// without a separator would glue the new key onto the end of the last value.
    #[test]
    fn a_file_with_no_trailing_newline_still_gets_a_line_of_its_own() {
        let after = with_value_set("updateCheck=on", KEY_SOUND, "off");
        assert_eq!(parse(&after).get(KEY_UPDATE_CHECK), Some(&"on"), "got {after:?}");
        assert_eq!(parse(&after).get(KEY_SOUND), Some(&"off"), "got {after:?}");
    }

    /// Spacing around the `=` is legal and `parse` trims it, so a file written that way must still
    /// be recognised as holding the key rather than having a second copy appended.
    #[test]
    fn spaced_out_lines_are_rewritten_rather_than_duplicated() {
        let after = with_value_set("  sound = on  \n", KEY_SOUND, "off");
        assert_eq!(after.lines().filter(|l| l.contains(KEY_SOUND)).count(), 1, "got {after:?}");
        assert!(!Config::from_values(&parse(&after)).sound, "got {after:?}");
    }

    /// A key that merely *starts* with another key's name is a different key. Without an exact
    /// match, `sound` would rewrite `soundVolume` and the real setting would be appended below it.
    #[test]
    fn a_longer_key_that_starts_the_same_is_a_different_key() {
        let after = with_value_set("soundVolume=3\n", KEY_SOUND, "off");
        assert!(after.contains("soundVolume=3"), "got {after:?}");
        assert_eq!(parse(&after).get(KEY_SOUND), Some(&"off"), "got {after:?}");
    }

    /// End to end through a real file: what the menu writes is what the next start reads.
    #[test]
    fn set_sound_round_trips_through_the_file_load_reads() {
        let dir = std::env::temp_dir().join(format!("githoot-config-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test temp dir");

        let (config, _) = Config::load(&dir);
        assert!(config.sound, "a fresh config.txt hoots");

        set_sound(&dir, false).expect("the menu must be able to write the file");
        let (config, first) = Config::load(&dir);
        assert!(!config.sound, "the file must now say off");
        assert_eq!(first, FirstRun::No, "writing a setting must not look like a fresh install");

        set_sound(&dir, true).expect("and back on again");
        assert!(Config::load(&dir).0.sound);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Portals ───────────────────────────────────────────────────────────────

    /// Every file that exists today describes one GitHub portal without naming it, and an empty
    /// file is the same file with nothing filled in. Both must keep working exactly as before.
    #[test]
    fn an_existing_file_describes_the_one_implicit_github_portal() {
        for text in ["", "sound=off\nreviewRequested=on\ncopilotReviews=off"] {
            let portals = values(text).portals();
            assert_eq!(portals.len(), 1, "got {portals:?}");
            assert_eq!(portals[0].id, PortalId("github".to_string()));
            assert_eq!(portals[0].kind, PortalKind::GitHub);
            assert_eq!(portals[0].base_url, "https://github.com");
        }
    }
}

