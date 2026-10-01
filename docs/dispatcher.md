# The Herdr dispatcher

**The one thing GitHoot does that is not reading.** Everywhere else it polls your portals (GitHub,
GitLab), draws an icon and opens a page. Here it creates git branches, creates worktrees, and starts agents. That is a
different kind of program, so it is off by default and a typo leaves it off.

It is the first [integration](integrations.md): built into GitHoot, off until you install it on the
**Integrations** tab.

## What it does, each pass

After every poll, and at least every thirty seconds, for each bar GitHoot is watching and the
dispatcher is switched on for (all but **approved**, by default; see [Settings](#settings)):

1. Take the bar from GitHoot's own poll. **A bar GitHoot has no confirmed answer for is skipped
   entirely.** That is not the same as an empty bar: acting on it would mean going quiet for
   exactly as long as GitHub is broken, and looking identical to a quiet morning.
2. Skip muted pull requests. They drop out of the state file, so when the mute ends the pull request
   is new to the dispatcher as well.
3. **A pull request new to the bar gets an agent.** One already dispatched while in the bar gets
   nothing more, whatever happens to it: a push, a comment, a label. One that leaves the bar is
   forgotten, so coming back into it (changes requested again, say) is an arrival like any other.
4. Somebody already in the worktree: say so and leave them alone. Nobody home: fetch the pull
   request's head, cut a branch of its own, open a worktree, start an agent, hand it the prompt.

**It works the same for every portal and asks no forge anything.** What is in a bar comes from
GitHoot's own poll, who is already working from Herdr, and the checkout from git: each portal says
which ref holds a pull request's head on the base repository (`refs/pull/<n>/head` on GitHub,
`refs/merge-requests/<iid>/head` on GitLab), and both forges publish that ref for forks too. A portal
added later gets the dispatcher by saying the same.

Agents are no longer nudged when someone comments after they started. That needed a forge tool to
read comments, which is exactly what the dispatcher no longer does; an agent you want to re-read
something, you ask in its pane.

### Two rules that shape everything

**A start that fails is retried when the pull request changes, not every pass.** A fetch that failed
or a Herdr that would not answer is recorded with the pull request's `updated_at`, and tried again
only once that moves. Retrying every thirty seconds would repeat a failure that cannot go
differently.

The exception is a **setup** failure, such as no clone where `integration.herdr.cloneRoot` says there should
be one. That is a mistake you can correct, and the very next pass can then succeed, so it is not
recorded. It is said once rather than every pass, or a wrong path would write a line per pull
request every thirty seconds forever. The line says where it looked and why there:

> `no clone of QUMEA/care-web-ui: "Clones live in" is empty, so it looks in C:\Users\me\projects, and that folder does not exist. To fix it, set "Clones live in" on the Herdr dispatcher's settings page`

A clone root that does not exist, or holds no git clone at all, is caught before any pull request
needs it: Install says so, the page lists it and reads *Installed, but check its settings*, and the
runner logs it once.

**The agent works on a branch of its own**, `githoot/pr-<number>-<repo>`, cut from the pull request's
head. Never the pull request's branch itself: that is usually checked out in your own clone, because
it is usually *your* pull request, and git refuses one branch in two worktrees. The agent is
assisting you, not replacing you, so it has to be able to start while you are mid-edit. The branch
name also makes an accidental push obvious.

## Installing it

**Integrations ▸ Herdr dispatcher** has the Install button, the two folders, and a line saying where
it will look right now:

> Right now it would look for clones in `D:\projects` and put worktrees in `D:\worktrees`.

There is no restart: the runner reads `config.txt` on every pass. **Uninstall** switches it off again and
keeps its prompts, state and settings, so installing it later picks up where it left off. Not on
macOS, where the page says so and offers no Install.

**Press Dry run first.** It runs a real pass with every effect suppressed and writes no state, so it
cannot change what the next real pass would do. It explains every outcome, including the quiet ones
a real pass does not bother to say, which is the whole point of pressing it.

### Settings

| Key | Default | Meaning |
|---|---|---|
| `integration.herdr.enabled` | `off` | Installed or not. What Install and Uninstall write. An unrecognised value leaves it off |
| `integration.herdr.workRequired` | `on` | Start agents for the amber bar: your pull requests that need work |
| `integration.herdr.requestedReviews` | `on` | Start agents for the red bar: reviews requested of you |
| `integration.herdr.approved` | `off` | Start agents for the green bar: your approved pull requests |
| `integration.herdr.cloneRoot` | `~/projects` | Where your clones live, one directory per repository name |
| `integration.herdr.worktreeRoot` | `~/worktrees` | Where the per-pull-request worktrees go |

Both folders have a **Browse…** button beside the box, which opens your system's folder picker
(Explorer's on Windows, `zenity` or `kdialog` on Linux) and fills the box with what you choose. Save
keeps it. A browser cannot give a page a real path, so GitHoot shows the picker itself.

The three bar switches are checkboxes on its page. **Approved is off by default**: an approved pull
request is usually one you are about to merge yourself, and an agent started for it mostly spends
tokens on work that is done. A bar switched off is skipped before anything is asked of `gh`.

### Switching on is "from now on"

Installing it, or switching a bar on, **starts nothing for what is already waiting.** The first pass
takes every pull request in the bar as dispatched, as of that moment, and says so in the log:

> `[work-required] switched on: 7 pull request(s) taken as seen, none started`

From then on the normal rule applies: a pull request that enters the bar gets an agent, and one that
was already there gets nothing, whatever is said on it. Switching a bar off, and
Uninstall, forget that baseline, so switching back on is "from now on" again. A bar GitHoot has no
confirmed answer for yet, such as before the first poll, waits: a baseline of nothing would make the
backlog look new a moment later. Dry run says what the first pass would take as seen.

`GITHOOT_CLONE_ROOT` and `GITHOOT_WORKTREE_ROOT` still work, but only as a one-off override for a run
started from a shell. The settings come first, deliberately: GitHoot is started from a tray icon, a
shortcut or autostart, none of which carry a shell's environment.

`GITHOOT_AGENT_KIND` picks the agent (`claude` by default; any kind `herdr agent start --kind` accepts).

### What it needs

`herdr` and `git`, on `PATH`. The page shows each as a chip and names any that will not run, and
nothing is dispatched until they all do. No forge tool is required by the dispatcher; the agent reads
its pull request with the one that fits (`gh` for GitHub, `glab` for GitLab), signed in as you, so
have that installed for the portals you use.

## The prompts

One file per bar: what the agent is told when a pull request arrives in it. Edit them on
its page, installed or not, or in `~/.githoot/integrations/herdr/prompts/`.

**Clear a box to go back to the shipped default.** Beside the prompts sits `.shipped`, holding a
hash of the text GitHoot last wrote into each one. At every start, a prompt that still matches its
hash has not been edited and is brought up to the new default; one that does not is yours and is
left alone, and the page names every prompt it kept. Clearing a box writes the default back *and
records it as ours*, so a prompt you reset follows future defaults again.

Placeholders: `{url}` `{repo}` `{number}` `{branch}` `{title}` `{author}` `{labels}` `{changes}`.
`{labels}` is the pull request's labels, comma-separated, or `none`. `{changes}` is its size, such as
`+120 -30 in 4 files`, or `unknown`. Both come from GitHoot's own poll, for every portal.

**`{title}`, `{author}` and `{labels}` are written by other people**, and the prompt is an
instruction to an agent holding your forge credential. The shipped defaults put them in a labelled
block that the next line tells the agent is data, not instructions. That is the standard mitigation
and not a cure. Keep it if you rewrite the prompts.

## The guard rail that actually matters

The prompts say "do not push". **That is a request, not a control.** Two things are controls:

**The agent's branch pushes nowhere.** Each `githoot/<slug>` branch gets a push remote that does not
exist (`branch.githoot/<slug>.pushRemote = githoot-no-push`) and no upstream, so a plain `git push`,
or `git push -u`, fails with "'githoot-no-push' does not appear to be a git repository", whatever your
git config says. That matters: with `push.autoSetupRemote` on, a branch without an upstream would
otherwise be created on the remote by a plain push, which is how 3.2.0 let agents publish their
`githoot/…` branches. It is set per branch, so your own branches push as always. The pull request's
head is fetched into `refs/githoot-pr/<slug>`, never under `refs/remotes/`, so git never mistakes it
for a branch on the remote.

The other control is the agent's own permission prompt, for anything explicit such as
`git push origin …`. Herdr starts it interactively, so a `gh pr review`, a `glab mr merge` or an
explicit push asks you first, in that pane. That holds only while you never start the dispatched agent
with `--dangerously-skip-permissions`.

What GitHoot does **not** hand over: its own credential. GitHoot's token is read-only and never
leaves it. The agent reads the diff and the comments itself, with `gh` or `glab`, under your credential.

## Who is already working a pull request

Herdr answers that. A live agent sitting in the worktree is the claim, and only an agent the
dispatcher started counts: a session you opened yourself in your own clone is not a claim on a
dispatcher worktree.

When the agent has gone but its worktree is still on disk, the worktree is reused if Herdr still
knows it, recycled if Herdr has forgotten it **and** it holds no uncommitted changes and no unpushed
commits, and **left alone with a line in the log** if it holds either. The dispatcher never deletes
work.

## When something goes wrong

Anything about a specific pull request is logged at **error** level, so it survives the default
`logLevel=error`. A dispatcher that failed quietly every pass would look exactly like a quiet
morning, which is the one thing this feature must never do. Routine progress is info level.

### Windows: `agent_pane_busy`

> `agent target pane w1:p1 is not an available shell`

Herdr refuses a pane whose shell has a child process, and Git for Windows' `bin\bash.exe` is a shim
that launches `usr\bin\bash.exe` and blocks on it. Every pane is then `bash -> bash`. Point Herdr at
the real one in `%APPDATA%\herdr\config.toml`:

```toml
[terminal]
default_shell = 'C:\Program Files\Git\usr\bin\bash.exe'
```

Then `herdr server reload-config`. Existing panes keep their old shell until they are recreated. The
same applies to any shell that chain-launches another, such as a PowerShell 5.1 profile that execs
`pwsh`. This is expected Herdr behaviour, not a bug in either program.

GitHoot treats this as a setup failure, so the pull request is not recorded and the next pass picks
it up once the config is fixed.

### An agent that starts slowly

> `timed out waiting for agent startup` or `agent … is blocked during startup and is not ready for prompts`

Both mean Herdr stopped waiting, not that the agent stopped starting. Claude with a few MCP servers
often takes longer than Herdr's default thirty seconds. GitHoot gives `agent start` two minutes, and
when it still gives up, checks whether the agent is there anyway. If it is, GitHoot waits up to two
more minutes for it to be ready and then sends the prompt. The prompt counts as delivered only once
the agent is seen working on it.

If the agent never becomes ready, it is usually asking something on screen, such as whether to trust
the folder or allow an MCP server. The log names the pane. Answer the question there, then send the
prompt with the `herdr agent prompt` command the log gives.

## Its files

Everything it keeps is in `~/.githoot/integrations/herdr/`: one state file per bar, a `.armed` marker
per bar that has its baseline, `prompts/`, and two caches (`viewer`, your login, and `branches/`,
each pull request's head branch).

## Upgrading from 2.4.0 or 3.0.0

The dispatcher was a setting of its own then. It is an integration now, and nothing is migrated:

- **The keys are gone.** `dispatcher`, `dispatcherCloneRoot` and `dispatcherWorktreeRoot` are no
  longer read, and nothing says so: delete the lines whenever you like. Install it under Integrations and set the
  two folders on its page, or write the `integration.herdr.*` keys above.
- **Its files moved.** `~/.githoot/dispatch/` and `~/.githoot/prompts/` are no longer read. Move
  `prompts/` into `~/.githoot/integrations/herdr/` to keep edited prompts. The old state is not
  needed: Install takes whatever is in the bars as seen and starts nothing for it.

## Upgrading from the shipped script

Before 2.4.0 the dispatcher was a bash script installed by a button, kept alive by a systemd user
service. Nothing removes it for you, and if it is still running it will keep dispatching alongside
the in-process one. On Linux:

```bash
systemctl --user disable --now ght-dispatch
rm ~/.local/bin/ght-dispatch ~/.local/bin/ght-dispatch-loop \
   ~/.config/systemd/user/ght-dispatch.service
```

Your prompts moved from `~/.config/ght-dispatch/prompts/` to `~/.githoot/integrations/herdr/prompts/`.
Copy any you had edited across; the rest are shipped defaults and will be written for you.

The dispatcher no longer uses [the local API](local-api.md), so `localApi` is only needed if you
have your own scripts reading the bars.
