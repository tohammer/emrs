# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

1. Windows crash, next experiment: on the branch `debug/windows-crash`,
   write and close the wake-up descriptor through `msvcrt.dll` (resolved
   with `GetModuleHandle`/`GetProcAddress`) instead of `libc`, and rerun the
   diagnostics workflow. Switching to the GNU target did not help, because it
   links UCRT as well (spec section 13.1, backlog "Windows support"). Delete
   `.github/workflows/windows-debug.yml` when the investigation ends.

## Watch out for

- Windows is not supported. The CI job `Windows (best effort)` is allowed to
  fail, so a red Windows job does not block a merge.
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
