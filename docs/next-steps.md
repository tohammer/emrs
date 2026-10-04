# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

1. Windows crash, permanent fix: the cause is confirmed (spec section 13.1).
   Decide how the module finds Emacs's C runtime, record it as an ADR, turn
   the experiment on `debug/windows-crash` into the fix, make the Windows CI
   job required, and delete `.github/workflows/windows-debug.yml`.

## Watch out for

- Windows is not supported. The CI job `Windows (best effort)` is allowed to
  fail, so a red Windows job does not block a merge.
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
