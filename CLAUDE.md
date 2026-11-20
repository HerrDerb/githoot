# Project instructions

## Git workflow

**Every change goes through a pull request.** `main` is protected by the `main` ruleset: no direct
pushes, a PR is required, and it merges only when CI and the `label` check pass. The `commit-and-pr`
house style applies in full: branch names, commit messages, PR titles and descriptions.

Every PR carries **exactly one** category label, because the release notes are grouped by them
(`.github/release.yml`), and people read those notes on the Updates page before they install:

| Label | Use for |
|---|---|
| `feature` | Something new a user can do |
| `improvement` | Something existing that works better |
| `bug` | Something that did not work |
| `security` | A security fix or hardening |
| `dependencies` | A dependency update |
| `internal` | CI, refactors, tests, docs, release bumps. Left out of the notes |

Add **`breaking`** on top when the change makes something stop working or asks the user to act: a
renamed or removed setting, a changed local API or `endpoint.json`, a removed route, a new system
requirement, a changed default. Never with `internal`. Pick the label when opening the PR.

Commit only when I ask. Push only when I ask.

## Releasing

The tag is the source of truth for a release, not `Cargo.toml` — CI injects it as `GITHOOT_VERSION`. The
ritual, as the history shows it:

1. Land the changes, each through its own labelled PR.
2. A `Bump to X.Y.Z` PR, labelled `internal`, touching only `Cargo.toml` and `Cargo.lock`. Merge it.
3. **Wait for CI to pass on all three platforms** on `main`.
4. Only then push the `vX.Y.Z` tag on that commit, which triggers `release.yml`. Tags are not covered by
   the ruleset. The draft's notes are GitHub's generated ones, grouped by label; edit the draft before
   publishing if something needs saying that no PR title says.

Step 3 is not optional. Published releases are immutable — assets and the tag cannot be changed or
deleted, and a release cannot be re-cut — so a tag pushed onto a red commit is a burnt version number.
Fix forward with a new patch tag. See `docs/updates-and-releases.md`.
