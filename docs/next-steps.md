# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

Nothing scheduled. Pick from `docs/backlog.md`.

## Watch out for

- GitHub Actions are disabled (2026-10-04, Actions quota low): pull requests
  get no CI. Run the test commands from the README locally, and re-enable with
  `gh workflow enable CI` when the quota allows.

- The CI job `Windows (best effort)` is allowed to fail, so a red Windows job
  does not block a merge. Check it anyway: it passes since the fix of ADR 0001
  (PR #2).
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
