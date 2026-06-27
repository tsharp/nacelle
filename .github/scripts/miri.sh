#!/bin/bash
set -e

rustup component add miri
cargo miri setup

export MIRIFLAGS="-Zmiri-strict-provenance"

cargo miri test

# run with wrapping integer overflow instead of panic
cargo miri test --release