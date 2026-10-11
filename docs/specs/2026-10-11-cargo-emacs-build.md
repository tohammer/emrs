# `cargo emacs build`

Status: implemented
Date: 2026-10-11

## Purpose

Module authors build a module and get it under the name Emacs loads with one
command. Today the README ("Building a module") tells them to copy
`target/release/libecho_rs.so` to `echo-rs-module.so` by hand, and the
repository's own scripts repeat that copy in shell loops that guess the file
name.

Success: `cargo emacs build` in a module crate produces
`target/emacs/<name>-module.<suffix>`, on Linux, macOS and Windows, and the
ERT and example suites of this repository use it instead of their copy loops.

## Scope

In:

- A new workspace crate `cargo-emacs` with the binary `cargo-emacs`, used as
  `cargo emacs build`.
- Building one or more module crates and installing each library under
  `<name>-module.<suffix>`.
- The key `name` in `[package.metadata.emacs-module]`.
- Use of the tool in `tests/run-ert.sh` and `examples/run-examples.sh`.

Out (they stay in `docs/backlog.md`):

- The subcommands `test`, `package` and `release`.
- Further metadata keys, platform names such as `linux-x86_64`, a reusable
  GitHub Action.
- Publishing to crates.io. The tool is installed from the repository.
- Checking that the installed name matches the `module!` literal (ADR 0002).

## Design

### Command

```text
cargo emacs build [--dir <path>] [<cargo build options>...]
cargo emacs --help | --version
```

Cargo starts the binary as `cargo-emacs emacs build ...`. The binary also
accepts `cargo-emacs build ...` without the leading `emacs`.

`build` does this:

1. Split the arguments. The tool's own option is `--dir <path>` or
   `--dir=<path>`; the value of the first form must not be empty or start
   with `-`. It also reads, without removing them, `--manifest-path` and
   `--target-dir`. `--help` or `-h` anywhere after `build` shows the tool's
   help.
   Everything else is passed to `cargo build` unchanged and in order, so
   `--release`, `--profile`, `-p`, `--target`, `--features` and options added
   to Cargo later work without the tool knowing them. `--message-format` is
   rejected.
2. Run `cargo metadata --format-version 1 --no-deps`, with `--manifest-path`
   if given. It supplies the target directory, the workspace members and each
   member's `metadata` table.
3. Run `cargo build --message-format=json-render-diagnostics <passed
   options>`. Standard error is inherited, so compiler diagnostics and
   progress appear as usual. If the build fails, exit with Cargo's exit code
   and install nothing.
4. From the `compiler-artifact` messages, take those whose package is a
   workspace member and whose target has the crate type `cdylib`. The library
   is the entry of `filenames` with the extension `so`, `dylib` or `dll`.
5. For each such library, copy it to `<dir>/<name>-module.<ext>`, where
   `<ext>` is the extension Cargo produced, and print the absolute
   destination path on standard output, one per line.

The Cargo executable is the value of the environment variable `CARGO`, which
Cargo sets for subcommands, or `cargo` if it is unset.

The profile follows Cargo: without `--release` the build is a debug build.

### Module name

`<name>` is `package.metadata.emacs-module.name` if present, otherwise the
package name. It must equal the literal in `emrs::module!`.

```toml
[package.metadata.emacs-module]
name = "emrs-test"
```

The name must be a non-empty string without `/`, `\` or NUL. The reasons for
this approach are in [ADR 0002](../decisions/0002-cargo-emacs-finds-library-and-name.md).

### Destination

`<dir>` is `--dir` if given, relative to the current directory, otherwise
`<target-directory>/emacs`. The target directory is the value of
`--target-dir` if given, otherwise the one `cargo metadata` reports. The directory is created if missing. The same
directory is used for all profiles and targets; a later build replaces the
file.

The copy goes to a temporary file in the destination directory, which is then
renamed over the destination. A running Emacs that has the old file loaded
keeps its mapping of the old file instead of seeing it change in place. On
Windows a loaded DLL cannot be replaced; the rename fails and the tool
reports it (see Error handling).

### Crate

`cargo-emacs/` in the workspace root, a member of the workspace, with the
workspace's edition, Rust version and license. Its only dependency is
`serde_json`; arguments are parsed by hand. The logic that can be tested
without running Cargo (argument splitting, name resolution, artifact
selection from message lines) is in functions separate from `main`.

Installation, for the README:

```sh
cargo install --git https://github.com/tohammer/emrs cargo-emacs
```

### Use in this repository

- `tests/emrs-test-module/Cargo.toml` gets
  `[package.metadata.emacs-module] name = "emrs-test"`.
- `tests/run-ert.sh` and `examples/run-examples.sh` replace `cargo build`
  and the copy loops with
  `cargo run -q -p cargo-emacs -- build -p <package>...` and put
  `target/emacs` on the load path instead of `target/elisp`.
- The CI job "Minimum supported Rust" also checks `cargo-emacs`.

## Error handling

Every error is one message on standard error that starts with
`cargo emacs: `, and a non-zero exit status. Nothing is installed unless the
whole build succeeded and every name is valid.

| Situation | Behaviour |
|---|---|
| No subcommand, unknown subcommand, `--dir` without a value | Usage message, exit status 2 |
| `--message-format` among the options | Message that the tool sets this option itself, exit status 2 |
| `cargo metadata` fails | Its error output is shown; exit with its status |
| `cargo build` fails | Exit with its status |
| The build produced no `cdylib` of a workspace member | Message that names the fix: `crate-type = ["cdylib"]` under `[lib]`; exit status 1 |
| `name` in the metadata is not a string, is empty or contains a path separator | Message with the package and the value; exit status 1 |
| Two built packages resolve to the same module name | Message that names both packages; exit status 1 |
| Creating the directory, copying or renaming fails | Message with the path and the system error; the temporary file is removed; exit status 1. On Windows the message adds that Emacs may have the module loaded and must be closed first |

## Testing

- Unit tests in `cargo-emacs`: argument splitting (`--dir` in both forms,
  pass-through order, rejection of `--message-format`, optional leading
  `emacs`), name resolution (default, override, invalid values), artifact
  selection from recorded message lines (cdylib of a member, non-member,
  `rlib` only, Windows-style file list with `.dll` and `.dll.lib`).
- Integration tests in `cargo-emacs/tests/` that run the built binary against
  a fixture crate created in a temporary directory. The fixture is a `cdylib`
  without dependencies and with its own `[workspace]` table, so the tests
  need no network and no Emacs. Covered: default destination and printed
  path; `--dir`; the metadata override; a crate without `cdylib`; a build
  error returns Cargo's status and installs nothing; a second build replaces
  the file.
- End to end: `tests/run-ert.sh` and `examples/run-examples.sh` install their
  modules with the tool, so the CI Emacs jobs on Linux, macOS and Windows
  load what it installed.
- Not tested: cross builds with `--target`, and the Windows message for a
  loaded DLL.

## Tasks

- [x] 1. Create the crate `cargo-emacs` with `build`, `--help` and
  `--version` as designed, and its unit and integration tests. Add it to the
  workspace members. *Done when* `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass, and
  `cargo run -q -p cargo-emacs -- build -p echo-rs` prints the path of an
  existing `target/emacs/echo-rs-module.<suffix>`.
- [x] 2. Use the tool in the repository: the metadata key in
  `tests/emrs-test-module/Cargo.toml`, both scripts as designed, and
  `cargo check -p emrs -p cargo-emacs` in the CI job "Minimum supported
  Rust". *Done when* `tests/run-ert.sh` and `examples/run-examples.sh` pass
  locally, neither script contains a copy loop or `target/elisp`, and
  `cargo +1.85 check -p cargo-emacs` passes (or, without that toolchain
  locally, the CI job does).
- [x] 3. Documentation: README "Building a module" (install command,
  `cargo emacs build`, `--dir`, the metadata key, the manual copy as the
  fallback without the tool) and the list of repository parts; a
  `cargo-emacs` row in the components table of `docs/architecture.md`; the
  backlog item reduced to what remains; the doc comment of `module!` in
  `emrs/src/module.rs` mentions the tool. *Done when* the README commands
  match the tool's `--help` and no document describes the manual copy as the
  only way.
