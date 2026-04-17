#!/usr/bin/env bash
# Build all nacelle stress-server variants and the stress-test client in
# release mode, then copy the resulting binaries into ./artifacts/.
set -e
cd "$(dirname "$0")"

ARTIFACTS="$(pwd)/artifacts"
mkdir -p "$ARTIFACTS"

# ── Feature flags for each server binary ─────────────────────────────────────
declare -A FEATURES=(
    [tokio-server]="tokio-runtime"
    [monoio-server]="monoio-runtime"
    [compio-iouring-server]="compio-iouring-runtime"
    [compio-epoll-server]="compio-epoll-runtime"
    #[compio-iocp-server]="compio-iocp-runtime"
    [kimojio-iouring-server]="kimojio-iouring-runtime"
    [kimojio-epoll-server]="kimojio-epoll-runtime"
    #[kimojio-iocp-server]="kimojio-iocp-runtime"
)

echo "==> Building nacelle-stress-test"
cargo build --release --package nacelle-stress-test
cp target/release/nacelle-stress-test "$ARTIFACTS/"
echo "    copied nacelle-stress-test"

echo ""
echo "==> Building server variants"
for BIN in "${!FEATURES[@]}"; do
    FEAT="${FEATURES[$BIN]}"
    echo "    building $BIN  (--features $FEAT)"
    cargo build --release \
        --package nacelle-stress-server \
        --bin "$BIN" \
        --no-default-features \
        --features "$FEAT"
    cp "target/release/$BIN" "$ARTIFACTS/"
    echo "    copied $BIN"
done

echo ""
echo "==> Artifacts written to $ARTIFACTS/"
ls -lh "$ARTIFACTS/"
