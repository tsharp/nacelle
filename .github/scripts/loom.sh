#!/bin/bash
set -e

RUSTFLAGS="--cfg loom -Dwarnings" cargo test --lib