# emrs

Write Emacs dynamic modules in Rust as message-passing services.

Rust owns threads and state, Emacs owns its event loop and UI, and typed
messages are the only boundary between them. Service code never runs on the
Emacs thread and never sees the Emacs module API.

- `emrs/` is the Rust crate.
- `elisp/emrs.el` is the companion Elisp library (Emacs 30.1 or later).
- `examples/` holds two complete modules: `echo` and a streaming `grep`.
- `docs/specification.md` is the full specification.

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

## Tests

```sh
cargo test --workspace     # runtime and data mapping, no Emacs needed
tests/run-ert.sh           # end to end in Emacs, with module assertions
examples/run-examples.sh   # the example modules
```

## Licence

The Rust crates are licensed under either MIT or Apache-2.0, at your option.
`elisp/emrs.el` is licensed under GPL-3.0-or-later.
