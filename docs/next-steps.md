# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

1. Windows crash, step 2: build the test module for `x86_64-pc-windows-gnu`
   (which uses `msvcrt.dll`, like Emacs) in the diagnostics workflow on the
   branch `debug/windows-crash` and rerun. Step 1 pointed at the C runtime
   mismatch (spec section 13.1, backlog "Windows support"). Delete
   `.github/workflows/windows-debug.yml` when the investigation ends.

## Watch out for

- Windows is not supported. The CI job `Windows (best effort)` is allowed to
  fail, so a red Windows job does not block a merge.
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
