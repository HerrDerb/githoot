# The PR page

The three PR menu entries open a page GitHoot renders and serves itself, on loopback, listing exactly
the pull requests that bar counted.

They used to open GitHub. That worked for one of the three and not the other two, because **no GitHub
search URL can express what those bars count** — `review:approved` misses every approval in a
repository that requires no reviews, and `review:changes_requested` keeps matching a pull request you
have already handed back (→ [PR status](pr-status.md)). So those two entries opened **one browser tab
per pull request** instead, and fell back to a hand-built search URL that claimed to be "the same
query" — which it was not, since the bars narrow their hits after the search, and which did not work
anyway. Every link out now goes to GitHub's own pull-request inbox, which is honest about being a
different view and is always reachable.

The page shows what the bar counted and what GitHub's list could not: the check rollup, and every
reviewer's standing verdict.

| | |
|---|---|
| Address | `http://githoot.localhost:<port>/<token>/<bar>`, plus `/items` and `/settings` |
| Bound | On the first click, never at startup, unless [`localApi`](local-api.md) is on |
| Port and token | New on every run |
| Contents | Title, repo, number, author, age, draft, checks, merge conflict, open comments from the portal's automatic reviewer (Copilot on GitHub), per-reviewer verdicts |

**Grouped by portal.** With one configured portal the page is a bare list. With several, each
portal's cards sit under a heading with their own empty state and their own inbox link, because
"nothing here" on one forge says nothing about another. Only URLs under a portal's own link prefix
become links (`https://github.com/` for GitHub, trailing slash included), so a URL that is not the
portal's own is shown as text rather than offered as a click → [Portals](portals.md).

**Newest first.** GitHub's search answers in *best match* relevance order whenever the query names no
sort, and none of the three does, so what comes back is effectively arbitrary and can differ between
two polls over the same pull requests. The page sorts by last update instead, so the list does not
shuffle under you between reloads. A pull request GitHub gave no date sorts last.

**Links open in the same window.** Opening a tab per click is the habit this page exists to get away
from; the back button is the way back to the list.

**It keeps itself current**, without a reload and without touching GitHub — so a change is on screen
within about five seconds of the poll that found it.

Every five seconds the page asks `/<token>/<bar>/items` whether anything has moved, as a **conditional
request**. Each answer carries an `ETag` — the version of the snapshot the poll loop last published —
and the page sends it back as `If-None-Match`. Between polls the reply is a bodyless `304`, which is a
few dozen bytes; only a poll that actually published something returns a list, and only then is the
list on screen replaced. Rebuilding it otherwise would drop hover, focus and any text selection inside
it, for rows that have not moved.

**The age is computed by the page, not sent by the server**, and that is what makes the `304` honest.
"as of 47 s ago" changes every second, so a response carrying it could never be unchanged and the
server would have to resend the whole list every few seconds to keep one line current. Instead each
answer says how old the data was at that instant, the page anchors a local clock to it, and the line
ticks on with no request at all.

Three things keep a forgotten tab cheap: it pauses while the tab is hidden and catches up the moment
it is shown, it never reaches GitHub, and it **stops after five consecutive failures** and says
*GitHoot is not running* — otherwise a tab left open would poll a dead port for as long as it lived.

This is the one place GitHoot serves anything executable. The script is a dozen lines, and the page's
CSP names it by **nonce** rather than allowing inline script at large: a fresh random value per
response, so only that exact `<script>` may run and anything injected into the page cannot borrow the
permission. `connect-src 'self'` is opened alongside it, because `default-src 'none'` would otherwise
block the fetch. Everything else GitHoot serves still runs nothing at all.

**It never shows a zero it cannot stand behind.** A bar whose poll failed, or that has not answered
yet, gets a page saying the list is *not known* — not an empty one. The same refusal the tray makes
about a stale count in a menu label.

## Why `githoot.localhost`

`.localhost` is reserved by RFC 6761 and browsers resolve every subdomain of it to loopback on their
own. No `hosts` file, no DNS, no admin rights.

A bare `githoot/prlist` is not achievable and was not a near miss: a single-label name resolves only
via the system `hosts` file, which needs root, and a URL with no port means port 80, which needs root
on Linux and macOS and is usually held by `http.sys` on Windows. The token would still sit in the
path, so the tidy part would not have survived anyway.

GitHoot listens on `127.0.0.1` **and** `[::1]` on the same port, because a browser may resolve the
name to `::1` first. If the IPv6 bind fails the IPv4 one carries on alone and browsers fall back.

## Why a server and not a file

An HTML file in `~/.githoot/` would be the looser option, not the tighter one. It persists after
the app exits, it is readable by anything running as you and by every browser profile on the machine,
and it is stale the moment the next poll lands. A render per request cannot go stale, and a token that
dies with the process bounds what a leaked URL is worth.

## What guards it

A listening socket on your machine is worth being precise about. Four things stand in front of the
page, and each one stops something the others do not.

- **It binds `127.0.0.1` and `[::1]` only** — never `0.0.0.0`, never `[::]`. Nothing off this machine
  can reach it; the LAN address answers connection refused. This is also why it should raise no
  Windows firewall prompt: that prompt is for listeners reachable from the network.
- **A 128-bit token in the path**, from the platform's CSPRNG (`/dev/urandom`, or `BCryptGenRandom` on
  Windows). This is the defence against a hostile web page in *your own browser*, which can `fetch()`
  across every loopback port from JavaScript. That attacker gets unlimited guesses, which is why it is
  a real CSPRNG and not a hash seed. A wrong token answers `404`, not `403`, so the reply does not
  confirm the rest of the path was right, and the comparison is constant-time.
- **A `Host` allowlist** of exactly `githoot.localhost:<port>` and `127.0.0.1:<port>`. This is the
  DNS-rebinding defence: a rebinding request arrives carrying the attacker's own name. Accepting
  `githoot.localhost` costs nothing here, because browsers pin `.localhost` to loopback themselves and
  there is no record for anyone to flip. Bare `localhost` is rejected with the rest — nothing GitHoot
  hands the browser uses it, and it *is* a name a resolver can be talked out of.
- **No CORS header, ever**, so a cross-origin page cannot read the body even if it reached the path.
- **An `Origin` check on writes.** Only the settings page accepts a `POST`, and only when the browser
  says this page submitted it. That is a different question from the `Host` allowlist, which a
  cross-site form passes for free → [the settings page](menu-and-settings.md#the-settings-page).

Two more things about what leaves the page:

- **`rel="noreferrer"` on every link out, and `Referrer-Policy: same-origin`** on every page. Without
  them the first click through to a pull request would hand GitHub this page's URL, token and all, in
  the `Referer` header. `same-origin` sends nothing to another site, and the `rel` covers the link
  itself, so GitHub still sees nothing.

  **Not `no-referrer`, which the PR pages used until they gained mute links.** `no-referrer` does more than suppress
  `Referer`: per the Fetch standard a non-`GET`/`HEAD` request under that policy has its `Origin`
  serialized as `null`, so the page could not tell us it had submitted its own form and every save was
  refused. `same-origin` still sends nothing to another site — the token is exactly as protected —
  while keeping a real `Origin` on a request back to us.
- **`Content-Security-Policy: default-src 'none'`**, with `script-src 'nonce-…'` on the PR pages and
  nothing at all elsewhere. Pull request titles come from whoever opened them, so they are escaped as
  hostile text; the CSP is the backstop behind that, not the control.

Only `GET`, `HEAD` and — on the settings route alone — `POST` are answered. There is no keep-alive, the
request head is capped at 8 KiB and a form body at 64 KiB, and both have a five-second timeout, so a
client that connects and says nothing cannot wedge the listener.

## Muting a pull request

**Every row offers "Mute for 3 days · 7 days · 30 days".** A muted pull request drops out of its bar:
it does not light the icon, does not count on the menu entry or the page, and does not hoot. It is not
hidden. It moves to a **Muted** section at the bottom of the same page, saying how long is left and
offering **Unmute**.

**Every muted pull request is on one page too**, `/<token>/muted`, the **Muted** tab on every one of
GitHoot's own pages, and linked from the Muted heading of any bar. It is the only way to
a muted pull request whose bar is otherwise empty, because an empty bar's menu entry is hidden. Pull
requests no bar holds any more, merged or closed while muted, are listed by id so they can still be
unmuted; their mute ends by itself otherwise.

**It comes back as a new pull request.** When the mute ends, by the link or by time, the pull request
arrives as if never seen: the bar lights and the owl hoots. A mute is a snooze, and a snooze that
ended in silence would be a way to lose a review.

**Mutes are kept on disk**, in `~/.githoot/muted.txt`, one pull request per line, because a mute
measured in days has to survive the restarts a self-update causes. Expired lines are dropped whenever
the file is read or written. The links are forms, not links, so a mute is a `POST` behind the same
`Origin` check as the settings page, and it accepts only the three offered durations and only a pull
request that is on the page right now. The icon catches up within seconds, because a mute wakes the
poll loop.

The [local API](local-api.md) still serves muted pull requests, marked `"muted": true`, and
[the dispatcher](dispatcher.md) skips them.

## When it cannot start

If the loopback bind fails — a hardened container, a sandbox, a machine with no loopback — the menu
entries open GitHub's own pull-request inbox instead. The failure is logged
once, with the operating system's own error, and not retried, since nothing that causes it clears
between two clicks.

The listener stops when GitHoot does. There is nothing to clean up, and nothing is written to disk.

**One setting changes both halves of that sentence.** With [`localApi`](local-api.md) on, the
listener binds at startup rather than on a click, and the port and token are written to
`~/.githoot/endpoint.json` so a script can find them. It ships off, and off it changes nothing
here.
