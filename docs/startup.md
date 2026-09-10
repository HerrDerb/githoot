# Starting with your session, and where the icon sits

Two things GitHoot Tray does on the way in, both about being *there* rather than about pull requests.
Neither is a `config.txt` key, for the same reason: the operating system already stores the answer, and
a second copy in a settings file would be one more thing that can disagree with reality.

## The question you get exactly once

On a genuinely first run, GitHoot asks:

> **Start GitHoot automatically when you sign in?**
> This adds an entry for your account only, under *…*, pointing at *…*
> You will only be asked this once. Remove the entry there at any time to undo it.

**"First run" means `~/.githoot-tray/config.txt` did not exist and has just been written.** That file is
the app's only record of having met you, so its absence is the one moment GitHoot can be sure it has
never asked. Every later start finds the file and says nothing.

Three consequences worth knowing:

- **If you already had a `config.txt` before this feature shipped, you will never be asked.** Add the
  entry by hand from the table below if you want it, or delete `config.txt` to be asked on the next
  start — the file is rewritten with every setting at its default, so you lose your edits.
- **Declining is permanent**, in the sense that nothing asks again. Nothing is written anywhere; the
  entry simply is not created.
- **If no dialog can be shown at all** — a headless box, a systemd user service, no `zenity` or
  `kdialog` — the answer is taken as *no*. "Could not ask" must never register something that outlives
  the process. The log line says so.

Nothing is registered for other users of the machine, and nothing needs elevation.

### What gets written, and how to undo it

| Platform | What is created | Remove it with |
|---|---|---|
| 🪟 Windows | A `GitHootTray` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, holding the quoted path to the `.exe` | **Task Manager › Startup apps**, or **Settings › Apps › Startup** |
| 🐧 Linux | `$XDG_CONFIG_HOME/autostart/githoot-tray.desktop` (`~/.config/autostart/…` when that is unset) | Delete the file, or use your desktop's own startup-applications tool |
| 🍎 macOS | `~/Library/LaunchAgents/com.githoot.GitHootTray.plist`, with `RunAtLoad` and no `KeepAlive` | Delete the plist, or **System Settings › General › Login Items** |

The path written is wherever the running binary is, which is also where the self-updater installs, so
an update does not invalidate the entry. A build inside `target/` is registered too if you say yes to
the prompt on a fresh `cargo run` — deliberately, since refusing would make the feature impossible to
try out by hand, but it is worth remembering before `cargo clean`.

No `KeepAlive` on macOS on purpose: it would make the app unquittable, restarting it every time you
used the tray's own **Quit**.

## Windows only: keeping the icon out of the overflow flyout

Windows 11 hides new tray icons in the **^** flyout by default. An owl nobody can see cannot do the one
job this app has, so on Windows GitHoot asks the shell to keep its icon on the taskbar.

This is **not** part of the prompt above and is never asked about. It is cosmetic, it is undone in two
clicks, and a second dialog on a first run would cost more attention than the decision is worth.

**How it works.** Windows 11 records per-icon visibility under
`HKCU\Control Panel\NotifyIconSettings\<hash>`. Each subkey holds an `ExecutablePath` naming the
program, and — once anything has decided — an `IsPromoted` DWORD: `1` on the taskbar, `0` in the
flyout. GitHoot finds the subkey whose `ExecutablePath` is its own and sets `IsPromoted=1`.

**It only ever fills in a blank.** If `IsPromoted` already exists, whatever its value, it is left
alone. So hiding the icon yourself sticks (that writes `0`), and this can never fight a choice you have
made. To change it: **Settings › Personalisation › Taskbar › Other system tray icons**.

**Three honest limitations:**

- **It is undocumented.** Microsoft provides no Win32 call for an app to promote its own icon — it is
  meant to be your choice. This key was verified by hand on Windows 11 build 26200, and a future
  Windows could move or rename it. If it does, the log says the key could not be opened and nothing
  else changes.
- **It may not apply until your next sign-in.** The shell reads the setting when it builds the tray, so
  the icon can stay in the flyout for the rest of the current session.
- **Windows 10 and older are not supported for this.** There the same setting lives in
  `TrayNotify\IconStreams`, an undocumented obfuscated blob that cannot be edited without killing
  `explorer.exe`. That is not worth doing to anyone's machine, so on Windows 10 the icon stays wherever
  Windows puts it and the log says the key is absent.

Because it fills in blanks rather than tracking a first run, this runs on every start, not just the
first. That makes it self-healing: the subkey is created by the shell a moment *after* the icon appears,
so a first run that got there too early simply succeeds on the next one.

## What the log says

Set `logLevel=info` in `config.txt` to see any of this. All of it is best effort — nothing here can
fail in a way that stops the app.

```
registered to start at sign-in: C:\Users\you\AppData\Local\Programs\GitHoot Tray\githoot-tray.exe
not starting at sign-in (declined, or no dialog could be shown) — nothing was registered
tray icon set to always show (1 matching entry under Control Panel\NotifyIconSettings); Windows may only apply it from the next sign-in
leaving the tray icon's visibility alone — it has already been decided
not promoting the tray icon: could not open HKCU\Control Panel\NotifyIconSettings (2)
```
