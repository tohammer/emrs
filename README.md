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

- `emrs`: the Rust crate for services.
- `emrs.el`: the Elisp library that starts services and calls them
  synchronously, asynchronously or as streams.
- `cargo emacs`: builds a module and installs it under the name Emacs loads.
- `emrs-install.el` and a reusable GitHub workflow: prebuilt modules for
  the users of a package, so they do not need Rust.

## Status

The core is complete and tested. Requires Emacs 30.1 or later and Rust 1.85
or later. emrs is not on crates.io; use it from Git.

| Platform | Status |
|---|---|
| Linux x86_64 and aarch64 | Supported; tested in CI with Emacs 30.1 and 31.1 |
| macOS aarch64 | Supported; tested in CI with Emacs 30.1 and 31.1 |
| macOS x86_64 | Builds in CI; not tested; no prebuilt modules |
| Windows x86_64 | Best effort: tested in CI with the official Emacs 30.1 build and the MSVC Rust target; MSYS2 UCRT64 Emacs not tested |

## Quick start

Requirements: Rust 1.85 or later, Emacs 30.1 or later, Git.

1. Install `cargo emacs`:

   ```sh
   cargo install --git https://github.com/tohammer/emrs --tag v0.2.0 cargo-emacs
   ```

2. Create a library crate:

   ```sh
   cargo new --lib echo-rs
   cd echo-rs
   ```

3. Add to `Cargo.toml`:

   ```toml
   [lib]
   crate-type = ["cdylib"]

   [dependencies]
   emrs = { git = "https://github.com/tohammer/emrs", tag = "v0.2.0" }
   serde = { version = "1", features = ["derive"] }
   ```

4. Replace `src/lib.rs` with:

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

5. Build the module:

   ```sh
   cargo emacs build
   ```

   It prints the path of the module, `target/emacs/echo-rs-module.so` on
   Linux.

6. Get `emrs.el` of the same release and call the service:

   ```sh
   git clone --depth 1 --branch v0.2.0 https://github.com/tohammer/emrs ../emrs
   emacs -Q --batch -L ../emrs/elisp -L target/emacs --eval '
     (progn
       (require (quote emrs))
       (let ((e (emrs-start (quote echo-rs) "echo" nil)))
         (message "%S" (emrs-call-sync e (quote (:echo "hi"))))))'
   ```

   Output:

   ```text
   "hi"
   ```

The request `(:echo "hi")` became `Request::Echo("hi")`. How Lisp values
map to Rust types is in [the reference](docs/user/reference.md#data-mapping).

## Building a module

A module is a `cdylib` crate that depends on `emrs` and ends with
`emrs::module!("<module>", [<Service>, ...])`. Depend on a release tag;
`master` is the development version. Keep `panic = "unwind"` (the default);
the build fails with `panic = "abort"`.

Emacs loads a module from the file `<module>-module` plus
`module-file-suffix` on `load-path`. `cargo emacs build` builds the crate
and installs the library under that name:

```sh
cargo emacs build --release   # prints target/emacs/echo-rs-module.so
```

- The library goes to `emacs/` in the target directory; `--dir <path>`
  chooses another directory. Add that directory to `load-path`.
- All other options are passed to `cargo build`, for example `--release`,
  `-p <package>` or `--target <triple>`. Without `--release` the build is a
  debug build.
- The tool takes the module name from the package name. If the name in
  `emrs::module!` differs, state it in `Cargo.toml`:

  ```toml
  [package.metadata.emacs-module]
  name = "echo-rs"
  ```

Without the tool, run `cargo build --release` and copy the library yourself,
for example `target/release/libecho_rs.so` to `echo-rs-module.so`.

Emacs cannot unload a module; restart Emacs to load a new build.

A package that uses emrs ships `emrs.el` and `emrs-install.el` with its own
Elisp. In a package recipe, include both, for example
`:files ("elisp/*.el")`.

## Calling services from Elisp

`emrs-start` loads the module if needed and returns an instance at once;
requests sent while it starts are queued. The third argument is decoded to
the service's `Args` type. Stop the instance with `emrs-stop`.

```elisp
(setq e (emrs-start 'echo-rs "echo" nil
                    :on-exit (lambda (_inst status err)
                               (message "echo ended: %s %S" status err))))
(emrs-stop e)
```

### Call and wait

```elisp
(emrs-call-sync e '(:echo "hi") 1.0)   ; => "hi"
```

The optional timeout is in seconds. `C-g` cancels the call. While it waits,
other callbacks of the same module can run.

### Call without waiting

```elisp
(emrs-call e '(:echo "hi")
           (lambda (reply err)
             (if err
                 (message "failed: %s" (plist-get err :message))
               (message "got %S" reply))))
```

The callback runs exactly once. `emrs-send` sends a request without
expecting an answer.

### Receive a stream

The service answers with `reply.stream()` and sends items from a spawned
task (see `examples/grep`). Elisp receives them in batches:

```elisp
(setq call
      (emrs-call-stream g '(:grep :pattern "TODO" :root "/src")
        :on-batch (lambda (items) (my-insert-matches items))
        :on-end (lambda (status err) (message "grep: %s" status))))
(emrs-cancel call)   ; for example when the user changes the query
```

Each item is a plist such as
`(:path "/src/a.rs" :line-number 42 :preview "TODO first")`.
`emrs-call-stream-sync` waits for the end and returns all items as one
list.

### Receive events

A service sends events with `ctx.emit(event)`, without a request:

```elisp
(emrs-start 'my-rs "index" nil
            :on-event (lambda (_inst event) (message "event: %S" event)))
```

### Handle errors

The synchronous calls signal `emrs-error` with the data
`(KIND MESSAGE DATA)`:

```elisp
(condition-case err
    (emrs-call-sync g '(:grep :pattern "x" :root "/no/such/dir") 5.0)
  (emrs-error (message "%s: %s" (nth 1 err) (nth 2 err))))
```

The asynchronous calls pass the same information as a plist
`(:kind KIND :message MESSAGE :data DATA)`. The error kinds are listed in
[the reference](docs/user/reference.md#error-kinds). An error in your
callback is logged to `*emrs-log*` and shown as a warning; it does not stop
other deliveries.

## Shipping prebuilt modules

Users of a package should not need a Rust toolchain. A reusable GitHub
workflow builds the module for Linux, macOS and Windows and attaches it to
the release of the tag `v<version>`. In the package, `emrs-install.el`
loads the module: an installed copy, a checked download of the release or
a local cargo build.

```elisp
(require 'emrs-install)
(emrs-install-register 'echo-rs
  :version "0.2.0"
  :url "https://github.com/OWNER/REPO/releases/download"
  :source (or load-file-name buffer-file-name))

(emrs-install-load 'echo-rs)
```

Setup, versions, workflow inputs and installer options:
[Shipping prebuilt modules](docs/user/distributing.md).

## Reference

- [Elisp API, error kinds and data mapping](docs/user/reference.md).
- Rust API: run `cargo doc -p emrs --open` in a module crate.
- Examples: `examples/echo` and the streaming `examples/grep`.

## Limitations

- Emacs cannot unload a module. Restart Emacs to load a new build; emrs
  warns when the loaded file changed on disk.
- All calls go through messages. There are no synchronous exported
  functions and no Lisp objects that hold Rust data; keep large data in the
  service and request the parts you need.
- `nil` stands for `None`, `false`, `()` and an empty list alike, so
  `Option<bool>` and `Option<Vec<T>>` lose the difference. Use an enum
  instead.
- `#[serde(flatten)]` is not supported.
- A running instance lives until `emrs-stop`, a failure or Emacs exit; it is
  not garbage-collected.
- No prebuilt modules for Intel Macs; they build the module locally.

## Troubleshooting

**`Module <module> uses emrs protocol <n> but this emrs.el supports 1 to <m>`**:
the module was built with a newer emrs than the loaded `emrs.el`. Load the
`emrs.el` of the emrs version the module depends on.

**`<file> changed on disk; Emacs cannot unload modules, restart to use the new build`**:
you rebuilt a module that is already loaded. Restart Emacs.

**`Cannot open load file: No such file or directory, <module>-module`**:
the module file is not on `load-path`. Add the directory that
`cargo emacs build` printed.

## Development

```sh
cargo test --workspace     # runtime and data mapping, no Emacs needed
tests/run-ert.sh           # end to end in Emacs, with module assertions
examples/run-examples.sh   # the example modules
```

Set `EMACS` to run the Emacs suites with another binary, for example
`EMACS=/path/to/emacs-30 tests/run-ert.sh`. CI runs all of this on pull
requests and on pushes to `master`. When `cargo-emacs`, `emrs`,
`elisp`, `examples/echo`, the workspace's `Cargo.toml` or `Cargo.lock`, or
`.github/workflows/module-release.yml` or `release-check.yml` change, the
workflow "Release check" also runs the release workflow on `echo` for all
four platforms, without publishing; it can also be started by hand.

Repository layout: `emrs/` (the crate), `elisp/` (`emrs.el`,
`emrs-install.el`), `cargo-emacs/`, `.github/workflows/module-release.yml`,
`examples/`, `tests/`.

Design documents:

- `docs/specs/2026-10-03-emrs-core.md`: the core specification, including
  the module protocol (section 6) and open issues (section 13).
- `docs/specs/`: the specifications of later features, such as
  `cargo emacs build`, the release pipeline and the module installer.
- `docs/decisions/`: the decisions behind them (ADRs).
- `docs/architecture.md`: how the system is built now.
- `docs/implementation-plan.md`: the plan the first version was built from;
  its code shows the state before review.
- `docs/backlog.md` and `docs/next-steps.md`: open points and what comes next.

## Projects using emrs

- [emrs-dirdiff](https://github.com/tohammer/emrs-dirdiff): fast directory
  tree comparison for Emacs.
- [emrs-quarry](https://github.com/tohammer/emrs-quarry): a fast project
  file finder and an interactive grep for Emacs, both backed by one Rust
  module.

## Licence

The Rust crates are licensed under either MIT or Apache-2.0, at your option.
`elisp/emrs.el` and `elisp/emrs-install.el` are licensed under
GPL-3.0-or-later.

## AI Disclaimer

This project was developed with the help of AI coding agents.
