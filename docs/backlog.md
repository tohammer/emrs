# Backlog

Open points only. Delete an item when it is done; git keeps the history.
Deferred review findings name their origin, the reason and when to revisit.

## Windows support

Modules crash Emacs on Windows as soon as the first service instance starts.
The CI job `Windows (best effort)` fails on every run and is allowed to fail.
The suspected cause is mismatched C runtimes: the module is built for MSVC,
Emacs for MinGW, and the descriptor from `open_channel` is written from the
MSVC side. Evidence, hypotheses and the planned investigation are in
`docs/specs/2026-10-03-emrs-core.md` section 13.1.

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
