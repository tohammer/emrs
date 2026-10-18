# 0007 — No release builds for Intel Macs

Date: 2026-10-18
Status: accepted; amends the platform list of ADR 0004

## Context

ADR 0004 listed five release platforms, among them `macos-x86_64`. In the
first run of the release workflow, the build for it succeeded (cross-built
on Apple silicon), but the tests on the runner `macos-15-intel` failed:
`purcell/setup-emacs` installs Emacs through Nix, and nixpkgs 26.11 dropped
`x86_64-darwin`.

The platform is being wound down: macOS 26 is the last release for Intel
Macs, Rust moved `x86_64-apple-darwin` to tier 2, and `macos-15-intel` is
GitHub's last Intel image, announced to be retired in 2027. Testing would
need a separate way to install Emacs (for example the universal builds of
emacsformacosx.com) that stops working with the runner anyway.

## Decision

`macos-x86_64` is not a release platform. The reusable workflow does not
build it, and `cargo emacs package` has no platform name for
`x86_64-apple-darwin` (an author can still pass `--platform`). The user
chose this on 2026-10-18.

## Consequences

- Releases have four assets: `linux-x86_64`, `linux-aarch64`,
  `macos-aarch64`, `windows-x86_64`.
- Users on Intel Macs get no prebuilt module; the installer falls back to a
  local `cargo build` (ADR 0006), which needs Rust.
- emrs itself still checks in CI that it builds for `x86_64-apple-darwin`
  (job "Build for macOS x86_64"); the README's platform table is unchanged.
