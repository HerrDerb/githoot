//! Offers to start the app when the user signs in, once, on the very first run.
//!
//! **Why only once, and only on a first run.** The question is asked when `config.txt` did not exist
//! and has just been written (see `config::FirstRun`), which is the one moment this app can be sure it
//! has never spoken to this user before. Every later start finds the file and stays quiet. That is
//! deliberate: a tray app that re-asks to be added to startup every launch is the kind of nagging that
//! gets an app uninstalled, and there is nowhere honest to record "asked and declined" that is not just
//! this same file.
//!
//! **Why the OS entry is the only record.** No `autostart=` setting is written anywhere. The registry
//! value, the desktop entry and the Launch Agent *are* the state, so there is nothing to drift out of
//! step with them — no case where the config says on and the machine says off. Changing your mind later
//! means removing the entry with the tool your OS already gives you for it, which is documented in the
//! README; the alternative (a setting the app reconciles on every start) would fight anyone who removed
//! the entry by hand.
//!
//! **Why three implementations.** There is no cross-platform autostart. Windows has an `HKCU` registry
//! value, freedesktop has `~/.config/autostart/*.desktop`, and macOS has a per-user Launch Agent. They
//! agree on nothing but the intent, so each arm is written out rather than abstracted, and the pure
//! parts (what the command line, entry or plist *says*) are split from the I/O so they can be tested
//! without touching the machine running the tests.

use crate::config::FirstRun;
use crate::{errorln, infoln};
use std::path::Path;

/// The name the entry carries wherever it is registered.
///
/// One constant for all three platforms: it is the registry value name on Windows, the desktop entry's
/// `Name` on Linux and the plist `Label`'s tail on macOS. A user looking at any of the three sees the
/// same word, which is what makes the entry findable when they want it gone.
const ENTRY_NAME: &str = "GitHoot";

/// Where the entry ends up, named in the prompt so removing it later is not a research task.
#[cfg(target_os = "windows")]
const ENTRY_LOCATION: &str = "Startup apps in Windows Settings";
#[cfg(target_os = "macos")]
const ENTRY_LOCATION: &str = "~/Library/LaunchAgents";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const ENTRY_LOCATION: &str = "~/.config/autostart";

/// The Settings page where Windows lets the user keep a tray icon on the taskbar.
#[cfg(any(target_os = "windows", test))]
const TASKBAR_SETTINGS: &str = "ms-settings:taskbar";

/// What a first run on Windows says about the overflow flyout.
///
/// Windows 11 hides new tray icons behind the **^** arrow, and an owl nobody can see cannot do its one
/// job. There is no supported call for an app to promote its own icon, so instead of writing the
/// shell's `NotifyIconSettings` behind the user's back, this points them at the switch and lets them
/// flip it.
#[cfg(any(target_os = "windows", test))]
fn tray_hint() -> String {
    format!(
        "Windows may hide the {ENTRY_NAME} owl behind the ^ arrow next to the clock.\n\n\
         To keep it on the taskbar, turn {ENTRY_NAME} on under Settings › Personalisation › Taskbar › \
         Other system tray icons.\n\n\
         Open those settings now?"
    )
}

/// Asks, on a first run only, whether to start with the user's session — and registers it if so.
///
/// Returns immediately: the prompt runs on its own thread, the same way `dialog::show_device_code_prompt`
/// does and for the same reason. Blocking here would hold the tray icon off the bar until the question
/// was answered, and an app whose icon appears only after you have dealt with a dialog looks broken. The
/// answer is needed by nothing else in startup, so there is nothing to wait for.
pub fn offer_on_first_run(first_run: FirstRun) {
    if first_run == FirstRun::No {
        return;
    }

    // Resolved on this thread, before the spawn, so a failure is reported in startup order rather than
    // arriving in the log some seconds later next to a dialog that should never have been shown.
    //
    // Deliberately `current_exe` rather than `update::resolve_current_exe`: that one refuses anything
    // inside `target/`, to stop the updater clobbering a developer's build. Nothing is being written
    // over here, and refusing would make the whole feature impossible to try out by hand — the same
    // reasoning `update::restart_target` sets out.
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            errorln!("not offering to start at sign-in: could not locate this executable ({e})");
            return;
        }
    };

    std::thread::spawn(move || {
        let body = format!(
            "Start GitHoot automatically when you sign in?\n\n\
             This adds an entry for your account only, under {ENTRY_LOCATION}, pointing at:\n\
             {}\n\n\
             You will only be asked this once. Remove the entry there at any time to undo it.",
            exe.display()
        );

        if !crate::dialog::confirm_autostart("githoot: start at sign-in", &body) {
            infoln!("not starting at sign-in (declined, or no dialog could be shown) — nothing was registered");
        } else {
            match enable(&exe) {
                Ok(()) => infoln!("registered to start at sign-in: {}", exe.display()),
                // Not fatal and not worth a second dialog: the app is running, and the only thing lost
                // is a convenience the user can set up with their OS's own startup tool.
                Err(e) => errorln!("could not register to start at sign-in ({e}) — add it by hand under {ENTRY_LOCATION}"),
            }
        }

        // After the startup question rather than before, so the two dialogs never stack. macOS has no
        // overflow flyout and Linux trays differ too much to name one switch.
        #[cfg(target_os = "windows")]
        offer_taskbar_settings();
    });
}

/// Points the user at the taskbar settings, once, on a first run.
#[cfg(target_os = "windows")]
fn offer_taskbar_settings() {
    if !crate::dialog::confirm_taskbar_settings("githoot: keep the owl visible", &tray_hint()) {
        infoln!("not opening the taskbar settings, so the owl may sit behind the ^ arrow");
        return;
    }
    if let Err(e) = open::that(TASKBAR_SETTINGS) {
        errorln!("could not open {TASKBAR_SETTINGS} ({e})");
    }
}

/// Registers or unregisters the startup entry, for the tray's Start-at-sign-in checkbox.
///
/// The one place the "which executable" decision lives, so the menu never has to make it. Same
/// `current_exe` as [`offer_on_first_run`], and for the same reason: `update::resolve_current_exe`
/// refuses anything under `target/`, which would make the checkbox impossible to try out from a
/// development build.
///
/// Synchronous, unlike `offer_on_first_run`: this is a registry write or a file delete with no dialog
/// in front of it, and the caller is a menu click that wants to know whether to leave the tick where
/// the user just put it.
pub fn set_enabled(on: bool) -> Result<(), String> {
    if !on {
        return disable();
    }
    let exe =
        std::env::current_exe().map_err(|e| format!("could not locate this executable ({e})"))?;
    enable(&exe)
}

// ─── Windows: the little bit of registry this needs ──────────────────────────
//
// Hand-rolled on `winapi`, which is already a dependency, rather than adding `windows-registry` for
// four operations. Wrapped in an RAII `Key` because the alternative is a `RegCloseKey` on every early
// return, and the leaks that shape of code grows are invisible.

/// A NUL-terminated UTF-16 string, which is what every `…W` entry point wants. The same shape
/// `dialog.rs` builds for `MessageBoxW`.
#[cfg(target_os = "windows")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// An open `HKEY` under `HKEY_CURRENT_USER` that closes itself.
#[cfg(target_os = "windows")]
struct Key(winapi::shared::minwindef::HKEY);

#[cfg(target_os = "windows")]
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful `RegOpenKeyExW` and is closed exactly once, since
        // `Key` is neither `Copy` nor `Clone`.
        unsafe { winapi::um::winreg::RegCloseKey(self.0) };
    }
}

#[cfg(target_os = "windows")]
impl Key {
    /// Opens an **existing** key under `HKCU`, failing if it is not there.
    ///
    /// Used to read or clear the `Run` key, where absence is the answer rather than an obstacle.
    /// Writing goes through [`Key::create`], since a fresh profile has no `Run` key at all.
    fn open(path: &str, access: u32) -> Result<Self, String> {
        use std::ptr::null_mut;
        use winapi::um::winreg::{HKEY_CURRENT_USER, RegOpenKeyExW};

        let path_w = wide(path);
        let mut key = null_mut();
        // SAFETY: `path_w` is NUL-terminated and outlives the call; `key` is a valid out-pointer.
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path_w.as_ptr(), 0, access, &mut key) };
        if status == 0 {
            Ok(Self(key))
        } else {
            Err(format!("could not open HKCU\\{path} ({status})"))
        }
    }

    /// Opens a key under `HKCU`, creating it if it is not there.
    ///
    /// **`HKCU\…\CurrentVersion\Run` is not guaranteed to exist.** That assumption is what an earlier
    /// version of this file got wrong, and CI proved it: on a GitHub `windows-latest` runner the key is
    /// absent and every `RegOpenKeyExW` against it returned `2` (`ERROR_FILE_NOT_FOUND`). The subkey is
    /// created by whatever first adds a startup entry, so a profile that has never had one has never
    /// had the key — which is precisely the fresh install this whole feature exists for.
    ///
    /// Creating it is what Windows itself does when you add a startup entry, and `Run` is a
    /// Microsoft-defined location, so this is not the app inventing a key somewhere it does not
    /// belong. `REG_OPTION_NON_VOLATILE` so it survives a reboot; an existing key is opened untouched.
    fn create(path: &str, access: u32) -> Result<Self, String> {
        use std::ptr::null_mut;
        use winapi::um::winnt::REG_OPTION_NON_VOLATILE;
        use winapi::um::winreg::{HKEY_CURRENT_USER, RegCreateKeyExW};

        let path_w = wide(path);
        let mut key = null_mut();
        // SAFETY: `path_w` is NUL-terminated and outlives the call; `key` is a valid out-pointer, and
        // the class, security-attributes and disposition out-parameters are all optional.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path_w.as_ptr(),
                0,
                null_mut(),
                REG_OPTION_NON_VOLATILE,
                access,
                null_mut(),
                &mut key,
                null_mut(),
            )
        };
        if status == 0 {
            Ok(Self(key))
        } else {
            Err(format!("could not open or create HKCU\\{path} ({status})"))
        }
    }

    /// A `REG_SZ` value, or `None` if it is absent or of any other type.
    ///
    /// Absent is `None` rather than an error because absent is the ordinary state — it is what a
    /// machine that has never been asked looks like.
    fn string(&self, name: &str) -> Option<String> {
        use std::ptr::null_mut;
        use winapi::shared::minwindef::DWORD;
        use winapi::um::winnt::REG_SZ;
        use winapi::um::winreg::RegQueryValueExW;

        let name_w = wide(name);
        // Two passes, the documented way to read a value of unknown length: ask for the size and the
        // type with no buffer, then read into one that fits.
        let mut kind: DWORD = 0;
        let mut bytes: DWORD = 0;
        // SAFETY: `name_w` is NUL-terminated; a null data pointer with a valid size out-pointer is
        // exactly the size-query form of this call.
        let status = unsafe {
            RegQueryValueExW(self.0, name_w.as_ptr(), null_mut(), &mut kind, null_mut(), &mut bytes)
        };
        // Anything but a string is not ours, whatever wrote it.
        if status != 0 || kind != REG_SZ {
            return None;
        }

        // `max(1)`: a zero-length value would otherwise hand the call a dangling pointer.
        let mut buffer: Vec<u16> = vec![0; (bytes as usize).div_ceil(2).max(1)];
        let mut size = bytes;
        // SAFETY: `buffer` holds at least `size` bytes, the size the call above reported.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name_w.as_ptr(),
                null_mut(),
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }

        // Cut at the terminator rather than trusting the returned size: whether the stored length
        // counts the NUL depends on what wrote the value, and a trailing NUL inside a Rust `String`
        // would make an otherwise-equal comparison fail.
        let chars: Vec<u16> =
            buffer.into_iter().take(size as usize / 2).take_while(|&c| c != 0).collect();
        Some(String::from_utf16_lossy(&chars))
    }

    fn set_string(&self, name: &str, value: &str) -> Result<(), String> {
        use winapi::um::winnt::REG_SZ;
        use winapi::um::winreg::RegSetValueExW;

        let name_w = wide(name);
        let value_w = wide(value);
        // Byte count *including* the terminator: `REG_SZ` is defined as a NUL-terminated string, and
        // a length leaving the NUL out produces a value other tools read as unterminated.
        let bytes = (value_w.len() * 2) as u32;
        // SAFETY: both buffers are NUL-terminated and outlive the call, and `bytes` is their true size.
        let status = unsafe {
            RegSetValueExW(self.0, name_w.as_ptr(), 0, REG_SZ, value_w.as_ptr().cast(), bytes)
        };
        if status == 0 { Ok(()) } else { Err(format!("could not write {name} ({status})")) }
    }

    /// Removes a value.
    ///
    /// Test-only until the tray grew a Start-at-sign-in checkbox, which is what unticking it does.
    /// The old note here said the app never un-registers, on the grounds that the OS entry is the
    /// only record of the answer and a second way to change it would be a second source of truth.
    /// The checkbox does not break that: it *reads* the OS entry every time the menu is built, so
    /// there is still exactly one record, and this is how the user edits it.
    ///
    /// Already gone counts as success: the contract is "this value is not there", and neither an
    /// unticked box nor a test cleaning up after a part-way failure should have to know whether the
    /// write ever happened.
    fn delete_value(&self, name: &str) -> Result<(), String> {
        use winapi::shared::winerror::ERROR_FILE_NOT_FOUND;
        use winapi::um::winreg::RegDeleteValueW;

        let name_w = wide(name);
        // SAFETY: `name_w` is NUL-terminated and outlives the call.
        let status = unsafe { RegDeleteValueW(self.0, name_w.as_ptr()) };
        if status == 0 || status == ERROR_FILE_NOT_FOUND as i32 {
            Ok(())
        } else {
            Err(format!("could not remove {name} ({status})"))
        }
    }

    /// Removes a subkey of this key.
    ///
    /// Test-only, like `delete_value`, and for a stronger reason: nothing in the app ever deletes a
    /// key. It exists so the registry tests can build their own small tree, prove the read, write and
    /// enumeration operations agree, and then take it away again — rather than exercising the real
    /// `Run` key, which would mean writing a live startup entry on the machine running the tests.
    ///
    /// Only ever called on a key this process created under a PID-suffixed name, and only deletes one
    /// level: `RegDeleteKeyW` refuses a key that still has subkeys, which is a useful guard rather
    /// than a limitation.
    #[cfg(test)]
    fn delete_subkey(&self, name: &str) -> Result<(), String> {
        use winapi::shared::winerror::ERROR_FILE_NOT_FOUND;
        use winapi::um::winreg::RegDeleteKeyW;

        let name_w = wide(name);
        // SAFETY: `name_w` is NUL-terminated and outlives the call.
        let status = unsafe { RegDeleteKeyW(self.0, name_w.as_ptr()) };
        if status == 0 || status == ERROR_FILE_NOT_FOUND as i32 {
            Ok(())
        } else {
            Err(format!("could not remove the {name} subkey ({status})"))
        }
    }
}

// ─── Windows: starting at sign-in ─────────────────────────────────────────────

/// The per-user key Windows runs at sign-in. `HKCU`, never `HKLM`: no elevation, and nothing is
/// registered for other accounts on the machine.
#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Registers `exe` to start at sign-in, and reads the value back to prove it took.
///
/// The read-back is not belt-and-braces. `RegSetValueExW` reports success on the write, not on the
/// value being what was intended, so a wrong length or a policy that quietly filters `Run` entries
/// would leave a happy return value and no startup entry — the one failure a user cannot diagnose,
/// because the only symptom is the app not being there weeks later. Better to say so now.
#[cfg(target_os = "windows")]
pub fn enable(exe: &Path) -> Result<(), String> {
    enable_in(RUN_KEY, exe)
}

/// Whether an entry is registered at all.
///
/// **Presence, not correctness.** An entry left by an install in another directory still starts the
/// app at sign-in, so the checkbox has to show it ticked; ticking it again rewrites the value to this
/// executable, which is the repair. Asking "does it point at *me*" would show an empty box beside an
/// app that does start itself, and unticking would then be the only way to fix a path nobody can see.
#[cfg(target_os = "windows")]
pub fn is_enabled() -> bool {
    is_enabled_in(RUN_KEY)
}

/// Removes the entry. Nothing registered is success: the user asked for the box to be empty, and it is.
#[cfg(target_os = "windows")]
pub fn disable() -> Result<(), String> {
    disable_in(RUN_KEY)
}

/// The three operations against a named key, so the tests can exercise the whole round trip without
/// registering a real startup entry on the machine running them. Only the path differs.
#[cfg(target_os = "windows")]
fn enable_in(run_key: &str, exe: &Path) -> Result<(), String> {
    use winapi::um::winnt::{KEY_QUERY_VALUE, KEY_SET_VALUE};

    let command = run_command(exe);
    // `create`, not `open`: a profile that has never had a startup entry has no `Run` key at all —
    // see `Key::create` for the CI run that proved it. That is the fresh install this feature is for.
    let key = Key::create(run_key, KEY_SET_VALUE | KEY_QUERY_VALUE)?;
    key.set_string(ENTRY_NAME, &command)?;

    match key.string(ENTRY_NAME) {
        Some(stored) if stored == command => Ok(()),
        Some(stored) => {
            Err(format!("the {ENTRY_NAME} value reads back as {stored:?}, not {command:?}"))
        }
        None => Err(format!("the {ENTRY_NAME} value was written but is not there when read back")),
    }
}

#[cfg(target_os = "windows")]
fn is_enabled_in(run_key: &str) -> bool {
    use winapi::um::winnt::KEY_QUERY_VALUE;

    // `open`, never `create`: reading whether something is registered must not create the key it reads
    // from. An absent key is the answer rather than an obstacle — it is what a profile that has never
    // had a startup entry looks like.
    Key::open(run_key, KEY_QUERY_VALUE).is_ok_and(|key| key.string(ENTRY_NAME).is_some())
}

#[cfg(target_os = "windows")]
fn disable_in(run_key: &str) -> Result<(), String> {
    use winapi::um::winnt::KEY_SET_VALUE;

    // An absent key means nothing is registered, which is the state being asked for. Creating it in
    // order to delete nothing out of it would leave a key behind as the only trace of unticking a box.
    let Ok(key) = Key::open(run_key, KEY_SET_VALUE) else {
        return Ok(());
    };
    key.delete_value(ENTRY_NAME)
}

/// The command line the `Run` value holds.
///
/// Always quoted. Windows parses an unquoted `Run` value by trying each space as a possible break, so
/// `C:\Program Files\GitHoot\githoot.exe` would first be tried as `C:\Program` with
/// `Files\GitHoot\githoot.exe` as an argument — the classic unquoted-path trap. A path can never
/// itself contain a quote on Windows, so wrapping is the whole of the escaping needed.
#[cfg(target_os = "windows")]
fn run_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Deletes a file, treating "it was not there" as success.
///
/// Shared by the two platforms whose entry is a file. An entry the user has already deleted by hand is
/// the state they are asking for, so reporting it as a failure would put an error in front of someone
/// who got what they wanted.
#[cfg(not(target_os = "windows"))]
fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove {} ({e})", path.display())),
    }
}

// ─── macOS ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn enable(exe: &Path) -> Result<(), String> {
    let dir = launch_agents_dir()?;
    write_launch_agent(&dir, exe)
}

/// Whether a Launch Agent is installed. Presence, not correctness — see the Windows [`is_enabled`].
///
/// A home directory that cannot be resolved reads as "not enabled": nothing can be starting from a
/// plist nobody can find, and the alternative is a checkbox showing a state it could not verify.
#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    launch_agents_dir().is_ok_and(|dir| is_enabled_in(&dir))
}

/// Removes the Launch Agent.
///
/// Deleting the plist is the whole of it, with no `launchctl unload` to match: the agent carries
/// `RunAtLoad` and deliberately no `KeepAlive`, so nothing of it is still running once the app has
/// been started. `launchd` reads the directory again at the next sign-in, the only moment it matters.
#[cfg(target_os = "macos")]
pub fn disable() -> Result<(), String> {
    let dir = launch_agents_dir()?;
    disable_in(&dir)
}

/// The two operations against a named directory, so the tests need no `~/Library/LaunchAgents`.
#[cfg(target_os = "macos")]
fn is_enabled_in(dir: &Path) -> bool {
    launch_agent_path(dir).exists()
}

#[cfg(target_os = "macos")]
fn disable_in(dir: &Path) -> Result<(), String> {
    remove_if_present(&launch_agent_path(dir))
}

/// Where the plist goes. One definition, so what `enable` writes is what `disable` deletes and what
/// `is_enabled` looks for.
#[cfg(target_os = "macos")]
fn launch_agent_path(dir: &Path) -> std::path::PathBuf {
    dir.join(format!("{}.plist", launch_agent_label()))
}

/// The reverse-DNS label `launchd` files everything under, and the plist's file name bar the suffix.
///
/// Built from [`ENTRY_NAME`] rather than spelled out, so the same word appears here as in the Windows
/// registry value and the Linux desktop entry. A function and not a `const` only because `const` cannot
/// interpolate one, and repeating the name to keep it a `const` is exactly the drift worth avoiding.
#[cfg(target_os = "macos")]
fn launch_agent_label() -> String {
    format!("com.githoot.{ENTRY_NAME}")
}

/// `~/Library/LaunchAgents`, the per-user agent directory.
///
/// Never `/Library/LaunchAgents`, which is machine-wide and needs root: this registers a tray icon for
/// one person, and nothing here should ever want a privilege it cannot get.
#[cfg(target_os = "macos")]
fn launch_agents_dir() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("could not find home directory")?;
    Ok(home.join("Library").join("LaunchAgents"))
}

#[cfg(target_os = "macos")]
fn write_launch_agent(dir: &Path, exe: &Path) -> Result<(), String> {
    // `create_dir_all`: `~/Library/LaunchAgents` genuinely does not exist on an account that has never
    // installed one, so a plain write would fail on exactly the fresh machine this runs on.
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = launch_agent_path(dir);
    std::fs::write(&path, launch_agent(exe))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// The Launch Agent property list.
///
/// `ProgramArguments` names the executable inside the bundle rather than the `.app` itself, which is
/// what `launchd` wants — it launches a process, not a document. The bundle is still found from the
/// executable's path, so the app's `LSUIElement` still applies and no Dock icon appears.
///
/// Only `RunAtLoad`. Deliberately no `KeepAlive`: that would make the app unquittable, restarting it
/// every time someone used the tray's own Quit entry.
#[cfg(target_os = "macos")]
fn launch_agent(exe: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{}</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n",
        launch_agent_label(),
        xml_escape(&exe.display().to_string())
    )
}

/// Escapes a path for an XML text node.
///
/// `&` first, or the entities the other two produce would have their own ampersands escaped again.
/// `&`, `<` and `>` are all legal in a macOS file name and all illegal raw in XML, and the failure is
/// silent: `launchd` simply refuses to load a malformed plist, and nothing in this app reads it back.
#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// ─── Linux and other unixes ───────────────────────────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn enable(exe: &Path) -> Result<(), String> {
    let dir = autostart_dir()?;
    write_desktop_entry(&dir, exe)
}

/// Whether an autostart entry is installed. Presence, not correctness — see the Windows [`is_enabled`].
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_enabled() -> bool {
    autostart_dir().is_ok_and(|dir| is_enabled_in(&dir))
}

/// Removes the autostart entry.
///
/// The file is deleted rather than left in place with `X-GNOME-Autostart-enabled=false`. That key is a
/// GNOME extension every other session ignores, so an entry "disabled" that way would still start the
/// app on KDE, Xfce and the rest — a checkbox that unticks itself and changes nothing.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn disable() -> Result<(), String> {
    let dir = autostart_dir()?;
    disable_in(&dir)
}

/// The two operations against a named directory, so the tests need no `~/.config/autostart`.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn is_enabled_in(dir: &Path) -> bool {
    dir.join(DESKTOP_FILE).exists()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn disable_in(dir: &Path) -> Result<(), String> {
    remove_if_present(&dir.join(DESKTOP_FILE))
}

/// The entry's file name. Named after the binary rather than after [`ENTRY_NAME`], because this is the
/// one place the convention is a file name in a shared directory and every other entry in it is named
/// after its program.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DESKTOP_FILE: &str = "githoot.desktop";

/// `$XDG_CONFIG_HOME/autostart`, falling back to `~/.config/autostart`.
///
/// Honouring `XDG_CONFIG_HOME` is what the Base Directory spec asks for, and it is also the only way
/// the tests can check this function without writing into the home directory of whoever is running
/// them. A relative value is ignored rather than joined, which the spec requires and which also stops
/// an entry landing somewhere relative to the working directory the app happened to start in.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn autostart_dir() -> Result<std::path::PathBuf, String> {
    if let Some(configured) = std::env::var_os("XDG_CONFIG_HOME") {
        let dir = std::path::PathBuf::from(configured);
        if dir.is_absolute() {
            return Ok(dir.join("autostart"));
        }
    }
    let home = dirs::home_dir().ok_or("could not find home directory")?;
    Ok(home.join(".config").join("autostart"))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn write_desktop_entry(dir: &Path, exe: &Path) -> Result<(), String> {
    // `create_dir_all`: `~/.config/autostart` does not exist until something puts an entry in it, so
    // on a fresh account a plain write would fail.
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = dir.join(DESKTOP_FILE);
    std::fs::write(&path, desktop_entry(exe))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// The freedesktop autostart entry.
///
/// `Terminal=false` is not decoration: an entry without it is read as a console program, which some
/// sessions show as a broken startup item and others refuse to run at all.
///
/// `X-GNOME-Autostart-enabled=true` is a GNOME extension every other session ignores. It is included
/// because GNOME's own Tweaks writes it, and its absence has been read as "disabled" by some versions.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={ENTRY_NAME}\n\
         Comment=Watch your GitHub pull requests from the tray\n\
         Exec=\"{}\"\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exec_escape(&exe.display().to_string())
    )
}

/// Escapes a path for use inside a quoted `Exec` argument.
///
/// The `Exec` value is quoted for the reason the Windows `Run` value is: an unquoted space starts a
/// new argument, so `/opt/git hoot/githoot` would be launched as `/opt/git` with `hoot/...` as its
/// first argument. Inside those quotes the desktop entry spec reserves `"`, `` ` ``, `$` and `\`, each
/// escaped with a backslash — and the backslash goes first, or the escapes the others add would then be
/// escaped in turn.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn exec_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tray_hint_tests {
    use super::*;

    /// The hint replaces writing `IsPromoted` ourselves, so it has to name the switch the user flips
    /// and the page the button opens.
    #[test]
    fn the_tray_hint_names_the_switch_and_the_settings_page() {
        assert_eq!(TASKBAR_SETTINGS, "ms-settings:taskbar");
        let hint = tray_hint();
        assert!(hint.contains("Other system tray icons"), "{hint}");
        assert!(hint.contains(ENTRY_NAME), "{hint}");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;
    use winapi::um::winnt::{KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE};

    /// Where the registry tests do their work: a key this process makes and takes away again.
    ///
    /// Deliberately **not** the real `Run` key. An earlier version of these tests used it, on the
    /// grounds that a stand-in proves nothing about the registry — and it was right about that, but
    /// wrong about the cost: exercising `Run` for real means writing a live startup entry on whatever
    /// machine runs `cargo test`. This owns its own patch instead, and `enable`'s six lines over the
    /// same `Key` are covered by the `#[ignore]`d test at the bottom.
    ///
    /// Named per **test**, not per process. Cargo runs these on parallel threads of a single process,
    /// so a PID-only name handed every test the same key: the non-ASCII test overwrote the round-trip
    /// test's value, and each `Cleanup` deleted a tree another test was still using. The PID stays in
    /// the name so two concurrent `cargo test` runs cannot collide either.
    fn test_root(label: &str) -> String {
        format!("Software\\{ENTRY_NAME}-selftest-{}-{label}", std::process::id())
    }

    /// Deletes the whole test tree however the test ends, so a panicking assertion leaves nothing
    /// behind. Depth-first, because `RegDeleteKeyW` refuses a key that still has subkeys.
    struct Cleanup {
        root: String,
        children: Vec<String>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(root) = Key::open(&self.root, KEY_SET_VALUE) {
                for child in &self.children {
                    let _ = root.delete_subkey(child);
                }
            }
            if let Ok(software) = Key::open("Software", KEY_SET_VALUE) {
                let leaf = self.root.rsplit('\\').next().unwrap_or(&self.root);
                let _ = software.delete_subkey(leaf);
            }
        }
    }

    fn fresh_test_key(label: &str, children: &[&str]) -> (Key, Cleanup) {
        let root = test_root(label);
        let cleanup = Cleanup {
            root: root.clone(),
            children: children.iter().map(|c| c.to_string()).collect(),
        };
        let key = Key::create(&root, KEY_SET_VALUE | KEY_QUERY_VALUE | KEY_READ)
            .expect("a key under HKCU\\Software must be creatable without elevation");
        for child in children {
            Key::create(&format!("{root}\\{child}"), KEY_SET_VALUE)
                .expect("a subkey must be creatable");
        }
        (key, cleanup)
    }

    #[test]
    fn the_run_command_is_quoted_so_a_path_with_spaces_survives() {
        let command = run_command(Path::new(r"C:\Program Files\GitHoot\githoot.exe"));
        assert_eq!(command, "\"C:\\Program Files\\GitHoot\\githoot.exe\"");
    }

    /// The `Run` key is **not** guaranteed to exist: on a GitHub `windows-latest` runner it is absent,
    /// because the subkey is created by whatever first adds a startup entry and that profile never
    /// has. `create` must therefore succeed where `open` would return `ERROR_FILE_NOT_FOUND` — this is
    /// the regression test for the bug that shipped in the first draft of this module.
    ///
    /// Only opens or creates the key; writes no value, so no startup entry is registered. An empty
    /// `Run` key is exactly what a profile that has never had a startup entry would grow the moment
    /// anything added one.
    #[test]
    fn the_run_key_can_be_reached_even_on_a_profile_that_has_never_had_one() {
        Key::create(RUN_KEY, KEY_SET_VALUE | KEY_QUERY_VALUE)
            .expect("Run must be creatable when absent, not just openable when present");
    }

    #[test]
    fn a_string_value_round_trips_through_the_registry() {
        let (key, _cleanup) = fresh_test_key("round-trip", &[]);

        let command = run_command(Path::new(r"C:\Program Files\GitHoot\githoot.exe"));
        key.set_string(ENTRY_NAME, &command).expect("the test key must be writable");

        let read_back = key.string(ENTRY_NAME).expect("the value just written must read back");
        assert_eq!(read_back, command, "what comes out must be what went in");

        key.delete_value(ENTRY_NAME).expect("a value this test wrote must be removable");
        assert_eq!(key.string(ENTRY_NAME), None, "a removed value must read back as absent");
    }

    /// A path long enough to force the two-pass read to allocate properly, and non-ASCII so the
    /// UTF-16 round trip is exercised rather than assumed. A user name with an umlaut is not exotic.
    #[test]
    fn a_long_non_ascii_path_survives_the_utf16_round_trip() {
        let (key, _cleanup) = fresh_test_key("utf16", &[]);

        let exe = Path::new(r"C:\Users\Jörg Müller\AppData\Local\Programs\GitHoot\githoot.exe");
        let command = run_command(exe);
        key.set_string(ENTRY_NAME, &command).expect("the test key must be writable");

        assert_eq!(key.string(ENTRY_NAME).as_deref(), Some(command.as_str()));
    }

    /// Absent is not an error: it is the ordinary state, and the read has to say so plainly rather
    /// than by failing, or nothing could tell deletion from a broken read.
    #[test]
    fn a_value_that_was_never_written_reads_back_as_absent() {
        let (key, _cleanup) = fresh_test_key("absent", &[]);
        assert_eq!(key.string("never-written"), None);
    }

    /// The one path no automated test may take: `enable` writes a **real** startup entry for the
    /// account running the tests. `#[ignore]`d because `cargo test` must never do that to anyone.
    /// Run by hand with `cargo test -- --ignored registers_a_real_startup_entry`, then check Task
    /// Manager › Startup apps and remove it — this test deliberately does not clean up, because the
    /// thing worth seeing is the entry sitting where a user would find it.
    #[test]
    #[ignore = "writes a real startup entry for the current account; remove it by hand afterwards"]
    fn registers_a_real_startup_entry() {
        let exe = std::env::current_exe().expect("the test binary has a path");
        enable(&exe).expect("enable must succeed and read its own value back");

        let key = Key::create(RUN_KEY, KEY_QUERY_VALUE).expect("Run must be reachable");
        assert_eq!(key.string(ENTRY_NAME).as_deref(), Some(run_command(&exe).as_str()));
    }

    // ── Reading and removing the entry ──────────────────────────────────────
    //
    // Against the same scratch key the rest of these tests use, never the real `Run`: the whole round
    // trip can be proved without registering a startup entry on the machine running `cargo test`.

    /// The three states the menu's checkbox moves between: nothing registered, registered, and
    /// removed again. The second removal is not padding — a user can untick a box whose entry they
    /// have already cleared in Task Manager, and that has to be success rather than an error dialog.
    #[test]
    fn an_entry_can_be_written_read_back_and_removed() {
        let root = test_root("autostart-toggle");
        let _cleanup = Cleanup { root: root.clone(), children: Vec::new() };
        let exe = std::env::current_exe().expect("the test binary has a path");

        assert!(!is_enabled_in(&root), "a key that does not exist yet registers nothing");

        enable_in(&root, &exe).expect("writing the entry must succeed");
        assert!(is_enabled_in(&root), "a written entry must read back as enabled");

        disable_in(&root).expect("removing the entry must succeed");
        assert!(!is_enabled_in(&root), "a removed entry must read back as disabled");

        disable_in(&root).expect("removing an entry that is already gone is not a failure");
    }

    /// A `Run` key that does not exist at all is what a profile that has never had a startup entry
    /// looks like — see `Key::create`. Reading it must answer "not enabled" rather than fail, and
    /// asking to remove an entry from it must not create the key in order to delete nothing.
    #[test]
    fn a_run_key_that_does_not_exist_reads_as_not_enabled() {
        let absent = test_root("autostart-absent");

        assert!(!is_enabled_in(&absent));
        disable_in(&absent).expect("nothing to remove is success, not failure");
        assert!(
            Key::open(&absent, KEY_QUERY_VALUE).is_err(),
            "asking to disable must not have created the key"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn the_launch_agent_runs_the_binary_at_load() {
        let plist = launch_agent(Path::new("/Applications/GitHoot.app/Contents/MacOS/githoot"));
        assert!(plist.starts_with("<?xml"), "must be a plist, got {plist:?}");
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"), "got {plist:?}");
        assert!(
            plist.contains("/Applications/GitHoot.app/Contents/MacOS/githoot"),
            "must name the binary, got {plist:?}"
        );
        assert!(plist.contains(ENTRY_NAME), "must carry the shared entry name, got {plist:?}");
    }

    /// `&` is legal in a macOS path and illegal raw in XML, so an unescaped path would produce a plist
    /// `launchd` refuses to load — silently, since nothing in this app reads it back.
    #[test]
    fn a_path_with_xml_significant_characters_is_escaped() {
        let plist = launch_agent(Path::new("/Users/me/R&D/<app>/githoot"));
        assert!(plist.contains("R&amp;D/&lt;app&gt;"), "got {plist:?}");
        assert!(!plist.contains("R&D"), "the raw ampersand must not survive, got {plist:?}");
    }

    #[test]
    fn the_launch_agent_is_written_into_the_directory_it_is_given() {
        let dir = std::env::temp_dir().join(format!("githoot-launchagent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let exe = Path::new("/Applications/GitHoot.app/Contents/MacOS/githoot");
        write_launch_agent(&dir, exe).expect("must create the directory and the plist");

        let written = std::fs::read_to_string(dir.join(format!("com.githoot.{ENTRY_NAME}.plist")))
            .expect("the plist must be where the loader looks for it");
        assert_eq!(written, launch_agent(exe));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The three states the menu's checkbox turns on, against a temp directory rather than the real
    /// `~/Library/LaunchAgents`. Removing twice is included because a user can uncheck a box whose
    /// plist they have already deleted by hand.
    #[test]
    fn a_launch_agent_can_be_written_read_back_and_removed() {
        let dir = std::env::temp_dir().join(format!("githoot-agent-toggle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let exe = Path::new("/Applications/GitHoot.app/Contents/MacOS/githoot");

        assert!(!is_enabled_in(&dir), "an empty directory holds no agent");

        write_launch_agent(&dir, exe).expect("the plist must be written");
        assert!(is_enabled_in(&dir), "a written plist must read back as enabled");

        disable_in(&dir).expect("removing the plist must succeed");
        assert!(!is_enabled_in(&dir), "a removed plist must read back as disabled");

        disable_in(&dir).expect("removing a plist that is already gone is not a failure");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, not(any(target_os = "windows", target_os = "macos"))))]
mod unix_tests {
    use super::*;

    #[test]
    fn the_desktop_entry_is_an_autostart_application_that_runs_the_binary() {
        let entry = desktop_entry(Path::new("/usr/local/bin/githoot"));
        assert!(entry.starts_with("[Desktop Entry]"), "got {entry:?}");
        assert!(entry.contains("Type=Application"), "got {entry:?}");
        assert!(entry.contains("Exec=\"/usr/local/bin/githoot\""), "got {entry:?}");
        assert!(entry.contains(&format!("Name={ENTRY_NAME}")), "got {entry:?}");
        // Without this a GNOME session shows a window-less tray app in its startup list as a broken
        // entry, and some sessions refuse to run it at all.
        assert!(entry.contains("Terminal=false"), "got {entry:?}");
    }

    /// The `Exec` key is quoted for the same reason the Windows `Run` value is: a bare space would be
    /// read as the start of an argument. Quotes and backslashes inside the path are escaped per the
    /// desktop entry spec, which is otherwise a silently malformed file.
    #[test]
    fn the_exec_key_quotes_and_escapes_the_path() {
        let entry = desktop_entry(Path::new("/opt/git hoot/githoot"));
        assert!(entry.contains("Exec=\"/opt/git hoot/githoot\""), "got {entry:?}");

        let odd = desktop_entry(Path::new(r#"/opt/we"ird\path/githoot"#));
        assert!(odd.contains(r#"Exec="/opt/we\"ird\\path/githoot""#), "got {odd:?}");
    }

    #[test]
    fn the_desktop_entry_is_written_into_the_directory_it_is_given() {
        let dir = std::env::temp_dir().join(format!("githoot-autostart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let exe = Path::new("/usr/local/bin/githoot");
        write_desktop_entry(&dir, exe).expect("must create the directory and the entry");

        let written = std::fs::read_to_string(dir.join("githoot.desktop"))
            .expect("the entry must be where the session looks for it");
        assert_eq!(written, desktop_entry(exe));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The three states the menu's checkbox turns on, against a temp directory rather than the real
    /// `~/.config/autostart`. Removing twice is included because a user can uncheck a box whose entry
    /// their desktop's own startup tool has already deleted.
    #[test]
    fn a_desktop_entry_can_be_written_read_back_and_removed() {
        let dir = std::env::temp_dir().join(format!("githoot-desktop-toggle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let exe = Path::new("/usr/local/bin/githoot");

        assert!(!is_enabled_in(&dir), "an empty directory holds no entry");

        write_desktop_entry(&dir, exe).expect("the entry must be written");
        assert!(is_enabled_in(&dir), "a written entry must read back as enabled");

        disable_in(&dir).expect("removing the entry must succeed");
        assert!(!is_enabled_in(&dir), "a removed entry must read back as disabled");

        disable_in(&dir).expect("removing an entry that is already gone is not a failure");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `XDG_CONFIG_HOME` is what the spec says to honour, and honouring it is also the only way this
    /// function can be checked without writing into the home directory of whoever runs the tests.
    #[test]
    fn the_autostart_dir_honours_xdg_config_home() {
        // SAFETY: single-threaded test, and the variable is restored before it returns.
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/tmp/githoot-xdg-test") }

        let dir = autostart_dir();

        unsafe {
            match previous {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        assert_eq!(dir, Ok(std::path::PathBuf::from("/tmp/githoot-xdg-test/autostart")));
    }
}
