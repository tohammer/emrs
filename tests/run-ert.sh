#!/bin/sh
# Build the test module, install it under the name Emacs expects, and run
# the ERT suite with module assertions enabled.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build -p emrs-test-module
out="$root/target/elisp"
mkdir -p "$out"
for ext in so dylib dll; do
    for built in "target/debug/libemrs_test_module.$ext" "target/debug/emrs_test_module.$ext"; do
        if [ -f "$built" ]; then
            cp "$built" "$out/emrs-test-module.$ext"
        fi
    done
done
exec "${EMACS:-emacs}" --batch --module-assertions \
    -L "$root/elisp" -L "$out" \
    -l "$root/tests/elisp/emrs-tests.el" \
    -f ert-run-tests-batch-and-exit
