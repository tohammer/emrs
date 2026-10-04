# Backlog

Open points only. Delete an item when it is done; git keeps the history.
Deferred review findings name their origin, the reason and when to revisit.

## Windows CI

The Windows job stays best effort (`continue-on-error`), tests only the
official Emacs 30.1 build (`msvcrt.dll`) with the MSVC target, and does not run
the examples. Not covered: MSYS2 UCRT64 Emacs (the UCRT path of ADR 0001),
the GNU target, Emacs 31. Origin: Windows crash fix, 2026-10-04; the owner
chose to keep the job best effort. Revisit when Windows users report
problems or before claiming Windows support.

## Windows: closing the descriptor bypasses Emacs's bookkeeping

`replace_fd` closes the old wake-up descriptor with the C runtime's `_close`.
Emacs on Windows tracks descriptors in its own `fd_info` table and closes
them through `sys_close`; whether a raw `_close` leaves a stale entry that
affects a later descriptor with the same number has not been checked against
the Emacs w32 sources. It only happens when the pipe process is replaced.
Origin: review of PR #2 (Windows crash fix), 2026-10-04; deferred because it
predates the fix and is untested. Revisit with the Windows CI work or on a
report of misbehaving processes after a pipe repair.

## Resource handles and synchronous functions

Opaque native objects (`user-ptr` with finalizers) and synchronous exported
functions. Deferred from the core version. The FFI layer is private, so these
can be added without breaking the public API (spec section 14).

## Prebuilt artifacts and distribution

Download of prebuilt module binaries with a compile fallback, a CI action,
and an installer. `info` already reports the module version, the `emrs`
version, the protocol and the target triple for diagnostics (spec section 14).

## `cargo emacs` build tooling

A tool that builds a module and installs it under the name Emacs expects
(`<name>-module` + suffix). Today this copy is a manual step (README,
"Building a module").

## Pull-style streams and overflow policies

Consumer-driven stream consumption, and drop or coalesce policies as
alternatives to blocking backpressure. Changing the delivery shapes bumps the
protocol version (spec section 14).

## Async adapters

Adapters for async runtimes. `try_send`, `CancelToken` and all handles already
work from async code (spec section 14).

## Automatic restart of failed services

Not provided; a package can restart a service from `on-exit` today. Revisit
if several packages end up writing the same restart logic.
