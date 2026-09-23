//! Installs the shipped dispatcher from the settings page. Linux only.
//!
//! The three files under `contrib/` are compiled in, so the dispatcher a user installs is always
//! the one built against this binary's API, and `cargo test` can refuse to ship a script that does
//! not parse. Installing writes them under the home directory and enables a user service; nothing
//! here runs the dispatcher itself, and GitHoot still never executes anything on its own schedule.
//!
//! **This is the one place the settings page writes an executable.** What bounds it: the content is
//! these constants and the page cannot choose it; the paths are fixed; the route answers `404` unless
//! `localApi` is on; it demands the same same-origin `Origin` as saving settings; and this module
//! does not exist in the binary on any other platform. `contrib/README.md` says the same to users.
//!
//! Preflight refuses to write while a tool the script needs is missing, because an installed
//! dispatcher that fails every five seconds is worse than a page that says what to install first.

use std::io::Write;
use std::path::{Path, PathBuf};

pub const SCRIPT: &str = include_str!("../contrib/ght-dispatch");
pub const LOOP: &str = include_str!("../contrib/ght-dispatch-loop");
pub const UNIT: &str = include_str!("../contrib/ght-dispatch.service");

/// The prompt files the dispatcher reads, and what each ships as. One per bar, plus what a
/// nudge says. The script carries copies of these as its fallback for a hand install; a test
/// below fails if the two ever drift.
pub const DEFAULT_PROMPTS: [(&str, &str); 4] = [
    ("work-required", include_str!("../contrib/prompts/work-required.txt")),
    ("requested-reviews", include_str!("../contrib/prompts/requested-reviews.txt")),
    ("approved", include_str!("../contrib/prompts/approved.txt")),
    ("update", include_str!("../contrib/prompts/update.txt")),
];

/// Every binary the script calls. Checked here before installing, and again by the script itself.
pub const REQUIRED: [&str; 5] = ["herdr", "gh", "jq", "git", "curl"];

/// The marker the script carries so `status` can tell an old install from the shipped one.
const VERSION_MARKER: &str = "shipped with GitHoot ";
const VERSION_PLACEHOLDER: &str = "@GHT_VERSION@";

fn bin_dir(home: &Path) -> PathBuf {
    home.join(".local/bin")
}
fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}
fn script_path(home: &Path) -> PathBuf {
    bin_dir(home).join("ght-dispatch")
}
fn loop_path(home: &Path) -> PathBuf {
    bin_dir(home).join("ght-dispatch-loop")
}
fn unit_path(home: &Path) -> PathBuf {
    unit_dir(home).join("ght-dispatch.service")
}
fn prompts_dir(home: &Path) -> PathBuf {
    home.join(".config/ght-dispatch/prompts")
}
fn prompt_path(home: &Path, name: &str) -> PathBuf {
    prompts_dir(home).join(format!("{name}.txt"))
}

/// One prompt as the settings page shows it: the file if you have one, else what would be written.
#[derive(Debug, PartialEq, Eq)]
pub struct Prompt {
    pub name: &'static str,
    pub text: String,
    /// No file of yours exists; `text` is the shipped default.
    pub is_default: bool,
}

pub fn prompts(home: &Path) -> Vec<Prompt> {
    DEFAULT_PROMPTS
        .iter()
        .map(|(name, default)| match std::fs::read_to_string(prompt_path(home, name)) {
            Ok(text) => Prompt { name, is_default: text == *default, text },
            Err(_) => Prompt { name, text: default.to_string(), is_default: true },
        })
        .collect()
}

/// Saves the prompts the settings page posted. Only the four known names are ever written, so a
/// hand-made form cannot name a file. Line endings are normalised, because a browser sends CRLF.
/// **An emptied box is the reset**: the shipped default is written back and recorded as ours, so
/// a later Update keeps it current.
pub fn save_prompts(home: &Path, given: &[(String, String)]) -> std::io::Result<()> {
    std::fs::create_dir_all(prompts_dir(home))?;
    let mut shipped = read_shipped(home);
    for (name, default) in DEFAULT_PROMPTS {
        let Some((_, text)) = given.iter().find(|(n, _)| n == name) else { continue };
        let text = text.replace("\r\n", "\n");
        if text.trim().is_empty() {
            std::fs::write(prompt_path(home, name), default)?;
            shipped.insert(name.to_string(), digest(default));
        } else {
            std::fs::write(prompt_path(home, name), format!("{}\n", text.trim_end()))?;
        }
    }
    write_shipped(home, &shipped)
}

// ── Keeping untouched prompts current ────────────────────────────────────────
//
// Beside the prompts sits `.shipped`: one line per prompt, the sha-256 of the text this module last
// wrote there. On Update, a prompt whose file still hashes to that value has not been touched by
// you, so it is replaced with the new default. Anything else is yours and is left alone. A digest
// rather than a copy, because the point is only to recognise our own words, and a hash cannot be
// mistaken for a second prompt to edit.
//
// No history of older defaults is needed: 2.3.0 is the first release that ships prompts, so every
// install that has them also has `.shipped` from the start.

fn shipped_path(home: &Path) -> PathBuf {
    prompts_dir(home).join(".shipped")
}

fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn read_shipped(home: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(shipped_path(home))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(n, h)| (n.to_string(), h.to_string())))
        .collect()
}

fn write_shipped(home: &Path, shipped: &std::collections::BTreeMap<String, String>) -> std::io::Result<()> {
    let text: String = shipped.iter().map(|(n, h)| format!("{n}\t{h}\n")).collect();
    std::fs::write(shipped_path(home), text)
}

/// Brings each prompt up to the shipped default unless you have edited it. Returns the names kept
/// because they are yours, so the page can say so rather than let an Update look like it ignored
/// a new default.
fn install_default_prompts(home: &Path) -> std::io::Result<Vec<&'static str>> {
    std::fs::create_dir_all(prompts_dir(home))?;
    let mut shipped = read_shipped(home);
    let mut kept = Vec::new();
    for (name, default) in DEFAULT_PROMPTS {
        let path = prompt_path(home, name);
        let ours = match std::fs::read_to_string(&path) {
            Err(_) => true,
            Ok(current) => current == default || shipped.get(name).is_some_and(|h| *h == digest(&current)),
        };
        if ours {
            std::fs::write(&path, default)?;
            shipped.insert(name.to_string(), digest(default));
        } else {
            kept.push(name);
        }
    }
    write_shipped(home, &shipped)?;
    Ok(kept)
}

/// What the page shows before offering the button.
#[derive(Debug, PartialEq, Eq)]
pub struct Preflight {
    /// Tools from `REQUIRED` found nowhere on `PATH` nor in the usual per-user and system dirs.
    pub missing: Vec<&'static str>,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
    }
}

/// The directories the installed service searches, and therefore the only ones preflight may.
///
/// Must match the `PATH=` line in `contrib/ght-dispatch.service`; a test holds the two together.
/// Deliberately **not** GitHoot's own `PATH`: a tool found only there (say `~/.cargo/bin/herdr`)
/// would pass the check and then be invisible to the service, which is the worst of both.
fn service_dirs(home: &Path) -> [PathBuf; 4] {
    [bin_dir(home), PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
}

/// Looks for each required tool where the service will look, and nowhere else.
pub fn preflight(home: &Path) -> Preflight {
    let dirs = service_dirs(home);
    let missing = REQUIRED
        .into_iter()
        .filter(|bin| !dirs.iter().any(|d| d.join(bin).is_file()))
        .collect();
    Preflight { missing }
}

/// How the install stands right now, read from disk and from the user's service manager.
#[derive(Debug, PartialEq, Eq)]
pub struct Status {
    /// The GitHoot version the installed script came from, or `None` when nothing is installed.
    pub installed: Option<String>,
    /// The version this binary would install.
    pub shipped: &'static str,
    pub service_active: bool,
}

impl Status {
    /// The installed script came from a different GitHoot than this one.
    pub fn is_outdated(&self) -> bool {
        self.installed.as_deref().is_some_and(|v| v != self.shipped)
    }
}

pub fn status(home: &Path) -> Status {
    Status {
        installed: installed_version(&script_path(home)),
        shipped: crate::version::VERSION,
        service_active: systemctl(["is-active", "--quiet", "ght-dispatch.service"]).is_ok(),
    }
}

fn installed_version(script: &Path) -> Option<String> {
    let text = std::fs::read_to_string(script).ok()?;
    text.lines()
        .find_map(|l| l.split_once(VERSION_MARKER).map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
}

fn stamp(text: &str) -> String {
    text.replace(VERSION_PLACEHOLDER, crate::version::VERSION)
}

/// Writes the three files and brings untouched prompts current. Pure file I/O, no service
/// manager, so it can be tested in a temp home. Returns the prompts kept because they are yours.
pub fn install_files(home: &Path) -> std::io::Result<Vec<&'static str>> {
    std::fs::create_dir_all(bin_dir(home))?;
    std::fs::create_dir_all(unit_dir(home))?;
    write_executable(&script_path(home), &stamp(SCRIPT))?;
    write_executable(&loop_path(home), &stamp(LOOP))?;
    std::fs::write(unit_path(home), UNIT)?;
    install_default_prompts(home)
}

/// Removes the three files. Nothing there is not an error: the outcome is the same.
pub fn remove_files(home: &Path) -> std::io::Result<()> {
    for path in [script_path(home), loop_path(home), unit_path(home)] {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

fn write_executable(path: &Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .and_then(|mut f| f.write_all(content.as_bytes()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// The button. Preflight, write, then hand the unit to the user's service manager.
///
/// `Err` is the sentence the page shows. Preflight failing writes nothing at all.
pub fn install(home: &Path) -> Result<Vec<&'static str>, String> {
    let check = preflight(home);
    if !check.ok() {
        return Err(format!("missing: {}", check.missing.join(", ")));
    }
    let kept = install_files(home).map_err(|e| format!("could not write the dispatcher files: {e}"))?;
    systemctl(["daemon-reload"])?;
    // `restart`, not just `enable --now`: on an Update the loop is already running the old script
    // and must pick up the new one.
    systemctl(["enable", "ght-dispatch.service"])?;
    systemctl(["restart", "ght-dispatch.service"])?;
    Ok(kept)
}

/// The other button. Stop and disable first, so no tick runs against files that are going away.
pub fn uninstall(home: &Path) -> Result<(), String> {
    // A unit that was never enabled makes this fail, and that is fine: the files are what matter.
    let _ = systemctl(["disable", "--now", "ght-dispatch.service"]);
    remove_files(home).map_err(|e| format!("could not remove the dispatcher files: {e}"))?;
    let _ = systemctl(["daemon-reload"]);
    Ok(())
}

fn systemctl<const N: usize>(args: [&str; N]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl --user {}: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-dispatcher-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The strongest guard this module can offer: a script that does not parse never ships. Runs
    /// bash itself, because nothing short of the real parser is the real parser.
    #[test]
    fn the_shipped_scripts_parse() {
        for (name, text) in [("ght-dispatch", SCRIPT), ("ght-dispatch-loop", LOOP)] {
            let path = temp_home("parse").join(name);
            std::fs::write(&path, text).unwrap();
            let out = std::process::Command::new("bash").arg("-n").arg(&path).output().unwrap();
            assert!(out.status.success(), "{name} does not parse: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    /// The one line in the script that must never be deleted. Without it an outage reads as an
    /// empty board and the dispatcher goes quiet for exactly as long as GitHub is down.
    #[test]
    fn the_shipped_script_keeps_the_honesty_gate() {
        assert!(SCRIPT.contains("jq -r .known"), "the known gate is gone from ght-dispatch");
    }

    /// The script carries copies of the shipped prompts as its fallback for a hand install. Two
    /// copies of anything drift, so this reads each heredoc back out of the script and holds it
    /// against the file the settings page and the install button use.
    #[test]
    fn the_script_defaults_match_the_shipped_prompt_files() {
        for (name, file) in DEFAULT_PROMPTS {
            let start_marker = format!("    {name}) cat <<'EOF'\n");
            let start = SCRIPT.find(&start_marker).unwrap_or_else(|| panic!("no heredoc for {name}")) + start_marker.len();
            let end = SCRIPT[start..].find("\nEOF\n").expect("heredoc end") + start;
            assert_eq!(&SCRIPT[start..end], file.trim_end_matches('\n'), "{name}: script and contrib/prompts differ");
        }
    }

    /// Every default asks the agent to end with the link, so the pane always closes on something
    /// clickable, and the data block is labelled as data wherever it appears.
    #[test]
    fn every_default_prompt_ends_on_the_link_and_labels_its_data() {
        for (name, text) in DEFAULT_PROMPTS {
            assert!(text.trim_end().ends_with("{url}"), "{name} must end with the link");
            if text.contains("{title}") {
                assert!(text.contains("not instructions"), "{name} carries untrusted fields without saying so");
            }
        }
    }

    /// Prompts a user has not touched show as the default; a saved one shows as theirs; an emptied
    /// box takes them back to the default by removing the file rather than writing an empty one.
    #[test]
    fn saving_and_clearing_a_prompt_round_trips_through_the_file() {
        let home = temp_home("prompts");
        let before = prompts(&home);
        assert!(before.iter().all(|p| p.is_default));
        assert_eq!(before[0].text, DEFAULT_PROMPTS[0].1);

        save_prompts(&home, &[("approved".to_string(), "Line one\r\nLine two  \r\n".to_string())]).unwrap();
        let after = prompts(&home);
        let approved = after.iter().find(|p| p.name == "approved").unwrap();
        assert!(!approved.is_default);
        assert_eq!(approved.text, "Line one\nLine two\n", "CRLF normalised, trailing space dropped, one newline kept");
        assert!(after.iter().filter(|p| p.name != "approved").all(|p| p.is_default), "only the named prompt changed");

        save_prompts(&home, &[("approved".to_string(), "   \r\n".to_string())]).unwrap();
        assert!(prompts(&home).iter().all(|p| p.is_default), "an emptied box resets to the default");
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "approved")).unwrap(), DEFAULT_PROMPTS[2].1);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A name the form invents is ignored, never written: the four filenames are fixed.
    #[test]
    fn an_unknown_prompt_name_is_never_written() {
        let home = temp_home("unknown-prompt");
        save_prompts(&home, &[("../../evil".to_string(), "x".to_string()), ("nope".to_string(), "y".to_string())]).unwrap();
        assert!(!home.join("evil").exists() && !prompt_path(&home, "nope").exists());
        assert!(prompts(&home).iter().all(|p| p.is_default));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Install writes the prompts you lack and leaves the ones you have alone.
    #[test]
    fn install_writes_default_prompts_only_where_none_exist() {
        let home = temp_home("install-prompts");
        std::fs::create_dir_all(prompts_dir(&home)).unwrap();
        std::fs::write(prompt_path(&home, "update"), "mine\n").unwrap();
        let kept = install_files(&home).unwrap();
        assert_eq!(kept, vec!["update"]);
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "update")).unwrap(), "mine\n");
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "approved")).unwrap(), DEFAULT_PROMPTS[2].1);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The point of `.shipped`: a prompt we wrote and you never touched follows the default when
    /// it changes; one you edited stays yours across the same Update.
    #[test]
    fn an_update_refreshes_untouched_prompts_and_keeps_edited_ones() {
        let home = temp_home("refresh");
        install_files(&home).unwrap();
        // Pretend an older GitHoot wrote these two: file text and recorded hash both the old default.
        let old = "an older default {url}\n";
        let mut shipped = read_shipped(&home);
        for name in ["approved", "update"] {
            std::fs::write(prompt_path(&home, name), old).unwrap();
            shipped.insert(name.to_string(), digest(old));
        }
        write_shipped(&home, &shipped).unwrap();
        // Then you edit one of them.
        std::fs::write(prompt_path(&home, "update"), "my own words {url}\n").unwrap();

        let kept = install_files(&home).unwrap();
        assert_eq!(kept, vec!["update"]);
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "approved")).unwrap(), DEFAULT_PROMPTS[2].1, "untouched: refreshed");
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "update")).unwrap(), "my own words {url}\n", "edited: kept");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A prompt saved from the page is yours from then on, even at the next Update.
    #[test]
    fn a_prompt_saved_from_the_page_survives_an_update() {
        let home = temp_home("saved-survives");
        install_files(&home).unwrap();
        save_prompts(&home, &[("approved".to_string(), "tuned by hand {url}".to_string())]).unwrap();
        assert_eq!(install_files(&home).unwrap(), vec!["approved"]);
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "approved")).unwrap(), "tuned by hand {url}\n");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_files_writes_both_scripts_executable_and_the_unit() {
        let home = temp_home("install");
        install_files(&home).unwrap();
        for path in [script_path(&home), loop_path(&home)] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "{} must be executable", path.display());
        }
        assert_eq!(std::fs::read_to_string(unit_path(&home)).unwrap(), UNIT);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The installed script says which GitHoot it came from, so the page can offer an update when
    /// this binary is newer. The placeholder must not survive the write.
    #[test]
    fn the_shipped_version_is_stamped_and_read_back() {
        let home = temp_home("stamp");
        install_files(&home).unwrap();
        let written = std::fs::read_to_string(script_path(&home)).unwrap();
        assert!(!written.contains(VERSION_PLACEHOLDER));
        assert!(written.contains(crate::version::VERSION));
        assert_eq!(installed_version(&script_path(&home)).as_deref(), Some(crate::version::VERSION));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_installed_reads_as_none_not_as_an_empty_string() {
        let home = temp_home("none");
        assert_eq!(installed_version(&script_path(&home)), None);
    }

    #[test]
    fn an_older_install_is_reported_as_outdated() {
        let s = Status { installed: Some("2.2.0".into()), shipped: "2.3.0", service_active: false };
        assert!(s.installed.is_some() && s.is_outdated());
        let same = Status { installed: Some("2.3.0".into()), shipped: "2.3.0", service_active: true };
        assert!(!same.is_outdated());
        let none = Status { installed: None, shipped: "2.3.0", service_active: false };
        assert!(none.installed.is_none() && !none.is_outdated());
    }

    #[test]
    fn remove_files_leaves_nothing_behind_and_tolerates_absence() {
        let home = temp_home("remove");
        install_files(&home).unwrap();
        remove_files(&home).unwrap();
        assert!(!script_path(&home).exists() && !loop_path(&home).exists() && !unit_path(&home).exists());
        remove_files(&home).unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Preflight has to name what is missing, not just refuse. A bare home has no `~/.local/bin`,
    /// and no Linux box ships herdr in the system dirs.
    #[test]
    fn preflight_names_each_missing_tool() {
        let check = preflight(&temp_home("preflight"));
        assert!(check.missing.contains(&"herdr"), "herdr is never in /usr/bin: {:?}", check.missing);
        assert!(!check.ok());
    }

    /// A tool found through GitHoot's own `PATH` but not the service's must count as missing, or
    /// the card offers Install and the service then cannot find it.
    #[test]
    fn preflight_ignores_tools_the_service_cannot_see() {
        let home = temp_home("path-mismatch");
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("herdr"), "").unwrap();
        let saved = std::env::var_os("PATH");
        // SAFETY: restored below; no other test in this module reads PATH.
        unsafe { std::env::set_var("PATH", &elsewhere) };
        let check = preflight(&home);
        if let Some(p) = saved {
            unsafe { std::env::set_var("PATH", p) };
        }
        assert!(check.missing.contains(&"herdr"), "found via GitHoot's PATH only: {:?}", check.missing);
    }

    /// The dirs preflight searches and the unit's `PATH=` line are one list in two places.
    #[test]
    fn preflight_searches_exactly_the_service_path() {
        let line = UNIT.lines().find_map(|l| l.strip_prefix("Environment=PATH=")).expect("unit sets PATH");
        let home = Path::new("/home/x");
        let unit: Vec<PathBuf> = line.split(':').map(|d| PathBuf::from(d.replace("%h", "/home/x"))).collect();
        assert_eq!(unit, service_dirs(home).to_vec());
    }
}
