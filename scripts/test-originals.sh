#!/usr/bin/env sh
# Compare every .h4m recursively, including copies on different discs.
set -eu
cd "$(dirname "$0")/.."
: "${H4M_ORIGINALS:?Set H4M_ORIGINALS to the root of your extracted movies}"
: "${H4M_REFERENCE_PLANES:?Run this script inside nix develop}"
exec cargo test --locked --offline --release --test originals "$@" -- --ignored --nocapture
