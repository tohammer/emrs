# 0003 — The synchronous wait reads only the module's pipe and drains itself

Date: 2026-10-13
Status: accepted

## Context

`emrs--wait`, the loop behind `emrs-call-sync` and `emrs-call-stream-sync`,
slept in `(accept-process-output PIPE SECONDS)`. Two defects came from that
call:

1. Without JUST-THIS-ONE, Emacs reads every process and runs their filters
   during the wait. `emrs-call-stream-sync` exists so that a package can wait
   without that (spec section 5.2), and its docstring promised it.
2. With a process argument, `accept-process-output` keeps reading that
   process for as long as output keeps arriving, whatever its timeout. The
   pipe filter drained the module, the drain released stream capacity or
   re-armed the wake-up, and the next byte arrived before the call returned.
   One call asked for 0.1 s lasted up to 1.6 s on the CI runners. Neither the
   deadline nor the completion of the call was checked meanwhile, so a
   timeout of 0.2 s on an endless stream took 1 to 6 s, and a reply during an
   event flood was returned only when the flood was over.

Options for JUST-THIS-ONE:

1. `t`: only the pipe is read; timers still run.
2. An integer: timers are held back as well.

Options for the overshoot:

1. The pipe filter does not drain while the wait reads the pipe; the wait
   loop drains on every turn.
2. Leave the wait with a `throw` from the filter. This unwinds whatever else
   runs inside that `accept-process-output`, for example a timer.
3. Enforce the deadline with a timer. It does not help a call that completed
   during a flood, and it needs timers to run.

## Decision

JUST-THIS-ONE is `t`. A wait can last as long as the service takes, and
stopping all timers for that time would freeze `with-timeout`, idle work and
the timer by which `emrs--drain` resumes after a quit in a callback.

The filter does not drain while `emrs--waiting` is bound to its module; it
sets the module's `woken` flag. The runtime writes one byte per transition of
its wake-pending flag and clears that flag only in `drain`, so no further
byte arrives and `accept-process-output` returns at once. The loop drains
before every test of the completion predicate. When the wait is left by a
non-local exit with `woken` set, it schedules a drain with a timer, because
the byte is read and the runtime will not write another one.

## Consequences

- A timeout is exceeded by at most one drain of `emrs-drain-max` deliveries
  and their callbacks.
- Process output of other packages is not handled during a synchronous wait.
  A callback or timer that needs it must not be run from inside one.
- Code that runs from a timer during the wait and itself waits for this
  module with `(accept-process-output nil ...)` sees no deliveries until the
  synchronous wait takes its next turn, at most 0.1 s later.
- The lost wake-up after a quit is covered only by
  `emrs-test-stream-sync-quit-cancels-the-stream`, which hits the window in
  about one run in twelve without the recovery.
