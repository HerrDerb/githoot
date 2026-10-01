//! Starts a Herdr agent for each pull request that needs one, from inside GitHoot.
//!
//! **This is the one thing GitHoot does that is not reading.** Everywhere else it polls GitHub,
//! draws an icon and opens a page; here it creates branches and worktrees and starts agents. That
//! boundary used to be a process boundary: a shipped script, installed by a button, kept alive by
//! systemd or Task Scheduler. It is now the first integration (see `crate::integration`), off
//! until you install it on the Integrations tab, which writes `integration.herdr.enabled=on`.
//!
//! The move inward is not a tidy-up. Two things forced it:
//!
//!   * **A console program cannot be a background service on Windows without a trick.** Task
//!     Scheduler hands a console to whatever it starts, so the loop sat in a terminal on the
//!     desktop. Every escape is a trade: session 0 cannot reach the Herdr you have open, a script
//!     host shim is what antivirus flags, and hiding a window still created one. GitHoot is
//!     already a windowless GUI-subsystem process with a poll thread, so in here the problem does
//!     not exist rather than being worked around.
//!   * **One behaviour, two implementations, guaranteed to drift.** Bash for Linux and PowerShell
//!     for Windows meant five hundred lines each of the same rules, where a divergence is
//!     invisible until it bites a user on one platform only.
//!
//! What did **not** change, because it was right:
//!
//!   * GitHoot's own token stays read-only and is never handed to anything. The agent reads the
//!     diff and the comments itself, with the forge's own tool (`gh`, `glab`), under your credential.
//!   * The prompts are files you own, and clearing one restores the shipped default.
//!   * The agent works on a branch of its own, `githoot/<slug>`, never the pull request's.
//!
//! **An agent is started by a pull request arriving in a bar, and by nothing else**, and the
//! dispatcher asks no forge anything: the bars come from GitHoot, who is working from Herdr, and the
//! checkout from git, fetching the head ref the portal names. So it serves every portal, including
//! ones not written yet. See `decide`.
//!
//! What GitHoot knows it no longer re-fetches: the bars come from the integration runner, the same
//! judgement the icon and the pages use, with muted pull requests already taken out. Its files live
//! in `~/.githoot/integrations/herdr/`: one state file per bar and the prompts.

pub mod prompts;

use super::{Batch, Context, Info, Integration, Kind, Problem, Said, Setting};
use crate::page::esc;
use crate::portal::types::PrEntry;
use crate::portal::PortalKind;
use crate::state::PrAxis;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a tick decided about one pull request. Separated from doing it so the rule can be tested
/// without a Herdr, a forge or a clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Already dispatched while in this bar, or a failed start whose pull request has not changed
    /// since. The common case, and silent.
    AlreadySeen,
    /// New to the bar, but an agent is already in its worktree: say so once and leave it alone.
    AlreadyThere,
    /// New to the bar, or a failed start whose pull request has since changed, and nobody is home.
    Start,
}

/// What this bar recorded about one pull request last time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Handled {
    /// The `updated_at` it was last looked at. Only matters while `dispatched` is false: a failed
    /// start is tried again when the pull request changes, not every pass.
    pub updated: String,
    /// An agent was started for it, or was already there, or it was in the bar when the bar was
    /// switched on. Nothing more starts for it while it stays in the bar.
    pub dispatched: bool,
}

/// The whole rule, in one place, with no I/O in it.
///
/// **An agent is started by a pull request arriving in a bar, and by nothing else.** The arrival
/// comes from GitHoot's own lists, the same judgement the icon shows, so the rule is the same for
/// every portal and asks no forge anything. A push, a comment or a label on a pull request already
/// dispatched starts nothing. A pull request that leaves the bar is forgotten, so coming back into
/// it (changes requested again, say) is an arrival like any other.
pub fn decide(prev: Option<&Handled>, updated: &str, live_agent: Option<&str>) -> Action {
    if let Some(seen) = prev
        && (seen.dispatched || seen.updated == updated)
    {
        return Action::AlreadySeen;
    }
    match live_agent {
        Some(_) => Action::AlreadyThere,
        None => Action::Start,
    }
}

/// The branch and worktree name for a pull request. Lowercased, reduced to what git and a
/// directory name both tolerate, and capped so a long repo name cannot produce an unusable path.
pub fn slug(repo: &str, number: u64) -> String {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let raw = format!("pr-{number}-{name}").to_lowercase();
    let mut out: String = raw
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    out.truncate(32);
    out
}

/// Everything a tick needs that a user can change.
#[derive(Debug, Clone)]
pub struct Settings {
    pub clone_root: PathBuf,
    /// Where `clone_root` came from, so a message about it can say which knob moves it.
    pub clone_root_from: RootFrom,
    pub worktree_root: PathBuf,
    pub agent_kind: String,
    /// Says what it would do and touches nothing, including the state file. What the page's Dry run
    /// button asks for, and never what the runner does.
    pub dry_run: bool,
}

impl Settings {
    pub fn worktree(&self, slug: &str) -> PathBuf {
        self.worktree_root.join(slug)
    }
    pub fn clone_of(&self, repo: &str) -> PathBuf {
        self.clone_root.join(repo.rsplit('/').next().unwrap_or(repo))
    }
}

/// Which of the three places a root was taken from. See `settings_now`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootFrom {
    /// "Clones live in" on the page, `integration.herdr.cloneRoot` in the file.
    Setting,
    /// `GITHOOT_CLONE_ROOT`.
    Env,
    /// Neither, so `~/projects`.
    Default,
}

/// The clone root, and how it came to be that.
fn clone_root_said(settings: &Settings) -> String {
    let root = settings.clone_root.display();
    match settings.clone_root_from {
        RootFrom::Setting => format!("\"{CLONE_ROOT_LABEL}\" is {root}"),
        RootFrom::Env => format!("GITHOOT_CLONE_ROOT is {root}"),
        RootFrom::Default => format!("\"{CLONE_ROOT_LABEL}\" is empty, so it looks in {root}"),
    }
}

/// What to log when `repo` has no clone: where it looked, why there, and what to change.
fn no_clone(settings: &Settings, repo: &str) -> String {
    let fix = format!("set \"{CLONE_ROOT_LABEL}\" on the Herdr dispatcher's settings page");
    if !settings.clone_root.is_dir() {
        return format!("no clone of {repo}: {}, and that folder does not exist. To fix it, {fix}", clone_root_said(settings));
    }
    format!(
        "no clone of {repo} at {}: {}, and that has no clone named {}. Clone it there, or {fix}",
        settings.clone_of(repo).display(),
        clone_root_said(settings),
        repo.rsplit('/').next().unwrap_or(repo)
    )
}

/// What is wrong with the folders before any pull request arrives. Empty when they look right.
///
/// Only what is certainly wrong: a clone root that is not there, or holds no clone at all, which is
/// almost always the wrong folder. A worktree root that does not exist yet is fine, `git worktree add`
/// creates it.
pub fn setup_problems(settings: &Settings) -> Vec<Problem> {
    let mut problems = Vec::new();
    let clone_root = |text: String| Problem { key: "cloneRoot", text };
    match std::fs::read_dir(&settings.clone_root) {
        Err(_) => problems.push(clone_root(format!("{}, and that folder does not exist", clone_root_said(settings)))),
        Ok(entries) => {
            if !entries.flatten().any(|e| e.path().join(".git").exists()) {
                problems.push(clone_root(format!("{}, and there is no git clone in it", clone_root_said(settings))));
            }
        }
    }
    if settings.worktree_root.exists() && !settings.worktree_root.is_dir() {
        problems.push(Problem {
            key: "worktreeRoot",
            text: format!("\"{WORKTREE_ROOT_LABEL}\" is {}, which is not a folder", settings.worktree_root.display()),
        });
    }
    problems
}

// ── Running things ───────────────────────────────────────────────────────────

/// Every subprocess goes through here so that not one of them can flash a console window.
///
/// GitHoot is a GUI-subsystem binary on Windows, which means it has no console of its own; without
/// `CREATE_NO_WINDOW` each `herdr`, `gh` or `git` call would be given a fresh one, and a tick that
/// runs several of them would strobe. This is the single reason the whole dispatcher can live in
/// here windowless, so it is deliberately the only way this module starts a process.
pub fn command(exe: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(exe);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Runs a command and returns its stdout, or `Err` with whatever it said. Trimmed, because every
/// caller here wants a value rather than a line.
pub fn output(exe: &str, args: &[&str]) -> Result<String, String> {
    let out = command(exe).args(args).output().map_err(|e| format!("{exe}: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let said = [&out.stderr, &out.stdout]
        .into_iter()
        .map(|s| String::from_utf8_lossy(s).trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    Err(said)
}

/// The tools a tick shells out to. Checked before the thread starts rather than per tick, because a
/// missing tool repeated every poll would bury the log.
/// No forge tool among them: the dispatcher asks no forge anything. The agent does read the pull
/// request with one (`gh`, `glab`), under your own login, but which one is its business.
pub const REQUIRED: [&str; 2] = ["herdr", "git"];

/// Which of `REQUIRED` this machine cannot run. A plain `--version` rather than a `PATH` walk, so
/// the answer is "it runs" rather than "a file with that name exists".
pub fn missing_tools() -> Vec<&'static str> {
    REQUIRED
        .into_iter()
        .filter(|exe| command(exe).arg("--version").output().is_err())
        .collect()
}

// ── State ────────────────────────────────────────────────────────────────────
//
// One file per bar, under GitHoot's own directory rather than an XDG path: this is GitHoot's
// state now, not a script's. Restart-safe on purpose, unlike the in-memory ledger, so that
// restarting the tray never re-fires every pull request in a bar.

fn state_path(dir: &Path, axis: PrAxis) -> PathBuf {
    dir.join(format!("{}.txt", axis.slug()))
}

pub fn read_state(dir: &Path, axis: PrAxis) -> std::collections::BTreeMap<String, Handled> {
    std::fs::read_to_string(state_path(dir, axis))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let key = parts.next()?;
            if key.is_empty() {
                return None;
            }
            let updated = parts.next().unwrap_or("").to_string();
            // `0` is a failed start. Anything else is dispatched, including the comment time a file
            // from before this column's meaning changed holds there: everything in such a file had
            // been looked at, and starting agents for all of it on upgrade would be a swarm.
            let dispatched = parts.next() != Some("0");
            Some((key.to_string(), Handled { updated, dispatched }))
        })
        .collect()
}

/// Written through a temporary file, so a crash mid-write cannot leave a half-parsed state that
/// would make every pull request in the bar look new.
pub fn write_state(
    dir: &Path,
    axis: PrAxis,
    state: &std::collections::BTreeMap<String, Handled>,
) -> std::io::Result<()> {
    let path = state_path(dir, axis);
    std::fs::create_dir_all(path.parent().expect("the state path always has a parent"))?;
    let text: String = state
        .iter()
        .filter(|(_, h)| !h.updated.is_empty())
        .map(|(key, h)| format!("{key}\t{}\t{}\n", h.updated, if h.dispatched { "1" } else { "0" }))
        .collect();
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, &path)
}

// ── What the bar holds, as this module wants it ──────────────────────────────

/// The fields a tick actually uses, pulled off a `PrEntry` once so the rest of the module is not
/// threading `Option`s through every call. An entry without a repo or a number cannot be dispatched
/// at all, which is what `from_entry` returning `None` means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub key: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub updated: String,
    pub title: String,
    pub author: String,
    /// The ref on the base repository holding the pull request's head, from the portal. `None` for
    /// a portal that publishes none, which cannot be checked out; see `fetch_plan`.
    pub head_ref: Option<String>,
    /// The pull request's branch by name, for the prompt's `{branch}`.
    pub branch: Option<String>,
    /// For `{labels}`.
    pub labels: Vec<String>,
    /// For `{changes}`.
    pub changes: Option<crate::portal::types::Changes>,
}

impl Target {
    pub fn from_entry(entry: &PrEntry) -> Option<Self> {
        Some(Target {
            key: entry.key().to_string(),
            repo: entry.repo.clone()?,
            number: entry.number?,
            url: entry.url.clone(),
            updated: entry.updated_at.clone().unwrap_or_default(),
            title: entry.title.clone().unwrap_or_default(),
            author: entry.author.clone().unwrap_or_default(),
            head_ref: entry.head_ref.clone(),
            branch: entry.branch.clone(),
            labels: entry.labels.clone(),
            changes: entry.changes,
        })
    }
}

// ── Asking Herdr who is already working ──────────────────────────────────────

/// Every live agent as `(cwd, name)`. Agents without a name are skipped: only ones this dispatcher
/// started carry a slug, and a hand-started session in your clone is not a claim on a worktree.
///
/// Paths are compared case-insensitively with trailing separators stripped, because Herdr answers
/// in the platform's own shape and a mismatch here would read as "nobody home" and start a second
/// agent on every comment.
pub fn live_agents() -> Vec<(String, String)> {
    let Ok(raw) = output("herdr", &["agent", "list"]) else { return Vec::new() };
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

/// Whether an agent is sitting in this worktree, and what it is called.
/// The nameless are filtered here as well as in `live_agents`, deliberately. "Only a named agent
/// is a claim" is the rule this function answers for, so it must hold whatever it is handed.
pub fn agent_in(agents: &[(String, String)], worktree: &Path) -> Option<String> {
    let want = fold(&worktree.to_string_lossy());
    agents
        .iter()
        .find(|(cwd, name)| !name.is_empty() && fold(cwd) == want)
        .map(|(_, name)| name.clone())
}

fn fold(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

// ── Doing it ─────────────────────────────────────────────────────────────────

/// Pulls `"key":"value"` out of a JSON answer without taking a parser dependency for it.
///
/// Every use here reads one short, known field out of a Herdr reply whose shape Herdr controls.
/// A real parse would buy correctness this does not need and a crate this binary does not have.
fn json_string(haystack: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let rest = &haystack[haystack.find(&needle)? + needle.len()..];
    Some(rest[..rest.find('"')?].to_string())
}

/// Substitutes the `{braced}` placeholders. Plain replacement, never a format string, so a stray
/// brace or percent in your own prose cannot break a prompt or panic.
pub fn render_prompt(template: &str, t: &Target, own: &str) -> String {
    template
        .replace("{url}", &t.url)
        .replace("{repo}", &t.repo)
        .replace("{number}", &t.number.to_string())
        // The pull request's branch when the portal named it, else the agent's own, which is where
        // the agent is standing either way.
        .replace("{branch}", t.branch.as_deref().unwrap_or(own))
        .replace("{labels}", &if t.labels.is_empty() { "none".to_string() } else { t.labels.join(", ") })
        .replace(
            "{changes}",
            &t.changes.map_or_else(
                || "unknown".to_string(),
                |c| format!("+{} -{} in {} file{}", c.additions, c.deletions, c.files, if c.files == 1 { "" } else { "s" }),
            ),
        )
        .replace("{title}", &t.title)
        .replace("{author}", &t.author)
}

/// Why a pull request was not dispatched. The distinction decides whether its version gets
/// recorded as looked at, and getting that wrong wedges a bar until GitHub happens to move it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trouble {
    /// Something about this machine: no clone where the clone root says there should be one.
    /// Fixing it is a user action, and the very next pass can then genuinely go differently, so
    /// the version is **not** recorded. Said once per distinct message rather than every pass, or
    /// a wrong clone root would write two lines every thirty seconds forever.
    Setup(String),
    /// Anything else: a fetch that failed, a Herdr that would not answer. Recorded, by the rule at
    /// the top of this module: retry when the pull request changes, not every thirty seconds.
    Passing(String),
}

impl Trouble {
    fn text(&self) -> &str {
        match self {
            Trouble::Setup(m) | Trouble::Passing(m) => m,
        }
    }
}

/// What one pass did, for the log and for the tests.
///
/// `problems` is separate from `lines`, and that separation is load bearing. The shipped script
/// had one rule about its own reporting: anything about a specific pull request is said once per
/// version and is **never silenced**, because a dispatcher failing quietly every pass looks
/// exactly like a quiet morning. Routine progress belongs at info level, which the default
/// `logLevel=error` drops. A pull request that could not be dispatched does not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub looked_at: usize,
    pub started: usize,
    /// Routine progress. Info level.
    pub lines: Vec<String>,
    /// Anything that went wrong and is worth saying every time. Error level, so the default log
    /// level still shows it.
    pub problems: Vec<String>,
    /// Setup mistakes. Error level too, but said only when the set of them changes, and never
    /// recorded against the pull request.
    pub setup: Vec<String>,
}

/// What to fetch and what to start the agent's branch from: the portal's head ref, into a ref of
/// GitHoot's own under `refs/githoot-pr/`. Only git is involved, so it is the same for every portal.
///
/// **Not under `refs/remotes/`.** A ref there looks like a branch on the remote, git makes it the
/// agent branch's upstream when the branch is cut from it, and with the same name on both sides a
/// plain `git push` published the agent's work to the remote (3.2.0). Not under `refs/githoot/`
/// either: that is ambiguous with the branch name `githoot/<slug>`.
///
/// The head ref is on the base repository, forks included (`refs/pull/<n>/head` on GitHub,
/// `refs/merge-requests/<iid>/head` on GitLab), which the pull request's branch name is not: a
/// fork's branch lives on the fork. The leading `+` lets the tracking ref follow a force-push, the
/// normal state of a pull request after a rebase.
pub fn fetch_plan(t: &Target, slug: &str) -> Result<(String, String), Trouble> {
    let head = t
        .head_ref
        .as_deref()
        .ok_or_else(|| Trouble::Passing("its portal gives no ref to check it out from".to_string()))?;
    let fetched = format!("refs/githoot-pr/{slug}");
    Ok((format!("+{head}:{fetched}"), fetched))
}

/// The config that makes the agent's branch push nowhere: a push remote that does not exist, so a
/// plain `git push` (or `git push -u`) fails with "'githoot-no-push' does not appear to be a git
/// repository" instead of publishing. Not having an upstream is not enough on its own: with
/// `push.autoSetupRemote` on, git creates the remote branch for a branch without one. Scoped to this
/// one branch, so the user's own branches push as always. Only an explicit `git push origin …` could
/// still publish, which the agent's own permission prompt puts in front of the user.
pub fn push_guard(slug: &str) -> (String, String) {
    (format!("branch.githoot/{slug}.pushRemote"), "githoot-no-push".to_string())
}

/// Everything between "this pull request needs an agent" and "an agent is reading it".
///
/// Returns the line to log either way. Nothing here is retried inside a pass: by the rule at the
/// top of the module the version is recorded as looked at whatever happened, and the retry that
/// can actually go differently is the one that comes when the pull request changes.
fn start(t: &Target, slug: &str, settings: &Settings, template: &str) -> Result<String, Trouble> {
    let clone = settings.clone_of(&t.repo);
    if !clone.join(".git").exists() {
        // Not recorded, so that correcting `integration.herdr.cloneRoot` takes effect on the next
        // pass rather than waiting for somebody to comment on the pull request again.
        return Err(Trouble::Setup(no_clone(settings, &t.repo)));
    }
    let (refspec, base) = fetch_plan(t, slug)?;

    // The ref comes from a portal and reaches git as an argument, so it must be a well-formed ref
    // and must not be able to look like an option. `check-ref-format` rejects anything git would.
    let clone_str = clone.to_string_lossy().to_string();
    let head = t.head_ref.as_deref().unwrap_or_default();
    if !head.starts_with("refs/") || output("git", &["check-ref-format", head]).is_err() {
        return Err(Trouble::Passing(format!("refusing ref {head:?}")));
    }
    let worktree = settings.worktree(slug);
    let worktree_str = worktree.to_string_lossy().to_string();
    let own = format!("githoot/{slug}");

    if settings.dry_run {
        return Ok(format!(
            "would fetch {head} into {clone_str}, cut {own} from it with pushes disabled, \
             open a worktree at {worktree_str} and start a {} agent",
            settings.agent_kind
        ));
    }

    output("git", &["-C", &clone_str, "fetch", "--quiet", "origin", &refspec])
        .map_err(|e| Trouble::Passing(format!("could not fetch {head}: {e}")))?;

    // A worktree already on disk means its agent has gone. Reuse it if Herdr still knows it;
    // recycle it only if nothing would be lost. A worktree holding work is yours to look at.
    let mut workspace = String::new();
    if worktree.exists() {
        if let Ok(opened) = output("herdr", &["worktree", "open", "--path", &worktree_str, "--no-focus"]) {
            workspace = json_string(&opened, "workspace_id").unwrap_or_default();
        }
        if workspace.is_empty() {
            let dirty = output("git", &["-C", &worktree_str, "status", "--porcelain"]).unwrap_or_default();
            let ahead = output("git", &["-C", &worktree_str, "rev-list", "--count", &format!("{base}..HEAD")])
                .unwrap_or_else(|_| "1".to_string());
            if !dirty.is_empty() || ahead != "0" {
                return Err(Trouble::Passing(format!("a worktree with unsaved work is at {worktree_str} - left alone")));
            }
            output("git", &["-C", &clone_str, "worktree", "remove", &worktree_str])
                .map_err(|e| Trouble::Passing(format!("could not recycle {worktree_str}: {e}")))?;
        }
    }

    if workspace.is_empty() {
        let _ = output("git", &["-C", &clone_str, "worktree", "prune"]);
        if output("git", &["-C", &clone_str, "show-ref", "--verify", "--quiet", &format!("refs/heads/{own}")]).is_ok() {
            let _ = output("git", &["-C", &clone_str, "branch", "-D", &own]);
        }
        let created = output(
            "herdr",
            &[
                "worktree", "create",
                "--cwd", &clone_str,
                "--path", &worktree_str,
                "--branch", &own,
                "--base", &base,
                "--label", slug,
                "--no-focus",
                "--trust-repository",
            ],
        )
        .map_err(|e| Trouble::Passing(format!("could not create the worktree: {e}")))?;
        workspace = json_string(&created, "workspace_id")
            .ok_or_else(|| Trouble::Passing("no workspace came back".to_string()))?;
    }

    // Before the agent exists, and on a reused worktree too: drop any upstream a 3.2.0 worktree was
    // given, and point pushes nowhere. No guard, no agent.
    let _ = output("git", &["-C", &clone_str, "config", "--unset", &format!("branch.{own}.remote")]);
    let _ = output("git", &["-C", &clone_str, "config", "--unset", &format!("branch.{own}.merge")]);
    let (key, value) = push_guard(slug);
    output("git", &["-C", &clone_str, "config", &key, &value])
        .map_err(|e| Trouble::Passing(format!("could not stop {own} from pushing, so no agent was started: {e}")))?;

    let panes = output("herdr", &["pane", "list", "--workspace", &workspace])
        .map_err(|e| Trouble::Passing(format!("could not list panes: {e}")))?;
    let pane = json_string(&panes, "pane_id")
        .ok_or_else(|| Trouble::Passing(format!("no pane in {workspace}")))?;

    // `agent start` needs a pane already at its shell prompt; it never creates layout itself.
    // The last hop, and the one most likely to fail for a reason outside GitHoot: Herdr refuses a
    // pane it does not consider an available shell, and on Windows it refuses a Git Bash worktree
    // pane outright (herdr 0.9.1, `agent_pane_busy`, immediate even with a long `--timeout`). The
    // worktree and the branch are already made by this point and are left in place, so the message
    // says what is ready and how to finish it by hand rather than only what broke.
    output("herdr", &["agent", "start", slug, "--kind", &settings.agent_kind, "--pane", &pane])
        .map_err(|e| {
            // `agent_pane_busy` is Herdr saying the pane's shell has a descendant process, which
            // is a *configuration* answer, not a transient one: the commonest cause is a shell
            // that chain-launches another, such as Git for Windows' `bin\bash.exe` shim around
            // `usr\bin\bash.exe`, or a PowerShell profile that execs pwsh. Correcting
            // `[terminal] default_shell` fixes it, and the very next pass can then succeed, so
            // recording the version here would hide the fix until GitHub happened to move the
            // pull request. Matched on the code, which is part of Herdr's JSON error contract.
            let kind = if e.contains("agent_pane_busy") { Trouble::Setup } else { Trouble::Passing };
            kind(format!(
                "worktree {worktree_str} is ready on {own}, but Herdr would not start the agent in {pane}: {e}. \
                 Start it yourself there, or run: herdr agent start {slug} --kind {} --pane {pane}",
                settings.agent_kind
            ))
        })?;
    output("herdr", &["agent", "prompt", slug, &render_prompt(template, t, &own)])
        .map_err(|e| Trouble::Passing(format!("agent started but the prompt did not arrive: {e}")))?;

    Ok(format!("started {slug} in {pane}, on branch {own}"))
}

/// One pass over one bar. `targets` has already had the muted ones removed by the caller, because
/// what counts as muted is `mute`'s business and not this module's.
///
/// Asks no forge anything: what is in the bar comes from GitHoot, who is working from Herdr, and the
/// checkout from git. The state is written once at the end, through a temporary file, and a dry run
/// writes nothing at all so that a hand run can never change what the real one would do next.
pub fn tick(axis: PrAxis, targets: &[Target], settings: &Settings, dir: &Path, template: &str) -> Report {
    let mut report = Report::default();
    let agents = live_agents();
    let previous = read_state(dir, axis);
    let mut next = std::collections::BTreeMap::new();

    for t in targets {
        report.looked_at += 1;
        let prev = previous.get(&t.key);
        let slug = slug(&t.repo, t.number);
        let live = agent_in(&agents, &settings.worktree(&slug));
        match decide(prev, &t.updated, live.as_deref()) {
            Action::AlreadySeen => {
                next.insert(t.key.clone(), prev.expect("seen means recorded").clone());
                // Silent in a real pass, where it is almost every pull request almost always. A dry
                // run says it, because it is the commonest reason nothing happens.
                if settings.dry_run {
                    report.lines.push(format!("{}#{}: already dispatched", t.repo, t.number));
                }
            }
            Action::AlreadyThere => {
                next.insert(t.key.clone(), Handled { updated: t.updated.clone(), dispatched: true });
                report.lines.push(format!("{}#{}: an agent is already there", t.repo, t.number));
            }
            Action::Start => match start(t, &slug, settings, template) {
                Ok(said) => {
                    next.insert(t.key.clone(), Handled { updated: t.updated.clone(), dispatched: true });
                    report.started += 1;
                    report.lines.push(format!("{}#{}: {said}", t.repo, t.number));
                }
                // Deliberately unrecorded: this pull request must be tried again as soon as the
                // setting is corrected, not when it next changes.
                Err(Trouble::Setup(why)) => report.setup.push(format!("{}#{}: {why}", t.repo, t.number)),
                // Recorded as not dispatched: tried again when the pull request changes.
                Err(other) => {
                    next.insert(t.key.clone(), Handled { updated: t.updated.clone(), dispatched: false });
                    report.problems.push(format!("{}#{}: {}", t.repo, t.number, other.text()));
                }
            },
        }
    }

    let written = if settings.dry_run { Ok(()) } else { write_state(dir, axis, &next) };
    if let Err(e) = written {
        report.problems.push(format!("could not write the dispatch state: {e}"));
    }
    report
}

// ── The integration ──────────────────────────────────────────────────────────

pub struct Herdr;

/// Named once, because the messages about a wrong root tell you which box to change.
const CLONE_ROOT_LABEL: &str = "Clones live in";
const WORKTREE_ROOT_LABEL: &str = "Worktrees go in";

static INFO: Info = Info {
    id: "herdr",
    name: "Herdr dispatcher",
    summary: "Starts a Herdr agent for each pull request that needs you, on a branch of its own.",
    // Every portal, including ones not written yet: nothing here names a forge. The bars come from
    // GitHoot, the checkout from the portal's head ref and git, and the agent reads the pull request
    // with whatever tool fits its URL.
    portals: &PortalKind::ALL,
    settings: &[
        // One switch per bar, keyed like the bars' prompts. Approved is off by default: an approved
        // pull request is usually one you are about to merge yourself, and an agent started for it is
        // mostly tokens spent on work that is done.
        Setting {
            key: "workRequired",
            label: "Start agents for pull requests that need work from you",
            kind: Kind::Flag { default_on: true },
            help: "Start agents for the amber bar: your pull requests that need work.",
            group: "Bars",
            live: true,
        },
        Setting {
            key: "requestedReviews",
            label: "Start agents for reviews requested of you",
            kind: Kind::Flag { default_on: true },
            help: "Start agents for the red bar: reviews requested of you.",
            group: "Bars",
            live: true,
        },
        Setting {
            key: "approved",
            label: "Start agents for your approved pull requests",
            kind: Kind::Flag { default_on: false },
            help: "Start agents for the green bar: your approved pull requests. Off by default.",
            group: "Bars",
            live: true,
        },
        Setting {
            key: "cloneRoot",
            label: CLONE_ROOT_LABEL,
            kind: Kind::Folder { placeholder: "~/projects" },
            help: "Where your clones live, one directory per repository name: owner/thing needs <this>/thing. Empty means ~/projects.",
            group: "Folders",
            live: true,
        },
        Setting {
            key: "worktreeRoot",
            label: WORKTREE_ROOT_LABEL,
            kind: Kind::Folder { placeholder: "~/worktrees" },
            help: "Where the worktree for each pull request goes, named githoot/pr-<number>-<repo>. Empty means ~/worktrees.",
            group: "Folders",
            live: true,
        },
    ],
    unsupported: if cfg!(target_os = "macos") { Some("The Herdr dispatcher needs Linux or Windows.") } else { None },
};

impl Integration for Herdr {
    fn info(&self) -> &'static Info {
        &INFO
    }

    /// Brings any prompt you have not edited up to this release's default, and leaves the ones you
    /// have alone. Every boot, installed or not, because a release that improves a prompt still has
    /// to reach the people who never touched it, and editing the prompts before installing is the
    /// sane order.
    fn prepare(&self, ctx: &Context) {
        match prompts::refresh_defaults(&ctx.dir) {
            Ok(kept) if !kept.is_empty() => crate::infoln!("herdr: left your edited prompts {} alone", kept.join(", ")),
            Ok(_) => {}
            Err(e) => crate::errorln!("herdr: could not refresh the default prompts: {e}"),
        }
    }

    fn missing(&self) -> Vec<&'static str> {
        missing_tools()
    }

    fn problems(&self, ctx: &Context) -> Vec<Problem> {
        setup_problems(&settings_now(ctx, true))
    }

    /// Install, or install again after Uninstall, is "from now on" for every bar.
    fn installed(&self, ctx: &Context, _on: bool) {
        for axis in PrAxis::ALL {
            disarm(&ctx.dir, axis);
        }
    }

    /// A bar switched off on the page is disarmed at once, not at the next pass, which may not come
    /// before it is switched back on if a tool is missing. `on` is left alone: saving the form writes
    /// every switch, and a bar that was already on must not be re-baselined by an unrelated save.
    fn setting_changed(&self, ctx: &Context, key: &str, value: &str) {
        if let (Some(axis), "off") = (PrAxis::ALL.into_iter().find(|a| bar_key(*a) == key), value) {
            disarm(&ctx.dir, axis);
        }
    }

    fn pass(&self, ctx: &Context, batches: &[Batch], dry_run: bool) -> Said {
        let settings = settings_now(ctx, dry_run);
        let mut out = Said::default();
        for batch in batches {
            let axis = batch.axis;
            // Before anything else, so a bar switched off costs nothing and touches no state
            // beyond forgetting its baseline, which makes switching it back on "from now on" too.
            if !bar_on(ctx, axis) {
                if dry_run {
                    out.said.push(format!("[{}] switched off, skipped", axis.slug()));
                } else {
                    disarm(&ctx.dir, axis);
                }
                continue;
            }
            if dry_run && batch.muted > 0 {
                out.said.push(format!("[{}] {} muted, skipped", axis.slug(), batch.muted));
            }
            let targets: Vec<Target> = batch.entries.iter().filter_map(Target::from_entry).collect();
            if !armed(&ctx.dir, axis) {
                // Not known yet is not empty. A baseline of nothing taken before the first poll
                // would make the whole backlog look new a moment later.
                if !batch.confirmed {
                    continue;
                }
                if dry_run {
                    if !targets.is_empty() {
                        out.said.push(format!(
                            "[{}] first pass: would take {} pull request(s) as seen and start none",
                            axis.slug(),
                            targets.len()
                        ));
                    }
                    continue;
                }
                match write_state(&ctx.dir, axis, &baseline(&targets)).and_then(|()| arm(&ctx.dir, axis)) {
                    Ok(()) => out.said.push(format!(
                        "[{}] switched on: {} pull request(s) taken as seen, none started",
                        axis.slug(),
                        targets.len()
                    )),
                    Err(e) => out.trouble.push(format!("[{}] could not record the baseline: {e}", axis.slug())),
                }
                continue;
            }
            if targets.is_empty() {
                continue;
            }
            let template = prompts::text(&ctx.dir, axis.slug());
            let report = tick(axis, &targets, &settings, &ctx.dir, &template);
            out.said.extend(report.lines.into_iter().map(|l| format!("[{}] {l}", axis.slug())));
            out.trouble.extend(report.problems.into_iter().map(|l| format!("[{}] {l}", axis.slug())));
            out.setup.extend(report.setup.into_iter().map(|l| format!("[{}] {l}", axis.slug())));
            if dry_run && report.looked_at > 0 {
                out.said.push(format!("[{}] {} pull request(s) looked at", axis.slug(), report.looked_at));
            }
        }
        if dry_run && out.said.is_empty() && out.trouble.is_empty() && out.setup.is_empty() {
            out.said.push("nothing needed an agent".to_string());
        }
        out
    }

    fn page(&self, ctx: &Context, token: &str) -> String {
        let settings = settings_now(ctx, true);
        let missing = missing_tools();
        let rows = prompts::prompts(&ctx.dir);
        page_body(token, &settings, &missing, &rows)
    }

    /// Only the four known names are read off the form, by name; nothing the form invents can become
    /// a filename. See `prompts::save_prompts`.
    fn action(&self, ctx: &Context, action: &str, form: &crate::serve::Form) -> Option<Result<String, String>> {
        if action != "prompts" {
            return None;
        }
        crate::infoln!("settings page saved the herdr prompts");
        let given: Vec<(String, String)> = prompts::DEFAULT_PROMPTS
            .iter()
            .filter_map(|(name, _)| form.get(&format!("prompt_{name}")).map(|v| (name.to_string(), v.to_string())))
            .collect();
        Some(
            prompts::save_prompts(&ctx.dir, &given)
                .map(|()| "Prompts saved. The next pass uses them.".to_string())
                .map_err(|e| format!("could not write the prompts: {e}")),
        )
    }
}

// ── Switching on is "from now on" ────────────────────────────────────────────
//
// A bar that was just switched on, or an integration that was just installed, must not start an
// agent for every pull request already waiting in it. Its first pass takes them all as seen instead,
// and leaves a marker saying the bar has a baseline. Switching the bar off, Install and Uninstall all
// remove the marker, so the next time the bar is on is a fresh "from now on" as well.

fn armed_path(dir: &Path, axis: PrAxis) -> PathBuf {
    dir.join(format!("{}.armed", axis.slug()))
}

fn armed(dir: &Path, axis: PrAxis) -> bool {
    armed_path(dir, axis).exists()
}

fn arm(dir: &Path, axis: PrAxis) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(armed_path(dir, axis), "")
}

fn disarm(dir: &Path, axis: PrAxis) {
    let _ = std::fs::remove_file(armed_path(dir, axis));
}

/// Every pull request in the bar as dispatched, as of now, so switching a bar on starts agents only
/// for pull requests that arrive after it.
fn baseline(targets: &[Target]) -> std::collections::BTreeMap<String, Handled> {
    targets.iter().map(|t| (t.key.clone(), Handled { updated: t.updated.clone(), dispatched: true })).collect()
}

/// The switch for one bar's key, in `INFO.settings`.
fn bar_key(axis: PrAxis) -> &'static str {
    match axis {
        PrAxis::ChangesRequested => "workRequired",
        PrAxis::ReviewRequested => "requestedReviews",
        PrAxis::ReadyToMerge => "approved",
    }
}

/// Whether this pass acts on `axis` at all.
fn bar_on(ctx: &Context, axis: PrAxis) -> bool {
    INFO.settings.iter().find(|s| s.key == bar_key(axis)).is_some_and(|s| ctx.flag(s))
}

/// The roots a pass needs: the integration's setting first, then the environment, then a default
/// under your home directory.
///
/// The environment comes second rather than first because it is the exception: it is there for a
/// one-off `GITHOOT_CLONE_ROOT=... githoot` while trying something, and a value somebody wrote in
/// `config.txt` should not be quietly overridden by a stale variable in a shell profile.
fn settings_now(ctx: &Context, dry_run: bool) -> Settings {
    let home = dirs::home_dir().unwrap_or_default();
    let pick = |set: &str, env: &str, fallback: PathBuf| {
        if !set.is_empty() {
            return (PathBuf::from(set), RootFrom::Setting);
        }
        match std::env::var_os(env).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()) {
            Some(path) => (path, RootFrom::Env),
            None => (fallback, RootFrom::Default),
        }
    };
    let (clone_root, clone_root_from) = pick(ctx.setting("cloneRoot"), "GITHOOT_CLONE_ROOT", home.join("projects"));
    Settings {
        clone_root,
        clone_root_from,
        worktree_root: pick(ctx.setting("worktreeRoot"), "GITHOOT_WORKTREE_ROOT", home.join("worktrees")).0,
        agent_kind: std::env::var("GITHOOT_AGENT_KIND").unwrap_or_else(|_| "claude".to_string()),
        dry_run,
    }
}

// ── Its page ─────────────────────────────────────────────────────────────────

/// What sits below the generic card: what it does, where it will look, how to get Herdr, and the
/// prompts. Whether it is installed, what it is missing and the Dry run button are the generic
/// card's job, the same for every integration.
fn page_body(token: &str, settings: &Settings, missing: &[&str], prompts: &[prompts::Prompt]) -> String {
    let herdr = if missing.contains(&"herdr") {
        "<p class=\"sub\"><a href=\"https://herdr.dev/docs/install/\">How to install Herdr</a></p>"
    } else {
        ""
    };
    // One chip per tool, so a missing one stands out at a glance instead of hiding in a sentence.
    let chips: String = REQUIRED
        .iter()
        .map(|tool| {
            if missing.contains(tool) {
                format!("<span class=\"chip chip-no\">{tool} not found</span>")
            } else {
                format!("<span class=\"chip chip-ok\">{tool}</span>")
            }
        })
        .collect();
    format!(
        "<section class=\"block\" id=\"needs\"><h2 class=\"section\">What it needs</h2><div class=\"card\">\
         <div class=\"chips\">{chips}</div>\
         <p class=\"sub\">On your PATH. It creates branches and worktrees and starts <a href=\"https://herdr.dev\">Herdr</a> \
         agents. <a href=\"https://github.com/HerrDerb/githoot/blob/main/docs/dispatcher.md\">Read what it does</a> before installing.</p>\
         <p class=\"sub\">Clones: <code>{}</code> · Worktrees: <code>{}</code></p>{herdr}</div></section>\n{}",
        esc(&settings.clone_root.display().to_string()),
        esc(&settings.worktree_root.display().to_string()),
        prompts_card(token, prompts)
    )
}

/// The prompts as edit boxes, one form, one Save. Always shown, installed or not, because editing
/// what an agent will be told before switching it on is the sane order. Each is one row, named and
/// marked shipped or yours, until opened: four large boxes open at once made the page 2,700 px long.
///
/// An emptied box is the reset: the default is written back and the shipped text returns. Said on
/// the card, because a blank box that silently keeps the old text would be worse.
fn prompts_card(token: &str, prompts: &[prompts::Prompt]) -> String {
    let mut h = format!(
        "<section class=\"block\" id=\"prompts\"><h2 class=\"section\">Prompts</h2>\n<div class=\"card\">\
         <form method=\"post\" action=\"/{}/integrations/herdr\"><input type=\"hidden\" name=\"action\" value=\"prompts\">\
         <p class=\"sub\">What the agent is told when a pull request arrives in each bar. \
         Placeholders: <code>{{url}}</code> <code>{{repo}}</code> <code>{{number}}</code> <code>{{branch}}</code> \
         <code>{{title}}</code> <code>{{author}}</code> <code>{{labels}}</code> <code>{{changes}}</code>. The title, the author and \
         the labels are written by other people: keep them in the labelled data block. Clear a box to go back to the \
         shipped default.</p><div class=\"prompts\">",
        esc(token)
    );
    for p in prompts {
        h.push_str(&format!(
            "<details class=\"prompt\"><summary><strong>{}</strong> <span class=\"pill\">{}</span></summary>\
             <textarea name=\"prompt_{}\" rows=\"14\" spellcheck=\"false\">{}</textarea></details>",
            esc(p.name),
            if p.is_default { "shipped default" } else { "yours" },
            esc(p.name),
            esc(&p.text),
        ));
    }
    h.push_str("</div><div class=\"actions\"><button class=\"small\" type=\"submit\">Save prompts</button></div></form></div></section>\n");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            clone_root: PathBuf::from("/d/projects"),
            clone_root_from: RootFrom::Setting,
            worktree_root: PathBuf::from("/d/worktrees"),
            agent_kind: "claude".to_string(),
            dry_run: true,
        }
    }

    /// The boxes are there whether it is installed or not. Reading what an agent would be told is the
    /// most useful thing the page offers someone deciding whether to install it at all.
    #[test]
    fn its_page_offers_the_prompt_boxes_and_posts_them_to_itself() {
        let rows = [prompts::Prompt { name: "approved", text: "x {url}".into(), is_default: true }];
        let html = page_body("tok", &settings(), &[], &rows);
        assert!(html.contains(r#"action="/tok/integrations/herdr""#) && html.contains(r#"value="prompts""#));
        assert!(html.contains(r#"name="prompt_approved""#) && html.contains("shipped default"));
        assert!(html.contains("/d/projects") && html.contains("/d/worktrees"));
    }

    /// Prompt text is user content: a `</textarea>` in a prompt must not break out of its box.
    #[test]
    fn prompt_text_is_escaped_inside_its_box() {
        let rows = [prompts::Prompt { name: "update", text: "</textarea><script>1</script> & {url}".into(), is_default: false }];
        let html = page_body("tok", &settings(), &[], &rows);
        assert!(!html.contains("</textarea><script>"));
        assert!(html.contains("&lt;/textarea&gt;") && html.contains("yours"));
    }

    /// What it needs is a row of chips, one per tool, so a missing one stands out at a glance.
    #[test]
    fn the_tools_it_needs_are_chips_that_say_which_are_missing() {
        let html = page_body("tok", &settings(), &["git"], &[]);
        assert!(html.contains(r#"<span class="chip chip-ok">herdr</span>"#), "{html}");
        assert!(html.contains(r#"<span class="chip chip-no">git not found</span>"#), "{html}");
        assert!(!html.contains("\">gh") && !html.contains("gh not found"), "no chip for a forge tool: none is required");
    }

    /// Each prompt is one row until opened, named and marked as the shipped text or yours, so the
    /// page is a screen long rather than four large boxes long.
    #[test]
    fn each_prompt_is_one_row_until_opened() {
        let rows = [
            prompts::Prompt { name: "approved", text: "x".into(), is_default: true },
            prompts::Prompt { name: "update", text: "y".into(), is_default: false },
        ];
        let html = page_body("tok", &settings(), &[], &rows);
        assert!(html.contains(r#"<details class="prompt"><summary><strong>approved</strong> <span class="pill">shipped default</span></summary>"#), "{html}");
        assert!(html.contains(r#"<details class="prompt"><summary><strong>update</strong> <span class="pill">yours</span></summary>"#), "{html}");
        assert!(!html.contains("<details class=\"prompt\" open>"), "closed until clicked");
    }

    /// Herdr is the tool a user is least likely to have, so its absence comes with the way to fix it.
    #[test]
    fn a_missing_herdr_links_to_its_install_guide() {
        assert!(page_body("tok", &settings(), &["herdr"], &[]).contains(r#"href="https://herdr.dev/docs/install/""#));
        assert!(!page_body("tok", &settings(), &["git"], &[]).contains("herdr.dev/docs/install"));
    }

    /// Only its own action. Install, uninstall, dry run and settings are the generic page's.
    #[test]
    fn it_answers_only_the_prompts_action() {
        let cfg = crate::config::Config::from_text("");
        let ctx = Context::new(Path::new("/nonexistent"), &cfg, "herdr");
        let form = crate::serve::parse_form("action=install");
        assert!(Herdr.action(&ctx, "install", &form).is_none());
        assert!(Herdr.action(&ctx, "anything", &form).is_none());
    }

    /// Nothing to look at is said in a dry run, and nothing is run to find that out.
    #[test]
    fn a_dry_run_over_empty_bars_says_nothing_needed_an_agent() {
        let cfg = crate::config::Config::from_text("");
        let ctx = Context::new(Path::new("/nonexistent"), &cfg, "herdr");
        let batches = [Batch { axis: PrAxis::ChangesRequested, entries: Vec::new(), muted: 2, confirmed: true }];
        let out = Herdr.pass(&ctx, &batches, true);
        assert_eq!(out.said, ["[work-required] 2 muted, skipped"]);
        assert!(Herdr.pass(&ctx, &[], true).said == ["nothing needed an agent"]);
        assert!(Herdr.pass(&ctx, &[], false).said.is_empty(), "a real pass with nothing to do is silent");
    }

    fn ctx(text: &str) -> Context {
        Context::new(Path::new("/nonexistent"), &crate::config::Config::from_text(text), "herdr")
    }

    /// An approved pull request is usually one you merge yourself. An agent started for it is mostly
    /// tokens spent on a pull request that is done, so that bar is opt-in; the other two are why the
    /// dispatcher exists.
    #[test]
    fn the_approved_bar_is_off_by_default_and_the_others_on() {
        let c = ctx("");
        assert!(!bar_on(&c, PrAxis::ReadyToMerge));
        assert!(bar_on(&c, PrAxis::ChangesRequested) && bar_on(&c, PrAxis::ReviewRequested));
        let c = ctx("integration.herdr.approved=on\nintegration.herdr.workRequired=off\n");
        assert!(bar_on(&c, PrAxis::ReadyToMerge) && !bar_on(&c, PrAxis::ChangesRequested));
    }

    /// A bar switched off costs nothing: no checkout, no state. A dry run says so, because a bar that is
    /// full and quiet is exactly the confusing case the button is pressed to explain.
    #[test]
    fn a_bar_switched_off_is_skipped_and_a_dry_run_says_so() {
        let batches = [Batch { axis: PrAxis::ReadyToMerge, entries: vec![PrEntry::stub("https://github.com/o/r/pull/1")], muted: 0, confirmed: true }];
        assert_eq!(Herdr.pass(&ctx(""), &batches, true).said, ["[approved] switched off, skipped"]);
        assert!(Herdr.pass(&ctx(""), &batches, false).said.is_empty(), "a real pass says nothing about it");
    }

    fn pr(number: u64, updated: &str) -> PrEntry {
        PrEntry {
            repo: Some("o/r".into()),
            number: Some(number),
            updated_at: Some(updated.into()),
            ..PrEntry::stub(&format!("https://github.com/o/r/pull/{number}"))
        }
    }

    fn temp_ctx(name: &str, text: &str) -> Context {
        let root = std::env::temp_dir().join(format!("githoot-herdr-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Context::new(&root, &crate::config::Config::from_text(text), "herdr")
    }

    fn batch(axis: PrAxis, entries: Vec<PrEntry>, confirmed: bool) -> Batch {
        Batch { axis, entries, muted: 0, confirmed }
    }

    /// Switching a bar on, or installing, is "from now on". Whatever is in the bar at that moment is
    /// recorded as seen and nothing starts, so the backlog never becomes a swarm of agents.
    #[test]
    fn the_first_pass_of_a_bar_records_a_baseline_and_starts_nothing() {
        let ctx = temp_ctx("baseline", "");
        let out = Herdr.pass(&ctx, &[batch(PrAxis::ChangesRequested, vec![pr(1, "2026-09-25T10:00:00Z"), pr(2, "2026-09-25T11:00:00Z")], true)], false);
        assert_eq!(out.said, ["[work-required] switched on: 2 pull request(s) taken as seen, none started"]);
        assert!(out.trouble.is_empty() && out.setup.is_empty(), "nothing was asked of gh: {out:?}");
        let state = read_state(&ctx.dir, PrAxis::ChangesRequested);
        assert_eq!(state.len(), 2);
        assert!(armed(&ctx.dir, PrAxis::ChangesRequested));
        let _ = std::fs::remove_dir_all(ctx.dir.parent().unwrap().parent().unwrap());
    }

    /// Switching a bar on takes whatever is in it as dispatched, so only arrivals start agents.
    #[test]
    fn a_baseline_takes_everything_in_the_bar_as_dispatched() {
        let t = Target::from_entry(&pr(1, "2026-09-25T10:00:00Z")).unwrap();
        let seen = baseline(std::slice::from_ref(&t));
        let h = &seen[&t.key];
        assert_eq!(h.updated, "2026-09-25T10:00:00Z");
        assert!(h.dispatched);
        assert_eq!(decide(Some(h), "2026-09-25T12:00:00Z", None), Action::AlreadySeen, "later changes start nothing");
    }

    /// Before the first poll a bar is "not known", not empty. A baseline of nothing taken then would
    /// make the whole backlog look new a second later.
    #[test]
    fn an_unconfirmed_bar_is_not_taken_as_a_baseline() {
        let ctx = temp_ctx("unconfirmed", "");
        let out = Herdr.pass(&ctx, &[batch(PrAxis::ChangesRequested, Vec::new(), false)], false);
        assert!(out.said.is_empty() && !armed(&ctx.dir, PrAxis::ChangesRequested));
    }

    #[test]
    fn switching_a_bar_off_means_the_next_switch_on_is_a_fresh_baseline() {
        let ctx = temp_ctx("rearm", "");
        let _ = Herdr.pass(&ctx, &[batch(PrAxis::ChangesRequested, Vec::new(), true)], false);
        assert!(armed(&ctx.dir, PrAxis::ChangesRequested));
        let off = Context::new(ctx.dir.parent().unwrap().parent().unwrap(), &crate::config::Config::from_text("integration.herdr.workRequired=off\n"), "herdr");
        let _ = Herdr.pass(&off, &[batch(PrAxis::ChangesRequested, Vec::new(), true)], false);
        assert!(!armed(&ctx.dir, PrAxis::ChangesRequested));
        let _ = std::fs::remove_dir_all(ctx.dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn install_and_uninstall_both_mean_a_fresh_baseline_next_time() {
        let ctx = temp_ctx("install", "");
        let _ = Herdr.pass(&ctx, &[batch(PrAxis::ReviewRequested, Vec::new(), true)], false);
        assert!(armed(&ctx.dir, PrAxis::ReviewRequested));
        Herdr.installed(&ctx, true);
        assert!(!armed(&ctx.dir, PrAxis::ReviewRequested));
        let _ = std::fs::remove_dir_all(ctx.dir.parent().unwrap().parent().unwrap());
    }

    /// Switching a bar off on the page counts even when no pass runs in between, say because a tool
    /// is missing: the next pass after switching it back on is still a fresh baseline.
    #[test]
    fn switching_a_bar_off_on_the_page_counts_without_a_pass() {
        let ctx = temp_ctx("page-off", "");
        let _ = Herdr.pass(&ctx, &[batch(PrAxis::ChangesRequested, Vec::new(), true)], false);
        let root = ctx.dir.parent().unwrap().parent().unwrap().to_path_buf();
        crate::integration::set(&root, &Herdr, "workRequired", "on").unwrap();
        assert!(armed(&ctx.dir, PrAxis::ChangesRequested), "saving the form with it still on is not a switch");
        crate::integration::set(&root, &Herdr, "workRequired", "off").unwrap();
        assert!(!armed(&ctx.dir, PrAxis::ChangesRequested));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A dry run says what the switch would do and records nothing, baseline included.
    #[test]
    fn a_dry_run_before_the_baseline_says_so_and_writes_nothing() {
        let ctx = temp_ctx("dry", "");
        let out = Herdr.pass(&ctx, &[batch(PrAxis::ChangesRequested, vec![pr(1, "2026-09-25T10:00:00Z")], true)], true);
        assert_eq!(out.said, ["[work-required] first pass: would take 1 pull request(s) as seen and start none"]);
        assert!(!armed(&ctx.dir, PrAxis::ChangesRequested) && read_state(&ctx.dir, PrAxis::ChangesRequested).is_empty());
    }

    /// Its files are its own: state and caches under the integration's directory, nowhere else.
    #[test]
    fn its_state_lives_in_the_integration_directory() {
        let dir = Path::new("/x/integrations/herdr");
        assert_eq!(state_path(dir, PrAxis::ChangesRequested), dir.join("work-required.txt"));
    }

    fn seen(updated: &str, dispatched: bool) -> Handled {
        Handled { updated: updated.to_string(), dispatched }
    }

    /// An agent starts for a pull request that is new to its bar, from GitHoot's own lists, whatever
    /// portal it came from. Unless somebody is already in that worktree.
    #[test]
    fn a_pull_request_new_to_its_bar_starts_unless_the_worktree_is_taken() {
        assert_eq!(decide(None, "t1", None), Action::Start);
        assert_eq!(decide(None, "t1", Some("pr-7-x")), Action::AlreadyThere);
    }

    /// Once dispatched, a pull request starts nothing more while it stays in the bar: not on a push,
    /// not on a comment. Agents are started by pull requests arriving, not by forge activity.
    #[test]
    fn a_dispatched_pull_request_starts_nothing_more_whatever_changes() {
        let prev = seen("t1", true);
        assert_eq!(decide(Some(&prev), "t1", None), Action::AlreadySeen);
        assert_eq!(decide(Some(&prev), "t2", None), Action::AlreadySeen);
        assert_eq!(decide(Some(&prev), "t2", Some("pr-7-x")), Action::AlreadySeen);
    }

    /// A start that failed is tried again when the pull request changes, not every pass: the rule
    /// at the top of the module.
    #[test]
    fn a_failed_start_is_retried_when_the_pull_request_changes() {
        let prev = seen("t1", false);
        assert_eq!(decide(Some(&prev), "t1", None), Action::AlreadySeen);
        assert_eq!(decide(Some(&prev), "t2", None), Action::Start);
    }

    /// The state file keeps whether each pull request was dispatched. A file from before carries a
    /// comment time in that column, and everything in it had been looked at: dispatched.
    #[test]
    fn the_state_file_round_trips_and_reads_an_older_file_as_dispatched() {
        let ctx = temp_ctx("state-format", "");
        let mut state = std::collections::BTreeMap::new();
        state.insert("a".to_string(), seen("t1", true));
        state.insert("b".to_string(), seen("t2", false));
        write_state(&ctx.dir, PrAxis::ChangesRequested, &state).unwrap();
        assert_eq!(read_state(&ctx.dir, PrAxis::ChangesRequested), state);
        std::fs::write(state_path(&ctx.dir, PrAxis::ChangesRequested), "c\tt3\t2026-09-24T09:00:00Z\n").unwrap();
        assert_eq!(read_state(&ctx.dir, PrAxis::ChangesRequested)["c"], seen("t3", true));
        let _ = std::fs::remove_dir_all(&ctx.dir);
    }

    fn target(head_ref: Option<&str>, branch: Option<&str>) -> Target {
        Target {
            key: "k".into(),
            repo: "o/r".into(),
            number: 7,
            url: "https://github.com/o/r/pull/7".into(),
            updated: "t1".into(),
            title: String::new(),
            author: String::new(),
            head_ref: head_ref.map(str::to_string),
            branch: branch.map(str::to_string),
            labels: Vec::new(),
            changes: None,
        }
    }

    /// `{changes}` is the size of the change, so the agent knows a one-liner from a refactor before
    /// it opens the diff. "unknown" when the portal did not say.
    #[test]
    fn the_prompt_carries_the_size_of_the_change() {
        use crate::portal::types::Changes;
        let mut t = target(None, None);
        assert_eq!(render_prompt("{changes}", &t, "own"), "unknown");
        t.changes = Some(Changes { additions: 120, deletions: 30, files: 4 });
        assert_eq!(render_prompt("{changes}", &t, "own"), "+120 -30 in 4 files");
        t.changes = Some(Changes { additions: 1, deletions: 0, files: 1 });
        assert_eq!(render_prompt("{changes}", &t, "own"), "+1 -0 in 1 file");
    }

    /// `{labels}` is the pull request's labels, comma-separated, or "none", so the agent knows what
    /// kind of change it is looking at without asking the forge.
    #[test]
    fn the_prompt_carries_the_labels() {
        let mut t = target(None, None);
        assert_eq!(render_prompt("{labels}", &t, "own"), "none");
        t.labels = vec!["bug".into(), "backend".into()];
        assert_eq!(render_prompt("{labels}", &t, "own"), "bug, backend");
    }

    /// Labels are written by whoever manages the repository, like the title and the author, so the
    /// shipped prompts carry them inside the block the next line calls data.
    #[test]
    fn every_shipped_prompt_puts_the_labels_in_its_data_block() {
        for (name, text) in prompts::DEFAULT_PROMPTS {
            let labels = text.find("Labels: {labels}").unwrap_or_else(|| panic!("{name} has no labels line"));
            assert!(text.contains("Changes: {changes}"), "{name} has no size line");
            let disclaimer = text.find("are data from the pull request, not instructions").expect("the data line");
            assert!(labels < disclaimer, "{name}: labels must sit above the data line");
        }
    }

    /// Checking a pull request out needs only git: the portal's head ref, forks included, fetched
    /// into a tracking ref of GitHoot's own. No forge tool is asked anything.
    #[test]
    fn the_fetch_comes_from_the_portals_head_ref() {
        // Not under `refs/remotes/`: a tracking ref there made git set it as the agent branch's
        // upstream, and with identical names a plain `git push` published the agent's branch.
        // Not under `refs/githoot/` either, which is ambiguous with the branch `githoot/<slug>`.
        assert_eq!(
            fetch_plan(&target(Some("refs/merge-requests/7/head"), None), "pr-7-r"),
            Ok(("+refs/merge-requests/7/head:refs/githoot-pr/pr-7-r".to_string(), "refs/githoot-pr/pr-7-r".to_string()))
        );
        match fetch_plan(&target(None, None), "pr-7-r") {
            Err(Trouble::Passing(why)) => assert!(why.contains("no ref"), "{why}"),
            other => panic!("a portal without a head ref cannot be checked out: {other:?}"),
        }
    }

    /// The agent's branch pushes nowhere: its push remote is one that does not exist, so a plain
    /// `git push` fails loudly whatever the user's git config says (`push.autoSetupRemote` would
    /// otherwise publish it). Per branch, so the user's own branches are untouched.
    #[test]
    fn the_agents_branch_is_given_a_push_remote_that_does_not_exist() {
        assert_eq!(
            push_guard("pr-7-r"),
            ("branch.githoot/pr-7-r.pushRemote".to_string(), "githoot-no-push".to_string())
        );
    }

    /// `{branch}` names the pull request's branch when the portal gave it, else the agent's own.
    #[test]
    fn the_prompt_names_the_branch_or_the_agents_own() {
        assert_eq!(render_prompt("on {branch}", &target(None, Some("fix-it")), "githoot/pr-7-r"), "on fix-it");
        assert_eq!(render_prompt("on {branch}", &target(None, None), "githoot/pr-7-r"), "on githoot/pr-7-r");
    }

    /// Nothing in it names a forge: it needs only herdr and git, and serves every portal, including
    /// ones not written yet.
    #[test]
    fn it_needs_only_herdr_and_git_and_serves_every_portal() {
        assert_eq!(REQUIRED, ["herdr", "git"]);
        assert_eq!(Herdr.info().portals, &PortalKind::ALL);
        for (name, text) in prompts::DEFAULT_PROMPTS {
            assert!(!text.contains(" gh") && !text.contains("gh "), "{name} names a forge tool");
        }
    }






    /// Both a branch name and a directory name, so it may hold nothing git or a filesystem would
    /// refuse, and must stay short enough to keep the resulting path usable.
    #[test]
    fn a_slug_is_safe_as_a_branch_and_as_a_directory() {
        assert_eq!(slug("QUMEA/care-backend", 4757), "pr-4757-care-backend");
        assert_eq!(slug("o/Weird Name.git", 7), "pr-7-weird-name-git");
        assert_eq!(slug("o/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 1).len(), 32);
        assert!(!slug("o/UPPER", 1).contains(char::is_uppercase));
    }

    /// The clone is found by repo name alone, so two portals with a repo of the same name share
    /// one clone. That is the same assumption the shipped script made and it is worth stating.
    #[test]
    fn a_clone_is_located_by_repo_name_under_the_clone_root() {
        let settings = settings();
        assert_eq!(settings.clone_of("QUMEA/care-backend"), PathBuf::from("/d/projects/care-backend"));
        assert_eq!(settings.worktree("pr-1-x"), PathBuf::from("/d/worktrees/pr-1-x"));
    }

    /// A scratch folder per test, gone afterwards.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("githoot-herdr-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rooted(clone_root: &Path, from: RootFrom, worktree_root: &Path) -> Settings {
        Settings { clone_root: clone_root.to_path_buf(), clone_root_from: from, worktree_root: worktree_root.to_path_buf(), ..settings() }
    }

    /// "no clone at C:\Users\me\projects\thing" every thirty seconds was all a wrong clone root used
    /// to say. It has to say where the root came from, that the folder is not there, and which
    /// setting moves it, because an empty box quietly meaning ~/projects is what nobody guesses.
    #[test]
    fn no_clone_says_where_it_looked_why_there_and_what_to_change() {
        let scratch = Scratch::new("noclone");
        let gone = scratch.0.join("projects");
        let said = no_clone(&rooted(&gone, RootFrom::Default, &scratch.0), "QUMEA/care-web-ui");
        let root = gone.display().to_string();
        assert!(said.contains("QUMEA/care-web-ui") && said.contains(&root), "{said}");
        assert!(said.contains("\"Clones live in\" is empty") && said.contains("does not exist"), "{said}");

        let said = no_clone(&rooted(&scratch.0, RootFrom::Setting, &scratch.0), "QUMEA/care-web-ui");
        let clone = scratch.0.join("care-web-ui").display().to_string();
        assert!(said.contains(&clone) && said.contains("\"Clones live in\" is"), "{said}");
        assert!(!said.contains("does not exist") && !said.contains("empty"), "{said}");

        let said = no_clone(&rooted(&scratch.0, RootFrom::Env, &scratch.0), "o/x");
        assert!(said.contains("GITHOOT_CLONE_ROOT"), "{said}");
    }

    /// What is wrong with the folders before any pull request arrives: what Install and the page say,
    /// so a wrong root is found while you are looking at it, not hours later in a log.
    #[test]
    fn setup_problems_catch_a_wrong_root_before_any_pull_request_does() {
        let scratch = Scratch::new("problems");
        let gone = scratch.0.join("nope");
        let problems = setup_problems(&rooted(&gone, RootFrom::Default, &scratch.0));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].text.contains("does not exist") && problems[0].text.contains("empty"), "{problems:?}");
        assert_eq!(problems[0].key, "cloneRoot", "it names the box to fix, so the page can mark it");

        // There, but nothing in it is a clone: almost certainly the wrong folder.
        let problems = setup_problems(&rooted(&scratch.0, RootFrom::Setting, &scratch.0));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].text.contains("no git clone") && problems[0].key == "cloneRoot", "{problems:?}");

        std::fs::create_dir_all(scratch.0.join("thing").join(".git")).unwrap();
        assert_eq!(setup_problems(&rooted(&scratch.0, RootFrom::Setting, &scratch.0)), []);

        // A worktree root that does not exist yet is fine, git makes it. A file in its place is not.
        let file = scratch.0.join("a-file");
        std::fs::write(&file, "").unwrap();
        let problems = setup_problems(&rooted(&scratch.0, RootFrom::Setting, &file));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].text.contains("Worktrees go in") && problems[0].text.contains("not a folder"), "{problems:?}");
        assert_eq!(problems[0].key, "worktreeRoot");
        assert!(setup_problems(&rooted(&scratch.0, RootFrom::Setting, &scratch.0.join("later"))).is_empty());
    }



    /// The bug that cost the whole Windows detour, now unable to come back: Herdr answers in the
    /// platform's own path shape, and a mismatch reads as "nobody home", which starts a second
    /// agent on every comment instead of nudging the one already there.
    #[test]
    fn an_agent_is_found_whatever_shape_herdr_names_its_worktree() {
        let agents = vec![(r"D:\worktrees\pr-7-x".to_string(), "pr-7-x".to_string())];
        assert_eq!(agent_in(&agents, Path::new(r"D:\worktrees\pr-7-x")), Some("pr-7-x".to_string()));
        assert_eq!(agent_in(&agents, Path::new("D:/worktrees/pr-7-x/")), Some("pr-7-x".to_string()));
        assert_eq!(agent_in(&agents, Path::new(r"d:\WORKTREES\PR-7-X")), Some("pr-7-x".to_string()));
        assert_eq!(agent_in(&agents, Path::new(r"D:\worktrees\pr-8-y")), None, "a different worktree");
    }

    /// An agent with no name was not started by this dispatcher. A session you opened yourself in
    /// your own clone is not a claim on a dispatcher worktree, and counting it would wedge the bar.
    #[test]
    fn an_unnamed_agent_is_not_a_claim() {
        let agents = vec![(r"D:\projects\x".to_string(), String::new())];
        assert_eq!(agent_in(&agents, Path::new(r"D:\projects\x")), None);
    }

    /// An entry with no repo or number cannot be dispatched, and that must be a `None` here rather
    /// than a panic later: GitHub answers with partial data for a node the token cannot fully see.
    #[test]
    fn an_entry_without_a_repo_or_number_is_not_a_target() {
        let mut entry = PrEntry::stub("https://github.com/o/r/pull/1");
        assert_eq!(Target::from_entry(&entry), None, "a stub has neither");

        entry.repo = Some("o/r".to_string());
        assert_eq!(Target::from_entry(&entry), None, "still no number");

        entry.number = Some(1);
        let target = Target::from_entry(&entry).expect("now it is dispatchable");
        assert_eq!(target.repo, "o/r");
        assert_eq!(target.number, 1);
        assert_eq!(target.title, "", "a missing title is empty, never a panic");
    }
}
