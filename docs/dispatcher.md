# The Herdr dispatcher

**The one thing GitHoot does that is not reading.** Everywhere else it polls GitHub, draws an icon
and opens a page. Here it creates git branches, creates worktrees, and starts agents. That is a
different kind of program, so it is off by default and a typo leaves it off.

It is the first [integration](integrations.md): built into GitHoot, off until you install it on the
**Integrations** tab.

## What it does, each pass

After every poll, and at least every thirty seconds, for each bar GitHoot is watching and the
dispatcher is switched on for (all but **approved**, by default; see [Settings](#settings)):

1. Take the bar from GitHoot's own poll. **A bar GitHoot has no confirmed answer for is skipped
   entirely.** That is not the same as an empty bar: acting on it would mean going quiet for
   exactly as long as GitHub is broken, and looking identical to a quiet morning.
2. Skip muted pull requests, and pull requests from any forge but GitHub: the dispatcher asks `gh`
   about each one, so it cannot act on anything else. Muted ones drop out of the state file, so when
   the mute ends the pull request is new to the dispatcher as well.
3. For each of the rest, compare `updated_at` with what was recorded last time. Unchanged: nothing,
   silently, which is almost every pull request almost always.
4. Changed: ask GitHub once for the newest comment, review or reply **not written by you**. Only
   that is worth waking an agent for. A push of your own fixes is exactly the case that must not
   re-run a review and bill you for "no changes".
5. Somebody already in the worktree: **nudge them** to re-read. Nobody home: fetch the head branch,
   cut a branch of its own, open a worktree, start an agent, hand it the prompt.

### Two rules that shape everything

**Each version of a pull request is looked at exactly once.** Started, nudged, skipped or failed,
its `updated_at` is recorded and it is not touched again until GitHub changes it. Retrying every
thirty seconds would burn your API rate limit on a `gh` lookup that cannot go differently. Retrying
when the pull request changes is the retry that can.

The exception is a **setup** failure, such as no clone where `integration.herdr.cloneRoot` says there should
be one. That is a mistake you can correct, and the very next pass can then succeed, so it is not
recorded. It is said once rather than every pass, or a wrong path would write a line per pull
request every thirty seconds forever.

**The agent works on a branch of its own**, `githoot/pr-<number>-<repo>`, cut from the pull request's
head. Never the pull request's branch itself: that is usually checked out in your own clone, because
it is usually *your* pull request, and git refuses one branch in two worktrees. The agent is
assisting you, not replacing you, so it has to be able to start while you are mid-edit. The branch
name also makes an accidental push obvious.

## Installing it

**Integrations ▸ Herdr dispatcher** has the Install button, the two folders, and a line saying where
it will look right now:

> Right now it would look for clones in `D:\projects` and put worktrees in `D:\worktrees`.

There is no restart: the runner reads `config.txt` on every pass. **Remove** switches it off again and
keeps its prompts, state and settings, so installing it later picks up where it left off. Not on
macOS, where the page says so and offers no Install.

**Press Dry run first.** It runs a real pass with every effect suppressed and writes no state, so it
cannot change what the next real pass would do. It explains every outcome, including the quiet ones
a real pass does not bother to say, which is the whole point of pressing it.

### Settings

| Key | Default | Meaning |
|---|---|---|
| `integration.herdr.enabled` | `off` | Installed or not. What Install and Remove write. An unrecognised value leaves it off |
| `integration.herdr.workRequired` | `on` | Start agents for the amber bar: your pull requests that need work |
| `integration.herdr.requestedReviews` | `on` | Start agents for the red bar: reviews requested of you |
| `integration.herdr.approved` | `off` | Start agents for the green bar: your approved pull requests |
| `integration.herdr.cloneRoot` | `~/projects` | Where your clones live, one directory per repository name |
| `integration.herdr.worktreeRoot` | `~/worktrees` | Where the per-pull-request worktrees go |

The three bar switches are checkboxes on its page. **Approved is off by default**: an approved pull
request is usually one you are about to merge yourself, and an agent started for it mostly spends
tokens on work that is done. A bar switched off is skipped before anything is asked of `gh`.

### Switching on is "from now on"

Installing it, or switching a bar on, **starts nothing for what is already waiting.** The first pass
takes every pull request in the bar as seen, as of that moment, with everything said on it so far as
read, and says so in the log:

> `[work-required] switched on: 7 pull request(s) taken as seen, none started`

From then on the normal rules apply: a pull request that enters the bar is new, and a comment from
someone else on one that was already there wakes an agent like any other. Switching a bar off, and
Remove, forget that baseline, so switching back on is "from now on" again. A bar GitHoot has no
confirmed answer for yet, such as before the first poll, waits: a baseline of nothing would make the
backlog look new a moment later. Dry run says what the first pass would take as seen.

`GITHOOT_CLONE_ROOT` and `GITHOOT_WORKTREE_ROOT` still work, but only as a one-off override for a run
started from a shell. The settings come first, deliberately: GitHoot is started from a tray icon, a
shortcut or autostart, none of which carry a shell's environment.

`GITHOOT_AGENT_KIND` picks the agent (`claude` by default; any kind `herdr agent start --kind` accepts).

### What it needs

`herdr`, `gh` (signed in) and `git`, on `PATH`. The page names any that will not run, and nothing is
dispatched until they all do.

## The prompts

One file per bar, plus the nudge an agent gets when its pull request changes under it. Edit them on
its page, installed or not, or in `~/.githoot/integrations/herdr/prompts/`.

**Clear a box to go back to the shipped default.** Beside the prompts sits `.shipped`, holding a
hash of the text GitHoot last wrote into each one. At every start, a prompt that still matches its
hash has not been edited and is brought up to the new default; one that does not is yours and is
left alone, and the page names every prompt it kept. Clearing a box writes the default back *and
records it as ours*, so a prompt you reset follows future defaults again.

Placeholders: `{url}` `{repo}` `{number}` `{branch}` `{title}` `{author}`.

**`{title}` and `{author}` are written by whoever opened the pull request**, and the prompt is an
instruction to an agent holding your `gh` credential. The shipped defaults put them in a labelled
block that the next line tells the agent is data, not instructions. That is the standard mitigation
and not a cure. Keep it if you rewrite the prompts.

## The guard rail that actually matters

The prompts say "do not push". **That is a request, not a control.**

The control is the agent's own permission prompt. Herdr starts it interactively, so a `gh pr review`,
a `gh pr merge` or a `git push` asks you first, in that pane. That is the whole defence, and it holds
only while you never start the dispatched agent with `--dangerously-skip-permissions`.

What GitHoot does **not** hand over: its own credential. GitHoot's token is read-only and never
leaves it. The agent reads the diff and the comments itself, with `gh`, under your credential.

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

## Its files

Everything it keeps is in `~/.githoot/integrations/herdr/`: one state file per bar, a `.armed` marker
per bar that has its baseline, `prompts/`, and two caches (`viewer`, your login, and `branches/`,
each pull request's head branch).

## Upgrading from 2.4.0 or 3.0.0

The dispatcher was a setting of its own then. It is an integration now, and nothing is migrated:

- **The keys are gone.** `dispatcher`, `dispatcherCloneRoot` and `dispatcherWorktreeRoot` are no
  longer read, and nothing says so: delete the lines whenever you like. Install it on the Integrations tab and set the
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
