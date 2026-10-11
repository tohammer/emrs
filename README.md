# emrs

[![CI](https://github.com/tohammer/emrs/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/tohammer/emrs/actions/workflows/ci.yml)

Write Emacs dynamic modules in Rust as message-passing services.

Rust owns threads and state, Emacs owns its event loop and UI, and typed
messages are the only boundary between them. Service code never runs on the
Emacs thread and never sees the Emacs module API.

It is meant for packages where Emacs Lisp provides the UI and the expensive
part is parsing, searching, indexing or talking to other processes: a Git
backend, an LSP client, grep and file scanning over large trees, a project
index, database access. Such a service keeps its state (caches, indexes,
connections) across calls and sends Emacs only the results it needs.

Status: the core is implemented and tested on Linux and macOS with Emacs 30
and 31. Windows works with the official Emacs build but is tested less (see
below).

- `emrs/` is the Rust crate.
- `elisp/emrs.el` is the companion Elisp library (Emacs 30.1 or later).
- `examples/` holds two complete modules: `echo` and a streaming `grep`.
- `docs/specs/2026-10-03-emrs-core.md` is the full specification.

## A module in 25 lines

```rust
use emrs::{Context, Error, Reply, Service};
use serde::Deserialize;

#[derive(Deserialize)]
enum Request {
    Echo(String),
}

struct Echo;

impl Service for Echo {
    const NAME: &'static str = "echo";
    type Args = ();
    type Request = Request;
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Echo)
    }

    fn handle(&mut self, request: Request, reply: Reply, _: &Context<Self>) {
        match request {
            Request::Echo(text) => reply.ok(text),
        }
    }
}

emrs::module!("echo-rs", [Echo]);
```

```elisp
(require 'emrs)
(setq e (emrs-start 'echo-rs "echo" nil))
(emrs-call-sync e '(:echo "hi") 1.0)   ; => "hi"
```

## Building a module

1. Make the crate a `cdylib` and keep `panic = "unwind"` (the default).
2. `cargo build --release`.
3. Copy the library to a directory on `load-path` under the name
   `<module>-module` plus Emacs's `module-file-suffix`, for example
   `target/release/libecho_rs.so` to `echo-rs-module.so`.

Emacs cannot unload a module; restart Emacs to load a new build.

## Platform support

| Platform | Status |
|---|---|
| Linux x86_64 and aarch64 | Supported; tested in CI with Emacs 30.1 and 31.1 |
| macOS aarch64 | Supported; tested in CI with Emacs 30.1 and 31.1 |
| macOS x86_64 | Builds in CI; not tested |
| Windows x86_64 | Best effort: the ERT suite passes in CI with the official Emacs 30.1 build and the default (MSVC) Rust target. The GNU target passed in a one-off run. MSYS2 UCRT64 Emacs builds are not tested. |

The minimum Rust version is 1.85. The minimum Emacs version is 30.1.

## Tests

```sh
cargo test --workspace     # runtime and data mapping, no Emacs needed
tests/run-ert.sh           # end to end in Emacs, with module assertions
examples/run-examples.sh   # the example modules
```

Set `EMACS` to run the Emacs suites with another binary, for example
`EMACS=/path/to/emacs-30 tests/run-ert.sh`. CI runs all of this on pull
requests and on pushes to `master`.

## Documentation

- `docs/specs/2026-10-03-emrs-core.md`: the specification. What emrs does and
  why, including the Lisp data mapping (section 8), the module protocol
  (section 6) and open issues (section 13).
- `docs/implementation-plan.md`: the task-by-task plan the first version was
  built from. Its code blocks show the state before review; the repository is
  the current source.
- `docs/architecture.md`: how the system is built now.
- `docs/backlog.md` and `docs/next-steps.md`: open points and what comes next.

## Licence

The Rust crates are licensed under either MIT or Apache-2.0, at your option.
`elisp/emrs.el` is licensed under GPL-3.0-or-later.

## AI Disclaimer

This project was developed with the help of AI coding agents.
