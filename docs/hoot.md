# The hoot

Whenever a PR count **goes up**, the app plays a short hoot. Each of the three axes hoots for itself:
reviews requested of you, your PRs that were approved, your PRs with changes requested.

Any rise counts. Zero to three hoots, and so does one to four — four reviews waiting where one was
waiting is three pieces of news, and the tray having been lit already is no reason not to mention them.
Two or three axes rising in the same poll is still one hoot, not three: overlapping plays of the same
clip are a noise rather than a notification.

The other direction is silent. A count falling is work leaving, which is what you wanted, and a flat
count is nothing at all.

Launching into a queue that already has PRs in it hoots too, once. Strictly that is not a rise — the app
knew nothing before it asked — but it is the moment you want telling, and staying silent there would mean
the hoot only ever worked for people who left the app running. Signing in works the same way.

One case stays deliberately silent: an axis recovering from a run of failed polls at the same count it
left. The last known count is held through the failures precisely so that comparison can be made, which
is what stops every network blip becoming a notification. If it comes back *higher*, that does hoot —
those PRs turned up while the poll was down, and they are as new to you as if the app had been watching.

One limit worth knowing: the searches cap at 100 hits, so an axis already pinned at 100 cannot show
growth and stays quiet. The count in the tooltip has always undercounted the same way, and for a sound,
erring quiet is the right direction.

The clip is embedded in the binary, unpacked once per run into the system temp directory, and played by
whatever the platform already has: `winmm` (MCI) on Windows, `afplay` on macOS, and the first of `mpv`,
`ffplay`, `mpg123`, `gst-play-1.0` or `cvlc` that is installed on Linux. So no audio crate joins the
dependency tree for one short sound, and a Linux box with none of those players stays silent with a line
in the log rather than failing. Playback runs on its own thread and never delays a poll; hoots that
overlap are dropped rather than layered.

Set `sound=off` in `config.txt` to silence it. That switches off the sound and nothing else — the icon,
the tooltip and the menu counts behave identically either way, so silence costs no information. There is
no volume setting; the system mixer is the only control over how loud it is.

The clip is a sound effect by [elevenlabs.io](https://elevenlabs.io/sound-effects/free), used under
their free plan: attribution required, non-commercial use only. It is the one file in this repository
the public-domain dedication does not cover — see [NOTICE](../NOTICE) before using this app for anything
commercial.
