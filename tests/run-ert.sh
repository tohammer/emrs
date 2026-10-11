#!/bin/sh
# Build the test module, install it under the name Emacs expects, and run
# the ERT suite with module assertions enabled.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo run -q -p cargo-emacs -- build -p emrs-test-module
exec "${EMACS:-emacs}" --batch --module-assertions \
    -L "$root/elisp" -L "$root/target/emacs" \
    -l "$root/tests/elisp/emrs-tests.el" \
    -f ert-run-tests-batch-and-exit
