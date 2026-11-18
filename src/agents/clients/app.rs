//! The Herdr desktop app, through `hc`, the CLI of herdr-core, which runs the app's terminals.
//!
//! What its manual (`hc --skill`, `herdr-app --skill`) settles, and this follows:
//!
//!   * **No window is needed.** A terminal started with `hc run --label herdr.space=<id>` shows up in
//!     the app, and an id it does not know becomes a new space, named after the folder. So the agent
//!     appears the next time the app is open, and `herdr-app` itself is never run.
//!   * **No worktree command.** The app shows any git worktree, nested under its repository, so plain
//!     `git worktree add` is how the app's own New worktree makes one too.
//!   * **The prompt goes in the launch.** "A send to a booting agent can be dropped", so it is the
//!     agent's first argument, not a message sent after it has started.
//!   * **`--no-parent`.** A GitHoot started from inside an app terminal would otherwise make every
//!     agent that terminal's child, and close them all when it closes.
//!
//! Claude is the only agent for now. Codex would need `--no-daemon` here, or its commands run outside
//! `hc`, as the manual says; that comes back with Codex.

use super::{Client, Launch, NewWorktree, Run};
use crate::agents::dispatch::{json_string, Trouble};

#[derive(Debug)]
pub struct HerdrApp;

impl Client for HerdrApp {
    fn id(&self) -> &'static str {
        "herdr-app"
    }
    fn name(&self) -> &'static str {
        "Herdr app"
    }
    fn tool(&self) -> &'static str {
        "hc"
    }
    fn install_url(&self) -> &'static str {
        "https://herdr.dev"
    }
    fn note(&self) -> &'static str {
        "Each agent opens as a space of its own in the Herdr app, the next time the app is open. Claude asks once per new worktree whether to trust it."
    }
    /// Live terminals only, which is what `hc ls` lists without `--all`. A named one is a claim, as in
    /// the terminal client, and so is an unnamed one reporting a status, under its address: only an agent
    /// reports one, so that is an agent you opened yourself, not a shell left behind.
    fn agents(&self, run: &mut Run) -> Result<Vec<(String, String)>, String> {
        // No "not running" case: `hc` starts its core when none runs.
        let raw = match run("hc", &["ls", "--json"]) {
            Ok(raw) => raw,
            Err(e) if super::not_installed(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(raw.split("{\"address\":\"")
            .skip(1)
            .filter_map(|entry| {
                let cwd = json_string(entry, "cwd")?.replace("\\\\", "\\");
                let name = json_string(entry, "name")
                    .filter(|n| !n.is_empty())
                    .or_else(|| json_string(entry, "status").and(entry.split('"').next().map(str::to_string)))?;
                Some((cwd, name))
            })
            .collect())
    }

    fn create(&self, w: &NewWorktree, run: &mut Run) -> Result<String, Trouble> {
        run("git", &["-C", w.clone, "worktree", "add", "-b", w.branch, w.path, w.base])
            .map_err(|e| Trouble::Passing(format!("could not create the worktree: {e}")))?;
        Ok(w.path.to_string())
    }

    /// `--name` is the claim, and unique among live terminals, so a second start for the same pull
    /// request fails instead of doubling up. `--keep` is what the app gives the terminals it adopts.
    fn start(&self, l: &Launch, run: &mut Run) -> Result<(), Trouble> {
        let space = format!("herdr.space=githoot-{}", l.slug);
        let args = ["run", "--bg", "--json", "--no-parent", "--keep", "--name", l.slug, "--cwd", l.worktree, "--label", &space, "--", l.kind, l.prompt];
        run("hc", &args).map(|_| ()).map_err(|e| {
            Trouble::Passing(format!(
                "the Herdr app would not start the agent: {e}. Start it yourself: \
                 hc run --bg --no-parent --name {slug} --cwd {wt} --label {space} -- {kind} \"<the {slug} prompt>\"",
                slug = l.slug,
                wt = l.worktree,
                kind = l.kind,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::recorder;
    use super::*;

    fn launch(kind: &str) -> Launch<'_> {
        Launch { slug: "pr-1-x", kind, prompt: "Review it", worktree: "/w/pr-1-x", place: "/w/pr-1-x" }
    }

    /// Only a named terminal is a claim, as in the terminal client: an agent you opened yourself is not.
    #[test]
    fn its_claims_are_its_named_terminals() {
        let (mut run, ran) = recorder(|_| {
            Ok(r#"{"terminals":[{"address":"t3","agent":"claude-code","cwd":"/p/x","state":"live"},{"address":"t4","agent":"claude-code","cwd":"/w/pr-1-x","name":"pr-1-x","state":"live"}]}"#.into())
        });
        assert_eq!(HerdrApp.agents(&mut run), Ok(vec![("/w/pr-1-x".to_string(), "pr-1-x".to_string())]));
        assert_eq!(ran.borrow().as_slice(), ["hc ls --json"]);
    }

    /// An agent you opened yourself in a dispatcher worktree has no name, but it is working there, and
    /// recycling the worktree under it would pull the floor out. `hc` tells an agent from a shell: only
    /// an agent reports a status. So it counts, under its address; a plain shell left there does not.
    #[test]
    fn an_unnamed_agent_reporting_a_status_is_a_claim_and_a_shell_is_not() {
        let (mut run, _) = recorder(|_| {
            Ok(r#"{"terminals":[{"address":"t18","agent":"claude-code","cwd":"/w/pr-5-y","state":"live","status":"idle","turn":0},{"address":"t19","agent":"bash","cwd":"/w/pr-6-z","state":"live","turn":0}]}"#.into())
        });
        assert_eq!(HerdrApp.agents(&mut run), Ok(vec![("/w/pr-5-y".to_string(), "t18".to_string())]));
    }

    /// `hc` starts its core when none runs, so it has no "not running" answer: not installed is
    /// nobody working, and any other failure is nobody can say.
    #[test]
    fn not_installed_is_nobody_working_and_a_failed_answer_is_unsure() {
        let (mut missing, _) = recorder(|_| Err("hc: not installed".into()));
        assert_eq!(HerdrApp.agents(&mut missing), Ok(vec![]));
        let (mut broken, _) = recorder(|_| Err(r#"{"error":{"kind":"io","message":"broken pipe"}}"#.into()));
        assert!(HerdrApp.agents(&mut broken).is_err());
    }

    /// Plain git, because the app has no worktree command and shows whatever git has.
    #[test]
    fn its_worktree_is_cut_with_plain_git() {
        let (mut run, ran) = recorder(|_| Ok(String::new()));
        let w = NewWorktree { clone: "/p/x", path: "/w/pr-1-x", branch: "githoot/pr-1-x", base: "refs/githoot-pr/pr-1-x", slug: "pr-1-x" };
        assert_eq!(HerdrApp.create(&w, &mut run), Ok("/w/pr-1-x".to_string()));
        assert_eq!(ran.borrow().as_slice(), ["git -C /p/x worktree add -b githoot/pr-1-x /w/pr-1-x refs/githoot-pr/pr-1-x"]);
    }

    /// One run: in the background, named for the claim, parentless, labelled into its own space, with
    /// the prompt as the agent's first argument.
    #[test]
    fn the_agent_starts_with_its_prompt_in_one_run() {
        let (mut run, ran) = recorder(|_| Ok(r#"{"address":"t9","name":"pr-1-x","state":"live"}"#.into()));
        assert_eq!(HerdrApp.start(&launch("claude"), &mut run), Ok(()));
        assert_eq!(
            ran.borrow().as_slice(),
            ["hc run --bg --json --no-parent --keep --name pr-1-x --cwd /w/pr-1-x --label herdr.space=githoot-pr-1-x -- claude Review it"]
        );
    }

    /// The worktree is ready by now, so the line says how to finish by hand.
    #[test]
    fn a_failed_run_says_how_to_start_it_by_hand() {
        let (mut run, _) = recorder(|_| Err(r#"{"error":{"kind":"spawn_failed","message":"no such program"}}"#.into()));
        let Err(Trouble::Passing(said)) = HerdrApp.start(&launch("claude"), &mut run) else { panic!() };
        assert!(said.contains("spawn_failed"), "{said}");
        assert!(said.contains("hc run --bg --no-parent --name pr-1-x --cwd /w/pr-1-x --label herdr.space=githoot-pr-1-x -- claude"), "{said}");
    }
}
