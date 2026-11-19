//! The Agents log, `~/.githoot/agents/log.txt`: what it decided about each pull request, and every
//! program a start ran, with how it ended.
//!
//! Its own file, written whatever `logLevel` says. Agents is experimental, and "where is my agent?"
//! is answered by its routine lines, which the main log drops at the `error` default: a pull request
//! taken as seen when Agents was switched on left no trace at all. Errors still go to the main log as
//! well. Only real passes write here; a dry run is shown on the page and recorded nowhere.

use std::io::Write;
use std::path::{Path, PathBuf};

const FILE: &str = "log.txt";

/// Past this size the older half is dropped. Half rather than all, so the lines before whatever went
/// wrong are still there to read.
const MAX_BYTES: u64 = 512 * 1024;

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Appends `lines`, each with a UTC timestamp. Never fails the caller: a log that can stop a dispatch
/// is worse than none.
pub fn append(dir: &Path, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let file = path(dir);
    if std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        keep_newer_half(&file);
    }
    let stamp = crate::log::utc_now();
    if let Ok(mut out) = crate::log::open_log(&file) {
        for line in lines {
            let _ = writeln!(out, "{stamp} {line}");
        }
    }
}

fn keep_newer_half(file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else {
        let _ = std::fs::write(file, "");
        return;
    };
    let cut = text.len() / 2;
    let from = text[cut..].find('\n').map_or(text.len(), |n| cut + n + 1);
    let _ = std::fs::write(file, format!("-- older lines dropped --\n{}", &text[from..]));
}

/// The last `n` lines, oldest first.
pub fn tail(dir: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read_to_string(path(dir)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-journal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn lines_are_stamped_and_read_back_oldest_first() {
        let d = dir("stamp");
        append(&d, &["one".to_string(), "two".to_string()]);
        append(&d, &[]);
        append(&d, &["three".to_string()]);
        let tail = tail(&d, 2);
        assert_eq!(tail.len(), 2);
        assert!(tail[0].ends_with("Z two") && tail[1].ends_with("Z three"), "{tail:?}");
        assert!(tail[0].starts_with("20"), "a UTC timestamp first: {tail:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn no_log_is_no_lines() {
        assert!(tail(&dir("none"), 50).is_empty());
    }

    /// A full log keeps its newer half, so what led up to a failure is still there.
    #[test]
    fn a_full_log_keeps_its_newer_half() {
        let d = dir("full");
        std::fs::create_dir_all(&d).unwrap();
        let old: String = (0..80_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(path(&d), old).unwrap();
        append(&d, &["newest".to_string()]);
        let text = std::fs::read_to_string(path(&d)).unwrap();
        assert!(text.starts_with("-- older lines dropped --\n"));
        assert!(!text.contains("line 0\n") && text.contains("line 79999\n") && text.trim_end().ends_with("newest"));
        assert!((text.len() as u64) < MAX_BYTES);
        let _ = std::fs::remove_dir_all(&d);
    }
}
