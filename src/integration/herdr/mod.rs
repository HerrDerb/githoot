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
//!     diff and the comments itself, with `gh`, under your credential.
//!   * The prompts are files you own, and clearing one restores the shipped default.
//!   * Each version of a pull request is looked at exactly once, recorded whatever the outcome.
//!   * The agent works on a branch of its own, `githoot/<slug>`, never the pull request's.
//!
//! What GitHoot knows it no longer re-fetches: the bars come from the integration runner, the same
//! judgement the icon and the pages use, with muted pull requests already taken out. Its files live
//! in `~/.githoot/integrations/herdr/`: one state file per bar, the prompts, and two small caches.

pub mod prompts;

use super::{Batch, Context, Info, Integration, Kind, Said, Setting};
use crate::page::esc;
use crate::portal::types::PrEntry;
use crate::portal::PortalKind;
use crate::state::PrAxis;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a tick decided about one pull request. Separated from doing it so the rules can be tested
/// without a Herdr, a GitHub or a clone, which is most of what there is to get wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// This exact version was looked at already. The common case, and silent.
    AlreadySeen,
    /// First sight of a pull request that predates comment tracking: record, do not act.
    Baseline,
    /// `updated_at` moved, but not because anyone said anything. A push, CI, a label.
    NoNewComment,
    /// An agent is in the worktree and the pull request has moved since it last looked.
    Nudge(String),
    /// An agent is in the worktree and this is the first time we have seen the pull request.
    AlreadyThere,
    /// Nobody is home.
    Start,
}

/// What this axis recorded about one pull request last time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Handled {
    /// The `updated_at` it was last looked at.
    pub updated: String,
    /// The newest comment by anyone but you at that point. Empty for state written before comment
    /// tracking existed, which is why `Baseline` is a separate answer from `NoNewComment`.
    pub comment: String,
}

/// The whole rule, in one place, with no I/O in it.
///
/// `latest_comment` is the newest comment from anyone but you, as ISO-8601. Ordinal comparison is
/// correct and intended: these are UTC timestamps, where byte order is time order.
///
/// The ordering matters and is not arbitrary. "Looked at already" comes first because it is almost
/// every call and must cost nothing. The comment check comes before the agent check because a push
/// of your own fixes must not wake an agent to say "no changes" and burn tokens, whether or not
/// anyone is sitting in the worktree.
pub fn decide(prev: Option<&Handled>, updated: &str, latest_comment: &str, live_agent: Option<&str>) -> Action {
    if let Some(seen) = prev {
        if seen.updated == updated {
            return Action::AlreadySeen;
        }
        if seen.comment.is_empty() {
            return Action::Baseline;
        }
        if latest_comment <= seen.comment.as_str() {
            return Action::NoNewComment;
        }
    }
    match (live_agent, prev) {
        (Some(agent), Some(_)) => Action::Nudge(agent.to_string()),
        (Some(_), None) => Action::AlreadyThere,
        (None, _) => Action::Start,
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
pub const REQUIRED: [&str; 3] = ["herdr", "gh", "git"];

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
            let comment = parts.next().unwrap_or("").to_string();
            Some((key.to_string(), Handled { updated, comment }))
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
        .map(|(key, h)| format!("{key}\t{}\t{}\n", h.updated, h.comment))
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
        })
    }
}

// ── Asking GitHub what changed ───────────────────────────────────────────────

/// Your own login, cached on disk. Used only to tell your comments from everyone else's.
pub fn viewer(dir: &Path) -> Option<String> {
    let path = dir.join("viewer");
    if let Ok(cached) = std::fs::read_to_string(&path) {
        let cached = cached.trim().to_string();
        if !cached.is_empty() {
            return Some(cached);
        }
    }
    let login = output("gh", &["api", "user", "-q", ".login"]).ok()?;
    if login.is_empty() {
        return None;
    }
    let _ = std::fs::create_dir_all(path.parent().expect("integration dir"));
    let _ = std::fs::write(&path, &login);
    Some(login)
}

/// One query for the newest thing anyone but you said on a pull request.
///
/// `None` means gh could not answer, which must never read as "nothing new": that would start an
/// agent on a push. A pull request nobody has commented on answers with the epoch instead.
///
/// Reviews count only when they say something: a body, or a changes-requested verdict. An approval
/// with no text is a state change the bars already express and not a thing to read.
pub fn latest_foreign_comment(repo: &str, number: u64, me: &str) -> Option<String> {
    const QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){\
comments(last:100){nodes{createdAt author{login}}}\
reviews(last:100){nodes{submittedAt state body author{login}}}\
reviewThreads(last:100){nodes{comments(last:100){nodes{createdAt author{login}}}}}}}}";

    let (owner, name) = repo.split_once('/')?;
    let raw = output(
        "gh",
        &[
            "api",
            "graphql",
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("name={name}"),
            "-F",
            &format!("number={number}"),
            "-f",
            &format!("query={QUERY}"),
        ],
    )
    .ok()?;

    // Deliberately not a JSON dependency: every timestamp of interest is preceded by its own key,
    // and the only question asked of the answer is "which is newest, excluding mine". A structured
    // parse would buy nothing here and would pull a crate in for one query.
    Some(newest_foreign(&raw, me))
}

/// Picks the newest `createdAt`/`submittedAt` whose nearest following `login` is not `me`.
///
/// GitHub answers with the author inline after each timestamp, so a forward scan pairs them without
/// a parser. Anything unpaired is skipped rather than guessed at.
fn newest_foreign(json: &str, me: &str) -> String {
    let mut newest = "1970-01-01T00:00:00Z".to_string();
    let mine = format!("\"login\":\"{me}\"");
    for (at, key) in json.match_indices("\"createdAt\":\"").chain(json.match_indices("\"submittedAt\":\"")) {
        let rest = &json[at + key.len()..];
        let Some(end) = rest.find('"') else { continue };
        let stamp = &rest[..end];
        // The author of this node is the next login mentioned after it.
        let after = &rest[end..];
        let Some(login_at) = after.find("\"login\":\"") else { continue };
        if after[login_at..].starts_with(&mine) {
            continue;
        }
        if stamp > newest.as_str() {
            newest = stamp.to_string();
        }
    }
    newest
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
pub fn render_prompt(template: &str, t: &Target, branch: &str) -> String {
    template
        .replace("{url}", &t.url)
        .replace("{repo}", &t.repo)
        .replace("{number}", &t.number.to_string())
        .replace("{branch}", branch)
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

/// The head branch, from `gh`, cached on disk. GitHoot does not carry it and asking every tick
/// would burn rate limit for a value that does not change.
fn head_branch(dir: &Path, t: &Target) -> Result<String, String> {
    let name: String = format!("{}#{}", t.repo, t.number)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let path = dir.join("branches").join(name);
    if let Ok(cached) = std::fs::read_to_string(&path) {
        let cached = cached.trim().to_string();
        if !cached.is_empty() {
            return Ok(cached);
        }
    }
    let branch = output(
        "gh",
        &["pr", "view", &t.number.to_string(), "--repo", &t.repo, "--json", "headRefName", "-q", ".headRefName"],
    )?;
    if branch.is_empty() {
        return Err("gh gave no head branch".to_string());
    }
    let _ = std::fs::create_dir_all(path.parent().expect("branches dir"));
    let _ = std::fs::write(&path, &branch);
    Ok(branch)
}

/// Everything between "this pull request needs an agent" and "an agent is reading it".
///
/// Returns the line to log either way. Nothing here is retried inside a pass: by the rule at the
/// top of the module the version is recorded as looked at whatever happened, and the retry that
/// can actually go differently is the one that comes when the pull request changes.
fn start(dir: &Path, t: &Target, slug: &str, settings: &Settings, template: &str) -> Result<String, Trouble> {
    let clone = settings.clone_of(&t.repo);
    if !clone.join(".git").exists() {
        // Not recorded, so that correcting `integration.herdr.cloneRoot` takes effect on the next
        // pass rather than waiting for somebody to comment on the pull request again.
        return Err(Trouble::Setup(format!("no clone at {}", clone.display())));
    }
    let branch = head_branch(dir, t).map_err(Trouble::Passing)?;

    // The branch name is chosen by whoever opened the pull request and reaches git as an argument,
    // so it must not be able to look like an option. `check-ref-format` rejects anything git would,
    // a leading dash included.
    let clone_str = clone.to_string_lossy().to_string();
    if output("git", &["check-ref-format", "--branch", &branch]).is_err() {
        return Err(Trouble::Passing(format!("refusing branch name {branch:?}")));
    }

    // Full refspec, so the branch can never be read as anything but a ref. The leading `+` lets the
    // remote-tracking ref follow a force-push, which is the normal state of a pull request branch
    // after a rebase and would otherwise be refused as non-fast-forward.
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    let worktree = settings.worktree(slug);
    let worktree_str = worktree.to_string_lossy().to_string();
    let own = format!("githoot/{slug}");

    if settings.dry_run {
        return Ok(format!(
            "would fetch {branch} into {clone_str}, cut {own} from origin/{branch}, \
             open a worktree at {worktree_str} and start a {} agent",
            settings.agent_kind
        ));
    }

    output("git", &["-C", &clone_str, "fetch", "--quiet", "origin", &refspec])
        .map_err(|e| Trouble::Passing(format!("could not fetch {branch}: {e}")))?;

    // A worktree already on disk means its agent has gone. Reuse it if Herdr still knows it;
    // recycle it only if nothing would be lost. A worktree holding work is yours to look at.
    let mut workspace = String::new();
    if worktree.exists() {
        if let Ok(opened) = output("herdr", &["worktree", "open", "--path", &worktree_str, "--no-focus"]) {
            workspace = json_string(&opened, "workspace_id").unwrap_or_default();
        }
        if workspace.is_empty() {
            let dirty = output("git", &["-C", &worktree_str, "status", "--porcelain"]).unwrap_or_default();
            let ahead = output("git", &["-C", &worktree_str, "rev-list", "--count", &format!("origin/{branch}..HEAD")])
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
                "--base", &format!("origin/{branch}"),
                "--label", slug,
                "--no-focus",
                "--trust-repository",
            ],
        )
        .map_err(|e| Trouble::Passing(format!("could not create the worktree: {e}")))?;
        workspace = json_string(&created, "workspace_id")
            .ok_or_else(|| Trouble::Passing("no workspace came back".to_string()))?;
    }

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
    output("herdr", &["agent", "prompt", slug, &render_prompt(template, t, &branch)])
        .map_err(|e| Trouble::Passing(format!("agent started but the prompt did not arrive: {e}")))?;

    Ok(format!("started {slug} in {pane}, on branch {own}"))
}

/// One pass over one bar. `targets` has already had the muted ones removed by the caller, because
/// what counts as muted is `mute`'s business and not this module's.
///
/// The state is written once at the end, through a temporary file, and a dry run writes nothing at
/// all so that a hand run can never change what the real one would do next.
pub fn tick(axis: PrAxis, targets: &[Target], settings: &Settings, dir: &Path, templates: &Templates) -> Report {
    let mut report = Report::default();
    let Some(me) = viewer(dir) else {
        report.problems.push("gh could not say who you are - is it signed in?".to_string());
        return report;
    };
    let agents = live_agents();
    let previous = read_state(dir, axis);
    let mut next = std::collections::BTreeMap::new();

    for t in targets {
        report.looked_at += 1;
        let prev = previous.get(&t.key);

        if prev.is_some_and(|p| p.updated == t.updated) {
            next.insert(t.key.clone(), prev.expect("just checked").clone());
            // Cheap in a real pass: no `gh` call and nothing said. A dry run says it anyway,
            // because this is the commonest reason nothing happens and the silence is the
            // confusing part when you have pressed a button to find out.
            if settings.dry_run {
                report.lines.push(format!("{}#{}: unchanged since the last pass", t.repo, t.number));
            }
            continue;
        }

        let Some(latest) = latest_foreign_comment(&t.repo, t.number, &me) else {
            // Recorded as looked at, with the comment time we had, so this retries when the pull
            // request next changes rather than every pass.
            next.insert(
                t.key.clone(),
                Handled { updated: t.updated.clone(), comment: prev.map(|p| p.comment.clone()).unwrap_or_default() },
            );
            report.problems.push(format!("{}#{}: gh could not read the comments", t.repo, t.number));
            continue;
        };
        next.insert(t.key.clone(), Handled { updated: t.updated.clone(), comment: latest.clone() });

        let slug = slug(&t.repo, t.number);
        let live = agent_in(&agents, &settings.worktree(&slug));
        match decide(prev, &t.updated, &latest, live.as_deref()) {
            // Said only in a dry run. In a real pass this is almost every pull request almost
            // always, and it is the one outcome genuinely not worth a line.
            Action::AlreadySeen | Action::NoNewComment if !settings.dry_run => {}
            Action::AlreadySeen => {
                report.lines.push(format!("{}#{}: unchanged since the last pass", t.repo, t.number))
            }
            Action::NoNewComment => report
                .lines
                .push(format!("{}#{}: changed, but nobody else has said anything since", t.repo, t.number)),
            Action::Baseline => report.lines.push(format!("{}#{}: comment baseline recorded", t.repo, t.number)),
            Action::AlreadyThere => {
                report.lines.push(format!("{}#{}: an agent is already there", t.repo, t.number))
            }
            Action::Nudge(agent) => {
                let said = if settings.dry_run {
                    Ok(format!("would nudge {agent}"))
                } else {
                    output("herdr", &["agent", "prompt", &agent, &render_prompt(&templates.update, t, "")])
                        .map(|_| format!("nudged {agent}"))
                };
                match said {
                    Ok(ok) => report.lines.push(format!("{}#{}: {ok}", t.repo, t.number)),
                    Err(e) => report.problems.push(format!("{}#{}: nudge failed: {e}", t.repo, t.number)),
                }
            }
            Action::Start => match start(dir, t, &slug, settings, &templates.bar) {
                Ok(said) => {
                    report.started += 1;
                    report.lines.push(format!("{}#{}: {said}", t.repo, t.number));
                }
                Err(Trouble::Setup(why)) => {
                    // Deliberately un-recorded: this pull request must be tried again as soon as
                    // the setting is corrected, not when GitHub next touches it.
                    next.remove(&t.key);
                    report.setup.push(format!("{}#{}: {why}", t.repo, t.number));
                }
                Err(other) => report.problems.push(format!("{}#{}: {}", t.repo, t.number, other.text())),
            },
        }
    }

    // A dry run writes nothing at all, so pressing Dry run can never change what the next real
    // pass would do. That is the whole value of the button.
    let written = if settings.dry_run { Ok(()) } else { write_state(dir, axis, &next) };
    if let Err(e) = written {
        report.problems.push(format!("could not write the dispatch state: {e}"));
    }
    report
}

/// The two prompts a pass needs: the one for this bar, and the nudge.
pub struct Templates {
    pub bar: String,
    pub update: String,
}

// ── The integration ──────────────────────────────────────────────────────────

pub struct Herdr;

static INFO: Info = Info {
    id: "herdr",
    name: "Herdr dispatcher",
    summary: "Starts a Herdr agent for each pull request that needs you, on a branch of its own.",
    // `gh` answers who you are, the head branch and the comments. Nothing here would work against
    // another forge's pull request, so none ever reaches it.
    portals: &[PortalKind::GitHub],
    settings: &[
        // One switch per bar, keyed like the bars' prompts. Approved is off by default: an approved
        // pull request is usually one you are about to merge yourself, and an agent started for it is
        // mostly tokens spent on work that is done.
        Setting {
            key: "workRequired",
            label: "Start agents for pull requests that need work from you",
            kind: Kind::Flag { default_on: true },
            help: "Start agents for the amber bar: your pull requests that need work.",
        },
        Setting {
            key: "requestedReviews",
            label: "Start agents for reviews requested of you",
            kind: Kind::Flag { default_on: true },
            help: "Start agents for the red bar: reviews requested of you.",
        },
        Setting {
            key: "approved",
            label: "Start agents for your approved pull requests",
            kind: Kind::Flag { default_on: false },
            help: "Start agents for the green bar: your approved pull requests. Off by default.",
        },
        Setting {
            key: "cloneRoot",
            label: "Clones live in",
            kind: Kind::Text { placeholder: "~/projects" },
            help: "Where your clones live, one directory per repository name: owner/thing needs <this>/thing. Empty means ~/projects.",
        },
        Setting {
            key: "worktreeRoot",
            label: "Worktrees go in",
            kind: Kind::Text { placeholder: "~/worktrees" },
            help: "Where the worktree for each pull request goes, named githoot/pr-<number>-<repo>. Empty means ~/worktrees.",
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

    /// Install, or install again after Remove, is "from now on" for every bar.
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
            // Before anything else, so a bar switched off costs no `gh` call and touches no state
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
            let templates = Templates {
                bar: prompts::text(&ctx.dir, axis.slug()),
                update: prompts::text(&ctx.dir, "update"),
            };
            let report = tick(axis, &targets, &settings, &ctx.dir, &templates);
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
// and leaves a marker saying the bar has a baseline. Switching the bar off, Install and Remove all
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

/// Every pull request in the bar as seen, as of now, with everything said on it so far as read.
///
/// The comment time recorded is the pull request's own `updated_at`, and that is not a shortcut: a
/// comment or a review moves `updated_at`, so no comment that exists yet can be newer than it. The
/// first one that is was written after the switch, which is exactly when an agent should wake. And
/// no `gh` call is made to find out, so a baseline over a full bar costs nothing.
fn baseline(targets: &[Target]) -> std::collections::BTreeMap<String, Handled> {
    targets
        .iter()
        .map(|t| (t.key.clone(), Handled { updated: t.updated.clone(), comment: t.updated.clone() }))
        .collect()
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
            return PathBuf::from(set);
        }
        std::env::var_os(env).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).unwrap_or(fallback)
    };
    Settings {
        clone_root: pick(ctx.setting("cloneRoot"), "GITHOOT_CLONE_ROOT", home.join("projects")),
        worktree_root: pick(ctx.setting("worktreeRoot"), "GITHOOT_WORKTREE_ROOT", home.join("worktrees")),
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
    format!(
        "<div class=\"card\"><p class=\"sub\">Starts a <a href=\"https://herdr.dev\">Herdr</a> agent for each pull request \
         that needs you, on a branch of its own, under your own <code>gh</code>. It needs <code>herdr</code>, <code>gh</code> \
         and <code>git</code>. This is the one thing GitHoot does that is not reading, so \
         <a href=\"https://github.com/HerrDerb/githoot/blob/main/docs/dispatcher.md\">read what it does</a> first.</p>\
         <p class=\"sub\">Right now it would look for clones in <code>{}</code> and put worktrees in <code>{}</code>.</p>{herdr}</div>\n{}",
        esc(&settings.clone_root.display().to_string()),
        esc(&settings.worktree_root.display().to_string()),
        prompts_card(token, prompts)
    )
}

/// The prompts as edit boxes, one form, one Save. Always shown, installed or not, because editing
/// what an agent will be told before switching it on is the sane order.
///
/// An emptied box is the reset: the default is written back and the shipped text returns. Said on
/// the card, because a blank box that silently keeps the old text would be worse.
fn prompts_card(token: &str, prompts: &[prompts::Prompt]) -> String {
    let mut h = format!(
        "<h2 class=\"section\">Prompts</h2>\n<div class=\"card\">\
         <form method=\"post\" action=\"/{}/integrations/herdr\"><input type=\"hidden\" name=\"action\" value=\"prompts\">\
         <p class=\"sub\">What the agent is told, per bar, plus the nudge it gets when a pull request changes under it. \
         Placeholders: <code>{{url}}</code> <code>{{repo}}</code> <code>{{number}}</code> <code>{{branch}}</code> \
         <code>{{title}}</code> <code>{{author}}</code>. The last two are written by whoever opened the pull request: \
         keep them in the labelled data block. Clear a box to go back to the shipped default.</p>",
        esc(token)
    );
    for p in prompts {
        h.push_str(&format!(
            "<label class=\"row\"><strong>{}</strong> <span class=\"sub\">{}</span></label>\
             <textarea name=\"prompt_{}\" rows=\"14\" spellcheck=\"false\">{}</textarea>",
            esc(p.name),
            if p.is_default { "shipped default" } else { "yours" },
            esc(p.name),
            esc(&p.text),
        ));
    }
    h.push_str("<button class=\"small\" type=\"submit\">Save prompts</button></form></div>\n");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            clone_root: PathBuf::from("/d/projects"),
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

    /// Herdr is the tool a user is least likely to have, so its absence comes with the way to fix it.
    #[test]
    fn a_missing_herdr_links_to_its_install_guide() {
        assert!(page_body("tok", &settings(), &["herdr"], &[]).contains(r#"href="https://herdr.dev/docs/install/""#));
        assert!(!page_body("tok", &settings(), &["gh"], &[]).contains("herdr.dev/docs/install"));
    }

    /// Only its own action. Install, remove, dry run and settings are the generic page's.
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

    /// A bar switched off costs nothing: no `gh`, no state. A dry run says so, because a bar that is
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

    /// The baseline counts any comment up to now as read. A pull request's `updated_at` moves with
    /// every comment, so no comment that exists yet can be newer than it, and the first one that is
    /// was written after the switch.
    #[test]
    fn a_baseline_takes_everything_said_so_far_as_read() {
        let t = Target::from_entry(&pr(1, "2026-09-25T10:00:00Z")).unwrap();
        let seen = baseline(std::slice::from_ref(&t));
        let h = &seen[&t.key];
        assert_eq!(h.updated, "2026-09-25T10:00:00Z");
        assert_eq!(decide(Some(h), "2026-09-25T10:00:00Z", "2026-09-25T09:00:00Z", None), Action::AlreadySeen);
        assert_eq!(decide(Some(h), "2026-09-25T12:00:00Z", "2026-09-25T09:00:00Z", None), Action::NoNewComment, "a push");
        assert_eq!(decide(Some(h), "2026-09-25T12:00:00Z", "2026-09-25T11:00:00Z", None), Action::Start, "a new comment");
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
    fn install_and_remove_both_mean_a_fresh_baseline_next_time() {
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

    fn seen(updated: &str, comment: &str) -> Handled {
        Handled { updated: updated.to_string(), comment: comment.to_string() }
    }

    /// The common case, and the one that must cost nothing: the bar is full of pull requests that
    /// have not moved, every poll, forever.
    #[test]
    fn an_unchanged_pull_request_is_not_looked_at_again() {
        let prev = seen("2026-09-24T10:00:00Z", "2026-09-24T09:00:00Z");
        assert_eq!(decide(Some(&prev), "2026-09-24T10:00:00Z", "2026-09-24T09:00:00Z", None), Action::AlreadySeen);
    }

    /// The rule that keeps a dispatcher affordable. `updated_at` moves on a push, a label, a CI
    /// run and a title edit. Waking an agent for those means it reads the diff, finds nothing
    /// anyone asked about, and bills you for saying so.
    #[test]
    fn a_push_with_no_new_comment_wakes_nobody() {
        let prev = seen("2026-09-24T10:00:00Z", "2026-09-24T09:00:00Z");
        // Moved, but the newest foreign comment is the one we already knew about.
        assert_eq!(decide(Some(&prev), "2026-09-24T11:00:00Z", "2026-09-24T09:00:00Z", None), Action::NoNewComment);
        // Even with an agent sitting right there, which is the case that would otherwise nudge.
        assert_eq!(
            decide(Some(&prev), "2026-09-24T11:00:00Z", "2026-09-24T09:00:00Z", Some("pr-7-x")),
            Action::NoNewComment
        );
    }

    /// A genuinely new comment is the one thing worth the tokens.
    #[test]
    fn a_new_comment_starts_or_nudges() {
        let prev = seen("2026-09-24T10:00:00Z", "2026-09-24T09:00:00Z");
        assert_eq!(decide(Some(&prev), "2026-09-24T11:00:00Z", "2026-09-24T10:30:00Z", None), Action::Start);
        assert_eq!(
            decide(Some(&prev), "2026-09-24T11:00:00Z", "2026-09-24T10:30:00Z", Some("pr-7-x")),
            Action::Nudge("pr-7-x".to_string())
        );
    }

    /// State written before comment tracking has no comment time. Guessing one would either wake
    /// every agent at once or silence them all, so the first sight records and does nothing.
    #[test]
    fn state_without_a_comment_time_records_a_baseline_first() {
        let prev = seen("2026-09-24T10:00:00Z", "");
        assert_eq!(decide(Some(&prev), "2026-09-24T11:00:00Z", "2026-09-24T10:30:00Z", None), Action::Baseline);
    }

    /// A pull request nobody has seen: start, unless somebody is already in that worktree, in
    /// which case say so once and leave them alone rather than starting a second agent.
    #[test]
    fn an_unseen_pull_request_starts_unless_the_worktree_is_taken() {
        assert_eq!(decide(None, "2026-09-24T11:00:00Z", "2026-09-24T10:00:00Z", None), Action::Start);
        assert_eq!(decide(None, "2026-09-24T11:00:00Z", "2026-09-24T10:00:00Z", Some("pr-7-x")), Action::AlreadyThere);
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
        let settings = Settings {
            clone_root: PathBuf::from("/d/projects"),
            worktree_root: PathBuf::from("/d/worktrees"),
            agent_kind: "claude".to_string(),
            dry_run: true,
        };
        assert_eq!(settings.clone_of("QUMEA/care-backend"), PathBuf::from("/d/projects/care-backend"));
        assert_eq!(settings.worktree("pr-1-x"), PathBuf::from("/d/worktrees/pr-1-x"));
    }

    /// The comment scan must ignore your own and pick the newest of the rest. Fed real answer
    /// shapes rather than a tidy fixture, because the thing being relied on is that GitHub puts
    /// the author inline right after each timestamp.
    #[test]
    fn the_newest_comment_from_anyone_but_you_wins() {
        let json = r#"{"data":{"repository":{"pullRequest":{
          "comments":{"nodes":[
            {"createdAt":"2026-09-24T09:00:00Z","author":{"login":"herrderb"}},
            {"createdAt":"2026-09-24T12:00:00Z","author":{"login":"someone"}}]},
          "reviews":{"nodes":[
            {"submittedAt":"2026-09-24T10:00:00Z","state":"CHANGES_REQUESTED","body":"","author":{"login":"other"}}]},
          "reviewThreads":{"nodes":[]}}}}}"#;
        assert_eq!(newest_foreign(json, "herrderb"), "2026-09-24T12:00:00Z");
        // Your own later comment must not count, or every push of yours wakes an agent.
        assert_eq!(newest_foreign(json, "someone"), "2026-09-24T10:00:00Z");
    }

    /// A pull request nobody has said anything on answers with the epoch, which compares older
    /// than any recorded time and so decides "nothing new" rather than "start".
    #[test]
    fn a_silent_pull_request_reads_as_the_epoch() {
        let empty = r#"{"data":{"repository":{"pullRequest":{"comments":{"nodes":[]},"reviews":{"nodes":[]},"reviewThreads":{"nodes":[]}}}}}"#;
        assert_eq!(newest_foreign(empty, "me"), "1970-01-01T00:00:00Z");
        let prev = seen("old", "1970-01-01T00:00:00Z");
        assert_eq!(decide(Some(&prev), "new", &newest_foreign(empty, "me"), None), Action::NoNewComment);
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
