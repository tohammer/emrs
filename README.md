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
- `elisp/emrs-install.el` gives a package its module: an installed copy, a
  release download or a cargo build.
- `cargo-emacs/` is the build tool (`cargo emacs build`, `cargo emacs package`).
- `.github/workflows/module-release.yml` is a reusable workflow that
  publishes prebuilt modules.
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

emrs is not on crates.io. Depend on a release tag; `master` is the
development version (`X.Y.Z-snapshot`):

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
emrs = { git = "https://github.com/tohammer/emrs", tag = "v0.2.0" }
```

Keep `panic = "unwind"` (the default). Emacs loads a module from the file
`<module>-module` plus its `module-file-suffix` on `load-path`, where
`<module>` is the name given to `emrs::module!`.

`cargo emacs build` builds the crate and installs the library under that
name. Install the tool once, from the same release:

```sh
cargo install --git https://github.com/tohammer/emrs --tag v0.2.0 cargo-emacs
```

Then, in the module crate:

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

## Releasing prebuilt modules

Users of a package should not need a Rust toolchain. emrs defines a layout
for prebuilt modules on a release and a reusable GitHub workflow that
produces it. `emrs-install.el` downloads these files for the users of a
package (see "Installing modules for users"); a file can also be downloaded
by hand and renamed to `<name>-module.<ext>`.

A release of a module is the release tagged `v<version>`, where `<version>`
is the `version` in the module's `Cargo.toml`. It holds one file per
platform and a checksum file (macOS x86_64 is not among them, ADR 0007;
Intel Macs build the module locally):

```text
<name>-module-linux-x86_64.so
<name>-module-linux-aarch64.so
<name>-module-macos-aarch64.dylib
<name>-module-windows-x86_64.dll
SHA256SUMS
```

`SHA256SUMS` has the format of `sha256sum`. Linux files are built against
glibc 2.28 and load on Debian 10, RHEL 8, Ubuntu 18.10 and later.

After a release, set the crate version to the next development version
with the suffix `-snapshot` (for example `1.3.0-snapshot`; ADR 0008). The
installer downloads only the release that matches the version exactly, and
for a `-snapshot` version it does not download at all but builds locally, so
users who follow the default branch never get the binary of the previous
release. Emacs rejects
`-dev` as a version, so do not use it.

### `cargo emacs package`

```sh
cargo emacs package                 # target/emacs/dist/echo-rs-module-linux-x86_64.so
cargo emacs package --target aarch64-apple-darwin
```

- It builds a release build (it adds `--release` unless `--release`,
  `-r` or `--profile` is given) and writes `<name>-module-<platform>.<ext>` to
  `emacs/dist/` in the target directory, or to `--dir <path>`.
- The platform comes from `--target`, or from the host if there is none.
  `--platform <name>` sets it for other targets.
- `--zigbuild` runs `cargo zigbuild` instead of `cargo build`, for example
  `cargo emacs package --zigbuild --target x86_64-unknown-linux-gnu.2.28`.
- It does not write `SHA256SUMS`. For a release by hand, run
  `sha256sum -- *-module-* > SHA256SUMS` in the output directory.

### The release workflow

Add `.github/workflows/release.yml` to the module's repository:

```yaml
name: Release
on:
  push:
    tags: ["v*"]
  workflow_dispatch:
permissions:
  contents: write
jobs:
  release:
    uses: tohammer/emrs/.github/workflows/module-release.yml@v0.2.0
    with:
      package: echo-rs
      cargo-emacs: v0.2.0
      publish: ${{ startsWith(github.ref, 'refs/tags/v') }}
```

Pushing the tag `v<version>` builds every platform, tests each file in Emacs
with `--module-assertions` and uploads the files and `SHA256SUMS` to the
release. If the release does not exist, it is created as a draft, filled and
then published, as a prerelease if the version contains `-`; a release you
created beforehand, draft or not, is kept as it is. A run started by hand
builds and tests without publishing.

Only the publish job gets the write token; the jobs that build and test run
with read access. Pin the workflow and `cargo-emacs` to the same emrs
release, as above, and preferably the one the module's `emrs` dependency
uses; with `@master` the build changes with emrs's development version. A
`rust-toolchain.toml` in the module's repository overrides
`rust-toolchain`; it must then list the targets of the platforms built.

| Input | Default | Meaning |
|---|---|---|
| `package` | `""` | Cargo package of the module; empty for the package in `working-directory` |
| `working-directory` | `"."` | Where Cargo runs |
| `platforms` | all four | JSON array, for example `'["linux-x86_64", "macos-aarch64"]'` |
| `emacs-versions` | `'["30.1", "31.1"]'` | Emacs versions for the tests on Linux and macOS; Windows uses 30.1 |
| `test-command` | `""` | Bash command run after the smoke test for each file |
| `publish` | `false` | Upload to the release of the tag; the tag must be `v<version>` |
| `cargo-emacs` | `master` | Branch, tag or full commit hash of emrs to install `cargo-emacs` from, or `path:<dir>` |
| `rust-toolchain` | `stable` | Rust toolchain for builds and tests |

Each test first loads the file and checks that the module reports its name
and version. A `test-command` then runs in `working-directory` with the Rust
toolchain installed and these variables set: `EMRS_MODULE_FILE` (the file,
renamed to `<name>-module.<ext>`), `EMRS_MODULE_DIR` (its directory, for
`load-path`), `EMRS_MODULE_NAME`, `EMRS_MODULE_VERSION`, and `EMRS_EL_DIR`
(the directory of the `emrs.el` that belongs to the emrs version the module
depends on). The command must not rebuild the module.

## Installing modules for users

`elisp/emrs-install.el` gives a package its module, so that users without
Rust can use it. A package registers its module once, at top level of its
main file, and loads it through the installer:

```elisp
(require 'emrs-install)

(emrs-install-register 'echo-rs
  :version "0.2.0"          ; the crate version, ADR 0008
  :url "https://github.com/OWNER/REPO/releases/download"
  :source (or load-file-name buffer-file-name))

;; where the package needs the module:
(emrs-install-load 'echo-rs)              ; returns the module handle
```

| Key | Meaning |
|---|---|
| `:version` | Required. The crate version; between releases `X.Y.Z-snapshot` |
| `:url` | Base URL of the releases, `https://` (or `file:`); the files of a version are at `<url>/v<version>/` |
| `:source` | A file of the package; the nearest directory above it with a `Cargo.toml` is the checkout to build from |
| `:package` | The Cargo package to build, in a workspace with several |

`emrs-install-load` loads the module and returns its handle. If the module
is not installed yet, it tries these in order:

1. The installed module of this version in
   `emrs/modules/<module>/<version>/` under `user-emacs-directory`, or
   `<module>-module` on `load-path`. In an interactive session, a local
   build that is older than its sources is offered for a rebuild before it
   is loaded.
2. The release file for this platform, checked against `SHA256SUMS`. With
   `emrs-install-download` set to `ask` (the default) the user is asked
   first, `always` downloads without asking, `never` skips this step.
   Skipped for `-snapshot` versions.
3. A build with cargo in the package's checkout. In an interactive session
   the user is asked first; the build runs in the buffer
   `*emrs-install <module>*`, and the command stops and has to be run again
   when the build is done. Not offered in batch mode.

If none works, the error says what was tried. After loading, a module that
reports another version than the registered one gives a warning.

Commands: `M-x emrs-install-module` installs a module again (with a prefix
argument it builds), `M-x emrs-install-build` builds it, and
`M-x emrs-install-clean` deletes versions that are no longer used. User
options (group `emrs-install`): `emrs-install-download` (`ask`, `always`,
`never`; in batch mode `ask` means `never`), `emrs-install-directory`,
`emrs-install-timeout` (seconds without data before a download is
stopped), `emrs-install-cargo-program`.

Package recipes must include both files of emrs, for example
`:files ("elisp/*.el")` instead of `("elisp/emrs.el")`.

For pre-build steps of package managers, `emrs-install-build-sync` builds a
registered module and never signals; a failure becomes a warning. How such
a step finds `emrs-install.el` depends on the package manager and has not
been tried yet.

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
requests and on pushes to `master`. When `cargo-emacs`, `emrs`,
`elisp`, `examples/echo`, the workspace's `Cargo.toml` or `Cargo.lock`, or
`.github/workflows/module-release.yml` or `release-check.yml` change, the
workflow "Release check" also runs the release workflow on `echo` for all
four platforms, without publishing; it can also be started by hand.

## Documentation

- `docs/specs/2026-10-03-emrs-core.md`: the specification. What emrs does and
  why, including the Lisp data mapping (section 8), the module protocol
  (section 6) and open issues (section 13).
- `docs/implementation-plan.md`: the task-by-task plan the first version was
  built from. Its code blocks show the state before review; the repository is
  the current source.
- `docs/specs/`: the specifications of later features, such as
  `cargo emacs build`, the release pipeline and the module installer.
- `docs/decisions/`: the decisions behind them (ADRs).
- `docs/architecture.md`: how the system is built now.
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
