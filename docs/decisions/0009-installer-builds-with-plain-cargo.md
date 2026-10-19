# 0009 — The installer builds with plain cargo and finds the library through `cargo metadata`

Date: 2026-10-19
Status: accepted

## Context

When no release file fits, `emrs-install.el` builds the module in the
package's checkout (ADR 0006). quarry and emrs-dirdiff do this today with
`cargo build --release` and a library name written into their Elisp
(`libquarry.so`, `quarry.dll`), which repeats Cargo's naming rules.

`cargo emacs build` knows where the library is, but users of a package do
not have it installed, and installing it would mean another `cargo install`
and network access.

## Decision

The installer runs `cargo build --release --target-dir <checkout>/target`
(with `-p <package>` if the package names one) and takes the library from
the cdylib target in `cargo metadata --no-deps --format-version 1`: the file
`<target-dir>/release/<prefix><name><suffix>`, where `<name>` is the
target's name with `-` replaced by `_`, `<prefix>` is `lib` except on
Windows and `<suffix>` is `module-file-suffix`. It does not use
`cargo-emacs`.

## Consequences

- Users need only Rust, as today.
- The installer repeats a small part of Cargo's naming rules (prefix, `_`,
  suffix) for the release profile and the host target only. Cross builds and
  other profiles are out of its scope.
- The explicit `--target-dir` keeps `CARGO_TARGET_DIR` and Cargo
  configuration from moving the library away from where the installer looks.
