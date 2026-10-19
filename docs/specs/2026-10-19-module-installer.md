# Installer for prebuilt modules

Status: approved
Date: 2026-10-19

## Purpose

Packages built on emrs should work without a Rust toolchain. The release
pipeline ([spec of 2026-10-17](2026-10-17-release-pipeline.md)) publishes a
tested module file per platform. This spec adds the consuming half: the
Elisp library `elisp/emrs-install.el`, which gives a package its module.

Today quarry and emrs-dirdiff each carry about 300 lines that find their
repository, run cargo, install the library by rename, offer a rebuild when
the module is older than its sources, and explain failures. The installer
replaces all of that with one registration form and one call, and adds the
download.

Success:

- In an Emacs without Rust, a package registered with a released version
  gets its module with one confirmation, and the command that needed it
  continues.
- A package used from a checkout of an unreleased (`-snapshot`) version gets
  a local build, as today.
- A failure says what was detected and what to do.

## Scope

In:

- `elisp/emrs-install.el`: registration, lookup, download with checksum,
  local build (asynchronous and synchronous), loading with a version check,
  the commands `emrs-install-module` and `emrs-install-clean`, and the user
  options.
- The version convention of [ADR 0008](../decisions/0008-module-version-is-one-string-with-snapshot-suffix.md)
  in the README.
- ERT tests that run offline, and a manual check against emrs-dirdiff's
  release v0.1.0.

Out:

- Moving quarry and emrs-dirdiff to the installer. Each does it in its own
  repository afterwards (backlog items there).
- Asynchronous downloads, signatures, mirrors, proxies beyond what `url.el`
  does by itself.
- Cross builds and profiles other than release in the local build.
- Packaging for MELPA.

## Design

### Placement

`elisp/emrs-install.el` is a second file of the emrs Elisp package
(`;; Package-Requires: ((emacs "30.1"))`, GPL like `emrs.el`). It does not
require `emrs` at load time, so a package manager's pre-build step can load
it alone; `emrs-install-load` requires `emrs` when it is called. Recipes
that list `elisp/emrs.el` alone must list `elisp/*.el` instead; the README
says so.

### Registration

A package registers its module once, at top level of its main file:

```elisp
(emrs-install-register 'emrs-dirdiff
  :version "0.2.0-snapshot"
  :url "https://github.com/tohammer/emrs-dirdiff/releases/download"
  :source (or load-file-name buffer-file-name))
```

| Key | Required | Meaning |
|---|---|---|
| `:version` | yes | Module version, equal to the crate version and valid for `version-to-list` (ADR 0008) |
| `:url` | no | Base URL of the releases; the files of a version are at `<url>/v<version>/<file>` (ADR 0004). Without it, nothing is downloaded |
| `:source` | no | A file of the package. The checkout is the nearest directory above the true name of this file (of the `.el` next to a `.elc`) that contains `Cargo.toml`. Without it, or without such a directory, nothing is built |
| `:package` | no | Cargo package to build (`-p`), for a workspace with several packages |

The module symbol is the name given to `emrs::module!`. Registering again
replaces the entry. `emrs-install-register` signals an error for a missing
or invalid `:version`.

### Files

- Install directory: user option `emrs-install-directory`, default
  `(locate-user-emacs-file "emrs/modules/")`.
- A module of a version lives at
  `<directory>/<module>/<version>/<module>-module<module-file-suffix>`
  (ADR 0006).
- A locally built module has a file `built-from` next to it that holds the
  absolute path of the checkout. A downloaded one has none.
- Every installation writes a temporary file in the target directory, sets
  its mode to 0755 and renames it over the target, so a loaded module keeps
  a valid file.

### Platform

`(emrs-install-platform)` returns the platform name of ADR 0004 / ADR 0007
for this Emacs, or nil:

| `system-type` | `system-configuration` | Platform |
|---|---|---|
| `gnu/linux` | starts with `x86_64-`, ends with `-linux-gnu` | `linux-x86_64` |
| `gnu/linux` | starts with `aarch64-`, ends with `-linux-gnu` | `linux-aarch64` |
| `darwin` | starts with `aarch64-` or `arm-` | `macos-aarch64` |
| `windows-nt` | starts with `x86_64-` | `windows-x86_64` |
| anything else (musl, Intel Mac, BSD, …) | | nil |

### Lookup

`(emrs-install-file MODULE)` returns the file to load, without side effects:

1. The installed file of the registered version, if it exists.
2. Otherwise `<module>-module` plus `module-file-suffix` found with
   `locate-library` (a manual installation).
3. Otherwise nil.

### Getting the module

`(emrs-install-ensure MODULE)` returns a file to load or signals a
`user-error`. Steps, in order:

1. **Present.** If `emrs-install-file` finds a file, it is used. If it is a
   local build (`built-from` exists), the checkout still exists, the session
   is interactive and no module of that name is loaded, the installer
   compares modification times: if a source file is newer than the module,
   it asks "The <module> module is older than its sources. Rebuild it now
   with cargo?". Yes starts the build (step 3); no uses the old file. The
   sources are the `*.rs` files below `src/` and `build.rs`, `Cargo.toml`
   and `Cargo.lock` of the checkout.
2. **Download**, if `:url` is set, the version has no `-snapshot` suffix and
   the platform is known.
   - Consent comes from `emrs-install-download`: `ask` (default) asks
     "Download the <module> module <version> for <platform> from <url>?",
     `always` downloads, `never` skips. In batch mode `ask` counts as
     `never`.
   - The installer fetches `<url>/v<version>/SHA256SUMS` and then
     `<url>/v<version>/<module>-module-<platform><module-file-suffix>` with
     `url-retrieve-synchronously`, each within `emrs-install-timeout`
     seconds (default 60), showing a message while it runs. A response other
     than 200 for `http` and `https`, or a missing file for `file`, is a
     failure.
   - `SHA256SUMS` lines have the form `<64 hex digits>`, two spaces or a
     space and `*`, file name. The file's `secure-hash` `sha256` must equal
     its line.
   - On success the file is installed (see Files) and returned.
3. **Build**, if a checkout is found. Interactive sessions ask "Build the
   <module> module <version> with cargo?" unless the user just said yes to a
   rebuild. Yes starts `emrs-install-build` and signals a `user-error`
   "Building the <module> module; run the command again when it is done".
   No, or batch mode, goes on to step 4.
4. **Nothing worked.** A `user-error` that names the module, the version,
   the platform (or "no prebuilt module for <system-configuration>"), what
   was tried and why it failed, and the ways out: `M-x emrs-install-module`,
   the README of the package, or installing `<module>-module<suffix>` on
   `load-path`.

A failed download (network error, no release for the version) shows a
message and goes on to step 3. A checksum mismatch does not: it signals a
`user-error` naming the file, the expected and the actual checksum, installs
nothing and does not offer a build, because the release does not match its
own checksums.

### Loading

`(emrs-install-load MODULE)` requires `emrs`, calls `emrs-install-ensure`
unless the module is already loaded, loads the file with
`emrs-load-module`, and returns the module handle. When the file is in the
install directory, its version directory is added to the front of
`load-path` first (ADR 0006), so a later `(require '<module>-module)` finds
the same file.

- If the module's `:version` differs from the registered one, it shows a
  warning (`display-warning`, type `emrs-install`) once per module: both
  versions, the file, and that Emacs must be restarted after
  `M-x emrs-install-module`, since a module cannot be unloaded.
- If loading signals an error, the installer signals a `user-error` with
  the file, the error, the platform it detected, `system-configuration`,
  `emacs-version`, what the release files need (Linux: glibc 2.28 or later;
  macOS: Apple silicon) and `M-x emrs-install-module` as the way to install
  again.

A package calls `emrs-install-load` where it calls `emrs-load-module` today.

### Local build

`emrs-install-build` (command, asynchronous) and
`emrs-install-build-sync` (function) build a registered module from its
checkout ([ADR 0009](../decisions/0009-installer-builds-with-plain-cargo.md)).

- Command: `emrs-install-cargo-program` (default `"cargo"`) with
  `build --release --target-dir <checkout>/target`, plus `-p <package>` if
  registered, run in the checkout. A missing program is a `user-error` that
  names the option.
- The library is the cdylib target of the package in
  `cargo metadata --no-deps --format-version 1` (the registered package, or
  else the package whose manifest is the checkout's `Cargo.toml`):
  `<checkout>/target/release/<prefix><name><module-file-suffix>`, with `-` in
  `<name>` replaced by `_` and `<prefix>` `lib` except on Windows.
- The asynchronous build runs in the compilation buffer
  `*emrs-install <module>*`. When it finishes successfully, the library is
  installed under the registered version with `built-from`, and a message
  says so, adding "restart Emacs to use it" if the module is loaded. A
  second build of the same module while one runs is a `user-error`.
- `emrs-install-build-sync` runs the same command with `call-process`, never
  signals and returns non-nil on success; a failure becomes a warning that
  in batch mode includes the last 30 lines of output. It is meant for
  pre-build steps of package managers.

### Commands and options

- `emrs-install-module` (MODULE, with completion over registered modules):
  installs the registered version again, by download if possible,
  otherwise by build, ignoring what is installed. With a prefix argument it
  builds. It reports where the module went and whether a restart is needed.
- `emrs-install-clean`: for each registered module, lists the version
  directories other than the registered version and the version of a
  loaded module, asks once with the list, and deletes them.
- Options (group `emrs-install`): `emrs-install-directory`,
  `emrs-install-download` (`ask`, `always`, `never`),
  `emrs-install-timeout`, `emrs-install-cargo-program`.

### README

A section "Installing modules for users" for package authors: the
registration form, `emrs-install-load`, the recipe change to `elisp/*.el`,
a pre-build form for straight and Doom
(`(emrs-install-build-sync 'MODULE)` after loading the package's main
file), and the user options. The release section changes `1.3.0-dev` to
`1.3.0-snapshot` and links ADR 0008.

## Error handling

| Situation | Behaviour |
|---|---|
| `:version` missing or invalid at registration | `error` at load time of the package, naming the module and the value |
| Module not registered | `user-error` naming it and `emrs-install-register` |
| Unknown platform | No download; build or final `user-error` with `system-configuration` |
| User declines the download | Go on to the build step |
| Network error, timeout, HTTP status other than 200 | Message with URL and status; go on to the build step |
| Checksum line missing for the file | Like a failed download |
| Checksum mismatch | `user-error` with file and both checksums; nothing installed; no build offered |
| Install directory not writable, rename fails | `user-error` with path and system error; temporary file removed. On Windows the message adds that a loaded module cannot be replaced before a restart |
| Cargo not found | `user-error` naming `emrs-install-cargo-program` |
| Build fails | Asynchronous: message pointing to the compilation buffer. Synchronous: warning, with the output tail in batch mode |
| `cargo metadata` finds no cdylib, or the library is missing after the build | `user-error` (asynchronous: message) naming the package and `crate-type = ["cdylib"]` |
| Loaded module reports another version | Warning, once per module |
| Loading fails | `user-error` with the details listed under Loading |

## Testing

ERT tests in `tests/elisp/emrs-install-tests.el`, run by `tests/run-ert.sh`
like the existing suite. They use a temporary `emrs-install-directory`, a
`file://` release directory, and never the network.

- Platform: every row of the table, with `system-type` and
  `system-configuration` bound.
- `SHA256SUMS` parsing: two spaces, ` *`, missing line, malformed line.
- Registration: missing and invalid `:version`, replacement.
- Lookup order: installed version before `load-path`; nil when neither.
- Download: a release directory holding the emrs-test module (the file the
  suite already builds) as `emrs-test-module-<platform><suffix>` and its
  `SHA256SUMS`; with `emrs-install-download` `always` it is installed,
  without `built-from`. With `never`, with a `-snapshot` version, with a
  missing file and with a wrong checksum (no installation, `user-error`), it
  is not.
- Local build: `emrs-install-build-sync` on the emrs-test module crate of
  this repository (`:source` a file in `tests/emrs-test-module/`,
  `:package "emrs-test-module"`) installs the library with `built-from`.
- Stale check: a temporary fake checkout with a module file older and newer
  than its sources.
- Loading, in a separate `emacs --batch --module-assertions` process (the
  suite's Emacs already has the module loaded): `emrs-install-load`
  installs from the `file://` release and returns a handle; a registered
  version that differs from the module's gives the warning.
- `emrs-install-clean`: deletes the other version directories and keeps the
  registered and the loaded one (prompt answered by binding
  `yes-or-no-p`/`y-or-n-p`).

Manual check: in a fresh `emacs -Q` without `cargo` on `PATH`, register
`emrs-dirdiff` with `:version "0.1.0"` and its release URL, run
`(emrs-install-load 'emrs-dirdiff)` and answer the prompt; the module is
downloaded over HTTPS, loads and reports version 0.1.0.

Not tested: Windows and macOS by hand (the ERT suite runs there in CI),
proxies, the asynchronous build's compilation buffer beyond its finish
hook.

## Tasks

- [x] 1. Write `elisp/emrs-install.el` with registration, platform, lookup,
  download with checksum and consent, `emrs-install-ensure` (without the
  build step, which ends in the final `user-error` for now) and
  `emrs-install-load` with the version warning and load errors, plus the
  tests for these parts. *Done when* `tests/run-ert.sh` passes with the new
  tests, the byte compiler reports no warnings for the file, and
  `checkdoc` reports nothing.
- [x] 2. Add the local build (`emrs-install-build`,
  `emrs-install-build-sync`, the stale check, step 3 of
  `emrs-install-ensure`) and its tests. *Done when* `tests/run-ert.sh`
  passes, and in an interactive Emacs `M-x emrs-install-build` on the
  registered emrs-test module shows the compilation buffer and installs the
  module.
- [x] 3. Add `emrs-install-module` and `emrs-install-clean` and their
  tests. *Done when* `tests/run-ert.sh` passes.
- [x] 4. Manual check against emrs-dirdiff v0.1.0 as described under
  Testing. *Done when* the module loads and reports 0.1.0 in an Emacs
  without cargo.
- [ ] 5. Documentation: the README section, the `-snapshot` change in the
  release section, an `emrs-install.el` row in `docs/architecture.md`, the
  backlog item "Installer for prebuilt modules" removed, and
  `docs/next-steps.md` naming the adoption by quarry and emrs-dirdiff.
  *Done when* the doc-checker reports nothing stale.
