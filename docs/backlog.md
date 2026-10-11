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

Design notes from the original brainstorming:

- Purpose: keep large data (indexes, parse trees, huge result sets) in Rust
  and create Lisp values only for what Elisp asks for (`get`, `slice`,
  `length`). For large results the cost is building Lisp objects, not
  computing them. This is independent of streaming; the two combine, for
  example a stream of references into a Rust-side result store.
- An accessor is a synchronous module call and blocks Emacs for as long as it
  runs. Keep accessors cheap; anything expensive stays a call or a stream.
  The same applies to synchronous functions such as hashing a file.
- A finalizer runs during garbage collection and must only drop the handle.
  Shutting down what the handle owns happens asynchronously.
- Never hand Lisp a pointer into Rust memory. Accessors copy the requested
  part into a Lisp value.

## Prebuilt artifacts and distribution

Download of prebuilt module binaries with a compile fallback, a CI action,
and an installer. `info` already reports the module version, the `emrs`
version, the protocol and the target triple for diagnostics (spec section 14).

Design notes from the original brainstorming:

- Installing a package from MELPA should not require a Rust toolchain. The
  installer first uses a module that is already present, then downloads a
  prebuilt binary for the platform, and only then offers a local `cargo
  build`.
- The installer is a separate Elisp library, not part of the runtime.
- One binary per platform serves all supported Emacs versions: the module
  API only appends fields, and `emacs_module_init` checks the sizes. The
  open risk is the operating system, mainly the glibc version a Linux binary
  requires. Build against a fixed old glibc baseline (for example with
  `cargo-zigbuild`).
- Test every artifact against every supported Emacs version with
  `--module-assertions`, and publish checksums.
- On a load failure, report what was detected (Emacs version, platform) and
  what the artifact requires, instead of the bare loader error.

## `cargo emacs`: further subcommands and configuration

`cargo emacs build` exists (spec `docs/specs/2026-10-11-cargo-emacs-build.md`).
Ideas from the original brainstorming that were left out: subcommands `test`,
`package` and `release`; further keys under `[package.metadata.emacs-module]`;
platform names such as `linux-x86_64` or a `standard` set instead of raw
target triples; a reusable GitHub Action that builds and tests the release
matrix (`cargo-dist` is a model); publishing the tool to crates.io. Revisit
with the prebuilt artifacts work.

## `cargo emacs build` does not check the module name

The tool installs under the package name or the metadata key and cannot see
the literal in `emrs::module!`; a mismatch shows up in Emacs as a failed
`require` (ADR 0002). Origin: spec of 2026-10-11, left out to avoid
object-file parsing. Revisit if users report confusion about it.

## `cargo emacs build`: deferred review findings

Origin: review of the `cargo emacs build` change (spec of 2026-10-11),
2026-10-11. Deferred as rare and harmless; revisit on a user report.

- A target directory set with `--config build.target-dir=...` is used for the
  build but not for the default install directory, because `cargo metadata`
  does not get the option. `--target-dir` and `CARGO_TARGET_DIR` work.
- A run that is killed between copy and rename leaves a
  `.<name>-module.<ext>.<pid>.tmp` file in the install directory.
- A Cargo exit status outside 0 to 255, or a death by signal, is reported as
  exit status 1.
- Cargo options that print and exit (`--list`, `--version` after `build`) end
  with the "no dynamic library" error.

## Pull-style streams and overflow policies

Consumer-driven stream consumption, and drop or coalesce policies as
alternatives to blocking backpressure. Changing the delivery shapes bumps the
protocol version (spec section 14).

## Async adapters

Adapters for async runtimes. `try_send`, `CancelToken` and all handles already
work from async code (spec section 14).

## Shared state between instances

Instances of a service have private state only. An opt-in way to share a
cache or connection pool between the instances of one module (the
brainstorming sketched `ctx.shared::<T>()`) is not provided; a service can
use its own `static` today. Revisit when a real module needs it.

## Event dispatch helpers in `emrs.el`

`on-event` receives every event and the package dispatches with `pcase`.
Helpers that register a handler per event keyword or pattern were sketched
in the brainstorming and left out. Revisit if packages repeat that dispatch
code.

## Automatic restart of failed services

Not provided; a package can restart a service from `on-exit` today. Revisit
if several packages end up writing the same restart logic.
