# The hoot

Whenever a **pull request you have not been told about** turns up, the app plays a short hoot. Each of
the three axes hoots for itself: reviews requested of you, your PRs that were approved, your PRs needing
work.

A PR is news when it is one the axis has not seen, when it comes back with something new on it, or when
a merge conflict has appeared on it. Several at once is still one hoot, not one each: overlapping plays
of the same clip are a noise rather than a notification.

Work *leaving* is silent. That is what you wanted, and it needs no sound.

Launching into a queue that already has PRs in it hoots once — the app knew nothing before it asked, so
every one of them is news. Signing in works the same way. An axis recovering from a run of failed polls
on the same PRs is silent, because they are the same PRs; anything that turned up while the poll was
down hoots, and should.

## Why it is not "the count went up"

It was, until 2.0.0, and that was only ever a proxy for the thing the sound means.

**GitHub's search index is eventually consistent.** It will drop a pull request from one answer and hand
it back in the next, untouched. Counting rises, that reads as `3 → 2 → 3`, and the app hooted for a pull
request you had already been told about. Annoying, and worse, confusing: nothing had happened.

Counting also missed news in the other direction. One PR closing and another arriving in the same cycle
leaves the count flat, and the count rule said nothing at all.

So each axis keeps a short ledger of the pull requests it has already hooted for, and a returning one is
judged on whether it actually **changed**:

- **`activity`** — the later of the PR's own `updatedAt` and its newest review's `submittedAt`. GitHub
  does not document which events bump `updatedAt`, and a review carries only `submittedAt` with no
  update stamp of its own, so taking the later of the two settles it by construction: if a submitted
  review does not move one, it moves the other.
- **strictly newer, not merely different.** An index that hides a pull request can also serve a stale
  copy of it, and treating an *older* timestamp as a change would bring the false hoot straight back.
  A timestamp only moves forward, so the comparison does too.
- **a conflict appearing** counts even though no timestamp moved. A conflict arriving because somebody
  else merged to main touches nothing on your pull request, so nothing is dated — but it is still work
  landing on you.

The ledger holds 200 pull requests per axis and drops the least recently seen. That bound is deliberately
far out of reach of a blip: an axis shows at most 100 at once, so a PR must sit absent while a further
hundred churn past before it is forgotten. Eviction is the one thing that could bring the false hoot
back, so it is set where a blip cannot reach it. Nothing is written to disk — a restart forgets the
ledger, and the launch rule above covers that.

With `logLevel=info`, a dropped-and-returned pull request writes a line naming it and how many polls it
was missing, so how often this really happens is a number rather than an impression.

One answer the ledger cannot judge: a payload GitHub only partly sent, where one counted hit arrives
without its URL and the whole list is discarded rather than shown short. There is nothing to compare
identities against, so the old count rule stands in. That is not a hole — the blip returns a perfectly
readable list both times and never reaches this path.

One limit worth knowing: the searches cap at 100 hits. An axis pinned at the cap cannot see past it, and
what falls off the end is decided by GitHub's own relevance order, which is not stable — so a PR can
churn across the boundary. The ledger absorbs that the same way it absorbs a blip. The count in the
tooltip has always undercounted the same way, and for a sound, erring quiet is the right direction.

The clip is embedded in the binary, unpacked once per run into the system temp directory, and played by
whatever the platform already has: `winmm` (MCI) on Windows, `afplay` on macOS, and the first of `mpv`,
`ffplay`, `mpg123`, `gst-play-1.0` or `cvlc` that is installed on Linux. So no audio crate joins the
dependency tree for one short sound, and a Linux box with none of those players stays silent with a line
in the log rather than failing. Playback runs on its own thread and never delays a poll; hoots that
overlap are dropped rather than layered.

Untick **Settings ▸ Hoot on new pull requests** to silence it, which takes effect at once and writes
`sound=off` to `config.txt` for the next start; setting that key by hand does the same thing. Ticking the
box plays one hoot, so you hear what you have just switched on. That switches off the sound and nothing else — the icon,
the tooltip and the menu counts behave identically either way, so silence costs no information. There is
no volume setting; the system mixer is the only control over how loud it is.

The clip is a sound effect by [elevenlabs.io](https://elevenlabs.io/sound-effects/free), used under
their free plan: attribution required, non-commercial use only. It is the one file in this repository
the public-domain dedication does not cover — see [NOTICE](../NOTICE) before using this app for anything
commercial.
