//! The app is called `githoot`, and no tracked file may call it anything else.
//!
//! ## Why this test exists
//!
//! The app has been renamed twice, `git-system-tray` to `githoot-tray` to `githoot`, and each rename
//! touched some forty files: the crate, the binary, the release assets, the updater's repository, the
//! config directory, the autostart entry, the macOS bundle id, the Windows manifest, the env prefix,
//! the dispatcher's branch prefix, the docs. Review catches most of those, and "most" is the problem:
//! one stale literal in a User-Agent or a doc link is invisible until somebody trips over it.
//!
//! ## What is allowed
//!
//! Only lines that talk about the old names *as old names*: the upgrade notes that tell someone where
//! their settings used to live, the dispatcher's cleanup instructions for scripts an earlier version
//! installed, and one signed test fixture in the updater whose bytes cannot change without the key that signed it.

use std::process::Command;

/// Old names, matched case-insensitively anywhere in a line.
const RETIRED: [&str; 6] = [
    "githoot-tray",
    "githoot_tray",
    "githoot tray",
    "githoottray",
    "git-system-tray",
    "github-trayicon",
];

/// Old prefixes, matched case-sensitively and only at the start of a word, so `height_` or
/// `light/` do not count.
const RETIRED_PREFIXES: [&str; 3] = ["GHT_", "ght/", "ght-dispatch"];

/// `(file, retired name)` pairs where the old name is the subject of the line, not a leftover.
const ALLOWED: [(&str, &str); 8] = [
    // Upgrade notes: where settings, the binary and the autostart entry used to be.
    ("README.md", "githoot-tray"),
    ("docs/menu-and-settings.md", "githoottray"),
    ("docs/menu-and-settings.md", "githoot-tray"),
    ("docs/menu-and-settings.md", "git-system-tray"),
    ("docs/menu-and-settings.md", "github-trayicon"),
    // Scripts an earlier dispatcher installed, named so they can be removed.
    ("docs/dispatcher.md", "ght-dispatch"),
    ("contrib/README.md", "ght-dispatch"),
    // The signed fixture in the updater's tests. See the comment on `FIXTURE_SUMS`.
    ("src/update.rs", "git-system-tray"),
];

/// Files that must name the old names in order to test for them.
const SELF: [&str; 2] = ["tests/naming.rs", "tests/release_assets.rs"];

fn tracked_files() -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git must be available to list tracked files");
    assert!(out.status.success(), "git ls-files failed");
    String::from_utf8(out.stdout)
        .expect("file names must be UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn starts_a_word(line: &str, at: usize) -> bool {
    line[..at].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
}

/// Every retired name or prefix found in `line`.
fn retired_in(line: &str) -> Vec<&'static str> {
    let lower = line.to_ascii_lowercase();
    let mut found: Vec<&'static str> = RETIRED.into_iter().filter(|n| lower.contains(n)).collect();
    for prefix in RETIRED_PREFIXES {
        if line.match_indices(prefix).any(|(at, _)| starts_a_word(line, at)) {
            found.push(prefix);
        }
    }
    found
}

#[test]
fn no_tracked_file_uses_a_retired_name() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offences = Vec::new();

    for path in tracked_files() {
        if SELF.contains(&path.as_str()) || path == "Cargo.lock" {
            continue;
        }
        if !retired_in(&path).is_empty() {
            offences.push(format!("{path}: the file name itself"));
        }
        // Binary files (icons, the hoot) are skipped: they carry no names.
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else { continue };
        for (number, line) in text.lines().enumerate() {
            for name in retired_in(line) {
                if !ALLOWED.contains(&(path.as_str(), name)) {
                    offences.push(format!("{path}:{}: `{name}` in {:?}", number + 1, line.trim()));
                }
            }
        }
    }

    assert!(offences.is_empty(), "retired names still in use:\n{}", offences.join("\n"));
}

/// The matcher itself, so a passing scan means "nothing there" and not "nothing matched".
#[test]
fn the_matcher_finds_what_it_is_meant_to() {
    assert_eq!(retired_in("see ~/.GitHoot-Tray/config.txt"), ["githoot-tray"]);
    assert_eq!(retired_in("GHT_VERSION: v1"), ["GHT_"]);
    assert_eq!(retired_in("export GHT_VERSION"), ["GHT_"]);
    assert_eq!(retired_in("cut ght/pr-1-repo"), ["ght/"]);
    assert!(retired_in("RIGHT_EDGE, height/2, light/dark").is_empty());
    assert!(retired_in("githoot, GitHoot, githoot.exe, GITHOOT_VERSION").is_empty());
}
