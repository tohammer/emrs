# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

- Make the synchronous wait read the module's pipe only. `emrs--wait`
  (`elisp/emrs.el`) calls `(accept-process-output (emrs--module-pipe m)
  TIMEOUT)` with two arguments, so Emacs still reads the output of every
  other process and runs their filters while `emrs-call-sync` and
  `emrs-call-stream-sync` wait. The docstring of `emrs-call-stream-sync`
  says the opposite ("the filters of other processes do not run"). Checked
  on Emacs 31.1 with two plain processes, not with emrs itself: with two
  arguments the other process's filter ran, with `nil t` as third and
  fourth argument (JUST-THIS-ONE) it did not. To do: pass `nil t`, add a
  test in `tests/elisp/emrs-tests.el` in which another process writes
  during the wait and its filter runs only afterwards, and decide whether
  timers should wait too (they still run with `t`; an integer as
  JUST-THIS-ONE holds them back as well). Check the docstring of
  `emrs-call-sync` and `docs/architecture.md` against the result.
  Found on 2026-10-13 from quarry: its PR #2 (tohammer/emrs-quarry)
  moves `project-files` to `emrs-call-stream-sync` for exactly this
  property and waits for the fix; afterwards it pins the fixing commit.

After that, pick from `docs/backlog.md`.

## Watch out for

- The CI job `Windows (best effort)` is allowed to fail, so a red Windows job
  does not block a merge. Check it anyway: it passes since the fix of ADR 0001
  (PR #2).
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
