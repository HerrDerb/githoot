# PR status

Three independent signals, each a GitHub Search query against your own pull requests:

| Bar | Query | Then |
|---|---|---|
| 🔴 | `is:pr review-requested:@me state:open draft:false archived:false -label:dependencies -author:app/dependabot -author:app/renovate` | counted as is |
| 🟢 | `is:pr author:@me state:open draft:false archived:false` | a hit counts when its latest reviews hold an `APPROVED` and no `CHANGES_REQUESTED` |
| 🟠 | `is:pr author:@me state:open draft:false archived:false` | a hit counts when a reviewer's objection still stands, when it has a merge conflict and someone is waiting on it, **or** when Copilot has unresolved comments on it |

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
disagreed with the entry beside it. Approval is the whole signal now. Whether CI is green is shown beside
each pull request on [the page](pr-page.md) the entry opens, where it informs rather than hides.

Still not checked: branch protection needing multiple approvals or named reviewers. Merge conflicts used
to be on that list, for the lazy-computation reason below; the amber bar reads them now. Also unchanged:
the GraphQL page reads at most 100 open PRs of yours, so past that the bar undercounts.

**Checks: read** and **Commit statuses: read** were unused for several releases, after the green bar
stopped gating on CI. They were kept rather than withdrawn, because narrowing an installed App's
permissions makes every installation owner re-approve. That turned out well: the PR page reads the check
rollup again, at no re-approval cost to anyone.

**The amber bar means work required from you, and counts two things.**

*A reviewer's objection still standing.* Re-requesting a review does not dismiss the earlier verdict, so
`review:changes_requested` keeps matching a pull request you have already handed back, and the bar used to
stay lit until the reviewer replied. A hit counts only when **no re-review is pending from a reviewer who
requested changes**. Adding a *different* reviewer does not clear it — the original objection still
stands. A pending *team* request does not clear it either, since a team has no login to match against the
blocker; erring that way keeps the bar lit rather than hiding work.

*A merge conflict with someone waiting.* GitHub's `mergeable` says `CONFLICTING`, and the pull request has
a reviewer attached — either a pending request or a standing review. A conflict blocks the reviewer as
surely as their own objection does, and it is yours to fix. A conflict on a pull request **nobody is
attached to** blocks nobody and does not count.

*Unresolved Copilot comments.* **Copilot never approves and never requests changes — it only
comments**, so `latestOpinionatedReviews` drops its review entirely and the objection rule above cannot
see it at all. Its verdict is the review threads it leaves open, so an open thread started by
`copilot-pull-request-reviewer` counts as work.

Resolved threads do not count, and neither do **outdated** ones — a thread pointing at a line you have
since changed, which GitHub's own UI hides. The cost is that pushing something unrelated can outdate a
comment you never read and the bar goes quiet on it; erring quiet is the direction taken everywhere
else here. Only the thread's *first* comment is read, so a human replying to Copilot does not make the
thread theirs. Capped at the first 20 threads per pull request, undercounting like every other cap.

Switch it off with `copilotReviews=off` if your team reads Copilot as suggestions rather than work. That
also stops the two axes asking GitHub for review threads at all, which is the most expensive part of the
query — the other axis never asks, since Copilot's comments are the PR author's problem, not its
reviewer's.

**Only a definite `CONFLICTING` counts.** GitHub computes `mergeable` lazily and answers `UNKNOWN` until
something asks; asking is what starts the computation, so a cold poll sees `UNKNOWN` and the next cycle
sees the answer. Treating `UNKNOWN` as a conflict would light the bar on every freshly pushed pull request
and clear it a minute later. Undercounting for one cycle is the cheaper error.

**Anything that makes it work takes the pull request off the green bar.** The two never light for the
same one: a conflict or an open Copilot thread is work before it is news, and the approval is still there
to be told about the moment you deal with it.

**The query stopped filtering.** It was `is:pr author:@me review:changes_requested …`, which would now
hide every conflicted pull request that carries no objection, and `mergeable` is not a search qualifier.
So it asks for every open pull request of yours and decides here. `draft:false` came with that: a draft is
work you already know is unfinished, so neither half is news on one. Before this, a draft carrying a
changes-requested review did count.

Caps, which undercount rather than overcount: the first 100 matching pull requests, and the first 20
reviews and 20 pending requests within each.

**All three bars now read the same GraphQL document.** The red bar was a plain `total_count` off REST
Search until 1.17.0 — an exact, uncapped number with no members. The PR page has to *name* the pull
requests behind a bar, which a total never could, so it fetches the hits instead. The query string is
handed to GraphQL unchanged, so the count does not move, except that the red bar now shares the 100-hit
cap above: past 100 pull requests awaiting your review, the extras are not seen. Undercounting rather
than overcounting, and unreachable by the inbox this app exists for.

**The check rollup is back, for display only.** Each hit's `statusCheckRollup` is read again so the PR
page can show whether CI is green. It decides nothing: the green bar still means approved, not
mergeable. A repository with no checks answers `null`, and a token refused the field degrades to the
same value, so both render as *Checks unknown* rather than as a failure — and a refusal on that one
field costs the field, not the poll. That distinction matters: under the old rule any error failed the
whole poll, which is what left this axis frozen from 1.7.0 to 1.10.0.

The rollup is read **off the pull request**, never through `commits(last:1)`. Resolving a
`PullRequestCommit` needs **Contents: read** — read access to every line of source in every installed
repo, to power a tray icon — and this App deliberately does not request it. That detour is the bug
above, and a guard test fails if the document ever walks through `commits` again.

## Why a GitHub App

All three queries need to see PRs in private repos. The only *classic* OAuth scope that can is `repo`,
which also grants **write** to everything you can reach. Rather than hand out a key that broad, PR status
uses one shared fine-grained **GitHub App** with read-only Pull requests, Metadata, Checks and Commit
statuses permissions. The last two read the check rollup the PR page shows; see the note above for the
stretch where they were held but unused. **Contents is deliberately not requested**, which is why no
query may traverse a commit object. Device Flow needs no client secret,
so its Client ID is public and baked in rather than configured.

Access to a private org is granted by **installing** the App there, not by a wider scope — an org owner
approves once and every member benefits.

If you have authorized but see nothing, check whether it is installed anywhere:
[github.com/settings/installations](https://github.com/settings/installations). A confident "nothing to
show" from a zero-installation App would be indistinguishable from genuinely having nothing, so the app
checks once at startup and turns the bars off with an explanation instead of guessing.

The credential renews itself — proactively before expiry, and reactively if GitHub rejects it. Only when a
renewal needs a browser does the exclamation come back.
