# Tray menu and settings

## Tray menu

| Item | Shown when | Does |
|---|---|---|
| **Install update: X.Y.Z** | A newer release exists | Shows what changed, then verifies, installs and restarts |
| **GitHub is githubing again, check status** | GitHub reports an incident | Opens [githubstatus.com](https://www.githubstatus.com) |
| *— separator —* | Something above **and** below it | |
| **Authenticate GitHub PR Status** | PR status has no usable credential | Starts the sign-in flow |
| **Open GitHub Notifications** | Notifications on and something unread | Opens them, then re-checks a few seconds later |
| **Open Requested Reviews (N)** | A PR waits on your review | Opens exactly what the red bar counts, newest first |
| **Open Approved PRs (N)** | One of yours has been approved | Opens the exact PRs the bar counts, one tab each; falls back to a list of your open PRs when it has no confirmed list (see below) |
| **Open Changes Requested (N)** | A reviewer asked for changes and it is still on you | Opens the exact PRs the bar counts, one tab each; falls back to GitHub's changes-requested list when it has no confirmed list (see below) |
| **Open PR inbox** | None of the three entries above is shown | Opens [GitHub's own PR inbox](https://github.com/pulls/inbox) |
| *— separator —* | Always | |
| **Settings ▸** | Always | A submenu: two checkboxes, the settings file, and the repository (see below) |
| **Quit** | Always | Exits |

The update and GitHub-status entries come first: they are about the app and the service rather than about
your pull requests, and when the icon is wearing a mark two of its three causes are explained up there.

The upper separator appears only when there is something above it. A rule with nothing on one side is a
stray line, which reads as a rendering fault rather than a grouping — and below it there is now always
something, because **Open PR inbox** stands in whenever the three PR entries have nothing to open.

That fallback exists because every other PR entry hides itself when its count is zero: an entry opening an
empty list is a dead end. On a quiet day that left no way into your pull requests at all. Being a plain
URL rather than a search of ours, it also needs no credential, so it stays while the app is waiting to be
authorized and the three entries that do need one are hidden.

The requested-reviews URL is generated from the same query its bar counts, so that page cannot disagree
with the icon. Labels carry the exact count, unbounded.

**The other two bars apply a filter GitHub's web search cannot express, so no search page can match
them.** Approved reads each PR's reviews, because `review:approved` misses every approval in a repository
that requires none (see below). Changes requested counts a PR only while *no re-review is pending from the
reviewer who asked*, and `review:changes_requested` keeps matching one you have already handed back.
Clicking either entry therefore opens the exact pull requests its bar counts, one tab each, straight from
the poll that counted them. A search page remains the fallback whenever there is no confirmed list to
open: before the first answer, while the axis has lost track, or when the bar counts zero. For approved
that page is simply your open PRs; for changes requested it is GitHub's changes-requested list.


---

## The Settings submenu

| Item | Does | Where the answer is kept |
|---|---|---|
| ☑ **Hoot on new pull requests** | Silences the hoot, or brings it back | `sound` in `config.txt` |
| ☑ **Start at sign-in** | Registers or removes the startup entry | The OS itself — a registry value, a `.desktop` file, a Launch Agent |
| **Open settings file** | Opens `config.txt`, then offers a restart once your edits settle (see below) | — |
| **Open GitHoot on GitHub** | Opens this app's own repository: releases, issues, and these docs | — |

Both checkboxes **take effect the moment you click them**, with no restart: the hoot is a flag the poll
loop reads each cycle, and the startup entry is written straight to the OS. Everything else in
`config.txt` is still read only at startup, which is why editing the file still ends in a restart
prompt.

**Ticking Hoot plays one hoot.** The question behind that box is not "is the setting on" but "what will
I hear", and a silent tick leaves you waiting for a pull request to find out whether it works. Unticking
plays nothing; the silence is the confirmation.

**The menu closes when you click a checkbox.** Every platform draws this menu itself — a native
`TrackPopupMenu` on Windows, an `NSMenu` on macOS, and on Linux whatever your panel makes of the menu we
export over DBus — and all three dismiss the popup on any click, with no flag to say otherwise. Re-opening
it afterwards was tried and thrown away: a submenu is positioned by Windows against its parent, that
position cannot be read back, and a menu that reappears somewhere else is worse than one that closes.

So two settings means two trips into the menu. The tick you find there next time is read fresh: the hoot
from the file, the startup entry from the OS.

**Start at sign-in shows presence, not correctness.** An entry left behind by a copy of GitHoot in
another directory still starts something at sign-in, so the box is ticked. Ticking an already-ticked box
is not possible, but *unticking and ticking again* rewrites the entry to point at the copy you are
running, which is the repair for a stale path.

A failure is never silent: if the setting cannot be written, or the startup entry cannot be changed, the
tick goes back to what it was and a dialog says why.

---

## Settings

`~/.githoot-tray/config.txt` is created on first run with every setting at its default. An existing
file is **never** rewritten wholesale, so your edits are safe — which also means a later version's new keys
will not appear in it, and the table below is the complete list.

The one thing that edits an existing file is the **Hoot** checkbox, and it changes exactly one value
line: comments, blank lines, spacing and keys this version has never heard of all survive it byte for
byte. If the file has no `sound` line at all — every file written before that key existed — the key is
appended rather than the file regenerated.

> **Upgrading from `git-system-tray`?** The app was renamed, and with it the asset names, the binary
> and this directory — settings and log used to live in `~/.github-trayicon/`. Nothing is migrated
> automatically, so copy your old `config.txt` across if you want to keep it. Install the new release by
> hand as well: a pre-rename copy still *sees* the new version and offers it, then fails the integrity
> check with "the signed sums file has no entry for git-system-tray", because the asset it wants is no
> longer published.

| Key | Default | Does |
|---|---|---|
| `updateCheck` | `on` | Check for a newer release daily and at startup |
| `notificationIndication` | `off` | Tint the icon blue on unread notifications |
| `reviewRequested` | `on` | The red bar |
| `readyToMerge` | `on` | The green bar (approved PRs; the key keeps its old name so existing `config.txt` files still work) |
| `changesRequested` | `on` | The amber bar |
| `sound` | `on` | Play the hoot whenever a PR count goes up |
| `logLevel` | `error` | How much `log.txt` records: `error` logs only failures, `info` adds lifecycle detail for diagnosing |
| `statusComponents` | every component | Which parts of GitHub may raise the outage mark — see below |

Only `off`, `false`, `0` or `no` switch something off; anything else leaves the default, so a typo cannot
silently disable a feature. Two keys are not toggles: `logLevel` takes `error` or `info`, falling back to
`error`; `statusComponents` takes a comma-separated list.

**Two things are deliberately not keys here:** whether GitHoot starts when you sign in, and whether its
icon sits on the Windows taskbar or in the overflow flyout. Both are stored by the operating system
already — a registry value, an autostart entry, a Launch Agent — so a copy in this file would be a
second source of truth to keep in step with reality. That is why **Start at sign-in** is a checkbox and
not a key: it reads the OS entry every time the menu opens and writes the same one when you click, so
there is still exactly one record of the answer. The startup question is also asked once, on a first run
→ [Startup](startup.md).

## Which parts of GitHub count as an outage

GitHub's page-wide verdict is one judgement over everything it runs, and most of that is nothing to do
with pull requests. A single degraded component — Copilot, say — makes the whole page read "Partially
Degraded Service", which put a red exclamation on the tray for a service this app never touches. Cry wolf
often enough and the mark stops meaning anything.

So `statusComponents` names the parts that may raise it. A fresh `config.txt` lists every component GitHub
publishes, on one line; delete the ones you do not care about:

```
statusComponents=Git Operations, Webhooks, API Requests, Issues, Pull Requests, Actions, Packages, Pages, Copilot, Codespaces, Copilot AI Model Providers
```

- **One line, commas between.** There is no line-continuation syntax, so a wrapped list loses everything
  after the first line.
- **Names match GitHub's own, bar case and surrounding spaces.** `issues` is `Issues`; `operations` is not
  `Git Operations`, and `copilot` is not `Copilot AI Model Providers`. Partial matches are refused on
  purpose — one word would otherwise drag in a component you meant to leave out. A name matching nothing
  is named in `log.txt`, once, since its only other symptom would be a mark that never appears.
- **A component is faulty at `degraded_performance`, `partial_outage` or `major_outage`.** Scheduled
  maintenance is not, matching how the page-wide check has always treated it.
- **An empty list, or no key at all, watches the whole page** — the old behaviour, which is what every
  `config.txt` written before this key existed says. Those files are never rewritten, so they keep it.
- The tooltip and the log then name what is actually broken: `GitHub: Issues (Partial Outage)` rather
  than `GitHub: Partially Degraded Service`.

Naming components costs one extra request per five minutes' check: `components.json` rather than the
219-byte `status.json`. An unfiltered install still reads the small one.

**Restart after editing — the menu offers it.** Settings are read once at startup, so **Open settings
file** opens the file and then watches it. Once the contents change and stay unchanged for a few seconds, a dialog
offers to restart. Details worth knowing:

- It watches the **file**, not the editor. The usual handler for a text file is a DBus-activated,
  single-instance app (gedit, VS Code), so the launcher exits immediately and there is no editor process to
  wait on — a "wait until closed" would fire the moment it opened.
- Content, not timestamps, so saving an unchanged buffer does nothing, and typing an edit then undoing it
  before the prompt appears is correctly treated as no change.
- **Asked once.** Declining means the edits apply at the next start; it will not ask again.
- The watch is armed by the menu click and gives up quietly after 15 minutes, so editing the file by hand
  later never restarts the app unexpectedly.
- With no display server to show the dialog, nothing restarts and the edits apply on the next start.
- `$VISUAL`/`$EDITOR` is preferred over the desktop association, skipping terminal-only editors (vim, nano
  and friends) since a tray app has no terminal to host them.

Turning a PR signal off removes its bar and menu entry **and stops it being searched for**, so it costs
nothing against the rate limit. Turning all three off skips PR sign-in entirely — no network call, no
dialog. Because slot positions are fixed, disabling the middle signal leaves a gap rather than closing up.

**Renamed in 1.4.0, breaking:** `notifications` → `notificationIndication`, and `update_check` →
`updateCheck`. The old spellings are no longer read. The app logs a line naming the replacement, but
cannot honour the old key — which matters most for `update_check=off`, since left unedited it stops being
read and update checks come back on.
