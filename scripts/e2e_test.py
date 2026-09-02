#!/usr/bin/env python3
"""End-to-end test for the simulation pipeline.

Boots the server against the services in local/e2e-docker-compose.yaml, replays
two historical mainnet transactions through /v1/simulate-transaction, and checks
the results. This exercises the whole path - RPC fork state, blockifier/cheatnet
execution, trace collection, JSON serialisation - which unit tests do not cover.

Everything the test needs - the compiler, the server binary, the backing
services - is prepared beforehand; this script only runs the checks.

Usage:
    sh scripts/install-usc.sh                                    # once
    cargo build --bin server
    docker compose -f local/e2e-docker-compose.yaml up -d --wait postgres minio
    docker compose -f local/e2e-docker-compose.yaml run --rm createbuckets
    E2E_RPC_URL=https://... ./scripts/e2e_test.py

E2E_SERVER_BIN overrides the binary under test; it defaults to the debug build
at target/debug/server.

E2E_RPC_URL must serve JSON-RPC spec 0.10. Older specs omit the block header
commitment fields that starknet-rust requires; because MaybePreConfirmedBlockWithTxs
is #[serde(untagged)], the parse failure surfaces as the misleading error
"Pre-confirmed block is not allowed at the configuration level".
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Historical mainnet transactions, replayed at their original block. Both are
# finalised, so the expectations below are stable over time.
#
# Only structural facts are asserted. Gas figures, step counts and the estimated
# fee move with the blockifier version, and l1_data_flamechart node ordering plus
# event-to-call attribution vary between processes (dict iteration order), so
# none of them belong in an assertion.
CASES = [
    # A successful multicall: 5 contract calls, none failed.
    {
        "name": "success",
        "tx": "0x645b8e535eaeda98fda9d4471ee696cfcc40363d4a2d7e1111231a9f491394b",
        "status": "SUCCEEDED",
        "revert_reason": None,
        "calls": 5,
        "failed": 0,
    },
    # A revert: the panic data must survive decoding into a readable reason, and
    # the failed calls must be reported as such. This is the path that regressed
    # most easily when the cheatnet trace types changed.
    {
        "name": "revert",
        "tx": "0x25c73dadcec6b6850d2c3278d165aa83b9b0b0a6c7ac365b8e2c19d3f123b87",
        "status": "REVERTED",
        "revert_reason": "LIMIT_DIRECTION",
        "calls": 28,
        "failed": 8,
    },
]


class CheckFailed(Exception):
    """One expectation about a simulation response did not hold."""


def log(message):
    print(f"==> {message}", flush=True)


def tail(path, lines=30):
    try:
        return "".join(path.read_text(errors="replace").splitlines(keepends=True)[-lines:])
    except OSError as error:
        return f"(could not read {path}: {error})"


def find_executable(candidate):
    """Resolve `candidate` the way the server will spawn it, and return that
    exact path - relative to the repo root (the server's cwd) or via PATH.
    Returning the resolved form keeps what we validate and what we hand over the
    same thing; a bare name sitting in the repo root but not on PATH would
    otherwise pass the check and then fail to spawn."""
    local = REPO_ROOT / candidate
    if local.is_file() and os.access(local, os.X_OK):
        return str(local)
    return shutil.which(candidate)


def resolve_usc():
    """Locate the Universal Sierra Compiler the server needs to run simulations.

    Installing it is not this script's job - CI does it in its own step, and
    locally `sh scripts/install-usc.sh` drops the binary in the repo root. We
    only check it is there, so a missing compiler fails fast with a clear
    message instead of a server that boots and then cannot compile anything.
    """
    configured = os.environ.get("UNIVERSAL_SIERRA_COMPILER")
    if configured:
        resolved = find_executable(configured)
        if resolved:
            return resolved
        sys.exit(f"UNIVERSAL_SIERRA_COMPILER={configured} is not an executable")

    resolved = find_executable("universal-sierra-compiler")
    if resolved:
        return resolved

    sys.exit(
        "universal-sierra-compiler not found - install it first with "
        "`sh scripts/install-usc.sh` (CI does this in a dedicated step)"
    )


def resolve_server():
    """Locate the server binary to run, without building it.

    Compiling is a separate CI step so a build failure is reported as one, and
    so the e2e timing does not swallow a cold cargo build.
    """
    configured = os.environ.get("E2E_SERVER_BIN")
    binary = Path(configured) if configured else REPO_ROOT / "target" / "debug" / "server"
    if not binary.is_absolute():
        binary = REPO_ROOT / binary
    if not os.access(binary, os.X_OK):
        sys.exit(f"{binary} is not an executable - build it first with `cargo build --bin server`")
    return binary


def server_env(rpc_url, usc):
    """Points at local/e2e-docker-compose.yaml. The RPC URLs are only validated
    at startup; each request carries the endpoint it should actually use."""
    ethereum_rpc = os.environ.get("E2E_ETHEREUM_RPC_URL") or "https://eth.llamarpc.com"
    return {
        **os.environ,
        "DATABASE_URL": "postgres://postgres:postgres@localhost:1234/walnut",
        "SQLX_OFFLINE": "true",
        "STARKNET_MAINNET_RPC_URL": rpc_url,
        "STARKNET_SEPOLIA_RPC_URL": rpc_url,
        "ETHEREUM_MAINNET_RPC_URL": ethereum_rpc,
        "ETHEREUM_SEPOLIA_RPC_URL": ethereum_rpc,
        "S3_ENDPOINT": "http://localhost:9010",
        "S3_REGION": "us-east-1",
        "AWS_ACCESS_KEY_ID": "minioadmin",
        "AWS_SECRET_ACCESS_KEY": "minioadmin",
        "CLASSES_S3_BUCKET_NAME": "walnut-classes",
        "BINARIES_S3_BUCKET_NAME": "walnut-binaries",
        "BINARIES_SAVE_DIRECTORY_PATH": "./binaries",
        "UNIVERSAL_SIERRA_COMPILER": usc,
        "WALNUT_ADMIN_TOKEN": "e2e-local-token",
        "LOG_LEVEL": "INFO",
    }


def wait_for_health(base_url, process, log_path, attempts=60, interval=2):
    for _ in range(attempts):
        try:
            with urllib.request.urlopen(f"{base_url}/health", timeout=5) as response:
                if response.status == 200:
                    return
        except (urllib.error.URLError, OSError):
            pass
        if process.poll() is not None:
            raise RuntimeError(f"server exited during startup:\n{tail(log_path)}")
        time.sleep(interval)
    raise RuntimeError(f"server did not become healthy:\n{tail(log_path)}")


def simulate(base_url, rpc_url, tx_hash):
    """POST one transaction hash and return the decoded response body."""
    body = json.dumps({"WithTxHash": {"rpc_url": rpc_url, "tx_hash": tx_hash}}).encode()
    # skip_tracking suppresses the Slack/Grafana notification, whose message
    # embeds the RPC URL - API key and all.
    request = urllib.request.Request(
        f"{base_url}/v1/simulate-transaction?skip_tracking=true",
        data=body,
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=600) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        detail = error.read()[:500].decode(errors="replace")
        raise CheckFailed(f"expected HTTP 200, got {error.code}: {detail}") from None
    except (OSError, ValueError) as error:
        raise CheckFailed(f"request failed: {error}") from None


def check(case, payload):
    """Assert the structural facts of one simulation response."""
    l2 = payload.get("l2_transaction_data")
    if not l2:
        raise CheckFailed(f"no l2_transaction_data in response: {json.dumps(payload)[:300]}")

    result = l2.get("simulation_result")
    if not result:
        raise CheckFailed(f"no simulation_result in response: {json.dumps(payload)[:300]}")

    execution = result.get("execution_result") or {}
    status = execution.get("execution_status")
    if status != case["status"]:
        raise CheckFailed(f"expected {case['status']}, got {status}")

    if case["revert_reason"] is not None:
        reason = execution.get("revert_reason")
        if reason != case["revert_reason"]:
            raise CheckFailed(
                f"expected revert_reason {case['revert_reason']!r}, got {reason!r}"
            )

    calls = result.get("contract_calls_map") or {}
    if len(calls) != case["calls"]:
        raise CheckFailed(f"expected {case['calls']} contract calls, got {len(calls)}")

    failed = [key for key, call in calls.items() if call.get("is_failed")]
    if len(failed) != case["failed"]:
        raise CheckFailed(
            f"expected {case['failed']} failed calls, got {len(failed)}: {sorted(failed)}"
        )

    # Every call must carry a serialised result. A call that failed has to report
    # an Err, and one that succeeded an Ok - this is what catches a regression in
    # the cheatnet trace-result types or their Serialize impls.
    for call_id, call in calls.items():
        if "result" not in call:
            raise CheckFailed(f"call {call_id} has no result field")
        keys = set(call["result"]) if isinstance(call["result"], dict) else set()
        if call.get("is_failed") and "Err" not in keys:
            raise CheckFailed(
                f"call {call_id} is_failed but result is {json.dumps(call['result'])[:80]}"
            )
        if not call.get("is_failed") and "Ok" not in keys:
            raise CheckFailed(
                f"call {call_id} succeeded but result is {json.dumps(call['result'])[:80]}"
            )

    # A revert must decode its panic data into readable text rather than leaking
    # raw felts to the caller.
    if case["status"] == "REVERTED":
        recoverable = [
            call
            for call in calls.values()
            if isinstance(call.get("result"), dict)
            and "Recoverable" in (call["result"].get("Err") or {})
        ]
        if not recoverable:
            raise CheckFailed("expected at least one Recoverable failure among the calls")

    print(f"  ok {case['name']}: {status}, {len(calls)} calls, {len(failed)} failed", flush=True)


def run_cases(base_url, rpc_url):
    failures = 0
    for case in CASES:
        log(f"simulating {case['name']} ({case['tx']})")
        try:
            check(case, simulate(base_url, rpc_url, case["tx"]))
        except CheckFailed as error:
            print(f"FAIL {case['name']}: {error}", file=sys.stderr, flush=True)
            failures += 1
    return failures


def main():
    rpc_url = os.environ.get("E2E_RPC_URL")
    if not rpc_url:
        sys.exit("set E2E_RPC_URL to a Starknet mainnet RPC serving spec 0.10")

    base_url = "http://localhost:3000"

    usc = resolve_usc()
    server_bin = resolve_server()

    work_dir = Path(tempfile.mkdtemp())
    log_path = work_dir / "server.log"
    process = None
    try:
        log(f"starting server at {base_url}")
        with log_path.open("wb") as log_file:
            process = subprocess.Popen(
                [str(server_bin)],
                cwd=REPO_ROOT,
                env=server_env(rpc_url, usc),
                stdout=log_file,
                stderr=subprocess.STDOUT,
            )
            wait_for_health(base_url, process, log_path)
            log("server healthy")
            failures = run_cases(base_url, rpc_url)
            if failures:
                print(f"--- server log ---\n{tail(log_path, 100)}", file=sys.stderr, flush=True)
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        shutil.rmtree(work_dir, ignore_errors=True)

    if failures:
        sys.exit(f"==> {failures} e2e check(s) failed")
    log("all e2e checks passed")


if __name__ == "__main__":
    main()
