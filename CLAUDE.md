# Project instructions

## Git workflow

**Commit directly to `main`. Do not create a branch.**

This overrides the usual "if HEAD is on the default branch, branch first" habit, and it overrides the
branch-naming rules in the `commit-and-pr` skill. Only create a branch when I ask for one in that
message, in so many words — a branch is never the right default here, and "this change looks big" is
not a reason to make one.

Everything else in the `commit-and-pr` house style still applies to the commit itself:

- Uppercase imperative subject, no conventional-commit prefix, no trailing period, aim under 72 chars.
- Subject only, no body. Reasoning belongs in a PR description, not in `git log`.
- No issue or ticket references in the commit.

Commit only when I ask. Push only when I ask.

## Releasing

The tag is the source of truth for a release, not `Cargo.toml` — CI injects it as `GHT_VERSION`. The
ritual, as the history shows it:

1. Land the change.
2. A separate `Bump to X.Y.Z` commit touching only `Cargo.toml` and `Cargo.lock`.
3. Push `main` and **wait for CI to pass on all three platforms.**
4. Only then push the `vX.Y.Z` tag, which triggers `release.yml`.

Step 3 is not optional. Published releases are immutable — assets and the tag cannot be changed or
deleted, and a release cannot be re-cut — so a tag pushed onto a red commit is a burnt version number.
Fix forward with a new patch tag. See `docs/updates-and-releases.md`.
