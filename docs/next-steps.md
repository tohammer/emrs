# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

Adoption of `emrs-install.el` by emrs-dirdiff and quarry, in their own
repositories (their backlogs list it): register the module, replace their
build files and loading code with `emrs-install-load`, change their recipes
to `elisp/*.el`, and move emrs-dirdiff's crate version from `0.2.0-dev` to
`0.2.0-snapshot` (ADR 0008).

## Watch out for

- The CI job `Windows (best effort)` is allowed to fail, so a red Windows job
  does not block a merge. Check it anyway: it passes since the fix of ADR 0001
  (PR #2).
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- Module repositories call `module-release.yml@master`, so a change to its
  inputs or to the asset layout of ADR 0004 breaks them. Change both only
  in a compatible way, or with a new ADR and a note in
  `docs/user/distributing.md`.
- An emrs release moves the `v0.2.0` pins to the new tag (README quick
  start: dependency, `cargo install`, `git clone`;
  `docs/user/distributing.md`: release workflow), then `master` to the
  next `-snapshot` version.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
