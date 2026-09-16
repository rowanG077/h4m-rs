#!/usr/bin/env sh
# nix develop supplies the pinned reference binaries from the Nix store.
set -eu
cd "$(dirname "$0")/.."
: "${H4M_REFERENCE:?Run this script inside nix develop}"
: "${H4M_REFERENCE_PLANES:?Run this script inside nix develop}"
exec cargo test --locked --offline --test equivalence "$@" -- --ignored --nocapture
