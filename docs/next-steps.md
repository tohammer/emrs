# Next steps

Forward-looking only. Remove an item when it is done; move its lessons to
architecture.md or an ADR.

## Next

Spec for the installer `emrs-install.el` (backlog "Installer for prebuilt
modules"), the consuming half of the release pipeline. The decisions of ADR
0006 are settled; test it against the assets of a real release made with
`module-release.yml`.

## Watch out for

- The CI job `Windows (best effort)` is allowed to fail, so a red Windows job
  does not block a merge. Check it anyway: it passes since the fix of ADR 0001
  (PR #2).
- `docs/implementation-plan.md` shows the code before review. The repository
  is the current source.
- Module repositories call `module-release.yml@master`, so a change to its
  inputs or to the asset layout of ADR 0004 breaks them. Change both only
  in a compatible way, or with a new ADR and a note in the README.
- A change to the protocol functions or delivery shapes must bump the
  protocol integer and keep the older code path in `emrs.el` (spec section
  6.2).
