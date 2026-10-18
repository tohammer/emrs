# 0004 — Release assets: one per platform, per exact version, checked by SHA256SUMS

Date: 2026-10-17
Status: accepted; the platform list is amended by ADR 0007

## Context

Packages built on emrs are to be installable without a Rust toolchain. An
installer in Emacs downloads a prebuilt module and must find the binary that
matches the package's Elisp. Users of straight or Doom often follow the
default branch, so their Elisp can be newer than the latest release.

Options for matching:

1. Exact tag: the Elisp states a version and the installer fetches the release
   tagged `v<version>`; no such release means no download.
2. Latest compatible release, from compatibility ranges the author maintains.
3. Latest release.

Options for integrity:

1. A `SHA256SUMS` file in the same release, fetched over HTTPS.
2. Checksums committed to the package source after each release build.
3. Signatures (minisign, sigstore), verified in Emacs.

Emacs has no built-in tool to unpack archives on every platform.

## Decision

- A release of a module is the GitHub release (or any host with the same
  layout) tagged `v<version>`, where `<version>` is the module crate's
  version, which the module reports as `:version` in its info. The installer
  downloads only from the exact tag. The user chose this on 2026-10-17.
- Each platform has one asset, the raw library file named
  `<name>-module-<platform>.<ext>`, not an archive. `<platform>` is one of a
  fixed set of names (`linux-x86_64`, `linux-aarch64`, `macos-x86_64`,
  `macos-aarch64`, `windows-x86_64`) and `<ext>` is the platform's module
  suffix without the dot. ADR 0007 removed `macos-x86_64` from this
  set.
- The release contains `SHA256SUMS` in the format of `sha256sum`, one line
  per asset. The installer verifies the download against it. The user chose
  this on 2026-10-17.

## Consequences

- An author who follows the default branch must bump the crate version after
  a release (for example to `1.3.0-dev`), otherwise users of the branch would
  download the binary of the previous release for newer Elisp. With a version
  that has no release, the installer falls back to a local build.
- The checksum catches corrupt and truncated downloads, not a tampered
  release: whoever can replace the asset can replace `SHA256SUMS`. Such an
  attacker can usually change the Elisp as well.
- No unpacking code is needed in Emacs. A user can also download an asset by
  hand and rename it to `<name>-module.<ext>`.
- Adding a platform means adding a name to the fixed set in both the build
  tool and the installer.
