#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"

BIND="127.0.0.1:7878"
SERVER_THREADS="$(nproc)"
DURATION="30"
CONNECTIONS="256"
PIPELINE="8"
PAYLOAD="256"

usage() {
    echo "Usage: $0 [--bind ADDR] [--server-threads N] [--connections N] [--pipeline N] [--duration-secs S] [--payload-bytes N]"
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bind)           BIND="$2";           shift 2 ;;
        --server-threads) SERVER_THREADS="$2"; shift 2 ;;
        --connections)    CONNECTIONS="$2";    shift 2 ;;
        --pipeline)       PIPELINE="$2";       shift 2 ;;
        --duration-secs)  DURATION="$2";       shift 2 ;;
        --payload-bytes)  PAYLOAD="$2";        shift 2 ;;
        --help|-h)        usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

# Kill any stale instances
pkill -f 'monoio-server' 2>/dev/null || true
sleep 0.2

# Require pre-built artifacts; run ./build-all.sh first
if [[ ! -x artifacts/monoio-server || ! -x artifacts/nacelle-stress-test ]]; then
    echo "artifacts not found — run ./build-all.sh first"
    exit 1
fi

# Start server in background
./artifacts/monoio-server \
    --bind "$BIND" \
    --server-threads "$SERVER_THREADS" &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null' EXIT

# Wait for the port to be open (up to 5 seconds)
for i in $(seq 1 50); do
    if ss -tlpn "sport = :${BIND##*:}" 2>/dev/null | grep -q LISTEN; then
        break
    fi
    sleep 0.1
done

echo "--- monoio  threads=$SERVER_THREADS  connections=$CONNECTIONS  pipeline=$PIPELINE ---"

./artifacts/nacelle-stress-test \
    --addr "$BIND" \
    --connections "$CONNECTIONS" \
    --pipeline "$PIPELINE" \
    --duration-secs "$DURATION" \
    --payload-bytes "$PAYLOAD"
