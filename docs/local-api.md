# The local API

**Off unless you ask.** `localApi=off` is the only setting in `config.txt` that ships off, and the
only one where a typo leaves it shut rather than open. Everything below happens only once you turn
it on and restart.

On, GitHoot serves the same judged lists the page renders as structured JSON, to scripts running as
you on this machine. Nothing else changes: no extra GitHub request, no second token, no new
permission, and the tray behaves exactly as it always did.

## Why it exists

Two of the three bars cannot be expressed as a GitHub search URL. `review:approved` misses every
approval in a repository that requires no reviews, and `review:changes_requested` keeps matching a
pull request you have already handed back (→ [PR status](pr-status.md)). Anything outside GitHoot
that wants those bars has to reimplement that judgement or do without it.

This hands it over instead, already judged.

## Turning it on

```
localApi=on
```

in `~/.githoot-tray/config.txt`, or the box in the [settings page](menu-and-settings.md#the-settings-page).
**Restart to apply**, because the listener binds once.

Two things then differ from an ordinary run:

| | |
|---|---|
| The loopback listener | binds at startup, rather than on your first menu click |
| `~/.githoot-tray/endpoint.json` | written, owner-only, naming the port and this run's token |

Off, neither happens and the route answers `404`.

## Finding the door

The port is ephemeral and the token is new on every run, so the address cannot be guessed and is not
worth writing down. `endpoint.json` is how a script finds it:

```json
{
  "schema": 1,
  "pid": 12345,
  "port": 49213,
  "token": "0f3a…",
  "host_header": "127.0.0.1:49213",
  "base_url": "http://127.0.0.1:49213/0f3a…",
  "axes": {
    "requested-reviews": "http://127.0.0.1:49213/0f3a…/requested-reviews/entries",
    "approved":          "http://127.0.0.1:49213/0f3a…/approved/entries",
    "work-required":     "http://127.0.0.1:49213/0f3a…/work-required/entries"
  }
}
```

**The URLs name `127.0.0.1`, not `githoot.localhost`.** Browsers resolve every `.localhost`
subdomain to loopback themselves, which is what makes the page's address readable; curl and most
script HTTP clients do not. Both literals are accepted, so publishing the numeric one is what makes
the URL work with no `--resolve` and no `Host` override.

**The file is a hint, never a fact.** There is no shutdown path, deliberately
(→ [the PR page](pr-page.md#what-guards-it)), so a crash or a `kill -9` leaves it behind holding a
dead port. It is removed at the next start, whatever the setting says, and rewritten only when the
listener binds with `localApi` on. A reader connects and treats a refused connection or a `404` as
"GitHoot is not running". `pid` is advisory, because pids get reused. **Never act on the file alone.**

A missing file is not an error either. Between a self-update's `exec` and its successor's bind there
is a moment with none.

## Reading a bar

```bash
PORT=$(jq -r .port  ~/.githoot-tray/endpoint.json)
TOK=$(jq  -r .token ~/.githoot-tray/endpoint.json)
curl -sS --fail-with-body "http://127.0.0.1:${PORT}/${TOK}/approved/entries"
```

No `-H Host:` is needed: curl sends the literal it dialled, and that literal is allowed.

```json
{
  "schema": 1,
  "axis": "approved",
  "version": 12,
  "generated_at_unix": 1758499200,
  "polled_age_seconds": 47,
  "known": true,
  "portals": [
    {
      "id": "github",
      "display_name": "GitHub",
      "inbox_url": "https://github.com/pulls/inbox",
      "known": true,
      "entries": [
        {
          "key": "PR_kwDOAbCd",
          "id": "PR_kwDOAbCd",
          "url": "https://github.com/acme/widget/pull/7",
          "title": "Tighten the poll backoff",
          "repo": "acme/widget",
          "number": 7,
          "author": "alice",
          "updated_at": "2026-09-20T11:04:00Z",
          "is_draft": false,
          "conflicting": false,
          "bot_review": { "name": "Copilot", "unresolved": 3 },
          "checks": "failure",
          "verdicts": [ { "login": "bob", "state": "approved" } ],
          "pending_reviewers": [ { "kind": "team", "name": "platform" } ],
          "muted": false,
          "muted_until_unix": null
        }
      ]
    }
  ]
}
```

Every key is always present, `null` when there is nothing to say. `checks` is never `null`: it is one
of `unknown`, `success`, `pending`, `failure`, `error`, `expected`, and `unknown` is a real answer
covering a repository with no checks, a payload hole, and a permission degraded away. There is no
`count` field, because the array length is the count and a second copy of it is a second thing that
can disagree.

`pending_reviewers` tags `user` against `team` rather than flattening both to a name, because a team
has no login and cannot be looked up as a person.

**`muted` is `true` for a pull request you have muted from the PR page**, and `muted_until_unix` says
until when. Muted pull requests are still listed, because hiding them would be the API deciding for
you; they do not count on the icon, and anything acting on a bar should skip them, as the shipped
dispatcher does → [the PR page](pr-page.md#muting-a-pull-request).

## The one rule that matters

**A list that is not known is `null`, never `[]`.**

| | `known` | `entries` | What it means |
|---|---|---|---|
| Not known | `false` | `null` | The poll failed, or has never confirmed. **Do nothing.** |
| Confirmed empty | `true` | `[]` | Genuinely nothing in this bar |
| A list | `true` | `[...]` | These, exactly |

This is the same refusal to report a confident zero that the icon, the tooltip and the page all make
(→ [Troubleshooting](troubleshooting.md)), pushed out to a caller that can see none of them. A script
that reads an outage as an empty board goes quiet at the exact moment it was written to act, and
nobody finds out for a week.

`null` rather than a missing key is deliberate: the careless path throws. `.entries.length` is a
`TypeError`, `jq '.entries[]'` is an error, where a missing key would have quietly yielded nothing.

**`known` at the top level is a third guard**, for `"portals": []`. That happens when no portal is
configured or the poll loop has not published yet, and an empty array one level up would read as a
confident zero just as easily. Check the top-level `known` first, then each portal's own.

## What it does not give you

**Identity, never content.** An entry carries the repo, the number, the URL, the title, the author,
each reviewer's verdict *state* and a count of open bot comments. It carries **no diff and no comment
bodies**, because GitHoot never fetches them: the bars do not need them.

A tool that wants to read a pull request fetches it itself, with `gh` or the API, under its own
credential. That is the right split rather than a gap to be closed:

| Who | Gives |
|---|---|
| GitHoot | which pull requests are in which bar, judged |
| `gh` and friends | the diff, the comments, and anything that writes |

**GitHoot's own credential is not lent out, and cannot usefully be.** It is a read-only fine-grained
GitHub App token (Pull requests, Metadata, Checks, Commit statuses, all read) and the whole
least-privilege argument rests on it staying that way → [PR status](pr-status.md#why-a-github-app).
`endpoint.json` holds the loopback token, never the GitHub one.

## What guards it

Everything that guards the page, unchanged → [the PR page](pr-page.md#what-guards-it). Specifically:

- **Loopback only.** `127.0.0.1` and `[::1]`, never `0.0.0.0`.
- **The per-run token** in the path, compared in constant time. A wrong one answers `404`, not `403`,
  so a probe cannot confirm the rest of the path was right.
- **The `Host` allowlist**, as a DNS-rebinding defence: exactly `127.0.0.1:<port>` or
  `githoot.localhost:<port>`. Bare `localhost:<port>` and `[::1]:<port>` are refused.
- **`GET` and `HEAD` only.** A `POST` is `405`, and always will be. See below.
- **`endpoint.json` is owner-only** (`0600` on Unix), the same treatment `pr_token.txt` gets, because
  it holds this run's key to the port. Windows has no equivalent, which is the posture `pr_token.txt`
  already has while holding a more valuable secret.

`ETag` and `If-None-Match` work, so a polling script costs a bodyless `304` most of the time:

```bash
curl -sS --etag-compare .etag --etag-save .etag \
     "http://127.0.0.1:${PORT}/${TOK}/approved/entries"
```

## What this is not, and will not become

**Not a webhook, and not a callback.** GitHoot never spawns a process and there is no command string
in `config.txt`. That was the obvious design and it was rejected: it would turn a settings file into
an execution vector, so anything that could write your config would run code as you at the next poll.
A script that polls a JSON route costs one request a minute and carries none of that.

**Not a claim endpoint.** There is no way to tell GitHoot which pull requests something has already
taken, and there will not be. The moment GitHoot remembers that, it is persisting pull-request state
across restarts, and the design that makes this feature small collapses. Whatever is reading the
lists owns its own bookkeeping, where it can be inspected and reset. A test asserts the `405`.

**Not a push.** The poll floor is 60 seconds (→ [Troubleshooting](troubleshooting.md)), so a reader
learns about a change up to a minute after GitHub did, and no sooner than GitHoot itself.

## The dispatcher, if you want one

Reading the bars is the general thing. Turning them into agents is one particular use of it, and
GitHoot ships that too, as a separate, unsupported, Linux-only piece: `ght-dispatch`, installed with
one button on the **Dispatcher** tab once `localApi` is on. It keeps its own small state file, asks
Herdr who is already working what, and starts one agent per pull request with a prompt you own.

Everything about it, including what the button writes and the one guard rail that actually holds,
is in [`contrib/README.md`](../contrib/README.md). It is documented there rather than here on
purpose: this page is the contract, and the dispatcher is one caller of it.
