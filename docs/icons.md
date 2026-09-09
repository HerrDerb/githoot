# Icons

Everything is composited at runtime, so `assets/` holds two image files and the variants cannot drift
apart. (A third file lives there, `hoot.mp3` — the notification sound, not an icon, and the only
bundled file that is not this project's own work: see [NOTICE](../NOTICE).)

The base glyph is an owl, drawn for this project: the thing that sits still and watches so you do
not have to. It used to be GitHub's Invertocat, which is their trademark and not this app's to wear
as an application icon. The glyph carries no state, so replacing it cost nothing but the drawing.

It is dark (<img src="icons/tray.png" alt="base icon" height="20" valign="middle">), or
blue when notifications are on and something is unread
(<img src="icons/tray_blue.png" alt="blue base icon" height="20" valign="middle">). On top of it:

| Icon | Mark | Means |
|:---:|---|---|
| <img src="icons/tray_review.png" alt="review bar" height="28"> | red bar | A PR is waiting on your review |
| <img src="icons/tray_merge.png" alt="merge bar" height="28"> | green bar | One of your PRs is approved |
| <img src="icons/tray_changes.png" alt="changes bar" height="28"> | amber bar | A reviewer asked for changes on your PR |
| <img src="icons/tray_update.png" alt="update arrow" height="28"> | green up-arrow | A newer release is available |
| <img src="icons/tray_alert.png" alt="exclamation" height="28"> | red exclamation | Something needs saying, see below |

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
| <img src="icons/tray_blue_review_merge_changes_update_alert.png" alt="every mark at once" height="28"> | Every mark at once: blue base, three bars, arrow and exclamation |

The arrow sits in the top middle and is drawn last, so it overlaps whatever is beneath it. Each bar's
**centre** survives that: clip a bar's end and it still reads as a bar, reach its middle and it stops being
one. That is asserted, not hoped for.

**The exclamation has three causes, and only the tooltip and menu say which:**

| Cause | What the menu offers |
|---|---|
| PR status is not authorized | **Authenticate GitHub PR Status** |
| GitHub reports an incident | **GitHub is githubing again, check status** |
| A poll failed, so a signal is unknown | nothing to click — the tooltip names the axis |

One mark for three causes is a deliberate trade: a second mark would need somewhere to live on an icon that
has no free space left. The causes stay separate internally, so an incident never offers a sign-in and a
missing credential never points at the status page.

"A poll failed" means exactly that — asked and not answered. A freshly started app has answered nothing yet
and shows no mark, and a single transient blip holds the last known value rather than raising one.
