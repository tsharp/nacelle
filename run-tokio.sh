#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"

BIND="127.0.0.1:7878"
SERVER_THREADS="$(nproc)"
DURATION="30"
CONNECTIONS="256"
PIPELINE="8"
PAYLOAD="256"
CONFIG_PATH="config.toml"

config_bool_value() {
    local config_path="$1"
    local key="$2"
    if [[ ! -f "$config_path" ]]; then
        return
    fi

    awk '
        BEGIN {
            key = ARGV[1]
            ARGV[1] = ""
        }
        {
            sub(/[[:space:]]*#.*/, "", $0)
            pattern = "^[[:space:]]*" key "[[:space:]]*="
            if ($0 ~ pattern) {
                value = $0
                sub(/^[^=]*=/, "", value)
                gsub(/[[:space:]]/, "", value)
                print tolower(value)
                exit
            }
        }
    ' "$key" "$config_path"
}

effective_tls_self_signed() {
    local value
    value="$(config_bool_value "config.toml" "tls_self_signed")"
    if [[ -z "$value" ]]; then
        value="false"
    fi

    if [[ "$CONFIG_PATH" != "config.toml" && "$CONFIG_PATH" != "./config.toml" ]]; then
        local override
        override="$(config_bool_value "$CONFIG_PATH" "tls_self_signed")"
        if [[ -n "$override" ]]; then
            value="$override"
        fi
    fi

    echo "$value"
}

usage() {
    echo "Usage: $0 [--config PATH] [--bind ADDR] [--server-threads N] [--connections N] [--pipeline N] [--duration-secs S] [--payload-bytes N]"
    echo "Uses ./config.toml by default; client TLS mode follows the effective tls_self_signed value."
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
        --config)         CONFIG_PATH="$2";    shift 2 ;;
        --help|-h)        usage ;;
        *) echo "Unknown option: $1"; usage ;;
    esac
done

# Kill any stale instances
pkill -f 'tokio-server' 2>/dev/null || true
sleep 0.2

# Require pre-built artifacts; run ./build-all.sh first
if [[ ! -x artifacts/tokio-server || ! -x artifacts/nacelle-stress-test ]]; then
    echo "artifacts not found — run ./build-all.sh first"
    exit 1
fi

SERVER_ARGS=(
    --bind "$BIND"
    --server-threads "$SERVER_THREADS"
)

if [[ "$CONFIG_PATH" != "config.toml" && "$CONFIG_PATH" != "./config.toml" ]]; then
    SERVER_ARGS+=(--config "$CONFIG_PATH")
fi

# Start server in background
./artifacts/tokio-server "${SERVER_ARGS[@]}" &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null' EXIT

# Wait for the port to be open (up to 5 seconds)
for i in $(seq 1 50); do
    if ss -tlpn "sport = :${BIND##*:}" 2>/dev/null | grep -q LISTEN; then
        break
    fi
    sleep 0.1
done

echo "--- tokio  threads=$SERVER_THREADS  connections=$CONNECTIONS  pipeline=$PIPELINE ---"

CLIENT_ARGS=(
    --addr "$BIND"
    --connections "$CONNECTIONS"
    --pipeline "$PIPELINE"
    --duration-secs "$DURATION"
    --payload-bytes "$PAYLOAD"
)

if [[ "$(effective_tls_self_signed)" == "true" ]]; then
    CLIENT_ARGS+=(--tls-insecure)
fi

./artifacts/nacelle-stress-test "${CLIENT_ARGS[@]}"
