# Shipping prebuilt modules

Users of a package should not need a Rust toolchain. You publish the module
for each platform on the GitHub release of the tag `v<version>`, and the
package loads it through `emrs-install.el`, which falls back to a local
build where no file fits.

- [Version the module](#version-the-module)
- [Publish a release with the workflow](#publish-a-release-with-the-workflow)
- [Load the module in the package](#load-the-module-in-the-package)
- [Release layout](#release-layout)
- [`cargo emacs package`](#cargo-emacs-package)
- [Workflow inputs](#workflow-inputs)
- [Installer reference](#installer-reference)

## Version the module

One version string names a release: the `version` in the module's
`Cargo.toml`. The package passes the same string to `emrs-install-register`,
and its `;; Version:` header should match.

1. Before a release, set the version to the release number, for example
   `1.3.0`, in `Cargo.toml` and in the package's Elisp.
2. Tag the commit `v1.3.0` and push the tag.
3. After the release, set both to the next version with the suffix
   `-snapshot`, for example `1.4.0-snapshot`.

The installer downloads only the release that matches the version exactly
and never downloads a `-snapshot` version, so users who follow the default
branch build locally instead of getting the previous release. Do not use
`-dev`: Emacs rejects it as a version.

## Publish a release with the workflow

1. Add `.github/workflows/release.yml` to the module's repository:

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

   Pin the workflow and `cargo-emacs` to the same emrs release, preferably
   the one the module's `emrs` dependency uses. With `@master` the build
   changes with emrs's development version.

2. Push the tag `v<version>`.

The workflow builds every platform, loads each file in Emacs with
`--module-assertions`, checks that the module reports its name and version,
and uploads the files and `SHA256SUMS` to the release. A missing release is
created as a draft, filled and then published, as a prerelease if the
version contains `-`. A release you created beforehand, draft or not, is
kept as it is. A run started by hand builds and tests without publishing.

Only the publish job gets the write token; the build and test jobs run with
read access.

Optional: to run your own tests on each built file, set `test-command`. It
runs in `working-directory` with the Rust toolchain installed and these
variables set; it must not rebuild the module:

| Variable | Value |
|---|---|
| `EMRS_MODULE_FILE` | The built file, renamed to `<name>-module.<ext>` |
| `EMRS_MODULE_DIR` | Its directory, for `load-path` |
| `EMRS_MODULE_NAME` | The module name |
| `EMRS_MODULE_VERSION` | The module version |
| `EMRS_EL_DIR` | The directory of the `emrs.el` of the emrs version the module depends on |

A `rust-toolchain.toml` in the module's repository overrides the input
`rust-toolchain`; it must then list the targets of the platforms built.

## Load the module in the package

1. Register the module once, at top level of the package's main file:

   ```elisp
   (require 'emrs-install)

   (emrs-install-register 'echo-rs
     :version "0.2.0"
     :url "https://github.com/OWNER/REPO/releases/download"
     :source (or load-file-name buffer-file-name))
   ```

2. Where the package needs the module, load it:

   ```elisp
   (emrs-install-load 'echo-rs)   ; returns the module handle
   ```

3. In the package recipe, include both Elisp files of emrs, for example
   `:files ("elisp/*.el")` instead of `("elisp/emrs.el")`.

`emrs-install-load` tries these in order and uses the first that works:

1. The installed module of this version in `emrs-install-directory`, or
   `<module>-module` on `load-path`. In an interactive session, a local
   build that is older than its sources is offered for a rebuild first.
2. The release file for this platform, checked against `SHA256SUMS`.
   `emrs-install-download` decides whether the user is asked. Skipped for
   `-snapshot` versions.
3. A build with cargo in the package's checkout. In an interactive session
   the user is asked first; the build runs in the buffer
   `*emrs-install <module>*`, and the command that needed the module ends
   with a message to run it again once the build is done. Not offered in
   batch mode.

If none works, the error says what was tried. A loaded module that reports
another version than the registered one gives a warning.

## Release layout

A release holds one file per platform and a checksum file:

```text
<name>-module-linux-x86_64.so
<name>-module-linux-aarch64.so
<name>-module-macos-aarch64.dylib
<name>-module-windows-x86_64.dll
SHA256SUMS
```

- `SHA256SUMS` has the format of `sha256sum`.
- Linux files are built against glibc 2.28 and load on Debian 10, RHEL 8,
  Ubuntu 18.10 and later.
- There is no file for macOS x86_64; Intel Macs build the module locally.
- A file can also be downloaded by hand and renamed to
  `<name>-module.<ext>`.

## `cargo emacs package`

Builds one file of the release layout. The workflow uses it; you need it
only for a release by hand.

```sh
cargo emacs package                 # target/emacs/dist/echo-rs-module-linux-x86_64.so
cargo emacs package --target aarch64-apple-darwin
```

| Option | Description |
|---|---|
| `--dir <path>` | Output directory; default `emacs/dist/` in the target directory |
| `--platform <name>` | Platform part of the file name; default from `--target`, or the host |
| `--zigbuild` | Run `cargo zigbuild` instead of `cargo build`, for example with `--target x86_64-unknown-linux-gnu.2.28` |
| all others | Passed to `cargo build`; `--release` is added unless `--release`, `-r` or `--profile` is given |

It does not write `SHA256SUMS`. For a release by hand, run
`sha256sum -- *-module-* > SHA256SUMS` in the output directory.

## Workflow inputs

Inputs of `module-release.yml`:

| Input | Default | Description |
|---|---|---|
| `package` | `""` | Cargo package of the module; empty for the package in `working-directory` |
| `working-directory` | `"."` | Where Cargo runs |
| `platforms` | all four | JSON array, for example `'["linux-x86_64", "macos-aarch64"]'` |
| `emacs-versions` | `'["30.1", "31.1"]'` | Emacs versions for the tests on Linux and macOS; Windows uses 30.1 |
| `test-command` | `""` | Bash command run after the built-in check of each file |
| `publish` | `false` | Upload to the release of the tag; the tag must be `v<version>` |
| `cargo-emacs` | `master` | Branch, tag or full commit hash of emrs to install `cargo-emacs` from, or `path:<dir>` in the caller's checkout |
| `rust-toolchain` | `stable` | Rust toolchain for builds and tests |

## Installer reference

### `emrs-install-register` keys

| Key | Description |
|---|---|
| `:version` | Required. The crate version; between releases `X.Y.Z-snapshot` |
| `:url` | Base URL of the releases, `https://` or `file:`; the files of a version are at `<url>/v<version>/`. Without it nothing is downloaded |
| `:source` | A file of the package; the nearest directory above it with a `Cargo.toml` is the checkout to build from |
| `:package` | The Cargo package to build, in a workspace with several |

The module name consists of letters, digits, `.`, `_`, `+` and `-`, and
starts with a letter or digit.
Registering a module again replaces its entry.

### Commands

| Command | Action |
|---|---|
| `M-x emrs-install-module` | Download the registered version again, or build it where no download is possible; with a prefix argument, build it |
| `M-x emrs-install-build` | Build a module with cargo |
| `M-x emrs-install-clean` | Delete installed versions that are no longer used |

### User options

Group `emrs-install`.

| Option | Default | Description |
|---|---|---|
| `emrs-install-download` | `ask` | `ask` the user before a download, `always` download, `never` download; in batch mode `ask` means `never` |
| `emrs-install-directory` | `emrs/modules/` in `user-emacs-directory` | Where modules are installed, as `<module>/<version>/` |
| `emrs-install-timeout` | `60` | Seconds without data before a download is stopped |
| `emrs-install-cargo-program` | `"cargo"` | The cargo program for builds |

### Functions

| Function | Description |
|---|---|
| `(emrs-install-load MODULE)` | Get the module if needed, load it and return its handle |
| `(emrs-install-ensure MODULE)` | Get the module if needed and return the file to load, without loading it; signals `user-error` if nothing works |
| `(emrs-install-file MODULE)` | The installed file of the registered version, else `<module>-module` on `load-path`, else `nil`; never downloads or builds |
| `(emrs-install-platform)` | The platform name of this Emacs in the release layout, such as `linux-x86_64`, or `nil` |
| `(emrs-install-build-sync MODULE)` | Build the module with cargo and wait; never signals, a failure becomes a warning. Returns non-nil on success |

`emrs-install-build-sync` is meant for pre-build steps of package managers.
How such a step finds `emrs-install.el` depends on the package manager and
has not been tried.
