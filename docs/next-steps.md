# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

Nothing scheduled. Pick from `docs/backlog.md`.

## Watch out for

- The CI job `Windows (best effort)` is allowed to fail, so a red Windows job
  does not block a merge. Check it anyway: it passes since the fix of ADR 0001
  (PR #2).
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
