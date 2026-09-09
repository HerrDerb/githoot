# PR status

Three independent signals, each a GitHub Search query against your own pull requests:

| Bar | Query | Then |
|---|---|---|
| 🔴 | `is:pr review-requested:@me state:open draft:false archived:false -label:dependencies -author:app/dependabot -author:app/renovate` | counted as is |
| 🟢 | `is:pr author:@me state:open draft:false archived:false` | a hit counts when its latest reviews hold an `APPROVED` and no `CHANGES_REQUESTED` |
| 🟠 | `is:pr author:@me review:changes_requested state:open archived:false` | a hit counts while no re-review is pending from the reviewer who asked |

**The green bar reads the reviews, not `review:approved`.** That qualifier looks like "somebody approved
this", and it is not. It is a projection of `reviewDecision`, GitHub's verdict on whether the base branch's
*review policy* is satisfied, and a repository that requires no reviews gets no verdict: `reviewDecision`
is `null` on every one of its pull requests, approved or not, and `review:approved` matches none of them.
Measured on a repository with no required-review rule: 100 of its last 100 PRs reported `null`, six of them
carrying a real `APPROVED` review on the head commit. The PR page still showed its green **Approved**
badge, because the page reads the reviews. So the bar does too, over GraphQL: the query above narrows to
your open PRs, and each hit is judged by its `latestOpinionatedReviews`, one verdict per reviewer.

One approver's yes does not outrank another's no. A PR with an `APPROVED` and a standing
`CHANGES_REQUESTED` counts as changes requested, not as approved, which is GitHub's own precedence and what
keeps the two bars from lighting for the same PR. An approval on a commit you have since pushed over still
counts: the PR page shows it, and this bar reports news rather than gating a merge.

**The green bar means approved, not mergeable.** It used to mean both: through 1.10.0 the bar counted an
approved pull request only when its GraphQL `statusCheckRollup` was `SUCCESS`, so one red or still-running
check darkened it. That hid the thing worth being told — somebody approved your work — and it also
disagreed with the entry beside it. Approval is the whole signal now. Whether CI is green is a question you
answer on the page the entry opens.

Still not checked, and never was: branch protection needing multiple approvals or named reviewers, and
merge conflicts (`mergeable` is computed lazily and reads `UNKNOWN` on a cold poll). Also unchanged: the
GraphQL page reads at most 100 open PRs of yours, so past that the bar undercounts.

Two GitHub App permissions are now unused: **Checks: read** and **Commit statuses: read**, which the
rollup needed. They are still requested rather than withdrawn, because narrowing an installed App's
permissions makes every installation owner re-approve — a real cost to pay for tidiness.

**Changes requested does one thing more than its query.** Re-requesting a review does not dismiss the
reviewer's earlier verdict, so `review:changes_requested` keeps matching a pull request you have already
handed back, and the bar used to stay lit until the reviewer replied. That query is now sent through
GraphQL, which also returns each pull request's pending review requests, and a hit is counted only when
**no re-review is pending from a reviewer who requested changes**. The same GraphQL answer carries each
counted PR's URL, which is what the menu entry opens. Adding a *different* reviewer does not
clear it — the original objection still stands. A pending *team* request does not clear it either, since a
team has no login to match against the blocker; erring that way keeps the bar lit rather than hiding work.

Caps, which undercount rather than overcount: the first 100 matching pull requests, and the first 20
reviews and 20 pending requests within each.

## Why a GitHub App

All three queries need to see PRs in private repos. The only *classic* OAuth scope that can is `repo`,
which also grants **write** to everything you can reach. Rather than hand out a key that broad, PR status
uses one shared fine-grained **GitHub App** with read-only Pull requests, Metadata, Checks and Commit
statuses permissions. The last two are now unused — the green bar no longer reads check health — and are
kept only to spare every installation owner a re-approval; see the note above. **Contents is deliberately
not requested**, which is why no query may traverse a commit object. Device Flow needs no client secret,
so its Client ID is public and baked in rather than configured.

Access to a private org is granted by **installing** the App there, not by a wider scope — an org owner
approves once and every member benefits.

If you have authorized but see nothing, check whether it is installed anywhere:
[github.com/settings/installations](https://github.com/settings/installations). A confident "nothing to
show" from a zero-installation App would be indistinguishable from genuinely having nothing, so the app
checks once at startup and turns the bars off with an explanation instead of guessing.

The credential renews itself — proactively before expiry, and reactively if GitHub rejects it. Only when a
renewal needs a browser does the exclamation come back.
