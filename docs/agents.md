# Agents

**The one thing GitHoot does that is not reading.** Everywhere else it polls your portals (GitHub,
GitLab), draws an icon and opens a page. Here it starts a coding agent for each pull request that
needs you: it creates a git branch and a worktree, opens a terminal in your agent workspace of choice
(the **client**), and hands the agent its prompt. That is a different kind of program, so it is off
by default and a typo leaves it off.

It is part of GitHoot, with its own **Agents** page in the settings. It used to be the *Herdr
dispatcher*, an integration; see [Upgrading from the Herdr dispatcher](#upgrading-from-the-herdr-dispatcher-34x).

## What it does, each pass

After every poll, and at least every thirty seconds, for each bar GitHoot is watching and the
Agents is switched on for (all but **approved**, by default; see [Settings](#settings)):

1. Take the bar from GitHoot's own poll. **A bar GitHoot has no confirmed answer for is skipped
   entirely.** That is not the same as an empty bar: acting on it would mean going quiet for
   exactly as long as GitHub is broken, and looking identical to a quiet morning.
2. Skip muted pull requests. They drop out of the state file, so when the mute ends the pull request
   is new to Agents as well.
3. **A pull request new to the bar gets an agent.** One already dispatched while in the bar gets
   nothing more, whatever happens to it: a push, a comment, a label. One that leaves the bar is
   forgotten, so coming back into it (changes requested again, say) is an arrival like any other.
4. Somebody already in the worktree: say so and leave them alone. Nobody home: fetch the pull
   request's head, cut a branch of its own, open a worktree, start an agent, hand it the prompt.

**It works the same for every portal and asks no forge anything.** What is in a bar comes from
GitHoot's own poll, who is already working from the clients, and the checkout from git: each portal says
which ref holds a pull request's head on the base repository (`refs/pull/<n>/head` on GitHub,
`refs/merge-requests/<iid>/head` on GitLab), and both forges publish that ref for forks too. A portal
added later gets agents by saying the same.

Agents are no longer nudged when someone comments after they started. That needed a forge tool to
read comments, which is exactly what GitHoot no longer does; an agent you want to re-read something,
you ask in its terminal.

### Two rules that shape everything

**A start that fails is retried when the pull request changes, not every pass.** A fetch that failed
or a client that would not answer is recorded with the pull request's `updated_at`, and tried again
only once that moves. Retrying every thirty seconds would repeat a failure that cannot go
differently.

The exception is a **setup** failure, such as no clone where `agentsCloneRoot` says there should
be one. That is a mistake you can correct, and the very next pass can then succeed, so it is not
recorded. It is said once rather than every pass, or a wrong path would write a line per pull
request every thirty seconds forever. The line says where it looked and why there:

> `no clone of QUMEA/care-web-ui: "Clones live in" is empty, so it looks in C:\Users\me\projects, and that folder does not exist. To fix it, set "Clones live in" on the Agents settings page`

A clone root that does not exist, or holds no git clone at all, is caught before any pull request
needs it: switching on says so, the page lists it and reads *On, but check its settings*, and the
runner logs it once.

**The agent works on a branch of its own**, `githoot/pr-<number>-<repo>`, cut from the pull request's
head. Never the pull request's branch itself: that is usually checked out in your own clone, because
it is usually *your* pull request, and git refuses one branch in two worktrees. The agent is
assisting you, not replacing you, so it has to be able to start while you are mid-edit. The branch
name also makes an accidental push obvious.

## Switching it on

**Settings ▸ Agents** has the **Switch on** button, the client and agent choices, the bars, the two
folders, and a line saying where it will look right now. There is no restart: the runner reads
`config.txt` on every pass. **Switch off** keeps its prompts, state and settings, so switching it on
later picks up where it left off. Not on macOS, where the page says so and offers no switch.

**Press Dry run first.** It runs a real pass with every effect suppressed and writes no state, so it
cannot change what the next real pass would do. It explains every outcome, including the quiet ones
a real pass does not bother to say, which is the whole point of pressing it.

### Settings

| Key | Default | Meaning |
|---|---|---|
| `agents` | `off` | On or off. What Switch on and Switch off write. An unrecognised value leaves it off |
| `agentsClient` | `herdr` | Where agents open: `herdr` (Herdr in a terminal), `herdr-app` (the [Herdr app](#the-herdr-app)) or `orca` ([Orca](#orca)). An unrecognised value is `herdr` |
| `agentsKind` | `claude` | Which agent starts. Claude is the only one for now; the page shows it as a choice of one |
| `agentsWorkRequired` | `on` | Start agents for the amber bar: your pull requests that need work |
| `agentsRequestedReviews` | `on` | Start agents for the red bar: reviews requested of you |
| `agentsApproved` | `off` | Start agents for the green bar: your approved pull requests |
| `agentsCloneRoot` | `~/projects` | Where your clones live, one directory per repository name |
| `agentsWorktreeRoot` | `~/worktrees` | Where the per-pull-request worktrees go |

Under the client radio sits the chosen client's card: the programs it needs as chips, how to get one
that is missing, and what it needs from you, such as Orca's permission setting.

Both folders have a **Browse…** button beside the box, which opens your system's folder picker
(Explorer's on Windows, `zenity` or `kdialog` on Linux) and fills the box with what you choose. Save
keeps it. A browser cannot give a page a real path, so GitHoot shows the picker itself.

The three bar switches are checkboxes on its page. **Approved is off by default**: an approved pull
request is usually one you are about to merge yourself, and an agent started for it mostly spends
tokens on work that is done. A bar switched off is skipped before anything is asked of `gh`.

### Switching on is "from now on"

Switching it on, or switching a bar on, **starts nothing for what is already waiting.** The first pass
takes every pull request in the bar as dispatched, as of that moment, and says so in the log:

> `[work-required] switched on: 7 pull request(s) taken as seen, none started`

From then on the normal rule applies: a pull request that enters the bar gets an agent, and one that
was already there gets nothing, whatever is said on it. Switching a bar off, and
switching Agents off, forget that baseline, so switching back on is "from now on" again. A bar GitHoot has no
confirmed answer for yet, such as before the first poll, waits: a baseline of nothing would make the
backlog look new a moment later. Dry run says what the first pass would take as seen.

`GITHOOT_CLONE_ROOT` and `GITHOOT_WORKTREE_ROOT` still work, but only as a one-off override for a run
started from a shell. The settings come first, deliberately: GitHoot is started from a tray icon, a
shortcut or autostart, none of which carry a shell's environment.

### What it needs

The chosen client's program and `git`, on `PATH`: `herdr` for Herdr in a terminal, `hc` for the Herdr
app, `orca-ide` for Orca (`orca` on Windows). The client card shows each as a chip and names any that
will not run, and nothing is started until they all do. No forge tool is required by GitHoot; the agent reads
its pull request with the one that fits (`gh` for GitHub, `glab` for GitLab), signed in as you, so
have that installed for the portals you use.

## The prompts

One file per bar: what the agent is told when a pull request arrives in it. Edit them on
its page, on or not, or in `~/.githoot/agents/prompts/`.

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
`git push origin …`. The client starts it interactively, so a `gh pr review`, a `glab mr merge` or an
explicit push asks you first, in its terminal. That holds only while the agent is never started with
`--dangerously-skip-permissions`, which is why GitHoot refuses an Orca launch that would add it (see
[Orca](#orca)).

What GitHoot does **not** hand over: its own credential. GitHoot's token is read-only and never
leaves it. The agent reads the diff and the comments itself, with `gh` or `glab`, under your credential.

## The Herdr app

**Agents open in** on its page picks the client: **Herdr in a terminal** (the `herdr` server the TUI
attaches to, the default), the **Herdr app** or **Orca**. The two Herdrs are separate programs with
separate lists of terminals, so an agent started in one never shows up in the other.

In the app, GitHoot drives `hc`, the command line of herdr-core, which runs the app's terminals. It
never runs `herdr-app` itself, so the app does not need to be open:

- **The worktree** is made with plain `git worktree add`. The app has no worktree command; it shows
  any git worktree, nested under its repository, which is what its own New worktree does too.
- **The agent** starts with one `hc run --bg --no-parent --keep --name <slug> --cwd <worktree>
  --label herdr.space=githoot-<slug> -- <agent> "<prompt>"`. The label puts it in a space of its
  own, named after the worktree, the next time the app is open. The prompt is the agent's first
  argument, because the app's manual warns that a message sent to an agent still starting can be
  lost. `--no-parent` keeps the agent alive if GitHoot itself was started from an app terminal.
- **No folder trust is skipped.** Herdr in a terminal passes `--trust-repository`; `hc` has no such
  switch, so Claude asks whether to trust each new worktree once. The app shows that agent as
  waiting for you.

## Orca

With **Orca** chosen, GitHoot drives [Orca](https://orca.dev) through its own command line, which
is `orca-ide` on Linux, where `orca` is the GNOME screen reader, and `orca` on Windows. **Orca must
be running**: GitHoot never starts it. If it is not, the log says so and the next pass tries again.
Every call runs with `ORCA_TELEMETRY_DISABLED=1`, as Orca otherwise reports usage to an analytics
service.

**Set Orca's agent permissions to Manual or Ask first** (Settings ▸ Agents ▸ Agent permissions).
Orca chooses the agent's permission flags itself, on every launch, and its default, Yolo, starts
Claude with `--dangerously-skip-permissions`. That would take away the guard rail above: the agent
would push or merge without asking you. Orca has no command to change or read that setting, but
each launch reports the arguments it used. So GitHoot checks them, and an agent Orca would let act
without asking (Yolo, or Auto) is closed again before it is given the pull request, with this in
the log until the setting is changed:

> `Orca starts claude with --dangerously-skip-permissions, which lets it act without asking you, so it was closed before it got the pull request. In Orca, set Settings > Agents > Agent permissions to Manual or Ask`

What a dispatch does there:

- **The repository** is added to Orca (`repo add`, harmless when it is already there) and set to
  show worktrees made outside Orca (`repo set --external-worktree-visibility show`).
- **The worktree** is made with plain `git worktree add`, as for the Herdr app. Orca's own
  `worktree create` chooses its own path and may rename a taken branch, which would break the
  folders and the push guard.
- **The agent** starts in an Orca terminal in that worktree (`terminal create --command claude`).
  Once it is ready (`terminal wait --for tui-idle`), the prompt is pasted as one block
  (`terminal send --enter`), and it counts as delivered only when the agent is seen starting a turn.
- **Who is working:** every agent `agent list` shows in one of these worktrees, finished ones
  included, since Orca keeps those listed while they sleep.

Adding another client is one new file in `src/agents/clients/` and one more option on the radio.

## Who is already working a pull request

Every client answers that, **whichever one is chosen**: an agent you started in another one is still
somebody working that pull request. A live agent sitting in the worktree is the claim, and only an
agent counts, not a shell: in Herdr in a terminal that is a named agent, one GitHoot started,
because Herdr keeps a plain shell pane in a worktree after its agent has gone. In the app it is a
named terminal or any terminal reporting an agent status, since there only an agent reports one. In
Orca it is every agent it lists.

When the agent has gone but its worktree is still on disk, the worktree is reused if Herdr in a
terminal still knows it, recycled if it does not (the app and Orca never do) **and** it holds no
uncommitted changes and no unpushed commits, and **left alone with a line in the log** if it holds
either. GitHoot never deletes work.

A client that is installed but **does not answer** the question is not taken as "nobody there". A
client that is not installed, or not running, has nobody working and is skipped. But one that is
there and fails to answer (a timeout, a server restarting) might be hiding the agent standing in that
worktree. So while any client fails to answer, no worktree is recycled: the pull
request waits, the log says which client and why, and the next pass tries again.

## When something goes wrong

Anything about a specific pull request is logged at **error** level, so it survives the default
`logLevel=error`. Agents failing quietly every pass would look exactly like a quiet
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

Everything it keeps is in `~/.githoot/agents/`: one state file per bar, a `.armed` marker
per bar that has its baseline, `prompts/`, and two caches (`viewer`, your login, and `branches/`,
each pull request's head branch).

## Upgrading from the Herdr dispatcher (3.4.x)

The Herdr dispatcher became Agents, part of GitHoot rather than an integration. **A hard cut, nothing
is migrated**, so after updating it is off:

- **Switch it on again** under **Settings ▸ Agents**, and set the two folders there. Nothing reads
  `integration.herdr.*` any more; delete those lines whenever you like. If `integration.herdr.enabled`
  was `on`, the log says once that it is no longer read and where it went. The Integrations tab is
  gone, and so are its addresses.
- **Its files moved** from `~/.githoot/integrations/herdr/` to `~/.githoot/agents/`. To keep edited
  prompts, copy `prompts/` across before switching it on. The old state is not needed: switching on
  takes whatever is in the bars as seen and starts nothing for it.
- **`GITHOOT_AGENT_KIND` is gone.** Claude is the only agent for now, shown on the page as a choice of
  one.

## Upgrading from 2.4.0 or 3.0.0

The dispatcher was a setting of its own then, and nothing is migrated:

- **The keys are gone.** `dispatcher`, `dispatcherCloneRoot` and `dispatcherWorktreeRoot` are no
  longer read, and nothing says so: delete the lines whenever you like. Switch Agents on and set the
  two folders on its page, or write the `agents*` keys above.
- **Its files moved.** `~/.githoot/dispatch/` and `~/.githoot/prompts/` are no longer read. Copy
  `prompts/` into `~/.githoot/agents/` to keep edited prompts. The old state is not needed.

## Upgrading from the shipped script

Before 2.4.0 the dispatcher was a bash script installed by a button, kept alive by a systemd user
service. Nothing removes it for you, and if it is still running it will keep dispatching alongside
GitHoot. On Linux:

```bash
systemctl --user disable --now ght-dispatch
rm ~/.local/bin/ght-dispatch ~/.local/bin/ght-dispatch-loop \
   ~/.config/systemd/user/ght-dispatch.service
```

Your prompts moved from `~/.config/ght-dispatch/prompts/` to `~/.githoot/agents/prompts/`.
Copy any you had edited across; the rest are shipped defaults and will be written for you.

Agents does not use [the local API](local-api.md), so `localApi` is only needed if you
have your own scripts reading the bars.
