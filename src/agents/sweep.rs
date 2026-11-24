//! Deleting the worktrees of merged pull requests, when `agentsDeleteMerged` is on.
//!
//! **Driven by the disk, not by events.** Each pass lists the worktrees GitHoot made (a
//! `githoot/pr-<n>-<repo>` branch under the worktree root) and asks what became of the pull
//! requests no bar holds any more. Nothing has to be seen leaving a bar, so a merge while the app was
//! closed is caught too, and so is one that left as another arrived.
//!
//! **Asked on the poll thread.** The credential belongs to the poll thread, and a refresh from any other
//! could sign you out (see `portal::github::people`). So a pass only leaves its questions in
//! [`QUESTIONS`]; the poll thread asks the portal after its next poll and leaves the answers in
//! [`FATES`], for the pass after.
//!
//! **Open is asked again, closed is remembered.** A reviewed pull request leaves the bar while still
//! open, so open is asked about after every poll until it is merged or closed. Closed is kept in memory
//! until the app quits, so it is asked about once per run. One that comes back into a bar is forgotten,
//! and asked about again when it leaves. Nothing is written to disk.
//!
//! **GitHoot never deletes work.** A merged pull request's worktree is kept, with one line in the log,
//! when it holds uncommitted changes or commits that are not in the merged head, when a client did not
//! answer, or when its agent could not be closed.

use super::clients::{Client, Run};
use crate::portal::types::{PrFate, PrRef};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One of GitHoot's worktrees, as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// The clone it belongs to, where `git worktree remove` runs.
    pub clone: PathBuf,
    /// `githoot/pr-<n>-<repo>`.
    pub branch: String,
    pub pr: PrRef,
}

/// The pull request number in one of GitHoot's own branch names, `githoot/pr-<n>-<repo>`. `None` for
/// any other branch, which is never touched.
pub fn pr_number(branch: &str) -> Option<u64> {
    let rest = branch.strip_prefix("githoot/pr-")?;
    let (number, name) = rest.split_once('-')?;
    (!name.is_empty()).then_some(())?;
    number.parse().ok()
}

/// `(host, owner/name)` out of a clone's `origin`: `https://host/owner/name(.git)`,
/// `git@host:owner/name(.git)` or `ssh://git@host(:port)/owner/name(.git)`.
pub fn origin(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        (host.split(':').next()?, path)
    } else {
        // scp style: git@host:owner/name
        let (authority, path) = url.split_once(':')?;
        (authority.rsplit('@').next()?, path)
    };
    let repo = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = repo.split_once('/')?;
    (!host.is_empty() && !owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| (host.to_string(), repo.to_string()))
}

/// What a pass does with one worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Its pull request is in a bar, was closed, or its worktree was kept.
    Leave,
    /// Not known yet: ask the portal.
    Ask,
    /// Merged at `head`: close its agent and delete it, if nothing would be lost.
    Clean { head: String },
}

/// The decision, pure: whether its pull request is in a bar, what the portal said this run, and
/// whether its worktree was already kept for holding work.
pub fn next(in_bar: bool, fate: Option<&PrFate>, kept: bool) -> Next {
    match fate {
        _ if in_bar || kept => Next::Leave,
        // Open is not final: a reviewed pull request leaves the bar long before it is merged.
        None | Some(PrFate::Open) => Next::Ask,
        Some(PrFate::Merged { head }) => Next::Clean { head: head.clone() },
        Some(_) => Next::Leave,
    }
}

/// Why a merged pull request's worktree is still there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// It holds work. Not tried again this run.
    Work(String),
    /// Something that may pass: a client that did not answer, an agent not closed, a failed fetch.
    /// Tried again next pass.
    Retry(String),
}

/// Closes the agents in `f`'s worktree and deletes it and its branch, unless that would lose work.
///
/// The checks come before anything is closed, so an agent is never closed for a worktree that then
/// stays. They are repeated after, for whatever the agent did in between, and `git worktree remove`
/// without `--force` refuses a dirty one regardless.
pub fn clean(f: &Found, head: &str, clients: &[&dyn Client], run: &mut Run) -> Result<String, Kept> {
    let path = f.path.to_string_lossy().to_string();
    let clone = f.clone.to_string_lossy().to_string();

    // Who is in it, asked of every client before anything happens. One that does not answer may be
    // hiding the agent standing there.
    let mut inside = Vec::new();
    for client in clients {
        let agents = client.agents(run).map_err(|e| Kept::Retry(format!("{} did not answer ({e}), so nobody can say whether an agent is in it", client.name())))?;
        let want = super::dispatch::fold(&path);
        inside.extend(agents.into_iter().filter(|(cwd, _)| super::dispatch::fold(cwd) == want).map(|(_, name)| (*client, name)));
    }

    run("git", &["-C", &clone, "fetch", "--quiet", "origin", &format!("refs/pull/{}/head", f.pr.number)])
        .map_err(|e| Kept::Retry(format!("could not fetch the merged head: {e}")))?;
    holds_no_work(&path, head, run)?;

    for (client, name) in &inside {
        client.close(&path, name, run).map_err(|e| Kept::Retry(format!("its agent {name} was not closed: {e}")))?;
    }
    // Again, for whatever the agent did before it closed.
    holds_no_work(&path, head, run)?;

    run("git", &["-C", &clone, "worktree", "remove", &path]).map_err(|e| Kept::Retry(format!("git would not remove it: {e}")))?;
    let _ = run("git", &["-C", &clone, "branch", "-D", &f.branch]);
    let closed = if inside.is_empty() { "" } else { "closed its agent and " };
    Ok(format!("{}#{} was merged: {closed}deleted {path} and {}", f.pr.repo, f.pr.number, f.branch))
}

/// Uncommitted changes, or commits that are not in `head`, the merged head: work, which stays.
fn holds_no_work(path: &str, head: &str, run: &mut Run) -> Result<(), Kept> {
    let dirty = run("git", &["-C", path, "status", "--porcelain"]).map_err(|e| Kept::Retry(format!("could not read its status: {e}")))?;
    if !dirty.trim().is_empty() {
        return Err(Kept::Work("it holds uncommitted changes".to_string()));
    }
    let ahead = run("git", &["-C", path, "rev-list", "--count", &format!("{head}..HEAD")])
        .map_err(|e| Kept::Retry(format!("could not compare it with the merged head: {e}")))?;
    match ahead.trim() {
        "0" => Ok(()),
        n => Err(Kept::Work(format!("it holds {n} commit(s) that are not in the merged pull request"))),
    }
}

/// Its setting: off unless you switch it on.
pub const KEY: &str = "agentsDeleteMerged";

/// GitHoot's worktrees under `root`. What each one is never changes, so it is asked of git once per
/// path and remembered; a folder that is not one of them is looked at again next pass, since it may be
/// one being made.
pub fn find(root: &Path, run: &mut Run) -> Vec<Found> {
    static SEEN: std::sync::Mutex<BTreeMap<PathBuf, Found>> = std::sync::Mutex::new(BTreeMap::new());
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    let Ok(mut seen) = SEEN.lock() else { return Vec::new() };
    seen.retain(|path, _| dirs.contains(path));
    let mut out = Vec::new();
    for dir in dirs {
        if let Some(f) = seen.get(&dir) {
            out.push(f.clone());
        } else if let Some(f) = inspect(&dir, run) {
            seen.insert(dir, f.clone());
            out.push(f);
        }
    }
    out
}

/// What git says `path` is, if it is one of GitHoot's worktrees.
fn inspect(path: &Path, run: &mut Run) -> Option<Found> {
    let at = path.to_string_lossy();
    let branch = run("git", &["-C", &at, "rev-parse", "--abbrev-ref", "HEAD"]).ok()?.trim().to_string();
    let number = pr_number(&branch)?;
    let (host, repo) = origin(&run("git", &["-C", &at, "remote", "get-url", "origin"]).ok()?)?;
    let common = run("git", &["-C", &at, "rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?;
    let clone = Path::new(common.trim()).parent()?.to_path_buf();
    Some(Found { path: path.to_path_buf(), clone, branch, pr: PrRef { host, repo, number } })
}

/// One sweep: what it did and what it kept, as `(is_error, line)`. Its questions are left for the
/// poll thread. Nothing at all before any bar has a confirmed list, or the first pass after a start
/// would ask about every worktree there is.
pub fn pass(worktree_root: &Path, batches: &[super::Batch], clients: &[&dyn Client], run: &mut Run) -> Vec<(bool, String)> {
    static KEPT: std::sync::Mutex<BTreeSet<PrRef>> = std::sync::Mutex::new(BTreeSet::new());
    static SAID: std::sync::Mutex<BTreeMap<PrRef, String>> = std::sync::Mutex::new(BTreeMap::new());
    if !batches.iter().any(|b| b.confirmed) {
        return Vec::new();
    }
    let in_bars: BTreeSet<(String, u64)> = batches
        .iter()
        .flat_map(|b| &b.entries)
        .filter_map(|e| Some((e.repo.as_ref()?.to_lowercase(), e.number?)))
        .collect();

    let mut out = Vec::new();
    let mut ask = Vec::new();
    for f in find(worktree_root, run) {
        let in_bar = in_bars.contains(&(f.pr.repo.to_lowercase(), f.pr.number));
        if in_bar {
            // Back in a bar: whatever was said about it no longer holds.
            FATES.lock().map(|mut m| m.remove(&f.pr)).ok();
            KEPT.lock().map(|mut k| k.remove(&f.pr)).ok();
            SAID.lock().map(|mut m| m.remove(&f.pr)).ok();
        }
        // Copied out, so the poll thread is never kept waiting on git.
        let fate = FATES.lock().ok().and_then(|m| m.get(&f.pr).cloned());
        let kept = KEPT.lock().is_ok_and(|k| k.contains(&f.pr));
        match next(in_bar, fate.as_ref(), kept) {
            Next::Leave => {}
            Next::Ask => ask.push(f.pr.clone()),
            Next::Clean { head } => match clean(&f, &head, clients, run) {
                Ok(line) => {
                    FATES.lock().map(|mut m| m.remove(&f.pr)).ok();
                    out.push((false, line));
                }
                Err(Kept::Work(why)) => {
                    KEPT.lock().map(|mut k| k.insert(f.pr.clone())).ok();
                    out.push((true, format!("{}#{} was merged, but {} was kept: {why}", f.pr.repo, f.pr.number, f.path.display())));
                }
                Err(Kept::Retry(why)) => {
                    // Said once per reason, not every pass.
                    let new = SAID.lock().is_ok_and(|mut m| m.insert(f.pr.clone(), why.clone()).as_deref() != Some(why.as_str()));
                    if new {
                        out.push((true, format!("{}#{} was merged, but {} is not deleted yet: {why}. Tried again next pass", f.pr.repo, f.pr.number, f.path.display())));
                    }
                }
            },
        }
    }
    if let Ok(mut q) = QUESTIONS.lock() {
        *q = ask;
    }
    out
}

/// The pull requests a pass wants asked about. Taken by the poll thread.
static QUESTIONS: std::sync::Mutex<Vec<PrRef>> = std::sync::Mutex::new(Vec::new());
/// What the portals said, this run.
static FATES: std::sync::Mutex<BTreeMap<PrRef, PrFate>> = std::sync::Mutex::new(BTreeMap::new());

/// For the poll thread: the pull requests to ask about now. Each is handed out once; a pass asks again
/// for any that got no answer.
pub fn questions() -> Vec<PrRef> {
    QUESTIONS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// For the poll thread: what a portal answered.
pub fn answered(answers: Vec<(PrRef, PrFate)>) {
    if let Ok(mut fates) = FATES.lock() {
        fates.extend(answers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::clients::HerdrApp;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn found() -> Found {
        Found {
            path: PathBuf::from("/w/pr-7-web"),
            clone: PathBuf::from("/p/web"),
            branch: "githoot/pr-7-web".into(),
            pr: PrRef { host: "github.com".into(), repo: "acme/web".into(), number: 7 },
        }
    }

    type Ran = Rc<RefCell<Vec<String>>>;

    /// Replays an answer per command line and records every line run.
    fn recorder(answer: impl Fn(&str) -> Result<String, String>) -> (impl FnMut(&str, &[&str]) -> Result<String, String>, Ran) {
        let ran = Rc::new(RefCell::new(Vec::new()));
        let log = ran.clone();
        let run = move |exe: &str, args: &[&str]| {
            let line = format!("{exe} {}", args.join(" "));
            log.borrow_mut().push(line.clone());
            answer(&line)
        };
        (run, ran)
    }

    /// A worktree with nothing in it, merged at `abc`, and no agent anywhere.
    fn clean_world(line: &str) -> Result<String, String> {
        match line {
            l if l.starts_with("hc ls") => Ok(r#"{"terminals":[]}"#.into()),
            l if l.contains("rev-list --count") => Ok("0".into()),
            _ => Ok(String::new()),
        }
    }

    #[test]
    fn only_its_own_branches_name_a_pull_request() {
        assert_eq!(pr_number("githoot/pr-7-web"), Some(7));
        assert_eq!(pr_number("githoot/pr-1234-some-long-repo"), Some(1234));
        assert_eq!(pr_number("main"), None);
        assert_eq!(pr_number("feature/pr-7-web"), None);
        assert_eq!(pr_number("githoot/pr-x-web"), None);
    }

    #[test]
    fn the_repository_is_read_off_origin_in_every_shape() {
        let want = Some(("github.com".to_string(), "acme/web".to_string()));
        assert_eq!(origin("https://github.com/acme/web.git"), want);
        assert_eq!(origin("https://github.com/acme/web"), want);
        assert_eq!(origin("git@github.com:acme/web.git"), want);
        assert_eq!(origin("ssh://git@github.com/acme/web.git"), want);
        assert_eq!(origin("ssh://git@ghe.example.com:2222/acme/web.git"), Some(("ghe.example.com".to_string(), "acme/web".to_string())));
        assert_eq!(origin("/local/path"), None);
    }

    /// In a bar wins over everything: an open pull request is never asked about.
    #[test]
    fn a_pull_request_in_a_bar_is_left_alone() {
        assert_eq!(next(true, None, false), Next::Leave);
        assert_eq!(next(true, Some(&PrFate::Merged { head: "abc".into() }), false), Next::Leave);
    }

    #[test]
    fn one_not_asked_about_yet_is_asked() {
        assert_eq!(next(false, None, false), Next::Ask);
    }

    /// Closed or unknown is final for the run: asked once, answered once.
    #[test]
    fn a_closed_or_unknown_one_is_left() {
        for fate in [PrFate::Closed, PrFate::Unknown] {
            assert_eq!(next(false, Some(&fate), false), Next::Leave, "{fate:?}");
        }
    }

    /// Open is not final: a pull request you reviewed leaves the bar long before it is merged, so it
    /// is asked about again after every poll until it is merged or closed.
    #[test]
    fn an_open_one_is_asked_again() {
        assert_eq!(next(false, Some(&PrFate::Open), false), Next::Ask);
    }

    #[test]
    fn a_merged_one_is_cleaned_unless_it_was_kept() {
        let merged = PrFate::Merged { head: "abc".into() };
        assert_eq!(next(false, Some(&merged), false), Next::Clean { head: "abc".into() });
        assert_eq!(next(false, Some(&merged), true), Next::Leave);
    }

    /// The whole cleanup, in order: checks, fetch of the merged head, the checks again, then the
    /// worktree and its branch.
    #[test]
    fn a_clean_merged_worktree_is_deleted_with_its_branch() {
        let (mut run, ran) = recorder(clean_world);
        assert!(clean(&found(), "abc", &[&HerdrApp], &mut run).is_ok());
        let ran = ran.borrow();
        assert!(ran.contains(&"git -C /p/web fetch --quiet origin refs/pull/7/head".to_string()), "{ran:?}");
        assert!(ran.contains(&"git -C /w/pr-7-web rev-list --count abc..HEAD".to_string()), "{ran:?}");
        let remove = ran.iter().position(|l| l == "git -C /p/web worktree remove /w/pr-7-web").expect("removed");
        let branch = ran.iter().position(|l| l == "git -C /p/web branch -D githoot/pr-7-web").expect("branch deleted");
        assert!(remove < branch);
        assert!(!ran.iter().any(|l| l.contains("--force")), "never forced: {ran:?}");
    }

    #[test]
    fn uncommitted_changes_keep_it() {
        let (mut run, ran) = recorder(|l| if l.contains("status --porcelain") { Ok(" M src/x.rs".into()) } else { clean_world(l) });
        assert!(matches!(clean(&found(), "abc", &[&HerdrApp], &mut run), Err(Kept::Work(_))));
        assert!(!ran.borrow().iter().any(|l| l.contains("worktree remove")));
    }

    /// Commits the merged head does not hold are somebody's work, even with the pull request merged.
    #[test]
    fn commits_not_in_the_merged_head_keep_it() {
        let (mut run, ran) = recorder(|l| if l.contains("rev-list --count") { Ok("2".into()) } else { clean_world(l) });
        assert!(matches!(clean(&found(), "abc", &[&HerdrApp], &mut run), Err(Kept::Work(_))));
        assert!(!ran.borrow().iter().any(|l| l.contains("worktree remove")));
    }

    /// A client that did not answer may be hiding the agent in that worktree.
    #[test]
    fn a_client_that_did_not_answer_holds_it_for_a_retry() {
        let (mut run, ran) = recorder(|l| if l.starts_with("hc ls") { Err("timed out".into()) } else { clean_world(l) });
        assert!(matches!(clean(&found(), "abc", &[&HerdrApp], &mut run), Err(Kept::Retry(_))));
        assert!(!ran.borrow().iter().any(|l| l.contains("worktree remove")));
    }

    /// An agent in the worktree is closed first, and only then is the worktree removed.
    #[test]
    fn its_agent_is_closed_before_the_worktree_goes() {
        let (mut run, ran) = recorder(|l| {
            if l.starts_with("hc ls") {
                Ok(r#"{"terminals":[{"address":"t4","agent":"claude-code","cwd":"/w/pr-7-web","name":"pr-7-web","state":"live"}]}"#.into())
            } else {
                clean_world(l)
            }
        });
        assert!(clean(&found(), "abc", &[&HerdrApp], &mut run).is_ok());
        let ran = ran.borrow();
        let kill = ran.iter().position(|l| l == "hc kill pr-7-web").expect("closed");
        let remove = ran.iter().position(|l| l.contains("worktree remove")).expect("removed");
        assert!(kill < remove, "{ran:?}");
    }

    /// An agent that would not close keeps its worktree, to be tried again.
    #[test]
    fn an_agent_that_would_not_close_keeps_it() {
        let (mut run, ran) = recorder(|l| {
            if l.starts_with("hc ls") {
                Ok(r#"{"terminals":[{"address":"t4","agent":"claude-code","cwd":"/w/pr-7-web","name":"pr-7-web","state":"live"}]}"#.into())
            } else if l.starts_with("hc kill") {
                Err("refused".into())
            } else {
                clean_world(l)
            }
        });
        assert!(matches!(clean(&found(), "abc", &[&HerdrApp], &mut run), Err(Kept::Retry(_))));
        assert!(!ran.borrow().iter().any(|l| l.contains("worktree remove")));
    }

    /// A failed fetch says nothing about the work, so it is tried again rather than kept.
    #[test]
    fn a_failed_fetch_is_tried_again() {
        let (mut run, _) = recorder(|l| if l.contains(" fetch ") { Err("network down".into()) } else { clean_world(l) });
        assert!(matches!(clean(&found(), "abc", &[&HerdrApp], &mut run), Err(Kept::Retry(_))));
    }

    /// Checking happens before closing: an agent is never closed for a worktree that stays.
    #[test]
    fn work_is_found_before_any_agent_is_closed() {
        let (mut run, ran) = recorder(|l| {
            if l.starts_with("hc ls") {
                Ok(r#"{"terminals":[{"address":"t4","agent":"claude-code","cwd":"/w/pr-7-web","name":"pr-7-web","state":"live"}]}"#.into())
            } else if l.contains("status --porcelain") {
                Ok("?? notes.md".into())
            } else {
                clean_world(l)
            }
        });
        assert!(matches!(clean(&found(), "abc", &[&HerdrApp], &mut run), Err(Kept::Work(_))));
        assert!(!ran.borrow().iter().any(|l| l.starts_with("hc kill")), "the agent stays with its work");
    }
}
