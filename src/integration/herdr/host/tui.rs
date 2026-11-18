//! Herdr in a terminal: the `herdr` server the TUI attaches to. What the dispatcher always drove.
//!
//! Herdr makes the worktree and a workspace for it in one step, and remembers it, which is why this
//! host can reopen a worktree it still knows instead of recycling it. The agent starts in the
//! workspace's first pane and is prompted once it is ready; see `hand_over` for why that takes care.

use super::{Host, Launch, NewWorktree, Run};
use crate::integration::herdr::{hand_over, json_string, Trouble};

#[derive(Debug)]
pub struct HerdrTui;

impl Host for HerdrTui {
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
    fn agents(&self, run: &mut Run) -> Vec<(String, String)> {
        run("herdr", &["agent", "list"]).map(|raw| super::named_agents(&raw)).unwrap_or_default()
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

    #[test]
    fn its_claims_are_herdrs_named_agents() {
        let (mut run, ran) = recorder(|_| Ok(r#"{"result":{"agents":[{"cwd":"/w/pr-1-x","name":"pr-1-x"},{"cwd":"/p/x"}]}}"#.into()));
        assert_eq!(HerdrTui.agents(&mut run), [("/w/pr-1-x".to_string(), "pr-1-x".to_string())]);
        assert_eq!(ran.borrow()[0], "herdr agent list");
    }
}
