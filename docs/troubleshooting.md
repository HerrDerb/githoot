# Troubleshooting

The tooltip always states what the app currently believes, including *why* it is unsure. Full history is
appended to `~/.githoot-tray/log.txt` — the only way to see errors on Windows and macOS, where there is
no console. The log never contains a token.

That directory holds the four files worth opening — `config.txt`, `log.txt` and the two credentials — plus
an `icons/` subdirectory with the 64 generated PNGs. Linux only: Windows and macOS build their icons in
memory and never write one to disk.

| Tooltip | Meaning |
|---|---|
| `N PR(s) awaiting your review` / `No reviews requested` | Confirmed by a successful search |
| `N PR(s) approved` / `No approvals yet` | Confirmed |
| `N PR(s) needing your work` / `Nothing needing your work` | Confirmed |
| `PR status: not authorized yet` | No credential — use the menu entry |
| `PR status off: install the GitHub App to see your PRs` | Authorized, but installed nowhere |
| `PR status off: setup failed` | No HTTP client could be built; clicking will not help |
| `PR status off in config.txt` | All three signals switched off |
| `... state unknown` | Several polls failed; that signal is no longer trustworthy, and the exclamation is up |
| `GitHub: <description>` | GitHub reports an incident; quotes its own wording, and comes first in the tooltip. With `statusComponents` set, the description names the components: `Issues (Partial Outage)` |
| `Update available: X.Y.Z` | A newer release exists |

A bar that is simply absent, with no message, means you genuinely have nothing pending. That distinction
is the whole point: an unreadable answer must never be reported as a confident zero.

Signals are independent — a failing search on one never disturbs another. Each
cycle logs one line naming every signal it actually asked about; a switched-off one is absent:

```
poll → ReviewRequested→fresh, ReadyToMerge→fresh, ChangesRequested→fresh → No/No/No (next in 60s) [...]
```

Polling is at least every 60s, backing off automatically when GitHub says so via `x-poll-interval` or
`retry-after`. Opening a list re-checks a few seconds later, so a bar clears as soon as you have acted.


## Windows Defender may flag the binary

It has happened: `Trojan:Win32/Bearfoos.B!ml`. The `!ml` suffix means a machine-learning verdict
rather than a signature match, and this binary fits the shape those models are trained to distrust —
it is **not code-signed**, so it carries no publisher reputation, it runs with no console and no
window, and its updater downloads a new `.exe`, stages it beside the running one and renames it into
place. Every one of those is also how a dropper behaves.

The releases now embed a `VERSIONINFO` resource, an application manifest and an icon, so the binary at
least identifies itself in Explorer's Properties dialog and to anything that reads publisher metadata.
That reduces the odds; it does not eliminate them, and it is not a claim that the binary is safe.

**What to do instead of taking anyone's word for it.** Verify the download against the signed manifest
— that chain is described under [What is verified](updates-and-releases.md#what-is-verified-and-what-that-proves), and it is the
same check the updater runs before installing anything:

```bash
# Compare your download against the signed digest list for its tag.
curl -LO https://github.com/HerrDerb/githoot-tray/releases/download/vX.Y.Z/sha256sums.txt
curl -LO https://github.com/HerrDerb/githoot-tray/releases/download/vX.Y.Z/sha256sums.txt.minisig
minisign -Vm sha256sums.txt \
  -P 'RWSYrhd3sxiQUDZtxm8c+p0iRdj+z+fGQKdLq62ojrmfii2OjCG8PX8D'  # authenticity
sha256sum -c sha256sums.txt --ignore-missing                    # integrity
```

If both pass, the file is the one CI built and signed. Whether you then allow it to run is your call,
and a Defender exclusion is a decision to make deliberately rather than a step in an install guide.

---

## Platforms

| | |
|---|---|
| 🐧 Linux | `libappindicator` + GTK 3 |
| 🪟 Windows | `tray-icon` + `winit`, no console window |
| 🍎 macOS | `tray-icon` + `winit`, `LSUIElement` bundle, no Dock icon |

Launching a second copy shows a notice and exits (Windows; macOS handles it itself for bundles).
