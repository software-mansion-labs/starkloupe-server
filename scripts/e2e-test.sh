#!/usr/bin/env bash
#
# End-to-end test for the simulation pipeline.
#
# Boots the server against the services in local/e2e-docker-compose.yaml, replays
# two historical mainnet transactions through /v1/simulate-transaction, and checks
# the results. This exercises the whole path — RPC fork state, blockifier/cheatnet
# execution, trace collection, JSON serialisation — which unit tests do not cover.
#
# Usage:
#   docker compose -f local/e2e-docker-compose.yaml up -d --wait   # or: make e2e-deps
#   E2E_RPC_URL=https://... ./scripts/e2e-test.sh
#
# E2E_RPC_URL must serve JSON-RPC spec 0.10. Older specs omit the block header
# commitment fields that starknet-rust requires; because MaybePreConfirmedBlockWithTxs
# is #[serde(untagged)], the parse failure surfaces as the misleading error
# "Pre-confirmed block is not allowed at the configuration level".
set -euo pipefail

cd "$(dirname "$0")/.."

: "${E2E_RPC_URL:?set E2E_RPC_URL to a Starknet mainnet RPC serving spec 0.10}"

PORT="${E2E_PORT:-3000}"
BASE_URL="http://localhost:${PORT}"
WORK_DIR="$(mktemp -d)"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

# The Universal Sierra Compiler is required to run simulations.
if [ ! -x ./universal-sierra-compiler ] && ! command -v universal-sierra-compiler >/dev/null 2>&1; then
  echo "==> installing universal-sierra-compiler"
  sh scripts/install-usc.sh
fi

echo "==> building server"
cargo build --bin server

# Points at local/e2e-docker-compose.yaml. The RPC URLs are only validated at
# startup; each request carries the endpoint it should actually use.
export DATABASE_URL="postgres://postgres:postgres@localhost:1234/walnut"
export SQLX_OFFLINE=true
export STARKNET_MAINNET_RPC_URL="$E2E_RPC_URL"
export STARKNET_SEPOLIA_RPC_URL="$E2E_RPC_URL"
export ETHEREUM_MAINNET_RPC_URL="${E2E_ETHEREUM_RPC_URL:-https://eth.llamarpc.com}"
export ETHEREUM_SEPOLIA_RPC_URL="${E2E_ETHEREUM_RPC_URL:-https://eth.llamarpc.com}"
export S3_ENDPOINT="http://localhost:9010"
export S3_REGION=us-east-1
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export CLASSES_S3_BUCKET_NAME=walnut-classes
export BINARIES_S3_BUCKET_NAME=walnut-binaries
export BINARIES_SAVE_DIRECTORY_PATH=./binaries
export UNIVERSAL_SIERRA_COMPILER="${UNIVERSAL_SIERRA_COMPILER:-./universal-sierra-compiler}"
export WALNUT_ADMIN_TOKEN=e2e-local-token
export LOG_LEVEL=INFO

echo "==> starting server on :${PORT}"
./target/debug/server > "$WORK_DIR/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 60); do
  if curl -sf -m 5 -o /dev/null "$BASE_URL/health" 2>/dev/null; then break; fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "server exited during startup:" >&2
    tail -30 "$WORK_DIR/server.log" >&2
    exit 1
  fi
  sleep 2
done

if ! curl -sf -m 5 -o /dev/null "$BASE_URL/health"; then
  echo "server did not become healthy:" >&2
  tail -30 "$WORK_DIR/server.log" >&2
  exit 1
fi
echo "==> server healthy"

# Historical mainnet transactions, replayed at their original block. Both are
# finalised, so the expectations below are stable over time.
#
# Only structural facts are asserted. Gas figures, step counts and the estimated
# fee move with the blockifier version, and l1_data_flamechart node ordering plus
# event-to-call attribution vary between processes (HashMap iteration order), so
# none of them belong in an assertion.
simulate() {
  local name="$1" tx="$2" expected="$3" calls="$4" failed="$5"
  local out="$WORK_DIR/${name}.json"

  echo "==> simulating ${name} (${tx})"
  local code
  code=$(curl -s -m 600 -o "$out" -w '%{http_code}' \
    -X POST "$BASE_URL/v1/simulate-transaction" \
    -H 'content-type: application/json' \
    -d "{\"WithTxHash\":{\"rpc_url\":\"${E2E_RPC_URL}\",\"tx_hash\":\"${tx}\"}}")

  if [ "$code" != "200" ]; then
    echo "FAIL ${name}: expected HTTP 200, got ${code}" >&2
    head -c 500 "$out" >&2
    echo >&2
    return 1
  fi

  E2E_NAME="$name" E2E_EXPECTED="$expected" E2E_CALLS="$calls" E2E_FAILED="$failed" \
    python3 scripts/check_simulation.py "$out"
}

failures=0

# A successful multicall: 5 contract calls, none failed.
simulate success \
  0x645b8e535eaeda98fda9d4471ee696cfcc40363d4a2d7e1111231a9f491394b \
  SUCCEEDED 5 0 || failures=$((failures + 1))

# A revert: the panic data must survive decoding into a readable reason, and the
# failed calls must be reported as such. This is the path that regressed most
# easily when the cheatnet trace types changed.
simulate revert \
  0x25c73dadcec6b6850d2c3278d165aa83b9b0b0a6c7ac365b8e2c19d3f123b87 \
  'REVERTED:LIMIT_DIRECTION' 28 8 || failures=$((failures + 1))

if [ "$failures" -ne 0 ]; then
  echo "==> ${failures} e2e check(s) failed" >&2
  exit 1
fi

echo "==> all e2e checks passed"
