//! Keeps GitHoot to one running copy per user on macOS.
//!
//! **Why a lock file and not LaunchServices.** LaunchServices refuses a second copy only when it is
//! the one doing the launch. The Launch Agent has `launchd` run the inner binary directly, which never
//! asks LaunchServices, and macOS's own "Reopen windows when logging back in" relaunches through
//! LaunchServices at the same moment. Each path alone starts one copy; together they start several,
//! and every copy then races the others for the single-use refresh token, so all but one end up
//! signed out. A lock taken by the process itself holds whichever launcher fired.
//!
//! **Why `try_lock`.** The OS releases the lock when the process exits, crashes included, so a stale
//! file can never keep the app from starting. The file's content is irrelevant; only the lock is.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// The lock file's name inside `~/.githoot`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const LOCK_FILE: &str = "instance.lock";

/// Proof that this process is the one running copy. Held for the life of the process: dropping it
/// releases the lock.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Guard {
    _file: File,
}

/// Takes the single-instance lock in `dir`.
///
/// `Ok(None)` means another copy holds it. `Err` means the lock could not even be attempted (an
/// unwritable directory, a file system without locks): the caller should start anyway, because no
/// tray icon at all is a worse failure than a possible duplicate.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn acquire(dir: &Path) -> Result<Option<Guard>, String> {
    let path = dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Guard { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(format!("could not lock {}: {e}", path.display())),
    }
}

/// One log line saying who started this copy, so a report of duplicates carries its own cause.
///
/// On macOS `XPC_SERVICE_NAME` names the launcher: our Launch Agent's label for a login start, an
/// `application.…` name for anything LaunchServices opened (Finder, `open`, an update's restart,
/// macOS reopening apps at login). Other platforms leave it unset, and the line still has the PIDs.
pub fn describe_launch(pid: u32, parent: Option<u32>, xpc_service: Option<&str>, exe: &Path) -> String {
    let parent = parent.map_or("unknown".to_string(), |p| p.to_string());
    format!(
        "started: pid {pid}, parent {parent}, by {} (XPC_SERVICE_NAME={}), from {}",
        launcher(xpc_service),
        xpc_service.unwrap_or("unset"),
        exe.display()
    )
}

/// [`describe_launch`] for this process.
pub fn launch_line() -> String {
    #[cfg(unix)]
    let parent = Some(std::os::unix::process::parent_id());
    #[cfg(not(unix))]
    let parent = None;
    let xpc_service = std::env::var("XPC_SERVICE_NAME").ok();
    let exe = std::env::current_exe().unwrap_or_default();
    describe_launch(std::process::id(), parent, xpc_service.as_deref(), &exe)
}

/// Which launcher an `XPC_SERVICE_NAME` value points to.
///
/// `com.githoot.` is the prefix of the Launch Agent label `autostart` writes. Matched as a prefix so a
/// label from an older release still reads as ours.
fn launcher(xpc_service: Option<&str>) -> &'static str {
    match xpc_service {
        Some(s) if s.starts_with("com.githoot.") => "Launch Agent",
        Some(s) if s.starts_with("application.") => "LaunchServices",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_start_names_the_launch_agent() {
        assert_eq!(launcher(Some("com.githoot.GitHoot")), "Launch Agent");
    }

    #[test]
    fn a_launchservices_start_is_named_as_such() {
        assert_eq!(launcher(Some("application.com.github.herrderb.githoot.1234.5678")), "LaunchServices");
    }

    #[test]
    fn an_unset_or_unrecognised_service_reads_as_unknown() {
        assert_eq!(launcher(None), "unknown");
        assert_eq!(launcher(Some("0")), "unknown");
    }

    #[test]
    fn the_line_carries_everything_a_report_needs() {
        let line = describe_launch(
            42,
            Some(1),
            Some("com.githoot.GitHoot"),
            Path::new("/Applications/githoot.app/Contents/MacOS/githoot"),
        );
        for part in ["pid 42", "parent 1", "Launch Agent", "com.githoot.GitHoot", "/Applications/githoot.app"] {
            assert!(line.contains(part), "missing {part:?} in {line:?}");
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("githoot-single-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn the_first_copy_gets_the_lock() {
        let dir = scratch("first");
        assert!(acquire(&dir).expect("the lock must be attemptable").is_some());
    }

    /// The bug itself: a second launcher firing while one copy runs must be turned away.
    #[test]
    fn a_second_copy_is_turned_away_while_the_first_runs() {
        let dir = scratch("second");
        let _first = acquire(&dir).unwrap().expect("the first copy must get the lock");
        assert!(acquire(&dir).unwrap().is_none(), "a second copy must not get the lock");
    }

    /// The update hand-over: the successor starts only once its predecessor has gone, and must then
    /// get the lock, or every update would end with no tray icon.
    #[test]
    fn the_lock_is_free_again_once_its_holder_is_gone() {
        let dir = scratch("released");
        let first = acquire(&dir).unwrap().expect("the first copy must get the lock");
        drop(first);
        assert!(acquire(&dir).unwrap().is_some(), "the successor must get the lock");
    }

    #[test]
    fn a_missing_directory_is_an_error_not_a_refusal() {
        let dir = scratch("missing").join("does-not-exist");
        assert!(acquire(&dir).is_err(), "no directory must read as 'could not try', not 'taken'");
    }
}
