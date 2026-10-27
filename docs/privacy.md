# Privacy policy

GitHoot is a tray app that runs on your computer. It has no server of its own, no account of its own,
no telemetry, no analytics and no ads. Nothing it learns about you is sent to the project or to anyone
else.

## What it connects to

GitHoot only talks to the services below, and only to do the job you set it up for.

| Service | When | Why |
|---|---|---|
| GitHub (`github.com`, `api.github.com`), or your GitHub Enterprise host | While you are signed in to GitHub in GitHoot | Sign-in, and reading the pull requests that need you |
| GitLab (`gitlab.com`), or your self-hosted GitLab | While you are signed in to GitLab in GitHoot | Sign-in, and reading the merge requests that need you |
| Public status pages (`www.githubstatus.com`, `api.status.io`, `status.gitlab.com`) | While the matching service is in use | Showing whether GitHub or GitLab is having an outage |
| GitHub Releases for this project (`api.github.com`, `github.com`) | At startup and once a day | Checking for, and on your request installing, a newer GitHoot. Turn it off with `updateCheck=off` in `config.txt` |

These requests go straight from your computer to those services. Like any web request, they reveal your
IP address and what is being asked for to that service, which handles it under its own privacy policy:
[GitHub](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement),
[GitLab](https://about.gitlab.com/privacy/).

## What it stores, and where

Everything stays on your computer, in the `.githoot` folder in your home directory:

- `config.txt`: your settings.
- `pr_token.txt` and `work_token.txt`: the sign-in tokens for GitHub and GitLab. Delete a file, or sign
  out on the settings page, to remove one.
- `log.txt`: what the app did, for troubleshooting. It can contain repository names, pull request
  titles and user names. It never contains a token.

Start at sign-in, when you agree to it, adds an entry to your system's startup list. The hoot sound is
unpacked once into your system's temporary folder. Uninstalling GitHoot means deleting the program, the
`.githoot` folder and that startup entry.

## Local pages

The settings page and the pull request page are served by GitHoot itself, on `127.0.0.1` and `::1`
only, so they are not reachable from other computers. Each run uses a random port and a random token in
the address. The optional [local API](local-api.md) is off unless you turn it on.

## Integrations

[Integrations](integrations.md) are off until you install them. The one that exists today, the
[Herdr dispatcher](dispatcher.md), uses tools already on your computer: it fetches the pull request with
`git`, creates a branch and a worktree, and starts a coding agent through Herdr with a prompt about that
pull request. Whatever that agent then sends, for example code to an AI provider, is decided by how you
set up that agent and is covered by its provider's policy, not by GitHoot.

## Changes

This policy changes with the code, in the same repository, so its history is the
[git history of this file](https://github.com/HerrDerb/githoot/commits/main/docs/privacy.md).
Questions go to the [issue tracker](https://github.com/HerrDerb/githoot/issues).
