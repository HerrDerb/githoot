//! Herdr in a terminal: the `herdr` server the TUI attaches to. What the dispatcher always drove.
//!
//! When no Herdr server is running, the first dispatch starts one headless (`herdr server`), as `hc`
//! starts its core for the Herdr app: the agent's workspace is there the next time you open `herdr`.
//!
//! Herdr makes the worktree and a workspace for it in one step, and remembers it, which is why this
//! client can reopen a worktree it still knows instead of recycling it. The agent starts in the
//! workspace's first pane and is prompted once it is ready; see `hand_over` for why that takes care.

use super::{Client, Launch, NewWorktree, Run};
use crate::agents::dispatch::{hand_over, json_string, Trouble};

#[derive(Debug)]
pub struct HerdrTui;

impl Client for HerdrTui {
    fn id(&self) -> &'static str {
        "herdr"
    }
    fn name(&self) -> &'static str {
        "Herdr"
    }
    fn tool(&self) -> &'static str {
        "herdr"
    }
    fn install_url(&self) -> &'static str {
        "https://herdr.dev/docs/install/"
    }
    fn note(&self) -> &'static str {
        "Each agent opens as a workspace of its own in the Herdr you have running in a terminal."
    }
    /// `server_not_running` is Herdr's own code for no server, part of its JSON error contract.
    fn ready(&self, run: &mut Run) -> Result<(), Trouble> {
        ensure_server(run, &mut || std::thread::sleep(SERVER_POLL))
    }

    fn agents(&self, run: &mut Run) -> Result<Vec<(String, String)>, String> {
        match run("herdr", &["agent", "list"]) {
            Ok(raw) => Ok(super::named_agents(&raw)),
            Err(e) if super::not_installed(&e) || e.contains("server_not_running") => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn reopen(&self, worktree: &str, run: &mut Run) -> Option<String> {
        let opened = run("herdr", &["worktree", "open", "--path", worktree, "--no-focus"]).ok()?;
        json_string(&opened, "workspace_id").filter(|w| !w.is_empty())
    }

    fn create(&self, w: &NewWorktree, run: &mut Run) -> Result<String, Trouble> {
        let created = run(
            "herdr",
            &[
                "worktree", "create",
                "--cwd", w.clone,
                "--path", w.path,
                "--branch", w.branch,
                "--base", w.base,
                "--label", w.slug,
                "--no-focus",
                "--trust-repository",
            ],
        )
        .map_err(|e| Trouble::Passing(format!("could not create the worktree: {e}")))?;
        json_string(&created, "workspace_id").ok_or_else(|| Trouble::Passing("could not create the worktree: no workspace came back".to_string()))
    }

    fn start(&self, l: &Launch, run: &mut Run) -> Result<(), Trouble> {
        let panes = run("herdr", &["pane", "list", "--workspace", l.place])
            .map_err(|e| Trouble::Passing(format!("could not list panes: {e}")))?;
        let pane = json_string(&panes, "pane_id").ok_or_else(|| Trouble::Passing(format!("no pane in {}", l.place)))?;
        hand_over(l.slug, l.kind, &pane, l.prompt, &mut |args| run("herdr", args))
    }
}

/// How often, and how many times, to look for a server just started. Ten seconds in all.
const SERVER_POLL: std::time::Duration = std::time::Duration::from_millis(500);
const SERVER_TRIES: usize = 20;

/// Starts the Herdr server when none is running, the way `hc` starts its core, and waits until it
/// answers. Not running is Herdr's own `server_not_running`, part of its JSON error contract;
/// anything else is not this function's business and is left to the call that meets it.
pub fn ensure_server(run: &mut Run, pause: &mut dyn FnMut()) -> Result<(), Trouble> {
    let not_running = |answer: &Result<String, String>| answer.as_ref().err().is_some_and(|e| e.contains("server_not_running"));
    if !not_running(&run("herdr", &["agent", "list"])) {
        return Ok(());
    }
    // Headless: it hands itself to the background and the launcher returns. Opening `herdr` later
    // attaches to it, with the agent's workspace already there. What it says is not a verdict: the
    // next question is.
    let started = run("herdr", &["server"]);
    for _ in 0..SERVER_TRIES {
        pause();
        if !not_running(&run("herdr", &["agent", "list"])) {
            return Ok(());
        }
    }
    Err(Trouble::Setup(format!(
        "Herdr is not running, and starting its server did not help{}. Start Herdr, and the next pass picks this up",
        started.err().map(|e| format!(" ({e})")).unwrap_or_default()
    )))
}

#[cfg(test)]
mod tests {
    use super::super::tests::recorder;
    use super::*;

    /// The same commands the dispatcher ran before there was a choice, in the same order.
    #[test]
    fn it_makes_a_workspace_with_the_worktree_and_starts_in_its_first_pane() {
        let (mut run, ran) = recorder(|line| match line {
            l if l.starts_with("herdr worktree create") => Ok(r#"{"result":{"workspace_id":"w7"}}"#.into()),
            l if l.starts_with("herdr pane list") => Ok(r#"{"result":{"panes":[{"pane_id":"w7:p1"}]}}"#.into()),
            _ => Ok(String::new()),
        });
        let w = NewWorktree { clone: "/p/x", path: "/w/pr-1-x", branch: "githoot/pr-1-x", base: "refs/githoot-pr/pr-1-x", slug: "pr-1-x" };
        let place = HerdrTui.create(&w, &mut run).unwrap();
        assert_eq!(place, "w7");
        let l = Launch { slug: "pr-1-x", kind: "claude", prompt: "Review it", worktree: "/w/pr-1-x", place: &place };
        assert_eq!(HerdrTui.start(&l, &mut run), Ok(()));
        let ran = ran.borrow();
        assert_eq!(
            ran[0],
            "herdr worktree create --cwd /p/x --path /w/pr-1-x --branch githoot/pr-1-x --base refs/githoot-pr/pr-1-x --label pr-1-x --no-focus --trust-repository"
        );
        assert_eq!(ran[1], "herdr pane list --workspace w7");
        assert!(ran[2].starts_with("herdr agent start pr-1-x --kind claude --pane w7:p1"), "{ran:?}");
    }

    /// A worktree Herdr still knows is reopened, not recycled.
    #[test]
    fn a_worktree_it_still_knows_is_reopened() {
        let (mut run, ran) = recorder(|_| Ok(r#"{"result":{"workspace_id":"w3"}}"#.into()));
        assert_eq!(HerdrTui.reopen("/w/pr-1-x", &mut run), Some("w3".to_string()));
        assert_eq!(ran.borrow()[0], "herdr worktree open --path /w/pr-1-x --no-focus");
        let (mut forgot, _) = recorder(|_| Err("not found".into()));
        assert_eq!(HerdrTui.reopen("/w/pr-1-x", &mut forgot), None);
    }

    const NO_SERVER: &str = r#"{"id":"cli:agent:list","error":{"code":"server_not_running","message":"no herdr server is running at /x/herdr.sock"}}"#;

    /// A server that answers is left alone: one question, nothing started.
    #[test]
    fn a_running_server_is_left_alone() {
        let (mut run, ran) = recorder(|_| Ok(r#"{"result":{"agents":[]}}"#.into()));
        assert_eq!(ensure_server(&mut run, &mut || {}), Ok(()));
        assert_eq!(ran.borrow().as_slice(), ["herdr agent list"]);
    }

    /// No server: start one headless, as `hc` starts its core, and go on once it answers.
    #[test]
    fn no_server_starts_one_and_goes_on_once_it_answers() {
        let asked = std::cell::Cell::new(0);
        let (mut run, ran) = recorder(move |line| match line {
            "herdr agent list" => {
                asked.set(asked.get() + 1);
                if asked.get() < 3 { Err(NO_SERVER.into()) } else { Ok(r#"{"result":{"agents":[]}}"#.into()) }
            }
            _ => Ok(String::new()),
        });
        assert_eq!(ensure_server(&mut run, &mut || {}), Ok(()));
        assert_eq!(ran.borrow().as_slice(), ["herdr agent list", "herdr server", "herdr agent list", "herdr agent list"]);
    }

    /// A server that never comes up is setup: the pull request waits for the next pass, unrecorded.
    #[test]
    fn a_server_that_never_comes_up_is_setup() {
        let (mut run, ran) = recorder(|line| if line == "herdr server" { Ok(String::new()) } else { Err(NO_SERVER.into()) });
        let Err(Trouble::Setup(said)) = ensure_server(&mut run, &mut || {}) else { panic!() };
        assert!(said.contains("Herdr") && said.contains("not running"), "{said}");
        assert_eq!(ran.borrow().iter().filter(|l| *l == "herdr server").count(), 1, "started once, not every try");
        assert!(ran.borrow().len() <= SERVER_TRIES + 2, "{:?}", ran.borrow());
    }

    /// Any other answer is not "no server": nothing is started, and the call that meets it says so.
    #[test]
    fn another_failure_starts_nothing() {
        let (mut run, ran) = recorder(|_| Err(r#"{"error":{"code":"timeout"}}"#.into()));
        assert_eq!(ensure_server(&mut run, &mut || {}), Ok(()));
        assert_eq!(ran.borrow().as_slice(), ["herdr agent list"]);
    }

    #[test]
    fn its_claims_are_herdrs_named_agents() {
        let (mut run, ran) = recorder(|_| Ok(r#"{"result":{"agents":[{"cwd":"/w/pr-1-x","name":"pr-1-x"},{"cwd":"/p/x"}]}}"#.into()));
        assert_eq!(HerdrTui.agents(&mut run), Ok(vec![("/w/pr-1-x".to_string(), "pr-1-x".to_string())]));
        assert_eq!(ran.borrow()[0], "herdr agent list");
    }

    /// No server is nobody working; a server that does not answer is nobody can say.
    #[test]
    fn no_server_is_nobody_working_and_a_failed_answer_is_unsure() {
        let no_server = r#"{"id":"cli:agent:list","error":{"code":"server_not_running","message":"no herdr server is running at /x/herdr.sock"}}"#;
        let (mut run, _) = recorder(move |_| Err(no_server.into()));
        assert_eq!(HerdrTui.agents(&mut run), Ok(vec![]));
        let (mut missing, _) = recorder(|_| Err("herdr: not installed".into()));
        assert_eq!(HerdrTui.agents(&mut missing), Ok(vec![]));
        let (mut broken, _) = recorder(|_| Err(r#"{"error":{"code":"timeout"}}"#.into()));
        assert!(HerdrTui.agents(&mut broken).is_err());
    }
}
