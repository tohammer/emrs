#!/bin/sh
# Build the example modules, install them under the names Emacs expects,
# and run their smoke tests.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build -p echo-rs -p grep-rs
out="$root/target/elisp"
mkdir -p "$out"
for name in echo grep; do
    for ext in so dylib dll; do
        for built in "target/debug/lib${name}_rs.$ext" "target/debug/${name}_rs.$ext"; do
            if [ -f "$built" ]; then
                cp "$built" "$out/$name-rs-module.$ext"
            fi
        done
    done
done
exec "${EMACS:-emacs}" --batch --module-assertions \
    -L "$root/elisp" -L "$out" \
    -l "$root/examples/examples-test.el" \
    -f ert-run-tests-batch-and-exit
