# Release pipeline for prebuilt modules

Status: approved
Date: 2026-10-17

## Purpose

Packages built on emrs are to be installable without a Rust toolchain. This
spec covers the producing half: a module author tags a release and gets one
tested binary per platform plus a checksum file, attached to the GitHub
release, in a layout an installer can rely on. The consuming half, the Elisp
installer `emrs-install.el`, is a separate spec that follows this one; the
decisions it depends on are in ADR 0004 and ADR 0006.

Success:

- `cargo emacs package` turns a module crate into
  `<name>-module-<platform>.<ext>` for the host or a given target.
- A module repository adds a short workflow that calls emrs's reusable
  workflow. A pushed tag `v<version>` then produces the release assets for
  five platforms, each tested in Emacs with `--module-assertions`, and
  `SHA256SUMS`.
- emrs's own CI runs that workflow on the example `echo` without publishing.

## Scope

In:

- The release asset contract (ADR 0004): tag, file names, platform names,
  `SHA256SUMS`.
- The subcommand `cargo emacs package`.
- The reusable workflow `.github/workflows/module-release.yml`.
- A workflow in emrs that runs it on `examples/echo` as a dry run.
- README and architecture updates.

Out:

- The installer `emrs-install.el` (next spec).
- Moving quarry to the pipeline. That happens in quarry's repository after
  both specs.
- Signing assets, notarizing macOS binaries, hosts other than GitHub in the
  workflow (the contract itself is host-neutral).
- The subcommands `test` and `release`, and publishing `cargo-emacs` to
  crates.io; they stay in the backlog.
- Windows targets other than `x86_64-pc-windows-msvc` and Linux targets other
  than glibc in the workflow.

## Design

### Release asset contract

The reasons are in [ADR 0004](../decisions/0004-release-assets-per-exact-version.md).

- **Version.** `<version>` is the `version` of the module crate in
  `Cargo.toml`, the value the module reports as `:version` in
  `<name>--emrs-info`. The release tag is `v<version>`.
- **Location.** The assets of a release are at
  `<base>/v<version>/<file>`. For GitHub, `<base>` is
  `https://github.com/<owner>/<repo>/releases/download`.
- **Asset.** `<name>-module-<platform>.<ext>`: the raw library, where `<name>`
  is the module name (as for `cargo emacs build`, ADR 0002) and `<ext>` is the
  extension Cargo produced (`so`, `dylib`, `dll`).
- **Platforms.** The name depends on the target triple. For zigbuild the
  glibc suffix is ignored, so `x86_64-unknown-linux-gnu.2.28` maps like
  `x86_64-unknown-linux-gnu`.

  | Platform | Target triple(s) | Built by the workflow on | Tested by the workflow on |
  |---|---|---|---|
  | `linux-x86_64` | `x86_64-unknown-linux-gnu` | `ubuntu-latest`, zigbuild, glibc 2.28 | `ubuntu-latest` |
  | `linux-aarch64` | `aarch64-unknown-linux-gnu` | `ubuntu-latest`, zigbuild, glibc 2.28 | `ubuntu-24.04-arm` |
  | `macos-x86_64` | `x86_64-apple-darwin` | `macos-latest`, cross | `macos-15-intel` |
  | `macos-aarch64` | `aarch64-apple-darwin` | `macos-latest` | `macos-latest` |
  | `windows-x86_64` | `x86_64-pc-windows-msvc`, `x86_64-pc-windows-gnu` | `windows-latest` (MSVC) | `windows-latest` |

- **Checksums.** `SHA256SUMS` in the release, in the output format of
  `sha256sum` (`<64 lowercase hex>  <file>`, two spaces), one line per asset,
  sorted by file name.

The installer spec maps Emacs's `system-configuration` to the same platform
names.

### `cargo emacs package`

```text
cargo emacs package [--dir <path>] [--platform <name>] [--zigbuild] [<cargo build options>...]
```

It shares the argument handling, `cargo metadata` step, artifact selection,
name resolution and copy-by-rename of `build`
([spec of 2026-10-11](2026-10-11-cargo-emacs-build.md)), with these
differences:

1. **Own options.** `--dir` as for `build`. `--platform <name>` or
   `--platform=<name>` sets the platform name. `--zigbuild` runs
   `cargo zigbuild` instead of `cargo build`. All three are removed before
   the options are passed to Cargo.
2. **Profile.** If the passed options contain neither `--release` nor
   `--profile` (in any of its forms), the tool adds `--release`.
3. **Platform.** Determined before the build. Without `--platform`, the
   triple is the value of `--target` (either form) if given, otherwise the
   `host:` line of `rustc -vV` (`RUSTC` if set, otherwise `rustc`). A
   trailing suffix of `.` followed by digits and dots is removed, then the
   triple is looked up in the table above. `--platform` must be non-empty
   and contain only `a`–`z`, `0`–`9`, `-` and `_`.
4. **Destination.** `<dir>/<name>-module-<platform>.<ext>`. `<dir>` defaults
   to `<target-directory>/emacs/dist`.
5. **Output.** The absolute path of each written file on standard output,
   one per line, as with `build`.

The tool does not write `SHA256SUMS`; the workflow does, and an author who
releases by hand runs `sha256sum` (README).

`cargo emacs --help` lists `package` next to `build`, and
`cargo emacs package --help` describes its options.

### Reusable workflow

`.github/workflows/module-release.yml`, triggered by `workflow_call`. A
module repository uses it like this (README):

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
    uses: tohammer/emrs/.github/workflows/module-release.yml@master
    with:
      package: quarry
      publish: ${{ startsWith(github.ref, 'refs/tags/v') }}
```

Inputs:

| Input | Type, default | Meaning |
|---|---|---|
| `package` | string, `""` | Cargo package of the module, passed as `-p`. Empty: the package in `working-directory` |
| `working-directory` | string, `"."` | Directory of the caller's repository where Cargo runs |
| `platforms` | string (JSON array), all five | Platforms to build, test and publish |
| `emacs-versions` | string (JSON array), `["30.1", "31.1"]` | Emacs versions for the tests on Linux and macOS. Windows is tested with 30.1 only |
| `test-command` | string, `""` | Bash command that tests one asset, run after the smoke test; empty runs the smoke test only |
| `publish` | boolean, `false` | Upload to the GitHub release of the tag |
| `cargo-emacs` | string, `"master"` | Where `cargo-emacs` comes from: a git ref of the emrs repository, or `path:<dir>` for a directory in the caller's checkout |
| `rust-toolchain` | string, `"stable"` | Rust toolchain for the builds and the test jobs |

Jobs:

1. **`prepare`** (Ubuntu). Checks out the caller's repository and reads the
   module crate's version with `cargo metadata`. With `publish` true, the
   workflow ref must be the tag `v<version>`; otherwise the job fails with a
   message that names both. Every entry of `platforms` must be in the
   table. It outputs the version.
2. **`build`** (one job per platform in `platforms`, on the runner from the
   table). Installs the toolchain and its target, `cargo-emacs` as `cargo-emacs`
   says, and for Linux pinned versions of zig and `cargo-zigbuild`. Runs
   `cargo emacs package --target <triple> [-p <package>] --dir dist`, with
   `--zigbuild` and the triple `<triple>.2.28` on Linux. The run must produce
   exactly one file. On Linux, the highest `GLIBC_` symbol version that
   `readelf --dyn-syms -W` reports for the file must be at most 2.28
   ([ADR 0005](../decisions/0005-linux-builds-with-zigbuild-against-glibc-2-28.md)).
   The file is uploaded as a workflow artifact.
3. **`test`** (one job per platform and Emacs version, on the test runner
   from the table). Checks out the caller's repository, downloads the
   platform's artifact, sets up Emacs (`purcell/setup-emacs`, and
   `jcs090218/setup-emacs-windows` on Windows), installs the Rust toolchain
   `rust-toolchain` (no extra target), and copies the asset to a new
   directory as `<name>-module.<ext>`, with `<name>` taken from the asset's
   file name. It exports `EMRS_MODULE_FILE` (that copy), `EMRS_MODULE_DIR`
   (its directory), `EMRS_MODULE_NAME`, `EMRS_MODULE_VERSION` and
   `EMRS_EL_DIR`.

   `EMRS_EL_DIR` is the `elisp/` directory of the `emrs` package the module
   depends on, found with `cargo metadata` in `working-directory` (the
   directory of emrs's `Cargo.toml`, its parent, then `elisp/`), so a test
   loads the `emrs.el` that matches the module. It is empty if that
   directory has no `emrs.el`. The toolchain is there so that a
   `test-command` can run `cargo` (for example `cargo metadata` or Rust
   tests); it must not rebuild the module, since the asset is what is under
   test.

   Then the job runs the smoke test: `emacs --batch --module-assertions`
   loads the file with `module-load` and fails unless `<name>--emrs-info`
   returns `:module` equal to `<name>` and `:version` equal to the version.
   If `test-command` is not empty, the job runs it next, with Bash in
   `working-directory`.
4. **`publish`** (Ubuntu, only with `publish` true, after every `test` job
   passed). Downloads all assets, writes `SHA256SUMS` over them with
   `sha256sum`, creates the release for the tag if it does not exist (title
   `v<version>`, no notes) and uploads the assets and `SHA256SUMS` with
   `gh release upload --clobber`. A release the author created beforehand,
   with notes or as a draft, is kept and gets the files.

Every third-party action is pinned to a major version or a commit, as in
`ci.yml`.

### Dry run in emrs

`.github/workflows/release-check.yml` calls the reusable workflow with
`package: echo-rs`, `cargo-emacs: path:cargo-emacs`, `publish: false` and a
`test-command` that loads `emrs.el` from `EMRS_EL_DIR` and the module from
`EMRS_MODULE_DIR`, starts the echo service and checks one `emrs-call-sync`
round trip.
It runs on pull requests and on pushes to `master` that change
`cargo-emacs/`, `emrs/`, `examples/echo/` or either workflow file, and on
`workflow_dispatch`. It is a separate workflow, so the regular CI keeps its
duration.

## Error handling

`cargo emacs package` follows the rules and exit statuses of `build`, plus:

| Situation | Behaviour |
|---|---|
| The triple is not in the table and `--platform` is not given | Message naming the triple and `--platform`; exit status 1; nothing is built |
| `--platform` without a value, or with characters outside the allowed set | Usage message, exit status 2 |
| `rustc -vV` fails or has no `host:` line | Message with the error; exit status 1 |
| `--zigbuild` and `cargo zigbuild` is not installed | Cargo's error is shown; exit with its status |

In the workflow, every failure fails the run, and with `publish` true nothing
is uploaded unless all builds and tests passed:

| Situation | Behaviour |
|---|---|
| Tag and crate version differ | `prepare` fails with both values |
| A platform in `platforms` is not in the table | `prepare` fails and names it |
| A build produces no file or more than one | `build` fails; the message points to `package` |
| A Linux library needs glibc newer than 2.28 | `build` fails and names the symbol version found |
| `cargo metadata` fails in a test job | `test` fails with Cargo's error |
| Smoke test or `test-command` fails | `test` fails; `publish` does not run |
| Uploading fails part way | `publish` fails; rerunning the job replaces the files (`--clobber`) |

## Testing

- Unit tests in `cargo-emacs`: platform from triple (all table entries,
  zigbuild suffixes such as `.2.28` and `.2.17`, unknown triple), validation
  of `--platform`, splitting of `--platform`, `--zigbuild` and `--dir` from
  the passed options, adding `--release` (absent, `--release` present,
  `--profile x`, `--profile=x`), reading `--target` in both forms, parsing
  `rustc -vV` output.
- Integration tests in `cargo-emacs/tests/` with the existing fixture crate:
  `package` writes `<dir>/<name>-module-<host platform>.<ext>` and prints its
  path; the build is a release build (the file comes from
  `target/release`); `--platform` overrides the name; an unknown `--target`
  without `--platform` fails with exit status 1 before Cargo runs.
- The dry run in emrs builds and tests `echo` on all five platforms, with
  the smoke test and the round trip through `EMRS_EL_DIR`. Its passing run
  on the pull request is the test of the workflow.
- Manual: the `publish` path runs only on a tag. It is checked once by
  calling the workflow from a scratch repository owned by the user, tagged
  with that module's version. Whoever checks it confirms that the release
  has five assets and a `SHA256SUMS` that `sha256sum -c` accepts after
  downloading.
- Not tested: zigbuild locally (it runs only in the workflow), Emacs 31 on
  Windows, other Windows Emacs builds.

## Tasks

- [ ] 1. Add `package` to `cargo-emacs` as designed, with `--help` text and
  the unit and integration tests above. *Done when*
  `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass, and
  `cargo run -q -p cargo-emacs -- package -p echo-rs` prints the path of an
  existing `target/emacs/dist/echo-rs-module-<host platform>.<ext>`.
- [ ] 2. Write `.github/workflows/module-release.yml` and
  `.github/workflows/release-check.yml` as designed. *Done when* the release
  check passes on the pull request for all five platforms, including the
  glibc check on both Linux platforms, the smoke test and the echo round
  trip through `EMRS_EL_DIR` with both Emacs versions, and a deliberate failure is seen to fail the run once (for
  example a temporary smoke test with a wrong version, removed afterwards).
  If the runner `macos-15-intel` does not exist, stop and report instead of
  dropping the test.
- [ ] 3. Check the `publish` path manually as described under Testing, with
  the user's permission and a repository they name. *Done when* the release
  holds five assets and `SHA256SUMS`, and `sha256sum -c SHA256SUMS` passes on
  the downloaded files.
- [ ] 4. Documentation: README section "Releasing prebuilt modules" (the
  contract, `cargo emacs package`, the caller workflow, the version bump after
  a release from ADR 0004, `sha256sum` for manual releases), `cargo-emacs`
  and workflow rows in `docs/architecture.md`, the backlog items
  "Prebuilt artifacts and distribution" and "`cargo emacs`: further
  subcommands" reduced to what remains, and `docs/next-steps.md` naming the
  installer spec as next. *Done when* the README commands match the tool's
  `--help` and the workflow's inputs, and the doc-checker reports nothing
  stale.
