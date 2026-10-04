# 0001. Write the wake-up descriptor through Emacs's C runtime

Status: accepted (2026-10-04)

## Context

`open_channel` returns a file descriptor of the C runtime Emacs is linked
against. Descriptor tables belong to one C runtime. On Windows, official GNU
Emacs builds use `msvcrt.dll`, MSYS2 UCRT64 builds use the universal C runtime
(`ucrtbase.dll`), and Rust modules always link UCRT, for both the MSVC and
the GNU target. Writing an `msvcrt.dll` descriptor through UCRT calls
UCRT's invalid-parameter handler, which ends Emacs with `0xC0000409`
(spec `docs/specs/2026-10-03-emrs-core.md` section 13.1).

Options considered:

1. Use `msvcrt.dll` whenever it is loaded. This is wrong on a UCRT Emacs into
   which some other DLL has loaded `msvcrt.dll`.
2. Read the C runtime from the import table of the executable that loaded the
   module.
3. Have `emrs.el` report the runtime to `attach`. This changes the module
   protocol.
4. Require a module target that uses `msvcrt.dll`. Current Rust toolchains
   offer none.

## Decision

Option 2. On Windows, `emrs/src/crt.rs` reads the import table of `emacs.exe`
in memory (`GetModuleHandleA(NULL)`). If it imports `msvcrt.dll`, the
module calls `_write` and `_close` from that DLL, resolved with
`GetProcAddress`. Otherwise, or if detection fails, it calls them through
`libc`, that is UCRT, which is the same `ucrtbase.dll` instance Emacs uses.
On other platforms `libc` is used directly.

## Consequences

- Modules work on official Windows Emacs builds with either Rust target, and
  no protocol change is needed.
- The import-table parser is about 50 lines of bounds-checked code with unit
  tests on synthetic images. The calls through `msvcrt.dll` run only on
  Windows.
- Only the `msvcrt.dll` path is tested in CI (official Emacs 30.1). The UCRT
  path follows from `ucrtbase.dll` being one DLL per process but is
  untested. The Windows CI job stays best effort (backlog "Windows CI").
- An Emacs that imports neither runtime gets the UCRT path.
