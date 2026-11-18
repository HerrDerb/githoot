//! Where the agent's terminal opens: the one part of a dispatch that differs between agent hosts.
//!
//! Everything else in a dispatch is the same wherever the agent ends up: the fetch, the agent's own
//! branch, the push guard, the "recycle a worktree only if nothing would be lost" rule and the state
//! files. What differs is three questions, and a `Host` answers exactly those:
//!
//!   * **Who is already working?** Its live, named agents and the folder each is standing in.
//!   * **Where does the worktree come from?** The Herdr terminal makes it and a workspace with it;
//!     the Herdr app has no worktree command and shows any git worktree, so plain git makes it there.
//!   * **How does the agent start and get its prompt?**
//!
//! A new host (another agent workspace, say) is one new file here and one more entry in `HOSTS` and
//! `CHOICES`. The claim check asks every host, not only the chosen one: an agent you started in the
//! other one is still somebody working that pull request.

mod app;
mod tui;

pub use app::HerdrApp;
pub use tui::HerdrTui;

use super::Trouble;

/// Runs one program with its arguments and answers its stdout, or what it said when it failed.
/// The real one is `super::output`; tests hand in a recorder, so no host needs a live program to test.
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

pub trait Host: Sync + std::fmt::Debug {
    /// The value `integration.herdr.host` holds for it.
    fn id(&self) -> &'static str;
    /// How the log names it: "started pr-7-x in Herdr app".
    fn name(&self) -> &'static str;
    /// The program it needs on `PATH`, besides git.
    fn tool(&self) -> &'static str;
    /// Where to get that program, for the page when it is missing.
    fn install_url(&self) -> &'static str;
    /// Every live agent that has a name, as `(cwd, name)`. Empty when the program is missing or
    /// answers nothing: a host you do not run has nobody working.
    fn agents(&self, run: &mut Run) -> Vec<(String, String)>;
    /// A worktree already on disk that this host still knows, as the place to start in. `None`
    /// means it does not, and the worktree is recycled if nothing would be lost.
    fn reopen(&self, _worktree: &str, _run: &mut Run) -> Option<String> {
        None
    }
    /// Makes the worktree and answers the place to start in.
    fn create(&self, w: &NewWorktree, run: &mut Run) -> Result<String, String>;
    /// Starts the agent and gives it its prompt.
    fn start(&self, l: &Launch, run: &mut Run) -> Result<(), Trouble>;
}

/// Every host, the default first.
pub static HOSTS: [&dyn Host; 2] = [&HerdrTui, &HerdrApp];

/// The radio buttons, in the order of `HOSTS`: `(id, label)`. A literal because a setting's options
/// are a constant; a test keeps the two in step.
pub const CHOICES: &[(&str, &str)] = &[("herdr", "Herdr in a terminal"), ("herdr-app", "Herdr app")];

/// The host `id` names. Empty or unknown is the first, so a typo in `config.txt` keeps the default.
pub fn by_id(id: &str) -> &'static dyn Host {
    HOSTS.iter().copied().find(|h| h.id() == id).unwrap_or(HOSTS[0])
}

/// Every live, named agent across every host.
pub fn agents_everywhere(run: &mut Run) -> Vec<(String, String)> {
    HOSTS.iter().flat_map(|h| h.agents(run)).collect()
}

/// Every `{"cwd":"…", … "name":"…"}` in a JSON list, as `(cwd, name)`, skipping the nameless.
///
/// Both hosts answer a list of objects with these two keys, so one reader serves both. Not a real
/// parse, as `super::json_string` explains: one short, known field out of a reply the host controls.
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

    /// Replays `answer` for every program a host runs, and hands back what it ran.
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

    /// The radio and the hosts are one list. A host without a button cannot be chosen, and a button
    /// without a host would quietly start agents somewhere else.
    #[test]
    fn every_host_has_its_radio_button_and_the_first_is_the_default() {
        let ids: Vec<&str> = HOSTS.iter().map(|h| h.id()).collect();
        let choices: Vec<&str> = CHOICES.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, choices);
        assert_eq!(by_id("").id(), "herdr", "unset is the terminal, as before there was a choice");
        assert_eq!(by_id("orca?").id(), "herdr", "a typo keeps the default");
        assert_eq!(by_id("herdr-app").id(), "herdr-app");
    }

    /// An agent in either host is somebody working, whichever one the radio names. Missing programs
    /// answer nothing rather than failing the pass.
    #[test]
    fn the_claim_asks_every_host() {
        let (mut run, ran) = recorder(|line| match line {
            l if l.starts_with("herdr agent list") => Ok(r#"{"result":{"agents":[{"cwd":"/w/pr-1-a","name":"pr-1-a"}]}}"#.into()),
            l if l.starts_with("hc ls") => Ok(r#"{"terminals":[{"address":"t2","cwd":"/w/pr-2-b","name":"pr-2-b"}]}"#.into()),
            _ => Err("no such program".into()),
        });
        let found = agents_everywhere(&mut run);
        assert_eq!(found, [("/w/pr-1-a".to_string(), "pr-1-a".to_string()), ("/w/pr-2-b".to_string(), "pr-2-b".to_string())]);
        assert_eq!(ran.borrow().len(), 2, "{:?}", ran.borrow());

        let (mut none, _) = recorder(|_| Err("not found".into()));
        assert!(agents_everywhere(&mut none).is_empty());
    }
}
