# Tray menu and settings

## Tray menu

| Item | Shown when | Does |
|---|---|---|
| **Install update: X.Y.Z** | A newer release exists | Shows what changed, then verifies, installs and restarts |
| **GitHub is githubing again, check status** | GitHub reports an incident | Opens [githubstatus.com](https://www.githubstatus.com) |
| *— separator —* | Something above **and** below it | |
| **Authenticate GitHub PR Status** | PR status has no usable credential | Starts the sign-in flow |
| **Open Requested Reviews (N)** | A PR waits on your review | Opens GitHoot's own page for exactly what the red bar counts |
| **Open Approved PRs (N)** | One of yours has been approved | Opens GitHoot's own page for exactly what the green bar counts |
| **Open Work Required (N)** | An objection stands, a conflict is blocking one, or Copilot has open comments | Opens GitHoot's own page for exactly what the amber bar counts |
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

**All three now open one tab: a page GitHoot renders and serves itself** → [The PR page](pr-page.md).
Approved and changes requested used to open one browser tab *per pull request*, because no GitHub search
URL can express what either bar counts — `review:approved` misses every approval in a repository that
requires none, and `review:changes_requested` keeps matching a PR you have already handed back (see
below). The page shows the exact pull requests the bar counted, with the check state and the reviewer
verdicts GitHub's list cannot show.

The page says so plainly when a bar has no confirmed list — before the first answer, or after the axis
has lost track — rather than rendering a zero, and links to GitHub's own search from there. If the local
listener cannot start at all, the entries fall back to that search page directly.


---

## The Settings submenu

| Item | Does | Where the answer is kept |
|---|---|---|
| ☑ **Hoot on new pull requests** | Silences the hoot, or brings it back | `sound` in `config.txt` |
| ☑ **Count Copilot comments as work** | Whether Copilot's unresolved comments light the amber bar | `copilotReviews` in `config.txt` |
| ☑ **Start at sign-in** | Registers or removes the startup entry | The OS itself — a registry value, a `.desktop` file, a Launch Agent |
| **Open settings page** | Opens every setting as a form in your browser, served locally — see [the PR page](pr-page.md) | — |
| **Open settings file** | Opens `config.txt`, then offers a restart once your edits settle (see below) | — |
| **Open GitHoot on GitHub** | Opens this app's own repository: releases, issues, and these docs | — |

All three checkboxes **take effect the moment you click them**, with no restart: the hoot and the
Copilot rule are flags the poll loop reads each cycle, and the startup entry is written straight to the
OS. Everything else in `config.txt` is still read only at startup, which is why editing the file still
ends in a restart prompt.

**Unticking Copilot re-polls at once.** Unlike the hoot, that box changes what is *counted*, so the
amber bar and its page would otherwise sit on a number the rule behind them no longer produces — which
reads as the click not having worked. Switching it off also stops both affected queries asking GitHub
for review threads at all, so it is a saving as well as a preference.

**Ticking Hoot plays one hoot.** The question behind that box is not "is the setting on" but "what will
I hear", and a silent tick leaves you waiting for a pull request to find out whether it works. Unticking
plays nothing; the silence is the confirmation.

**The menu closes when you click a checkbox.** Every platform draws this menu itself — a native
`TrackPopupMenu` on Windows, an `NSMenu` on macOS, and on Linux whatever your panel makes of the menu we
export over DBus — and all three dismiss the popup on any click, with no flag to say otherwise. Re-opening
it afterwards was tried and thrown away: a submenu is positioned by Windows against its parent, that
position cannot be read back, and a menu that reappears somewhere else is worse than one that closes.

So three settings means three trips into the menu. The tick you find there next time is read fresh: the hoot
from the file, the startup entry from the OS.

**Start at sign-in shows presence, not correctness.** An entry left behind by a copy of GitHoot in
another directory still starts something at sign-in, so the box is ticked. Ticking an already-ticked box
is not possible, but *unticking and ticking again* rewrites the entry to point at the copy you are
running, which is the repair for a stale path.

A failure is never silent: if the setting cannot be written, or the startup entry cannot be changed, the
tick goes back to what it was and a dialog says why.

---

## The settings page

**Open settings page** serves every setting as a plain HTML form on the same loopback listener the PR
pages use, behind the same token and `Host` checks. It is the easier way in for two reasons: the
component list becomes tick boxes instead of an exact comma-separated line, and nothing depends on
having an editor or a desktop association — the file route needs `$EDITOR` or a file handler, and does
nothing at all with no display server.

Saving writes **one line per changed setting**, through the same surgical edit the tray checkboxes
make: your comments, blank lines, spacing and any keys this version has never heard of survive byte for
byte. A form you did not touch writes nothing. The page then names which of the settings you changed
need a restart — the hoot and the Copilot rule take effect at once, everything else on the next start.

If the local listener cannot start, the entry falls back to opening `config.txt` in an editor, since
losing the only way into the configuration because a socket would not bind is the worse failure.

**Writing needs more than reading did**, so the page carries one guard the PR pages do not: a save is a
`POST` and is refused unless the browser says the request came from this page itself. A form on another
site can make your browser post here, and the `Host` header cannot see that — it is filled in with
*our* host either way. `Origin` is the header that names who asked, and a cross-site form always sends
one, so a missing or foreign `Origin` is refused. The form also posts key *names* only; the values
written are produced from a typed configuration, so a hand-crafted post cannot write an arbitrary line
into the file, and a component name GitHub does not publish is dropped rather than saved.

---

## Settings

`~/.githoot-tray/config.txt` is created on first run with every setting at its default. An existing
file is **never** rewritten wholesale, so your edits are safe — which also means a later version's new keys
will not appear in it, and the table below is the complete list.

The only things that edit an existing file are the **Hoot** and **Copilot** checkboxes, and each
changes exactly one value line: comments, blank lines, spacing and keys this version has never heard of
all survive it byte for byte. If the file has no such line at all — every file written before that key
existed — the key is appended rather than the file regenerated.

> **Upgrading from `git-system-tray`?** The app was renamed, and with it the asset names, the binary
> and this directory — settings and log used to live in `~/.github-trayicon/`. Nothing is migrated
> automatically, so copy your old `config.txt` across if you want to keep it. Install the new release by
> hand as well: a pre-rename copy still *sees* the new version and offers it, then fails the integrity
> check with "the signed sums file has no entry for git-system-tray", because the asset it wants is no
> longer published.

| Key | Default | Does |
|---|---|---|
| `updateCheck` | `on` | Check for a newer release daily and at startup |
| `reviewRequested` | `on` | The red bar |
| `readyToMerge` | `on` | The green bar (approved PRs; the key keeps its old name so existing `config.txt` files still work) |
| `changesRequested` | `on` | The amber bar (work required; the key keeps its old name so existing `config.txt` files still work) |
| `copilotReviews` | `on` | Count unresolved comments from GitHub's automatic reviewer as work — see [PR status](pr-status.md) |
| `sound` | `on` | Play the hoot whenever a PR count goes up |
| `logLevel` | `error` | How much `log.txt` records: `error` logs only failures, `info` adds lifecycle detail for diagnosing |
| `statusComponents` | the parts a PR tray uses | Which parts of GitHub may raise the outage mark — see below |

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

So `statusComponents` names the parts that may raise it. A fresh `config.txt` names **the parts this app
actually uses**, on one line; delete the ones you do not care about:

```
statusComponents=Git Operations, Webhooks, API Requests, Issues, Pull Requests, Actions, Packages, Pages
```

The three GitHub publishes that are left out — `Copilot`, `Codespaces` and `Copilot AI Model Providers` —
are named in a comment beside the key, so adding one back is an edit rather than a trip to the status
page. Nothing this app calls goes near them.

**This changes nothing for an existing install.** An existing `config.txt` is never rewritten, so a file
with no `statusComponents` line keeps watching the whole page, which is what an absent key has always
meant. Only a fresh file gets the narrower watch.

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

**Renamed in 1.4.0, breaking:** `update_check` → `updateCheck`. The old spelling is no longer read. The
app logs a line naming the replacement, but cannot honour the old key — which matters most for
`update_check=off`, since left unedited it stops being read and update checks come back on.

**Removed in 2.0.0:** the blue "unread notifications" tint, along with `notificationIndication`, the
separate OAuth credential it needed and the **Open GitHub Notifications** entry. An old
`notificationIndication` line in your `config.txt` is now simply an unknown key, which has always been
ignored. `~/.githoot-tray/access_token.txt` and `client_id.txt` are no longer read by anything.
