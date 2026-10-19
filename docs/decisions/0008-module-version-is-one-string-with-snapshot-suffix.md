# 0008 — The module version is one string, with `-snapshot` between releases

Date: 2026-10-19
Status: accepted; replaces the `-dev` example of ADR 0004

## Context

ADR 0004 ties a release to the crate version and told authors to bump to a
development version such as `1.3.0-dev` after a release. The installer
`emrs-install.el` gets the version from the package's Elisp, uses it for the
release tag and compares it with the `:version` the loaded module reports.

Emacs rejects `0.2.0-dev`: `version-to-list` signals "Invalid version
syntax". It accepts `0.2.0-snapshot` (and `-alpha`, `-beta`, `-pre`, `-rc`),
which is also a valid semver prerelease. emrs-dirdiff already had
`0.2.0-dev` in `Cargo.toml` and `0.2.0-snapshot` in its Elisp header.

Options: the package passes the version explicitly and one convention holds
for Cargo and Elisp; the installer reads the package's `;; Version:` header;
or any string is allowed as long as it equals the crate version.

## Decision

- A package passes its module version to the installer as a string. It must
  equal the crate version in `Cargo.toml` and be valid for `version-to-list`.
- Between releases the version carries the suffix `-snapshot`, for example
  `0.2.0-snapshot`, in `Cargo.toml`, in the string passed to the installer
  and, by recommendation, in the package's `;; Version:` header.
- The installer does not look for a release of a `-snapshot` version.
- After loading, the installer compares the module's `:version` with the
  string and warns on a difference.

The user chose this on 2026-10-19.

## Consequences

- An author keeps one version string per release, in two or three places;
  the warning after loading catches a forgotten one.
- A `-snapshot` version is never downloaded, so users on the default branch
  get a local build without a failed download first.
- emrs-dirdiff has to change its crate version from `0.2.0-dev` to
  `0.2.0-snapshot`.
- The README's release section changes its example from `-dev` to
  `-snapshot`.
