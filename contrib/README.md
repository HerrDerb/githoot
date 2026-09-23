# The dispatcher

**Linux and [Herdr](https://herdr.dev) only, and unsupported in the sense that matters:** GitHoot
ships it, installs it, and versions it, but it depends on tools GitHoot does not control. When
Herdr's CLI changes, this breaks, and the fix is a GitHoot release. That is the trade for one
click.

It turns GitHoot's bars into agents. A pull request lands in **work required** and an agent in
its own worktree reads the review comments and challenges them. Somebody **requests your
review** and an agent summarises the pull request and tells you whether to read it yourself.
Both through GitHoot's [local API](../docs/local-api.md), on your own `gh` credential.

## Installing

Tick **Serve the lists as JSON to local scripts** on the settings page, restart, and the
**Agent dispatcher** section appears. It checks for `herdr`, `gh`, `jq`, `git` and `curl`, and
for a signed-in `gh`, and refuses to install while anything is missing. No Herdr yet? [Install it first](https://herdr.dev/docs/install/); the card links there too. Then one button:

| What it writes | Where |
|---|---|
| `ght-dispatch` | `~/.local/bin/`, executable |
| `ght-dispatch-loop` | `~/.local/bin/`, executable |
| `ght-dispatch.service` | `~/.config/systemd/user/`, enabled and started |

The same button reads **Update** when GitHoot ships a newer script than the one installed, and
there is an **Uninstall** beside it that stops the service and removes all three. Nothing else
is touched: your prompts, state and cache survive an uninstall so a reinstall picks up where
it left off.

**The button writes an executable and enables a service from a page your browser can reach.**
That is a bigger thing than any other setting does, and it is deliberate. What bounds it: the
content is compiled into GitHoot and the page cannot choose it; the paths are fixed; the route
answers `404` unless `localApi` is on; it demands the same same-origin `Origin` as saving
settings; and it does not exist in the binary on any platform but Linux.

## What it does, each tick

Every five seconds, for each bar:

1. Read the bar from GitHoot. **If GitHoot says the list is not known, stop.** That is an
   outage, not an empty board, and acting on it would mean going quiet for exactly as long as
   GitHub is down.
2. For each pull request, compare its `updated_at` with the one recorded in
   `~/.local/state/ght-dispatch/handled.<bar>`.
3. Unchanged: nothing. Changed with an agent already in its worktree: **nudge that agent** to
   re-read. Changed or new with nobody home: fetch the branch, create a worktree through Herdr
   **on a branch of the dispatcher's own, `ght/pr-<n>-<repo>`**, start an agent in it, hand it
   the prompt.
4. Record that this version was looked at. Pull requests that left the bar are forgotten, so
   one that comes back is new again.

**Each version of a pull request is looked at exactly once.** Started, nudged, skipped or failed,
its `updated_at` is recorded and it is not touched again until GitHub changes it. Retrying every
five seconds would spam the journal and, for a `gh` lookup, burn your API rate limit. Retrying
when the pull request changes is the retry that can actually go differently. So a skipped pull
request says why, once, in the journal, and that line is never silenced.

**The agent never works on the pull request's own branch.** It is usually *your* pull request,
so that branch is checked out in your clone, often with your own agent in it, and git refuses
one branch in two worktrees. The dispatcher cuts `ght/pr-<n>-<repo>` from the pull request's
head instead. The agent is assisting you, not replacing you, so it has to be able to start
while you are mid-edit. It means you can also see at a glance which branches are the
dispatcher's.

**Who is already working a pull request is answered by Herdr.** A live agent sitting in the
worktree is the claim. When the agent has gone but its worktree is still on disk, the worktree
is reused if Herdr still knows it, recycled if Herdr has forgotten it *and* it holds no
uncommitted changes and no unpushed commits, and **left alone with a journal line if it holds
either**. The dispatcher never deletes work.

## Prompts

Edit them on the settings page, under **Dispatcher prompts**, one box per bar plus the nudge an
agent gets when its pull request changes. **Clear a box to go back to the shipped default.** The
boxes write `~/.config/ght-dispatch/prompts/<bar>.txt`, so an editor works just as well.

**Update keeps untouched prompts current and never touches yours.** Beside the prompts,
`.shipped` holds a hash of the text GitHoot last wrote into each one. On Update, a prompt that
still matches that hash has not been edited and is replaced with the new default; one that does
not is yours and is left alone, and the page names every prompt it kept. Clearing a box writes the
shipped default back, so a reset prompt follows future defaults again.

Placeholders: `{url}` `{repo}` `{number}` `{branch}` `{title}` `{author}`.

The shipped defaults are short and shaped the same way: a labelled block of facts at the top, the
ask, and an instruction to **end the answer with the pull request's link on its own line**, so the
bottom of every pane is something you can click.

**`{title}` and `{author}` are written by whoever opened the pull request**, and the prompt is an
instruction to an agent holding your `gh` credential. The defaults put them in a block that the
next line tells the agent is data, not instructions. That is the standard mitigation, not a cure.
Keep it if you rewrite the prompts, and never run the dispatched agent with
`--dangerously-skip-permissions`; see below for why that is the line that actually holds.

## The guard rail that actually matters

The prompts say "do not push". **That is a request, not a control.** The control is Claude
Code's own permission prompt: Herdr starts the agent interactively, so a `gh pr review`, a
`gh pr merge` or a `git push` asks you first, in that pane. That is the whole defence, and it
holds only while you never start the dispatched agent with `--dangerously-skip-permissions`.

## Knobs

All environment, all optional, all read by `ght-dispatch`:

| Variable | Default | Meaning |
|---|---|---|
| `GHT_CLONE_ROOT` | `~/projects` | where `<repo-name>` clones live |
| `GHT_WORKTREE_ROOT` | `~/worktrees` | where per-PR worktrees go |
| `GHT_AGENT_KIND` | `claude` | any kind `herdr agent start --kind` accepts |
| `GHT_AXES` | `work-required requested-reviews` | bars the loop covers |
| `GHT_INTERVAL` | `5` | seconds between ticks |
| `GHT_ONLY` | unset | touch one pull request number only |
| `GHT_DRY_RUN` | `1` by hand, `0` in the service | print instead of act |

Run `ght-dispatch work-required` by hand any time. It is a dry run and prints exactly what
the service would do.

## Removing by hand

```bash
systemctl --user disable --now ght-dispatch
rm ~/.local/bin/ght-dispatch ~/.local/bin/ght-dispatch-loop ~/.config/systemd/user/ght-dispatch.service
```
