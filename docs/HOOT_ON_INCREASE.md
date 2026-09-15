# Hoot on every increase

Running checklist for this session. Tick as understood.

## 1. The problem
- [ ] What the hoot did before: fired only on a *presence* edge (`No` -> `Yes`), plus the first
      confirmed answer of a launch.
- [ ] Why that was too quiet: `count` 1 -> 5 is four new pull requests and made no sound, because
      `Presence` was already `Yes`. The axis was "already busy", so nothing announced the new work.
- [ ] Why presence, not count, was the original signal: the tray icon itself is presence-based
      (a dot is lit or dark), and the hoot was written to match the icon.

## 2. The change
- [ ] The latch now compares the previous `count` with the new one and fires when it went up.
- [ ] The presence rule survives as a fallback for when there is no previous count.

## 3. The branches (which transitions hoot)
- [ ] `count` 0 -> 3 : hoot (also the old rule)
- [ ] `count` 2 -> 5 : hoot (NEW; used to be silent)
- [ ] `count` 5 -> 2 : silent (work went away)
- [ ] `count` 2 -> 2 : silent
- [ ] first answer of the run, count > 0 : hoot (unchanged)
- [ ] failure streak recovering at the same count : silent (unchanged)
- [ ] failure streak recovering at a *higher* count : hoot (NEW)
- [ ] `304 Not Modified` : silent, by construction
- [ ] notifications axis : never hoots (unchanged)

## 4. Design decisions
- [ ] Why compare counts instead of adding a second latch.
- [ ] Why `ever_confirmed` still has to exist.
- [ ] Why the count is held through failures, and what that buys the new rule.
- [ ] Why still only one hoot per poll cycle even when three axes rise.
- [ ] Why the search cap (100) makes the rule undercount rather than over-hoot.

## 5. Impact
- [ ] More hoots than before on a busy repository. Who feels that, and the escape hatch.
