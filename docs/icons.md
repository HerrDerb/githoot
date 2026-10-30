# Icons

Everything is composited at runtime, so `assets/` holds one image file and the variants cannot drift
apart. (One more lives there, `hoot.mp3`: the sound, not an icon, synthesized by
`scripts/make-hoot.py`.) There used to be a blue owl beside it, for
unread notifications; that feature and its asset were removed in 2.0.0.

The base glyph is an owl, drawn for this project: the thing that sits still and watches so you do
not have to. It used to be GitHub's Invertocat, which is their trademark and not this app's to wear
as an application icon. The glyph carries no state, so replacing it cost nothing but the drawing.

The glyph is dark (<img src="icons/tray.png" alt="base icon" height="20" valign="middle">) and carries
no state of its own. On top of it:

| Icon | Mark | Means |
|:---:|---|---|
| <img src="icons/tray_review.png" alt="review bar" height="28"> | red bar | A PR is waiting on your review |
| <img src="icons/tray_merge.png" alt="merge bar" height="28"> | green bar | One of your PRs is approved |
| <img src="icons/tray_changes.png" alt="changes bar" height="28"> | amber bar | Your PR needs work: an objection stands, or it conflicts |
| <img src="icons/tray_update.png" alt="update arrow" height="28"> | green up-arrow | A newer release is available |
| <img src="icons/tray_alert.png" alt="exclamation" height="28"> | red exclamation | Something needs saying, see below |
| <img src="icons/tray_hushed.png" alt="small red dot" height="28"> | small red dot | An outage you acknowledged, see below |

The icons above are the real ones the app draws, not mock-ups.

The three PR bars stack in one column, so there is one place to look rather than three corners, and they
fill nearly the whole height — 92 of 96 pixels. Positions are fixed, so a bar always means the same thing
and switching one off leaves a gap rather than closing up.

There is deliberately no spare slot. One was reserved for a while for a signal that never arrived, and
holding it open cost a quarter of the icon's height, taken straight out of the bars that do exist. Adding a
fourth signal later means re-tuning the geometry, which is the right way round.

**Everything combines.** All six marks are independent, so any state can be drawn — bars, arrow and
exclamation together if that is the truth. The exclamation used to *replace* the bars, because it sat on top
of their column; moving it to the left is what freed the counts to stay visible while something is wrong.

A few real composites:

| Icon | State |
|:---:|---|
| <img src="icons/tray_review_merge_changes.png" alt="all three bars" height="28"> | All three PR bars lit |
| <img src="icons/tray_review_changes_alert.png" alt="bars beside the exclamation" height="28"> | Counts still visible beside the exclamation |
| <img src="icons/tray_review_merge_changes_update_alert.png" alt="every mark at once" height="28"> | Every mark at once: three bars, arrow and exclamation |

The arrow sits in the top middle and is drawn last, so it overlaps whatever is beneath it. Each bar's
**centre** survives that: clip a bar's end and it still reads as a bar, reach its middle and it stops being
one. That is asserted, not hoped for.

**The exclamation has three causes, and only the tooltip and menu say which:**

| Cause | What the menu offers |
|---|---|
| PR status is not authorized | **Authenticate GitHub PR Status** |
| A portal reports an incident | An entry naming its mascot, such as **The Octocat (GitHub) is having a rough day, check status** |
| A poll failed, so a signal is unknown | nothing to click — the tooltip names the axis |

One mark for three causes is a deliberate trade: a second mark would need somewhere to live on an icon that
has no free space left. The causes stay separate internally, so an incident never offers a sign-in and a
missing credential never points at the status page.

"A poll failed" means exactly that — asked and not answered. A freshly started app has answered nothing yet
and shows no mark, and a single transient blip holds the last known value rather than raising one.

## Acknowledging an outage

An incident can last hours, and nothing the user does ends it. A mark that cannot be acted on teaches
people to ignore the mark, and then they miss the sign-in or the failed poll it also stands for. So
opening the status page from the tray menu acknowledges the incident: the user has gone and looked. No
entry of its own, so nothing new to find; the cost is that quieting the mark always opens a tab.

| Icon | State |
|:---:|---|
| <img src="icons/tray_review_changes_alert.png" alt="exclamation beside the bars" height="28"> | Incident, not yet acknowledged |
| <img src="icons/tray_review_changes_hushed.png" alt="small dot beside the bars" height="28"> | The same incident, acknowledged |

The exclamation shrinks to its own dot, in the same corner and the same red. Red because it is still an
outage; small because it has been seen. Grey was tried and lost: at 16px on a light panel it disappears.
The status entry and the tooltip line stay, the line marked `(acknowledged)`, so the truth is one hover
away.

**What brings the exclamation back:**

- **The incident changes.** The acknowledgement is keyed on the portal's own words, which every portal
  builds from the severity and the parts that are down. Worse, better or another part down all read as
  new. A repeat of the same verdict does not.
- **It ends, and another starts.** A recovery forgets the acknowledgement, so the next incident is loud
  even if it reads the same.
- **A restart.** It lives in memory only, which errs on the side of saying too much.
- **Another cause.** It quiets the outage and nothing else: a missing credential or a failed poll during
  an acknowledged outage still raises the full mark. With several portals, the dot shows only while every
  portal that is down has been acknowledged.

The mark is therefore a three-state value (`state::Mark`: none, alert, hushed), and the icon set holds 48
variants rather than 32.
