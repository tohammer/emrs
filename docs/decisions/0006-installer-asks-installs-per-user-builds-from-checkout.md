# 0006 — The installer asks before downloading, installs per user and builds only from a checkout

Date: 2026-10-17
Status: accepted; the installer spec of 2026-10-19 adds an offer to rebuild a
local build older than its sources, which changes the last consequence

## Context

The planned installer `emrs-install.el` gives a package its module: one that
is already present, else a downloaded release asset (ADR 0004), else a local
`cargo build`. Three questions had to be settled.

Consent: the download is native code that Emacs loads into its own process.

Location: package managers put a build directory on `load-path` whose
contents they control; straight and elpaca recreate it, and package.el
installs from tarballs without a repository. quarry today writes the module
into its repository and resolves symlinks to find it (quarry ADR 0006).

Fallback source: if no release exists for the Elisp's version, the module
can be built from the package's checkout, if there is one, or from the
source archive of the tag.

## Decision

- The installer asks before a download, naming the URL. A user option with
  the values always, ask and never changes that; in batch mode `ask` means
  never.
- Installed modules go to `<user-emacs-directory>/emrs/modules/<module>/<version>/`,
  for downloads and local builds alike. The installer puts the directory of
  the version in use on `load-path`.
- The local build uses only a checkout that the package's own files resolve
  to and that contains `Cargo.toml`. Source archives are not downloaded.

The user chose all three on 2026-10-17.

## Consequences

- One prompt on first use, unless the user allows downloads in their
  configuration.
- Installation works the same with every package manager, and a newer
  version can be installed while an older one is loaded. Old versions
  accumulate; the installer needs a command that removes them.
- A package installed from a tarball, for a version without a release, cannot
  get its module automatically. The error says so and names the manual
  routes.
- A local build of an unreleased version is installed under that version and
  replaced by the next local build; a source update without a rebuild leaves
  the old build in use, as today.
