# Integrations

[Portals](portals.md) are where pull requests come from. **Integrations are where they can go:** what
GitHoot may do with the pull requests it finds, beyond showing them. Today there is one, the
[Herdr dispatcher](dispatcher.md). The seam is there so the next one is one new directory, not edits
across the app.

## Installed means switched on

Every integration ships inside GitHoot. **Integrations** in the settings page lists them, each with
an **Install** button, or **Uninstall** once it is installed, and each has a page with:

- **Install** and **Uninstall.** They write `integration.<id>.enabled=on` or `off` to `config.txt`, the
  same one-line edit every other setting gets, and reload the page they were pressed on. Uninstalling
  keeps the integration's settings and files, so installing it again picks up where it left off.
- **Dry run.** A real pass with every effect suppressed and nothing recorded, so it cannot change
  what the next real pass does. Offered installed or not: asking first is the only safe way to learn
  what installing would do.
- **Its settings**, as a form of their own: text boxes and switches. Only keys the integration
  declared can be written, a switch only as `on` or `off`, and an unticked switch saves as `off`.
- **What it is missing.** Installed but unable to run reads *Installed, but idle*, never *Installed*.
- **Whatever it adds**, such as the dispatcher's prompts.

No restart anywhere: the runner reads `config.txt` on every pass.

Where a build cannot run an integration, it is still listed, as *Not available here*, with the
reason and no Install button. The list of what exists is the same on every platform.

### Why not plugins

An integration is code inside the binary, not a program GitHoot starts. That is deliberate. The
dispatcher was an external script until 2.4.0, and being external was the problem: Task Scheduler
gave it a console window on Windows, the ways around that are what antivirus flags, and bash and
PowerShell versions of the same rules drifted. Plugins would bring all of that back for every
integration, plus a protocol to keep compatible forever and GitHoot starting programs it did not
ship.

Scripts of your own already have a door: [the local API](local-api.md) serves the same judged lists
as JSON.

## The shape

Everything about one integration lives under `src/integration/<id>/`. The core (`config`, `page`,
`serve`, `main`) knows the list in `integration::all()` and the `Integration` trait, and nothing about
any one of them.

- **`Info`** is what the app needs without running it: the id (the config namespace, the URL segment
  and the directory name, so lowercase letters only), the name, a one-line summary, the portal kinds
  it can act on, its settings, and why this build cannot run it, if it cannot.
- **`pass`** gets the bars, already judged, and acts. It never polls a forge.
- **`prepare`** runs once at startup, installed or not: the dispatcher brings unedited prompts up to
  the new defaults there.
- **`missing`**, **`page`** and **`action`** feed its page.
- **`integration::fake`** is a test double that imports nothing from `herdr`. It compiles only if the
  trait can be implemented without a single Herdr type, which is the whole promise of the seam.

## What an integration is given

- **The bars as the icon and the pages see them**, one batch per bar. A bar GitHoot has no confirmed
  answer for is not handed over at all, because "nobody could ask" is not "nothing there".
- **Only portals it declared.** The dispatcher asks `gh` about every pull request, so it declares
  GitHub, and a GitLab pull request will never reach it.
- **No muted pull requests.** They are counted, so a dry run can say how many were skipped, and never
  handed over. Starting an agent for a pull request you asked to stop hearing about would be the
  loudest way to ignore that.
- **Its own directory**, `~/.githoot/integrations/<id>/`, and its own settings, `integration.<id>.*`.

## When it runs

One thread runs every installed integration, off the poll thread, so a slow `git fetch` never holds up
the icon. It wakes right after each poll publishes, and at least every thirty seconds, which is also
how soon an Install takes effect.

What a pass did is logged at info level; what went wrong with a pull request at error level, every
time; what is wrong with the setup, such as a missing tool, at error level once until it changes.
