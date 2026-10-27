# Starting with your session, and where the icon sits

Two things GitHoot does on the way in, both about being *there* rather than about pull requests.
Neither is a `config.txt` key, for the same reason: the operating system already stores the answer, and
a second copy in a settings file would be one more thing that can disagree with reality.

Starting with your session is also a checkbox: **Settings ▸ Start at sign-in**. It reads the OS entry
described below every time the menu opens, and writes that same entry when you click it — so it is a way
to *edit* the one record, not a second record of the answer. Everything on this page still applies.

## The question you get exactly once

On a genuinely first run, GitHoot asks:

> **Start GitHoot automatically when you sign in?**
> This adds an entry for your account only, under *…*, pointing at *…*
> You will only be asked this once. Remove the entry there at any time to undo it.

**"First run" means `~/.githoot/config.txt` did not exist and has just been written.** That file is
the app's only record of having met you, so its absence is the one moment GitHoot can be sure it has
never asked. Every later start finds the file and says nothing.

Three consequences worth knowing:

- **If you already had a `config.txt` before this feature shipped, you will never be asked.** Tick
  **Settings ▸ Start at sign-in** instead, or add the entry by hand from the table below.
- **Declining is permanent**, in the sense that nothing asks again. Nothing is written anywhere; the
  entry simply is not created — tick the box in the menu whenever you change your mind.
- **If no dialog can be shown at all** — a headless box, a systemd user service, no `zenity` or
  `kdialog` — the answer is taken as *no*. "Could not ask" must never register something that outlives
  the process. The log line says so.

Nothing is registered for other users of the machine, and nothing needs elevation.

### What gets written, and how to undo it

| Platform | What is created | Remove it with |
|---|---|---|
| 🪟 Windows | A `GitHoot` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, holding the quoted path to the `.exe` | **Task Manager › Startup apps**, or **Settings › Apps › Startup** |
| 🐧 Linux | `$XDG_CONFIG_HOME/autostart/githoot.desktop` (`~/.config/autostart/…` when that is unset) | Delete the file, or use your desktop's own startup-applications tool |
| 🍎 macOS | `~/Library/LaunchAgents/com.githoot.GitHoot.plist`, with `RunAtLoad` and no `KeepAlive` | Delete the plist, or **System Settings › General › Login Items** |

The path written is wherever the running binary is, which is also where the self-updater installs, so
an update does not invalidate the entry. A build inside `target/` is registered too if you say yes to
the prompt on a fresh `cargo run` — deliberately, since refusing would make the feature impossible to
try out by hand, but it is worth remembering before `cargo clean`.

No `KeepAlive` on macOS on purpose: it would make the app unquittable, restarting it every time you
used the tray's own **Quit**.

## Windows only: keeping the icon out of the overflow flyout

Windows 11 hides new tray icons behind the **^** arrow by default, and an owl nobody can see cannot do
the one job this app has. So right after the startup question, a first run on Windows says so and offers
to open the taskbar settings (`ms-settings:taskbar`). There you turn GitHoot on under **Settings ›
Personalisation › Taskbar › Other system tray icons**. On Windows 10 the same page has **Select which
icons appear on the taskbar**.

**GitHoot does not change this setting itself.** Microsoft provides no supported call for an app to
promote its own icon, because it is meant to be your choice. Earlier versions wrote the undocumented
`HKCU\Control Panel\NotifyIconSettings` key directly. That was dropped: quietly editing the shell's
settings is the kind of behaviour antivirus models score as suspicious, and the switch is two clicks
away.

Declining, or a dialog that cannot be shown, opens nothing, and nothing asks again.

## What the log says

Set `logLevel=info` in `config.txt` to see any of this. All of it is best effort — nothing here can
fail in a way that stops the app.

```
registered to start at sign-in: C:\Users\you\AppData\Local\Programs\GitHoot\githoot.exe
not starting at sign-in (declined, or no dialog could be shown) — nothing was registered
not opening the taskbar settings, so the owl may sit behind the ^ arrow
```
