# The PR page

The three PR menu entries open a page GitHoot renders and serves itself, on loopback, listing exactly
the pull requests that bar counted.

They used to open GitHub. That worked for one of the three and not the other two, because **no GitHub
search URL can express what those bars count** — `review:approved` misses every approval in a
repository that requires no reviews, and `review:changes_requested` keeps matching a pull request you
have already handed back (→ [PR status](pr-status.md)). So those two entries opened **one browser tab
per pull request** instead, and fell back to a search page listing more than the bar claimed.

The page shows what the bar counted and what GitHub's list could not: the check rollup, and every
reviewer's standing verdict.

| | |
|---|---|
| Address | `http://githoot.localhost:<port>/<token>/<bar>`, and `/settings` |
| Bound | On the first click, never at startup |
| Port and token | New on every run |
| Contents | Title, repo, number, author, age, draft, checks, merge conflict, open Copilot comments, per-reviewer verdicts |

**Newest first.** GitHub's search answers in *best match* relevance order whenever the query names no
sort, and none of the three does, so what comes back is effectively arbitrary and can differ between
two polls over the same pull requests. The page sorts by last update instead, so the list does not
shuffle under you between reloads. A pull request GitHub gave no date sorts last.

**Links open in the same window.** Opening a tab per click is the habit this page exists to get away
from; the back button is the way back to the list.

**Reloading re-renders.** Each request is answered from the latest poll, so the tab stays useful
without going back to the tray. The header says how old the data is. The page never refreshes itself:
a tab you forgot about would re-render for ever, and the "as of" line makes staleness visible instead.

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

An HTML file in `~/.githoot-tray/` would be the looser option, not the tighter one. It persists after
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

- **`Referrer-Policy: no-referrer`** on the PR pages, plus the same in a `<meta>` and `rel="noreferrer"`
  on every link. Without it the first click through to a pull request would hand GitHub this page's
  URL, token and all, in the `Referer` header.

  **The settings page uses `same-origin` instead, and must.** `no-referrer` does more than suppress
  `Referer`: per the Fetch standard a non-`GET`/`HEAD` request under that policy has its `Origin`
  serialized as `null`, so the page could not tell us it had submitted its own form and every save was
  refused. `same-origin` still sends nothing to another site — the token is exactly as protected —
  while keeping a real `Origin` on a request back to us.
- **`Content-Security-Policy: default-src 'none'`**. The page runs no script at all. Pull request
  titles come from whoever opened them, so they are escaped as hostile text; the CSP is the backstop
  behind that, not the control.

Only `GET`, `HEAD` and — on the settings route alone — `POST` are answered. There is no keep-alive, the
request head is capped at 8 KiB and a form body at 64 KiB, and both have a five-second timeout, so a
client that connects and says nothing cannot wedge the listener.

## When it cannot start

If the loopback bind fails — a hardened container, a sandbox, a machine with no loopback — the menu
entries fall back to what they did before: GitHub's search page for that bar. The failure is logged
once, with the operating system's own error, and not retried, since nothing that causes it clears
between two clicks.

The listener stops when GitHoot does. There is nothing to clean up, and nothing is written to disk.
