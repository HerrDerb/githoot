//! Starts a Herdr agent for each pull request that needs one, from inside GitHoot.
//!
//! **This is the one thing GitHoot does that is not reading.** Everywhere else it polls GitHub,
//! draws an icon and opens a page; here it creates branches and worktrees and starts agents. That
//! boundary used to be a process boundary: a shipped script, installed by a button, kept alive by
//! systemd or Task Scheduler. It is now a setting, `dispatcher`, off by default and off in every
//! `config.txt` that does not say otherwise.
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
//! What GitHoot knows it no longer re-fetches: the bars come from `scheduler::pr_snapshot`, the
//! same judgement the icon and the pages use. The local API is no longer part of this path at all.

use crate::portal::types::PrEntry;
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
    /// Prints what it would do and touches nothing, including the state file. The default for a
    /// hand run (`--dispatch`), and never the default for the thread.
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

fn state_path(home: &Path, axis: PrAxis) -> PathBuf {
    home.join("dispatch").join(format!("{}.txt", axis.slug()))
}

pub fn read_state(home: &Path, axis: PrAxis) -> std::collections::BTreeMap<String, Handled> {
    std::fs::read_to_string(state_path(home, axis))
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
    home: &Path,
    axis: PrAxis,
    state: &std::collections::BTreeMap<String, Handled>,
) -> std::io::Result<()> {
    let path = state_path(home, axis);
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
pub fn viewer(home: &Path) -> Option<String> {
    let path = home.join("dispatch").join("viewer");
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
    let _ = std::fs::create_dir_all(path.parent().expect("dispatch dir"));
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
fn head_branch(home: &Path, t: &Target) -> Result<String, String> {
    let name: String = format!("{}#{}", t.repo, t.number)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let path = home.join("dispatch").join("branches").join(name);
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
fn start(home: &Path, t: &Target, slug: &str, settings: &Settings, template: &str) -> Result<String, Trouble> {
    let clone = settings.clone_of(&t.repo);
    if !clone.join(".git").exists() {
        // Not recorded, so that correcting `dispatcherCloneRoot` takes effect on the next
        // pass rather than waiting for somebody to comment on the pull request again.
        return Err(Trouble::Setup(format!("no clone at {}", clone.display())));
    }
    let branch = head_branch(home, t).map_err(Trouble::Passing)?;

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
pub fn tick(axis: PrAxis, targets: &[Target], settings: &Settings, home: &Path, templates: &Templates) -> Report {
    let mut report = Report::default();
    let Some(me) = viewer(home) else {
        report.problems.push("gh could not say who you are - is it signed in?".to_string());
        return report;
    };
    let agents = live_agents();
    let previous = read_state(home, axis);
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
            Action::Start => match start(home, t, &slug, settings, &templates.bar) {
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
    let written = if settings.dry_run { Ok(()) } else { write_state(home, axis, &next) };
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

// ── When a pass happens ──────────────────────────────────────────────────────

/// How often a pass runs. GitHoot polls GitHub at most once a minute, so the snapshot this reads
/// cannot change faster than that; anything quicker would be work to discover nothing.
const EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// The last dry run the settings page asked for, so the answer survives the redirect back.
static LAST_DRY_RUN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn last_dry_run() -> Vec<String> {
    LAST_DRY_RUN.lock().map(|l| l.clone()).unwrap_or_default()
}

/// Reads the settings a pass needs out of `config.txt`, with the roots it does not yet expose as
/// settings taken from the environment so they can be moved without a release.
/// `config.txt` first, then the environment, then a default under your home directory.
///
/// The environment comes second rather than first because it is the exception: it is there for a
/// one-off `GITHOOT_CLONE_ROOT=... githoot` while trying something, and a value somebody wrote in
/// `config.txt` should not be quietly overridden by a stale variable in a shell profile.
fn settings_now(app_asset_path: &Path, dry_run: bool) -> Settings {
    let cfg = crate::config::Config::load(app_asset_path).0;
    let home = dirs::home_dir().unwrap_or_default();
    let pick = |set: &str, env: &str, fallback: PathBuf| {
        if !set.trim().is_empty() {
            return PathBuf::from(set.trim());
        }
        std::env::var_os(env).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).unwrap_or(fallback)
    };
    Settings {
        clone_root: pick(&cfg.clone_root, "GITHOOT_CLONE_ROOT", home.join("projects")),
        worktree_root: pick(&cfg.worktree_root, "GITHOOT_WORKTREE_ROOT", home.join("worktrees")),
        agent_kind: std::env::var("GITHOOT_AGENT_KIND").unwrap_or_else(|_| "claude".to_string()),
        dry_run,
    }
}

/// The roots a pass would use right now, for the settings page to show. Diagnosing "it found no
/// clones" from a card that does not say where it looked is guesswork.
pub fn roots(app_asset_path: &Path) -> (String, String) {
    let s = settings_now(app_asset_path, true);
    (s.clone_root.display().to_string(), s.worktree_root.display().to_string())
}

/// One pass over every bar GitHoot is watching, using the lists it already has.
///
/// **The honesty gate.** A bar whose list is `None` is one GitHoot has no confirmed answer for,
/// not an empty one. Acting on it would mean going quiet for exactly as long as GitHub is broken
/// and looking identical to a quiet morning, so an unconfirmed bar is skipped entirely.
pub fn pass(app_asset_path: &Path, dry_run: bool) -> Said {
    let settings = settings_now(app_asset_path, dry_run);
    let now = crate::mute::unix_now();
    let mut said = Vec::new();
    let mut trouble = Vec::new();
    let mut setup = Vec::new();

    for axis in PrAxis::ALL {
        let snapshot = crate::scheduler::pr_snapshot(axis);
        let mut targets = Vec::new();
        let mut muted = 0usize;
        for (_, list) in &snapshot.groups {
            // `None` is "not known", and is the whole reason this is not `unwrap_or_default`.
            let Some(entries) = list else { continue };
            for entry in entries {
                if crate::mute::until(entry.key(), now).is_some() {
                    muted += 1;
                    continue;
                }
                if let Some(target) = Target::from_entry(entry) {
                    targets.push(target);
                }
            }
        }
        // Said in a dry run only, and worth saying: a bar whose every pull request is muted looks
        // from the outside exactly like an empty one, and "nothing needed an agent" is then true
        // but useless to somebody wondering why their pull request was ignored.
        if dry_run && muted > 0 {
            said.push(format!("[{}] {muted} muted, skipped", axis.slug()));
        }
        if targets.is_empty() {
            continue;
        }

        let home = app_asset_path;
        let templates = Templates {
            bar: crate::prompts::text(home, axis.slug()),
            update: crate::prompts::text(home, "update"),
        };
        let report = tick(axis, &targets, &settings, home, &templates);
        for line in report.lines {
            said.push(format!("[{}] {line}", axis.slug()));
        }
        for problem in report.problems {
            trouble.push(format!("[{}] {problem}", axis.slug()));
        }
        for problem in report.setup {
            setup.push(format!("[{}] {problem}", axis.slug()));
        }
        if dry_run && report.looked_at > 0 {
            said.push(format!("[{}] {} pull request(s) looked at", axis.slug(), report.looked_at));
        }
    }
    if said.is_empty() && trouble.is_empty() && setup.is_empty() {
        said.push("nothing needed an agent".to_string());
    }
    Said { said, trouble, setup }
}

/// What a pass has to say, split by how loudly and how often to say it.
#[derive(Debug, Default)]
pub struct Said {
    /// Routine. Info level.
    pub said: Vec<String>,
    /// Went wrong. Error level, every time.
    pub trouble: Vec<String>,
    /// A setting needs fixing. Error level, but only when the set changes.
    pub setup: Vec<String>,
}

/// Runs a pass without acting and remembers what it said, for the page's Dry run button.
pub fn dry_run_now(app_asset_path: &Path) {
    let out = pass(app_asset_path, true);
    // All three, in one block. Somebody pressing Dry run wants the whole answer, and splitting it
    // between a page and a log file would be the same mistake twice.
    let mut lines = out.said;
    lines.extend(out.trouble);
    lines.extend(out.setup);
    if let Ok(mut last) = LAST_DRY_RUN.lock() {
        *last = lines;
    }
}

/// Said once per change rather than once per pass, because a missing tool repeated every thirty
/// seconds would bury everything else in the log.
static LAST_COMPLAINT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn complain_once(said: String) {
    let Ok(mut last) = LAST_COMPLAINT.lock() else { return };
    if *last == said {
        return;
    }
    if !said.is_empty() {
        crate::errorln!("dispatcher: {said}");
    } else if !last.is_empty() {
        crate::infoln!("dispatcher: everything it needs is back");
    }
    *last = said;
}

/// The thread. Always started, and it reads the setting on every pass rather than at boot.
///
/// That is deliberate, and it is the difference between a switch and a note telling you to
/// restart. The thread costs one config read every thirty seconds while the dispatcher is off,
/// which is nothing, and in exchange the button on the Dispatcher tab means what a button should.
/// Nothing is created, fetched or started until a pass finds the setting on.
pub fn spawn(app_asset_path: PathBuf) {
    std::thread::Builder::new()
        .name("dispatch".to_string())
        .spawn(move || loop {
            std::thread::sleep(EVERY);
            if !crate::config::Config::load(&app_asset_path).0.dispatcher {
                continue;
            }
            let missing = missing_tools();
            complain_once(if missing.is_empty() { String::new() } else { format!("missing {}", missing.join(", ")) });
            if !missing.is_empty() {
                continue;
            }
            let out = pass(&app_asset_path, false);
            for line in out.said {
                crate::infoln!("dispatcher: {line}");
            }
            // Never silenced by the log level. A pull request that could not be dispatched has to
            // be visible at `logLevel=error`, which is the default and what most people run.
            for problem in out.trouble {
                crate::errorln!("dispatcher: {problem}");
            }
            // Said once per change. These repeat every pass by nature, and a wrong clone root
            // would otherwise write a line per pull request every thirty seconds forever.
            complain_once(out.setup.join("; "));
        })
        .map(|_| ())
        .unwrap_or_else(|e| crate::errorln!("dispatcher: could not start its thread: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;

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
