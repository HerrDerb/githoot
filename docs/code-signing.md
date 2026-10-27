# Code signing policy

> **Applied for, not yet active.** GitHoot has applied to the SignPath Foundation's free code signing
> program for open source projects. Until it is approved and this note is gone, the Windows executable
> is not code signed.

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

## What is signed

The Windows executable, `githoot.exe`. It is built by GitHub Actions on a GitHub-hosted runner from
the tagged commit in [HerrDerb/githoot](https://github.com/HerrDerb/githoot), and every signing request
is approved by hand.

The macOS bundle is ad-hoc signed. Every release asset is also listed in a minisign-signed
`sha256sums.txt`, which the built-in updater checks before installing anything; see
[Updates and releases](updates-and-releases.md).

## Team roles

- Committers and reviewers: [HerrDerb](https://github.com/HerrDerb)
- Approvers: [HerrDerb](https://github.com/HerrDerb)

GitHoot has a single maintainer. Changes from anyone else are reviewed by the maintainer before they
are merged.

## Privacy

See the [privacy policy](privacy.md).
