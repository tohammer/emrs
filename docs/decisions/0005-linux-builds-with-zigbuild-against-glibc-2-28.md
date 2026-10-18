# 0005 — Linux release builds use cargo-zigbuild against glibc 2.28

Date: 2026-10-17
Status: accepted

## Context

A Linux module links against the glibc of the build machine and refuses to
load where the glibc is older. GitHub's current Ubuntu runners have glibc
2.39 (24.04) or 2.35 (22.04); Debian 11, RHEL 8 and their derivatives ship
older versions.

Options:

1. `cargo-zigbuild` with a target such as `x86_64-unknown-linux-gnu.2.28`:
   zig links against a chosen glibc version, and both architectures build on
   one runner.
2. A `manylinux_2_28` container per architecture, with Rust installed inside.
3. The runner's own glibc.

## Decision

Release builds for Linux use `cargo-zigbuild` with glibc 2.28 for x86_64
and aarch64. The release workflow checks with `readelf` that no symbol needs
a newer glibc version. The user chose this on 2026-10-17.

## Consequences

- Modules load on distributions with glibc 2.28 or later (Debian 10,
  RHEL 8, Ubuntu 18.10 and later).
- The workflow needs zig and `cargo-zigbuild`. Crates whose build scripts
  compile C code are built with zig's C compiler; if one fails there, the
  author has to fix it or build Linux assets another way.
- `cargo emacs package` must run `cargo zigbuild` instead of `cargo build`
  on request, and must strip the glibc suffix from the target triple to find
  the platform name.
