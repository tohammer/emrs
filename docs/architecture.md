# Architecture

This document describes the system as it is now. The specification with the
reasons behind it is `docs/specs/2026-10-03-emrs-core.md`.

## Overview

emrs lets authors write Emacs dynamic modules in Rust as message-passing
services. A module author implements the `Service` trait and adds one
`emrs::module!` line; the author never sees `unsafe`, `emacs_env` or
`emacs_value`. Service code runs only on runtime threads, never on the Emacs
thread. Messages are `serde` types that are converted directly to and from
Lisp values.

The project has two parts: the Rust crate `emrs` (dependencies `serde` and
`libc` only) and the Elisp library `elisp/emrs.el` (Emacs 30.1 or later).
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
| `emrs/src/module.rs` | The eight protocol functions and the `module!` macro; wraps every body in `catch_unwind` | `ffi`, `convert`, `runtime` |
| `emrs/src/testing.rs` | Host for testing services without Emacs | `runtime` |
| `elisp/emrs.el` | Loads modules and checks their protocol version, owns the wake-up pipe process, drains deliveries and dispatches them to callbacks; public API `emrs-send`, `emrs-call`, `emrs-call-sync`, `emrs-call-stream`, `emrs-stop` | module protocol |
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

- Before Emacs 31, `process-adaptive-read-buffering` delays reading after
  one-byte reads by up to 50 ms. That would throttle every stream, so the
  wake-up pipe is created with the option bound to nil.
- The descriptor is closed only when a new pipe is attached, under the lock
  that the wake-up write also takes, so a wake-up never writes to a reused
  descriptor number. `shutdown` leaves the descriptor open, because the user
  may cancel the exit.
- On Windows, a module crashes Emacs at the first successful instance start
  (`0xC0000409` in `ucrtbase.dll`). Emacs uses `msvcrt.dll`, but an MSVC-built
  module writes the descriptor from `open_channel` through `ucrtbase.dll`.
  A descriptor is only valid in the C runtime that created it. Under Git Bash,
  a crashed native process shows up only as exit code 127, so run it from
  PowerShell to see the real status. Spec section 13.1 has the evidence.
- `copy_string_contents` returns the raw bytes of unibyte strings, including
  bytes above 127. No fallback is needed.
