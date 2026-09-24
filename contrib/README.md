# The shipped prompts

What the dispatcher tells an agent, one file per bar plus the nudge an agent gets when its pull
request changes under it. These four files are compiled into GitHoot and written beside `config.txt`
the first time it starts.

**Edit them there, or on the Dispatcher tab, not here.** A file in this directory is the shipped
default; a file in your own `prompts/` directory is yours and GitHoot will not overwrite it.

| File | When it is used |
|---|---|
| `work-required.txt` | Your pull request needs work |
| `requested-reviews.txt` | Somebody asked you to review |
| `approved.txt` | Your pull request is approved |
| `update.txt` | The nudge, when a pull request changes while an agent is on it |

Placeholders: `{url}` `{repo}` `{number}` `{branch}` `{title}` `{author}`.

**`{title}` and `{author}` are written by whoever opened the pull request**, and the prompt is an
instruction to an agent holding your `gh` credential. Each default puts them in a labelled block
that the next line tells the agent is data, not instructions. That is the standard mitigation and
not a cure. Keep it if you rewrite them.

Each default ends with the pull request's link on its own line, so the bottom of every agent pane is
something you can click.

Everything else about the dispatcher, including what it costs you and the one guard rail that
actually holds, is in [`docs/dispatcher.md`](../docs/dispatcher.md).

---

**This directory used to hold `ght-dispatch`**, a bash script installed by a button and kept alive
by a systemd user service, plus a PowerShell port for Windows. Both are gone: the dispatcher is a
thread inside GitHoot now. If you still have the script installed, `docs/dispatcher.md` says how to
remove it.
