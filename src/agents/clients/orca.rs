//! Orca (orca.dev), through its own CLI: `orca-ide` on Linux, where `orca` is the GNOME screen reader,
//! and `orca` elsewhere. Every command talks to the running Orca app; none of it works without one.
//!
//! What its command schema (`orca-ide agent-context --json`) and a spike against Orca 1.4.263 settled:
//!
//!   * **Git makes the worktree, as for the Herdr app.** Orca's own `worktree create` picks the path
//!     and may suffix a taken branch name, which would break the worktree root, the exact starting ref
//!     and the push guard. A worktree made by git shows up once the repository is in Orca and its
//!     external worktrees are set to show, so that is set before every launch, harmlessly repeated.
//!   * **Orca picks the agent's permission flags, and its default is to skip every prompt.** Its
//!     "Agent permissions" setting adds `--dangerously-skip-permissions` to Claude (and the like to
//!     other agents) on every launch, including a plain terminal whose command is `claude`, and no
//!     command can change or read the setting. That would remove the guard rail this dispatcher rests
//!     on: the agent asks before it pushes. What a launch does return is the arguments Orca used, so
//!     anything but asking is closed again before the agent is given the pull request.
//!   * **The prompt is pasted once the agent is ready.** `terminal send --enter` pastes a whole
//!     multi-line prompt into a recognised agent as one block and reports when its turn started.

use super::{Client, Launch, NewWorktree, Run};
use crate::agents::dispatch::{json_string, Trouble};

#[derive(Debug)]
pub struct Orca;

/// The agent arguments Orca may launch Claude with that still ask before acting: none at all (its
/// "Manual", so Claude's own settings decide), and the explicit ask and accept-edits modes. Claude is
/// the only agent for now; another one brings its own asking modes. An allowlist,
/// not a list of the bypass flags, so a mode Orca adds later is refused until somebody looks at it.
const ASKING: &[&str] = &["", "--permission-mode default", "--permission-mode acceptEdits"];

/// How long the agent has to come up before its prompt, and how long the prompt has to be taken up.
const READY_WAIT_MS: &str = "120000";
const SUBMIT_WAIT_S: &str = "30";

impl Client for Orca {
    fn id(&self) -> &'static str {
        "orca"
    }
    fn name(&self) -> &'static str {
        "Orca"
    }
    fn tool(&self) -> &'static str {
        if cfg!(target_os = "linux") { "orca-ide" } else { "orca" }
    }
    fn install_url(&self) -> &'static str {
        "https://orca.dev"
    }
    fn note(&self) -> &'static str {
        "Orca must be running, and its agent permissions set to Manual or Ask (Settings > Agents > Agent permissions). An agent Orca would let act without asking is closed before it gets the pull request."
    }
    /// Every agent Orca lists, by the worktree in its `worktreeId` (`<repo-id>::<path>`). A finished one
    /// stays listed while it sleeps, and still counts: its worktree is still somebody's. Orca not
    /// running (`runtime_unavailable`, its own code) is nobody working; any other failure is unsure.
    /// Orca is a desktop app, so GitHoot does not start it: not running is absent, said once on the
    /// desktop, and the pull request waits until you open Orca. Its own `runtime_unavailable` is the
    /// one answer that means that; anything else is left to the call that meets it.
    fn ready(&self, run: &mut Run) -> Result<(), Trouble> {
        match run(self.tool(), &["agent", "list", "--json"]) {
            Ok(said) | Err(said) if said.contains("runtime_unavailable") => {
                Err(Trouble::Absent("Orca is not running. Open Orca, and the next pass starts the agent".to_string()))
            }
            _ => Ok(()),
        }
    }

    /// Its own command line opens it, and returns once it is ready to take an agent.
    fn opener(&self) -> Option<&'static [&'static str]> {
        Some(&["open"])
    }

    fn agents(&self, run: &mut Run) -> Result<Vec<(String, String)>, String> {
        let raw = match run(self.tool(), &["agent", "list", "--json"]) {
            Ok(raw) if !is_false(&raw, "ok") => raw,
            Ok(said) | Err(said) if super::not_installed(&said) || said.contains("runtime_unavailable") => return Ok(Vec::new()),
            Ok(said) | Err(said) => return Err(json_string(&said, "message").unwrap_or(said)),
        };
        Ok(raw
            .split("\"worktreeId\"")
            .skip(1)
            .filter_map(|rest| json_string(&format!("\"worktreeId\"{rest}"), "worktreeId"))
            .filter_map(|id| id.split_once("::").map(|(_, path)| path.replace("\\\\", "\\")))
            .map(|cwd| (cwd, "orca".to_string()))
            .collect())
    }

    /// Orca first, because Orca not answering is the commonest failure and costs nothing to find out.
    fn create(&self, w: &NewWorktree, run: &mut Run) -> Result<String, Trouble> {
        let tool = self.tool();
        orca(tool, run, &["repo", "add", "--path", w.clone, "--json"])
            .map_err(|e| Trouble::Setup(format!("Orca did not answer, so nothing was started. Is it running? {e}")))?;
        let repo = format!("path:{}", w.clone);
        orca(tool, run, &["repo", "set", "--repo", &repo, "--external-worktree-visibility", "show", "--json"])
            .map_err(|e| Trouble::Passing(format!("Orca would not show worktrees made outside it for {}: {e}", w.clone)))?;
        run("git", &["-C", w.clone, "worktree", "add", "-b", w.branch, w.path, w.base])
            .map_err(|e| Trouble::Passing(format!("could not create the worktree: {e}")))?;
        Ok(w.path.to_string())
    }

    fn start(&self, l: &Launch, run: &mut Run) -> Result<(), Trouble> {
        let tool = self.tool();
        let at = format!("path:{}", l.worktree);
        let created = orca(tool, run, &["terminal", "create", "--worktree", &at, "--title", l.slug, "--command", l.kind, "--json"])
            .map_err(|e| Trouble::Passing(format!("Orca would not open a terminal for the agent: {e}")))?;
        let handle = json_string(&created, "handle").ok_or_else(|| Trouble::Passing("Orca opened no terminal".to_string()))?;

        // Before anything else, and before it is told what the pull request is. Missing is not "": Orca
        // leaves `launchConfig` out when it has none, and "" is Manual, so only a reported "" passes.
        let Some(args) = json_string(&created, "agentArgs") else {
            let _ = orca(tool, run, &["terminal", "close", "--terminal", &handle, "--json"]);
            return Err(Trouble::Setup(format!(
                "Orca did not say how it starts {}, so whether it asks before acting is unknown, and it was closed before it \
                 got the pull request. Check that Orca knows {0} as an agent",
                l.kind
            )));
        };
        // Some full-access switches are environment variables rather than arguments (goose's
        // `GOOSE_MODE=auto`), so an asking launch sets nothing there either. Missing counts as set.
        let env = literal(&created, "agentEnv")
            .and_then(|v| v.strip_prefix('{'))
            .and_then(|v| v.find('}').map(|end| v[..end].trim().to_string()));
        if !ASKING.contains(&args.trim()) || env.as_deref() != Some("") {
            let _ = orca(tool, run, &["terminal", "close", "--terminal", &handle, "--json"]);
            let with = match env.as_deref() {
                Some("") => args.clone(),
                Some(set) => format!("{args} and the environment {{{set}}}"),
                None => format!("{args} and an environment it did not report"),
            };
            return Err(Trouble::Setup(format!(
                "Orca starts {} with {with}, which lets it act without asking you, so it was closed before it got the \
                 pull request. In Orca, set Settings > Agents > Agent permissions to Manual or Ask",
                l.kind
            )));
        }

        let by_hand = format!("{tool} terminal send --terminal {handle} --text \"<the {} prompt>\" --enter", l.slug);
        let not_ready = |why: &str| {
            Trouble::Passing(format!(
                "the agent is running in Orca but never became ready ({why}), so its prompt was not sent. Answer it there, then run {by_hand}"
            ))
        };
        let waited = orca(tool, run, &["terminal", "wait", "--terminal", &handle, "--for", "tui-idle", "--timeout-ms", READY_WAIT_MS, "--json"])
            .map_err(|e| not_ready(&e))?;
        if !is_true(&waited, "satisfied") {
            return Err(not_ready(&json_string(&waited, "blockedReason").unwrap_or_else(|| "it timed out".to_string())));
        }

        let sent = orca(tool, run, &["terminal", "send", "--terminal", &handle, "--text", l.prompt, "--enter", "--wait-submit", SUBMIT_WAIT_S, "--json"])
            .map_err(|e| Trouble::Passing(format!("the prompt did not arrive: {e}. Send it with {by_hand}")))?;
        // Accepted is not arrived: the agent has to be seen starting a turn on it.
        if !sent.contains("turn_started") {
            return Err(Trouble::Passing(format!("the prompt was taken but its turn never started. Look at the agent in Orca, or send it with {by_hand}")));
        }
        Ok(())
    }
}

/// Runs one Orca command. A refusal can come back on stdout with a zero exit (`"ok": false`), so that
/// counts as failing as much as a non-zero exit does, and says Orca's own message.
fn orca(tool: &str, run: &mut Run, args: &[&str]) -> Result<String, String> {
    let out = run(tool, args)?;
    if is_false(&out, "ok") {
        return Err(json_string(&out, "message").unwrap_or(out));
    }
    Ok(out)
}

fn is_true(json: &str, key: &str) -> bool {
    literal(json, key).is_some_and(|v| v.starts_with("true"))
}

fn is_false(json: &str, key: &str) -> bool {
    literal(json, key).is_some_and(|v| v.starts_with("false"))
}

/// What follows `"key":`, spaces skipped.
fn literal<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let at = json.find(&needle)? + needle.len();
    json[at..].trim_start().strip_prefix(':').map(str::trim_start)
}

#[cfg(test)]
mod tests {
    use super::super::tests::recorder;
    use super::*;

    const TOOL: &str = if cfg!(target_os = "linux") { "orca-ide" } else { "orca" };

    /// Trimmed from a real `agent list --json` (Orca 1.4.263).
    const AGENT_LIST: &str = r#"{
  "id": "6c3c0dab",
  "ok": true,
  "result": {
    "agents": [
      {
        "orcaSessionId": "7f9e3db6-1056-4825-90a4-5c5f45dfa93e",
        "kind": "terminal",
        "agent": "claude",
        "state": "working",
        "worktreeId": "16f172fd-480f-4a78-abba-914e76516138::/w/pr-3-spike-repo",
        "workspace": "githoot/pr-3-spike-repo",
        "repo": "spike-repo"
      },
      {
        "orcaSessionId": null,
        "kind": "terminal",
        "agent": "codex",
        "state": "done",
        "worktreeId": "0a1b::C:\\w\\pr-4-x",
        "workspace": "githoot/pr-4-x"
      }
    ],
    "truncated": false
  }
}"#;

    /// Trimmed from a real `terminal create --json`, with Orca's permission setting on Ask.
    fn created(args: &str) -> String {
        format!(
            r#"{{
  "id": "50e5",
  "ok": true,
  "result": {{
    "terminal": {{
      "launchConfig": {{
        "agentCommand": "claude",
        "agentArgs": "{args}",
        "agentEnv": {{}}
      }},
      "launchAgent": "claude",
      "processId": 463281,
      "handle": "term_9cde",
      "worktreeId": "16f1::/w/pr-1-x"
    }}
  }}
}}"#
        )
    }
    const READY: &str = r#"{"ok": true, "result": {"wait": {"handle": "term_9cde", "condition": "tui-idle", "satisfied": true, "status": "running"}}}"#;
    const SENT: &str = r#"{"ok": true, "result": {"send": {"handle": "term_9cde", "accepted": true, "prompt": {"stages": ["input_accepted", "turn_started"], "provider": "claude"}}}}"#;

    fn launch() -> Launch<'static> {
        Launch { slug: "pr-1-x", kind: "claude", prompt: "Review it\nline two", worktree: "/w/pr-1-x", place: "/w/pr-1-x" }
    }

    /// Every agent Orca lists is a claim on its worktree, whatever its state: a finished one is listed
    /// while it sleeps, and its worktree is still somebody's. Shells are not listed at all.
    #[test]
    fn its_claims_are_the_worktrees_of_the_agents_it_lists() {
        let (mut run, ran) = recorder(|_| Ok(AGENT_LIST.into()));
        let found = Orca.agents(&mut run).unwrap();
        let cwds: Vec<&str> = found.iter().map(|(cwd, _)| cwd.as_str()).collect();
        assert_eq!(cwds, ["/w/pr-3-spike-repo", r"C:\w\pr-4-x"]);
        assert!(found.iter().all(|(_, name)| !name.is_empty()), "every one is a claim: {found:?}");
        assert_eq!(ran.borrow().as_slice(), [format!("{TOOL} agent list --json")]);
    }

    /// Orca not running is the one thing only you can fix, so it is said as absent, which tells you on
    /// the desktop; anything else is left to the call that meets it.
    #[test]
    fn orca_not_running_is_absent_before_anything_is_touched() {
        let not_running = r#"{"id": "local", "ok": false, "error": {"code": "runtime_unavailable", "message": "Start the Orca app first."}}"#;
        let (mut run, ran) = recorder(move |_| Err(not_running.into()));
        let Err(Trouble::Absent(said)) = Orca.ready(&mut run) else { panic!() };
        assert!(said.contains("Orca is not running") && said.contains("Open Orca"), "{said}");
        assert_eq!(ran.borrow().as_slice(), [format!("{TOOL} agent list --json")]);
        let (mut running, _) = recorder(|_| Ok(AGENT_LIST.into()));
        assert_eq!(Orca.ready(&mut running), Ok(()));
        let (mut broken, _) = recorder(|_| Err(r#"{"ok": false, "error": {"code": "request_timeout"}}"#.into()));
        assert_eq!(Orca.ready(&mut broken), Ok(()), "not absent, so the next call says what it is");
    }

    /// Orca not running is nobody working; Orca there and failing is nobody can say. Both answers are
    /// as Orca 1.4.263 gives them: exit 1 with its JSON on stdout.
    #[test]
    fn orca_not_running_is_nobody_working_and_a_failed_answer_is_unsure() {
        let not_running = r#"{"id": "local", "ok": false, "error": {"code": "runtime_unavailable", "message": "Start the Orca app first."}}"#;
        let (mut run, _) = recorder(move |_| Err(not_running.into()));
        assert_eq!(Orca.agents(&mut run), Ok(vec![]));
        let (mut missing, _) = recorder(|_| Err(format!("{TOOL}: not installed")));
        assert_eq!(Orca.agents(&mut missing), Ok(vec![]));
        let broken = r#"{"ok": false, "error": {"code": "request_timeout", "message": "Orca did not answer in time"}}"#;
        let (mut run, _) = recorder(move |_| Ok(broken.into()));
        assert_eq!(Orca.agents(&mut run), Err("Orca did not answer in time".to_string()));
    }

    /// Git makes the worktree; Orca is told about the repository and to show its outside worktrees first.
    #[test]
    fn the_worktree_is_cut_with_git_after_orca_is_told_to_show_it() {
        let (mut run, ran) = recorder(|_| Ok(r#"{"ok": true}"#.into()));
        let w = NewWorktree { clone: "/p/x", path: "/w/pr-1-x", branch: "githoot/pr-1-x", base: "refs/githoot-pr/pr-1-x", slug: "pr-1-x" };
        assert_eq!(Orca.create(&w, &mut run), Ok("/w/pr-1-x".to_string()));
        assert_eq!(
            ran.borrow().as_slice(),
            [
                format!("{TOOL} repo add --path /p/x --json"),
                format!("{TOOL} repo set --repo path:/p/x --external-worktree-visibility show --json"),
                "git -C /p/x worktree add -b githoot/pr-1-x /w/pr-1-x refs/githoot-pr/pr-1-x".to_string(),
            ]
        );
    }

    /// Orca not answering is this machine's setup, not the pull request's: tried again next pass.
    #[test]
    fn an_orca_that_does_not_answer_is_setup() {
        let (mut run, ran) = recorder(|_| Err("connect ECONNREFUSED".into()));
        let w = NewWorktree { clone: "/p/x", path: "/w/pr-1-x", branch: "b", base: "r", slug: "pr-1-x" };
        let Err(Trouble::Setup(said)) = Orca.create(&w, &mut run) else { panic!() };
        assert!(said.contains("ECONNREFUSED") && said.contains("running"), "{said}");
        assert_eq!(ran.borrow().len(), 1, "nothing after the first refusal");
    }

    /// Asking: a terminal running the agent, then its prompt once it is ready, seen to start a turn.
    #[test]
    fn an_asking_agent_is_started_then_given_its_prompt() {
        let (mut run, ran) = recorder(|line| match line {
            l if l.contains("terminal create") => Ok(created("--permission-mode default")),
            l if l.contains("terminal wait") => Ok(READY.into()),
            _ => Ok(SENT.into()),
        });
        assert_eq!(Orca.start(&launch(), &mut run), Ok(()));
        let ran = ran.borrow();
        assert_eq!(ran[0], format!("{TOOL} terminal create --worktree path:/w/pr-1-x --title pr-1-x --command claude --json"));
        assert_eq!(ran[1], format!("{TOOL} terminal wait --terminal term_9cde --for tui-idle --timeout-ms 120000 --json"));
        assert_eq!(ran[2], format!("{TOOL} terminal send --terminal term_9cde --text Review it\nline two --enter --wait-submit 30 --json"));
        assert_eq!(ran.len(), 3);
    }

    /// The default, Yolo, and Auto too: anything but asking is closed before the agent gets the pull
    /// request, and said as setup, so fixing Orca's setting is enough for the next pass to start it.
    #[test]
    fn an_agent_orca_lets_skip_its_prompts_is_closed_before_its_prompt() {
        for args in ["--dangerously-skip-permissions", "--permission-mode auto", "--permission-mode default --dangerously-skip-permissions"] {
            let (mut run, ran) = recorder(move |line| if line.contains("terminal create") { Ok(created(args)) } else { Ok(r#"{"ok": true}"#.into()) });
            let Err(Trouble::Setup(said)) = Orca.start(&launch(), &mut run) else { panic!("{args}") };
            assert!(said.contains(args) && said.contains("Agent permissions"), "{said}");
            let ran = ran.borrow();
            assert_eq!(ran[1], format!("{TOOL} terminal close --terminal term_9cde --json"), "{args}");
            assert!(!ran.iter().any(|l| l.contains("terminal send")), "no prompt: {ran:?}");
        }
    }

    /// No word on how Orca started the agent is no word that it asks: Orca leaves `launchConfig` out
    /// of a reply that has none, and "" is Manual, so reading a missing one as "" would wave it through.
    #[test]
    fn an_agent_orca_does_not_say_how_it_started_is_closed_before_its_prompt() {
        let unsaid = r#"{"ok": true, "result": {"terminal": {"launchAgent": "claude", "handle": "term_9cde"}}}"#;
        let (mut run, ran) = recorder(move |line| if line.contains("terminal create") { Ok(unsaid.into()) } else { Ok(r#"{"ok": true}"#.into()) });
        let Err(Trouble::Setup(said)) = Orca.start(&launch(), &mut run) else { panic!() };
        assert!(said.contains("did not say how it starts claude"), "{said}");
        let ran = ran.borrow();
        assert_eq!(ran[1], format!("{TOOL} terminal close --terminal term_9cde --json"));
        assert!(!ran.iter().any(|l| l.contains("terminal send")), "no prompt: {ran:?}");
    }

    /// Orca puts some full-access switches in the environment rather than the arguments (goose's
    /// `GOOSE_MODE=auto`, with no argument at all), so an asking launch also has nothing set there.
    #[test]
    fn an_agent_orca_lets_skip_its_prompts_through_its_environment_is_closed() {
        let env = r#"{"ok": true, "result": {"terminal": {"launchConfig": {"agentArgs": "", "agentEnv": {"GOOSE_MODE": "auto"}}, "handle": "term_9cde"}}}"#;
        let (mut run, ran) = recorder(move |line| if line.contains("terminal create") { Ok(env.into()) } else { Ok(r#"{"ok": true}"#.into()) });
        let Err(Trouble::Setup(said)) = Orca.start(&launch(), &mut run) else { panic!() };
        assert!(said.contains("GOOSE_MODE") && said.contains("Agent permissions"), "{said}");
        assert!(!ran.borrow().iter().any(|l| l.contains("terminal send")), "no prompt");
    }

    /// An agent still asking something at startup gets no prompt pasted into that question.
    #[test]
    fn an_agent_that_never_comes_ready_gets_no_prompt() {
        let (mut run, ran) = recorder(|line| match line {
            l if l.contains("terminal create") => Ok(created("")),
            l if l.contains("terminal wait") => Ok(r#"{"ok": true, "result": {"wait": {"satisfied": false, "blockedReason": "agent-startup-menu-prompt"}}}"#.into()),
            _ => Ok(SENT.into()),
        });
        let Err(Trouble::Passing(said)) = Orca.start(&launch(), &mut run) else { panic!() };
        assert!(said.contains("agent-startup-menu-prompt") && said.contains("terminal send --terminal term_9cde"), "{said}");
        assert!(!ran.borrow().iter().any(|l| l.contains("terminal send")));
    }

    /// Accepted is not arrived: the turn has to be seen to start.
    #[test]
    fn a_prompt_that_never_starts_a_turn_is_a_failure() {
        let (mut run, _) = recorder(|line| match line {
            l if l.contains("terminal create") => Ok(created("")),
            l if l.contains("terminal wait") => Ok(READY.into()),
            _ => Ok(r#"{"ok": true, "result": {"send": {"accepted": true, "prompt": {"stages": ["input_accepted"]}}}}"#.into()),
        });
        let Err(Trouble::Passing(said)) = Orca.start(&launch(), &mut run) else { panic!() };
        assert!(said.contains("never started"), "{said}");
    }
}
