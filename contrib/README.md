# The dispatcher

**Linux and Windows, [Herdr](https://herdr.dev) only, and unsupported in the sense that matters:**
GitHoot ships it, installs it, and versions it, but it depends on tools GitHoot does not control.
When Herdr's CLI changes, this breaks, and the fix is a GitHoot release. That is the trade for one
click. There is no macOS build, because nothing there has been tried.

On Windows it is the **same bash script**, run by the bash that arrives with
[Git for Windows](https://git-scm.com/download/win), and it lands in the same places: under Git
Bash `$HOME` is your Windows profile directory, so `~/.local/bin` and `~/.config/ght-dispatch` mean
what they say. Two things differ and the table below says which.

It turns GitHoot's bars into agents. A pull request lands in **work required** and an agent in
its own worktree reads the review comments and challenges them. Somebody **requests your
review** and an agent summarises the pull request and tells you whether to read it yourself.
Both through GitHoot's [local API](../docs/local-api.md), on your own `gh` credential.

## Installing

Tick **Serve the lists as JSON to local scripts** on the settings page, restart, and a
**Dispatcher** tab appears beside Settings, Accounts and Muted. It checks for `herdr`, `gh`, `jq`, `git` and `curl`, and
for a signed-in `gh`, and refuses to install while anything is missing. No Herdr yet? [Install it first](https://herdr.dev/docs/install/); the card links there too. Then one button:

| What it writes | Where, on Linux | Where, on Windows |
|---|---|---|
| `ght-dispatch` | `~/.local/bin/`, executable | `~\.local\bin\` |
| `ght-dispatch-loop` | `~/.local/bin/`, executable | `~\.local\bin\` |
| what keeps it running | `ght-dispatch.service` in `~/.config/systemd/user/`, enabled and started | a **GitHoot dispatcher** task in Task Scheduler, registered from `~\.config\ght-dispatch\ght-dispatch.xml` and started |

**How the two checks differ.** On Linux a systemd user service gets a `PATH` of its own, so
preflight looks in exactly the directories the unit's `PATH=` line names and nowhere else: a
`herdr` that only GitHoot can see would pass the check and then be invisible to the service. On
Windows a task inherits your environment, so there is no second list to disagree with, and
preflight asks a real `bash -l` instead. That also means Windows checks for `bash` first, and says
only that when it is missing, because without a shell nothing else can be looked for.

**The Windows task runs as you, when you are logged on**, not as a background service. That is
deliberate rather than a shortcut: the other option puts it in a session where it cannot reach the
Herdr you have open, which is the one thing it needs. The environment the unit sets with
`Environment=` lines rides along in the task's command instead, and stays just as readable:
`schtasks /Query /TN "GitHoot dispatcher" /XML` prints it back, the way `systemctl --user cat` does.

The same button reads **Update** when GitHoot ships a newer script than the one installed, and
there is an **Uninstall** beside it that stops the service and removes all three. Nothing else
is touched: your prompts, state and cache survive an uninstall so a reinstall picks up where
it left off.

**The button writes an executable and registers something that starts at login, from a page your
browser can reach.** That is a bigger thing than any other setting does, and it is deliberate. What
bounds it: the content is compiled into GitHoot and the page cannot choose it; the paths are fixed;
the route answers `404` unless `localApi` is on; it demands the same same-origin `Origin` as saving
settings; and it does not exist in the macOS binary at all.

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
   **Muted pull requests are skipped** (muted from the PR page); they drop out of the state file,
   so when the mute ends the pull request is new to the dispatcher as well.
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

Edit them on the **Dispatcher** tab, under **Dispatcher prompts**, one box per bar plus the nudge an
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

**On Windows, check `GHT_CLONE_ROOT` before you turn the dry run off.** The default is
`~/projects`, which under Git Bash is `C:\Users\you\projects`. If your clones live on another
drive, say `D:\projects`, the dispatcher finds no clone and skips every pull request with a line
saying so. Set `GHT_CLONE_ROOT=/d/projects` and `GHT_WORKTREE_ROOT=/d/worktrees`, in POSIX form,
since the script is bash. It converts them to `D:\...` itself wherever Herdr needs that shape.

## Removing by hand

Linux:

```bash
systemctl --user disable --now ght-dispatch
rm ~/.local/bin/ght-dispatch ~/.local/bin/ght-dispatch-loop ~/.config/systemd/user/ght-dispatch.service
```

Windows, in Git Bash:

```bash
schtasks //End //TN "GitHoot dispatcher"
schtasks //Delete //TN "GitHoot dispatcher" //F
rm ~/.local/bin/ght-dispatch ~/.local/bin/ght-dispatch-loop ~/.config/ght-dispatch/ght-dispatch.xml
```

The doubled slashes are for Git Bash, which would otherwise read `/End` as a path and rewrite it.
From `cmd` or PowerShell, use single ones.
