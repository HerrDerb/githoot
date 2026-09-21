# Portals

GitHoot watches **portals**: the code forges that hold your pull requests. Today there is one, GitHub,
and a GitHub-only install looks and behaves exactly as it did before this seam existed. The seam is
there so GitLab and Bitbucket Cloud can arrive as one new directory each, not as edits across the app.

## The shape

Everything that talks to a forge lives under `src/portal/`. Everything else — the state machine, the
hoot ledger, the tray wording, the PR page, the poll loop — speaks one vocabulary and one trait:

- `portal::types` is the vocabulary: a pull request as the page shows it (`PrEntry`), and every
  outcome a poll can have (`PollResult`). Nothing in it names a forge. Where a field is GitHub-shaped
  by birth (a node id, `owner/name`) its doc comment says what it *means*, so another portal can fill
  it honestly or leave it at its "no evidence" value.
- `portal::Portal` is the trait. One object per configured portal, owned by the poll thread. It loads
  or obtains a credential, renews it, answers a poll for whichever of the three axes are in play, and
  says how it is doing when it publishes a status page. `PortalInfo` is what the rest of the app needs
  to know without holding the portal: its name, which URLs may become links, where its inbox is, what
  it is capable of, and how slowly it wants to be asked.
- `portal::github` is the first adapter: the GraphQL client, the GitHub App device flow, the three
  Search queries and the rule that judges each axis. `portal::statuspage` reads an Atlassian
  Statuspage, which GitHub, GitLab and Bitbucket all run, so it takes a base URL rather than knowing
  one.
- `portal::fake` is a test double that imports nothing from `portal::github`. It compiles only if
  the trait can be implemented without a single GitHub type, which is the whole promise of the seam.

## What the seam was shaped by

The trait was cut to fit the portals that are not here yet, because a seam cut to fit one side is
not a seam:

- **One call answers every axis.** GitLab returns reviews requested, authored-and-approved and
  authored-and-blocked from a single GraphQL document. So `poll` takes the axes wanted and answers all
  of them at once; GitHub issues three requests inside its own `poll`, as it always did. An axis the
  user switched off must cost no request, and that decision is the loop's, not the adapter's.
- **Auth styles differ.** GitHub and GitLab (17.9 and later) hand out tokens through a device flow
  started from the menu. Bitbucket Cloud has neither a device flow nor PKCE, so its user pastes an API
  token they made on the site. Both are "get me a credential", so both are methods on the portal, and
  the difference is declared as a capability for the menu's wording.
- **Not every portal knows everything.** Bitbucket Cloud cannot say whether a branch conflicts, has no
  notion of a re-review being pending, and has no team reviewers. `Capabilities` says so, and an honest
  adapter leaves the field it cannot fill at its "no evidence" value rather than guessing. The
  automatic reviewer is a capability too: `PrEntry::bot_review` names the bot itself ("Copilot"), so
  the page words its pill without knowing which portal a card came from.
- **Rate budgets differ.** Bitbucket Cloud allows a thousand requests an hour, and its
  reviews-requested list costs one request per repository. Each portal states the floor it wants to be
  polled at, and the loop paces to the slowest.

## Several portals at once

The design is for several portals live in one tray, and the code is written that way; only one is
configured today.

- **State is per portal.** Each portal has its own `PollState`, with its own three tracks and its own
  hoot ledgers. The same pull-request id on two portals is two pull requests, and neither silences the
  other. Nothing about [the hoot](hoot.md) changes.
- **The icon, tooltip and menu are merged**, in `overview`. A dot is lit if any portal lights it; an
  axis is unknown only when no portal confirms it; the exclamation flags are "any"; the menu counts
  are summed over confirmed lists. With several portals every tooltip line is prefixed with its
  portal's name and the length cap is applied once, to the whole text. With one portal every one of
  those rules is the identity, and golden tests hold the output byte for byte to what it was before.
- **The page groups by portal.** Each portal's cards sit under a heading with their own empty state and
  their own inbox link, because "nothing here" on GitLab says nothing about GitHub. Only URLs under a
  portal's own `link_prefix` become links, so a GitLab URL under the GitHub group is text. One portal
  renders with no heading, as the page always did.
- **The available update is app-level**, not any portal's, and joins the tooltip and the menu from
  the overview rather than from a state.

## Configuration

Every existing `config.txt` describes one GitHub portal without naming it, and keeps working
unchanged. Naming portals in the file is reserved for the release that ships a second kind:

```
portal.<name>.type=github|gitlab|bitbucket
portal.<name>.url=https://...
portal.<name>.enabled=on
portal.<name>.interval=120
portal.<name>.clientId=...
```

The rule for that day is already fixed: the moment any `portal.` key is present, the implicit GitHub
portal disappears, so an old file and a new file never describe two different models at once. The
flat `key=value` parser reads dotted keys as they are.

**GitHub Enterprise Server is not shipped yet, on purpose.** The adapter derives every endpoint from a
base URL and is tested against a GHES-shaped one, but a GHES instance also needs its own GitHub App
registered on it, so the device flow can ask it for a token. A `url` key without a `clientId` key
would be half a feature that fails at sign-in, so both arrive together or not at all.

`copilotReviews` and `statusComponents` stay as they are: GitHub-specific settings, read into the
GitHub adapter's options.

## What a second adapter has to bring

For GitLab (gitlab.com and self-hosted, same API):

- One GraphQL query on `currentUser.reviewRequestedMergeRequests` and `authoredMergeRequests` with
  reviewer states (`UNREVIEWED`, `REVIEW_STARTED`, `REQUESTED_CHANGES`, `APPROVED`), `headPipeline`,
  `conflicts` or `detailedMergeStatus`, and `draft`.
- Device flow, generally available since 17.9; tokens live two hours with a rotating refresh token, so
  `needs_refresh` will be true often and `reauthenticate` has to be cheap.
- No conditional requests: GitLab documents no `ETag` on its APIs.
- Self-hosted is the base URL plus an OAuth application registered on that instance.
- To-dos stand in for a notification count, should that ever come back.

For Bitbucket Cloud:

- REST only. There is no cross-repository "reviews requested of me" endpoint; the adapter iterates a
  configured repository list and filters on `reviewers.uuid`. Participants carry `approved` or
  `changes_requested`; CI comes from per-pull-request statuses; drafts exist. There is no conflict
  field, only an undocumented diffstat marker, so `conflict_state` is `false`.
- Authentication is a user-created API token (app passwords stop working in 2026). No device flow, no
  PKCE. `AuthStyle::PastedToken`.
- A thousand requests an hour, so a slower `min_poll_interval` and a cached repository list.
- Bitbucket Data Center is a materially different API and would be its own adapter, not a base URL.

Everything above about GitLab and Bitbucket was checked against their current documentation when the
seam was designed, and is a starting point for the adapter, not a substitute for reading the docs
again when it is written.
