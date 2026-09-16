# Updating and releases

## Updating

Checked daily and at startup. When a newer release exists, a green up-arrow appears top-left and an
**Install update** entry appears in the menu. Picking it shows the release notes of **every** version
between yours and the newest, then asks. Nothing is ever installed silently. On confirmation it downloads,
verifies, replaces itself and restarts. Turn it off with `updateCheck=off`.

## What is verified, and what that proves

Each release publishes `sha256sums.txt` and a detached `sha256sums.txt.minisig`. Before installing:

1. The signature on the digest file is checked against a public key **compiled into the copy already on
   your machine**.
2. The asset's SHA-256 must match its entry in that signed file.
3. It must look like the right kind of artefact, at a plausible size.
4. It is run with `--print-version` and must report the version expected.

**Only step 1 proves authenticity.** The rest catch corruption and wrong-artefact mistakes. The transport
alone is not enough: TLS trust comes from the system certificate store, so a corporate TLS-inspecting
proxy or any locally installed root CA could serve arbitrary bytes, and release assets are mutable. A
checksum fetched over that same channel would be replaced along with the asset. Only a key that was
*already installed* breaks that circularity.

Any failure refuses the update and leaves the running version untouched. The app never asks for elevation:
if it lives somewhere you do not own, it says so and stops rather than escalating to write unverified
bytes.

## If a restart is interrupted

On Linux the swap is one atomic `rename`, so the binary is never missing. On Windows and macOS it is
necessarily two renames, and a crash between them leaves `githoot-tray.exe.old` (or `.app.old`) with
nothing at the original name. The app cannot repair that — the repairer is the missing file. Rename the
`.old` back. The recovery line is logged *before* the swap starts, so it is there to find.

## Command-line flags

A **compatibility contract**: the updater in one release drives the next through these, so they cannot be
renamed without breaking updates from every release already published.

| Flag | Does |
|---|---|
| `--print-version` | Prints the version and exits |
| `--await-exit <pid>` | Waits for that process to exit before starting (Windows in effect) |

## Version numbers

The binary reports the git tag it was built from, passed in by CI as `GHT_VERSION`, not `Cargo.toml`'s
version. A local build has no tag and falls back to `Cargo.toml`, so local and release builds of the same
commit can report different versions. Deliberate: a local build is not a release.


---

## Releases

Download from the [Releases](https://github.com/HerrDerb/githoot-tray/releases) page.

| Platform | Asset |
|---|---|
| Windows x86-64 | `githoot-tray.exe` |
| Linux x86-64 | `githoot-tray` |
| macOS Apple Silicon | `githoot-tray-macos-aarch64.zip` |
| All | `sha256sums.txt`, `sha256sums.txt.minisig` |

Only those three are built. On anything else — Arm Linux, an Intel Mac — the updater reports that there is
no build for your platform rather than downloading something that will not run. `cargo build --release`
works there if you build it yourself.

## macOS

On macOS the bare binary takes a Dock icon and an app menu, because `LSUIElement` needs a bundle. The
release workflow runs the same script you can run locally, so local and released bundles are identical:

```bash
scripts/bundle-macos.sh target/release/githoot-tray dist 1.6.0
open dist/githoot-tray.app
```

The bundle is ad-hoc signed, not notarized (that needs a paid Apple account), so Gatekeeper quarantines a
download. Clear it once:

```bash
unzip githoot-tray-macos-aarch64.zip
xattr -dr com.apple.quarantine githoot-tray.app
open githoot-tray.app
```

The icon then appears in the menu bar with no Dock icon. It keeps its colour rather than using a macOS
template image, which is drawn monochrome and would erase every bar.

## Before you tag

A published release cannot be re-cut, so the tag only ever goes onto a commit CI has already passed
on all three platforms. Two checks are worth running before the push that triggers it, because both
catch things a plain `cargo test` on Linux cannot:

```
cargo build --target x86_64-pc-windows-msvc --bin githoot-tray
```

**`cargo build`, not `cargo check`.** Roughly a third of this app is behind `#[cfg(windows)]` or
`#[cfg(macos)]`, so a Linux build compiles none of it — but `check` does not run codegen either, and
`unconditional_panic` (an out-of-bounds index the compiler can prove) is a *codegen* lint. It will
report the error and then fail on the missing `link.exe`, which is expected and harmless: the
compile errors come first. Everything Windows-specific except linking is covered.

macOS has no equivalent here, so its `#[cfg]` arms are only ever proved by CI.

## Immutability

Published releases are frozen: assets and the Git tag cannot be changed or deleted, and GitHub generates a
signed attestation. So the workflow creates a **draft**, attaches everything, and publishes last. A
release therefore **cannot be re-cut** — re-running against a published tag is refused rather than
half-succeeding and leaving it without its signed manifest. Fix forward with a new patch tag.

Attestations do not replace the minisign signature: an attestation is verified against GitHub over the
network, while the updater verifies offline against a compiled-in key. That offline property is the point.

## Signing keys, for maintainers

Already set up; this is for rotating. `rsign2` rather than `minisign` itself — no `sudo` needed, and one
tool for both generating and signing removes any question about a passwordless key round-tripping.

```bash
cargo install rsign2
rsign generate -W -p minisign.pub -s minisign.key
gh secret set MINISIGN_SECRET_KEY < minisign.key    # piped, so it is never printed
```

`-W` makes it passwordless: it lives in an Actions secret, which is the real protection. `-W` is needed on
**signing** too, or `rsign` prompts and fails on a runner with no terminal.

Then copy the public key line from `minisign.pub` — the second line, not the `untrusted comment:` one —
into `PUBLIC_KEY` in `src/update.rs` and the verify step in `release.yml`. `minisign.pub` is committed;
`minisign.key` is gitignored and must stay so.

Until that is done the constant holds a placeholder and every install is refused with "this build has no
update signing key compiled in" — visible rather than silent. `the_compiled_in_key_parses` catches a bad
paste at test time.

Rotating means shipping a release signed with the *old* key that carries the *new* one, since users can
only verify with the key they already have. The signature tests use a throwaway fixture key, so they
survive a rotation.
