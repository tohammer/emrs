# Architecture

This document describes the system as it is now. The reasons behind it are
in the specifications in `docs/specs/` (the core is
`docs/specs/2026-10-03-emrs-core.md`) and the decisions in `docs/decisions/`.

## Overview

emrs lets authors write Emacs dynamic modules in Rust as message-passing
services. A module author implements the `Service` trait and adds one
`emrs::module!` line; the author never sees `unsafe`, `emacs_env` or
`emacs_value`. Service code runs only on runtime threads, never on the Emacs
thread. Messages are `serde` types that are converted directly to and from
Lisp values.

The project has two parts: the Rust crate `emrs` (dependencies `serde` and
`libc` only) and the Elisp library `elisp/emrs.el` (Emacs 30.1 or later).
The build tool `cargo-emacs` is a separate crate that modules do not link.
Every module statically links its own copy of `emrs` and so has its own
runtime. Modules built against different `emrs` versions coexist in one
Emacs.

## Components

| Component | Responsibility | Depends on |
|---|---|---|
| `emrs/src/ffi/` (`sys.rs`, `env.rs`) | Private hand-written bindings to the Emacs module API. `env.rs` checks for non-local exits after every call that can raise one | Emacs module ABI |
| `emrs/src/ffi/fake.rs` | Fake Emacs environment so the conversion code can be unit-tested under `cargo test` | `ffi` |
| `emrs/src/convert/` (`ser.rs`, `de.rs`) | `serde` serializer and deserializer that work directly on Lisp values (keyword-headed lists, plists) | `ffi`, `names` |
| `emrs/src/names.rs` | Converts Rust identifiers to Lisp keyword names | — |
| `emrs/src/runtime/` | Elastic thread pool (`pool.rs`), cancellation tokens (`cancel.rs`), outbound FIFO and instance table (`core.rs`), instance lifecycle (`instance.rs`), replies and bounded streams (`reply.rs`) | `payload`, `error` |
| `emrs/src/crt.rs` | Writes and closes the wake-up descriptor in the C runtime that owns it; on Windows reads it from the import table of `emacs.exe` (ADR 0001) | `libc`, Windows `kernel32` |
| `emrs/src/module.rs` | The eight protocol functions and the `module!` macro; wraps every body in `catch_unwind` | `ffi`, `convert`, `runtime` |
| `emrs/src/testing.rs` | Host for testing services without Emacs | `runtime` |
| `elisp/emrs.el` | Loads modules and checks their protocol version, owns the wake-up pipe process, drains deliveries and dispatches them to callbacks; public API `emrs-send`, `emrs-call`, `emrs-call-sync`, `emrs-call-stream`, `emrs-call-stream-sync`, `emrs-stop` | module protocol |
| `elisp/emrs-install.el` | Gives a registered package its module: the installed version under `~/.emacs.d/emrs/modules/`, else the release file for the platform checked against `SHA256SUMS` after asking, else a cargo build in the package's checkout; then loads it and warns on a version mismatch (ADR 0006, 0008, 0009). Does not require `emrs.el` until it loads a module | `url`, `compile`, `emrs.el`, release layout (ADR 0004) |
| `cargo-emacs/` | The Cargo subcommand. `build` runs `cargo build`, reads the library path from Cargo's JSON messages and installs it as `<name>-module` plus suffix in `target/emacs/` (ADR 0002). `package` does the same as a release build, optionally with `cargo zigbuild`, and writes `<name>-module-<platform>.<ext>` to `target/emacs/dist/`, with the platform name from the target triple (ADR 0004). Not linked into modules | `cargo`, `serde_json` |
| `.github/workflows/module-release.yml` | Reusable workflow for module repositories: builds the four platforms (ADR 0007) with `cargo emacs package` (Linux with zigbuild against glibc 2.28, ADR 0005), checks the glibc baseline, tests each file in Emacs, and on a tag `v<version>` uploads the files and `SHA256SUMS` to the release (ADR 0004). `release-check.yml` runs it on `examples/echo` without publishing | `cargo-emacs`, GitHub Actions |
| `tests/emrs-test-module/` | `cdylib` with round-trip and misbehaving services for the ERT suite | `emrs` |
| `tests/elisp/`, `tests/run-ert.sh` | ERT suite, run with `--module-assertions` | test module, `emrs.el` |
| `examples/echo`, `examples/grep` | Complete example modules with smoke tests (`examples/run-examples.sh`) | `emrs`, `emrs.el` |

## How it fits together

```text
 Emacs thread                                  Runtime threads (per module)
 emrs.el
   emrs-send / emrs-call ──► module fn ──decode──► mailbox ──► handler (pool)
                                                                  │
   dispatch to callbacks ◄── module fn `drain` ◄── outbound FIFO ◄┘
            ▲                                          │
            └──── pipe process filter ◄── 1 byte ──────┘ (open_channel fd)
```

- **Protocol.** For a module `M`, `emacs_module_init` defines `M--emrs-info`,
  `-attach`, `-start`, `-send`, `-cancel`, `-stop`, `-drain` and `-shutdown`.
  Only `emrs.el` calls them. `info` reports a protocol integer (currently 1);
  `emrs.el` rejects newer protocols and drives older supported ones.
- **Inbound.** A module function decodes the Lisp request on the Emacs thread
  and pushes it to the instance's unbounded mailbox. It never blocks on
  service code, except for the time-bounded `shutdown`.
- **Outbound.** Handlers push deliveries (`event`, `reply`, `error`, `items`,
  `end`, `state`, `log`) to one FIFO per module. On the transition of an
  atomic wake-pending flag from clear to set, the runtime writes one byte to
  the descriptor obtained with `open_channel` from a pipe process.
- **Drain.** The pipe filter calls `drain`, which converts at most `MAX`
  deliveries to Lisp values. If the FIFO is still not empty, `drain` re-arms
  the wake-up so that Emacs returns to its command loop between batches.
  Consecutive items of one stream are merged into a single `items` delivery.
- **Synchronous wait.** `emrs-call-sync` and `emrs-call-stream-sync` loop
  over drain, check and `accept-process-output` on the module's pipe only
  (JUST-THIS-ONE `t`), in slices of at most 0.1 s. The filters of other
  processes do not run; timers do. During that call the pipe filter records
  the wake-up and does not drain, so every wake-up returns control to the
  loop, which tests completion and the deadline (ADR 0003).
- **Backpressure.** Each stream has a capacity. `send` blocks the producer
  once that many items are in flight, and an item stops counting when `drain`
  converts it. Emacs → Rust queues are unbounded.
- **Lifetimes.** Lisp values exist only inside one module function call on
  the Emacs thread. The runtime holds no global references, user-ptrs or
  finalizers.
- **Instance lifecycle.** An instance moves from starting to running, then to
  stopping, and ends stopped or failed. Every open call is completed, and
  `on-exit` runs once. A panic fails only its instance.

## Lessons

- `accept-process-output` with a process argument keeps reading that process
  for as long as output keeps arriving, whatever its timeout. A filter that
  drains makes the module produce the next wake-up, so one call asked for
  0.1 s lasted up to 1.6 s on the CI runners and timeouts of the synchronous
  calls overshot by seconds (ADR 0003).
- Before Emacs 31, `process-adaptive-read-buffering` delays reading after
  one-byte reads by up to 50 ms. That would throttle every stream, so the
  wake-up pipe is created with the option bound to nil.
- The descriptor is closed only when a new pipe is attached, under the lock
  that the wake-up write also takes, so a wake-up never writes to a reused
  descriptor number. `shutdown` leaves the descriptor open, because the user
  may cancel the exit.
- A descriptor from `open_channel` is valid only in Emacs's C runtime. On
  Windows, official Emacs uses `msvcrt.dll` and Rust modules use UCRT, with
  either target; using the wrong one ended Emacs with `0xC0000409` in
  `ucrtbase.dll` (ADR 0001, spec section 13.1).
- Under Git Bash, a crashed native Windows process shows up only as exit
  code 127. Run it from PowerShell for the real status, and read the
  Application event log for the faulting module.
- `purcell/setup-emacs` installs Emacs through Nix, and nixpkgs dropped
  `x86_64-darwin` in 26.11, so it cannot test on Intel Macs; this is why
  macOS x86_64 is not a release platform (ADR 0007).
- A module built on a current runner needs the runner's glibc (2.34 or
  newer for a plain `cargo build` on Ubuntu 24.04). zigbuild with
  `<triple>.2.28` keeps it at 2.28; the release workflow checks this with
  `readelf` (ADR 0005).
- `copy_string_contents` returns the raw bytes of unibyte strings, including
  bytes above 127. No fallback is needed.
