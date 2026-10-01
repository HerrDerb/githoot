# Portals

GitHoot watches **portals**: the code forges that hold your pull requests. There are two, GitHub and
GitLab, and a GitHub-only install looks and behaves exactly as it did before this seam existed. GitLab
arrived as one new directory plus a config section, which was the test the seam was built to pass;
what it taught the seam is under [What GitLab changed](#what-gitlab-changed).

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
- `portal::github` is the first adapter: the GraphQL client, the GitHub App's client id and
  installations check, the three Search queries and the rule that judges each axis.
- `portal::gitlab` is the second: one GraphQL document for all three axes, and GitLab's own review
  rules (see below).
- `portal::oauth` is the device flow (RFC 8628) both adapters sign in with: the credential file, the
  polling loop and the refresh grant. A portal hands it its endpoints, client id, scope and file.
- **Each portal brings its own status reader**, chosen in its `health()`. Two exist:
  - `portal::statuspage` reads an Atlassian Statuspage. GitHub runs one, at githubstatus.com.
  - `portal::statusio` reads status.io, where GitLab.com's page is (status.gitlab.com). status.io's
    codes from 300 (Degraded Performance) up raise the mark; maintenance and unknown codes do not.
  A self-managed GitLab or a GitHub Enterprise instance has no public status page, so it publishes no
  status at all rather than gitlab.com's or github.com's, which would say nothing about it.
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

Several portals live in one tray. Configure two and each gets its own state, its own group on the
page, and its own sign-in.

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
unchanged. On **Settings ▸ Portals**, Sign in is install and Sign out is uninstall: Sign in on GitLab
writes the section below, builds the portal and starts its sign-in; Sign out deletes the credential,
switches the section off and stops the portal (except the last portal, which only signs out). A hand
edit to the file needs a restart. By hand, naming
portals is the same thing:

```
portal.<name>.type=github|gitlab
portal.<name>.url=https://...        # GitLab self-managed only
portal.<name>.clientId=...           # GitLab: required
portal.<name>.enabled=off            # leave it out without deleting it
```

**The moment any `portal.` key is present, the implicit GitHub portal disappears**, so an old file and a
new file never describe two different models at once. To watch GitHub and GitLab, name both. Portals
appear in name order. A section that cannot be built (no type, an unknown type, a url that is not
http(s)) is left out with a line in the log.

**gitlab.com signs in with a shipped application**, the way github.com does with the shipped GitHub App:
a non-confidential OAuth application with the `read_api` scope, whose id is public by design. Unlike the
GitHub App it needs no installing on a group: a GitLab token acts as the user, and `read_api` reads
everything they can read, repository contents included, since no narrower scope still lists merge
requests. **A self-managed instance cannot use it**, because the application is registered on
gitlab.com: register one there (User settings, Applications, non-confidential, `read_api`) and set
`clientId`. Without one that portal shows off with a reason, never a sign-in button that can only fail.
`GITLAB_APP_CLIENT_ID` overrides a missing key, for testing. Each GitLab portal keeps its credential in
`~/.githoot/<name>_token.txt`.

**GitHub Enterprise Server is not shipped yet, on purpose.** The adapter derives every endpoint from a
base URL and is tested against a GHES-shaped one, but a GHES instance also needs its own GitHub App
registered on it. A `url` on a GitHub section is ignored, with a line in the log, until `url` and
`clientId` can ship together for it.

`copilotReviews` and `statusComponents` stay as they are: GitHub-specific settings, read into the
GitHub adapter's options. The `portal.` keys are edited in `config.txt` only; the settings pages show
the portals but cannot write their sections yet.

## How GitLab is judged

One document answers all three axes: `currentUser.reviewRequestedMergeRequests` and
`authoredMergeRequests`, both `state:opened, draft:false`, with `@include` dropping a list no axis in
play needs. Measured against gitlab.com on 2026-09-28 at complexity 157 of the 250 allowed.

- **Review requested: your own reviewer state, not the list.** GitLab keeps you in `reviewers` after you
  review, where GitHub drops the request. So a merge request counts while *your* `reviewState` is
  `UNREVIEWED`, `REVIEW_STARTED` or `UNAPPROVED` (an approval you took back and have not replaced with
  a verdict). If you cannot be found among the reviewers returned, or your state comes back `null`,
  GitLab's list is trusted. Merge requests authored by a bot are left out, as Dependabot and Renovate
  are on GitHub.
- **Needs work: a `REQUESTED_CHANGES` reviewer, or a conflict with anyone attached.** Re-requesting a
  review resets that reviewer to `UNREVIEWED` and removes their approval (GitLab's
  `RequestReviewService`), so a standing `REQUESTED_CHANGES` is still on you by definition; no second
  list is needed to tell "handed back" apart.
- **Approved: anyone in `approvedBy`**, with no objection and no conflict. A reviewer's `APPROVED`
  state is deliberately not read for the count: it is a column GitLab updates on the way past, and
  approvals can be reset without it moving. `approvedBy` is the approvals themselves, and it also
  catches people who approved without being asked.
- **Checks** come from `headPipeline.status`. Cancelled and skipped read as unknown (GitLab paints them
  grey), a manual job as expected.
- **Not read:** team reviewers (GitLab has none) and an automatic reviewer. GitLab Duo reviews by
  commenting like Copilot, but its account name is not documented, and guessing would fail silently.

Not verified against a signed-in session: the device flow and the secret-less refresh for a
non-confidential application. Both need a registered application to exercise.

## What GitLab changed

Adding the second adapter was also a test of the seam. What it found:

- **The device flow was not GitHub's.** About 80% of `github/auth.rs` was RFC 8628 with GitHub's name on
  it. It is `portal::oauth` now, and each portal supplies only what differs.
- **The vocabulary still named GitHub** in one place: a rejected token was logged as "token rejected by
  GitHub" whoever rejected it.
- **"Every forge runs Atlassian Statuspage" was wrong.** GitLab's status page is status.io, which
  `portal::statusio` reads now.
- **The status menu entry opened githubstatus.com** whoever was down. It now names and opens the
  portal that is degraded (see `overview::status_menu`); two down at once share one entry that opens
  both pages.
- **Still GitHub-shaped, not yet fixed:** the menu's "Open PR inbox" fallback opens the first portal's inbox only; the settings pages write only
  `type` and `enabled` of a `portal.` section, so `url` and `clientId` are still by hand; per-portal
  `interval` is not read; and only one GitHub portal may be named, because the shipped App and its
  credential file are singular.

## What the next adapter has to bring

What was written here for GitLab before it existed, kept for comparison with what shipped above:

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
