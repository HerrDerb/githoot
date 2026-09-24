//! The dispatcher's prompts: what an agent is told, per bar.
//!
//! Four files you own, under GitHoot's own directory. They outlived the shipped scripts that used
//! to read them, because nothing about a prompt was ever shell-specific, and they keep the promise
//! `config.txt` makes: **a file you have edited is never rewritten.**
//!
//! Beside them sits `.shipped`, one sha-256 per prompt of the text GitHoot last wrote there. On an
//! update a prompt that still hashes to that value has not been touched and is brought up to the
//! new default; anything else is yours and is left alone, and the settings page names the ones it
//! kept. A digest rather than a copy, because the only question is "are these still our words",
//! and a hash cannot be mistaken for a second prompt to edit.
//!
//! Clearing a box on the settings page is the reset: the default is written back *and recorded as
//! ours*, so a prompt you reset starts following future defaults again.

use std::path::{Path, PathBuf};





/// The prompt files the dispatcher reads, and what each ships as. One per bar, plus what a
/// nudge says. The script carries copies of these as its fallback for a hand install; a test
/// below fails if the two ever drift.
pub const DEFAULT_PROMPTS: [(&str, &str); 4] = [
    ("work-required", include_str!("../contrib/prompts/work-required.txt")),
    ("requested-reviews", include_str!("../contrib/prompts/requested-reviews.txt")),
    ("approved", include_str!("../contrib/prompts/approved.txt")),
    ("update", include_str!("../contrib/prompts/update.txt")),
];



/// `home` is GitHoot's own directory, the one holding `config.txt`, not a home directory.
fn prompts_dir(home: &Path) -> PathBuf {
    home.join("prompts")
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
/// One prompt's text: yours if you have edited it, otherwise the shipped default.
///
/// Never fails and never writes. A pass must not be able to fall over because a prompt file was
/// deleted between two ticks, and the shipped default is always a correct answer to "what should
/// this agent be told".
pub fn text(home: &Path, name: &str) -> String {
    std::fs::read_to_string(prompt_path(home, name)).unwrap_or_else(|_| {
        DEFAULT_PROMPTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, default)| default.to_string())
            .unwrap_or_default()
    })
}

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
pub fn refresh_defaults(home: &Path) -> std::io::Result<Vec<&'static str>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt;

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-dispatcher-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
        let kept = refresh_defaults(&home).unwrap();
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
        refresh_defaults(&home).unwrap();
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

        let kept = refresh_defaults(&home).unwrap();
        assert_eq!(kept, vec!["update"]);
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "approved")).unwrap(), DEFAULT_PROMPTS[2].1, "untouched: refreshed");
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "update")).unwrap(), "my own words {url}\n", "edited: kept");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A prompt saved from the page is yours from then on, even at the next Update.
    #[test]
    fn a_prompt_saved_from_the_page_survives_an_update() {
        let home = temp_home("saved-survives");
        refresh_defaults(&home).unwrap();
        save_prompts(&home, &[("approved".to_string(), "tuned by hand {url}".to_string())]).unwrap();
        assert_eq!(refresh_defaults(&home).unwrap(), vec!["approved"]);
        assert_eq!(std::fs::read_to_string(prompt_path(&home, "approved")).unwrap(), "tuned by hand {url}\n");
        let _ = std::fs::remove_dir_all(&home);
    }

    // ── One script, two platforms ────────────────────────────────────────────

        // ── Windows: building the task ───────────────────────────────────────────

}
