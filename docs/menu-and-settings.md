# Tray menu and settings

## Tray menu

| Item | Shown when | Does |
|---|---|---|
| **Install update: X.Y.Z** | A newer release exists | Shows what changed, then verifies, installs and restarts |
| **GitHub is githubing again, check status** | GitHub reports an incident | Opens [githubstatus.com](https://www.githubstatus.com) |
| *— separator —* | Something above **and** below it | |
| **Authenticate GitHub PR Status** | PR status has no usable credential | Opens **Portals ▸ GitHub**, where the sign-in button is |
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

That fallback exists because every other PR entry hides itself when its count is zero — and before the
first answer has arrived, which is the same dead end a second earlier: an entry opening an empty list, or
a list nobody has fetched yet, goes nowhere. They appear only on a confirmed count above zero. On a quiet day that left no way into your pull requests at all. Being a plain
URL rather than a search of ours, it also needs no credential, so it stays while the app is waiting to be
authorized and the three entries that do need one are hidden.

**All three now open one tab: a page GitHoot renders and serves itself** → [The PR page](pr-page.md).
Approved and changes requested used to open one browser tab *per pull request*, because no GitHub search
URL can express what either bar counts — `review:approved` misses every approval in a repository that
requires none, and `review:changes_requested` keeps matching a PR you have already handed back (see
below). The page shows the exact pull requests the bar counted, with the check state and the reviewer
verdicts GitHub's list cannot show.

The page says so plainly when a bar has no confirmed list — before the first answer, or after the axis
has lost track — rather than rendering a zero, and offers GitHub's own pull-request inbox from there.
If the local listener cannot start at all, the entries open that inbox directly.


---

## The Settings submenu

| Item | Does | Where the answer is kept |
|---|---|---|
| ☑ **Hoot on new pull requests** | Silences the hoot, or brings it back | `sound` in `config.txt` |
| ☑ **Start at sign-in** | Registers or removes the startup entry | The OS itself — a registry value, a `.desktop` file, a Launch Agent |
| **Open settings page** | Opens every setting as a form in your browser, served locally — see [the PR page](pr-page.md) | — |
| **Open GitHoot on GitHub** | Opens this app's own repository: releases, issues, and these docs | — |

Both checkboxes **take effect the moment you click them**, with no restart: the hoot is a flag the poll
loop reads each cycle, and the startup entry is written straight to the OS. Everything else is on the
settings page, including the green bar's rules (**General**), which also apply at once and re-poll,
since they change what is counted.

The menu no longer opens `config.txt` itself. The page covers every setting and checks what it writes;
the file is still yours to edit by hand, and **Open settings page** falls back to opening it, with a
restart offered once your edits settle, when the page cannot be served.

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

## The settings pages

**Open settings page** opens a small site on the same loopback listener the PR pages use, behind the
same token and `Host` checks. It is organised by what you manage, with a sidebar on every page:

| Page | Holds |
|---|---|
| **General** | what the tray shows and does: the three bars, the hoot, and what makes a pull request green |
| **Portals ▸ GitHub** | where pull requests come from: the sign-in, which parts of GitHub count as an outage. The Portals list is also where GitLab is installed |
| **Integrations ▸ Herdr dispatcher** | where pull requests go: Install, a dry run, its settings and prompts → [integrations](integrations.md) |
| **Muted** | every muted pull request, with Unmute → [muting](pr-page.md#muting-a-pull-request) |
| **Updates** | this version, the last check, **Check now**, **Update to X.Y.Z** when there is one, and the automatic check |
| **Advanced** | the local API → [the local API](local-api.md), and how much the log records |

Portals and Integrations are the two collections: a list with one card per item, its state, its main
action and a ⚙ **Settings** button, and a page per item. The sidebar lists under each collection only
what is in use, the portals you are signed in to and the integrations you installed, plus the page you
are on, and marks where you are. Everything else is on the list page, where it is signed in to or
installed. The page title says where you are too, as a trail from **Settings**, such as *Settings ›
Integrations › Herdr dispatcher*, with every step but the last a link back.

**Checkboxes and choices save the moment you change them.** No Save button, like the tray's own
checkboxes: a line beside the controls says *Saved.* and fades, or names the settings that take effect
only after a restart and stays. Sections with a text box (the Herdr folders, its prompts) keep a
**Save** button, because half-typed text should not save itself. Either way a save writes that
section's settings and no others, **one line per changed setting**, through the same surgical edit the
tray checkboxes make: your comments, blank lines, spacing and any keys this version has never heard of
survive byte for byte. Only the hoot and the green bar's rules take effect at once among the core settings;
an integration's settings always do. Without JavaScript every section shows its Save button and posts
as before.

**What makes a pull request green** is drawn on General as the path a pull request walks to the green
bar: two fixed steps (somebody approved it, nobody's objection stands), then one switch per rule, each
with what happens to a pull request that fails it, *else amber* or *else waits*. A rule you switch off
fades and is ignored on **both** bars, so nothing is ever green and amber at once. The switches are
independent; their order is only who decides: people, then git, then CI, then the automatic reviewer.

**Parts of GitHub that count as an outage** is a choice between *The whole status page* and *Only these
parts*; the parts appear only under the second. It is the same `statusComponents` key: the whole page
is the empty list.

**A page that reloads itself shows only the thing that is running.** A sign-in in flight and an update
check under way reload their page until they land, and while they do, the page shows that and nothing
else, so no half-made edit is ever on screen to lose.

If the local listener cannot start, the entry falls back to opening `config.txt` in an editor, since
losing the only way into the configuration because a socket would not bind is the worse failure.

**Updating by hand:** **Updates ▸ Check now** asks at once whether a newer release exists, whether or not
the automatic check is on. The answer shows on the page within a second or two, a newer version is also
named in the sidebar and gets the arrow on the tray icon, and **Update to X.Y.Z** on the page installs
it just as the tray's **Install update** does.

## Signing in

**Portals ▸ GitHub** is where a sign-in happens. Its first section says how the sign-in stands: *Signed
in*, *Not signed in*, the running sign-in itself, or the reason nothing can be seen.

The **Portals** list shows every kind of portal. *Not signed in* offers **Sign in** and nothing else;
*Signed in* offers **⚙ Settings** and **Sign out**. There are no Install or Uninstall buttons: Sign in on a
portal that is not running installs it, and Sign out deletes the saved credential (`pr_token.txt` for
GitHub) and removes the portal, unless it is the only one, which stays listed and signed out, where a
fresh install starts: bars dark, exclamation up, **Authenticate** back on the menu. A portal's settings
appear only once it is signed in. It does not revoke the authorization on GitHub's side; that is done at
github.com/settings/applications.

Signing in happens in the card you pressed, on the list or on the portal's own page. **Sign in to
GitLab** turns that card into the sign-in: "Waiting for you to authorize", the code in large type, one
**Copy code and open GitLab** button, how long the code lasts ("Expires in 4:59", counting down) and
**Cancel**. The code arrives in about half a second, because the sign-in runs on its own thread instead
of waiting for the poll loop, and it lands in a box that was already there, so nothing moves. The button
copies the code and opens the portal's page in a new tab in one click; GitHoot has also put the code on
your clipboard. Nothing reloads while you are away: the card asks in the background, and changes once,
to *Signed in* when the portal confirms, or back to **Sign in** with one line saying what happened if
the code expired or the sign-in was denied. On GitLab, the page you are left on after Authorize is
GitLab's blank code form; that is GitLab confirming, not asking again, and the card says so beforehand.
Without JavaScript the buttons still post and the page reloads until the sign-in lands. The
tray's **Authenticate** entry only opens this page; nothing starts a sign-in but the button. When the
local listener cannot start at all, the entry falls back to running the flow with the old native
dialog, since there is no page to show the code on.

Cancel is polled, not pushed: the poll thread is inside the flow and reads no channel there, so it
looks for the cancel between its polls, about once a second. Every button posts to the portal's own
page, guarded by the same `Origin` check as a save → [Portals](portals.md).

**Writing needs more than reading did**, so the page carries one guard the PR pages do not: a save is a
`POST` and is refused unless the browser says the request came from this page itself. A form on another
site can make your browser post here, and the `Host` header cannot see that — it is filled in with
*our* host either way. `Origin` is the header that names who asked, and a cross-site form always sends
one, so a missing or foreign `Origin` is refused. The form also posts key *names* only; the values
written are produced from a typed configuration, so a hand-crafted post cannot write an arbitrary line
into the file, and a component name GitHub does not publish is dropped rather than saved.

---

## Settings

`~/.githoot/config.txt` is created on first run with every setting at its default. An existing
file is **never** rewritten wholesale, so your edits are safe — which also means a later version's new keys
will not appear in it, and the table below is the complete list.

The only things that edit an existing file are the **Hoot** and **Copilot** checkboxes, and each
changes exactly one value line: comments, blank lines, spacing and keys this version has never heard of
all survive it byte for byte. If the file has no such line at all — every file written before that key
existed — the key is appended rather than the file regenerated.

> **Upgrading from `githoot-tray`?** The app is now plain `githoot`: binary, release assets,
> repository, this directory and the startup entry. Nothing is migrated, on purpose:
>
> - **Install the new release by hand.** A `githoot-tray` copy still sees the new version and offers it,
>   then fails with "the signed sums file has no entry for githoot-tray", because that asset is no
>   longer published.
> - **Rename `~/.githoot-tray/` to `~/.githoot/` before the first start.** That keeps your settings,
>   mutes, sign-in and dispatcher prompts. Started without it, the new version treats it as a first
>   run: defaults, a fresh sign-in, and the startup question again.
> - **Remove the old startup entry**, or the old binary starts next to the new one: the `GitHootTray`
>   value in Windows startup apps, `~/.config/autostart/githoot-tray.desktop`, or
>   `~/Library/LaunchAgents/com.githoot.GitHootTray.plist`.
>
> From the even older `git-system-tray`, the same applies, with settings in `~/.github-trayicon/`.

| Key | Default | Does |
|---|---|---|
| `updateCheck` | `on` | Check for a newer release daily and at startup |
| `reviewRequested` | `on` | The red bar |
| `readyToMerge` | `on` | The green bar (approved PRs; the key keeps its old name so existing `config.txt` files still work) |
| `changesRequested` | `on` | The amber bar (work required; the key keeps its old name so existing `config.txt` files still work) |
| `copilotReviews` | `on` | Count unresolved comments from GitHub's automatic reviewer as work, and keep the PR off green; see [PR status](pr-status.md) |
| `ruleConflicts` | `on` | A merge conflict while somebody is reviewing: amber, not green. Off ignores conflicts on both bars |
| `ruleFailedChecks` | `on` | Failed checks on an approved pull request: amber, not green. Off ignores them on both bars |
| `ruleRunningChecks` | `on` | Checks still running keep an approved pull request off green until they finish. Off shows it at once |
| `sound` | `on` | Play the hoot whenever a PR count goes up |
| `logLevel` | `error` | How much `log.txt` records: `error` logs only failures, `info` adds lifecycle detail for diagnosing |
| `statusComponents` | the parts a PR tray uses | Which parts of GitHub may raise the outage mark — see below |
| `localApi` | `off` | Serve the judged lists as JSON to local scripts, and bind the local port at startup — see [The local API](local-api.md). Off by default, and a typo leaves it shut rather than open |
| `integration.<id>.enabled` | `off` | Whether that integration is installed. What its Install and Uninstall buttons write, and like `localApi` a typo leaves it off → [integrations](integrations.md) |
| `integration.<id>.<key>` | per integration | That integration's own settings, listed on its page and in its doc. The Herdr dispatcher has `cloneRoot` and `worktreeRoot` → [the dispatcher](dispatcher.md#settings) |

| `portal.<name>.type` | none | `github` or `gitlab`. **Naming any portal retires the implicit GitHub one**, so to watch both, name both → [Portals](portals.md#configuration) |
| `portal.<name>.url` | the forge's own | A self-managed GitLab's origin. Ignored on a GitHub section until Enterprise Server ships |
| `portal.<name>.clientId` | shipped, for gitlab.com | The OAuth application a GitLab portal signs in with. Only a self-managed instance needs one; without it that portal shows off, with a reason |
| `portal.<name>.enabled` | `on` | `off` leaves that portal out without deleting its section. What Sign in and Sign out write |

**Sign in is install, Sign out is uninstall.** Sign in on a portal that is not running writes its `type`
and `enabled` (naming GitHub first if the file names no portal yet), builds the portal and starts the
sign-in. Sign out deletes the credential, writes `enabled=off` and stops the portal, keeping the rest of
its section. The one exception is the last portal, which Sign out only signs out of. `url` and
`clientId` are edited in this file, and those edits take effect after a restart.

Only `off`, `false`, `0` or `no` switch something off; anything else leaves the default, so a typo cannot
silently disable a feature. The exceptions are `localApi` and `integration.<id>.enabled`, which need an
explicit `on`. Some keys are not toggles: `logLevel` takes `error` or `info`, falling back to
`error`; `statusComponents` takes a comma-separated list; an integration's own keys take text, and empty
means that setting's default.

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
ignored. `~/.githoot/access_token.txt` and `client_id.txt` are no longer read by anything.
