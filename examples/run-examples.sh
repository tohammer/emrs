#!/bin/sh
# Build the example modules, install them under the names Emacs expects,
# and run their smoke tests.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo run -q -p cargo-emacs -- build -p echo-rs -p grep-rs
exec "${EMACS:-emacs}" --batch --module-assertions \
    -L "$root/elisp" -L "$root/target/emacs" \
    -l "$root/examples/examples-test.el" \
    -f ert-run-tests-batch-and-exit
