#!/usr/bin/env bash
# Build the Tokio stress server and stress-test client in release mode, then
# copy the resulting binaries into ./artifacts/.
set -e
cd "$(dirname "$0")"

ARTIFACTS="$(pwd)/artifacts"
mkdir -p "$ARTIFACTS"

echo "==> Building nacelle-stress-test"
cargo build --release --package nacelle-stress-test
cp target/release/nacelle-stress-test "$ARTIFACTS/"
echo "    copied nacelle-stress-test"

echo ""
echo "==> Building tokio-server"
cargo build --release --package nacelle-stress-server --bin tokio-server
cp target/release/tokio-server "$ARTIFACTS/"
echo "    copied tokio-server"

echo ""
echo "==> Artifacts written to $ARTIFACTS/"
ls -lh "$ARTIFACTS/"
