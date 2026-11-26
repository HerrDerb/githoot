//! Claude Desktop's Code tab, through the one door it documents for other programs: the link
//! `claude://code/new?folder=…&q=…` (https://support.claude.com/en/articles/14729294).
//!
//! What its docs settle, and this follows:
//!
//!   * **Git makes the worktree, as for the Herdr app.** Desktop's own Worktree switch picks its own
//!     path and branch name. A folder from the link opens as is, so the branch, the path and the
//!     starting ref stay exactly what GitHoot chose.
//!   * **Nothing runs until you send.** Desktop treats a folder from a link as untrusted and asks you
//!     to confirm it every time, and the prompt in `q` is only filled in. So this client is
//!     semi-manual: GitHoot prepares the worktree and opens the session, you confirm and send.
//!   * **The starting permission mode is read, not set.** No link parameter or command sets it. A new
//!     folder starts in `permissions.defaultMode` from managed settings or `~/.claude/settings.json`;
//!     a project's `.claude/` cannot set `auto` or bypass in Desktop at all. So those two files are
//!     read, and a launch that would start without asking is refused before the link is opened.
//!
//! What the docs leave open, and this decides:
//!
//!   * **Who is working.** Desktop has no command to list its sessions. It keeps one JSON file per Code
//!     session in its data folder, with the folder it was opened on (`originCwd`), the worktree it
//!     made there if any (`cwd`) and whether it was archived. That format is Desktop's own and not
//!     documented, which is one reason this client is Experimental, so a file it cannot read makes
//!     the claim unsure rather than empty.
//!   * **Closing.** No way to close a session from outside, so `close` keeps the trait's "close it there".

use std::path::{Path, PathBuf};

use super::{Client, Launch, NewWorktree, Run};
use crate::agents::dispatch::Trouble;

#[derive(Debug)]
pub struct ClaudeDesktop;

/// Prompt text in `q` is "truncated to roughly 14,000 characters", per the link's help article.
const PROMPT_LIMIT: usize = 14_000;

/// The starting modes that still ask before acting. An allowlist, as for Orca, so a mode Desktop adds
/// later is refused until somebody looks at it. `auto` is out for the same reason as in Orca: it
/// lets a classifier, not you, decide what runs, and this dispatcher rests on the agent asking you
/// before it pushes.
const ASKING: &[&str] = &["default", "acceptEdits", "plan"];

impl Client for ClaudeDesktop {
    fn id(&self) -> &'static str {
        "claude-desktop"
    }
    fn name(&self) -> &'static str {
        "Claude Desktop"
    }
    /// An app, not a program on `PATH`, so `installed` answers for it.
    fn tool(&self) -> &'static str {
        "Claude Desktop"
    }
    fn install_url(&self) -> &'static str {
        "https://claude.com/download"
    }
    fn note(&self) -> &'static str {
        "Each agent opens as a new Code session in Claude Desktop with its prompt filled in. Confirm the folder and press send: nothing runs until you do. Your default permission mode must ask before acting (Manual, Accept edits or Plan)."
    }
    fn installed(&self) -> Option<bool> {
        Some(!data_roots().is_empty())
    }
    fn agents(&self, _run: &mut Run) -> Result<Vec<(String, String)>, String> {
        sessions_in(&data_roots())
    }
    fn create(&self, w: &NewWorktree, run: &mut Run) -> Result<String, Trouble> {
        run("git", &["-C", w.clone, "worktree", "add", "-b", w.branch, w.path, w.base])
            .map_err(|e| Trouble::Passing(format!("could not create the worktree: {e}")))?;
        Ok(w.path.to_string())
    }
    fn start(&self, l: &Launch, run: &mut Run) -> Result<(), Trouble> {
        let files = settings_files();
        let read: Vec<(&str, Option<String>)> = files.iter().map(|(shown, path)| (shown.as_str(), std::fs::read_to_string(path).ok())).collect();
        start_with(l, run, &read)
    }
}

/// The link that opens a new Code session on `folder` with `prompt` filled in.
fn link(folder: &str, prompt: &str) -> String {
    format!("claude://code/new?folder={}&q={}", encode(folder), encode(prompt))
}

/// Percent-encodes everything but the characters a URL never reads as syntax, byte by byte of UTF-8.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The program that hands a link to whatever claims its scheme, with its leading arguments.
/// `rundll32` on Windows rather than `cmd /c start`, which would read every `&` in the link as a
/// second command, and rather than `explorer`, which exits 1 even when it opened the link.
fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(windows) {
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else if cfg!(target_os = "macos") {
        ("open", &[])
    } else {
        ("xdg-open", &[])
    }
}

/// The settings that decide how a new folder starts, strongest first, as `(how the log names it, path)`.
fn settings_files() -> Vec<(String, PathBuf)> {
    let managed = if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode\managed-settings.json")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    };
    let mut files = vec![(managed.display().to_string(), managed)];
    if let Some(home) = dirs::home_dir() {
        let mine = home.join(".claude").join("settings.json");
        files.push((mine.display().to_string(), mine));
    }
    files
}

/// The first `permissions.defaultMode` among `settings`, strongest first, as `(mode, where)`. A file
/// that is missing, not JSON or without the key leaves it to the next.
fn starting_mode(settings: &[(&str, Option<String>)]) -> Option<(String, String)> {
    settings.iter().find_map(|(shown, text)| {
        let value: serde_json::Value = serde_json::from_str(text.as_deref()?).ok()?;
        let mode = value.get("permissions")?.get("defaultMode")?.as_str()?;
        Some((mode.to_string(), shown.to_string()))
    })
}

/// `start`, with the settings handed in, so a test does not read this machine's.
fn start_with(l: &Launch, run: &mut Run, settings: &[(&str, Option<String>)]) -> Result<(), Trouble> {
    // Before anything is opened. No mode set anywhere is Desktop's own default, Manual.
    if let Some((mode, file)) = starting_mode(settings)
        && !ASKING.contains(&mode.as_str())
    {
        return Err(Trouble::Setup(format!(
            "Claude Desktop would start the session in {mode}, set by permissions.defaultMode in {file}, which lets it act \
             without asking you, so it was not opened. Set it to default, acceptEdits or plan"
        )));
    }
    let chars = l.prompt.chars().count();
    if chars > PROMPT_LIMIT {
        return Err(Trouble::Passing(format!(
            "its prompt is {chars} characters and Claude Desktop cuts a prompt in a link at about {PROMPT_LIMIT}, so it was not opened. \
             Shorten the prompt, or open {} in Claude Desktop and paste it",
            l.worktree
        )));
    }
    let url = link(l.worktree, l.prompt);
    let (exe, lead) = opener();
    let args: Vec<&str> = lead.iter().copied().chain([url.as_str()]).collect();
    run(exe, &args).map(|_| ()).map_err(|e| {
        Trouble::Passing(format!(
            "Claude Desktop would not open: {e}. Open {} in Claude Desktop yourself and paste the prompt",
            l.worktree
        ))
    })
}

/// The folders Claude Desktop keeps its data in, those that exist. `%APPDATA%\Claude`,
/// `~/Library/Application Support/Claude` or `~/.config/Claude`, and on Windows the Store build's
/// `%LOCALAPPDATA%\Claude-Data\claude_<publisher>`. Empty in tests, so no test reads this machine's.
fn data_roots() -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    let mut roots: Vec<PathBuf> = dirs::config_dir().map(|d| d.join("Claude")).into_iter().collect();
    if cfg!(windows)
        && let Some(store) = dirs::data_local_dir().map(|d| d.join("Claude-Data"))
        && let Ok(entries) = std::fs::read_dir(store)
    {
        roots.extend(entries.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("claude_"))));
    }
    roots.into_iter().filter(|r| r.join("claude-code-sessions").is_dir()).collect()
}

/// Every Code session under `roots` that is not archived, as `(folder, title or id)`, once for the
/// folder it was opened on and once more for the worktree Desktop made there, if it made one.
fn sessions_in(roots: &[PathBuf]) -> Result<Vec<(String, String)>, String> {
    let mut found = Vec::new();
    for root in roots {
        let mut files = Vec::new();
        session_files(&root.join("claude-code-sessions"), 3, &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("could not read {}: {e}", file.display()))?;
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("could not read {}: {e}", file.display()))?;
            if value.get("isArchived").and_then(serde_json::Value::as_bool) == Some(true) {
                continue;
            }
            let field = |key: &str| value.get(key).and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            let Some(name) = field("title").or_else(|| field("sessionId")) else { continue };
            let mut folders: Vec<String> = [field("originCwd"), field("cwd")].into_iter().flatten().collect();
            folders.dedup();
            found.extend(folders.into_iter().map(|cwd| (cwd, name.clone())));
        }
    }
    Ok(found)
}

/// The `local_*.json` files under `dir`, at most `depth` folders down.
fn session_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            if depth > 0 {
                session_files(&path, depth - 1, out);
            }
        } else if path.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n.starts_with("local_") && n.ends_with(".json")
        }) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::recorder;
    use super::*;

    const OPENER: &str = if cfg!(windows) {
        "rundll32 url.dll,FileProtocolHandler"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    fn launch(prompt: &str) -> Launch<'_> {
        Launch { slug: "pr-1-x", kind: "claude", prompt, worktree: "/w/pr-1-x", place: "/w/pr-1-x" }
    }

    /// A temporary data folder laid out as Claude Desktop keeps its Code sessions:
    /// `claude-code-sessions/<account>/<org>/local_<id>.json`.
    fn data_root(name: &str, sessions: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("githoot-desktop-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("claude-code-sessions").join("acct").join("org");
        std::fs::create_dir_all(&dir).unwrap();
        for (file, body) in sessions {
            std::fs::write(dir.join(file), body).unwrap();
        }
        std::fs::write(dir.join("scheduled-tasks.json"), "[]").unwrap();
        root
    }

    /// Plain git, as for the Herdr app: the branch, path and starting ref stay exactly what GitHoot chose.
    #[test]
    fn its_worktree_is_cut_with_plain_git() {
        let (mut run, ran) = recorder(|_| Ok(String::new()));
        let w = NewWorktree { clone: "/p/x", path: "/w/pr-1-x", branch: "githoot/pr-1-x", base: "refs/githoot-pr/pr-1-x", slug: "pr-1-x" };
        assert_eq!(ClaudeDesktop.create(&w, &mut run), Ok("/w/pr-1-x".to_string()));
        assert_eq!(ran.borrow().as_slice(), ["git -C /p/x worktree add -b githoot/pr-1-x /w/pr-1-x refs/githoot-pr/pr-1-x"]);
    }

    /// Every value URL-encoded: spaces, line breaks, `&`, `#`, `%` and non-ASCII, so the prompt cannot
    /// end the parameter early or start another one.
    #[test]
    fn the_link_carries_the_folder_and_the_prompt_encoded() {
        assert_eq!(
            link(r"C:\w\pr-1 x", "Review #7 & fix 100%\nnext: ü"),
            "claude://code/new?folder=C%3A%5Cw%5Cpr-1%20x&q=Review%20%237%20%26%20fix%20100%25%0Anext%3A%20%C3%BC"
        );
        assert_eq!(link("/w/a-b_c.d~e", ""), "claude://code/new?folder=%2Fw%2Fa-b_c.d~e&q=");
    }

    /// Managed settings win over your own; a project's `.claude/` cannot set `auto` or bypass in
    /// Claude Desktop at all, so only these two decide how a new folder starts.
    #[test]
    fn the_starting_mode_is_managed_first_then_yours() {
        let mine = Some(r#"{"permissions":{"defaultMode":"auto"}}"#.to_string());
        let managed = Some(r#"{"permissions":{"defaultMode":"default"}}"#.to_string());
        assert_eq!(starting_mode(&[("managed", managed), ("mine", mine.clone())]), Some(("default".into(), "managed".into())));
        assert_eq!(starting_mode(&[("managed", None), ("mine", mine)]), Some(("auto".into(), "mine".into())));
        assert_eq!(starting_mode(&[("managed", Some("{}".into())), ("mine", Some("not json".into()))]), None);
    }

    /// Asking: the link is opened with the worktree and the prompt, and nothing else is run.
    #[test]
    fn an_asking_setup_opens_the_link() {
        for set in [None, Some(r#"{"permissions":{"defaultMode":"acceptEdits"}}"#.to_string()), Some(r#"{"permissions":{"defaultMode":"plan"}}"#.to_string())] {
            let (mut run, ran) = recorder(|_| Ok(String::new()));
            assert_eq!(start_with(&launch("Review it"), &mut run, &[("mine", set.clone())]), Ok(()), "{set:?}");
            assert_eq!(ran.borrow().as_slice(), [format!("{OPENER} claude://code/new?folder=%2Fw%2Fpr-1-x&q=Review%20it")]);
        }
    }

    /// A session that would start without asking is never opened: the pull request is not handed over,
    /// and the message says which file sets it and what to set it to.
    #[test]
    fn a_setup_that_skips_asking_opens_nothing() {
        for mode in ["auto", "bypassPermissions", "dontAsk", "somethingNew"] {
            let set = Some(format!(r#"{{"permissions":{{"defaultMode":"{mode}"}}}}"#));
            let (mut run, ran) = recorder(|_| Ok(String::new()));
            let Err(Trouble::Setup(said)) = start_with(&launch("Review it"), &mut run, &[("/home/me/.claude/settings.json", set)]) else { panic!("{mode}") };
            assert!(said.contains(mode) && said.contains("/home/me/.claude/settings.json") && said.contains("defaultMode"), "{said}");
            assert!(ran.borrow().is_empty(), "nothing opened for {mode}");
        }
    }

    /// Claude Desktop cuts a longer prompt, and an agent given half a prompt does half the job.
    #[test]
    fn a_prompt_too_long_for_the_link_is_not_sent_cut() {
        let long = "x".repeat(PROMPT_LIMIT + 1);
        let (mut run, ran) = recorder(|_| Ok(String::new()));
        let Err(Trouble::Passing(said)) = start_with(&launch(&long), &mut run, &[]) else { panic!() };
        assert!(said.contains("14001") && said.contains("14000"), "{said}");
        assert!(ran.borrow().is_empty());
    }

    /// The worktree is ready by now, so a failed open says how to finish by hand.
    #[test]
    fn a_link_that_would_not_open_says_how_to_finish_by_hand() {
        let (mut run, _) = recorder(|_| Err("no handler for claude://".into()));
        let Err(Trouble::Passing(said)) = start_with(&launch("Review it"), &mut run, &[]) else { panic!() };
        assert!(said.contains("no handler") && said.contains("/w/pr-1-x"), "{said}");
    }

    /// Every Code session it keeps and has not archived is a claim, on the folder it was opened on
    /// and on the worktree it made there if any. Archived ones are closed.
    #[test]
    fn its_claims_are_its_sessions_not_archived() {
        let root = data_root(
            "claims",
            &[
                ("local_a.json", r#"{"sessionId":"local_a","cwd":"/w/pr-1-x","originCwd":"/w/pr-1-x","isArchived":false,"title":"Review PR 1"}"#),
                ("local_b.json", r#"{"sessionId":"local_b","cwd":"/w/pr-2-y/.claude/worktrees/z","originCwd":"/w/pr-2-y","isArchived":false,"title":""}"#),
                ("local_c.json", r#"{"sessionId":"local_c","cwd":"/w/pr-3-z","originCwd":"/w/pr-3-z","isArchived":true,"title":"old"}"#),
            ],
        );
        let mut found = sessions_in(&[root.clone(), root.join("no-such-folder")]).unwrap();
        found.sort();
        assert_eq!(
            found,
            [
                ("/w/pr-1-x".to_string(), "Review PR 1".to_string()),
                ("/w/pr-2-y".to_string(), "local_b".to_string()),
                ("/w/pr-2-y/.claude/worktrees/z".to_string(), "local_b".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// No Claude Desktop is nobody working. A session it cannot read is nobody can say, so no worktree
    /// is recycled on the strength of it.
    #[test]
    fn no_desktop_is_nobody_working_and_an_unreadable_session_is_unsure() {
        assert_eq!(sessions_in(&[]), Ok(vec![]));
        let root = data_root("unreadable", &[("local_a.json", "{ not json")]);
        let said = sessions_in(std::slice::from_ref(&root)).unwrap_err();
        assert!(said.contains("local_a.json"), "{said}");
        let _ = std::fs::remove_dir_all(root);
    }
}
