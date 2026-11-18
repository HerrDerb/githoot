//! Where the agent's terminal opens: the one part of a dispatch that differs between agent clients.
//!
//! Everything else in a dispatch is the same wherever the agent ends up: the fetch, the agent's own
//! branch, the push guard, the "recycle a worktree only if nothing would be lost" rule and the state
//! files. What differs is three questions, and a `Client` answers exactly those:
//!
//!   * **Who is already working?** Its live, named agents and the folder each is standing in.
//!   * **Where does the worktree come from?** The Herdr terminal makes it and a workspace with it;
//!     the Herdr app has no worktree command and shows any git worktree, so plain git makes it there.
//!   * **How does the agent start and get its prompt?**
//!
//! A new client (another agent workspace, say) is one new file here and one more entry in `CLIENTS` and
//! `CHOICES`. The claim check asks every client, not only the chosen one: an agent you started in the
//! other one is still somebody working that pull request.

mod app;
mod orca;
mod tui;

pub use app::HerdrApp;
pub use orca::Orca;
pub use tui::HerdrTui;

use super::dispatch::Trouble;

/// Runs one program with its arguments and answers its stdout, or what it said when it failed.
/// The real one is `super::output`; tests hand in a recorder, so no client needs a live program to test.
pub type Run<'a> = dyn FnMut(&str, &[&str]) -> Result<String, String> + 'a;

/// A worktree to make: `branch`, cut from `base`, checked out at `path`, off the clone at `clone`.
pub struct NewWorktree<'a> {
    pub clone: &'a str,
    pub path: &'a str,
    pub branch: &'a str,
    pub base: &'a str,
    pub slug: &'a str,
}

/// An agent to start in a worktree that is ready.
pub struct Launch<'a> {
    pub slug: &'a str,
    pub kind: &'a str,
    pub prompt: &'a str,
    pub worktree: &'a str,
    /// What `reopen` or `create` answered: the Herdr terminal's workspace, the app's worktree path.
    pub place: &'a str,
}

pub trait Client: Sync + std::fmt::Debug {
    /// The value `agentsClient` holds for it.
    fn id(&self) -> &'static str;
    /// How the log names it: "started pr-7-x in Herdr app".
    fn name(&self) -> &'static str;
    /// The program it needs on `PATH`, besides git.
    fn tool(&self) -> &'static str;
    /// Where to get that program, for the page when it is missing.
    fn install_url(&self) -> &'static str;
    /// One sentence for its card on the Agents page: where an agent shows up, and anything it needs from
    /// you that GitHoot cannot set for you.
    fn note(&self) -> &'static str;
    /// Every live agent that has a name, as `(cwd, name)`. Empty when its program is not installed or
    /// its server is not running: nobody can be working there. `Err` when it is there and did not
    /// answer, because then nobody can say, and an empty list would read as "nobody home".
    fn agents(&self, run: &mut Run) -> Result<Vec<(String, String)>, String>;
    /// Before anything is touched for a pull request: make sure the client is there to take it. A client
    /// that can start itself does so here. `Trouble::Setup` when it is not there, so nothing is done for
    /// the pull request yet and it is tried again on the next pass.
    fn ready(&self, _run: &mut Run) -> Result<(), Trouble> {
        Ok(())
    }
    /// A worktree already on disk that this client still knows, as the place to start in. `None`
    /// means it does not, and the worktree is recycled if nothing would be lost.
    fn reopen(&self, _worktree: &str, _run: &mut Run) -> Option<String> {
        None
    }
    /// Makes the worktree and answers the place to start in. `Trouble::Setup` when the client itself is
    /// not there to ask, so the pull request is tried again as soon as it is.
    fn create(&self, w: &NewWorktree, run: &mut Run) -> Result<String, Trouble>;
    /// Starts the agent and gives it its prompt.
    fn start(&self, l: &Launch, run: &mut Run) -> Result<(), Trouble>;
}

/// Every client, the default first.
pub static CLIENTS: [&dyn Client; 3] = [&HerdrTui, &HerdrApp, &Orca];

/// The radio buttons, in the order of `CLIENTS`: `(id, label)`. A literal because a setting's options
/// are a constant; a test keeps the two in step.
pub const CHOICES: &[(&str, &str)] = &[("herdr", "Herdr in a terminal"), ("herdr-app", "Herdr app"), ("orca", "Orca")];

/// The client `id` names. Empty or unknown is the first, so a typo in `config.txt` keeps the default.
pub fn by_id(id: &str) -> &'static dyn Client {
    CLIENTS.iter().copied().find(|h| h.id() == id).unwrap_or(CLIENTS[0])
}

/// Every live, named agent across every client.
pub fn agents_everywhere(run: &mut Run) -> Inventory {
    let mut all = Inventory::default();
    for client in CLIENTS {
        match client.agents(run) {
            Ok(agents) => all.agents.extend(agents),
            Err(e) => all.unsure.push(format!("{}: {e}", client.name())),
        }
    }
    all
}

/// Who is working, as far as every client could say.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub agents: Vec<(String, String)>,
    /// Clients that are there but did not answer, each with what it said. Non-empty means an agent
    /// may be working somewhere this list does not show.
    pub unsure: Vec<String>,
}

/// Whether a failed run means its program is not installed, as `super::output` words it.
pub fn not_installed(said: &str) -> bool {
    said.ends_with(": not installed")
}

/// Every `{"cwd":"…", … "name":"…"}` in a JSON list, as `(cwd, name)`, skipping the nameless.
///
/// Both clients answer a list of objects with these two keys, so one reader serves both. Not a real
/// parse, as `super::json_string` explains: one short, known field out of a reply the client controls.
pub fn named_agents(raw: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for chunk in raw.split("\"cwd\":\"").skip(1) {
        let Some(end) = chunk.find('"') else { continue };
        let cwd = chunk[..end].replace("\\\\", "\\");
        let name = chunk
            .find("\"name\":\"")
            .map(|at| &chunk[at + 8..])
            .and_then(|rest| rest.find('"').map(|end| rest[..end].to_string()))
            .unwrap_or_default();
        if !name.is_empty() {
            found.push((cwd, name));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a recorder ran, one `exe args…` line per program.
    pub type Ran = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// Replays `answer` for every program a client runs, and hands back what it ran.
    pub fn recorder(answer: impl Fn(&str) -> Result<String, String>) -> (impl FnMut(&str, &[&str]) -> Result<String, String>, Ran) {
        let ran = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = ran.clone();
        let run = move |exe: &str, args: &[&str]| {
            let line = format!("{exe} {}", args.join(" "));
            log.borrow_mut().push(line.clone());
            answer(&line)
        };
        (run, ran)
    }

    /// The radio and the clients are one list. A client without a button cannot be chosen, and a button
    /// without a client would quietly start agents somewhere else.
    #[test]
    fn every_host_has_its_radio_button_and_the_first_is_the_default() {
        let ids: Vec<&str> = CLIENTS.iter().map(|h| h.id()).collect();
        let choices: Vec<&str> = CHOICES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, choices);
        assert_eq!(by_id("").id(), "herdr", "unset is the terminal, as before there was a choice");
        assert_eq!(by_id("orca?").id(), "herdr", "a typo keeps the default");
        assert_eq!(by_id("herdr-app").id(), "herdr-app");
        assert_eq!(by_id("orca").id(), "orca");
    }

    /// An agent in either client is somebody working, whichever one the radio names. Missing programs
    /// answer nothing rather than failing the pass.
    #[test]
    fn the_claim_asks_every_host() {
        let (mut run, ran) = recorder(|line| match line {
            l if l.starts_with("herdr agent list") => Ok(r#"{"result":{"agents":[{"cwd":"/w/pr-1-a","name":"pr-1-a"}]}}"#.into()),
            l if l.starts_with("hc ls") => Ok(r#"{"terminals":[{"address":"t2","cwd":"/w/pr-2-b","name":"pr-2-b"}]}"#.into()),
            _ => Err("no such program".into()),
        });
        let found = agents_everywhere(&mut run);
        assert_eq!(found.agents, [("/w/pr-1-a".to_string(), "pr-1-a".to_string()), ("/w/pr-2-b".to_string(), "pr-2-b".to_string())]);
        assert_eq!(ran.borrow().len(), CLIENTS.len(), "{:?}", ran.borrow());

        let (mut none, _) = recorder(|line| Err(format!("{}: not installed", line.split(' ').next().unwrap())));
        assert_eq!(agents_everywhere(&mut none), Inventory::default(), "a client you do not have is nobody working");
    }

    /// A client that is there and does not answer is not "nobody working": the claim says it is unsure,
    /// and which client and why, so nothing is recycled on the strength of a list that is missing part.
    #[test]
    fn a_host_that_does_not_answer_makes_the_claim_unsure() {
        let (mut run, _) = recorder(|line| match line {
            l if l.starts_with("herdr agent list") => Err("connection reset by peer".into()),
            l => Err(format!("{}: not installed", l.split(' ').next().unwrap())),
        });
        let found = agents_everywhere(&mut run);
        assert!(found.agents.is_empty());
        assert_eq!(found.unsure, ["Herdr: connection reset by peer"]);
    }
}
