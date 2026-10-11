# 0002 — How `cargo emacs` finds the library and its module name

Date: 2026-10-11
Status: accepted

## Context

`cargo emacs build` must copy a built module to `<name>-module` plus the
platform suffix. It needs three facts: where Cargo put the library, which
suffix Emacs expects, and the module name, which is the literal passed to
`emrs::module!` and not necessarily the package name (the package
`emrs-test-module` defines the module `emrs-test`).

Options for the library path and suffix:

1. Compute the path from the target directory, profile, target triple and
   crate name. This repeats Cargo's rules (`lib` prefix, `-` to `_`, profile
   directory names) and requires the tool to understand every `cargo build`
   option that affects them.
2. Run `cargo build --message-format=json-render-diagnostics` and read the
   file names from the `compiler-artifact` messages.

Options for the module name:

1. Package name, with an override in `[package.metadata.emacs-module]`.
2. Export the name from `module!` and read it from the built library. One
   source of truth, but needs object-file parsing for ELF, Mach-O and PE.
3. Require the metadata key in every module.

## Decision

The tool takes the library path from Cargo's JSON messages and passes all
build options through to `cargo build` without interpreting them. The suffix
is the extension of the file Cargo produced (`so`, `dylib` or `dll`), which
equals Emacs's `module-file-suffix` on each platform, so cross builds work
and Emacs is not run.

The module name is the package name unless
`[package.metadata.emacs-module] name` is set. The user chose this on
2026-10-11.

## Consequences

- The tool knows only its own options and `--manifest-path`; new `cargo
  build` options work without a change.
- The user cannot pass `--message-format`; the tool rejects it.
- A module whose name differs from its package states the name twice, in
  `module!` and in `Cargo.toml`. The tool cannot detect a mismatch; it shows
  up in Emacs as a failed `require` of `<name>-module`.
- The tool depends on `serde_json`. The `emrs` crate keeps its two
  dependencies.
