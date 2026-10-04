# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

1. Investigate the Windows crash: get the real exit status in CI, then build
   for `x86_64-pc-windows-gnu` to test the C runtime hypothesis (backlog
   "Windows support", spec section 13.1).

## Watch out for

- Windows is not supported. The CI job `Windows (best effort)` is allowed to
  fail, so a red Windows job does not block a merge.
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
