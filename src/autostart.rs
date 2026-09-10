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
const ENTRY_NAME: &str = "GitHootTray";

/// Where the entry ends up, named in the prompt so removing it later is not a research task.
#[cfg(target_os = "windows")]
const ENTRY_LOCATION: &str = "Startup apps in Windows Settings";
#[cfg(target_os = "macos")]
const ENTRY_LOCATION: &str = "~/Library/LaunchAgents";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const ENTRY_LOCATION: &str = "~/.config/autostart";

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

        if !crate::dialog::confirm_autostart("githoot-tray: start at sign-in", &body) {
            infoln!("not starting at sign-in (declined, or no dialog could be shown) — nothing was registered");
            return;
        }

        match enable(&exe) {
            Ok(()) => infoln!("registered to start at sign-in: {}", exe.display()),
            // Not fatal and not worth a second dialog: the app is running, and the only thing lost is a
            // convenience the user can set up with their OS's own startup tool.
            Err(e) => errorln!("could not register to start at sign-in ({e}) — add it by hand under {ENTRY_LOCATION}"),
        }
    });
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
    /// Opens an existing key under `HKCU` with exactly the access asked for.
    ///
    /// Open, not create: every key this module touches is part of a stock Windows profile, so a
    /// failure to open one is a real fault worth reporting rather than something to paper over by
    /// creating a key somewhere this app does not own. It is also how Windows 10 is detected —
    /// `Control Panel\NotifyIconSettings` simply is not there.
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

    /// Whether a value exists at all, whatever its type.
    ///
    /// The distinction the icon promotion turns on: an absent `IsPromoted` means nobody has ever
    /// decided, while a present one — `0` or `1` — is a decision, and decisions are not overwritten.
    fn has_value(&self, name: &str) -> bool {
        use std::ptr::null_mut;
        use winapi::shared::minwindef::DWORD;
        use winapi::um::winreg::RegQueryValueExW;

        let name_w = wide(name);
        let mut bytes: DWORD = 0;
        // SAFETY: the size-query form again, with the type discarded — only presence matters here.
        let status = unsafe {
            RegQueryValueExW(self.0, name_w.as_ptr(), null_mut(), null_mut(), null_mut(), &mut bytes)
        };
        status == 0
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

    fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        use winapi::um::winnt::REG_DWORD;
        use winapi::um::winreg::RegSetValueExW;

        let name_w = wide(name);
        // SAFETY: `value` is a live `u32` for the duration of the call, and 4 is its true size.
        let status = unsafe {
            RegSetValueExW(
                self.0,
                name_w.as_ptr(),
                0,
                REG_DWORD,
                std::ptr::from_ref(&value).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if status == 0 { Ok(()) } else { Err(format!("could not write {name} ({status})")) }
    }

    /// The names of this key's immediate subkeys.
    ///
    /// Collected rather than streamed: the caller reopens each one to read from it, and holding an
    /// enumeration open across those opens is the shape that goes wrong if the shell adds an entry
    /// mid-walk. A name that cannot be read ends the walk rather than failing the whole call — a
    /// partial list is still useful, and the caller retries anyway.
    fn subkeys(&self) -> Vec<String> {
        use std::ptr::null_mut;
        use winapi::shared::minwindef::DWORD;
        use winapi::um::winreg::RegEnumKeyExW;

        // 255 is the documented maximum key-name length, plus one for the terminator.
        const MAX_KEY_NAME: usize = 256;

        let mut names = Vec::new();
        for index in 0.. {
            let mut buffer = [0u16; MAX_KEY_NAME];
            // In characters and not counting the terminator, in and out.
            let mut length = (MAX_KEY_NAME - 1) as DWORD;
            // SAFETY: `buffer` holds `MAX_KEY_NAME` characters and `length` says so; every optional
            // out-parameter is passed as null, which this call permits.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    buffer.as_mut_ptr(),
                    &mut length,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if status != 0 {
                break;
            }
            names.push(String::from_utf16_lossy(&buffer[..length as usize]));
        }
        names
    }

    /// Removes a value.
    ///
    /// Test-only, and deliberately not offered to the rest of the app. Nothing here ever
    /// un-registers: the question is asked once and the OS entry is the only record of the answer, so
    /// a `disable` would be a second way to change a state nothing reconciles (see the module docs).
    /// This exists so the round-trip test can clean up after itself rather than leaving a stray
    /// startup entry in a real user's registry.
    ///
    /// Already gone counts as success: the contract is "this value is not there", and a test cleaning
    /// up after a failure part-way through should not have to know whether the write ever happened.
    #[cfg(test)]
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
    use winapi::um::winnt::{KEY_QUERY_VALUE, KEY_SET_VALUE};

    let command = run_command(exe);
    let key = Key::open(RUN_KEY, KEY_SET_VALUE | KEY_QUERY_VALUE)?;
    key.set_string(ENTRY_NAME, &command)?;

    match key.string(ENTRY_NAME) {
        Some(stored) if stored == command => Ok(()),
        Some(stored) => {
            Err(format!("the {ENTRY_NAME} value reads back as {stored:?}, not {command:?}"))
        }
        None => Err(format!("the {ENTRY_NAME} value was written but is not there when read back")),
    }
}

/// The command line the `Run` value holds.
///
/// Always quoted. Windows parses an unquoted `Run` value by trying each space as a possible break, so
/// `C:\Program Files\GitHoot\githoot-tray.exe` would first be tried as `C:\Program` with
/// `Files\GitHoot\githoot-tray.exe` as an argument — the classic unquoted-path trap. A path can never
/// itself contain a quote on Windows, so wrapping is the whole of the escaping needed.
#[cfg(target_os = "windows")]
fn run_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

// ─── Windows: keeping the icon out of the overflow flyout ─────────────────────

/// Where Windows 11 records, per tray icon, whether it sits on the taskbar or in the overflow flyout.
///
/// **Undocumented, and there is no supported alternative.** Microsoft deliberately provides no Win32
/// call for an app to promote its own icon — it is meant to be the user's choice, and this app is only
/// making that choice once, on the way in, for an icon whose entire purpose is to be looked at. The
/// key was verified by hand on Windows 11 (build 26200): each subkey is an opaque hash carrying
/// `ExecutablePath`, `UID`, `InitialTooltip` and, once anything has decided, `IsPromoted`.
///
/// Windows 10 has no such key. There the same setting lives in `TrayNotify\IconStreams`, an
/// undocumented, obfuscated binary blob that cannot be edited without restarting `explorer.exe`. That
/// is not worth doing to a user's machine, so on Windows 10 this feature simply does not apply and
/// says so in the log.
#[cfg(target_os = "windows")]
const NOTIFY_ICON_SETTINGS: &str = r"Control Panel\NotifyIconSettings";

/// The value naming the program an entry belongs to.
#[cfg(target_os = "windows")]
const EXECUTABLE_PATH: &str = "ExecutablePath";

/// `1` puts the icon on the taskbar, `0` in the overflow flyout, absent means nobody has decided.
#[cfg(target_os = "windows")]
const IS_PROMOTED: &str = "IsPromoted";

/// How many times to look for our entry, and how long to wait between looks.
///
/// The entry is created by the shell when it first receives our icon, which is a moment or two after
/// the tray is built, so the first look usually finds nothing. Five seconds all told: long enough for
/// a slow shell, short enough that a thread is not left sitting around on a machine where the key will
/// never appear at all.
#[cfg(target_os = "windows")]
const PROMOTION_ATTEMPTS: u32 = 10;
#[cfg(target_os = "windows")]
const PROMOTION_RETRY: std::time::Duration = std::time::Duration::from_millis(500);

/// What one sweep of `NotifyIconSettings` concluded.
#[cfg(target_os = "windows")]
#[derive(Debug, PartialEq, Eq)]
enum Promotion {
    /// Set `IsPromoted` on this many entries.
    Promoted(usize),
    /// Our entry is there and already carries an `IsPromoted` — ours from a previous start, or the
    /// user's own choice. Either way it is left exactly as it is.
    AlreadyDecided,
    /// No entry names this executable yet. The shell has not registered the icon; worth another look.
    NotListedYet,
    Failed(String),
}

/// Asks Windows to keep this app's tray icon on the taskbar rather than in the overflow flyout.
///
/// Returns immediately; the work happens on a background thread, because it has to wait for the shell
/// to notice the icon and nothing in startup should wait for that.
///
/// **Why this is not gated on a first run**, unlike [`offer_on_first_run`]: it is gated on
/// `IsPromoted` being *absent*, which is a stronger version of the same idea. Absent means nobody has
/// ever decided, so setting it takes nothing away from anyone. A value that is present is a decision —
/// `0` because the user hid the icon in Settings, `1` because an earlier start did this — and is never
/// touched, so this can never fight someone who wants the icon hidden. Running every start also makes
/// it self-healing: a first run that raced the shell and found nothing simply succeeds on the next one,
/// where a first-run-only version would have missed its single chance for good.
///
/// Deliberately not asked about in a dialog. It is cosmetic, it is undone in two clicks under
/// Settings › Personalisation › Taskbar › Other system tray icons, and an owl hidden inside a flyout
/// cannot do the one job this app has. A second prompt on a first run would cost more attention than
/// the decision is worth.
#[cfg(target_os = "windows")]
pub fn promote_tray_icon() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            errorln!("not promoting the tray icon: could not locate this executable ({e})");
            return;
        }
    };

    std::thread::spawn(move || {
        for attempt in 1..=PROMOTION_ATTEMPTS {
            match promote_once(&exe) {
                Promotion::Promoted(count) => {
                    infoln!(
                        "tray icon set to always show ({count} matching entr{} under \
                         {NOTIFY_ICON_SETTINGS}); Windows may only apply it from the next sign-in",
                        if count == 1 { "y" } else { "ies" }
                    );
                    return;
                }
                Promotion::AlreadyDecided => {
                    infoln!("leaving the tray icon's visibility alone — it has already been decided");
                    return;
                }
                // Only worth saying once the waiting is over: on a normal start the first few sweeps
                // find nothing at all, which is not news.
                Promotion::NotListedYet if attempt == PROMOTION_ATTEMPTS => {
                    infoln!(
                        "the tray icon never appeared under {NOTIFY_ICON_SETTINGS} — leaving its \
                         visibility to Windows"
                    );
                }
                Promotion::NotListedYet => std::thread::sleep(PROMOTION_RETRY),
                // Includes Windows 10 and older, where the key does not exist. Not an error the user
                // can act on, and nothing is broken by it.
                Promotion::Failed(e) => {
                    infoln!("not promoting the tray icon: {e}");
                    return;
                }
            }
        }
    });
}

/// One sweep of `NotifyIconSettings`.
#[cfg(target_os = "windows")]
fn promote_once(exe: &Path) -> Promotion {
    use winapi::um::winnt::{KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE};

    let root = match Key::open(NOTIFY_ICON_SETTINGS, KEY_READ) {
        Ok(root) => root,
        Err(e) => return Promotion::Failed(e),
    };

    let mut matched = 0usize;
    let mut promoted = 0usize;
    for name in root.subkeys() {
        // Opened for reading and writing up front, so a match needs no second open, and so a key this
        // account cannot write is skipped before anything is read from it.
        let Ok(entry) = Key::open(
            &format!("{NOTIFY_ICON_SETTINGS}\\{name}"),
            KEY_QUERY_VALUE | KEY_SET_VALUE,
        ) else {
            continue;
        };
        if !entry.string(EXECUTABLE_PATH).is_some_and(|recorded| refers_to(&recorded, exe)) {
            continue;
        }

        matched += 1;
        // The whole restraint of this feature, in one branch: a decision that already exists stands.
        if entry.has_value(IS_PROMOTED) {
            continue;
        }
        // Every matching entry, not just the first: the shell keeps one per icon UID, and a stale
        // entry from an earlier run would otherwise be the one left in the flyout.
        if let Err(e) = entry.set_dword(IS_PROMOTED, 1) {
            return Promotion::Failed(e);
        }
        promoted += 1;
    }

    if promoted > 0 {
        Promotion::Promoted(promoted)
    } else if matched > 0 {
        Promotion::AlreadyDecided
    } else {
        Promotion::NotListedYet
    }
}

/// Whether a recorded `ExecutablePath` names `exe`.
///
/// Two forms, both seen in a real `NotifyIconSettings`. Most entries hold a literal path — including,
/// usefully, everything under `%LOCALAPPDATA%\Programs`, which is where this app installs. Programs
/// under a handful of shell folders instead get a `{KNOWNFOLDERID}\rest\of\path` token, so
/// `Program Files` appears as `{6D809377-6AF0-444B-8957-A3773F02200E}\…`.
///
/// The token form is resolved by matching the tail rather than by looking the GUID up, which needs no
/// `SHGetKnownFolderPath` and no table of GUIDs to keep current. The trade is that a program of the
/// same name in a different root would also match — reachable only for token-form entries, and the
/// worst case is promoting an icon that was going to be visible anyway, so it is not worth a
/// known-folder lookup to avoid.
///
/// Folded with `to_lowercase` rather than `eq_ignore_ascii_case`, because Windows paths are
/// case-insensitive past ASCII too and a user name with an umlaut in it is not exotic.
#[cfg(target_os = "windows")]
fn refers_to(recorded: &str, exe: &Path) -> bool {
    let exe = exe.display().to_string().to_lowercase();
    let recorded = recorded.to_lowercase();

    if recorded == exe {
        return true;
    }
    // `{GUID}\rest\of\path` becomes `rest\of\path`, matched as a tail on a separator boundary so that
    // a recorded `…\bar.exe` cannot be satisfied by an actual `…\foobar.exe`.
    match recorded.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
        Some((_, tail)) => {
            let tail = tail.trim_start_matches('\\');
            !tail.is_empty() && exe.ends_with(&format!("\\{tail}"))
        }
        None => false,
    }
}

// ─── macOS ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn enable(exe: &Path) -> Result<(), String> {
    let dir = launch_agents_dir()?;
    write_launch_agent(&dir, exe)
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
    let path = dir.join(format!("{}.plist", launch_agent_label()));
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

/// The entry's file name. Named after the binary rather than after [`ENTRY_NAME`], because this is the
/// one place the convention is a file name in a shared directory and every other entry in it is named
/// after its program.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DESKTOP_FILE: &str = "githoot-tray.desktop";

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
/// new argument, so `/opt/git hoot/githoot-tray` would be launched as `/opt/git` with `hoot/...` as its
/// first argument. Inside those quotes the desktop entry spec reserves `"`, `` ` ``, `$` and `\`, each
/// escaped with a backslash — and the backslash goes first, or the escapes the others add would then be
/// escaped in turn.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn exec_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;

    /// Removes a test value however the test ends, so a panicking assertion cannot leave a stray
    /// startup entry behind in the registry of whoever is running the tests.
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(key) = Key::open(RUN_KEY, winapi::um::winnt::KEY_SET_VALUE) {
                let _ = key.delete_value(&self.0);
            }
        }
    }

    /// A value name no real install could collide with, so these tests can use the *real* `Run` key —
    /// which is the point, since a stand-in would prove nothing about the registry.
    fn test_value_name(suffix: &str) -> String {
        format!("{ENTRY_NAME}-selftest-{suffix}-{}", std::process::id())
    }

    fn run_key() -> Key {
        use winapi::um::winnt::{KEY_QUERY_VALUE, KEY_SET_VALUE};
        Key::open(RUN_KEY, KEY_SET_VALUE | KEY_QUERY_VALUE)
            .expect("HKCU Run must be openable without elevation")
    }

    #[test]
    fn the_run_command_is_quoted_so_a_path_with_spaces_survives() {
        let command = run_command(Path::new(r"C:\Program Files\GitHoot\githoot-tray.exe"));
        assert_eq!(command, "\"C:\\Program Files\\GitHoot\\githoot-tray.exe\"");
    }

    /// The real thing: `HKCU\…\Run` is writable without elevation, so this exercises the actual
    /// registry rather than a stand-in. Under a test-only value name, because the point is to prove
    /// the operations agree with each other, not to register anything.
    #[test]
    fn a_string_value_round_trips_through_the_registry() {
        let name = test_value_name("string");
        let _guard = Cleanup(name.clone());
        let key = run_key();

        let command = run_command(Path::new(r"C:\Program Files\GitHoot\githoot-tray.exe"));
        key.set_string(&name, &command).expect("HKCU Run must be writable without elevation");

        let read_back = key.string(&name).expect("the value just written must read back");
        assert_eq!(read_back, command, "what comes out must be what went in");
        assert!(key.has_value(&name), "a written value must report as present");

        key.delete_value(&name).expect("a value this test wrote must be removable");
        assert_eq!(key.string(&name), None, "a removed value must read back as absent");
        assert!(!key.has_value(&name), "a removed value must not report as present");
    }

    /// `has_value` is the whole basis of the icon promotion's restraint — a decision that exists is
    /// never overwritten — so it has to notice a value of a type `string` deliberately refuses to
    /// return. A `DWORD` read as a string is `None`, but it is emphatically *present*.
    #[test]
    fn a_dword_is_present_even_though_it_is_not_a_string() {
        let name = test_value_name("dword");
        let _guard = Cleanup(name.clone());
        let key = run_key();

        key.set_dword(&name, 1).expect("HKCU Run must be writable without elevation");

        assert!(key.has_value(&name), "a DWORD that was just written must report as present");
        assert_eq!(key.string(&name), None, "a DWORD is not a string and must not be read as one");

        key.delete_value(&name).expect("a value this test wrote must be removable");
        assert!(!key.has_value(&name));
    }

    /// Absent is not an error: it is the ordinary state, and the read has to say so plainly rather
    /// than by failing, or nothing could tell deletion from a broken read.
    #[test]
    fn a_value_that_was_never_written_reads_back_as_absent() {
        let key = run_key();
        let name = test_value_name("never-written");
        assert_eq!(key.string(&name), None);
        assert!(!key.has_value(&name));
    }

    /// `NotifyIconSettings` is enumerated to find our own entry among everything else on the machine,
    /// so the walk has to actually return names. Read-only, and asserted loosely: what is in there is
    /// whatever this machine happens to run.
    #[test]
    fn subkeys_enumerates_the_run_keys_neighbours() {
        let key = Key::open(r"Software\Microsoft\Windows\CurrentVersion", winapi::um::winnt::KEY_READ)
            .expect("a stock Windows profile always has this key");
        let names = key.subkeys();
        assert!(names.len() > 1, "expected several subkeys, got {names:?}");
        assert!(names.iter().any(|n| n == "Run"), "Run must be among them, got {names:?}");
        assert!(names.iter().all(|n| !n.is_empty()), "no name may come back empty: {names:?}");
        assert!(
            names.iter().all(|n| !n.contains('\0')),
            "a name must be cut at its terminator, not padded with NULs: {names:?}"
        );
    }

    #[test]
    fn a_literal_executable_path_matches_whatever_its_case() {
        let exe = Path::new(r"C:\Users\me\AppData\Local\Programs\GitHoot Tray\githoot-tray.exe");
        assert!(refers_to(r"C:\Users\me\AppData\Local\Programs\GitHoot Tray\githoot-tray.exe", exe));
        assert!(refers_to(r"c:\users\me\appdata\local\programs\githoot tray\GITHOOT-TRAY.EXE", exe));
    }

    /// The form Windows uses for anything under a shell folder: `{FOLDERID_ProgramFilesX64}\…`.
    #[test]
    fn a_known_folder_token_matches_by_its_tail() {
        let exe = Path::new(r"C:\Program Files\GitHoot\githoot-tray.exe");
        assert!(refers_to(r"{6D809377-6AF0-444B-8957-A3773F02200E}\GitHoot\githoot-tray.exe", exe));
    }

    /// The tail has to match on a separator, or `…\tray.exe` would be satisfied by `…\githoot-tray.exe`
    /// and this app would promote a different program's icon.
    #[test]
    fn a_tail_that_is_not_on_a_separator_boundary_does_not_match() {
        let exe = Path::new(r"C:\Program Files\GitHoot\githoot-tray.exe");
        assert!(!refers_to(r"{6D809377-6AF0-444B-8957-A3773F02200E}\GitHoot\tray.exe", exe));
    }

    #[test]
    fn another_program_does_not_match() {
        let exe = Path::new(r"C:\Program Files\GitHoot\githoot-tray.exe");
        assert!(!refers_to(r"C:\Program Files\Docker\Docker Desktop.exe", exe));
        assert!(!refers_to(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Docker\Docker Desktop.exe", exe));
        // A token with nothing after it names no program at all and must never match everything.
        assert!(!refers_to(r"{6D809377-6AF0-444B-8957-A3773F02200E}", exe));
        assert!(!refers_to(r"{6D809377-6AF0-444B-8957-A3773F02200E}\", exe));
        assert!(!refers_to("", exe));
    }

    /// A sweep against a real `NotifyIconSettings` for an executable that is certainly not in it. It
    /// must come back `NotListedYet` — the outcome that makes the caller wait and try again — and
    /// must not write anything. `Failed` here would mean Windows 10 or a key that has moved, which is
    /// worth knowing about too, so it is allowed for and named.
    #[test]
    fn a_sweep_for_an_unlisted_executable_writes_nothing() {
        let nowhere = Path::new(r"C:\definitely\not\a\tray\app\nothing-here.exe");
        match promote_once(nowhere) {
            Promotion::NotListedYet => {}
            Promotion::Failed(e) => {
                // Windows 10 and older have no such key at all. Not a failure of this code.
                assert!(e.contains(NOTIFY_ICON_SETTINGS), "unexpected failure: {e}");
            }
            other => panic!("an executable with no tray icon must not be promoted: {other:?}"),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn the_launch_agent_runs_the_binary_at_load() {
        let plist = launch_agent(Path::new("/Applications/GitHoot.app/Contents/MacOS/githoot-tray"));
        assert!(plist.starts_with("<?xml"), "must be a plist, got {plist:?}");
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"), "got {plist:?}");
        assert!(
            plist.contains("/Applications/GitHoot.app/Contents/MacOS/githoot-tray"),
            "must name the binary, got {plist:?}"
        );
        assert!(plist.contains(ENTRY_NAME), "must carry the shared entry name, got {plist:?}");
    }

    /// `&` is legal in a macOS path and illegal raw in XML, so an unescaped path would produce a plist
    /// `launchd` refuses to load — silently, since nothing in this app reads it back.
    #[test]
    fn a_path_with_xml_significant_characters_is_escaped() {
        let plist = launch_agent(Path::new("/Users/me/R&D/<app>/githoot-tray"));
        assert!(plist.contains("R&amp;D/&lt;app&gt;"), "got {plist:?}");
        assert!(!plist.contains("R&D"), "the raw ampersand must not survive, got {plist:?}");
    }

    #[test]
    fn the_launch_agent_is_written_into_the_directory_it_is_given() {
        let dir = std::env::temp_dir().join(format!("githoot-launchagent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let exe = Path::new("/Applications/GitHoot.app/Contents/MacOS/githoot-tray");
        write_launch_agent(&dir, exe).expect("must create the directory and the plist");

        let written = std::fs::read_to_string(dir.join(format!("com.githoot.{ENTRY_NAME}.plist")))
            .expect("the plist must be where the loader looks for it");
        assert_eq!(written, launch_agent(exe));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, not(any(target_os = "windows", target_os = "macos"))))]
mod unix_tests {
    use super::*;

    #[test]
    fn the_desktop_entry_is_an_autostart_application_that_runs_the_binary() {
        let entry = desktop_entry(Path::new("/usr/local/bin/githoot-tray"));
        assert!(entry.starts_with("[Desktop Entry]"), "got {entry:?}");
        assert!(entry.contains("Type=Application"), "got {entry:?}");
        assert!(entry.contains("Exec=\"/usr/local/bin/githoot-tray\""), "got {entry:?}");
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
        let entry = desktop_entry(Path::new("/opt/git hoot/githoot-tray"));
        assert!(entry.contains("Exec=\"/opt/git hoot/githoot-tray\""), "got {entry:?}");

        let odd = desktop_entry(Path::new(r#"/opt/we"ird\path/githoot-tray"#));
        assert!(odd.contains(r#"Exec="/opt/we\"ird\\path/githoot-tray""#), "got {odd:?}");
    }

    #[test]
    fn the_desktop_entry_is_written_into_the_directory_it_is_given() {
        let dir = std::env::temp_dir().join(format!("githoot-autostart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let exe = Path::new("/usr/local/bin/githoot-tray");
        write_desktop_entry(&dir, exe).expect("must create the directory and the entry");

        let written = std::fs::read_to_string(dir.join("githoot-tray.desktop"))
            .expect("the entry must be where the session looks for it");
        assert_eq!(written, desktop_entry(exe));

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
